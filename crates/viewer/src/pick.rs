// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! Picking: the nearest triangle under a ray among the drawn shapes.

use glam::Vec3;

use crate::mesh::{Aabb, ShapeKind};
use crate::scene::Scene;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hit {
    pub quad: usize,
    pub shape: usize,
    pub triangle: u32,
    pub point: [f32; 3],
    pub distance: f32,
}

/// Slab test; returns the entry distance.
pub fn ray_aabb(origin: Vec3, dir: Vec3, b: &Aabb) -> Option<f32> {
    let inv = Vec3::new(1.0 / dir.x, 1.0 / dir.y, 1.0 / dir.z);
    let t0 = (Vec3::from(b.min) - origin) * inv;
    let t1 = (Vec3::from(b.max) - origin) * inv;
    let tmin = t0.min(t1).max_element().max(0.0);
    let tmax = t0.max(t1).min_element();
    (tmax >= tmin).then_some(tmin)
}

/// Möller–Trumbore, both sides.
pub fn ray_triangle(origin: Vec3, dir: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Option<f32> {
    let e1 = b - a;
    let e2 = c - a;
    let p = dir.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-9 {
        return None;
    }
    let inv = 1.0 / det;
    let s = origin - a;
    let u = s.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return None;
    }
    let q = s.cross(e1);
    let v = dir.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return None;
    }
    let t = e2.dot(q) * inv;
    (t > 0.0).then_some(t)
}

/// Nearest hit among `quads` (indices into the scene) for shapes whose kind passes `layer`.
pub fn pick(
    scene: &Scene,
    quads: &[usize],
    layer: &dyn Fn(ShapeKind, bool) -> bool,
    origin: Vec3,
    dir: Vec3,
) -> Option<Hit> {
    let mut best: Option<Hit> = None;
    for &qi in quads {
        let q = &scene.quads[qi];
        if ray_aabb(origin, dir, &q.aabb).is_none_or(|t| best.is_some_and(|b| t > b.distance)) {
            continue;
        }
        for (si, s) in q.shapes.iter().enumerate() {
            if !layer(s.kind, s.large_ref)
                || ray_aabb(origin, dir, &s.aabb)
                    .is_none_or(|t| best.is_some_and(|b| t > b.distance))
            {
                continue;
            }
            let cpu = &q.data.shapes[s.cpu];
            for (ti, t) in cpu.indices.chunks_exact(3).enumerate() {
                let (a, b, c) = (
                    Vec3::from(cpu.positions[t[0] as usize]),
                    Vec3::from(cpu.positions[t[1] as usize]),
                    Vec3::from(cpu.positions[t[2] as usize]),
                );
                if let Some(d) = ray_triangle(origin, dir, a, b, c) {
                    if best.is_none_or(|h| d < h.distance) {
                        best = Some(Hit {
                            quad: qi,
                            shape: si,
                            triangle: ti as u32,
                            point: (origin + dir * d).to_array(),
                            distance: d,
                        });
                    }
                }
            }
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intersections() {
        let b = Aabb {
            min: [0.0, 0.0, 0.0],
            max: [10.0, 10.0, 10.0],
        };
        assert_eq!(
            ray_aabb(Vec3::new(5.0, 5.0, 20.0), -Vec3::Z, &b),
            Some(10.0)
        );
        assert_eq!(ray_aabb(Vec3::new(50.0, 5.0, 20.0), -Vec3::Z, &b), None);
        let t = ray_triangle(
            Vec3::new(1.0, 1.0, 5.0),
            -Vec3::Z,
            Vec3::ZERO,
            Vec3::new(10.0, 0.0, 0.0),
            Vec3::new(0.0, 10.0, 0.0),
        );
        assert!((t.unwrap() - 5.0).abs() < 1e-6);
        // Reversed winding still hits.
        let t = ray_triangle(
            Vec3::new(1.0, 1.0, 5.0),
            -Vec3::Z,
            Vec3::ZERO,
            Vec3::new(0.0, 10.0, 0.0),
            Vec3::new(10.0, 0.0, 0.0),
        );
        assert!(t.is_some());
        assert!(
            ray_triangle(
                Vec3::new(20.0, 20.0, 5.0),
                -Vec3::Z,
                Vec3::ZERO,
                Vec3::new(10.0, 0.0, 0.0),
                Vec3::new(0.0, 10.0, 0.0)
            )
            .is_none()
        );
    }
}
