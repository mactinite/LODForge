// SPDX-License-Identifier: MPL-2.0
// This file is part of LODForge; it is an MPL-2.0 port of xEdit's wbLOD.pas and stays MPL-2.0
// inside the GPL-3.0-only work (MPL 2.0 section 3.3). See LICENSE and THIRD-PARTY-NOTICES.txt.
//! Tree LOD for Skyrim SE, ported from the tree branch of xEdit's `wbGenerateLODTES5` and
//! `TwbLodTES5TreeList` / `TwbLodTES5TreeBlock` (`Core\wbLOD.pas`, MPL):
//!
//! - billboards: `textures\terrain\LODGen\<plugin>\<model>_<object id>.dds` (+ `.txt` with
//!   `[LOD]` Width/Height/ShiftX/Y/Z/Scale), one per TREE (or STAT with "Has Tree LOD");
//! - `meshes\terrain\<world>\trees\<world>.lst`: tree types with their atlas UV rectangles;
//! - `meshes\terrain\<world>\trees\<world>.4.<x>.<y>.btt`: per LOD4 block, references per type;
//! - `textures\terrain\<world>\trees\<world>treelod.dds`: the billboard atlas (packed here,
//!   pixels composed by the caller).
//!
//! xEdit gives each tree a random rotation; here it is derived from the reference's form ID so
//! builds are deterministic.

use std::collections::{HashMap, HashSet};

use lodforge_esp::record::{FLAG_DELETED, FLAG_INITIALLY_DISABLED, FLAG_PERSISTENT};
use lodforge_esp::{LoadOrder, Sig, Worldspace, fields, winning_records};
use lodforge_objlod::source::angle_as_lodgen;

/// A TREE, or a STAT flagged "Has Tree LOD" (winning override data).
#[derive(Debug, Clone, Default)]
pub struct TreeBase {
    pub signature: [u8; 4],
    pub flags: u32,
    pub model: String,
    /// OBND (x1, y1, z1, x2, y2, z2).
    pub bounds: Option<[i16; 6]>,
    /// File name of the plugin that defines the record (its master).
    pub file: String,
}

pub const STAT_HAS_TREE_LOD: u32 = 0x40;
pub const REFR_IS_FULL_LOD: u32 = 0x10000;

/// The plugin a global form ID belongs to.
pub fn defining_plugin(lo: &LoadOrder, id: u32) -> Option<&str> {
    let prefix = if id >> 24 == 0xFE { id & 0xFFFF_F000 } else { id & 0xFF00_0000 };
    lo.plugins.iter().find(|p| p.index.global(0) == prefix).map(|p| p.plugin.name.as_str())
}

pub fn read_tree_bases(lo: &LoadOrder) -> lodforge_esp::Result<HashMap<u32, TreeBase>> {
    let mut out = HashMap::new();
    for sig in [*b"TREE", *b"STAT"] {
        for (id, (_, r)) in winning_records(lo, Sig(sig))? {
            if &sig == b"STAT" && r.flags & STAT_HAS_TREE_LOD == 0 {
                continue;
            }
            let data = r.data()?;
            let mut b = TreeBase { signature: sig, flags: r.flags, file: defining_plugin(lo, id).unwrap_or_default().to_string(), ..Default::default() };
            for f in fields(&data) {
                match &f.sig.0 {
                    b"MODL" => b.model = f.zstring(),
                    b"OBND" if f.data.len() >= 12 => {
                        let v = |i: usize| i16::from_le_bytes([f.data[i * 2], f.data[i * 2 + 1]]);
                        b.bounds = Some([v(0), v(1), v(2), v(3), v(4), v(5)]);
                    }
                    _ => {}
                }
            }
            out.insert(id, b);
        }
    }
    Ok(out)
}

/// `TwbLodTES5TreeList.BillboardFileName`.
pub fn billboard_name(file: &str, model: &str, form_id: u32) -> String {
    let stem = model.rsplit(['\\', '/']).next().unwrap_or(model);
    let stem = stem.rsplit_once('.').map(|(a, _)| a).unwrap_or(stem);
    let object = if form_id >> 24 == 0xFE { form_id & 0xFFF } else { form_id & 0x00FF_FFFF };
    format!("Textures\\Terrain\\LODGen\\{file}\\{stem}_{object:08X}.dds")
}

/// One billboard: size, per-tree tuning from its `.txt`, and a checksum of the DDS bytes
/// (identical billboards share an atlas slot).
#[derive(Debug, Clone)]
pub struct Tree {
    pub index: i32,
    pub form_id: u32,
    pub billboard: String,
    pub crc: u32,
    pub width: f32,
    pub height: f32,
    pub shift: [f32; 3],
    pub scale: f32,
    /// Billboard image size in pixels (0 when missing).
    pub image: (u32, u32),
}

/// Reads a billboard's companion ini (`[LOD]` section keys).
fn read_ini(text: &str) -> HashMap<String, f32> {
    let mut out = HashMap::new();
    let mut in_lod = false;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            in_lod = l.eq_ignore_ascii_case("[LOD]");
            continue;
        }
        if !in_lod {
            continue;
        }
        if let Some((k, v)) = l.split_once('=')
            && let Ok(v) = v.trim().parse::<f32>()
        {
            out.insert(k.trim().to_ascii_lowercase(), v);
        }
    }
    out
}

/// CRC-32 (IEEE), xEdit's `TwbHash.CRC32`.
pub fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
        }
    }
    !c
}

/// One BTT reference (32 bytes).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TreeRef {
    pub pos: [f32; 3],
    pub rotation: f32,
    pub scale: f32,
    pub form_id: u32,
}

/// A LOD4 block: tree types in first-use order, each with its references.
#[derive(Debug, Clone, Default)]
pub struct Block {
    pub cell: (i32, i32),
    pub types: Vec<(i32, Vec<TreeRef>)>,
}

/// LST entry (32 bytes).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TreeType {
    pub index: i32,
    pub width: f32,
    pub height: f32,
    pub uv_min: [f32; 2],
    pub uv_max: [f32; 2],
}

/// Placement of one unique billboard on the atlas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AtlasSlot {
    pub crc: u32,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

#[derive(Debug, Default)]
pub struct TreeLod {
    pub world: String,
    pub trees: Vec<Tree>,
    pub blocks: Vec<Block>,
    pub types: Vec<TreeType>,
    pub atlas: (u32, u32),
    pub slots: Vec<AtlasSlot>,
    pub duplicates: usize,
    pub missing_billboards: Vec<String>,
}

/// Source of game files (VFS).
pub trait Files {
    fn read(&self, path: &str) -> Option<Vec<u8>>;
    /// Width and height of a DDS.
    fn dds_size(&self, bytes: &[u8]) -> Option<(u32, u32)>;
}

/// Gap between billboards on the atlas, as xLODGen 4.1.5o lays it out (golden: 512-px blocks 528 apart).
const ATLAS_PADDING: u32 = 16;

/// Deterministic stand-in for xEdit's `2*Pi*Random`.
fn rotation_for(form_id: u32) -> f32 {
    let mut x = u64::from(form_id).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    x ^= x >> 31;
    ((x >> 40) as f32 / (1u64 << 24) as f32) * std::f32::consts::TAU
}

/// Which references go to tree LOD. The default is xLODGen 4.1.5o; `dyndolod` switches to DynDOLOD
/// 3.0 (checked against its debug log and output for Tamriel).
#[derive(Debug, Clone, Default)]
pub struct Rules {
    pub dyndolod: bool,
    /// `TreeLODMaxTilt` (degrees, DynDOLOD).
    pub max_tilt: f64,
    /// Child worldspace `Bounds` (SW x, SW y, NE x, NE y cells) whose trees use object LOD instead.
    pub object_bounds: Vec<(i32, i32, i32, i32)>,
    /// LOD4 blocks written even without trees (DynDOLOD: every terrain LOD4 quad).
    pub blocks: Vec<(i32, i32)>,
    /// References left out (DynDOLOD: dynamic LOD references and those its reference rules
    /// disable).
    pub exclude: HashSet<u32>,
}

impl Rules {
    pub fn dyndolod(max_tilt: f64, object_bounds: Vec<(i32, i32, i32, i32)>, blocks: Vec<(i32, i32)>) -> Rules {
        Rules { dyndolod: true, max_tilt, object_bounds, blocks, exclude: HashSet::new() }
    }
}

/// DynDOLOD's tilt test: xEdit's `max < angle < 360 - max` window on the x or y rotation, in the
/// degrees DynDOLOD reads (stored radians, not normalised, rounded to 4 decimals, as single).
/// Stored negative angles therefore stay in tree LOD whatever their size. Exactly the 391 Tamriel
/// exclusions in DynDOLOD's debug log, none extra (`lodforge_dyndolod::export::tree_tilted`).
pub fn dyndolod_tilted(rotation: [f32; 3], max: f64) -> bool {
    let tilted = |rad: f32| {
        let d = (f64::from(rad) * 180.0 / std::f64::consts::PI * 1e4).round_ties_even() / 1e4;
        let d = f64::from(d as f32);
        d > max && d < 360.0 - max
    };
    tilted(rotation[0]) || tilted(rotation[1])
}

/// Builds tree LOD for `world`. `south_west` is the lodsettings cell; `swaps` maps base records
/// for a season.
pub fn build(world: &Worldspace, bases: &HashMap<u32, TreeBase>, south_west: (i32, i32), swaps: &HashMap<u32, u32>, files: &dyn Files, max_atlas: u32) -> TreeLod {
    build_with(world, bases, south_west, swaps, files, max_atlas, &Rules::default())
}

/// [`build`] with explicit reference rules.
pub fn build_with(world: &Worldspace, bases: &HashMap<u32, TreeBase>, south_west: (i32, i32), swaps: &HashMap<u32, u32>, files: &dyn Files, max_atlas: u32, rules: &Rules) -> TreeLod {
    let mut out = TreeLod { world: world.editor_id.clone(), ..Default::default() };
    out.blocks.extend(rules.blocks.iter().map(|&cell| Block { cell, types: Vec::new() }));
    let mut tree_of: HashMap<u32, usize> = HashMap::new();
    let mut seen_objects: HashSet<u32> = HashSet::new();
    let mut ids: Vec<u32> = world.refs.keys().copied().collect();
    ids.sort_unstable();
    for id in ids {
        let p = &world.refs[&id];
        let base_id = swaps.get(&p.base).copied().unwrap_or(p.base);
        let Some(b) = bases.get(&base_id) else { continue };
        let cell = ((p.position[0] / 4096.0).floor() as i32, (p.position[1] / 4096.0).floor() as i32);
        let block = (south_west.0 + (cell.0 - south_west.0) / 4 * 4, south_west.1 + (cell.1 - south_west.1) / 4 * 4);
        if block.0 < south_west.0 || block.1 < south_west.1 {
            continue;
        }
        // Blocks exist for every cell with tree references, so empty blocks override vanilla ones.
        let bi = match out.blocks.iter().position(|bl| bl.cell == block) {
            Some(i) => i,
            None => {
                out.blocks.push(Block { cell: block, types: Vec::new() });
                out.blocks.len() - 1
            }
        };
        // xLODGen 4.1.5o leaves out references defined in light plugins (FExxxyyy): golden Tamriel
        // has none of the 14 tree refs from four CC .esl files. Not in the public source.
        // DynDOLOD too ("Ignored tree reference for tree LOD added by ESL"); it keeps trees with
        // enable parents.
        if id >> 24 == 0xFE {
            continue;
        }
        if p.flags & (FLAG_INITIALLY_DISABLED | FLAG_DELETED) != 0 || (p.enable_parent && !rules.dyndolod) {
            continue;
        }
        if rules.exclude.contains(&id) {
            continue;
        }
        // DynDOLOD converts these "neverfade" trees and keeps them in tree LOD.
        if p.flags & FLAG_PERSISTENT != 0 && p.flags & REFR_IS_FULL_LOD != 0 && !rules.dyndolod {
            continue;
        }
        // Fallen trees (tilted more than 30 degrees around x or y) are left out. xLODGen 4.1.5o checks
        // this before adding the tree to the list (public source: after), which decides tree indices.
        // Degrees as xEdit reads them (normalised, rounded to 4 decimals): 5.7595863 rad is 330.0
        // and stays, 0.5235988 rad is 30.0.
        if rules.dyndolod {
            if dyndolod_tilted(p.rotation, rules.max_tilt) || rules.object_bounds.iter().any(|&(x0, y0, x1, y1)| (x0..=x1).contains(&cell.0) && (y0..=y1).contains(&cell.1)) {
                continue;
            }
        } else {
            let (rx, ry) = (angle_as_lodgen(p.rotation[0]), angle_as_lodgen(p.rotation[1]));
            if (rx > 30.0 && rx < 330.0) || (ry > 30.0 && ry < 330.0) {
                continue;
            }
        }
        let ti = *tree_of.entry(base_id).or_insert_with(|| {
            let width = b.bounds.map(|o| f32::from(o[3]) - f32::from(o[0])).unwrap_or(0.0);
            let height = b.bounds.map(|o| f32::from(o[5]) - f32::from(o[2])).unwrap_or(0.0);
            let next = out.trees.iter().map(|t| t.index).max().unwrap_or(-1) + 1;
            let billboard = billboard_name(&b.file, &b.model, base_id);
            let mut t = Tree { index: next, form_id: base_id, billboard: billboard.clone(), crc: 0, width, height, shift: [0.0; 3], scale: 1.0, image: (0, 0) };
            match files.read(&billboard.to_lowercase()).and_then(|d| files.dds_size(&d).map(|s| (d, s))) {
                Some((data, size)) => {
                    t.crc = crc32(&data);
                    t.image = size;
                    let txt = billboard.rsplit_once('.').map(|(a, _)| format!("{a}.txt")).unwrap_or_default();
                    if let Some(ini) = files.read(&txt.to_lowercase()).map(|b| read_ini(&String::from_utf8_lossy(&b))) {
                        if let Some(&w) = ini.get("width").filter(|v| **v != 0.0) {
                            t.width = w;
                        }
                        if let Some(&h) = ini.get("height").filter(|v| **v != 0.0) {
                            t.height = h;
                        }
                        t.shift = [ini.get("shiftx").copied().unwrap_or(0.0), ini.get("shifty").copied().unwrap_or(0.0), ini.get("shiftz").copied().unwrap_or(0.0)];
                        t.scale = ini.get("scale").copied().unwrap_or(1.0);
                    }
                }
                None => {
                    out.missing_billboards.push(billboard);
                    t.index = -1;
                }
            }
            out.trees.push(t);
            out.trees.len() - 1
        });
        let t = &out.trees[ti];
        if t.index == -1 {
            continue;
        }
        // xEdit reads position and XSCL through RoundToEx(v, -6) (0.79999995 -> 0.8, 0.97000003 ->
        // 0.96999997) and stores Singles.
        let pos = [round6(p.position[0]) + t.shift[0], round6(p.position[1]) + t.shift[1], round6(p.position[2]) + t.shift[2]];
        let scale = if p.has_scale { round6(p.scale) } else { 1.0 } * t.scale;
        // Duplicate check on the object id (light plugins: the low 12 bits).
        // DynDOLOD does the same ("Ignored duplicate tree reference": 170BC4B0 after 000BC4B0).
        let object = id & if id >> 24 == 0xFE { 0xFFF } else { 0x00FF_FFFF };
        if !seen_objects.insert(object) {
            out.duplicates += 1;
            continue;
        }
        let (index, r) = (t.index, TreeRef { pos, rotation: rotation_for(id), scale, form_id: id });
        let bl = &mut out.blocks[bi];
        match bl.types.iter_mut().find(|(i, _)| *i == index) {
            Some((_, v)) => v.push(r),
            None => bl.types.push((index, vec![r])),
        }
    }
    pack_atlas(&mut out, max_atlas);
    out
}

/// `TwbLodTES5TreeList.BuildAtlas`: unique billboards (by CRC) packed with 16-pixel padding (xLODGen
/// 4.1.5o; the public xEdit source pads 2) on a
/// 512x512 page that doubles (width first while not wider) until everything fits.
fn pack_atlas(out: &mut TreeLod, max_atlas: u32) {
    let mut blocks: Vec<(u32, u32, u32)> = Vec::new();
    for t in &out.trees {
        if t.index == -1 || blocks.iter().any(|b| b.0 == t.crc) {
            continue;
        }
        blocks.push((t.crc, t.image.0, t.image.1));
    }
    if blocks.is_empty() {
        return;
    }
    let (mut w, mut h) = (512u32.min(max_atlas), 512u32.min(max_atlas));
    let placed = loop {
        if let Some(p) = pack(&blocks, w, h, ATLAS_PADDING) {
            break p;
        }
        if w <= h {
            w *= 2;
        } else {
            h *= 2;
        }
        if w > max_atlas || h > max_atlas {
            return;
        }
    };
    out.atlas = (w, h);
    out.slots = placed;
    for t in &out.trees {
        if t.index == -1 {
            continue;
        }
        let s = out.slots.iter().find(|s| s.crc == t.crc).unwrap();
        out.types.push(TreeType {
            index: t.index,
            width: t.width,
            height: t.height,
            uv_min: [s.x as f32 / w as f32, s.y as f32 / h as f32],
            uv_max: [(s.x + s.w) as f32 / w as f32, (s.y + s.h) as f32 / h as f32],
        });
    }
}

/// `TwbBinPacker.Fit` with padding: stable bubble sort by (height, width) descending (xLODGen 4.1.5o,
/// as for the object atlas; the public source sorts by the larger side), first free node.
fn pack(blocks: &[(u32, u32, u32)], width: u32, height: u32, pad: u32) -> Option<Vec<AtlasSlot>> {
    let mut b: Vec<(u32, u32, u32)> = blocks.to_vec();
    let mut changed = true;
    while changed {
        changed = false;
        for i in 0..b.len().saturating_sub(1) {
            if (b[i].2, b[i].1) < (b[i + 1].2, b[i + 1].1) {
                b.swap(i, i + 1);
                changed = true;
            }
        }
    }
    // Free rectangles as a tree flattened into a list of (x, y, w, h, used, right, down).
    #[derive(Clone, Copy)]
    struct N {
        x: i64,
        y: i64,
        w: i64,
        h: i64,
        used: bool,
        right: usize,
        down: usize,
    }
    const NONE: usize = usize::MAX;
    let mut nodes = vec![N { x: 0, y: 0, w: i64::from(width), h: i64::from(height), used: false, right: NONE, down: NONE }];
    fn find(nodes: &[N], i: usize, w: i64, h: i64) -> Option<usize> {
        if i == NONE {
            return None;
        }
        let n = nodes[i];
        if n.used { find(nodes, n.right, w, h).or_else(|| find(nodes, n.down, w, h)) } else if w <= n.w && h <= n.h { Some(i) } else { None }
    }
    let pad = i64::from(pad);
    let mut out = Vec::new();
    for (crc, bw, bh) in b {
        let (bw, bh) = (i64::from(bw), i64::from(bh));
        let i = find(&nodes, 0, bw, bh)?;
        let n = nodes[i];
        let down = N { x: n.x, y: n.y + bh + pad, w: n.w, h: n.h - bh - pad, used: false, right: NONE, down: NONE };
        // xLODGen 4.1.5o: the right node keeps the padding row (h + pad; public source: h).
        let right = N { x: n.x + bw + pad, y: n.y, w: n.w - bw - pad, h: bh + pad, used: false, right: NONE, down: NONE };
        nodes.push(down);
        nodes.push(right);
        let len = nodes.len();
        nodes[i].used = true;
        nodes[i].down = len - 2;
        nodes[i].right = len - 1;
        out.push(AtlasSlot { crc, x: n.x as u32, y: n.y as u32, w: bw as u32, h: bh as u32 });
    }
    Some(out)
}

impl TreeLod {
    pub fn list_path(&self) -> String {
        format!("meshes\\terrain\\{w}\\trees\\{w}.lst", w = self.world)
    }

    pub fn atlas_path(&self) -> String {
        format!("textures\\terrain\\{w}\\trees\\{w}treelod.dds", w = self.world)
    }

    pub fn block_path(&self, b: &Block, season: Option<&str>) -> String {
        match season {
            Some(s) => format!("meshes\\terrain\\{w}\\trees\\{w}.4.{}.{}.{s}.btt", b.cell.0, b.cell.1, w = self.world),
            None => format!("meshes\\terrain\\{w}\\trees\\{w}.4.{}.{}.btt", b.cell.0, b.cell.1, w = self.world),
        }
    }

    /// LST bytes: count + 32-byte entries.
    pub fn list_bytes(&self) -> Vec<u8> {
        let mut o = Vec::with_capacity(4 + self.types.len() * 32);
        o.extend_from_slice(&(self.types.len() as i32).to_le_bytes());
        for t in &self.types {
            o.extend_from_slice(&t.index.to_le_bytes());
            for v in [t.width, t.height, t.uv_min[0], t.uv_min[1], t.uv_max[0], t.uv_max[1]] {
                o.extend_from_slice(&v.to_le_bytes());
            }
            o.extend_from_slice(&0i32.to_le_bytes());
        }
        o
    }

    /// BTT bytes: type count, then per type its index, count and 32-byte references.
    pub fn block_bytes(b: &Block) -> Vec<u8> {
        let mut o = Vec::new();
        o.extend_from_slice(&(b.types.len() as i32).to_le_bytes());
        for (index, refs) in &b.types {
            o.extend_from_slice(&index.to_le_bytes());
            o.extend_from_slice(&(refs.len() as i32).to_le_bytes());
            for r in refs {
                for v in [r.pos[0], r.pos[1], r.pos[2], r.rotation, r.scale] {
                    o.extend_from_slice(&v.to_le_bytes());
                }
                o.extend_from_slice(&r.form_id.to_le_bytes());
                o.extend_from_slice(&0i32.to_le_bytes());
                o.extend_from_slice(&0i32.to_le_bytes());
            }
        }
        o
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_crc() {
        assert_eq!(billboard_name("Skyrim.esm", "Landscape\\Trees\\TreePineForest01.nif", 0x0006_E6FB), "Textures\\Terrain\\LODGen\\Skyrim.esm\\TreePineForest01_0006E6FB.dds");
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        let p = pack(&[(1, 256, 512), (2, 256, 256), (3, 256, 256)], 512, 512, 2);
        assert!(p.is_none());
        let p = pack(&[(1, 256, 512), (2, 128, 128)], 512, 512, 2).unwrap();
        assert_eq!((p[1].x, p[1].y), (258, 0));
    }
}

/// xEdit's `RoundToEx(v, -6)` with the factor in single precision and banker's rounding (as in
/// `lodforge-objlod`), stored back as a Single.
fn round6(v: f32) -> f32 {
    let f = f64::from(10f32.powi(-6));
    ((f64::from(v) / f).round_ties_even() * f) as f32
}
