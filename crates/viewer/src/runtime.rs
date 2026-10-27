// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! LODForge Runtime output, drawn the way the runtime SKSE plugin draws it (D:\Modlists\
//! LODForge-Runtime: `InstanceCull.hlsl`, `StaticCull.hlsl`, `ImpostorVS.hlsl`, `Settings.h`).
//!
//! - **Trees** (`<World>[.<SEASON>].trees.bin`, docs/runtime/instance-formats.md 5): every tree
//!   the runtime owns. Near the camera a tree draws its type's LOD4 3D model from
//!   `TreeMeshes.bin` (the mesh tier, up to `fBlockLevel0Distance`, handed over to the impostor
//!   per tree inside a 4096-unit band by a hash rank); beyond that its hemi-octahedral impostor
//!   (`textures\LODForge\Impostors\<World>\`, docs/runtime/impostors.md): the nearest frame
//!   with a per-tree jittered switch angle, alpha test 0.5, lit by the game's DistantTree pixel
//!   shader as a billboard (the albedo is pre-lit; per-type brightness / tint is not applied in
//!   game). Impostors thin out by hash rank from 60000 to 200000 units (35 % kept) and fade
//!   out from 200000 to 240000.
//! - **Repeated statics** (`<World>[.<SEASON>].statics.bin` + `StaticMeshes.bin`, section 6):
//!   object LOD models the build instanced instead of merging into the `.bto`; each record is
//!   drawn inside its level's distance band (LOD4 up to `fBlockLevel0Distance`, LOD8 up to
//!   `fBlockLevel1Distance`, ...) with a 2048-unit dithered hand-over per reference.
//!
//! The runtime decides by the distance from the camera; the viewer does the same (per frame on
//! the CPU for the instanced meshes, in the vertex shader for the impostors).

use std::ops::Range;

use glam::{Mat3, Vec3, Vec4};
use serde::{Deserialize, Serialize};

use lodforge_instances::meshpack::{self, MeshPack};
use lodforge_instances::statics::{MESH_LARGE_REF, StaticsFile};
use lodforge_instances::trees::TreeFile;

use crate::files::{Files, normalize};
use crate::lodselect::{LodDistances, LodMode};

/// Folder of the runtime's data files (Data-relative, lower case).
pub const DATA_DIR: &str = "skse\\plugins\\lodforgeruntime";

/// The runtime files of one worldspace and season (`None`: absent).
#[derive(Debug, Clone, Default, Serialize)]
pub struct RuntimeFiles {
    pub trees: Option<String>,
    pub statics: Option<String>,
    /// Dynamic LOD (`.dyn.bin`) and far grass (`.lfg`): found, not drawn.
    pub dynamic: Option<String>,
    pub grass: Option<String>,
    pub tree_pack: Option<String>,
    pub static_pack: Option<String>,
    pub impostor_index: Option<String>,
}

impl RuntimeFiles {
    /// The output was built for the runtime (it owns trees or repeated statics).
    pub fn present(&self) -> bool {
        self.trees.is_some() || self.statics.is_some()
    }
}

/// Finds the runtime files of `world`. Whether the output is a runtime build is decided by what
/// `lister` (the output) has; the shared packs and textures may resolve through `files`.
pub fn discover(files: &dyn Files, lister: &dyn Files, world: &str, season: Option<&str>) -> RuntimeFiles {
    let base = format!("{DATA_DIR}\\{}", world.to_ascii_lowercase());
    let own = |ext: &str| {
        if let Some(s) = season {
            let p = normalize(&format!("{base}.{s}.{ext}"));
            if lister.exists(&p) {
                return Some(p);
            }
        }
        let p = normalize(&format!("{base}.{ext}"));
        lister.exists(&p).then_some(p)
    };
    let shared = |p: String| {
        let p = normalize(&p);
        (lister.exists(&p) || files.exists(&p)).then_some(p)
    };
    let mut r = RuntimeFiles { trees: own("trees.bin"), statics: own("statics.bin"), dynamic: own("dyn.bin"), grass: own("lfg"), ..Default::default() };
    if r.trees.is_some() {
        r.tree_pack = shared(format!("{DATA_DIR}\\{}", meshpack::FILE_NAME));
        r.impostor_index = shared(crate::impostors::index_path(world));
    }
    if r.statics.is_some() {
        r.static_pack = shared(format!("{DATA_DIR}\\staticmeshes.bin"));
    }
    r
}

/// Which tree / repeated-static LOD the viewer draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LodSource {
    /// The runtime's when the output has runtime files, else the classic LOD.
    #[default]
    Auto,
    /// As the game draws it with the LODForge Runtime plugin installed.
    Runtime,
    /// As the game draws it without the plugin: `.bto` / `.btt` only.
    Classic,
}

impl LodSource {
    pub fn label(self) -> &'static str {
        match self {
            LodSource::Auto => "auto",
            LodSource::Runtime => "runtime",
            LodSource::Classic => "classic",
        }
    }

    pub fn parse(s: &str) -> Option<LodSource> {
        [LodSource::Auto, LodSource::Runtime, LodSource::Classic].into_iter().find(|m| m.label().eq_ignore_ascii_case(s))
    }

    /// Whether runtime LOD is drawn when the scene has (`available`) runtime data.
    pub fn runtime(self, available: bool) -> bool {
        available && self != LodSource::Classic
    }
}

/// The runtime's tree and statics settings (`LODForgeRuntime.ini` defaults, `Settings.h`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RuntimeParams {
    /// `[Trees] fMeshTierEnd`: where the mesh tier ends (0 = `fBlockLevel0Distance`).
    pub mesh_tier_end: f32,
    /// `fMeshTierBand`: width of the dithered mesh -> impostor hand-over before that distance.
    pub mesh_tier_band: f32,
    /// `fFadeOutStart`, `fFadeOutEnd` of the impostors.
    pub tree_fade_out: [f32; 2],
    /// `fThinStart`, `fThinEnd`, `fThinMinKeep` of the impostors.
    pub tree_thin: [f32; 3],
    /// `fFrameDither`: per-tree jitter of the frame-switch angle, in frames.
    pub frame_dither: f32,
    /// `fAlphaRef`: the impostors' alpha-test reference.
    pub alpha_ref: f32,
    /// `[Statics] fBand`: width of the dithered LOD-level hand-over before each level's end.
    pub static_band: f32,
    /// Blend the three frames around the view direction (the shader contract) instead of the
    /// runtime's nearest frame.
    pub blend_frames: bool,
    /// Apply the impostor types' `brightness * tint` (the runtime cannot yet: CS's DistantTree
    /// pixel shader has no colour input).
    pub impostor_tint: bool,
}

impl Default for RuntimeParams {
    fn default() -> Self {
        RuntimeParams {
            mesh_tier_end: 0.0,
            mesh_tier_band: 4096.0,
            tree_fade_out: [200_000.0, 240_000.0],
            tree_thin: [60_000.0, 200_000.0, 0.35],
            frame_dither: 0.35,
            alpha_ref: 0.5,
            static_band: 2048.0,
            blend_frames: false,
            impostor_tint: false,
        }
    }
}

impl RuntimeParams {
    /// The mesh tier's end for the switch distances in use.
    pub fn mesh_end(&self, d: &LodDistances, mult: f32) -> f32 {
        if self.mesh_tier_end > 0.0 { self.mesh_tier_end } else { d.level0 * mult }
    }

    /// The shader constants (`Globals.rt`): (mesh end, band, frame dither, alpha ref), (fade
    /// start, fade end, thin start, thin end), (thin keep, blend frames, rules on, tint).
    pub fn gpu(&self, d: &LodDistances, mult: f32, mesh_tier: bool, rules: bool) -> [[f32; 4]; 3] {
        [
            [if mesh_tier { self.mesh_end(d, mult) } else { 0.0 }, self.mesh_tier_band, self.frame_dither, self.alpha_ref],
            [self.tree_fade_out[0], self.tree_fade_out[1], self.tree_thin[0], self.tree_thin[1]],
            [self.tree_thin[2], f32::from(u8::from(self.blend_frames)), f32::from(u8::from(rules)), f32::from(u8::from(self.impostor_tint))],
        ]
    }
}

/// Hash ranks of the runtime's culling shaders (`(hash * k) >> 8` over 2^24).
pub fn rank(hash: u32, k: u32) -> f32 {
    (hash.wrapping_mul(k) >> 8) as f32 * (1.0 / 16_777_216.0)
}

pub const RANK_MESH: u32 = 2_654_435_761;

/// A tree of the trees file.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tree {
    pub pos: [f32; 3],
    /// Z rotation as the reference stores it: world = P + Rz(-yaw) * s * model.
    pub yaw: f32,
    pub scale: f32,
    pub type_: u16,
    pub hash: u32,
    /// The type's mesh in `TreeMeshes.bin` (`meshpack::NO_MESH`: none, the impostor draws near too).
    pub mesh: u32,
    /// Impostor centre height above the trunk base at scale 1 (the runtime's distance point).
    pub center_z: f32,
}

impl Tree {
    /// Model -> world rows (rotation * scale, translation in w).
    pub fn rows(&self) -> [[f32; 4]; 3] {
        rows(Mat3::from_rotation_z(-self.yaw) * self.scale, self.pos)
    }

    /// The point the runtime measures the tree's distance to.
    pub fn distance_point(&self) -> Vec3 {
        Vec3::new(self.pos[0], self.pos[1], self.pos[2] + self.center_z * self.scale)
    }
}

/// A record of the statics file.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Static {
    pub rows: [[f32; 4]; 3],
    /// Bounding sphere (world).
    pub center: [f32; 3],
    pub radius: f32,
    /// Mesh in `StaticMeshes.bin`.
    pub mesh: u32,
    /// 0 = LOD4, 1 = LOD8, 2 = LOD16, 3 = LOD32.
    pub level: u8,
    pub hash: u32,
    pub large_ref: bool,
}

fn rows(m: Mat3, t: [f32; 3]) -> [[f32; 4]; 3] {
    let r = m.transpose();
    [[r.x_axis.x, r.x_axis.y, r.x_axis.z, t[0]], [r.y_axis.x, r.y_axis.y, r.y_axis.z, t[1]], [r.z_axis.x, r.z_axis.y, r.z_axis.z, t[2]]]
}

/// The bounding sphere of a model-space AABB under rows.
fn sphere(r: &[[f32; 4]; 3], b: &[f32; 6]) -> ([f32; 3], f32) {
    let c = [(b[0] + b[3]) * 0.5, (b[1] + b[4]) * 0.5, (b[2] + b[5]) * 0.5];
    let half = Vec3::new(b[3] - b[0], b[4] - b[1], b[5] - b[2]).length() * 0.5;
    let scale = (0..3).map(|i| Vec3::new(r[0][i], r[1][i], r[2][i]).length()).fold(0.0f32, f32::max);
    let w = |k: usize| r[k][0] * c[0] + r[k][1] * c[1] + r[k][2] * c[2] + r[k][3];
    ([w(0), w(1), w(2)], half * scale)
}

/// Trees of a trees file, with their impostor centres (`impostor_center_z`, per type) and pack
/// meshes (`TMPK`; empty: no mesh tier).
pub fn trees_of(f: &TreeFile, impostor_center_z: &[f32]) -> Vec<Tree> {
    let mut out = Vec::with_capacity(f.records());
    for (&cell, recs) in &f.cells {
        for r in recs {
            let t = usize::from(r.type_);
            out.push(Tree {
                pos: r.position(cell),
                yaw: r.yaw_rad(),
                scale: r.scale.to_f32(),
                type_: r.type_,
                hash: r.hash,
                mesh: f.pack_meshes.get(t).copied().unwrap_or(meshpack::NO_MESH),
                center_z: impostor_center_z.get(t).copied().unwrap_or(0.0),
            });
        }
    }
    out
}

/// Records of a statics file that have a pack mesh; `bounds` are the pack meshes' model-space
/// AABBs.
pub fn statics_of(f: &StaticsFile, bounds: &[[f32; 6]]) -> Vec<Static> {
    let mut out = Vec::with_capacity(f.records());
    for (&(level, _, _), recs) in &f.blocks {
        let li = match level {
            4 => 0,
            8 => 1,
            16 => 2,
            _ => 3,
        };
        for r in recs {
            let Some(&mesh) = f.pack_meshes.get(usize::from(r.mesh)) else { continue };
            let Some(b) = bounds.get(mesh as usize) else { continue };
            let [ax, ay, az] = r.angles_rad();
            let m = Mat3::from_rotation_x(-ax) * Mat3::from_rotation_y(-ay) * Mat3::from_rotation_z(-az) * r.scale.to_f32();
            let rw = rows(m, r.pos);
            let (center, radius) = sphere(&rw, b);
            let large_ref = f.meshes.get(usize::from(r.mesh)).is_some_and(|m| m.flags & MESH_LARGE_REF != 0);
            out.push(Static { rows: rw, center, radius, mesh, level: li, hash: r.hash, large_ref });
        }
    }
    out
}

/// One instance of a pack mesh for the GPU: model -> world rows.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PackInstance {
    pub rows: [[f32; 4]; 3],
}

/// One pack mesh's index ranges, by atlas page.
#[derive(Debug, Clone, Default)]
pub struct PackMesh {
    pub ranges: Vec<(u16, Range<u32>)>,
    pub triangles: u32,
}

/// Index ranges of every mesh of a pack: a mesh's clusters are contiguous and grouped by page,
/// each `3 * 64` indices (degenerate padding included).
pub fn pack_meshes(p: &MeshPack) -> Vec<PackMesh> {
    let n = (3 * meshpack::CLUSTER_TRIANGLES) as u32;
    p.meshes
        .iter()
        .map(|m| {
            let mut ranges: Vec<(u16, Range<u32>)> = Vec::new();
            for c in p.clusters.iter().skip(m.first_cluster as usize).take(m.cluster_count as usize) {
                match ranges.last_mut() {
                    Some((page, r)) if *page == c.page && r.end == c.first_index => r.end += n,
                    _ => ranges.push((c.page, c.first_index..c.first_index + n)),
                }
            }
            PackMesh { ranges, triangles: m.triangles }
        })
        .collect()
}

/// A frame's instances of one pack: instances grouped by mesh, and the draws (page, index range,
/// instance range).
#[derive(Debug, Default)]
pub struct PackFrame {
    pub instances: Vec<PackInstance>,
    pub draws: Vec<(u16, Range<u32>, Range<u32>)>,
    pub triangles: u64,
}

impl PackFrame {
    fn from_pairs(mut pairs: Vec<(u32, PackInstance)>, meshes: &[PackMesh]) -> PackFrame {
        pairs.sort_unstable_by_key(|p| p.0);
        let mut f = PackFrame { instances: Vec::with_capacity(pairs.len()), ..Default::default() };
        let mut i = 0;
        while i < pairs.len() {
            let mesh = pairs[i].0;
            let start = f.instances.len() as u32;
            while i < pairs.len() && pairs[i].0 == mesh {
                f.instances.push(pairs[i].1);
                i += 1;
            }
            let inst = start..f.instances.len() as u32;
            if let Some(m) = meshes.get(mesh as usize) {
                f.triangles += u64::from(m.triangles) * u64::from(inst.end - inst.start);
                for (page, r) in &m.ranges {
                    f.draws.push((*page, r.clone(), inst.clone()));
                }
            }
        }
        // Fewer texture rebinds.
        f.draws.sort_by_key(|d| d.0);
        f
    }
}

fn sphere_visible(planes: &[Vec4; 5], c: Vec3, r: f32) -> bool {
    planes.iter().all(|p| p.truncate().dot(c) + p.w >= -r)
}

/// The mesh-tier trees for a camera: those with a pack mesh whose hash rank puts them on the
/// mesh side of the hand-over band (`InstanceCull.hlsl`); the impostor shader drops the same ones.
pub fn select_tree_meshes(trees: &[Tree], meshes: &[PackMesh], bounds: &[[f32; 6]], cam: Vec3, planes: &[Vec4; 5], end: f32, band: f32) -> PackFrame {
    if end <= 0.0 {
        return PackFrame::default();
    }
    let pairs = trees
        .iter()
        .filter(|t| t.mesh != meshpack::NO_MESH)
        .filter_map(|t| {
            let dist = t.distance_point().distance(cam);
            if dist >= end {
                return None;
            }
            let mesh_t = ((dist - (end - band)) / band.max(1.0)).clamp(0.0, 1.0);
            if rank(t.hash, RANK_MESH) < mesh_t {
                return None;
            }
            let r = t.rows();
            let (c, rad) = sphere(&r, bounds.get(t.mesh as usize)?);
            sphere_visible(planes, Vec3::from(c), rad).then_some((t.mesh, PackInstance { rows: r }))
        })
        .collect();
    PackFrame::from_pairs(pairs, meshes)
}

/// The statics records drawn for a camera (`StaticCull.hlsl`): in the game arrangement each
/// record inside its level's distance band, the switch to the next level placed inside the band
/// by the reference's hash; with one level shown, that level's records.
pub fn select_statics(st: &[Static], meshes: &[PackMesh], cam: Vec3, planes: &[Vec4; 5], mode: LodMode, d: &LodDistances, band: f32) -> PackFrame {
    let ends = |mult: f32| [d.level0 * mult, d.level1 * mult, d.maximum * mult, f32::INFINITY];
    let pairs = st
        .iter()
        .filter(|s| match mode {
            LodMode::Single(level) => i32::from(4u8 << s.level) == level,
            LodMode::Game { distance_mult } => {
                let e = ends(distance_mult);
                let dist = Vec3::from(s.center).distance(cam);
                let rk = rank(s.hash, RANK_MESH);
                let switch = |edge: f32| edge - band + band * rk;
                let li = usize::from(s.level);
                !(li > 0 && dist < switch(e[li - 1])) && dist < switch(e[li])
            }
        })
        .filter(|s| sphere_visible(planes, Vec3::from(s.center), s.radius))
        .map(|s| (s.mesh, PackInstance { rows: s.rows }))
        .collect();
    PackFrame::from_pairs(pairs, meshes)
}

/// Pack vertices for the GPU (`render::PackVertex`): model space, unpacked normal frame.
pub fn pack_vertices(p: &MeshPack) -> Vec<crate::render::PackVertex> {
    // Every vertex's page, from the clusters that use it.
    let mut page = vec![0u32; p.vertices.len()];
    for c in &p.clusters {
        let s = c.first_index as usize;
        for &i in p.indices.get(s..s + 3 * meshpack::CLUSTER_TRIANGLES).unwrap_or(&[]) {
            if let Some(v) = page.get_mut(i as usize) {
                *v = u32::from(c.page);
            }
        }
    }
    p.vertices
        .iter()
        .zip(page)
        .map(|(v, page)| {
            let (n, _) = meshpack::unpack_unit(v.normal);
            let (t, w) = meshpack::unpack_unit(v.tangent);
            let (nv, tv) = (Vec3::from(n), Vec3::from(t));
            let b = nv.cross(tv) * if w & 1 == 1 { 1.0 } else { -1.0 };
            crate::render::PackVertex {
                pos: v.pos,
                uv: [f32::from(v.uv[0]) / 65535.0, f32::from(v.uv[1]) / 65535.0],
                normal: crate::render::snorm(n),
                tangent: crate::render::snorm(t),
                bitangent: crate::render::snorm(b.to_array()),
                color: v.color,
                page,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use lodforge_instances::meshpack::{Cluster, Mesh};

    #[test]
    fn ranks_and_rows() {
        assert_eq!(rank(0, RANK_MESH), 0.0);
        assert!(rank(u32::MAX, 1) > 0.99);
        let t = Tree { pos: [100.0, 200.0, 300.0], yaw: std::f32::consts::FRAC_PI_2, scale: 2.0, type_: 0, hash: 0, mesh: 0, center_z: 50.0 };
        let r = t.rows();
        // Model +X at yaw 90 (clockwise): world -Y, times the scale.
        let p = |v: [f32; 3]| [0, 1, 2].map(|k| r[k][0] * v[0] + r[k][1] * v[1] + r[k][2] * v[2] + r[k][3]);
        let w = p([1.0, 0.0, 0.0]);
        assert!((w[0] - 100.0).abs() < 1e-4 && (w[1] - 198.0).abs() < 1e-4 && (w[2] - 300.0).abs() < 1e-4, "{w:?}");
        assert_eq!(t.distance_point(), Vec3::new(100.0, 200.0, 400.0));
    }

    #[test]
    fn pack_ranges_group_pages() {
        let c = |i: u32, page: u16| Cluster { first_index: i * 192, first_vertex: 0, mesh: 0, page, triangles: 64, flags: 0, min: [0.0; 3], max: [0.0; 3] };
        let p = MeshPack {
            meshes: vec![Mesh { first_cluster: 0, cluster_count: 3, triangles: 150, ..Default::default() }, Mesh { first_cluster: 3, cluster_count: 1, triangles: 10, ..Default::default() }],
            clusters: vec![c(0, 0), c(1, 0), c(2, 1), c(3, 1)],
            ..Default::default()
        };
        let m = pack_meshes(&p);
        assert_eq!(m[0].ranges, vec![(0, 0..384), (1, 384..576)]);
        assert_eq!(m[1].ranges, vec![(1, 576..768)]);
        let inst = PackInstance { rows: [[0.0; 4]; 3] };
        let f = PackFrame::from_pairs(vec![(1, inst), (0, inst), (1, inst)], &m);
        assert_eq!(f.instances.len(), 3);
        assert_eq!(f.triangles, 150 + 20);
        assert_eq!(f.draws, vec![(0, 0..384, 0..1), (1, 384..576, 0..1), (1, 576..768, 1..3)]);
    }

    #[test]
    fn statics_bands() {
        let s = |level: u8, x: f32| Static { rows: [[1.0, 0.0, 0.0, x], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0]], center: [x, 0.0, 0.0], radius: 1.0, mesh: 0, level, hash: 0, large_ref: false };
        let m = vec![PackMesh { ranges: vec![(0, 0..192)], triangles: 1 }];
        let d = LodDistances { level0: 10_000.0, level1: 20_000.0, maximum: 40_000.0 };
        let planes = [Vec4::new(0.0, 0.0, 0.0, 1.0); 5];
        let all = vec![s(0, 5_000.0), s(1, 5_000.0), s(0, 15_000.0), s(1, 15_000.0), s(3, 100_000.0)];
        let f = select_statics(&all, &m, Vec3::ZERO, &planes, LodMode::Game { distance_mult: 1.0 }, &d, 2048.0);
        // LOD4 near, LOD8 in its band, LOD32 far.
        assert_eq!(f.instances.len(), 3);
        let xs: Vec<f32> = f.instances.iter().map(|i| i.rows[0][3]).collect();
        assert!(xs.contains(&5_000.0) && xs.contains(&15_000.0) && xs.contains(&100_000.0));
        let f = select_statics(&all, &m, Vec3::ZERO, &planes, LodMode::Single(8), &d, 2048.0);
        assert_eq!(f.instances.len(), 2);
    }
}
