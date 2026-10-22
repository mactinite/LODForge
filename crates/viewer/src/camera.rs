// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! Cameras over Skyrim's world (X east, Y north, Z up): a free camera (heading clockwise from
//! north, pitch up) and a top-down overview, both producing a reversed-Z projection.

use glam::{Mat4, Vec3, Vec3Swizzles, Vec4};
use serde::{Deserialize, Serialize};

/// Free camera. `yaw` is the heading in degrees clockwise from north (+Y), as the game's Z
/// rotation; `pitch` is degrees above the horizon.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FreeCam {
    pub pos: [f32; 3],
    pub yaw: f32,
    pub pitch: f32,
    /// Horizontal field of view, degrees: the game's `fov` console value (and skyrim-mcp's
    /// vantage `fov`) is horizontal, so the same number gives the same framing.
    pub fov: f32,
}

impl Default for FreeCam {
    fn default() -> Self {
        FreeCam {
            pos: [0.0, 0.0, 20000.0],
            yaw: 0.0,
            pitch: -30.0,
            fov: 70.0,
        }
    }
}

impl FreeCam {
    pub fn forward(&self) -> Vec3 {
        let (sy, cy) = self.yaw.to_radians().sin_cos();
        let (sp, cp) = self.pitch.to_radians().sin_cos();
        Vec3::new(sy * cp, cy * cp, sp).normalize()
    }

    pub fn right(&self) -> Vec3 {
        let (sy, cy) = self.yaw.to_radians().sin_cos();
        Vec3::new(cy, -sy, 0.0)
    }

    pub fn view(&self) -> Mat4 {
        Mat4::look_to_rh(Vec3::from(self.pos), self.forward(), Vec3::Z)
    }

    /// Reversed-Z infinite perspective (depth 1 at the near plane, 0 at infinity).
    pub fn projection(&self, aspect: f32, near: f32) -> Mat4 {
        Mat4::perspective_infinite_reverse_rh(self.vertical_fov(aspect), aspect, near)
    }

    /// Vertical field of view (radians) for an aspect ratio, from the horizontal `fov`.
    pub fn vertical_fov(&self, aspect: f32) -> f32 {
        let half = (self.fov.to_radians() * 0.5).tan() / aspect.max(1e-3);
        2.0 * half.atan()
    }

    /// Turns by mouse deltas (pixels); positive dx turns right, positive dy looks down.
    pub fn turn(&mut self, dx: f32, dy: f32, sensitivity: f32) {
        self.yaw = (self.yaw + dx * sensitivity).rem_euclid(360.0);
        self.pitch = (self.pitch - dy * sensitivity).clamp(-89.0, 89.0);
    }

    /// Looks at a point from the current position.
    pub fn look_at(&mut self, target: [f32; 3]) {
        let d = Vec3::from(target) - Vec3::from(self.pos);
        if d.length() < 1e-3 {
            return;
        }
        self.yaw = d.x.atan2(d.y).to_degrees().rem_euclid(360.0);
        self.pitch = (d.z / d.xy().length().max(1e-6))
            .atan()
            .to_degrees()
            .clamp(-89.0, 89.0);
    }

    /// `x,y,z,yaw,pitch` (yaw and pitch optional).
    pub fn parse(s: &str) -> Option<FreeCam> {
        let v: Vec<f32> = s
            .split(',')
            .map(|p| p.trim().parse::<f32>().ok())
            .collect::<Option<_>>()?;
        if v.len() < 3 || v.len() > 5 {
            return None;
        }
        Some(FreeCam {
            pos: [v[0], v[1], v[2]],
            yaw: v.get(3).copied().unwrap_or(0.0),
            pitch: v.get(4).copied().unwrap_or(-20.0),
            ..Default::default()
        })
    }

    pub fn format(&self) -> String {
        format!(
            "{:.0},{:.0},{:.0},{:.1},{:.1}",
            self.pos[0], self.pos[1], self.pos[2], self.yaw, self.pitch
        )
    }
}

/// Top-down orthographic view of the map.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Overview {
    pub center: [f32; 2],
    /// Half the visible height in game units.
    pub half_height: f32,
    /// Camera height above the highest geometry (depth range only).
    pub top_z: f32,
    pub bottom_z: f32,
}

impl Default for Overview {
    fn default() -> Self {
        Overview {
            center: [0.0, 0.0],
            half_height: 200000.0,
            top_z: 200000.0,
            bottom_z: -100000.0,
        }
    }
}

impl Overview {
    pub fn view(&self) -> Mat4 {
        // Looking straight down; +Y (north) up on screen.
        Mat4::look_to_rh(
            Vec3::new(self.center[0], self.center[1], self.top_z),
            -Vec3::Z,
            Vec3::Y,
        )
    }

    /// Reversed-Z orthographic projection.
    pub fn projection(&self, aspect: f32) -> Mat4 {
        let h = self.half_height;
        let w = h * aspect;
        // Swapping near/far makes glam's [0,1] orthographic depth reversed.
        Mat4::orthographic_rh(-w, w, -h, h, self.top_z - self.bottom_z, 0.0)
    }

    /// World (x, y) under a pixel.
    pub fn world_at(&self, px: f32, py: f32, width: f32, height: f32) -> [f32; 2] {
        let aspect = width / height.max(1.0);
        let nx = (px / width.max(1.0)) * 2.0 - 1.0;
        let ny = 1.0 - (py / height.max(1.0)) * 2.0;
        [
            self.center[0] + nx * self.half_height * aspect,
            self.center[1] + ny * self.half_height,
        ]
    }
}

/// What the renderer needs from either camera.
#[derive(Debug, Clone, Copy)]
pub struct ViewParams {
    pub view: Mat4,
    pub proj: Mat4,
    pub eye: Vec3,
    pub ortho: bool,
}

impl ViewParams {
    pub fn free(c: &FreeCam, aspect: f32) -> ViewParams {
        ViewParams {
            view: c.view(),
            proj: c.projection(aspect, 8.0),
            eye: Vec3::from(c.pos),
            ortho: false,
        }
    }

    pub fn overview(o: &Overview, aspect: f32) -> ViewParams {
        ViewParams {
            view: o.view(),
            proj: o.projection(aspect),
            eye: Vec3::new(o.center[0], o.center[1], o.top_z),
            ortho: true,
        }
    }

    pub fn view_proj(&self) -> Mat4 {
        self.proj * self.view
    }

    /// Frustum planes (a, b, c, d with ax + by + cz + d >= 0 inside) from the view-projection,
    /// for wgpu's z in [0, 1] (reversed: near = 1). The far plane is left out (infinite).
    pub fn frustum(&self) -> [Vec4; 5] {
        let m = self.view_proj().transpose();
        let (r0, r1, r2, r3) = (m.x_axis, m.y_axis, m.z_axis, m.w_axis);
        // Reversed-Z: inside is 0 <= z/w <= 1 -> z >= 0 (far, dropped) and w - z >= 0 (near).
        [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r3 - r2].map(|p| p / p.truncate().length().max(1e-9))
    }

    /// World-space ray through a pixel: (origin, direction).
    pub fn ray(&self, px: f32, py: f32, width: f32, height: f32) -> (Vec3, Vec3) {
        let inv = self.view_proj().inverse();
        let nx = (px / width.max(1.0)) * 2.0 - 1.0;
        let ny = 1.0 - (py / height.max(1.0)) * 2.0;
        // Reversed depth: near plane at z = 1.
        let near = inv.project_point3(Vec3::new(nx, ny, 1.0));
        let far = inv.project_point3(Vec3::new(nx, ny, 0.001));
        if self.ortho {
            (near, (far - near).normalize_or_zero())
        } else {
            (self.eye, (far - near).normalize_or_zero())
        }
    }
}

/// Whether an axis-aligned box touches the frustum.
pub fn aabb_in_frustum(planes: &[Vec4; 5], min: [f32; 3], max: [f32; 3]) -> bool {
    planes.iter().all(|p| {
        let v = Vec3::new(
            if p.x >= 0.0 { max[0] } else { min[0] },
            if p.y >= 0.0 { max[1] } else { min[1] },
            if p.z >= 0.0 { max[2] } else { min[2] },
        );
        p.truncate().dot(v) + p.w >= 0.0
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heading_convention() {
        let c = FreeCam {
            yaw: 0.0,
            pitch: 0.0,
            ..Default::default()
        };
        assert!((c.forward() - Vec3::Y).length() < 1e-6);
        let c = FreeCam {
            yaw: 90.0,
            pitch: 0.0,
            ..Default::default()
        };
        assert!((c.forward() - Vec3::X).length() < 1e-5);
        assert!((c.right() + Vec3::Y).length() < 1e-5);
        let mut c = FreeCam {
            pos: [0.0, 0.0, 0.0],
            ..Default::default()
        };
        c.look_at([1000.0, 1000.0, 1000.0]);
        assert!(
            (c.yaw - 45.0).abs() < 1e-3 && (c.pitch - 35.264).abs() < 1e-2,
            "{c:?}"
        );
        let p = FreeCam::parse("1, 2, 3, 90, -10").unwrap();
        assert_eq!((p.pos, p.yaw, p.pitch), ([1.0, 2.0, 3.0], 90.0, -10.0));
        assert!(FreeCam::parse("1,2").is_none());
    }

    #[test]
    fn frustum_and_rays() {
        let c = FreeCam {
            pos: [0.0, 0.0, 100.0],
            yaw: 0.0,
            pitch: 0.0,
            fov: 90.0,
        };
        let v = ViewParams::free(&c, 1.0);
        let f = v.frustum();
        assert!(aabb_in_frustum(
            &f,
            [-10.0, 1000.0, 90.0],
            [10.0, 1010.0, 110.0]
        ));
        assert!(!aabb_in_frustum(
            &f,
            [-10.0, -1010.0, 90.0],
            [10.0, -1000.0, 110.0]
        ));
        // Far away but in front: still inside (no far plane).
        assert!(aabb_in_frustum(
            &f,
            [-10.0, 1.0e7, 90.0],
            [10.0, 1.0e7 + 10.0, 110.0]
        ));
        let (o, d) = v.ray(50.0, 50.0, 100.0, 100.0);
        assert!((o - Vec3::new(0.0, 0.0, 100.0)).length() < 1e-3);
        assert!((d - Vec3::Y).length() < 1e-3, "{d:?}");
        // Reversed depth: a near point projects to z ~ 1, a far one near 0.
        let vp = v.view_proj();
        let near = vp.project_point3(Vec3::new(0.0, 9.0, 100.0)).z;
        let far = vp.project_point3(Vec3::new(0.0, 1.0e6, 100.0)).z;
        assert!(near > 0.8 && far < 0.01, "{near} {far}");
        let o = Overview {
            center: [1000.0, 2000.0],
            half_height: 500.0,
            top_z: 100.0,
            bottom_z: -100.0,
        };
        assert_eq!(o.world_at(0.0, 0.0, 200.0, 100.0), [0.0, 2500.0]);
        let ov = ViewParams::overview(&o, 2.0);
        let z = ov
            .view_proj()
            .project_point3(Vec3::new(1000.0, 2000.0, 50.0))
            .z;
        assert!(z > 0.5 && z < 1.0, "{z}");
    }
}
