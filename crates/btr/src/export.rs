//! Terrain input straight from the load order (docs/re/terrain-lod-spec.md 2.1, 2.3, 2.4):
//! every CELL inside the bounding rectangle of the worldspace's LAND records, heights from VHGT,
//! water from the cell's XCLW or the worldspace default, hide flags from XCLC. A cell without
//! LAND, or a LAND without VHGT, is flat at the default land height. The rectangle and the water
//! rule reproduce xLODGen's own terrain export byte for byte for all 11 vanilla worldspaces
//! (measured 2026-10-17 with `examples/export_check.rs`).

use rayon::prelude::*;

use lodforge_esp::terrain::decode_vhgt;
use lodforge_esp::{CELL_SHORT_DATA, LoadOrder, Worldspace};

use crate::input::{GRID, TerrainCell, TerrainInput};

/// Default land and water heights of a worldspace (WRLD DNAM), following the parent worldspace
/// where the PNAM flags say the child uses the parent's land (0x01) or water (0x08) data, or where
/// the child has no DNAM of its own. A WRLD DATA flag 0x08 ("no LOD water") gives `None`.
pub fn world_defaults(world: &Worldspace, parent: Option<&Worldspace>) -> (f32, Option<f32>) {
    let from_parent = |flag: u8| matches!(parent, Some(_) if world.parent_flags & flag != 0 || world.default_heights.is_none());
    let land_src = if from_parent(0x01) {
        parent.unwrap()
    } else {
        world
    };
    let water_src = if from_parent(0x08) {
        parent.unwrap()
    } else {
        world
    };
    let land = land_src.default_heights.map_or(0.0, |d| d.0);
    let water = match water_src.default_heights {
        None => None,
        Some(_) if water_src.flags & 0x08 != 0 => None,
        Some(d) => Some(d.1),
    };
    (land, water)
}

/// The cells of `world` (LAND from `land_world`, which is the parent when the child uses its
/// land data), as xLODGen exports them. Sorted by (y, x).
pub fn export_cells(
    lo: &LoadOrder,
    world: &Worldspace,
    land_world: &Worldspace,
    default_land: f32,
    default_water: Option<f32>,
) -> lodforge_esp::Result<Vec<TerrainCell>> {
    let lands = land_world.land_by_cell();
    let Some(((x0, y0), (x1, y1))) =
        lands
            .keys()
            .fold(None, |acc: Option<((i32, i32), (i32, i32))>, &(x, y)| {
                Some(acc.map_or(((x, y), (x, y)), |((x0, y0), (x1, y1))| {
                    ((x0.min(x), y0.min(y)), (x1.max(x), y1.max(y)))
                }))
            })
    else {
        return Ok(Vec::new());
    };
    let mut cells: Vec<(i32, i32)> = world
        .cells
        .keys()
        .chain(land_world.cells.keys())
        .chain(lands.keys())
        .copied()
        .filter(|&(x, y)| x >= x0 && x <= x1 && y >= y0 && y <= y1)
        .collect();
    cells.sort_by_key(|c| (c.1, c.0));
    cells.dedup();
    cells
        .par_iter()
        .map(|&cell| {
            let heights = match lands.get(&cell) {
                Some(l) => {
                    let lp = &lo.plugins[l.plugin as usize];
                    lp.land_vhgt(&lp.record_at(l.offset)?)?
                        .map(|v| decode_vhgt(&v))
                }
                None => None,
            };
            // XCLW = FLT_MAX (0x7F7FFFFF) means "use the worldspace default"; vanilla Tamriel also
            // holds garbage values near +-2^31 and 2^32, treated the same. A one-byte CELL DATA
            // means "no water".
            let short = world
                .cell_flags
                .get(&cell)
                .is_some_and(|f| f & CELL_SHORT_DATA != 0);
            let water = match world.cell_water.get(&cell) {
                _ if short => None,
                Some(&h) if h.abs() < 16_777_216.0 => Some(h),
                _ => default_water,
            };
            let hide = world.cell_hide.get(&cell).copied().unwrap_or(0) as u8 & 0xF;
            Ok(TerrainCell {
                x: cell.0,
                y: cell.1,
                water,
                heights: heights.unwrap_or_else(|| vec![default_land; GRID * GRID]),
                hide_quadrants: hide,
            })
        })
        .collect()
}

/// The terrain input of a worldspace: lattice origin and top level from `lodsettings`, defaults
/// from DNAM (with the parent rules), cells from the load order.
pub fn terrain_input(
    lo: &LoadOrder,
    world: &Worldspace,
    parent: Option<&Worldspace>,
    south_west: (i32, i32),
    max_level: i32,
) -> lodforge_esp::Result<TerrainInput> {
    let (default_land, default_water) = world_defaults(world, parent);
    let land_world = match parent {
        Some(p) if world.parent_flags & 0x01 != 0 => p,
        _ => world,
    };
    let cells = export_cells(lo, world, land_world, default_land, default_water)?;
    let mut t = TerrainInput {
        world: world.editor_id.clone(),
        south_west,
        max_level,
        default_land,
        default_water,
        cells,
    };
    t.normalize();
    Ok(t)
}

/// `lodsettings\<World>.lod`: (south-west cell, max level).
pub fn parse_lodsettings(bytes: &[u8]) -> Option<((i32, i32), i32)> {
    if bytes.len() < 16 {
        return None;
    }
    let sw = (
        i32::from(i16::from_le_bytes([bytes[0], bytes[1]])),
        i32::from(i16::from_le_bytes([bytes[2], bytes[3]])),
    );
    let max_level = i32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
    Some((sw, max_level))
}

/// The cells that have a LAND record, as the export uses them: the parent worldspace's when the
/// child uses its parent's land data (PNAM flag 0x01).
pub fn land_cells(world: &Worldspace, parent: Option<&Worldspace>) -> std::collections::HashSet<(i32, i32)> {
    let land_world = match parent {
        Some(p) if world.parent_flags & 0x01 != 0 => p,
        _ => world,
    };
    land_world.land_by_cell().into_keys().collect()
}
