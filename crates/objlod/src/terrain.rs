//! Terrain and water heights from the LAND records (docs/re/object-lod-spec.md 1.6): LAND
//! `VHGT` heights sampled bilinearly inside their cell, the cell's water or the worldspace
//! default, from the load order (`lodforge_btr::export`) or from a terrain dump
//! (`lodforge_btr::terrain_bin`). One of the two terrain sources of [`crate::hidden`].

use std::collections::HashMap;

use lodforge_btr::input::{CELL_UNITS, GRID, STEP_UNITS, TerrainCell};
use lodforge_btr::TerrainInput;
use lodforge_esp::{LoadOrder, Worldspace};

pub struct TerrainHeights {
    pub input: TerrainInput,
    index: HashMap<(i32, i32), usize>,
}

impl TerrainHeights {
    pub fn new(input: TerrainInput) -> TerrainHeights {
        let index = input.cells.iter().enumerate().map(|(i, c)| ((c.x, c.y), i)).collect();
        TerrainHeights { input, index }
    }

    /// Heights of `world` from the load order (`parent` for a child worldspace that uses its
    /// parent's land or water defaults).
    pub fn from_load_order(lo: &LoadOrder, world: &Worldspace, parent: Option<&Worldspace>, south_west: (i32, i32), max_level: i32) -> lodforge_esp::Result<TerrainHeights> {
        Ok(TerrainHeights::new(lodforge_btr::export::terrain_input(lo, world, parent, south_west, max_level)?))
    }

    /// Heights from a terrain dump (`LODGen_Terrain_<World>.bin` / DynDOLOD's `TerrainData`).
    pub fn from_bin(bytes: &[u8], world: &str, south_west: (i32, i32), max_level: i32, default_land: f32, default_water: Option<f32>) -> Result<TerrainHeights, String> {
        let cells = lodforge_btr::terrain_bin::read_cells(bytes)?;
        Ok(TerrainHeights::new(lodforge_btr::terrain_bin::terrain_input(&cells, world, south_west, max_level, default_land, default_water)))
    }

    pub fn cells(&self) -> usize {
        self.input.cells.len()
    }

    pub fn cell(&self, cx: i32, cy: i32) -> Option<&TerrainCell> {
        self.index.get(&(cx, cy)).map(|&i| &self.input.cells[i])
    }

    /// Terrain height under (x, y), bilinear in the cell's 33 x 33 grid; `None` where the cell
    /// has no data.
    pub fn height_at(&self, x: f32, y: f32) -> Option<f32> {
        let (cx, cy) = crate::grid::cell_of(x, y);
        let c = self.cell(cx, cy)?;
        let gx = (x - cx as f32 * CELL_UNITS) / STEP_UNITS;
        let gy = (y - cy as f32 * CELL_UNITS) / STEP_UNITS;
        let (ix, iy) = (gx.floor().clamp(0.0, (GRID - 2) as f32), gy.floor().clamp(0.0, (GRID - 2) as f32));
        let (fx, fy) = ((gx - ix).clamp(0.0, 1.0), (gy - iy).clamp(0.0, 1.0));
        let (col, row) = (ix as usize, iy as usize);
        let top = c.height(col, row) * (1.0 - fx) + c.height(col + 1, row) * fx;
        let bottom = c.height(col, row + 1) * (1.0 - fx) + c.height(col + 1, row + 1) * fx;
        Some(top * (1.0 - fy) + bottom * fy)
    }

    /// Water height of the cell under (x, y), `None` where the cell has no water or no data.
    pub fn water_at(&self, x: f32, y: f32) -> Option<f32> {
        let (cx, cy) = crate::grid::cell_of(x, y);
        self.cell(cx, cy)?.water
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bilinear() {
        // A single cell (0, 0) whose height is 8 * x grid index, water at 100.
        let mut heights = vec![0f32; GRID * GRID];
        for row in 0..GRID {
            for col in 0..GRID {
                heights[row * GRID + col] = col as f32 * 8.0;
            }
        }
        let mut input = TerrainInput { world: "W".into(), south_west: (-96, -96), max_level: 32, default_land: -2000.0, default_water: Some(-14000.0), cells: vec![TerrainCell { x: 0, y: 0, water: Some(100.0), heights, hide_quadrants: 0 }] };
        input.normalize();
        let t = TerrainHeights::new(input);
        assert_eq!(t.height_at(0.0, 0.0), Some(0.0));
        assert_eq!(t.height_at(128.0, 500.0), Some(8.0));
        assert_eq!(t.height_at(64.0, 0.0), Some(4.0));
        // 4095 / 128 = 31.9921875: between columns 31 and 32.
        assert!((t.height_at(4095.0, 100.0).unwrap() - 255.9375).abs() < 0.01);
        assert_eq!(t.water_at(10.0, 10.0), Some(100.0));
        assert_eq!(t.height_at(-10.0, 0.0), None);
        assert_eq!(t.water_at(5000.0, 0.0), None);
    }
}
