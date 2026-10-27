//! The far-grass mesh pack (`SKSE\Plugins\LODForgeRuntime\GrassMeshes.bin`, docs/runtime/
//! instance-formats.md section 7a): for every grass type of the `.lfg` files in an output, three
//! pack meshes (the grass model, a simplified copy and a two-quad card), plus per-type colour
//! stats, linked from the `.lfg` files with `GMPK`.
//!
//! The simplified tier comes from LODForge's own simplifier (`lodforge_lodmodel`); the card is
//! two crossed, double-sided quads over the model's bounds showing the model's first textured
//! shape's diffuse (UV 0..1), with that shape's shader, alpha test and mean vertex colour.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};

use lodforge_instances::WriteOptions;
use lodforge_instances::grass::{GrassFile, GrassTypePack};
use lodforge_instances::meshpack::NO_MESH;
use lodforge_objlod::mesh::{LodMesh, MeshPart};

use crate::profile::{Profile, ProfileArgs};
use crate::tree_pack::{Report, Settings, make, vfs_mesh};

pub const PAGE_BASE: &str = r"textures\LODForge\GrassAtlas\GrassAtlas";
pub const FILE_NAME: &str = "GrassMeshes.bin";

/// Simplification of the decimated tier: grass is small (tens of units), so the error limit is a
/// few units and at least a third of the triangles stay.
const DECIMATE: lodforge_lodmodel::LevelParams = lodforge_lodmodel::LevelParams { max_error: 3.0, max_ratio: 0.35, min_triangles: 4, normal_weight: 0.3, uv_weight: 0.1, permissive: true };

const TIER_SUFFIX: [&str; 3] = ["", "#decimated", "#card"];

/// The decimated tier of `model` (`None` when the simplifier skips the model).
fn decimated(p: &Profile, model: &str) -> Option<LodMesh> {
    let bytes = p.vfs.read(model).ok().flatten()?;
    let (out, _) = lodforge_lodmodel::generate(&bytes, &DECIMATE, "grass").ok()?;
    lodforge_objlod::mesh::import(model, &out).ok()
}

/// One rendered card: per view (0: yaw 0, 1: yaw 90) the billboard texture and normal map
/// (Data-relative, in the card work folder) and the silhouette's size in model units.
#[derive(Clone)]
pub struct RenderedCard {
    pub views: [CardView; 2],
    /// Mean colour of the covered texels of view 0 (sRGB) and the covered share.
    pub albedo: [u8; 4],
}

#[derive(Clone)]
pub struct CardView {
    pub diffuse: String,
    pub normal: String,
    pub width: f32,
    pub height: f32,
    /// Silhouette centre offset from the origin, positive toward image left.
    pub adjust: f32,
    pub shift_z: f32,
}

/// Renders every grass model in `models` into two billboard views with the TexGen-style
/// renderer (unlit albedo, the model's vertex colours, alpha-tested coverage, normal map), written
/// as DDS under `work` and added to the VFS so the page renderer can place them.
/// Resolved diffuse paths (lower case) of the grass models' parts that are complex grass textures.
pub fn complex_diffuses(p: &Profile, models: &[String]) -> std::collections::HashSet<String> {
    let mut tx = crate::instances::TextureIndex::default();
    let mut seen: HashMap<String, bool> = HashMap::new();
    for m in models {
        let Some(lm) = vfs_mesh(p, m) else { continue };
        for part in &lm.parts {
            if part.textures[0].trim().is_empty() {
                continue;
            }
            let t = tx.resolve(p, &part.textures[0]).to_ascii_lowercase();
            if !seen.contains_key(&t) {
                let c = p.vfs.read(&t).ok().flatten().is_some_and(|b| is_complex_texture(&b));
                seen.insert(t, c);
            }
        }
    }
    seen.into_iter().filter(|(_, c)| *c).map(|(t, _)| t).collect()
}

pub fn render_cards(p: &mut Profile, models: &[String], work: &Path, complex: &std::collections::HashSet<String>) -> Result<HashMap<String, RenderedCard>> {
    use lodforge_texgen::{Light, Renderer, Scene, Settings};
    let gpu = lodforge_gpu::Gpu::new()?;
    let renderer = Renderer::new(&gpu);
    // Unlit: the runtime lights the cards with the LOD lighting material and the baked normals.
    // Grass is small (tens of units): half a unit per pixel, 32..256 px per view.
    let settings = Settings { units_per_pixel: 0.5, min_size: 32, max_size: 256, light: Light { direct: 0.0, ambient: 1.0, dir: [0.0, 0.0, 1.0], pbr_scale: 0.7, normal_pull: 1.0 }, coverage_threshold: 112 };
    let mut out = HashMap::new();
    let mut rels: Vec<String> = Vec::new();
    for model in models {
        let Some(bytes) = p.vfs.read(model).ok().flatten() else { continue };
        let Ok(nif) = lodforge_nif::parse_all_geometry(&bytes) else { continue };
        let mut scene = Scene::from_nif(&nif);
        lodforge_texgen::resolve_textures(&mut scene, |t| p.vfs.exists(t));
        // Complex grass: the model's UVs span the whole texture, the colour is its top half (CS
        // samples v * 0.5).
        for m in &mut scene.meshes {
            if complex.contains(&m.diffuse.to_ascii_lowercase()) {
                for uv in &mut m.uvs {
                    uv[1] *= 0.5;
                }
            }
        }
        // Grass vertex alpha drives wind sway, not opacity.
        for m in &mut scene.meshes {
            for c in &mut m.colors {
                c[3] = 1.0;
            }
        }
        let tex = lodforge_texgen::load_textures(&scene, |path| crate::texgen_billboards::load_texture(&gpu, &p.vfs, path));
        let Some(rb) = lodforge_texgen::make_rendered_billboard(&gpu, &renderer, &scene, &tex, 1.0, &settings, [0.0, 90.0]) else { continue };
        let stem = model.rsplit('\\').next().unwrap_or("grass").trim_end_matches(".nif").to_string();
        let crc = crc32fast::hash(model.as_bytes());
        let mut views = Vec::new();
        for (k, v) in rb.views.iter().enumerate() {
            let diffuse = format!("textures\\lodforge\\grasscards\\{stem}_{crc:08x}_{k}.dds");
            let normal = format!("textures\\lodforge\\grasscards\\{stem}_{crc:08x}_{k}_n.dds");
            for (rel, px) in [(&diffuse, &v.rgba), (&normal, &v.normal)] {
                if px.len() != (v.width_px * v.height_px * 4) as usize {
                    continue;
                }
                let path = work.join(rel.replace('\\', "/"));
                std::fs::create_dir_all(path.parent().unwrap())?;
                std::fs::write(&path, crate::texgen_billboards::bc7_dds(&gpu, v.width_px, v.height_px, px)?)?;
                rels.push(rel.clone());
            }
            views.push(CardView { diffuse, normal, width: v.width, height: v.height, adjust: v.adjust, shift_z: v.shift_z });
        }
        let v0 = &rb.views[0];
        let (mut sum, mut n) = ([0f64; 3], 0f64);
        for px in v0.rgba.chunks_exact(4).filter(|px| px[3] >= 128) {
            for k in 0..3 {
                sum[k] += f64::from(px[k]);
            }
            n += 1.0;
        }
        let total = (v0.rgba.len() / 4).max(1) as f64;
        let albedo = if n > 0.0 { [(sum[0] / n).round() as u8, (sum[1] / n).round() as u8, (sum[2] / n).round() as u8, (n / total * 255.0).round() as u8] } else { [0; 4] };
        let Ok(views) = <[CardView; 2]>::try_from(views) else { continue };
        out.insert(model.clone(), RenderedCard { views, albedo });
    }
    p.vfs.add_loose_files(work, rels.iter().map(String::as_str));
    Ok(out)
}

/// The card tier from rendered views: quad 0 in the XZ plane showing view 0, quad 1 in the YZ
/// plane showing view 1, each sized to its view's silhouette, double-sided, alpha-tested at 128,
/// upward vertex normals (the views' normal maps carry the detail).
pub fn rendered_card(full: &LodMesh, rc: &RenderedCard) -> Option<LodMesh> {
    let first = full.parts.iter().find(|p| !p.textures[0].trim().is_empty())?;
    let mut parts = Vec::new();
    for (k, v) in rc.views.iter().enumerate() {
        let hw = v.width.max(1.0) * 0.5;
        let c = -v.adjust;
        let (z0, z1) = (v.shift_z, v.shift_z + v.height.max(1.0));
        let corners = if k == 0 { [[c - hw, 0.0, z1], [c + hw, 0.0, z1], [c + hw, 0.0, z0], [c - hw, 0.0, z0]] } else { [[0.0, c - hw, z1], [0.0, c + hw, z1], [0.0, c + hw, z0], [0.0, c - hw, z0]] };
        let mut part: MeshPart = first.clone();
        part.name = format!("card{k}");
        part.textures = Default::default();
        part.textures[0] = v.diffuse.clone();
        part.textures[1] = v.normal.clone();
        part.alpha = Some((0x0200 | 0x00EC, 128));
        part.positions = corners.to_vec();
        part.uvs = vec![[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
        part.normals = vec![[0.0, 0.0, 1.0]; 4];
        part.tangents = vec![if k == 0 { [1.0, 0.0, 0.0] } else { [0.0, 1.0, 0.0] }; 4];
        part.bitangents = vec![[0.0, 0.0, -1.0]; 4];
        part.colors = None;
        part.triangles = vec![[0, 1, 2], [0, 2, 3], [0, 2, 1], [0, 3, 2]];
        parts.push(part);
    }
    Some(LodMesh { path: full.path.clone(), parts, warnings: Vec::new(), skipped: 0 })
}

/// The card tier without a rendered billboard: two crossed double-sided quads over the bounds of
/// `full` showing its first textured shape's whole diffuse.
pub fn card(full: &LodMesh) -> Option<LodMesh> {
    let first = full.parts.iter().find(|p| !p.textures[0].trim().is_empty())?;
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    let mut color = [0f64; 4];
    let mut nc = 0f64;
    for p in &full.parts {
        for v in &p.positions {
            for k in 0..3 {
                lo[k] = lo[k].min(v[k]);
                hi[k] = hi[k].max(v[k]);
            }
        }
        for c in p.colors.iter().flatten() {
            for k in 0..4 {
                color[k] += f64::from(c[k]);
            }
            nc += 1.0;
        }
    }
    if lo[0] > hi[0] || hi[2] <= lo[2] {
        return None;
    }
    let c = [(lo[0] + hi[0]) * 0.5, (lo[1] + hi[1]) * 0.5];
    let (hw, hd) = ((hi[0] - lo[0]).max(1.0) * 0.5, (hi[1] - lo[1]).max(1.0) * 0.5);
    let (z0, z1) = (lo[2], hi[2]);
    // Quad A in the XZ plane through the centre, quad B in the YZ plane; corners top-left,
    // top-right, bottom-right, bottom-left (v = 0 at the top).
    let quads = [[[c[0] - hw, c[1], z1], [c[0] + hw, c[1], z1], [c[0] + hw, c[1], z0], [c[0] - hw, c[1], z0]], [[c[0], c[1] - hd, z1], [c[0], c[1] + hd, z1], [c[0], c[1] + hd, z0], [c[0], c[1] - hd, z0]]];
    let uv = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    let mut part: MeshPart = first.clone();
    part.name = "card".into();
    part.positions = quads.iter().flatten().copied().collect();
    part.uvs = (0..2).flat_map(|_| uv).collect();
    // Upward normals: far grass reads as ground cover, lit like the terrain below it.
    part.normals = vec![[0.0, 0.0, 1.0]; 8];
    part.tangents = vec![[1.0, 0.0, 0.0]; 8];
    part.bitangents = vec![[0.0, 1.0, 0.0]; 8];
    part.colors = (nc > 0.0).then(|| vec![[0, 1, 2, 3].map(|k| (color[k] / nc).round() as u8); 8]);
    // Both windings: the card is seen from both sides.
    part.triangles = (0..2u16).flat_map(|q| {
        let b = 4 * q;
        [[b, b + 1, b + 2], [b, b + 2, b + 3], [b, b + 2, b + 1], [b, b + 3, b + 2]]
    }).collect();
    Some(LodMesh { path: full.path.clone(), parts: vec![part], warnings: Vec::new(), skipped: 0 })
}

/// Mean colour (sRGB) of the covered texels of the model's first diffuse (alpha >= 128 where the
/// shape alpha-tests) and the covered share.
fn albedo_stats(p: &Profile, full: &LodMesh, textures: &mut crate::instances::TextureIndex) -> [u8; 4] {
    let Some(part) = full.parts.iter().find(|p| !p.textures[0].trim().is_empty()) else { return [0; 4] };
    let path = textures.resolve(p, &part.textures[0]);
    let Some(img) = p.vfs.read(&path).ok().flatten().and_then(|b| {
        let info = lodforge_dds::parse(&b).ok()?;
        // A small mip is enough for a mean.
        let level = info.mip_count.saturating_sub(1).min(4);
        lodforge_dds::decode(&b, &info, level).ok()
    }) else {
        return [0; 4];
    };
    let tested = part.alpha.is_some_and(|(f, _)| f & 0x200 != 0);
    let (mut sum, mut n, mut total) = ([0f64; 3], 0f64, 0f64);
    for px in img.rgba.chunks_exact(4) {
        total += 1.0;
        if tested && px[3] < 128 {
            continue;
        }
        for k in 0..3 {
            sum[k] += f64::from(px[k]);
        }
        n += 1.0;
    }
    if n == 0.0 {
        return [0, 0, 0, 0];
    }
    [(sum[0] / n).round() as u8, (sum[1] / n).round() as u8, (sum[2] / n).round() as u8, (n / total.max(1.0) * 255.0).round() as u8]
}

/// Whether a grass diffuse is a Community Shaders "complex grass" texture: colour in the top half,
/// normal (RGB) and specular (A) in the bottom half. CS decides it the same way: the texel at
/// (0, height - 1) of the top mip, read as a normal (rgb * 2 - 1), has length 1 within
/// `ComplexGrassThreshold` (0.03).
pub fn is_complex_texture(bytes: &[u8]) -> bool {
    let Ok(info) = lodforge_dds::parse(bytes) else { return false };
    let Ok(img) = lodforge_dds::decode(bytes, &info, 0) else { return false };
    let (w, h) = (info.width as usize, info.height as usize);
    if w == 0 || h < 2 {
        return false;
    }
    let o = (h - 1) * w * 4;
    let Some(px) = img.rgba.get(o..o + 3) else { return false };
    let n = px.iter().map(|&c| f32::from(c) / 255.0 * 2.0 - 1.0);
    ((n.map(|v| v * v).sum::<f32>()).sqrt() - 1.0).abs() < 0.03
}

/// Builds `GrassMeshes.bin` from the `.lfg` files in `out` and links them (`GMPK`).
pub fn build(p: &mut Profile, out: &Path, s: &Settings, write: WriteOptions) -> Result<Report> {
    let t0 = Instant::now();
    let dir = crate::instances::out_dir(out);
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)?.flatten().map(|e| e.path()).filter(|p| p.to_string_lossy().to_ascii_lowercase().ends_with(".lfg")).collect();
    files.sort();
    let mut models: BTreeMap<String, (String, Vec<(String, String)>, Vec<u8>)> = BTreeMap::new();
    for f in &files {
        let g = GrassFile::from_bytes(&std::fs::read(f)?).with_context(|| format!("reading {}", f.display()))?;
        for t in &g.types {
            for suffix in TIER_SUFFIX {
                let key = format!("{}{suffix}", t.model);
                models.entry(key.clone()).or_insert_with(|| (key, Vec::new(), Vec::new()));
            }
        }
    }
    // Full meshes once, for the card and the stats.
    let mut full: HashMap<String, Option<LodMesh>> = HashMap::new();
    // Cards: every grass model rendered into two billboard views (a scratch folder next to the
    // output, read through the VFS while the pages are made).
    let work = out.parent().unwrap_or(out).join(format!(".lodforge-grass-cards-{}", std::process::id()));
    let grass_models: Vec<String> = models.keys().filter(|k| !k.contains('#')).cloned().collect();
    // Community Shaders complex grass (colour over normal/specular in one texture).
    {
        let mut tx = crate::instances::TextureIndex::default();
        let complex: Vec<&String> = grass_models
            .iter()
            .filter(|m| {
                vfs_mesh(p, m).and_then(|lm| lm.parts.iter().find(|q| !q.textures[0].trim().is_empty()).map(|q| tx.resolve(p, &q.textures[0]))).and_then(|t| p.vfs.read(&t).ok().flatten()).is_some_and(|b| is_complex_texture(&b))
            })
            .collect();
        println!("grass: {} of {} grass models use complex grass textures", complex.len(), grass_models.len());
        if std::env::var_os("LODFORGE_GRASS_COMPLEX_LIST").is_some() {
            for m in &complex {
                println!("  complex: {m}");
            }
        }
    }
    let complex_textures = complex_diffuses(p, &grass_models);
    let s = &Settings { tile_cap: s.tile_cap, page: s.page, complex: complex_textures.clone() };
    let cards = render_cards(p, &grass_models, &work, &complex_textures)?;
    let p: &Profile = p;
    let load = |key: &str| -> Option<LodMesh> {
        if let Some(m) = key.strip_suffix("#decimated") {
            return decimated(p, m);
        }
        if let Some(m) = key.strip_suffix("#card") {
            let f = vfs_mesh(p, m)?;
            return match cards.get(m) {
                Some(rc) => rendered_card(&f, rc),
                None => card(&f),
            };
        }
        vfs_mesh(p, key)
    };
    let made = make(p, out, &models, s, write, PAGE_BASE, FILE_NAME, &load);
    let _ = std::fs::remove_dir_all(&work);
    let (mut rep, mesh_of) = made?;
    rep.rendered_cards = cards.len();
    let mut textures = crate::instances::TextureIndex::default();
    let mut stats: HashMap<String, ([u8; 4], f32)> = HashMap::new();
    for f in &files {
        let mut g = GrassFile::from_bytes(&std::fs::read(f)?)?;
        g.pack = g
            .types
            .iter()
            .map(|t| {
                let (albedo, height) = *stats.entry(t.model.clone()).or_insert_with(|| {
                    let m = full.entry(t.model.clone()).or_insert_with(|| vfs_mesh(p, &t.model));
                    match m {
                        Some(m) => {
                            let (lo, hi) = m.parts.iter().flat_map(|p| &p.positions).fold((f32::MAX, f32::MIN), |(l, h), v| (l.min(v[2]), h.max(v[2])));
                            let albedo = cards.get(&t.model).map(|c| c.albedo).unwrap_or_else(|| albedo_stats(p, m, &mut textures));
                            (albedo, if hi >= lo { hi - lo } else { 0.0 })
                        }
                        None => ([0; 4], 0.0),
                    }
                });
                let tiers = TIER_SUFFIX.map(|suffix| mesh_of.get(&format!("{}{suffix}", t.model)).copied().unwrap_or(NO_MESH));
                GrassTypePack { tiers, albedo, height }
            })
            .collect();
        std::fs::write(f, g.to_bytes(write)?)?;
    }
    rep.trees_files = files.len();
    rep.ms = t0.elapsed().as_millis();
    Ok(rep)
}

/// `lodforge runtime-grass-pack`: (re)builds only `GrassMeshes.bin` for the `.lfg` files of an
/// output and links them.
#[derive(clap::Args)]
pub struct Args {
    #[command(flatten)]
    pub profile: ProfileArgs,
    /// The output (Data or mod folder) holding SKSE\Plugins\LODForgeRuntime\*.lfg.
    #[arg(long)]
    pub data: PathBuf,
    #[arg(long, default_value_t = 512)]
    pub tile: u32,
    #[arg(long, default_value_t = 3)]
    pub zstd: i32,
}

pub fn run(a: &Args) -> Result<()> {
    let mut p = a.profile.open()?;
    // GrassMeshes.bin takes the output's run token (Build.txt), like the files it links to.
    println!("run token {}", crate::instances::init_build_token(&a.data));
    p.vfs.add_loose_root_since(&a.data, &lodforge_archive::DirSnapshot::default())?;
    let rep = build(&mut p, &a.data, &Settings { tile_cap: a.tile, ..Default::default() }, WriteOptions { zstd: (a.zstd > 0).then_some(a.zstd), crc: true })?;
    println!("grass {} ({} cards rendered as billboards)", rep.summary(), rep.rendered_cards);
    for u in &rep.unresolved {
        println!("  unresolved: {u}");
    }
    Ok(())
}
