//! CPU bake that follows the reverse-engineered xLODGen pipeline step by step
//! (docs/re/xlodgen-terrain-textures.md, §1–§7). It is the fidelity reference: slow, 8-bit at
//! every stored step, one cell at a time.
//!
//! Per cell (W = 24 · texPx, e.g. 384 px; four W/2 quadrants; row 0 = south while building):
//! 1. Start from the default texture tiled 24x24 (each texture is Lanczos-shrunk to texPx and
//!    copied, never sampled).
//! 2. Per quadrant: the base (BTXT, else default) replaces the quadrant; each ATXT layer's opacity
//!    is splatted onto a 33x33 mask with the "square"/"diamond" 3x3 brushes (checkerboard),
//!    Lanczos-stretched to W/2 + W/64, cropped at (+3, +4), and the layer texture premultiplied
//!    by it is added into an accumulation buffer.
//! 3. out = clamp(Σ w·c) + base · (1 − clamp(Σ w)).
//! Diffuse: 4. vertex colours (33x33 Lanczos-stretched to W·33/32, centre-cropped) multiplied;
//!    5. flipped to north-up.
//! Normal: 4. the landscape `_n` composite (steps 1–3 with normal maps) is flipped north-up;
//!    5. the terrain normal (VNML, R=x G=z B=y, 33 -> 66 Lanczos, interior 64) is Lanczos-stretched
//!    to W; 6. per pixel out = n(tx·T + ty·B + tz·N) with a = n(X×N), T = n(N×a), B = −a.
//! Tile: Lanczos-resize each cell to the tile's pixels per cell, place it; diffuse only:
//! brightness/contrast then v^(1/gamma), and default tiles shrink to the default size.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rayon::prelude::*;

use lodforge_esp::terrain::{GRID, QUAD_GRID};
use lodforge_esp::{Land, LandTexture};

use crate::bake::{texture_path, TextureCache};
use crate::image::Image;

/// Repeats of a landscape texture across one cell edge (TES5/SSE).
pub const REPEATS: u32 = 24;

/// Which LOD texture is being built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Diffuse,
    Normal,
}

#[derive(Debug, Clone)]
pub struct SpecParams {
    /// Shrunk landscape texture size (golden: 16 → 384 px cells).
    pub tex_px: u32,
    pub default_diffuse: String,
    pub default_normal: String,
    pub brightness: f32,
    pub contrast: f32,
    pub gamma: f32,
    pub vertex_color_multiplier: f32,
    /// Output size of diffuse tiles with no content (None: no shrink).
    pub default_size: Option<u32>,
    /// Output size of normal tiles with no content (golden: None).
    pub default_normal_size: Option<u32>,
}

impl Default for SpecParams {
    fn default() -> Self {
        SpecParams {
            tex_px: 16,
            default_diffuse: "textures\\landscape\\dirt02.dds".into(),
            default_normal: "textures\\landscape\\dirt02_n.dds".into(),
            brightness: 0.0,
            contrast: 0.0,
            gamma: 1.25,
            vertex_color_multiplier: 1.0,
            default_size: Some(32),
            default_normal_size: None,
        }
    }
}

impl SpecParams {
    /// `texPx = nextPow2(floor(ppc / 24) + 1)` with ppc floored at 64.
    pub fn tex_px_for(max_ppc: u32) -> u32 {
        (max_ppc.max(64) / REPEATS + 1).next_power_of_two()
    }

    pub fn default_path(&self, kind: Kind) -> String {
        texture_path(match kind {
            Kind::Diffuse => &self.default_diffuse,
            Kind::Normal => &self.default_normal,
        })
    }
}

/// Landscape textures shrunk to `tex_px` (Lanczos from the full-size image), 8-bit, alpha 1.
pub struct ShrunkTextures<'a> {
    cache: &'a TextureCache<'a>,
    tex_px: u32,
    /// Round when storing the shrunk texture (diffuse pipeline) instead of truncating (normal maps).
    round: bool,
    map: Mutex<HashMap<String, Option<Arc<Image>>>>,
}

impl<'a> ShrunkTextures<'a> {
    /// Textures for the `kind` pipeline (its Lanczos downscales round for diffuse, truncate for
    /// normal maps; matched against golden).
    pub fn new(cache: &'a TextureCache<'a>, tex_px: u32, kind: Kind) -> Self {
        ShrunkTextures { cache, tex_px, round: kind == Kind::Diffuse, map: Mutex::new(HashMap::new()) }
    }

    fn get(&self, path: &str) -> Option<Arc<Image>> {
        let key = texture_path(path);
        if let Some(v) = self.map.lock().unwrap().get(&key) {
            return v.clone();
        }
        let shrunk = self.cache.get(&key).map(|t| {
            let mut img = t.mips[0].lanczos(self.tex_px, self.tex_px);
            img.quantize8_with(self.round);
            img.px.iter_mut().for_each(|p| p[3] = 1.0);
            Arc::new(img)
        });
        self.map.lock().unwrap().insert(key, shrunk.clone());
        shrunk
    }
}

pub struct SpecBaker<'a> {
    pub textures: &'a ShrunkTextures<'a>,
    pub ltex: &'a HashMap<u32, LandTexture>,
    pub params: SpecParams,
}

/// Tiles `tex` into `img` over the `w` x `h` region at (`x0`, `y0`), starting at texel (0, 0).
fn tile_into(img: &mut Image, tex: &Image, x0: u32, y0: u32, w: u32, h: u32) {
    for y in 0..h {
        for x in 0..w {
            img.px[((y0 + y) * img.width + x0 + x) as usize] = tex.get(x % tex.width, y % tex.height);
        }
    }
}

fn tiled(tex: &Image, w: u32, h: u32) -> Image {
    let mut img = Image::new(w, h);
    tile_into(&mut img, tex, 0, 0, w, h);
    img
}

fn flip_vertical(img: &Image) -> Image {
    let mut out = Image::new(img.width, img.height);
    for y in 0..img.height {
        for x in 0..img.width {
            out.px[(y * img.width + x) as usize] = img.get(x, img.height - 1 - y);
        }
    }
    out
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l < 1e-12 { v } else { [v[0] / l, v[1] / l, v[2] / l] }
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

/// The terrain-normal bake for one pixel. `t`: landscape normal-map texel (R, G, B in 0..1);
/// `terrain`: terrain normal texel (stored R = x, G = z, B = y in 0..1). Returns the encoded
/// output texel (R = x, G = z, B = y in 0..1).
pub fn bake_normal_pixel(t: [f32; 3], terrain: [f32; 3]) -> [f32; 3] {
    let t = normalize([t[0] * 2.0 - 1.0, t[1] * 2.0 - 1.0, t[2] * 2.0 - 1.0]);
    let n = normalize([terrain[0] * 2.0 - 1.0, terrain[2] * 2.0 - 1.0, terrain[1] * 2.0 - 1.0]);
    let a = normalize(cross([1.0, 0.0, 0.0], n));
    let tan = normalize(cross(n, a));
    let bit = [-a[0], -a[1], -a[2]];
    let o = normalize([
        t[0] * tan[0] + t[1] * bit[0] + t[2] * n[0],
        t[0] * tan[1] + t[1] * bit[1] + t[2] * n[1],
        t[0] * tan[2] + t[1] * bit[2] + t[2] * n[2],
    ]);
    [(o[0] + 1.0) / 2.0, (o[2] + 1.0) / 2.0, (o[1] + 1.0) / 2.0]
}

impl SpecBaker<'_> {
    fn cell_px(&self) -> u32 {
        REPEATS * self.params.tex_px
    }

    /// Texture of `kind` for an LTEX (or the default when the LTEX or its path is missing).
    pub fn texture_path(&self, ltex: Option<u32>, kind: Kind) -> String {
        let t = ltex.and_then(|id| self.ltex.get(&id));
        let p = t.map(|t| match kind {
            Kind::Diffuse => &t.diffuse,
            Kind::Normal => &t.normal,
        });
        match p {
            Some(p) if !p.is_empty() => texture_path(p),
            _ => self.params.default_path(kind),
        }
    }

    pub fn diffuse_path(&self, ltex: Option<u32>) -> String {
        self.texture_path(ltex, Kind::Diffuse)
    }

    fn texture(&self, path: &str, kind: Kind) -> Arc<Image> {
        let grey = if kind == Kind::Normal { [0.5, 0.5, 1.0, 1.0] } else { [0.5, 0.5, 0.5, 1.0] };
        self.textures
            .get(path)
            .or_else(|| self.textures.get(&self.params.default_path(kind)))
            .unwrap_or_else(|| Arc::new(Image { width: 1, height: 1, px: vec![grey] }))
    }

    /// Diffuse: a cell is "default" when every texture resolves to the default diffuse and it has
    /// no VCLR. Normal: when it has no LAND or no VNML.
    pub fn cell_is_default(&self, land: Option<&Land>) -> bool {
        let Some(land) = land else { return true };
        let default = self.params.default_path(Kind::Diffuse);
        land.colors.is_none() && land.textures().all(|t| self.diffuse_path(Some(t)) == default)
    }

    pub fn cell_is_default_normal(land: Option<&Land>) -> bool {
        land.is_none_or(|l| l.normals.is_none())
    }

    /// The 33x33 splat mask of one layer (row 0 = south), values 0..=1.
    pub fn splat_mask(opacity: &[f32]) -> Image {
        let n = 2 * QUAD_GRID as u32 - 1; // 33
        let mut m = vec![0.0f32; (n * n) as usize];
        for (p, &op) in opacity.iter().enumerate() {
            if op <= 0.0 {
                continue;
            }
            let a = (op.clamp(0.0, 1.0) * 255.0).trunc() / 255.0;
            let (col, row) = ((p % QUAD_GRID) as i32, (p / QUAD_GRID) as i32);
            let square = p % 2 == 0;
            for dy in -1i32..=1 {
                for dx in -1i32..=1 {
                    let w = match (dx, dy) {
                        (0, 0) => 1.0,
                        (0, _) | (_, 0) => 128.0 / 255.0,
                        _ if square => 128.0 / 255.0,
                        _ => 0.0,
                    };
                    let (x, y) = (2 * col + dx, 2 * row + dy);
                    if x < 0 || y < 0 || x >= n as i32 || y >= n as i32 {
                        continue;
                    }
                    let i = (y as u32 * n + x as u32) as usize;
                    // Saturating 8-bit add.
                    m[i] = ((m[i] + w * a) * 255.0).trunc().min(255.0) / 255.0;
                }
            }
        }
        Image { width: n, height: n, px: m.into_iter().map(|v| [v, v, v, v]).collect() }
    }

    /// Steps 1–3: the layered landscape composite of `kind`, south-up, 8-bit.
    fn composite(&self, land: Option<&Land>, kind: Kind) -> Image {
        let w = self.cell_px();
        let q = w / 2;
        let default_tex = self.texture(&self.params.default_path(kind), kind);
        let mut cell = tiled(&default_tex, w, w);
        let Some(land) = land else { return cell };
        let mut acc = Image::new(w, w);
        acc.px.iter_mut().for_each(|p| *p = [0.0; 4]);
        let mut any_layer = false;
        for (qi, quad) in land.quadrants.iter().enumerate() {
            let (ox, oy) = ((qi as u32 % 2) * q, (qi as u32 / 2) * q);
            if quad.base.is_none() && quad.layers.is_empty() {
                continue; // stays default
            }
            // Slot 0: base (or default) replaces the quadrant.
            let base = self.texture(&self.texture_path(quad.base, kind), kind);
            tile_into(&mut cell, &base, ox, oy, q, q);
            // Slots >= 1: one layer per index; a later duplicate overwrites an earlier one.
            let mut slots: Vec<&lodforge_esp::Layer> = Vec::new();
            for l in &quad.layers {
                if let Some(s) = slots.iter_mut().find(|s| s.layer == l.layer) {
                    *s = l;
                } else {
                    slots.push(l);
                }
            }
            for layer in slots {
                any_layer = true;
                let tex = self.texture(&self.texture_path(Some(layer.ltex), kind), kind);
                let big = q + q / 32;
                let mask = Self::splat_mask(&layer.opacity).lanczos(big, big);
                let (cx, cy) = ((big - q) / 2, (big - q) / 2 + 1);
                for y in 0..q {
                    for x in 0..q {
                        let m8 = (mask.get(cx + x, cy + y)[0].clamp(0.0, 1.0) * 255.0).trunc();
                        let t = tex.get(x % tex.width, y % tex.height);
                        let i = ((oy + y) * w + ox + x) as usize;
                        let a = &mut acc.px[i];
                        for c in 0..3 {
                            // Integer-truncated premultiply, saturating 8-bit add.
                            let v = ((t[c] * 255.0).round() * m8 / 255.0).trunc();
                            a[c] = ((a[c] * 255.0 + v).min(255.0)) / 255.0;
                        }
                        a[3] = ((a[3] * 255.0 + m8).min(255.0)) / 255.0;
                    }
                }
            }
        }
        if any_layer {
            for (o, a) in cell.px.iter_mut().zip(&acc.px) {
                for c in 0..3 {
                    o[c] = (a[c] + o[c] * (1.0 - a[3])).clamp(0.0, 1.0);
                }
            }
            cell.quantize8();
        }
        cell
    }

    /// Renders one diffuse cell at full internal resolution, north-up.
    pub fn render_cell(&self, land: Option<&Land>) -> Image {
        let w = self.cell_px();
        let mut cell = self.composite(land, Kind::Diffuse);
        if let Some(vc) = land.and_then(|l| l.colors.as_ref()) {
            let mut v = Image::new(GRID as u32, GRID as u32);
            for (i, c) in vc.iter().enumerate().take(GRID * GRID) {
                v.px[i] = [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, 1.0];
            }
            let big = w * 33 / 32;
            let mut v = v.lanczos(big, big);
            v.quantize8(); // stored as an 8-bit image
            let off = (big - w) / 2;
            let k = self.params.vertex_color_multiplier;
            for y in 0..w {
                for x in 0..w {
                    let vcp = v.get(off + x, off + y);
                    let o = &mut cell.px[(y * w + x) as usize];
                    for c in 0..3 {
                        let m = o[c] * vcp[c].clamp(0.0, 1.0);
                        o[c] = m + (o[c] - m) * (1.0 - k);
                    }
                }
            }
            cell.quantize8();
        }
        flip_vertical(&cell)
    }

    /// The 64x64 terrain normal of a cell (north-up; R = x, G = z, B = y), before the bake.
    pub fn terrain_normal(land: Option<&Land>) -> Image {
        let Some(vn) = land.and_then(|l| l.normals.as_ref()) else {
            let mut img = Image::new(64, 64);
            img.px.iter_mut().for_each(|p| *p = [128.0 / 255.0, 1.0, 128.0 / 255.0, 1.0]);
            return img;
        };
        let mut v = Image::new(GRID as u32, GRID as u32);
        for row in 0..GRID {
            for col in 0..GRID {
                let n = vn[row * GRID + col];
                // Signed bytes stored offset-binary (+0x80).
                let enc = |b: i8| f32::from((b as u8).wrapping_add(0x80)) / 255.0;
                // North-up: image row = 32 - VNML row.
                v.px[(GRID - 1 - row) * GRID + col] = [enc(n[0]), enc(n[2]), enc(n[1]), 1.0];
            }
        }
        let mut big = v.lanczos(66, 66);
        big.quantize8();
        // Interior 64x64 (vertex k -> pixel 2k); offsets 0 and 2 score ~5 dB worse vs golden.
        big.crop(1, 1, 64, 64)
    }

    /// Renders one baked normal cell at full internal resolution, north-up.
    pub fn render_cell_normal(&self, land: Option<&Land>) -> Image {
        let w = self.cell_px();
        let maps = flip_vertical(&self.composite(land, Kind::Normal));
        let mut terrain = Self::terrain_normal(land).lanczos(w, w);
        terrain.quantize8();
        let mut out = Image::new(w, w);
        for (o, (t, n)) in out.px.iter_mut().zip(maps.px.iter().zip(&terrain.px)) {
            let b = bake_normal_pixel([t[0], t[1], t[2]], [n[0], n[1], n[2]]);
            *o = [b[0], b[1], b[2], 1.0];
        }
        out.quantize8();
        out
    }

    /// Bakes a `level`-cell tile of `kind` at `ppc` output pixels per cell. Returns the image and
    /// whether it is a default (content-free) tile, already shrunk to the default size.
    pub fn bake_tile_kind(&self, lands: &HashMap<(i32, i32), Land>, origin: (i32, i32), level: i32, ppc: u32, kind: Kind) -> (Image, bool) {
        let size = ppc * level as u32;
        let cell_image = |land: Option<&Land>| -> Image {
            let img = match kind {
                Kind::Diffuse => self.render_cell(land),
                Kind::Normal => self.render_cell_normal(land),
            };
            let mut img = img.lanczos(ppc, ppc);
            img.quantize8_with(kind == Kind::Diffuse);
            img
        };
        // Cells without LAND all render identically: do it once.
        let no_land = std::sync::OnceLock::new();
        let render = |land: Option<&Land>| -> Image {
            match land {
                Some(_) => cell_image(land),
                None => no_land.get_or_init(|| cell_image(None)).clone(),
            }
        };
        let cells: Vec<(i32, i32, Image, bool)> = (0..level)
            .flat_map(|dy| (0..level).map(move |dx| (dx, dy)))
            .collect::<Vec<_>>()
            .par_iter()
            .map(|&(dx, dy)| {
                let land = lands.get(&(origin.0 + dx, origin.1 + dy));
                let def = match kind {
                    Kind::Diffuse => self.cell_is_default(land),
                    Kind::Normal => Self::cell_is_default_normal(land),
                };
                (dx, dy, render(land), def)
            })
            .collect();
        let mut tile = Image::new(size, size);
        let mut is_default = true;
        for (dx, dy, img, def) in cells {
            is_default &= def;
            let (x0, y0) = (dx as u32 * ppc, (level - 1 - dy) as u32 * ppc);
            for y in 0..ppc {
                for x in 0..ppc {
                    tile.px[((y0 + y) * size + x0 + x) as usize] = img.get(x, y);
                }
            }
        }
        let p = &self.params;
        if kind == Kind::Diffuse && (p.brightness != 0.0 || p.contrast != 0.0 || p.gamma != 1.0) {
            for px in &mut tile.px {
                for c in px.iter_mut().take(3) {
                    let mut v = *c;
                    if p.brightness != 0.0 || p.contrast != 0.0 {
                        v = v * (1.0 + p.contrast / 100.0) + p.brightness / 100.0;
                    }
                    if p.gamma != 1.0 {
                        v = v.max(0.0).powf(1.0 / p.gamma);
                    }
                    *c = v;
                }
            }
            tile.quantize8();
        }
        let default_size = match kind {
            Kind::Diffuse => p.default_size,
            Kind::Normal => p.default_normal_size,
        };
        if is_default {
            if let Some(s) = default_size {
                tile = tile.lanczos(s, s);
                tile.quantize8_with(true);
            }
        }
        (tile, is_default)
    }

    /// A default (content-free) diffuse tile for the stitched `hq` filter, matching what the GPU
    /// path gives a tile of default cells: the default cell resized to `base_ppc` and rounded
    /// (the stitched base), then resized in linear light straight to the output size. Every cell
    /// is the same, so the stitched image is periodic and one cell resized with wrapped edges,
    /// repeated, is the whole tile. The output size is the default-tile size (32 px) when set,
    /// else `ppc` per cell. Returns (size, RGBA8) with the tone curve applied.
    pub fn default_tile_hq(&self, level: i32, ppc: u32, base_ppc: u32) -> (u32, Vec<u8>) {
        use lodforge_dds::mips::{Edge, MipKind, resize_rgba8};
        let level = level as u32;
        let size = self.params.default_size.unwrap_or(ppc * level).max(level);
        let per = size / level;
        let mut cell = self.render_cell(None).lanczos(base_ppc, base_ppc);
        cell.quantize8_with(true);
        let small = resize_rgba8(base_ppc, base_ppc, &cell.to_rgba8(), (per, per), MipKind::Color, Edge::Wrap);
        let p = &self.params;
        let tone = |c: u8| -> u8 {
            let mut v = f32::from(c) / 255.0;
            if p.brightness != 0.0 || p.contrast != 0.0 {
                v = v * (1.0 + p.contrast / 100.0) + p.brightness / 100.0;
            }
            if p.gamma != 1.0 {
                v = v.max(0.0).powf(1.0 / p.gamma);
            }
            (v.clamp(0.0, 1.0) * 255.0).round() as u8
        };
        let side = per * level;
        let mut out = vec![0u8; (side * side * 4) as usize];
        for y in 0..side {
            for x in 0..side {
                let s = (((y % per) * per + x % per) * 4) as usize;
                let d = ((y * side + x) * 4) as usize;
                for c in 0..3 {
                    out[d + c] = tone(small[s + c]);
                }
                out[d + 3] = 255;
            }
        }
        (side, out)
    }

    /// Diffuse tile (see [`SpecBaker::bake_tile_kind`]).
    pub fn bake_tile(&self, lands: &HashMap<(i32, i32), Land>, origin: (i32, i32), level: i32, ppc: u32) -> (Image, bool) {
        self.bake_tile_kind(lands, origin, level, ppc, Kind::Diffuse)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_terrain_keeps_map_axes() {
        // Flat terrain (N = up) leaves a tangent-space normal unchanged: x -> east, y -> north.
        let flat = [0.5, 1.0, 0.5];
        let up = bake_normal_pixel([0.5, 0.5, 1.0], flat);
        assert!((up[0] - 0.5).abs() < 1e-6 && (up[1] - 1.0).abs() < 1e-6 && (up[2] - 0.5).abs() < 1e-6, "{up:?}");
        let east = bake_normal_pixel([1.0, 0.5, 0.5], flat);
        assert!((east[0] - 1.0).abs() < 1e-6 && (east[1] - 0.5).abs() < 1e-6, "{east:?}");
        let north = bake_normal_pixel([0.5, 1.0, 0.5], flat);
        assert!((north[2] - 1.0).abs() < 1e-6, "{north:?}");
    }
}
