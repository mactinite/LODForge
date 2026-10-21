//! Packing terrain data for the GPU spec pipeline (lodforge-gpu terrain_spec.wgsl).

use std::collections::HashMap;

use rayon::prelude::*;

use lodforge_archive::Vfs;
use lodforge_esp::terrain::GRID;
use lodforge_esp::{Land, LandTexture};
use lodforge_gpu::terrain_spec::{CellBatch, Placement, SpecCell, SpecLayer, NONE, TOP_LEVEL};

use crate::bake::texture_path;
use crate::image::Image;
use crate::spec::{Kind, SpecBaker};

/// Landscape textures of one kind (diffuse or normal maps) shrunk to `tex_px` (Lanczos from the
/// full-size image).
pub struct SpecTextureSet {
    pub tex_px: u32,
    pub index: HashMap<String, u32>,
    pub images: Vec<Vec<u8>>,
    pub default_tex: u32,
}

/// One landscape texture shrunk for the `kind` pipeline: the top mip Lanczos-resized to
/// `tex_px`, quantised (rounding for diffuse, truncating for normal maps), alpha 1. `None` when
/// the file is missing or unreadable.
pub fn shrink_texture(vfs: &Vfs, path: &str, tex_px: u32, kind: Kind) -> Option<Vec<u8>> {
    let bytes = vfs.read(path).ok()??;
    let info = lodforge_dds::parse(&bytes).ok()?;
    let top = lodforge_dds::decode(&bytes, &info, 0).ok()?;
    let mut img = Image::from_rgba8(top.width, top.height, &top.rgba).lanczos(tex_px, tex_px);
    img.quantize8_with(kind == Kind::Diffuse);
    img.px.iter_mut().for_each(|px| px[3] = 1.0);
    Some(img.to_rgba8())
}

impl SpecTextureSet {
    /// The set of every LTEX texture of `kind` in the load order (plus the default), shrunk from
    /// the VFS.
    pub fn build(vfs: &Vfs, ltex: &HashMap<u32, LandTexture>, default_path: &str, tex_px: u32, kind: Kind) -> SpecTextureSet {
        let paths = ltex
            .values()
            .map(|t| match kind {
                Kind::Diffuse => &t.diffuse,
                Kind::Normal => &t.normal,
            })
            .filter(|p| !p.is_empty())
            .map(|p| texture_path(p));
        Self::build_from(paths, default_path, tex_px, kind, |p| shrink_texture(vfs, p, tex_px, kind))
    }

    /// The set of the given texture paths (TXST or Data-relative; plus the default), each loaded
    /// by `load` (a shrunk `tex_px` x `tex_px` RGBA image, or `None` to leave it out so cells
    /// referencing it fall back to the default). Slot numbers follow the sorted paths; a cell
    /// renders the same whichever other textures the set holds.
    pub fn build_from(paths: impl IntoIterator<Item = String>, default_path: &str, tex_px: u32, kind: Kind, load: impl Fn(&str) -> Option<Vec<u8>> + Sync) -> SpecTextureSet {
        let mut paths: Vec<String> = paths.into_iter().map(|p| texture_path(&p)).collect();
        let default = texture_path(default_path);
        paths.push(default.clone());
        paths.sort();
        paths.dedup();
        let loaded: Vec<(String, Vec<u8>)> = paths.par_iter().filter_map(|p| load(p).map(|img| (p.clone(), img))).collect();
        let mut set = SpecTextureSet { tex_px, index: HashMap::new(), images: Vec::new(), default_tex: 0 };
        for (p, img) in loaded {
            set.index.insert(p, set.images.len() as u32);
            set.images.push(img);
        }
        if set.images.is_empty() {
            let grey: [u8; 4] = if kind == Kind::Normal { [128, 128, 255, 255] } else { [128, 128, 128, 255] };
            set.images.push(grey.repeat((tex_px * tex_px) as usize));
        }
        set.default_tex = set.index.get(&default).copied().unwrap_or(0);
        set
    }

    fn of(&self, path: &str) -> u32 {
        self.index.get(path).copied().unwrap_or(self.default_tex)
    }
}

/// Batches for one top (LOD32) tile: cells that render identically share one slot per batch.
pub struct TopTile {
    pub batches: Vec<CellBatch>,
    /// Per cell (x, y relative to the top tile): whether it is a default ("no content") cell.
    pub default_cells: HashMap<(i32, i32), bool>,
}

impl TopTile {
    /// Whether the `level` tile at (tx, ty) (in tiles inside the top tile) is a default tile.
    pub fn tile_is_default(&self, level: i32, tx: i32, ty: i32) -> bool {
        (0..level).all(|dy| (0..level).all(|dx| self.default_cells.get(&(tx * level + dx, ty * level + dy)).copied().unwrap_or(true)))
    }
}

fn default_cell(set: &SpecTextureSet) -> SpecCell {
    SpecCell { vc_offset: NONE, terrain_offset: 0, _pad: [0; 2], base: [set.default_tex; 4], layer_start: [0; 4], layer_count: [0; 4] }
}

fn pack_rgba(img: &Image) -> impl Iterator<Item = u32> + '_ {
    img.px.iter().map(|p| {
        let c = p.map(|v| (v.clamp(0.0, 1.0) * 255.0).trunc() as u32);
        c[0] | c[1] << 8 | c[2] << 16 | c[3] << 24
    })
}

/// Packs the cells of the top tile whose south-west cell is `origin` for `kind`.
///
/// Diffuse: default cells (default textures, no VCLR) share slot 0, the default render.
/// Normal: only cells without LAND share slot 0 (flat terrain normal, default normal map); a cell
/// is "default" for the tile-size decision when it has no LAND or no VNML.
pub fn top_tile(baker: &SpecBaker<'_>, set: &SpecTextureSet, lands: &HashMap<(i32, i32), Land>, origin: (i32, i32), max_slots: usize, kind: Kind) -> TopTile {
    let n = TOP_LEVEL as i32;
    let flat_terrain: Vec<u32> = pack_rgba(&SpecBaker::terrain_normal(None)).collect();
    // Terrain normals (two Lanczos passes per cell) are computed up front, in parallel.
    let terrain_normals: HashMap<(i32, i32), Vec<u32>> = if kind == Kind::Normal {
        (0..n * n)
            .into_par_iter()
            .filter_map(|i| {
                let c = (origin.0 + i % n, origin.1 + i / n);
                lands.get(&c).map(|l| (c, pack_rgba(&SpecBaker::terrain_normal(Some(l))).collect()))
            })
            .collect()
    } else {
        HashMap::new()
    };
    let mut default_cells = HashMap::new();
    let mut batches: Vec<CellBatch> = Vec::new();
    let new_batch = |set: &SpecTextureSet| {
        let terrain = if kind == Kind::Normal { flat_terrain.clone() } else { Vec::new() };
        CellBatch { cells: vec![default_cell(set)], terrain, ..Default::default() }
    };
    let mut cur = new_batch(set);
    for y in 0..n {
        for x in 0..n {
            let land = lands.get(&(origin.0 + x, origin.1 + y));
            let (is_default, shares_default_render) = match kind {
                Kind::Diffuse => {
                    let d = baker.cell_is_default(land);
                    (d, d)
                }
                Kind::Normal => (SpecBaker::cell_is_default_normal(land), land.is_none()),
            };
            default_cells.insert((x, y), is_default);
            if shares_default_render {
                cur.placements.push(Placement { x: x as u32, y: y as u32, slot: 0, _pad: 0 });
                continue;
            }
            let land = land.expect("cells with their own render have LAND");
            if cur.cells.len() > max_slots {
                batches.push(std::mem::replace(&mut cur, new_batch(set)));
            }
            let slot = cur.cells.len() as u32;
            let mut cell = default_cell(set);
            match kind {
                Kind::Diffuse => {
                    if let Some(vc) = land.colors.as_ref().filter(|v| v.len() >= GRID * GRID) {
                        cell.vc_offset = cur.vcolors.len() as u32;
                        cur.vcolors.extend(vc.iter().take(GRID * GRID).map(|c| u32::from(c[0]) | u32::from(c[1]) << 8 | u32::from(c[2]) << 16 | 0xFF00_0000));
                    }
                }
                Kind::Normal => {
                    cell.terrain_offset = cur.terrain.len() as u32;
                    cur.terrain.extend_from_slice(&terrain_normals[&(origin.0 + x, origin.1 + y)]);
                }
            }
            for (qi, quad) in land.quadrants.iter().enumerate() {
                if quad.base.is_none() && quad.layers.is_empty() {
                    continue;
                }
                cell.base[qi] = set.of(&baker.texture_path(quad.base, kind));
                // One layer per index; a later duplicate overwrites an earlier one.
                let mut slots: Vec<&lodforge_esp::Layer> = Vec::new();
                for l in &quad.layers {
                    if let Some(s) = slots.iter_mut().find(|s| s.layer == l.layer) {
                        *s = l;
                    } else {
                        slots.push(l);
                    }
                }
                cell.layer_start[qi] = cur.layers.len() as u32;
                cell.layer_count[qi] = slots.len() as u32;
                for l in slots {
                    let mask = SpecBaker::splat_mask(&l.opacity);
                    cur.layers.push(SpecLayer { tex: set.of(&baker.texture_path(Some(l.ltex), kind)), splat_offset: cur.splats.len() as u32 });
                    cur.splats.extend(mask.px.iter().map(|p| p[0]));
                }
            }
            cur.cells.push(cell);
            cur.placements.push(Placement { x: x as u32, y: y as u32, slot, _pad: 0 });
        }
    }
    batches.push(cur);
    TopTile { batches, default_cells }
}
