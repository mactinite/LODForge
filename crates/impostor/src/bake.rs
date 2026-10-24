// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! Bakes one tree model into hemi-octahedral impostor frames: N x N orthographic views over the
//! upper hemisphere, rendered by `lodforge-texgen` into two atlases (docs/runtime/impostors.md):
//!
//! - **albedo + coverage**: unlit colour (texture times vertex colour, sRGB-encoded as the source
//!   textures are), alpha = coverage of the supersampled render;
//! - **normal + depth**: model-space shading normal * 0.5 + 0.5 (linear), alpha = depth across the
//!   capture slab (0 at the camera side of the bounding sphere, 1 at the far side).
//!
//! Capture conventions follow Project Terra's `TreeImpostorCapture` (bounding sphere about the
//! bounds centre, square ortho frame of the sphere's diameter, slab depth across the sphere),
//! in Skyrim's Z-up model space. Frames are dilated cell by cell (Terra dilates the whole atlas,
//! which lets a frame flood into its neighbours' empty texels).

use std::collections::HashMap;

use glam::Vec3;
use lodforge_texgen::{Basis, Job, Light, Pass, Renderer, Scene, TexData, View};

use crate::{dilation, hemioct};

/// Bumped whenever the baked pixels or the manifest's meaning change (part of the cache key).
pub const BAKER_VERSION: u32 = 1;

/// Unlit albedo: the texture colour times the vertex colour, no light terms.
pub const ALBEDO_LIGHT: Light = Light { direct: 0.0, ambient: 1.0, dir: [0.0, 0.0, 1.0], pbr_scale: 1.0, normal_pull: 0.0 };

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Settings {
    /// Frames per side; 0 picks 16 for frames up to 64 px and 12 for larger frames.
    pub grid: u32,
    /// Frame size in pixels; 0 sizes it by the tree's height ([Settings::units_per_px]).
    pub frame_px: u32,
    /// Auto frame size: `next_pow2(height / units_per_px)`, clamped to `min_frame..=max_frame`.
    pub units_per_px: f32,
    pub min_frame: u32,
    pub max_frame: u32,
    /// Dilation rings per frame.
    pub dilation: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { grid: 0, frame_px: 0, units_per_px: 12.0, min_frame: 32, max_frame: 128, dilation: 16 }
    }
}

impl Settings {
    /// (grid, frame px) for a tree `height` units tall.
    pub fn layout(&self, height: f32) -> (u32, u32) {
        let frame = if self.frame_px > 0 {
            self.frame_px
        } else {
            let px = (height / self.units_per_px.max(1e-3)).round().max(1.0) as u32;
            px.next_power_of_two().clamp(self.min_frame.max(4), self.max_frame.max(self.min_frame.max(4)))
        };
        // Frames stay a multiple of 4 (BC7 blocks).
        let frame = frame.div_ceil(4) * 4;
        let grid = if self.grid > 0 { self.grid.clamp(2, 64) } else if frame <= 64 { 16 } else { 12 };
        (grid, frame)
    }
}

/// Bounding sphere about the centre of the model's axis-aligned bounds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fit {
    pub min: Vec3,
    pub max: Vec3,
    pub center: Vec3,
    /// Largest distance of a vertex from `center`.
    pub radius: f32,
}

/// The [`Fit`] of the scene's vertices (`None` without geometry).
pub fn fit(scene: &Scene) -> Option<Fit> {
    let (lo, hi) = scene.bounds()?;
    let (min, max) = (Vec3::from(lo), Vec3::from(hi));
    let center = (min + max) * 0.5;
    let radius = scene.meshes.iter().flat_map(|m| m.positions.iter()).map(|p| (Vec3::from(*p) - center).length()).fold(0.0f32, f32::max);
    (radius > 1e-3).then_some(Fit { min, max, center, radius })
}

/// One baked impostor type (atlases not yet mipmapped or compressed).
#[derive(Debug, Clone, PartialEq)]
pub struct Baked {
    pub grid: u32,
    pub frame_px: u32,
    pub fit: Fit,
    /// RGBA8, `size` x `size` (`size = grid * frame_px`), rows top first.
    pub albedo: Vec<u8>,
    pub normal_depth: Vec<u8>,
}

impl Baked {
    pub fn size(&self) -> u32 {
        self.grid * self.frame_px
    }

    /// Quad edge length in model units (the bounding sphere's diameter).
    pub fn quad_size(&self) -> f32 {
        2.0 * self.fit.radius
    }

    /// Mean coverage over the atlas (0..1).
    pub fn coverage(&self) -> f32 {
        let n = (self.albedo.len() / 4).max(1);
        self.albedo.chunks_exact(4).map(|p| f32::from(p[3])).sum::<f32>() / (255.0 * n as f32)
    }
}

/// The capture view of frame (i, j): `frame_px` square, centred on the bounding sphere.
pub fn frame_view(fit: &Fit, i: u32, j: u32, grid: u32, frame_px: u32) -> View {
    let dir = hemioct::frame_to_direction(i, j, grid);
    let (right, up) = hemioct::frame_basis(dir);
    let r = fit.radius;
    let (cx, cy) = (fit.center.dot(right), fit.center.dot(up));
    let basis = Basis { right: right.into(), up: up.into(), toward: dir.into(), slab: (fit.center.dot(dir) + r, 2.0 * r) };
    View { yaw_deg: 0.0, x0: cx - r, x1: cx + r, z0: cy - r, z1: cy + r, width: frame_px, height: frame_px, basis: Some(basis) }
}

/// Subsampled pixels per GPU round trip (memory bound of one `render_many` batch).
const BATCH_PIXELS: u64 = 48 << 20;

/// Bakes `scene` (`None` without geometry). Deterministic for the same inputs and GPU.
pub fn bake(gpu: &lodforge_gpu::Gpu, renderer: &Renderer, scene: &Scene, textures: &HashMap<String, TexData>, s: &Settings) -> Option<Baked> {
    let fit = fit(scene)?;
    let (grid, frame) = s.layout(fit.max.z - fit.min.z);
    let size = (grid * frame) as usize;
    let mut albedo = vec![0u8; size * size * 4];
    let mut nd: Vec<u8> = std::iter::repeat_n([128u8, 128, 255, 255], size * size).flatten().collect();

    let frames: Vec<(u32, u32)> = (0..grid).flat_map(|j| (0..grid).map(move |i| (i, j))).collect();
    let ss = u64::from(lodforge_texgen::render::supersample_for(frame, frame));
    let per_frame = u64::from(frame * frame) * ss * ss * 2;
    let chunk = (BATCH_PIXELS / per_frame.max(1)).clamp(1, frames.len() as u64) as usize;
    let mut any = false;
    for part in frames.chunks(chunk) {
        let mut jobs = Vec::with_capacity(part.len() * 2);
        for &(i, j) in part {
            let view = frame_view(&fit, i, j, grid, frame);
            jobs.push(Job { scene, textures, view, light: ALBEDO_LIGHT, pass: Pass::Diffuse });
            jobs.push(Job { scene, textures, view, light: ALBEDO_LIGHT, pass: Pass::ObjectNormalDepth });
        }
        let imgs = renderer.render_many(gpu, &jobs);
        for (k, &(i, j)) in part.iter().enumerate() {
            let mut a = imgs[2 * k].clone();
            let n = &imgs[2 * k + 1];
            if a.len() != (frame * frame * 4) as usize || n.len() != a.len() {
                continue;
            }
            // Un-premultiply the averaged colour and bleed it into transparent texels (the
            // billboards' soft alpha finish).
            lodforge_texgen::finish_alpha_soft(&mut a, frame);
            let f = frame as usize;
            let (x0, y0) = (i as usize * f, j as usize * f);
            for y in 0..f {
                let d = ((y0 + y) * size + x0) * 4;
                albedo[d..d + f * 4].copy_from_slice(&a[y * f * 4..(y + 1) * f * 4]);
                nd[d..d + f * 4].copy_from_slice(&n[y * f * 4..(y + 1) * f * 4]);
            }
            any |= a.chunks_exact(4).any(|p| p[3] > 0);
        }
    }
    if !any {
        return None;
    }
    let f = frame as usize;
    // Cells are independent: dilate them in parallel on row bands of the atlas.
    {
        use rayon::prelude::*;
        let band = f * size * 4;
        albedo.par_chunks_mut(band).zip(nd.par_chunks_mut(band)).for_each(|(a, n)| {
            for i in 0..grid as usize {
                dilation::dilate_cell(a, n, size, i * f, 0, f, s.dilation);
            }
        });
    }
    Some(Baked { grid, frame_px: frame, fit, albedo, normal_depth: nd })
}

/// FNV-1a over 32-bit words, as Terra's `ComputePrototypeKey`.
#[derive(Debug, Clone, Copy)]
pub struct KeyHasher(u64);

impl Default for KeyHasher {
    fn default() -> Self {
        KeyHasher(0xcbf2_9ce4_8422_2325)
    }
}

impl KeyHasher {
    pub fn mix(&mut self, v: u32) -> &mut Self {
        for b in v.to_le_bytes() {
            self.0 = (self.0 ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
        }
        self
    }

    pub fn mix_str(&mut self, s: &str) -> &mut Self {
        self.mix(s.len() as u32);
        for b in s.bytes() {
            self.0 = (self.0 ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3);
        }
        self
    }

    pub fn finish(&self) -> u64 {
        self.0
    }
}

/// Cache key of one bake: the model's CRC-32, its textures' (path, CRC-32) in order, the layout
/// and dilation, the supersampling, and [`BAKER_VERSION`].
pub fn cache_key(model_crc: u32, textures: &[(String, u32)], grid: u32, frame_px: u32, s: &Settings) -> u64 {
    let mut h = KeyHasher::default();
    h.mix(model_crc);
    for (p, c) in textures {
        h.mix_str(p).mix(*c);
    }
    h.mix(grid).mix(frame_px).mix(s.dilation).mix(lodforge_texgen::render::supersample()).mix(BAKER_VERSION);
    h.finish()
}

const CACHE_MAGIC: &[u8; 4] = b"LFIC";

/// Serialises a bake for the cache (LZ4-framed pixels).
pub fn encode_cache(b: &Baked) -> Vec<u8> {
    let mut o = Vec::new();
    o.extend_from_slice(CACHE_MAGIC);
    for v in [BAKER_VERSION, b.grid, b.frame_px] {
        o.extend_from_slice(&v.to_le_bytes());
    }
    for v in [b.fit.min, b.fit.max, b.fit.center] {
        for c in v.to_array() {
            o.extend_from_slice(&c.to_le_bytes());
        }
    }
    o.extend_from_slice(&b.fit.radius.to_le_bytes());
    let mut px = b.albedo.clone();
    px.extend_from_slice(&b.normal_depth);
    o.extend_from_slice(&lz4_flex::compress_prepend_size(&px));
    o
}

/// The inverse of [`encode_cache`] (`None` for another version or damaged data).
pub fn decode_cache(d: &[u8]) -> Option<Baked> {
    if d.get(0..4)? != CACHE_MAGIC {
        return None;
    }
    let u = |o: usize| Some(u32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?));
    let f = |o: usize| Some(f32::from_le_bytes(d.get(o..o + 4)?.try_into().ok()?));
    if u(4)? != BAKER_VERSION {
        return None;
    }
    let (grid, frame_px) = (u(8)?, u(12)?);
    let v = |o: usize| Some(Vec3::new(f(o)?, f(o + 4)?, f(o + 8)?));
    let fit = Fit { min: v(16)?, max: v(28)?, center: v(40)?, radius: f(52)? };
    let px = lz4_flex::decompress_size_prepended(d.get(56..)?).ok()?;
    let n = (grid * frame_px) as usize;
    if px.len() != n * n * 8 {
        return None;
    }
    let (a, nd) = px.split_at(n * n * 4);
    Some(Baked { grid, frame_px, fit, albedo: a.to_vec(), normal_depth: nd.to_vec() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lodforge_texgen::scene::Mesh;

    /// A cross of two vertical alpha-tested cards plus a horizontal one, textured with a 4x4
    /// image whose left half is transparent.
    fn test_scene() -> (Scene, HashMap<String, TexData>) {
        let mut m = Mesh { diffuse: "textures\\test.dds".into(), alpha_test: Some(128), double_sided: true, ..Default::default() };
        let quads: [[[f32; 3]; 4]; 3] = [
            [[-50.0, 0.0, 0.0], [50.0, 0.0, 0.0], [50.0, 0.0, 200.0], [-50.0, 0.0, 200.0]],
            [[0.0, -50.0, 0.0], [0.0, 50.0, 0.0], [0.0, 50.0, 200.0], [0.0, -50.0, 200.0]],
            [[-60.0, -60.0, 120.0], [60.0, -60.0, 120.0], [60.0, 60.0, 120.0], [-60.0, 60.0, 120.0]],
        ];
        for (k, q) in quads.iter().enumerate() {
            let n = if k == 2 { [0.0, 0.0, 1.0] } else if k == 0 { [0.0, 1.0, 0.0] } else { [1.0, 0.0, 0.0] };
            let base = m.positions.len() as u32;
            for (v, uv) in q.iter().zip([[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]]) {
                m.positions.push(*v);
                m.uvs.push(uv);
                m.normals.push(n);
                m.colors.push([1.0; 4]);
            }
            m.indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        let mut px = Vec::new();
        for _y in 0..4 {
            for x in 0..4 {
                px.extend_from_slice(if x < 1 { &[0, 0, 0, 0] } else { &[40, 160, 60, 255] });
            }
        }
        let tex = TexData { width: 4, height: 4, mips: vec![px], bc: None };
        let mut textures = HashMap::new();
        textures.insert(m.diffuse.clone(), tex);
        (Scene { meshes: vec![m], opts: Default::default() }, textures)
    }

    #[test]
    fn layout_scales_with_height_and_picks_the_grid() {
        let s = Settings::default();
        assert_eq!(s.layout(1500.0), (12, 128));
        assert_eq!(s.layout(700.0), (16, 64));
        assert_eq!(s.layout(100.0), (16, 32));
        assert_eq!(s.layout(90000.0), (12, 128));
        let fixed = Settings { grid: 14, frame_px: 50, ..s };
        assert_eq!(fixed.layout(1500.0), (14, 52));
    }

    #[test]
    fn frame_views_frame_the_bounding_sphere_with_a_right_handed_basis() {
        let fit = Fit { min: Vec3::new(-10.0, -10.0, 0.0), max: Vec3::new(10.0, 10.0, 100.0), center: Vec3::new(0.0, 0.0, 50.0), radius: 52.0 };
        for j in 0..12 {
            for i in 0..12 {
                let v = frame_view(&fit, i, j, 12, 64);
                let b = v.basis.unwrap();
                let (r, u, t) = (Vec3::from(b.right), Vec3::from(b.up), Vec3::from(b.toward));
                assert!((r.cross(u) - t).length() < 1e-4);
                assert!((v.x1 - v.x0 - 104.0).abs() < 1e-3 && (v.z1 - v.z0 - 104.0).abs() < 1e-3);
                // The slab spans the sphere: the centre is at depth 0.5.
                assert!(((b.slab.0 - fit.center.dot(t)) / b.slab.1 - 0.5).abs() < 1e-5);
            }
        }
    }

    #[test]
    fn cache_round_trips() {
        let b = Baked {
            grid: 2,
            frame_px: 4,
            fit: Fit { min: Vec3::ZERO, max: Vec3::ONE, center: Vec3::splat(0.5), radius: 0.9 },
            albedo: (0..256).map(|v| v as u8).collect(),
            normal_depth: (0..256).map(|v| (255 - v) as u8).collect(),
        };
        assert_eq!(decode_cache(&encode_cache(&b)), Some(b.clone()));
        assert!(decode_cache(b"LFIC").is_none());
        let s = Settings::default();
        let k = cache_key(1, &[("a".into(), 2)], 12, 64, &s);
        assert_eq!(k, cache_key(1, &[("a".into(), 2)], 12, 64, &s));
        assert_ne!(k, cache_key(1, &[("a".into(), 3)], 12, 64, &s));
        assert_ne!(k, cache_key(1, &[("a".into(), 2)], 16, 64, &s));
    }

    #[test]
    fn bake_is_deterministic_and_fills_the_frames() {
        let Ok(gpu) = lodforge_gpu::Gpu::new() else {
            eprintln!("no GPU adapter; skipped");
            return;
        };
        let renderer = Renderer::new(&gpu);
        let (scene, tex) = test_scene();
        let s = Settings { frame_px: 32, grid: 8, ..Default::default() };
        let a = bake(&gpu, &renderer, &scene, &tex, &s).expect("baked");
        let b = bake(&gpu, &renderer, &scene, &tex, &s).expect("baked");
        assert_eq!(a, b, "same inputs, same pixels");
        assert_eq!((a.grid, a.frame_px, a.size()), (8, 32, 256));
        assert!((a.quad_size() - 2.0 * a.fit.radius).abs() < 1e-6);
        // Every frame sees some of the cross (the cards face every direction).
        let f = 32usize;
        for j in 0..8 {
            for i in 0..8 {
                let covered = (0..f).flat_map(|y| (0..f).map(move |x| ((j * f + y) * 256 + i * f + x) * 4 + 3)).filter(|&k| a.albedo[k] > 0).count();
                assert!(covered > 10, "frame ({i},{j}) has {covered} covered texels");
            }
        }
        // Covered texels: the card colour and a unit normal; the top-down frame of an even grid
        // does not exist, but frames near the zenith see the horizontal card's +Z normal.
        let c = a.albedo.chunks_exact(4).filter(|p| p[3] == 255).next().unwrap();
        assert!(c[1] > c[0] && c[1] > c[2], "green card: {c:?}");
        for (p, n) in a.albedo.chunks_exact(4).zip(a.normal_depth.chunks_exact(4)) {
            if p[3] == 255 {
                let v = [n[0], n[1], n[2]].map(|c| f32::from(c) / 127.5 - 1.0);
                let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
                assert!((l - 1.0).abs() < 0.05, "unit normal: {n:?}");
            }
        }
    }
}
