//! DynDOLOD-style object LOD atlas planning (docs/re/object-lod-spec.md 1.2 and 7.2) for
//! `lodforge build --mode dyndolod`: which texture keys of the LOD models the export names go
//! on which page, and where.
//!
//! Observed in the golden `LODGen_SSE_ObjectAtlasMap_Tamriel.txt` (700 rows): keys are
//! `diffuse,normal` for the big pages (`DynDOLOD_<World>.dds` 8192 x 4096, `_Alpha.dds` for
//! see-through textures and billboards, `_Glow.dds` with `diffuse,normal,glow` keys), an `=N`
//! suffix for alpha-tested shapes with a threshold other than 128 (N = threshold / 16), one-part
//! keys for TexGen's LOD-texture replacements (`textures\lod\<name>lod.dds` pages) and ten-column
//! rows for the stitched mini atlases. Tiles are the texture's size capped at
//! `ObjectLODMaxTileSize` for LOD textures and `FullTextureMaxTileSize` for full ones; pages are
//! packed with xEdit's packer ([`lodforge_objlod::atlas_build`]) into square bins. PBR
//! `<stem>linear.dds` names (AltTextures) are keyed as `<stem>.dds`. Shapes whose UVs leave
//! `[-tolerance, 1 + tolerance]` are not atlasable (LODGen asks for a tiled mini atlas instead,
//! spec 7.4) and keep their texture; a texture no shape can use is left off the pages.

use std::collections::{BTreeMap, HashMap, HashSet};

use rayon::prelude::*;

use lodforge_objlod::atlas_build::{AtlasEntry, AtlasLayout};
use lodforge_objlod::atlas_map::AtlasMap;
use lodforge_objlod::export::texture_key;
use lodforge_objlod::material::SLSF2_GLOW_MAP;
use lodforge_objlod::mesh::{self, ShaderSource};
use lodforge_objlod::sidefiles::AltTextures;
use lodforge_texgen::stitch::MiniAtlasRow;

use crate::dyndolod_build::{AtlasSettings, WorldExport, has_transparency, ignored_texture, is_lod_texture, square_layout, tile_size};
use crate::profile::Profile;

pub struct PlanInput<'a> {
    pub settings: AtlasSettings,
    /// `AtlasTolerance` of the export (0.2).
    pub tolerance: f32,
    /// `IgnoreTexture` masks plus the HD LOD textures: never atlased.
    pub ignore_masks: &'a [String],
    /// TexGen mini-atlas rows (written through as ten-column rows; their sources leave the pages).
    pub mini: &'a [MiniAtlasRow],
    /// Billboard textures (trees, grass views) that go on the `_Alpha` page as one-part keys.
    pub billboards: &'a [String],
    /// `textures\DynDOLOD\LOD\DynDOLOD_<World>` (page names get `.dds`, `_Alpha.dds`, `_Glow.dds`).
    pub base: String,
}

#[derive(Debug, Default)]
pub struct PlanStats {
    pub models: usize,
    pub keys: usize,
    pub main: usize,
    pub alpha: usize,
    pub glow: usize,
    pub mini: usize,
    pub skipped_tiled: usize,
    pub skipped_missing: usize,
    pub skipped_ignored: usize,
}

struct KeyInfo {
    diffuse: String,
    normal: String,
    glow: String,
    alpha: Option<u8>,
    usable: bool,
}

fn strip_linear(t: String) -> String {
    match t.strip_suffix("linear.dds") {
        Some(stem) => format!("{stem}.dds"),
        None => t,
    }
}

/// The atlas-map key of a shape's texture set (`d,n[,g][=N]`) and its parts.
pub fn shape_key(textures: &[String], glow_map: bool, alpha: Option<(u16, u8)>) -> Option<(String, KeyParts)> {
    let d = strip_linear(texture_key(textures.first().map(String::as_str).unwrap_or("")));
    if d.is_empty() {
        return None;
    }
    let n = match textures.get(1).map(|t| texture_key(t)) {
        Some(n) if !n.is_empty() => n,
        _ => format!("{}_n.dds", d.trim_end_matches(".dds")),
    };
    let g = if glow_map { textures.get(2).map(|t| texture_key(t)).unwrap_or_default() } else { String::new() };
    let suffix = AtlasMap::alpha_suffix(alpha);
    let mut key = format!("{d},{n}");
    if !g.is_empty() {
        key.push(',');
        key.push_str(&g);
    }
    if let Some(a) = suffix {
        key.push_str(&format!("={a}"));
    }
    Some((key, KeyParts { diffuse: d, normal: n, glow: g, alpha: suffix }))
}

pub struct KeyParts {
    pub diffuse: String,
    pub normal: String,
    pub glow: String,
    pub alpha: Option<u8>,
}

/// Plans the worldspace's atlas from the LOD models of every season's export lines.
pub fn plan(p: &Profile, exports: &WorldExport, input: &PlanInput<'_>) -> (AtlasLayout, PlanStats) {
    let mut stats = PlanStats::default();
    // The models (lower-case paths) and the AltTextures of every season.
    let mut models: HashSet<String> = HashSet::new();
    for s in &exports.seasons {
        for l in &s.lines {
            for c in &l.columns {
                let m = c.to_ascii_lowercase().replace('/', "\\");
                if m.ends_with(".nif") {
                    models.insert(m);
                }
            }
        }
    }
    let alts: Vec<AltTextures> = exports.seasons.iter().map(|s| AltTextures::parse(&s.alt_textures)).collect();
    let model_list: Vec<String> = models.into_iter().collect();
    stats.models = model_list.len();
    // Per model: the shapes' texture slots, glow flag, alpha, index, name, tolerance fit.
    struct Shape {
        index: usize,
        name: String,
        textures: [String; 9],
        glow_map: bool,
        alpha: Option<(u16, u8)>,
        fits: bool,
    }
    let loaded: HashMap<String, Vec<Shape>> = model_list
        .par_iter()
        .filter_map(|m| {
            let bytes = p.vfs.read(m).ok().flatten()?;
            let mesh = mesh::import(m, &bytes).ok()?;
            let shapes = mesh
                .parts
                .iter()
                .map(|part| Shape {
                    index: part.index,
                    name: part.name.clone(),
                    textures: part.textures.clone(),
                    glow_map: matches!(&part.shader, ShaderSource::Lighting(l) if l.flags2 & SLSF2_GLOW_MAP != 0),
                    alpha: part.alpha,
                    fits: !lodforge_objlod::reuv::needs_recut(&part.uvs, input.tolerance),
                })
                .collect();
            Some((m.clone(), shapes))
        })
        .collect();
    // Keys from every (reference, model, shape), AltTextures applied.
    let mut keys: BTreeMap<String, KeyInfo> = BTreeMap::new();
    for (s, alt) in exports.seasons.iter().zip(&alts) {
        for l in &s.lines {
            let ref_id = format!("{:08X}", l.ref_id);
            for c in &l.columns {
                let m = c.to_ascii_lowercase().replace('/', "\\");
                let Some(shapes) = loaded.get(&m) else { continue };
                for sh in shapes {
                    let textures: Vec<String> = match alt.textures_for(&ref_id, sh.index, &sh.name) {
                        Some(t) => t.to_vec(),
                        None => sh.textures.to_vec(),
                    };
                    let Some((key, parts)) = shape_key(&textures, sh.glow_map, sh.alpha) else { continue };
                    let e = keys.entry(key).or_insert(KeyInfo { diffuse: parts.diffuse, normal: parts.normal, glow: parts.glow, alpha: parts.alpha, usable: false });
                    e.usable |= sh.fits;
                }
            }
        }
    }
    stats.keys = keys.len();
    let mini_sources: HashMap<String, String> = input.mini.iter().map(|r| (texture_key(&r.source), texture_key(&r.dest))).collect();
    // A mini page used by some shape gets a row of its own onto the big page (two-step remap).
    let mut chained: BTreeMap<String, KeyInfo> = BTreeMap::new();
    for k in keys.values() {
        if !k.usable {
            continue;
        }
        if let Some(dest) = mini_sources.get(&k.diffuse) {
            let stem = dest.trim_end_matches(".dds");
            chained.entry(format!("{dest},{stem}_n.dds")).or_insert(KeyInfo { diffuse: dest.clone(), normal: format!("{stem}_n.dds"), glow: String::new(), alpha: None, usable: true });
        }
    }
    keys.extend(chained);
    // Texture sizes and transparency, read once per diffuse.
    let diffuses: Vec<String> = keys.values().map(|k| k.diffuse.clone()).chain(input.billboards.iter().map(|b| texture_key(b))).collect::<HashSet<_>>().into_iter().collect();
    let info: HashMap<String, (u32, u32, bool)> = diffuses
        .par_iter()
        .filter_map(|d| {
            let bytes = p.vfs.read(d).ok().flatten()?;
            let i = lodforge_dds::parse(&bytes).ok()?;
            let transparent = lodforge_dds::decode(&bytes, &i, 0).map(|img| has_transparency(&img.rgba)).unwrap_or(false);
            Some((d.clone(), (i.width, i.height, transparent)))
        })
        .collect();
    let settings = &input.settings;
    let cap = |t: &str| if is_lod_texture(t) { settings.max_tile } else { settings.full_max_tile };
    let mut main: Vec<(String, u32, u32)> = Vec::new();
    let mut alpha: Vec<(String, u32, u32)> = Vec::new();
    let mut glow: Vec<(String, u32, u32)> = Vec::new();
    for (key, k) in &keys {
        if !k.usable {
            stats.skipped_tiled += 1;
            continue;
        }
        if ignored_texture(&k.diffuse, input.ignore_masks) {
            stats.skipped_ignored += 1;
            continue;
        }
        if mini_sources.contains_key(&k.diffuse) {
            continue;
        }
        let Some(&(w, h, transparent)) = info.get(&k.diffuse) else {
            stats.skipped_missing += 1;
            continue;
        };
        let (tw, th) = tile_size(w, h, cap(&k.diffuse));
        if !k.glow.is_empty() {
            glow.push((key.clone(), tw, th));
        } else if settings.separate_alpha() && (transparent || k.alpha.is_some()) {
            alpha.push((key.clone(), tw, th));
        } else {
            main.push((key.clone(), tw, th));
        }
        let _ = &k.normal;
    }
    for b in input.billboards {
        let d = texture_key(b);
        if let Some(&(w, h, _)) = info.get(&d) {
            let (tw, th) = tile_size(w, h, cap(&d));
            // A billboard with a normal map is keyed `d,n` as the golden grass map rows are
            // (`..._1.dds,..._1_n.dds` on the `_Alpha` page); the reader accepts both.
            let n = format!("{}_n.dds", d.trim_end_matches(".dds"));
            let key = if p.vfs.exists(&n) { format!("{d},{n}") } else { d };
            alpha.push((key, tw, th));
        } else {
            stats.skipped_missing += 1;
        }
    }
    alpha.sort();
    alpha.dedup();
    stats.main = main.len();
    stats.alpha = alpha.len();
    stats.glow = glow.len();
    let mut layout = square_layout(&main, settings.max_atlas, &input.base);
    let alpha_layout = square_layout(&alpha, settings.max_atlas, &format!("{}_Alpha", input.base));
    layout = crate::glow_atlas::merge(layout, alpha_layout);
    let glow_layout = crate::glow_atlas::glow_layout(&glow.iter().map(|(k, _, _)| k.clone()).collect::<Vec<_>>(), |t| info.get(&texture_key(t)).map(|i| (i.0, i.1)), settings.max_atlas, &format!("{}_Glow", input.base));
    layout = crate::glow_atlas::merge(layout, glow_layout);
    for r in input.mini {
        layout.mini_rows.push((r.source.clone(), format!("{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}", r.source, r.w, r.h, r.x, r.y, r.dest, r.atlas_w, r.atlas_h, r.scale[0], r.scale[1])));
    }
    stats.mini = layout.mini_rows.len();
    let _ = AtlasEntry { texture: String::new(), width: 0, height: 0, x: 0, y: 0, page: 0 };
    (layout, stats)
}
