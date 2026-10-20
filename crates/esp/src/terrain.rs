//! Landscape data: LAND (heights, normals, vertex colours, texture layers) and the
//! LTEX -> TXST chain that names landscape texture files.
//!
//! LAND subrecords (SSE):
//! - `DATA` u32 flags
//! - `VNML` 33x33 x (i8 x, i8 y, i8 z) vertex normals
//! - `VHGT` f32 offset, 33x33 i8 deltas (row-wise running sum), 3 pad bytes; height = value * 8
//! - `VCLR` 33x33 x (u8 r, g, b) vertex colours
//! - `BTXT` LTEX u32, quadrant u8, pad u8, layer i16 — base texture of a quadrant
//! - `ATXT` LTEX u32, quadrant u8, pad u8, layer u16 — additional layer, followed by
//! - `VTXT` n x (position u16 in the quadrant's 17x17 grid, pad u16, opacity f32)
//!
//! Quadrants: 0 = bottom-left (south-west), 1 = bottom-right, 2 = top-left, 3 = top-right.
//! Grid rows run south to north (row 0 is the cell's southern edge).

use std::collections::HashMap;

use crate::load_order::{LoadOrder, LoadedPlugin};
use crate::record::{f32_at, fields, u16_at, u32_at, Entries, Entry, Record, Sig};
use crate::{Error, Result};

pub const GRID: usize = 33;
pub const QUAD_GRID: usize = 17;

#[derive(Debug, Clone, Default)]
pub struct Layer {
    /// Global LTEX form id.
    pub ltex: u32,
    pub layer: u16,
    /// Opacity per vertex of the quadrant's 17x17 grid (row-major, south row first); 0 where unset.
    pub opacity: Vec<f32>,
}

#[derive(Debug, Clone, Default)]
pub struct Quadrant {
    /// Global LTEX form id of the base texture; `None` means the default landscape texture.
    pub base: Option<u32>,
    /// Additional layers, sorted by layer index.
    pub layers: Vec<Layer>,
}

#[derive(Debug, Clone, Default)]
pub struct Land {
    pub flags: u32,
    /// 33x33 absolute heights in game units, row-major from the south-west corner.
    pub heights: Option<Vec<f32>>,
    pub normals: Option<Vec<[i8; 3]>>,
    pub colors: Option<Vec<[u8; 3]>>,
    pub quadrants: [Quadrant; 4],
}

impl Land {
    /// Every LTEX this cell references (bases and layers).
    pub fn textures(&self) -> impl Iterator<Item = u32> + '_ {
        self.quadrants.iter().flat_map(|q| q.base.into_iter().chain(q.layers.iter().map(|l| l.ltex)))
    }
}

impl LoadedPlugin {
    /// The record whose header starts at `offset` in this plugin.
    pub fn record_at(&self, offset: usize) -> Result<Record<'_>> {
        let b = self.plugin.bytes();
        match Entries::new(b.get(offset..).unwrap_or_default(), offset).next() {
            Some(Ok(Entry::Record(r))) => Ok(r),
            Some(Err(e)) => Err(e),
            _ => Err(Error::Malformed { offset, what: "no record at offset" }),
        }
    }

    /// Decodes a LAND record of this plugin.
    pub fn decode_land(&self, r: &Record<'_>) -> Result<Land> {
        let data = r.data()?;
        let mut land = Land::default();
        let mut current: Option<(usize, Layer)> = None;
        let finish = |land: &mut Land, cur: &mut Option<(usize, Layer)>| {
            if let Some((q, layer)) = cur.take() {
                land.quadrants[q].layers.push(layer);
            }
        };
        for f in fields(&data) {
            match &f.sig.0 {
                b"DATA" if f.data.len() >= 4 => land.flags = u32_at(f.data, 0),
                b"VHGT" if f.data.len() >= 4 + GRID * GRID => land.heights = Some(decode_vhgt(f.data)),
                b"VNML" if f.data.len() >= GRID * GRID * 3 => {
                    land.normals = Some(f.data.chunks_exact(3).take(GRID * GRID).map(|c| [c[0] as i8, c[1] as i8, c[2] as i8]).collect());
                }
                b"VCLR" if f.data.len() >= GRID * GRID * 3 => {
                    land.colors = Some(f.data.chunks_exact(3).take(GRID * GRID).map(|c| [c[0], c[1], c[2]]).collect());
                }
                b"BTXT" if f.data.len() >= 8 => {
                    finish(&mut land, &mut current);
                    let q = (f.data[4] & 3) as usize;
                    land.quadrants[q].base = self.global_form_id(u32_at(f.data, 0)).filter(|&id| id & 0x00FF_FFFF != 0);
                }
                b"ATXT" if f.data.len() >= 8 => {
                    finish(&mut land, &mut current);
                    let q = (f.data[4] & 3) as usize;
                    if let Some(ltex) = self.global_form_id(u32_at(f.data, 0)) {
                        let layer = Layer { ltex, layer: u16_at(f.data, 6), opacity: vec![0.0; QUAD_GRID * QUAD_GRID] };
                        current = Some((q, layer));
                    }
                }
                b"VTXT" => {
                    if let Some((_, layer)) = current.as_mut() {
                        for e in f.data.chunks_exact(8) {
                            let pos = u16_at(e, 0) as usize;
                            if pos < QUAD_GRID * QUAD_GRID {
                                layer.opacity[pos] = f32_at(e, 4);
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        finish(&mut land, &mut current);
        for q in &mut land.quadrants {
            q.layers.sort_by_key(|l| l.layer);
        }
        Ok(land)
    }
}

impl LoadedPlugin {
    /// The raw VHGT payload (offset f32, 1089 deltas, 3 pad bytes) of a LAND record.
    pub fn land_vhgt(&self, r: &Record<'_>) -> Result<Option<Vec<u8>>> {
        let data = r.data()?;
        Ok(fields(&data).find(|f| f.sig == Sig(*b"VHGT")).map(|f| f.data.to_vec()))
    }
}

/// VHGT: f32 offset then 33x33 i8 deltas. Each row's first delta is added to the previous row's
/// start value; the rest accumulate along the row. Result is in game units (x8).
pub fn decode_vhgt(d: &[u8]) -> Vec<f32> {
    let mut h = vec![0f32; GRID * GRID];
    let mut row_start = f32_at(d, 0);
    for y in 0..GRID {
        row_start += f32::from(d[4 + y * GRID] as i8);
        let mut v = row_start;
        h[y * GRID] = v * 8.0;
        for x in 1..GRID {
            v += f32::from(d[4 + y * GRID + x] as i8);
            h[y * GRID + x] = v * 8.0;
        }
    }
    h
}

/// Texture paths (Data-relative, as stored) of a landscape texture.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LandTexture {
    pub editor_id: String,
    pub diffuse: String,
    pub normal: String,
    /// Specular exponent (SNAM).
    pub specular_exponent: u8,
}

/// Winning version of every record of one top-level type, keyed by global form id.
pub fn winning_records<'a>(lo: &'a LoadOrder, sig: Sig) -> Result<HashMap<u32, (u16, Record<'a>)>> {
    let mut out = HashMap::new();
    for (i, lp) in lo.plugins.iter().enumerate() {
        let Some(g) = lp.plugin.top_group(sig)? else { continue };
        for e in g.entries() {
            if let Entry::Record(r) = e? {
                if let Some(id) = lp.global_form_id(r.form_id) {
                    if r.is_deleted() {
                        out.remove(&id);
                    } else {
                        out.insert(id, (i as u16, r));
                    }
                }
            }
        }
    }
    Ok(out)
}

/// Resolves every LTEX in the load order to its TXST texture paths.
pub fn land_textures(lo: &LoadOrder) -> Result<HashMap<u32, LandTexture>> {
    let txst = winning_records(lo, Sig(*b"TXST"))?;
    let ltex = winning_records(lo, Sig(*b"LTEX"))?;
    let mut out = HashMap::with_capacity(ltex.len());
    for (id, (pi, r)) in ltex {
        let lp = &lo.plugins[pi as usize];
        let data = r.data()?;
        let mut lt = LandTexture::default();
        let mut tnam = None;
        for f in fields(&data) {
            match &f.sig.0 {
                b"EDID" => lt.editor_id = f.zstring(),
                b"TNAM" => tnam = f.u32().and_then(|t| lp.global_form_id(t)),
                b"SNAM" if !f.data.is_empty() => lt.specular_exponent = f.data[0],
                _ => {}
            }
        }
        if let Some((_, tr)) = tnam.and_then(|t| txst.get(&t)) {
            let td = tr.data()?;
            for f in fields(&td) {
                match &f.sig.0 {
                    b"TX00" => lt.diffuse = f.zstring(),
                    b"TX01" => lt.normal = f.zstring(),
                    _ => {}
                }
            }
        }
        out.insert(id, lt);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vhgt_running_sum() {
        // offset 1; row 0: first delta +1, then +2 each; row 1: first delta -3, rest 0.
        let mut d = 1.0f32.to_le_bytes().to_vec();
        let mut grid = vec![0u8; GRID * GRID];
        grid[0] = 1;
        for g in grid.iter_mut().take(GRID).skip(1) {
            *g = 2;
        }
        grid[GRID] = (-3i8) as u8;
        d.extend(&grid);
        d.extend([0u8; 3]);
        let h = decode_vhgt(&d);
        assert_eq!(h[0], 16.0); // (1 + 1) * 8
        assert_eq!(h[GRID - 1], (2.0 + 2.0 * 32.0) * 8.0);
        assert_eq!(h[GRID], (2.0 - 3.0) * 8.0); // row start chains from row 0's start, not its end
        assert_eq!(h[2 * GRID - 1], -8.0);
    }
}