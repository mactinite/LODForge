// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! TexGen-compatible billboards rendered on the GPU (clean room, docs/re/dyndolod-spec.md
//! "TexGen billboards and LOD textures"): size estimate, silhouette trim pass, final MSAA render,
//! binary alpha, colour bleed, BC7 DDS plus the `.txt` sidecar LODGen and DynDOLOD read.

pub mod render;
pub mod stitch;
pub mod scene;

use std::collections::HashMap;

pub use render::{Job, Light, Pass, Renderer, TexData, View};
pub use scene::{LoadOptions, Scene};

#[derive(Debug, Clone, Copy)]
pub struct Settings {
    /// `BillboardsUnitsPerPixel / 10`.
    pub units_per_pixel: f32,
    pub min_size: u32,
    pub max_size: u32,
    pub light: Light,
    /// MSAA coverage threshold for opaque texels (`TreeMSAlphaThreshold`, 0..255).
    pub coverage_threshold: u8,
}

/// Billboard resolution multiplier (f32 bits) applied by [Settings::default]: 2.0 renders twice
/// the pixels per side (TexGen units per pixel / 2, size cap x 2).
static BILLBOARD_SCALE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x3F80_0000);

/// Sets the billboard resolution multiplier for every [Settings::default] made afterwards
/// (clamped to 0.25..4).
pub fn set_billboard_scale(scale: f32) {
    BILLBOARD_SCALE.store(scale.clamp(0.25, 4.0).to_bits(), std::sync::atomic::Ordering::Relaxed);
}

pub fn billboard_scale() -> f32 {
    f32::from_bits(BILLBOARD_SCALE.load(std::sync::atomic::Ordering::Relaxed))
}

impl Default for Settings {
    /// TexGen defaults for tree billboards (UPP 11.0, 32..1024 px, Direct 1.35, Ambient 0.55,
    /// light from the top), times [billboard_scale].
    fn default() -> Self {
        let k = billboard_scale();
        Settings { units_per_pixel: 11.0 / k, min_size: 32, max_size: ((1024.0 * k) as u32).clamp(256, 4096), light: Light { direct: 1.35, ambient: 0.55, dir: [0.0, 0.0, 1.0], pbr_scale: 0.7 }, coverage_threshold: 128 }
    }
}

#[derive(Debug, Clone)]
pub struct Billboard {
    pub width_px: u32,
    pub height_px: u32,
    /// RGBA8, binary alpha, transparent texels bled with the average opaque colour.
    pub rgba: Vec<u8>,
    /// The `_n` normal texture (same size), when asked for.
    pub normal: Option<Vec<u8>>,
    pub width: f32,
    pub depth: f32,
    pub height: f32,
    pub shift_z: f32,
}

fn next_pow2(v: u32) -> u32 {
    v.max(1).next_power_of_two()
}

/// TexGen's `unmulta` test on a resolved texel: fully covered, or coverage at the threshold with
/// some colour (a black partially covered texel is dropped).
fn opaque(px: &[u8], thr: u8) -> bool {
    px[3] == 255 || (px[3] >= thr.max(1) && (px[0] | px[1] | px[2]) != 0)
}

/// Opaque bounding box (left col, right col, top row, bottom row) of a coverage image.
fn opaque_box(rgba: &[u8], w: u32, h: u32, thr: u8) -> Option<(u32, u32, u32, u32)> {
    let (mut l, mut r, mut t, mut b) = (u32::MAX, 0, u32::MAX, 0);
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            if opaque(&rgba[i..i + 4], thr) {
                l = l.min(x);
                r = r.max(x);
                t = t.min(y);
                b = b.max(y);
            }
        }
    }
    (l != u32::MAX).then_some((l, r, t, b))
}

/// Pre-pass view of one yaw: the geometry's horizontal range `[x0, x1]` and `[z0, z1]` at
/// `ceil(h * (x1 - x0) / (z1 - z0))` x `h` pixels.
fn prepass_view(scene: &Scene, yaw: f32, z0: f32, z1: f32, h: u32) -> View {
    let (x0, x1) = view_range(scene, yaw);
    let w = ((h as f32) * (x1 - x0) / (z1 - z0)).ceil().max(1.0) as u32;
    View { yaw_deg: yaw, x0, x1, z0, z1, width: w, height: h }
}

/// The opaque silhouette of a pre-pass image as (left x, right x, bottom z, top z) in view units.
fn silhouette(img: &[u8], v: &View, thr: u8) -> Option<(f32, f32, f32, f32)> {
    let (l, r, t, b) = opaque_box(img, v.width, v.height, thr)?;
    let px = (v.x1 - v.x0) / v.width as f32;
    let pz = (v.z1 - v.z0) / v.height as f32;
    Some((v.x0 + l as f32 * px, v.x0 + (r + 1) as f32 * px, v.z1 - (b + 1) as f32 * pz, v.z1 - t as f32 * pz))
}

/// Horizontal range of the scene in a view (`yaw` degrees).
fn view_range(scene: &Scene, yaw: f32) -> (f32, f32) {
    scene.meshes.iter().flat_map(|m| m.positions.iter()).map(|p| render::view_x(*p, yaw)).fold((f32::MAX, f32::MIN), |(a, b), x| (a.min(x), b.max(x)))
}

/// Pixel height TexGen gives a billboard: `round(modelHeight * scale / UPP)` rounded up to a power
/// of two and clamped, where `modelHeight` is the z extent of the rendered geometry.
pub fn pixel_height(model_height: f32, scale: f32, s: &Settings) -> u32 {
    let p0 = (model_height * scale / s.units_per_pixel).round() as u32;
    next_pow2(p0).clamp(s.min_size, s.max_size)
}

/// One billboard to make: the scene, its textures and the reference scale used for sizing.
#[derive(Clone, Copy)]
pub struct Item<'a> {
    pub scene: &'a Scene,
    pub textures: &'a HashMap<String, TexData>,
    pub scale: f32,
}

/// Bottom and top z, and pixel height, of an item (`None` without geometry).
fn item_frame(it: &Item, s: &Settings) -> Option<(f32, f32, u32)> {
    let (lo, hi) = it.scene.bounds()?;
    (hi[2] > lo[2]).then(|| (lo[2], hi[2], pixel_height(hi[2] - lo[2], it.scale, s)))
}

/// Final renders (diffuse, then normal bake when `normals`) of `views` in one GPU round trip, with
/// TexGen's alpha and bleed applied.
fn final_images(gpu: &lodforge_gpu::Gpu, renderer: &Renderer, views: &[(Item, View)], s: &Settings, normals: bool) -> Vec<(Vec<u8>, Option<Vec<u8>>)> {
    let mut jobs = Vec::new();
    for (it, v) in views {
        jobs.push(render::Job { scene: it.scene, textures: it.textures, view: *v, light: s.light, pass: render::Pass::Diffuse });
        if normals {
            jobs.push(render::Job { scene: it.scene, textures: it.textures, view: *v, light: s.light, pass: render::Pass::Normal });
        }
    }
    let mut imgs = renderer.render_many(gpu, &jobs).into_iter();
    views
        .iter()
        .map(|(_, v)| {
            let mut img = imgs.next().unwrap_or_default();
            // Supersampled renders carry real edge coverage: keep it (see finish_alpha_soft).
            let finish = |img: &mut Vec<u8>| if render::supersample() > 1 { finish_alpha_soft(img, v.width) } else { finish_alpha(img, v.width, s.coverage_threshold) };
            finish(&mut img);
            let normal = normals.then(|| {
                let mut n = imgs.next().unwrap_or_default();
                finish(&mut n);
                n
            });
            (img, normal)
        })
        .collect()
}

/// Renders a standard (front view) tree/grass billboard. `scale` is the largest reference scale
/// of the base (or the type's fallback multiplier). See [`make_billboards`].
pub fn make_billboard(gpu: &lodforge_gpu::Gpu, renderer: &Renderer, scene: &Scene, tex: &HashMap<String, TexData>, scale: f32, s: &Settings, normals: bool) -> Option<Billboard> {
    make_billboards(gpu, renderer, &[Item { scene, textures: tex, scale }], s, normals).pop().flatten()
}

/// Standard billboards for a batch of items in two GPU round trips (all pre-passes, then all final
/// renders).
///
/// Framing (spec Â§2.1): a pre-pass of the front view framed on the geometry finds the opaque
/// silhouette; the final render spans it vertically and symmetrically about the origin
/// horizontally, `4 * ceil(h * 2 * halfWidth / height / 4)` pixels wide. `Depth` comes from the
/// same measurement of the side view.
pub fn make_billboards(gpu: &lodforge_gpu::Gpu, renderer: &Renderer, items: &[Item], s: &Settings, normals: bool) -> Vec<Option<Billboard>> {
    let frames: Vec<Option<(f32, f32, u32)>> = items.iter().map(|it| item_frame(it, s)).collect();
    let mut pre = Vec::new();
    for (it, f) in items.iter().zip(&frames) {
        if let Some((z0, z1, h)) = *f {
            for yaw in [0.0, 90.0] {
                let v = prepass_view(it.scene, yaw, z0, z1, h);
                pre.push(render::Job { scene: it.scene, textures: it.textures, view: v, light: s.light, pass: render::Pass::Diffuse });
            }
        }
    }
    let pre_imgs = renderer.render_many(gpu, &pre);
    // Per item: (front silhouette, side silhouette) of the pre-passes.
    let mut sil = pre.chunks(2).zip(pre_imgs.chunks(2)).map(|(j, i)| (silhouette(&i[0], &j[0].view, s.coverage_threshold), silhouette(&i[1], &j[1].view, s.coverage_threshold)));
    let mut finals = Vec::new();
    let mut meta = Vec::new();
    for (it, f) in items.iter().zip(&frames) {
        let Some((_, _, h)) = *f else {
            meta.push(None);
            continue;
        };
        let (front, side) = sil.next().unwrap_or((None, None));
        let Some((xl, xr, zb, zt)) = front else {
            meta.push(None);
            continue;
        };
        let height = zt - zb;
        let quantise = |half: f32| (4.0 * ((h as f32) * 2.0 * half / height / 4.0).ceil()).max(4.0) as u32;
        let w = quantise(xl.abs().max(xr.abs()));
        let width = w as f32 * height / h as f32;
        let depth = side.map(|(l, r, _, _)| quantise(l.abs().max(r.abs())) as f32 * height / h as f32).unwrap_or(width);
        finals.push((*it, View { yaw_deg: 0.0, x0: -width / 2.0, x1: width / 2.0, z0: zb, z1: zt, width: w, height: h }));
        meta.push(Some((w, h, width, depth, height, zb)));
    }
    let mut imgs = final_images(gpu, renderer, &finals, s, normals).into_iter();
    meta.into_iter()
        .map(|m| {
            let (w, h, width, depth, height, shift_z) = m?;
            let (rgba, normal) = imgs.next()?;
            Some(Billboard { width_px: w, height_px: h, rgba, normal, width, depth, height, shift_z })
        })
        .collect()
}

/// One view of a rendered (HD / trunk) billboard: trimmed tightly and asymmetrically.
#[derive(Debug, Clone)]
pub struct ViewImage {
    pub width_px: u32,
    pub height_px: u32,
    pub rgba: Vec<u8>,
    pub normal: Vec<u8>,
    /// `Width_n`: the trimmed silhouette width in model units (the texture spans exactly it).
    pub width: f32,
    /// `Height_n`.
    pub height: f32,
    /// `Adjust_n`: horizontal offset of the silhouette centre from the origin, positive toward
    /// image left (the golden trunks' sign).
    pub adjust: f32,
    pub shift_z: f32,
}

/// Views of trunk billboards: from the back (diffuse 27.7 dB and 97.5% alpha agreement on the
/// golden trunks against 20.9 dB and 84.5% from 0Â° / 90Â°: single-sided cards are culled differently
/// from the two sides).
pub const TRUNK_YAWS: [f32; 2] = [180.0, 270.0];

/// A rendered billboard (`_1` front and `_2` side views, each with `_n`), as TexGen makes for
/// `Render\Skyrim\Billboards\**\*.nif` (hybrid tree trunks) and HD tree/grass billboards.
#[derive(Debug, Clone)]
pub struct RenderedBillboard {
    pub views: [ViewImage; 2],
    /// `Width`: the front texture's width at the front view's scale (`w1 * Height_1 / h`).
    pub width: f32,
    pub depth: f32,
    pub height: f32,
    pub shift_z: f32,
}

/// One rendered billboard; see [`make_rendered_billboards`].
pub fn make_rendered_billboard(gpu: &lodforge_gpu::Gpu, renderer: &Renderer, scene: &Scene, tex: &HashMap<String, TexData>, scale: f32, s: &Settings, yaws: [f32; 2]) -> Option<RenderedBillboard> {
    make_rendered_billboards(gpu, renderer, &[Item { scene, textures: tex, scale }], s, yaws).pop().flatten()
}

/// Both views of rendered billboards for a batch of items in two GPU round trips (spec Â§1.2 /
/// Â§2.1, fitted on the 119 golden trunk billboards): per view a pre-pass framed on the geometry,
/// then a final render framed exactly on the trimmed silhouette at
/// `4 * ceil(h * Width_n / Height_n / 4)` x `h` pixels. `yaws`: front and side view; TexGen renders
/// trunk billboards from the back, [`TRUNK_YAWS`].
pub fn make_rendered_billboards(gpu: &lodforge_gpu::Gpu, renderer: &Renderer, items: &[Item], s: &Settings, yaws: [f32; 2]) -> Vec<Option<RenderedBillboard>> {
    let frames: Vec<Option<(f32, f32, u32)>> = items.iter().map(|it| item_frame(it, s)).collect();
    let mut pre = Vec::new();
    for (it, f) in items.iter().zip(&frames) {
        if let Some((z0, z1, h)) = *f {
            for yaw in yaws {
                pre.push(render::Job { scene: it.scene, textures: it.textures, view: prepass_view(it.scene, yaw, z0, z1, h), light: s.light, pass: render::Pass::Diffuse });
            }
        }
    }
    let pre_imgs = renderer.render_many(gpu, &pre);
    let mut sil = pre.iter().zip(&pre_imgs).map(|(j, i)| silhouette(i, &j.view, s.coverage_threshold));
    let mut finals = Vec::new();
    // Per item and view: (w, h, Width_n, Height_n, Adjust_n, ShiftZ).
    let mut meta: Vec<Option<[(u32, u32, f32, f32, f32, f32); 2]>> = Vec::new();
    for (it, f) in items.iter().zip(&frames) {
        let Some((_, _, h)) = *f else {
            meta.push(None);
            continue;
        };
        let sils = [sil.next().flatten(), sil.next().flatten()];
        let (Some(a), Some(b)) = (sils[0], sils[1]) else {
            meta.push(None);
            continue;
        };
        let mut m = [(0, 0, 0.0, 0.0, 0.0, 0.0); 2];
        for (k, (xl, xr, zb, zt)) in [a, b].into_iter().enumerate() {
            let (width, height) = (xr - xl, zt - zb);
            let w = (4.0 * ((h as f32) * width / height / 4.0).ceil()).max(4.0) as u32;
            finals.push((*it, View { yaw_deg: yaws[k], x0: xl, x1: xr, z0: zb, z1: zt, width: w, height: h }));
            m[k] = (w, h, width, height, -(xl + xr) / 2.0, zb);
        }
        meta.push(Some(m));
    }
    let mut imgs = final_images(gpu, renderer, &finals, s, true).into_iter();
    meta.into_iter()
        .map(|m| {
            let m = m?;
            let views = m.map(|(w, h, width, height, adjust, shift_z)| {
                let (rgba, normal) = imgs.next().unwrap_or_default();
                ViewImage { width_px: w, height_px: h, rgba, normal: normal.unwrap_or_default(), width, height, adjust, shift_z }
            });
            let h = views[0].height_px;
            let width = views[0].width_px as f32 * views[0].height / h as f32;
            // Side silhouette extent about the origin, not quantised (fits the golden trunks).
            let side = &views[1];
            let depth = 2.0 * (side.adjust - side.width / 2.0).abs().max((side.adjust + side.width / 2.0).abs());
            let (height, shift_z) = (views[0].height, views[0].shift_z);
            Some(RenderedBillboard { views, width, depth, height, shift_z })
        })
        .collect()
}


/// TexGen's `BleedRadius` default.
pub const BLEED_RADIUS: i32 = 2;

/// TexGen's `unmulta` + bleed (spec Â§2.3): texels passing the opacity test become opaque with their
/// colour un-premultiplied, the rest transparent. A transparent texel then takes the mean colour of
/// the opaque texels within [`BLEED_RADIUS`] (a square, edges clamped), or the average opaque
/// colour of the image when there are none.
pub fn finish_alpha(img: &mut [u8], width: u32, thr: u8) {
    finish_alpha_with(img, width, thr, false);
}

/// Coverage below this is treated as empty by [finish_alpha_soft] (stray single samples).
pub const SOFT_ALPHA_MIN: u8 = 16;

/// [finish_alpha] that keeps the rendered coverage as alpha instead of cutting it to 0 / 255:
/// texels with any real coverage are un-premultiplied and keep their partial alpha, so the
/// game's alpha test on the bilinearly filtered texture traces a smooth silhouette between
/// texels instead of a staircase, and thin foliage fades in the mips instead of breaking up into
/// pixel noise. TexGen cuts at the coverage threshold.
pub fn finish_alpha_soft(img: &mut [u8], width: u32) {
    finish_alpha_with(img, width, SOFT_ALPHA_MIN, true);
}

fn finish_alpha_with(img: &mut [u8], width: u32, thr: u8, soft: bool) {
    let mut sum = [0u64; 3];
    let mut n = 0u64;
    for px in img.chunks_exact_mut(4) {
        let a = px[3];
        if if soft { a >= thr } else { opaque(px, thr) } {
            for c in &mut px[..3] {
                *c = ((u32::from(*c) * 255 + u32::from(a) / 2) / u32::from(a)).min(255) as u8;
            }
            if !soft {
                px[3] = 255;
            }
            for k in 0..3 {
                sum[k] += u64::from(px[k]);
            }
            n += 1;
        } else {
            px[3] = 0;
        }
    }
    let round = |s: u64, k: u64| ((s + k / 2) / k.max(1)) as u8;
    let avg = if n > 0 { [round(sum[0], n), round(sum[1], n), round(sum[2], n)] } else { [0; 3] };
    let (w, h) = (width.max(1) as i32, (img.len() / 4) as i32 / width.max(1) as i32);
    let src = img.to_vec();
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            if src[i + 3] != 0 {
                continue;
            }
            let (mut s, mut k) = ([0u64; 3], 0u64);
            for dy in -BLEED_RADIUS..=BLEED_RADIUS {
                for dx in -BLEED_RADIUS..=BLEED_RADIUS {
                    let j = (((y + dy).clamp(0, h - 1) * w + (x + dx).clamp(0, w - 1)) * 4) as usize;
                    if (dx != 0 || dy != 0) && src[j + 3] != 0 {
                        for c in 0..3 {
                            s[c] += u64::from(src[j + c]);
                        }
                        k += 1;
                    }
                }
            }
            let c = if k > 0 { [round(s[0], k), round(s[1], k), round(s[2], k)] } else { avg };
            img[i..i + 3].copy_from_slice(&c);
        }
    }
}

/// What a billboard was made from, for the `.txt` sidecar: the `Model=` value, the CRC32 of the
/// NIF bytes and every texture used with the CRC32 of its file.
#[derive(Debug, Clone, Default)]
pub struct Source {
    pub model: String,
    pub model_crc: u32,
    pub textures: Vec<(String, u32)>,
}

/// Every texture the scene's shapes name (texture-set slots 0..2, original case), in first-use
/// order without repeats. TexGen's own order looks hash-based; DynDOLOD only checks membership.
pub fn texture_names(scene: &Scene) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for m in &scene.meshes {
        for t in &m.texture_names {
            if !out.iter().any(|o| o.eq_ignore_ascii_case(t)) {
                out.push(t.clone());
            }
        }
    }
    out
}

fn sidecar_tail(src: &Source) -> String {
    let mut s = format!("Model={}\r\nModelCRC32={:08X}\r\n", src.model, src.model_crc);
    for (k, (t, crc)) in src.textures.iter().enumerate() {
        s.push_str(&format!("Texture_{0}={t}\r\nTextureCRC32_{0}={crc:08X}\r\n", k + 1));
    }
    s.push_str("\r\n");
    s
}

/// The `.txt` sidecar of a standard billboard (spec Â§1.3) as TexGen writes it: CRLF lines, an empty
/// last line, `CRC32` of the final DDS.
pub fn sidecar(b: &Billboard, scale: f32, crc: u32, src: &Source) -> String {
    format!(
        "[LOD]\r\nWidth={:.6}\r\nDepth={:.6}\r\nHeight={:.6}\r\nShiftX=0.000000\r\nShiftY=0.000000\r\nShiftZ={:.6}\r\nScale={:.6}\r\nCRC32={crc:08X}\r\n{}",
        b.width,
        b.depth,
        b.height,
        b.shift_z,
        scale,
        sidecar_tail(src)
    )
}

/// The `.txt` sidecar of a rendered billboard: per-view `Width_n`, `Height_n`, `Adjust_n` and
/// `CRC32_n` (of `<stem>_n.dds`) instead of `CRC32`.
pub fn rendered_sidecar(b: &RenderedBillboard, scale: f32, complex: Option<bool>, crcs: [u32; 2], src: &Source) -> String {
    let mut s = format!("[LOD]\r\nWidth={:.6}\r\nDepth={:.6}\r\nHeight={:.6}\r\nShiftX=0.000000\r\nShiftY=0.000000\r\nShiftZ={:.6}\r\nScale={:.6}\r\n", b.width, b.depth, b.height, b.shift_z, scale);
    if let Some(c) = complex {
        s.push_str(if c { "Complex=True\r\n" } else { "Complex=False\r\n" });
    }
    for (k, v) in b.views.iter().enumerate() {
        s.push_str(&format!("Width_{0}={1:.6}\r\nHeight_{0}={2:.6}\r\nAdjust_{0}={3:.6}\r\nCRC32_{0}={4:08X}\r\n", k + 1, v.width, v.height, v.adjust, crcs[k]));
    }
    s.push_str(&sidecar_tail(src));
    s
}

/// Image comparison of two same-size RGBA8 billboards (`a` the reference).
#[derive(Debug, Clone, Copy, Default)]
pub struct Comparison {
    /// PSNR (dB) of RGB over texels opaque in `a` (texels transparent in `b` count with their
    /// bleed colour, so alpha differences are included).
    pub psnr_ref_opaque: f64,
    /// PSNR (dB) of RGB over texels opaque in both (shading only).
    pub psnr_both_opaque: f64,
    /// Fraction of texels whose binary alpha agrees.
    pub alpha_agreement: f64,
    /// PSNR (dB) of the mean RGB of 4x4 blocks over texels opaque in both: shading without
    /// texel-level noise (a diagnostic, not a parity measure).
    pub psnr_blocks: f64,
}

/// [`compare`] for images `width` texels wide, with the 4x4 block measure.
pub fn compare_sized(a: &[u8], b: &[u8], width: u32) -> Comparison {
    let mut c = compare(a, b);
    let (w, h) = (width as usize, a.len() / 4 / width.max(1) as usize);
    let (mut se, mut n) = (0f64, 0u64);
    for by in (0..h).step_by(4) {
        for bx in (0..w).step_by(4) {
            let (mut sa, mut sb, mut k) = ([0f64; 3], [0f64; 3], 0f64);
            for y in by..(by + 4).min(h) {
                for x in bx..(bx + 4).min(w) {
                    let i = (y * w + x) * 4;
                    if a[i + 3] >= 128 && b[i + 3] >= 128 {
                        for ch in 0..3 {
                            sa[ch] += f64::from(a[i + ch]);
                            sb[ch] += f64::from(b[i + ch]);
                        }
                        k += 1.0;
                    }
                }
            }
            if k > 0.0 {
                se += (0..3).map(|ch| ((sa[ch] - sb[ch]) / k).powi(2)).sum::<f64>();
                n += 3;
            }
        }
    }
    let mse = if n > 0 { se / n as f64 } else { 0.0 };
    c.psnr_blocks = if mse == 0.0 { 99.0 } else { 10.0 * (255.0f64 * 255.0 / mse).log10() };
    c
}

pub fn compare(a: &[u8], b: &[u8]) -> Comparison {
    let psnr = |se: f64, n: u64| {
        let mse = if n > 0 { se / n as f64 } else { 0.0 };
        if mse == 0.0 { 99.0 } else { 10.0 * (255.0f64 * 255.0 / mse).log10() }
    };
    let (mut se_a, mut n_a, mut se_b, mut n_b, mut agree, mut total) = (0f64, 0u64, 0f64, 0u64, 0u64, 0u64);
    for (pa, pb) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
        total += 1;
        let (oa, ob) = (pa[3] >= 128, pb[3] >= 128);
        if oa == ob {
            agree += 1;
        }
        if oa {
            let se: f64 = (0..3).map(|k| (f64::from(pa[k]) - f64::from(pb[k])).powi(2)).sum();
            se_a += se;
            n_a += 3;
            if ob {
                se_b += se;
                n_b += 3;
            }
        }
    }
    Comparison { psnr_ref_opaque: psnr(se_a, n_a), psnr_both_opaque: psnr(se_b, n_b), alpha_agreement: agree as f64 / total.max(1) as f64, psnr_blocks: 0.0 }
}

/// TexGen's replacement model for `model` (Data-relative, `meshes\...`): when
/// `<dyndolod>\Edit Scripts\DynDOLOD\Render\Skyrim\Replace\<path below meshes>` has a
/// `<name>_static.nif`, TexGen renders that instead (skinned or animated trees). Returns the file
/// and the `Model=` value TexGen writes for it (`DynDOLOD\Render\Skyrim\Replace\...`).
pub fn replacement_model(dyndolod: &std::path::Path, model: &str) -> Option<(std::path::PathBuf, String)> {
    let m = model.replace('/', "\\");
    let lower = m.to_ascii_lowercase();
    let rel = if lower.starts_with("meshes\\") { &m[7..] } else { &m[..] };
    let stem = rel.strip_suffix(".nif").or_else(|| rel.strip_suffix(".NIF")).unwrap_or(rel);
    let model_value = format!(r"DynDOLOD\Render\Skyrim\Replace\{stem}_static.nif");
    let file = dyndolod.join("Edit Scripts").join(&model_value);
    file.is_file().then_some((file, model_value))
}

/// Every texture the scene's meshes use (diffuse, normal, glow), loaded once each with `load`.
pub fn load_textures(scene: &Scene, mut load: impl FnMut(&str) -> Option<TexData>) -> HashMap<String, TexData> {
    let mut tex = HashMap::new();
    for m in &scene.meshes {
        for path in [&m.diffuse, &m.normal, &m.glow] {
            if !path.is_empty()
                && !tex.contains_key(path)
                && let Some(t) = load(path)
            {
                tex.insert(path.clone(), t);
            }
        }
    }
    tex
}

/// Settles each shape's textures the way TexGen resolves them: a normal map that does not exist
/// falls back to `<diffuse>_n.dds`; the glow slot counts only when the shader uses it (Glow_Map,
/// Soft, Rim or Back lighting) and the back-light slot only with Back_Lighting; names of files
/// that do not exist are dropped from the `.txt` list.
pub fn resolve_textures(scene: &mut Scene, exists: impl Fn(&str) -> bool) {
    for m in &mut scene.meshes {
        let flags2 = m.shader.as_ref().map(|s| s.flags2).unwrap_or(0);
        // Shared effect normals (grass uses `textures\effects\highfrequencynormals.dds`) count as
        // missing: the golden HD grass lists `<diffuse>_n.dds` or no normal map instead.
        if m.normal.starts_with(r"textures\effects\") {
            let old = m.normal.clone();
            m.texture_names.retain(|t| !t.eq_ignore_ascii_case(&old));
            m.normal.clear();
        }
        if (m.normal.is_empty() || !exists(&m.normal))
            && let Some(stem) = m.diffuse.strip_suffix(".dds")
        {
            let n = format!("{stem}_n.dds");
            m.normal = if exists(&n) { n } else { String::new() };
            if !m.normal.is_empty() {
                let orig = m.texture_names.first().and_then(|d| d.get(..d.len() - 4)).map(|d| format!("{d}_n.dds")).unwrap_or_else(|| m.normal.clone());
                m.texture_names.push(orig);
            }
        }
        let uses_glow = flags2 & ((1 << 6) | (1 << 25) | (1 << 26) | (1 << 27)) != 0;
        if !uses_glow {
            m.glow.clear();
        }
        let keep: Vec<String> = [m.diffuse.clone(), m.normal.clone(), m.glow.clone()].into_iter().filter(|p| !p.is_empty()).collect();
        let back_light = flags2 & (1 << 27) != 0;
        m.texture_names.retain(|t| {
            let l = t.to_ascii_lowercase();
            exists(&l) && (keep.contains(&l) || back_light)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unmulta_and_bleed() {
        // 3x1: covered texel, half-covered texel, empty texel.
        let mut img = vec![200, 100, 50, 255, 60, 30, 10, 128, 0, 0, 0, 0];
        finish_alpha(&mut img, 3, 128);
        assert_eq!(&img[0..4], &[200, 100, 50, 255]);
        // Un-premultiplied: 60 * 255 / 128 = 119.5 -> 120.
        assert_eq!(&img[4..8], &[120, 60, 20, 255]);
        // Bled from the two opaque neighbours within the radius, alpha stays 0.
        assert_eq!(&img[8..12], &[160, 80, 35, 0]);
        // A black half-covered texel is dropped (unmulta needs some colour).
        let mut black = vec![0, 0, 0, 200, 90, 90, 90, 255];
        finish_alpha(&mut black, 2, 128);
        assert_eq!(black[3], 0);
    }

    #[test]
    fn sidecar_layout() {
        let b = Billboard { width_px: 4, height_px: 4, rgba: Vec::new(), normal: None, width: 10.0, depth: 12.5, height: 20.0, shift_z: -1.0 };
        let src = Source { model: r"Meshes\a\b.nif".into(), model_crc: 0xABCDEF01, textures: vec![(r"textures\a\b.dds".into(), 0x12)] };
        let s = sidecar(&b, 1.0, 0xDEADBEEF, &src);
        assert_eq!(
            s,
            "[LOD]\r\nWidth=10.000000\r\nDepth=12.500000\r\nHeight=20.000000\r\nShiftX=0.000000\r\nShiftY=0.000000\r\nShiftZ=-1.000000\r\nScale=1.000000\r\nCRC32=DEADBEEF\r\nModel=Meshes\\a\\b.nif\r\nModelCRC32=ABCDEF01\r\nTexture_1=textures\\a\\b.dds\r\nTextureCRC32_1=00000012\r\n\r\n"
        );
    }

    #[test]
    fn pixel_height_rounds() {
        let s = Settings::default();
        // 1452.5 * 1.78 / 11 = 235.04 -> 235 -> 256; 1455 * 1.61 / 11 = 212.96 -> 213 -> 256.
        assert_eq!(pixel_height(1452.5, 1.78, &s), 256);
        // 2565.1 / 11 = 233.2 -> 256; 924.2 * 1.0 / 11 = 84.0 -> 128.
        assert_eq!(pixel_height(924.2, 1.0, &s), 128);
        assert_eq!(pixel_height(10.0, 1.0, &s), 32);
    }
}
