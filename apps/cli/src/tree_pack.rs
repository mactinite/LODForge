//! The near-tier tree mesh pack (`SKSE\Plugins\LODForgeRuntime\TreeMeshes.bin`, docs/runtime/
//! instance-formats.md section 7a): every `TMSH` model of the trees files in an output, with its
//! textures moved onto shared atlas pages (the object LOD atlas planner and page renderer:
//! per-tile mips, alpha coverage, BC7), tiled UVs re-cut into one texture repeat, merged into
//! one shape and cut into 64-triangle clusters. The trees files get a `TMPK` chunk naming each
//! type's pack mesh.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};

use lodforge_instances::meshpack::{CLUSTER_SIZE, MeshInput, MeshPack, NO_MESH, Page, VERTEX_SIZE, Vertex, pack_unit};
use lodforge_instances::trees::TreeFile;
use lodforge_instances::{Common, WriteOptions};

use crate::profile::Profile;

/// Game path stem of the pages (`<stem>_Alpha[NN].dds`: the `_Alpha` suffix makes the page renderer treat them as alpha-tested
/// pages: average colour under the cut-out, coverage-preserving mips).
pub const PAGE_BASE: &str = r"textures\LODForge\TreeAtlas\TreeAtlas";
/// UV slack before a part is re-cut (half a texel of a 512 tile).
const UV_SLACK: f32 = 0.001;

pub struct Settings {
    /// Largest atlas tile per source texture (halved until it fits).
    pub tile_cap: u32,
    /// Page size.
    pub page: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { tile_cap: 512, page: 4096 }
    }
}

#[derive(Default, Debug)]
pub struct Report {
    pub models: usize,
    pub meshes: usize,
    pub textures: usize,
    pub pages: usize,
    pub page_bytes: u64,
    pub vertices: usize,
    pub triangles: usize,
    pub clusters: usize,
    pub buffer_bytes: usize,
    pub pack_bytes: usize,
    pub recut_parts: usize,
    pub dropped_parts: usize,
    pub other_threshold_parts: usize,
    /// Parts drawn without alpha test (tile alpha forced to 255).
    pub opaque_parts: usize,
    pub trees_files: usize,
    pub ms: u128,
    /// A sample of parts dropped for want of a texture.
    pub unresolved: Vec<String>,
}

impl Report {
    pub fn summary(&self) -> String {
        let mb = |b: f64| b / 1e6;
        format!(
            "mesh pack: {} models -> {} meshes, {} textures on {} atlas page(s) ({:.1} MB BC7 + mips); {} vertices, {} triangles in {} clusters of 64 ({:.1}% padding); GPU buffers {:.1} MB, pack file {:.1} MB; {} parts re-cut, {} parts without a texture dropped, {} parts with an alpha threshold other than 128 (alpha remapped to 128), {} opaque parts; {} instance files linked; {:.1} s",
            self.models,
            self.meshes,
            self.textures,
            self.pages,
            mb(self.page_bytes as f64),
            self.vertices,
            self.triangles,
            self.clusters,
            100.0 * (1.0 - self.triangles as f64 / (self.clusters * 64).max(1) as f64),
            mb(self.buffer_bytes as f64),
            mb(self.pack_bytes as f64),
            self.recut_parts,
            self.dropped_parts,
            self.other_threshold_parts,
            self.opaque_parts,
            self.trees_files,
            self.ms as f64 / 1000.0
        )
    }
}

fn trees_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir).with_context(|| format!("listing {}", dir.display()))?.flatten().map(|e| e.path()).filter(|p| p.to_string_lossy().to_ascii_lowercase().ends_with(".trees.bin")).collect();
    v.sort();
    Ok(v)
}

/// Builds the tree pack from the trees files in `out` and links them to it (`TMPK`).
pub fn build(p: &Profile, out: &Path, s: &Settings, write: WriteOptions) -> Result<Report> {
    let t0 = Instant::now();
    let dir = crate::instances::out_dir(out);
    let files = trees_files(&dir)?;
    // Every model (sorted, so the pack is deterministic) with its resolved textures.
    let mut models: BTreeMap<String, (String, Vec<(String, String)>, Vec<u8>)> = BTreeMap::new();
    for f in &files {
        let t = TreeFile::from_bytes(&std::fs::read(f)?).with_context(|| format!("reading {}", f.display()))?;
        for m in t.meshes {
            if !m.model.is_empty() {
                models.entry(m.model.clone()).or_insert_with(|| (m.model.clone(), m.shapes.iter().map(|s| (s.diffuse.clone(), s.normal.clone())).collect(), Vec::new()));
            }
        }
    }
    let (mut rep, mesh_of) = make(p, out, &models, s, write, PAGE_BASE, lodforge_instances::meshpack::FILE_NAME)?;
    rep.trees_files = files.len();
    for f in &files {
        let mut t = TreeFile::from_bytes(&std::fs::read(f)?)?;
        t.pack_meshes = t.meshes.iter().map(|m| mesh_of.get(&m.model).copied().unwrap_or(NO_MESH)).collect();
        std::fs::write(f, t.to_bytes(write)?)?;
    }
    rep.ms = t0.elapsed().as_millis();
    Ok(rep)
}

/// Game path of the static mesh pack's pages.
pub const STATIC_PAGE_BASE: &str = r"textures\LODForge\StaticAtlas\StaticAtlas";
pub const STATIC_FILE_NAME: &str = "StaticMeshes.bin";

/// Builds the static pack from the `.statics.bin` files in `out` (every `MESH` model) and links
/// them to it (`SMPK`).
pub fn build_statics(p: &Profile, out: &Path, s: &Settings, write: WriteOptions) -> Result<Report> {
    use lodforge_instances::statics::StaticsFile;
    let t0 = Instant::now();
    let dir = crate::instances::out_dir(out);
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)?.flatten().map(|e| e.path()).filter(|p| p.to_string_lossy().to_ascii_lowercase().ends_with(".statics.bin")).collect();
    files.sort();
    // One pack mesh per (model, texture set); large-reference entries share it.
    let key_of = |m: &lodforge_instances::statics::Mesh| format!("{}|{}|{:?}", m.model, m.textures.iter().map(|(d, n)| format!("{d},{n}")).collect::<Vec<_>>().join(";"), m.classes);
    let mut textures = crate::instances::TextureIndex::default();
    let mut models: BTreeMap<String, (String, Vec<(String, String)>, Vec<u8>)> = BTreeMap::new();
    for f in &files {
        let t = StaticsFile::from_bytes(&std::fs::read(f)?).with_context(|| format!("reading {}", f.display()))?;
        for m in &t.meshes {
            models.entry(key_of(m)).or_insert_with(|| (m.model.clone(), m.textures.iter().map(|(d, n)| (textures.resolve(p, d), textures.resolve(p, n))).collect(), m.classes.clone()));
        }
    }
    let (mut rep, mesh_of) = make(p, out, &models, s, write, STATIC_PAGE_BASE, STATIC_FILE_NAME)?;
    rep.trees_files = files.len();
    for f in &files {
        let mut t = StaticsFile::from_bytes(&std::fs::read(f)?)?;
        t.pack_meshes = t.meshes.iter().map(|m| mesh_of.get(&key_of(m)).copied().unwrap_or(NO_MESH)).collect();
        std::fs::write(f, t.to_bytes(write)?)?;
    }
    rep.ms = t0.elapsed().as_millis();
    Ok(rep)
}

/// A mesh pack of `models` (with their shapes' resolved diffuse/normal in part order; an empty
/// list resolves the textures the model names) written to `out` as `file_name` with pages under
/// `page_base`. Returns the report and model -> pack mesh.
pub fn make(p: &Profile, out: &Path, models: &BTreeMap<String, (String, Vec<(String, String)>, Vec<u8>)>, s: &Settings, write: WriteOptions, page_base: &str, file_name: &str) -> Result<(Report, HashMap<String, u32>)> {
    let dir = crate::instances::out_dir(out);
    std::fs::create_dir_all(&dir)?;
    let mut rep = Report { models: models.len(), ..Default::default() };
    let mut textures = crate::instances::TextureIndex::default();
    struct Part {
        part: lodforge_objlod::mesh::MeshPart,
        key: String,
        class: u8,
        whiten: bool,
    }
    let mut loaded: Vec<(String, String, Vec<Part>)> = Vec::new();
    let mut sizes: BTreeMap<(u8, String), (u32, u32)> = BTreeMap::new();
    for (key, (model, shapes, classes)) in models {
        let Some(bytes) = p.vfs.read(model).ok().flatten() else { continue };
        let Ok(lm) = lodforge_objlod::mesh::import(model, &bytes) else { continue };
        let mut parts = Vec::new();
        for (i, part) in lm.parts.into_iter().enumerate() {
            let shape = match shapes.get(i) {
                Some(s) => s.clone(),
                None if shapes.is_empty() => (textures.resolve(p, &part.textures[0]), textures.resolve(p, &part.textures[1])),
                None => {
                    rep.dropped_parts += 1;
                    continue;
                }
            };
            let shape = lodforge_instances::trees::MeshShape { diffuse: shape.0, normal: shape.1, ..Default::default() };
            if shape.diffuse.is_empty() {
                rep.dropped_parts += 1;
                if rep.unresolved.len() < 12 {
                    rep.unresolved.push(format!("{model}: {:?} {:?}", part.name, part.textures[0]));
                }
                continue;
            }
            // Shapes alpha-testing at another threshold get their own tile with remapped alpha.
            let mut key = format!("{},{}", shape.diffuse, shape.normal);
            match part.alpha {
                Some((flags, threshold)) if flags & 0x200 != 0 => {
                    if threshold != 128 {
                        rep.other_threshold_parts += 1;
                        key.push_str(&format!("=t{threshold}"));
                    }
                }
                // No alpha test (or blending only): drawn opaque, so its tile gets alpha 255.
                _ => {
                    rep.opaque_parts += 1;
                    key.push_str("=o");
                }
            }
            let class_flags = classes.get(i).copied().unwrap_or(0);
            let (class, whiten) = (class_flags & 0x7F, class_flags & 0x80 != 0);
            if !sizes.contains_key(&(class, key.clone())) {
                let size = p.vfs.read(&shape.diffuse).ok().flatten().and_then(|b| lodforge_dds::parse(&b).ok()).map(|i| crate::dyndolod_build::tile_size(i.width, i.height, s.tile_cap));
                let Some(size) = size else {
                    rep.dropped_parts += 1;
                    continue;
                };
                sizes.insert((class, key.clone()), size);
            }
            let part = if lodforge_objlod::reuv::needs_recut(&part.uvs, UV_SLACK) {
                rep.recut_parts += 1;
                lodforge_objlod::reuv::recut(&part)
            } else {
                part
            };
            parts.push(Part { part, key, class, whiten });
        }
        if !parts.is_empty() {
            loaded.push((key.clone(), model.clone(), parts));
        }
    }
    rep.textures = sizes.len();
    // Atlas: the object LOD planner and renderer, largest tiles first; one page family per LOD
    // material class (`<stem>[_Snow|_SnowHD|_Ash|_AshHD]_Alpha[NN].dds`).
    let gpu = lodforge_gpu::Gpu::new()?;
    let formats = crate::object_atlas::Formats { diffuse: crate::object_atlas::Encoding::Bc7, alpha_diffuse: crate::object_atlas::Encoding::Bc7, normal: crate::object_atlas::Encoding::Bc7, mips: u32::MAX, alpha_fill: true };
    let mut pack = MeshPack { common: Common { world: String::new(), season: String::new(), generator: crate::instances::generator(), plugins: Vec::new(), params: format!("tile_cap={}\npage={}\nalpha_threshold=128", s.tile_cap, s.page) }, ..Default::default() };
    // (class, texture key) -> (pack page, tile).
    let mut entry: HashMap<(u8, String), (u16, lodforge_objlod::atlas_build::AtlasEntry)> = HashMap::new();
    let classes: std::collections::BTreeSet<u8> = sizes.keys().map(|(c, _)| *c).collect();
    for class in classes {
        let mut list: Vec<(String, u32, u32)> = sizes.iter().filter(|((c, _), _)| *c == class).map(|((_, k), (w, h))| (k.clone(), *w, *h)).collect();
        list.sort_by(|a, b| (b.1 * b.2).cmp(&(a.1 * a.2)).then(a.0.cmp(&b.0)));
        let tag = match class {
            lodforge_instances::meshpack::CLASS_SNOW => "_Snow",
            lodforge_instances::meshpack::CLASS_SNOW_HD => "_SnowHD",
            lodforge_instances::meshpack::CLASS_ASH => "_Ash",
            lodforge_instances::meshpack::CLASS_ASH_HD => "_AshHD",
            _ => "",
        };
        let layout = lodforge_objlod::atlas_build::layout(&list, s.page, s.page, &format!("{page_base}{tag}_Alpha"));
        let pages = crate::object_atlas::render(&p.vfs, &layout, &gpu, &formats)?;
        crate::object_atlas::write(out, &pages)?;
        rep.pages += pages.len();
        rep.page_bytes += pages.iter().map(|pg| (pg.diffuse.len() + pg.normal.len()) as u64).sum::<u64>();
        let first = pack.pages.len() as u16;
        pack.pages.extend(layout.pages.iter().map(|pg| Page { width: pg.width, height: pg.height, mips: pg.width.max(pg.height).ilog2() + 1, alpha_threshold: 128, class, albedo: pg.name.to_ascii_lowercase(), normal: crate::object_atlas::normal_name(&pg.name).to_ascii_lowercase() }));
        for e in layout.entries.into_iter().filter(|e| e.page != usize::MAX) {
            entry.insert((class, e.texture.clone()), (first + e.page as u16, e));
        }
    }
    let mut mesh_of: HashMap<String, u32> = HashMap::new();
    for (key, model, parts) in loaded {
        let mut input = MeshInput { model: model.clone(), vertices: Vec::new(), triangles: Vec::new() };
        for Part { part, key, class, whiten } in &parts {
            let Some((page, e)) = entry.get(&(*class, key.clone())) else {
                rep.dropped_parts += 1;
                continue;
            };
            let pg = &pack.pages[usize::from(*page)];
            let base = input.vertices.len() as u32;
            // Half-texel inset so bilinear filtering stays inside the tile.
            let map = |uv: f32, origin: u32, size: u32, page: u32| ((f64::from(origin) + 0.5 + f64::from(uv.clamp(0.0, 1.0)) * f64::from(size.saturating_sub(1))) / f64::from(page) * 65535.0).round().clamp(0.0, 65535.0) as u16;
            for (i, pos) in part.positions.iter().enumerate() {
                let n = part.normals.get(i).copied().unwrap_or([0.0, 0.0, 1.0]);
                let t = part.tangents.get(i).copied().unwrap_or([1.0, 0.0, 0.0]);
                let b = part.bitangents.get(i).copied().unwrap_or([0.0, 1.0, 0.0]);
                let c = [n[1] * t[2] - n[2] * t[1], n[2] * t[0] - n[0] * t[2], n[0] * t[1] - n[1] * t[0]];
                let w = u32::from(c[0] * b[0] + c[1] * b[1] + c[2] * b[2] >= 0.0);
                let uv = part.uvs.get(i).copied().unwrap_or([0.0, 0.0]);
                input.vertices.push(Vertex {
                    pos: *pos,
                    normal: pack_unit(n, 0),
                    tangent: pack_unit(t, w),
                    uv: [map(uv[0], e.x, e.width, pg.width), map(uv[1], e.y, e.height, pg.height)],
                    color: part.colors.as_ref().and_then(|c| c.get(i).copied()).map(|c| if *whiten { [255, 255, 255, c[3]] } else { c }).unwrap_or([255; 4]),
                });
            }
            for tri in &part.triangles {
                input.triangles.push(([base + u32::from(tri[0]), base + u32::from(tri[1]), base + u32::from(tri[2])], *page));
            }
        }
        if !input.triangles.is_empty() {
            mesh_of.insert(key, pack.add_mesh(input));
        }
    }
    rep.meshes = pack.meshes.len();
    rep.vertices = pack.vertices.len();
    rep.triangles = pack.meshes.iter().map(|m| m.triangles as usize).sum();
    rep.clusters = pack.clusters.len();
    rep.buffer_bytes = pack.buffer_bytes();
    debug_assert_eq!(rep.buffer_bytes, rep.vertices * VERTEX_SIZE + rep.clusters * (192 * 4 + CLUSTER_SIZE));
    let bytes = pack.to_bytes(write)?;
    anyhow::ensure!(MeshPack::from_bytes(&bytes)?.clusters.len() == pack.clusters.len(), "{file_name} reads back differently");
    std::fs::write(dir.join(file_name), &bytes)?;
    rep.pack_bytes = bytes.len();
    Ok((rep, mesh_of))
}

/// `lodforge runtime-statics-pack`: rebuilds only `StaticMeshes.bin` (and its pages) from the
/// statics files of an output, with that output's own files visible as inputs.
#[derive(clap::Args)]
pub struct StaticsArgs {
    #[command(flatten)]
    pub profile: crate::profile::ProfileArgs,
    /// The output (Data or mod folder) holding SKSE\Plugins\LODForgeRuntime\*.statics.bin.
    #[arg(long)]
    pub data: PathBuf,
    #[arg(long, default_value_t = 512)]
    pub tile: u32,
    #[arg(long, default_value_t = 3)]
    pub zstd: i32,
}

pub fn run_statics(a: &StaticsArgs) -> Result<()> {
    let mut p = a.profile.open()?;
    p.vfs.add_loose_root_since(&a.data, &lodforge_archive::DirSnapshot::default())?;
    let rep = build_statics(&p, &a.data, &Settings { tile_cap: a.tile, ..Default::default() }, WriteOptions { zstd: (a.zstd > 0).then_some(a.zstd), crc: true })?;
    println!("static {}", rep.summary());
    for u in &rep.unresolved {
        println!("  unresolved: {u}");
    }
    Ok(())
}
