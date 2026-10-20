//! Terrain LOD meshes as a height source: the cells the load order has no landscape for (cut
//! content, the sea floor beyond the last LAND record) take their heights from the worldspace's
//! LOD4 terrain meshes (`meshes\terrain\<World>\<World>.4.<x>.<y>.btr`, one per 4 x 4 cells), as
//! the public description of the feature says (docs/re/occlusion-spec.md, section 1). The
//! meshes are read with LODForge's NIF reader; every shape's triangles, the LOD water planes
//! included (they are the sea surface the reference sees over, spec section 10.6), are
//! rasterised at the 128-unit samples, the highest triangle winning.

use lodforge_nif::{parse, Block};

use super::grid::{HeightMap, SAMPLES, SAMPLE_UNITS};
use super::Grid;

/// Game units of a cell side.
const CELL_UNITS: f32 = 4096.0;

/// The cell rectangle (inclusive) the worldspace's terrain LOD meshes cover, found by probing
/// the coarsest level's files (32, else 16, else 8 cells per file) over the grid.
pub fn mesh_extent(grid: &Grid, world: &str, read: &dyn Fn(&str) -> Option<Vec<u8>>) -> Option<((i32, i32), (i32, i32))> {
    for level in [32, 16, 8] {
        if level > grid.stride {
            continue;
        }
        let mut found: Option<((i32, i32), (i32, i32))> = None;
        let mut qy = grid.origin.1;
        while qy < grid.origin.1 + grid.stride {
            let mut qx = grid.origin.0;
            while qx < grid.origin.0 + grid.stride {
                if read(&format!("meshes\\terrain\\{world}\\{world}.{level}.{qx}.{qy}.btr")).is_some() {
                    let (lo, hi) = found.unwrap_or(((qx, qy), (qx, qy)));
                    found = Some(((lo.0.min(qx), lo.1.min(qy)), (hi.0.max(qx + level - 1), hi.1.max(qy + level - 1))));
                }
                qx += level;
            }
            qy += level;
        }
        if found.is_some() {
            return found;
        }
    }
    None
}

/// Fills the samples of cells without landscape from the LOD4 meshes of `world` that cover the
/// map. Returns the number of meshes read.
pub fn add_terrain_lod(map: &mut HeightMap, grid: &Grid, world: &str, read: &dyn Fn(&str) -> Option<Vec<u8>>) -> usize {
    let mut files = 0;
    let (x1, y1) = (map.x0 + map.cells_x as i32 - 1, map.y0 + map.cells_y as i32 - 1);
    let first = |v: i32, o: i32| o + (v - o).div_euclid(4) * 4;
    let mut qy = first(map.y0, grid.origin.1);
    while qy <= y1 {
        let mut qx = first(map.x0, grid.origin.0);
        while qx <= x1 {
            let needs = (0..4).any(|j| (0..4).any(|i| map.contains_cell(qx + i, qy + j) && !map.has_land(qx + i, qy + j)));
            if needs
                && let Some(bytes) = read(&format!("meshes\\terrain\\{world}\\{world}.4.{qx}.{qy}.btr"))
                && let Ok(nif) = parse(&bytes)
            {
                files += 1;
                // Shapes hold quad-local positions scaled by the LOD level and shifted in z
                // (checked on `tamriel.4.-96.-96.btr`: scale 4, the world box spans cells
                // -96..-93).
                let origin = (qx as f32 * CELL_UNITS, qy as f32 * CELL_UNITS);
                for b in &nif.blocks {
                    let Block::TriShape(s) = b else { continue };
                    let t = &s.av.transform;
                    let to_world = |p: [f32; 3]| [origin.0 + t.translation[0] + t.scale * p[0], origin.1 + t.translation[1] + t.scale * p[1], t.translation[2] + t.scale * p[2]];
                    for tri in &s.triangles {
                        let [a, b, c] = [to_world(s.positions[tri[0] as usize]), to_world(s.positions[tri[1] as usize]), to_world(s.positions[tri[2] as usize])];
                        rasterize(map, a, b, c);
                    }
                }
            }
            qx += 4;
        }
        qy += 4;
    }
    files
}

/// Raises the samples under a triangle (world units) to the triangle's height there.
fn rasterize(map: &mut HeightMap, a: [f32; 3], b: [f32; 3], c: [f32; 3]) {
    let sx = |p: [f32; 3]| (p[0] / SAMPLE_UNITS - map.x0 as f32 * SAMPLES as f32, p[1] / SAMPLE_UNITS - map.y0 as f32 * SAMPLES as f32);
    let (ax, ay) = sx(a);
    let (bx, by) = sx(b);
    let (cx, cy) = sx(c);
    let det = (bx - ax) * (cy - ay) - (cx - ax) * (by - ay);
    if det.abs() < 1e-9 {
        return;
    }
    let (w, h) = (map.width() as i32, map.height() as i32);
    let x_lo = ax.min(bx).min(cx).ceil().max(0.0) as i32;
    let x_hi = (ax.max(bx).max(cx).floor() as i32).min(w - 1);
    let y_lo = ay.min(by).min(cy).ceil().max(0.0) as i32;
    let y_hi = (ay.max(by).max(cy).floor() as i32).min(h - 1);
    let inv = 1.0 / det;
    for y in y_lo..=y_hi {
        for x in x_lo..=x_hi {
            let (px, py) = (x as f32, y as f32);
            let u = ((bx - ax) * (py - ay) - (px - ax) * (by - ay)) * inv; // weight of c
            let v = ((px - ax) * (cy - ay) - (cx - ax) * (py - ay)) * inv; // weight of b
            let wgt = 1.0 - u - v;
            const E: f32 = -1e-4;
            if u >= E && v >= E && wgt >= E {
                map.set_mesh_sample(x, y, wgt * a[2] + v * b[2] + u * c[2]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_triangle_raises_the_samples_under_it_in_cells_without_land() {
        let mut map = HeightMap::new(0, 0, 2, 1, -27000.0);
        map.set_land(1, 0, &vec![10.0; 33 * 33], f32::NEG_INFINITY);
        // A triangle over the south-west quarter of cell (0, 0) at z 500, reaching into cell (1, 0).
        rasterize(&mut map, [0.0, 0.0, 500.0], [6000.0, 0.0, 500.0], [0.0, 2048.0, 500.0]);
        assert_eq!(map.ground_at(0, 0), 500.0);
        assert_eq!(map.ground_at(10, 5), 500.0);
        assert_eq!(map.ground_at(31, 0), 500.0);
        assert_eq!(map.ground_at(33, 0), 10.0, "cells with landscape keep it");
        assert_eq!(map.ground_at(0, 17), -27000.0, "outside the triangle");
        assert_eq!(map.top_at(10, 5), 500.0);
    }
}
