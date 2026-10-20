//! Transforms (docs/re/object-lod-spec.md 2.2): the NIF node transform `v' = R (s v) + t` and
//! the placed-reference transform `T(position) R(rotation) S(scale)`.
//!
//! Derived from AnvilLOD `AnvilLOD.Meshes/Transforms.cs` (Copyright (c) 2026 CageTV,
//! GPL-3.0-only, commit 606eece): the reference rotation `R = Rx(-x) Ry(-y) Rz(-z)` with
//! column-vector rotation matrices ("verified against existing LODGen/DynDOLOD output to within
//! 0.004 units on a reference rotated on all three axes"), `RotationOnly` for normals and the
//! byte encoding of unit vectors. Modified: written for column vectors directly (no row-vector
//! `Matrix4x4`), the rotation is composed from degrees, and the NIF composition is a method.

/// 3x3 matrix, row-major, column-vector convention (`v' = M v`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mat3(pub [[f32; 3]; 3]);

impl Mat3 {
    pub const IDENTITY: Mat3 = Mat3([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);

    pub fn rot_x(a: f32) -> Mat3 {
        let (s, c) = a.sin_cos();
        Mat3([[1.0, 0.0, 0.0], [0.0, c, -s], [0.0, s, c]])
    }

    pub fn rot_y(a: f32) -> Mat3 {
        let (s, c) = a.sin_cos();
        Mat3([[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]])
    }

    pub fn rot_z(a: f32) -> Mat3 {
        let (s, c) = a.sin_cos();
        Mat3([[c, -s, 0.0], [s, c, 0.0], [0.0, 0.0, 1.0]])
    }

    pub fn mul(&self, o: &Mat3) -> Mat3 {
        let (a, b) = (&self.0, &o.0);
        let mut m = [[0f32; 3]; 3];
        for (i, row) in m.iter_mut().enumerate() {
            for (j, v) in row.iter_mut().enumerate() {
                *v = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
            }
        }
        Mat3(m)
    }

    pub fn scaled(&self, s: f32) -> Mat3 {
        Mat3(self.0.map(|r| r.map(|v| v * s)))
    }

    pub fn apply(&self, v: [f32; 3]) -> [f32; 3] {
        let m = &self.0;
        [m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2], m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2], m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2]]
    }
}

/// An affine transform `v' = m v + t` (`m` = rotation times uniform scale).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Xform {
    pub m: Mat3,
    pub t: [f32; 3],
}

impl Xform {
    pub const IDENTITY: Xform = Xform { m: Mat3::IDENTITY, t: [0.0; 3] };

    /// A NIF node / shape transform: `v' = R (s v) + t`, `R` as stored (rows).
    pub fn from_nif(t: &lodforge_nif::Transform) -> Xform {
        Xform { m: Mat3(t.rotation).scaled(t.scale), t: t.translation }
    }

    /// A placed reference: position, rotation in **degrees** (the export's columns 6-8) and the
    /// uniform `XSCL` scale. `R = Rx(-x) Ry(-y) Rz(-z)`.
    pub fn reference(position: [f32; 3], rotation_deg: [f32; 3], scale: f32) -> Xform {
        let r = rotation_deg.map(f32::to_radians);
        let m = Mat3::rot_x(-r[0]).mul(&Mat3::rot_y(-r[1])).mul(&Mat3::rot_z(-r[2]));
        Xform { m: m.scaled(scale), t: position }
    }

    /// `self` after `child`: `v' = self(child(v))`.
    pub fn then(&self, child: &Xform) -> Xform {
        Xform { m: self.m.mul(&child.m), t: add(self.m.apply(child.t), self.t) }
    }

    pub fn apply(&self, v: [f32; 3]) -> [f32; 3] {
        add(self.m.apply(v), self.t)
    }

    /// The rotation part (the matrix divided by its uniform scale), for normals and tangents.
    pub fn rotation_only(&self) -> Mat3 {
        let r = self.m.0[0];
        let s = (r[0] * r[0] + r[1] * r[1] + r[2] * r[2]).sqrt();
        if s < 1e-12 { Mat3::IDENTITY } else { self.m.scaled(1.0 / s) }
    }

    /// The transform without its translation (spec 2.2 `IgnoreTranslation`).
    pub fn without_translation(&self) -> Xform {
        Xform { m: self.m, t: [0.0; 3] }
    }
}

fn add(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

/// Unit vector, or `fallback` when the length is zero or not finite.
pub fn normalize(v: [f32; 3], fallback: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > 1e-8 && l.is_finite() { [v[0] / l, v[1] / l, v[2] / l] } else { fallback }
}

pub fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

pub fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 3], b: [f32; 3]) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < 1e-4)
    }

    #[test]
    fn reference_rotation() {
        // Rz(-90 deg) with the column-vector matrix turns +X into -Y.
        let x = Xform::reference([0.0; 3], [0.0, 0.0, 90.0], 1.0);
        assert!(close(x.apply([1.0, 0.0, 0.0]), [0.0, -1.0, 0.0]));
        // A rotation about Z keeps Z; scale and translation apply after the rotation.
        let x = Xform::reference([10.0, 20.0, 30.0], [0.0, 0.0, 135.0], 2.0);
        let p = x.apply([1.0, 0.0, 5.0]);
        assert!(close(p, [10.0 - 2.0 * std::f32::consts::FRAC_1_SQRT_2, 20.0 - 2.0 * std::f32::consts::FRAC_1_SQRT_2, 40.0]));
        assert!(close(x.rotation_only().apply([0.0, 0.0, 1.0]), [0.0, 0.0, 1.0]));
        // Composition: parent(child(v)).
        let child = Xform { m: Mat3::IDENTITY.scaled(3.0), t: [1.0, 0.0, 0.0] };
        let parent = Xform::reference([0.0, 0.0, 100.0], [0.0, 0.0, 90.0], 1.0);
        let both = parent.then(&child);
        assert!(close(both.apply([1.0, 0.0, 0.0]), parent.apply(child.apply([1.0, 0.0, 0.0]))));
        assert!(close(both.apply([1.0, 0.0, 0.0]), [0.0, -4.0, 100.0]));
        assert!(close(normalize([0.0, 0.0, 0.0], [0.0, 0.0, 1.0]), [0.0, 0.0, 1.0]));
        assert!(close(cross([1.0, 0.0, 0.0], [0.0, 1.0, 0.0]), [0.0, 0.0, 1.0]));
        let n = lodforge_nif::Transform { translation: [0.0, 0.0, 1.0], rotation: [[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]], scale: 2.0 };
        assert!(close(Xform::from_nif(&n).apply([1.0, 0.0, 0.0]), [0.0, 2.0, 1.0]));
    }
}
