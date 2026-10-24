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

/// The card tier: two crossed double-sided quads over the bounds of `full`.
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

/// Builds `GrassMeshes.bin` from the `.lfg` files in `out` and links them (`GMPK`).
pub fn build(p: &Profile, out: &Path, s: &Settings, write: WriteOptions) -> Result<Report> {
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
    let load = |key: &str| -> Option<LodMesh> {
        if let Some(m) = key.strip_suffix("#decimated") {
            return decimated(p, m);
        }
        if let Some(m) = key.strip_suffix("#card") {
            return vfs_mesh(p, m).and_then(|f| card(&f));
        }
        vfs_mesh(p, key)
    };
    let (mut rep, mesh_of) = make(p, out, &models, s, write, PAGE_BASE, FILE_NAME, &load)?;
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
                            (albedo_stats(p, m, &mut textures), if hi >= lo { hi - lo } else { 0.0 })
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
    p.vfs.add_loose_root_since(&a.data, &lodforge_archive::DirSnapshot::default())?;
    let rep = build(&p, &a.data, &Settings { tile_cap: a.tile, ..Default::default() }, WriteOptions { zstd: (a.zstd > 0).then_some(a.zstd), crc: true })?;
    println!("grass {}", rep.summary());
    for u in &rep.unresolved {
        println!("  unresolved: {u}");
    }
    Ok(())
}
