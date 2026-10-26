// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! Hemi-octahedral direction <-> UV mapping shared by the impostor baker, the viewer preview and
//! the runtime shader (docs/runtime/impostors.md is the contract; any change here must be made
//! there too, or runtime frame selection desyncs from the baked atlases).
//!
//! Ported from Project Terra's `HemiOctMath.cs` (the user's own project) with the up axis moved
//! from Unity's +Y to Skyrim's +Z. Unity is left-handed and Skyrim right-handed, so the swap of
//! the y and z axes is a reflection: Terra's formulas carry over with `y <-> z`, and the frame
//! basis keeps the right-handed screen convention of `lodforge-texgen`
//! (`right x up = toward the camera`).
//!
//! Conventions: the upper hemisphere (z >= 0) of the tree's model space. uv (0.5, 0.5) is the
//! top-down view; the unit square's perimeter is the horizon ring. Frames use grid-**vertex**
//! sampling: frame (i, j) of an N x N grid is captured from `uv_to_direction((i, j) / (N - 1))`
//! and stored in atlas cell column i, row j (rows counted from the top of the image).

use glam::{Vec2, Vec3};

/// View direction (tree -> camera, model space) for atlas frame (i, j) of an N x N grid.
pub fn frame_to_direction(i: u32, j: u32, grid: u32) -> Vec3 {
    let n = (grid.max(2) - 1) as f32;
    uv_to_direction(Vec2::new(i as f32 / n, j as f32 / n))
}

/// Decodes uv in [0,1]^2 to a unit direction on the upper (z >= 0) hemisphere.
pub fn uv_to_direction(uv: Vec2) -> Vec3 {
    let s = uv.x * 2.0 - 1.0;
    let t = uv.y * 2.0 - 1.0;
    let x = (s + t) * 0.5;
    let y = (s - t) * 0.5;
    let z = 1.0 - x.abs() - y.abs();
    Vec3::new(x, y, z).normalize()
}

/// Encodes a unit direction with z >= 0 to uv in [0,1]^2; the exact inverse of
/// [`uv_to_direction`] on the upper hemisphere.
pub fn direction_to_uv(d: Vec3) -> Vec2 {
    let n = d.x.abs() + d.y.abs() + d.z;
    let x = d.x / n;
    let y = d.y / n;
    Vec2::new((x + y) * 0.5 + 0.5, (x - y) * 0.5 + 0.5)
}

/// Capture-camera basis for a frame direction `dir` (tree -> camera): `(right, up)` are the
/// image's axes in model space, with `right x up = dir`. A frame's image x grows along `right`
/// and its image y (rows, top to bottom) along `-up`. Top-down views (|dir.z| >= 0.999) have no
/// horizon-derived basis and use right = +X, up = +Y.
pub fn frame_basis(dir: Vec3) -> (Vec3, Vec3) {
    if dir.z.abs() < 0.999 {
        let right = Vec3::Z.cross(dir).normalize();
        let up = dir.cross(right);
        (right, up)
    } else {
        (Vec3::X, Vec3::Y)
    }
}

/// Atlas sub-rectangle (normalized, `[x, y, w, h]`, y down) holding frame (i, j) of an N x N grid.
pub fn frame_uv_rect(i: u32, j: u32, grid: u32) -> [f32; 4] {
    let inv = 1.0 / grid as f32;
    [i as f32 * inv, j as f32 * inv, inv, inv]
}

/// The three grid-vertex frames whose triangle encloses the encoded direction `enc` (from
/// [`direction_to_uv`]) and their barycentric weights (summing to 1). Adjacent views
/// cross-dissolve instead of popping (Terra's `HemiOctFrames3`).
pub fn frames3(enc: Vec2, grid: u32) -> ([[u32; 2]; 3], [f32; 3]) {
    let max_idx = (grid.max(2) - 1) as f32;
    let g = enc.clamp(Vec2::ZERO, Vec2::ONE) * max_idx;
    // Base vertex, clamped so base + 1 stays on the grid.
    let b = g.floor().min(Vec2::splat(max_idx - 1.0));
    let f = g - b;
    let (bx, by) = (b.x as u32, b.y as u32);
    if f.x + f.y <= 1.0 {
        ([[bx, by], [bx + 1, by], [bx, by + 1]], [1.0 - f.x - f.y, f.x, f.y])
    } else {
        ([[bx + 1, by + 1], [bx + 1, by], [bx, by + 1]], [f.x + f.y - 1.0, 1.0 - f.y, 1.0 - f.x])
    }
}

/// Frame-local texture coordinates (0..1, y down like D3D / wgpu) of a model-space offset
/// `p` (relative to the impostor centre) in frame (i, j): the projection onto that frame's
/// capture plane, `quad` units across.
pub fn frame_local_uv(p: Vec3, frame: [u32; 2], grid: u32, quad: f32) -> Vec2 {
    let (right, up) = frame_basis(frame_to_direction(frame[0], frame[1], grid));
    Vec2::new(0.5 + p.dot(right) / quad, 0.5 - p.dot(up) / quad)
}

/// The hemisphere direction an impostor uses for a camera at `cam` (model space, relative to
/// the impostor centre): normalized, with z raised to at least `min_z` (views from below the
/// horizon reuse the horizon ring).
pub fn view_direction(cam: Vec3, min_z: f32) -> Vec3 {
    let d = cam.normalize_or(Vec3::Z);
    Vec3::new(d.x, d.y, d.z.max(min_z)).normalize()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_dir(expected: Vec3, actual: Vec3, ctx: &str) {
        assert!((expected - actual).abs().max_element() < 1e-4, "{ctx}: expected {expected}, got {actual}");
    }

    #[test]
    fn anchor_points_map_as_documented() {
        assert_dir(Vec3::Z, uv_to_direction(Vec2::new(0.5, 0.5)), "center = top-down");
        assert_dir(Vec3::new(-1.0, 0.0, 0.0), uv_to_direction(Vec2::new(0.0, 0.0)), "corner (0,0)");
        assert_dir(Vec3::new(1.0, 0.0, 0.0), uv_to_direction(Vec2::new(1.0, 1.0)), "corner (1,1)");
        assert_dir(Vec3::new(0.0, 1.0, 0.0), uv_to_direction(Vec2::new(1.0, 0.0)), "corner (1,0)");
        assert_dir(Vec3::new(0.0, -1.0, 0.0), uv_to_direction(Vec2::new(0.0, 1.0)), "corner (0,1)");
    }

    #[test]
    fn encode_decode_round_trips_upper_hemisphere() {
        // Deterministic sweep over the hemisphere, poles and horizon included.
        for a in 0..24 {
            for e in 0..=12 {
                let azimuth = a as f32 * std::f32::consts::TAU / 24.0;
                let elevation = e as f32 * std::f32::consts::FRAC_PI_2 / 12.0;
                let dir = Vec3::new(elevation.cos() * azimuth.cos(), elevation.cos() * azimuth.sin(), elevation.sin());
                assert_dir(dir, uv_to_direction(direction_to_uv(dir)), &format!("azimuth {a}, elevation {e}"));
            }
        }
    }

    #[test]
    fn frame_directions_are_unit_upper_hemisphere_with_horizon_perimeter() {
        let n = 12;
        for j in 0..n {
            for i in 0..n {
                let dir = frame_to_direction(i, j, n);
                assert!((dir.length() - 1.0).abs() < 1e-4, "frame ({i},{j}) unit length");
                assert!(dir.z >= -1e-4, "frame ({i},{j}) upper hemisphere");
                if i == 0 || j == 0 || i == n - 1 || j == n - 1 {
                    assert!(dir.z.abs() < 1e-4, "frame ({i},{j}) on the horizon ring");
                }
            }
        }
        // Odd grid: the exact center frame is the top-down view.
        assert_dir(Vec3::Z, frame_to_direction(6, 6, 13), "odd-grid center");
    }

    #[test]
    fn frame_basis_is_orthonormal_and_right_handed_toward_the_camera() {
        let n = 12;
        for j in 0..n {
            for i in 0..n {
                let dir = frame_to_direction(i, j, n);
                let (right, up) = frame_basis(dir);
                assert!((right.length() - 1.0).abs() < 1e-4);
                assert!((up.length() - 1.0).abs() < 1e-4);
                assert!(right.dot(up).abs() < 1e-4);
                // Screen axes are perpendicular to the view direction, and right x up is the
                // direction toward the camera (Terra, left-handed, asserts -dir here).
                assert!(right.dot(dir).abs() < 1e-4);
                assert!(up.dot(dir).abs() < 1e-4);
                assert_dir(dir, right.cross(up), &format!("frame ({i},{j}) toward"));
                // Side views keep model up on screen.
                if dir.z < 0.99 {
                    assert!(up.z > 0.0, "frame ({i},{j}) up points up");
                }
            }
        }
    }

    #[test]
    fn frame_basis_top_down_fallback_is_deterministic() {
        let (right, up) = frame_basis(Vec3::Z);
        assert_dir(Vec3::X, right, "top-down right");
        assert_dir(Vec3::Y, up, "top-down up");
    }

    #[test]
    fn side_basis_matches_texgen_yaw_views() {
        // Camera on +Y (texgen yaw 0): image right = -X, up = +Z.
        let (right, up) = frame_basis(Vec3::Y);
        assert_dir(Vec3::new(-1.0, 0.0, 0.0), right, "yaw 0 right");
        assert_dir(Vec3::Z, up, "yaw 0 up");
    }

    #[test]
    fn frame_uv_rects_tile_the_atlas_exactly() {
        let n = 12;
        for j in 0..n {
            for i in 0..n {
                let r = frame_uv_rect(i, j, n);
                assert!((r[0] - i as f32 / n as f32).abs() < 1e-6);
                assert!((r[1] - j as f32 / n as f32).abs() < 1e-6);
                assert!((r[0] + r[2] - (i + 1) as f32 / n as f32).abs() < 1e-6);
                assert!((r[1] + r[3] - (j + 1) as f32 / n as f32).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn frames3_weights_sum_to_one_and_hit_exact_frames() {
        let n = 16;
        // On a grid vertex the weight is all on that frame.
        for (i, j) in [(0, 0), (3, 7), (15, 15), (15, 0), (0, 15), (8, 8)] {
            let dir = frame_to_direction(i, j, n);
            let (f, w) = frames3(direction_to_uv(dir), n);
            let best = (0..3).max_by(|&a, &b| w[a].total_cmp(&w[b])).unwrap();
            assert_eq!(f[best], [i, j], "vertex ({i},{j})");
            assert!((w[best] - 1.0).abs() < 1e-3);
        }
        // Anywhere: weights in 0..1 summing to 1, frames on the grid and adjacent.
        for a in 0..37 {
            for e in 0..10 {
                let az = a as f32 * 0.17;
                let el = e as f32 * 0.16;
                let dir = Vec3::new(el.cos() * az.cos(), el.cos() * az.sin(), el.sin());
                let (f, w) = frames3(direction_to_uv(dir), n);
                assert!((w.iter().sum::<f32>() - 1.0).abs() < 1e-5);
                assert!(w.iter().all(|v| (-1e-5..=1.0 + 1e-5).contains(v)));
                for k in f {
                    assert!(k[0] < n && k[1] < n);
                }
            }
        }
    }

    #[test]
    fn frame_local_uv_centres_the_impostor_and_follows_the_image_axes() {
        let n = 12;
        let quad = 200.0;
        let c = frame_local_uv(Vec3::ZERO, [3, 4], n, quad);
        assert!((c - Vec2::splat(0.5)).abs().max_element() < 1e-6);
        // Side frame (0,0) looks from -X: right = cross(Z, -X) = -Y, up = Z.
        let dir = frame_to_direction(0, 0, n);
        assert_dir(Vec3::new(-1.0, 0.0, 0.0), dir, "frame (0,0)");
        let top = frame_local_uv(Vec3::new(0.0, 0.0, 50.0), [0, 0], n, quad);
        assert!((top.y - 0.25).abs() < 1e-6, "higher = smaller v (rows top to bottom)");
        let side = frame_local_uv(Vec3::new(0.0, -50.0, 0.0), [0, 0], n, quad);
        assert!((side.x - 0.75).abs() < 1e-6, "along right = larger u");
    }

    #[test]
    fn view_direction_clamps_below_the_horizon() {
        let d = view_direction(Vec3::new(10.0, 0.0, -5.0), 0.02);
        assert!(d.z > 0.0 && (d.length() - 1.0).abs() < 1e-5);
        assert_dir(Vec3::Z, view_direction(Vec3::ZERO, 0.02), "degenerate camera");
    }
}
