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

pub enum LoadEvent {
    Quad(Box<GpuQuad>),
    Trees(Box<GpuTrees>),
    Message(String),
    Done,
}

/// The loaded world.
#[derive(Default)]
pub struct Scene {
    pub quads: Vec<GpuQuad>,
    pub by_id: HashMap<(QuadId, FileKind), usize>,
    pub trees: Option<GpuTrees>,
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
        self.quads.len() + usize::from(self.trees.is_some())
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
    plan(world, o).len() + usize::from(o.trees && !world.trees.is_empty())
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
