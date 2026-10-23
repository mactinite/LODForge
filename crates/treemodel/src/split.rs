//! Derived from TreeLODGen, Copyright (C) 2026 Clemus (GPL-3.0, commit dc4cc8a); ported to Rust and
//! modified for LODForge 2026-10-23. See THIRD-PARTY-NOTICES.txt part 1.7.
//!
//! Bark ("trunk") and foliage ("crown") parts of a tree (TreeLODGen `Split.cs` classification and
//! `WoodSplit.Analyze`; spec 3).
//!
//! Per source shape, the first rule that applies:
//! 1. its name contains `leaf`, `leaves`, `crown`, `frond`, `needle` or `foliage`: crown;
//! 2. its name contains `trunk` or `bark`: trunk;
//! 3. it has no `NiAlphaProperty`: trunk ("opaque shape");
//! 4. otherwise per piece (vertex-index islands): bark when at least 90% of its area samples
//!    opaque texels of the diffuse's alpha (28 samples per triangle, all at or above the alpha
//!    threshold); without the texture, bark when more than half of its area has a tiling UV or the
//!    piece has more than 200 triangles. A shape with both becomes two parts.
//!
//! Divergence: texture paths are normalised (`textures\` prefix, backslashes) before the lookup;
//! TreeLODGen passes them as stored and falls back to the UV test when a prefix is missing.

use std::sync::Arc;

use lodforge_nif::{Block, Nif, TriShape};

use crate::collect::SourceShape;
use crate::islands::{by_index, triangle_area};
use crate::texture::AlphaImage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Crown,
    Trunk,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    Name(&'static str),
    OpaqueShape,
    TextureAlpha,
    /// The diffuse could not be read: decided by tiling UVs.
    UvTiling,
}

/// One part: a whole source shape or the bark or foliage vertices of a mixed one.
#[derive(Debug, Clone)]
pub struct Part {
    pub kind: Kind,
    /// Index into the [`crate::collect`] list.
    pub shape: usize,
    /// Vertices of this part when the shape is mixed (both kinds); `None` = the whole shape.
    pub vertices: Option<Vec<u32>>,
    pub triangles: usize,
    pub reason: Reason,
}

/// Notes about a split (ambiguous pieces, shapes without usable UVs).
#[derive(Debug, Clone, Default)]
pub struct Notes(pub Vec<String>);

const CROWN_HINTS: [&str; 6] = ["leaf", "leaves", "crown", "frond", "needle", "foliage"];
const TRUNK_HINTS: [&str; 2] = ["trunk", "bark"];
pub const OPAQUE_ISLAND_RATIO: f32 = 0.9;
pub const FALLBACK_WOOD_ISLAND_TRIS: usize = 200;
pub const DEFAULT_ALPHA_THRESHOLD: u8 = 128;
const TILING_TOLERANCE: f32 = 0.01;

/// Slot-0 texture of a shape, normalised to a Data-relative `textures\...` path (lower case).
pub fn diffuse(nif: &Nif, s: &TriShape) -> Option<String> {
    let Some(Block::LightingShader(sh)) = s.shader_property.and_then(|r| nif.blocks.get(r as usize)) else { return None };
    let Some(Block::TextureSet(t)) = sh.texture_set.and_then(|r| nif.blocks.get(r as usize)) else { return None };
    let d = t.first()?.trim().replace('/', "\\").to_ascii_lowercase();
    if d.is_empty() {
        return None;
    }
    let d = d.trim_start_matches('\\').to_string();
    Some(if d.starts_with("textures\\") { d } else { format!("textures\\{d}") })
}

fn alpha_threshold(nif: &Nif, s: &TriShape) -> Option<u8> {
    match s.alpha_property.and_then(|r| nif.blocks.get(r as usize)) {
        Some(Block::Alpha { threshold, .. }) => Some(*threshold),
        _ => None,
    }
}

/// All 28 samples of a triangle at or above the threshold.
fn opaque(tex: &AlphaImage, uv: [[f32; 2]; 3], threshold: u8) -> bool {
    const STEPS: i32 = 6;
    for i in 0..=STEPS {
        for j in 0..=(STEPS - i) {
            let w0 = (i as f32 + 0.25) / 6.75;
            let w1 = (j as f32 + 0.25) / 6.75;
            let w2 = 1.0 - w0 - w1;
            if w2 < 0.0 {
                continue;
            }
            let u = w0 * uv[0][0] + w1 * uv[1][0] + w2 * uv[2][0];
            let v = w0 * uv[0][1] + w1 * uv[1][1] + w2 * uv[2][1];
            if tex.alpha(u, v) < threshold {
                return false;
            }
        }
    }
    true
}

fn tiling(uv: [f32; 2]) -> bool {
    uv[0] < -TILING_TOLERANCE || uv[0] > 1.0 + TILING_TOLERANCE || uv[1] < -TILING_TOLERANCE || uv[1] > 1.0 + TILING_TOLERANCE
}

/// Bark and foliage vertices and triangle counts of an alpha-tested shape (`WoodSplit.Analyze`).
struct Wood {
    wood: Vec<u32>,
    leaf: Vec<u32>,
    wood_tris: usize,
    leaf_tris: usize,
    by_texture: bool,
}

fn analyze(s: &TriShape, tex: Option<&AlphaImage>, threshold: u8, name: &str, notes: &mut Notes, ambiguous: &mut usize) -> Option<Wood> {
    if s.triangles.is_empty() || s.uvs.len() != s.positions.len() {
        return None;
    }
    let tris: Vec<[u32; 3]> = s.triangles.iter().map(|t| t.map(u32::from)).collect();
    let islands = by_index(s.positions.len(), &tris);
    let mut w = Wood { wood: Vec::new(), leaf: Vec::new(), wood_tris: 0, leaf_tris: 0, by_texture: tex.is_some() };
    let mut seen_w = vec![false; s.positions.len()];
    let mut seen_l = vec![false; s.positions.len()];
    for isl in &islands {
        let (mut area, mut opaque_area) = (0.0f32, 0.0f32);
        for &t in &isl.triangles {
            let [a, b, c] = tris[t].map(|i| i as usize);
            let ar = triangle_area(s.positions[a], s.positions[b], s.positions[c]);
            area += ar;
            let o = match tex {
                Some(img) => opaque(img, [s.uvs[a], s.uvs[b], s.uvs[c]], threshold),
                None => [a, b, c].iter().any(|&i| tiling(s.uvs[i])),
            };
            if o {
                opaque_area += ar;
            }
        }
        let ratio = if area > 0.0 { opaque_area / area } else { 0.0 };
        let wood = match tex {
            Some(_) => ratio >= OPAQUE_ISLAND_RATIO,
            None => ratio > 0.5 || isl.triangles.len() > FALLBACK_WOOD_ISLAND_TRIS,
        };
        if tex.is_some() && ratio > 0.3 && ratio < OPAQUE_ISLAND_RATIO && isl.triangles.len() > 50 && *ambiguous < 5 {
            notes.0.push(format!("ambiguous-piece:{name}"));
            *ambiguous += 1;
        }
        let (list, seen, count) = if wood { (&mut w.wood, &mut seen_w, &mut w.wood_tris) } else { (&mut w.leaf, &mut seen_l, &mut w.leaf_tris) };
        *count += isl.triangles.len();
        for &t in &isl.triangles {
            for v in tris[t] {
                if !seen[v as usize] {
                    seen[v as usize] = true;
                    list.push(v);
                }
            }
        }
    }
    w.wood.sort_unstable();
    w.leaf.sort_unstable();
    Some(w)
}

/// The parts of a tree's source shapes; `texture` returns the alpha of a diffuse (`textures\...`).
pub fn split(nif: &Nif, shapes: &[SourceShape], texture: &dyn Fn(&str) -> Option<Arc<AlphaImage>>, notes: &mut Notes) -> Vec<Part> {
    let mut parts = Vec::new();
    for (si, src) in shapes.iter().enumerate() {
        let Some(Block::TriShape(s)) = nif.blocks.get(src.block) else { continue };
        let lname = src.name.to_ascii_lowercase();
        let whole = |kind, reason| Part { kind, shape: si, vertices: None, triangles: s.triangles.len(), reason };
        if let Some(h) = CROWN_HINTS.iter().find(|h| lname.contains(*h)) {
            parts.push(whole(Kind::Crown, Reason::Name(h)));
            continue;
        }
        if let Some(h) = TRUNK_HINTS.iter().find(|h| lname.contains(*h)) {
            parts.push(whole(Kind::Trunk, Reason::Name(h)));
            continue;
        }
        let Some(threshold) = alpha_threshold(nif, s) else {
            parts.push(whole(Kind::Trunk, Reason::OpaqueShape));
            continue;
        };
        let tex = diffuse(nif, s).and_then(|d| texture(&d));
        let mut ambiguous = 0;
        let Some(w) = analyze(s, tex.as_deref(), threshold, &src.name, notes, &mut ambiguous) else {
            notes.0.push(format!("no-uvs:{}", src.name));
            continue;
        };
        let reason = if w.by_texture { Reason::TextureAlpha } else { Reason::UvTiling };
        let mixed = w.wood_tris > 0 && w.leaf_tris > 0;
        if w.wood_tris > 0 {
            parts.push(Part { kind: Kind::Trunk, shape: si, vertices: mixed.then(|| w.wood.clone()), triangles: w.wood_tris, reason: reason.clone() });
        }
        if w.leaf_tris > 0 {
            parts.push(Part { kind: Kind::Crown, shape: si, vertices: mixed.then(|| w.leaf.clone()), triangles: w.leaf_tris, reason });
        }
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_samples_cover_the_triangle() {
        // Left half opaque, right half transparent.
        let (w, h) = (64u32, 64u32);
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w / 2 {
                rgba[((y * w + x) * 4 + 3) as usize] = 255;
            }
        }
        let img = AlphaImage::from_rgba(w, h, &rgba);
        assert!(opaque(&img, [[0.0, 0.0], [0.45, 0.0], [0.0, 1.0]], 128));
        assert!(!opaque(&img, [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]], 128));
    }

    #[test]
    fn tiling_uvs_leave_the_unit_square() {
        assert!(tiling([1.2, 0.5]));
        assert!(!tiling([1.005, 0.5]));
    }
}
