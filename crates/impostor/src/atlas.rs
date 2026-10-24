// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! Atlas pages: baked impostor types packed into shared pages (one albedo and one normal-depth
//! DDS per page), mipmapped and BC7 compressed.
//!
//! Types are square (`grid * frame_px`, a multiple of 128 for the default layouts) and packed on
//! shelves of equal height, largest first, so every type's rectangle starts on a multiple of its
//! own frame size and its frames stay aligned through the mip chain.

use anyhow::Result;
use lodforge_dds::mips::{Edge, MipKind};

use crate::bake::Baked;

/// Where one type sits: page index and its pixel rectangle (square, `size` texels).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Slot {
    pub page: u32,
    pub x: u32,
    pub y: u32,
    pub size: u32,
}

struct Shelf {
    y: u32,
    height: u32,
    x: u32,
}

struct PageState {
    shelves: Vec<Shelf>,
    next_y: u32,
    /// Width of a page that holds one oversized type alone.
    solo: Option<u32>,
}

/// Packs squares of `sizes` into pages of `page_size` x `page_size` texels (0: one page per
/// type). Larger squares go first; ties keep the input order, so the result is deterministic.
pub fn pack(sizes: &[u32], page_size: u32) -> Vec<Slot> {
    let mut order: Vec<usize> = (0..sizes.len()).collect();
    order.sort_by(|&a, &b| sizes[b].cmp(&sizes[a]).then(a.cmp(&b)));
    let mut pages: Vec<PageState> = Vec::new();
    let mut out = vec![Slot { page: 0, x: 0, y: 0, size: 0 }; sizes.len()];
    for i in order {
        let s = sizes[i];
        if page_size == 0 || s > page_size {
            pages.push(PageState { shelves: Vec::new(), next_y: s, solo: Some(s) });
            out[i] = Slot { page: pages.len() as u32 - 1, x: 0, y: 0, size: s };
            continue;
        }
        let mut placed = None;
        'pages: for (pi, p) in pages.iter_mut().enumerate() {
            if p.solo.is_some() {
                continue;
            }
            for sh in &mut p.shelves {
                if sh.height == s && sh.x + s <= page_size {
                    placed = Some((pi, sh.x, sh.y));
                    sh.x += s;
                    break 'pages;
                }
            }
            if p.next_y + s <= page_size {
                p.shelves.push(Shelf { y: p.next_y, height: s, x: s });
                placed = Some((pi, 0, p.next_y));
                p.next_y += s;
                break;
            }
        }
        let (pi, x, y) = placed.unwrap_or_else(|| {
            pages.push(PageState { shelves: vec![Shelf { y: 0, height: s, x: s }], next_y: s, solo: None });
            (pages.len() - 1, 0, 0)
        });
        out[i] = Slot { page: pi as u32, x, y, size: s };
    }
    out
}

/// One composed page (RGBA8, rows top first), cropped to the area its slots use.
pub struct Page {
    pub width: u32,
    pub height: u32,
    pub albedo: Vec<u8>,
    pub normal_depth: Vec<u8>,
    /// Smallest frame size on the page (sets the mip count).
    pub min_frame: u32,
}

/// Composes the pages of `slots` (`bakes[k]` goes to `slots[k]`). Unused texels are
/// transparent black (albedo) and up-normal / far (normal-depth).
pub fn compose(bakes: &[&Baked], slots: &[Slot]) -> Vec<Page> {
    let n = slots.iter().map(|s| s.page + 1).max().unwrap_or(0) as usize;
    (0..n)
        .map(|pi| {
            let on: Vec<usize> = (0..slots.len()).filter(|&k| slots[k].page as usize == pi).collect();
            let w = on.iter().map(|&k| slots[k].x + slots[k].size).max().unwrap_or(4).div_ceil(4) * 4;
            let h = on.iter().map(|&k| slots[k].y + slots[k].size).max().unwrap_or(4).div_ceil(4) * 4;
            let mut albedo = vec![0u8; (w * h * 4) as usize];
            let mut nd: Vec<u8> = std::iter::repeat_n([128u8, 128, 255, 255], (w * h) as usize).flatten().collect();
            for &k in &on {
                let (s, b) = (slots[k], bakes[k]);
                let size = b.size() as usize;
                for y in 0..size {
                    let d = (((s.y as usize + y) * w as usize) + s.x as usize) * 4;
                    albedo[d..d + size * 4].copy_from_slice(&b.albedo[y * size * 4..(y + 1) * size * 4]);
                    nd[d..d + size * 4].copy_from_slice(&b.normal_depth[y * size * 4..(y + 1) * size * 4]);
                }
            }
            let min_frame = on.iter().map(|&k| bakes[k].frame_px).min().unwrap_or(4);
            Page { width: w, height: h, albedo, normal_depth: nd, min_frame }
        })
        .collect()
}

impl Page {
    /// Mip levels written: down to 1 texel per frame of the smallest frame on the page.
    pub fn mip_levels(&self) -> u32 {
        self.min_frame.max(1).ilog2() + 1
    }
}

/// Which page image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Map {
    Albedo,
    NormalDepth,
}

/// The page's mip chain of `map` (albedo filtered as sRGB colour, normals renormalised).
pub fn mips(p: &Page, map: Map) -> Vec<(u32, u32, Vec<u8>)> {
    let (px, kind) = match map {
        Map::Albedo => (&p.albedo, MipKind::Color),
        Map::NormalDepth => (&p.normal_depth, MipKind::Normal),
    };
    lodforge_dds::mips::chain(p.width, p.height, px, p.mip_levels(), kind, Edge::Clamp)
}

/// BC7 DDS of one page map, every mip level.
pub fn page_dds(gpu: &lodforge_gpu::Gpu, p: &Page, map: Map) -> Result<Vec<u8>> {
    let chain = mips(p, map);
    let imgs: Vec<lodforge_gpu::RgbaImage> = chain.iter().map(|(w, h, px)| lodforge_gpu::RgbaImage { width: *w, height: *h, rgba: px }).collect();
    let blocks = gpu.encode_bc7(&imgs)?;
    Ok(lodforge_dds::write_bc7(p.width, p.height, &blocks))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_shelves_without_overlap_and_deterministically() {
        let sizes = [1536, 1024, 512, 512, 1536, 1536, 1024, 512, 1024, 1536, 8192];
        let slots = pack(&sizes, 4096);
        assert_eq!(slots, pack(&sizes, 4096));
        for (k, s) in slots.iter().enumerate() {
            assert_eq!(s.size, sizes[k]);
            if s.size <= 4096 {
                assert!(s.x + s.size <= 4096 && s.y + s.size <= 4096);
                // Aligned to the type's own frame size of every default layout.
                assert_eq!(s.x % 128, 0);
                assert_eq!(s.y % 128, 0);
            }
            for (k2, t) in slots.iter().enumerate().skip(k + 1) {
                if s.page == t.page {
                    let apart = s.x + s.size <= t.x || t.x + t.size <= s.x || s.y + s.size <= t.y || t.y + t.size <= s.y;
                    assert!(apart, "{k} and {k2} overlap");
                }
            }
        }
        // The oversized type has a page of its own.
        let big = slots[10];
        assert!(slots.iter().enumerate().all(|(k, s)| k == 10 || s.page != big.page));
        // Without pages, one per type.
        let solo = pack(&sizes, 0);
        let mut pages: Vec<u32> = solo.iter().map(|s| s.page).collect();
        pages.dedup();
        assert_eq!(pages.len(), sizes.len());
    }

    #[test]
    fn compose_places_types_and_crops_pages() {
        let fit = crate::bake::Fit { min: glam::Vec3::ZERO, max: glam::Vec3::ONE, center: glam::Vec3::splat(0.5), radius: 1.0 };
        let a = Baked { grid: 2, frame_px: 4, fit, albedo: vec![200; 8 * 8 * 4], normal_depth: vec![10; 8 * 8 * 4] };
        let b = Baked { grid: 2, frame_px: 2, fit, albedo: vec![100; 4 * 4 * 4], normal_depth: vec![20; 4 * 4 * 4] };
        let slots = pack(&[8, 4], 16);
        let pages = compose(&[&a, &b], &slots);
        assert_eq!(pages.len(), 1);
        let p = &pages[0];
        // b gets a shelf of its own height below a.
        assert_eq!(slots[1], Slot { page: 0, x: 0, y: 8, size: 4 });
        assert_eq!((p.width, p.height), (8, 12));
        assert_eq!(p.min_frame, 2);
        assert_eq!(p.mip_levels(), 2);
        assert_eq!(p.albedo[0], 200);
        assert_eq!(p.albedo[(8 * 8 * 4) as usize], 100);
        // Unused corner: transparent albedo, up-normal + far depth.
        let corner = ((11 * 8 + 7) * 4) as usize;
        assert_eq!(&p.albedo[corner..corner + 4], &[0, 0, 0, 0]);
        assert_eq!(&p.normal_depth[corner..corner + 4], &[128, 128, 255, 255]);
        assert_eq!(mips(p, Map::Albedo).len(), 2);
    }
}
