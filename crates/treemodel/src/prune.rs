//! Derived from TreeLODGen, Copyright (C) 2026 Clemus (GPL-3.0, commit dc4cc8a); ported to Rust and
//! modified for LODForge 2026-10-23. See THIRD-PARTY-NOTICES.txt part 1.7.
//!
//! Crown pruning (TreeLODGen `Prune.Apply`, `Split.ClampSmallUvOvershoot`; spec 4.2), in the
//! shape's local space:
//!
//! 1. leaf pieces = welded islands; nothing to do with fewer than two;
//! 2. pieces in Morton order of their centroids (10 bits per axis over the shape's bounding cube);
//! 3. error diffusion keeps the share `keep` evenly along that order (no randomness);
//! 4. kept pieces grow about their centroid by `min(2, sqrt(max(1, total * area / kept)))` so
//!    the crown keeps `area` of its leaf area, then are pulled back inside the crown's outline
//!    (24 height bands, the widest radius in each and its neighbours);
//! 5. the dropped pieces' vertices go.
//!
//! Normals and tangents are not changed.

use crate::islands::{triangle_area, welded};
use crate::mesh::PartMesh;

pub const MAX_CARD_SCALE: f32 = 2.0;
pub const BANDS: usize = 24;

fn spread(q: u32) -> u64 {
    (0..10).map(|b| (u64::from((q >> b) & 1)) << (3 * b)).sum()
}

fn morton(c: [f32; 3], min: [f32; 3], size: f32) -> u64 {
    if size <= 0.0 {
        return 0;
    }
    let q = |k: usize| (((c[k] - min[k]) / size * 1023.0) as i32).clamp(0, 1023) as u32;
    spread(q(0)) | (spread(q(1)) << 1) | (spread(q(2)) << 2)
}

/// The crown's outline: radius per height band about the mean x/y of all vertices.
struct Profile {
    cx: f32,
    cy: f32,
    min_z: f32,
    max_z: f32,
    radius: [f32; BANDS],
}

impl Profile {
    fn of(positions: &[[f32; 3]]) -> Profile {
        let n = positions.len().max(1) as f32;
        let cx = positions.iter().map(|p| p[0]).sum::<f32>() / n;
        let cy = positions.iter().map(|p| p[1]).sum::<f32>() / n;
        let min_z = positions.iter().map(|p| p[2]).fold(f32::INFINITY, f32::min);
        let max_z = positions.iter().map(|p| p[2]).fold(f32::NEG_INFINITY, f32::max);
        let mut p = Profile { cx, cy, min_z, max_z, radius: [0.0; BANDS] };
        let mut r = [0.0f32; BANDS];
        for v in positions {
            let b = p.band(v[2]);
            r[b] = r[b].max(((v[0] - cx).powi(2) + (v[1] - cy).powi(2)).sqrt());
        }
        for i in 0..BANDS {
            let lo = r[i.saturating_sub(1)];
            let hi = r[(i + 1).min(BANDS - 1)];
            p.radius[i] = r[i].max(lo).max(hi);
        }
        p
    }

    fn band(&self, z: f32) -> usize {
        (((z - self.min_z) / (self.max_z - self.min_z).max(1e-3) * BANDS as f32) as i32).clamp(0, BANDS as i32 - 1) as usize
    }

    fn clamp(&self, v: [f32; 3]) -> [f32; 3] {
        let z = v[2].clamp(self.min_z, self.max_z);
        let limit = self.radius[self.band(z)];
        let (dx, dy) = (v[0] - self.cx, v[1] - self.cy);
        let rad = (dx * dx + dy * dy).sqrt();
        if rad <= limit || rad < 1e-3 {
            return [v[0], v[1], z];
        }
        let k = limit / rad;
        [self.cx + dx * k, self.cy + dy * k, z]
    }
}

/// Prunes a crown mesh to the share `keep` of its pieces, restoring `area` of its leaf area.
/// Returns false when nothing changed.
pub fn apply(m: &mut PartMesh, keep: f32, area: f32) -> bool {
    let islands = welded(&m.positions, &m.triangles);
    if islands.len() < 2 || m.positions.is_empty() {
        return false;
    }
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for p in &m.positions {
        for k in 0..3 {
            min[k] = min[k].min(p[k]);
            max[k] = max[k].max(p[k]);
        }
    }
    let size = (0..3).map(|k| max[k] - min[k]).fold(0.0f32, f32::max);
    // Per island: unique vertices, centroid, area.
    let info: Vec<(Vec<u32>, [f32; 3], f32)> = islands
        .iter()
        .map(|isl| {
            let mut verts: Vec<u32> = isl.triangles.iter().flat_map(|&t| m.triangles[t]).collect();
            verts.sort_unstable();
            verts.dedup();
            let n = verts.len().max(1) as f32;
            let mut c = [0.0f32; 3];
            for &v in &verts {
                for (ck, pk) in c.iter_mut().zip(m.positions[v as usize]) {
                    *ck += pk;
                }
            }
            let a: f32 = isl.triangles.iter().map(|&t| {
                let [x, y, z] = m.triangles[t].map(|i| m.positions[i as usize]);
                triangle_area(x, y, z)
            }).sum();
            (verts, c.map(|v| v / n), a)
        })
        .collect();
    let mut order: Vec<usize> = (0..islands.len()).collect();
    order.sort_by_key(|&i| morton(info[i].1, min, size)); // stable: ties keep island order
    let mut kept = vec![false; islands.len()];
    let mut acc = 0.5f32;
    for &i in &order {
        acc += keep;
        if acc >= 1.0 {
            acc -= 1.0;
            kept[i] = true;
        }
    }
    if !kept.iter().any(|&k| k) {
        return false;
    }
    let total: f32 = info.iter().map(|x| x.2).sum();
    let kept_area: f32 = info.iter().zip(&kept).filter(|(_, k)| **k).map(|(x, _)| x.2).sum();
    let scale = if kept_area > 0.0 { MAX_CARD_SCALE.min((total * area / kept_area).max(1.0).sqrt()) } else { 1.0 };
    let profile = Profile::of(&m.positions);
    for (i, (verts, c, _)) in info.iter().enumerate() {
        if !kept[i] {
            continue;
        }
        for &v in verts {
            let p = m.positions[v as usize];
            let grown = [c[0] + (p[0] - c[0]) * scale, c[1] + (p[1] - c[1]) * scale, c[2] + (p[2] - c[2]) * scale];
            m.positions[v as usize] = profile.clamp(grown);
        }
    }
    let mut remove = vec![false; m.positions.len()];
    for (i, (verts, _, _)) in info.iter().enumerate() {
        if !kept[i] {
            for &v in verts {
                remove[v as usize] = true;
            }
        }
    }
    m.delete_vertices(&remove);
    true
}

/// UVs that leave [0, 1] by at most 0.05 are clamped into it (LODGen would split those
/// triangles); larger overshoots stay.
pub fn clamp_small_uv_overshoot(m: &mut PartMesh) {
    let inside = |v: f32| (-0.05..=1.05).contains(&v);
    if m.uvs.is_empty() || !m.uvs.iter().all(|uv| inside(uv[0]) && inside(uv[1])) {
        return;
    }
    if m.uvs.iter().any(|uv| !(0.0..=1.0).contains(&uv[0]) || !(0.0..=1.0).contains(&uv[1])) {
        for uv in &mut m.uvs {
            *uv = [uv[0].clamp(0.0, 1.0), uv[1].clamp(0.0, 1.0)];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `n` unit cards in a row along x, each two triangles.
    fn cards(n: usize) -> PartMesh {
        let mut m = PartMesh::default();
        for i in 0..n {
            let x = i as f32 * 3.0;
            let b = m.positions.len() as u32;
            m.positions.extend([[x, 0.0, 0.0], [x + 1.0, 0.0, 0.0], [x + 1.0, 0.0, 1.0], [x, 0.0, 1.0]]);
            m.uvs.extend([[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]);
            m.triangles.extend([[b, b + 1, b + 2], [b, b + 2, b + 3]]);
        }
        m
    }

    #[test]
    fn keeps_the_share_and_grows_the_kept_cards() {
        let mut m = cards(10);
        assert!(apply(&mut m, 0.4, 0.8));
        assert_eq!(m.triangles.len(), 8, "4 of 10 cards");
        // 10 * 0.8 / 4 = 2 -> scale sqrt(2): a card is now about 1.41 wide (before the outline clamp).
        let w = m.positions[1][0] - m.positions[0][0];
        assert!(w > 1.3 && w < 1.5, "{w}");
    }

    #[test]
    fn growth_is_capped_at_twice_the_size() {
        let mut m = cards(20);
        assert!(apply(&mut m, 0.05, 1.0));
        assert_eq!(m.triangles.len(), 2);
        let w = m.positions[1][0] - m.positions[0][0];
        assert!((w - 2.0).abs() < 1e-4, "{w}");
    }

    #[test]
    fn small_uv_overshoot_is_clamped() {
        let mut m = cards(1);
        m.uvs[1] = [1.03, 0.0];
        clamp_small_uv_overshoot(&mut m);
        assert_eq!(m.uvs[1], [1.0, 0.0]);
        m.uvs[1] = [1.2, 0.0];
        clamp_small_uv_overshoot(&mut m);
        assert_eq!(m.uvs[1], [1.2, 0.0]);
    }
}
