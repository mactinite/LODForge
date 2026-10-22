//! Mip chains for LOD textures, filtered in float.
//!
//! Each level is computed from the previous level kept in float (no 8-bit round trip between
//! levels) with a Lanczos-3 kernel. Colour is averaged in linear light (sRGB decode, filter,
//! encode), so contrasty textures do not darken with distance as gamma-space averaging makes
//! them. Normal maps are decoded to vectors, filtered and renormalised, so distant lighting keeps
//! its relief instead of flattening. Alpha is filtered linearly.

/// What the RGB channels hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MipKind {
    /// sRGB-encoded colour (diffuse, glow).
    Color,
    /// Unit vectors stored as `v * 0.5 + 0.5` in RGB (any axis order).
    Normal,
    /// Plain values averaged as stored.
    Data,
}

/// How the filter reads past the image edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Clamp,
    Wrap,
}

/// Converts one sRGB-encoded channel (0..1) to linear light.
pub fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.040_45 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

/// Converts one linear-light channel (0..1) to sRGB encoding.
pub fn linear_to_srgb(v: f32) -> f32 {
    let v = v.clamp(0.0, 1.0);
    if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 }
}

fn decode_table() -> &'static [f32; 256] {
    static T: std::sync::OnceLock<[f32; 256]> = std::sync::OnceLock::new();
    T.get_or_init(|| std::array::from_fn(|i| srgb_to_linear(i as f32 / 255.0)))
}

/// Linear light (0..1, 16-bit steps) -> rounded 8-bit sRGB. A step of 1/65535 in linear light
/// moves sRGB by at most 0.05 of an 8-bit level (the steepest part of the curve, near black).
fn encode_table() -> &'static [u8] {
    static T: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    T.get_or_init(|| (0..=65535u32).map(|i| (linear_to_srgb(i as f32 / 65535.0) * 255.0).round() as u8).collect())
}

fn encode_srgb8(v: f32) -> u8 {
    encode_table()[(v.clamp(0.0, 1.0) * 65535.0 + 0.5) as usize]
}

/// A float image (RGBA, row-major) in the working space of its [MipKind].
#[derive(Debug, Clone)]
pub struct FloatImage {
    pub width: u32,
    pub height: u32,
    pub px: Vec<[f32; 4]>,
}

impl FloatImage {
    /// Decodes RGBA8 into the working space of `kind`.
    pub fn from_rgba8(width: u32, height: u32, rgba: &[u8], kind: MipKind) -> FloatImage {
        let lut = decode_table();
        let px = rgba
            .chunks_exact(4)
            .map(|p| {
                let a = f32::from(p[3]) / 255.0;
                match kind {
                    MipKind::Color => [lut[p[0] as usize], lut[p[1] as usize], lut[p[2] as usize], a],
                    MipKind::Normal => {
                        let f = |c: u8| f32::from(c) / 255.0 * 2.0 - 1.0;
                        [f(p[0]), f(p[1]), f(p[2]), a]
                    }
                    MipKind::Data => [f32::from(p[0]) / 255.0, f32::from(p[1]) / 255.0, f32::from(p[2]) / 255.0, a],
                }
            })
            .collect();
        FloatImage { width, height, px }
    }

    /// Encodes back to RGBA8 (rounded); normals are renormalised first.
    pub fn to_rgba8(&self, kind: MipKind) -> Vec<u8> {
        let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        let mut out = Vec::with_capacity(self.px.len() * 4);
        if kind == MipKind::Color {
            for p in &self.px {
                out.extend_from_slice(&[encode_srgb8(p[0]), encode_srgb8(p[1]), encode_srgb8(p[2]), q(p[3])]);
            }
            return out;
        }
        for p in &self.px {
            let rgb = match kind {
                MipKind::Color => [linear_to_srgb(p[0]), linear_to_srgb(p[1]), linear_to_srgb(p[2])],
                MipKind::Normal => {
                    let n = renormalize([p[0], p[1], p[2]]);
                    [n[0] * 0.5 + 0.5, n[1] * 0.5 + 0.5, n[2] * 0.5 + 0.5]
                }
                MipKind::Data => [p[0], p[1], p[2]],
            };
            out.extend_from_slice(&[q(rgb[0]), q(rgb[1]), q(rgb[2]), q(p[3])]);
        }
        out
    }

    /// Lanczos-3 resize to `width` x `height` (downscales widen the kernel by the scale).
    pub fn resize(&self, width: u32, height: u32, edge: Edge) -> FloatImage {
        if (width, height) == (self.width, self.height) {
            return self.clone();
        }
        let wx = weights(self.width, width);
        let wy = weights(self.height, height);
        let idx = |i: i32, n: u32| -> usize {
            match edge {
                Edge::Clamp => i.clamp(0, n as i32 - 1) as usize,
                Edge::Wrap => i.rem_euclid(n as i32) as usize,
            }
        };
        let (sw, sh) = (self.width as usize, self.height as usize);
        let mut tmp = vec![[0f32; 4]; width as usize * sh];
        for y in 0..sh {
            let row = &self.px[y * sw..(y + 1) * sw];
            for (x, (first, w)) in wx.iter().enumerate() {
                let mut acc = [0f32; 4];
                for (k, wk) in w.iter().enumerate() {
                    let p = row[idx(first + k as i32, self.width)];
                    for c in 0..4 {
                        acc[c] += p[c] * wk;
                    }
                }
                tmp[y * width as usize + x] = acc;
            }
        }
        let mut px = vec![[0f32; 4]; width as usize * height as usize];
        for (y, (first, w)) in wy.iter().enumerate() {
            for x in 0..width as usize {
                let mut acc = [0f32; 4];
                for (k, wk) in w.iter().enumerate() {
                    let p = tmp[idx(first + k as i32, self.height) * width as usize + x];
                    for c in 0..4 {
                        acc[c] += p[c] * wk;
                    }
                }
                px[y * width as usize + x] = acc;
            }
        }
        FloatImage { width, height, px }
    }

    /// Exact 2x downscale (both sides even), the same Lanczos-3 kernel as [FloatImage::resize]
    /// with its 12 fixed taps: the vertical pass first as sums of whole rows (vectorises), then
    /// the horizontal pass on half the rows.
    pub fn half(&self, edge: Edge) -> FloatImage {
        let (w, h) = (self.width as usize, self.height as usize);
        debug_assert!(w % 2 == 0 && h % 2 == 0);
        // Output o samples source centre 2o + 0.5; taps 2o-5 ..= 2o+6.
        let taps: [f32; 12] = {
            let mut t: [f32; 12] = std::array::from_fn(|i| lanczos3((i as f32 - 5.0 - 0.5) / 2.0));
            let s: f32 = t.iter().sum();
            t.iter_mut().for_each(|v| *v /= s);
            t
        };
        let idx = |i: isize, n: usize| -> usize {
            match edge {
                Edge::Clamp => i.clamp(0, n as isize - 1) as usize,
                Edge::Wrap => i.rem_euclid(n as isize) as usize,
            }
        };
        let flat: &[f32] = self.px.as_flattened();
        let row_len = w * 4;
        let (oh, ow) = (h / 2, w / 2);
        // Vertical: tmp row y = sum of 12 source rows.
        let mut tmp = vec![0f32; oh * row_len];
        for y in 0..oh {
            let out = &mut tmp[y * row_len..(y + 1) * row_len];
            for (t, wt) in taps.iter().enumerate() {
                let sy = idx(2 * y as isize - 5 + t as isize, h);
                let src = &flat[sy * row_len..(sy + 1) * row_len];
                for (o, s) in out.iter_mut().zip(src) {
                    *o += wt * s;
                }
            }
        }
        // Horizontal.
        let mut px = vec![[0f32; 4]; ow * oh];
        for y in 0..oh {
            let row = &tmp[y * row_len..(y + 1) * row_len];
            for x in 0..ow {
                let mut acc = [0f32; 4];
                let first = 2 * x as isize - 5;
                if first >= 0 && (first as usize) + 12 <= w {
                    let base = first as usize * 4;
                    for (t, wt) in taps.iter().enumerate() {
                        let s = &row[base + t * 4..base + t * 4 + 4];
                        for c in 0..4 {
                            acc[c] += wt * s[c];
                        }
                    }
                } else {
                    for (t, wt) in taps.iter().enumerate() {
                        let sx = idx(first + t as isize, w) * 4;
                        for c in 0..4 {
                            acc[c] += wt * row[sx + c];
                        }
                    }
                }
                px[y * ow + x] = acc;
            }
        }
        FloatImage { width: ow as u32, height: oh as u32, px }
    }

    /// Clamps colour and alpha to their ranges and renormalises normals (Lanczos overshoots).
    pub fn settle(&mut self, kind: MipKind) {
        for p in &mut self.px {
            match kind {
                MipKind::Normal => {
                    let n = renormalize([p[0], p[1], p[2]]);
                    p[0] = n[0];
                    p[1] = n[1];
                    p[2] = n[2];
                }
                _ => {
                    for c in p.iter_mut().take(3) {
                        *c = c.clamp(0.0, 1.0);
                    }
                }
            }
            p[3] = p[3].clamp(0.0, 1.0);
        }
    }
}

fn renormalize(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l < 1e-6 { [0.0, 0.0, 1.0] } else { [v[0] / l, v[1] / l, v[2] / l] }
}

fn lanczos3(x: f32) -> f32 {
    let x = x.abs();
    if x < 1e-6 {
        return 1.0;
    }
    if x >= 3.0 {
        return 0.0;
    }
    let px = std::f32::consts::PI * x;
    3.0 * px.sin() * (px / 3.0).sin() / (px * px)
}

/// Per output sample: (first source index, normalised weights).
fn weights(src: u32, dst: u32) -> Vec<(i32, Vec<f32>)> {
    let scale = src as f32 / dst as f32;
    let k = scale.max(1.0);
    let support = 3.0 * k;
    (0..dst)
        .map(|d| {
            let center = (d as f32 + 0.5) * scale - 0.5;
            let first = (center - support).floor() as i32;
            let last = (center + support).ceil() as i32;
            let mut w: Vec<f32> = (first..=last).map(|j| lanczos3((j as f32 - center) / k)).collect();
            let sum: f32 = w.iter().sum();
            if sum.abs() > 1e-6 {
                w.iter_mut().for_each(|v| *v /= sum);
            }
            (first, w)
        })
        .collect()
}

/// The mip chain of an RGBA8 image: `levels` levels at most (the top one, unchanged, included),
/// each half the size of the previous one down to 1x1, filtered as [MipKind] says.
pub fn chain(width: u32, height: u32, rgba: &[u8], levels: u32, kind: MipKind, edge: Edge) -> Vec<(u32, u32, Vec<u8>)> {
    let mut out = vec![(width, height, rgba.to_vec())];
    let mut img = FloatImage::from_rgba8(width, height, rgba, kind);
    let (mut w, mut h) = (width, height);
    while (w > 1 || h > 1) && (out.len() as u32) < levels {
        let even = w % 2 == 0 && h % 2 == 0;
        w = (w / 2).max(1);
        h = (h / 2).max(1);
        img = if even { img.half(edge) } else { img.resize(w, h, edge) };
        img.settle(kind);
        out.push((w, h, img.to_rgba8(kind)));
    }
    out
}

/// Resize of an RGBA8 image in the working space of `kind` (for tiles smaller than their source).
pub fn resize_rgba8(width: u32, height: u32, rgba: &[u8], to: (u32, u32), kind: MipKind, edge: Edge) -> Vec<u8> {
    let mut img = FloatImage::from_rgba8(width, height, rgba, kind).resize(to.0, to.1, edge);
    img.settle(kind);
    img.to_rgba8(kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checker(size: u32, a: [u8; 4], b: [u8; 4]) -> Vec<u8> {
        (0..size * size).flat_map(|i| if (i % size + i / size) % 2 == 0 { a } else { b }).collect()
    }

    #[test]
    fn colour_mips_average_in_linear_light() {
        // Black/white checker: linear-light mean 0.5 -> sRGB ~188, not the gamma-space 128.
        let top = checker(16, [0, 0, 0, 255], [255, 255, 255, 255]);
        let c = chain(16, 16, &top, 99, MipKind::Color, Edge::Wrap);
        assert_eq!(c.iter().map(|l| l.0).collect::<Vec<_>>(), [16, 8, 4, 2, 1]);
        for (_, _, px) in &c[1..] {
            for p in px.chunks_exact(4) {
                assert!((186..=190).contains(&p[0]), "{p:?}");
                assert_eq!(p[3], 255);
            }
        }
        // Data mips average as stored.
        let d = chain(16, 16, &top, 2, MipKind::Data, Edge::Wrap);
        assert!(d[1].2.chunks_exact(4).all(|p| (127..=128).contains(&p[0])));
    }

    #[test]
    fn normal_mips_stay_unit_length() {
        // Alternating normals tilted +-45 degrees about y: their mean is (0, 0, 0.707); the mip
        // must be the renormalised (0, 0, 1), not a shortened vector.
        let e = |v: f32| ((v * 0.5 + 0.5) * 255.0).round() as u8;
        let s = std::f32::consts::FRAC_1_SQRT_2;
        let top = checker(8, [e(s), e(0.0), e(s), 255], [e(-s), e(0.0), e(s), 255]);
        let c = chain(8, 8, &top, 2, MipKind::Normal, Edge::Wrap);
        for p in c[1].2.chunks_exact(4) {
            let f = |v: u8| f32::from(v) / 255.0 * 2.0 - 1.0;
            let len = (f(p[0]).powi(2) + f(p[1]).powi(2) + f(p[2]).powi(2)).sqrt();
            assert!((len - 1.0).abs() < 0.02, "{p:?} {len}");
            assert!(p[2] >= 253, "{p:?}");
        }
    }

    #[test]
    fn half_matches_generic_resize() {
        let rgba: Vec<u8> = (0..16u32 * 8 * 4).map(|i| (i * 37 % 251) as u8).collect();
        for edge in [Edge::Clamp, Edge::Wrap] {
            let img = FloatImage::from_rgba8(16, 8, &rgba, MipKind::Data);
            let (a, b) = (img.half(edge), img.resize(8, 4, edge));
            for (p, q) in a.px.iter().zip(&b.px) {
                for c in 0..4 {
                    assert!((p[c] - q[c]).abs() < 1e-4, "{edge:?} {p:?} {q:?}");
                }
            }
        }
    }

    #[test]
    fn chain_respects_level_cap_and_odd_sizes() {
        let top = vec![200u8; 12 * 6 * 4];
        let c = chain(12, 6, &top, 3, MipKind::Color, Edge::Clamp);
        assert_eq!(c.iter().map(|l| (l.0, l.1)).collect::<Vec<_>>(), [(12, 6), (6, 3), (3, 1)]);
        // A flat image stays flat.
        assert!(c[2].2.chunks_exact(4).all(|p| p == [200, 200, 200, 200]));
    }
}
