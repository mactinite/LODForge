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
    /// Far-LOD bake matching: `Some(output dir)` scales the tiles of pre-baked LOD textures that
    /// only LOD8+ models use (not written by this build) to the mean colour of the LOD4 textures
    /// of the same references, so a cliff does not darken when its `_lod_1` model with its own
    /// baked texture (e.g. `textures\lod\mtncliff04lod.dds`, from a vanilla-look LOD texture
    /// pack) replaces the `_lod_0` model showing the current full texture.
    /// The strength (0..1) moves the tile that fraction of the way from its own mean to the LOD4
    /// mean: 1 matches the texture means outright, which overshoots on screen because the LOD4
    /// shapes also carry darkening vertex colours and a different normal map; 0.5 put the far
    /// cliffs of the Throat of the World on the near ones in the viewer.
    pub match_far: Option<(&'a std::path::Path, f32)>,
}

/// A repeat tile may be up to this many times the ordinary tile cap per side (at 4 repeats and
/// more), so each repeat keeps at least a quarter of an ordinary tile's detail
/// (`FullTextureMaxTileSize` 256: 1024 px tiles, 128..256 px per repeat).
pub const REPEAT_TILE_GROWTH: u32 = 2;

#[derive(Debug, Default)]
pub struct PlanStats {
    pub models: usize,
    pub keys: usize,
    pub main: usize,
    pub alpha: usize,
    pub glow: usize,
    pub mini: usize,
    /// Far-LOD bakes whose tiles get an RGB factor (`PlanInput::match_far`).
    pub matched: usize,
    /// Repeat tiles for generated models (`lodforge_objlod::tiling`).
    pub repeat_tiles: usize,
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
    /// A repeat tile (`lodforge_objlod::tiling`): the texture `ru x rv` times in one tile.
    repeats: Option<(u32, u32)>,
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
        /// Mean vertex-colour luminance (0..1; 1 without colours): what the shape's diffuse
        /// is multiplied by on screen.
        vc: f32,
        /// Generated models whose UVs repeat: the repeat tile they are drawn from
        /// (`lodforge_objlod::tiling`; `(1, 1)` = the ordinary tile after a whole-repeat shift).
        repeats: Option<(u32, u32)>,
    }
    let loaded: HashMap<String, Vec<Shape>> = model_list
        .par_iter()
        .filter_map(|m| {
            let bytes = p.vfs.read(m).ok().flatten()?;
            let mesh = mesh::import(m, &bytes).ok()?;
            let generated = lodforge_objlod::tiling::is_generated_model(m);
            let shapes = mesh
                .parts
                .iter()
                .map(|part| Shape {
                    repeats: (generated && lodforge_objlod::reuv::needs_recut(&part.uvs, lodforge_objlod::build::ATLAS_UV_SLACK)).then(|| lodforge_objlod::tiling::tile_repeats(&part.uvs)).flatten(),
                    index: part.index,
                    name: part.name.clone(),
                    textures: part.textures.clone(),
                    glow_map: matches!(&part.shader, ShaderSource::Lighting(l) if l.flags2 & SLSF2_GLOW_MAP != 0),
                    alpha: part.alpha,
                    fits: !lodforge_objlod::reuv::needs_recut(&part.uvs, input.tolerance),
                    vc: part.colors.as_ref().filter(|c| !c.is_empty()).map_or(1.0, |c| c.iter().map(|p| 0.2126 * f32::from(p[0]) + 0.7152 * f32::from(p[1]) + 0.0722 * f32::from(p[2])).sum::<f32>() / (255.0 * c.len() as f32)),
                })
                .collect();
            Some((m.clone(), shapes))
        })
        .collect();
    // Keys from every (reference, model, shape), AltTextures applied.
    let mut keys: BTreeMap<String, KeyInfo> = BTreeMap::new();
    // Repeat tiles of generated models: per base key, the most repeats per axis.
    let mut repeat_max: BTreeMap<String, (KeyInfo, (u32, u32))> = BTreeMap::new();
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
                    // Generated models with repeating UVs: a repeat tile (glow shapes keep theirs).
                    if let Some(r) = sh.repeats.filter(|_| parts.glow.is_empty()) {
                        if r == (1, 1) {
                            // Inside one repeat once shifted: the ordinary tile.
                            keys.entry(key).or_insert(KeyInfo { diffuse: parts.diffuse, normal: parts.normal, glow: String::new(), alpha: parts.alpha, usable: false, repeats: None }).usable = true;
                        } else {
                            // One repeat tile per texture, holding the most repeats any shape needs.
                            let e = repeat_max.entry(key).or_insert((KeyInfo { diffuse: parts.diffuse, normal: parts.normal, glow: String::new(), alpha: parts.alpha, usable: true, repeats: None }, (1, 1)));
                            e.1 = (e.1.0.max(r.0), e.1.1.max(r.1));
                        }
                        continue;
                    }
                    let e = keys.entry(key).or_insert(KeyInfo { diffuse: parts.diffuse, normal: parts.normal, glow: parts.glow, alpha: parts.alpha, usable: false, repeats: None });
                    e.usable |= sh.fits;
                }
            }
        }
    }
    for (key, (mut k, r)) in repeat_max {
        k.repeats = Some(r);
        keys.insert(lodforge_objlod::tiling::tiled_key(&key, r), k);
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
            chained.entry(format!("{dest},{stem}_n.dds")).or_insert(KeyInfo { diffuse: dest.clone(), normal: format!("{stem}_n.dds"), glow: String::new(), alpha: None, usable: true, repeats: None });
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
        // (A repeat tile is drawn from the full texture even when TexGen stitched it into a
        // mini atlas for the installed LOD models.)
        if mini_sources.contains_key(&k.diffuse) && k.repeats.is_none() {
            continue;
        }
        let Some(&(w, h, transparent)) = info.get(&k.diffuse) else {
            stats.skipped_missing += 1;
            continue;
        };
        let (tw, th) = match k.repeats {
            // The texture ru x rv times; the tile may grow up to REPEAT_TILE_GROWTH times the cap
            // so a repeat keeps some detail.
            Some((ru, rv)) => {
                stats.repeat_tiles += 1;
                tile_size(w * ru, h * rv, cap(&k.diffuse) * ru.max(rv).min(REPEAT_TILE_GROWTH))
            }
            None => tile_size(w, h, cap(&k.diffuse)),
        };
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
    if let Some((out, strength)) = input.match_far {
        layout.tones = far_lod_tones(p, exports, &alts, |m| loaded.get(m).map(|shapes| shapes.iter().map(|s| (s.index, s.name.clone(), s.textures.clone(), s.vc)).collect()), out, strength);
        stats.matched = layout.tones.len();
    }
    let _ = AtlasEntry { texture: String::new(), width: 0, height: 0, x: 0, y: 0, page: 0 };
    (layout, stats)
}

/// RGB factors for the diffuses that only LOD8+ models of some references use and that this
/// build did not write (pre-baked LOD textures from the load order): the mean colour of the
/// LOD4 diffuses of the same references over the bake's own mean, per channel, clamped to
/// 0.5..2. Means are alpha-weighted (alpha >= 128) over a small mip, in the stored (sRGB)
/// encoding, so the ratio is what the LOD shader sees. A `...linear.dds` LOD4 diffuse that is not
/// in the VFS yet is taken as its source times the PBR scale.
fn far_lod_tones(p: &Profile, exports: &WorldExport, alts: &[AltTextures], shapes_of: impl Fn(&str) -> Option<Vec<(usize, String, [String; 9], f32)>>, out: &std::path::Path, strength: f32) -> BTreeMap<String, [f32; 3]> {
    let mut lod4_used: HashSet<String> = HashSet::new();
    // far diffuse -> (LOD4 diffuse -> (references, sum of their vertex-colour factors))
    let mut pairs: HashMap<String, HashMap<String, (usize, f32)>> = HashMap::new();
    let diffuses = |alt: &AltTextures, ref_id: &str, model: &str| -> Vec<(String, f32)> {
        let m = model.to_ascii_lowercase().replace('/', "\\");
        let Some(shapes) = shapes_of(&m) else { return Vec::new() };
        shapes
            .iter()
            .filter_map(|(index, name, textures, vc)| {
                let t = alt.textures_for(ref_id, *index, name).map(|t| t[0].clone()).unwrap_or_else(|| textures[0].clone());
                (!t.is_empty()).then(|| (texture_key(&t), *vc))
            })
            .collect()
    };
    for (s, alt) in exports.seasons.iter().zip(alts) {
        for l in &s.lines {
            let ref_id = format!("{:08X}", l.ref_id);
            let d4 = diffuses(alt, &ref_id, &l.columns[0]);
            lod4_used.extend(d4.iter().map(|(d, _)| d.clone()));
            if d4.is_empty() {
                continue;
            }
            let mut far: HashSet<String> = HashSet::new();
            for c in &l.columns[1..] {
                far.extend(diffuses(alt, &ref_id, c).into_iter().map(|(d, _)| d));
            }
            for f in far {
                if d4.iter().any(|(d, _)| *d == f) {
                    continue;
                }
                let e = pairs.entry(f).or_default();
                for (d, vc) in &d4 {
                    let v = e.entry(d.clone()).or_insert((0, 0.0));
                    v.0 += 1;
                    v.1 += vc;
                }
            }
        }
    }
    let ours = |d: &str| p.vfs.loose_path(d).is_some_and(|path| path.starts_with(out));
    let candidates: Vec<(String, HashMap<String, (usize, f32)>)> = pairs.into_iter().filter(|(f, _)| !lod4_used.contains(f) && !ours(f)).collect();
    let means: HashMap<String, [f32; 3]> = candidates
        .iter()
        .flat_map(|(f, d4)| std::iter::once(f.clone()).chain(d4.keys().cloned()))
        .collect::<HashSet<_>>()
        .into_par_iter()
        .filter_map(|t| mean_color(p, &t).map(|m| (t, m)))
        .collect();
    let mut tones = BTreeMap::new();
    for (f, d4) in candidates {
        let Some(mf) = means.get(&f) else { continue };
        // Target: the LOD4 texture means (the vertex colours are recorded for the record; the
        // strength stands in for what they and the LOD4 shading take off on screen).
        let (mut sum, mut n) = ([0f32; 3], 0usize);
        for (d, (count, _vc_sum)) in &d4 {
            if let Some(m) = means.get(d) {
                for c in 0..3 {
                    sum[c] += m[c] * *count as f32;
                }
                n += count;
            }
        }
        if n == 0 {
            continue;
        }
        let tone: [f32; 3] = std::array::from_fn(|c| if mf[c] > 1.0 { 1.0 + ((sum[c] / n as f32 / mf[c]).clamp(0.5, 2.0) - 1.0) * strength } else { 1.0 });
        if tone.iter().any(|t| (t - 1.0).abs() > 0.02) {
            tones.insert(f, tone);
        }
    }
    tones
}

/// Alpha-weighted mean RGB (0..255, as stored) of a texture over its smallest mip of at least
/// 64 px; `None` when unreadable.
fn mean_color(p: &Profile, path: &str) -> Option<[f32; 3]> {
    let mut scale = 1.0f32;
    let bytes = match p.vfs.read(path).ok().flatten() {
        Some(b) => b,
        None => {
            // A linear copy this build writes later: its source times the PBR scale.
            let src = path.strip_suffix("linear.dds").map(|s| format!("{s}.dds"))?;
            scale = crate::pbr_linear::SCALE;
            p.vfs.read(&src).ok().flatten()?
        }
    };
    let info = lodforge_dds::parse(&bytes).ok()?;
    let mut level = 0u32;
    while level + 1 < info.mip_count && (info.width >> (level + 1)).max(1) >= 64 {
        level += 1;
    }
    let img = lodforge_dds::decode(&bytes, &info, level).ok()?;
    let (mut sum, mut n) = ([0f64; 3], 0f64);
    for px in img.rgba.chunks_exact(4) {
        if px[3] < 128 {
            continue;
        }
        for c in 0..3 {
            sum[c] += f64::from(px[c]);
        }
        n += 1.0;
    }
    if n == 0.0 {
        for px in img.rgba.chunks_exact(4) {
            for c in 0..3 {
                sum[c] += f64::from(px[c]);
            }
            n += 1.0;
        }
    }
    (n > 0.0).then(|| std::array::from_fn(|c| (sum[c] / n) as f32 * scale))
}
