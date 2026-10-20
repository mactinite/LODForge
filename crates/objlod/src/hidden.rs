//! Hidden-face removal (docs/re/object-lod-spec.md section 4): triangles that lie below the
//! terrain LOD mesh of the same level (xLODGen: "uses terrain LOD meshes to optimize the object
//! LOD meshes by removing any triangles that are below the terrain or water planes") or below
//! the LAND heights (DynDOLOD's `TerrainData`), or below the cell's water, are left out.
//!
//! Two terrain sources, both samplers of world (x, y) -> height: [`BtrTerrain`] reads the
//! `.btr` files of the worldspace (our own or the golden run's) and answers from the land
//! triangle containing the point; [`crate::terrain::TerrainHeights`] interpolates the LAND
//! grid. The sample set, the margin and the water test are a [`HiddenRule`], settled against
//! the golden files with `lodforge object-lod --hidden-study` (section 12 of the spec).
//!
//! The per-triangle test (every sample below the surface minus the margin, a sample without
//! data keeps the triangle) follows AnvilLOD `AnvilLOD.Meshes/BtoBuilder.cs` `TerrainTest`
//! (Copyright (c) 2026 CageTV, GPL-3.0-only, commit 606eece); the sample sets and margins are
//! parameters here instead of AnvilLOD's fixed seven points and per-level margins.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::grid::{LodGrid, Quad, cell_of, file_name_btr};
use crate::terrain::TerrainHeights;

/// Which points of a triangle are tested.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Samples {
    /// The three corners.
    Vertices,
    /// Corners and centroid.
    VerticesCentroid,
    /// Corners, edge midpoints and centroid (AnvilLOD).
    Seven,
    /// The centroid alone.
    Centroid,
    /// Any one corner below hides the triangle.
    AnyCorner,
    /// Two of the three corners below hide the triangle.
    Majority,
}

/// Where the terrain height comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerrainKind {
    /// The `.btr` meshes of the same level, the quad under each sample point.
    Btr,
    /// The `.btr` mesh of the quad being built only: samples outside it have no data.
    BtrOwn,
    /// The `.btr` mesh of the quad being built, samples outside it clamped to its border.
    BtrClamp,
    /// LAND `VHGT` heights, bilinear.
    Land,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HiddenRule {
    pub terrain: TerrainKind,
    pub samples: Samples,
    /// Game units the surface is lowered by before the test (`RemoveUnseenZShift`).
    pub shift: f32,
    /// Also drop triangles entirely below the cell's water.
    pub water: bool,
    /// A sample without terrain data counts as below the terrain (else it keeps the triangle).
    pub outside_hidden: bool,
}

impl HiddenRule {
    /// `btr|btrown|land:v3|v3c|s7:<shift>:water|nowater[:outhidden|outkept]`, e.g.
    /// `btrown:v3:0:water:outhidden`.
    pub fn parse(s: &str) -> Option<HiddenRule> {
        let p: Vec<&str> = s.split(':').collect();
        if p.len() != 4 && p.len() != 5 {
            return None;
        }
        let terrain = match p[0] {
            "btr" => TerrainKind::Btr,
            "btrown" => TerrainKind::BtrOwn,
            "btrclamp" => TerrainKind::BtrClamp,
            "land" => TerrainKind::Land,
            _ => return None,
        };
        let samples = match p[1] {
            "v3" => Samples::Vertices,
            "v3c" => Samples::VerticesCentroid,
            "s7" => Samples::Seven,
            "c" => Samples::Centroid,
            "any" => Samples::AnyCorner,
            "maj" => Samples::Majority,
            _ => return None,
        };
        let shift = p[2].parse().ok()?;
        let water = match p[3] {
            "water" => true,
            "nowater" => false,
            _ => return None,
        };
        let outside_hidden = match p.get(4) {
            None | Some(&"outkept") => false,
            Some(&"outhidden") => true,
            _ => return None,
        };
        Some(HiddenRule { terrain, samples, shift, water, outside_hidden })
    }

    pub fn text(&self) -> String {
        format!(
            "{}:{}:{}:{}:{}",
            match self.terrain {
                TerrainKind::Btr => "btr",
                TerrainKind::BtrOwn => "btrown",
                TerrainKind::BtrClamp => "btrclamp",
                TerrainKind::Land => "land",
            },
            match self.samples {
                Samples::Vertices => "v3",
                Samples::VerticesCentroid => "v3c",
                Samples::Seven => "s7",
                Samples::Centroid => "c",
                Samples::AnyCorner => "any",
                Samples::Majority => "maj",
            },
            self.shift,
            if self.water { "water" } else { "nowater" },
            if self.outside_hidden { "outhidden" } else { "outkept" }
        )
    }
}

/// The terrain sources of one worldspace.
pub struct Terrain<'a> {
    pub btr: Option<BtrTerrain<'a>>,
    pub land: Option<TerrainHeights>,
}

impl<'a> Terrain<'a> {
    /// Terrain height under a world point from the given source; `quad` is the quad being
    /// built (its level for `Btr`, the file itself for `BtrOwn`).
    pub fn height_at(&self, kind: TerrainKind, quad: &Quad, x: f32, y: f32) -> Option<f32> {
        match kind {
            // Where no .btr covers the point (a quad outside the terrain output), the LAND
            // heights stand in.
            TerrainKind::Btr => self.btr.as_ref().and_then(|b| b.height_at(quad.level, x, y)).or_else(|| self.land.as_ref()?.height_at(x, y)),
            TerrainKind::BtrOwn => self.btr.as_ref()?.height_in(*quad, x, y),
            TerrainKind::BtrClamp => {
                let o = quad.origin();
                let size = quad.level as f32 * 4096.0;
                self.btr.as_ref()?.height_in(*quad, x.clamp(o[0] + 0.01, o[0] + size - 0.01), y.clamp(o[1] + 0.01, o[1] + size - 0.01))
            }
            TerrainKind::Land => self.land.as_ref()?.height_at(x, y),
        }
    }

    /// Water height of the cell under a world point (LAND data, else the BTR water shapes).
    pub fn water_at(&self, level: i32, x: f32, y: f32) -> Option<f32> {
        if let Some(l) = &self.land {
            return l.water_at(x, y);
        }
        self.btr.as_ref()?.water_at(level, x, y)
    }

    pub fn has(&self, kind: TerrainKind) -> bool {
        match kind {
            TerrainKind::Btr | TerrainKind::BtrOwn | TerrainKind::BtrClamp => self.btr.is_some(),
            TerrainKind::Land => self.land.is_some(),
        }
    }

    /// The sample points of a triangle in the order corners, midpoints, centroid.
    pub fn sample_points(tri: &[[f32; 3]; 3]) -> [[f32; 3]; 7] {
        let mid = |a: [f32; 3], b: [f32; 3]| [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5, (a[2] + b[2]) * 0.5];
        let c = [(tri[0][0] + tri[1][0] + tri[2][0]) / 3.0, (tri[0][1] + tri[1][1] + tri[2][1]) / 3.0, (tri[0][2] + tri[1][2] + tri[2][2]) / 3.0];
        [tri[0], tri[1], tri[2], mid(tri[0], tri[1]), mid(tri[1], tri[2]), mid(tri[2], tri[0]), c]
    }

    /// Height of each sample above the terrain (`None` without data) and above the water.
    pub fn depths(&self, kind: TerrainKind, quad: &Quad, tri: &[[f32; 3]; 3]) -> ([Option<f32>; 7], [Option<f32>; 7]) {
        let pts = Self::sample_points(tri);
        let mut t = [None; 7];
        let mut w = [None; 7];
        for (i, p) in pts.iter().enumerate() {
            t[i] = self.height_at(kind, quad, p[0], p[1]).map(|h| p[2] - h);
            w[i] = self.water_at(quad.level, p[0], p[1]).map(|h| p[2] - h);
        }
        (t, w)
    }

    /// Whether a world-space triangle of `quad` is hidden under `rule`.
    pub fn hidden(&self, rule: &HiddenRule, quad: &Quad, tri: &[[f32; 3]; 3]) -> bool {
        let (t, w) = self.depths(rule.terrain, quad, tri);
        rule_hidden(rule, &t, &w)
    }

    /// The terrain around `quad` (its `.btr` and the eight neighbours at its level), resolved
    /// once so that the per-triangle test of a quad builder takes no locks.
    pub fn view(&self, quad: &Quad) -> TerrainView<'_, 'a> {
        let mut meshes = Vec::new();
        if let Some(b) = &self.btr {
            for dy in -1..=1 {
                for dx in -1..=1 {
                    let q = Quad { level: quad.level, x: quad.x + dx * quad.level, y: quad.y + dy * quad.level };
                    meshes.push((q, b.mesh(q)));
                }
            }
        }
        TerrainView { terrain: self, quad: *quad, meshes }
    }
}

/// See [`Terrain::view`].
pub struct TerrainView<'t, 'a> {
    terrain: &'t Terrain<'a>,
    quad: Quad,
    meshes: Vec<(Quad, Arc<Option<BtrMesh>>)>,
}

impl TerrainView<'_, '_> {
    fn btr_mesh(&self, x: f32, y: f32) -> Option<&BtrMesh> {
        let b = self.terrain.btr.as_ref()?;
        let q = b.grid.quad_for(cell_of(x, y), self.quad.level);
        match self.meshes.iter().find(|(mq, _)| *mq == q) {
            Some((_, m)) => m.as_ref().as_ref(),
            None => None,
        }
    }

    pub fn height_at(&self, kind: TerrainKind, x: f32, y: f32) -> Option<f32> {
        match kind {
            TerrainKind::Btr => match self.btr_mesh(x, y) {
                Some(m) => m.height_at(x, y).or_else(|| self.terrain.land.as_ref()?.height_at(x, y)),
                None => self.terrain.height_at(kind, &self.quad, x, y),
            },
            TerrainKind::BtrOwn | TerrainKind::BtrClamp | TerrainKind::Land => self.terrain.height_at(kind, &self.quad, x, y),
        }
    }

    pub fn water_at(&self, x: f32, y: f32) -> Option<f32> {
        if let Some(l) = &self.terrain.land {
            return l.water_at(x, y);
        }
        match self.btr_mesh(x, y) {
            Some(m) => m.water_at(x, y),
            None => self.terrain.water_at(self.quad.level, x, y),
        }
    }

    pub fn hidden(&self, rule: &HiddenRule, tri: &[[f32; 3]; 3]) -> bool {
        let pts = Terrain::sample_points(tri);
        let mut t = [None; 7];
        let mut w = [None; 7];
        for (i, p) in pts.iter().enumerate() {
            t[i] = self.height_at(rule.terrain, p[0], p[1]).map(|h| p[2] - h);
            w[i] = if rule.water { self.water_at(p[0], p[1]).map(|h| p[2] - h) } else { None };
        }
        rule_hidden(rule, &t, &w)
    }
}

/// The rule on precomputed sample depths (above terrain, above water).
pub fn rule_hidden(rule: &HiddenRule, terrain: &[Option<f32>; 7], water: &[Option<f32>; 7]) -> bool {
    let (idx, needed): (&[usize], usize) = match rule.samples {
        Samples::Vertices => (&[0, 1, 2], 3),
        Samples::VerticesCentroid => (&[0, 1, 2, 6], 4),
        Samples::Seven => (&[0, 1, 2, 3, 4, 5, 6], 7),
        Samples::Centroid => (&[6], 1),
        Samples::AnyCorner => (&[0, 1, 2], 1),
        Samples::Majority => (&[0, 1, 2], 2),
    };
    let below = |d: &[Option<f32>; 7], margin: f32, none_below: bool| idx.iter().filter(|&&i| d[i].map_or(none_below, |v| v < -margin)).count() >= needed;
    below(terrain, rule.shift, rule.outside_hidden) || (rule.water && below(water, 0.0, false))
}

/// The land triangles of one `.btr` file in world units with a bin index.
pub struct BtrMesh {
    origin: [f32; 2],
    bin: f32,
    nx: usize,
    ny: usize,
    tris: Vec<[[f32; 3]; 3]>,
    bins: Vec<Vec<u32>>,
    /// Water heights of the quad's cells: (cell, height).
    water: HashMap<(i32, i32), f32>,
}

impl BtrMesh {
    /// From the parsed description of a file at quad `q`.
    pub fn new(b: &lodforge_btr::Btr, q: &Quad) -> BtrMesh {
        let level = q.level as f32;
        let origin = [q.x as f32 * 4096.0, q.y as f32 * 4096.0];
        let to_world = |p: [f32; 3]| [p[0] * level + origin[0], p[1] * level + origin[1], p[2] * level + b.z_shift];
        let mut tris: Vec<[[f32; 3]; 3]> = Vec::with_capacity(b.land.triangles.len());
        for t in &b.land.triangles {
            let tri = [to_world(b.land.positions[t[0] as usize]), to_world(b.land.positions[t[1] as usize]), to_world(b.land.positions[t[2] as usize])];
            // Skirts are vertical: no footprint, never a surface.
            let area2 = (tri[1][0] - tri[0][0]) * (tri[2][1] - tri[0][1]) - (tri[2][0] - tri[0][0]) * (tri[1][1] - tri[0][1]);
            if area2.abs() > 1e-3 {
                tris.push(tri);
            }
        }
        let bin = 256.0f32;
        let n = (q.level as usize * 4096 / 256).max(1);
        let mut bins = vec![Vec::new(); n * n];
        for (i, tri) in tris.iter().enumerate() {
            let (mut x0, mut y0, mut x1, mut y1) = (f32::INFINITY, f32::INFINITY, f32::NEG_INFINITY, f32::NEG_INFINITY);
            for p in tri {
                x0 = x0.min(p[0]);
                y0 = y0.min(p[1]);
                x1 = x1.max(p[0]);
                y1 = y1.max(p[1]);
            }
            let bx0 = (((x0 - origin[0]) / bin).floor() as i64).clamp(0, n as i64 - 1) as usize;
            let bx1 = (((x1 - origin[0]) / bin).floor() as i64).clamp(0, n as i64 - 1) as usize;
            let by0 = (((y0 - origin[1]) / bin).floor() as i64).clamp(0, n as i64 - 1) as usize;
            let by1 = (((y1 - origin[1]) / bin).floor() as i64).clamp(0, n as i64 - 1) as usize;
            for by in by0..=by1 {
                for bx in bx0..=bx1 {
                    bins[by * n + bx].push(i as u32);
                }
            }
        }
        let mut water = HashMap::new();
        for w in &b.water {
            for t in &w.triangles {
                // The cell of the triangle's centroid (its corners sit on cell lines).
                let c = t.map(|i| to_world(w.positions[i as usize]));
                let cx = ((c[0][0] + c[1][0] + c[2][0]) / 3.0 / 4096.0).floor() as i32;
                let cy = ((c[0][1] + c[1][1] + c[2][1]) / 3.0 / 4096.0).floor() as i32;
                water.entry((cx, cy)).or_insert(c[0][2]);
            }
        }
        BtrMesh { origin, bin, nx: n, ny: n, tris, bins, water }
    }

    /// Terrain height at a world point inside the quad, `None` outside the land triangles.
    pub fn height_at(&self, x: f32, y: f32) -> Option<f32> {
        let bx = ((x - self.origin[0]) / self.bin).floor();
        let by = ((y - self.origin[1]) / self.bin).floor();
        if bx < 0.0 || by < 0.0 || bx >= self.nx as f32 || by >= self.ny as f32 {
            return None;
        }
        let mut best: Option<f32> = None;
        for &i in &self.bins[by as usize * self.nx + bx as usize] {
            let t = &self.tris[i as usize];
            let (ax, ay) = (t[0][0], t[0][1]);
            let (v0x, v0y) = (t[1][0] - ax, t[1][1] - ay);
            let (v1x, v1y) = (t[2][0] - ax, t[2][1] - ay);
            let (v2x, v2y) = (x - ax, y - ay);
            let den = v0x * v1y - v1x * v0y;
            if den.abs() < 1e-6 {
                continue;
            }
            let u = (v2x * v1y - v1x * v2y) / den;
            let v = (v0x * v2y - v2x * v0y) / den;
            let eps = -1e-4;
            if u >= eps && v >= eps && u + v <= 1.0 - eps {
                let z = t[0][2] + u * (t[1][2] - t[0][2]) + v * (t[2][2] - t[0][2]);
                best = Some(best.map_or(z, |b: f32| b.max(z)));
            }
        }
        best
    }

    pub fn water_at(&self, x: f32, y: f32) -> Option<f32> {
        self.water.get(&cell_of(x, y)).copied()
    }

    pub fn triangles(&self) -> usize {
        self.tris.len()
    }
}

/// The `.btr` files of a worldspace, parsed on demand and shared between the quad builders.
pub struct BtrTerrain<'a> {
    world: String,
    grid: LodGrid,
    read: Box<dyn Fn(&str) -> Option<Vec<u8>> + Send + Sync + 'a>,
    cache: Mutex<HashMap<Quad, Arc<Option<BtrMesh>>>>,
    /// Files asked for and not found.
    pub missing: Mutex<Vec<String>>,
}

impl<'a> BtrTerrain<'a> {
    /// `read` resolves a file name (`<World>.<L>.<x>.<y>.btr`) to its bytes.
    pub fn new(world: &str, grid: LodGrid, read: Box<dyn Fn(&str) -> Option<Vec<u8>> + Send + Sync + 'a>) -> BtrTerrain<'a> {
        BtrTerrain { world: world.to_string(), grid, read, cache: Mutex::new(HashMap::new()), missing: Mutex::new(Vec::new()) }
    }

    pub fn mesh(&self, q: Quad) -> Arc<Option<BtrMesh>> {
        if let Some(m) = self.cache.lock().unwrap().get(&q) {
            return m.clone();
        }
        let name = file_name_btr(&self.world, &q);
        let mesh = (self.read)(&name).and_then(|bytes| lodforge_nif::parse(&bytes).ok()).and_then(|nif| lodforge_btr::Btr::from_nif(&nif)).map(|b| BtrMesh::new(&b, &q));
        if mesh.is_none() {
            self.missing.lock().unwrap().push(name);
        }
        let m = Arc::new(mesh);
        self.cache.lock().unwrap().entry(q).or_insert(m).clone()
    }

    pub fn height_at(&self, level: i32, x: f32, y: f32) -> Option<f32> {
        let q = self.grid.quad_for(cell_of(x, y), level);
        self.mesh(q).as_ref().as_ref()?.height_at(x, y)
    }

    /// Height from the mesh of `q` only (`None` outside its land triangles).
    pub fn height_in(&self, q: Quad, x: f32, y: f32) -> Option<f32> {
        self.mesh(q).as_ref().as_ref()?.height_at(x, y)
    }

    pub fn water_at(&self, level: i32, x: f32, y: f32) -> Option<f32> {
        let q = self.grid.quad_for(cell_of(x, y), level);
        self.mesh(q).as_ref().as_ref()?.water_at(x, y)
    }

    pub fn loaded(&self) -> usize {
        self.cache.lock().unwrap().values().filter(|m| m.is_some()).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lodforge_btr::btr::ShapeData;

    fn flat_btr(level: i32, height: f32, water: Option<f32>) -> lodforge_btr::Btr {
        let z = height / level as f32;
        let land = ShapeData { positions: vec![[0.0, 0.0, z], [4096.0, 0.0, z], [0.0, 4096.0, z], [4096.0, 4096.0, z]], uvs: Vec::new(), triangles: vec![[0, 1, 2], [1, 3, 2]], bound_center: [0.0; 3], bound_radius: 0.0, segments: None };
        let water = water
            .map(|w| {
                let wz = w / level as f32;
                vec![ShapeData { positions: vec![[0.0, 0.0, wz], [1024.0, 0.0, wz], [0.0, 1024.0, wz], [1024.0, 1024.0, wz]], uvs: Vec::new(), triangles: vec![[0, 1, 2], [1, 3, 2]], bound_center: [0.0; 3], bound_radius: 0.0, segments: None }]
            })
            .unwrap_or_default();
        lodforge_btr::Btr { level, x: 0, y: 0, z_shift: 0.0, land, water, land_aabb: Default::default(), water_aabb: None }
    }

    #[test]
    fn btr_sampling_and_rules() {
        let q = Quad { level: 4, x: 0, y: -16 };
        let m = BtrMesh::new(&flat_btr(4, 800.0, Some(900.0)), &q);
        assert_eq!(m.triangles(), 2);
        assert_eq!(m.height_at(100.0, -65000.0), Some(800.0));
        assert_eq!(m.height_at(16383.0, -49153.0), Some(800.0));
        assert_eq!(m.height_at(-10.0, -65000.0), None);
        assert_eq!(m.water_at(100.0, -65000.0), Some(900.0));
        assert_eq!(m.water_at(5000.0, -65000.0), None);
        let grid = LodGrid::new((-96, -96), Some(256));
        let bytes = flat_btr(4, 800.0, None);
        let read = move |name: &str| if name == "Tamriel.4.0.-16.btr" { Some(bytes.to_nif("Tamriel", None).to_bytes().unwrap()) } else { None };
        let t = Terrain { btr: Some(BtrTerrain::new("Tamriel", grid, Box::new(read))), land: None };
        let buried = [[10.0, -65000.0, 700.0], [20.0, -65000.0, 700.0], [10.0, -64990.0, 790.0]];
        let straddling = [[10.0, -65000.0, 700.0], [20.0, -65000.0, 900.0], [10.0, -64990.0, 790.0]];
        let outside = [[-10.0, -65000.0, 700.0], [-20.0, -65000.0, 700.0], [-10.0, -64990.0, 790.0]];
        let r = |shift: f32, samples: Samples, outside_hidden: bool| HiddenRule { terrain: TerrainKind::BtrOwn, samples, shift, water: false, outside_hidden };
        assert!(t.hidden(&r(0.0, Samples::Seven, false), &q, &buried));
        assert!(!t.hidden(&r(10.0, Samples::Seven, false), &q, &buried));
        assert!(!t.hidden(&r(0.0, Samples::Vertices, false), &q, &straddling));
        assert!(!t.hidden(&r(0.0, Samples::Seven, false), &q, &outside));
        assert!(t.hidden(&r(0.0, Samples::Seven, true), &q, &outside));
        // The water lookup under the outside point asked for the west neighbour, which does not exist.
        assert_eq!(t.btr.as_ref().unwrap().missing.lock().unwrap().len(), 1);
        let view = t.view(&q);
        assert!(view.hidden(&r(0.0, Samples::Seven, false), &buried));
        assert!(!view.hidden(&r(0.0, Samples::Seven, false), &outside));
        assert_eq!(HiddenRule::parse("btr:v3c:10:water").unwrap().text(), "btr:v3c:10:water:outkept");
        assert_eq!(HiddenRule::parse("btrown:v3:0:water:outhidden").unwrap().text(), "btrown:v3:0:water:outhidden");
        assert!(HiddenRule::parse("x").is_none());
    }
}
