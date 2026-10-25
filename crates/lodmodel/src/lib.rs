// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! LOD models generated from full models: LODForge's own replacement for the hand-made LOD meshes
//! of DynDOLOD Resources (docs/re/own-lod-resources.md). Nothing here reads or derives from those
//! assets; the only input is the full model the load order uses.
//!
//! A level is made in four steps:
//! 1. **Collect.** Every visible, non-skinned BSTriShape (or LE NiTriShape) under the root, with
//!    its node transforms applied, so the result has one flat level of shapes. Skipped: hidden
//!    nodes, editor markers, all but the first child of switch nodes, effect-shader shapes (water,
//!    mist, fire: those belong to dynamic LOD), decals, alpha-blended (not alpha-tested) shapes,
//!    shapes without a lighting shader.
//! 2. **Merge.** Shapes with the same shader, textures and alpha property become one mesh.
//! 3. **Simplify.** All merged meshes together through meshoptimizer's attribute-aware edge
//!    collapse (positions, normals, UVs), with an absolute error limit per level and connected
//!    parts smaller than twice that limit removed first; a second, triangle-count pass caps levels that stay too dense.
//!    Collapses keep a subset of the original vertices, so UVs, normals, tangents and vertex
//!    colours are the source's values; UV seams and material borders stay where they were.
//! 4. **Write.** One BSTriShape per material with the source's shader and texture set (UVs are
//!    unchanged, so the object LOD atlas and tiling rules treat it like the full model), under a
//!    BSFadeNode root with identity transforms.

use std::collections::HashMap;

use lodforge_nif::{AvObject, Block, LightingShader, Nif, Node, ObjectNet, Transform, TriShape, VertexDesc};

/// How far one level may simplify.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LevelParams {
    /// Largest geometric error in game units (world-space, unscaled model).
    pub max_error: f32,
    /// Triangle cap as a share of the source (applied after the error pass).
    pub max_ratio: f32,
    /// Never go below this many triangles through the ratio cap.
    pub min_triangles: usize,
    /// Weight of normals relative to positions (positions are normalised to the model's extent).
    pub normal_weight: f32,
    /// Weight of UVs.
    pub uv_weight: f32,
    /// Allow collapses across UV / normal seams and material borders (smears texture
    /// coordinates along the seam; fine at a distance).
    pub permissive: bool,
}

/// Defaults for model levels 0, 1, 2 (object LOD 4, 8, 16). LOD4 starts around 5 cells away
/// (fBlockLevel0Distance), where one pixel at 1080p covers roughly 10..20 units; each further
/// level doubles the distance.
pub const LEVELS: [LevelParams; 3] = [
    LevelParams { max_error: 32.0, max_ratio: 0.3, min_triangles: 12, normal_weight: 0.5, uv_weight: 0.05, permissive: true },
    LevelParams { max_error: 56.0, max_ratio: 0.15, min_triangles: 8, normal_weight: 0.25, uv_weight: 0.02, permissive: true },
    LevelParams { max_error: 140.0, max_ratio: 0.08, min_triangles: 4, normal_weight: 0.1, uv_weight: 0.01, permissive: true },
];

/// Bumped whenever generation changes the output for the same input (cache key). 2: broken
/// source UVs repaired ([`repair_uvs`]).
pub const GENERATOR_VERSION: u32 = 2;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stats {
    pub source_triangles: usize,
    pub triangles: usize,
    pub source_shapes: usize,
    pub shapes: usize,
    /// Shapes left out (skinned, effect, decal, blended, hidden, no shader).
    pub skipped_shapes: usize,
    /// Geometric error of the error pass (game units).
    pub error: f32,
}

/// Why a model produced nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Skip {
    Parse(String),
    /// No shape survived collection.
    NoGeometry,
}

/// One merged material: source shader blocks plus geometry in model space.
#[derive(Default)]
struct Mesh {
    shader: Option<LightingShader>,
    textures: Vec<String>,
    alpha: Option<(u16, u8)>,
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    tangents: Vec<[f32; 3]>,
    bitangents: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    colors: Vec<[u8; 4]>,
    triangles: Vec<[u32; 3]>,
    has: [bool; 5],
    /// Name of the first source shape merged into this mesh.
    source_name: Option<String>,
}

/// Affine transform as NIF stores it: v' = R v * s + t, R row-major.
#[derive(Clone, Copy)]
struct Xf {
    r: [[f32; 3]; 3],
    s: f32,
    t: [f32; 3],
}

impl Xf {
    const IDENTITY: Xf = Xf { r: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], s: 1.0, t: [0.0; 3] };

    fn of(t: &Transform) -> Xf {
        Xf { r: t.rotation, s: t.scale, t: t.translation }
    }

    fn rot(&self, v: [f32; 3]) -> [f32; 3] {
        let r = &self.r;
        [r[0][0] * v[0] + r[0][1] * v[1] + r[0][2] * v[2], r[1][0] * v[0] + r[1][1] * v[1] + r[1][2] * v[2], r[2][0] * v[0] + r[2][1] * v[1] + r[2][2] * v[2]]
    }

    fn point(&self, v: [f32; 3]) -> [f32; 3] {
        let p = self.rot(v);
        [p[0] * self.s + self.t[0], p[1] * self.s + self.t[1], p[2] * self.s + self.t[2]]
    }

    /// The child's local transform `self` placed under `parent`.
    fn then(&self, parent: &Xf) -> Xf {
        let mut r = [[0.0f32; 3]; 3];
        for (i, row) in r.iter_mut().enumerate() {
            for (j, v) in row.iter_mut().enumerate() {
                *v = (0..3).map(|k| parent.r[i][k] * self.r[k][j]).sum();
            }
        }
        Xf { r, s: self.s * parent.s, t: parent.point(self.t) }
    }
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > 1e-12 { [v[0] / l, v[1] / l, v[2] / l] } else { v }
}

/// BSLightingShaderProperty flags1: decal, dynamic decal.
const SLSF1_DECAL: u32 = 0x0400_0000;
const SLSF1_DYNAMIC_DECAL: u32 = 0x0800_0000;

fn is_marker(name: Option<&str>) -> bool {
    name.is_some_and(|n| n.to_ascii_lowercase().contains("editormarker"))
}

fn collect(nif: &Nif, idx: usize, parent: Xf, meshes: &mut Vec<Mesh>, keys: &mut HashMap<String, usize>, st: &mut Stats) {
    let Some(Block::Node(node)) = nif.blocks.get(idx) else { return };
    if node.av.flags & 1 != 0 || is_marker(node.av.net.name.as_deref()) {
        return;
    }
    let xf = Xf::of(&node.av.transform).then(&parent);
    let switch = nif.block_type(idx) == "NiSwitchNode";
    for c in node.children.iter().flatten().take(if switch { 1 } else { usize::MAX }) {
        let ci = *c as usize;
        match nif.blocks.get(ci) {
            Some(Block::Node(_)) => collect(nif, ci, xf, meshes, keys, st),
            Some(Block::TriShape(s)) => {
                st.source_shapes += 1;
                st.source_triangles += s.triangles.len();
                if !add_shape(nif, s, &xf, meshes, keys) {
                    st.skipped_shapes += 1;
                }
            }
            _ => {}
        }
    }
}

fn add_shape(nif: &Nif, s: &TriShape, parent: &Xf, meshes: &mut Vec<Mesh>, keys: &mut HashMap<String, usize>) -> bool {
    if s.skin.is_some() || s.av.flags & 1 != 0 || s.triangles.is_empty() || s.positions.is_empty() || is_marker(s.av.net.name.as_deref()) {
        return false;
    }
    let Some(Block::LightingShader(sh)) = s.shader_property.and_then(|r| nif.blocks.get(r as usize)) else { return false };
    if sh.flags1 & (SLSF1_DECAL | SLSF1_DYNAMIC_DECAL) != 0 {
        return false;
    }
    let alpha = match s.alpha_property.and_then(|r| nif.blocks.get(r as usize)) {
        Some(Block::Alpha { flags, threshold, .. }) => {
            let blend = flags & 1 != 0;
            let test = flags & 0x200 != 0;
            if blend && !test {
                return false;
            }
            Some((*flags, *threshold))
        }
        _ => None,
    };
    let textures: Vec<String> = sh
        .texture_set
        .and_then(|r| match nif.blocks.get(r as usize) {
            Some(Block::TextureSet(t)) => Some(t.clone()),
            _ => None,
        })
        .unwrap_or_default();
    let mut shader = sh.clone();
    shader.net = ObjectNet { name: shader.net.name.clone(), ..Default::default() };
    shader.texture_set = None;
    let n = s.positions.len();
    let has = [true, s.uvs.len() == n, s.normals.len() == n, s.tangents.len() == n, s.colors.len() == n];
    let key = format!("{shader:?}|{textures:?}|{alpha:?}|{has:?}");
    let mi = *keys.entry(key).or_insert_with(|| {
        meshes.push(Mesh { shader: Some(shader), textures, alpha, has, source_name: s.av.net.name.clone(), ..Default::default() });
        meshes.len() - 1
    });
    let m = &mut meshes[mi];
    let xf = Xf::of(&s.av.transform).then(parent);
    let base = m.positions.len() as u32;
    m.positions.extend(s.positions.iter().map(|&p| xf.point(p)));
    let rot = |v: [f32; 3]| normalize(xf.rot(v));
    if has[2] {
        m.normals.extend(s.normals.iter().map(|&v| rot(v)));
    }
    if has[3] {
        m.tangents.extend(s.tangents.iter().map(|&v| rot(v)));
        if s.bitangents.len() == n {
            m.bitangents.extend(s.bitangents.iter().map(|&v| rot(v)));
        } else {
            m.bitangents.extend(std::iter::repeat_n([0.0; 3], n));
        }
    }
    if has[1] {
        // Broken UVs in the source (USSEP's burntrubblenodirt05.nif stores infinite ones and
        // 56,352) would reach the LOD model, poison the simplifier's UV error and hang anything
        // that cuts along UV repeat lines.
        let mut uvs = s.uvs.clone();
        repair_uvs(&mut uvs, &s.triangles);
        m.uvs.extend_from_slice(&uvs);
    }
    if has[4] {
        m.colors.extend_from_slice(&s.colors);
    }
    m.triangles.extend(
        s.triangles
            .iter()
            .filter(|t| t[0] != t[1] && t[1] != t[2] && t[0] != t[2] && t.iter().all(|&i| (i as usize) < n))
            .map(|t| [base + u32::from(t[0]), base + u32::from(t[1]), base + u32::from(t[2])]),
    );
    true
}

/// UVs beyond this many texture repeats are broken data, not tiling.
pub const MAX_SANE_UV: f32 = 1024.0;

/// Replaces non-finite UVs and UVs beyond [`MAX_SANE_UV`] by the mean of the valid UVs of the
/// vertices sharing a triangle with them (0, 0 when there are none). Returns how many it replaced.
pub fn repair_uvs(uvs: &mut [[f32; 2]], triangles: &[[u16; 3]]) -> usize {
    let bad = |uv: &[f32; 2]| !(uv[0].is_finite() && uv[1].is_finite()) || uv[0].abs() > MAX_SANE_UV || uv[1].abs() > MAX_SANE_UV;
    let broken: Vec<usize> = (0..uvs.len()).filter(|&i| bad(&uvs[i])).collect();
    if broken.is_empty() {
        return 0;
    }
    let mut fixed = Vec::with_capacity(broken.len());
    for &i in &broken {
        let (mut sum, mut n) = ([0.0f32; 2], 0u32);
        for t in triangles.iter().filter(|t| t.iter().any(|&v| usize::from(v) == i)) {
            for &v in t {
                let uv = uvs.get(usize::from(v)).copied().unwrap_or([f32::NAN; 2]);
                if usize::from(v) != i && !bad(&uv) {
                    sum = [sum[0] + uv[0], sum[1] + uv[1]];
                    n += 1;
                }
            }
        }
        fixed.push(if n > 0 { [sum[0] / n as f32, sum[1] / n as f32] } else { [0.0; 2] });
    }
    for (&i, uv) in broken.iter().zip(fixed) {
        uvs[i] = uv;
    }
    broken.len()
}

/// Simplifies all meshes together; returns per-mesh index lists (mesh-local) and the error of
/// the error pass.
fn simplify(meshes: &[Mesh], p: &LevelParams) -> (Vec<Vec<u32>>, f32) {
    // One vertex buffer across meshes. Vertices are never shared between meshes, so material
    // borders are attribute seams for meshoptimizer and stay in place.
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut attrs: Vec<f32> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut owner: Vec<u32> = Vec::new();
    let mut starts: Vec<u32> = Vec::new();
    for (mi, m) in meshes.iter().enumerate() {
        let base = positions.len() as u32;
        starts.push(base);
        // Exact duplicates (same position and attributes) become one vertex, else
        // meshoptimizer treats them as seams and will not collapse across them.
        let canon = weld_duplicates(m);
        for v in 0..m.positions.len() {
            positions.push(m.positions[v]);
            let n = m.normals.get(v).copied().unwrap_or([0.0, 0.0, 1.0]);
            let uv = m.uvs.get(v).copied().unwrap_or([0.0, 0.0]);
            attrs.extend_from_slice(&[n[0], n[1], n[2], uv[0], uv[1]]);
            owner.push(mi as u32);
        }
        indices.extend(m.triangles.iter().flat_map(|t| t.iter().map(|&i| base + canon[i as usize])));
    }
    let indices = drop_small_parts(&positions, &indices, 2.0 * p.max_error);
    let weights = [p.normal_weight, p.normal_weight, p.normal_weight, p.uv_weight, p.uv_weight];
    // Vertices on a material border (coincident with a vertex of another mesh) keep the border
    // in permissive mode.
    let mut meshes_at: HashMap<[u32; 3], u32> = HashMap::new();
    for (v, pos) in positions.iter().enumerate() {
        let e = meshes_at.entry(pos.map(f32::to_bits)).or_insert(owner[v]);
        if *e != owner[v] {
            *e = u32::MAX;
        }
    }
    let locks: Vec<meshopt::SimplifyVertex> = positions.iter().map(|pos| if meshes_at[&pos.map(f32::to_bits)] == u32::MAX { meshopt::SimplifyVertex::Protect } else { meshopt::SimplifyVertex::None }).collect();
    let source = indices.len() / 3;
    let opts = if p.permissive { meshopt::SimplifyOptions::ErrorAbsolute | meshopt::SimplifyOptions::Permissive } else { meshopt::SimplifyOptions::ErrorAbsolute };
    let mut err = 0.0f32;
    let mut out = meshopt::simplify_with_attributes_and_locks_decoder(&indices, &positions, &attrs, &weights, 5 * 4, &locks, p.min_triangles.min(source) * 3, p.max_error, opts, Some(&mut err));
    let cap = ((source as f32 * p.max_ratio).ceil() as usize).max(p.min_triangles);
    if out.len() / 3 > cap {
        out = meshopt::simplify_with_attributes_and_locks_decoder(&out, &positions, &attrs, &weights, 5 * 4, &locks, cap * 3, f32::MAX, opts, None);
    }
    let mut per: Vec<Vec<u32>> = vec![Vec::new(); meshes.len()];
    for t in out.chunks_exact(3) {
        let mi = owner[t[0] as usize] as usize;
        if !t.iter().all(|&i| owner[i as usize] as usize == mi) && std::env::var_os("LODFORGE_LODMODEL_DEBUG").is_some() {
            eprintln!("mixed triangle {:?}", t.iter().map(|&i| owner[i as usize]).collect::<Vec<_>>());
        }
        if t.iter().all(|&i| owner[i as usize] as usize == mi) {
            per[mi].extend(t.iter().map(|&i| i - starts[mi]));
        }
    }
    (per, err)
}

/// For each vertex of a mesh, the first vertex with identical position and attributes.
fn weld_duplicates(m: &Mesh) -> Vec<u32> {
    let mut seen: HashMap<Vec<u32>, u32> = HashMap::new();
    (0..m.positions.len())
        .map(|v| {
            let mut k: Vec<u32> = m.positions[v].iter().map(|f| f.to_bits()).collect();
            k.extend(m.normals.get(v).into_iter().flatten().map(|f| f.to_bits()));
            k.extend(m.uvs.get(v).into_iter().flatten().map(|f| f.to_bits()));
            k.extend(m.tangents.get(v).into_iter().flatten().map(|f| f.to_bits()));
            k.extend(m.colors.get(v).map(|c| u32::from_le_bytes(*c)));
            *seen.entry(k).or_insert(v as u32)
        })
        .collect()
}

/// Removes connected parts (triangles joined through shared or coincident vertices) whose
/// bounding box diagonal is below `min_size`: they are too small to see at the level's distance.
/// The largest part always stays.
fn drop_small_parts(positions: &[[f32; 3]], indices: &[u32], min_size: f32) -> Vec<u32> {
    // Weld coincident positions (0.01-unit grid) so parts split only by UV or normal seams or
    // material borders count as one.
    let mut weld: HashMap<[i64; 3], u32> = HashMap::new();
    let key = |p: &[f32; 3]| p.map(|v| (f64::from(v) * 100.0).round() as i64);
    let id: Vec<u32> = positions
        .iter()
        .map(|p| {
            let n = weld.len() as u32;
            *weld.entry(key(p)).or_insert(n)
        })
        .collect();
    let mut parent: Vec<u32> = (0..weld.len() as u32).collect();
    fn find(parent: &mut [u32], mut x: u32) -> u32 {
        while parent[x as usize] != x {
            parent[x as usize] = parent[parent[x as usize] as usize];
            x = parent[x as usize];
        }
        x
    }
    for t in indices.chunks_exact(3) {
        let a = find(&mut parent, id[t[0] as usize]);
        for &v in &t[1..] {
            let b = find(&mut parent, id[v as usize]);
            if a != b {
                parent[b as usize] = a;
            }
        }
    }
    let mut boxes: HashMap<u32, ([f32; 3], [f32; 3])> = HashMap::new();
    for &v in indices {
        let r = find(&mut parent, id[v as usize]);
        let p = positions[v as usize];
        let e = boxes.entry(r).or_insert((p, p));
        for k in 0..3 {
            e.0[k] = e.0[k].min(p[k]);
            e.1[k] = e.1[k].max(p[k]);
        }
    }
    let diag = |b: &([f32; 3], [f32; 3])| ((b.1[0] - b.0[0]).powi(2) + (b.1[1] - b.0[1]).powi(2) + (b.1[2] - b.0[2]).powi(2)).sqrt();
    let largest = boxes.iter().max_by(|a, b| diag(a.1).total_cmp(&diag(b.1))).map(|(r, _)| *r);
    let keep: std::collections::HashSet<u32> = boxes.iter().filter(|(r, b)| Some(**r) == largest || diag(b) >= min_size).map(|(r, _)| *r).collect();
    indices.chunks_exact(3).filter(|t| keep.contains(&find(&mut parent, id[t[0] as usize]))).flatten().copied().collect()
}

fn bounds(points: &[[f32; 3]]) -> ([f32; 3], f32) {
    if points.is_empty() {
        return ([0.0; 3], 0.0);
    }
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for p in points {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let c = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0];
    let r = points.iter().map(|p| ((p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2) + (p[2] - c[2]).powi(2)).sqrt()).fold(0.0f32, f32::max);
    (c, r)
}

/// Builds a LOD model from a full model's bytes.
/// [generate], but every output shape keeps the name of the first source shape merged into it
/// (so per-reference side files keyed by shape name, such as DynDOLOD's AltTextures, apply to
/// the generated model as to its source).
pub fn generate_keep_names(source: &[u8], p: &LevelParams, name: &str) -> Result<(Vec<u8>, Stats), Skip> {
    let nif = lodforge_nif::parse(source).map_err(|e| Skip::Parse(e.to_string()))?;
    let (nif, st) = generate_impl(&nif, p, name, true)?;
    let bytes = nif.to_bytes().map_err(|e| Skip::Parse(e.to_string()))?;
    Ok((bytes, st))
}

pub fn generate(full: &[u8], p: &LevelParams, name: &str) -> Result<(Vec<u8>, Stats), Skip> {
    let nif = lodforge_nif::parse(full).map_err(|e| Skip::Parse(e.to_string()))?;
    let (nif, st) = generate_nif(&nif, p, name)?;
    let bytes = nif.to_bytes().map_err(|e| Skip::Parse(e.to_string()))?;
    Ok((bytes, st))
}

pub fn generate_nif(nif: &Nif, p: &LevelParams, name: &str) -> Result<(Nif, Stats), Skip> {
    generate_impl(nif, p, name, false)
}

fn generate_impl(nif: &Nif, p: &LevelParams, name: &str, keep_names: bool) -> Result<(Nif, Stats), Skip> {
    let mut st = Stats::default();
    let mut meshes = Vec::new();
    let mut keys = HashMap::new();
    let root = nif.roots.first().copied().flatten().unwrap_or(0) as usize;
    match nif.blocks.get(root) {
        Some(Block::Node(_)) => collect(nif, root, Xf::IDENTITY, &mut meshes, &mut keys, &mut st),
        Some(Block::TriShape(s)) => {
            st.source_shapes += 1;
            st.source_triangles += s.triangles.len();
            if !add_shape(nif, s, &Xf::IDENTITY, &mut meshes, &mut keys) {
                st.skipped_shapes += 1;
            }
        }
        _ => {}
    }
    meshes.retain(|m| !m.triangles.is_empty());
    if meshes.is_empty() {
        return Err(Skip::NoGeometry);
    }
    let (per, err) = simplify(&meshes, p);
    if std::env::var_os("LODFORGE_LODMODEL_DEBUG").is_some() {
        eprintln!("meshes {} tris {:?} -> {:?} err {err}", meshes.len(), meshes.iter().map(|m| m.triangles.len()).collect::<Vec<_>>(), per.iter().map(|v| v.len() / 3).collect::<Vec<_>>());
    }
    st.error = err;

    let mut out = Nif::new();
    out.header.block_types = vec!["BSFadeNode".into()];
    out.header.block_type_index = vec![0];
    out.add_block(Block::Node(Node { av: AvObject { net: ObjectNet::named(name), ..Default::default() }, ..Default::default() }));
    let mut children = Vec::new();
    for (m, idx) in meshes.iter().zip(per) {
        if idx.is_empty() {
            continue;
        }
        // Compact the vertices this mesh still uses (u16 indices: split never needed at LOD
        // densities, extra triangles are dropped if a mesh ever exceeds 65535 vertices).
        let mut remap: HashMap<u32, u16> = HashMap::new();
        let mut order: Vec<usize> = Vec::new();
        let mut tris: Vec<[u16; 3]> = Vec::with_capacity(idx.len() / 3);
        for t in idx.chunks_exact(3) {
            if order.len() + 3 > 65535 {
                break;
            }
            let mut tri = [0u16; 3];
            for (k, &v) in t.iter().enumerate() {
                let next = order.len() as u16;
                tri[k] = *remap.entry(v).or_insert_with(|| {
                    order.push(v as usize);
                    next
                });
            }
            tris.push(tri);
        }
        let pick3 = |src: &Vec<[f32; 3]>| -> Vec<[f32; 3]> { order.iter().map(|&i| src[i]).collect() };
        let positions = pick3(&m.positions);
        let (center, radius) = bounds(&positions);
        let ts = out.add_block(Block::TextureSet(m.textures.clone()));
        let mut sh = m.shader.clone().unwrap_or_default();
        sh.texture_set = Some(ts);
        let si = out.add_block(Block::LightingShader(sh));
        let ai = m.alpha.map(|(flags, threshold)| out.add_block(Block::Alpha { net: ObjectNet::default(), flags, threshold }));
        let shape_name = if keep_names { m.source_name.clone().unwrap_or_else(|| name.to_string()) } else { name.to_string() };
        let shape = TriShape {
            av: AvObject { net: ObjectNet::named(&shape_name), ..Default::default() },
            bound_center: center,
            bound_radius: radius,
            shader_property: Some(si),
            alpha_property: ai,
            vertex_desc: VertexDesc::with_attributes(true, m.has[1], m.has[2], m.has[3], m.has[4]).0,
            uvs: if m.has[1] { order.iter().map(|&i| m.uvs[i]).collect() } else { Vec::new() },
            normals: if m.has[2] { pick3(&m.normals) } else { Vec::new() },
            tangents: if m.has[3] { pick3(&m.tangents) } else { Vec::new() },
            bitangents: if m.has[3] { pick3(&m.bitangents) } else { Vec::new() },
            colors: if m.has[4] { order.iter().map(|&i| m.colors[i]).collect() } else { Vec::new() },
            positions,
            triangles: tris,
            ..Default::default()
        };
        st.triangles += shape.triangles.len();
        st.shapes += 1;
        children.push(Some(out.add_block(Block::TriShape(shape))));
    }
    if children.is_empty() {
        return Err(Skip::NoGeometry);
    }
    if let Block::Node(r) = &mut out.blocks[0] {
        r.children = children;
    }
    Ok((out, st))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Broken source UVs (infinite, NaN, 56,352) take their triangle neighbours' mean; good ones
    /// are kept, an isolated broken one becomes 0, 0.
    #[test]
    fn broken_uvs_are_repaired() {
        let mut uvs = [[0.0, 0.0], [1.0, 0.0], [f32::INFINITY, 0.5], [0.0, 56352.0], [f32::NAN, 1.0], [3.0, 2.0]];
        let tris = [[0, 1, 2], [1, 2, 3], [0, 1, 5]];
        assert_eq!(repair_uvs(&mut uvs, &tris), 3);
        // 2 shares triangles with 0 and 1 (valid) and 3 (broken): mean of 0, 1, 1 -> (2/3, 0).
        assert!((uvs[2][0] - 2.0 / 3.0).abs() < 1e-6 && uvs[2][1] == 0.0, "{:?}", uvs[2]);
        // 3 only touches 1 and 2; 2 was broken when the means were taken.
        assert_eq!(uvs[3], [1.0, 0.0]);
        assert_eq!(uvs[4], [0.0, 0.0]);
        assert_eq!(uvs[5], [3.0, 2.0]);
        assert!(uvs.iter().all(|uv| uv[0].is_finite() && uv[1].is_finite()));
        assert_eq!(repair_uvs(&mut uvs, &tris), 0);
    }

    /// A subdivided flat grid with one shader collapses to a few triangles and round-trips.
    #[test]
    fn grid_simplifies() {
        let n = 20usize;
        let mut s = TriShape::default();
        for y in 0..=n {
            for x in 0..=n {
                s.positions.push([x as f32 * 10.0, y as f32 * 10.0, 0.0]);
                s.uvs.push([x as f32 / n as f32, y as f32 / n as f32]);
                s.normals.push([0.0, 0.0, 1.0]);
            }
        }
        let w = (n + 1) as u16;
        for y in 0..n {
            for x in 0..n {
                let i = (y * (n + 1) + x) as u16;
                s.triangles.push([i, i + 1, i + w]);
                s.triangles.push([i + 1, i + w + 1, i + w]);
            }
        }
        let mut nif = Nif::new();
        nif.add_block(Block::Node(Node::default()));
        let ts = nif.add_block(Block::TextureSet(vec!["textures\\a.dds".into()]));
        let sh = nif.add_block(Block::LightingShader(LightingShader { texture_set: Some(ts), ..Default::default() }));
        s.shader_property = Some(sh);
        s.vertex_desc = VertexDesc::with_attributes(true, true, true, false, false).0;
        let si = nif.add_block(Block::TriShape(s));
        if let Block::Node(r) = &mut nif.blocks[0] {
            r.children = vec![Some(si)];
        }
        let (out, st) = generate_nif(&nif, &LEVELS[0], "t").unwrap();
        assert_eq!(st.source_triangles, 800);
        assert!(st.triangles <= 96 && st.triangles >= 2, "{st:?}");
        let bytes = out.to_bytes().unwrap();
        let back = lodforge_nif::parse(&bytes).unwrap();
        let tris: usize = back.blocks.iter().map(|b| if let Block::TriShape(s) = b { s.triangles.len() } else { 0 }).sum();
        assert_eq!(tris, st.triangles);
    }
}

#[cfg(test)]
mod file_tests {
    /// `LODFORGE_NIF=<file> cargo test -p lodforge-lodmodel real_file -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_file() {
        let path = std::env::var("LODFORGE_NIF").unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let nif = lodforge_nif::parse(&bytes).unwrap();
        println!("roots {:?}", nif.roots);
        for (i, b) in nif.blocks.iter().enumerate() {
            match b {
                lodforge_nif::Block::Node(n) => println!("{i} {} flags {:#x} children {:?}", nif.block_type(i), n.av.flags, n.children),
                lodforge_nif::Block::TriShape(s) if std::env::var_os("LODFORGE_ZRANGE").is_some() => println!("{i} zrange {:.0} {:.0}", s.positions.iter().map(|p| p[2]).fold(f32::MAX, f32::min), s.positions.iter().map(|p| p[2]).fold(f32::MIN, f32::max)),
                lodforge_nif::Block::TriShape(s) => println!("{i} shape flags {:#x} skin {:?} shader {:?} alpha {:?} v {} uv {} n {} t {} c {} tris {}", s.av.flags, s.skin, s.shader_property, s.alpha_property, s.positions.len(), s.uvs.len(), s.normals.len(), s.tangents.len(), s.colors.len(), s.triangles.len()),
                lodforge_nif::Block::LightingShader(l) => println!("{i} shader {:#x} {:#x}", l.flags1, l.flags2),
                _ => println!("{i} {}", nif.block_type(i)),
            }
        }
        for (k, p) in super::LEVELS.iter().enumerate() {
            println!("level {k}: {:?}", super::generate_nif(&nif, p, "x").map(|r| r.1));
        }
    }
}
