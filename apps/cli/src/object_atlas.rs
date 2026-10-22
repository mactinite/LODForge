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

const FLAT_NORMAL: [u8; 4] = [128, 128, 255, 255];

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
                    let sources = e.texture.split('=').next().unwrap_or("");
                    let mut parts = sources.split(',');
                    let d = parts.next().unwrap_or("");
                    let Some(mut rgba) = load_tile(vfs, d, e.width, e.height, MipKind::Color) else {
                        return (None, chain(&flat(), MipKind::Normal), None);
                    };
                    if alpha_page && formats.alpha_fill {
                        fill_average_color(&mut rgba);
                    }
                    let npath = match parts.next() {
                        Some(n) if !n.is_empty() => n.to_string(),
                        _ => normal_for(d, |n| vfs.read(&n.to_lowercase()).ok().flatten().is_some()),
                    };
                    let n = opaque(load_tile(vfs, &npath, e.width, e.height, MipKind::Normal).unwrap_or_else(flat));
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
            let tiles: Vec<(u32, u32, u32, u32)> = layout.entries.iter().filter(|e| e.page == pi).map(|e| (e.x, e.y, e.width, e.height)).collect();
            keep_alpha_coverage(&mut d_levels, &tiles);
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
