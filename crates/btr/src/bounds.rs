//! Bounding boxes, bounding spheres and multibound AABBs as observed in the golden terrain files
//! (docs/re/terrain-lod-spec.md 1.4, 1.7, 1.8), plus the vertex-cache triangle ordering used by
//! the object LOD builder.

pub use crate::vcache::forsyth_order;

/// Axis-aligned box; empty when `min > max`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BBox {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Default for BBox {
    fn default() -> Self {
        BBox {
            min: [f32::INFINITY; 3],
            max: [f32::NEG_INFINITY; 3],
        }
    }
}

/// BSMultiBoundAABB: centre and half size.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Aabb {
    pub position: [f32; 3],
    pub extent: [f32; 3],
}

impl BBox {
    pub fn from_points(points: &[[f32; 3]]) -> Self {
        let mut b = BBox::default();
        for p in points {
            b.add(*p);
        }
        b
    }

    pub fn add(&mut self, p: [f32; 3]) {
        for i in 0..3 {
            self.min[i] = self.min[i].min(p[i]);
            self.max[i] = self.max[i].max(p[i]);
        }
    }

    /// Alias of [`BBox::add`].
    pub fn grow(&mut self, p: [f32; 3]) {
        self.add(p);
    }

    /// Extends the box to contain `other`.
    pub fn grow_box(&mut self, other: &BBox) {
        if !other.is_empty() {
            self.add(other.min);
            self.add(other.max);
        }
    }

    pub fn is_empty(&self) -> bool {
        (0..3).any(|i| self.min[i] > self.max[i])
    }

    /// Top of the box as the multibound rule sees it: max z, but never below 0.
    fn top(&self, clamp_top: bool) -> f32 {
        if clamp_top {
            self.max[2].max(0.0)
        } else {
            self.max[2]
        }
    }

    /// Component-wise (min + max) / 2; with `clamp_top` the top of the box is clamped to z >= 0.
    pub fn center(&self, clamp_top: bool) -> [f32; 3] {
        [
            (self.min[0] + self.max[0]) / 2.0,
            (self.min[1] + self.max[1]) / 2.0,
            (self.min[2] + self.top(clamp_top)) / 2.0,
        ]
    }

    /// Component-wise (max - min) / 2 with the same clamp.
    pub fn extent(&self, clamp_top: bool) -> [f32; 3] {
        [
            (self.max[0] - self.min[0]) / 2.0,
            (self.max[1] - self.min[1]) / 2.0,
            (self.top(clamp_top) - self.min[2]) / 2.0,
        ]
    }

    /// Bounding-sphere radius of a land shape: distance from the centre to the max corner.
    pub fn radius(&self) -> f32 {
        let c = self.center(false);
        ((self.max[0] - c[0]).powi(2) + (self.max[1] - c[1]).powi(2) + (self.max[2] - c[2]).powi(2))
            .sqrt()
    }

    /// Bounding-sphere radius of a water shape: largest distance from the centre to any point.
    pub fn max_radius(&self, points: &[[f32; 3]]) -> f32 {
        let c = self.center(false);
        points
            .iter()
            .map(|p| ((p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2) + (p[2] - c[2]).powi(2)).sqrt())
            .fold(0.0, f32::max)
    }

    /// The multibound AABB of a quad-local box: game units (x `level`), top clamped to z >= 0.
    pub fn lod_aabb(&self, level: f32) -> Aabb {
        let c = self.center(true);
        let e = self.extent(true);
        Aabb {
            position: [c[0] * level, c[1] * level, c[2] * level],
            extent: [e[0] * level, e[1] * level, e[2] * level],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aabb_rule_from_the_spec() {
        // Tamriel.4.0.0.btr: land bbox z -1919..-990 local, x/y 0..4096 -> AABB (8192, 8192, -3838) / (8192, 8192, 3838).
        let b = BBox {
            min: [0.0, 0.0, -1919.0],
            max: [4096.0, 4096.0, -990.0],
        };
        let a = b.lod_aabb(4.0);
        assert_eq!(a.position, [8192.0, 8192.0, -3838.0]);
        assert_eq!(a.extent, [8192.0, 8192.0, 3838.0]);
        // Water quad of side 1024: radius 724.077.
        let w = [
            [0.0, 0.0, 1.0],
            [1024.0, 0.0, 1.0],
            [0.0, 1024.0, 1.0],
            [1024.0, 1024.0, 1.0],
        ];
        let wb = BBox::from_points(&w);
        assert!((wb.max_radius(&w) - 724.077).abs() < 0.01);
        assert_eq!(wb.center(false), [512.0, 512.0, 1.0]);
    }
}
