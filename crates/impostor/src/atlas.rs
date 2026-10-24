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
    /// A column of smaller squares stacked inside the shelf: x, square size, height used.
    col: (u32, u32, u32),
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
                // Smaller squares stack in columns inside a taller shelf.
                if s < sh.height {
                    if sh.col.1 == s && sh.col.2 + s <= sh.height {
                        placed = Some((pi, sh.col.0, sh.y + sh.col.2));
                        sh.col.2 += s;
                        break 'pages;
                    }
                    if sh.x + s <= page_size {
                        sh.col = (sh.x, s, s);
                        placed = Some((pi, sh.x, sh.y));
                        sh.x += s;
                        break 'pages;
                    }
                }
            }
            if p.next_y + s <= page_size {
                p.shelves.push(Shelf { y: p.next_y, height: s, x: s, col: (0, 0, 0) });
                placed = Some((pi, 0, p.next_y));
                p.next_y += s;
                break;
            }
        }
        let (pi, x, y) = placed.unwrap_or_else(|| {
            pages.push(PageState { shelves: vec![Shelf { y: 0, height: s, x: s, col: (0, 0, 0) }], next_y: s, solo: None });
            (pages.len() - 1, 0, 0)
        });
        out[i] = Slot { page: pi as u32, x, y, size: s };
    }
    out
}

/// One composed page (RGBA8, rows top first), cropped to the area its slots use, with its mip
/// chain (level 0 first).
pub struct Page {
    pub width: u32,
    pub height: u32,
    /// Level 0 of both maps.
    pub albedo: Vec<u8>,
    pub normal_depth: Vec<u8>,
    /// Smallest frame size on the page (sets the mip count).
    pub min_frame: u32,
    /// Every level of both maps, level 0 included.
    pub albedo_mips: Vec<Vec<u8>>,
    pub normal_depth_mips: Vec<Vec<u8>>,
}

/// Mip levels of a type with `frame_px` frames: down to 1 texel per frame.
pub fn levels_for(frame_px: u32) -> u32 {
    frame_px.max(1).ilog2() + 1
}

/// Scales the alpha of every `cell` x `cell` frame of `level` (`w` texels wide) so the share of
/// texels at or above `alpha_ref` matches `target` (that frame's share at level 0): the
/// alpha-tested silhouette keeps its coverage down the mip chain instead of thinning out
/// (alpha-to-coverage-preserving mips).
pub fn preserve_coverage(level: &mut [u8], w: usize, cell: usize, target: &[f32], alpha_ref: f32) {
    let cols = w / cell;
    let rows = level.len() / 4 / w / cell;
    let r = alpha_ref * 255.0;
    for cy in 0..rows {
        for cx in 0..cols {
            let Some(&want) = target.get(cy * cols + cx) else { continue };
            let idx = |k: usize| ((cy * cell + k / cell) * w + cx * cell + k % cell) * 4 + 3;
            let n = cell * cell;
            let share = |s: f32| (0..n).filter(|&k| (f32::from(level[idx(k)]) * s).min(255.0) >= r).count() as f32 / n as f32;
            // Coverage grows with the scale: bisect for the smallest scale reaching the target.
            let (mut lo, mut hi) = (0.0f32, 16.0f32);
            if share(hi) < want {
                lo = hi;
            } else {
                for _ in 0..24 {
                    let mid = 0.5 * (lo + hi);
                    if share(mid) >= want { hi = mid } else { lo = mid }
                }
            }
            let s = if share(lo) >= want { lo } else { hi };
            for k in 0..n {
                let i = idx(k);
                level[i] = (f32::from(level[i]) * s).round().min(255.0) as u8;
            }
        }
    }
}

/// Share of each frame's texels with alpha at or above `alpha_ref`, row by row.
fn coverage_shares(albedo: &[u8], w: usize, cell: usize, alpha_ref: f32) -> Vec<f32> {
    let cols = w / cell;
    let rows = albedo.len() / 4 / w / cell;
    let r = (alpha_ref * 255.0).ceil() as u8;
    (0..rows * cols)
        .map(|c| {
            let (cy, cx) = (c / cols, c % cols);
            let mut k = 0;
            for y in 0..cell {
                for x in 0..cell {
                    if albedo[((cy * cell + y) * w + cx * cell + x) * 4 + 3] >= r {
                        k += 1;
                    }
                }
            }
            k as f32 / (cell * cell) as f32
        })
        .collect()
}

/// A type's own mip chains (`levels` levels): albedo filtered as sRGB colour with per-frame
/// coverage kept at `alpha_ref`, normals renormalised.
pub fn type_mips(b: &Baked, levels: u32, alpha_ref: f32) -> (Vec<Vec<u8>>, Vec<Vec<u8>>) {
    let size = b.size();
    let a = lodforge_dds::mips::chain(size, size, &b.albedo, levels, MipKind::Color, Edge::Clamp);
    let n = lodforge_dds::mips::chain(size, size, &b.normal_depth, levels, MipKind::Normal, Edge::Clamp);
    let target = coverage_shares(&b.albedo, size as usize, b.frame_px as usize, alpha_ref);
    let mut albedo: Vec<Vec<u8>> = a.into_iter().map(|l| l.2).collect();
    for (l, px) in albedo.iter_mut().enumerate().skip(1) {
        let cell = (b.frame_px >> l).max(1) as usize;
        preserve_coverage(px, (size >> l).max(1) as usize, cell, &target, alpha_ref);
    }
    (albedo, n.into_iter().map(|l| l.2).collect())
}

/// Composes the pages of `slots` (`bakes[k]` goes to `slots[k]`) with their mip chains. Unused
/// texels are transparent black (albedo) and up-normal / far (normal-depth).
pub fn compose(bakes: &[&Baked], slots: &[Slot], alpha_ref: f32) -> Vec<Page> {
    let n = slots.iter().map(|s| s.page + 1).max().unwrap_or(0) as usize;
    (0..n)
        .map(|pi| {
            let on: Vec<usize> = (0..slots.len()).filter(|&k| slots[k].page as usize == pi).collect();
            let w = on.iter().map(|&k| slots[k].x + slots[k].size).max().unwrap_or(4).div_ceil(4) * 4;
            let h = on.iter().map(|&k| slots[k].y + slots[k].size).max().unwrap_or(4).div_ceil(4) * 4;
            let min_frame = on.iter().map(|&k| bakes[k].frame_px).min().unwrap_or(4);
            let levels = levels_for(min_frame);
            let mut albedo_mips = Vec::new();
            let mut nd_mips = Vec::new();
            for l in 0..levels {
                let (lw, lh) = ((w >> l).max(1), (h >> l).max(1));
                albedo_mips.push(vec![0u8; (lw * lh * 4) as usize]);
                nd_mips.push(std::iter::repeat_n([128u8, 128, 255, 255], (lw * lh) as usize).flatten().collect::<Vec<u8>>());
            }
            for &k in &on {
                let (s, b) = (slots[k], bakes[k]);
                let (am, nm) = type_mips(b, levels, alpha_ref);
                for l in 0..levels as usize {
                    let size = (b.size() >> l).max(1) as usize;
                    let lw = (w >> l).max(1) as usize;
                    let (x0, y0) = ((s.x >> l) as usize, (s.y >> l) as usize);
                    for y in 0..size {
                        let d = ((y0 + y) * lw + x0) * 4;
                        albedo_mips[l][d..d + size * 4].copy_from_slice(&am[l][y * size * 4..(y + 1) * size * 4]);
                        nd_mips[l][d..d + size * 4].copy_from_slice(&nm[l][y * size * 4..(y + 1) * size * 4]);
                    }
                }
            }
            Page { width: w, height: h, albedo: albedo_mips[0].clone(), normal_depth: nd_mips[0].clone(), min_frame, albedo_mips, normal_depth_mips: nd_mips }
        })
        .collect()
}

impl Page {
    /// Mip levels written: down to 1 texel per frame of the smallest frame on the page.
    pub fn mip_levels(&self) -> u32 {
        levels_for(self.min_frame)
    }
}

/// Which page image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Map {
    Albedo,
    NormalDepth,
}

/// The page's mip chain of `map`: (width, height, RGBA8) per level.
pub fn mips(p: &Page, map: Map) -> Vec<(u32, u32, Vec<u8>)> {
    let levels = match map {
        Map::Albedo => &p.albedo_mips,
        Map::NormalDepth => &p.normal_depth_mips,
    };
    levels.iter().enumerate().map(|(l, px)| ((p.width >> l).max(1), (p.height >> l).max(1), px.clone())).collect()
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
        let pages = compose(&[&a, &b], &slots, 0.5);
        assert_eq!(pages.len(), 1);
        let p = &pages[0];
        // b stacks in a column beside a, inside a's shelf.
        assert_eq!(slots[1], Slot { page: 0, x: 8, y: 0, size: 4 });
        assert_eq!((p.width, p.height), (12, 8));
        assert_eq!(p.min_frame, 2);
        assert_eq!(p.mip_levels(), 2);
        assert_eq!(p.albedo[0], 200);
        assert_eq!(p.albedo[(8 * 4) as usize], 100);
        // Unused corner: transparent albedo, up-normal + far depth.
        let corner = ((7 * 12 + 11) * 4) as usize;
        assert_eq!(&p.albedo[corner..corner + 4], &[0, 0, 0, 0]);
        assert_eq!(&p.normal_depth[corner..corner + 4], &[128, 128, 255, 255]);
        assert_eq!(mips(p, Map::Albedo).len(), 2);
    }
}

#[cfg(test)]
mod coverage_tests {
    use super::*;

    #[test]
    fn mips_keep_each_frames_alpha_tested_coverage() {
        // One 8x8 frame of a 2x1 grid: a fine checkerboard of opaque / clear texels (half
        // covered); the other frame fully clear. A plain box filter averages it to alpha ~127,
        // which an alpha test at 0.5 rejects everywhere.
        let fit = crate::bake::Fit { min: glam::Vec3::ZERO, max: glam::Vec3::ONE, center: glam::Vec3::splat(0.5), radius: 1.0 };
        let (w, f) = (16usize, 8usize);
        let mut albedo = vec![0u8; w * w * 4];
        for y in 0..f {
            for x in 0..f {
                let i = (y * w + x) * 4;
                albedo[i..i + 4].copy_from_slice(&[50, 120, 40, if (x + y) % 2 == 0 { 255 } else { 0 }]);
            }
        }
        let b = Baked { grid: 2, frame_px: 8, fit, albedo, normal_depth: vec![128; w * w * 4] };
        let (am, _) = type_mips(&b, levels_for(8), 0.5);
        assert_eq!(am.len(), 4);
        for (l, px) in am.iter().enumerate().skip(1) {
            let (lw, cell) = (w >> l, f >> l);
            let covered = (0..cell).flat_map(|y| (0..cell).map(move |x| (y, x))).filter(|&(y, x)| px[(y * lw + x) * 4 + 3] >= 128).count();
            let share = covered as f32 / (cell * cell) as f32;
            assert!((share - 0.5).abs() <= 0.5 / (cell * cell) as f32 + 0.26, "level {l}: coverage {share}");
            assert!(covered > 0, "level {l} kept some coverage");
            // The empty frame stays empty.
            assert!((0..cell).all(|y| (cell..2 * cell).all(|x| px[(y * lw + x) * 4 + 3] < 128)), "level {l}");
        }
    }
}
