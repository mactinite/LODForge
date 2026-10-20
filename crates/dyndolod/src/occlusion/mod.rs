//! Pre-computed occlusion: the `TVDT` field of exterior CELL records and the Occlusion.esp that
//! carries it, written from docs/re/occlusion-spec.md (public documentation, xEdit's record
//! definitions, the shipped configuration keys and the golden output) and AnvilLOD's occlusion
//! files (GPL-3.0-only, attributed in the submodules).
//!
//! Pipeline per main worldspace, then per child worldspace that uses its LOD (`PNAM` 0x02):
//! the worldspace's exterior cells and winning LAND heights with their water ([`land_cells`]),
//! a height map at 128 units ([`HeightMap`]: landscape clamped to water, raised by the object
//! LOD height map; a child starts from its own cells and takes the parent's elsewhere), the
//! cells to process ([`select_cells`], `OcclusionMode`), the visibility bits per cell
//! ([`Calculator`]) and the plugin content ([`world_content`]).

pub mod content;
pub mod grid;
pub mod lodmesh;
pub mod tvdt;
pub mod visibility;

pub use content::{plugin_builder, strings_lookup, world_content, CELL_ORDER};
pub use grid::HeightMap;
pub use lodmesh::add_terrain_lod;
pub use tvdt::{Layout, Tvdt};
pub use visibility::{Calculator, Model, Occluder};

use std::collections::{HashMap, HashSet};

use lodforge_esp::{fields, LandRef, LoadOrder, RecordRef, Sig, Worldspace};
use rayon::prelude::*;

/// Water height of a cell without water.
pub const NO_WATER: f32 = -f32::MAX;

/// A cell's winning LAND heights with the cell's water height.
#[derive(Debug, Clone)]
pub struct LandCell {
    pub x: i32,
    pub y: i32,
    /// `XCLW` of the winning CELL when it holds a real height, else the worldspace default;
    /// [`NO_WATER`] for no water.
    pub water: f32,
    /// Raw `VHGT` payload: `f32` offset, 33 x 33 `i8` deltas, 3 pad bytes.
    pub vhgt: Vec<u8>,
}

impl LandCell {
    /// 33 x 33 absolute heights in game units, row-major from the south-west corner.
    pub fn heights(&self) -> Vec<f32> {
        lodforge_esp::terrain::decode_vhgt(&self.vhgt)
    }

    /// Perfectly flat terrain: every delta is zero.
    pub fn is_flat(&self) -> bool {
        self.vhgt[4..4 + grid::POINTS * grid::POINTS].iter().all(|&d| d == 0)
    }

    /// Water height for the height map: `NEG_INFINITY` when the cell has none.
    pub fn water_level(&self) -> f32 {
        if self.water > NO_WATER { self.water } else { f32::NEG_INFINITY }
    }
}

/// The water height of a cell: `XCLW` of the winning CELL when it holds a real height, none for
/// the vanilla cells whose `DATA` is a single byte, else the worldspace default.
pub fn cell_water(world: &Worldspace, cell: (i32, i32), default: f32) -> f32 {
    if world.cell_flags.get(&cell).is_some_and(|f| f & lodforge_esp::CELL_SHORT_DATA != 0) {
        return NO_WATER;
    }
    match world.cell_water.get(&cell) {
        Some(&h) if h.abs() < 16_777_216.0 => h,
        _ => default,
    }
}

/// The winning LAND `VHGT` of every exterior cell of a worldspace with the cell's water height
/// ([`cell_water`] with `default_water`), sorted by (y, x).
pub fn land_cells(lo: &LoadOrder, world: &Worldspace, default_water: f32) -> lodforge_esp::Result<Vec<LandCell>> {
    let mut refs: Vec<((i32, i32), LandRef)> = world.land_by_cell().into_iter().collect();
    refs.sort_by_key(|(c, _)| (c.1, c.0));
    let cells: Vec<Option<LandCell>> = refs
        .par_iter()
        .map(|(c, l)| {
            let lp = &lo.plugins[usize::from(l.plugin)];
            let r = lp.record_at(l.offset)?;
            Ok(lp.land_vhgt(&r)?.filter(|v| v.len() >= 4 + grid::POINTS * grid::POINTS).map(|vhgt| LandCell { x: c.0, y: c.1, water: cell_water(world, *c, default_water), vhgt }))
        })
        .collect::<lodforge_esp::Result<_>>()?;
    Ok(cells.into_iter().flatten().collect())
}

/// (default land height, default water height) of a worldspace: its `DNAM`, or the parent's
/// where `PNAM` says Use Land Data (0x01) / Use Water Data (0x08) or the worldspace has no
/// `DNAM`; a worldspace without the water flag and without `DNAM` has water height 0 (golden:
/// RiftenWorld, KatariahWorld); a No LOD Water worldspace (DATA 0x08) has [`NO_WATER`].
pub fn world_heights(world: &Worldspace, parent: Option<&Worldspace>) -> (f32, f32) {
    let parent_dnam = parent.and_then(|p| p.default_heights);
    let land = match (world.default_heights, parent_dnam) {
        (_, Some(p)) if world.parent_flags & 0x01 != 0 => p.0,
        (Some(d), _) => d.0,
        (None, Some(p)) => p.0,
        (None, None) => 0.0,
    };
    let water = match (world.default_heights, parent_dnam) {
        (_, Some(p)) if world.parent_flags & 0x08 != 0 => p.1,
        (Some(_), _) if world.flags & 0x08 != 0 => NO_WATER,
        (Some(d), _) => d.1,
        (None, _) => 0.0,
    };
    (land, water)
}

/// Height of the cells inside the map that have no landscape and no terrain LOD mesh: the water
/// surface when the worldspace has water above its default land, else the default land height.
pub fn default_height(land: f32, water: f32) -> f32 {
    if water > NO_WATER { water.max(land) } else { land }
}

/// Cells the map extends beyond the terrain LOD meshes on every side: the golden output shows
/// one block of sea beyond the vanilla meshes (visible from sea-level observers) before the
/// data ends (spec section 10.5).
pub const MESH_MARGIN: i32 = 4;

/// Form ids of the regions flagged Border Region (REGN record flag 0x40).
pub fn border_regions(lo: &LoadOrder) -> lodforge_esp::Result<HashSet<u32>> {
    Ok(lodforge_esp::winning_records(lo, Sig(*b"REGN"))?.into_iter().filter(|(_, (_, r))| r.flags & 0x40 != 0).map(|(id, _)| id).collect())
}

/// What a winning CELL record says that cell selection needs.
#[derive(Debug, Clone, Default)]
pub struct CellRecordInfo {
    /// `XCLR` regions (load-order form ids).
    pub regions: Vec<u32>,
    pub has_tvdt: bool,
    pub has_mhdt: bool,
}

pub fn cell_record_info(lo: &LoadOrder, at: RecordRef) -> lodforge_esp::Result<CellRecordInfo> {
    let lp = &lo.plugins[usize::from(at.plugin)];
    let r = lp.record_at(at.offset)?;
    let data = r.data()?;
    let mut info = CellRecordInfo::default();
    for f in fields(&data) {
        match &f.sig.0 {
            b"XCLR" => info.regions.extend(f.data.chunks_exact(4).filter_map(|b| lp.global_form_id(u32::from_le_bytes(b.try_into().unwrap())))),
            b"TVDT" => info.has_tvdt = true,
            b"MHDT" => info.has_mhdt = true,
            _ => {}
        }
    }
    Ok(info)
}

/// `OcclusionMode` terms (case-insensitive): `All`, `-Flat`, `+Border`, `+TVDT`, `-TVDT`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Mode {
    pub text: String,
    pub no_flat: bool,
    pub border: bool,
    /// `Some(true)` only cells with TVDT, `Some(false)` only cells without.
    pub tvdt: Option<bool>,
}

impl Mode {
    pub fn parse(text: &str) -> Mode {
        let mut m = Mode { text: text.trim().to_string(), ..Default::default() };
        for term in text.split_whitespace() {
            match term.to_ascii_lowercase().as_str() {
                "all" => m = Mode { text: m.text.clone(), ..Default::default() },
                "-flat" => m.no_flat = true,
                "+border" => m.border = true,
                "+tvdt" => m.tvdt = Some(true),
                "-tvdt" => m.tvdt = Some(false),
                _ => {}
            }
        }
        m
    }

    /// The mode a child worldspace gets: the same terms without `+Border`.
    pub fn without_border(&self) -> Mode {
        let text: Vec<&str> = self.text.split_whitespace().filter(|t| !t.eq_ignore_ascii_case("+border")).collect();
        Mode { text: text.join(" "), border: false, ..self.clone() }
    }
}

/// The LOD grid of a worldspace (`lodsettings\<World>.lod`): south-west cell and stride.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grid {
    pub origin: (i32, i32),
    pub stride: i32,
}

impl Grid {
    /// Tamriel's grid, the fallback when no lodsettings file is found.
    pub const TAMRIEL: Grid = Grid { origin: (-96, -96), stride: 256 };

    pub fn contains(&self, (x, y): (i32, i32)) -> bool {
        x >= self.origin.0 && y >= self.origin.1 && x < self.origin.0 + self.stride && y < self.origin.1 + self.stride
    }

    pub fn layout(&self) -> Option<Layout> {
        Layout::for_stride(self.stride)
    }
}

/// Reads `lodsettings\<World>.lod` (`int16 x, int16 y, int32 stride, int32 min level, int32 max
/// level`) through `read`.
pub fn lod_settings(read: &dyn Fn(&str) -> Option<Vec<u8>>, world: &str) -> Option<Grid> {
    let l = read(&format!("lodsettings\\{world}.lod")).filter(|l| l.len() >= 8)?;
    Some(Grid { origin: (i32::from(i16::from_le_bytes([l[0], l[1]])), i32::from(i16::from_le_bytes([l[2], l[3]]))), stride: i32::from_le_bytes([l[4], l[5], l[6], l[7]]) })
}

/// The main worldspace, then (with `children`) the worldspaces whose parent it is and that use
/// its LOD data (`PNAM` 0x02) and have exterior cells, in form id order.
pub fn occlusion_worlds<'a>(worlds: &'a [Worldspace], main: &'a Worldspace, children: bool) -> Vec<&'a Worldspace> {
    let mut todo = vec![main];
    if children {
        let mut kids: Vec<&Worldspace> = worlds.iter().filter(|w| w.parent == Some(main.form_id) && w.parent_flags & 0x02 != 0 && !w.cell_records.is_empty()).collect();
        kids.sort_by_key(|w| w.form_id);
        todo.extend(kids);
    }
    todo
}

/// The cells of a worldspace that get TVDT: its LAND cells inside the grid, filtered by the
/// mode (`-Flat` drops perfectly flat terrain, `+Border` keeps cells in a region flagged Border
/// Region, `+TVDT`/`-TVDT` filter on an existing TVDT in the winning record), sorted by (y, x).
pub fn select_cells(lo: &LoadOrder, world: &Worldspace, lands: &[LandCell], grid: &Grid, mode: &Mode, border: &HashSet<u32>) -> lodforge_esp::Result<Vec<(i32, i32)>> {
    let keep: Vec<Option<(i32, i32)>> = lands
        .par_iter()
        .map(|c| {
            let cell = (c.x, c.y);
            if !grid.contains(cell) || (mode.no_flat && c.is_flat()) {
                return Ok(None);
            }
            if mode.border || mode.tvdt.is_some() {
                let Some(&rr) = world.cell_records.get(&cell) else { return Ok(None) };
                let info = cell_record_info(lo, rr)?;
                if mode.border && !info.regions.iter().any(|r| border.contains(r)) {
                    return Ok(None);
                }
                if mode.tvdt.is_some_and(|want| want != info.has_tvdt) {
                    return Ok(None);
                }
            }
            Ok(Some(cell))
        })
        .collect::<lodforge_esp::Result<_>>()?;
    Ok(keep.into_iter().flatten().collect())
}

/// The three numeric occlusion settings (`OcclusionQuality`, `OcclusionRadius`,
/// `OcclusionHeight`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Params {
    pub quality: i32,
    pub radius: i32,
    pub height: f32,
}

impl Params {
    pub fn model(&self) -> Model {
        Model::new(self.quality, self.radius, self.height)
    }
}

/// The cells of an object LOD height map (`<World>.objects.heightmap.dat`: `i32 count`, then per
/// cell `i32 x, i32 y` and 33 x 33 `f32` tops in x-major order).
pub fn object_heightmap(data: &[u8]) -> Vec<((i32, i32), Vec<f32>)> {
    const CELL: usize = 8 + 4 * grid::POINTS * grid::POINTS;
    let Some(count) = data.get(..4).map(|b| u32::from_le_bytes(b.try_into().unwrap()) as usize) else { return Vec::new() };
    data[4..]
        .chunks_exact(CELL)
        .take(count)
        .map(|r| {
            let x = i32::from_le_bytes(r[0..4].try_into().unwrap());
            let y = i32::from_le_bytes(r[4..8].try_into().unwrap());
            let pts = r[8..].chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect();
            ((x, y), pts)
        })
        .collect()
}

/// The height map of a worldspace over the rectangle of its land and object cells and `extra`
/// (the terrain LOD mesh coverage) inside the grid: the landscape of `lands` clamped to each
/// cell's water, raised by `objects`.
pub fn build_height_map(grid: &Grid, lands: &[&LandCell], objects: &[((i32, i32), Vec<f32>)], extra: Option<((i32, i32), (i32, i32))>, default: f32) -> HeightMap {
    let corners = extra.into_iter().flat_map(|(lo, hi)| [lo, hi]).map(|c| (c.0.clamp(grid.origin.0, grid.origin.0 + grid.stride - 1), c.1.clamp(grid.origin.1, grid.origin.1 + grid.stride - 1)));
    let cells = lands.iter().map(|c| (c.x, c.y)).chain(objects.iter().map(|o| o.0)).filter(|c| grid.contains(*c)).chain(corners);
    let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for (x, y) in cells {
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    if x0 > x1 {
        return HeightMap::new(grid.origin.0, grid.origin.1, 0, 0, default);
    }
    let mut map = HeightMap::new(x0, y0, (x1 - x0 + 1) as usize, (y1 - y0 + 1) as usize, default);
    for c in lands {
        map.set_land(c.x, c.y, &c.heights(), c.water_level());
    }
    for (cell, pts) in objects {
        map.raise_top(cell.0, cell.1, pts);
    }
    map
}

/// Everything one worldspace's TVDT computation starts from.
#[derive(Debug)]
pub struct WorldSetup {
    pub grid: Grid,
    pub land_height: f32,
    pub water_height: f32,
    /// Height where the map has no data.
    pub default: f32,
    /// The worldspace's own LAND cells.
    pub lands: Vec<LandCell>,
    pub map: HeightMap,
    /// Terrain LOD meshes read for the cells without landscape.
    pub lod_meshes: usize,
}

/// Prepares a worldspace: its default heights (`land_height` overrides the worldspace's), its
/// LAND cells, and its height map, which a child worldspace fills with the parent's LAND cells
/// (`parent_lands`) where it has none of its own, then the main worldspace's LOD4 terrain
/// meshes where there is still no landscape (read through `read`), then the main worldspace's
/// object LOD height map (`objects`).
///
/// A child's LAND cells are its own LAND records plus, for every exterior CELL record of the
/// child without one, the parent's LAND at that grid with the child's cell water (the children
/// of the golden run inherit their land data, `PNAM` 0x01, and have no LAND of their own; the
/// golden gives TVDT to exactly their CELL records, spec section 5).
#[allow(clippy::too_many_arguments)]
pub fn prepare_world(lo: &LoadOrder, read: &dyn Fn(&str) -> Option<Vec<u8>>, main: &Worldspace, world: &Worldspace, grid: Grid, land_height: Option<f32>, parent_lands: Option<&[LandCell]>, objects: &[((i32, i32), Vec<f32>)]) -> lodforge_esp::Result<WorldSetup> {
    let is_main = world.form_id == main.form_id;
    let (land, water) = world_heights(world, (!is_main).then_some(main));
    let land = land_height.unwrap_or(land);
    let mut lands = land_cells(lo, world, water)?;
    let default = if is_main {
        default_height(land, water)
    } else {
        let (ml, mw) = world_heights(main, None);
        default_height(ml, mw)
    };
    let mut own: HashSet<(i32, i32)> = lands.iter().map(|c| (c.x, c.y)).collect();
    if let Some(parent) = parent_lands {
        let by_grid: HashMap<(i32, i32), &LandCell> = parent.iter().map(|c| ((c.x, c.y), c)).collect();
        let mut borrowed: Vec<LandCell> = world.cell_records.keys().filter(|g| !own.contains(g)).filter_map(|&g| by_grid.get(&g).map(|p| LandCell { x: g.0, y: g.1, water: cell_water(world, g, water), vhgt: p.vhgt.clone() })).collect();
        borrowed.sort_by_key(|c| (c.y, c.x));
        own.extend(borrowed.iter().map(|c| (c.x, c.y)));
        lands.extend(borrowed);
        lands.sort_by_key(|c| (c.y, c.x));
    }
    let fill: Vec<&LandCell> = lands.iter().chain(parent_lands.unwrap_or(&[]).iter().filter(|c| !own.contains(&(c.x, c.y)))).collect();
    let meshes = lodmesh::mesh_extent(&grid, &main.editor_id, read);
    let extent = meshes.map(|(lo, hi)| ((lo.0 - MESH_MARGIN, lo.1 - MESH_MARGIN), (hi.0 + MESH_MARGIN, hi.1 + MESH_MARGIN)));
    let mut map = build_height_map(&grid, &fill, objects, extent, default);
    let lod_meshes = if meshes.is_some() { add_terrain_lod(&mut map, &grid, &main.editor_id, read) } else { 0 };
    for (cell, pts) in objects {
        map.raise_top(cell.0, cell.1, pts);
    }
    Ok(WorldSetup { grid, land_height: land, water_height: water, default, lands, map, lod_meshes })
}

/// The TVDT of every cell (in parallel); cells outside the grid are skipped.
pub fn compute_cells(calc: &Calculator<'_>, cells: &[(i32, i32)]) -> Vec<((i32, i32), Vec<u8>)> {
    cells.par_iter().filter_map(|&c| calc.compute(c.0, c.1).map(|t| (c, t.bytes))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_terms() {
        let m = Mode::parse("-Flat +Border");
        assert!(m.no_flat && m.border && m.tvdt.is_none());
        let c = m.without_border();
        assert_eq!((c.text.as_str(), c.no_flat, c.border), ("-Flat", true, false));
        let t = Mode::parse("-tvdt -flat");
        assert_eq!(t.tvdt, Some(false));
        let all = Mode::parse("-Flat All");
        assert!(!all.no_flat && !all.border);
    }

    #[test]
    fn world_default_heights_follow_the_parent_flags() {
        let tamriel = Worldspace { form_id: 0x3C, default_heights: Some((-27000.0, -14000.0)), ..Default::default() };
        assert_eq!(world_heights(&tamriel, None), (-27000.0, -14000.0));
        let windhelm = Worldspace { parent: Some(0x3C), parent_flags: 0x7F, ..Default::default() };
        assert_eq!(world_heights(&windhelm, Some(&tamriel)), (-27000.0, -14000.0));
        let riften = Worldspace { parent: Some(0x3C), parent_flags: 0x77, ..Default::default() };
        assert_eq!(world_heights(&riften, Some(&tamriel)), (-27000.0, 0.0));
        let own = Worldspace { default_heights: Some((-100.0, 50.0)), flags: 0x08, ..Default::default() };
        assert_eq!(world_heights(&own, None), (-100.0, NO_WATER));
        assert_eq!(default_height(-27000.0, -14000.0), -14000.0);
        assert_eq!(default_height(-100.0, NO_WATER), -100.0);
    }

    #[test]
    fn height_map_rectangle_and_object_parsing() {
        let grid = Grid::TAMRIEL;
        let mut vhgt = 0f32.to_le_bytes().to_vec();
        vhgt.resize(4 + grid::POINTS * grid::POINTS + 3, 0);
        let a = LandCell { x: 3, y: 4, water: NO_WATER, vhgt: vhgt.clone() };
        let b = LandCell { x: -2, y: 9, water: -14000.0, vhgt };
        assert!(a.is_flat());
        let mut hm = 1u32.to_le_bytes().to_vec();
        hm.extend_from_slice(&7i32.to_le_bytes());
        hm.extend_from_slice(&(-1i32).to_le_bytes());
        hm.extend((0..grid::POINTS * grid::POINTS).flat_map(|_| 250f32.to_le_bytes()));
        let objects = object_heightmap(&hm);
        assert_eq!(objects.len(), 1);
        assert_eq!(objects[0].0, (7, -1));
        let map = build_height_map(&grid, &[&a, &b], &objects, None, -14000.0);
        assert_eq!((map.x0, map.y0, map.cells_x, map.cells_y), (-2, -1, 10, 11));
        let wide = build_height_map(&grid, &[&a, &b], &objects, Some(((-200, -8), (5, 3))), -14000.0);
        assert_eq!((wide.x0, wide.y0, wide.cells_x, wide.cells_y), (-96, -8, 104, 18));
        assert!(map.has_land(3, 4) && map.has_land(-2, 9) && !map.has_land(7, -1));
        let (sx, sy) = map.cell_sample(7, -1);
        assert_eq!(map.top_at(sx, sy), 250.0);
        assert_eq!(map.ground_at(sx, sy), -14000.0);
        let (sx, sy) = map.cell_sample(-2, 9);
        assert_eq!(map.ground_at(sx, sy), 0.0); // land at 0 above its water at -14000
        assert_eq!(map.ground_at(sx - 1, sy), 0.0); // outside the map: height 0
        let (sx, sy) = map.cell_sample(0, 0);
        assert_eq!(map.ground_at(sx, sy), -14000.0); // inside the map without data: the default
    }
}
