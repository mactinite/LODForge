// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! Hi-to-lo detail bake for rock and mountain LOD models.
//!
//! A LOD model keeps a small share of its full model's triangles and shows the full model's
//! tiling texture, so at distance it loses the large-scale shape and shading of the rock and
//! reads as a flat, vertex-coloured slab. The bake gives the LOD model its own texture:
//!
//! 1. the LOD mesh is cut into non-overlapping planar charts (region growing within a 50 degree
//!    normal cone, a 2D overlap test per triangle), packed by their real shape on a 4-texel
//!    grid at one texel density, in a power-of-two texture (height cut to what the charts use);
//! 2. rays along the LOD surface normal find the full model under each texel (2 x 2 samples):
//!    albedo = full diffuse x vertex colours (PBR albedo x0.65, the brightness the LOD shader
//!    shows), normal = the full model's normal-mapped normal re-expressed in the LOD mesh's
//!    tangent frame, ambient occlusion = self-occlusion rays on the full model x RMAOS AO;
//! 3. AO only adds contrast: the albedo mean stays the mean of texture x vertex colour (the full
//!    model's vertex colours already carry the artist's occlusion);
//! 4. where the full model has no surface (LOD skirts, filled gaps) the texel shows what the LOD
//!    model itself shows, its tiling texture through its own UVs.
//!
//! Tangent frames follow the game: the Lighting vertex shader builds its TBN rows as
//! (bitangent field, tangent field, normal), so a normal map's x runs along the NIF bitangent
//! field and y along the tangent field (`objlod::mesh::tangents_from_uv` writes them that way).

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Result, anyhow, bail};
use glam::{Vec2, Vec3};
use lodforge_dds::mips::{self, Edge, MipKind};
use lodforge_gpu::{Bc7Quality, RgbaImage};
use lodforge_nif::{AvObject, Block, LightingShader, Nif, Node, ObjectNet, TriShape};
use lodforge_objlod::mesh::{LodMesh, MeshPart, ShaderSource};
use rayon::prelude::*;

const SLSF2_VERTEX_COLORS: u32 = 1 << 5;
/// DynDOLOD's PBR albedo scale for LOD (`pbr_linear::SCALE` in the CLI).
pub const PBR_SCALE: f32 = 0.65;
/// Supersamples per texel along each axis.
const SS: u32 = 2;
/// Packing grid cell in texels; charts keep one free cell around them (the gutter).
const BLOCK: u32 = 4;
/// Albedo alpha marker during the bake: a texel without AO (kept out of the balance).
const FIXED: f32 = 2.0;

/// Bake settings of one LOD model.
#[derive(Debug, Clone, Copy)]
pub struct Params {
    /// Target texels per world unit (lowered when the charts do not fit `max_size`).
    pub density: f32,
    pub max_size: u32,
    pub ao_rays: u32,
    /// 0..1: how much of the AO contrast reaches the albedo.
    pub ao_strength: f32,
}

impl Params {
    /// The defaults per LOD level (4, 8, 16): about 1:1 texels on a 3440 px wide screen at
    /// the distance each level starts.
    pub fn for_level(level: u8) -> Params {
        let density = match level {
            0..=4 => 0.3,
            5..=8 => 0.08,
            _ => 0.04,
        };
        Params { density, max_size: 2048, ao_rays: 48, ao_strength: 0.85 }
    }
}

// ---------------------------------------------------------------------------------------------
// Textures

/// One decoded mip level, RGBA8 as stored.
pub struct Tex {
    w: u32,
    h: u32,
    px: Vec<[u8; 4]>,
}

impl Tex {
    /// Bilinear, wrapping; 0..1 as stored (gamma space for colour).
    fn sample(&self, uv: Vec2) -> [f32; 4] {
        let x = uv.x * self.w as f32 - 0.5;
        let y = uv.y * self.h as f32 - 0.5;
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let wrap = |v: f32, n: u32| (v as i64).rem_euclid(n as i64) as usize;
        let (xa, xb) = (wrap(x0, self.w), wrap(x0 + 1.0, self.w));
        let (ya, yb) = (wrap(y0, self.h), wrap(y0 + 1.0, self.h));
        let w = self.w as usize;
        let p = |x: usize, y: usize| self.px[y * w + x];
        let (a, b, c, d) = (p(xa, ya), p(xb, ya), p(xa, yb), p(xb, yb));
        let mut o = [0.0; 4];
        for k in 0..4 {
            let f = |v: [u8; 4]| v[k] as f32 / 255.0;
            o[k] = (f(a) * (1.0 - fx) + f(b) * fx) * (1.0 - fy) + (f(c) * (1.0 - fx) + f(d) * fx) * fy;
        }
        o
    }
}

/// Texture access for the bake: a reader for Data-relative paths and a cache of decoded mip
/// levels shared by every model of a build (one rock texture serves many models).
pub struct Textures<'a> {
    read: &'a (dyn Fn(&str) -> Option<Vec<u8>> + Sync),
    exists: &'a (dyn Fn(&str) -> bool + Sync),
    cache: Mutex<HashMap<(String, u32), Option<Arc<Tex>>>>,
}

impl<'a> Textures<'a> {
    pub fn new(read: &'a (dyn Fn(&str) -> Option<Vec<u8>> + Sync), exists: &'a (dyn Fn(&str) -> bool + Sync)) -> Textures<'a> {
        Textures { read, exists, cache: Mutex::new(HashMap::new()) }
    }

    /// The smallest mip level of `path` at least `want_w` wide (the top one if none is).
    fn get(&self, path: &str, want_w: u32) -> Option<Arc<Tex>> {
        let key = (path.to_string(), want_w);
        if let Some(t) = self.cache.lock().unwrap().get(&key) {
            return t.clone();
        }
        let t = (|| {
            let b = (self.read)(path)?;
            let info = lodforge_dds::parse(&b).ok()?;
            let mut level = 0;
            while level + 1 < info.mip_count && (info.width >> (level + 1)) >= want_w.max(1) {
                level += 1;
            }
            let img = lodforge_dds::decode(&b, &info, level).ok()?;
            Some(Arc::new(Tex { w: img.width, h: img.height, px: img.rgba.chunks_exact(4).map(|p| [p[0], p[1], p[2], p[3]]).collect() }))
        })();
        self.cache.lock().unwrap().insert(key, t.clone());
        t
    }

    /// Drops the decoded textures (between worldspaces or when memory matters).
    pub fn clear(&self) {
        self.cache.lock().unwrap().clear();
    }
}

/// A texture slot as a Data-relative lower-case path (`textures\...`), `None` when empty.
pub fn texture_path(slot: &str) -> Option<String> {
    let t = slot.trim().replace('/', "\\").to_ascii_lowercase();
    if t.is_empty() {
        return None;
    }
    let t = t.strip_prefix("data\\").map(str::to_string).unwrap_or(t);
    Some(if t.starts_with("textures\\") { t } else { format!("textures\\{}", t.trim_start_matches('\\')) })
}

fn srgb_to_linear(v: f32) -> f32 {
    mips::srgb_to_linear(v.clamp(0.0, 1.0))
}

fn linear_to_srgb(v: f32) -> f32 {
    mips::linear_to_srgb(v.clamp(0.0, 1.0))
}

// ---------------------------------------------------------------------------------------------
// BVH over the full model

#[derive(Clone, Copy)]
struct BNode {
    min: Vec3,
    max: Vec3,
    /// Leaf: first triangle in `order`; inner: the right child (the left one follows the node).
    index: u32,
    /// Triangles in a leaf; 0 for inner nodes.
    count: u32,
}

struct Bvh {
    nodes: Vec<BNode>,
    order: Vec<u32>,
    tris: Vec<[Vec3; 3]>,
}

#[derive(Clone, Copy)]
struct Hit {
    t: f32,
    tri: u32,
    u: f32,
    v: f32,
}

impl Bvh {
    fn new(tris: Vec<[Vec3; 3]>) -> Bvh {
        let mut order: Vec<u32> = (0..tris.len() as u32).collect();
        let cent: Vec<Vec3> = tris.iter().map(|t| (t[0] + t[1] + t[2]) / 3.0).collect();
        let mut nodes = Vec::with_capacity(tris.len() * 2);
        if !tris.is_empty() {
            Self::build(&tris, &cent, &mut order, 0, tris.len(), &mut nodes);
        }
        Bvh { nodes, order, tris }
    }

    fn build(tris: &[[Vec3; 3]], cent: &[Vec3], order: &mut [u32], lo: usize, hi: usize, nodes: &mut Vec<BNode>) -> usize {
        let (mut mn, mut mx) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for &i in &order[lo..hi] {
            for p in tris[i as usize] {
                mn = mn.min(p);
                mx = mx.max(p);
            }
        }
        let me = nodes.len();
        nodes.push(BNode { min: mn, max: mx, index: lo as u32, count: (hi - lo) as u32 });
        if hi - lo <= 4 {
            return me;
        }
        let (mut cmn, mut cmx) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for &i in &order[lo..hi] {
            cmn = cmn.min(cent[i as usize]);
            cmx = cmx.max(cent[i as usize]);
        }
        let ext = cmx - cmn;
        let axis = if ext.x >= ext.y && ext.x >= ext.z { 0 } else if ext.y >= ext.z { 1 } else { 2 };
        let mid = (lo + hi) / 2;
        order[lo..hi].select_nth_unstable_by(mid - lo, |a, b| cent[*a as usize][axis].total_cmp(&cent[*b as usize][axis]));
        Self::build(tris, cent, order, lo, mid, nodes);
        let right = Self::build(tris, cent, order, mid, hi, nodes);
        nodes[me].index = right as u32;
        nodes[me].count = 0;
        me
    }

    fn slab(n: &BNode, o: Vec3, inv: Vec3, tmax: f32) -> bool {
        let t0 = (n.min - o) * inv;
        let t1 = (n.max - o) * inv;
        let near = t0.min(t1).max_element();
        let far = t0.max(t1).min_element();
        near <= far && far >= 0.0 && near <= tmax
    }

    fn tri_hit(t: &[Vec3; 3], o: Vec3, d: Vec3) -> Option<(f32, f32, f32)> {
        let e1 = t[1] - t[0];
        let e2 = t[2] - t[0];
        let p = d.cross(e2);
        let det = e1.dot(p);
        if det.abs() < 1e-12 {
            return None;
        }
        let inv = 1.0 / det;
        let s = o - t[0];
        let u = s.dot(p) * inv;
        if !(-1e-6..=1.0 + 1e-6).contains(&u) {
            return None;
        }
        let q = s.cross(e1);
        let v = d.dot(q) * inv;
        if v < -1e-6 || u + v > 1.0 + 1e-6 {
            return None;
        }
        Some((e2.dot(q) * inv, u, v))
    }

    /// Closest hit with `tmin <= t <= tmax` (both faces); `any` stops at the first.
    fn trace(&self, o: Vec3, d: Vec3, tmin: f32, tmax: f32, any: bool) -> Option<Hit> {
        if self.nodes.is_empty() {
            return None;
        }
        let inv = Vec3::ONE / d;
        let mut best: Option<Hit> = None;
        let mut limit = tmax;
        let mut stack = [0u32; 64];
        let mut sp = 1;
        while sp > 0 {
            sp -= 1;
            let ni = stack[sp];
            let n = &self.nodes[ni as usize];
            if !Self::slab(n, o, inv, limit) {
                continue;
            }
            if n.count > 0 {
                for k in 0..n.count {
                    let ti = self.order[(n.index + k) as usize];
                    if let Some((t, u, v)) = Self::tri_hit(&self.tris[ti as usize], o, d)
                        && t >= tmin
                        && t <= limit
                    {
                        best = Some(Hit { t, tri: ti, u, v });
                        limit = t;
                        if any {
                            return best;
                        }
                    }
                }
            } else if sp + 2 <= stack.len() {
                stack[sp] = n.index;
                stack[sp + 1] = ni + 1;
                sp += 2;
            }
        }
        best
    }
}

// ---------------------------------------------------------------------------------------------
// The full model

struct SrcShape {
    part: MeshPart,
    diffuse: Option<String>,
    normal: Option<String>,
    rmaos: Option<String>,
    pbr: bool,
    vertex_colors: bool,
    uv_scale: Vec2,
    uv_offset: Vec2,
    /// Alpha cut-off (0..1) of an alpha-tested or blended shape: texels below it are holes the
    /// bake rays pass through (moss, roots and rock-skirt overlays fade out by alpha, and the
    /// colour under their transparent texels is filler).
    cutoff: Option<f32>,
}

/// NiAlphaProperty flags of a shape -> its alpha cut-off (test threshold, or 0.5 for blending).
fn cutoff_of(alpha: Option<(u16, u8)>) -> Option<f32> {
    let (flags, threshold) = alpha?;
    if flags & 0x200 != 0 {
        Some(threshold as f32 / 255.0)
    } else if flags & 1 != 0 {
        Some(0.5)
    } else {
        None
    }
}

/// Whether a LOD model has parts the bake cannot carry over: alpha-tested or blended shapes
/// (skirts that fade into the terrain) and effect shaders. Merged into one opaque shape they
/// would turn into solid sheets.
pub fn has_cutout_parts(m: &LodMesh) -> bool {
    m.parts.iter().any(|p| matches!(p.shader, ShaderSource::Effect(_)) || cutoff_of(p.alpha).is_some())
}

/// A full model prepared for baking (its triangles in a BVH).
pub struct Source {
    shapes: Vec<SrcShape>,
    /// Per BVH triangle: shape and triangle index.
    tri_ref: Vec<(u32, u32)>,
    bvh: Bvh,
    /// The opaque shapes only (ambient occlusion rays ignore cut-out overlays).
    bvh_opaque: Bvh,
    /// Median texture repeats per world unit.
    uv_per_unit: f32,
    min: Vec3,
    max: Vec3,
}

impl Source {
    /// The lighting shapes of `full` with UVs and a diffuse texture; `None` without any.
    pub fn new(full: &LodMesh) -> Option<Source> {
        let mut shapes = Vec::new();
        for p in &full.parts {
            let ShaderSource::Lighting(l) = &p.shader else { continue };
            if p.triangles.is_empty() || p.uvs.len() != p.positions.len() {
                continue;
            }
            let diffuse = texture_path(&p.textures[0]);
            if diffuse.is_none() {
                continue;
            }
            shapes.push(SrcShape {
                pbr: diffuse.as_deref().is_some_and(|d| d.contains("\\pbr\\")),
                diffuse,
                normal: texture_path(&p.textures[1]),
                rmaos: texture_path(&p.textures[5]).filter(|t| t.ends_with("_rmaos.dds")),
                vertex_colors: l.flags2 & SLSF2_VERTEX_COLORS != 0 && p.colors.as_ref().is_some_and(|c| c.len() == p.positions.len()),
                uv_scale: Vec2::from(l.uv_scale),
                uv_offset: Vec2::from(l.uv_offset),
                cutoff: cutoff_of(p.alpha),
                part: p.clone(),
            });
        }
        if shapes.is_empty() {
            return None;
        }
        let mut tris = Vec::new();
        let mut tri_ref = Vec::new();
        let mut ratios = Vec::new();
        let (mut mn, mut mx) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for (si, s) in shapes.iter().enumerate() {
            let p = &s.part;
            for (k, t) in p.triangles.iter().enumerate() {
                let v = t.map(|i| Vec3::from(p.positions[i as usize]));
                for q in v {
                    mn = mn.min(q);
                    mx = mx.max(q);
                }
                let wa = (v[1] - v[0]).cross(v[2] - v[0]).length();
                let u = t.map(|i| Vec2::from(p.uvs[i as usize]) * s.uv_scale);
                let ua = (u[1] - u[0]).perp_dot(u[2] - u[0]).abs();
                if wa > 1e-3 && ua > 0.0 {
                    ratios.push((ua / wa).sqrt());
                }
                tris.push(v);
                tri_ref.push((si as u32, k as u32));
            }
        }
        ratios.sort_by(f32::total_cmp);
        let uv_per_unit = ratios.get(ratios.len() / 2).copied().unwrap_or(1.0 / 1024.0);
        let opaque: Vec<[Vec3; 3]> = tris.iter().zip(&tri_ref).filter(|(_, (si, _))| shapes[*si as usize].cutoff.is_none()).map(|(t, _)| *t).collect();
        Some(Source { shapes, tri_ref, bvh: Bvh::new(tris), bvh_opaque: Bvh::new(opaque), uv_per_unit, min: mn, max: mx })
    }

    /// Diffuse textures of the model's shapes (lower case, `textures\...`).
    pub fn diffuses(&self) -> Vec<String> {
        self.shapes.iter().filter_map(|s| s.diffuse.clone()).collect()
    }

    /// Whether most of the model's surface uses PBR textures (Community Shaders lights it
    /// with its PBR path, which also draws directional snow darker and without the vanilla snow
    /// sparkle).
    pub fn is_pbr(&self) -> bool {
        let pbr = self.tri_ref.iter().filter(|(si, _)| self.shapes[*si as usize].pbr).count();
        pbr * 2 > self.tri_ref.len()
    }

    pub fn triangles(&self) -> usize {
        self.tri_ref.len()
    }
}

// ---------------------------------------------------------------------------------------------
// The LOD model and its unwrap

struct Lod {
    pos: Vec<Vec3>,
    nrm: Vec<Vec3>,
    alpha: Vec<u8>,
    uv: Vec<Vec2>,
    tan: Vec<Vec3>,
    bit: Vec<Vec3>,
    rgb: Vec<[f32; 3]>,
    tris: Vec<[u32; 3]>,
    shader: Option<LightingShader>,
    textures: Option<(String, String)>,
}

impl Lod {
    fn new(m: &LodMesh) -> Lod {
        let mut l = Lod { pos: vec![], nrm: vec![], alpha: vec![], uv: vec![], tan: vec![], bit: vec![], rgb: vec![], tris: vec![], shader: None, textures: None };
        for p in &m.parts {
            let ShaderSource::Lighting(sh) = &p.shader else { continue };
            if p.triangles.is_empty() {
                continue;
            }
            let base = l.pos.len() as u32;
            let n = p.positions.len();
            if l.shader.is_none() {
                l.shader = Some(sh.clone());
                l.textures = texture_path(&p.textures[0]).map(|d| (d, texture_path(&p.textures[1]).unwrap_or_default()));
            }
            let (sc, off) = (Vec2::from(sh.uv_scale), Vec2::from(sh.uv_offset));
            l.pos.extend(p.positions.iter().map(|v| Vec3::from(*v)));
            l.nrm.extend(p.normals.iter().map(|v| Vec3::from(*v).normalize_or(Vec3::Z)));
            l.uv.extend((0..n).map(|i| p.uvs.get(i).map_or(Vec2::ZERO, |u| Vec2::from(*u) * sc + off)));
            l.tan.extend(p.tangents.iter().map(|v| Vec3::from(*v)));
            l.bit.extend(p.bitangents.iter().map(|v| Vec3::from(*v)));
            let vc = sh.flags2 & SLSF2_VERTEX_COLORS != 0;
            let colors = p.colors.as_deref();
            l.rgb.extend((0..n).map(|i| match colors.and_then(|c| c.get(i)) {
                Some(c) if vc => [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0],
                _ => [1.0; 3],
            }));
            l.alpha.extend((0..n).map(|i| colors.and_then(|c| c.get(i)).map_or(255, |c| c[3])));
            l.tris.extend(p.triangles.iter().map(|t| [u32::from(t[0]) + base, u32::from(t[1]) + base, u32::from(t[2]) + base]));
        }
        l
    }

    fn uv_per_unit(&self) -> f32 {
        let mut r: Vec<f32> = self
            .tris
            .iter()
            .filter_map(|t| {
                let p = t.map(|i| self.pos[i as usize]);
                let u = t.map(|i| self.uv[i as usize]);
                let wa = (p[1] - p[0]).cross(p[2] - p[0]).length();
                let ua = (u[1] - u[0]).perp_dot(u[2] - u[0]).abs();
                (wa > 1e-3 && ua > 0.0).then(|| (ua / wa).sqrt())
            })
            .collect();
        r.sort_by(f32::total_cmp);
        r.get(r.len() / 2).copied().unwrap_or(1.0 / 1024.0)
    }
}

struct Unwrap {
    /// Output vertices: source LOD vertex and UV.
    verts: Vec<(u32, Vec2)>,
    tris: Vec<[u32; 3]>,
    width: u32,
    height: u32,
    density: f32,
    charts: usize,
}

type Chart = Vec<(usize, [Vec2; 3])>;

fn basis(n: Vec3) -> (Vec3, Vec3) {
    let a = if n.z.abs() < 0.9 { Vec3::Z } else { Vec3::X };
    let e1 = a.cross(n).normalize();
    (e1, n.cross(e1))
}

fn overlap(a: &[Vec2; 3], b: &[Vec2; 3], eps: f32) -> bool {
    for tri in [a, b] {
        for i in 0..3 {
            let e = tri[(i + 1) % 3] - tri[i];
            let n = Vec2::new(-e.y, e.x).normalize_or_zero();
            if n == Vec2::ZERO {
                continue;
            }
            let pa = a.map(|p| p.dot(n));
            let pb = b.map(|p| p.dot(n));
            let (amn, amx) = (pa.iter().copied().fold(f32::MAX, f32::min), pa.iter().copied().fold(f32::MIN, f32::max));
            let (bmn, bmx) = (pb.iter().copied().fold(f32::MAX, f32::min), pb.iter().copied().fold(f32::MIN, f32::max));
            if amx <= bmn + eps || bmx <= amn + eps {
                return false;
            }
        }
    }
    true
}

#[derive(Clone, Copy)]
struct Place {
    x: u32,
    y: u32,
    turn: bool,
}

fn chart_point(p: Vec2, e: Vec2, turn: bool) -> Vec2 {
    if turn { Vec2::new(e.y - p.y, p.x) } else { p }
}

/// The cells a chart covers at density `d`, grown by one cell: width, height, cells. Cell
/// `(1, 1)` holds the chart origin.
fn chart_mask(c: &Chart, e: Vec2, d: f32, turn: bool) -> (usize, usize, Vec<(u16, u16)>) {
    let ext = if turn { Vec2::new(e.y, e.x) } else { e };
    let s = d / BLOCK as f32;
    let w = (ext.x * s).ceil() as usize + 3;
    let h = (ext.y * s).ceil() as usize + 3;
    let mut core = vec![false; w * h];
    let mark = |x: f32, y: f32, core: &mut Vec<bool>| {
        let (cx, cy) = (x.floor().clamp(0.0, (w - 1) as f32) as usize, y.floor().clamp(0.0, (h - 1) as f32) as usize);
        core[cy * w + cx] = true;
    };
    for (_, t) in c {
        let q = t.map(|p| chart_point(p, e, turn) * s + Vec2::ONE);
        for p in q {
            mark(p.x, p.y, &mut core);
        }
        let area = (q[1] - q[0]).perp_dot(q[2] - q[0]);
        if area.abs() < 1e-9 {
            continue;
        }
        let (mn, mx) = (q[0].min(q[1]).min(q[2]), q[0].max(q[1]).max(q[2]));
        for sy in (mn.y * 4.0).floor() as i32..=(mx.y * 4.0).ceil() as i32 {
            for sx in (mn.x * 4.0).floor() as i32..=(mx.x * 4.0).ceil() as i32 {
                let p = Vec2::new((sx as f32 + 0.5) / 4.0, (sy as f32 + 0.5) / 4.0);
                let w1 = (p - q[0]).perp_dot(q[2] - q[0]) / area;
                let w2 = (q[1] - q[0]).perp_dot(p - q[0]) / area;
                if w1 >= -0.01 && w2 >= -0.01 && w1 + w2 <= 1.01 {
                    mark(p.x, p.y, &mut core);
                }
            }
        }
    }
    let mut cells = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let near = (y.saturating_sub(1)..=(y + 1).min(h - 1)).any(|yy| (x.saturating_sub(1)..=(x + 1).min(w - 1)).any(|xx| core[yy * w + xx]));
            if near {
                cells.push((x as u16, y as u16));
            }
        }
    }
    (w, h, cells)
}

/// Packs the charts' real shapes into a `gw` x `gh` cell grid: biggest first, each at its
/// lowest, then leftmost free position, turned by 90 degrees when that sits lower.
fn pack_shapes(charts: &[Chart], extents: &[Vec2], d: f32, gw: u32, gh: u32) -> Option<(Vec<Place>, u32)> {
    let (gw, gh) = (gw as usize, gh as usize);
    let masks: Vec<[(usize, usize, Vec<(u16, u16)>); 2]> = charts.iter().zip(extents).map(|(c, e)| [chart_mask(c, *e, d, false), chart_mask(c, *e, d, true)]).collect();
    let mut order: Vec<usize> = (0..charts.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse(masks[i][0].2.len()));
    let mut grid = vec![false; gw * gh];
    let mut out = vec![Place { x: 0, y: 0, turn: false }; charts.len()];
    let mut used = 0;
    for ci in order {
        let mut best: Option<(usize, usize, usize)> = None;
        for (ti, (mw, mh, cells)) in masks[ci].iter().enumerate() {
            if *mw > gw || *mh > gh {
                continue;
            }
            'search: for y in 0..=gh - mh {
                if best.is_some_and(|b| b.0 <= y) {
                    break;
                }
                for x in 0..=gw - mw {
                    if cells.iter().all(|&(cx, cy)| !grid[(y + cy as usize) * gw + x + cx as usize]) {
                        best = Some((y, x, ti));
                        break 'search;
                    }
                }
            }
        }
        let (y, x, ti) = best?;
        let (_, mh, cells) = &masks[ci][ti];
        for &(cx, cy) in cells {
            grid[(y + cy as usize) * gw + x + cx as usize] = true;
        }
        out[ci] = Place { x: x as u32, y: y as u32, turn: ti == 1 };
        used = used.max(y + mh);
    }
    Some((out, used as u32))
}

fn unwrap(m: &Lod, density: f32, max_size: u32) -> Unwrap {
    let nf = m.tris.len();
    let mut weld: HashMap<[i64; 3], u32> = HashMap::new();
    let wid: Vec<u32> = m
        .pos
        .iter()
        .map(|p| {
            let k = [(p.x * 10.0).round() as i64, (p.y * 10.0).round() as i64, (p.z * 10.0).round() as i64];
            let n = weld.len() as u32;
            *weld.entry(k).or_insert(n)
        })
        .collect();
    let mut edges: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    for (f, t) in m.tris.iter().enumerate() {
        for k in 0..3 {
            let (a, b) = (wid[t[k] as usize], wid[t[(k + 1) % 3] as usize]);
            edges.entry((a.min(b), a.max(b))).or_default().push(f);
        }
    }
    let fnorm: Vec<Vec3> = m.tris.iter().map(|t| (m.pos[t[1] as usize] - m.pos[t[0] as usize]).cross(m.pos[t[2] as usize] - m.pos[t[0] as usize])).collect();
    let mut order: Vec<usize> = (0..nf).collect();
    order.sort_by(|a, b| fnorm[*b].length().total_cmp(&fnorm[*a].length()));
    let cos_limit = 50f32.to_radians().cos();
    let mut chart_of = vec![usize::MAX; nf];
    let mut charts: Vec<Chart> = Vec::new();
    for &seed in &order {
        if chart_of[seed] != usize::MAX {
            continue;
        }
        let n0 = fnorm[seed].normalize_or(Vec3::Z);
        let (e1, e2) = basis(n0);
        let proj = |f: usize| m.tris[f].map(|i| Vec2::new(m.pos[i as usize].dot(e1), m.pos[i as usize].dot(e2)));
        let ci = charts.len();
        let mut faces = vec![(seed, proj(seed))];
        chart_of[seed] = ci;
        let mut queue: VecDeque<usize> = VecDeque::new();
        let push_nb = |f: usize, q: &mut VecDeque<usize>| {
            let t = m.tris[f];
            for k in 0..3 {
                let (a, b) = (wid[t[k] as usize], wid[t[(k + 1) % 3] as usize]);
                for &g in &edges[&(a.min(b), a.max(b))] {
                    q.push_back(g);
                }
            }
        };
        push_nb(seed, &mut queue);
        let mut tries = 0;
        while let Some(f) = queue.pop_front() {
            tries += 1;
            if chart_of[f] != usize::MAX || tries > nf * 8 {
                continue;
            }
            if fnorm[f].normalize_or(Vec3::Z).dot(n0) < cos_limit {
                continue;
            }
            let p = proj(f);
            if (p[1] - p[0]).perp_dot(p[2] - p[0]) <= 1e-4 {
                continue;
            }
            if faces.iter().any(|(_, q)| overlap(&p, q, 0.01)) {
                continue;
            }
            chart_of[f] = ci;
            faces.push((f, p));
            push_nb(f, &mut queue);
        }
        charts.push(faces);
    }
    // Each chart turned to its smallest bounding box and moved to the origin.
    let mut extents = Vec::with_capacity(charts.len());
    for c in &mut charts {
        let mut best = (f32::MAX, 0f32);
        for step in 0..30 {
            let a = (step as f32 * 3.0).to_radians();
            let r = Vec2::from_angle(a);
            let (mut mn, mut mx) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
            for (_, p) in c.iter() {
                for q in p {
                    let q = r.rotate(*q);
                    mn = mn.min(q);
                    mx = mx.max(q);
                }
            }
            let area = (mx - mn).x * (mx - mn).y;
            if area < best.0 {
                best = (area, a);
            }
        }
        let r = Vec2::from_angle(best.1);
        let (mut mn, mut mx) = (Vec2::splat(f32::MAX), Vec2::splat(f32::MIN));
        for (_, p) in c.iter_mut() {
            for q in p.iter_mut() {
                *q = r.rotate(*q);
                mn = mn.min(*q);
                mx = mx.max(*q);
            }
        }
        for (_, p) in c.iter_mut() {
            for q in p.iter_mut() {
                *q -= mn;
            }
        }
        extents.push(mx - mn);
    }
    // Texture size and density: grow the square, then lower the density at the cap.
    let mut d = density;
    let mut size = 32;
    let (placed, used) = loop {
        if let Some(p) = pack_shapes(&charts, &extents, d, size / BLOCK, size / BLOCK) {
            break p;
        }
        if size < max_size {
            size *= 2;
            continue;
        }
        let (mut lo, mut hi) = (0.0f32, d);
        for _ in 0..12 {
            let mid = (lo + hi) * 0.5;
            if pack_shapes(&charts, &extents, mid, size / BLOCK, size / BLOCK).is_some() { lo = mid } else { hi = mid }
        }
        d = lo;
        match pack_shapes(&charts, &extents, d, size / BLOCK, size / BLOCK) {
            Some(p) => break p,
            None => break (vec![Place { x: 0, y: 0, turn: false }; charts.len()], size / BLOCK),
        }
    };
    let height = (used * BLOCK).next_power_of_two().max(4);
    let mut verts = Vec::new();
    let mut vmap: HashMap<(usize, u32), u32> = HashMap::new();
    let mut tris = vec![[0u32; 3]; nf];
    for (ci, c) in charts.iter().enumerate() {
        let pl = placed[ci];
        let off = Vec2::new(((pl.x + 1) * BLOCK) as f32, ((pl.y + 1) * BLOCK) as f32);
        for (f, p) in c {
            for k in 0..3 {
                let src = m.tris[*f][k];
                let uv = (off + chart_point(p[k], extents[ci], pl.turn) * d) / Vec2::new(size as f32, height as f32);
                let id = *vmap.entry((ci, src)).or_insert_with(|| {
                    verts.push((src, uv));
                    (verts.len() - 1) as u32
                });
                tris[*f][k] = id;
            }
        }
    }
    Unwrap { verts, tris, width: size, height, density: d, charts: charts.len() }
}

// ---------------------------------------------------------------------------------------------
// The bake

/// A baked LOD model.
pub struct Baked {
    /// The LOD model with its own UVs and tangent frame, white vertex RGB (alpha kept: the snow
    /// amount of `objSnow*` shapes), textures `out_diffuse` / `out_normal`.
    pub nif: Vec<u8>,
    /// BC7 DDS with full mips.
    pub diffuse: Vec<u8>,
    pub normal: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub density: f32,
    pub charts: usize,
    pub triangles: usize,
    pub samples: usize,
    /// Samples without a full-model surface (filled from the LOD model's own texture).
    pub misses: usize,
    pub ao_balance: f32,
    pub ms: u128,
}

impl Baked {
    /// Bytes on the GPU at full resolution (BC7 diffuse + normal with mips).
    pub fn gpu_bytes(&self) -> u64 {
        (self.diffuse.len() + self.normal.len()).saturating_sub(2 * 148) as u64
    }
}

fn hash(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^ (x >> 16)
}

fn rnd(a: u32, b: u32) -> f32 {
    (hash(a.wrapping_mul(0x9e37_79b9) ^ hash(b)) >> 8) as f32 / (1u32 << 24) as f32
}

/// The tiling textures a LOD model shows: its own diffuse (the PBR twin's `linear` LOD copy
/// when there is one) and normal map (the PBR one when there is one).
fn lod_textures(tex: &Textures<'_>, l: &Lod) -> Option<(String, String)> {
    let (d, n) = l.textures.clone()?;
    let pbr = |p: &str| -> String { if p.contains("\\pbr\\") { p.to_string() } else { p.replacen("textures\\", "textures\\pbr\\", 1) } };
    let dl = pbr(&d).replace(".dds", "linear.dds");
    let d = if (tex.exists)(&dl) { dl } else { d };
    let n = if !n.is_empty() && (tex.exists)(&pbr(&n)) { pbr(&n) } else { n };
    Some((d, n))
}

struct Surf {
    albedo: [f32; 3],
    normal: Vec3,
    geo: Vec3,
    pos: Vec3,
    mat_ao: f32,
}

/// Bakes `lod` (a LOD model of `src`'s full model). The NIF names `out_diffuse` / `out_normal`
/// (Data-relative) as its textures.
pub fn bake(src: &Source, lod_mesh: &LodMesh, tex: &Textures<'_>, p: &Params, out_diffuse: &str, out_normal: &str) -> Result<Baked> {
    let t0 = Instant::now();
    let lod = Lod::new(lod_mesh);
    if lod.tris.is_empty() {
        bail!("no lighting shapes");
    }
    if lod.pos.len() > 60_000 {
        bail!("too many vertices ({})", lod.pos.len());
    }
    let ext = src.max - src.min;
    let diag = ext.length();
    let ao_radius = (diag * 0.08).clamp(128.0, 1024.0);
    let search = (diag * 0.06).clamp(64.0, 512.0);
    let uw = unwrap(&lod, p.density, p.max_size);
    // Output mesh attributes; the tangent frame in the Bethesda layout.
    let pos: Vec<[f32; 3]> = uw.verts.iter().map(|(s, _)| lod.pos[*s as usize].to_array()).collect();
    let nrm: Vec<[f32; 3]> = uw.verts.iter().map(|(s, _)| lod.nrm[*s as usize].to_array()).collect();
    let uv: Vec<[f32; 2]> = uw.verts.iter().map(|(_, u)| u.to_array()).collect();
    let tris16: Vec<[u16; 3]> = uw.tris.iter().map(|t| [t[0] as u16, t[1] as u16, t[2] as u16]).collect();
    let (tan, bit) = lodforge_objlod::mesh::tangents_from_uv(&pos, &uv, &nrm, &tris16);

    // Source textures at the mip a 2x2-supersampled bake at this density needs.
    let want = ((uw.density * SS as f32 / src.uv_per_unit).ceil() as u32).next_power_of_two();
    struct ShapeTex {
        diffuse: Option<Arc<Tex>>,
        normal: Option<Arc<Tex>>,
        rmaos: Option<Arc<Tex>>,
    }
    let stex: Vec<ShapeTex> = src
        .shapes
        .iter()
        .map(|s| ShapeTex {
            diffuse: s.diffuse.as_deref().and_then(|d| tex.get(d, want)),
            normal: s.normal.as_deref().and_then(|d| tex.get(d, want)),
            rmaos: s.rmaos.as_deref().and_then(|d| tex.get(d, want)),
        })
        .collect();
    if stex.iter().all(|t| t.diffuse.is_none()) {
        bail!("full model textures not found");
    }
    let fb_want = ((uw.density * SS as f32 / lod.uv_per_unit()).ceil() as u32).next_power_of_two();
    let fallback = lod_textures(tex, &lod).map(|(d, n)| (tex.get(&d, fb_want), if n.is_empty() { None } else { tex.get(&n, fb_want) }));
    let (fb_d, fb_n) = match &fallback {
        Some((d, n)) => (d.clone(), n.clone()),
        None => (None, None),
    };

    let (w, h) = (uw.width as usize, uw.height as usize);
    let (sw, sh) = (w * SS as usize, h * SS as usize);
    // Output triangles rasterised into the supersample grid: (triangle, barycentrics).
    let mut cover: Vec<Option<(u32, f32, f32)>> = vec![None; sw * sh];
    for (ti, t) in uw.tris.iter().enumerate() {
        let q = t.map(|i| uw.verts[i as usize].1 * Vec2::new(sw as f32, sh as f32));
        let (mn, mx) = (q[0].min(q[1]).min(q[2]), q[0].max(q[1]).max(q[2]));
        let area = (q[1] - q[0]).perp_dot(q[2] - q[0]);
        if area.abs() < 1e-9 {
            continue;
        }
        for y in (mn.y.floor().max(0.0) as usize)..=(mx.y.ceil() as usize).min(sh - 1) {
            for x in (mn.x.floor().max(0.0) as usize)..=(mx.x.ceil() as usize).min(sw - 1) {
                let c = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                let w1 = (c - q[0]).perp_dot(q[2] - q[0]) / area;
                let w2 = (q[1] - q[0]).perp_dot(c - q[0]) / area;
                if 1.0 - w1 - w2 >= -1e-4 && w1 >= -1e-4 && w2 >= -1e-4 {
                    cover[y * sw + x] = Some((ti as u32, w1, w2));
                }
            }
        }
    }
    // Interpolated output frame (position, normal, tangent field, bitangent field) and the LOD
    // model's own attributes (UV, frame, vertex colour) at a point of an output triangle.
    let frame = |ti: u32, w1: f32, w2: f32| -> (Vec3, Vec3, Vec3, Vec3) {
        let t = uw.tris[ti as usize];
        let wt = [1.0 - w1 - w2, w1, w2];
        let (mut p, mut n, mut tt, mut bb) = (Vec3::ZERO, Vec3::ZERO, Vec3::ZERO, Vec3::ZERO);
        for k in 0..3 {
            let o = t[k] as usize;
            p += Vec3::from(pos[o]) * wt[k];
            n += Vec3::from(nrm[o]) * wt[k];
            tt += Vec3::from(tan[o]) * wt[k];
            bb += Vec3::from(bit[o]) * wt[k];
        }
        (p, n.normalize_or(Vec3::Z), tt.normalize_or(Vec3::Y), bb.normalize_or(Vec3::X))
    };
    let orig = |ti: u32, w1: f32, w2: f32| -> (Vec2, Vec3, Vec3, Vec3, [f32; 3]) {
        let t = uw.tris[ti as usize];
        let wt = [1.0 - w1 - w2, w1, w2];
        let (mut u, mut tt, mut bb, mut nn, mut vc) = (Vec2::ZERO, Vec3::ZERO, Vec3::ZERO, Vec3::ZERO, [0f32; 3]);
        for k in 0..3 {
            let s = uw.verts[t[k] as usize].0 as usize;
            u += lod.uv[s] * wt[k];
            tt += lod.tan.get(s).copied().unwrap_or(Vec3::Y) * wt[k];
            bb += lod.bit.get(s).copied().unwrap_or(Vec3::X) * wt[k];
            nn += lod.nrm[s] * wt[k];
            for c in 0..3 {
                vc[c] += lod.rgb[s][c] * wt[k];
            }
        }
        (u, tt.normalize_or(Vec3::Y), bb.normalize_or(Vec3::X), nn.normalize_or(Vec3::Z), vc)
    };
    let surf = |hit: &Hit, o: Vec3, d: Vec3| -> Surf {
        let (si, ti) = src.tri_ref[hit.tri as usize];
        let s = &src.shapes[si as usize];
        let st = &stex[si as usize];
        let part = &s.part;
        let idx = part.triangles[ti as usize].map(usize::from);
        let wt = [1.0 - hit.u - hit.v, hit.u, hit.v];
        let lerp3 = |v: &[[f32; 3]]| -> Option<Vec3> { (v.len() == part.positions.len()).then(|| idx.iter().zip(wt).map(|(&i, w)| Vec3::from(v[i]) * w).sum()) };
        let tri = &src.bvh.tris[hit.tri as usize];
        let mut geo = (tri[1] - tri[0]).cross(tri[2] - tri[0]).normalize_or(Vec3::Z);
        let ns = lerp3(&part.normals).map(|n| n.normalize_or(geo)).unwrap_or(geo);
        if geo.dot(ns) < 0.0 {
            geo = -geo;
        }
        let uv: Vec2 = idx.iter().zip(wt).map(|(&i, w)| Vec2::from(part.uvs[i]) * w).sum::<Vec2>() * s.uv_scale + s.uv_offset;
        let mut albedo = st.diffuse.as_ref().map_or([0.5; 3], |t| {
            let c = t.sample(uv);
            [c[0], c[1], c[2]]
        });
        if s.vertex_colors
            && let Some(cols) = &part.colors
        {
            for k in 0..3 {
                albedo[k] *= idx.iter().zip(wt).map(|(&i, w)| cols[i][k] as f32 / 255.0 * w).sum::<f32>();
            }
        }
        if s.pbr {
            for c in &mut albedo {
                *c *= PBR_SCALE;
            }
        }
        let mut normal = ns;
        if let (Some(nt), Some(t), Some(b)) = (st.normal.as_ref(), lerp3(&part.tangents), lerp3(&part.bitangents)) {
            let c = nt.sample(uv);
            let v = Vec3::new(c[0] * 2.0 - 1.0, c[1] * 2.0 - 1.0, c[2] * 2.0 - 1.0);
            // The game's frame: x along the bitangent field, y along the tangent field.
            normal = (b.normalize_or(Vec3::X) * v.x + t.normalize_or(Vec3::Y) * v.y + ns * v.z).normalize_or(ns);
        }
        let mat_ao = st.rmaos.as_ref().map_or(1.0, |t| t.sample(uv)[2]);
        Surf { albedo, normal, geo, pos: o + d * hit.t, mat_ao }
    };
    // The closest hit that is not a hole: texels of alpha-tested / blended shapes below their
    // cut-off are passed through (up to 8 layers).
    let trace_solid = |o: Vec3, d: Vec3, tmax: f32| -> Option<Hit> {
        let mut tmin = 0.0;
        for _ in 0..8 {
            let hit = src.bvh.trace(o, d, tmin, tmax, false)?;
            let (si, ti) = src.tri_ref[hit.tri as usize];
            let s = &src.shapes[si as usize];
            let Some(cut) = s.cutoff else { return Some(hit) };
            let Some(tex) = stex[si as usize].diffuse.as_ref() else { return Some(hit) };
            let idx = s.part.triangles[ti as usize].map(usize::from);
            let wt = [1.0 - hit.u - hit.v, hit.u, hit.v];
            let uv: Vec2 = idx.iter().zip(wt).map(|(&i, w)| Vec2::from(s.part.uvs[i]) * w).sum::<Vec2>() * s.uv_scale + s.uv_offset;
            if tex.sample(uv)[3] >= cut {
                return Some(hit);
            }
            tmin = hit.t + 0.01;
        }
        None
    };
    let find = |p: Vec3, n: Vec3| -> Option<(Hit, Vec3, Vec3)> {
        let up = trace_solid(p, n, search);
        let down = trace_solid(p, -n, search);
        match (up, down) {
            (Some(a), Some(b)) => Some(if a.t <= b.t { (a, p, n) } else { (b, p, -n) }),
            (Some(a), None) => Some((a, p, n)),
            (None, Some(b)) => Some((b, p, -n)),
            (None, None) => None,
        }
    };
    type Row = (Vec<[f32; 4]>, Vec<[f32; 4]>, usize, usize);
    let rows: Vec<Row> = (0..h)
        .into_par_iter()
        .map(|y| {
            let mut alb = vec![[0.0; 4]; w];
            let mut nrm_out = vec![[0.0; 4]; w];
            let (mut misses, mut samples) = (0, 0);
            for x in 0..w {
                let (mut acc_a, mut acc_n, mut cnt) = ([0.0f32; 3], Vec3::ZERO, 0u32);
                let mut ao_at: Option<Surf> = None;
                let mut any_cover = false;
                for sy in 0..SS as usize {
                    for sx in 0..SS as usize {
                        let Some((ti, w1, w2)) = cover[(y * SS as usize + sy) * sw + x * SS as usize + sx] else { continue };
                        any_cover = true;
                        samples += 1;
                        let (fp, fnn, ft, fb) = frame(ti, w1, w2);
                        // Encoded for the game's frame: x along the bitangent field, y along the tangent field.
                        let enc = |v: Vec3| Vec3::new(v.dot(fb), v.dot(ft), v.dot(fnn));
                        let Some((hit, o, d)) = find(fp, fnn) else {
                            misses += 1;
                            if let Some(fd) = &fb_d {
                                let (u, t, b, n, vc) = orig(ti, w1, w2);
                                let c = fd.sample(u);
                                for k in 0..3 {
                                    acc_a[k] += srgb_to_linear(c[k] * vc[k]);
                                }
                                let wn = fb_n.as_ref().map_or(n, |nt| {
                                    let c = nt.sample(u);
                                    (b * (c[0] * 2.0 - 1.0) + t * (c[1] * 2.0 - 1.0) + n * (c[2] * 2.0 - 1.0)).normalize_or(n)
                                });
                                acc_n += enc(wn);
                                cnt += 1;
                            }
                            continue;
                        };
                        let s = surf(&hit, o, d);
                        for k in 0..3 {
                            acc_a[k] += srgb_to_linear(s.albedo[k]);
                        }
                        acc_n += enc(s.normal);
                        cnt += 1;
                        if ao_at.is_none() {
                            ao_at = Some(s);
                        }
                    }
                }
                if cnt == 0 {
                    if any_cover {
                        alb[x] = [0.2, 0.2, 0.2, FIXED];
                        nrm_out[x] = [0.0, 0.0, 1.0, 1.0];
                    }
                    continue;
                }
                let n = acc_n.normalize_or(Vec3::Z);
                nrm_out[x] = [n.x, n.y, n.z, 1.0];
                let mean = [acc_a[0] / cnt as f32, acc_a[1] / cnt as f32, acc_a[2] / cnt as f32];
                let Some(s) = ao_at else {
                    // Fallback only: no AO, kept out of the brightness balance.
                    alb[x] = [mean[0], mean[1], mean[2], FIXED];
                    continue;
                };
                // Ambient occlusion: cosine-weighted rays over the full model's surface.
                let (e1, e2) = basis(s.geo);
                let mut open = 0.0;
                let id = (y * w + x) as u32;
                for k in 0..p.ao_rays {
                    let r1 = (k as f32 + rnd(id, k)) / p.ao_rays as f32;
                    let r2 = rnd(k ^ 0x5bd1_e995, id);
                    let phi = std::f32::consts::TAU * r2;
                    let rad = r1.sqrt();
                    let dir = (e1 * (rad * phi.cos()) + e2 * (rad * phi.sin()) + s.geo * (1.0 - r1).max(0.0).sqrt()).normalize();
                    match src.bvh_opaque.trace(s.pos + s.geo * 0.5, dir, 0.25, ao_radius, true) {
                        Some(hh) => open += (hh.t / ao_radius).powi(2),
                        None => open += 1.0,
                    }
                }
                let total = open / p.ao_rays as f32 * s.mat_ao;
                // Alpha carries the AO factor until the brightness balance below.
                alb[x] = [mean[0], mean[1], mean[2], 1.0 - p.ao_strength + p.ao_strength * total];
            }
            (alb, nrm_out, misses, samples)
        })
        .collect();
    let mut alb: Vec<[f32; 4]> = Vec::with_capacity(w * h);
    let mut nrm_img: Vec<[f32; 4]> = Vec::with_capacity(w * h);
    let (mut misses, mut samples) = (0, 0);
    for (a, n, m, s) in rows {
        alb.extend(a);
        nrm_img.extend(n);
        misses += m;
        samples += s;
    }
    // AO redistributes light: the mean of texture x vertex colour stays (linear luminance).
    let lum = |q: &[f32; 4]| 0.2126 * q[0] + 0.7152 * q[1] + 0.0722 * q[2];
    let (mut m0, mut m1) = (0f64, 0f64);
    for q in alb.iter().filter(|q| q[3] > 0.0 && q[3] < FIXED) {
        m0 += lum(q) as f64;
        m1 += (lum(q) * q[3]) as f64;
    }
    let balance = if m1 > 0.0 { (m0 / m1) as f32 } else { 1.0 };
    for q in alb.iter_mut().filter(|q| q[3] > 0.0) {
        let f = if q[3] >= FIXED { 1.0 } else { q[3] * balance };
        *q = [q[0] * f, q[1] * f, q[2] * f, 1.0];
    }
    dilate(&mut alb, &mut nrm_img, w, h);
    let to8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    let albedo8: Vec<u8> = alb.iter().flat_map(|q| [to8(linear_to_srgb(q[0])), to8(linear_to_srgb(q[1])), to8(linear_to_srgb(q[2])), 255]).collect();
    let normal8: Vec<u8> = nrm_img.iter().flat_map(|q| [to8(q[0] * 0.5 + 0.5), to8(q[1] * 0.5 + 0.5), to8(q[2] * 0.5 + 0.5), 255]).collect();
    let diffuse = encode_dds(uw.width, uw.height, &albedo8, MipKind::Color);
    let normal = encode_dds(uw.width, uw.height, &normal8, MipKind::Normal);
    let mut shader = lod.shader.clone().unwrap_or_default();
    shader.texture_clamp_mode = 0;
    shader.flags2 |= SLSF2_VERTEX_COLORS;
    // Vertex alpha: the full model's snow mask (its vertex alpha, which the game's directional
    // material multiplies into its slope test; the LOD model's own alpha follows the HD LOD
    // convention, a bias around 0.5, and means something else). The mask varies far faster than
    // the LOD mesh, so a single sample at a LOD vertex can land on a small masked spot and strip
    // the snow off the whole fan of large triangles around it. Each LOD triangle takes the mean
    // over points spread across it instead, and each vertex the area-weighted mean of its
    // triangles. 255 where nothing is hit.
    let alpha_at = |p: Vec3, n: Vec3| -> Option<f32> {
        let (hit, _, _) = find(p, n)?;
        let (si, ti) = src.tri_ref[hit.tri as usize];
        let part = &src.shapes[si as usize].part;
        Some(match &part.colors {
            Some(c) if c.len() == part.positions.len() => {
                let idx = part.triangles[ti as usize].map(usize::from);
                let wt = [1.0 - hit.u - hit.v, hit.u, hit.v];
                idx.iter().zip(wt).map(|(&i, w)| f32::from(c[i][3]) * w).sum::<f32>()
            }
            _ => 255.0,
        })
    };
    // Barycentric sample points: centroid, the three vertex-side and three edge-side points.
    const SAMPLES: [[f32; 3]; 7] = [[1.0 / 3.0; 3], [0.6, 0.2, 0.2], [0.2, 0.6, 0.2], [0.2, 0.2, 0.6], [0.45, 0.45, 0.1], [0.1, 0.45, 0.45], [0.45, 0.1, 0.45]];
    let tri_alpha: Vec<(f32, f32)> = tris16
        .par_iter()
        .map(|t| {
            let v = t.map(|i| Vec3::from(pos[usize::from(i)]));
            let vn = t.map(|i| Vec3::from(nrm[usize::from(i)]));
            let area = (v[1] - v[0]).cross(v[2] - v[0]).length() * 0.5;
            let (mut sum, mut k) = (0.0f32, 0u32);
            for b in SAMPLES {
                let p = v[0] * b[0] + v[1] * b[1] + v[2] * b[2];
                let n = (vn[0] * b[0] + vn[1] * b[1] + vn[2] * b[2]).normalize_or_zero();
                if let Some(a) = alpha_at(p, n) {
                    sum += a;
                    k += 1;
                }
            }
            (if k > 0 { sum / k as f32 } else { 255.0 }, area.max(1e-6))
        })
        .collect();
    let mut acc = vec![(0.0f32, 0.0f32); pos.len()];
    for (t, (a, w)) in tris16.iter().zip(&tri_alpha) {
        for &i in t {
            acc[usize::from(i)].0 += a * w;
            acc[usize::from(i)].1 += w;
        }
    }
    let colors: Vec<[u8; 4]> = acc.iter().map(|(s, w)| [255, 255, 255, if *w > 0.0 { (s / w).round().clamp(0.0, 255.0) as u8 } else { 255 }]).collect();
    let nif = write_nif(pos, nrm, tan, bit, uv, colors, tris16.clone(), shader, out_diffuse, out_normal)?;
    Ok(Baked { nif, diffuse, normal, width: uw.width, height: uw.height, density: uw.density, charts: uw.charts, triangles: tris16.len(), samples, misses, ao_balance: balance, ms: t0.elapsed().as_millis() })
}

/// Grows the baked charts outward so mips and filtering never read empty texels.
fn dilate(alb: &mut [[f32; 4]], nrm: &mut [[f32; 4]], w: usize, h: usize) {
    let mut valid: Vec<bool> = alb.iter().map(|q| q[3] > 0.0).collect();
    for _ in 0..64 {
        let mut next = valid.clone();
        let mut grew = false;
        for y in 0..h {
            for x in 0..w {
                if valid[y * w + x] {
                    continue;
                }
                let (mut a, mut n, mut c) = ([0.0f32; 3], Vec3::ZERO, 0.0f32);
                for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1), (-1, -1), (1, 1), (-1, 1), (1, -1)] {
                    let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                    if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                        continue;
                    }
                    let j = ny as usize * w + nx as usize;
                    if valid[j] {
                        for k in 0..3 {
                            a[k] += alb[j][k];
                        }
                        n += Vec3::new(nrm[j][0], nrm[j][1], nrm[j][2]);
                        c += 1.0;
                    }
                }
                if c > 0.0 {
                    let i = y * w + x;
                    alb[i] = [a[0] / c, a[1] / c, a[2] / c, 1.0];
                    let nn = n.normalize_or(Vec3::Z);
                    nrm[i] = [nn.x, nn.y, nn.z, 1.0];
                    next[i] = true;
                    grew = true;
                }
            }
        }
        valid = next;
        if !grew {
            break;
        }
    }
    // Whatever stays empty (no chart within 64 texels) gets the mean colour and a flat normal.
    let filled: Vec<&[f32; 4]> = alb.iter().zip(&valid).filter(|(_, v)| **v).map(|(q, _)| q).collect();
    let n = filled.len().max(1) as f32;
    let mean = filled.iter().fold([0.0f32; 3], |m, q| [m[0] + q[0] / n, m[1] + q[1] / n, m[2] + q[2] / n]);
    for (i, v) in valid.iter().enumerate() {
        if !v {
            alb[i] = [mean[0], mean[1], mean[2], 1.0];
            nrm[i] = [0.0, 0.0, 1.0, 1.0];
        }
    }
}

/// BC7 DDS with a full mip chain (colour filtered in linear light, normals renormalised).
pub fn encode_dds(w: u32, h: u32, rgba: &[u8], kind: MipKind) -> Vec<u8> {
    let chain = mips::chain(w, h, rgba, 32, kind, Edge::Clamp);
    let blocks: Vec<Vec<u8>> = chain.iter().map(|(cw, ch, px)| lodforge_gpu::bc7_cpu::encode(&RgbaImage { width: *cw, height: *ch, rgba: px }, Bc7Quality::High)).collect();
    lodforge_dds::write_bc7(w, h, &blocks)
}

#[allow(clippy::too_many_arguments)]
fn write_nif(pos: Vec<[f32; 3]>, nrm: Vec<[f32; 3]>, tan: Vec<[f32; 3]>, bit: Vec<[f32; 3]>, uv: Vec<[f32; 2]>, colors: Vec<[u8; 4]>, tris: Vec<[u16; 3]>, shader: LightingShader, diffuse: &str, normal: &str) -> Result<Vec<u8>> {
    let mut nif = Nif::new();
    nif.header.block_types = vec!["BSFadeNode".into()];
    nif.header.block_type_index = vec![0];
    nif.add_block(Block::Node(Node { av: AvObject { net: ObjectNet::named("LODForgeDetailBake"), ..Default::default() }, ..Default::default() }));
    let mut set = vec![String::new(); 9];
    set[0] = diffuse.to_string();
    set[1] = normal.to_string();
    let ts = nif.add_block(Block::TextureSet(set));
    let mut sh = shader;
    sh.texture_set = Some(ts);
    let si = nif.add_block(Block::LightingShader(sh));
    let (mut mn, mut mx) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for p in &pos {
        mn = mn.min(Vec3::from(*p));
        mx = mx.max(Vec3::from(*p));
    }
    let mut s = TriShape {
        av: AvObject { net: ObjectNet::named("LODForgeDetailBake"), ..Default::default() },
        bound_center: ((mn + mx) * 0.5).to_array(),
        bound_radius: (mx - mn).length() * 0.5,
        shader_property: Some(si),
        positions: pos,
        uvs: uv,
        normals: nrm,
        tangents: tan,
        bitangents: bit,
        colors,
        triangles: tris,
        ..Default::default()
    };
    s.update_vertex_desc();
    let child = nif.add_block(Block::TriShape(s));
    if let Block::Node(r) = &mut nif.blocks[0] {
        r.children = vec![Some(child)];
    }
    nif.to_bytes().map_err(|e| anyhow!("{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A wall of two triangles baked against itself: every sample hits, the bake writes a
    /// readable NIF and DDS, and the baked normal map is flat (the LOD and full surfaces agree).
    /// A 1000 x 1000 wall in the xz plane at `y`, facing -y, subdivided `nx` times.
    fn wall(nx: usize, y: f32, texture: &str) -> MeshPart {
        let mut positions = Vec::new();
        let mut uvs = Vec::new();
        for j in 0..=nx {
            for i in 0..=nx {
                let (u, v) = (i as f32 / nx as f32, j as f32 / nx as f32);
                positions.push([u * 1000.0, y, v * 1000.0]);
                uvs.push([u, v]);
            }
        }
        let mut triangles = Vec::new();
        let row = nx as u16 + 1;
        for j in 0..nx as u16 {
            for i in 0..nx as u16 {
                let a = j * row + i;
                triangles.push([a, a + row, a + 1]);
                triangles.push([a + 1, a + row, a + row + 1]);
            }
        }
        let normals = vec![[0.0, -1.0, 0.0]; positions.len()];
        let (tangents, bitangents) = lodforge_objlod::mesh::tangents_from_uv(&positions, &uvs, &normals, &triangles);
        let mut textures: [String; 9] = Default::default();
        textures[0] = texture.into();
        MeshPart { name: "Rock".into(), index: 0, textures, shader: ShaderSource::Lighting(LightingShader::default()), positions, uvs, normals, tangents, bitangents, colors: None, triangles, source_normals: true, source_tangents: true, legacy: false, alpha: None }
    }

    /// A one-colour 16 x 16 BC7 DDS.
    fn solid_dds(c: [u8; 4]) -> Vec<u8> {
        let rgba: Vec<u8> = (0..16 * 16).flat_map(|_| c).collect();
        lodforge_dds::write_bc7(16, 16, &[lodforge_gpu::bc7_cpu::encode(&RgbaImage { width: 16, height: 16, rgba: &rgba }, Bc7Quality::High)])
    }

    /// An alpha-tested overlay whose texels are all holes, 20 units in front of a rock wall: the
    /// bake shows the wall's colour, and a LOD model with such a part is reported as cut-out.
    #[test]
    fn rays_pass_through_alpha_holes() {
        let mut overlay = wall(4, -20.0, "textures\\test\\moss.dds");
        overlay.alpha = Some((0x200, 128));
        let full = LodMesh { path: "full.nif".into(), parts: vec![wall(8, 0.0, "textures\\test\\rock.dds"), overlay.clone()], warnings: vec![], skipped: 0 };
        let lod = LodMesh { path: "lod.nif".into(), parts: vec![wall(1, -30.0, "textures\\test\\rock.dds")], warnings: vec![], skipped: 0 };
        assert!(!has_cutout_parts(&lod));
        assert!(has_cutout_parts(&LodMesh { path: "x.nif".into(), parts: vec![overlay], warnings: vec![], skipped: 0 }));
        let (rock, moss) = (solid_dds([200, 40, 40, 255]), solid_dds([40, 200, 40, 0]));
        let read = move |p: &str| match p {
            "textures\\test\\rock.dds" => Some(rock.clone()),
            "textures\\test\\moss.dds" => Some(moss.clone()),
            _ => None,
        };
        let exists = |_: &str| false;
        let tex = Textures::new(&read, &exists);
        let src = Source::new(&full).expect("source");
        let b = bake(&src, &lod, &tex, &Params { density: 0.05, max_size: 256, ao_rays: 8, ao_strength: 0.0 }, "textures\\out.dds", "textures\\out_n.dds").expect("bake");
        let info = lodforge_dds::parse(&b.diffuse).expect("dds");
        let img = lodforge_dds::decode(&b.diffuse, &info, 0).expect("decode");
        let c = &img.rgba[((img.height / 4) * img.width + img.width / 4) as usize * 4..][..3];
        assert!(c[0] > 150 && c[1] < 90, "the overlay's colour came through: {c:?}");
    }

    #[test]
    fn flat_wall_bakes_flat() {
        let full = LodMesh { path: "full.nif".into(), parts: vec![wall(8, 0.0, "textures\\test\\rock.dds")], warnings: vec![], skipped: 0 };
        let lod = LodMesh { path: "lod.nif".into(), parts: vec![wall(1, 0.0, "textures\\test\\rock.dds")], warnings: vec![], skipped: 0 };
        let dds = solid_dds([128, 100, 80, 255]);
        let read = move |p: &str| (p == "textures\\test\\rock.dds").then(|| dds.clone());
        let exists = |_: &str| false;
        let tex = Textures::new(&read, &exists);
        let src = Source::new(&full).expect("source");
        let b = bake(&src, &lod, &tex, &Params { density: 0.05, max_size: 256, ao_rays: 8, ao_strength: 0.85 }, "textures\\out.dds", "textures\\out_n.dds").expect("bake");
        assert!(b.misses * 50 < b.samples, "{} of {} samples missed", b.misses, b.samples);
        assert!(b.width >= 32 && b.width <= 256, "{}x{}", b.width, b.height);
        let nif = lodforge_nif::parse(&b.nif).expect("nif");
        assert!(nif.blocks.iter().any(|x| matches!(x, Block::TriShape(s) if s.triangles.len() == 2)));
        let info = lodforge_dds::parse(&b.normal).expect("dds");
        let img = lodforge_dds::decode(&b.normal, &info, 0).expect("decode");
        // Centre texel of the (single) chart: flat normal (128, 128, 255) within BC7 error.
        let c = &img.rgba[((img.height / 4) * img.width + img.width / 4) as usize * 4..][..3];
        assert!((c[0] as i32 - 128).abs() <= 6 && (c[1] as i32 - 128).abs() <= 6 && c[2] >= 245, "{c:?}");
    }
}
