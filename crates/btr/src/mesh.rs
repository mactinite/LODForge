//! The quad mesher: one LOD quad (level x level cells) of a worldspace as a land mesh with
//! skirts plus its water cells, built from the height sampler with the greedy insertion of
//! [`crate::greedy`] (docs/re/terrain-lod-spec.md section 3).
//!
//! Coordinates: quad-local units (one unit = `level` game units), x east, y north, grid point
//! (i, j) at (i * 128 / level, j * 128 / level); heights are world heights / level.
//!
//! Rules implemented here, with the spec section they come from:
//! - every output vertex is a grid point with the source height (3.2), modified only by the
//!   Optimize Unseen offsets (3.7);
//! - LOD4 with protected cell borders: each cell is its own greedy problem, the cell borders are
//!   simplified once as a 1-D profile and pre-inserted on both sides, so the cells stitch without
//!   T-junctions (3.2, 7.1);
//! - LOD8+: one problem per quad with the 17 x 17 lattice (every 256 local units) pre-inserted
//!   (3.2, 3.6); the quad borders are simplified as 1-D profiles from the world-wide height
//!   sampler, so neighbouring quads select the same border points (3.4);
//! - Optimize Unseen (3.7): with the option on, grid points deeper than the delta below their
//!   water are never candidates; shallow ones are pushed to `water - delta` and dry vertices
//!   raised by delta / 100 (the observed offsets; delta = value / level local units);
//! - hidden quadrants and missing cells (HideQuads, the underside's `RemoveUnseenFaces`): grid
//!   points inside such a region are never candidates, every grid point on its boundary is
//!   pre-inserted (so no triangle straddles the boundary) and triangles whose centroid lies in the
//!   region are dropped;
//! - skirts (3.8): a vertical quad under every border edge, depth per level;
//! - water planes (3.9): one cell quad per cell whose water lies above its lowest height.

use crate::greedy::Greedy;
use crate::input::{CELL_STEPS, CELL_UNITS, HeightField, STEP_UNITS};
use crate::profile;
use crate::settings::{Settings, skirt_depth};

/// Options of one quad build.
#[derive(Debug, Clone, PartialEq)]
pub struct QuadOptions {
    pub level: i32,
    /// Error threshold in local units.
    pub quality: f32,
    /// Point budget per quad (skirt vertices come on top; 2,000 are reserved for them).
    pub max_verts: u32,
    /// Optimize Unseen value in game units: < 0 off, 0 on, > 0 on with the push-down.
    pub water_delta: f32,
    /// Shoreline mode bed depth in game units (see `LevelSettings::water_floor`); <= 0 off.
    pub water_floor: f32,
    /// `Skirts` key (see [`skirt_depth`]); <= 0 none.
    pub skirts: i32,
    pub protect_cell_borders: bool,
    pub ignore_water: bool,
    /// Wind the triangles clockwise seen from above (faces down): the underside.
    pub flip: bool,
}

impl QuadOptions {
    /// The BTR options of `level` from the settings.
    pub fn for_level(settings: &Settings, level: i32) -> Self {
        let l = settings.level(level);
        QuadOptions {
            level,
            quality: l.quality,
            max_verts: l.max_verts,
            water_delta: l.water_delta,
            water_floor: l.water_floor,
            skirts: settings.skirts,
            protect_cell_borders: settings.protect_cell_borders,
            ignore_water: settings.ignore_water,
            flip: false,
        }
    }
}

/// Which land quadrants carry terrain: `(cell x, cell y, quadrant bit)` -> covered. Quadrant bits
/// as in CELL XCLC: 1 south-west, 2 south-east, 4 north-west, 8 north-east.
pub type Coverage<'a> = &'a (dyn Fn(i32, i32, u8) -> bool + Sync);

/// Everything covered (plain BTRs without `HideQuads`).
pub fn full_coverage(_: i32, _: i32, _: u8) -> bool {
    true
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct LandMesh {
    /// Quad-local positions; skirt vertices follow the grid vertices.
    pub positions: Vec<[f32; 3]>,
    /// (x / 4096, 1 - y / 4096), copied to skirt vertices.
    pub uvs: Vec<[f32; 2]>,
    pub triangles: Vec<[u16; 3]>,
    /// Number of grid vertices (the rest are skirt vertices).
    pub grid_vertices: usize,
    /// Number of land triangles before the skirt triangles.
    pub grid_triangles: usize,
}

/// A water plane over one cell of the quad.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WaterCell {
    /// Cell offset inside the quad.
    pub dx: u8,
    pub dy: u8,
    /// Water height in local units.
    pub height: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct QuadMesh {
    pub level: i32,
    /// South-west cell of the quad.
    pub x: i32,
    pub y: i32,
    pub land: LandMesh,
    /// Water cells in (dx, dy) order.
    pub water: Vec<WaterCell>,
    /// Largest remaining candidate error of the greedy problems (0 when all met the threshold).
    pub max_error: f64,
    /// Whether any problem stopped at its point budget.
    pub budget_hit: bool,
    /// How well the land mesh keeps the water's outline (grid points with water).
    pub shore: ShoreStats,
}

/// Land mesh vs water plane at the grid points of water cells, comparing the source terrain with
/// the mesh surface over that point.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ShoreStats {
    /// Grid points whose source terrain is under the water.
    pub wet: u32,
    /// ... where the mesh surface is at or above the water (river or shore lost).
    pub wet_exposed: u32,
    /// Grid points whose source terrain is above the water, within [`SHORE_BAND`] grid steps of a
    /// wet point (the shore band).
    pub shore: u32,
    /// ... where the mesh surface is under the water (shore eroded).
    pub shore_drowned: u32,
    /// Grid points of water cells where the mesh surface is within one local unit of the water
    /// plane (L game units: z-fighting at a distance).
    pub near_plane: u32,
}

impl ShoreStats {
    pub fn add(&mut self, o: &ShoreStats) {
        self.wet += o.wet;
        self.wet_exposed += o.wet_exposed;
        self.shore += o.shore;
        self.shore_drowned += o.shore_drowned;
        self.near_plane += o.near_plane;
    }
}

const SKIRT_RESERVE: u32 = 2000;

/// Width of the shore band of [`ShoreStats`] in grid steps (128 game units each).
pub const SHORE_BAND: usize = 4;

/// [`ShoreStats`] of a quad: the mesh surface (triangles over grid points, heights `zmod`) is
/// sampled at every grid point and compared with the source heights `z` and the water `water`
/// (local units, NaN = no water).
fn shore_stats(n: usize, z: &[f32], water: &[f32], zmod: &[f32], points: &[(usize, usize)], triangles: &[[u32; 3]]) -> ShoreStats {
    let mut surface = vec![f32::NAN; n * n];
    for t in triangles {
        let p = t.map(|v| points[v as usize]);
        let h = p.map(|(i, j)| zmod[j * n + i]);
        let (x0, x1) = (p.iter().map(|q| q.0).min().unwrap(), p.iter().map(|q| q.0).max().unwrap());
        let (y0, y1) = (p.iter().map(|q| q.1).min().unwrap(), p.iter().map(|q| q.1).max().unwrap());
        let f = |a: (usize, usize)| (a.0 as f64, a.1 as f64);
        let (a, b, c) = (f(p[0]), f(p[1]), f(p[2]));
        let det = (b.1 - c.1) * (a.0 - c.0) + (c.0 - b.0) * (a.1 - c.1);
        if det == 0.0 {
            continue;
        }
        for j in y0..=y1 {
            for i in x0..=x1 {
                let (x, y) = (i as f64, j as f64);
                let l0 = ((b.1 - c.1) * (x - c.0) + (c.0 - b.0) * (y - c.1)) / det;
                let l1 = ((c.1 - a.1) * (x - c.0) + (a.0 - c.0) * (y - c.1)) / det;
                let l2 = 1.0 - l0 - l1;
                if l0 < -1e-9 || l1 < -1e-9 || l2 < -1e-9 {
                    continue;
                }
                surface[j * n + i] = (l0 * f64::from(h[0]) + l1 * f64::from(h[1]) + l2 * f64::from(h[2])) as f32;
            }
        }
    }
    // Shore band: dry points near a wet one (separable max filter of the wet mask).
    let wet: Vec<bool> = (0..n * n).map(|k| !water[k].is_nan() && z[k] < water[k]).collect();
    let mut rows = vec![false; n * n];
    for j in 0..n {
        for i in 0..n {
            let (lo, hi) = (i.saturating_sub(SHORE_BAND), (i + SHORE_BAND).min(n - 1));
            rows[j * n + i] = (lo..=hi).any(|k| wet[j * n + k]);
        }
    }
    let mut s = ShoreStats::default();
    for j in 0..n {
        for i in 0..n {
            let k = j * n + i;
            let (w, m) = (water[k], surface[k]);
            if w.is_nan() || m.is_nan() {
                continue;
            }
            if (m - w).abs() < 1.0 {
                s.near_plane += 1;
            }
            if wet[k] {
                s.wet += 1;
                s.wet_exposed += u32::from(m >= w);
            } else {
                let (lo, hi) = (j.saturating_sub(SHORE_BAND), (j + SHORE_BAND).min(n - 1));
                if (lo..=hi).any(|r| rows[r * n + i]) {
                    s.shore += 1;
                    s.shore_drowned += u32::from(m < w);
                }
            }
        }
    }
    s
}

/// Builds the quad whose south-west cell is (qx, qy).
pub fn build_quad(
    field: &HeightField<'_>,
    opts: &QuadOptions,
    qx: i32,
    qy: i32,
    covered: Coverage<'_>,
) -> Result<QuadMesh, String> {
    let level = opts.level;
    let lvl = level as f32;
    let cells = level as usize;
    let n = cells * CELL_STEPS as usize + 1;
    let step = STEP_UNITS / lvl;

    // Heights and water per grid point, in local units.
    let mut z = vec![0f32; n * n];
    let mut water = vec![f32::NAN; n * n];
    for j in 0..n {
        let gy = qy * CELL_STEPS + j as i32;
        for i in 0..n {
            let gx = qx * CELL_STEPS + i as i32;
            z[j * n + i] = field.height(gx, gy) / lvl;
            if !opts.ignore_water
                && let Some(w) = field.water(gx, gy)
            {
                water[j * n + i] = w / lvl;
            }
        }
    }

    // Coverage per quadrant of the quad (2 x 2 per cell).
    let qn = cells * 2;
    let mut quadrant_covered = vec![true; qn * qn];
    let mut any_uncovered = false;
    for qj in 0..qn {
        for qi in 0..qn {
            let bit = match (qj % 2 == 1, qi % 2 == 1) {
                (false, false) => 1,
                (false, true) => 2,
                (true, false) => 4,
                (true, true) => 8,
            };
            let c = covered(qx + (qi / 2) as i32, qy + (qj / 2) as i32, bit);
            quadrant_covered[qj * qn + qi] = c;
            any_uncovered |= !c;
        }
    }
    // Per grid point: 0 covered interior, 1 on the boundary of an uncovered region, 2 inside it.
    let point_region = |i: usize, j: usize| -> u8 {
        if !any_uncovered {
            return 0;
        }
        let touching = |v: usize| -> (usize, usize) {
            let q = v / 16;
            let lo = if v % 16 == 0 && q > 0 { q - 1 } else { q };
            let hi = q.min(qn - 1);
            (lo, hi)
        };
        let (i0, i1) = touching(i);
        let (j0, j1) = touching(j);
        let (mut cov, mut unc) = (false, false);
        for qj in j0..=j1 {
            for qi in i0..=i1 {
                if quadrant_covered[qj * qn + qi] {
                    cov = true;
                } else {
                    unc = true;
                }
            }
        }
        match (cov, unc) {
            (_, false) => 0,
            (true, true) => 1,
            (false, true) => 2,
        }
    };

    // Optimize Unseen: modified heights and the deep-water mask.
    let delta = if opts.ignore_water || opts.water_delta < 0.0 {
        None
    } else {
        Some(opts.water_delta / lvl)
    };
    // Shoreline mode: the bed is clamped to `floor` below the water and stays a candidate.
    let floor = delta.filter(|_| opts.water_floor > 0.0).map(|d| (opts.water_floor / lvl).max(d));
    let mut zmod = z.clone();
    let mut deep = vec![false; n * n];
    if let Some(d) = delta {
        for i in 0..n * n {
            let w = water[i];
            if w.is_nan() {
                continue;
            }
            if z[i] >= w {
                zmod[i] = z[i] + d / 100.0;
                // Shoreline mode: dry ground stays at least `Quality` + 1 local units above the
                // water. The greedy keeps the surface within `Quality` of these heights, so every
                // dry point stays at least one unit above the water plane: no drowned shore (the
                // map's LOD32 outline) and no beach sharing the water plane's depth.
                if floor.is_some() {
                    zmod[i] = zmod[i].max(w + opts.quality.max(0.0) + 1.0);
                }
            } else if let Some(f) = floor {
                zmod[i] = z[i].min(w - d).max(w - f);
            } else {
                zmod[i] = z[i].min(w - d);
                deep[i] = z[i] < w - d;
            }
        }
    }

    // Greedy problems: per cell at LOD4 with protected borders, else the whole quad.
    let per_cell = level == 4 && opts.protect_cell_borders;
    let problems: Vec<(usize, usize, usize)> = if per_cell {
        (0..cells)
            .flat_map(|dy| (0..cells).map(move |dx| (dx * 32, dy * 32, 33)))
            .collect()
    } else {
        vec![(0, 0, n)]
    };
    let lattice = if level >= 8 { 2 * cells } else { 32 };
    let budget = (opts.max_verts.saturating_sub(SKIRT_RESERVE) as usize).max(16) / problems.len();

    // Vertex ids per grid point (shared by neighbouring cell problems).
    const NONE: u32 = u32::MAX;
    let mut vid = vec![NONE; n * n];
    let mut grid_points: Vec<(usize, usize)> = Vec::new();
    let mut triangles: Vec<[u32; 3]> = Vec::new();
    let mut max_error = 0f64;
    let mut budget_hit = false;

    for &(x0, y0, size) in &problems {
        let mut sub = vec![0f32; size * size];
        for j in 0..size {
            sub[j * size..(j + 1) * size]
                .copy_from_slice(&zmod[(y0 + j) * n + x0..(y0 + j) * n + x0 + size]);
        }
        let mut g = Greedy::new(size, size, &sub);
        // Never-candidates: deep water and the inside of uncovered regions.
        for j in 0..size {
            for i in 0..size {
                let gi = (y0 + j) * n + x0 + i;
                if deep[gi] || point_region(x0 + i, y0 + j) == 2 {
                    g.ignore(i, j);
                }
            }
        }
        // Pre-insertions: the lattice, and the boundary of uncovered regions.
        for j in 0..size {
            for i in 0..size {
                let (gx, gy) = (x0 + i, y0 + j);
                let region = point_region(gx, gy);
                let on_lattice = gx % lattice == 0 && gy % lattice == 0;
                if region == 1 || (on_lattice && region == 0) {
                    g.select(i, j);
                }
            }
        }
        // Border profiles: both sides of a shared line select the same points.
        for side in 0..4 {
            let along = |k: usize| -> (usize, usize) {
                match side {
                    0 => (k, 0),
                    1 => (k, size - 1),
                    2 => (0, k),
                    _ => (size - 1, k),
                }
            };
            let heights: Vec<f32> = (0..size)
                .map(|k| {
                    let (i, j) = along(k);
                    sub[j * size + i]
                })
                .collect();
            let fixed: Vec<bool> = (0..size)
                .map(|k| {
                    let (i, j) = along(k);
                    g.status(i, j) == crate::greedy::USED
                })
                .collect();
            let candidate: Vec<bool> = (0..size)
                .map(|k| {
                    let (i, j) = along(k);
                    g.status(i, j) != crate::greedy::IGNORED
                })
                .collect();
            let selected = profile::simplify(&heights, &fixed, &candidate, f64::from(opts.quality));
            for k in 0..size {
                let (i, j) = along(k);
                if selected[k] {
                    g.select(i, j);
                } else {
                    g.ignore(i, j);
                }
            }
        }
        g.run(f64::from(opts.quality), budget);
        if g.point_count() >= budget {
            budget_hit = true;
        }
        max_error = max_error.max(g.max_error());
        for (i, j) in g.used_points() {
            let gi = (y0 + j) * n + x0 + i;
            if vid[gi] == NONE {
                vid[gi] = grid_points.len() as u32;
                grid_points.push((x0 + i, y0 + j));
            }
        }
        for t in g.triangles() {
            triangles.push(t.map(|(i, j)| vid[(y0 + j) * n + x0 + i]));
        }
    }

    // Triangles over uncovered regions.
    if any_uncovered {
        triangles.retain(|t| {
            let (sx, sy) = t.iter().fold((0usize, 0usize), |(sx, sy), &v| {
                (
                    sx + grid_points[v as usize].0,
                    sy + grid_points[v as usize].1,
                )
            });
            let qi = ((sx as f64 / 3.0) / 16.0).floor() as usize;
            let qj = ((sy as f64 / 3.0) / 16.0).floor() as usize;
            quadrant_covered[qj.min(qn - 1) * qn + qi.min(qn - 1)]
        });
        // Drop the vertices only the removed triangles used.
        let mut remap = vec![NONE; grid_points.len()];
        let mut kept = Vec::with_capacity(grid_points.len());
        for t in &mut triangles {
            for v in t.iter_mut() {
                if remap[*v as usize] == NONE {
                    remap[*v as usize] = kept.len() as u32;
                    kept.push(grid_points[*v as usize]);
                }
                *v = remap[*v as usize];
            }
        }
        grid_points = kept;
    }

    if grid_points.len() > usize::from(u16::MAX) {
        return Err(format!(
            "quad {level}.{qx}.{qy}: {} grid vertices exceed the u16 limit",
            grid_points.len()
        ));
    }

    // Output geometry.
    let mut land = LandMesh::default();
    for &(i, j) in &grid_points {
        let (x, y) = (i as f32 * step, j as f32 * step);
        land.positions.push([x, y, zmod[j * n + i]]);
        land.uvs.push([x / CELL_UNITS, 1.0 - y / CELL_UNITS]);
    }
    land.grid_vertices = grid_points.len();
    for t in &triangles {
        let t = t.map(|v| v as u16);
        land.triangles
            .push(if opts.flip { [t[0], t[2], t[1]] } else { t });
    }
    land.grid_triangles = land.triangles.len();
    let shore = shore_stats(n, &z, &water, &zmod, &grid_points, &triangles);

    // Skirts: a vertical quad under every border edge, facing away from the quad; two new
    // vertices per edge (the observed convention: 128 skirt vertices for the 64 border edges of
    // a minimal LOD8+ quad, spec 3.6).
    let depth = skirt_depth(opts.skirts, level);
    if depth > 0.0 && !opts.flip {
        let last = n - 1;
        let side_of = |(i, j): (usize, usize)| -> u8 {
            u8::from(i == 0)
                | u8::from(i == last) << 1
                | u8::from(j == 0) << 2
                | u8::from(j == last) << 3
        };
        let lower = |land: &mut LandMesh, v: u16| -> u16 {
            let p = land.positions[v as usize];
            let uv = land.uvs[v as usize];
            land.positions.push([p[0], p[1], p[2] - depth]);
            land.uvs.push(uv);
            (land.positions.len() - 1) as u16
        };
        for ti in 0..land.grid_triangles {
            let t = land.triangles[ti];
            for k in 0..3 {
                let (a, b) = (t[k], t[(k + 1) % 3]);
                if side_of(grid_points[a as usize]) & side_of(grid_points[b as usize]) == 0 {
                    continue;
                }
                if land.positions.len() + 2 > usize::from(u16::MAX) {
                    return Err(format!(
                        "quad {level}.{qx}.{qy}: skirt vertices exceed the u16 limit"
                    ));
                }
                let (a2, b2) = (lower(&mut land, a), lower(&mut land, b));
                land.triangles.push([a, a2, b]);
                land.triangles.push([b, a2, b2]);
            }
        }
    }
    if land.positions.len() > usize::from(u16::MAX) {
        return Err(format!(
            "quad {level}.{qx}.{qy}: {} vertices with skirts exceed the u16 limit",
            land.positions.len()
        ));
    }

    // Water planes.
    let mut water_cells = Vec::new();
    if !opts.ignore_water {
        for dy in 0..cells {
            for dx in 0..cells {
                let (w, min_h) = match field.cell(qx + dx as i32, qy + dy as i32) {
                    Some(c) => (c.water, c.min_height()),
                    None => (field.input.default_water, field.input.default_land),
                };
                if let Some(w) = w
                    && w > min_h
                {
                    water_cells.push(WaterCell {
                        dx: dx as u8,
                        dy: dy as u8,
                        height: w / lvl,
                    });
                }
            }
        }
    }

    Ok(QuadMesh {
        level,
        x: qx,
        y: qy,
        land,
        water: water_cells,
        max_error,
        budget_hit,
        shore,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{GRID, TerrainCell, TerrainInput};

    fn world(cells: Vec<TerrainCell>) -> TerrainInput {
        let mut t = TerrainInput {
            world: "T".into(),
            south_west: (-96, -96),
            max_level: 32,
            default_land: -27000.0,
            default_water: Some(-14000.0),
            cells,
        };
        t.normalize();
        t
    }

    fn bumpy(x: i32, y: i32) -> TerrainCell {
        let mut c = TerrainCell::flat(x, y, 0.0, Some(-100.0));
        for row in 0..GRID {
            for col in 0..GRID {
                let (gx, gy) = (x * 32 + col as i32, y * 32 + row as i32);
                c.heights[row * GRID + col] =
                    ((gx as f32 * 0.3).sin() * 600.0 + (gy as f32 * 0.2).cos() * 400.0).round()
                        * 8.0
                        / 8.0;
            }
        }
        c
    }

    fn check_mesh(m: &QuadMesh, n: usize, step: f32) {
        let land = &m.land;
        for (k, p) in land.positions.iter().enumerate().take(land.grid_vertices) {
            let (gi, gj) = (p[0] / step, p[1] / step);
            assert_eq!(gi.fract(), 0.0);
            assert_eq!(gj.fract(), 0.0);
            assert!(
                gi >= 0.0 && gi as usize <= n - 1 && gj >= 0.0 && gj as usize <= n - 1,
                "vertex {k} off the grid"
            );
            assert_eq!(land.uvs[k], [p[0] / 4096.0, 1.0 - p[1] / 4096.0]);
        }
        for t in &land.triangles[..land.grid_triangles] {
            let p = t.map(|i| land.positions[i as usize]);
            let area = (p[1][0] - p[0][0]) * (p[2][1] - p[0][1])
                - (p[1][1] - p[0][1]) * (p[2][0] - p[0][0]);
            assert!(area > 0.0, "clockwise or degenerate land triangle");
        }
    }

    #[test]
    fn lod4_cells_stitch_without_t_junctions() {
        let cells = (0..4)
            .flat_map(|y| (0..4).map(move |x| bumpy(x, y)))
            .collect();
        let w = world(cells);
        let f = w.sampler();
        let opts = QuadOptions::for_level(&Settings::default(), 4);
        let m = build_quad(&f, &opts, 0, 0, &full_coverage).unwrap();
        check_mesh(&m, 129, 32.0);
        assert!(m.max_error <= 5.0, "max error {}", m.max_error);
        // No triangle straddles an interior cell line.
        for t in &m.land.triangles[..m.land.grid_triangles] {
            let p = t.map(|i| m.land.positions[i as usize]);
            for line in [1024.0, 2048.0, 3072.0] {
                let (minx, maxx) = (
                    p.iter().map(|v| v[0]).fold(f32::MAX, f32::min),
                    p.iter().map(|v| v[0]).fold(f32::MIN, f32::max),
                );
                let (miny, maxy) = (
                    p.iter().map(|v| v[1]).fold(f32::MAX, f32::min),
                    p.iter().map(|v| v[1]).fold(f32::MIN, f32::max),
                );
                assert!(!(minx < line && line < maxx), "straddles x = {line}");
                assert!(!(miny < line && line < maxy), "straddles y = {line}");
            }
        }
        // Every vertex on an interior cell line is used by triangles on both sides.
        for line in [1024.0f32, 2048.0, 3072.0] {
            for (k, p) in m
                .land
                .positions
                .iter()
                .enumerate()
                .take(m.land.grid_vertices)
            {
                if p[0] != line {
                    continue;
                }
                let (mut west, mut east) = (false, false);
                for t in &m.land.triangles[..m.land.grid_triangles] {
                    if t.contains(&(k as u16)) {
                        let cx: f32 = t
                            .iter()
                            .map(|&i| m.land.positions[i as usize][0])
                            .sum::<f32>()
                            / 3.0;
                        if cx < line {
                            west = true;
                        } else {
                            east = true;
                        }
                    }
                }
                assert!(west && east, "T-junction at {:?}", p);
            }
        }
        // Skirts: two vertices and two triangles per border edge.
        assert!(m.land.positions.len() > m.land.grid_vertices);
        assert_eq!((m.land.triangles.len() - m.land.grid_triangles) % 2, 0);
        assert!(m.water.is_empty() || m.water.iter().all(|w| w.height == -25.0));
    }

    #[test]
    fn neighbouring_quads_share_border_points() {
        let cells = (0..8)
            .flat_map(|y| (0..16).map(move |x| bumpy(x, y)))
            .collect();
        let w = world(cells);
        let f = w.sampler();
        let opts = QuadOptions::for_level(&Settings::default(), 8);
        let a = build_quad(&f, &opts, 0, 0, &full_coverage).unwrap();
        let b = build_quad(&f, &opts, 8, 0, &full_coverage).unwrap();
        check_mesh(&a, 257, 16.0);
        let east: std::collections::BTreeSet<(u32, u32)> = a.land.positions[..a.land.grid_vertices]
            .iter()
            .filter(|p| p[0] == 4096.0)
            .map(|p| (p[1].to_bits(), p[2].to_bits()))
            .collect();
        let west: std::collections::BTreeSet<(u32, u32)> = b.land.positions[..b.land.grid_vertices]
            .iter()
            .filter(|p| p[0] == 0.0)
            .map(|p| (p[1].to_bits(), p[2].to_bits()))
            .collect();
        assert_eq!(east, west);
        assert!(east.len() >= 17);
        // The 17 x 17 lattice is present.
        let has = |m: &QuadMesh, x: f32, y: f32| {
            m.land.positions[..m.land.grid_vertices]
                .iter()
                .any(|p| p[0] == x && p[1] == y)
        };
        for k in 0..17 {
            for l in 0..17 {
                assert!(has(&a, k as f32 * 256.0, l as f32 * 256.0));
            }
        }
    }

    #[test]
    fn shoreline_mode_keeps_narrow_rivers_and_lifts_beaches() {
        // LOD16 quad of 16 x 16 cells: land at 400 with water at 0, a river 3 grid points wide
        // (bed -300) running north-south at grid x 200..202, and a beach at 5 (just above the
        // water) over grid x 300..340. The river is narrower than the 32-point lattice.
        let cells = (0..16)
            .flat_map(|y| {
                (0..16).map(move |x| {
                    let mut c = TerrainCell::flat(x, y, 400.0, Some(0.0));
                    for row in 0..GRID {
                        for col in 0..GRID {
                            let gx = x * 32 + col as i32;
                            c.heights[row * GRID + col] = match gx {
                                200..=202 => -300.0,
                                300..=340 => 5.0,
                                _ => 400.0,
                            };
                        }
                    }
                    c
                })
            })
            .collect();
        let w = world(cells);
        let f = w.sampler();

        let xl = build_quad(&f, &QuadOptions::for_level(&Settings::xlodgen(), 16), 0, 0, &full_coverage).unwrap();
        assert!(xl.shore.wet > 0);
        assert!(xl.shore.wet_exposed > 0, "xLODGen's rule bridges the river: {:?}", xl.shore);
        assert!(xl.shore.near_plane > 0, "the beach shares the water plane: {:?}", xl.shore);

        let m = build_quad(&f, &QuadOptions::for_level(&Settings::default(), 16), 0, 0, &full_coverage).unwrap();
        assert_eq!(m.shore.wet, xl.shore.wet);
        assert_eq!(m.shore.wet_exposed, 0, "{:?}", m.shore);
        assert_eq!(m.shore.near_plane, 0, "{:?}", m.shore);
        assert_eq!(m.shore.shore_drowned, 0, "{:?}", m.shore);
        // Wet vertices sit between the push-down and the bed, dry ones at least Quality + 1 up.
        let (d, fl, lift) = (550.0 / 16.0, 1100.0 / 16.0, 8.0 + 1.0);
        for p in &m.land.positions[..m.land.grid_vertices] {
            assert!(p[2] >= lift - 1e-3 || (p[2] <= -d + 1e-3 && p[2] >= -fl - 1e-3), "vertex height {}", p[2]);
        }
        // The same guarantee at the map's LOD32 (quality 10): no drowned shore, no lost river.
        let cells32 = (0..32)
            .flat_map(|y| {
                (0..32).map(move |x| {
                    let mut c = TerrainCell::flat(x, y, 400.0, Some(0.0));
                    for row in 0..GRID {
                        for col in 0..GRID {
                            let gx = x * 32 + col as i32;
                            c.heights[row * GRID + col] = match gx {
                                400..=405 => -300.0,
                                600..=700 => 5.0 + (gx - 600) as f32 * 3.0,
                                _ => 400.0,
                            };
                        }
                    }
                    c
                })
            })
            .collect();
        let w32 = world(cells32);
        let m32 = build_quad(&w32.sampler(), &QuadOptions::for_level(&Settings::default(), 32), 0, 0, &full_coverage).unwrap();
        assert!(m32.shore.wet > 0 && m32.shore.shore > 0);
        assert_eq!((m32.shore.wet_exposed, m32.shore.shore_drowned, m32.shore.near_plane), (0, 0, 0), "{:?}", m32.shore);

        // Control files reproduce xLODGen (no shoreline mode unless asked for).
        assert_eq!(crate::ControlFile::parse("").settings, Settings::xlodgen());
        let c = crate::ControlFile::parse("WaterDeltaLOD16=550\nWaterFloorLOD16=900\n");
        assert_eq!(c.settings.level(16).water_floor, 900.0);
        assert!(c.to_text().contains("WaterFloorLOD16=900"));
    }

    #[test]
    fn flat_quad_is_minimal_and_water_follows_the_rule() {
        let cells = (0..8)
            .flat_map(|y| (0..8).map(move |x| TerrainCell::flat(x, y, -27000.0, Some(-14000.0))))
            .collect();
        let w = world(cells);
        let f = w.sampler();
        let opts = QuadOptions::for_level(&Settings::default(), 8);
        let m = build_quad(&f, &opts, 0, 0, &full_coverage).unwrap();
        assert_eq!(m.land.grid_vertices, 289);
        assert_eq!(m.land.grid_triangles, 512);
        assert_eq!(m.land.positions.len(), 289 + 128);
        assert_eq!(m.land.triangles.len(), 512 + 128);
        assert_eq!(m.water.len(), 64);
        assert_eq!(m.water[0].height, -14000.0 / 8.0);
        // Missing cells (beyond the rectangle) are flat fill with the default water.
        let m2 = build_quad(&f, &opts, 8, 0, &full_coverage).unwrap();
        assert_eq!(m2.water.len(), 64);
    }

    #[test]
    fn hidden_quadrants_leave_a_hole() {
        let mut cells: Vec<TerrainCell> = (0..4)
            .flat_map(|y| (0..4).map(move |x| bumpy(x, y)))
            .collect();
        cells[5].hide_quadrants = 0x3; // cell (1, 1): both southern quadrants
        let w = world(cells);
        let f = w.sampler();
        let opts = QuadOptions {
            flip: true,
            skirts: 0,
            ..QuadOptions::for_level(&Settings::default(), 4)
        };
        let cov = |x: i32, y: i32, bit: u8| !(x == 1 && y == 1 && bit & 0x3 != 0);
        let m = build_quad(&f, &opts, 0, 0, &cov).unwrap();
        for t in &m.land.triangles {
            let p = t.map(|i| m.land.positions[i as usize]);
            let area = (p[1][0] - p[0][0]) * (p[2][1] - p[0][1])
                - (p[1][1] - p[0][1]) * (p[2][0] - p[0][0]);
            assert!(area < 0.0, "flipped mesh must be clockwise");
            let (cx, cy) = (
                p.iter().map(|v| v[0]).sum::<f32>() / 3.0,
                p.iter().map(|v| v[1]).sum::<f32>() / 3.0,
            );
            assert!(
                !(cx > 1024.0 && cx < 2048.0 && cy > 1024.0 && cy < 1536.0),
                "triangle inside the hole"
            );
            // No triangle crosses the hole boundary.
            let inside =
                |v: &[f32; 3]| v[0] > 1024.0 && v[0] < 2048.0 && v[1] > 1024.0 && v[1] < 1536.0;
            assert!(p.iter().all(|v| !inside(v)), "vertex inside the hole");
        }
    }
}
