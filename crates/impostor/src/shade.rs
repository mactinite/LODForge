// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! Pre-lighting of impostor frames. In game the impostors go through Community Shaders'
//! DistantTree pixel shader, which lights a flat quad as it lights TexGen billboards, so the
//! albedo must carry the tree's own shading the way a billboard does. Each covered texel of a frame
//! is lit on the CPU from what the bake already holds (unlit albedo, model-space normal, slab
//! depth):
//!
//! - **direct**: TexGen's billboard light (from the top, direct 1.35) by a wrapped N.L, times the
//!   sun's **transmittance** through the crown: the depth of foliage between the texel and the
//!   crown's lit surface, read from a depth map rendered from the light;
//! - **ambient**: TexGen's 0.55 times **occlusion** from the frame's own depth: texels recessed
//!   behind nearby foliage (the trunk and inner branches inside the crown) get less;
//!
//! in the textures' encoded (sRGB) space, as TexGen and the game's gamma-space lighting do.

use glam::{Vec2, Vec3};

use crate::hemioct;

/// Pre-lighting settings (part of the bake cache key).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shading {
    /// Off: the albedo stays unlit (the M2 baker's output).
    pub enabled: bool,
    /// TexGen `TreeBillboardsDirect` / `Ambient`.
    pub direct: f32,
    pub ambient: f32,
    /// Toward the light, model space (normalised when used).
    pub light: [f32; 3],
    /// Wrapped diffuse for thin foliage: `max((n.l + wrap) / (1 + wrap), 0)`.
    pub wrap: f32,
    /// Ambient occlusion: how much a fully recessed texel loses (0..1), and the recess depth (in
    /// bounding-sphere radii) at which it has lost 63% of that.
    pub ao_strength: f32,
    pub ao_depth: f32,
    /// Sun transmittance: foliage depth (in radii) that lets 37% of the light through.
    pub transmit: f32,
}

impl Default for Shading {
    fn default() -> Self {
        Shading { enabled: true, direct: 1.35, ambient: 0.55, light: [0.0, 0.0, 1.0], wrap: 0.3, ao_strength: 0.75, ao_depth: 0.25, transmit: 0.35 }
    }
}

impl Shading {
    /// The settings as 32-bit words (for the cache key).
    pub fn words(&self) -> [u32; 10] {
        [u32::from(self.enabled), self.direct.to_bits(), self.ambient.to_bits(), self.light[0].to_bits(), self.light[1].to_bits(), self.light[2].to_bits(), self.wrap.to_bits(), self.ao_strength.to_bits(), self.ao_depth.to_bits(), self.transmit.to_bits()]
    }

    pub fn light_dir(&self) -> Vec3 {
        Vec3::from(self.light).normalize_or(Vec3::Z)
    }
}

/// Depth of the crown as seen from the light: slab depth per texel (1 = nothing).
pub struct LightMap {
    pub size: u32,
    pub depth: Vec<f32>,
    right: Vec3,
    up: Vec3,
    toward: Vec3,
    center: Vec3,
    radius: f32,
}

impl LightMap {
    /// From a [`lodforge_texgen::Pass::ObjectNormalDepth`] render (RGBA8, depth in alpha) of the
    /// view toward `toward` framed like the impostor frames (the bounding sphere).
    pub fn from_normal_depth(nd: &[u8], size: u32, toward: Vec3, center: Vec3, radius: f32) -> LightMap {
        let (right, up) = hemioct::frame_basis(toward);
        let depth = nd.chunks_exact(4).map(|p| f32::from(p[3]) / 255.0).collect();
        LightMap { size, depth, right, up, toward, center, radius }
    }

    /// Sun transmittance at model-space point `p` (3 x 3 percentage filter).
    fn transmittance(&self, p: Vec3, s: &Shading) -> f32 {
        let q = p - self.center;
        let quad = 2.0 * self.radius;
        let uv = Vec2::new(0.5 + q.dot(self.right) / quad, 0.5 - q.dot(self.up) / quad);
        let d = (self.radius - q.dot(self.toward)) / quad;
        let n = self.size as i32;
        let (cx, cy) = ((uv.x * n as f32) as i32, (uv.y * n as f32) as i32);
        let ext = (s.transmit * self.radius).max(1e-3);
        // A small bias so a surface does not shadow itself.
        let bias = 0.02 * quad;
        let mut t = 0.0;
        for dy in -1..=1 {
            for dx in -1..=1 {
                let (x, y) = ((cx + dx).clamp(0, n - 1), (cy + dy).clamp(0, n - 1));
                let m = self.depth[(y * n + x) as usize];
                let thick = ((d - m) * quad - bias).max(0.0);
                t += (-thick / ext).exp();
            }
        }
        t / 9.0
    }
}

/// Lights the covered texels of one frame in place. `albedo` and `nd` are the frame's RGBA8 rows
/// (`frame` x `frame`); `dir` is the frame's view direction, `center` / `radius` the bounding
/// sphere.
pub fn shade_frame(albedo: &mut [u8], nd: &[u8], frame: u32, dir: Vec3, center: Vec3, radius: f32, light: &LightMap, s: &Shading) {
    if !s.enabled {
        return;
    }
    let f = frame as usize;
    let (right, up) = hemioct::frame_basis(dir);
    let quad = 2.0 * radius;
    let l = s.light_dir();
    let cov: Vec<bool> = albedo.chunks_exact(4).map(|p| p[3] > 0).collect();
    let covered = |i: usize| cov[i];
    // Nearest foliage in a neighbourhood: the local front of the crown (separable min filter).
    let reach = (f / 16).max(1) as i32;
    let depth: Vec<f32> = (0..f * f).map(|i| if covered(i) { f32::from(nd[i * 4 + 3]) / 255.0 } else { 1.0 }).collect();
    let mut rows = vec![1.0f32; f * f];
    for y in 0..f {
        for x in 0..f {
            let mut m = 1.0f32;
            for k in -reach..=reach {
                let xx = x as i32 + k;
                if xx >= 0 && (xx as usize) < f {
                    m = m.min(depth[y * f + xx as usize]);
                }
            }
            rows[y * f + x] = m;
        }
    }
    let mut front = vec![1.0f32; f * f];
    for y in 0..f {
        for x in 0..f {
            let mut m = 1.0f32;
            for k in -reach..=reach {
                let yy = y as i32 + k;
                if yy >= 0 && (yy as usize) < f {
                    m = m.min(rows[yy as usize * f + x]);
                }
            }
            front[y * f + x] = m;
        }
    }
    let ao_len = (s.ao_depth * radius).max(1e-3);
    for y in 0..f {
        for x in 0..f {
            let i = y * f + x;
            if !covered(i) {
                continue;
            }
            let n = Vec3::new(f32::from(nd[i * 4]), f32::from(nd[i * 4 + 1]), f32::from(nd[i * 4 + 2])) / 127.5 - Vec3::ONE;
            let n = n.normalize_or(Vec3::Z);
            let d = depth[i];
            let local = Vec2::new((x as f32 + 0.5) / f as f32, (y as f32 + 0.5) / f as f32);
            let p = center + right * ((local.x - 0.5) * quad) + up * ((0.5 - local.y) * quad) + dir * (radius - d * quad);
            let recess = ((d - front[i]) * quad).max(0.0);
            let ao = 1.0 - s.ao_strength * (1.0 - (-recess / ao_len).exp());
            let ndl = ((n.dot(l) + s.wrap) / (1.0 + s.wrap)).max(0.0);
            let t = light.transmittance(p, s);
            let k = s.ambient * ao + s.direct * ndl * t;
            for c in 0..3 {
                albedo[i * 4 + c] = (f32::from(albedo[i * 4 + c]) * k).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_map(size: u32) -> LightMap {
        LightMap::from_normal_depth(&vec![255u8; (size * size * 4) as usize], size, Vec3::Z, Vec3::ZERO, 10.0)
    }

    #[test]
    fn lit_open_surface_gets_ambient_plus_direct() {
        // One covered texel facing the light, nothing above it, nothing in front of it.
        let mut albedo = vec![0u8; 4 * 4 * 4];
        let mut nd = vec![0u8; 4 * 4 * 4];
        albedo[0..4].copy_from_slice(&[100, 100, 100, 255]);
        nd[0..4].copy_from_slice(&[128, 128, 255, 128]);
        shade_frame(&mut albedo, &nd, 4, Vec3::Y, Vec3::ZERO, 10.0, &empty_map(8), &Shading::default());
        // 100 * (0.55 + 1.35 * ~1.0)
        assert!((185..=191).contains(&albedo[0]), "{}", albedo[0]);
        assert_eq!(albedo[3], 255, "coverage untouched");
        assert_eq!(albedo[4], 0, "uncovered texels untouched");
    }

    #[test]
    fn recessed_and_shadowed_texels_are_darker() {
        let s = Shading::default();
        let f = 32;
        let mut albedo = vec![0u8; f * f * 4];
        let mut nd = vec![0u8; f * f * 4];
        for i in 0..f * f {
            albedo[i * 4..i * 4 + 4].copy_from_slice(&[100, 100, 100, 255]);
            // Front foliage everywhere at depth 0.2, the centre texel deep inside (0.8).
            nd[i * 4..i * 4 + 4].copy_from_slice(&[128, 128, 255, 51]);
        }
        let c = (f / 2) * f + f / 2;
        nd[c * 4 + 3] = 204;
        shade_frame(&mut albedo, &nd, f as u32, Vec3::Y, Vec3::ZERO, 10.0, &empty_map(8), &s);
        assert!(albedo[c * 4] + 40 < albedo[0], "recessed {} vs open {}", albedo[c * 4], albedo[0]);
        // Under a canopy: the light map sees foliage at depth 0 everywhere.
        let mut a2 = vec![100u8; 16];
        a2[3] = 255;
        let nd2 = [128u8, 128, 255, 128, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let canopy = LightMap::from_normal_depth(&vec![0u8; 8 * 8 * 4], 8, Vec3::Z, Vec3::ZERO, 10.0);
        shade_frame(&mut a2, &nd2, 2, Vec3::Y, Vec3::ZERO, 10.0, &canopy, &s);
        assert!(a2[0] < 80, "shadowed texel {}", a2[0]);
    }

    #[test]
    fn disabled_leaves_the_albedo() {
        let mut albedo = vec![100u8, 100, 100, 255];
        shade_frame(&mut albedo, &[128, 128, 255, 0], 1, Vec3::Y, Vec3::ZERO, 1.0, &empty_map(2), &Shading { enabled: false, ..Default::default() });
        assert_eq!(albedo, vec![100, 100, 100, 255]);
    }
}

/// Snow grade, applied to the pre-lit albedo when the pages are composed (after the bake cache,
/// so changing it re-bakes nothing).
///
/// In game the impostor quad goes through CS's DistantTree pixel shader, the near-tier 3D tree
/// LOD meshes through the Lighting shader. With the same albedo (the baked 3D LOD models and the
/// impostors of the Whiterun snow pines agree to 2%), the DistantTree path renders bright,
/// near-colourless surfaces much brighter and cyan: WIN snow pines near Whiterun read luma ~189
/// against ~138 for the 3D snow pines beside them (`f9-ft-whiterun-south`, 2026-10-25), while
/// dark, saturated foliage matches. The grade compresses the luminance of snow-like texels above
/// a knee and pulls their hue part-way to grey; saturated texels (needles, autumn leaves,
/// flowers) are left alone.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SnowGrade {
    /// Off: pixels unchanged.
    pub enabled: bool,
    /// Encoded (sRGB, 0..255) luminance above which snow-like texels are compressed.
    pub knee: f32,
    /// Slope above the knee (1 = no compression).
    pub slope: f32,
    /// Share of the way a fully graded texel's hue moves toward grey.
    pub neutral: f32,
    /// Saturation `(max - min) / max` at or below which a texel counts fully as snow, and at or
    /// above which not at all.
    pub sat_full: f32,
    pub sat_none: f32,
    /// A type is graded only when at least this share of its covered texels is snow-like
    /// ([`snow_share`]): snow-covered (winter) trees, not bark, grey wood or pale flowers.
    pub min_share: f32,
}

impl Default for SnowGrade {
    fn default() -> Self {
        SnowGrade { enabled: true, knee: 48.0, slope: 0.35, neutral: 0.5, sat_full: 0.12, sat_none: 0.30, min_share: 0.15 }
    }
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Applies the [`SnowGrade`] to RGBA8 albedo in place (alpha untouched). Returns the number of
/// texels it changed.
pub fn grade_snow(albedo: &mut [u8], g: &SnowGrade) -> usize {
    if !g.enabled {
        return 0;
    }
    let mut changed = 0;
    for p in albedo.chunks_exact_mut(4) {
        let (r, gr, b) = (f32::from(p[0]), f32::from(p[1]), f32::from(p[2]));
        let l = 0.299 * r + 0.587 * gr + 0.114 * b;
        if l <= g.knee {
            continue;
        }
        let mx = r.max(gr).max(b);
        let mn = r.min(gr).min(b);
        let sat = if mx > 0.0 { (mx - mn) / mx } else { 0.0 };
        // Snow is neutral to cool (blue >= red); grey wood and pale greens are warm.
        let w = (1.0 - smoothstep(g.sat_full, g.sat_none, sat)) * smoothstep(-8.0, 4.0, b - r);
        if w <= 0.0 {
            continue;
        }
        let target = g.knee + (l - g.knee) * g.slope;
        let l2 = l + (target - l) * w;
        let k = l2 / l;
        let n = g.neutral * w * ((l - g.knee) / 32.0).clamp(0.0, 1.0);
        let out = [r, gr, b].map(|c| (c * k * (1.0 - n) + l2 * n).round().clamp(0.0, 255.0) as u8);
        if out != [p[0], p[1], p[2]] {
            changed += 1;
        }
        p[..3].copy_from_slice(&out);
    }
    changed
}

#[cfg(test)]
mod grade_tests {
    use super::*;

    #[test]
    fn snow_is_compressed_and_neutralised_colour_is_kept() {
        let g = SnowGrade::default();
        // Bright cyan-white snow, dark green needles, bright autumn yellow, mid grey below the knee.
        let mut px = vec![200, 225, 230, 255, 36, 39, 17, 255, 200, 160, 40, 255, 40, 40, 40, 255];
        let n = grade_snow(&mut px, &g);
        assert_eq!(n, 1);
        let l = 0.299 * f32::from(px[0]) + 0.587 * f32::from(px[1]) + 0.114 * f32::from(px[2]);
        let want = 48.0 + (0.299 * 200.0 + 0.587 * 225.0 + 0.114 * 230.0 - 48.0) * 0.35;
        assert!((l - want).abs() < 2.0, "snow luma {l} vs {want}");
        // Less cyan: blue and green moved toward red.
        assert!(f32::from(px[2]) / f32::from(px[0]) < 230.0 / 200.0);
        assert_eq!(&px[4..16], &[36, 39, 17, 255, 200, 160, 40, 255, 40, 40, 40, 255]);
        assert_eq!(px[3], 255);
        let mut off = vec![200, 225, 230, 255];
        assert_eq!(grade_snow(&mut off, &SnowGrade { enabled: false, ..g }), 0);
        assert_eq!(off, vec![200, 225, 230, 255]);
    }
}

/// Share of the covered texels (alpha >= 128) that [`grade_snow`] treats as snow (weight >= 0.5).
pub fn snow_share(albedo: &[u8], g: &SnowGrade) -> f32 {
    let (mut covered, mut snow) = (0u64, 0u64);
    for p in albedo.chunks_exact(4) {
        if p[3] < 128 {
            continue;
        }
        covered += 1;
        let (r, gr, b) = (f32::from(p[0]), f32::from(p[1]), f32::from(p[2]));
        let l = 0.299 * r + 0.587 * gr + 0.114 * b;
        let mx = r.max(gr).max(b);
        let sat = if mx > 0.0 { (mx - r.min(gr).min(b)) / mx } else { 0.0 };
        if l > g.knee && (1.0 - smoothstep(g.sat_full, g.sat_none, sat)) * smoothstep(-8.0, 4.0, b - r) >= 0.5 {
            snow += 1;
        }
    }
    snow as f32 / covered.max(1) as f32
}

/// [`grade_snow`] of one type's albedo when it is snow-covered ([`SnowGrade::min_share`]);
/// returns the number of texels changed (0: not graded).
pub fn grade_snow_type(albedo: &mut [u8], g: &SnowGrade) -> usize {
    if !g.enabled || snow_share(albedo, g) < g.min_share {
        return 0;
    }
    grade_snow(albedo, g)
}

#[cfg(test)]
mod gate_tests {
    use super::*;

    #[test]
    fn only_snow_covered_types_are_graded() {
        let g = SnowGrade::default();
        // 40% bright cool snow texels: graded.
        let mut snowy: Vec<u8> = (0..10).flat_map(|i| if i < 4 { [205, 220, 228, 255] } else { [36, 39, 17, 255] }).collect();
        assert!(grade_snow_type(&mut snowy, &g) > 0);
        // Pale warm grey wood (dead pine) and a few cool texels (10%): left alone.
        let mut wood: Vec<u8> = (0..10).flat_map(|i| if i < 1 { [205, 220, 228, 255] } else { [120, 112, 100, 255] }).collect();
        let before = wood.clone();
        assert_eq!(grade_snow_type(&mut wood, &g), 0);
        assert_eq!(wood, before);
        assert!(snow_share(&before, &g) < 0.15);
    }
}
