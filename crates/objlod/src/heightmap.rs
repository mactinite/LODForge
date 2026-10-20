//! The object height map `<World>.objects.heightmap.dat` (docs/re/object-lod-spec.md 7.1, 12.4):
//! per exterior cell the highest LOD4 object surface over the cell's 33 x 33 LAND vertex grid
//! (`x = cellX * 4096 + i * 128`, `y = cellY * 4096 + j * 128`), written when the export says
//! `CreateHeightMap=True` (DynDOLOD, default season only) and read by the occlusion stage
//! (`crates/dyndolod/src/occlusion`, `object_heightmap` / `HeightMap::raise_top`).
//!
//! Layout (little endian): `i32 count`, then per cell `i32 x, i32 y, f32 height[33 * 33]` with
//! point `k` at `((k / 33) * 128, (k % 33) * 128)` from the cell's south-west corner, i.e.
//! `height[i * 33 + j]` for sample `(i, j)` (x outer: the order the occlusion module consumes
//! and the one the golden file has, see spec 12.4); `f32::MIN` where no object covers the
//! point. Cells without any covered point are left out. The golden
//! `Tamriel.objects.heightmap.dat` of golden2 has 14,125 cells (7,009 of them entirely empty).
//!
//! Two steps (spec 12.4). Point sampling: a grid point is covered by a triangle when it lies
//! inside the triangle's XY projection (edges inclusive); its height is the triangle plane's z
//! there, and the point keeps the highest. Every default-season LOD4 shape after hidden-face
//! removal contributes except grass (`grasspassthru`, which never occludes); tree billboards are
//! vertical and cover no point. Then the stored sample is the maximum of the point and its west
//! and south neighbours, `H(i, j) = max(P(i, j), P(i - 1, j), P(i, j - 1))`, looked up across
//! cell borders (the golden's footprint: 89.7% of its points within 1 unit, residuals symmetric;
//! the point itself alone gives 24%, the four corners of the square 66%). The cells a quad
//! touches are not limited to its own sixteen (a triangle reaching into the neighbouring quad
//! raises that cell too); quads are merged by maximum before the dilation.

use std::collections::HashMap;

use crate::build::BuiltShape;

pub const POINTS: usize = 33;
pub const NONE: f32 = f32::MIN;
const STEP: f32 = 128.0;

/// Cell -> 33 x 33 heights (x outer).
pub type Cells = HashMap<(i32, i32), Vec<f32>>;

/// Raises `cells` by the shapes of one quad (positions in the quad's block space: world =
/// position * level + origin).
pub fn rasterize(shapes: &[BuiltShape], level: i32, origin: [f32; 3], cells: &mut Cells) {
    let scale = level as f32;
    for s in shapes {
        if s.material.name.to_ascii_lowercase().starts_with("grass") {
            continue;
        }
        let world: Vec<[f32; 3]> = s.positions.iter().map(|p| [p[0] * scale + origin[0], p[1] * scale + origin[1], p[2] * scale + origin[2]]).collect();
        for t in &s.triangles {
            raise_triangle(world[t[0] as usize], world[t[1] as usize], world[t[2] as usize], cells);
        }
    }
}

fn raise_triangle(a: [f32; 3], b: [f32; 3], c: [f32; 3], cells: &mut Cells) {
    let det = (b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1]);
    if det.abs() < 1e-6 {
        return;
    }
    let (min_x, max_x) = (a[0].min(b[0]).min(c[0]), a[0].max(b[0]).max(c[0]));
    let (min_y, max_y) = (a[1].min(b[1]).min(c[1]), a[1].max(b[1]).max(c[1]));
    let (k0, k1) = ((min_x / STEP).ceil() as i64, (max_x / STEP).floor() as i64);
    let (m0, m1) = ((min_y / STEP).ceil() as i64, (max_y / STEP).floor() as i64);
    let inv = 1.0 / det;
    for m in m0..=m1 {
        let y = m as f32 * STEP;
        for k in k0..=k1 {
            let x = k as f32 * STEP;
            // Barycentric weights of (x, y); edges inclusive.
            let w1 = ((b[0] - a[0]) * (y - a[1]) - (x - a[0]) * (b[1] - a[1])) * inv;
            let w2 = ((x - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (y - a[1])) * inv;
            let w0 = 1.0 - w1 - w2;
            if w0 >= -1e-4 && w1 >= -1e-4 && w2 >= -1e-4 {
                raise_point(k, m, w0 * a[2] + w1 * c[2] + w2 * b[2], cells);
            }
        }
    }
}

/// Raises the sample at global grid point (k, m) in every cell that holds it (a point on a
/// cell's far edge belongs to the neighbour too).
fn raise_point(k: i64, m: i64, z: f32, cells: &mut Cells) {
    let (cx, i) = (k.div_euclid(32), k.rem_euclid(32) as usize);
    let (cy, j) = (m.div_euclid(32), m.rem_euclid(32) as usize);
    let mut targets = vec![(cx, cy, i, j)];
    if i == 0 {
        targets.push((cx - 1, cy, 32, j));
    }
    if j == 0 {
        targets.push((cx, cy - 1, i, 32));
    }
    if i == 0 && j == 0 {
        targets.push((cx - 1, cy - 1, 32, 32));
    }
    for (cx, cy, i, j) in targets {
        let v = cells.entry((cx as i32, cy as i32)).or_insert_with(|| vec![NONE; POINTS * POINTS]);
        let at = i * POINTS + j;
        if v[at] < z {
            v[at] = z;
        }
    }
}

/// Merges `from` into `into` by maximum.
pub fn merge(into: &mut Cells, from: Cells) {
    for (cell, pts) in from {
        match into.get_mut(&cell) {
            Some(v) => {
                for (a, b) in v.iter_mut().zip(pts) {
                    if *a < b {
                        *a = b;
                    }
                }
            }
            None => {
                into.insert(cell, pts);
            }
        }
    }
}

/// The stored samples from the merged point samples: `H(i, j) = max(P(i, j), P(i - 1, j),
/// P(i, j - 1))`, the west / south neighbour read from the adjacent cell at the border (sample
/// 32 of the western cell is sample 0 of this one, so `i - 1 < 0` is sample 31 of the cell to
/// the west). Cells that end up without a sample are dropped.
pub fn finish(points: &Cells) -> Cells {
    let at = |x: i32, y: i32, i: i64, j: i64| -> f32 {
        let (x, i) = if i < 0 { (x - 1, i + 32) } else { (x, i) };
        let (y, j) = if j < 0 { (y - 1, j + 32) } else { (y, j) };
        points.get(&(x, y)).map_or(NONE, |v| v[i as usize * POINTS + j as usize])
    };
    let mut candidates: Vec<(i32, i32)> = points.keys().flat_map(|&(x, y)| [(x, y), (x + 1, y), (x, y + 1), (x + 1, y + 1)]).collect();
    candidates.sort_unstable();
    candidates.dedup();
    let mut out = Cells::new();
    for (x, y) in candidates {
        let mut v = vec![NONE; POINTS * POINTS];
        let mut any = false;
        for i in 0..POINTS as i64 {
            for j in 0..POINTS as i64 {
                let h = at(x, y, i, j).max(at(x, y, i - 1, j)).max(at(x, y, i, j - 1));
                if h != NONE {
                    v[i as usize * POINTS + j as usize] = h;
                    any = true;
                }
            }
        }
        if any {
            out.insert((x, y), v);
        }
    }
    out
}

/// The file bytes, cells sorted by x then y.
pub fn encode(cells: &Cells) -> Vec<u8> {
    let mut keys: Vec<&(i32, i32)> = cells.keys().collect();
    keys.sort();
    let mut out = Vec::with_capacity(4 + keys.len() * (8 + POINTS * POINTS * 4));
    out.extend((keys.len() as i32).to_le_bytes());
    for k in keys {
        out.extend(k.0.to_le_bytes());
        out.extend(k.1.to_le_bytes());
        for v in &cells[k] {
            out.extend(v.to_le_bytes());
        }
    }
    out
}

/// The cells of a file (ours or the golden's).
pub fn decode(bytes: &[u8]) -> Cells {
    let mut out = Cells::new();
    let Some(n) = bytes.get(..4).map(|b| i32::from_le_bytes(b.try_into().unwrap()) as usize) else { return out };
    let rec = 8 + POINTS * POINTS * 4;
    for i in 0..n {
        let Some(r) = bytes.get(4 + i * rec..4 + (i + 1) * rec) else { break };
        let x = i32::from_le_bytes(r[0..4].try_into().unwrap());
        let y = i32::from_le_bytes(r[4..8].try_into().unwrap());
        out.insert((x, y), r[8..].chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect());
    }
    out
}

/// Comparison with a golden file: cells on each side and in common, and over the common cells
/// the points identical, within 1 unit, both `NONE`, present only in ours / only in the golden.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Comparison {
    pub cells_ours: usize,
    pub cells_golden: usize,
    pub cells_common: usize,
    pub points: usize,
    pub identical: usize,
    pub within_1: usize,
    pub both_none: usize,
    pub only_ours: usize,
    pub only_golden: usize,
}

pub fn compare(ours: &Cells, golden: &Cells) -> Comparison {
    let mut c = Comparison { cells_ours: ours.len(), cells_golden: golden.len(), ..Default::default() };
    for (cell, g) in golden {
        let Some(o) = ours.get(cell) else { continue };
        c.cells_common += 1;
        for (a, b) in o.iter().zip(g) {
            c.points += 1;
            match (*a == NONE, *b == NONE) {
                (true, true) => {
                    c.both_none += 1;
                    c.identical += 1;
                    c.within_1 += 1;
                }
                (false, true) => c.only_ours += 1,
                (true, false) => c.only_golden += 1,
                (false, false) => {
                    if a == b {
                        c.identical += 1;
                    }
                    if (a - b).abs() <= 1.0 {
                        c.within_1 += 1;
                    }
                }
            }
        }
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triangle_raises_the_points_it_covers() {
        let mut cells = Cells::new();
        // A flat triangle at z 100 over (0..512, 0..512) of cell (0, 0): covers the grid points
        // with x + y <= 512 (edges inclusive).
        raise_triangle([0.0, 0.0, 100.0], [512.0, 0.0, 100.0], [0.0, 512.0, 100.0], &mut cells);
        let v = &cells[&(0, 0)];
        assert_eq!(v[0], 100.0);
        assert_eq!(v[4 * POINTS], 100.0); // (4, 0) = (512, 0)
        assert_eq!(v[4], 100.0); // (0, 4) = (0, 512)
        assert_eq!(v[2 * POINTS + 2], 100.0); // (256, 256) on the hypotenuse
        assert_eq!(v[3 * POINTS + 2], NONE); // (384, 256) outside
        // The origin point also belongs to the three cells west / south of it.
        assert_eq!(cells[&(-1, 0)][32 * POINTS], 100.0);
        assert_eq!(cells[&(0, -1)][32], 100.0);
        assert_eq!(cells[&(-1, -1)][32 * POINTS + 32], 100.0);
        // A sloped triangle: the plane's z at the point, and only the maximum survives.
        raise_triangle([0.0, 0.0, 0.0], [512.0, 0.0, 200.0], [0.0, 512.0, 0.0], &mut cells);
        assert_eq!(cells[&(0, 0)][4 * POINTS], 200.0);
        assert_eq!(cells[&(0, 0)][2 * POINTS], 100.0);
        let bytes = encode(&cells);
        let back = decode(&bytes);
        assert_eq!(back.len(), 4);
        assert_eq!(back[&(0, 0)], cells[&(0, 0)]);
        let c = compare(&back, &cells);
        assert_eq!((c.cells_common, c.identical, c.points), (4, 4 * POINTS * POINTS, 4 * POINTS * POINTS));
    }

    #[test]
    fn finish_takes_the_point_and_its_west_and_south_neighbours() {
        let mut points = Cells::new();
        // Point samples: (2, 2) = 50 and, in cell (0, 0), the border point (0, 5) = 70 which
        // is also sample (32, 5) of cell (-1, 0).
        raise_point(2, 2, 50.0, &mut points);
        raise_point(0, 5, 70.0, &mut points);
        let h = finish(&points);
        let c = &h[&(0, 0)];
        assert_eq!(c[2 * POINTS + 2], 50.0);
        assert_eq!(c[3 * POINTS + 2], 50.0); // east neighbour sees it as its west point
        assert_eq!(c[2 * POINTS + 3], 50.0); // north neighbour sees it as its south point
        assert_eq!(c[3 * POINTS + 3], NONE); // the diagonal does not
        assert_eq!(c[POINTS + 2], NONE); // nor the west neighbour
        assert_eq!(c[5], 70.0);
        assert_eq!(c[POINTS + 5], 70.0);
        assert_eq!(c[6], 70.0);
        // The western cell holds the border point as sample 32 and raises nothing east of it.
        let w = &h[&(-1, 0)];
        assert_eq!(w[32 * POINTS + 5], 70.0);
        assert_eq!(w[32 * POINTS + 6], 70.0);
        assert_eq!(w[31 * POINTS + 5], NONE);
        // Cells without any sample are dropped.
        assert!(!h.contains_key(&(1, 0)));
    }
}
