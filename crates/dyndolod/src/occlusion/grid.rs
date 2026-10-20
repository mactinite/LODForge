//! Heights of a worldspace sampled every 128 game units: the 32 x 32 landscape vertices of a
//! cell (the 33rd row and column belong to the neighbour), the resolution the TVDT visibility
//! model and the MHDT max-height field work at. Two layers: `ground` is the landscape with its
//! water surface (a cell's water hides the terrain under it), `top` is the highest thing at the
//! sample: ground or object LOD. Samples the map has no data for read as the worldspace's
//! default height (open sea at the default water level).
//!
//! The golden run's per-cell log lines (`min`, `max`, `top` of every cell) match vertex sampling
//! and not the maximum of each 128-unit quad: its minimum is the lowest vertex and its maximum
//! leaves out the shared north and east edges.
//!
//! Copyright (c) 2026 CageTV (AnvilLOD, `src/AnvilLOD.Core/Occlusion/OcclusionGrid.cs`,
//! commit 606eece, GPL-3.0-only). Modified for LODForge: ported to Rust; samples are the
//! landscape vertices instead of quad maxima; the ground layer is clamped to the cell's water
//! height; object tops come from the 33 x 33 points of the object LOD height map instead of
//! footprints; per-cell statistics and the MHDT field added here.
//! This file is part of LODForge and is distributed under the GNU General Public License,
//! version 3 only.

/// Samples per cell side.
pub const SAMPLES: usize = 32;
/// Game units per sample.
pub const SAMPLE_UNITS: f32 = 128.0;
/// Vertices per cell side of a LAND VHGT / object height map cell.
pub const POINTS: usize = 33;
/// Height of a sample the map has no data for.
pub const NO_DATA: f32 = f32::NEG_INFINITY;

#[derive(Debug, Clone)]
pub struct HeightMap {
    /// South-west cell of the map.
    pub x0: i32,
    pub y0: i32,
    pub cells_x: usize,
    pub cells_y: usize,
    /// Height of everything inside the map that has no data.
    pub default: f32,
    /// Height of everything outside the map's rectangle (the golden output behaves as if the
    /// reference read 0 there: observers below 0 never see the blocks beyond its data, observers
    /// above 0 do, see docs/re/occlusion-spec.md section 10.5).
    pub outside: f32,
    ground: Vec<f32>,
    top: Vec<f32>,
    has_land: Vec<bool>,
}

impl HeightMap {
    pub fn new(x0: i32, y0: i32, cells_x: usize, cells_y: usize, default: f32) -> HeightMap {
        let n = cells_x * SAMPLES * cells_y * SAMPLES;
        HeightMap { x0, y0, cells_x, cells_y, default, outside: 0.0, ground: vec![NO_DATA; n], top: vec![NO_DATA; n], has_land: vec![false; cells_x * cells_y] }
    }

    /// Samples per row.
    pub fn width(&self) -> usize {
        self.cells_x * SAMPLES
    }

    pub fn height(&self) -> usize {
        self.cells_y * SAMPLES
    }

    pub fn contains_cell(&self, cx: i32, cy: i32) -> bool {
        cx >= self.x0 && cy >= self.y0 && ((cx - self.x0) as usize) < self.cells_x && ((cy - self.y0) as usize) < self.cells_y
    }

    /// The cell has landscape data.
    pub fn has_land(&self, cx: i32, cy: i32) -> bool {
        self.contains_cell(cx, cy) && self.has_land[(cy - self.y0) as usize * self.cells_x + (cx - self.x0) as usize]
    }

    /// Sample index of the cell's south-west sample.
    fn cell_base(&self, cx: i32, cy: i32) -> Option<(usize, usize)> {
        self.contains_cell(cx, cy).then(|| (((cx - self.x0) as usize) * SAMPLES, ((cy - self.y0) as usize) * SAMPLES))
    }

    /// The landscape of a cell from its 33 x 33 vertex heights (row-major from the south-west
    /// corner, row = y), clamped to `water` (`f32::NEG_INFINITY` for a cell without water): sample
    /// (i, j) is vertex (i, j).
    pub fn set_land(&mut self, cx: i32, cy: i32, heights: &[f32], water: f32) {
        assert_eq!(heights.len(), POINTS * POINTS);
        let Some((sx0, sy0)) = self.cell_base(cx, cy) else { return };
        let w = self.width();
        for j in 0..SAMPLES {
            for i in 0..SAMPLES {
                let h = heights[j * POINTS + i].max(water);
                let at = (sy0 + j) * w + sx0 + i;
                self.ground[at] = h;
                if self.top[at] < h {
                    self.top[at] = h;
                }
            }
        }
        self.has_land[(cy - self.y0) as usize * self.cells_x + (cx - self.x0) as usize] = true;
    }

    /// Raises the top layer of a cell to the object LOD tops given as 33 x 33 points, point `k`
    /// at `((k / 33) * 128, (k % 33) * 128)` game units from the cell's south-west corner (the
    /// object height map's order): sample (i, j) takes point (i, j).
    pub fn raise_top(&mut self, cx: i32, cy: i32, points: &[f32]) {
        assert_eq!(points.len(), POINTS * POINTS);
        let Some((sx0, sy0)) = self.cell_base(cx, cy) else { return };
        let w = self.width();
        for j in 0..SAMPLES {
            for i in 0..SAMPLES {
                let h = points[i * POINTS + j];
                let at = (sy0 + j) * w + sx0 + i;
                if self.top[at] < h {
                    self.top[at] = h;
                }
            }
        }
    }

    /// A terrain LOD mesh height at a sample of a cell without landscape: ground and top rise
    /// to it (cells with landscape are left alone).
    pub fn set_mesh_sample(&mut self, sx: i32, sy: i32, h: f32) {
        if sx < 0 || sy < 0 || sx as usize >= self.width() || sy as usize >= self.height() {
            return;
        }
        let cell = (sy as usize / SAMPLES) * self.cells_x + sx as usize / SAMPLES;
        if self.has_land[cell] {
            return;
        }
        let at = sy as usize * self.width() + sx as usize;
        if self.ground[at] < h {
            self.ground[at] = h;
        }
        if self.top[at] < h {
            self.top[at] = h;
        }
    }

    /// Whether a sample (map coordinates) lies inside the map's rectangle.
    #[inline]
    pub fn contains_sample(&self, sx: i32, sy: i32) -> bool {
        sx >= 0 && sy >= 0 && (sx as usize) < self.width() && (sy as usize) < self.height()
    }

    /// Ground height at a sample (map coordinates): the default where there is no data, the
    /// outside height beyond the map.
    #[inline]
    pub fn ground_at(&self, sx: i32, sy: i32) -> f32 {
        if sx < 0 || sy < 0 || sx as usize >= self.width() || sy as usize >= self.height() {
            return self.outside;
        }
        let h = self.ground[sy as usize * self.width() + sx as usize];
        if h > NO_DATA { h } else { self.default }
    }

    #[inline]
    pub fn top_at(&self, sx: i32, sy: i32) -> f32 {
        if sx < 0 || sy < 0 || sx as usize >= self.width() || sy as usize >= self.height() {
            return self.outside;
        }
        let h = self.top[sy as usize * self.width() + sx as usize];
        if h > NO_DATA { h } else { self.default }
    }

    /// Map sample coordinates of a cell's south-west sample (may be outside the map).
    pub fn cell_sample(&self, cx: i32, cy: i32) -> (i32, i32) {
        ((cx - self.x0) * SAMPLES as i32, (cy - self.y0) * SAMPLES as i32)
    }

    /// (lowest ground, highest ground, highest top) of a cell with data.
    pub fn cell_stats(&self, cx: i32, cy: i32) -> Option<(f32, f32, f32)> {
        let (sx0, sy0) = self.cell_base(cx, cy)?;
        let w = self.width();
        let (mut lo, mut hi, mut top) = (f32::INFINITY, NO_DATA, NO_DATA);
        for j in 0..SAMPLES {
            for i in 0..SAMPLES {
                let at = (sy0 + j) * w + sx0 + i;
                let g = if self.ground[at] > NO_DATA { self.ground[at] } else { self.default };
                let t = if self.top[at] > NO_DATA { self.top[at] } else { g };
                lo = lo.min(g);
                hi = hi.max(g);
                top = top.max(t);
            }
        }
        Some((lo, hi, top))
    }

    /// The cell's MHDT (max height data): `float Offset` then 32 x 32 `uint8` steps of 128 game
    /// units above it, byte `x * 32 + y` for the quad (x, y): the highest of the quad's four
    /// corner tops (the vertices (x, y) .. (x + 1, y + 1), the far corners being the neighbour
    /// cell's first samples), rounded up. The offset is the lowest `top` sample of the cell
    /// rounded up to a multiple of 128 (golden Occlusion.esp: every offset a multiple of 128,
    /// lowest -13952 = ceil(-14000 / 128) * 128; the offsets of walled-city cells lie above the
    /// terrain minimum because the object LOD covers it). Fit on the 279 golden cells whose
    /// MHDT the reference computed: offset identical in 242, bytes identical in 55% (one step
    /// below the golden's in 32%, above in 13%); rounding to nearest or the ground minimum
    /// instead did worse (165 / 214 offsets).
    pub fn mhdt(&self, cx: i32, cy: i32) -> Option<Vec<u8>> {
        let (sx0, sy0) = self.cell_base(cx, cy)?;
        let (sx0, sy0) = (sx0 as i32, sy0 as i32);
        let n = SAMPLES as i32;
        // At the map's edge the far corners fall back to the cell's own last samples.
        let top = |x: i32, y: i32| if self.contains_sample(x, y) { self.top_at(x, y) } else { self.top_at(x.min(sx0 + n - 1), y.min(sy0 + n - 1)) };
        let quad_top = |i: i32, j: i32| top(sx0 + i, sy0 + j).max(top(sx0 + i + 1, sy0 + j)).max(top(sx0 + i, sy0 + j + 1)).max(top(sx0 + i + 1, sy0 + j + 1));
        let mut lo = f32::INFINITY;
        for j in 0..n {
            for i in 0..n {
                lo = lo.min(self.top_at(sx0 + i, sy0 + j));
            }
        }
        let offset = (lo / SAMPLE_UNITS).ceil() * SAMPLE_UNITS;
        let mut out = Vec::with_capacity(4 + SAMPLES * SAMPLES);
        out.extend_from_slice(&offset.to_le_bytes());
        for i in 0..n {
            for j in 0..n {
                let steps = ((quad_top(i, j) - offset) / SAMPLE_UNITS).ceil().clamp(0.0, 255.0);
                out.push(steps as u8);
            }
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plane(h: f32) -> Vec<f32> {
        vec![h; POINTS * POINTS]
    }

    #[test]
    fn land_samples_are_the_vertices_clamped_to_water() {
        let mut m = HeightMap::new(-1, -1, 2, 2, -14000.0);
        let mut hs = plane(100.0);
        hs[5 * POINTS + 7] = 900.0; // vertex (x 7, y 5)
        hs[32 * POINTS + 32] = 5000.0; // the north-east corner belongs to the neighbour
        m.set_land(0, 0, &hs, 200.0);
        assert!(m.has_land(0, 0) && !m.has_land(-1, 0));
        let (sx, sy) = m.cell_sample(0, 0);
        assert_eq!(m.ground_at(sx, sy), 200.0);
        assert_eq!(m.ground_at(sx + 7, sy + 5), 900.0);
        assert_eq!(m.ground_at(sx + 6, sy + 4), 200.0);
        assert_eq!(m.ground_at(sx - 1, sy), -14000.0); // the empty cell west of it
        assert_eq!(m.ground_at(-5, 0), 0.0); // outside the map
        assert_eq!(m.cell_stats(0, 0), Some((200.0, 900.0, 900.0)));
    }

    #[test]
    fn object_tops_raise_only_the_top_layer() {
        let mut m = HeightMap::new(0, 0, 1, 1, -14000.0);
        m.set_land(0, 0, &plane(0.0), f32::NEG_INFINITY);
        let mut pts = plane(-20000.0);
        pts[3 * POINTS + 10] = 5000.0; // point (x 3, y 10) in the height map's x-major order
        m.raise_top(0, 0, &pts);
        assert_eq!(m.ground_at(3, 10), 0.0);
        assert_eq!(m.top_at(3, 10), 5000.0);
        assert_eq!(m.top_at(2, 9), 0.0);
        assert_eq!(m.top_at(4, 10), 0.0);
        assert_eq!(m.cell_stats(0, 0), Some((0.0, 0.0, 5000.0)));
        let mh = m.mhdt(0, 0).unwrap();
        assert_eq!(mh.len(), 1028);
        assert_eq!(f32::from_le_bytes(mh[..4].try_into().unwrap()), 0.0);
        // The point (3, 10) is a corner of the quads (2..3, 9..10).
        assert_eq!(mh[4 + 3 * 32 + 10], 40); // ceil(5000 / 128)
        assert_eq!(mh[4 + 2 * 32 + 9], 40);
        assert_eq!(mh[4 + 4 * 32 + 10], 0);
        assert_eq!(mh[4 + 3 * 32 + 11], 0);
    }

    #[test]
    fn mhdt_offset_rounds_the_lowest_ground_up_to_128() {
        let mut m = HeightMap::new(0, 0, 1, 1, -14000.0);
        m.set_land(0, 0, &plane(-16000.0), -14000.0);
        let mh = m.mhdt(0, 0).unwrap();
        assert_eq!(f32::from_le_bytes(mh[..4].try_into().unwrap()), -13952.0);
        assert!(mh[4..].iter().all(|&b| b == 0));
    }
}
