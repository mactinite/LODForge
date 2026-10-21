//! The two side files DynDOLOD writes next to each object LOD export (observed format facts in
//! docs/re/object-lod-spec.md sections 7.3 and 7.4):
//!
//! - `LODGen_SSE_VC_<World>[_SEASON].txt`: `fullmodel<TAB>shape<TAB>True|False` for every shape of
//!   every full model in the export. `False` (all vertex colours white) means the LOD shape with
//!   that name is written with whitened vertex colours.
//! - `LODGen_SSE_AltTextures_<World>[_SEASON].txt`: per reference, the texture swaps the object LOD
//!   builder applies to the shapes of its full and LOD models, `refID<TAB>numGeom=shape=tex0,...,tex9...`.
//!   A LOD shape takes the textures of the full model's shape with the same name (after the
//!   base's MODS alternate textures), so a LOD model made for vanilla textures shows the
//!   textures the reference shows up close (PBR, retextures, alternate texture sets).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use lodforge_esp::{LoadOrder, Sig, fields, winning_records};
use lodforge_nif::{Block, Nif};

use crate::bases::{Base, model_path};
use crate::export::RefLine;

/// One shape of a model, numbered in the model traversal order the side files use.
#[derive(Debug, Clone, Default)]
pub struct ShapeInfo {
    /// `numGeom`: the shape's position in the depth-first walk of the model's node tree (children in
    /// order, shapes counted as they come); -1 for shapes under hidden nodes or editor markers, which
    /// the golden side files never list (object-lod-spec 7.3).
    pub index: i32,
    pub name: String,
    /// Texture set slots as stored (lighting shaders), or the effect shader's source texture.
    pub textures: Vec<String>,
    /// Has vertex colours and at least one is not white.
    pub coloured: bool,
    /// Has an `NiAlphaProperty`.
    pub alpha: bool,
}

/// Every shape below the root nodes in the side files' traversal order (children in order, shapes
/// counted as they come, hidden nodes and editor markers not counted), as verified against the
/// golden side files and NIF trees (object-lod-spec 7.3).
pub fn walk_shapes(nif: &Nif) -> Vec<ShapeInfo> {
    fn walk(nif: &Nif, idx: usize, skipped: bool, n: &mut i32, out: &mut Vec<ShapeInfo>) {
        let Some(Block::Node(node)) = nif.blocks.get(idx) else { return };
        let skipped = skipped || node.av.flags & 1 != 0 || node.av.net.name.as_deref().is_some_and(|s| s.to_ascii_lowercase().contains("editormarker"));
        for c in node.children.iter().flatten() {
            let ci = *c as usize;
            match nif.blocks.get(ci) {
                Some(Block::Node(_)) => walk(nif, ci, skipped, n, out),
                Some(Block::TriShape(s)) => {
                    if !skipped {
                        *n += 1;
                    }
                    out.push(ShapeInfo {
                        index: if skipped { -1 } else { *n },
                        name: s.av.net.name.clone().unwrap_or_default(),
                        textures: nif.shape_textures(s),
                        // DynDOLOD checks for white only in SSE vertex data; LE (NiTriShapeData)
                        // colours always count.
                        coloured: s.colors.iter().any(|c| s.legacy || c[0] != 255 || c[1] != 255 || c[2] != 255),
                        alpha: matches!(nif.get(s.alpha_property), Some(Block::Alpha { .. })),
                    });
                }
                _ => {}
            }
        }
    }
    // From the footer's root nodes (usually block 0; some files start with an extra data block).
    let mut out = Vec::new();
    let mut n = -1;
    for r in nif.roots.iter().flatten() {
        walk(nif, *r as usize, false, &mut n, &mut out);
    }
    out
}

/// Texture paths as DynDOLOD writes them: lower case, backslashes, `textures\` prefix.
pub fn texture_path(t: &str) -> String {
    let t = t.trim().replace('/', "\\").to_ascii_lowercase();
    let t = t.trim_start_matches('\\');
    if t.is_empty() {
        return String::new();
    }
    if let Some(p) = t.find("textures\\") { t[p..].to_string() } else { format!("textures\\{t}") }
}

/// TXST field `TXnn` -> NIF texture set slot (TX02 environment mask is slot 5, TX03 glow slot 2,
/// TX04 height slot 3, TX05 environment slot 4).
const TXST_SLOT: [usize; 8] = [0, 1, 5, 2, 3, 4, 6, 7];

/// The winning TXST records' textures, in NIF texture set slot order.
pub fn read_texture_sets(lo: &LoadOrder) -> lodforge_esp::Result<HashMap<u32, Vec<String>>> {
    let mut out = HashMap::new();
    for (id, (_, r)) in winning_records(lo, Sig(*b"TXST"))? {
        let data = r.data()?;
        let mut t = vec![String::new(); 8];
        for f in fields(&data) {
            if f.sig.0[..2] == *b"TX"
                && let Some(i) = std::str::from_utf8(&f.sig.0[2..]).ok().and_then(|s| s.parse::<usize>().ok())
                && i < 8
            {
                t[TXST_SLOT[i]] = f.zstring();
            }
        }
        out.insert(id, t);
    }
    Ok(out)
}

/// What the side files need from the load order and the data folder.
pub struct Sources<'a> {
    pub bases: &'a HashMap<u32, Base>,
    pub texture_sets: &'a HashMap<u32, Vec<String>>,
    pub read: &'a (dyn Fn(&str) -> Option<Vec<u8>> + Sync),
    pub exists: &'a (dyn Fn(&str) -> bool + Sync),
    /// PBR materials (`--materials pbr`, Community Shaders with PBR): PBR diffuses become their
    /// `...linear.dds` copies and LOD textures take TexGen's `pbr_lod` variants. Off
    /// (`--materials complex`), no swap names a `textures\pbr\` path, a `linear.dds` copy or a
    /// `pbr_lod` texture: texture sets and full-model textures under `textures\pbr\` are ignored.
    pub pbr: bool,
}

impl Sources<'_> {
    /// Whether a texture set may be swapped in: always with PBR materials, otherwise only when
    /// no slot is under `textures\pbr\`.
    fn usable(&self, t: &[String]) -> bool {
        self.pbr || !t.iter().any(|x| is_pbr(x))
    }
}

/// Shapes per model path, shared by every season of a worldspace.
#[derive(Default)]
pub struct ModelCache(Mutex<HashMap<String, Option<Arc<Vec<ShapeInfo>>>>>);

impl ModelCache {
    pub fn shapes(&self, read: &(dyn Fn(&str) -> Option<Vec<u8>> + Sync), model: &str) -> Option<Arc<Vec<ShapeInfo>>> {
        let key = model_path(model);
        if let Some(s) = self.0.lock().unwrap().get(&key) {
            return s.clone();
        }
        let s = read(&key).and_then(|b| lodforge_nif::parse(&b).ok()).map(|n| Arc::new(walk_shapes(&n)));
        self.0.lock().unwrap().insert(key, s.clone());
        s
    }
}

/// `LODGen_SSE_VC_*.txt`: every shape of every full model, in export order (a child worldspace
/// copy's line names its LOD model as the full model, and so does this file).
pub fn vc_text(lines: &[RefLine], cache: &ModelCache, src: &Sources<'_>) -> String {
    let mut seen = HashSet::new();
    let mut out = String::new();
    for l in lines {
        if !seen.insert(l.full_model.to_ascii_lowercase()) {
            continue;
        }
        let Some(shapes) = cache.shapes(src.read, &l.full_model) else { continue };
        let mut names = HashSet::new();
        for s in shapes.iter().filter(|s| names.insert(s.name.to_ascii_lowercase())) {
            out.push_str(&format!("{}\t{}\t{}\r\n", l.full_model, s.name, if s.coloured { "True" } else { "False" }));
        }
    }
    out
}

/// A PBR diffuse as the `...linear.dds` copy LOD uses (see [`linear_texture_name`]).
pub fn is_pbr(t: &str) -> bool {
    texture_path(t).starts_with("textures\\pbr\\")
}

/// `textures\pbr\x\y.dds` -> `textures\pbr\x\ylinear.dds`.
pub fn linear_texture_name(t: &str) -> String {
    let t = texture_path(t);
    match t.strip_suffix(".dds") {
        Some(s) => format!("{s}linear.dds"),
        None => t,
    }
}

/// `textures\lod\dirt02lod.dds` -> `textures\lod\dirt02pbr_lod.dds`,
/// `mrkbuildingslod02.dds` -> `mrkbuildingspbr_lod02.dds` (TexGen's PBR LOD textures).
pub fn pbr_lod_name(t: &str) -> Option<String> {
    let t = texture_path(t);
    let s = t.strip_suffix(".dds")?;
    let (dir, file) = s.rsplit_once('\\')?;
    if file.contains("pbr_lod") {
        return None;
    }
    let i = file.rfind("lod")?;
    let head = &file[..i];
    let head = head.strip_suffix('_').unwrap_or(head);
    Some(format!("{dir}\\{head}pbr_{}.dds", &file[i..]))
}

/// The ten texture fields of an entry for source textures `t`: a full model's shape gets the
/// whole set but the height map (slot 3), a LOD model's shape only diffuse and normal. A PBR
/// diffuse with an `_rmaos` map becomes its `...linear.dds` copy (DynDOLOD's `AddTextureSet`
/// trigger).
fn entry_textures(t: &[String], whole_set: bool, src: &Sources<'_>) -> Vec<String> {
    let mut out: Vec<String> = (0..10).map(|i| if i < 2 || (whole_set && i != 3) { t.get(i).map(|s| texture_path(s)).unwrap_or_default() } else { String::new() }).collect();
    if src.pbr && is_pbr(&out[0]) && out[0].strip_suffix(".dds").is_some_and(|s| (src.exists)(&format!("{s}_rmaos.dds"))) {
        out[0] = linear_texture_name(&out[0]);
    }
    out
}

fn same_texture(a: &str, b: &str) -> bool {
    texture_path(a) == texture_path(b)
}

/// Whether a texture's format has an alpha channel (not BC1, BC4, BC5, BGRX, BGR or L8).
fn has_alpha(src: &Sources<'_>, t: &str) -> bool {
    use lodforge_dds::Format;
    (src.read)(&texture_path(t)).and_then(|b| lodforge_dds::parse(&b).ok()).is_some_and(|i| !matches!(i.format, Format::Bc1 | Format::Bc4 | Format::Bc5 | Format::Bgrx8 | Format::Bgr8 | Format::L8))
}

/// The textures a LOD model's shapes take for one base and full model, as
/// `(numGeom, shape, ten texture fields)`: the full model's textures for shapes with the same
/// name (the base's MODS alternate texture first), diffuse and normal only; TexGen's PBR variant
/// for LOD textures of a PBR model. Glow overlays (`*glow_lod*`) keep their own textures.
/// `lod_texture_mods`: MODS also applies to LOD-textured shapes (DynDOLOD.esm base copies do;
/// the AltTextures leave them to the full model's MODS entry with the same key).
fn lod_entries(base: Option<&Base>, full: Option<&Arc<Vec<ShapeInfo>>>, model: &str, lod_texture_mods: bool, cache: &ModelCache, src: &Sources<'_>) -> Vec<(i32, String, Vec<String>)> {
    let mut out = Vec::new();
    if model.to_ascii_lowercase().contains("glow_lod") {
        return out;
    }
    let Some(shapes) = cache.shapes(src.read, model) else { return out };
    let full_pbr = src.pbr && full.is_some_and(|f| f.iter().any(|s| s.textures.first().is_some_and(|t| is_pbr(t))));
    let alt = |name: &str| -> Option<&Vec<String>> {
        let a = base?.alt_textures.iter().find(|a| a.shape.eq_ignore_ascii_case(name))?;
        src.texture_sets.get(&a.txst).filter(|t| src.usable(t))
    };
    for s in shapes.iter().filter(|s| s.index >= 0) {
        let own = s.textures.first().map(String::as_str).unwrap_or("");
        let lod_texture = texture_path(own).contains("\\lod\\");
        let tex = if let Some(t) = alt(&s.name).filter(|t| (lod_texture_mods || !lod_texture) && !t.is_empty() && !same_texture(&t[0], own)) {
            let mut e = entry_textures(t, false, src);
            // Where the LOD shape's texture is not the full shape's (PBR aside: `dirtcliffs03_lod_0`
            // `fieldgrass02` vs PBR `fieldgrass02` keeps `snow01`), a replacement diffuse with
            // alpha on a LOD model without any NiAlphaProperty becomes TexGen's `<name>noalpha.dds`
            // copy (DynDOLOD: "Replacement texture ... has alpha while LOD model ... has no
            // AlphaProperty, using ...noalpha.dds"; golden Tamriel 00062D9F
            // `L2_RootsandRocks:1 - L1_Rock:1` -> `snow01noalphalinear.dds`; `mountaintrim01_lod_0`,
            // which has an alpha-tested shape, keeps `snow01` on `MountainTrim01:4`, and so does
            // `mountaintrim02_lod_0`, whose shape already shows `snow01`).
            let full_own = full.and_then(|f| f.iter().find(|f| f.name.eq_ignore_ascii_case(&s.name))).and_then(|f| f.textures.first());
            let noalpha = texture_path(&t[0]).strip_suffix(".dds").map(|x| format!("{x}noalpha.dds")).unwrap_or_default();
            let plain = |t: &str| texture_path(t).replacen("textures\\pbr\\", "textures\\", 1);
            if !shapes.iter().any(|x| x.alpha) && full_own.is_some_and(|f| !same_texture(&plain(f), own)) && !same_texture(&plain(&t[0]), own) &&!noalpha.is_empty() && (src.exists)(&noalpha) && has_alpha(src, &t[0]) {
                e[0] = if e[0].ends_with("linear.dds") { linear_texture_name(&noalpha) } else { noalpha };
            }
            Some(e)
        } else if texture_path(own).contains("\\lod\\") {
            pbr_lod_name(own).filter(|p| full_pbr && (src.exists)(p)).map(|p| {
                let mut v = vec![String::new(); 10];
                v[1] = p.replace(".dds", "_n.dds");
                v[0] = p;
                v
            })
        } else {
            let source = full.and_then(|f| f.iter().find(|f| f.name.eq_ignore_ascii_case(&s.name))).map(|f| &f.textures);
            source.filter(|t| !t.is_empty() && !same_texture(&t[0], own) && src.usable(t)).map(|t| entry_textures(t, false, src))
        };
        if let Some(t) = tex {
            out.push((s.index, s.name.clone(), t));
        }
    }
    out
}

/// One mini atlas DynDOLOD makes itself (`AddMiniAtlas`): a LOD model's shape uses the full
/// texture `base`, which has a TexGen mini atlas, and the full model's shape of the same name
/// shows `texture` (with `normal`) instead (PBR, retexture). DynDOLOD stitches `texture` into a
/// copy of `base`'s mini atlas (`MiniAtlasExport`) and LODGen re-UVs the shape onto it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MiniAtlasRequest {
    /// The LOD shape's own diffuse, `textures\...`.
    pub base: String,
    /// The full model's diffuse (a PBR `...linear.dds` read as the `.dds` it was made from).
    pub texture: String,
    pub normal: String,
}

/// The [`MiniAtlasRequest`]s of the LOD models of `lines`, each (base, texture) once, in export
/// order. Which of them become mini atlases is up to the caller (bases without a TexGen mini
/// atlas, textures with one of their own and existing destinations are left out).
pub fn mini_atlas_requests(lines: &[RefLine], cache: &ModelCache, src: &Sources<'_>) -> Vec<MiniAtlasRequest> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut done = HashSet::new();
    for l in lines {
        if crate::childworld::is_copy(l.ref_id) || !done.insert((l.base, l.full_model.to_ascii_lowercase(), l.columns.join("|").to_ascii_lowercase())) {
            continue;
        }
        let full = cache.shapes(src.read, &l.full_model);
        let base = src.bases.get(&l.base);
        for m in l.columns.iter().filter(|c| c.to_ascii_lowercase().ends_with(".nif")) {
            let Some(shapes) = cache.shapes(src.read, m) else { continue };
            for (index, name, t) in lod_entries(base, full.as_ref(), m, false, cache, src) {
                let Some(own) = shapes.iter().find(|s| s.index == index && s.name == name).and_then(|s| s.textures.first()).map(|t| texture_path(t)) else { continue };
                if own.is_empty() || own.contains("\\lod\\") {
                    continue;
                }
                let texture = if is_pbr(&t[0]) { t[0].strip_suffix("linear.dds").map(|s| format!("{s}.dds")).unwrap_or_else(|| t[0].clone()) } else { t[0].clone() };
                // The replacement as it was before the `noalpha` swap (golden: no mini atlas for
                // `snow01noalpha`; `pbr\landscape\snow01` has TexGen's `snow01pbr_lod`).
                let texture = texture.strip_suffix("noalpha.dds").map(|s| format!("{s}.dds")).filter(|s| (src.exists)(s)).unwrap_or(texture);
                if texture.is_empty() || same_texture(&texture, &own) || !seen.insert((own.clone(), texture.clone())) {
                    continue;
                }
                out.push(MiniAtlasRequest { base: own, texture, normal: t[1].clone() });
            }
        }
    }
    out
}

/// One texture swap on a shape of a dynamic LOD base copy's model (DynDOLOD.esm MODS +
/// `<texture>_DynDOLOD_TXST`, `CreatePBRBaseRecord`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextureSwap {
    pub index: i32,
    pub shape: String,
    /// Diffuse and normal, `textures\...` paths.
    pub diffuse: String,
    pub normal: String,
}

/// The swaps a copy of `base` showing `model` needs: the LOD-model entries of the AltTextures,
/// deduplicated by shape.
pub fn lod_model_swaps(base: &Base, model: &str, cache: &ModelCache, src: &Sources<'_>) -> Vec<TextureSwap> {
    let full = cache.shapes(src.read, &crate::export::export_model_text(&base.model));
    let mut seen = HashSet::new();
    lod_entries(Some(base), full.as_ref(), model, true, cache, src)
        .into_iter()
        .filter(|(i, n, _)| seen.insert((*i, n.to_ascii_lowercase())))
        .map(|(index, shape, t)| TextureSwap { index, shape, diffuse: t[0].clone(), normal: t[1].clone() })
        .collect()
}

/// The AltTextures line of one reference (all its export lines), or `None` without swaps.
pub fn alt_textures_line(ref_lines: &[&RefLine], base: Option<&Base>, cache: &ModelCache, src: &Sources<'_>) -> Option<String> {
    let first = ref_lines.first()?;
    // Child worldspace copies get none (DynDOLOD's copies have no line).
    if crate::childworld::is_copy(first.ref_id) {
        return None;
    }
    let (ref_id, full_model) = (first.ref_id, &first.full_model);
    let full = cache.shapes(src.read, full_model);
    let mut models: Vec<&str> = Vec::new();
    for l in ref_lines {
        for c in &l.columns {
            if !c.is_empty() && c.to_ascii_lowercase().ends_with(".nif") && !models.iter().any(|m| m.eq_ignore_ascii_case(c)) {
                models.push(c);
            }
        }
    }
    let entry = |index: i32, name: &str, t: &[String]| format!("{index}={name}={}", t.iter().map(|x| format!("{x},")).collect::<String>());
    // The full model: every MODS entry (last first) as stored, with the 3D index and name of the
    // original mesh (which need not match a PBR-patched copy of it) and the whole texture set.
    let mods: Vec<String> = base
        .map(|b| b.alt_textures.as_slice())
        .unwrap_or_default()
        .iter()
        .rev()
        .filter_map(|a| src.texture_sets.get(&a.txst).filter(|t| src.usable(t)).map(|t| entry(a.index, &a.shape, &entry_textures(t, true, src))))
        .collect();
    let lods = models.iter().flat_map(|m| lod_entries(base, full.as_ref(), m, false, cache, src)).map(|(i, n, t)| entry(i, &n, &t));
    // The same key may come twice with different textures (full and LOD model); LODGen uses the
    // first.
    let mut seen = HashSet::new();
    // MODS first matches 63% of DynDOLOD's lines exactly; its order varies between references
    // with the same models (Tamriel 00014272 / 00017368), so the rest is not reproducible.
    let entries: Vec<String> = mods.into_iter().chain(lods).filter(|e| seen.insert(e.to_ascii_lowercase())).collect();
    (!entries.is_empty()).then(|| format!("{ref_id:08X}\t{}", entries.join("\t")))
}

/// `LODGen_SSE_AltTextures_*.txt` for one season's reference lines.
pub fn alt_textures_text(lines: &[RefLine], cache: &ModelCache, src: &Sources<'_>) -> String {
    use rayon::prelude::*;
    let mut order: Vec<u32> = Vec::new();
    let mut by_ref: HashMap<u32, Vec<&RefLine>> = HashMap::new();
    for l in lines {
        let e = by_ref.entry(l.ref_id).or_default();
        if e.is_empty() {
            order.push(l.ref_id);
        }
        e.push(l);
    }
    let out: Vec<Option<String>> = order.par_iter().map(|id| alt_textures_line(&by_ref[id], src.bases.get(&by_ref[id][0].base), cache, src)).collect();
    out.into_iter().flatten().map(|l| l + "\r\n").collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bases::AltTexture;

    #[test]
    fn names() {
        assert_eq!(linear_texture_name("Textures\\PBR\\Landscape\\Snow01.dds"), "textures\\pbr\\landscape\\snow01linear.dds");
        assert_eq!(pbr_lod_name("textures\\lod\\Dirt02LOD.dds").as_deref(), Some("textures\\lod\\dirt02pbr_lod.dds"));
        assert_eq!(pbr_lod_name("textures\\lod\\bridge01_lod.dds").as_deref(), Some("textures\\lod\\bridge01pbr_lod.dds"));
        assert_eq!(pbr_lod_name("textures\\lod\\mrkbuildingslod02.dds").as_deref(), Some("textures\\lod\\mrkbuildingspbr_lod02.dds"));
        assert_eq!(pbr_lod_name("textures\\lod\\dirt02pbr_lod.dds"), None);
        assert_eq!(texture_path("landscape/rocks01.dds"), "textures\\landscape\\rocks01.dds");
    }

    #[test]
    fn mods_entries() {
        // Two entries: "RockSkirt" index 1 -> TXST 0xAE1, "A:4" index 2 -> 0x57DCE (local ids).
        let mut d = 2u32.to_le_bytes().to_vec();
        for (name, txst, index) in [("RockSkirt", 0xAE1u32, 1u32), ("A:4", 0x57DCE, 2)] {
            d.extend((name.len() as u32).to_le_bytes());
            d.extend(name.as_bytes());
            d.extend(txst.to_le_bytes());
            d.extend(index.to_le_bytes());
        }
        let m = crate::bases::parse_mods(&d, |t| Some(t | 0x0100_0000));
        assert_eq!(m, vec![AltTexture { shape: "RockSkirt".into(), txst: 0x0100_0AE1, index: 1 }, AltTexture { shape: "A:4".into(), txst: 0x0105_7DCE, index: 2 }]);
    }

    fn shape(index: i32, name: &str, textures: &[&str], coloured: bool) -> ShapeInfo {
        ShapeInfo { index, name: name.into(), textures: textures.iter().map(|t| t.to_string()).collect(), coloured, alpha: false }
    }

    #[test]
    fn side_files_of_one_reference() {
        let cache = ModelCache::default();
        let rocks = ["textures\\pbr\\landscape\\rocks01.dds", "textures\\pbr\\landscape\\rocks01_n.dds", "", "textures\\pbr\\landscape\\rocks01_p.dds", "", "textures\\pbr\\landscape\\rocks01_rmaos.dds"];
        let dirt = ["textures\\landscape\\dirt02.dds", "textures\\landscape\\dirt02_n.dds"];
        let insert = |m: &str, s: Vec<ShapeInfo>| cache.0.lock().unwrap().insert(model_path(m), Some(Arc::new(s)));
        insert("Meshes\\rock.nif", vec![shape(0, "Rock:0", &rocks, false), shape(1, "Skirt", &dirt, true)]);
        insert(
            "meshes\\lod\\rock_lod_0.nif",
            vec![
                shape(0, "Rock:0", &["textures\\landscape\\rocks01.dds", "textures\\landscape\\rocks01_n.dds"], false),
                shape(1, "Skirt", &dirt, false),
                shape(2, "Rock_LOD:2", &["textures\\lod\\rocklod.dds", "textures\\lod\\rocklod_n.dds"], false),
            ],
        );
        // The skirt has an alternate texture set (MODS); texture sets are in NIF slot order.
        let base = Base { alt_textures: vec![AltTexture { shape: "Skirt".into(), txst: 7, index: 1 }], ..Default::default() };
        let mut set = vec![String::new(); 8];
        for (i, t) in [(0, "riverbededge.dds"), (1, "riverbededge_n.dds"), (3, "riverbededge_p.dds"), (5, "riverbededge_rmaos.dds")] {
            set[i] = format!("textures\\pbr\\landscape\\{t}");
        }
        let texture_sets = HashMap::from([(7, set)]);
        let bases = HashMap::from([(0x800, base)]);
        let read = |_: &str| None;
        let exists = |p: &str| p == "textures\\lod\\rockpbr_lod.dds" || p.ends_with("_rmaos.dds");
        let src = Sources { bases: &bases, texture_sets: &texture_sets, read: &read, exists: &exists, pbr: true };
        let line = RefLine { ref_id: 0x1234, full_model: "Meshes\\rock.nif".into(), columns: ["meshes\\lod\\rock_lod_0.nif".into(), String::new(), String::new(), String::new()], base: 0x800, ..Default::default() };
        assert_eq!(vc_text(std::slice::from_ref(&line), &cache, &src), "Meshes\\rock.nif\tRock:0\tFalse\r\nMeshes\\rock.nif\tSkirt\tTrue\r\n");
        let alt = alt_textures_text(std::slice::from_ref(&line), &cache, &src);
        let e: Vec<&str> = alt.trim_end().split('\t').collect();
        assert_eq!(e[0], "00001234");
        // Full model: the MODS entry with the whole set but the height map.
        assert_eq!(e[1], "1=Skirt=textures\\pbr\\landscape\\riverbededgelinear.dds,textures\\pbr\\landscape\\riverbededge_n.dds,,,,textures\\pbr\\landscape\\riverbededge_rmaos.dds,,,,,");
        // LOD model: the full model's textures (diffuse and normal only), MODS for the skirt,
        // TexGen's PBR LOD texture for the LOD-textured shape.
        assert_eq!(e[2], "0=Rock:0=textures\\pbr\\landscape\\rocks01linear.dds,textures\\pbr\\landscape\\rocks01_n.dds,,,,,,,,,");
        assert_eq!(e[3], "1=Skirt=textures\\pbr\\landscape\\riverbededgelinear.dds,textures\\pbr\\landscape\\riverbededge_n.dds,,,,,,,,,");
        assert_eq!(e[4], "2=Rock_LOD:2=textures\\lod\\rockpbr_lod.dds,textures\\lod\\rockpbr_lod_n.dds,,,,,,,,,");
        assert_eq!(e.len(), 5);
        // `--materials complex`: the PBR texture set, the PBR full-model textures and the
        // `pbr_lod` variant are all ignored; the VC file does not change.
        let plain = Sources { pbr: false, ..src };
        assert_eq!(vc_text(std::slice::from_ref(&line), &cache, &plain), "Meshes\\rock.nif\tRock:0\tFalse\r\nMeshes\\rock.nif\tSkirt\tTrue\r\n");
        assert_eq!(alt_textures_text(std::slice::from_ref(&line), &cache, &plain), "");
    }

    #[test]
    fn noalpha_swap_and_mini_atlas_requests() {
        let cache = ModelCache::default();
        let insert = |m: &str, s: Vec<ShapeInfo>| cache.0.lock().unwrap().insert(model_path(m), Some(Arc::new(s)));
        // Like Tamriel's TundraPond01Snow01: MODS puts PBR snow01 (BC7, alpha) on a shape whose LOD
        // texture (mountainslab02) is not the full one (mountainslab01); the PBR full texture
        // `cliff` has the TexGen-mini-atlas source `dirtcliffs01` on the LOD shape.
        insert("Meshes\\pond.nif", vec![shape(0, "Rock:1", &["textures\\landscape\\mountainslab01.dds"], false), shape(1, "Cliff:0", &["textures\\pbr\\landscape\\cliff.dds", "textures\\pbr\\landscape\\cliff_n.dds"], false)]);
        insert("meshes\\lod\\pond_lod_0.nif", vec![shape(0, "Rock:1", &["textures\\landscape\\mountainslab02.dds"], false), shape(1, "Cliff:0", &["textures\\landscape\\dirtcliffs01.dds"], false)]);
        let mut with_alpha = shape(2, "Trim", &["textures\\lod\\trimlod.dds"], false);
        with_alpha.alpha = true;
        insert("meshes\\lod\\pond_alpha_lod_0.nif", vec![shape(0, "Rock:1", &["textures\\landscape\\mountainslab02.dds"], false), with_alpha]);
        let base = Base { alt_textures: vec![AltTexture { shape: "Rock:1".into(), txst: 7, index: 0 }], ..Default::default() };
        let mut set = vec![String::new(); 8];
        set[0] = "textures\\pbr\\landscape\\snow01.dds".into();
        set[1] = "textures\\pbr\\landscape\\snow01_n.dds".into();
        let texture_sets = HashMap::from([(7, set)]);
        let bases = HashMap::from([(0x800, base)]);
        let bc7 = lodforge_dds::write_bc7(4, 4, &[vec![0u8; 16]]);
        let read = move |p: &str| (p == "textures\\pbr\\landscape\\snow01.dds").then(|| bc7.clone());
        let exists = |p: &str| p.ends_with("_rmaos.dds") || p.ends_with("snow01noalpha.dds") || p.ends_with("snow01.dds");
        let src = Sources { bases: &bases, texture_sets: &texture_sets, read: &read, exists: &exists, pbr: true };
        let line = |lod: &str| RefLine { ref_id: 0x1234, full_model: "Meshes\\pond.nif".into(), columns: [lod.into(), String::new(), String::new(), String::new()], base: 0x800, ..Default::default() };
        let lines = [line("meshes\\lod\\pond_lod_0.nif")];
        let alt = alt_textures_text(&lines, &cache, &src);
        assert!(alt.contains("\t0=Rock:1=textures\\pbr\\landscape\\snow01noalphalinear.dds,textures\\pbr\\landscape\\snow01_n.dds,,"), "{alt}");
        assert!(alt.contains("\t1=Cliff:0=textures\\pbr\\landscape\\clifflinear.dds,"), "{alt}");
        // A LOD model with an NiAlphaProperty keeps the replacement.
        let alt = alt_textures_text(&[line("meshes\\lod\\pond_alpha_lod_0.nif")], &cache, &src);
        assert!(alt.contains("\t0=Rock:1=textures\\pbr\\landscape\\snow01linear.dds,"), "{alt}");
        // Requests name the replacement before the noalpha swap and without `linear`.
        let q = mini_atlas_requests(&lines, &cache, &src);
        let pairs: Vec<(&str, &str)> = q.iter().map(|r| (r.base.as_str(), r.texture.as_str())).collect();
        assert_eq!(pairs, vec![("textures\\landscape\\mountainslab02.dds", "textures\\pbr\\landscape\\snow01.dds"), ("textures\\landscape\\dirtcliffs01.dds", "textures\\pbr\\landscape\\cliff.dds")]);
        assert_eq!(q[1].normal, "textures\\pbr\\landscape\\cliff_n.dds");
        // `--materials complex`: the PBR MODS set and the PBR full texture are ignored, so the
        // shape takes the full model's plain texture; no `textures\pbr\` or `linear.dds` path.
        let plain = Sources { pbr: false, ..src };
        let alt = alt_textures_text(&lines, &cache, &plain);
        assert_eq!(alt, "00001234\t0=Rock:1=textures\\landscape\\mountainslab01.dds,,,,,,,,,,\r\n");
        let q = mini_atlas_requests(&lines, &cache, &plain);
        let pairs: Vec<(&str, &str)> = q.iter().map(|r| (r.base.as_str(), r.texture.as_str())).collect();
        assert_eq!(pairs, vec![("textures\\landscape\\mountainslab02.dds", "textures\\landscape\\mountainslab01.dds")]);
        let base = &bases[&0x800];
        assert!(lod_model_swaps(base, "meshes\\lod\\pond_lod_0.nif", &cache, &plain).iter().all(|s| !is_pbr(&s.diffuse) && !is_pbr(&s.normal) && !s.diffuse.ends_with("linear.dds")));
    }
}

