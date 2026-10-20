//! CPU BC1 (DXT1) encoder for opaque images: principal-axis endpoints, one least-squares
//! refinement, always the 4-colour mode (alpha is dropped, as DynDOLOD's BC1 normal and
//! non-alpha diffuse atlases).

fn to565(c: [f32; 3]) -> u16 {
    let r = (c[0].clamp(0.0, 255.0) * 31.0 / 255.0 + 0.5) as u16;
    let g = (c[1].clamp(0.0, 255.0) * 63.0 / 255.0 + 0.5) as u16;
    let b = (c[2].clamp(0.0, 255.0) * 31.0 / 255.0 + 0.5) as u16;
    (r << 11) | (g << 5) | b
}

fn from565(c: u16) -> [f32; 3] {
    let r = (c >> 11) & 31;
    let g = (c >> 5) & 63;
    let b = c & 31;
    [f32::from((r << 3) | (r >> 2)), f32::from((g << 2) | (g >> 4)), f32::from((b << 3) | (b >> 2))]
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
}

/// Palette indices (0 = c0, 1 = c1, 2 = 2/3 c0 + 1/3 c1, 3 = 1/3 c0 + 2/3 c1) and the error.
fn indices(px: &[[f32; 3]; 16], c0: u16, c1: u16) -> (u32, f32) {
    let (a, b) = (from565(c0), from565(c1));
    let pal = [a, b, [(2.0 * a[0] + b[0]) / 3.0, (2.0 * a[1] + b[1]) / 3.0, (2.0 * a[2] + b[2]) / 3.0], [(a[0] + 2.0 * b[0]) / 3.0, (a[1] + 2.0 * b[1]) / 3.0, (a[2] + 2.0 * b[2]) / 3.0]];
    let (mut bits, mut err) = (0u32, 0.0);
    for (i, p) in px.iter().enumerate() {
        let (mut best, mut bd) = (0u32, f32::MAX);
        for (k, q) in pal.iter().enumerate() {
            let d = dist(*p, *q);
            if d < bd {
                bd = d;
                best = k as u32;
            }
        }
        bits |= best << (2 * i);
        err += bd;
    }
    (bits, err)
}

/// Orders the endpoints for the 4-colour mode (c0 > c1) and remaps the indices to match.
fn block(c0: u16, c1: u16, px: &[[f32; 3]; 16]) -> ([u8; 8], f32) {
    let (mut c0, mut c1) = (c0, c1);
    if c0 < c1 {
        std::mem::swap(&mut c0, &mut c1);
    }
    let (bits, err) = if c0 == c1 { (0, px.iter().map(|p| dist(*p, from565(c0))).sum()) } else { indices(px, c0, c1) };
    let mut out = [0u8; 8];
    out[0..2].copy_from_slice(&c0.to_le_bytes());
    out[2..4].copy_from_slice(&c1.to_le_bytes());
    out[4..8].copy_from_slice(&bits.to_le_bytes());
    (out, err)
}

/// One 4x4 block of RGB pixels.
pub fn encode_block(px: &[[f32; 3]; 16]) -> [u8; 8] {
    let mut mean = [0.0f32; 3];
    for p in px {
        for c in 0..3 {
            mean[c] += p[c] / 16.0;
        }
    }
    let mut cov = [0.0f32; 6];
    for p in px {
        let d = [p[0] - mean[0], p[1] - mean[1], p[2] - mean[2]];
        cov[0] += d[0] * d[0];
        cov[1] += d[0] * d[1];
        cov[2] += d[0] * d[2];
        cov[3] += d[1] * d[1];
        cov[4] += d[1] * d[2];
        cov[5] += d[2] * d[2];
    }
    // Principal axis by power iteration.
    let mut axis = [1.0f32, 1.0, 1.0];
    for _ in 0..8 {
        let v = [cov[0] * axis[0] + cov[1] * axis[1] + cov[2] * axis[2], cov[1] * axis[0] + cov[3] * axis[1] + cov[4] * axis[2], cov[2] * axis[0] + cov[4] * axis[1] + cov[5] * axis[2]];
        let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        if n < 1e-6 {
            break;
        }
        axis = [v[0] / n, v[1] / n, v[2] / n];
    }
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    for p in px {
        let t = (p[0] - mean[0]) * axis[0] + (p[1] - mean[1]) * axis[1] + (p[2] - mean[2]) * axis[2];
        lo = lo.min(t);
        hi = hi.max(t);
    }
    let at = |t: f32| [mean[0] + axis[0] * t, mean[1] + axis[1] * t, mean[2] + axis[2] * t];
    let (e0, e1) = (at(hi), at(lo));
    let (mut best, best_err) = block(to565(e0), to565(e1), px);
    // Least-squares refit of the endpoints to the chosen indices.
    let c0 = u16::from_le_bytes([best[0], best[1]]);
    let c1 = u16::from_le_bytes([best[2], best[3]]);
    if c0 != c1 {
        let bits = u32::from_le_bytes([best[4], best[5], best[6], best[7]]);
        let w = [1.0f32, 0.0, 2.0 / 3.0, 1.0 / 3.0];
        let (mut aa, mut bb, mut ab) = (0.0f32, 0.0f32, 0.0f32);
        let (mut ax, mut bx) = ([0.0f32; 3], [0.0f32; 3]);
        for (i, p) in px.iter().enumerate() {
            let a = w[((bits >> (2 * i)) & 3) as usize];
            let b = 1.0 - a;
            aa += a * a;
            bb += b * b;
            ab += a * b;
            for c in 0..3 {
                ax[c] += a * p[c];
                bx[c] += b * p[c];
            }
        }
        let det = aa * bb - ab * ab;
        if det.abs() > 1e-6 {
            let f = 1.0 / det;
            let n0 = [0, 1, 2].map(|c| (ax[c] * bb - bx[c] * ab) * f);
            let n1 = [0, 1, 2].map(|c| (bx[c] * aa - ax[c] * ab) * f);
            let (b2, e2) = block(to565(n0), to565(n1), px);
            if e2 < best_err {
                best = b2;
            }
        }
    }
    best
}

/// BC1 blocks of rows `y0..y1` of 4x4 blocks of an RGBA8 image (edges clamp); call per block
/// row band to parallelise.
pub fn encode_bc1_rows(width: u32, height: u32, rgba: &[u8], y0: usize, y1: usize, out: &mut [u8]) {
    let bx = width.div_ceil(4) as usize;
    let (w, h) = (width as usize, height as usize);
    for y in y0..y1 {
        for x in 0..bx {
            let mut px = [[0.0f32; 3]; 16];
            for j in 0..4 {
                for i in 0..4 {
                    let sx = (x * 4 + i).min(w - 1);
                    let sy = (y * 4 + j).min(h - 1);
                    let o = (sy * w + sx) * 4;
                    px[j * 4 + i] = [f32::from(rgba[o]), f32::from(rgba[o + 1]), f32::from(rgba[o + 2])];
                }
            }
            let d = ((y - y0) * bx + x) * 8;
            out[d..d + 8].copy_from_slice(&encode_block(&px));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A colour ramp (colours on one line, as BC1 stores them) through the encoder and back through the decoder stays close.
    #[test]
    fn roundtrip_gradient() {
        let (w, h) = (16u32, 8u32);
        let mut rgba = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let t = (x + y) * 10;
                rgba.extend_from_slice(&[t as u8, (t / 2) as u8, (255 - t) as u8, 255]);
            }
        }
        let mut blocks = vec![0u8; (w / 4 * h / 4 * 8) as usize];
        encode_bc1_rows(w, h, &rgba, 0, (h / 4) as usize, &mut blocks);
        let dds = crate::write_dxt1(w, h, &[blocks]);
        let info = crate::parse(&dds).unwrap();
        assert_eq!((info.width, info.height, info.mip_count, info.format), (w, h, 1, crate::Format::Bc1));
        let img = crate::decode(&dds, &info, 0).unwrap();
        let err = img.rgba.iter().zip(&rgba).map(|(a, b)| (i32::from(*a) - i32::from(*b)).abs()).max().unwrap();
        assert!(err <= 12, "max error {err}");
    }
}
