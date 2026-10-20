//! Float RGBA images, PNG output and comparison metrics.

use std::path::Path;

/// Linear-in-file (no colour-space conversion) RGBA image with channels in 0..=1.
#[derive(Debug, Clone)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub px: Vec<[f32; 4]>,
}

impl Image {
    pub fn new(width: u32, height: u32) -> Image {
        Image { width, height, px: vec![[0.0, 0.0, 0.0, 1.0]; (width * height) as usize] }
    }

    pub fn from_rgba8(width: u32, height: u32, rgba: &[u8]) -> Image {
        let px = rgba.chunks_exact(4).map(|c| [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, c[3] as f32 / 255.0]).collect();
        Image { width, height, px }
    }

    pub fn to_rgba8(&self) -> Vec<u8> {
        self.px.iter().flat_map(|p| p.map(|c| (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)).collect()
    }

    pub fn get(&self, x: u32, y: u32) -> [f32; 4] {
        self.px[(y * self.width + x) as usize]
    }

    /// Box-filter downsample by an integer factor.
    pub fn downsample(&self, factor: u32) -> Image {
        let (w, h) = ((self.width / factor).max(1), (self.height / factor).max(1));
        let mut out = Image::new(w, h);
        let n = (factor * factor) as f32;
        for y in 0..h {
            for x in 0..w {
                let mut acc = [0.0f32; 4];
                for dy in 0..factor {
                    for dx in 0..factor {
                        let p = self.get((x * factor + dx).min(self.width - 1), (y * factor + dy).min(self.height - 1));
                        for c in 0..4 {
                            acc[c] += p[c];
                        }
                    }
                }
                out.px[(y * w + x) as usize] = acc.map(|a| a / n);
            }
        }
        out
    }

    pub fn write_png(&self, path: &Path) -> std::io::Result<()> {
        let file = std::io::BufWriter::new(std::fs::File::create(path)?);
        let mut enc = png::Encoder::new(file, self.width, self.height);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().map_err(std::io::Error::other)?;
        w.write_image_data(&self.to_rgba8()).map_err(std::io::Error::other)?;
        Ok(())
    }

    /// Mean RGB.
    pub fn mean_rgb(&self) -> [f32; 3] {
        let mut acc = [0.0f64; 3];
        for p in &self.px {
            for c in 0..3 {
                acc[c] += p[c] as f64;
            }
        }
        acc.map(|a| (a / self.px.len() as f64) as f32)
    }
}

fn lanczos3(x: f32) -> f32 {
    let x = x.abs();
    if x < 1e-6 {
        1.0
    } else if x < 3.0 {
        let px = std::f32::consts::PI * x;
        3.0 * px.sin() * (px / 3.0).sin() / (px * px)
    } else {
        0.0
    }
}

/// Per-output-sample (first source index, weights) for a 1-D Lanczos-3 resample with
/// clamped edges; the kernel widens by the scale factor when downscaling.
fn lanczos_weights(src: u32, dst: u32) -> Vec<(i32, Vec<f32>)> {
    let scale = src as f32 / dst as f32;
    let support = 3.0 * scale.max(1.0);
    let k = scale.max(1.0);
    (0..dst)
        .map(|i| {
            let center = (i as f32 + 0.5) * scale - 0.5;
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

impl Image {
    /// Separable Lanczos-3 resize with clamped edges (no clamping of the result range).
    pub fn lanczos(&self, width: u32, height: u32) -> Image {
        if (width, height) == (self.width, self.height) {
            return self.clone();
        }
        let wx = lanczos_weights(self.width, width);
        let wy = lanczos_weights(self.height, height);
        let clamp = |v: i32, n: u32| v.clamp(0, n as i32 - 1) as u32;
        // Horizontal pass.
        let mut tmp = Image::new(width, self.height);
        for y in 0..self.height {
            for (x, (first, w)) in wx.iter().enumerate() {
                let mut acc = [0.0f32; 4];
                for (k, wk) in w.iter().enumerate() {
                    let p = self.get(clamp(first + k as i32, self.width), y);
                    for c in 0..4 {
                        acc[c] += p[c] * wk;
                    }
                }
                tmp.px[(y * width) as usize + x] = acc;
            }
        }
        // Vertical pass.
        let mut out = Image::new(width, height);
        for (y, (first, w)) in wy.iter().enumerate() {
            for x in 0..width {
                let mut acc = [0.0f32; 4];
                for (k, wk) in w.iter().enumerate() {
                    let p = tmp.get(x, clamp(first + k as i32, self.height));
                    for c in 0..4 {
                        acc[c] += p[c] * wk;
                    }
                }
                out.px[y * width as usize + x as usize] = acc;
            }
        }
        out
    }

    /// Clamps to 0..=1 and truncates every channel to an 8-bit value, as Vampyre's float pixel
    /// setters do when xLODGen stores an intermediate image (truncation, not rounding, matched
    /// golden: normals 36 -> 41 dB low-pass, diffuse 41.7 -> 43.4 dB).
    pub fn quantize8(&mut self) {
        self.quantize8_with(false);
    }

    /// TEMPORARY: quantise with rounding (`true`) or truncation.
    pub fn quantize8_with(&mut self, round: bool) {
        for p in &mut self.px {
            for c in p.iter_mut() {
                let v = c.clamp(0.0, 1.0) * 255.0;
                *c = if round { v.round() } else { v.trunc() } / 255.0;
            }
        }
    }

    /// Copy of the `w` x `h` region starting at (`x`, `y`).
    pub fn crop(&self, x: u32, y: u32, w: u32, h: u32) -> Image {
        let mut out = Image::new(w, h);
        for j in 0..h {
            for i in 0..w {
                out.px[(j * w + i) as usize] = self.get((x + i).min(self.width - 1), (y + j).min(self.height - 1));
            }
        }
        out
    }
}

/// Side-by-side image: `a` left, `b` right (same size).
pub fn side_by_side(a: &Image, b: &Image) -> Image {
    let mut out = Image::new(a.width + b.width, a.height.max(b.height));
    for y in 0..a.height {
        for x in 0..a.width {
            out.px[(y * out.width + x) as usize] = a.get(x, y);
        }
    }
    for y in 0..b.height {
        for x in 0..b.width {
            out.px[(y * out.width + a.width + x) as usize] = b.get(x, y);
        }
    }
    out
}

#[derive(Debug, Clone, Copy)]
pub struct Metrics {
    /// PSNR over RGB in dB (8-bit scale).
    pub psnr: f32,
    /// Mean absolute RGB error, 0..255 scale.
    pub mae: f32,
    /// Mean RGB of each image, 0..255 scale.
    pub mean_a: [f32; 3],
    pub mean_b: [f32; 3],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lanczos_preserves_flat_and_shrinks_size() {
        let mut img = Image::new(64, 48);
        img.px.iter_mut().for_each(|p| *p = [0.25, 0.5, 0.75, 1.0]);
        for (w, h) in [(16, 12), (100, 75), (64, 48)] {
            let r = img.lanczos(w, h);
            assert_eq!((r.width, r.height), (w, h));
            for p in &r.px {
                for (a, b) in p.iter().zip([0.25, 0.5, 0.75, 1.0]) {
                    assert!((a - b).abs() < 1e-4, "{a} vs {b}");
                }
            }
        }
    }
}

/// Compares RGB of two same-sized images.
pub fn compare(a: &Image, b: &Image) -> Metrics {
    assert_eq!((a.width, a.height), (b.width, b.height), "compare needs equal sizes");
    let (mut se, mut ae) = (0.0f64, 0.0f64);
    for (p, q) in a.px.iter().zip(&b.px) {
        for c in 0..3 {
            let d = (p[c] - q[c]) as f64 * 255.0;
            se += d * d;
            ae += d.abs();
        }
    }
    let n = (a.px.len() * 3) as f64;
    let mse = se / n;
    let psnr = if mse == 0.0 { 99.0 } else { (10.0 * (255.0f64 * 255.0 / mse).log10()) as f32 };
    Metrics { psnr, mae: (ae / n) as f32, mean_a: a.mean_rgb().map(|v| v * 255.0), mean_b: b.mean_rgb().map(|v| v * 255.0) }
}
