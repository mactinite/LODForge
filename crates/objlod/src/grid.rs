//! The LOD grid of a worldspace (docs/re/object-lod-spec.md 1.3): `lodsettings\<World>.lod`,
//! the block a cell belongs to at each level, and the output file names.
//!
//! `LodSettings::parse` and `LodGrid::block_origin` / `is_inside` follow AnvilLOD
//! (Copyright (c) 2026 CageTV, GPL-3.0-only, commit 606eece, `AnvilLOD.Core/World/LodSettings.cs`
//! and `LodGrid.cs`), adapted to a south-west cell without a stride (the xLODGen export carries
//! only `CellSW`; then the inside test has no north-east bound, as in the xEdit writer).

/// Object LOD levels: the block edge length in cells.
pub const LEVELS: [i32; 4] = [4, 8, 16, 32];

/// Index of a level in [`LEVELS`] (4 -> 0, 8 -> 1, 16 -> 2, 32 -> 3); other values map to 0.
pub fn level_index(level: i32) -> usize {
    LEVELS.iter().position(|&l| l == level).unwrap_or(0)
}

/// One cell is 4096 game units.
pub const CELL_UNITS: f32 = 4096.0;

/// Cell of a world position (floor division, so negative coordinates land in the right cell).
pub fn cell_of(x: f32, y: f32) -> (i32, i32) {
    ((x / CELL_UNITS).floor() as i32, (y / CELL_UNITS).floor() as i32)
}

/// `Data\lodsettings\<World>.lod`: 16 bytes, little endian: `i16 swX, i16 swY, i32 stride,
/// i32 minLevel, i32 maxLevel` (UESP; spec 1.3). Tamriel: `-96, -96, 256, 4, 32`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LodSettings {
    pub south_west: (i32, i32),
    pub stride: i32,
    pub min_level: i32,
    pub max_level: i32,
}

impl LodSettings {
    pub fn parse(bytes: &[u8]) -> Option<LodSettings> {
        if bytes.len() < 16 {
            return None;
        }
        let i16_at = |o: usize| i32::from(i16::from_le_bytes([bytes[o], bytes[o + 1]]));
        let i32_at = |o: usize| i32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
        Some(LodSettings { south_west: (i16_at(0), i16_at(2)), stride: i32_at(4), min_level: i32_at(8), max_level: i32_at(12) })
    }

    pub fn to_bytes(&self) -> [u8; 16] {
        let mut b = [0u8; 16];
        b[0..2].copy_from_slice(&(self.south_west.0 as i16).to_le_bytes());
        b[2..4].copy_from_slice(&(self.south_west.1 as i16).to_le_bytes());
        b[4..8].copy_from_slice(&self.stride.to_le_bytes());
        b[8..12].copy_from_slice(&self.min_level.to_le_bytes());
        b[12..16].copy_from_slice(&self.max_level.to_le_bytes());
        b
    }

    pub fn supports(&self, level: i32) -> bool {
        level >= self.min_level && level <= self.max_level
    }
}

/// One output file: `<World>.<level>.<x>.<y>[.<SEASON>].bto`, `x`/`y` the block's south-west cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Quad {
    pub level: i32,
    pub x: i32,
    pub y: i32,
}

impl Quad {
    /// World-space origin of the block (south-west corner), the shape translation of spec 1.3.
    pub fn origin(&self) -> [f32; 3] {
        [self.x as f32 * CELL_UNITS, self.y as f32 * CELL_UNITS, 0.0]
    }

    /// LOD4 segment of a cell inside this quad: `x * 4 + y` with the offsets clamped to 0..3
    /// (settled on the golden `Tamriel.4.0.-16.bto`, spec open question 2).
    pub fn segment_of(&self, cell: (i32, i32)) -> usize {
        ((cell.0 - self.x).clamp(0, 3) * 4 + (cell.1 - self.y).clamp(0, 3)) as usize
    }
}

/// Output file name of a quad; the season code goes before the extension.
pub fn file_name(world: &str, q: &Quad, season: Option<&str>) -> String {
    match season {
        Some(s) if !s.is_empty() => format!("{world}.{}.{}.{}.{s}.bto", q.level, q.x, q.y),
        _ => format!("{world}.{}.{}.{}.bto", q.level, q.x, q.y),
    }
}

/// Terrain LOD file name of the same quad: `<World>.<level>.<x>.<y>.btr` (season files hold the
/// same geometry, so the default one serves every season).
pub fn file_name_btr(world: &str, q: &Quad) -> String {
    format!("{world}.{}.{}.{}.btr", q.level, q.x, q.y)
}

/// Cells -> blocks for one worldspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LodGrid {
    pub south_west: (i32, i32),
    /// Cells covered by the grid per axis; `None` when only the origin is known (no `.lod`).
    pub stride: Option<i32>,
}

impl LodGrid {
    pub fn new(south_west: (i32, i32), stride: Option<i32>) -> LodGrid {
        LodGrid { south_west, stride }
    }

    pub fn from_settings(s: &LodSettings) -> LodGrid {
        LodGrid { south_west: s.south_west, stride: Some(s.stride) }
    }

    /// South-west cell of the level-`level` block containing `cell`: `sw + floor((c - sw) /
    /// level) * level` per axis.
    pub fn block_origin(&self, cell: (i32, i32), level: i32) -> (i32, i32) {
        let sw = self.south_west;
        (sw.0 + (cell.0 - sw.0).div_euclid(level) * level, sw.1 + (cell.1 - sw.1).div_euclid(level) * level)
    }

    pub fn quad_for(&self, cell: (i32, i32), level: i32) -> Quad {
        let (x, y) = self.block_origin(cell, level);
        Quad { level, x, y }
    }

    /// `sw <= c` per axis, and `c < sw + stride` when the stride is known.
    pub fn is_inside(&self, cell: (i32, i32)) -> bool {
        let sw = self.south_west;
        if cell.0 < sw.0 || cell.1 < sw.1 {
            return false;
        }
        match self.stride {
            Some(s) => cell.0 < sw.0 + s && cell.1 < sw.1 + s,
            None => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tamriel_settings() {
        let s = LodSettings { south_west: (-96, -96), stride: 256, min_level: 4, max_level: 32 };
        assert_eq!(LodSettings::parse(&s.to_bytes()), Some(s));
        let g = LodGrid::from_settings(&s);
        // The farmhouse at (22176, -45866): cell (5, -12), LOD4 block (4, -12), LOD16 block (0, -16).
        let c = cell_of(22176.36, -45866.68);
        assert_eq!(c, (5, -12));
        assert_eq!(g.quad_for(c, 4), Quad { level: 4, x: 4, y: -12 });
        assert_eq!(g.quad_for(c, 16), Quad { level: 16, x: 0, y: -16 });
        assert_eq!(g.quad_for((-1, -1), 4), Quad { level: 4, x: -4, y: -4 });
        assert_eq!(g.quad_for((-97, 0), 4).x, -100);
        assert!(!g.is_inside((-97, 0)));
        assert!(g.is_inside((159, 159)));
        assert!(!g.is_inside((160, 0)));
        assert!(LodGrid::new((-96, -96), None).is_inside((160, 0)));
        assert_eq!(file_name("Tamriel", &Quad { level: 4, x: 0, y: -16 }, None), "Tamriel.4.0.-16.bto");
        assert_eq!(file_name("Tamriel", &Quad { level: 4, x: 0, y: -16 }, Some("WIN")), "Tamriel.4.0.-16.WIN.bto");
        let q = Quad { level: 4, x: 0, y: -16 };
        assert_eq!(q.segment_of((3, -16)), 12);
        assert_eq!(q.segment_of((2, -15)), 9);
        assert_eq!(q.segment_of((9, -20)), 12);
        assert_eq!(q.origin(), [0.0, -65536.0, 0.0]);
    }
}
