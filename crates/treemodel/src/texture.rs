//! Derived from TreeLODGen, Copyright (C) 2026 Clemus (GPL-3.0, commit dc4cc8a); ported to Rust and
//! modified for LODForge 2026-10-23. See THIRD-PARTY-NOTICES.txt part 1.7.
//!
//! Alpha channel of a diffuse texture for the bark/foliage test (TreeLODGen `Textures.cs`;
//! spec 3.4): top mip, point-downsampled by a power of two until the larger side is at most 2048
//! (the top-left texel of each block), nearest sampling with wrapping, no V flip.

#[derive(Debug, Clone)]
pub struct AlphaImage {
    pub width: u32,
    pub height: u32,
    /// Stored (possibly downsampled) size and alpha values, row 0 = v 0.
    stored_w: u32,
    stored_h: u32,
    alpha: Vec<u8>,
}

/// Largest stored side.
pub const MAX_STORED_SIDE: u32 = 2048;

impl AlphaImage {
    /// From RGBA8 pixels of the top mip.
    pub fn from_rgba(width: u32, height: u32, rgba: &[u8]) -> AlphaImage {
        let mut step = 1u32;
        while width.max(height) / step > MAX_STORED_SIDE {
            step *= 2;
        }
        let (sw, sh) = ((width / step).max(1), (height / step).max(1));
        let mut alpha = Vec::with_capacity((sw * sh) as usize);
        for y in 0..sh {
            for x in 0..sw {
                let i = (((y * step) * width + x * step) * 4 + 3) as usize;
                alpha.push(rgba.get(i).copied().unwrap_or(255));
            }
        }
        AlphaImage { width, height, stored_w: sw, stored_h: sh, alpha }
    }

    /// Alpha at (u, v), wrapped.
    pub fn alpha(&self, u: f32, v: f32) -> u8 {
        let (u, v) = (u - u.floor(), v - v.floor());
        let x = ((u * self.stored_w as f32) as u32).min(self.stored_w - 1);
        let y = ((v * self.stored_h as f32) as u32).min(self.stored_h - 1);
        self.alpha[(y * self.stored_w + x) as usize]
    }
}
