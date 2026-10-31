//! Prototype: hi-to-lo detail bake for one rock / mountain model (docs: memory note
//! "lodforge-rock-lod-detail"). For each LOD model given, it
//!
//! 1. unwraps the LOD mesh into non-overlapping planar charts (one texel density for the whole
//!    model) and packs them into a square texture,
//! 2. casts rays along the LOD surface normal to the full model and bakes what it finds there:
//!    albedo (full diffuse x vertex colours, PBR albedo x0.65 like the `linear` LOD copies),
//!    the full model's shading normal (geometry + its tangent-space normal map) re-expressed in
//!    the LOD mesh's tangent frame, and ambient occlusion (self-occlusion rays on the full model
//!    times the RMAOS AO channel) multiplied into the albedo,
//! 3. writes the baked LOD model (`.nif`, BC7 `.dds` + `_n.dds`) and PNG previews.
//!
//! It also writes the same LOD models with their tiling texture at the size the runtime shows
//! it (`--before-size`, default 256 px: 512 px page tile with `iSkipMips=1`), and a viewer test
//! world (`meshes\terrain\baketest\objects\baketest.4.0.0.bto`) with the full model, the
//! "before" and the "after" models side by side.
//!
//! ```text
//! cargo run --release --example detail_bake -- --instance D:\Modlists\skyrim-cs-base
//!   --full meshes\landscape\mountains\mountaincliff02.nif
//!   --lod lod4=meshes\lod\mountains\mountaincliff02_lod_0h.nif@0.3
//!   --lod far1=meshes\lod\lodforge\far\mountaincliff02_lod_0h_84273a0c_far1.nif@0.08
//!   --before-diffuse textures\pbr\landscape\mountains\mountainslab02linear.dds
//!   --before-normal textures\pbr\landscape\mountains\mountainslab02_n.dds --out <dir>
//! ```
//! Densities are texels per world unit.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result, anyhow, bail};
use glam::{Vec2, Vec3};
use lodforge_dds::mips::{self, Edge, MipKind};
use lodforge_gpu::{Bc7Quality, RgbaImage};
use lodforge_nif::{AvObject, Block, LightingShader, Nif, Node, ObjectNet, TriShape};
use lodforge_viewer::mesh::{FileKind, ShapeMesh, shapes_of};
use rayon::prelude::*;

const SLSF2_VERTEX_COLORS: u32 = 1 << 5;
const PBR_SCALE: f32 = 0.65;
/// Supersamples per texel along each axis.
const SS: u32 = 2;

// ---------------------------------------------------------------------------------------------
// Arguments and the load order

struct LodArg {
    label: String,
    path: String,
    density: f32,
}

struct Args {
    instance: PathBuf,
    profile: String,
    full: String,
    lods: Vec<LodArg>,
    before_diffuse: Option<String>,
    before_normal: Option<String>,
    before_size: u32,
    out: PathBuf,
    max_size: u32,
    ao_rays: u32,
    ao_radius: Option<f32>,
    ao_strength: f32,
    search: Option<f32>,
}

fn parse_args() -> Result<Args> {
    let mut a = Args {
        instance: PathBuf::new(),
        profile: "Default".into(),
        full: String::new(),
        lods: Vec::new(),
        before_diffuse: None,
        before_normal: None,
        before_size: 256,
        out: PathBuf::new(),
        max_size: 2048,
        ao_rays: 48,
        ao_radius: None,
        ao_strength: 0.85,
        search: None,
    };
    let v: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < v.len() {
        let k = v[i].as_str();
        let val = v.get(i + 1).cloned().ok_or_else(|| anyhow!("{k} needs a value"))?;
        match k {
            "--instance" => a.instance = val.into(),
            "--profile" => a.profile = val,
            "--full" => a.full = val,
            "--lod" => {
                let (label, rest) = val.split_once('=').ok_or_else(|| anyhow!("--lod label=path@density"))?;
                let (path, d) = rest.rsplit_once('@').ok_or_else(|| anyhow!("--lod label=path@density"))?;
                a.lods.push(LodArg { label: label.into(), path: path.into(), density: d.parse()? });
            }
            "--before-diffuse" => a.before_diffuse = Some(val),
            "--before-normal" => a.before_normal = Some(val),
            "--before-size" => a.before_size = val.parse()?,
            "--out" => a.out = val.into(),
            "--max-size" => a.max_size = val.parse()?,
            "--ao-rays" => a.ao_rays = val.parse()?,
            "--ao-radius" => a.ao_radius = Some(val.parse()?),
            "--ao-strength" => a.ao_strength = val.parse()?,
            "--search" => a.search = Some(val.parse()?),
            _ => bail!("unknown argument {k}"),
        }
        i += 2;
    }
    if a.full.is_empty() || a.lods.is_empty() || a.out.as_os_str().is_empty() {
        bail!("needs --instance, --full, --lod and --out");
    }
    Ok(a)
}

/// Loose files of an MO2 profile, highest priority first (BSAs are not read).
struct Vfs {
    roots: Vec<PathBuf>,
}

impl Vfs {
    fn new(instance: &Path, profile: &str) -> Result<Vfs> {
        let list = std::fs::read_to_string(instance.join("profiles").join(profile).join("modlist.txt")).context("reading modlist.txt")?;
        let mut roots: Vec<PathBuf> = list.lines().filter_map(|l| l.strip_prefix('+')).map(|m| instance.join("mods").join(m.trim())).collect();
        for d in ["Stock Game/Data", "Game Root/Data"] {
            let p = instance.join(d);
            if p.is_dir() {
                roots.push(p);
            }
        }
        Ok(Vfs { roots })
    }

    fn find(&self, rel: &str) -> Option<PathBuf> {
        let rel = rel.trim().replace('/', "\\");
        let rel = rel.strip_prefix("data\\").unwrap_or(&rel).to_string();
        self.roots.iter().map(|r| r.join(&rel)).find(|p| p.is_file())
    }

    fn read(&self, rel: &str) -> Result<(PathBuf, Vec<u8>)> {
        let p = self.find(rel).ok_or_else(|| anyhow!("not in the load order: {rel}"))?;
        let b = std::fs::read(&p)?;
        Ok((p, b))
    }
}

// ---------------------------------------------------------------------------------------------
// Textures

/// One decoded mip level, RGBA as stored (0..1, gamma space for colour).
struct Tex {
    w: u32,
    h: u32,
    px: Vec<[f32; 4]>,
}

impl Tex {
    /// The smallest mip level at least `want_w` wide (the top one if none is).
    fn load(vfs: &Vfs, rel: &str, want_w: u32) -> Result<(Tex, String)> {
        let (path, b) = vfs.read(rel)?;
        let info = lodforge_dds::parse(&b).map_err(|e| anyhow!("{rel}: {e}"))?;
        let mut level = 0;
        while level + 1 < info.mip_count && (info.width >> (level + 1)) >= want_w.max(1) {
            level += 1;
        }
        let img = lodforge_dds::decode(&b, &info, level).map_err(|e| anyhow!("{rel}: {e}"))?;
        let px = img.rgba.chunks_exact(4).map(|p| [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0, p[3] as f32 / 255.0]).collect();
        let what = format!("{} ({}x{} of {}x{})", path.display(), img.width, img.height, info.width, info.height);
        Ok((Tex { w: img.width, h: img.height, px }, what))
    }

    /// Bilinear, wrapping.
    fn sample(&self, uv: Vec2) -> [f32; 4] {
        let x = uv.x * self.w as f32 - 0.5;
        let y = uv.y * self.h as f32 - 0.5;
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let wrap = |v: i64, n: u32| v.rem_euclid(n as i64) as usize;
        let (xa, xb) = (wrap(x0 as i64, self.w), wrap(x0 as i64 + 1, self.w));
        let (ya, yb) = (wrap(y0 as i64, self.h), wrap(y0 as i64 + 1, self.h));
        let w = self.w as usize;
        let p = |x: usize, y: usize| self.px[y * w + x];
        let (a, b, c, d) = (p(xa, ya), p(xb, ya), p(xa, yb), p(xb, yb));
        let mut o = [0.0; 4];
        for k in 0..4 {
            o[k] = (a[k] * (1.0 - fx) + b[k] * fx) * (1.0 - fy) + (c[k] * (1.0 - fx) + d[k] * fx) * fy;
        }
        o
    }
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
    /// Leaf: first triangle in `order`; inner: right child (left is the next node).
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
        Self::build(&tris, &cent, &mut order, 0, tris.len(), &mut nodes);
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
                // Left child follows its parent; `index` is the right child.
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
    mesh: ShapeMesh,
    diffuse: Option<Tex>,
    normal: Option<Tex>,
    rmaos: Option<Tex>,
    pbr: bool,
}

struct Source {
    shapes: Vec<SrcShape>,
    /// Per BVH triangle: shape and the triangle's first index in that shape.
    tri_ref: Vec<(u32, u32)>,
    bvh: Bvh,
    /// Median UV units per world unit (texture repeats per unit).
    uv_per_unit: f32,
    min: Vec3,
    max: Vec3,
}

fn tex_path(m: &lodforge_viewer::mesh::Material, slot: usize) -> Option<String> {
    m.textures.get(slot).map(|t| t.trim().to_ascii_lowercase()).filter(|t| !t.is_empty()).map(|t| if t.starts_with("textures\\") { t } else { format!("textures\\{t}") })
}

fn load_source(vfs: &Vfs, rel: &str) -> Result<(Source, Nif)> {
    let (path, bytes) = vfs.read(rel)?;
    println!("full model: {}", path.display());
    let nif = lodforge_nif::parse(&bytes).map_err(|e| anyhow!("{rel}: {e}"))?;
    let meshes: Vec<ShapeMesh> = shapes_of(&nif, FileKind::Model).into_iter().filter(|s| !s.material.effect && !s.indices.is_empty() && s.uvs.len() == s.positions.len()).collect();
    if meshes.is_empty() {
        bail!("{rel}: no textured lighting shapes");
    }
    let mut tris = Vec::new();
    let mut tri_ref = Vec::new();
    let mut ratios = Vec::new();
    let (mut mn, mut mx) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for (si, s) in meshes.iter().enumerate() {
        let sc = Vec2::from(s.material.uv_scale);
        for (k, t) in s.indices.chunks_exact(3).enumerate() {
            let p = [Vec3::from(s.positions[t[0] as usize]), Vec3::from(s.positions[t[1] as usize]), Vec3::from(s.positions[t[2] as usize])];
            for q in p {
                mn = mn.min(q);
                mx = mx.max(q);
            }
            let wa = (p[1] - p[0]).cross(p[2] - p[0]).length();
            let uv = |i: u32| Vec2::from(s.uvs[i as usize]) * sc;
            let (a, b, c) = (uv(t[0]), uv(t[1]), uv(t[2]));
            let ua = ((b - a).perp_dot(c - a)).abs();
            if wa > 1e-3 && ua > 0.0 {
                ratios.push((ua / wa).sqrt());
            }
            tris.push(p);
            tri_ref.push((si as u32, (k * 3) as u32));
        }
    }
    ratios.sort_by(f32::total_cmp);
    let uv_per_unit = ratios.get(ratios.len() / 2).copied().unwrap_or(1.0 / 1024.0);
    let t0 = Instant::now();
    let bvh = Bvh::new(tris);
    println!("  {} shapes, {} triangles, {:.0} world units per texture repeat, BVH {:.0} ms", meshes.len(), tri_ref.len(), 1.0 / uv_per_unit, t0.elapsed().as_secs_f64() * 1e3);
    let shapes = meshes.into_iter().map(|mesh| SrcShape { pbr: mesh.material.diffuse.as_deref().is_some_and(|d| d.contains("\\pbr\\")), mesh, diffuse: None, normal: None, rmaos: None }).collect();
    Ok((Source { shapes, tri_ref, bvh, uv_per_unit, min: mn, max: mx }, nif))
}

/// Loads the full model's textures at the mip level a bake at `density` texels per unit needs.
fn load_source_textures(vfs: &Vfs, src: &mut Source, density: f32) -> Result<()> {
    // Source texels needed per world unit: supersampled bake density.
    let want_per_unit = density * SS as f32;
    for s in &mut src.shapes {
        let m = &s.mesh.material;
        let want = |tex_rel: &str| -> u32 {
            // A texture of width W gives W * uv_per_unit texels per unit.
            let _ = tex_rel;
            ((want_per_unit / src.uv_per_unit).ceil() as u32).next_power_of_two()
        };
        s.diffuse = match tex_path(m, 0) {
            Some(p) => {
                let (t, what) = Tex::load(vfs, &p, want(&p))?;
                println!("  diffuse {what}");
                Some(t)
            }
            None => None,
        };
        s.normal = match tex_path(m, 1) {
            Some(p) => match Tex::load(vfs, &p, want(&p)) {
                Ok((t, what)) => {
                    println!("  normal  {what}");
                    Some(t)
                }
                Err(e) => {
                    println!("  normal map skipped: {e}");
                    None
                }
            },
            None => None,
        };
        s.rmaos = match tex_path(m, 5).filter(|p| p.ends_with("_rmaos.dds")) {
            Some(p) => match Tex::load(vfs, &p, want(&p)) {
                Ok((t, what)) => {
                    println!("  rmaos   {what}");
                    Some(t)
                }
                Err(e) => {
                    println!("  rmaos skipped: {e}");
                    None
                }
            },
            None => None,
        };
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// The LOD mesh and its unwrap

struct LodMesh {
    pos: Vec<Vec3>,
    nrm: Vec<Vec3>,
    alpha: Vec<u8>,
    /// The model's own UVs, tangent frame and vertex colours (RGB 0..1), for the fallback where
    /// the full model has no surface.
    uv: Vec<Vec2>,
    tan: Vec<Vec3>,
    bit: Vec<Vec3>,
    rgb: Vec<[f32; 3]>,
    tris: Vec<[u32; 3]>,
    shader: Option<LightingShader>,
    name: String,
}

fn load_lod(vfs: &Vfs, rel: &str) -> Result<LodMesh> {
    let (path, bytes) = vfs.read(rel)?;
    let nif = lodforge_nif::parse(&bytes).map_err(|e| anyhow!("{rel}: {e}"))?;
    let mut m = LodMesh { pos: Vec::new(), nrm: Vec::new(), alpha: Vec::new(), uv: Vec::new(), tan: Vec::new(), bit: Vec::new(), rgb: Vec::new(), tris: Vec::new(), shader: None, name: String::new() };
    for s in shapes_of(&nif, FileKind::Model) {
        if s.material.effect || s.indices.is_empty() {
            continue;
        }
        let base = m.pos.len() as u32;
        let n = s.positions.len();
        m.pos.extend(s.positions.iter().map(|p| Vec3::from(*p)));
        if m.name.is_empty() {
            m.name = s.material.name.clone();
        }
        let nrm: Vec<Vec3> = if s.normals.len() == n {
            s.normals.iter().map(|v| Vec3::from(*v)).collect()
        } else {
            let mut acc = vec![Vec3::ZERO; n];
            for t in s.indices.chunks_exact(3) {
                let p = |i: u32| Vec3::from(s.positions[i as usize]);
                let f = (p(t[1]) - p(t[0])).cross(p(t[2]) - p(t[0]));
                for &i in t {
                    acc[i as usize] += f;
                }
            }
            acc.into_iter().map(|v| v.normalize_or(Vec3::Z)).collect()
        };
        let uvs: Vec<[f32; 2]> = if s.uvs.len() == n { s.uvs.clone() } else { vec![[0.0; 2]; n] };
        let sc = Vec2::from(s.material.uv_scale);
        let off = Vec2::from(s.material.uv_offset);
        m.uv.extend(uvs.iter().map(|u| Vec2::from(*u) * sc + off));
        let nrm_a: Vec<[f32; 3]> = nrm.iter().map(|v| v.to_array()).collect();
        let tris16: Vec<[u16; 3]> = s.indices.chunks_exact(3).map(|t| [t[0] as u16, t[1] as u16, t[2] as u16]).collect();
        let (t, b) = if s.tangents.len() == n && s.bitangents.len() == n { (s.tangents.clone(), s.bitangents.clone()) } else { lodforge_objlod::mesh::tangents_from_uv(&s.positions, &uvs, &nrm_a, &tris16) };
        m.tan.extend(t.iter().map(|v| Vec3::from(*v)));
        m.bit.extend(b.iter().map(|v| Vec3::from(*v)));
        let vc = s.material.vertex_colors && s.colors.len() == n;
        m.rgb.extend((0..n).map(|i| if vc { let c = s.colors[i]; [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0] } else { [1.0; 3] }));
        m.nrm.extend(nrm);
        m.alpha.extend((0..n).map(|i| s.colors.get(i).map_or(255, |c| c[3])));
        m.tris.extend(s.indices.chunks_exact(3).map(|t| [t[0] + base, t[1] + base, t[2] + base]));
    }
    m.shader = nif.blocks.iter().find_map(|b| if let Block::LightingShader(l) = b { Some(l.clone()) } else { None });
    println!("LOD model: {} ({} triangles)", path.display(), m.tris.len());
    Ok(m)
}

struct Unwrap {
    /// Output vertices: source LOD vertex and UV.
    verts: Vec<(u32, Vec2)>,
    tris: Vec<[u32; 3]>,
    /// Texture width and height (the height is cut to the power of two the charts need).
    size: u32,
    height: u32,
    density: f32,
    charts: usize,
}

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

/// Packing grid cell in texels; charts keep one free cell around them (the gutter).
const BLOCK: u32 = 4;

/// Where a chart went: block offset, and whether it is turned by 90 degrees.
#[derive(Clone, Copy)]
struct Place {
    x: u32,
    y: u32,
    turn: bool,
}

/// A chart point (world units, chart at the origin with extent `e`), optionally turned.
fn chart_point(p: Vec2, e: Vec2, turn: bool) -> Vec2 {
    if turn { Vec2::new(e.y - p.y, p.x) } else { p }
}

/// The cells a chart covers at density `d` (texels per unit), grown by one cell: width,
/// height and the covered cells. Cell `(1, 1)` holds the chart origin.
fn chart_mask(c: &[(usize, [Vec2; 3])], e: Vec2, d: f32, turn: bool) -> (usize, usize, Vec<(u16, u16)>) {
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
        // Samples a quarter cell apart (one texel).
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

/// Bitmap packing of the charts' real shapes into a `gw` x `gh` cell grid: biggest first, each at
/// its lowest, then leftmost free position, turned by 90 degrees when that sits lower. Returns
/// the placements and the rows used.
fn pack_shapes(charts: &[Vec<(usize, [Vec2; 3])>], extents: &[Vec2], d: f32, gw: u32, gh: u32) -> Option<(Vec<Place>, u32)> {
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

/// Skyline (bottom-left) packing of `sizes` (texels, padding included) into a `size`-wide,
/// `size`-tall area: tallest first, each rectangle where its top edge ends lowest. Returns the
/// positions and the height used.
#[allow(dead_code)]
fn pack(sizes: &[(u32, u32)], size: u32) -> Option<(Vec<(u32, u32)>, u32)> {
    let mut idx: Vec<usize> = (0..sizes.len()).collect();
    idx.sort_by_key(|&i| std::cmp::Reverse((sizes[i].1, sizes[i].0)));
    let s = size as usize;
    let mut sky = vec![0u32; s];
    let mut out = vec![(0, 0); sizes.len()];
    let mut used = 0;
    for i in idx {
        let (w, h) = (sizes[i].0 as usize, sizes[i].1);
        if w > s {
            return None;
        }
        // Sliding-window maximum of the skyline over every start column.
        let mut best: Option<(u32, usize)> = None;
        let mut dq: VecDeque<usize> = VecDeque::new();
        for x in 0..s {
            while dq.back().is_some_and(|&b| sky[b] <= sky[x]) {
                dq.pop_back();
            }
            dq.push_back(x);
            if x + 1 >= w {
                let start = x + 1 - w;
                while dq.front().is_some_and(|&f| f < start) {
                    dq.pop_front();
                }
                let y = sky[dq[0]];
                if best.is_none_or(|(by, _)| y < by) {
                    best = Some((y, start));
                }
            }
        }
        let (y, x) = best?;
        if y + h > size {
            return None;
        }
        for c in &mut sky[x..x + w] {
            *c = y + h;
        }
        out[i] = (x as u32, y);
        used = used.max(y + h);
    }
    Some((out, used))
}

fn unwrap(m: &LodMesh, density: f32, max_size: u32) -> Unwrap {
    let nf = m.tris.len();
    // Welded vertex ids for adjacency (LOD models split vertices at UV seams).
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
    // Per chart: faces and their 2D corners (world units).
    let mut charts: Vec<Vec<(usize, [Vec2; 3])>> = Vec::new();
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
            let n = fnorm[f].normalize_or(Vec3::Z);
            if n.dot(n0) < cos_limit {
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
    // Rotate each chart to its smallest bounding box and move it to the origin.
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
        break pack_shapes(&charts, &extents, d, size / BLOCK, size / BLOCK).expect("packs at the found density");
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
                // A vertex keeps one UV per chart; corners of one source vertex agree inside a chart.
                let id = *vmap.entry((ci, src)).or_insert_with(|| {
                    verts.push((src, uv));
                    (verts.len() - 1) as u32
                });
                tris[*f][k] = id;
            }
        }
    }
    Unwrap { verts, tris, size, height, density: d, charts: charts.len() }
}

// ---------------------------------------------------------------------------------------------
// The bake

struct Baked {
    size: u32,
    height: u32,
    albedo: Vec<u8>,
    normal: Vec<u8>,
    ao: Vec<u8>,
    misses: usize,
    samples: usize,
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

struct Frame {
    p: Vec3,
    n: Vec3,
    t: Vec3,
    b: Vec3,
}

#[allow(clippy::too_many_arguments)]
/// The LOD model's own textures, for texels where the full model has no surface.
struct Fallback<'a> {
    diffuse: &'a Tex,
    normal: Option<&'a Tex>,
}

/// Albedo alpha marker during the bake: a texel without AO (kept out of the balance).
const FIXED: f32 = 2.0;

#[allow(clippy::too_many_arguments)]
fn bake(src: &Source, lod: &LodMesh, uw: &Unwrap, tan: &[[f32; 3]], bit: &[[f32; 3]], search: f32, ao_radius: f32, ao_rays: u32, ao_strength: f32, fallback: Option<&Fallback<'_>>) -> Baked {
    // The LOD model's own attributes at a point of an output triangle: UV, tangent frame, vertex colour.
    let orig = |ti: u32, w1: f32, w2: f32| -> (Vec2, Vec3, Vec3, Vec3, [f32; 3]) {
        let t = uw.tris[ti as usize];
        let w = [1.0 - w1 - w2, w1, w2];
        let (mut uv, mut tt, mut bb, mut nn, mut vc) = (Vec2::ZERO, Vec3::ZERO, Vec3::ZERO, Vec3::ZERO, [0f32; 3]);
        for k in 0..3 {
            let s = uw.verts[t[k] as usize].0 as usize;
            uv += lod.uv[s] * w[k];
            tt += lod.tan[s] * w[k];
            bb += lod.bit[s] * w[k];
            nn += lod.nrm[s] * w[k];
            for c in 0..3 {
                vc[c] += lod.rgb[s][c] * w[k];
            }
        }
        (uv, tt.normalize_or(Vec3::X), bb.normalize_or(Vec3::Y), nn.normalize_or(Vec3::Z), vc)
    };
    let size = uw.size as usize;
    let height = uw.height as usize;
    let sub = size * SS as usize;
    let sub_h = height * SS as usize;
    // Rasterise the output triangles into the supersample grid: (triangle, barycentrics).
    let mut cover: Vec<Option<(u32, f32, f32)>> = vec![None; sub * sub_h];
    for (ti, t) in uw.tris.iter().enumerate() {
        let uv = t.map(|i| uw.verts[i as usize].1 * Vec2::new(sub as f32, sub_h as f32));
        let (mn, mx) = (uv[0].min(uv[1]).min(uv[2]), uv[0].max(uv[1]).max(uv[2]));
        let area = (uv[1] - uv[0]).perp_dot(uv[2] - uv[0]);
        if area.abs() < 1e-9 {
            continue;
        }
        for y in (mn.y.floor().max(0.0) as usize)..=(mx.y.ceil() as usize).min(sub_h - 1) {
            for x in (mn.x.floor().max(0.0) as usize)..=(mx.x.ceil() as usize).min(sub - 1) {
                let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                let w1 = (p - uv[0]).perp_dot(uv[2] - uv[0]) / area;
                let w2 = (uv[1] - uv[0]).perp_dot(p - uv[0]) / area;
                let w0 = 1.0 - w1 - w2;
                if w0 >= -1e-4 && w1 >= -1e-4 && w2 >= -1e-4 {
                    cover[y * sub + x] = Some((ti as u32, w1, w2));
                }
            }
        }
    }
    let frame = |ti: u32, w1: f32, w2: f32| -> Frame {
        let t = uw.tris[ti as usize];
        let w = [1.0 - w1 - w2, w1, w2];
        let (mut p, mut n, mut tt, mut bb) = (Vec3::ZERO, Vec3::ZERO, Vec3::ZERO, Vec3::ZERO);
        for k in 0..3 {
            let o = t[k] as usize;
            let s = uw.verts[o].0 as usize;
            p += lod.pos[s] * w[k];
            n += lod.nrm[s] * w[k];
            tt += Vec3::from(tan[o]) * w[k];
            bb += Vec3::from(bit[o]) * w[k];
        }
        Frame { p, n: n.normalize_or(Vec3::Z), t: tt.normalize_or(Vec3::X), b: bb.normalize_or(Vec3::Y) }
    };
    // Full-model attributes at a hit.
    struct Surf {
        albedo: [f32; 3],
        normal: Vec3,
        geo: Vec3,
        pos: Vec3,
        mat_ao: f32,
    }
    let surf = |h: &Hit, o: Vec3, d: Vec3| -> Surf {
        let (si, first) = src.tri_ref[h.tri as usize];
        let s = &src.shapes[si as usize];
        let m = &s.mesh;
        let idx = [m.indices[first as usize] as usize, m.indices[first as usize + 1] as usize, m.indices[first as usize + 2] as usize];
        let w = [1.0 - h.u - h.v, h.u, h.v];
        let lerp3 = |v: &[[f32; 3]]| -> Option<Vec3> { (v.len() == m.positions.len()).then(|| idx.iter().zip(w).map(|(&i, w)| Vec3::from(v[i]) * w).sum()) };
        let tri = &src.bvh.tris[h.tri as usize];
        let mut geo = (tri[1] - tri[0]).cross(tri[2] - tri[0]).normalize_or(Vec3::Z);
        let ns = lerp3(&m.normals).map(|n| n.normalize_or(geo)).unwrap_or(geo);
        if geo.dot(ns) < 0.0 {
            geo = -geo;
        }
        let uv: Vec2 = idx.iter().zip(w).map(|(&i, w)| Vec2::from(m.uvs[i]) * w).sum::<Vec2>() * Vec2::from(m.material.uv_scale) + Vec2::from(m.material.uv_offset);
        let mut albedo = s.diffuse.as_ref().map_or([0.5; 3], |t| {
            let c = t.sample(uv);
            [c[0], c[1], c[2]]
        });
        if m.material.vertex_colors && m.colors.len() == m.positions.len() && std::env::var_os("DETAIL_BAKE_NO_VC").is_none() {
            for k in 0..3 {
                let vc: f32 = idx.iter().zip(w).map(|(&i, w)| m.colors[i][k] as f32 / 255.0 * w).sum();
                albedo[k] *= vc;
            }
        }
        if s.pbr {
            for c in &mut albedo {
                *c *= PBR_SCALE;
            }
        }
        let mut normal = ns;
        if let (Some(nt), Some(t), Some(b)) = (s.normal.as_ref(), lerp3(&m.tangents), lerp3(&m.bitangents)) {
            let c = nt.sample(uv);
            let v = Vec3::new(c[0] * 2.0 - 1.0, c[1] * 2.0 - 1.0, c[2] * 2.0 - 1.0);
            // The game's tangent frame (CS Lighting.hlsl VS: rows = bitangent (W slots), tangent,
            // normal): x pairs with the NIF bitangent field, y with the tangent field.
            normal = (b.normalize_or(Vec3::X) * v.x + t.normalize_or(Vec3::Y) * v.y + ns * v.z).normalize_or(ns);
        }
        let mat_ao = s.rmaos.as_ref().map_or(1.0, |t| t.sample(uv)[2]);
        Surf { albedo, normal, geo, pos: o + d * h.t, mat_ao }
    };
    let find = |f: &Frame| -> Option<(Hit, Vec3, Vec3)> {
        let up = src.bvh.trace(f.p, f.n, 0.0, search, false);
        let down = src.bvh.trace(f.p, -f.n, 0.0, search, false);
        match (up, down) {
            (Some(a), Some(b)) => Some(if a.t <= b.t { (a, f.p, f.n) } else { (b, f.p, -f.n) }),
            (Some(a), None) => Some((a, f.p, f.n)),
            (None, Some(b)) => Some((b, f.p, -f.n)),
            (None, None) => None,
        }
    };
    let rows: Vec<(Vec<[f32; 4]>, Vec<[f32; 4]>, Vec<f32>, usize, usize)> = (0..height)
        .into_par_iter()
        .map(|y| {
            let mut alb = vec![[0.0; 4]; size];
            let mut nrm = vec![[0.0; 4]; size];
            let mut ao = vec![-1.0f32; size];
            let (mut misses, mut samples) = (0, 0);
            for x in 0..size {
                let (mut acc_a, mut acc_n, mut cnt, mut fb_only) = ([0.0f32; 3], Vec3::ZERO, 0u32, 0u32);
                let mut ao_at: Option<Surf> = None;
                let mut any_cover = false;
                for sy in 0..SS as usize {
                    for sx in 0..SS as usize {
                        let Some((ti, w1, w2)) = cover[(y * SS as usize + sy) * sub + x * SS as usize + sx] else { continue };
                        any_cover = true;
                        samples += 1;
                        let f = frame(ti, w1, w2);
                        let Some((h, o, d)) = find(&f) else {
                            misses += 1;
                            // No full-model surface here (LOD skirts, filled gaps): show what the
                            // LOD model itself shows, its tiling texture through its own UVs.
                            if let Some(fb) = fallback {
                                let (uv, t, b, n, vc) = orig(ti, w1, w2);
                                let c = fb.diffuse.sample(uv);
                                for k in 0..3 {
                                    acc_a[k] += srgb_to_linear(c[k] * vc[k]);
                                }
                                let wn = fb.normal.map_or(n, |nt| {
                                    let c = nt.sample(uv);
                                    (b * (c[0] * 2.0 - 1.0) + t * (c[1] * 2.0 - 1.0) + n * (c[2] * 2.0 - 1.0)).normalize_or(n)
                                });
                                acc_n += Vec3::new(wn.dot(f.b), wn.dot(f.t), wn.dot(f.n));
                                cnt += 1;
                                fb_only += 1;
                            }
                            continue;
                        };
                        let s = surf(&h, o, d);
                        for k in 0..3 {
                            acc_a[k] += srgb_to_linear(s.albedo[k]);
                        }
                        // Encoded for the game's frame: x along the bitangent field, y along the tangent field.
                        acc_n += Vec3::new(s.normal.dot(f.b), s.normal.dot(f.t), s.normal.dot(f.n));
                        cnt += 1;
                        if ao_at.is_none() {
                            ao_at = Some(s);
                        }
                    }
                }
                if cnt == 0 {
                    if any_cover {
                        // Covered but nothing hit and no fallback: flat, mid grey.
                        alb[x] = [0.2, 0.2, 0.2, FIXED];
                        nrm[x] = [0.0, 0.0, 1.0, 1.0];
                        ao[x] = 1.0;
                    }
                    continue;
                }
                let Some(s) = ao_at else {
                    // Fallback only: no AO, kept out of the brightness balance.
                    let n = acc_n.normalize_or(Vec3::Z);
                    alb[x] = [acc_a[0] / cnt as f32, acc_a[1] / cnt as f32, acc_a[2] / cnt as f32, FIXED];
                    nrm[x] = [n.x, n.y, n.z, 1.0];
                    ao[x] = 1.0;
                    continue;
                };
                let _ = fb_only;
                // Ambient occlusion: cosine-weighted rays over the full model's surface.
                let (e1, e2) = basis(s.geo);
                let mut open = 0.0;
                for k in 0..ao_rays {
                    let r1 = (k as f32 + rnd((y * size + x) as u32, k)) / ao_rays as f32;
                    let r2 = rnd(k ^ 0x5bd1_e995, (y * size + x) as u32);
                    let phi = std::f32::consts::TAU * r2;
                    let rad = r1.sqrt();
                    let dir = (e1 * (rad * phi.cos()) + e2 * (rad * phi.sin()) + s.geo * (1.0 - r1).max(0.0).sqrt()).normalize();
                    match src.bvh.trace(s.pos + s.geo * 0.5, dir, 0.25, ao_radius, true) {
                        // Near occluders count fully, far ones fade out (less banding at the radius).
                        Some(h) => open += (h.t / ao_radius).powi(2),
                        None => open += 1.0,
                    }
                }
                let geo_ao = open / ao_rays as f32;
                let total = geo_ao * s.mat_ao;
                let f = 1.0 - ao_strength + ao_strength * total;
                // Alpha carries the AO factor until the brightness is balanced below.
                alb[x] = [acc_a[0] / cnt as f32, acc_a[1] / cnt as f32, acc_a[2] / cnt as f32, f];
                let n = acc_n.normalize_or(Vec3::Z);
                nrm[x] = [n.x, n.y, n.z, 1.0];
                ao[x] = total;
            }
            (alb, nrm, ao, misses, samples)
        })
        .collect();
    let mut alb: Vec<[f32; 4]> = Vec::with_capacity(size * height);
    let mut nrm: Vec<[f32; 4]> = Vec::with_capacity(size * height);
    let mut ao: Vec<f32> = Vec::with_capacity(size * height);
    let (mut misses, mut samples) = (0, 0);
    for (a, n, o, m, s) in rows {
        alb.extend(a);
        nrm.extend(n);
        ao.extend(o);
        misses += m;
        samples += s;
    }
    // AO redistributes light instead of darkening: the full model's vertex colours already carry
    // the artist's occlusion, so the baked mean stays the mean of texture x vertex colour
    // (linear luminance over the baked surface) and only the contrast comes from the AO rays.
    let lum = |p: &[f32; 4]| 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2];
    let (mut m0, mut m1, mut n_ao) = (0f64, 0f64, 0usize);
    for p in alb.iter().filter(|p| p[3] > 0.0 && p[3] < FIXED) {
        m0 += lum(p) as f64;
        m1 += (lum(p) * p[3]) as f64;
        n_ao += 1;
    }
    let balance = if m1 > 0.0 { (m0 / m1) as f32 } else { 1.0 };
    for p in alb.iter_mut().filter(|p| p[3] > 0.0) {
        let f = if p[3] >= FIXED { 1.0 } else { p[3] * balance };
        *p = [p[0] * f, p[1] * f, p[2] * f, 1.0];
    }
    println!("  AO balance x{balance:.3} (mean linear luminance {:.4})", m0 / n_ao.max(1) as f64);
    // Gutter: grow the charts outward so mips and bilinear filtering never read empty texels.
    let mut valid: Vec<bool> = alb.iter().map(|p| p[3] > 0.0).collect();
    for _ in 0..64 {
        let mut next = valid.clone();
        let mut grew = false;
        for y in 0..height {
            for x in 0..size {
                if valid[y * size + x] {
                    continue;
                }
                let (mut a, mut n, mut o, mut c) = ([0.0f32; 3], Vec3::ZERO, 0.0f32, 0.0f32);
                for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1), (-1, -1), (1, 1), (-1, 1), (1, -1)] {
                    let (nx, ny) = (x as i32 + dx, y as i32 + dy);
                    if nx < 0 || ny < 0 || nx >= size as i32 || ny >= height as i32 {
                        continue;
                    }
                    let j = ny as usize * size + nx as usize;
                    if valid[j] {
                        for k in 0..3 {
                            a[k] += alb[j][k];
                        }
                        n += Vec3::new(nrm[j][0], nrm[j][1], nrm[j][2]);
                        o += ao[j];
                        c += 1.0;
                    }
                }
                if c > 0.0 {
                    let i = y * size + x;
                    alb[i] = [a[0] / c, a[1] / c, a[2] / c, 1.0];
                    let nn = n.normalize_or(Vec3::Z);
                    nrm[i] = [nn.x, nn.y, nn.z, 1.0];
                    ao[i] = o / c;
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
    let to8 = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    let albedo = alb.iter().flat_map(|p| [to8(linear_to_srgb(p[0])), to8(linear_to_srgb(p[1])), to8(linear_to_srgb(p[2])), 255]).collect();
    let normal = nrm.iter().flat_map(|p| [to8(p[0] * 0.5 + 0.5), to8(p[1] * 0.5 + 0.5), to8(p[2] * 0.5 + 0.5), 255]).collect();
    let ao8 = ao.iter().flat_map(|&o| {
        let v = to8(o.max(0.0));
        [v, v, v, 255]
    }).collect();
    Baked { size: uw.size, height: uw.height, albedo, normal, ao: ao8, misses, samples }
}

// ---------------------------------------------------------------------------------------------
// Output

fn write_dds(path: &Path, w: u32, h: u32, rgba: &[u8], kind: MipKind) -> Result<()> {
    let chain = mips::chain(w, h, rgba, 32, kind, Edge::Clamp);
    let blocks: Vec<Vec<u8>> = chain.iter().map(|(cw, ch, px)| lodforge_gpu::bc7_cpu::encode(&RgbaImage { width: *cw, height: *ch, rgba: px }, Bc7Quality::High)).collect();
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(path, lodforge_dds::write_bc7(w, h, &blocks))?;
    Ok(())
}

fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) -> Result<()> {
    std::fs::create_dir_all(path.parent().unwrap())?;
    let png = lodforge_viewer::headless::encode_png(rgba, w, h).map_err(|e| anyhow!(e))?;
    std::fs::write(path, png)?;
    Ok(())
}

/// One textured mesh in world space (for the standalone models and the test world).
struct OutMesh {
    name: String,
    pos: Vec<[f32; 3]>,
    nrm: Vec<[f32; 3]>,
    tan: Vec<[f32; 3]>,
    bit: Vec<[f32; 3]>,
    uv: Vec<[f32; 2]>,
    col: Vec<[u8; 4]>,
    tris: Vec<[u16; 3]>,
    textures: Vec<String>,
    shader: LightingShader,
}

impl OutMesh {
    fn moved(&self, by: Vec3, name: &str) -> OutMesh {
        OutMesh {
            name: name.into(),
            pos: self.pos.iter().map(|p| (Vec3::from(*p) + by).to_array()).collect(),
            nrm: self.nrm.clone(),
            tan: self.tan.clone(),
            bit: self.bit.clone(),
            uv: self.uv.clone(),
            col: self.col.clone(),
            tris: self.tris.clone(),
            textures: self.textures.clone(),
            shader: self.shader.clone(),
        }
    }
}

fn nine(d: &str, n: &str) -> Vec<String> {
    let mut v = vec![String::new(); 9];
    v[0] = d.into();
    v[1] = n.into();
    v
}

fn write_nif(path: &Path, meshes: &[OutMesh]) -> Result<()> {
    let mut nif = Nif::new();
    nif.header.block_types = vec!["BSFadeNode".into()];
    nif.header.block_type_index = vec![0];
    nif.add_block(Block::Node(Node { av: AvObject { net: ObjectNet::named("detail_bake"), ..Default::default() }, ..Default::default() }));
    let mut children = Vec::new();
    for m in meshes {
        let ts = nif.add_block(Block::TextureSet(m.textures.clone()));
        let mut sh = m.shader.clone();
        sh.texture_set = Some(ts);
        sh.flags2 |= SLSF2_VERTEX_COLORS;
        let si = nif.add_block(Block::LightingShader(sh));
        let (mut mn, mut mx) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for p in &m.pos {
            mn = mn.min(Vec3::from(*p));
            mx = mx.max(Vec3::from(*p));
        }
        let mut s = TriShape {
            av: AvObject { net: ObjectNet::named(&m.name), ..Default::default() },
            bound_center: ((mn + mx) * 0.5).to_array(),
            bound_radius: (mx - mn).length() * 0.5,
            shader_property: Some(si),
            positions: m.pos.clone(),
            uvs: m.uv.clone(),
            normals: m.nrm.clone(),
            tangents: m.tan.clone(),
            bitangents: m.bit.clone(),
            colors: m.col.clone(),
            triangles: m.tris.clone(),
            ..Default::default()
        };
        s.update_vertex_desc();
        children.push(Some(nif.add_block(Block::TriShape(s))));
    }
    if let Block::Node(r) = &mut nif.blocks[0] {
        r.children = children;
    }
    std::fs::create_dir_all(path.parent().unwrap())?;
    std::fs::write(path, nif.to_bytes().map_err(|e| anyhow!("{e}"))?)?;
    Ok(())
}

/// The LOD mesh as it is (its own UVs), for the "before" models.
fn before_mesh(vfs: &Vfs, rel: &str, textures: &[String]) -> Result<OutMesh> {
    let (_, bytes) = vfs.read(rel)?;
    let nif = lodforge_nif::parse(&bytes).map_err(|e| anyhow!("{rel}: {e}"))?;
    let mut o = OutMesh { name: String::new(), pos: vec![], nrm: vec![], tan: vec![], bit: vec![], uv: vec![], col: vec![], tris: vec![], textures: textures.to_vec(), shader: LightingShader::default() };
    o.shader = nif.blocks.iter().find_map(|b| if let Block::LightingShader(l) = b { Some(l.clone()) } else { None }).unwrap_or_default();
    for s in shapes_of(&nif, FileKind::Model) {
        if s.material.effect || s.indices.is_empty() || s.uvs.len() != s.positions.len() {
            continue;
        }
        let base = o.pos.len() as u16;
        let n = s.positions.len();
        o.pos.extend(&s.positions);
        o.uv.extend(&s.uvs);
        let nrm = if s.normals.len() == n { s.normals.clone() } else { vec![[0.0, 0.0, 1.0]; n] };
        let tris16: Vec<[u16; 3]> = s.indices.chunks_exact(3).map(|t| [t[0] as u16, t[1] as u16, t[2] as u16]).collect();
        let (t, b) = if s.tangents.len() == n && s.bitangents.len() == n { (s.tangents.clone(), s.bitangents.clone()) } else { lodforge_objlod::mesh::tangents_from_uv(&s.positions, &s.uvs, &nrm, &tris16) };
        o.nrm.extend(nrm);
        o.tan.extend(t);
        o.bit.extend(b);
        o.col.extend((0..n).map(|i| s.colors.get(i).copied().unwrap_or([255; 4])));
        o.tris.extend(tris16.iter().map(|t| [t[0] + base, t[1] + base, t[2] + base]));
    }
    Ok(o)
}

/// The full model as one mesh per shape, with PBR diffuse swapped for its `linear` LOD copy
/// (the brightness LOD shows), for the reference in the test world.
fn full_meshes(vfs: &Vfs, src: &Source) -> Vec<OutMesh> {
    let mut out = Vec::new();
    for s in &src.shapes {
        let m = &s.mesh;
        if m.positions.len() > 65535 {
            continue;
        }
        let mut tex = m.material.textures.clone();
        if s.pbr
            && let Some(d) = tex_path(&m.material, 0)
        {
            let lin = d.replace(".dds", "linear.dds");
            if vfs.find(&lin).is_some() {
                tex[0] = lin;
            }
        }
        let n = m.positions.len();
        let tris16: Vec<[u16; 3]> = m.indices.chunks_exact(3).map(|t| [t[0] as u16, t[1] as u16, t[2] as u16]).collect();
        let nrm = if m.normals.len() == n { m.normals.clone() } else { vec![[0.0, 0.0, 1.0]; n] };
        let (t, b) = if m.tangents.len() == n && m.bitangents.len() == n { (m.tangents.clone(), m.bitangents.clone()) } else { lodforge_objlod::mesh::tangents_from_uv(&m.positions, &m.uvs, &nrm, &tris16) };
        let mut sh = LightingShader { uv_scale: m.material.uv_scale, uv_offset: m.material.uv_offset, ..Default::default() };
        if !m.material.vertex_colors {
            sh.flags2 &= !SLSF2_VERTEX_COLORS;
        }
        out.push(OutMesh {
            name: "obj".into(),
            pos: m.positions.clone(),
            nrm,
            tan: t,
            bit: b,
            uv: m.uvs.clone(),
            col: if m.colors.len() == n && std::env::var_os("DETAIL_BAKE_NO_VC").is_none() { m.colors.clone() } else { vec![[255; 4]; n] },
            tris: tris16,
            textures: tex,
            shader: sh,
        });
    }
    out
}

/// The tiling textures a LOD model shows in the runtime: its own diffuse and normal map, with
/// the PBR twin's `linear` LOD copy for the diffuse and the PBR normal map when the load order
/// has them (as the PBR AltTextures do).
fn lod_textures(vfs: &Vfs, rel: &str) -> Result<(String, String)> {
    let (_, bytes) = vfs.read(rel)?;
    let nif = lodforge_nif::parse(&bytes).map_err(|e| anyhow!("{rel}: {e}"))?;
    let s = shapes_of(&nif, FileKind::Model).into_iter().find(|s| !s.material.effect && s.material.diffuse.is_some()).ok_or_else(|| anyhow!("{rel}: no textured shape"))?;
    let d = tex_path(&s.material, 0).unwrap_or_default();
    let n = tex_path(&s.material, 1).unwrap_or_default();
    let pbr = |p: &str| -> String { if p.contains("\\pbr\\") { p.to_string() } else { p.replacen("textures\\", "textures\\pbr\\", 1) } };
    let dl = pbr(&d).replace(".dds", "linear.dds");
    let d = if vfs.find(&dl).is_some() { dl } else { d };
    let n = if vfs.find(&pbr(&n)).is_some() { pbr(&n) } else { n };
    println!("before textures (from {rel}): {d}, {n}");
    Ok((d, n))
}

fn main() -> Result<()> {
    let a = parse_args()?;
    let vfs = Vfs::new(&a.instance, &a.profile)?;
    let (mut src, _) = load_source(&vfs, &a.full)?;
    let diag = (src.max - src.min).length();
    let ext = src.max - src.min;
    let ao_radius = a.ao_radius.unwrap_or((diag * 0.08).clamp(128.0, 1024.0));
    let search = a.search.unwrap_or((diag * 0.06).clamp(64.0, 512.0));
    println!("  extent {:.0} x {:.0} x {:.0}, AO radius {ao_radius:.0}, search {search:.0}", ext.x, ext.y, ext.z);
    let stem = Path::new(&a.full.replace('\\', "/")).file_stem().unwrap().to_string_lossy().to_lowercase();
    // Named after the first LOD model: one full model can have several LOD variants (_lod_0h / _lod_0l).
    let lod_stem = Path::new(&a.lods[0].path.replace('\\', "/")).file_stem().unwrap().to_string_lossy().to_lowercase();
    let tex_dir = format!("textures\\lodforge\\bake\\{lod_stem}");

    // "Before": the tiling texture at the size the runtime pages show it.
    let mut before_tex: Option<Vec<String>> = None;
    let auto = if a.before_diffuse.is_none() { Some(lod_textures(&vfs, &a.lods[0].path)?) } else { None };
    let bd = a.before_diffuse.clone().or(auto.as_ref().map(|t| t.0.clone()));
    let bn = a.before_normal.clone().or(auto.as_ref().map(|t| t.1.clone()));
    if let (Some(bd), Some(bn)) = (&bd, &bn) {
        let mut names = Vec::new();
        for (rel, kind, suffix) in [(bd, MipKind::Color, ""), (bn, MipKind::Normal, "_n")] {
            let (t, what) = Tex::load(&vfs, rel, a.before_size)?;
            let mut rgba: Vec<u8> = t.px.iter().flat_map(|p| p.map(|v| (v * 255.0 + 0.5) as u8)).collect();
            if t.w != a.before_size {
                rgba = mips::resize_rgba8(t.w, t.h, &rgba, (a.before_size, a.before_size), kind, Edge::Wrap);
            }
            for p in rgba.chunks_exact_mut(4) {
                p[3] = 255;
            }
            let name = format!("{tex_dir}\\before{suffix}.dds");
            write_dds(&a.out.join(&name), a.before_size, a.before_size, &rgba, kind)?;
            println!("before{suffix}: {what} -> {} px", a.before_size);
            names.push(name);
        }
        before_tex = Some(nine(&names[0], &names[1]));
    }

    let spacing = ext.x.max(ext.y) * 1.25;
    // Test world layout: one group per level, 100000 units apart along x (so a camera looking
    // north sees one group only); in each group full | before | after from west to east.
    const GROUP: f32 = 100_000.0;
    // DETAIL_BAKE_SNOW: draw the test world with the LOD snow material (full model objSnow, its
    // vertex alpha ~1; LOD models objSnowHD with alpha ~0.5: the same weight in the game's shader).
    let snow = std::env::var_os("DETAIL_BAKE_SNOW").is_some();
    let (full_name, lod_name) = if snow { ("objSnowHD", "objSnowHD") } else { ("obj", "obj") };
    let origin = Vec3::new(4096.0, 4096.0, 0.0) - Vec3::new(src.min.x, src.min.y, 0.0);
    let full = full_meshes(&vfs, &src);
    let mut world: Vec<OutMesh> = Vec::new();
    for row in 0..a.lods.len() {
        for m in &full {
            let mut fm = m.moved(origin + Vec3::new(row as f32 * GROUP, 0.0, 0.0), full_name);
            if snow {
                // Same HD weight as the LOD models (their vertex alpha 127).
                for c in &mut fm.col {
                    c[3] = 127;
                }
            }
            world.push(fm);
        }
    }
    let mut report = Vec::new();
    let mut cams: Vec<String> = Vec::new();
    for (row, l) in a.lods.iter().enumerate() {
        println!("\n== {} (target {:.3} texels/unit)", l.label, l.density);
        let lod = load_lod(&vfs, &l.path)?;
        let t0 = Instant::now();
        let uw = unwrap(&lod, l.density, a.max_size);
        println!("  unwrap: {} charts, {}x{} px at {:.3} texels/unit ({:.0} ms)", uw.charts, uw.size, uw.height, uw.density, t0.elapsed().as_secs_f64() * 1e3);
        load_source_textures(&vfs, &mut src, uw.density)?;
        let pos: Vec<[f32; 3]> = uw.verts.iter().map(|(s, _)| lod.pos[*s as usize].to_array()).collect();
        let nrm: Vec<[f32; 3]> = uw.verts.iter().map(|(s, _)| lod.nrm[*s as usize].to_array()).collect();
        let uv: Vec<[f32; 2]> = uw.verts.iter().map(|(_, u)| u.to_array()).collect();
        let tris16: Vec<[u16; 3]> = uw.tris.iter().map(|t| [t[0] as u16, t[1] as u16, t[2] as u16]).collect();
        // Bethesda layout (tangents_from_uv returns it): U in the bitangent field, V in the tangent field.
        let (tan, bit) = lodforge_objlod::mesh::tangents_from_uv(&pos, &uv, &nrm, &tris16);
        let t1 = Instant::now();
        // Fallback textures (the LOD model's own) at the mip this density needs.
        let mut ratios: Vec<f32> = lod
            .tris
            .iter()
            .filter_map(|t| {
                let p = t.map(|i| lod.pos[i as usize]);
                let u = t.map(|i| lod.uv[i as usize]);
                let wa = (p[1] - p[0]).cross(p[2] - p[0]).length();
                let ua = (u[1] - u[0]).perp_dot(u[2] - u[0]).abs();
                (wa > 1e-3 && ua > 0.0).then(|| (ua / wa).sqrt())
            })
            .collect();
        ratios.sort_by(f32::total_cmp);
        let lod_uv_per_unit = ratios.get(ratios.len() / 2).copied().unwrap_or(1.0 / 1024.0);
        let want = ((uw.density * SS as f32 / lod_uv_per_unit).ceil() as u32).next_power_of_two();
        let fb_d = match &bd { Some(p) => Some(Tex::load(&vfs, p, want)?.0), None => None };
        let fb_n = match &bn { Some(p) => Tex::load(&vfs, p, want).ok().map(|t| t.0), None => None };
        let fallback = fb_d.as_ref().map(|d| Fallback { diffuse: d, normal: fb_n.as_ref() });
        let b = bake(&src, &lod, &uw, &tan, &bit, search, ao_radius, a.ao_rays, a.ao_strength, fallback.as_ref());
        println!("  bake: {:.1} s, {} samples, {:.2}% without a hit", t1.elapsed().as_secs_f64(), b.samples, 100.0 * b.misses as f64 / b.samples.max(1) as f64);
        let dname = format!("{tex_dir}\\{}.dds", l.label);
        let nname = format!("{tex_dir}\\{}_n.dds", l.label);
        write_dds(&a.out.join(&dname), b.size, b.height, &b.albedo, MipKind::Color)?;
        write_dds(&a.out.join(&nname), b.size, b.height, &b.normal, MipKind::Normal)?;
        let prev = a.out.join("preview");
        write_png(&prev.join(format!("{}_albedo.png", l.label)), b.size, b.height, &b.albedo)?;
        write_png(&prev.join(format!("{}_normal.png", l.label)), b.size, b.height, &b.normal)?;
        write_png(&prev.join(format!("{}_ao.png", l.label)), b.size, b.height, &b.ao)?;
        let after = OutMesh {
            name: "obj".into(),
            pos,
            nrm,
            tan,
            bit,
            uv,
            col: uw.verts.iter().map(|(s, _)| [255, 255, 255, lod.alpha[*s as usize]]).collect(),
            tris: tris16,
            textures: nine(&dname, &nname),
            shader: LightingShader { texture_clamp_mode: 0, ..lod.shader.clone().unwrap_or_default() },
        };
        write_nif(&a.out.join(format!("meshes\\lod\\lodforge\\bake\\{stem}_{}.nif", l.label)), std::slice::from_ref(&after))?;
        // The same model at the path it replaces, for `runtime-statics-pack --substitute`.
        write_nif(&a.out.join("substitute").join(l.path.to_ascii_lowercase()), std::slice::from_ref(&after))?;
        let gx = row as f32 * GROUP;
        if let Some(bt) = &before_tex {
            let before = before_mesh(&vfs, &l.path, bt)?;
            write_nif(&a.out.join(format!("meshes\\lod\\lodforge\\bake\\{stem}_{}_before.nif", l.label)), std::slice::from_ref(&before))?;
            world.push(before.moved(origin + Vec3::new(gx + spacing, 0.0, 0.0), lod_name));
        }
        world.push(after.moved(origin + Vec3::new(gx + 2.0 * spacing, 0.0, 0.0), lod_name));
        let c = origin + (src.min + src.max) * 0.5 + Vec3::new(gx + spacing, 0.0, 0.0);
        println!("  group centre (before model): {:.0}, {:.0}, {:.0}", c.x, c.y, c.z);
        // Cameras for the shot script: centred close-ups per column, then the in-game distance
        // of this level (LOD4 15k, LOD8 60k, LOD16 150k; by row order).
        let size = ext.x.max(ext.y);
        for (i, col) in ["full", "before", "after"].iter().enumerate() {
            let x = c.x + (i as f32 - 1.0) * spacing;
            cams.push(format!("close {} {col} {:.0},{:.0},{:.0},0,-6", l.label, x, c.y - size * 2.0, c.z + size * 0.3));
        }
        let dist = [15_000.0, 60_000.0, 150_000.0][row.min(2)];
        cams.push(format!("dist {} all {:.0},{:.0},{:.0},0,0 {:.0}", l.label, c.x, c.y - dist, c.z + 800.0, spacing + size * 0.7));
        report.push(format!("{}: {} tris, {}x{} px, {:.3} texels/unit ({:.1} per 100 units)", l.label, lod.tris.len(), b.size, b.height, uw.density, uw.density * 100.0));
        let _ = &lod.name;
    }
    write_nif(&a.out.join("meshes\\terrain\\baketest\\objects\\baketest.4.0.0.bto"), &world)?;
    std::fs::write(a.out.join("cameras.txt"), cams.join("\n") + "\n")?;
    println!("\nlayout: x = full | before | after, spacing {spacing:.0}; groups {GROUP:.0} apart (west to east): {}", a.lods.iter().map(|l| l.label.as_str()).collect::<Vec<_>>().join(", "));
    println!("origin of the full model: {:.0}, {:.0}", origin.x + src.min.x, origin.y + src.min.y);
    for r in report {
        println!("  {r}");
    }
    Ok(())
}
