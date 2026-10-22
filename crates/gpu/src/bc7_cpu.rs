//! Multi-mode BC7 on the CPU (Intel's ISPC texture compressor via `intel_tex_2`). It searches
//! BC7's partitioned modes, which keep smooth gradients (terrain normal maps) and fine detail
//! (foliage billboards) that the single-subset mode 6 of the GPU encoder flattens into visible
//! 4x4 blocks. Images are split into bands of block rows encoded in parallel.

use intel_tex_2::{RgbaSurface, bc7};
use rayon::prelude::*;

use crate::RgbaImage;

/// BC7 encoder choice for [crate::Gpu::encode_bc7].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Bc7Quality {
    /// GPU, mode 6 only: fastest, blocky on gradients and fine detail.
    Fast,
    /// CPU, every mode with a quick search (ISPC "very fast" profile: on terrain normal maps as
    /// good as its "fast" and "basic" profiles; "ultra fast" falls back to mode-6 quality).
    /// Terrain diffuse keeps the GPU encoder in this mode (no measurable block artefacts there).
    #[default]
    High,
    /// CPU, every mode with the thorough search (ISPC "basic" profile), everything on the CPU.
    Best,
}

impl Bc7Quality {
    pub fn from_u8(v: u8) -> Bc7Quality {
        match v {
            0 => Bc7Quality::Fast,
            2 => Bc7Quality::Best,
            _ => Bc7Quality::High,
        }
    }

    pub fn as_u8(self) -> u8 {
        match self {
            Bc7Quality::Fast => 0,
            Bc7Quality::High => 1,
            Bc7Quality::Best => 2,
        }
    }
}

/// Process-wide default for new [crate::Gpu] contexts (a [Bc7Quality] as u8).
static DEFAULT: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(1);

/// Sets the encoder every [crate::Gpu] created afterwards starts with.
pub fn set_default_quality(q: Bc7Quality) {
    DEFAULT.store(q.as_u8(), std::sync::atomic::Ordering::Relaxed);
}

pub fn default_quality() -> Bc7Quality {
    Bc7Quality::from_u8(DEFAULT.load(std::sync::atomic::Ordering::Relaxed))
}

/// Block rows per parallel band.
const BAND_BLOCK_ROWS: usize = 8;

/// BC7 blocks (16 bytes per 4x4 block, row-major) of one RGBA8 image.
pub fn encode(img: &RgbaImage<'_>, quality: Bc7Quality) -> Vec<u8> {
    let (w, h) = (img.width.max(1), img.height.max(1));
    let (bx, by) = (w.div_ceil(4) as usize, h.div_ceil(4) as usize);
    let (pw, ph) = (bx * 4, by * 4);
    // Pad to whole blocks by repeating the last column and row.
    let padded: std::borrow::Cow<[u8]> = if (pw, ph) == (w as usize, h as usize) {
        std::borrow::Cow::Borrowed(img.rgba)
    } else {
        let mut p = vec![0u8; pw * ph * 4];
        for y in 0..ph {
            let sy = y.min(h as usize - 1);
            for x in 0..pw {
                let sx = x.min(w as usize - 1);
                let s = (sy * w as usize + sx) * 4;
                p[(y * pw + x) * 4..(y * pw + x) * 4 + 4].copy_from_slice(&img.rgba[s..s + 4]);
            }
        }
        std::borrow::Cow::Owned(p)
    };
    let opaque = padded.chunks_exact(4).all(|p| p[3] == 255);
    let settings = match (quality, opaque) {
        (Bc7Quality::Best, true) => bc7::opaque_basic_settings(),
        (Bc7Quality::Best, false) => bc7::alpha_basic_settings(),
        (_, true) => bc7::opaque_very_fast_settings(),
        (_, false) => bc7::alpha_very_fast_settings(),
    };
    let mut out = vec![0u8; bx * by * 16];
    let row_bytes = pw * 4;
    out.par_chunks_mut(bx * 16 * BAND_BLOCK_ROWS).enumerate().for_each(|(band, blocks)| {
        let rows = blocks.len() / (bx * 16);
        let y0 = band * BAND_BLOCK_ROWS * 4;
        let surface = RgbaSurface { data: &padded[y0 * row_bytes..(y0 + rows * 4) * row_bytes], width: pw as u32, height: (rows * 4) as u32, stride: row_bytes as u32 };
        bc7::compress_blocks_into(&settings, &surface, blocks);
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(w: u32, h: u32, blocks: &[u8]) -> Vec<u8> {
        let dds = lodforge_dds::write_bc7(w, h, &[blocks.to_vec()]);
        let info = lodforge_dds::parse(&dds).unwrap();
        lodforge_dds::decode(&dds, &info, 0).unwrap().rgba
    }

    #[test]
    fn encodes_odd_sizes_and_gradients_closely() {
        // A smooth two-channel gradient (normal-map like), 13x9: padded, every block decoded.
        let (w, h) = (13u32, 9u32);
        let rgba: Vec<u8> = (0..w * h).flat_map(|i| [(i % w * 19) as u8, (i / w * 28) as u8, 200, 255]).collect();
        for q in [Bc7Quality::High, Bc7Quality::Best] {
            let blocks = encode(&RgbaImage { width: w, height: h, rgba: &rgba }, q);
            assert_eq!(blocks.len(), 4 * 3 * 16);
            let dec = decode(16, 12, &blocks);
            let (mut worst, mut sum) = (0u8, 0u32);
            for y in 0..h as usize {
                for x in 0..w as usize {
                    for c in 0..4 {
                        let d = dec[(y * 16 + x) * 4 + c].abs_diff(rgba[(y * w as usize + x) * 4 + c]);
                        worst = worst.max(d);
                        sum += u32::from(d);
                    }
                }
            }
            let mean = sum as f32 / (w * h * 4) as f32;
            assert!(worst <= 16 && mean <= 2.0, "{q:?}: worst {worst}, mean {mean}");
        }
    }
}
