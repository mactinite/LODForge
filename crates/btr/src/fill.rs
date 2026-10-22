// SPDX-License-Identifier: GPL-3.0-only
//! Cells without LAND filled from the terrain LOD meshes the game ships (docs/re/terrain-lod-spec.md
//! 2.3, `[ours]`). Bethesda stripped the LAND records of Tamriel's outer cells from Skyrim.esm but
//! left the terrain LOD meshes built from the full data in its archives, so a LOD built from the
//! plugins alone ends in flat default land where those meshes still show coast and mountains (the
//! gap the "xLODGen Resource - SSE Terrain Tamriel" plugins paper over at generation time). Here
//! every cell without LAND inside the top-level lattice quads the export touches takes its 33 x 33
//! heights from the finest shipped mesh covering it: the mesh's land triangles are sampled at the
//! cell's grid points. A level-L mesh stores a vertex at local (i * 128 / L, j * 128 / L) with the
//! world height divided by L and the shape translated by `z_shift` (mesh.rs), so world = quad
//! origin + local * L and height = z * L + z_shift. Skirts are vertical and so cover no grid point.

use std::collections::{BTreeMap, HashMap};

use rayon::prelude::*;

use lodforge_nif::parse;

use crate::btr::{Btr, file_name};
use crate::input::{CELL_STEPS, GRID, STEP_UNITS, TerrainCell, TerrainInput};

/// LOD levels tried per block, finest first.
const LEVELS: [i32; 4] = [4, 8, 16, 32];
/// Cells per block: a block is the LOD4 quad a cell lies in.
const BLOCK: i32 = 4;
/// Barycentric tolerance, in grid units, so that points on a shared edge count as inside.
const EPS: f64 = 1e-6;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FillReport {
    /// Cells whose heights now come from a shipped mesh.
    pub filled: usize,
    /// Of those, cells that were not in the input (outside the exported rectangle; default water).
    pub added: usize,
    /// Cells without LAND for which no mesh could be read: left flat.
    pub left_flat: usize,
    /// Mesh files used, per LOD level.
    pub meshes: BTreeMap<i32, usize>,
    /// Mesh files that exist but are not laid out like a BTR.
    pub unreadable: usize,
    /// Grid points of filled cells that no land triangle covered (default land height).
    pub uncovered_points: usize,
}

impl FillReport {
    pub fn line(&self) -> String {
        let mut s = format!(
            "{} cells without LAND filled from the shipped terrain LOD meshes",
            self.filled
        );
        if !self.meshes.is_empty() {
            let m: Vec<String> = self
                .meshes
                .iter()
                .map(|(l, n)| format!("{n} LOD{l}"))
                .collect();
            s.push_str(&format!(" ({})", m.join(", ")));
        }
        if self.added > 0 {
            s.push_str(&format!(
                "; {} of them outside the LAND rectangle",
                self.added
            ));
        }
        if self.left_flat > 0 {
            s.push_str(&format!("; {} left flat (no mesh)", self.left_flat));
        }
        if self.unreadable > 0 {
            s.push_str(&format!("; {} mesh files unreadable", self.unreadable));
        }
        if self.uncovered_points > 0 {
            s.push_str(&format!(
                "; {} grid points uncovered",
                self.uncovered_points
            ));
        }
        s
    }
}

/// South-west cell of the level-`level` lattice quad holding `cell`.
fn quad_of(cell: (i32, i32), sw: (i32, i32), level: i32) -> (i32, i32) {
    (
        sw.0 + (cell.0 - sw.0).div_euclid(level) * level,
        sw.1 + (cell.1 - sw.1).div_euclid(level) * level,
    )
}

/// Fills every cell without LAND inside the `input.max_level` quads that intersect the exported
/// cells. `has_land(cell)` says whether the load order has a LAND record for the cell; `read(path)`
/// gives a Data-relative file's bytes (`meshes\terrain\<World>\<World>.<L>.<x>.<y>.btr`), the
/// shipped copy preferred over generated loose files by the caller. Cells already in the input
/// keep their water and hide flags; new cells get the default water. The input is normalized.
pub fn from_lod_meshes(
    input: &mut TerrainInput,
    has_land: &(dyn Fn((i32, i32)) -> bool + Sync),
    read: &(dyn Fn(&str) -> Option<Vec<u8>> + Sync),
) -> FillReport {
    let mut report = FillReport::default();
    let top = input.max_level.max(BLOCK);
    let sw = input.south_west;
    let mut blocks: HashMap<(i32, i32), Vec<(i32, i32)>> = HashMap::new();
    for (qx, qy) in input.quads(top) {
        for cy in qy..qy + top {
            for cx in qx..qx + top {
                if !has_land((cx, cy)) {
                    blocks
                        .entry(quad_of((cx, cy), sw, BLOCK))
                        .or_default()
                        .push((cx, cy));
                }
            }
        }
    }
    if blocks.is_empty() {
        return report;
    }
    let levels: Vec<i32> = LEVELS.iter().copied().filter(|&l| l <= top).collect();
    let world = input.world.clone();
    let default_land = input.default_land;
    let mut blocks: Vec<_> = blocks.into_iter().collect();
    blocks.sort_by_key(|(b, _)| (b.1, b.0));
    let results: Vec<BlockResult> = blocks
        .par_iter()
        .map(|(block, cells)| fill_block(&world, sw, *block, cells, &levels, default_land, read))
        .collect();

    let mut index: HashMap<(i32, i32), usize> = input
        .cells
        .iter()
        .enumerate()
        .map(|(i, c)| ((c.x, c.y), i))
        .collect();
    for r in results {
        report.unreadable += r.unreadable;
        report.uncovered_points += r.uncovered;
        report.left_flat += r.left_flat;
        if let Some(l) = r.level {
            *report.meshes.entry(l).or_default() += 1;
        }
        for ((x, y), heights) in r.cells {
            match index.get(&(x, y)) {
                Some(&i) => input.cells[i].heights = heights,
                None => {
                    index.insert((x, y), input.cells.len());
                    input.cells.push(TerrainCell {
                        x,
                        y,
                        water: input.default_water,
                        heights,
                        hide_quadrants: 0,
                    });
                    report.added += 1;
                }
            }
            report.filled += 1;
        }
    }
    input.normalize();
    report
}

struct BlockResult {
    cells: Vec<((i32, i32), Vec<f32>)>,
    left_flat: usize,
    level: Option<i32>,
    unreadable: usize,
    uncovered: usize,
}

/// Heights of the block's cells from the finest readable mesh covering the block.
fn fill_block(
    world: &str,
    sw: (i32, i32),
    block: (i32, i32),
    cells: &[(i32, i32)],
    levels: &[i32],
    default_land: f32,
    read: &(dyn Fn(&str) -> Option<Vec<u8>> + Sync),
) -> BlockResult {
    let n = BLOCK as usize * CELL_STEPS as usize + 1;
    let (x0, y0) = (block.0 * CELL_STEPS, block.1 * CELL_STEPS);
    let mut unreadable = 0;
    for &level in levels {
        let q = quad_of(block, sw, level);
        let path = format!(
            "meshes\\terrain\\{world}\\{}",
            file_name(world, level, q.0, q.1, None)
        );
        let Some(bytes) = read(&path) else { continue };
        let Some(btr) = parse(&bytes).ok().and_then(|nif| Btr::from_nif(&nif)) else {
            unreadable += 1;
            continue;
        };
        let window = rasterize(&btr, q, (x0, y0), n);
        let mut uncovered = 0;
        let out = cells
            .iter()
            .map(|&(cx, cy)| {
                let mut heights = vec![default_land; GRID * GRID];
                for row in 0..GRID {
                    for col in 0..GRID {
                        let gx = (cx * CELL_STEPS - x0) as usize + col;
                        let gy = (cy * CELL_STEPS - y0) as usize + row;
                        match window[gy * n + gx] {
                            Some(z) => heights[row * GRID + col] = z,
                            None => uncovered += 1,
                        }
                    }
                }
                ((cx, cy), heights)
            })
            .collect();
        return BlockResult {
            cells: out,
            left_flat: 0,
            level: Some(level),
            unreadable,
            uncovered,
        };
    }
    BlockResult {
        cells: Vec::new(),
        left_flat: cells.len(),
        level: None,
        unreadable,
        uncovered: 0,
    }
}

/// Heights of the mesh's land at the world grid points of the `n` x `n` window whose south-west
/// point is `origin` (grid units: cell * 32 + column). `quad` is the mesh's south-west cell.
/// `None` where no land triangle covers the point.
pub(crate) fn rasterize(
    btr: &Btr,
    quad: (i32, i32),
    origin: (i32, i32),
    n: usize,
) -> Vec<Option<f32>> {
    let scale = btr.level as f64;
    let per_grid = scale / STEP_UNITS as f64;
    let to_grid = |p: [f32; 3]| -> (f64, f64, f64) {
        (
            (quad.0 * CELL_STEPS) as f64 + p[0] as f64 * per_grid,
            (quad.1 * CELL_STEPS) as f64 + p[1] as f64 * per_grid,
            p[2] as f64 * scale + btr.z_shift as f64,
        )
    };
    let (xmin, ymin) = (origin.0 as f64, origin.1 as f64);
    let (xmax, ymax) = (
        (origin.0 + n as i32 - 1) as f64,
        (origin.1 + n as i32 - 1) as f64,
    );
    let mut out = vec![None; n * n];
    let pos = &btr.land.positions;
    for t in &btr.land.triangles {
        let (Some(&pa), Some(&pb), Some(&pc)) = (
            pos.get(t[0] as usize),
            pos.get(t[1] as usize),
            pos.get(t[2] as usize),
        ) else {
            continue;
        };
        let (a, b, c) = (to_grid(pa), to_grid(pb), to_grid(pc));
        let d = (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0);
        if d.abs() < 1e-9 {
            continue; // skirt or degenerate: no area on the ground
        }
        let bx0 = a.0.min(b.0).min(c.0).max(xmin).ceil() as i32;
        let bx1 = a.0.max(b.0).max(c.0).min(xmax).floor() as i32;
        let by0 = a.1.min(b.1).min(c.1).max(ymin).ceil() as i32;
        let by1 = a.1.max(b.1).max(c.1).min(ymax).floor() as i32;
        for gy in by0..=by1 {
            for gx in bx0..=bx1 {
                let (px, py) = (gx as f64, gy as f64);
                let l0 = ((b.0 - px) * (c.1 - py) - (b.1 - py) * (c.0 - px)) / d;
                let l1 = ((c.0 - px) * (a.1 - py) - (c.1 - py) * (a.0 - px)) / d;
                let l2 = 1.0 - l0 - l1;
                if l0 >= -EPS && l1 >= -EPS && l2 >= -EPS {
                    let i = (gy - origin.1) as usize * n + (gx - origin.0) as usize;
                    if out[i].is_none() {
                        out[i] = Some((l0 * a.2 + l1 * b.2 + l2 * c.2) as f32);
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::bounds::Aabb;
    use crate::btr::ShapeData;
    use crate::mesh::{QuadOptions, build_quad, full_coverage};
    use crate::settings::Settings;
    use lodforge_nif::writer::encode;

    /// A plane over grid point (gx, gy): any triangulation reproduces it exactly.
    fn plane(gx: i32, gy: i32) -> f32 {
        -3000.0 + 2.5 * gx as f32 - 1.25 * gy as f32
    }

    fn cell(x: i32, y: i32, f: &dyn Fn(i32, i32) -> f32) -> TerrainCell {
        let mut c = TerrainCell::flat(x, y, 0.0, Some(-14000.0));
        for row in 0..GRID {
            for col in 0..GRID {
                c.heights[row * GRID + col] =
                    f(x * CELL_STEPS + col as i32, y * CELL_STEPS + row as i32);
            }
        }
        c
    }

    fn input(cells: Vec<TerrainCell>, sw: (i32, i32), max_level: i32) -> TerrainInput {
        let mut t = TerrainInput {
            world: "W".into(),
            south_west: sw,
            max_level,
            default_land: -27000.0,
            default_water: Some(-14000.0),
            cells,
        };
        t.normalize();
        t
    }

    /// The LOD4 BTR of the 4 x 4 cells at `quad`, built by the mesher from `f`.
    fn btr_bytes(quad: (i32, i32), f: &dyn Fn(i32, i32) -> f32) -> Vec<u8> {
        let cells = (0..BLOCK)
            .flat_map(|dy| (0..BLOCK).map(move |dx| cell(quad.0 + dx, quad.1 + dy, f)))
            .collect();
        let truth = input(cells, quad, 4);
        let settings = Settings::default();
        let m = build_quad(
            &truth.sampler(),
            &QuadOptions::for_level(&settings, 4),
            quad.0,
            quad.1,
            &full_coverage,
        )
        .unwrap();
        let b = Btr::from_mesh(&m, settings.level(4).z_shift);
        let nif = b.to_nif("W", None);
        encode(&nif)
            .unwrap()
            .to_bytes(&nif.header, &nif.roots)
            .unwrap()
    }

    #[test]
    fn plane_cells_are_filled_from_the_lod4_mesh() {
        let bytes = btr_bytes((0, 0), &plane);
        // Only cell (1, 2) has LAND; the other 15 are flat in the export.
        let mut cells = vec![cell(1, 2, &plane)];
        for y in 0..4 {
            for x in 0..4 {
                if (x, y) != (1, 2) {
                    cells.push(TerrainCell::flat(x, y, -27000.0, Some(-500.0)));
                }
            }
        }
        let mut t = input(cells, (0, 0), 4);
        let asked = Mutex::new(Vec::new());
        let read = |p: &str| {
            asked.lock().unwrap().push(p.to_string());
            (p == "meshes\\terrain\\W\\W.4.0.0.btr").then(|| bytes.clone())
        };
        let r = from_lod_meshes(&mut t, &|c| c == (1, 2), &read);
        assert_eq!(
            (r.filled, r.added, r.left_flat, r.unreadable, r.uncovered_points),
            (15, 0, 0, 0, 0)
        );
        assert_eq!(r.meshes.get(&4), Some(&1));
        assert_eq!(
            asked.lock().unwrap().as_slice(),
            ["meshes\\terrain\\W\\W.4.0.0.btr"]
        );
        assert_eq!(t.cells.len(), 16);
        for c in &t.cells {
            let want_water = if (c.x, c.y) == (1, 2) { Some(-14000.0) } else { Some(-500.0) };
            assert_eq!(c.water, want_water, "water kept");
            for row in 0..GRID {
                for col in 0..GRID {
                    let want = plane(c.x * CELL_STEPS + col as i32, c.y * CELL_STEPS + row as i32);
                    let got = c.height(col, row);
                    assert!(
                        (got - want).abs() < 0.05,
                        "cell {},{} col {col} row {row}: {got} vs {want}",
                        c.x,
                        c.y
                    );
                }
            }
        }
    }

    #[test]
    fn cells_outside_the_rectangle_are_added_with_default_water() {
        let bytes = btr_bytes((-64, -64), &plane);
        let mut t = input(vec![cell(-62, -61, &plane)], (-96, -96), 32);
        let read = |p: &str| (p == "meshes\\terrain\\W\\W.4.-64.-64.btr").then(|| bytes.clone());
        let r = from_lod_meshes(&mut t, &|c| c == (-62, -61), &read);
        // The LOD32 quad -64..-33 holds 1,024 cells; one has LAND, 16 lie in the one block with a mesh.
        assert_eq!((r.filled, r.added, r.left_flat), (15, 15, 1024 - 16));
        assert_eq!(r.meshes.get(&4), Some(&1));
        assert_eq!(t.cells.len(), 16);
        assert!(
            t.cells
                .iter()
                .all(|c| c.water == Some(-14000.0) && c.hide_quadrants == 0)
        );
        assert_eq!(t.cell_rect(), Some(((-64, -64), (-61, -61))));
        let c = t.cells.iter().find(|c| (c.x, c.y) == (-64, -64)).unwrap();
        assert!((c.height(0, 0) - plane(-64 * 32, -64 * 32)).abs() < 0.05);
    }

    #[test]
    fn coarser_levels_are_tried_on_the_lattice_when_lod4_is_missing() {
        let asked = Mutex::new(Vec::new());
        let read = |p: &str| {
            asked.lock().unwrap().push(p.to_string());
            None
        };
        let mut t = input(vec![TerrainCell::flat(-63, -62, -27000.0, None)], (-96, -96), 32);
        let r = from_lod_meshes(&mut t, &|_| false, &read);
        assert_eq!((r.filled, r.left_flat), (0, 1024));
        let asked = asked.lock().unwrap();
        for name in [
            "W.4.-64.-64.btr",
            "W.8.-64.-64.btr",
            "W.16.-64.-64.btr",
            "W.32.-64.-64.btr",
        ] {
            assert!(
                asked.contains(&format!("meshes\\terrain\\W\\{name}")),
                "{name} asked"
            );
        }
        assert_eq!(asked.len(), 64 * 4, "four levels per block, 64 blocks in the LOD32 quad");
    }

    #[test]
    fn unreadable_files_are_counted_and_skipped() {
        let read = |p: &str| p.ends_with(".4.-64.-64.btr").then(|| b"not a nif".to_vec());
        let mut t = input(vec![TerrainCell::flat(-64, -64, -27000.0, None)], (-96, -96), 32);
        let r = from_lod_meshes(&mut t, &|_| false, &read);
        assert_eq!(r.unreadable, 1);
        assert_eq!(r.filled, 0);
    }

    #[test]
    fn rasterize_interpolates_land_and_ignores_skirts() {
        // A LOD8 mesh: two triangles over local (0,0)-(512,512) = world grid 0..32 at level 8
        // (512 * 8 / 128 = 32 grid steps), heights stored / 8 with z_shift 100, plus a vertical skirt.
        let land = ShapeData {
            positions: vec![
                [0.0, 0.0, 0.0],
                [512.0, 0.0, 100.0],
                [512.0, 512.0, 100.0],
                [0.0, 512.0, 0.0],
                [0.0, 0.0, -50.0],
                [512.0, 0.0, -50.0],
            ],
            uvs: Vec::new(),
            triangles: vec![[0, 1, 2], [0, 2, 3], [0, 1, 5], [0, 5, 4]],
            bound_center: [0.0; 3],
            bound_radius: 0.0,
            segments: None,
        };
        let btr = Btr {
            level: 8,
            x: 0,
            y: 0,
            z_shift: 100.0,
            land,
            water: Vec::new(),
            land_aabb: Aabb::default(),
            water_aabb: None,
        };
        let w = rasterize(&btr, (0, 0), (0, 0), 33);
        // z rises linearly with x: world z = (x_local / 512 * 100) * 8 + 100.
        assert_eq!(w[0], Some(100.0));
        assert_eq!(w[32], Some(900.0));
        assert_eq!(w[16 * 33 + 16], Some(500.0));
        assert_eq!(w[32 * 33], Some(100.0));
        assert!(
            w.iter().all(|z| z.is_some()),
            "the two triangles cover the whole window"
        );
        // Beyond the mesh: nothing.
        let far = rasterize(&btr, (0, 0), (40, 40), 5);
        assert!(far.iter().all(|z| z.is_none()));
    }
}
