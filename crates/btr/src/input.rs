//! Terrain input: the cells of one worldspace as the mesher sees them (docs/re/terrain-lod-spec.md
//! section 2.2), and the height sampler over the world grid that every quad reads from.
//!
//! A cell holds the LAND height grid of 33 x 33 samples, 128 game units apart, row 0 south and
//! column 0 west (UESP, LAND VHGT). Neighbouring cells share their border rows and columns; the
//! sampler gives every world grid point one owner, so that neighbouring quads and cells read the
//! same height on a shared line (section 7.5: the cell to the north / east owns a shared line,
//! the other one is used only when that cell is missing).

use std::collections::HashMap;

/// LAND samples per cell side.
pub const GRID: usize = 33;
/// Grid steps per cell side.
pub const CELL_STEPS: i32 = 32;
/// Game units per grid step.
pub const STEP_UNITS: f32 = 128.0;
/// Game units per cell side.
pub const CELL_UNITS: f32 = 4096.0;

#[derive(Debug, Clone, PartialEq)]
pub struct TerrainCell {
    pub x: i32,
    pub y: i32,
    /// Water height in game units; `None` = the cell has no water (a one-byte CELL DATA, or
    /// `IgnoreWater`).
    pub water: Option<f32>,
    /// 33 x 33 heights in game units, row-major from the south-west corner: `heights[row * 33 + col]`.
    pub heights: Vec<f32>,
    /// Hidden land quadrants (CELL XCLC): bit 1 south-west, 2 south-east, 4 north-west, 8 north-east.
    pub hide_quadrants: u8,
}

impl TerrainCell {
    /// A flat cell at `land` (missing LAND / missing cell fill).
    pub fn flat(x: i32, y: i32, land: f32, water: Option<f32>) -> Self {
        TerrainCell {
            x,
            y,
            water,
            heights: vec![land; GRID * GRID],
            hide_quadrants: 0,
        }
    }

    #[inline]
    pub fn height(&self, col: usize, row: usize) -> f32 {
        self.heights[row * GRID + col]
    }

    pub fn min_height(&self) -> f32 {
        self.heights.iter().copied().fold(f32::INFINITY, f32::min)
    }

    /// Whether the quadrant containing grid point (col, row) of this cell is hidden. Points on a
    /// quadrant boundary belong to the quadrant with the lower index on that axis.
    pub fn quadrant_bit(col: usize, row: usize) -> u8 {
        let east = col >= 16;
        let north = row >= 16;
        match (north, east) {
            (false, false) => 1,
            (false, true) => 2,
            (true, false) => 4,
            (true, true) => 8,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TerrainInput {
    pub world: String,
    /// South-west cell of the quad lattice (`lodsettings\<World>.lod`).
    pub south_west: (i32, i32),
    /// Highest LOD level to build (`lodsettings`: 32, or 16 for small worldspaces).
    pub max_level: i32,
    /// Default land height (WRLD DNAM) used for cells outside the input and cells without LAND.
    pub default_land: f32,
    /// Default water height (WRLD DNAM); `None` = the worldspace has no LOD water.
    pub default_water: Option<f32>,
    /// Sorted by (y, x), unique.
    pub cells: Vec<TerrainCell>,
}

impl TerrainInput {
    /// Sorts the cells by (y, x) and drops duplicates (the later one wins).
    pub fn normalize(&mut self) {
        self.cells.sort_by_key(|c| (c.y, c.x));
        let mut seen: HashMap<(i32, i32), usize> = HashMap::new();
        let mut keep = Vec::with_capacity(self.cells.len());
        for c in self.cells.drain(..) {
            match seen.get(&(c.x, c.y)) {
                Some(&i) => keep[i] = c,
                None => {
                    seen.insert((c.x, c.y), keep.len());
                    keep.push(c);
                }
            }
        }
        self.cells = keep;
    }

    /// Bounding rectangle of the cells, inclusive: ((min x, min y), (max x, max y)).
    pub fn cell_rect(&self) -> Option<((i32, i32), (i32, i32))> {
        let mut it = self.cells.iter();
        let f = it.next()?;
        Some(
            it.fold(((f.x, f.y), (f.x, f.y)), |((x0, y0), (x1, y1)), c| {
                ((x0.min(c.x), y0.min(c.y)), (x1.max(c.x), y1.max(c.y)))
            }),
        )
    }

    /// Keeps only the cells inside the inclusive rectangle.
    pub fn clip(&mut self, min: (i32, i32), max: (i32, i32)) {
        self.cells
            .retain(|c| c.x >= min.0 && c.x <= max.0 && c.y >= min.1 && c.y <= max.1);
    }

    /// The quads of `level` that intersect the cell rectangle: (x, y) of their south-west cells.
    pub fn quads(&self, level: i32) -> Vec<(i32, i32)> {
        let Some(((x0, y0), (x1, y1))) = self.cell_rect() else {
            return Vec::new();
        };
        let first = |v: i32, sw: i32| sw + (v - sw).div_euclid(level) * level;
        let mut out = Vec::new();
        let mut qy = first(y0, self.south_west.1);
        while qy <= y1 {
            let mut qx = first(x0, self.south_west.0);
            while qx <= x1 {
                out.push((qx, qy));
                qx += level;
            }
            qy += level;
        }
        out
    }

    pub fn sampler(&self) -> HeightField<'_> {
        HeightField::new(self)
    }
}

/// Height / water sampler over the world grid (grid point gx = cell x * 32 + column).
pub struct HeightField<'a> {
    pub input: &'a TerrainInput,
    x0: i32,
    y0: i32,
    w: i32,
    h: i32,
    index: Vec<u32>,
}

const ABSENT: u32 = u32::MAX;

impl<'a> HeightField<'a> {
    pub fn new(input: &'a TerrainInput) -> Self {
        let ((x0, y0), (x1, y1)) = input.cell_rect().unwrap_or(((0, 0), (-1, -1)));
        let (w, h) = ((x1 - x0 + 1).max(0), (y1 - y0 + 1).max(0));
        let mut index = vec![ABSENT; (w * h) as usize];
        for (i, c) in input.cells.iter().enumerate() {
            index[((c.y - y0) * w + (c.x - x0)) as usize] = i as u32;
        }
        HeightField {
            input,
            x0,
            y0,
            w,
            h,
            index,
        }
    }

    pub fn cell(&self, cx: i32, cy: i32) -> Option<&'a TerrainCell> {
        if cx < self.x0 || cy < self.y0 || cx >= self.x0 + self.w || cy >= self.y0 + self.h {
            return None;
        }
        let i = self.index[((cy - self.y0) * self.w + (cx - self.x0)) as usize];
        (i != ABSENT).then(|| &self.input.cells[i as usize])
    }

    /// Owner of world grid point (gx, gy): the cell and the (column, row) inside it. On a shared
    /// line the cell to the north / east owns the point; a missing owner falls back to the other
    /// side; when no candidate exists the nominal cell is returned with no data.
    pub fn owner(&self, gx: i32, gy: i32) -> (i32, i32, usize, usize, Option<&'a TerrainCell>) {
        let cx = gx.div_euclid(CELL_STEPS);
        let cy = gy.div_euclid(CELL_STEPS);
        let col = (gx - cx * CELL_STEPS) as usize;
        let row = (gy - cy * CELL_STEPS) as usize;
        let xs: &[(i32, usize)] = if col == 0 {
            &[(0, 0), (-1, 32)]
        } else {
            &[(0, col)]
        };
        let ys: &[(i32, usize)] = if row == 0 {
            &[(0, 0), (-1, 32)]
        } else {
            &[(0, row)]
        };
        for &(dy, r) in ys {
            for &(dx, c) in xs {
                if let Some(cell) = self.cell(cx + dx, cy + dy) {
                    return (cx + dx, cy + dy, c, r, Some(cell));
                }
            }
        }
        (cx, cy, col, row, None)
    }

    /// Height in game units (default land where no cell exists).
    pub fn height(&self, gx: i32, gy: i32) -> f32 {
        match self.owner(gx, gy) {
            (_, _, c, r, Some(cell)) => cell.height(c, r),
            _ => self.input.default_land,
        }
    }

    /// Water height in game units at a grid point (the owner cell's water; default where no
    /// cell exists).
    pub fn water(&self, gx: i32, gy: i32) -> Option<f32> {
        match self.owner(gx, gy) {
            (_, _, _, _, Some(cell)) => cell.water,
            _ => self.input.default_water,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(cells: Vec<TerrainCell>) -> TerrainInput {
        let mut t = TerrainInput {
            world: "W".into(),
            south_west: (-96, -96),
            max_level: 32,
            default_land: -27000.0,
            default_water: Some(-14000.0),
            cells,
        };
        t.normalize();
        t
    }

    #[test]
    fn shared_lines_belong_to_the_later_cell() {
        let mut a = TerrainCell::flat(0, 0, 10.0, None);
        a.heights[32] = 11.0; // row 0, col 32 (east edge)
        let mut b = TerrainCell::flat(1, 0, 20.0, None);
        b.heights[0] = 21.0; // row 0, col 0 (west edge)
        let t = input(vec![b, a]);
        let f = t.sampler();
        assert_eq!(f.height(0, 0), 10.0);
        assert_eq!(f.height(32, 0), 21.0); // shared column: the east cell owns it
        assert_eq!(f.height(64, 0), 20.0); // east edge of the rectangle: the real cell
        assert_eq!(f.height(10, 32), 10.0); // north edge of the rectangle: the real cell
        assert_eq!(f.height(5, 40), -27000.0); // no cell
        assert_eq!(f.water(5, 40), Some(-14000.0));
    }

    #[test]
    fn quads_cover_the_rectangle_on_the_lattice() {
        let cells = [(-64, -64), (63, 96)]
            .iter()
            .map(|&(x, y)| TerrainCell::flat(x, y, 0.0, None))
            .collect();
        let t = input(cells);
        let q = t.quads(32);
        assert_eq!(q.len(), 4 * 6);
        assert_eq!(q[0], (-64, -64));
        assert_eq!(*q.last().unwrap(), (32, 96));
        let q4 = t.quads(4);
        assert_eq!(q4.len(), 32 * 41);
    }
}
