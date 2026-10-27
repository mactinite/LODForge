// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! The loaded world on the GPU, filled by a background loader that parses the LOD files in
//! parallel, uploads their geometry and textures, and streams them to the window as they land.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};

use rayon::prelude::*;
use wgpu::util::DeviceExt;

use crate::checks::{LoadedQuad, TreeData};
use crate::files::Files;
use crate::mesh::{Aabb, FileKind, ShapeKind, Xform, shapes_with, terrain_base};
use crate::render::textures::{TexCache, dds_gpu_bytes};
use crate::render::{Layouts, MaterialGpu, TextureBindGroup, vertices_of};
use crate::trees::{self, TreeInstance};
use crate::world::{QuadFile, QuadId, WorldFiles};

/// What to load.
#[derive(Debug, Clone)]
pub struct LoadOptions {
    /// VRAM the terrain textures may take; top mip levels are dropped per level to fit.
    pub texture_budget: u64,
    pub normal_maps: bool,
    /// Levels to load (empty: every level present).
    pub levels: Vec<i32>,
    pub terrain: bool,
    pub objects: bool,
    pub trees: bool,
    /// LODForge Runtime trees and repeated statics, when the output has them.
    pub runtime: bool,
    /// Files nearest this point (x, y) load first.
    pub start: [f32; 2],
}

impl Default for LoadOptions {
    fn default() -> Self {
        LoadOptions {
            texture_budget: 3 << 30,
            normal_maps: true,
            levels: Vec::new(),
            terrain: true,
            objects: true,
            trees: true,
            runtime: true,
            start: [0.0, 0.0],
        }
    }
}

pub struct GpuShape {
    /// Index into the quad's `data.shapes`.
    pub cpu: usize,
    pub kind: ShapeKind,
    pub indices: Range<u32>,
    pub material_index: u32,
    pub textures: Arc<TextureBindGroup>,
    pub aabb: Aabb,
    pub blend: bool,
    pub double_sided: bool,
    pub large_ref: bool,
    pub triangles: u32,
}

pub struct GpuQuad {
    pub data: LoadedQuad,
    pub vbuf: wgpu::Buffer,
    pub ibuf: wgpu::Buffer,
    pub materials: Vec<MaterialGpu>,
    pub materials_buf: wgpu::Buffer,
    pub materials_bg: wgpu::BindGroup,
    pub shapes: Vec<GpuShape>,
    pub aabb: Aabb,
    pub bytes: u64,
}

impl GpuQuad {
    /// Sets a shape's highlight tint (alpha 0 clears it).
    pub fn set_tint(&mut self, queue: &wgpu::Queue, shape: usize, tint: [f32; 4]) {
        let Some(s) = self.shapes.get(shape) else {
            return;
        };
        let i = s.material_index as usize;
        if let Some(m) = self.materials.get_mut(i) {
            m.tint = tint;
            queue.write_buffer(
                &self.materials_buf,
                (i * std::mem::size_of::<MaterialGpu>()) as u64,
                bytemuck::bytes_of(m),
            );
        }
    }
}

pub struct GpuTrees {
    pub instances: Vec<TreeInstance>,
    pub buffer: wgpu::Buffer,
    pub atlas: Arc<TextureBindGroup>,
    pub data: TreeData,
    /// Instance range of each block.
    pub blocks: Vec<(QuadId, Range<u32>)>,
    pub unknown_types: usize,
}

/// Runtime tree impostors: instances grouped by atlas page, one texture bind group per page.
pub struct GpuImpostors {
    pub buffer: wgpu::Buffer,
    pub pages: Vec<(Range<u32>, Arc<TextureBindGroup>)>,
    pub count: u32,
    pub types: usize,
}

/// A runtime mesh pack on the GPU (`TreeMeshes.bin` / `StaticMeshes.bin`).
pub struct GpuPack {
    pub vbuf: wgpu::Buffer,
    pub ibuf: wgpu::Buffer,
    pub materials_bg: wgpu::BindGroup,
    pub pages: Vec<Arc<TextureBindGroup>>,
    pub meshes: Vec<crate::runtime::PackMesh>,
    /// Model-space AABB per mesh.
    pub bounds: Vec<[f32; 6]>,
}

/// The LODForge Runtime part of a world (see [`crate::runtime`]).
#[derive(Default)]
pub struct GpuRuntime {
    pub trees: Vec<crate::runtime::Tree>,
    pub impostors: Option<GpuImpostors>,
    pub tree_pack: Option<GpuPack>,
    pub statics: Vec<crate::runtime::Static>,
    pub static_pack: Option<GpuPack>,
}

pub enum LoadEvent {
    Quad(Box<GpuQuad>),
    Trees(Box<GpuTrees>),
    Runtime(Box<GpuRuntime>),
    Message(String),
    Done,
}

/// The loaded world.
#[derive(Default)]
pub struct Scene {
    pub quads: Vec<GpuQuad>,
    pub by_id: HashMap<(QuadId, FileKind), usize>,
    pub trees: Option<GpuTrees>,
    pub runtime: Option<GpuRuntime>,
    pub present_terrain: HashSet<QuadId>,
    pub present_objects: HashSet<QuadId>,
    pub bounds: Aabb,
    pub total: usize,
    pub done: bool,
    pub messages: Vec<String>,
    pub bytes: u64,
}

impl Scene {
    pub fn new(total: usize) -> Scene {
        Scene {
            total,
            bounds: Aabb::EMPTY,
            ..Default::default()
        }
    }

    pub fn loaded(&self) -> usize {
        self.quads.len() + usize::from(self.trees.is_some()) + usize::from(self.runtime.is_some())
    }

    pub fn apply(&mut self, ev: LoadEvent) {
        match ev {
            LoadEvent::Quad(q) => {
                let id = (q.data.quad, q.data.kind);
                if q.data.error.is_none() {
                    match q.data.kind {
                        FileKind::Terrain => self.present_terrain.insert(q.data.quad),
                        _ => self.present_objects.insert(q.data.quad),
                    };
                }
                self.bounds = self.bounds.union(&q.aabb);
                self.bytes += q.bytes;
                self.by_id.insert(id, self.quads.len());
                self.quads.push(*q);
            }
            LoadEvent::Trees(t) => self.trees = Some(*t),
            LoadEvent::Runtime(t) => self.runtime = Some(*t),
            LoadEvent::Message(m) => self.messages.push(m),
            LoadEvent::Done => self.done = true,
        }
    }

    /// The loaded files, for the checks.
    pub fn loaded_quads(&self) -> Vec<&LoadedQuad> {
        self.quads.iter().map(|q| &q.data).collect()
    }

    pub fn tree_data(&self) -> TreeData {
        self.trees
            .as_ref()
            .map(|t| t.data.clone())
            .unwrap_or_default()
    }

    pub fn triangles(&self) -> usize {
        self.quads
            .iter()
            .flat_map(|q| &q.shapes)
            .map(|s| s.triangles as usize)
            .sum()
    }
}

/// Everything the loader threads need.
pub struct Loader {
    pub files: Arc<dyn Files>,
    pub world: Arc<WorldFiles>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub layouts: Layouts,
    pub cache: Arc<TexCache>,
    pub options: LoadOptions,
    /// Set to stop loading early (a world switch).
    pub cancel: Arc<AtomicBool>,
}

/// Parses one LOD file into world-space shapes (no GPU).
pub fn parse_quad(files: &dyn Files, f: &QuadFile, kind: FileKind) -> LoadedQuad {
    let mut data = LoadedQuad {
        quad: f.quad,
        path: f.path.clone(),
        kind,
        shapes: Vec::new(),
        error: None,
    };
    let base = match kind {
        FileKind::Terrain => terrain_base(f.quad.x, f.quad.y),
        _ => Xform::IDENTITY,
    };
    match files.read(&f.path) {
        None => data.error = Some("file not readable".into()),
        Some(bytes) => match lodforge_nif::parse_all_geometry(&bytes) {
            Ok(nif) => data.shapes = shapes_with(&nif, kind, base),
            Err(e) => data.error = Some(e.to_string()),
        },
    }
    data
}

/// Reads the tree LOD list and blocks (no GPU); `None` when the world has no tree LOD.
pub fn parse_trees(
    files: &dyn Files,
    w: &WorldFiles,
    messages: &mut Vec<String>,
) -> Option<TreeData> {
    if w.trees.is_empty() {
        return None;
    }
    let mut data = TreeData::default();
    match w.tree_list.as_deref().and_then(|p| files.read(p)) {
        Some(bytes) => match trees::parse_lst(&bytes) {
            Some(t) => data.types = t,
            None => {
                data.list_error = Some(format!(
                    "{}: not a tree LOD list",
                    w.tree_list.as_deref().unwrap_or("")
                ))
            }
        },
        None => data.list_error = Some(format!("tree LOD list missing for {}", w.world)),
    }
    for f in &w.trees {
        let Some(bytes) = files.read(&f.path) else {
            messages.push(format!("{}: not readable", f.path));
            continue;
        };
        let Some(refs) = trees::parse_btt(&bytes) else {
            messages.push(format!("{}: not a tree LOD block", f.path));
            continue;
        };
        data.blocks.push((f.path.clone(), refs));
    }
    Some(data)
}

/// Parses a world's LOD files in parallel without a GPU (for the checks).
pub fn load_cpu(
    files: &dyn Files,
    world: &WorldFiles,
    o: &LoadOptions,
) -> (Vec<LoadedQuad>, TreeData, Vec<String>) {
    let items = plan(world, o);
    let quads: Vec<LoadedQuad> = items
        .par_iter()
        .map(|(f, kind)| parse_quad(files, f, *kind))
        .collect();
    let mut messages = Vec::new();
    let trees = if o.trees {
        parse_trees(files, world, &mut messages).unwrap_or_default()
    } else {
        TreeData::default()
    };
    (quads, trees, messages)
}

/// Files the loader will read, in load order (coarse levels and the nearest blocks first).
fn plan(world: &WorldFiles, o: &LoadOptions) -> Vec<(QuadFile, FileKind)> {
    let want = |q: &QuadId| o.levels.is_empty() || o.levels.contains(&q.level);
    let mut v: Vec<(QuadFile, FileKind)> = Vec::new();
    if o.terrain {
        v.extend(
            world
                .terrain
                .iter()
                .filter(|f| want(&f.quad))
                .map(|f| (f.clone(), FileKind::Terrain)),
        );
    }
    if o.objects {
        v.extend(
            world
                .objects
                .iter()
                .filter(|f| want(&f.quad))
                .map(|f| (f.clone(), FileKind::Objects)),
        );
    }
    v.sort_by(|a, b| {
        b.0.quad.level.cmp(&a.0.quad.level).then_with(|| {
            a.0.quad
                .distance_xy(o.start)
                .total_cmp(&b.0.quad.distance_xy(o.start))
        })
    });
    v
}

/// Number of files the loader will stream (quads plus one for the trees).
pub fn total_items(world: &WorldFiles, o: &LoadOptions) -> usize {
    plan(world, o).len() + usize::from(o.trees && !world.trees.is_empty()) + usize::from(o.runtime && world.runtime.present())
}

/// Drops top mips of the terrain textures per level until they fit the budget.
fn plan_texture_budget(l: &Loader, items: &[(QuadFile, FileKind)]) -> Vec<String> {
    let mut notes = Vec::new();
    let mut per_level: Vec<(i32, u64, usize)> = Vec::new();
    for level in [4, 8, 16, 32] {
        let quads: Vec<&QuadFile> = items
            .iter()
            .filter(|(f, k)| *k == FileKind::Terrain && f.quad.level == level)
            .map(|(f, _)| f)
            .collect();
        let Some(first) = quads.first() else { continue };
        let (d, n) = l.world.terrain_textures(l.files.as_ref(), &first.quad);
        let mut per = l.files.read(&d).map(|b| dds_gpu_bytes(&b, 0)).unwrap_or(0);
        if l.options.normal_maps {
            per += l.files.read(&n).map(|b| dds_gpu_bytes(&b, 0)).unwrap_or(0);
        }
        per_level.push((level, per, quads.len()));
    }
    let mut drops: HashMap<i32, u32> = HashMap::new();
    let total = |drops: &HashMap<i32, u32>| {
        per_level
            .iter()
            .map(|(lv, per, n)| (per >> (2 * drops.get(lv).copied().unwrap_or(0))) * *n as u64)
            .sum::<u64>()
    };
    while total(&drops) > l.options.texture_budget {
        let Some((lv, _, _)) = per_level
            .iter()
            .filter(|(lv, _, _)| drops.get(lv).copied().unwrap_or(0) < 4)
            .max_by_key(|(lv, per, n)| {
                (per >> (2 * drops.get(lv).copied().unwrap_or(0))) * *n as u64
            })
        else {
            break;
        };
        *drops.entry(*lv).or_default() += 1;
    }
    for (lv, d) in &drops {
        let w = l.world.world.to_ascii_lowercase();
        l.cache
            .set_drop(&format!("textures\\terrain\\{w}\\{w}.{lv}."), *d);
        notes.push(format!("terrain LOD{lv} textures: dropping {d} top mip level(s) to fit the {} MB texture budget", l.options.texture_budget >> 20));
    }
    notes
}

/// Starts loading on a background thread; events arrive on the returned channel.
pub fn start(loader: Loader) -> Receiver<LoadEvent> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("lod loader".into())
        .spawn(move || {
            let items = plan(&loader.world, &loader.options);
            for n in plan_texture_budget(&loader, &items) {
                let _ = tx.send(LoadEvent::Message(n));
            }
            if loader.options.trees {
                load_trees(&loader, &tx);
            }
            if loader.options.runtime && loader.world.runtime.present() {
                load_runtime(&loader, &tx);
            }
            items.par_iter().for_each_with(tx.clone(), |tx, (f, kind)| {
                if loader.cancel.load(Ordering::Relaxed) {
                    return;
                }
                let q = load_quad(&loader, f, *kind);
                let _ = tx.send(LoadEvent::Quad(Box::new(q)));
            });
            for m in loader.cache.failed() {
                let _ = tx.send(LoadEvent::Message(format!("texture {}: {}", m.0, m.1)));
            }
            let _ = tx.send(LoadEvent::Done);
        })
        .expect("loader thread");
    rx
}

fn load_quad(l: &Loader, f: &QuadFile, kind: FileKind) -> GpuQuad {
    let mut data = parse_quad(l.files.as_ref(), f, kind);
    let level_index = match f.quad.level {
        4 => 0,
        8 => 1,
        16 => 2,
        _ => 3,
    };
    let terrain_tex =
        (kind == FileKind::Terrain).then(|| l.world.terrain_textures(l.files.as_ref(), &f.quad));
    let mut vertices = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut materials = Vec::new();
    let mut shapes = Vec::new();
    let mut aabb = Aabb::EMPTY;
    let mut tex_groups: HashMap<(Option<String>, Option<String>), Arc<TextureBindGroup>> =
        HashMap::new();
    for (i, s) in data.shapes.iter_mut().enumerate() {
        let (dpath, npath) = match (&terrain_tex, s.kind) {
            (Some((d, n)), ShapeKind::Terrain) => (Some(d.clone()), Some(n.clone())),
            (Some(_), ShapeKind::Water) => (None, None),
            _ => (s.material.diffuse.clone(), s.material.normal.clone()),
        };
        let key = (
            dpath.clone(),
            if l.options.normal_maps {
                npath.clone()
            } else {
                None
            },
        );
        let group = tex_groups
            .entry(key)
            .or_insert_with(|| {
                let d = dpath
                    .as_deref()
                    .and_then(|p| l.cache.get(l.files.as_ref(), p, true));
                let n = if l.options.normal_maps {
                    npath
                        .as_deref()
                        .and_then(|p| l.cache.get(l.files.as_ref(), p, false))
                } else {
                    None
                };
                Arc::new(
                    l.layouts
                        .texture_bind_group(&l.device, d, n, s.kind == ShapeKind::Water),
                )
            })
            .clone();
        let base = vertices.len() as u32;
        let start = indices.len() as u32;
        vertices.extend(vertices_of(s));
        indices.extend(s.indices.iter().map(|i| i + base));
        let m = MaterialGpu::of(s, level_index, group.normal.is_some());
        materials.push(m);
        aabb = aabb.union(&s.aabb);
        shapes.push(GpuShape {
            cpu: i,
            kind: s.kind,
            indices: start..indices.len() as u32,
            material_index: materials.len() as u32 - 1,
            textures: group,
            aabb: s.aabb,
            blend: s.material.alpha_blend && !s.material.alpha_test || s.kind == ShapeKind::Water,
            double_sided: s.material.double_sided || s.kind == ShapeKind::Water,
            large_ref: s.material.large_ref,
            triangles: s.triangle_count() as u32,
        });
        // The GPU has these now; the checks and picking only need positions, indices and UVs.
        s.normals = Vec::new();
        s.tangents = Vec::new();
        s.bitangents = Vec::new();
        s.colors = Vec::new();
    }
    if vertices.is_empty() {
        vertices.push(bytemuck::Zeroable::zeroed());
        indices.extend([0, 0, 0]);
    }
    if materials.is_empty() {
        materials.push(MaterialGpu::default());
    }
    let bytes =
        (vertices.len() * std::mem::size_of::<crate::render::Vertex>() + indices.len() * 4) as u64;
    let vbuf = l
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(&f.path),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
    let ibuf = l
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some(&f.path),
            contents: bytemuck::cast_slice(&indices),
            usage: wgpu::BufferUsages::INDEX,
        });
    let materials_buf = l
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("materials"),
            contents: bytemuck::cast_slice(&materials),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        });
    let materials_bg = l.layouts.materials_bind_group(&l.device, &materials_buf);
    GpuQuad {
        data,
        vbuf,
        ibuf,
        materials,
        materials_buf,
        materials_bg,
        shapes,
        aabb,
        bytes,
    }
}

fn load_trees(l: &Loader, tx: &Sender<LoadEvent>) {
    let w = &l.world;
    let mut messages = Vec::new();
    let Some(data) = parse_trees(l.files.as_ref(), w, &mut messages) else {
        return;
    };
    for m in messages {
        let _ = tx.send(LoadEvent::Message(m));
    }
    let mut instances = Vec::new();
    let mut blocks = Vec::new();
    let mut unknown_types = 0;
    let by_path: HashMap<&str, &QuadFile> = w.trees.iter().map(|f| (f.path.as_str(), f)).collect();
    for (path, refs) in &data.blocks {
        let (inst, unknown) = trees::instances(refs, &data.types);
        unknown_types += unknown;
        let start = instances.len() as u32;
        instances.extend(inst);
        if let Some(f) = by_path.get(path.as_str()) {
            blocks.push((f.quad, start..instances.len() as u32));
        }
    }
    let diffuse = w
        .tree_atlas
        .as_deref()
        .and_then(|p| l.cache.get(l.files.as_ref(), p, true));
    let normal = w
        .tree_atlas_normal
        .as_deref()
        .and_then(|p| l.cache.get(l.files.as_ref(), p, false));
    if diffuse.is_none() {
        let _ = tx.send(LoadEvent::Message(format!(
            "tree atlas missing for {} ({})",
            w.world,
            w.tree_atlas.as_deref().unwrap_or("no file")
        )));
    }
    let atlas = Arc::new(
        l.layouts
            .texture_bind_group(&l.device, diffuse, normal, false),
    );
    let contents: Vec<TreeInstance> = if instances.is_empty() {
        vec![bytemuck::Zeroable::zeroed()]
    } else {
        instances.clone()
    };
    let buffer = l
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("trees"),
            contents: bytemuck::cast_slice(&contents),
            usage: wgpu::BufferUsages::VERTEX,
        });
    let _ = tx.send(LoadEvent::Trees(Box::new(GpuTrees {
        instances,
        buffer,
        atlas,
        data,
        blocks,
        unknown_types,
    })));
}


/// Loads the worldspace's LODForge Runtime data (see [`crate::runtime`]): the trees with their
/// impostor set and mesh-tier pack, and the repeated statics with their pack.
fn load_runtime(l: &Loader, tx: &Sender<LoadEvent>) {
    use lodforge_instances::meshpack::MeshPack;
    let msg = |m: String| {
        let _ = tx.send(LoadEvent::Message(m));
    };
    let rf = &l.world.runtime;
    let w = &l.world.world;
    // The runtime drops the top mip of its impostor and static atlas pages by default
    // (`iSkipMips`, `iStaticSkipMips` 1).
    l.cache.set_drop("textures\\lodforge\\impostors\\", 1);
    l.cache.set_drop("textures\\lodforge\\staticatlas\\", 1);
    let mut rt = GpuRuntime::default();
    if let Some(path) = &rf.trees {
        match l.files.read(path).map(|b| lodforge_instances::trees::TreeFile::from_bytes(&b)) {
            Some(Ok(tf)) => {
                let index = rf.impostor_index.as_deref().and_then(|p| l.files.read(p)).and_then(|b| crate::impostors::parse_index(&b));
                if index.is_none() {
                    msg(format!("runtime: no readable impostor set {}", crate::impostors::index_path(w)));
                }
                if let Some(ix) = &index {
                    let bad = tf.types.iter().zip(&ix.types).filter(|(a, b)| !a.name.eq_ignore_ascii_case(&b.name)).count();
                    if bad > 0 || tf.types.len() != ix.types.len() {
                        msg(format!("runtime: {path}: {bad} of {} tree types do not match the impostor set ({} types)", tf.types.len(), ix.types.len()));
                    }
                }
                let center_z: Vec<f32> = index.as_ref().map(|ix| ix.types.iter().map(|t| t.bounds_center_offset[2]).collect()).unwrap_or_default();
                rt.trees = crate::runtime::trees_of(&tf, &center_z);
                if !tf.pack_meshes.is_empty() {
                    match rf.tree_pack.as_deref().and_then(|p| l.files.read(p)).map(|b| MeshPack::from_bytes(&b)) {
                        Some(Ok(p)) => rt.tree_pack = Some(upload_pack(l, &p, crate::render::kind_index(crate::mesh::ShapeKind::TreeBillboard), "tree meshes", &msg)),
                        Some(Err(e)) => msg(format!("runtime: TreeMeshes.bin: {e}")),
                        None => msg("runtime: TreeMeshes.bin missing (no mesh tier)".into()),
                    }
                }
                if rt.tree_pack.is_none() {
                    // Without the pack the impostors draw near too.
                    for t in &mut rt.trees {
                        t.mesh = lodforge_instances::meshpack::NO_MESH;
                    }
                }
                if let Some(ix) = &index {
                    rt.impostors = Some(upload_impostors(l, &rt.trees, ix, &msg));
                }
            }
            Some(Err(e)) => msg(format!("runtime: {path}: {e}")),
            None => msg(format!("runtime: {path} not readable")),
        }
    }
    if let Some(path) = &rf.statics {
        match l.files.read(path).map(|b| lodforge_instances::statics::StaticsFile::from_bytes(&b)) {
            Some(Ok(sf)) => match rf.static_pack.as_deref().and_then(|p| l.files.read(p)).map(|b| MeshPack::from_bytes(&b)) {
                Some(Ok(p)) => {
                    let pack = upload_pack(l, &p, crate::render::kind_index(crate::mesh::ShapeKind::Object), "static meshes", &msg);
                    rt.statics = crate::runtime::statics_of(&sf, &pack.bounds);
                    let missing = sf.records() - rt.statics.len();
                    if missing > 0 {
                        msg(format!("runtime: {missing} statics records have no pack mesh"));
                    }
                    rt.static_pack = Some(pack);
                }
                Some(Err(e)) => msg(format!("runtime: StaticMeshes.bin: {e}")),
                None => msg("runtime: StaticMeshes.bin missing (repeated statics not drawn)".into()),
            },
            Some(Err(e)) => msg(format!("runtime: {path}: {e}")),
            None => msg(format!("runtime: {path} not readable")),
        }
    }
    let _ = tx.send(LoadEvent::Runtime(Box::new(rt)));
}

fn upload_impostors(l: &Loader, trees: &[crate::runtime::Tree], index: &lodforge_impostor::manifest::Index, msg: &(dyn Fn(String) + Sync)) -> GpuImpostors {
    let (gpu, ranges, unknown) = crate::impostors::instances(trees, index);
    if unknown > 0 {
        msg(format!("runtime: {unknown} trees of unknown impostor types"));
    }
    // Pre-lit albedo (baker v2) is lit like a billboard, without the normal pages (as in game).
    let normals = index.types.iter().any(|t| !t.prelit);
    let pages: Vec<(Range<u32>, Arc<TextureBindGroup>)> = index
        .pages
        .par_iter()
        .zip(ranges)
        .map(|(p, r)| {
            let used = !r.is_empty();
            let albedo = used.then(|| l.cache.get(l.files.as_ref(), &p.albedo, true)).flatten();
            let nd = (used && normals).then(|| l.cache.get(l.files.as_ref(), &p.normal_depth, false)).flatten();
            if used && albedo.is_none() {
                msg(format!("runtime: impostor page {} missing ({})", p.index, p.albedo));
            }
            (r, Arc::new(l.layouts.texture_bind_group(&l.device, albedo, nd, false)))
        })
        .collect();
    let contents: Vec<crate::impostors::ImpostorInstance> = if gpu.is_empty() { vec![bytemuck::Zeroable::zeroed()] } else { gpu.clone() };
    let buffer = l.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("impostors"), contents: bytemuck::cast_slice(&contents), usage: wgpu::BufferUsages::VERTEX });
    GpuImpostors { buffer, pages, count: gpu.len() as u32, types: index.types.len() }
}

/// Uploads a mesh pack: model-space vertices, indices, one material per atlas page (alpha test
/// at the page's threshold, vertex colours, the page's snow / ash class) and the page textures.
fn upload_pack(l: &Loader, p: &lodforge_instances::meshpack::MeshPack, kind: u32, label: &str, msg: &(dyn Fn(String) + Sync)) -> GpuPack {
    use crate::render::{F_ALPHA_TEST, F_DOUBLE_SIDED, F_HAS_NORMALS, F_HAS_TANGENTS, F_NORMAL_MAP, F_VERTEX_COLORS, MaterialGpu};
    let pages: Vec<(Arc<TextureBindGroup>, bool)> = p
        .pages
        .par_iter()
        .map(|pg| {
            let d = l.cache.get(l.files.as_ref(), &pg.albedo, true);
            let n = if l.options.normal_maps { l.cache.get(l.files.as_ref(), &pg.normal, false) } else { None };
            if d.is_none() {
                msg(format!("runtime: {label} page {} missing", pg.albedo));
            }
            let has_n = n.is_some();
            (Arc::new(l.layouts.texture_bind_group(&l.device, d, n, false)), has_n)
        })
        .collect();
    let mut materials: Vec<MaterialGpu> = p
        .pages
        .iter()
        .zip(&pages)
        .map(|(pg, (_, has_n))| MaterialGpu {
            flags: F_ALPHA_TEST | F_DOUBLE_SIDED | F_VERTEX_COLORS | F_HAS_NORMALS | F_HAS_TANGENTS | if *has_n { F_NORMAL_MAP } else { 0 },
            threshold: f32::from(pg.alpha_threshold) / 255.0,
            kind_level: kind | (u32::from(pg.class.min(4)) << 16),
            ..Default::default()
        })
        .collect();
    if materials.is_empty() {
        materials.push(MaterialGpu::default());
    }
    let mut vertices = crate::runtime::pack_vertices(p);
    if vertices.is_empty() {
        vertices.push(bytemuck::Zeroable::zeroed());
    }
    let indices: Vec<u32> = if p.indices.is_empty() { vec![0, 0, 0] } else { p.indices.clone() };
    let init = |label: &str, contents: &[u8], usage| l.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(label), contents, usage });
    let vbuf = init(label, bytemuck::cast_slice(&vertices), wgpu::BufferUsages::VERTEX);
    let ibuf = init(label, bytemuck::cast_slice(&indices), wgpu::BufferUsages::INDEX);
    let materials_buf = init("pack materials", bytemuck::cast_slice(&materials), wgpu::BufferUsages::STORAGE);
    GpuPack {
        vbuf,
        ibuf,
        materials_bg: l.layouts.materials_bind_group(&l.device, &materials_buf),
        pages: pages.into_iter().map(|(t, _)| t).collect(),
        meshes: crate::runtime::pack_meshes(p),
        bounds: p.meshes.iter().map(|m| m.bounds).collect(),
    }
}