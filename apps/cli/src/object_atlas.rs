//! Object LOD atlas textures (`<World>ObjectsLOD[NN].dds` + `_n.dds`) from an atlas layout, as
//! xEdit's `wbBuildAtlas` assembles them: source top mips copied 1:1 into the page (or the mip /
//! Lanczos resize of the tile size), normal maps resized to their diffuse with Lanczos, normal
//! alpha forced opaque, Lanczos mip chain. xLODGen writes DXT5 / DXT1; its pages here are BC7
//! (GPU), like the terrain LOD textures. DynDOLOD pages follow its INI formats ([Formats]):
//! BC1 opaque pages, a BC7 `_Alpha` page with average-colour backgrounds and alpha mips that keep
//! their coverage, 8 mips.

use std::path::Path;

use anyhow::{Context, Result};
use rayon::prelude::*;

use lodforge_archive::Vfs;
use lodforge_dds::mips::{Edge, MipKind};
use lodforge_gpu::{Gpu, RgbaImage};
use lodforge_objlod::atlas_build::AtlasLayout;
use lodforge_terrain::image::{Image, compare};

use crate::object_export::normal_for;

/// One rendered page: game path, diffuse and normal DDS bytes, and the top mips (RGBA8). Glow
/// pages (DynDOLOD `_Glow.dds`) also have a glow map (`_g.dds`).
pub struct Page {
    pub name: String,
    pub diffuse: Vec<u8>,
    pub normal: Vec<u8>,
    pub glow: Option<Vec<u8>>,
    pub top: (Vec<u8>, Vec<u8>),
    pub width: u32,
    pub height: u32,
}

fn blit(dst: &mut [u8], dw: u32, src: &[u8], sw: u32, sh: u32, x: u32, y: u32) {
    for row in 0..sh {
        let d = (((y + row) * dw + x) * 4) as usize;
        let s = (row * sw * 4) as usize;
        dst[d..d + (sw * 4) as usize].copy_from_slice(&src[s..s + (sw * 4) as usize]);
    }
}

/// Colour mip chain down to 1x1 (float, linear light, wrapped edges; see lodforge_dds::mips).
pub fn mip_chain(w: u32, h: u32, rgba: Vec<u8>) -> Vec<(u32, u32, Vec<u8>)> {
    mip_chain_n(w, h, rgba, u32::MAX)
}

/// [mip_chain] with at most `levels` levels (the top one included).
pub fn mip_chain_n(w: u32, h: u32, rgba: Vec<u8>, levels: u32) -> Vec<(u32, u32, Vec<u8>)> {
    lodforge_dds::mips::chain(w, h, &rgba, levels, MipKind::Color, Edge::Wrap)
}

pub fn encode(gpu: &Gpu, levels: &[(u32, u32, Vec<u8>)]) -> Result<Vec<u8>> {
    let images: Vec<RgbaImage<'_>> = levels.iter().map(|(w, h, p)| RgbaImage { width: *w, height: *h, rgba: p }).collect();
    let blocks = gpu.encode_bc7(&images)?;
    Ok(lodforge_dds::write_bc7(levels[0].0, levels[0].1, &blocks))
}

/// BC1 (DXT1, opaque) DDS of a mip chain, encoded on the CPU.
pub fn encode_bc1(levels: &[(u32, u32, Vec<u8>)]) -> Vec<u8> {
    let blocks: Vec<Vec<u8>> = levels
        .iter()
        .map(|(w, h, p)| {
            let bx = w.div_ceil(4) as usize;
            let mut out = vec![0u8; bx * h.div_ceil(4) as usize * 8];
            // Bands of 8 block rows.
            out.par_chunks_mut(bx * 8 * 8).enumerate().for_each(|(i, band)| {
                let y0 = i * 8;
                lodforge_dds::bc1::encode_bc1_rows(*w, *h, p, y0, y0 + band.len() / (bx * 8), band);
            });
            out
        })
        .collect();
    lodforge_dds::write_dxt1(levels[0].0, levels[0].1, &blocks)
}

/// Block compression of one atlas texture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    /// BC1 on the CPU, alpha dropped.
    Bc1,
    /// BC7 (mode 6) on the GPU.
    Bc7,
}

/// Formats and mips of the pages [render] writes. Pages named `*_Alpha.dds` use
/// `alpha_diffuse`; glow pages always use BC7 for their diffuse and glow map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Formats {
    pub diffuse: Encoding,
    pub alpha_diffuse: Encoding,
    pub normal: Encoding,
    /// Mip levels per page, the top one included (`u32::MAX`: down to 1x1).
    pub mips: u32,
    /// DynDOLOD's `ObjectLODCreateAverageColor` and alpha-coverage mips on alpha pages.
    pub alpha_fill: bool,
}

impl Formats {
    /// xLODGen-style pages: everything BC7 with the full mip chain.
    pub const BC7_FULL: Formats = Formats { diffuse: Encoding::Bc7, alpha_diffuse: Encoding::Bc7, normal: Encoding::Bc7, mips: u32::MAX, alpha_fill: false };
}

fn encode_as(gpu: &Gpu, enc: Encoding, levels: &[(u32, u32, Vec<u8>)]) -> Result<Vec<u8>> {
    match enc {
        Encoding::Bc1 => Ok(encode_bc1(levels)),
        Encoding::Bc7 => encode(gpu, levels),
    }
}

/// Whether `name` is a DynDOLOD alpha page (`DynDOLOD_<World>_Alpha[NN].dds`).
pub fn is_alpha_page(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    let stem = n.trim_end_matches(".dds").trim_end_matches(|c: char| c.is_ascii_digit());
    stem.ends_with("_alpha")
}

/// Alpha threshold LOD renders with (DynDOLOD: "LOD only uses transparency and the threshold is
/// 128").
const ALPHA_THRESHOLD: u8 = 128;

/// `ObjectLODCreateAverageColor`: the colour of texels LOD does not draw (alpha below the
/// threshold) becomes the alpha-weighted average colour of the drawn ones, so they do not bleed
/// dark fringes into the mips. Textures without such texels are left alone.
pub fn fill_average_color(rgba: &mut [u8]) {
    let (mut sum, mut weight) = ([0f64; 3], 0f64);
    let mut hidden = false;
    for px in rgba.chunks_exact(4) {
        if px[3] >= ALPHA_THRESHOLD {
            let a = f64::from(px[3]);
            for c in 0..3 {
                sum[c] += f64::from(px[c]) * a;
            }
            weight += a;
        } else {
            hidden = true;
        }
    }
    if !hidden || weight == 0.0 {
        return;
    }
    let avg = sum.map(|v| (v / weight).round() as u8);
    for px in rgba.chunks_exact_mut(4) {
        if px[3] < ALPHA_THRESHOLD {
            px[..3].copy_from_slice(&avg);
        }
    }
}

/// Share of texels at or above the alpha threshold in a rectangle of a mip.
fn coverage(rgba: &[u8], w: u32, rect: (u32, u32, u32, u32), scale: f32) -> f32 {
    let (x0, y0, rw, rh) = rect;
    let mut n = 0u32;
    for y in y0..y0 + rh {
        for x in x0..x0 + rw {
            let a = f32::from(rgba[((y * w + x) * 4 + 3) as usize]) * scale;
            if a >= f32::from(ALPHA_THRESHOLD) {
                n += 1;
            }
        }
    }
    n as f32 / (rw * rh).max(1) as f32
}

/// Alpha mips that keep each tile's coverage at the LOD alpha threshold (texconv's
/// `-keepcoverage`, DynDOLOD's `AlphaCoverage`): lower mips of alpha-tested tiles get their
/// alpha scaled until as many texels pass the threshold as on the top mip, so foliage does not
/// thin out with distance. `tiles` are (x, y, w, h) on the top mip.
pub fn keep_alpha_coverage(levels: &mut [(u32, u32, Vec<u8>)], tiles: &[(u32, u32, u32, u32)]) {
    let Some(((tw, _, top), rest)) = levels.split_first_mut() else { return };
    for &(x, y, w, h) in tiles {
        let target = coverage(top, *tw, (x, y, w, h), 1.0);
        if target <= 0.0 || target >= 1.0 {
            continue;
        }
        for (k, (lw, _, px)) in rest.iter_mut().enumerate() {
            let sh = k as u32 + 1;
            let rect = (x >> sh, y >> sh, (w >> sh).max(1), (h >> sh).max(1));
            if rect.0 + rect.2 > *lw {
                continue;
            }
            // Binary search for the alpha scale that reaches the target coverage.
            let (mut lo, mut hi) = (0.0f32, 8.0f32);
            for _ in 0..16 {
                let mid = (lo + hi) / 2.0;
                if coverage(px, *lw, rect, mid) < target { lo = mid } else { hi = mid }
            }
            let scale = hi;
            for yy in rect.1..rect.1 + rect.3 {
                for xx in rect.0..rect.0 + rect.2 {
                    let i = ((yy * *lw + xx) * 4 + 3) as usize;
                    px[i] = (f32::from(px[i]) * scale).round().min(255.0) as u8;
                }
            }
        }
    }
}

/// Opaque share of tree billboards in LOD, relative to their rendered coverage
/// (`--billboard-density`; 1 = as rendered). Billboards are two crossed planes, so from most
/// angles both silhouettes overlap, and the small mips distant LOD samples merge the gaps between
/// leaves: the billboard reads denser than the full tree, which lets light through, and the switch
/// to the full tree shows (skyrim-cs-base 2026-10-23). Below 1, the alpha of every level of a
/// billboard tile is scaled until only that share of its texels passes the alpha test.
static BILLBOARD_DENSITY: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x3f80_0000);

pub fn set_billboard_density(d: f32) {
    BILLBOARD_DENSITY.store(d.clamp(0.1, 1.0).to_bits(), std::sync::atomic::Ordering::Relaxed);
}

pub fn billboard_density() -> f32 {
    f32::from_bits(BILLBOARD_DENSITY.load(std::sync::atomic::Ordering::Relaxed))
}

/// Density for an atlas texture: [billboard_density] for tree billboards, 1 for the rest.
/// (A separate conifer density, 0.75, compensated for solid-looking pine billboards until their
/// alpha was dithered; removed 2026-10-24.)
pub fn density_for(texture: &str) -> f32 {
    if is_tree_billboard(texture) { billboard_density() } else { 1.0 }
}

/// Dither tree billboard alpha (`--billboard-soft` turns it off): every mip level of a billboard
/// tile keeps its supersampled soft coverage and is error-diffused into on/off alpha, instead of
/// being rescaled so the alpha test passes the same share of texels as the top mip. Thin foliage
/// then survives the game's alpha test as a fine pattern at the coverage it really has, instead of
/// dense clusters turning solid and sparse ones vanishing (the cloud crowns' fix, 2026-10-23).
static BILLBOARD_DITHER: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

pub fn set_billboard_dither(on: bool) {
    BILLBOARD_DITHER.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn billboard_dither() -> bool {
    BILLBOARD_DITHER.load(std::sync::atomic::Ordering::Relaxed)
}

/// Floyd-Steinberg (serpentine) of the alpha inside a rectangle of an RGBA image into 0 / 255.
pub fn dither_rect(rgba: &mut [u8], width: u32, rect: (u32, u32, u32, u32)) {
    let (x0, y0, w, h) = (rect.0 as usize, rect.1 as usize, rect.2 as usize, rect.3 as usize);
    let iw = width as usize;
    let mut err = vec![0.0f32; w * h];
    for y in 0..h {
        let ltr = y % 2 == 0;
        for i in 0..w {
            let x = if ltr { i } else { w - 1 - i };
            let k = y * w + x;
            let a = ((y0 + y) * iw + x0 + x) * 4 + 3;
            let v = f32::from(rgba[a]) / 255.0 + err[k];
            let out = if v >= 0.5 { 1.0 } else { 0.0 };
            rgba[a] = if out > 0.0 { 255 } else { 0 };
            let e = v - out;
            let fwd = |dx: isize| if ltr { x as isize + dx } else { x as isize - dx };
            let mut add = |xx: isize, yy: usize, f: f32| {
                if xx >= 0 && (xx as usize) < w && yy < h {
                    err[yy * w + xx as usize] += e * f;
                }
            };
            add(fwd(1), y, 7.0 / 16.0);
            add(fwd(-1), y + 1, 3.0 / 16.0);
            add(x as isize, y + 1, 5.0 / 16.0);
            add(fwd(1), y + 1, 1.0 / 16.0);
        }
    }
}

/// Whether an atlas texture key is a TexGen tree billboard (`textures\terrain\lodgen\...`).
pub fn is_tree_billboard(texture: &str) -> bool {
    texture.to_ascii_lowercase().replace('/', "\\").contains("\\terrain\\lodgen\\")
}

/// [keep_alpha_coverage], then for tiles with a `density` below 1 the alpha of every level (the
/// top one too) is scaled down until only `density` times the top mip's coverage passes the
/// test. Tiles with `dither` set instead keep their filtered soft alpha, times `density`, and are
/// dithered into on/off alpha on every level ([billboard_dither]).
pub fn keep_alpha_coverage_with(levels: &mut [(u32, u32, Vec<u8>)], tiles: &[(u32, u32, u32, u32)], density: &[f32], dither: &[bool]) {
    let targets: Vec<f32> = match levels.first() {
        Some((tw, _, top)) => tiles.iter().map(|&r| coverage(top, *tw, r, 1.0)).collect(),
        None => return,
    };
    // The soft alpha of dithered tiles, per level, before the coverage rescale.
    let soft: Vec<Option<Vec<Vec<u8>>>> = tiles
        .iter()
        .zip(dither.iter().chain(std::iter::repeat(&false)))
        .map(|(&(x, y, w, h), &on)| {
            on.then(|| {
                levels
                    .iter()
                    .enumerate()
                    .map(|(k, (lw, _, px))| {
                        let sh = k as u32;
                        let (rx, ry, rw, rh) = (x >> sh, y >> sh, (w >> sh).max(1), (h >> sh).max(1));
                        let mut a = Vec::with_capacity((rw * rh) as usize);
                        for yy in ry..ry + rh {
                            for xx in rx..rx + rw {
                                a.push(px.get(((yy * *lw + xx) * 4 + 3) as usize).copied().unwrap_or(0));
                            }
                        }
                        a
                    })
                    .collect()
            })
        })
        .collect();
    keep_alpha_coverage(levels, tiles);
    for (ti, ((&(x, y, w, h), &d), soft)) in tiles.iter().zip(density).zip(&soft).enumerate() {
        let _ = ti;
        let Some(soft) = soft else { continue };
        for (k, (lw, _, px)) in levels.iter_mut().enumerate() {
            let sh = k as u32;
            let rect = (x >> sh, y >> sh, (w >> sh).max(1), (h >> sh).max(1));
            if rect.0 + rect.2 > *lw {
                continue;
            }
            let mut i = 0;
            for yy in rect.1..rect.1 + rect.3 {
                for xx in rect.0..rect.0 + rect.2 {
                    px[((yy * *lw + xx) * 4 + 3) as usize] = (f32::from(soft[k][i]) * d.min(1.0)).round() as u8;
                    i += 1;
                }
            }
            dither_rect(px, *lw, rect);
        }
    }
    for (((&(x, y, w, h), &d), &top_cov), soft) in tiles.iter().zip(density).zip(&targets).zip(&soft) {
        if d >= 1.0 || top_cov <= 0.0 || soft.is_some() {
            continue;
        }
        let target = top_cov * d;
        for (k, (lw, _, px)) in levels.iter_mut().enumerate() {
            let sh = k as u32;
            let rect = (x >> sh, y >> sh, (w >> sh).max(1), (h >> sh).max(1));
            if rect.0 + rect.2 > *lw {
                continue;
            }
            // Largest scale (at most 1) whose coverage is still at least the target.
            let (mut lo, mut hi) = (0.0f32, 1.0f32);
            for _ in 0..16 {
                let mid = (lo + hi) / 2.0;
                if coverage(px, *lw, rect, mid) < target { lo = mid } else { hi = mid }
            }
            let scale = hi;
            for yy in rect.1..rect.1 + rect.3 {
                for xx in rect.0..rect.0 + rect.2 {
                    let i = ((yy * *lw + xx) * 4 + 3) as usize;
                    px[i] = (f32::from(px[i]) * scale).round().min(255.0) as u8;
                }
            }
        }
    }
}

/// One atlas tile with its own mip chain: level k is `max(w >> k, 1)` x `max(h >> k, 1)`.
pub struct TileChain {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    pub levels: Vec<Vec<u8>>,
}

impl TileChain {
    /// The tile's chain, filtered on its own (`edge` decides what lies past its border), so a
    /// tile's mips never take colour from its neighbours on the page.
    pub fn new(x: u32, y: u32, w: u32, h: u32, top: &[u8], levels: u32, kind: MipKind, edge: Edge) -> TileChain {
        let levels = lodforge_dds::mips::chain(w, h, top, levels, kind, edge).into_iter().map(|(_, _, px)| px).collect();
        TileChain { x, y, w, h, levels }
    }
}

/// Number of levels of a `w` x `h` page's chain down to 1x1, at most `cap`.
fn page_levels(w: u32, h: u32, cap: u32) -> u32 {
    let mut n = 1;
    let (mut cw, mut ch) = (w, h);
    while (cw > 1 || ch > 1) && n < cap {
        cw = (cw / 2).max(1);
        ch = (ch / 2).max(1);
        n += 1;
    }
    n
}

/// 2x2 box downscale (odd edges clamp) of an RGBA8 image.
fn box_half(w: u32, h: u32, px: &[u8]) -> (u32, u32, Vec<u8>) {
    let (ow, oh) = ((w / 2).max(1), (h / 2).max(1));
    let mut out = vec![0u8; (ow * oh * 4) as usize];
    for y in 0..oh {
        for x in 0..ow {
            for c in 0..4 {
                let at = |xx: u32, yy: u32| u32::from(px[((yy.min(h - 1) * w + xx.min(w - 1)) * 4 + c) as usize]);
                let s = at(2 * x, 2 * y) + at(2 * x + 1, 2 * y) + at(2 * x, 2 * y + 1) + at(2 * x + 1, 2 * y + 1);
                out[((y * ow + x) * 4 + c) as usize] = ((s + 2) / 4) as u8;
            }
        }
    }
    (ow, oh, out)
}

/// A page's mip chain from its tiles' own chains: level k places every tile whose position and
/// size are multiples of 2^k at (x >> k, y >> k). Texels no tile covers exactly at a level (gaps,
/// tiles already below one texel, unaligned tiles) come from a box downscale of the level above.
/// Level 0 is filled with `fill` where no tile is.
pub fn assemble(width: u32, height: u32, levels: u32, fill: [u8; 4], tiles: &[TileChain]) -> Vec<(u32, u32, Vec<u8>)> {
    let mut top: Vec<u8> = fill.repeat((width * height) as usize);
    for t in tiles {
        blit(&mut top, width, &t.levels[0], t.w, t.h, t.x, t.y);
    }
    let mut out = vec![(width, height, top)];
    for k in 1..levels {
        let (pw, ph, prev) = out.last().unwrap();
        let (w, h, mut px) = box_half(*pw, *ph, prev);
        let m = 1u32 << k;
        for t in tiles {
            let exact = t.x % m == 0 && t.y % m == 0 && t.w % m == 0 && t.h % m == 0;
            let Some(level) = t.levels.get(k as usize).filter(|_| exact) else { continue };
            let (tw, th) = (t.w >> k, t.h >> k);
            if (t.x >> k) + tw <= w && (t.y >> k) + th <= h {
                blit(&mut px, w, level, tw, th, t.x >> k, t.y >> k);
            }
        }
        out.push((w, h, px));
    }
    out
}

/// Decodes `path` at `w` x `h`: the source's mip of that size when it has one, else its top mip
/// resized in the working space of `kind` (linear light for colour, renormalised normals).
fn load_tile(vfs: &Vfs, path: &str, w: u32, h: u32, kind: MipKind) -> Option<Vec<u8>> {
    let b = vfs.read(&path.to_lowercase()).ok().flatten()?;
    let info = lodforge_dds::parse(&b).ok()?;
    let (mut mw, mut mh, mut level) = (info.width, info.height, 0u32);
    while (mw > w || mh > h) && level + 1 < info.mip_count {
        mw = (mw / 2).max(1);
        mh = (mh / 2).max(1);
        level += 1;
    }
    if (mw, mh) == (w, h)
        && let Ok(img) = lodforge_dds::decode(&b, &info, level)
        && (img.width, img.height) == (w, h)
    {
        return Some(img.rgba);
    }
    let img = lodforge_dds::decode(&b, &info, 0).ok()?;
    if (img.width, img.height) == (w, h) {
        return Some(img.rgba);
    }
    Some(lodforge_dds::mips::resize_rgba8(img.width, img.height, &img.rgba, (w, h), kind, Edge::Wrap))
}

/// A repeat tile's pixels: the texture at a `ru x rv`-th of the tile, laid out `ru x rv` times
/// (resized to the tile when it does not divide evenly).
fn load_repeated(vfs: &Vfs, path: &str, w: u32, h: u32, (ru, rv): (u32, u32), kind: MipKind) -> Option<Vec<u8>> {
    let (rw, rh) = ((w / ru).max(1), (h / rv).max(1));
    let one = load_tile(vfs, path, rw, rh, kind)?;
    let tiled = repeat_rgba(&one, rw, rh, ru, rv);
    let (tw, th) = (rw * ru, rh * rv);
    Some(if (tw, th) == (w, h) { tiled } else { lodforge_dds::mips::resize_rgba8(tw, th, &tiled, (w, h), kind, Edge::Wrap) })
}

/// An RGBA image `w x h` laid out `ru x rv` times.
fn repeat_rgba(px: &[u8], w: u32, h: u32, ru: u32, rv: u32) -> Vec<u8> {
    let (w, h) = (w as usize, h as usize);
    let tw = w * ru as usize;
    let mut out = vec![0u8; tw * h * rv as usize * 4];
    for y in 0..h * rv as usize {
        let src = &px[(y % h) * w * 4..(y % h + 1) * w * 4];
        for r in 0..ru as usize {
            out[(y * tw + r * w) * 4..(y * tw + (r + 1) * w) * 4].copy_from_slice(src);
        }
    }
    out
}

const FLAT_NORMAL: [u8; 4] = [128, 128, 255, 255];

/// Key suffixes of complex grass tiles: the source texture's top half (colour) or bottom half
/// (normal RGB, specular A).
pub const HALF_TOP: &str = "~ct";
pub const HALF_BOTTOM: &str = "~cb";

/// The render layout of a complex grass page family: each page twice as high, every entry
/// repeated in the bottom half for the texture's bottom half.
pub fn complex_layout(layout: &AtlasLayout) -> AtlasLayout {
    let mut l = layout.clone();
    let heights: Vec<u32> = l.pages.iter().map(|p| p.height).collect();
    for p in &mut l.pages {
        p.height *= 2;
    }
    let bottom: Vec<_> = layout
        .entries
        .iter()
        .filter(|e| e.page != usize::MAX)
        .map(|e| {
            let mut b = e.clone();
            b.y += heights[e.page];
            b.texture = format!("{}{HALF_BOTTOM}", e.texture.strip_suffix(HALF_TOP).unwrap_or(&e.texture));
            b
        })
        .collect();
    l.entries.extend(bottom);
    l
}

/// A half of a source texture at `w x h` (the full texture loaded at `w x 2h`).
fn load_half(vfs: &Vfs, path: &str, w: u32, h: u32, kind: MipKind, top: bool) -> Option<Vec<u8>> {
    let full = load_tile(vfs, path, w, h * 2, kind)?;
    let row = (w * 4) as usize;
    let start = if top { 0 } else { h as usize * row };
    full.get(start..start + h as usize * row).map(<[u8]>::to_vec)
}

/// Renders every page of `layout` in `formats`. Entry keys are `diffuse[,normal[,glow]][=alpha]`
/// (a plain diffuse takes its normal from [normal_for]); tiles smaller than their source use the
/// source's mip of that size or a resize. Every tile gets its own mip chain (colour in linear
/// light, normals renormalised, edges wrapped as the source texture repeats) and the page levels
/// are assembled from them, so distant LOD shows no colour from neighbouring tiles. A tile
/// whose normal map is missing gets a flat normal.
pub fn render(vfs: &Vfs, layout: &AtlasLayout, gpu: &Gpu, formats: &Formats) -> Result<Vec<Page>> {
    type Composed = (usize, Vec<(u32, u32, Vec<u8>)>, Vec<(u32, u32, Vec<u8>)>, Option<Vec<(u32, u32, Vec<u8>)>>);
    let composed: Vec<Composed> = (0..layout.pages.len())
        .into_par_iter()
        .map(|pi| {
            let p = &layout.pages[pi];
            let alpha_page = is_alpha_page(&p.name);
            let levels = page_levels(p.width, p.height, formats.mips);
            let entries: Vec<&lodforge_objlod::atlas_build::AtlasEntry> = layout.entries.iter().filter(|e| e.page == pi).collect();
            let glow_page = entries.iter().any(|e| crate::glow_atlas::is_glow_key(&e.texture));
            // Per tile: (diffuse, normal, glow) chains.
            let chains: Vec<(Option<TileChain>, TileChain, Option<TileChain>)> = entries
                .par_iter()
                .map(|e| {
                    let chain = |px: &[u8], kind: MipKind| TileChain::new(e.x, e.y, e.width, e.height, px, levels, kind, Edge::Wrap);
                    let flat = || FLAT_NORMAL.repeat((e.width * e.height) as usize);
                    let opaque = |mut px: Vec<u8>| {
                        // wbPrepareImageAlpha for the DXT1 normal format: every pixel opaque.
                        px.chunks_exact_mut(4).for_each(|p| p[3] = 255);
                        px
                    };
                    if crate::glow_atlas::is_glow_key(&e.texture) {
                        // `diffuse,normal,glow` tile, each source scaled to the tile.
                        let parts: Vec<&str> = e.texture.split(',').collect();
                        let d = load_tile(vfs, parts[0], e.width, e.height, MipKind::Color).map(|px| chain(&px, MipKind::Color));
                        let n = opaque(load_tile(vfs, parts[1], e.width, e.height, MipKind::Normal).unwrap_or_else(flat));
                        let g = load_tile(vfs, parts[2], e.width, e.height, MipKind::Color).map(|px| chain(&opaque(px), MipKind::Color));
                        return (d, chain(&n, MipKind::Normal), g);
                    }
                    // Complex grass halves: colour (top) like any tile but cropped; normal and
                    // specular (bottom) kept as they are, mipped as a normal map.
                    if let Some(t) = e.texture.strip_suffix(HALF_BOTTOM) {
                        let d = t.split('=').next().unwrap_or("").split(',').next().unwrap_or("");
                        let px = load_half(vfs, d, e.width, e.height, MipKind::Normal, false).unwrap_or_else(flat);
                        return (Some(chain(&px, MipKind::Normal)), chain(&flat(), MipKind::Normal), None);
                    }
                    let half_top = e.texture.ends_with(HALF_TOP);
                    let texture = e.texture.strip_suffix(HALF_TOP).unwrap_or(&e.texture);
                    // A repeat tile (`key*<ru>x<rv>`) holds its texture ru x rv times.
                    let (key, repeats) = lodforge_objlod::tiling::split_key(texture);
                    let load = |path: &str, kind: MipKind| match repeats {
                        Some(r) => load_repeated(vfs, path, e.width, e.height, r, kind),
                        None if half_top => load_half(vfs, path, e.width, e.height, kind, true),
                        None => load_tile(vfs, path, e.width, e.height, kind),
                    };
                    let sources = key.split('=').next().unwrap_or("");
                    let mut parts = sources.split(',');
                    let d = parts.next().unwrap_or("");
                    let Some(mut rgba) = load(d, MipKind::Color) else {
                        return (None, chain(&flat(), MipKind::Normal), None);
                    };
                    // `=o` (mesh packs): the shape has no alpha test; its alpha (often specular or
                    // a mask) must not cut holes on an alpha-tested page.
                    if texture.split_once('=').is_some_and(|(_, o)| o == "o") {
                        rgba.chunks_exact_mut(4).for_each(|px| px[3] = 255);
                    }
                    // `=tNNN` (tree mesh pack): the shape alpha-tests at NNN; remap alpha so that
                    // NNN lands on the page's 128 (piecewise linear, 0 and 255 kept).
                    if let Some(t) = texture.split_once("=t").and_then(|(_, t)| t.parse::<u8>().ok()).filter(|&t| t != 128 && t > 0 && t < 255) {
                        let t = f32::from(t);
                        for px in rgba.chunks_exact_mut(4) {
                            let a = f32::from(px[3]);
                            let r = if a < t { a * 128.0 / t } else { 128.0 + (a - t) * 127.0 / (255.0 - t) };
                            px[3] = r.round().clamp(0.0, 255.0) as u8;
                        }
                    }
                    // Far-LOD bake matching: the planner's per-diffuse RGB factors.
                    if let Some(t) = layout.tones.get(&d.to_lowercase()) {
                        for px in rgba.chunks_exact_mut(4) {
                            for c in 0..3 {
                                px[c] = (f32::from(px[c]) * t[c]).round().clamp(0.0, 255.0) as u8;
                            }
                        }
                    }
                    if alpha_page && formats.alpha_fill {
                        fill_average_color(&mut rgba);
                    }
                    let npath = match parts.next() {
                        Some(n) if !n.is_empty() => n.to_string(),
                        _ => normal_for(d, |n| vfs.read(&n.to_lowercase()).ok().flatten().is_some()),
                    };
                    let n = opaque(load(&npath, MipKind::Normal).unwrap_or_else(flat));
                    (Some(chain(&rgba, MipKind::Color)), chain(&n, MipKind::Normal), None)
                })
                .collect();
            let mut d_tiles = Vec::new();
            let mut n_tiles = Vec::new();
            let mut g_tiles = Vec::new();
            for (d, n, g) in chains {
                d_tiles.extend(d);
                n_tiles.push(n);
                g_tiles.extend(g);
            }
            let d = assemble(p.width, p.height, levels, [0, 0, 0, 0], &d_tiles);
            let n = assemble(p.width, p.height, levels, FLAT_NORMAL, &n_tiles);
            let g = glow_page.then(|| assemble(p.width, p.height, levels, [0, 0, 0, 255], &g_tiles));
            (pi, d, n, g)
        })
        .collect();
    let mut pages = Vec::new();
    for (pi, mut d_levels, n_levels, g_levels) in composed {
        let p = &layout.pages[pi];
        let alpha_page = is_alpha_page(&p.name);
        let glow_page = g_levels.is_some();
        if alpha_page && formats.alpha_fill {
            let page_entries: Vec<_> = layout.entries.iter().filter(|e| e.page == pi).collect();
            let tiles: Vec<(u32, u32, u32, u32)> = page_entries.iter().map(|e| (e.x, e.y, e.width, e.height)).collect();
            let density: Vec<f32> = page_entries.iter().map(|e| density_for(&e.texture)).collect();
            let dither: Vec<bool> = page_entries.iter().map(|e| billboard_dither() && is_tree_billboard(&e.texture)).collect();
            keep_alpha_coverage_with(&mut d_levels, &tiles, &density, &dither);
        }
        let d_enc = if glow_page { Encoding::Bc7 } else if alpha_page { formats.alpha_diffuse } else { formats.diffuse };
        let d_dds = encode_as(gpu, d_enc, &d_levels)?;
        let n_dds = encode_as(gpu, formats.normal, &n_levels)?;
        let g_dds = match &g_levels {
            Some(g) => Some(encode(gpu, g)?),
            None => None,
        };
        pages.push(Page { name: p.name.clone(), diffuse: d_dds, normal: n_dds, glow: g_dds, top: (d_levels.into_iter().next().unwrap().2, n_levels.into_iter().next().unwrap().2), width: p.width, height: p.height });
    }
    Ok(pages)
}

/// `textures\...\X.dds` -> `X_n.dds` path.
pub fn normal_name(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((a, b)) => format!("{a}_n.{b}"),
        None => format!("{name}_n"),
    }
}

/// `textures\...\X.dds` -> `X_g.dds` path.
pub fn glow_name(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((a, b)) => format!("{a}_g.{b}"),
        None => format!("{name}_g"),
    }
}

/// Writes the pages under `out` (a Data folder).
pub fn write(out: &Path, pages: &[Page]) -> Result<()> {
    for p in pages {
        let glow = p.glow.as_ref().map(|g| (glow_name(&p.name), g));
        for (name, bytes) in [(p.name.clone(), &p.diffuse), (normal_name(&p.name), &p.normal)].into_iter().chain(glow) {
            let path = out.join(&name);
            std::fs::create_dir_all(path.parent().unwrap())?;
            std::fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))?;
        }
    }
    Ok(())
}

/// Removes `<world>` object LOD files (`meshes\terrain\<world>\Objects\*.<SEASON>.bto`) of the
/// seasons this run does not build. The atlas pages are shared by every season, so a run that
/// rewrites them (a new layout) leaves an earlier run's season BTOs pointing at tiles that now
/// hold other textures (seen in game as teal billboard texels on Whiterun's roofs). Without the
/// season file the game uses the default season's, which this run wrote. Returns the count.
pub fn remove_stale_season_btos(out: &Path, world: &str, built: &[String]) -> Result<usize> {
    const SEASONS: [&str; 4] = ["SPR", "SUM", "AUT", "WIN"];
    let Some(dir) = find_ci(out, &format!("meshes\\terrain\\{world}\\Objects")) else { return Ok(0) };
    let mut removed = 0;
    for e in std::fs::read_dir(&dir)?.filter_map(|e| e.ok()) {
        let name = e.file_name().to_string_lossy().to_string();
        let mut parts = name.rsplitn(3, '.');
        let (Some(ext), Some(season)) = (parts.next(), parts.next()) else { continue };
        if !ext.eq_ignore_ascii_case("bto") {
            continue;
        }
        let stale = SEASONS.iter().any(|s| s.eq_ignore_ascii_case(season)) && !built.iter().any(|b| b.eq_ignore_ascii_case(season));
        if stale {
            std::fs::remove_file(e.path()).with_context(|| format!("removing {}", e.path().display()))?;
            removed += 1;
        }
    }
    Ok(removed)
}

/// Top-mip PSNR of our pages against xLODGen's (`golden` = its output Data folder).
pub fn compare_golden(golden: &Path, pages: &[Page]) {
    for p in pages {
        for (name, ours) in [(p.name.clone(), &p.top.0), (normal_name(&p.name), &p.top.1)] {
            let path = find_ci(golden, &name);
            let Some(bytes) = path.as_ref().and_then(|p| std::fs::read(p).ok()) else {
                println!("    {name}: no golden file");
                continue;
            };
            let Ok(info) = lodforge_dds::parse(&bytes) else { continue };
            let Ok(g) = lodforge_dds::decode(&bytes, &info, 0) else { continue };
            if (g.width, g.height) != (p.width, p.height) {
                println!("    {name}: golden {}x{} vs ours {}x{}", g.width, g.height, p.width, p.height);
                continue;
            }
            let m = compare(&Image::from_rgba8(g.width, g.height, &g.rgba), &Image::from_rgba8(p.width, p.height, ours));
            println!("    {name}: {:?} {}x{}, PSNR {:.2} dB, MAE {:.2}", info.format, g.width, g.height, m.psnr, m.mae);
        }
    }
}

/// Case-insensitive path lookup under `root` (golden folders use the game's mixed case).
fn find_ci(root: &Path, rel: &str) -> Option<std::path::PathBuf> {
    let mut cur = root.to_path_buf();
    for part in rel.split('\\') {
        let next = std::fs::read_dir(&cur).ok()?.filter_map(|e| e.ok()).find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(part))?;
        cur = next.path();
    }
    Some(cur)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn repeat_tiles_lay_the_texture_out() {
        // A 2 x 1 image (red, green) laid out 3 x 2 times.
        let px = [255, 0, 0, 255, 0, 255, 0, 255];
        let t = repeat_rgba(&px, 2, 1, 3, 2);
        assert_eq!(t.len(), 6 * 2 * 4);
        let at = |x: usize, y: usize| &t[(y * 6 + x) * 4..(y * 6 + x) * 4 + 4];
        for y in 0..2 {
            for x in 0..6 {
                assert_eq!(at(x, y), if x % 2 == 0 { &px[0..4] } else { &px[4..8] });
            }
        }
    }

    #[test]
    fn stale_season_btos_go() {
        let root = std::env::temp_dir().join(format!("lodforge-stale-{}", std::process::id()));
        let dir = root.join("meshes").join("terrain").join("Tamriel").join("Objects");
        std::fs::create_dir_all(&dir).unwrap();
        for f in ["Tamriel.4.4.0.bto", "Tamriel.4.4.0.SUM.bto", "Tamriel.4.4.0.WIN.bto", "Tamriel.4.-4.0.spr.bto", "Tamriel.4.4.0.SUM.txt"] {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        let n = remove_stale_season_btos(&root, "tamriel", &["WIN".to_string()]).unwrap();
        let mut left: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().to_string()).collect();
        left.sort();
        std::fs::remove_dir_all(&root).unwrap();
        assert_eq!(n, 2);
        assert_eq!(left, ["Tamriel.4.4.0.SUM.txt", "Tamriel.4.4.0.WIN.bto", "Tamriel.4.4.0.bto"]);
        assert_eq!(remove_stale_season_btos(Path::new("Z:\\nowhere"), "Tamriel", &[]).unwrap(), 0);
    }

    #[test]
    fn alpha_page_names() {
        assert!(is_alpha_page("textures\\DynDOLOD\\LOD\\DynDOLOD_Tamriel_Alpha.dds"));
        assert!(is_alpha_page("textures\\DynDOLOD\\LOD\\DynDOLOD_Tamriel_Alpha01.dds"));
        assert!(!is_alpha_page("textures\\DynDOLOD\\LOD\\DynDOLOD_Tamriel.dds"));
        assert!(!is_alpha_page("textures\\DynDOLOD\\LOD\\DynDOLOD_Tamriel_Glow.dds"));
    }

    #[test]
    fn average_color_fills_hidden_texels() {
        // Two drawn texels (red, alpha 255; blue, alpha 128), two hidden ones.
        let mut px = vec![255, 0, 0, 255, 0, 0, 255, 128, 0, 0, 0, 0, 9, 9, 9, 127];
        fill_average_color(&mut px);
        assert_eq!(&px[8..12], &[170, 0, 85, 0]);
        assert_eq!(&px[12..16], &[170, 0, 85, 127]);
        assert_eq!(&px[0..4], &[255, 0, 0, 255]);
        // Nothing hidden: unchanged.
        let mut solid = vec![1, 2, 3, 200];
        fill_average_color(&mut solid);
        assert_eq!(solid, [1, 2, 3, 200]);
    }

    #[test]
    fn alpha_mips_keep_coverage() {
        // 8x8 tile of sparse dots (one texel in four drawn, coverage 0.25): a plain downscale
        // averages them below the threshold.
        let (w, h) = (8u32, 8u32);
        let mut top = vec![0u8; (w * h * 4) as usize];
        for y in (0..h).step_by(2) {
            for x in (0..w).step_by(2) {
                top[((y * w + x) * 4 + 3) as usize] = 255;
            }
        }
        let mut levels = mip_chain_n(w, h, top, 3);
        let before = coverage(&levels[1].2, levels[1].0, (0, 0, 4, 4), 1.0);
        keep_alpha_coverage(&mut levels, &[(0, 0, w, h)]);
        let after = coverage(&levels[1].2, levels[1].0, (0, 0, 4, 4), 1.0);
        assert!(before < 0.25, "{before}");
        assert!(after >= 0.25, "{after}");
        assert_eq!(levels.len(), 3);
    }

    #[test]
    fn billboard_density_thins_every_level() {
        // A 16x16 billboard tile with graded alpha (half its texels pass the test) next to a
        // non-billboard tile: density 0.6 leaves 60% of the billboard's coverage on every level.
        let (w, h) = (32u32, 16u32);
        let mut top = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                top[((y * w + x) * 4 + 3) as usize] = ((x % 16) * 16 + y) as u8;
            }
        }
        let mut levels = mip_chain_n(w, h, top, 3);
        let before: Vec<f32> = (0..2u32).map(|k| coverage(&levels[k as usize].2, levels[k as usize].0, (0, 0, 16 >> k, 16 >> k), 1.0)).collect();
        let other = coverage(&levels[0].2, w, (16, 0, 16, 16), 1.0);
        keep_alpha_coverage_with(&mut levels, &[(0, 0, 16, 16), (16, 0, 16, 16)], &[0.6, 1.0], &[false, false]);
        for k in 0..2u32 {
            let after = coverage(&levels[k as usize].2, levels[k as usize].0, (0, 0, 16 >> k, 16 >> k), 1.0);
            assert!((after - before[0] * 0.6).abs() < 0.08, "level {k}: {after} vs {}", before[0] * 0.6);
        }
        assert_eq!(coverage(&levels[0].2, w, (16, 0, 16, 16), 1.0), other, "non-billboard tile untouched");
        assert!(is_tree_billboard(r"textures\terrain\lodgen\ulvenwald.esp\oaktall03_000a3263.dds"));
        assert!(!is_tree_billboard(r"textures\landscape\trees\pinebranch.dds"));
    }

    #[test]
    fn density_applies_to_tree_billboards_only() {
        set_billboard_density(0.7);
        assert_eq!(density_for(r"textures\terrain\lodgen\skyrim.esm\treepineforest01_0006f7dc.dds"), 0.7);
        assert_eq!(density_for(r"textures\lod\pinebranch_lod.dds"), 1.0);
        set_billboard_density(1.0);
    }

    #[test]
    fn page_mips_keep_tiles_apart() {
        // Two 4x4 tiles side by side, solid red and solid blue: every page level keeps them pure
        // (a whole-page filter mixes them into purple at the shared edge).
        let red = TileChain::new(0, 0, 4, 4, &[255, 0, 0, 255].repeat(16), 8, MipKind::Color, Edge::Wrap);
        let blue = TileChain::new(4, 0, 4, 4, &[0, 0, 255, 255].repeat(16), 8, MipKind::Color, Edge::Wrap);
        let levels = assemble(8, 4, page_levels(8, 4, 8), [0, 0, 0, 0], &[red, blue]);
        assert_eq!(levels.iter().map(|l| (l.0, l.1)).collect::<Vec<_>>(), [(8, 4), (4, 2), (2, 1), (1, 1)]);
        for (w, h, px) in &levels[..3] {
            for y in 0..*h {
                for x in 0..*w {
                    let p = &px[((y * w + x) * 4) as usize..((y * w + x) * 4 + 4) as usize];
                    let want: [u8; 4] = if x < w / 2 { [255, 0, 0, 255] } else { [0, 0, 255, 255] };
                    assert_eq!(p, want, "level {w}x{h} at {x},{y}");
                }
            }
        }
        // Below one texel per tile the page level is the box average of the level above.
        assert_eq!(&levels[3].2[..3], &[128, 0, 128]);
    }

    #[test]
    fn bc1_pages_decode() {
        let (w, h) = (16u32, 16u32);
        let rgba: Vec<u8> = (0..w * h).flat_map(|i| [(i % 16 * 16) as u8, 64, 128, 255]).collect();
        let dds = encode_bc1(&mip_chain_n(w, h, rgba, 8));
        let info = lodforge_dds::parse(&dds).unwrap();
        assert_eq!((info.width, info.height, info.mip_count, info.format), (16, 16, 5, lodforge_dds::Format::Bc1));
    }
}
