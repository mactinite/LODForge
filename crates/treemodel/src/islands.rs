//! Derived from TreeLODGen, Copyright (C) 2026 Clemus (GPL-3.0, commit dc4cc8a); ported to Rust and
//! modified for LODForge 2026-10-23. See THIRD-PARTY-NOTICES.txt part 1.7.
//!
//! Connected pieces of a triangle mesh (TreeLODGen `WoodSplit` islands and `Islands.Find`; spec
//! 3.1 and 3.2).
//!
//! Two flavours: [by_index] joins triangles that share a vertex index (no welding, so a leaf card
//! touching a branch stays apart from it), [welded] first joins vertices on a 0.01-unit grid
//! (round half to even), so a card split along a UV seam is one piece.

/// One piece: its triangles (indices into the mesh's triangle list) in mesh order.
#[derive(Debug, Clone, Default)]
pub struct Island {
    pub triangles: Vec<usize>,
}

struct Dsu(Vec<u32>);

impl Dsu {
    fn new(n: usize) -> Dsu {
        Dsu((0..n as u32).collect())
    }

    fn find(&mut self, mut x: u32) -> u32 {
        while self.0[x as usize] != x {
            let p = self.0[x as usize];
            self.0[x as usize] = self.0[p as usize];
            x = p;
        }
        x
    }

    fn union(&mut self, a: u32, b: u32) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.0[rb as usize] = ra;
        }
    }
}

fn group(dsu: &mut Dsu, triangles: &[[u32; 3]]) -> Vec<Island> {
    // Islands in order of first appearance while scanning triangles.
    let mut index: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
    let mut out: Vec<Island> = Vec::new();
    for (t, tri) in triangles.iter().enumerate() {
        let root = dsu.find(tri[0]);
        let i = *index.entry(root).or_insert_with(|| {
            out.push(Island::default());
            out.len() - 1
        });
        out[i].triangles.push(t);
    }
    out
}

/// Pieces joined by shared vertex indices only.
pub fn by_index(vertex_count: usize, triangles: &[[u32; 3]]) -> Vec<Island> {
    let mut dsu = Dsu::new(vertex_count);
    for t in triangles {
        dsu.union(t[0], t[1]);
        dsu.union(t[0], t[2]);
    }
    group(&mut dsu, triangles)
}

/// Weld grid of [welded] and of the bark simplifier (game units, shape-local).
pub const WELD_GRID: f32 = 0.01;

/// The weld key of a position.
pub fn weld_key(p: [f32; 3]) -> [i32; 3] {
    p.map(|v| (v / WELD_GRID).round_ties_even() as i32)
}

/// Pieces joined by shared (welded) positions.
pub fn welded(positions: &[[f32; 3]], triangles: &[[u32; 3]]) -> Vec<Island> {
    let mut dsu = Dsu::new(positions.len());
    let mut first: std::collections::HashMap<[i32; 3], u32> = std::collections::HashMap::new();
    for (i, p) in positions.iter().enumerate() {
        let f = *first.entry(weld_key(*p)).or_insert(i as u32);
        dsu.union(f, i as u32);
    }
    for t in triangles {
        dsu.union(t[0], t[1]);
        dsu.union(t[0], t[2]);
    }
    group(&mut dsu, triangles)
}

pub fn triangle_area(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> f32 {
    let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let x = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
    (x[0] * x[0] + x[1] * x[1] + x[2] * x[2]).sqrt() / 2.0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two quads: one card split along a seam (duplicated vertices 4..5 = 1..2), one apart.
    fn mesh() -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
        let p = vec![
            [0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0],
            [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [2.0, 0.0, 0.0], [2.0, 1.0, 0.0],
            [5.0, 0.0, 0.0], [6.0, 0.0, 0.0], [6.0, 1.0, 0.0],
        ];
        let t = vec![[0, 1, 2], [0, 2, 3], [4, 6, 7], [4, 7, 5], [8, 9, 10]];
        (p, t)
    }

    #[test]
    fn index_islands_keep_seams_apart() {
        let (p, t) = mesh();
        let i = by_index(p.len(), &t);
        assert_eq!(i.len(), 3);
        assert_eq!(i[0].triangles, vec![0, 1]);
    }

    #[test]
    fn welded_islands_join_seams() {
        let (p, t) = mesh();
        let i = welded(&p, &t);
        assert_eq!(i.len(), 2);
        assert_eq!(i[0].triangles, vec![0, 1, 2, 3]);
        assert_eq!(i[1].triangles, vec![4]);
    }
}
