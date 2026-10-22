//! The DynDOLOD glow page of the object LOD atlas (docs/re/object-lod-spec.md 7.2): shapes of
//! glow / passthru LOD models whose lighting shader has `Glow_Map` and a slot-2 texture get
//! their textures from `textures\DynDOLOD\LOD\DynDOLOD_<World>_Glow.dds` (+ `_n`, `_g`), keyed
//! in the atlas map as `diffuse,normal,glow`.
//!
//! Observed in the golden `dyndolod-golden2` map: six such rows, every tile 256 x 256 (sources
//! of 1024 and 2048 downscaled), tiles packed in key order onto a page as wide as the power of
//! two covering the square root of the tiles' area (1024), the page cropped to its rows (512).
//! The key collection itself is in [`crate::object_atlas_plan`]; this module lays the page out,
//! merges layouts and loads tiles at a size.

use lodforge_objlod::atlas_build::{AtlasEntry, AtlasLayout, AtlasPage};

/// Largest glow tile side.
pub const GLOW_TILE: u32 = 256;

/// Whether LODGen treats `model` as a passthru model (`PassThruMeshMask=glow_lod` /
/// `passthru_lod` in DynDOLOD's export header).
#[allow(dead_code)]
pub fn is_passthru_model(model: &str) -> bool {
    let m = model.to_ascii_lowercase();
    m.contains("glow_lod") || m.contains("passthru_lod")
}

/// Glow page layout for `keys` (sorted, unique `diffuse,normal,glow`); `dims` gives a texture's
/// size. Keys whose diffuse is missing are dropped. Tiles are the diffuse halved until both
/// sides fit [`GLOW_TILE`], laid out row by row on a page whose width is the power of two
/// covering the square root of the tiles' area (at most `max_size`) and whose height is the
/// power of two covering the rows used.
pub fn glow_layout(keys: &[String], dims: impl Fn(&str) -> Option<(u32, u32)>, max_size: u32, base: &str) -> AtlasLayout {
    let mut tiles: Vec<(String, u32, u32)> = Vec::new();
    for k in keys {
        let diffuse = k.split(',').next().unwrap_or("");
        let Some((w, h)) = dims(diffuse) else { continue };
        let (tw, th) = tile_size(w, h);
        tiles.push((k.clone(), tw, th));
    }
    let mut out = AtlasLayout::default();
    if tiles.is_empty() {
        return out;
    }
    let area: u64 = tiles.iter().map(|(_, w, h)| u64::from(*w) * u64::from(*h)).sum();
    let mut width = 1u32;
    while u64::from(width) * u64::from(width) < area {
        width *= 2;
    }
    let width = width.max(GLOW_TILE).min(max_size.max(GLOW_TILE));
    let (mut x, mut y, mut row_h, mut bottom) = (0u32, 0u32, 0u32, 0u32);
    let mut entries = Vec::with_capacity(tiles.len());
    for (key, w, h) in &tiles {
        if x + w > width {
            x = 0;
            y += row_h;
            row_h = 0;
        }
        entries.push(AtlasEntry { texture: key.clone(), width: *w, height: *h, x, y, page: 0 });
        x += w;
        row_h = row_h.max(*h);
        bottom = bottom.max(y + h);
    }
    let mut height = 1u32;
    while height < bottom {
        height *= 2;
    }
    out.pages.push(AtlasPage { name: format!("{base}.dds"), width, height });
    out.entries = entries;
    out
}

/// Halves a glow source until both sides fit [`GLOW_TILE`].
fn tile_size(mut w: u32, mut h: u32) -> (u32, u32) {
    while w > GLOW_TILE || h > GLOW_TILE {
        w = (w / 2).max(1);
        h = (h / 2).max(1);
    }
    (w, h)
}

/// Appends `other`'s pages and entries to `main` (page indices shifted); entries end up in the
/// order of the two layouts.
pub fn merge(mut main: AtlasLayout, other: AtlasLayout) -> AtlasLayout {
    let offset = main.pages.len();
    main.pages.extend(other.pages);
    main.entries.extend(other.entries.into_iter().map(|mut e| {
        e.page += offset;
        e
    }));
    main.mini_rows.extend(other.mini_rows);
    main
}

/// Whether an atlas entry is a glow tile: its key names three textures, `diffuse,normal,glow`
/// (spec 1.2 and 7.2); the other rows name one or two.
pub fn is_glow_key(texture: &str) -> bool {
    texture.split('=').next().unwrap_or("").split(',').filter(|p| !p.trim().is_empty()).count() >= 3
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn golden_glow_page() {
        let keys: Vec<String> = ["a,a_n,a_g", "b,b_n,b_g", "c,c_n,c_g", "d,d_n,d_g", "e,e_n,e_g", "f,f_n,f_g"].iter().map(|s| s.to_string()).collect();
        let l = glow_layout(&keys, |t| Some(if t == "a" { (1024, 1024) } else { (2048, 2048) }), 8192, "x_Glow");
        assert_eq!((l.pages[0].width, l.pages[0].height), (1024, 512));
        let pos: Vec<(u32, u32)> = l.entries.iter().map(|e| (e.x, e.y)).collect();
        assert_eq!(pos, vec![(0, 0), (256, 0), (512, 0), (768, 0), (0, 256), (256, 256)]);
        assert!(is_glow_key("a,a_n,a_g"));
        assert!(!is_glow_key("a,a_n=4"));
        let m = merge(AtlasLayout { pages: vec![AtlasPage { name: "p".into(), width: 1, height: 1 }], entries: vec![AtlasEntry { texture: "t".into(), width: 1, height: 1, x: 0, y: 0, page: 0 }], mini_rows: Vec::new() }, l);
        assert_eq!(m.pages.len(), 2);
        assert_eq!(m.entries[1].page, 1);
    }
}
