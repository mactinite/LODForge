//! NIF node transform (`v' = R v * s + t`, R row-major), composed child-under-parent.

use lodforge_nif::Transform;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Xf {
    pub r: [[f32; 3]; 3],
    pub s: f32,
    pub t: [f32; 3],
}

impl Xf {
    pub const IDENTITY: Xf = Xf { r: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], s: 1.0, t: [0.0; 3] };

    pub fn of(t: &Transform) -> Xf {
        Xf { r: t.rotation, s: t.scale, t: t.translation }
    }

    pub fn rot(&self, v: [f32; 3]) -> [f32; 3] {
        let r = &self.r;
        [r[0][0] * v[0] + r[0][1] * v[1] + r[0][2] * v[2], r[1][0] * v[0] + r[1][1] * v[1] + r[1][2] * v[2], r[2][0] * v[0] + r[2][1] * v[1] + r[2][2] * v[2]]
    }

    pub fn point(&self, v: [f32; 3]) -> [f32; 3] {
        let p = self.rot(v);
        [p[0] * self.s + self.t[0], p[1] * self.s + self.t[1], p[2] * self.s + self.t[2]]
    }

    /// The child's local transform `self` placed under `parent`.
    pub fn then(&self, parent: &Xf) -> Xf {
        let mut r = [[0.0f32; 3]; 3];
        for (i, row) in r.iter_mut().enumerate() {
            for (j, v) in row.iter_mut().enumerate() {
                *v = (0..3).map(|k| parent.r[i][k] * self.r[k][j]).sum();
            }
        }
        Xf { r, s: self.s * parent.s, t: parent.point(self.t) }
    }

    pub fn is_identity(&self) -> bool {
        *self == Xf::IDENTITY
    }
}
