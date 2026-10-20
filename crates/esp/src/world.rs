//! Worldspace scan: winning worldspace records, exterior cells, LAND records and placed references
//! across a whole load order.
//!
//! WRLD top group layout:
//! ```text
//! GRUP top "WRLD"
//!   WRLD record
//!   GRUP world children (type 1, label = WRLD form id)
//!     CELL (persistent cell) + GRUP cell children (6) -> GRUP persistent (8) -> REFR/ACHR/...
//!     GRUP exterior block (4) -> GRUP exterior sub-block (5)
//!       CELL + GRUP cell children (6) -> GRUP temporary (9) -> LAND, REFR, NAVM, ...
//! ```

use std::collections::HashMap;

use rayon::prelude::*;

use crate::load_order::{LoadOrder, LoadedPlugin};
use crate::record::{f32_at, fields, group_type, i32_at, Entry, Group, Record, Sig, FLAG_PERSISTENT};
use crate::Result;

/// Marker bit in [`Worldspace::cell_flags`]: the CELL's DATA field is a single byte (some vanilla
/// Blackreach cells). xLODGen writes "no water" (-FLT_MAX) for those cells whatever XCLW says.
pub const CELL_SHORT_DATA: u16 = 0x8000;

/// A placed reference (REFR) as the winning plugin defines it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    /// Global form id of the base object (NAME).
    pub base: u32,
    pub position: [f32; 3],
    /// Euler rotation in radians (DATA).
    pub rotation: [f32; 3],
    pub scale: f32,
    /// XSCL is present (xEdit prints a missing scale as `1.0`, a present one with 6 decimals).
    pub has_scale: bool,
    /// XESP (enable parent) is present.
    pub enable_parent: bool,
    /// XESP parent (global form id) and flags (1 = set enable state to opposite of parent).
    pub enable_parent_id: u32,
    pub enable_parent_flags: u8,
    pub flags: u32,
    /// Exterior cell the reference sits in; `None` for the persistent cell.
    pub cell: Option<(i32, i32)>,
    /// Absolute offset of the winning REFR record header in that plugin.
    pub offset: usize,
    /// Index into `LoadOrder::plugins` of the winning plugin.
    pub plugin: u16,
}

impl Placement {
    pub fn is_persistent(&self) -> bool {
        self.flags & FLAG_PERSISTENT != 0
    }
}

/// Where a record lives: load-order index of the plugin and absolute offset of its header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecordRef {
    pub plugin: u16,
    pub offset: usize,
}

/// Where a winning LAND record lives, so it can be decoded later without a rescan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LandRef {
    pub cell: (i32, i32),
    pub plugin: u16,
    /// Absolute offset of the LAND record header in that plugin.
    pub offset: usize,
}

#[derive(Debug, Clone, Default)]
pub struct Worldspace {
    pub form_id: u32,
    pub editor_id: String,
    /// Parent worldspace (WNAM), global form id.
    pub parent: Option<u32>,
    /// PNAM flags: what the worldspace takes from its parent (0x01 land, 0x02 LOD, 0x04 map,
    /// 0x08 water, 0x10 climate, 0x20 image space, 0x40 sky cell).
    pub parent_flags: u8,
    /// NAM4 LOD water height.
    pub lod_water: Option<f32>,
    /// Object bounds (NAM0 min, NAM9 max) in game units, as (x, y).
    pub object_bounds: Option<([f32; 2], [f32; 2])>,
    /// Plugins that contain any record for this worldspace, in load order.
    pub plugins: Vec<u16>,
    /// Exterior cells by grid coordinate -> global CELL form id.
    pub cells: HashMap<(i32, i32), u32>,
    /// Winning CELL water height (XCLW) by grid coordinate, where the cell sets one.
    pub cell_water: HashMap<(i32, i32), f32>,
    /// CELL DATA flags of the winning CELL per grid (0x02 = has water), plus [`CELL_SHORT_DATA`].
    pub cell_flags: HashMap<(i32, i32), u16>,
    /// Land hide flags (XCLC third field: one bit per hidden quadrant) of the winning CELL, where set.
    pub cell_hide: HashMap<(i32, i32), u32>,
    /// Default land and water heights (WRLD DNAM).
    pub default_heights: Option<(f32, f32)>,
    /// Winning LAND per global form id.
    pub lands: HashMap<u32, LandRef>,
    /// Winning placed references (REFR only) per global form id; deleted ones are removed.
    pub refs: HashMap<u32, Placement>,
    /// Winning WRLD TNAM (HD LOD diffuse texture).
    pub hd_lod_texture: String,
    /// Winning WRLD DATA flags.
    pub flags: u8,
    /// Large references (RNAM) of every plugin overriding the WRLD record, in load order:
    /// (plugin, grid cell (x, y), REFR global form id).
    pub large_refs: Vec<(u16, (i32, i32), u32)>,
    /// Cell each large reference is listed under in its RNAM entry (the reference's own cell, as
    /// opposed to the RNAM field's grid, which lists the neighbours' large references too); the
    /// last plugin listing it wins.
    pub large_ref_cells: HashMap<u32, (i32, i32)>,
    /// Exterior cells holding a navmesh (NAVM) in any plugin.
    pub navmesh_cells: std::collections::HashSet<(i32, i32)>,
    /// The winning WRLD record.
    pub world_record: Option<RecordRef>,
    /// The persistent CELL (global form id) and its winning record.
    pub persistent_cell: Option<(u32, RecordRef)>,
    /// Winning exterior CELL records by grid coordinate.
    pub cell_records: HashMap<(i32, i32), RecordRef>,
    /// First (master) CELL record per grid coordinate.
    pub cell_master_records: HashMap<(i32, i32), RecordRef>,
}

impl Worldspace {
    /// Cell range covered by the object bounds (NAM0/NAM9), inclusive.
    pub fn bounds_cells(&self) -> Option<((i32, i32), (i32, i32))> {
        let (lo, hi) = self.object_bounds?;
        let c = |v: f32| (v / 4096.0).floor() as i32;
        Some(((c(lo[0]), c(lo[1])), (c(hi[0]), c(hi[1]))))
    }

    /// The LAND used for each cell. A few cells carry more than one LAND record (vanilla
    /// Dawnguard.esm adds second ones at Tamriel -44,37 and -43,36). xLODGen uses the one from the
    /// earliest plugin (golden Tamriel.4.-44.36: 45 dB with Skyrim.esm's LAND, 35 dB with
    /// Dawnguard's); ties go to the lower form id, so the choice never depends on hash-map order.
    pub fn land_by_cell(&self) -> HashMap<(i32, i32), LandRef> {
        let mut by_cell: HashMap<(i32, i32), (u16, u32, LandRef)> = HashMap::new();
        for (&id, l) in &self.lands {
            let key = (l.plugin, id);
            match by_cell.get(&l.cell) {
                Some((p, i, _)) if (*p, *i) <= key => {}
                _ => {
                    by_cell.insert(l.cell, (l.plugin, id, *l));
                }
            }
        }
        by_cell.into_iter().map(|(c, (_, _, l))| (c, l)).collect()
    }

    pub fn cell_bounds(&self) -> Option<((i32, i32), (i32, i32))> {
        let mut it = self.cells.keys();
        let first = *it.next()?;
        Some(it.fold((first, first), |((x0, y0), (x1, y1)), &(x, y)| ((x0.min(x), y0.min(y)), (x1.max(x), y1.max(y)))))
    }
}

/// Everything one plugin says about worldspaces, in file order.
#[derive(Default)]
struct PluginWorlds {
    worlds: Vec<(u32, Option<WorldHeader>, Vec<Item>)>,
}

struct WorldHeader {
    editor_id: String,
    parent: Option<u32>,
    parent_flags: u8,
    lod_water: Option<f32>,
    object_bounds: Option<([f32; 2], [f32; 2])>,
    default_heights: Option<(f32, f32)>,
    hd_lod_texture: String,
    offset: usize,
    flags: u8,
    large_refs: Vec<((i32, i32), u32)>,
    large_ref_cells: Vec<(u32, (i32, i32))>,
}

enum Item {
    Cell { form_id: u32, grid: (i32, i32), water: Option<f32>, flags: u16, hide: u32, offset: usize },
    PersistentCell { form_id: u32, offset: usize },
    Land { form_id: u32, cell: (i32, i32), offset: usize },
    Navmesh { cell: (i32, i32) },
    Ref { form_id: u32, deleted: bool, placement: Placement },
}

/// Scans every plugin's WRLD group in parallel and merges the results in load order.
pub fn scan_worldspaces(lo: &LoadOrder) -> Result<Vec<Worldspace>> {
    let per_plugin: Vec<PluginWorlds> = lo.plugins.par_iter().enumerate().map(|(i, p)| scan_plugin(p, i as u16)).collect::<Result<_>>()?;

    let mut order: Vec<u32> = Vec::new();
    let mut worlds: HashMap<u32, Worldspace> = HashMap::new();
    for (pi, pw) in per_plugin.into_iter().enumerate() {
        for (wid, header, items) in pw.worlds {
            let w = worlds.entry(wid).or_insert_with(|| {
                order.push(wid);
                Worldspace { form_id: wid, ..Default::default() }
            });
            if w.plugins.last() != Some(&(pi as u16)) {
                w.plugins.push(pi as u16);
            }
            if let Some(h) = header {
                w.world_record = Some(RecordRef { plugin: pi as u16, offset: h.offset });
                w.editor_id = h.editor_id;
                w.parent = h.parent;
                w.parent_flags = h.parent_flags;
                w.lod_water = h.lod_water;
                w.object_bounds = h.object_bounds;
                w.default_heights = h.default_heights;
                w.hd_lod_texture = h.hd_lod_texture;
                w.flags = h.flags;
                w.large_refs.extend(h.large_refs.into_iter().map(|(g, r)| (pi as u16, g, r)));
                w.large_ref_cells.extend(h.large_ref_cells);
            }
            for item in items {
                match item {
                    Item::PersistentCell { form_id, offset } => {
                        w.persistent_cell = Some((form_id, RecordRef { plugin: pi as u16, offset }));
                    }
                    Item::Cell { form_id, grid, water, flags, hide, offset } => {
                        w.cell_flags.insert(grid, flags);
                        if hide != 0 {
                            w.cell_hide.insert(grid, hide);
                        } else {
                            w.cell_hide.remove(&grid);
                        }
                        w.cells.insert(grid, form_id);
                        w.cell_records.insert(grid, RecordRef { plugin: pi as u16, offset });
                        w.cell_master_records.entry(grid).or_insert(RecordRef { plugin: pi as u16, offset });
                        // The winning CELL decides: an override without XCLW drops the water.
                        match water {
                            Some(h) => w.cell_water.insert(grid, h),
                            None => w.cell_water.remove(&grid),
                        };
                    }
                    Item::Navmesh { cell } => {
                        w.navmesh_cells.insert(cell);
                    }
                    Item::Land { form_id, cell, offset } => {
                        w.lands.insert(form_id, LandRef { cell, plugin: pi as u16, offset });
                    }
                    Item::Ref { form_id, deleted, placement } => {
                        if deleted {
                            w.refs.remove(&form_id);
                        } else {
                            w.refs.insert(form_id, placement);
                        }
                    }
                }
            }
        }
    }
    Ok(order.into_iter().map(|id| worlds.remove(&id).unwrap()).collect())
}

fn scan_plugin(lp: &LoadedPlugin, plugin_index: u16) -> Result<PluginWorlds> {
    let mut out = PluginWorlds::default();
    let Some(top) = lp.plugin.top_group(Sig(*b"WRLD"))? else { return Ok(out) };

    let mut pending_header: HashMap<u32, WorldHeader> = HashMap::new();
    for e in top.entries() {
        match e? {
            Entry::Record(r) if r.sig == Sig(*b"WRLD") => {
                let Some(id) = lp.global_form_id(r.form_id) else { continue };
                pending_header.insert(id, world_header(lp, &r)?);
                // A WRLD with no children group still counts as touching the worldspace.
                out.worlds.push((id, None, Vec::new()));
            }
            Entry::Group(g) if g.group_type == group_type::WORLD_CHILDREN => {
                let Some(id) = lp.global_form_id(g.label_u32()) else { continue };
                let items = scan_world_children(lp, plugin_index, g)?;
                out.worlds.push((id, None, items));
            }
            _ => {}
        }
    }
    // Attach headers to the first entry of each world.
    for (id, header, _) in out.worlds.iter_mut() {
        if let Some(h) = pending_header.remove(id) {
            *header = Some(h);
        }
    }
    Ok(out)
}

fn world_header(lp: &LoadedPlugin, r: &Record<'_>) -> Result<WorldHeader> {
    let data = r.data()?;
    let mut h = WorldHeader { editor_id: String::new(), parent: None, parent_flags: 0, lod_water: None, object_bounds: None, default_heights: None, hd_lod_texture: String::new(), offset: r.offset, flags: 0, large_refs: Vec::new(), large_ref_cells: Vec::new() };
    let (mut min, mut max) = (None, None);
    for f in fields(&data) {
        match &f.sig.0 {
            b"EDID" => h.editor_id = f.zstring(),
            b"WNAM" => h.parent = f.u32().and_then(|id| lp.global_form_id(id)),
            b"PNAM" if !f.data.is_empty() => h.parent_flags = f.data[0],
            b"NAM4" if f.data.len() >= 4 => h.lod_water = Some(f32::from_le_bytes(f.data[..4].try_into().unwrap())),
            b"NAM0" if f.data.len() >= 8 => min = Some([f32_at(f.data, 0), f32_at(f.data, 4)]),
            b"NAM9" if f.data.len() >= 8 => max = Some([f32_at(f.data, 0), f32_at(f.data, 4)]),
            b"DNAM" if f.data.len() >= 8 => h.default_heights = Some((f32_at(f.data, 0), f32_at(f.data, 4))),
            b"TNAM" => h.hd_lod_texture = f.zstring(),
            b"DATA" if !f.data.is_empty() => h.flags = f.data[0],
            // Large references: Y i16, X i16, count u32, count x (REFR form id, Y i16, X i16).
            b"RNAM" if f.data.len() >= 8 => {
                let y = i32::from(i16::from_le_bytes([f.data[0], f.data[1]]));
                let x = i32::from(i16::from_le_bytes([f.data[2], f.data[3]]));
                let n = u32::from_le_bytes([f.data[4], f.data[5], f.data[6], f.data[7]]) as usize;
                for k in 0..n {
                    let o = 8 + k * 8;
                    let Some(b) = f.data.get(o..o + 8) else { break };
                    if let Some(id) = lp.global_form_id(u32::from_le_bytes([b[0], b[1], b[2], b[3]])) {
                        h.large_refs.push(((x, y), id));
                        let ey = i32::from(i16::from_le_bytes([b[4], b[5]]));
                        let ex = i32::from(i16::from_le_bytes([b[6], b[7]]));
                        h.large_ref_cells.push((id, (ex, ey)));
                    }
                }
            }
            _ => {}
        }
    }
    h.object_bounds = min.zip(max);
    Ok(h)
}

/// Exterior sub-blocks are independent, so they are scanned in parallel.
fn scan_world_children(lp: &LoadedPlugin, plugin_index: u16, children: Group<'_>) -> Result<Vec<Item>> {
    let mut sub_blocks = Vec::new();
    let mut items = Vec::new();
    for e in children.entries() {
        match e? {
            Entry::Record(r) if r.sig == Sig(*b"CELL") => {
                if let Some(id) = lp.global_form_id(r.form_id) {
                    items.push(Item::PersistentCell { form_id: id, offset: r.offset });
                }
            }
            Entry::Group(g) if g.group_type == group_type::CELL_CHILDREN => {
                // Children of the persistent cell (no grid).
                scan_cell_children(lp, plugin_index, g, None, &mut items)?;
            }
            Entry::Group(g) if g.group_type == group_type::EXTERIOR_BLOCK => {
                for sb in g.entries() {
                    if let Entry::Group(sb) = sb? {
                        if sb.group_type == group_type::EXTERIOR_SUB_BLOCK {
                            sub_blocks.push(sb);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    let nested: Vec<Vec<Item>> = sub_blocks.par_iter().map(|sb| scan_sub_block(lp, plugin_index, *sb)).collect::<Result<_>>()?;
    items.extend(nested.into_iter().flatten());
    Ok(items)
}

fn scan_sub_block(lp: &LoadedPlugin, plugin_index: u16, sb: Group<'_>) -> Result<Vec<Item>> {
    let mut items = Vec::new();
    let mut grid: Option<(i32, i32)> = None;
    for e in sb.entries() {
        match e? {
            Entry::Record(r) if r.sig == Sig(*b"CELL") => {
                let (g, water, flags, hide) = cell_info(&r)?;
                grid = g;
                if let (Some(g), Some(id)) = (grid, lp.global_form_id(r.form_id)) {
                    items.push(Item::Cell { form_id: id, grid: g, water, flags, hide, offset: r.offset });
                }
            }
            Entry::Group(g) if g.group_type == group_type::CELL_CHILDREN => {
                scan_cell_children(lp, plugin_index, g, grid, &mut items)?;
            }
            _ => {}
        }
    }
    Ok(items)
}

/// Grid position and land hide flags (XCLC), water height (XCLW) and flags (DATA) of an exterior
/// CELL.
fn cell_info(r: &Record<'_>) -> Result<(Option<(i32, i32)>, Option<f32>, u16, u32)> {
    let data = r.data()?;
    let (mut grid, mut water, mut flags, mut hide) = (None, None, 0, 0);
    for f in fields(&data) {
        match &f.sig.0 {
            b"XCLC" if f.data.len() >= 8 => {
                grid = Some((i32_at(f.data, 0), i32_at(f.data, 4)));
                if f.data.len() >= 12 {
                    hide = u32::from(f.data[8]);
                }
            }
            b"XCLW" if f.data.len() >= 4 => water = Some(f32_at(f.data, 0)),
            b"DATA" if f.data.len() == 1 => flags = u16::from(f.data[0]) | CELL_SHORT_DATA,
            b"DATA" if f.data.len() >= 2 => flags = u16::from_le_bytes([f.data[0], f.data[1]]),
            _ => {}
        }
    }
    Ok((grid, water, flags, hide))
}

fn scan_cell_children(lp: &LoadedPlugin, plugin_index: u16, g: Group<'_>, cell: Option<(i32, i32)>, items: &mut Vec<Item>) -> Result<()> {
    for e in g.entries() {
        let Entry::Group(sub) = e? else { continue };
        if sub.group_type != group_type::CELL_PERSISTENT && sub.group_type != group_type::CELL_TEMPORARY {
            continue;
        }
        for e in sub.entries() {
            let Entry::Record(r) = e? else { continue };
            let Some(id) = lp.global_form_id(r.form_id) else { continue };
            match &r.sig.0 {
                b"NAVM" => {
                    if let Some(c) = cell && !r.is_deleted() {
                        items.push(Item::Navmesh { cell: c });
                    }
                }
                b"LAND" => {
                    if let Some(c) = cell {
                        items.push(Item::Land { form_id: id, cell: c, offset: r.offset });
                    }
                }
                b"REFR" => {
                    if r.is_deleted() {
                        items.push(Item::Ref { form_id: id, deleted: true, placement: empty_placement(plugin_index) });
                    } else if let Some(p) = placement(lp, plugin_index, &r, cell)? {
                        items.push(Item::Ref { form_id: id, deleted: false, placement: p });
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn empty_placement(plugin: u16) -> Placement {
    Placement { base: 0, position: [0.0; 3], rotation: [0.0; 3], scale: 1.0, has_scale: false, enable_parent: false, enable_parent_id: 0, enable_parent_flags: 0, flags: 0, cell: None, plugin, offset: 0 }
}

fn placement(lp: &LoadedPlugin, plugin_index: u16, r: &Record<'_>, cell: Option<(i32, i32)>) -> Result<Option<Placement>> {
    let data = r.data()?;
    let mut p = Placement { flags: r.flags, cell, offset: r.offset, ..empty_placement(plugin_index) };
    let mut has_base = false;
    for f in fields(&data) {
        match &f.sig.0 {
            b"NAME" => {
                if let Some(b) = f.u32().and_then(|id| lp.global_form_id(id)) {
                    p.base = b;
                    has_base = true;
                }
            }
            b"DATA" if f.data.len() >= 24 => {
                for i in 0..3 {
                    p.position[i] = f32_at(f.data, i * 4);
                    p.rotation[i] = f32_at(f.data, 12 + i * 4);
                }
            }
            b"XSCL" if f.data.len() >= 4 => {
                p.scale = f32_at(f.data, 0);
                p.has_scale = true;
            }
            b"XESP" => {
                p.enable_parent = true;
                if f.data.len() >= 5 {
                    p.enable_parent_id = lp.global_form_id(u32::from_le_bytes([f.data[0], f.data[1], f.data[2], f.data[3]])).unwrap_or(0);
                    p.enable_parent_flags = f.data[4];
                }
            }
            _ => {}
        }
    }
    Ok(has_base.then_some(p))
}
