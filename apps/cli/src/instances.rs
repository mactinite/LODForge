//! `build --runtime lodforge`: the LODForge Runtime instance files for trees and repeated statics
//! (docs/runtime/instance-formats.md). Trees come from the DynDOLOD-mode export (the same
//! filters as object and tree LOD) and leave the `.bto`; the 3D LOD models of those trees, and
//! optionally other repeated LOD models, are written as per-block instance lists by the object
//! LOD stage (`lodforge_objlod::instancing`).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};

use lodforge_dyndolod::assign::is_tree;
use lodforge_dyndolod::bases::owner_key;
use lodforge_dyndolod::export::{RefLine, tree_tilted};
use lodforge_esp::Worldspace;
use lodforge_instances::statics::{MESH_PASSTHRU, MESH_TREE, MESH_VERTEX_ALPHA, Mesh, StaticsFile};
use lodforge_instances::trees::{MeshShape, TreeFile, TreeMesh, TreeType};
use lodforge_instances::{Common, WriteOptions};

use crate::dyndolod_build::{Config, SeasonExport};
use crate::profile::Profile;

/// `--runtime`.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, clap::ValueEnum)]
pub enum RuntimeMode {
    /// LOD for the game and DynDOLOD DLL (default).
    #[default]
    None,
    /// LODForge Runtime: trees and instanced statics as instance files, out of `.bto` / `.btt`.
    Lodforge,
}

/// The runtime options of `lodforge build`.
#[derive(clap::Args, Clone, Debug, Default)]
pub struct Opts {
    /// `lodforge`: write `SKSE\Plugins\LODForgeRuntime\*` instance files for the LODForge Runtime
    /// plugin, leave the trees it draws out of `.bto` / `.btt` and draw far grass from `.lfg`
    /// (needs `--mode dyndolod`; trees are kept in object LOD as with `--tree-lod ultra`).
    #[arg(long, value_enum, default_value = "none")]
    pub runtime: RuntimeMode,
    /// Runtime: other LOD models with at least this many copies in one object LOD block become
    /// instances too (0 = only the runtime trees' 3D LOD models).
    #[arg(long, default_value_t = 0)]
    pub runtime_min_copies: usize,
    /// Runtime: which models `--runtime-min-copies` applies to (substrings of the lower-case
    /// path; empty = every model).
    #[arg(long, value_delimiter = ',', default_value = "passthru")]
    pub runtime_model_masks: Vec<String>,
    /// Runtime: also write the runtime trees' 3D LOD models as per-block instances in
    /// `.statics.bin` (off: the trees file's per-type mesh table, `TMSH`, is the near tier and the
    /// tree records are the only placements).
    #[arg(long)]
    pub runtime_tree_statics: bool,
    /// Runtime: zstd level for the instance file sections (0 = uncompressed).
    #[arg(long, default_value_t = 0)]
    pub runtime_zstd: i32,
    /// Runtime far grass: folder with the NGIO grass cache (`<world>x<X>y<Y>[.<season>].cgid`;
    /// default: `Grass\` of the load order).
    #[arg(long)]
    pub runtime_grass_cache: Option<PathBuf>,
    /// Runtime: no far grass (`.lfg`).
    #[arg(long)]
    pub runtime_no_grass: bool,
}

impl Opts {
    pub fn on(&self) -> bool {
        self.runtime == RuntimeMode::Lodforge
    }

    pub fn write_options(&self) -> WriteOptions {
        WriteOptions { zstd: (self.runtime_zstd > 0).then_some(self.runtime_zstd), crc: true }
    }
}

/// `Data\SKSE\Plugins\LODForgeRuntime` under `out`.
pub fn out_dir(out: &Path) -> PathBuf {
    let mut d = out.to_path_buf();
    for part in lodforge_instances::DATA_DIR.split('\\') {
        d.push(part);
    }
    d
}

pub fn generator() -> String {
    format!("LODForge {}", env!("CARGO_PKG_VERSION"))
}

/// Load-order form id -> (defining plugin file name, local form id).
pub struct FormNames {
    owners: HashMap<u32, usize>,
    names: Vec<String>,
}

impl FormNames {
    pub fn new(p: &Profile) -> FormNames {
        FormNames { owners: lodforge_dyndolod::bases::owners(&p.load_order), names: p.plugin_names.clone() }
    }

    pub fn split(&self, id: u32) -> Option<(&str, u32)> {
        if lodforge_dyndolod::childworld::is_copy(id) || id == 0 {
            return None;
        }
        let i = *self.owners.get(&owner_key(id))?;
        let local = if id >> 24 == 0xFE { id & 0xFFF } else { id & 0x00FF_FFFF };
        Some((self.names.get(i)?.as_str(), local))
    }
}

/// Why a tree reference of the export stays in object LOD in runtime mode.
#[derive(Debug, Default, Clone, Copy)]
pub struct Kept {
    pub tilted: usize,
    pub large: usize,
    pub copies: usize,
    pub bounds: usize,
    pub no_plugin: usize,
    /// The base has no baked impostor type.
    pub no_impostor: usize,
}

/// The trees of one season export.
pub struct SeasonTrees {
    pub file: TreeFile,
    /// Load-order form ids of the references in `file`.
    pub refs: HashSet<u32>,
    pub kept: Kept,
    /// EditorIDs of bases without an impostor type and their reference counts.
    pub no_impostor: HashMap<String, usize>,
}

impl SeasonTrees {
    /// Export column 1 spelling of [`SeasonTrees::refs`].
    pub fn ref_columns(&self) -> HashSet<String> {
        self.refs.iter().map(|id| format!("{id:08X}")).collect()
    }
}

/// A tree reference of the export the runtime can draw (instance-formats.md section 5, without
/// the impostor condition): its base and placement. Counts the reasons for leaving one out.
fn eligible<'a>(c: &'a Config, world: &'a Worldspace, l: &RefLine, bounds: &[(i32, i32, i32, i32)], tilt: f64, kept: &mut Kept) -> Option<(&'a lodforge_dyndolod::bases::Base, &'a lodforge_esp::world::Placement)> {
    let b = c.bases.get(&l.base)?;
    if !is_tree(b) || !l.columns.iter().any(|c| !c.is_empty()) {
        return None;
    }
    if lodforge_dyndolod::childworld::is_copy(l.ref_id) {
        kept.copies += 1;
        return None;
    }
    if l.material.ends_with("-LargeRef") {
        kept.large += 1;
        return None;
    }
    let pl = world.refs.get(&l.ref_id)?;
    if tree_tilted(pl.rotation, tilt) {
        kept.tilted += 1;
        return None;
    }
    let cell = ((pl.position[0] / 4096.0).floor() as i32, (pl.position[1] / 4096.0).floor() as i32);
    if bounds.iter().any(|&(x0, y0, x1, y1)| (x0..=x1).contains(&cell.0) && (y0..=y1).contains(&cell.1)) {
        kept.bounds += 1;
        return None;
    }
    Some((b, pl))
}

/// Bases of every tree the runtime would draw in these season exports, whether or not an
/// impostor exists yet: the impostor stage bakes them all.
pub fn runtime_bases(c: &Config, world: &Worldspace, seasons: &[SeasonExport]) -> HashSet<u32> {
    let bounds = crate::dyndolod_build::tree_object_bounds(c, &world.editor_id);
    let tilt: f64 = c.num("TreeLODMaxTilt", 30.0);
    let mut kept = Kept::default();
    let mut out = HashSet::new();
    for s in seasons {
        for l in &s.lines {
            if eligible(c, world, l, &bounds, tilt, &mut kept).is_some() {
                out.insert(l.base);
            }
        }
    }
    out
}

/// Selects the trees of one season export the runtime draws, typed by the impostor set.
pub fn select_trees(c: &Config, world: &Worldspace, s: &SeasonExport, names: &FormNames, impostors: &crate::impostors::SetReport) -> SeasonTrees {
    let bounds = crate::dyndolod_build::tree_object_bounds(c, &world.editor_id);
    let tilt: f64 = c.num("TreeLODMaxTilt", 30.0);
    let season = s.season.map(|x| x.code().to_string()).unwrap_or_default();
    let mut file = TreeFile { common: Common { world: world.editor_id.clone(), season, generator: generator(), plugins: Vec::new(), params: format!("tree_max_tilt={tilt}") }, ..Default::default() };
    file.types = impostors.types.iter().map(|t| TreeType { name: t.name.clone(), model: t.model.clone(), model_crc32: t.model_crc32, ..Default::default() }).collect();
    let mut seen_base: HashSet<u32> = HashSet::new();
    let mut lod4_counts: Vec<HashMap<String, usize>> = vec![HashMap::new(); file.types.len()];
    let mut no_impostor: HashMap<String, usize> = HashMap::new();
    let mut refs = HashSet::new();
    let mut kept = Kept::default();
    for l in &s.lines {
        if refs.contains(&l.ref_id) {
            continue;
        }
        let Some((b, pl)) = eligible(c, world, l, &bounds, tilt, &mut kept) else { continue };
        let Some((plugin, local)) = names.split(l.ref_id) else {
            kept.no_plugin += 1;
            continue;
        };
        // The tree's type is its base's impostor type (the impostor set's index); bases without a
        // baked impostor stay in object LOD.
        let Some(&ti) = impostors.base_index.get(&l.base) else {
            kept.no_impostor += 1;
            *no_impostor.entry(b.editor_id.clone()).or_default() += 1;
            continue;
        };
        let ty = ti as u16;
        if seen_base.insert(l.base) {
            let bp = names.split(l.base).map(|(n, f)| (file.common.plugin_index(n), f)).unwrap_or((0xFFFF, l.base));
            let t = &mut file.types[usize::from(ty)];
            if t.bases.is_empty() {
                t.bounds = b.bounds;
                t.lod4 = l.columns[0].to_ascii_lowercase();
            }
            t.bases.push(bp);
            t.tree_record |= &b.signature == b"TREE";
        }
        let scale = if pl.has_scale { pl.scale } else { 1.0 };
        if file.add(pl.position, pl.rotation[2], scale, ty, plugin, local).is_ok() {
            refs.insert(l.ref_id);
            *lod4_counts[usize::from(ty)].entry(l.columns[0].to_ascii_lowercase()).or_default() += 1;
        }
    }
    // Each type's LOD4 column: the most used 3D model of its references, else the most used column.
    for (t, counts) in file.types.iter_mut().zip(&lod4_counts) {
        let best = |three_d: bool| counts.iter().filter(|(m, _)| !m.is_empty() && (!three_d || !m.ends_with(".dds"))).max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0))).map(|(m, _)| m.clone());
        if let Some(m) = best(true).or_else(|| best(false)) {
            t.lod4 = m;
        }
    }
    file.finish();
    SeasonTrees { file, refs, kept, no_impostor }
}

/// Writes one trees file; returns its path and size.
pub fn write_trees(out: &Path, t: &SeasonTrees, opts: WriteOptions) -> Result<(PathBuf, usize)> {
    let dir = out_dir(out);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(t.file.common.file_name(lodforge_instances::trees::EXT));
    let bytes = t.file.to_bytes(opts)?;
    anyhow::ensure!(TreeFile::from_bytes(&bytes)?.records() == t.file.records(), "trees file reads back differently");
    std::fs::write(&path, &bytes).with_context(|| format!("writing {}", path.display()))?;
    Ok((path, bytes.len()))
}

/// What the object LOD stage needs for runtime instancing.
#[derive(Clone)]
pub struct RuntimeObjects {
    /// Season code (`""`, `SPR`, ...) -> export column 1 of the runtime trees.
    pub refs: HashMap<String, Arc<HashSet<String>>>,
    pub tree_models: bool,
    pub min_copies: usize,
    pub model_masks: Vec<String>,
    pub names: Arc<FormNames>,
    pub write: WriteOptions,
}

impl std::fmt::Debug for RuntimeObjects {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RuntimeObjects({} seasons, min_copies {})", self.refs.len(), self.min_copies)
    }
}

impl RuntimeObjects {
    pub fn instancing(&self, season: &str) -> lodforge_objlod::instancing::Instancing {
        lodforge_objlod::instancing::Instancing {
            runtime_refs: self.refs.get(&season.to_ascii_uppercase()).cloned().unwrap_or_default(),
            runtime_ref_instances: self.tree_models,
            min_copies: self.min_copies,
            model_masks: self.model_masks.iter().map(|m| m.to_ascii_lowercase()).filter(|m| !m.is_empty()).collect(),
        }
    }
}

/// Builds the statics file of one world / season from the object LOD stage's instances.
pub fn statics_file(world: &str, season: &str, rt: &RuntimeObjects, quads: &[(lodforge_objlod::Quad, &[lodforge_objlod::instancing::Instance])], mesh: impl Fn(&str) -> Option<Arc<lodforge_objlod::mesh::LodMesh>>) -> StaticsFile {
    let params = format!("min_copies={}\nmodel_masks={}\ntree_refs={}", rt.min_copies, rt.model_masks.join(","), u8::from(rt.tree_models));
    let mut f = StaticsFile::new(Common { world: world.to_string(), season: season.to_string(), generator: generator(), plugins: Vec::new(), params });
    for (q, list) in quads {
        for i in *list {
            let m = f.mesh(&i.model, || mesh_entry(&i.model, mesh(&i.model).as_deref(), i.runtime_ref));
            if i.runtime_ref {
                f.meshes[usize::from(m)].flags |= MESH_TREE;
            }
            let id = u32::from_str_radix(&i.ref_id, 16).unwrap_or(0);
            let (plugin, form) = match rt.names.split(id) {
                Some((n, local)) => (Some(n.to_string()), local),
                None => (None, 0),
            };
            let angles = i.rotation_deg.map(f32::to_radians);
            f.add(q.level as u8, (q.x as i16, q.y as i16), i.position, angles, i.scale, m, plugin.as_deref(), form);
        }
    }
    f.finish();
    f
}

fn mesh_entry(model: &str, m: Option<&lodforge_objlod::mesh::LodMesh>, tree: bool) -> Mesh {
    let mut e = Mesh { bounds: [0.0; 6], flags: 0, triangles: 0, model: String::new() };
    if tree {
        e.flags |= MESH_TREE;
    }
    if model.contains("passthru") {
        e.flags |= MESH_PASSTHRU;
    }
    if let Some(m) = m {
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for part in &m.parts {
            for v in &part.positions {
                for k in 0..3 {
                    lo[k] = lo[k].min(v[k]);
                    hi[k] = hi[k].max(v[k]);
                }
            }
            e.triangles += part.triangles.len() as u32;
            if part.colors.as_ref().is_some_and(|c| c.iter().any(|v| v[3] != 255)) {
                e.flags |= MESH_VERTEX_ALPHA;
            }
        }
        if lo[0] <= hi[0] {
            e.bounds = [lo[0], lo[1], lo[2], hi[0], hi[1], hi[2]];
        }
    }
    e
}

/// Writes one statics file; returns its path and size.
pub fn write_statics(out: &Path, f: &StaticsFile, opts: WriteOptions) -> Result<(PathBuf, usize)> {
    let dir = out_dir(out);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(f.common.file_name(lodforge_instances::statics::EXT));
    let bytes = f.to_bytes(opts)?;
    anyhow::ensure!(StaticsFile::from_bytes(&bytes)?.records() == f.records(), "statics file reads back differently");
    std::fs::write(&path, &bytes).with_context(|| format!("writing {}", path.display()))?;
    Ok((path, bytes.len()))
}

/// The export lines whose references are in `refs` (for the tree report).
pub fn count_by_base(lines: &[RefLine], refs: &HashSet<u32>) -> HashMap<u32, usize> {
    let mut m: HashMap<u32, usize> = HashMap::new();
    let mut seen = HashSet::new();
    for l in lines {
        if refs.contains(&l.ref_id) && seen.insert(l.ref_id) {
            *m.entry(l.base).or_default() += 1;
        }
    }
    m
}

/// `lodforge bto-stats`: shapes (draw calls) and triangles of a worldspace's object LOD files per
/// level, to compare builds with and without `--runtime lodforge`.
#[derive(clap::Args)]
pub struct StatsArgs {
    /// Data folder (reads `meshes\terrain\<world>\Objects`).
    #[arg(long)]
    pub data: PathBuf,
    #[arg(long, default_value = "Tamriel")]
    pub world: String,
    /// Season code; default: the default season's files.
    #[arg(long)]
    pub season: Option<String>,
}

pub fn bto_stats(a: &StatsArgs) -> Result<()> {
    use rayon::prelude::*;
    let dir = a.data.join("meshes").join("terrain").join(&a.world).join("Objects");
    let want = a.season.as_deref().map(str::to_ascii_lowercase);
    let files: Vec<(i32, PathBuf)> = std::fs::read_dir(&dir)
        .with_context(|| format!("listing {}", dir.display()))?
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_str()?.to_ascii_lowercase();
            let stem = name.strip_suffix(".bto")?;
            let parts: Vec<&str> = stem.split('.').collect();
            let season = parts.get(4).copied();
            if parts.len() < 4 || season != want.as_deref() {
                return None;
            }
            Some((parts[1].parse().ok()?, e.path()))
        })
        .collect();
    let per: Vec<(i32, u64, usize, usize)> = files
        .par_iter()
        .map(|(level, path)| {
            let bytes = std::fs::read(path).unwrap_or_default();
            let (mut shapes, mut tris) = (0usize, 0usize);
            if let Ok(nif) = lodforge_nif::parse(&bytes) {
                for b in &nif.blocks {
                    if let lodforge_nif::Block::TriShape(s) = b {
                        shapes += 1;
                        tris += s.triangles.len();
                    }
                }
            }
            (*level, bytes.len() as u64, shapes, tris)
        })
        .collect();
    let mut levels: std::collections::BTreeMap<i32, (usize, u64, usize, usize)> = std::collections::BTreeMap::new();
    for (l, b, s, t) in per {
        let e = levels.entry(l).or_default();
        e.0 += 1;
        e.1 += b;
        e.2 += s;
        e.3 += t;
    }
    println!("{} object LOD{}:", a.world, a.season.as_deref().map(|s| format!(" [{s}]")).unwrap_or_default());
    for (l, (n, b, s, t)) in &levels {
        println!("  LOD{l:<2}: {n:>5} files, {:>8.1} MB, {s:>7} shapes, {t:>10} triangles, {:.1} shapes/file", *b as f64 / 1e6, *s as f64 / (*n).max(1) as f64);
    }
    Ok(())
}

impl SeasonTrees {
    /// The most referenced bases without an impostor type, `EditorID xN` comma separated.
    pub fn top_no_impostor(&self, n: usize) -> String {
        let mut v: Vec<(&String, &usize)> = self.no_impostor.iter().collect();
        v.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));
        v.into_iter().take(n).map(|(e, c)| format!("{e} x{c}")).collect::<Vec<_>>().join(", ")
    }
}

/// Texture paths by file name, for textures a model names under the wrong folder.
#[derive(Default)]
pub struct TextureIndex {
    by_name: Option<HashMap<String, Vec<String>>>,
}

impl TextureIndex {
    /// `path` if the load order has it, else the only texture with its file name, else `""`.
    pub fn resolve(&mut self, p: &Profile, path: &str) -> String {
        let path = path.replace('/', "\\").to_ascii_lowercase();
        if path.is_empty() {
            return path;
        }
        let path = if path.starts_with("textures\\") { path } else { format!("textures\\{path}") };
        if p.vfs.exists(&path) {
            return path;
        }
        let index = self.by_name.get_or_insert_with(|| {
            let mut m: HashMap<String, Vec<String>> = HashMap::new();
            for k in p.vfs.list_prefix("textures\\") {
                let name = k.rsplit('\\').next().unwrap_or_default().to_string();
                m.entry(name).or_default().push(k);
            }
            m
        });
        let name = path.rsplit('\\').next().unwrap_or_default();
        match index.get(name).map(Vec::as_slice) {
            Some([only]) => only.clone(),
            _ => String::new(),
        }
    }
}

/// Fills `file.meshes` (`TMSH`): each type's LOD4 3D model (its `lod4` column when that is a
/// mesh) with bounds, triangles and per-shape resolved textures. `cache` keeps models across
/// seasons and worldspaces.
pub fn fill_meshes(p: &Profile, out: &Path, file: &mut TreeFile, cache: &mut HashMap<String, TreeMesh>, textures: &mut TextureIndex) {
    file.meshes = file
        .types
        .iter()
        .map(|t| {
            let m = &t.lod4;
            if m.is_empty() || !m.ends_with(".nif") {
                return TreeMesh::default();
            }
            if let Some(c) = cache.get(m) {
                return c.clone();
            }
            let mut mesh = load_tree_mesh(p, m, textures);
            mesh.generated = !mesh.model.is_empty() && out.join(m.replace('\\', "/")).is_file();
            cache.insert(m.clone(), mesh.clone());
            mesh
        })
        .collect();
}

fn load_tree_mesh(p: &Profile, model: &str, textures: &mut TextureIndex) -> TreeMesh {
    let Some(bytes) = p.vfs.read(model).ok().flatten() else { return TreeMesh::default() };
    let Ok(lm) = lodforge_objlod::mesh::import(model, &bytes) else { return TreeMesh::default() };
    if lm.parts.is_empty() {
        return TreeMesh::default();
    }
    let mut out = TreeMesh { model: model.to_string(), passthru: model.contains("passthru"), ..Default::default() };
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for part in &lm.parts {
        for v in &part.positions {
            for k in 0..3 {
                lo[k] = lo[k].min(v[k]);
                hi[k] = hi[k].max(v[k]);
            }
        }
        out.triangles += part.triangles.len() as u32;
        let (has_alpha, alpha_flags, alpha_threshold) = match part.alpha {
            Some((f, t)) => (true, f, t),
            None => (false, 0, 0),
        };
        out.shapes.push(MeshShape {
            name: part.name.clone(),
            diffuse: textures.resolve(p, &part.textures[0]),
            normal: textures.resolve(p, &part.textures[1]),
            has_alpha,
            alpha_flags,
            alpha_threshold,
            vertex_colors: part.colors.is_some(),
            triangles: part.triangles.len() as u32,
        });
    }
    out.bounds = [lo[0], lo[1], lo[2], hi[0], hi[1], hi[2]];
    out
}
