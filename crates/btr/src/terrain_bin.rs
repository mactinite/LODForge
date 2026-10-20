//! Optional import / export of the terrain cell dump xLODGen writes next to its control file
//! (`LODGen_Terrain_<World>.bin`; DynDOLOD: `Export\LODGen_SSE_LODGen_Terrain_<W>.bin`), as
//! observed (docs/re/terrain-lod-spec.md 2.5): 1,112 bytes per cell, little endian, no header:
//! x i32, y i32, water f32, flags i32 (DynDOLOD: the cell's hide-quadrant bits), then the 1,096
//! bytes of the LAND `VHGT` subrecord (offset f32, 33 x 33 signed byte deltas, 3 pad bytes).
//! Cells are ordered by (y, x). A water value at or beyond +-2^24 (xLODGen writes -FLT_MAX)
//! means "no water".
//!
//! The pipeline itself works from [`TerrainInput`]; this is for comparisons against the
//! reference tools' own input and for building DynDOLOD's object LOD input.

use crate::input::{GRID, TerrainCell, TerrainInput};

pub const CELL_RECORD_LEN: usize = 16 + 4 + GRID * GRID + 3;
pub const VHGT_LEN: usize = 4 + GRID * GRID + 3;

/// One record of the dump.
#[derive(Debug, Clone, PartialEq)]
pub struct BinCell {
    pub x: i32,
    pub y: i32,
    /// As stored; `f32::MIN` / huge values mean no water (see [`BinCell::water`]).
    pub water_height: f32,
    pub flags: i32,
    /// Decoded heights in game units, `heights[col][row]` (column 0 west, row 0 south).
    pub heights: [[f32; GRID]; GRID],
    /// The raw VHGT bytes.
    pub vhgt: Vec<u8>,
}

impl BinCell {
    /// Water height, `None` for the "no water" marker.
    pub fn water(&self) -> Option<f32> {
        (self.water_height.abs() < 16_777_216.0).then_some(self.water_height)
    }

    pub fn to_terrain_cell(&self) -> TerrainCell {
        let mut heights = vec![0f32; GRID * GRID];
        for row in 0..GRID {
            for col in 0..GRID {
                heights[row * GRID + col] = self.heights[col][row];
            }
        }
        TerrainCell {
            x: self.x,
            y: self.y,
            water: self.water(),
            heights,
            hide_quadrants: (self.flags & 0xF) as u8,
        }
    }
}

/// VHGT decoding (UESP, LAND): heights in units of 8; each row's first value is relative to the
/// previous row's first value, the rest to the previous column.
pub fn decode_vhgt(vhgt: &[u8]) -> Vec<f32> {
    let mut h = vec![0f32; GRID * GRID];
    let mut row_start = f32::from_le_bytes([vhgt[0], vhgt[1], vhgt[2], vhgt[3]]);
    for row in 0..GRID {
        row_start += f32::from(vhgt[4 + row * GRID] as i8);
        let mut v = row_start;
        h[row * GRID] = v * 8.0;
        for col in 1..GRID {
            v += f32::from(vhgt[4 + row * GRID + col] as i8);
            h[row * GRID + col] = v * 8.0;
        }
    }
    h
}

/// VHGT bytes for a height grid (game units); heights are rounded to units of 8 and deltas
/// clamped to a signed byte, as the format allows.
pub fn encode_vhgt(heights: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(VHGT_LEN);
    let units: Vec<f32> = heights.iter().map(|h| (h / 8.0).round()).collect();
    out.extend_from_slice(&units[0].to_le_bytes());
    let mut prev_row_start = units[0];
    for row in 0..GRID {
        let start = units[row * GRID];
        let d = if row == 0 {
            0.0
        } else {
            start - prev_row_start
        };
        out.push((d.clamp(-128.0, 127.0) as i8) as u8);
        prev_row_start = start;
        let mut prev = start;
        for col in 1..GRID {
            let v = units[row * GRID + col];
            out.push(((v - prev).clamp(-128.0, 127.0) as i8) as u8);
            prev = v;
        }
    }
    out.extend_from_slice(&[0, 0, 0]);
    out
}

pub fn decode_cell(record: &[u8]) -> BinCell {
    let i32_at =
        |o: usize| i32::from_le_bytes([record[o], record[o + 1], record[o + 2], record[o + 3]]);
    let vhgt = record[16..16 + VHGT_LEN].to_vec();
    let flat = decode_vhgt(&vhgt);
    let mut heights = [[0f32; GRID]; GRID];
    for row in 0..GRID {
        for col in 0..GRID {
            heights[col][row] = flat[row * GRID + col];
        }
    }
    BinCell {
        x: i32_at(0),
        y: i32_at(4),
        water_height: f32::from_le_bytes([record[8], record[9], record[10], record[11]]),
        flags: i32_at(12),
        heights,
        vhgt,
    }
}

/// One record from its parts (`vhgt` = the 1,096 VHGT bytes).
pub fn encode_cell(x: i32, y: i32, water: f32, flags: i32, vhgt: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(CELL_RECORD_LEN);
    out.extend_from_slice(&x.to_le_bytes());
    out.extend_from_slice(&y.to_le_bytes());
    out.extend_from_slice(&water.to_le_bytes());
    out.extend_from_slice(&flags.to_le_bytes());
    out.extend_from_slice(&vhgt[..VHGT_LEN]);
    out
}

pub fn read_cells(bytes: &[u8]) -> Result<Vec<BinCell>, String> {
    if bytes.len() % CELL_RECORD_LEN != 0 {
        return Err(format!(
            "{} bytes is not a multiple of the {CELL_RECORD_LEN}-byte cell record",
            bytes.len()
        ));
    }
    Ok(bytes
        .chunks_exact(CELL_RECORD_LEN)
        .map(decode_cell)
        .collect())
}

/// A [`TerrainInput`] from a dump plus the control-file values it does not carry.
pub fn terrain_input(
    cells: &[BinCell],
    world: &str,
    south_west: (i32, i32),
    max_level: i32,
    default_land: f32,
    default_water: Option<f32>,
) -> TerrainInput {
    let mut t = TerrainInput {
        world: world.to_string(),
        south_west,
        max_level,
        default_land,
        default_water,
        cells: cells.iter().map(BinCell::to_terrain_cell).collect(),
    };
    t.normalize();
    t
}

/// The dump of a [`TerrainInput`] (water `-f32::MAX` where a cell has none; flags = hide bits).
pub fn write_cells(input: &TerrainInput) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.cells.len() * CELL_RECORD_LEN);
    for c in &input.cells {
        out.extend_from_slice(&encode_cell(
            c.x,
            c.y,
            c.water.unwrap_or(-f32::MAX),
            i32::from(c.hide_quadrants),
            &encode_vhgt(&c.heights),
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vhgt_round_trip() {
        let heights: Vec<f32> = (0..GRID * GRID)
            .map(|i| ((i % 37) as f32 - 18.0) * 8.0 + 1000.0)
            .collect();
        let v = encode_vhgt(&heights);
        assert_eq!(v.len(), VHGT_LEN);
        assert_eq!(decode_vhgt(&v), heights);
        let rec = encode_cell(-3, 7, -14000.0, 5, &v);
        assert_eq!(rec.len(), CELL_RECORD_LEN);
        let c = decode_cell(&rec);
        assert_eq!((c.x, c.y, c.water(), c.flags), (-3, 7, Some(-14000.0), 5));
        assert_eq!(c.heights[5][2], heights[2 * GRID + 5]);
        let tc = c.to_terrain_cell();
        assert_eq!(tc.heights, heights);
        assert_eq!(tc.hide_quadrants, 5);
        let none = decode_cell(&encode_cell(0, 0, -f32::MAX, 0, &v));
        assert_eq!(none.water(), None);
    }
}
