//! Derived from TreeLODGen, Copyright (C) 2026 Clemus (GPL-3.0, commit dc4cc8a); ported to Rust and
//! modified for LODForge 2026-10-23. See THIRD-PARTY-NOTICES.txt part 1.7.
//!
//! Bark simplification (TreeLODGen `Simplify.cs`; spec 5): quadric half-edge collapse onto
//! existing vertices, in shape-local space.
//!
//! - Positions are welded on the 0.01 grid into points; faces that become degenerate are dropped.
//! - Each point gets the area-weighted plane quadrics of its faces, plus planes along open edges
//!   (perpendicular to the face) weighted ten times, so outlines hold.
//! - Cheapest collapse first (`from` onto its neighbour `to`, error measured at `to`); a collapse
//!   is refused when a face would flip or turn by more than about 78 degrees (normal dot < 0.2).
//! - Vertices never move: the surviving corners keep their original UV, normal, tangent and
//!   colour; a moved corner takes the attributes of the corner at `to` whose UV is closest, so it
//!   stays on its side of a UV seam.
//!
//! Divergence: equal costs pop in (from, to) order; .NET's priority queue breaks ties by its heap
//! layout, so results can differ slightly from TreeLODGen's (spec 5.6).

use std::cmp::Ordering;
use std::collections::{BTreeSet, BinaryHeap, HashMap};

use crate::islands::weld_key;
use crate::mesh::PartMesh;

pub const BOUNDARY_WEIGHT: f64 = 10.0;
pub const FOLD_DOT: f64 = 0.2;
/// Smallest budget a shape gets.
pub const MIN_SHAPE_TRIS: usize = 12;

type Q = [f64; 10];

fn plane_quadric(n: [f64; 3], p: [f64; 3], w: f64) -> Q {
    let d = -(n[0] * p[0] + n[1] * p[1] + n[2] * p[2]);
    [n[0] * n[0], n[0] * n[1], n[0] * n[2], n[0] * d, n[1] * n[1], n[1] * n[2], n[1] * d, n[2] * n[2], n[2] * d, d * d].map(|v| v * w)
}

fn add(a: &mut Q, b: &Q) {
    for i in 0..10 {
        a[i] += b[i];
    }
}

fn error(q: &Q, v: [f64; 3]) -> f64 {
    let [x, y, z] = v;
    q[0] * x * x + 2.0 * q[1] * x * y + 2.0 * q[2] * x * z + 2.0 * q[3] * x + q[4] * y * y + 2.0 * q[5] * y * z + 2.0 * q[6] * y + q[7] * z * z + 2.0 * q[8] * z + q[9]
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn len(a: [f64; 3]) -> f64 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}

/// Unit normal and area of a triangle (zero normal and area when degenerate).
fn normal(a: [f64; 3], b: [f64; 3], c: [f64; 3]) -> ([f64; 3], f64) {
    let n = cross(sub(b, a), sub(c, a));
    let l = len(n);
    if l <= 1e-12 { ([0.0; 3], 0.0) } else { (n.map(|v| v / l), l / 2.0) }
}

#[derive(PartialEq)]
struct Cand {
    cost: f64,
    from: u32,
    to: u32,
    vf: u32,
    vt: u32,
}

impl Eq for Cand {}

impl Ord for Cand {
    fn cmp(&self, o: &Self) -> Ordering {
        // Min-heap on cost, then (from, to).
        o.cost.partial_cmp(&self.cost).unwrap_or(Ordering::Equal).then_with(|| o.from.cmp(&self.from)).then_with(|| o.to.cmp(&self.to))
    }
}

impl PartialOrd for Cand {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

/// Simplifies `m` to at most `target` triangles where it can. Returns the triangle count.
pub fn simplify(m: &mut PartMesh, target: usize) -> usize {
    if m.triangles.len() <= target || m.uvs.len() != m.positions.len() {
        return m.triangles.len();
    }
    // Weld positions into points.
    let mut point_of = Vec::with_capacity(m.positions.len());
    let mut pos: Vec<[f64; 3]> = Vec::new();
    let mut keys: HashMap<[i32; 3], u32> = HashMap::new();
    for p in &m.positions {
        let id = *keys.entry(weld_key(*p)).or_insert_with(|| {
            pos.push(p.map(f64::from));
            (pos.len() - 1) as u32
        });
        point_of.push(id);
    }
    let mut faces: Vec<[u32; 3]> = Vec::new();
    let mut corners: Vec<[u32; 3]> = Vec::new();
    for t in &m.triangles {
        let f = t.map(|v| point_of[v as usize]);
        if f[0] == f[1] || f[1] == f[2] || f[0] == f[2] {
            continue;
        }
        faces.push(f);
        corners.push(*t);
    }
    let np = pos.len();
    let mut q: Vec<Q> = vec![[0.0; 10]; np];
    let mut edge_uses: HashMap<(u32, u32), u32> = HashMap::new();
    let ekey = |a: u32, b: u32| if a < b { (a, b) } else { (b, a) };
    let mut face_n: Vec<([f64; 3], f64)> = Vec::with_capacity(faces.len());
    for f in &faces {
        let (n, area) = normal(pos[f[0] as usize], pos[f[1] as usize], pos[f[2] as usize]);
        face_n.push((n, area));
        if area > 0.0 {
            let pq = plane_quadric(n, pos[f[0] as usize], area);
            for &v in f {
                add(&mut q[v as usize], &pq);
            }
        }
        for k in 0..3 {
            *edge_uses.entry(ekey(f[k], f[(k + 1) % 3])).or_default() += 1;
        }
    }
    for (f, &(n, area)) in faces.iter().zip(&face_n) {
        if area <= 0.0 {
            continue;
        }
        for k in 0..3 {
            let (a, b) = (f[k], f[(k + 1) % 3]);
            if edge_uses[&ekey(a, b)] != 1 {
                continue;
            }
            let wall = cross(sub(pos[b as usize], pos[a as usize]), n);
            let l = len(wall);
            if l > 0.0 {
                let pq = plane_quadric(wall.map(|v| v / l), pos[a as usize], area * BOUNDARY_WEIGHT);
                add(&mut q[a as usize], &pq);
                add(&mut q[b as usize], &pq);
            }
        }
    }
    let mut faces_of: Vec<BTreeSet<usize>> = vec![BTreeSet::new(); np];
    for (i, f) in faces.iter().enumerate() {
        for &v in f {
            faces_of[v as usize].insert(i);
        }
    }
    let mut alive = vec![true; faces.len()];
    let mut version = vec![0u32; np];
    let mut heap = BinaryHeap::new();
    let offer = |from: u32, faces_of: &Vec<BTreeSet<usize>>, faces: &Vec<[u32; 3]>, q: &Vec<Q>, version: &Vec<u32>, heap: &mut BinaryHeap<Cand>| {
        let mut seen = BTreeSet::new();
        for &f in &faces_of[from as usize] {
            for &to in &faces[f] {
                if to != from && seen.insert(to) {
                    let mut s = q[from as usize];
                    add(&mut s, &q[to as usize]);
                    heap.push(Cand { cost: error(&s, pos[to as usize]), from, to, vf: version[from as usize], vt: version[to as usize] });
                }
            }
        }
    };
    for p in 0..np as u32 {
        offer(p, &faces_of, &faces, &q, &version, &mut heap);
    }
    let mut remaining = faces.len();
    while remaining > target {
        let Some(c) = heap.pop() else { break };
        let (from, to) = (c.from as usize, c.to as usize);
        if version[from] != c.vf || version[to] != c.vt || faces_of[from].is_empty() {
            continue;
        }
        // Fold test on the faces that move.
        let mut ok = true;
        for &f in &faces_of[from] {
            let fc = faces[f];
            if fc.contains(&c.to) {
                continue;
            }
            let p = fc.map(|v| pos[v as usize]);
            let (before, _) = normal(p[0], p[1], p[2]);
            let moved = fc.map(|v| if v == c.from { pos[to] } else { pos[v as usize] });
            let (after, area) = normal(moved[0], moved[1], moved[2]);
            if area <= 0.0 || before[0] * after[0] + before[1] * after[1] + before[2] * after[2] < FOLD_DOT {
                ok = false;
                break;
            }
        }
        if !ok {
            continue;
        }
        // Original vertices used at corner `to`.
        let mut targets: Vec<u32> = Vec::new();
        for &f in &faces_of[to] {
            if alive[f] {
                let k = faces[f].iter().position(|&v| v == c.to).unwrap();
                let a = corners[f][k];
                if !targets.contains(&a) {
                    targets.push(a);
                }
            }
        }
        let moving: Vec<usize> = faces_of[from].iter().copied().collect();
        for f in moving {
            if faces[f].contains(&c.to) {
                alive[f] = false;
                remaining -= 1;
                for &v in &faces[f] {
                    if v != c.from {
                        faces_of[v as usize].remove(&f);
                    }
                }
            } else {
                let k = faces[f].iter().position(|&v| v == c.from).unwrap();
                let uv = m.uvs[corners[f][k] as usize];
                if let Some(&best) = targets.iter().min_by(|&&a, &&b| {
                    let da = (m.uvs[a as usize][0] - uv[0]).powi(2) + (m.uvs[a as usize][1] - uv[1]).powi(2);
                    let db = (m.uvs[b as usize][0] - uv[0]).powi(2) + (m.uvs[b as usize][1] - uv[1]).powi(2);
                    da.partial_cmp(&db).unwrap_or(Ordering::Equal)
                }) {
                    corners[f][k] = best;
                }
                faces[f][k] = c.to;
                faces_of[to].insert(f);
            }
        }
        faces_of[from].clear();
        let qf = q[from];
        add(&mut q[to], &qf);
        version[from] += 1;
        version[to] += 1;
        offer(c.to, &faces_of, &faces, &q, &version, &mut heap);
        let neighbours: BTreeSet<u32> = faces_of[to].iter().flat_map(|&f| faces[f]).filter(|&v| v != c.to).collect();
        for n in neighbours {
            offer(n, &faces_of, &faces, &q, &version, &mut heap);
        }
    }
    m.triangles = corners.iter().zip(&alive).filter(|(_, a)| **a).map(|(c, _)| *c).collect();
    m.delete_unreferenced();
    m.triangles.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cylinder of `seg` segments and `rings` rings (open ends), UVs tiling around it.
    fn cylinder(seg: usize, rings: usize) -> PartMesh {
        let mut m = PartMesh::default();
        for r in 0..=rings {
            for s in 0..=seg {
                let a = s as f32 / seg as f32 * std::f32::consts::TAU;
                m.positions.push([a.cos() * 10.0, a.sin() * 10.0, r as f32 * 20.0]);
                m.uvs.push([s as f32 / seg as f32, r as f32 / rings as f32]);
            }
        }
        let w = (seg + 1) as u32;
        for r in 0..rings as u32 {
            for s in 0..seg as u32 {
                let (a, b, c, d) = (r * w + s, r * w + s + 1, (r + 1) * w + s + 1, (r + 1) * w + s);
                m.triangles.push([a, b, c]);
                m.triangles.push([a, c, d]);
            }
        }
        m
    }

    #[test]
    fn reaches_the_budget_on_a_trunk() {
        let mut m = cylinder(16, 20);
        assert_eq!(m.triangles.len(), 640);
        let n = simplify(&mut m, 120);
        assert!(n <= 120 && n > 60, "{n}");
        assert!(m.positions.len() < 400);
        // Surviving corners keep source data: every UV is one of the grid's.
        for uv in &m.uvs {
            assert!((uv[1] * 20.0 - (uv[1] * 20.0).round()).abs() < 1e-4);
        }
    }

    #[test]
    fn small_meshes_stay() {
        let mut m = cylinder(4, 1);
        assert_eq!(simplify(&mut m, 100), 8);
    }
}
