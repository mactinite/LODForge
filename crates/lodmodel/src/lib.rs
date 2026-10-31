// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! LOD models generated from full models: LODForge's own replacement for the hand-made LOD meshes
//! of DynDOLOD Resources (docs/re/own-lod-resources.md). Nothing here reads or derives from those
//! assets; the only input is the full model the load order uses.
//!
//! A level is made in four steps:
//! 1. **Collect.** Every visible, non-skinned BSTriShape (or LE NiTriShape) under the root, with
//!    its node transforms applied, so the result has one flat level of shapes. The root's own
//!    transform is left out: the engine replaces a loaded model's root transform with the
//!    reference's placement, so a model whose root carries one (a Blender export with its
//!    origin moved, every shape translated back) draws without it. Skipped: hidden
//!    nodes, editor markers, all but the first child of switch nodes, effect-shader shapes (water,
//!    mist, fire: those belong to dynamic LOD), decals, alpha-blended (not alpha-tested) shapes,
//!    shapes without a lighting shader.
//! 2. **Merge.** Shapes with the same shader, textures and alpha property become one mesh.
//! 3. **Simplify.** All merged meshes together through meshoptimizer's attribute-aware edge
//!    collapse (positions, normals, UVs), with an absolute error limit per level and connected
//!    parts smaller than twice that limit removed first (smallest first, never more than a fifth
//!    of the model's area: a deck's planks are each small and together the deck); a second,
//!    triangle-count pass caps levels that stay too dense.
//!    Each part keeps the vertices nearest its bounding box corners locked (thin axes folded):
//!    a closed thin box (plank, beam, rope segment) collapses flat and then away at almost no
//!    quadric error, so without them bridge decks, walkways and scaffolds shrank to a fragment
//!    at the far levels. Locks pin vertices, not the extent (neighbours may still collapse into
//!    a locked corner until no triangle is left there), so a level whose result falls short of
//!    the box by more than the error limit on any side is redone with a doubled triangle goal
//!    until it holds.
//!    The quadric error does not see area: with an error limit above a wheel's rim radius the
//!    ring folds into the axle and the wagon floats, and with one above the spacing of a gate's
//!    bars they merge into triangles that fill the open lattice. So each result's silhouette is
//!    compared with the source's from five directions ([`coverage`]: the covered share of each
//!    block of a quarter of the error limit, so a lattice's gaps filled in count as coverage
//!    added, while a surface shifted within the block does not), for each connected part that
//!    is a visible share of the model on its own (a wheel is a small share of a wagon's outline
//!    but all of its own) and for the rest together: where a result covers more than `fill_tol`
//!    extra or loses more than `loss_tol` of the source, that group is redone with the error
//!    limit halved, down to `min_error`.
//!    Collapses keep a subset of the original vertices, so UVs, normals, tangents and vertex
//!    colours are the source's values; UV seams and material borders stay where they were.
//! 4. **Write.** One BSTriShape per material with the source's shader and texture set (UVs are
//!    unchanged, so the object LOD atlas and tiling rules treat it like the full model), under a
//!    BSFadeNode root with identity transforms.
//!
//! An animated object can be generated in a chosen state: a [`Pose`] (the end of a controller
//! sequence, [`open_pose`] for a door's `Open`) replaces the transforms of the nodes it controls
//! while collecting, so a gate a reference flags open by default gets an open LOD model.

use std::collections::{BTreeMap, HashMap};

use lodforge_nif::anim::{EndPose, Sequence};
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
    /// Silhouette tolerance ([`coverage`]): the share of the source's silhouette (the model's,
    /// or any connected part's) the result may add (`fill_tol`, bars merged into a wall) or lose
    /// (`loss_tol`, wheels gone) from any of the five directions before the level is redone with
    /// a smaller error limit; 1 or more turns the check off.
    pub fill_tol: f32,
    pub loss_tol: f32,
    /// The error limit is halved down to this when the silhouette check fails.
    pub min_error: f32,
    /// Push the simplified surface back out to the source ([`fit_outward`]): edge collapse keeps
    /// a subset of the source vertices, so convex parts (peaks, ridges, corners) lose volume.
    pub fit_outward: bool,
}

/// Defaults for model levels 0, 1, 2 (object LOD 4, 8, 16). LOD4 starts around 5 cells away
/// (fBlockLevel0Distance), where one pixel at 1080p covers roughly 10..20 units; each further
/// level doubles the distance. The runtime shows LOD4 from the edge of the loaded cells, about
/// half that distance, so its silhouette tolerances are the tightest.
pub const LEVELS: [LevelParams; 3] = [
    LevelParams { max_error: 32.0, max_ratio: 0.3, min_triangles: 12, normal_weight: 0.5, uv_weight: 0.05, permissive: true, fill_tol: 0.2, loss_tol: 0.03, min_error: 4.0, fit_outward: true },
    LevelParams { max_error: 56.0, max_ratio: 0.15, min_triangles: 8, normal_weight: 0.25, uv_weight: 0.02, permissive: true, fill_tol: 0.25, loss_tol: 0.06, min_error: 8.0, fit_outward: true },
    LevelParams { max_error: 140.0, max_ratio: 0.08, min_triangles: 4, normal_weight: 0.1, uv_weight: 0.01, permissive: true, fill_tol: 0.4, loss_tol: 0.10, min_error: 20.0, fit_outward: true },
];

/// Bumped whenever generation changes the output for the same input (cache key). 2: broken
/// source UVs repaired ([`repair_uvs`]). 3: parts keep their extent ([`extent_locks`]). 4: a
/// level that lost an end is redone with a larger goal ([`extent_lost`]). 5: the root node's own
/// transform is ignored, as the engine does ([`collect`]). 6: the silhouette check ([`coverage`])
/// lowers the error limit where the result fills or loses too much. 7: the simplified surface is
/// pushed back out to the source ([`fit_outward`]), and the silhouette loss tolerances are 0.03 /
/// 0.06 / 0.10 instead of 0.2 / 0.25 / 0.4.
pub const GENERATOR_VERSION: u32 = 7;

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
    /// Error limit the level ended with (the level's, or less after silhouette retries).
    pub error_limit: f32,
    /// Silhouette of the result against the source's ([`coverage`]): the largest share added and
    /// the largest share lost over the five directions.
    pub fill: f32,
    pub loss: f32,
}

/// Why a model produced nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Skip {
    Parse(String),
    /// No shape survived collection.
    NoGeometry,
}

/// Node transforms of an animation state, by node (or shape) name: what a controller sequence
/// leaves each controlled node with when it ends. A component the sequence does not animate keeps
/// the node's own value.
#[derive(Debug, Clone, Default)]
pub struct Pose {
    nodes: HashMap<String, EndPose>,
}

impl Pose {
    pub fn from_sequence(s: &Sequence) -> Pose {
        Pose::from_channels(s.channels.iter().cloned())
    }

    pub fn from_channels(channels: impl IntoIterator<Item = EndPose>) -> Pose {
        Pose { nodes: channels.into_iter().map(|c| (c.node.to_ascii_lowercase(), c)).collect() }
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    fn get(&self, name: Option<&str>) -> Option<&EndPose> {
        self.nodes.get(&name?.to_ascii_lowercase())
    }
}

/// The state a door, gate or container is in once its `Open` sequence has played to the end: the
/// pose of a reference flagged open by default. None for a model without an `Open` sequence that
/// moves anything.
pub fn open_pose(nif: &Nif) -> Option<Pose> {
    let seqs = lodforge_nif::anim::sequences(nif);
    let s = seqs.iter().find(|s| s.name.eq_ignore_ascii_case("open"))?;
    let p = Pose::from_sequence(s);
    (!p.is_empty()).then_some(p)
}

/// A node's or shape's local transform in the pose (its own one when the pose does not name it).
fn posed(t: &Transform, name: Option<&str>, pose: Option<&Pose>) -> Transform {
    let Some(e) = pose.and_then(|p| p.get(name)) else { return t.clone() };
    Transform { translation: e.translation.unwrap_or(t.translation), rotation: e.rotation.unwrap_or(t.rotation), scale: e.scale.unwrap_or(t.scale) }
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

/// Shapes under node `idx`, each with the node transforms above it applied. `own` is false for
/// the root: its transform is the reference's in the engine, not the file's. `pose` replaces the
/// transforms of the nodes and shapes it names.
fn collect(nif: &Nif, idx: usize, parent: Xf, own: bool, pose: Option<&Pose>, meshes: &mut Vec<Mesh>, keys: &mut HashMap<String, usize>, st: &mut Stats) {
    let Some(Block::Node(node)) = nif.blocks.get(idx) else { return };
    if node.av.flags & 1 != 0 || is_marker(node.av.net.name.as_deref()) {
        return;
    }
    let xf = if own { Xf::of(&posed(&node.av.transform, node.av.net.name.as_deref(), pose)).then(&parent) } else { parent };
    let switch = nif.block_type(idx) == "NiSwitchNode";
    for c in node.children.iter().flatten().take(if switch { 1 } else { usize::MAX }) {
        let ci = *c as usize;
        match nif.blocks.get(ci) {
            Some(Block::Node(_)) => collect(nif, ci, xf, true, pose, meshes, keys, st),
            Some(Block::TriShape(s)) => {
                st.source_shapes += 1;
                st.source_triangles += s.triangles.len();
                if !add_shape(nif, s, &xf, true, pose, meshes, keys) {
                    st.skipped_shapes += 1;
                }
            }
            _ => {}
        }
    }
}

fn add_shape(nif: &Nif, s: &TriShape, parent: &Xf, own: bool, pose: Option<&Pose>, meshes: &mut Vec<Mesh>, keys: &mut HashMap<String, usize>) -> bool {
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
    let xf = if own { Xf::of(&posed(&s.av.transform, s.av.net.name.as_deref(), pose)).then(parent) } else { *parent };
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

/// Simplifies all meshes together; returns per-mesh index lists (mesh-local) and what the passes
/// reported.
/// Per mesh: the simplified triangles (indices into the mesh's vertices) and the vertex
/// positions to write (the source's, moved where [`fit_outward`] pushed them).
fn simplify(meshes: &[Mesh], p: &LevelParams) -> (Vec<Vec<u32>>, Vec<Vec<[f32; 3]>>, Simplified) {
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
    let extent = extent_locks(&positions, &indices, p.max_error);
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
    let locks: Vec<meshopt::SimplifyVertex> = positions
        .iter()
        .enumerate()
        .map(|(v, pos)| {
            let mut l = if meshes_at[&pos.map(f32::to_bits)] == u32::MAX { meshopt::SimplifyVertex::Protect } else { meshopt::SimplifyVertex::None };
            if extent[v] {
                l |= meshopt::SimplifyVertex::Lock;
            }
            l
        })
        .collect();
    let opts = if p.permissive { meshopt::SimplifyOptions::ErrorAbsolute | meshopt::SimplifyOptions::Permissive } else { meshopt::SimplifyOptions::ErrorAbsolute };
    let dbg = std::env::var_os("LODFORGE_LODMODEL_DEBUG").is_some();
    let per_owner = |idx: &[u32]| -> Vec<usize> {
        let mut c = vec![0usize; meshes.len()];
        for t in idx.chunks_exact(3) {
            c[owner[t[0] as usize] as usize] += 1;
        }
        c
    };
    if dbg {
        eprintln!("  after drop: {:?} tris per mesh, {} locked vertices", per_owner(&indices), extent.iter().filter(|&&b| b).count());
    }
    // Each group on its own: the goal (the triangle count the error pass may reach) doubles
    // while the result falls short of the group's box, since the locks pin the corner vertices
    // but meshoptimizer may still collapse a corner's neighbours into it until no triangle is
    // left there, and a thin end (a rope, a spike, a post) then vanishes for almost no quadric
    // error; the error limit halves while the silhouette check fails ([`coverage`]). The cap
    // never goes below the goal.
    let check = p.fill_tol < 1.0 || p.loss_tol < 1.0;
    let groups = groups(&positions, &indices, p.max_error, check, dbg);
    let mut moved = positions.clone();
    let mut out: Vec<u32> = Vec::new();
    let mut rep = Simplified { error: 0.0, error_limit: p.max_error, fill: 0.0, loss: 0.0 };
    for g in &groups {
        let source = g.indices.len() / 3;
        if source == 0 {
            continue;
        }
        // A part of its own keeps at least a few triangles; the rest keeps the level's floor.
        let (floor, min_goal) = if g.part.is_some() { (4usize, 4usize) } else { (p.min_triangles, p.min_triangles) };
        let cap = ((source as f32 * p.max_ratio).ceil() as usize).max(floor);
        let box_in = extent_of(&positions, &g.indices);
        let frames = check.then(|| Frames::new(&positions, &g.indices, p.max_error / 16.0));
        let source_masks = frames.as_ref().map(|f| f.masks(&positions, &g.indices));
        let mut goal = min_goal.min(source);
        let mut max_error = p.max_error;
        let mut err = 0.0f32;
        let mut cov;
        let mut got;
        let mut last: Option<(usize, (f32, f32))> = None;
        loop {
            got = meshopt::simplify_with_attributes_and_locks_decoder(&g.indices, &positions, &attrs, &weights, 5 * 4, &locks, goal * 3, max_error, opts, Some(&mut err));
            if got.len() / 3 > cap.max(goal) {
                got = meshopt::simplify_with_attributes_and_locks_decoder(&got, &positions, &attrs, &weights, 5 * 4, &locks, cap.max(goal) * 3, f32::MAX, opts, None);
            }
            let box_out = extent_of(&positions, &got);
            let lost = extent_lost(&box_in, &box_out, p.max_error);
            cov = match (&frames, &source_masks) {
                (Some(f), Some(s)) => coverage(s, &f.masks(&positions, &got)),
                _ => (0.0, 0.0),
            };
            let bad_shape = cov.0 > p.fill_tol || cov.1 > p.loss_tol;
            if dbg && (lost || bad_shape) {
                eprintln!("    group {:?} at goal {goal}, error {max_error}: {} triangles, extent {box_in:?} -> {box_out:?} lost {lost}, fill {:.2} loss {:.2}", g.part, got.len() / 3, cov.0, cov.1);
            }
            // A pass that changed nothing (the goal, not the limit, bounds this group): done.
            let same = last == Some((got.len(), cov));
            last = Some((got.len(), cov));
            // A lost end: more triangles (the collapse was almost free, a smaller limit rarely
            // helps). A changed silhouette: a smaller error limit (rims and bars survive it).
            if same {
                break;
            } else if lost && goal < source {
                goal = (goal * 2).min(source);
            } else if bad_shape && max_error > p.min_error {
                max_error = (max_error * 0.5).max(p.min_error);
            } else {
                break;
            }
        }
        if p.fit_outward && got.len() < g.indices.len() {
            let before: Vec<(u32, [f32; 3])> = got.iter().map(|&v| (v, moved[v as usize])).collect();
            // The fit keeps itself only where it brings the surface closer to the source; it
            // must also not merge what the silhouette check keeps apart.
            if fit_outward(&positions, &g.indices, &got, &extent, p.max_error, &mut moved) {
                let fitted = match (&frames, &source_masks) {
                    (Some(f), Some(s)) => coverage(s, &f.masks(&moved, &got)),
                    _ => cov,
                };
                if fitted.0 > p.fill_tol.max(cov.0) {
                    for (v, q) in before {
                        moved[v as usize] = q;
                    }
                } else {
                    cov = fitted;
                }
            }
        }
        if dbg {
            eprintln!("  group {:?}: {source} -> {} triangles, error {err:.1} (limit {max_error}), fill {:.2} loss {:.2}", g.part, got.len() / 3, cov.0, cov.1);
        }
        rep.error = rep.error.max(err);
        rep.error_limit = rep.error_limit.min(max_error);
        rep.fill = rep.fill.max(cov.0);
        rep.loss = rep.loss.max(cov.1);
        out.extend(got);
    }
    let mut per: Vec<Vec<u32>> = vec![Vec::new(); meshes.len()];
    for t in out.chunks_exact(3) {
        let mi = owner[t[0] as usize] as usize;
        if !t.iter().all(|&i| owner[i as usize] as usize == mi) && dbg {
            eprintln!("mixed triangle {:?}", t.iter().map(|&i| owner[i as usize]).collect::<Vec<_>>());
        }
        if t.iter().all(|&i| owner[i as usize] as usize == mi) {
            per[mi].extend(t.iter().map(|&i| i - starts[mi]));
        }
    }
    let per_pos: Vec<Vec<[f32; 3]>> = meshes.iter().enumerate().map(|(mi, m)| moved[starts[mi] as usize..starts[mi] as usize + m.positions.len()].to_vec()).collect();
    (per, per_pos, rep)
}

/// Triangles in a uniform grid for nearest-triangle queries within a cap distance.
struct TriGrid {
    cell: f32,
    cap: f32,
    tris: Vec<[[f32; 3]; 3]>,
    normals: Vec<[f32; 3]>,
    map: HashMap<[i32; 3], Vec<u32>>,
}

impl TriGrid {
    fn new(tris: Vec<[[f32; 3]; 3]>, cap: f32) -> TriGrid {
        let cell = (cap * 2.0).max(1.0);
        let mut map: HashMap<[i32; 3], Vec<u32>> = HashMap::new();
        let normals = tris
            .iter()
            .map(|q| {
                let n = cross3(sub3(q[1], q[0]), sub3(q[2], q[0]));
                let l = dot3(n, n).sqrt();
                if l > 0.0 { n.map(|x| x / l) } else { [0.0; 3] }
            })
            .collect();
        for (ti, q) in tris.iter().enumerate() {
            let lo = [0, 1, 2].map(|k| ((q[0][k].min(q[1][k]).min(q[2][k]) - cap) / cell).floor() as i32);
            let hi = [0, 1, 2].map(|k| ((q[0][k].max(q[1][k]).max(q[2][k]) + cap) / cell).floor() as i32);
            for x in lo[0]..=hi[0] {
                for y in lo[1]..=hi[1] {
                    for z in lo[2]..=hi[2] {
                        map.entry([x, y, z]).or_default().push(ti as u32);
                    }
                }
            }
        }
        TriGrid { cell, cap, tris, normals, map }
    }

    /// The nearest triangle facing like `facing` (unit normal) within the cap: (distance,
    /// triangle, barycentrics, whether `p` lies outside it).
    fn nearest(&self, p: [f32; 3], facing: [f32; 3]) -> Option<(f32, usize, [f32; 3], bool)> {
        let c = p.map(|x| (x / self.cell).floor() as i32);
        let mut best: Option<(f32, usize, [f32; 3], bool)> = None;
        for &ti in self.map.get(&c)? {
            let n = self.normals[ti as usize];
            if dot3(n, facing) < 0.5 {
                continue;
            }
            let q = self.tris[ti as usize];
            let (cp, bary) = closest_on_triangle(p, q[0], q[1], q[2]);
            let d = sub3(p, cp);
            let dist = dot3(d, d).sqrt();
            if dist <= self.cap && best.is_none_or(|b| dist < b.0) {
                best = Some((dist, ti as usize, bary, dot3(d, n) > 0.0));
            }
        }
        best
    }
}

fn sub3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross3(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

/// Points on triangles, about one per `spacing`^2 of area (at least the centroid), each with its
/// triangle's unit normal.
fn surface_samples(tris: &[[[f32; 3]; 3]], spacing: f32) -> Vec<([f32; 3], [f32; 3])> {
    let mut out = Vec::new();
    for q in tris {
        let (a, b, c) = (q[0], q[1], q[2]);
        let n = cross3(sub3(b, a), sub3(c, a));
        let area = 0.5 * dot3(n, n).sqrt();
        if area <= 0.0 {
            continue;
        }
        let n = n.map(|x| x / (2.0 * area));
        let k = ((area / (spacing * spacing)).sqrt().ceil() as usize).clamp(1, 6);
        for i in 0..k {
            for j in 0..k - i {
                let (u, v) = ((i as f32 + 1.0 / 3.0) / k as f32, (j as f32 + 1.0 / 3.0) / k as f32);
                let w = 1.0 - u - v;
                out.push(([a[0] * w + b[0] * u + c[0] * v, a[1] * w + b[1] * u + c[1] * v, a[2] * w + b[2] * u + c[2] * v], n));
            }
        }
    }
    out
}

/// Pushes a simplified surface back out to its source. Edge collapse keeps a subset of the
/// source vertices, so wherever the source is convex (peaks, ridges, corners) the result cuts
/// inside it and the silhouette shrinks. Points sampled on the source triangles (`source`) that
/// lie outside the simplified surface (`lod`, same vertex space) vote, by their distance and the
/// barycentric weight of their nearest point, on the three vertices of the nearest simplified
/// triangle facing the same way (so the far side of a thin part never pushes the near side);
/// each vertex position (coincident vertices together, so seams stay closed) moves along its
/// area-weighted surface normal by the weighted 85th percentile of its votes (points inside vote
/// 0, so one spike cannot inflate a face), at most `max_error`. Two rounds; locked (extent)
/// vertices stay. The result is kept only if the mismatch both ways (source outside the result
/// plus result outside the source, mean distance over surface samples) drops; else `moved` is
/// left as it was. Returns whether it was kept.
/// Quantile of a vertex's votes it moves to: the median balances cutting inside a convex bump
/// against overshooting beside it (tuned on 203 landscape and 40 architecture / clutter models with
/// the `lod_coverage --generated` example: 0.5 / 2 beat 0.7 and 0.85, and weight 1).
const FIT_QUANTILE: f32 = 0.5;
/// Weight of the source sticking out of the result against the result sticking out of the source
/// in [`fit_outward`]'s keep test: a shrunken peak against the sky shows more than a slight bulge.
const FIT_LOSS_WEIGHT: f32 = 2.0;

fn fit_outward(positions: &[[f32; 3]], source: &[u32], lod: &[u32], locked: &[bool], max_error: f32, moved: &mut [[f32; 3]]) -> bool {
    let key = |v: u32| positions[v as usize].map(f32::to_bits);
    let lod_tris: Vec<[u32; 3]> = lod.chunks_exact(3).map(|t| [t[0], t[1], t[2]]).collect();
    if lod_tris.is_empty() {
        return false;
    }
    let spacing = (max_error * 0.5).max(1.0);
    let src_tris: Vec<[[f32; 3]; 3]> = source.chunks_exact(3).map(|t| [0, 1, 2].map(|k| positions[t[k] as usize])).collect();
    let src_samples = surface_samples(&src_tris, spacing);
    let src_grid = TriGrid::new(src_tris, max_error);
    let lod_pos = |m: &[[f32; 3]]| -> Vec<[[f32; 3]; 3]> { lod_tris.iter().map(|t| t.map(|v| m[v as usize])).collect() };
    // Mean distance of the source outside the result plus of the result outside the source.
    let mismatch = |m: &[[f32; 3]]| -> f32 {
        let grid = TriGrid::new(lod_pos(m), max_error);
        let a = src_samples.iter().filter_map(|&(s, n)| grid.nearest(s, n)).map(|h| if h.3 { h.0 } else { 0.0 }).sum::<f32>() / src_samples.len().max(1) as f32;
        let lod_samples = surface_samples(&lod_pos(m), spacing);
        let b = lod_samples.iter().filter_map(|&(s, n)| src_grid.nearest(s, n)).map(|h| if h.3 { h.0 } else { 0.0 }).sum::<f32>() / lod_samples.len().max(1) as f32;
        FIT_LOSS_WEIGHT * a + b
    };
    let before_err = mismatch(moved);
    let before: Vec<(u32, [f32; 3])> = lod.iter().map(|&v| (v, moved[v as usize])).collect();
    for _round in 0..2 {
        let grid = TriGrid::new(lod_pos(moved), max_error);
        // Votes per position: (distance outside, weight); inside samples vote 0.
        let mut votes: HashMap<[u32; 3], Vec<(f32, f32)>> = HashMap::new();
        for &(s, n) in &src_samples {
            let Some((d, ti, bary, outside)) = grid.nearest(s, n) else { continue };
            for (k, &v) in lod_tris[ti].iter().enumerate() {
                if bary[k] > 0.0 {
                    votes.entry(key(v)).or_default().push((if outside { d } else { 0.0 }, bary[k]));
                }
            }
        }
        // Area-weighted normals per position, from the simplified surface.
        // Per position: summed face normals, summed face normal lengths (how much the faces
        // agree on a direction) and the summed / counted edge lengths around it.
        let mut normals: HashMap<[u32; 3], ([f32; 3], f32, f32, u32)> = HashMap::new();
        for t in &lod_tris {
            let q = t.map(|v| moved[v as usize]);
            let n = cross3(sub3(q[1], q[0]), sub3(q[2], q[0]));
            let nl = dot3(n, n).sqrt();
            for (i, &v) in t.iter().enumerate() {
                let e = normals.entry(key(v)).or_insert(([0.0; 3], 0.0, 0.0, 0));
                for k in 0..3 {
                    e.0[k] += n[k];
                }
                e.1 += nl;
                for j in [(i + 1) % 3, (i + 2) % 3] {
                    let d = sub3(q[j], q[i]);
                    e.2 += dot3(d, d).sqrt();
                    e.3 += 1;
                }
            }
        }
        let mut shift: HashMap<[u32; 3], [f32; 3]> = HashMap::new();
        for (k, mut list) in votes {
            list.sort_by(|a, b| a.0.total_cmp(&b.0));
            let total: f32 = list.iter().map(|x| x.1).sum();
            let (mut acc, mut d) = (0.0, 0.0);
            for (dist, w) in &list {
                acc += w;
                if acc >= FIT_QUANTILE * total {
                    d = *dist;
                    break;
                }
            }
            let Some(&(n, total, edges, count)) = normals.get(&k) else { continue };
            let nl = dot3(n, n).sqrt();
            // Faces that disagree on a direction (a lid meeting the staves, a fold, a seam
            // between parts) give no direction to move along; a vertex never moves more than
            // half its mean edge length, so it cannot become a spike.
            if d > 0.0 && nl > 0.5 * total && count > 0 {
                let d = d.min(max_error).min(0.5 * edges / count as f32);
                shift.insert(k, n.map(|x| x / nl * d));
            }
        }
        if shift.is_empty() {
            break;
        }
        let mut done = std::collections::HashSet::new();
        for &v in lod {
            if locked[v as usize] || !done.insert(v) {
                continue;
            }
            if let Some(s) = shift.get(&key(v)) {
                let p = &mut moved[v as usize];
                for k in 0..3 {
                    p[k] += s[k];
                }
            }
        }
    }
    if mismatch(moved) < before_err * 0.98 {
        true
    } else {
        for (v, q) in before {
            moved[v as usize] = q;
        }
        false
    }
}

/// The point of triangle `abc` closest to `p`, and its barycentric weights (Ericson, Real-Time
/// Collision Detection 5.1.5).
fn closest_on_triangle(p: [f32; 3], a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> ([f32; 3], [f32; 3]) {
    let sub = |x: [f32; 3], y: [f32; 3]| [x[0] - y[0], x[1] - y[1], x[2] - y[2]];
    let at = |u: f32, v: f32, w: f32| [a[0] * u + b[0] * v + c[0] * w, a[1] * u + b[1] * v + c[1] * w, a[2] * u + b[2] * v + c[2] * w];
    let (ab, ac, ap) = (sub(b, a), sub(c, a), sub(p, a));
    let (d1, d2) = (dot3(ab, ap), dot3(ac, ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return (a, [1.0, 0.0, 0.0]);
    }
    let bp = sub(p, b);
    let (d3, d4) = (dot3(ab, bp), dot3(ac, bp));
    if d3 >= 0.0 && d4 <= d3 {
        return (b, [0.0, 1.0, 0.0]);
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return (at(1.0 - v, v, 0.0), [1.0 - v, v, 0.0]);
    }
    let cp = sub(p, c);
    let (d5, d6) = (dot3(ab, cp), dot3(ac, cp));
    if d6 >= 0.0 && d5 <= d6 {
        return (c, [0.0, 0.0, 1.0]);
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return (at(1.0 - w, 0.0, w), [1.0 - w, 0.0, w]);
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return (at(0.0, 1.0 - w, w), [0.0, 1.0 - w, w]);
    }
    let denom = 1.0 / (va + vb + vc);
    let (v, w) = (vb * denom, vc * denom);
    (at(1.0 - v - w, v, w), [1.0 - v - w, v, w])
}

/// What [`simplify`] reports besides the triangles.
struct Simplified {
    error: f32,
    error_limit: f32,
    fill: f32,
    loss: f32,
}

/// A simplification group: one connected part that is a visible share of the model, or the rest
/// of the model together.
struct Group {
    part: Option<u32>,
    indices: Vec<u32>,
}

/// Splits the triangles into groups: each connected part (triangles joined through shared or
/// coincident positions) whose own silhouette is at least [`PART_SHARE`] of the model's from
/// some direction gets a group of its own (a wheel, a gate leaf, a wall board: a small share of
/// the model's outline but all of its own, and the one whose error limit the silhouette check
/// should lower), and the rest (nails, rings, hinges, which vanish at any limit) forms one group,
/// first. Without the check, or with one part, everything is one group. Collapses never join
/// parts (an edge joins vertices of one part), so the groups simplify independently.
fn groups(positions: &[[f32; 3]], indices: &[u32], max_error: f32, check: bool, dbg: bool) -> Vec<Group> {
    let all = || vec![Group { part: None, indices: indices.to_vec() }];
    if !check {
        return all();
    }
    let part_of = connected_parts(positions, indices);
    let mut per_part: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for t in indices.chunks_exact(3) {
        per_part.entry(part_of[t[0] as usize]).or_default().extend_from_slice(t);
    }
    if per_part.len() <= 1 {
        return all();
    }
    let texel = max_error / 16.0;
    let whole = Frames::new(positions, indices, texel);
    let count = |m: &Vec<u16>| m.iter().map(|&v| usize::from(v)).sum::<usize>();
    let whole_counts: Vec<usize> = whole.masks(positions, indices).iter().map(count).collect();
    let mut rest = Vec::new();
    let mut out = Vec::new();
    for (part, idx) in per_part {
        let (lo, hi) = extent_of(positions, &idx);
        let diag = ((hi[0] - lo[0]).powi(2) + (hi[1] - lo[1]).powi(2) + (hi[2] - lo[2]).powi(2)).sqrt();
        // Blocks are the same size in every frame, so the counts compare across them.
        let significant = diag >= 4.0 * texel * BLOCK as f32 && {
            let f = Frames::new(positions, &idx, texel);
            f.masks(positions, &idx).iter().zip(&whole_counts).any(|(pm, &wc)| count(pm) as f32 >= PART_SHARE * wc as f32)
        };
        if dbg && significant {
            eprintln!("    part {part}: {} triangles, extent {:.0}..{:.0} {:.0}..{:.0} {:.0}..{:.0}", idx.len() / 3, lo[0], hi[0], lo[1], hi[1], lo[2], hi[2]);
        }
        if significant {
            out.push(Group { part: Some(part), indices: idx });
        } else {
            rest.extend(idx);
        }
    }
    out.insert(0, Group { part: None, indices: rest });
    out
}

/// Connected parts (triangles joined through shared or coincident positions, 0.01-unit grid):
/// the part id of each vertex, u32::MAX for one no triangle uses.
fn connected_parts(positions: &[[f32; 3]], indices: &[u32]) -> Vec<u32> {
    let key = |p: &[f32; 3]| p.map(|v| (f64::from(v) * 100.0).round() as i64);
    let mut weld: HashMap<[i64; 3], u32> = HashMap::new();
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
    let mut used = vec![false; positions.len()];
    for &v in indices {
        used[v as usize] = true;
    }
    (0..positions.len()).map(|v| if used[v] { find(&mut parent, id[v]) } else { u32::MAX }).collect()
}

/// Orthographic silhouette frames of a mesh from five directions (along Y, X, Z and the two
/// horizontal diagonals), `texel` units per texel, reduced to the covered texel count of each
/// `BLOCK` x `BLOCK` texel block, framed on the reference mesh with a block of margin. The same
/// frames rasterize the reference and each result, so [`coverage`] compares like with like; a
/// result vertex outside the frame is clipped (its extent is checked separately).
struct Frames {
    views: Vec<Frame>,
}

struct Frame {
    right: [f32; 3],
    up: [f32; 3],
    x0: f32,
    z0: f32,
    texel: f32,
    /// Texels.
    w: usize,
    h: usize,
    /// Blocks.
    bw: usize,
    bh: usize,
}

const BLOCK: usize = 4;
const MAX_TEXELS: f32 = 1024.0;
/// Smallest share of the model's silhouette blocks (in the same view) a part needs for a check
/// of its own.
const PART_SHARE: f32 = 0.03;

impl Frames {
    fn new(positions: &[[f32; 3]], indices: &[u32], texel: f32) -> Frames {
        let s = std::f32::consts::FRAC_1_SQRT_2;
        let dirs: [([f32; 3], [f32; 3]); 5] = [
            ([1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
            ([0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
            ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
            ([s, -s, 0.0], [0.0, 0.0, 1.0]),
            ([s, s, 0.0], [0.0, 0.0, 1.0]),
        ];
        let used: Vec<[f32; 3]> = indices.iter().map(|&i| positions[i as usize]).collect();
        let views = dirs
            .iter()
            .map(|&(right, up)| {
                let (mut x0, mut x1, mut z0, mut z1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
                for p in &used {
                    let x = dot3(*p, right);
                    let z = dot3(*p, up);
                    x0 = x0.min(x);
                    x1 = x1.max(x);
                    z0 = z0.min(z);
                    z1 = z1.max(z);
                }
                if used.is_empty() {
                    (x0, x1, z0, z1) = (0.0, 1.0, 0.0, 1.0);
                }
                let margin = texel * BLOCK as f32;
                let (x0, x1, z0, z1) = (x0 - margin, x1 + margin, z0 - margin, z1 + margin);
                let texel = texel.max((x1 - x0).max(z1 - z0) / MAX_TEXELS).max(1e-3);
                let w = ((x1 - x0) / texel).ceil().max(1.0) as usize;
                let h = ((z1 - z0) / texel).ceil().max(1.0) as usize;
                Frame { right, up, x0, z0, texel, w, h, bw: w.div_ceil(BLOCK), bh: h.div_ceil(BLOCK) }
            })
            .collect();
        Frames { views }
    }

    /// One block mask per view: covered texels per block.
    fn masks(&self, positions: &[[f32; 3]], indices: &[u32]) -> Vec<Vec<u16>> {
        self.views.iter().map(|f| f.mask(positions, indices)).collect()
    }
}

fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

impl Frame {
    /// Rasterizes the triangles into texels (the interior at texel centres, either winding, and
    /// the three edges as lines, so a sliver or an edge-on surface covers the same texels whether
    /// it is one triangle or a thousand) and returns the covered texels per block.
    fn mask(&self, positions: &[[f32; 3]], indices: &[u32]) -> Vec<u16> {
        let mut bits = vec![false; self.w * self.h];
        let proj = |p: [f32; 3]| [(dot3(p, self.right) - self.x0) / self.texel, (dot3(p, self.up) - self.z0) / self.texel];
        let mark = |x: f32, y: f32, bits: &mut Vec<bool>| {
            if x >= 0.0 && y >= 0.0 && (x as usize) < self.w && (y as usize) < self.h {
                bits[y as usize * self.w + x as usize] = true;
            }
        };
        for t in indices.chunks_exact(3) {
            let a = proj(positions[t[0] as usize]);
            let b = proj(positions[t[1] as usize]);
            let c = proj(positions[t[2] as usize]);
            for (p, q) in [(a, b), (b, c), (c, a)] {
                let steps = ((q[0] - p[0]).abs().max((q[1] - p[1]).abs()).ceil() as usize).clamp(1, 4096);
                for s in 0..=steps {
                    let f = s as f32 / steps as f32;
                    mark(p[0] + (q[0] - p[0]) * f, p[1] + (q[1] - p[1]) * f, &mut bits);
                }
            }
            let area = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
            let (xmin, xmax) = (a[0].min(b[0]).min(c[0]), a[0].max(b[0]).max(c[0]));
            let (ymin, ymax) = (a[1].min(b[1]).min(c[1]), a[1].max(b[1]).max(c[1]));
            let i0 = (xmin.floor().max(0.0)) as usize;
            let i1 = (xmax.ceil().min(self.w as f32)) as usize;
            let j0 = (ymin.floor().max(0.0)) as usize;
            let j1 = (ymax.ceil().min(self.h as f32)) as usize;
            if i0 >= i1 || j0 >= j1 || area.abs() < 1e-12 {
                continue;
            }
            let sign = if area < 0.0 { -1.0 } else { 1.0 };
            for j in j0..j1 {
                let py = j as f32 + 0.5;
                for i in i0..i1 {
                    let px = i as f32 + 0.5;
                    let w0 = ((b[0] - a[0]) * (py - a[1]) - (b[1] - a[1]) * (px - a[0])) * sign;
                    let w1 = ((c[0] - b[0]) * (py - b[1]) - (c[1] - b[1]) * (px - b[0])) * sign;
                    let w2 = ((a[0] - c[0]) * (py - c[1]) - (a[1] - c[1]) * (px - c[0])) * sign;
                    if w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0 {
                        bits[j * self.w + i] = true;
                    }
                }
            }
        }
        let mut blocks = vec![0u16; self.bw * self.bh];
        for j in 0..self.h {
            for i in 0..self.w {
                if bits[j * self.w + i] {
                    blocks[(j / BLOCK) * self.bw + i / BLOCK] += 1;
                }
            }
        }
        blocks
    }
}

/// Silhouette of a result against the source's, both as covered texels per block and view: the
/// largest share of the source's coverage the result adds (fill: per block, what it covers beyond
/// the source) and the largest share it loses (loss) over the views (0 for an empty source).
fn coverage(source: &[Vec<u16>], result: &[Vec<u16>]) -> (f32, f32) {
    let mut fill = 0.0f32;
    let mut loss = 0.0f32;
    for (s, r) in source.iter().zip(result) {
        let n: u32 = s.iter().map(|&v| u32::from(v)).sum();
        if n == 0 {
            continue;
        }
        let added: u32 = s.iter().zip(r).map(|(&a, &b)| u32::from(b.saturating_sub(a))).sum();
        let gone: u32 = s.iter().zip(r).map(|(&a, &b)| u32::from(a.saturating_sub(b))).sum();
        fill = fill.max(added as f32 / n as f32);
        loss = loss.max(gone as f32 / n as f32);
    }
    (fill, loss)
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
/// The largest part always stays, and so does what many small parts add up to: they go smallest
/// first only while the area gone stays under a fifth of the model's (the planks of a deck or the
/// posts of a fence are each small and together the whole object; dropping them all left one).
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
    let mut area: HashMap<u32, f64> = HashMap::new();
    let mut total = 0.0f64;
    for t in indices.chunks_exact(3) {
        let [a, b, c] = [positions[t[0] as usize], positions[t[1] as usize], positions[t[2] as usize]];
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let cr = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
        let s = 0.5 * f64::from((cr[0] * cr[0] + cr[1] * cr[1] + cr[2] * cr[2]).sqrt());
        *area.entry(find(&mut parent, id[t[0] as usize])).or_default() += s;
        total += s;
    }
    let diag = |b: &([f32; 3], [f32; 3])| ((b.1[0] - b.0[0]).powi(2) + (b.1[1] - b.0[1]).powi(2) + (b.1[2] - b.0[2]).powi(2)).sqrt();
    let largest = boxes.iter().max_by(|a, b| diag(a.1).total_cmp(&diag(b.1))).map(|(r, _)| *r);
    let mut small: Vec<(f32, u32)> = boxes.iter().filter(|(r, b)| Some(**r) != largest && diag(b) < min_size).map(|(r, b)| (diag(b), *r)).collect();
    small.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    // The model's own box must survive: a part at an end of a deck or a fence goes only when
    // what stays still reaches within half `min_size` of the model's extent on every axis.
    let model = boxes.values().fold(([f32::MAX; 3], [f32::MIN; 3]), |m, b| ([m.0[0].min(b.0[0]), m.0[1].min(b.0[1]), m.0[2].min(b.0[2])], [m.1[0].max(b.1[0]), m.1[1].max(b.1[1]), m.1[2].max(b.1[2])]));
    let mut dropped: std::collections::HashSet<u32> = std::collections::HashSet::new();
    let mut gone = 0.0f64;
    for (_, r) in small {
        let a = area.get(&r).copied().unwrap_or(0.0);
        if gone + a > 0.2 * total {
            break;
        }
        let rest = boxes.iter().filter(|(k, _)| **k != r && !dropped.contains(k)).fold(([f32::MAX; 3], [f32::MIN; 3]), |m, (_, b)| ([m.0[0].min(b.0[0]), m.0[1].min(b.0[1]), m.0[2].min(b.0[2])], [m.1[0].max(b.1[0]), m.1[1].max(b.1[1]), m.1[2].max(b.1[2])]));
        if (0..3).any(|k| rest.0[k] - model.0[k] > 0.5 * min_size || model.1[k] - rest.1[k] > 0.5 * min_size) {
            continue;
        }
        gone += a;
        dropped.insert(r);
    }
    indices.chunks_exact(3).filter(|t| !dropped.contains(&find(&mut parent, id[t[0] as usize]))).flatten().copied().collect()
}

/// Vertices that pin each connected part's extent: for every part (triangles joined through
/// shared or coincident positions) the vertex nearest to each corner of its bounding box, with
/// an axis thinner than `thin` and a tenth of the part's diagonal folded to its middle (a plank
/// keeps its four footprint corners, a rope its two ends, a block all eight). Every vertex at a chosen position is marked, so a seam
/// cannot free a copy. Locked, the corners stop a thin closed part from collapsing flat and then
/// away: a plank's top face onto its bottom face costs its thickness in quadric error, the
/// degenerate box then has no border edges, and the rest goes for nothing.
fn extent_locks(positions: &[[f32; 3]], indices: &[u32], thin: f32) -> Vec<bool> {
    let key = |p: &[f32; 3]| p.map(|v| (f64::from(v) * 100.0).round() as i64);
    let mut weld: HashMap<[i64; 3], u32> = HashMap::new();
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
    let mut used = vec![false; positions.len()];
    let mut boxes: HashMap<u32, ([f32; 3], [f32; 3])> = HashMap::new();
    for &v in indices {
        used[v as usize] = true;
        let r = find(&mut parent, id[v as usize]);
        let p = positions[v as usize];
        let e = boxes.entry(r).or_insert((p, p));
        for k in 0..3 {
            e.0[k] = e.0[k].min(p[k]);
            e.1[k] = e.1[k].max(p[k]);
        }
    }
    // Per part and corner: the nearest welded position.
    let mut best: HashMap<u32, [(f32, u32); 8]> = HashMap::new();
    for (v, p) in positions.iter().enumerate() {
        if !used[v] {
            continue;
        }
        let r = find(&mut parent, id[v as usize]);
        let b = boxes[&r];
        // An axis folds (its two corners become one) when the part is thin along it: a tenth of
        // the part's diagonal at most, so a plank folds its thickness but keeps its width.
        let fold = (0.1 * ((b.1[0] - b.0[0]).powi(2) + (b.1[1] - b.0[1]).powi(2) + (b.1[2] - b.0[2]).powi(2)).sqrt()).min(thin);
        let e = best.entry(r).or_insert([(f32::MAX, u32::MAX); 8]);
        for (c, slot) in e.iter_mut().enumerate() {
            let mut d = 0.0f32;
            for k in 0..3 {
                let target = if b.1[k] - b.0[k] < fold { 0.5 * (b.0[k] + b.1[k]) } else if c & (1 << k) != 0 { b.1[k] } else { b.0[k] };
                d += (p[k] - target) * (p[k] - target);
            }
            if d < slot.0 {
                *slot = (d, id[v]);
            }
        }
    }
    let chosen: std::collections::HashSet<u32> = best.values().flatten().map(|&(_, w)| w).filter(|&w| w != u32::MAX).collect();
    id.iter().enumerate().map(|(v, w)| used[v] && chosen.contains(w)).collect()
}

/// Bounding box of the vertices the triangles reference: (min, max).
fn extent_of(positions: &[[f32; 3]], indices: &[u32]) -> ([f32; 3], [f32; 3]) {
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for &i in indices {
        let p = positions[i as usize];
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    (lo, hi)
}

/// Whether `out` falls short of `full` by more than `tol` on any side of any axis.
fn extent_lost(full: &([f32; 3], [f32; 3]), out: &([f32; 3], [f32; 3]), tol: f32) -> bool {
    (0..3).any(|k| out.0[k] > full.0[k] + tol || out.1[k] < full.1[k] - tol)
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
    let (nif, st) = generate_impl(&nif, p, name, true, None)?;
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
    generate_impl(nif, p, name, false, None)
}

/// [`generate_nif`] with the model in `pose` (its nodes' own transforms where the pose is None).
pub fn generate_nif_posed(nif: &Nif, p: &LevelParams, name: &str, pose: Option<&Pose>) -> Result<(Nif, Stats), Skip> {
    generate_impl(nif, p, name, false, pose)
}

fn generate_impl(nif: &Nif, p: &LevelParams, name: &str, keep_names: bool, pose: Option<&Pose>) -> Result<(Nif, Stats), Skip> {
    let mut st = Stats::default();
    let mut meshes = Vec::new();
    let mut keys = HashMap::new();
    let root = nif.roots.first().copied().flatten().unwrap_or(0) as usize;
    match nif.blocks.get(root) {
        Some(Block::Node(_)) => collect(nif, root, Xf::IDENTITY, false, pose, &mut meshes, &mut keys, &mut st),
        Some(Block::TriShape(s)) => {
            st.source_shapes += 1;
            st.source_triangles += s.triangles.len();
            if !add_shape(nif, s, &Xf::IDENTITY, false, pose, &mut meshes, &mut keys) {
                st.skipped_shapes += 1;
            }
        }
        _ => {}
    }
    meshes.retain(|m| !m.triangles.is_empty());
    if meshes.is_empty() {
        return Err(Skip::NoGeometry);
    }
    let (per, moved, sim) = simplify(&meshes, p);
    if std::env::var_os("LODFORGE_LODMODEL_DEBUG").is_some() {
        eprintln!("meshes {} tris {:?} -> {:?} err {} (limit {}) fill {:.2} loss {:.2}", meshes.len(), meshes.iter().map(|m| m.triangles.len()).collect::<Vec<_>>(), per.iter().map(|v| v.len() / 3).collect::<Vec<_>>(), sim.error, sim.error_limit, sim.fill, sim.loss);
    }
    st.error = sim.error;
    st.error_limit = sim.error_limit;
    st.fill = sim.fill;
    st.loss = sim.loss;

    let mut out = Nif::new();
    out.header.block_types = vec!["BSFadeNode".into()];
    out.header.block_type_index = vec![0];
    out.add_block(Block::Node(Node { av: AvObject { net: ObjectNet::named(name), ..Default::default() }, ..Default::default() }));
    let mut children = Vec::new();
    for ((m, idx), moved) in meshes.iter().zip(per).zip(&moved) {
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
        let positions = pick3(moved);
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

    /// Extent of every shape's vertices of a generated model: (min, max).
    fn extent(nif: &Nif) -> ([f32; 3], [f32; 3]) {
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for b in &nif.blocks {
            if let Block::TriShape(s) = b {
                for p in &s.positions {
                    for k in 0..3 {
                        lo[k] = lo[k].min(p[k]);
                        hi[k] = hi[k].max(p[k]);
                    }
                }
            }
        }
        (lo, hi)
    }

    /// A bumpy sphere (a boulder): the simplified surface cuts inside the bumps; the outward fit
    /// pushes it back, so the share of the source outside the result drops.
    #[test]
    fn outward_fit_keeps_volume() {
        let mut s = TriShape::default();
        let (segs, rings, r) = (48u16, 24u16, 300.0f32);
        for j in 0..=rings {
            let t = f32::from(j) / f32::from(rings) * std::f32::consts::PI;
            for i in 0..=segs {
                let a = f32::from(i) / f32::from(segs) * std::f32::consts::TAU;
                // Ridges: radius varies with both angles.
                let rr = r + 40.0 * ((a * 7.0).sin() * (t * 5.0).sin()).abs();
                let n = [t.sin() * a.cos(), t.sin() * a.sin(), t.cos()];
                s.positions.push(n.map(|x| x * rr));
                s.normals.push(n);
                s.uvs.push([f32::from(i) / f32::from(segs), f32::from(j) / f32::from(rings)]);
            }
        }
        let w = segs + 1;
        for j in 0..rings {
            for i in 0..segs {
                let p = j * w + i;
                s.triangles.push([p, p + w, p + 1]);
                s.triangles.push([p + 1, p + w, p + w + 1]);
            }
        }
        let mut nif = Nif::new();
        nif.add_block(Block::Node(Node::default()));
        let ts = nif.add_block(Block::TextureSet(vec!["textures\\a.dds".into()]));
        let sh = nif.add_block(Block::LightingShader(LightingShader { texture_set: Some(ts), ..Default::default() }));
        s.shader_property = Some(sh);
        s.vertex_desc = VertexDesc::with_attributes(true, true, true, false, false).0;
        let src = s.clone();
        let si = nif.add_block(Block::TriShape(s));
        if let Block::Node(r) = &mut nif.blocks[0] {
            r.children = vec![Some(si)];
        }
        // Mean distance of source vertices outside the result (nearest triangle, signed).
        let outside = |out: &Nif| -> f32 {
            let t = out.blocks.iter().find_map(|b| if let Block::TriShape(t) = b { Some(t.clone()) } else { None }).unwrap();
            let mut sum = 0.0;
            for p in &src.positions {
                let mut best = (f32::MAX, 0.0f32);
                for tri in &t.triangles {
                    let q = tri.map(|i| t.positions[i as usize]);
                    let (cp, _) = closest_on_triangle(*p, q[0], q[1], q[2]);
                    let d = [p[0] - cp[0], p[1] - cp[1], p[2] - cp[2]];
                    let dist = dot3(d, d).sqrt();
                    if dist < best.0 {
                        let e1 = [q[1][0] - q[0][0], q[1][1] - q[0][1], q[1][2] - q[0][2]];
                        let e2 = [q[2][0] - q[0][0], q[2][1] - q[0][1], q[2][2] - q[0][2]];
                        let n = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
                        best = (dist, if dot3(d, n) > 0.0 { dist } else { 0.0 });
                    }
                }
                sum += best.1;
            }
            sum / src.positions.len() as f32
        };
        let no_fit = LevelParams { fit_outward: false, ..LEVELS[0] };
        let (a, _) = generate_nif(&nif, &no_fit, "t").unwrap();
        let (b, st) = generate_nif(&nif, &LEVELS[0], "t").unwrap();
        let (oa, ob) = (outside(&a), outside(&b));
        assert!(ob < 0.6 * oa, "outside without fit {oa:.2}, with fit {ob:.2} ({st:?})");
    }

    /// A deck of thin closed planks (a stockade bridge, a walkway) keeps its length at every
    /// level instead of collapsing from the ends into a fragment.
    #[test]
    fn plank_deck_keeps_its_extent() {
        let mut s = TriShape::default();
        let plank = |s: &mut TriShape, x0: f32, x1: f32, y0: f32, y1: f32, z0: f32, z1: f32| {
            let base = s.positions.len() as u16;
            for z in [z0, z1] {
                for (x, y) in [(x0, y0), (x1, y0), (x1, y1), (x0, y1)] {
                    s.positions.push([x, y, z]);
                    s.uvs.push([(x - x0) / (x1 - x0), (y - y0) / (y1 - y0)]);
                    s.normals.push([0.0, 0.0, if z == z1 { 1.0 } else { -1.0 }]);
                }
            }
            // top, bottom, four sides
            let quads: [[u16; 4]; 6] = [[4, 5, 6, 7], [0, 3, 2, 1], [0, 1, 5, 4], [1, 2, 6, 5], [2, 3, 7, 6], [3, 0, 4, 7]];
            for q in quads {
                s.triangles.push([base + q[0], base + q[1], base + q[2]]);
                s.triangles.push([base + q[0], base + q[2], base + q[3]]);
            }
        };
        // 20 planks across a 1200-unit span, 56 wide, 4 thick: the stockade bridge deck's shape.
        for i in 0..20 {
            let x0 = -600.0 + 60.0 * i as f32;
            plank(&mut s, x0, x0 + 56.0, -120.0, 120.0, -2.0, 2.0);
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
        for (k, p) in LEVELS.iter().enumerate() {
            let (out, st) = generate_nif(&nif, p, "t").unwrap();
            let (lo, hi) = extent(&out);
            // The deck spans 1196 x 240; a level may lose at most its error limit at each end.
            let e = 2.0 * p.max_error;
            assert!(hi[0] - lo[0] >= 1196.0 - e && hi[1] - lo[1] >= 240.0 - e, "level {k}: extent {lo:?}..{hi:?} {st:?}");
            assert!(st.triangles <= 240, "level {k}: {st:?}");
        }
    }

    /// A model whose root is translated and whose shape translates back (a Blender export with
    /// its origin moved: Northern Roads' Whiterun bridge) keeps the shape's own translation only,
    /// as the engine does; the root's is the reference's placement there.
    #[test]
    fn root_transform_is_ignored() {
        let mut s = TriShape::default();
        for z in [-2.0f32, 2.0] {
            for (x, y) in [(-50.0f32, -20.0f32), (50.0, -20.0), (50.0, 20.0), (-50.0, 20.0)] {
                s.positions.push([x, y, z]);
                s.uvs.push([(x + 50.0) / 100.0, (y + 20.0) / 40.0]);
                s.normals.push([0.0, 0.0, if z > 0.0 { 1.0 } else { -1.0 }]);
            }
        }
        let quads: [[u16; 4]; 6] = [[4, 5, 6, 7], [0, 3, 2, 1], [0, 1, 5, 4], [1, 2, 6, 5], [2, 3, 7, 6], [3, 0, 4, 7]];
        for q in quads {
            s.triangles.push([q[0], q[1], q[2]]);
            s.triangles.push([q[0], q[2], q[3]]);
        }
        s.av.transform.translation = [0.0, 759.6, 0.0];
        let mut root = Node::default();
        root.av.transform.translation = [0.0, -759.6, 0.0];
        let mut nif = Nif::new();
        nif.add_block(Block::Node(root));
        let ts = nif.add_block(Block::TextureSet(vec!["textures\\a.dds".into()]));
        let sh = nif.add_block(Block::LightingShader(LightingShader { texture_set: Some(ts), ..Default::default() }));
        s.shader_property = Some(sh);
        s.vertex_desc = VertexDesc::with_attributes(true, true, true, false, false).0;
        let si = nif.add_block(Block::TriShape(s));
        if let Block::Node(r) = &mut nif.blocks[0] {
            r.children = vec![Some(si)];
        }
        let (out, st) = generate_nif(&nif, &LEVELS[0], "t").unwrap();
        let (lo, hi) = extent(&out);
        assert!(lo[1] > 700.0 && hi[1] < 820.0 && lo[0] < -40.0 && hi[0] > 40.0, "extent {lo:?}..{hi:?} {st:?}");
    }

    /// A wheel (a ring of thin segments on an axle box) keeps its ring at level 0 and 1 instead of
    /// folding into the axle: the loss check lowers the error limit below the rim radius.
    #[test]
    fn wheel_keeps_its_ring() {
        let mut s = TriShape::default();
        // Rim: 24 segments of a torus-like ring, radius 60, tube 4, in the XZ plane at y = 0.
        let (segs, sides, r, tube) = (24u16, 6u16, 60.0f32, 4.0f32);
        for i in 0..segs {
            let a = f32::from(i) / f32::from(segs) * std::f32::consts::TAU;
            for j in 0..sides {
                let b = f32::from(j) / f32::from(sides) * std::f32::consts::TAU;
                let rr = r + tube * b.cos();
                s.positions.push([rr * a.cos(), tube * b.sin(), rr * a.sin()]);
                s.uvs.push([f32::from(i) / f32::from(segs), f32::from(j) / f32::from(sides)]);
                s.normals.push(normalize([b.cos() * a.cos(), b.sin(), b.cos() * a.sin()]));
            }
        }
        for i in 0..segs {
            let n = (i + 1) % segs;
            for j in 0..sides {
                let m = (j + 1) % sides;
                let (p0, p1, p2, p3) = (i * sides + j, n * sides + j, n * sides + m, i * sides + m);
                s.triangles.push([p0, p1, p2]);
                s.triangles.push([p0, p2, p3]);
            }
        }
        // Axle box joined to the rim's inner side (a long bar the ring sits on).
        let base = s.positions.len() as u16;
        for z in [-6.0f32, 6.0] {
            for (x, y) in [(-70.0f32, -3.0f32), (70.0, -3.0), (70.0, 3.0), (-70.0, 3.0)] {
                s.positions.push([x, y, z]);
                s.uvs.push([(x + 70.0) / 140.0, (y + 3.0) / 6.0]);
                s.normals.push([0.0, 0.0, if z > 0.0 { 1.0 } else { -1.0 }]);
            }
        }
        let quads: [[u16; 4]; 6] = [[4, 5, 6, 7], [0, 3, 2, 1], [0, 1, 5, 4], [1, 2, 6, 5], [2, 3, 7, 6], [3, 0, 4, 7]];
        for q in quads {
            s.triangles.push([base + q[0], base + q[1], base + q[2]]);
            s.triangles.push([base + q[0], base + q[2], base + q[3]]);
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
        for (k, p) in LEVELS.iter().take(2).enumerate() {
            let (out, st) = generate_nif(&nif, p, "t").unwrap();
            let (lo, hi) = extent(&out);
            // The ring's top and bottom (z = +-64) survive and the silhouette stays within tolerance.
            assert!(hi[2] >= 56.0 && lo[2] <= -56.0, "level {k}: extent {lo:?}..{hi:?} {st:?}");
            // The tube (radius 4) flattens at the smallest limit, which halves it from one side.
            assert!((st.loss <= p.loss_tol && st.fill <= p.fill_tol) || st.error_limit <= p.min_error, "level {k}: {st:?}");
            assert!(st.error_limit < p.max_error, "level {k}: the check had to lower the limit, {st:?}");
            let no_check = LevelParams { fill_tol: 1.0, loss_tol: 1.0, ..*p };
            let (_, st0) = generate_nif(&nif, &no_check, "t").unwrap();
            assert!(st0.triangles <= st.triangles, "level {k}: unchecked {st0:?} vs checked {st:?}");
        }
    }

    /// A gate leaf under a hinge node, generated in the pose at the end of an `Open` sequence that
    /// turns the hinge 90 degrees about Z, swings from +X to +Y; without the pose it stays at +X.
    #[test]
    fn posed_hinge_swings_the_leaf() {
        let mut s = TriShape::default();
        for z in [0.0f32, 100.0] {
            for (x, y) in [(0.0f32, -2.0f32), (120.0, -2.0), (120.0, 2.0), (0.0, 2.0)] {
                s.positions.push([x, y, z]);
                s.uvs.push([x / 120.0, z / 100.0]);
                s.normals.push([0.0, if y > 0.0 { 1.0 } else { -1.0 }, 0.0]);
            }
        }
        let quads: [[u16; 4]; 6] = [[4, 5, 6, 7], [0, 3, 2, 1], [0, 1, 5, 4], [1, 2, 6, 5], [2, 3, 7, 6], [3, 0, 4, 7]];
        for q in quads {
            s.triangles.push([q[0], q[1], q[2]]);
            s.triangles.push([q[0], q[2], q[3]]);
        }
        let mut nif = Nif::new();
        nif.add_block(Block::Node(Node::default()));
        let mut hinge = Node::default();
        hinge.av.net.name = Some("Gate".into());
        hinge.av.transform.translation = [50.0, 0.0, 0.0];
        let hi = nif.add_block(Block::Node(hinge));
        let ts = nif.add_block(Block::TextureSet(vec!["textures\\a.dds".into()]));
        let sh = nif.add_block(Block::LightingShader(LightingShader { texture_set: Some(ts), ..Default::default() }));
        s.shader_property = Some(sh);
        s.vertex_desc = VertexDesc::with_attributes(true, true, true, false, false).0;
        let si = nif.add_block(Block::TriShape(s));
        if let Block::Node(h) = &mut nif.blocks[hi as usize] {
            h.children = vec![Some(si)];
        }
        if let Block::Node(r) = &mut nif.blocks[0] {
            r.children = vec![Some(hi)];
        }
        let (closed, _) = generate_nif(&nif, &LEVELS[0], "t").unwrap();
        let (lo, hi_) = extent(&closed);
        assert!(lo[0] >= 49.0 && hi_[0] <= 171.0 && hi_[1] <= 3.0, "closed extent {lo:?}..{hi_:?}");
        let pose = Pose::from_channels([EndPose { node: "gate".into(), rotation: Some(lodforge_nif::anim::euler_matrix([0.0, 0.0, std::f32::consts::FRAC_PI_2])), ..Default::default() }]);
        let (open, _) = generate_nif_posed(&nif, &LEVELS[0], "t", Some(&pose)).unwrap();
        let (lo, hi_) = extent(&open);
        assert!(lo[1] >= -3.0 && hi_[1] >= 119.0 && hi_[0] <= 53.0 && lo[0] >= 47.0, "open extent {lo:?}..{hi_:?}");
        assert!(open_pose(&nif).is_none(), "no sequences in a file without a controller manager");
    }

    /// A long thin rope (one closed sagging tube) keeps its length at every level. Locked end
    /// vertices alone do not guarantee that: meshoptimizer may collapse an end's neighbours into
    /// the locked vertex until no triangle is left at that end, for almost no quadric error.
    #[test]
    fn rope_keeps_its_length() {
        let mut s = TriShape::default();
        let rings = 10usize;
        let sides = 6u16;
        for r in 0..rings {
            let t = r as f32 / (rings - 1) as f32;
            let x = -300.0 + 600.0 * t;
            let sag = -40.0 * (std::f32::consts::PI * t).sin();
            for v in 0..sides {
                let a = v as f32 / sides as f32 * std::f32::consts::TAU;
                s.positions.push([x, 1.5 * a.cos(), sag + 1.5 * a.sin()]);
                s.uvs.push([t, v as f32 / sides as f32]);
                s.normals.push([0.0, a.cos(), a.sin()]);
            }
        }
        for r in 0..rings - 1 {
            let b0 = (r as u16) * sides;
            let b1 = b0 + sides;
            for v in 0..sides {
                let w = (v + 1) % sides;
                s.triangles.push([b0 + v, b1 + v, b1 + w]);
                s.triangles.push([b0 + v, b1 + w, b0 + w]);
            }
        }
        let last = ((rings - 1) as u16) * sides;
        for v in 1..sides - 1 {
            s.triangles.push([0, v + 1, v]);
            s.triangles.push([last, last + v, last + v + 1]);
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
        for (k, p) in LEVELS.iter().enumerate() {
            let (out, st) = generate_nif(&nif, p, "t").unwrap();
            let (lo, hi) = extent(&out);
            assert!(hi[0] - lo[0] >= 600.0 - 2.0 * p.max_error, "level {k}: extent {lo:?}..{hi:?} {st:?}");
            assert!(st.triangles <= 60, "level {k}: {st:?}");
        }
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
        let out_dir = std::env::var_os("LODFORGE_LODMODEL_OUT");
        for (k, p) in super::LEVELS.iter().enumerate() {
            let r = super::generate_nif(&nif, p, "x");
            println!("level {k}: {:?}", r.as_ref().map(|r| r.1.clone()));
            if let (Some(dir), Ok((out, _))) = (&out_dir, &r) {
                let stem = std::path::Path::new(&path).file_stem().unwrap().to_string_lossy().to_string();
                std::fs::write(std::path::Path::new(dir).join(format!("{stem}_lod_{k}.nif")), out.to_bytes().unwrap()).unwrap();
            }
        }
    }
}

