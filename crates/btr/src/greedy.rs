//! Greedy insertion of a height field (Garland & Heckbert, "Fast Polygonal Approximation of
//! Terrains and Height Fields", CMU-CS-95-181), ported from the public-domain Terra 0.7
//! (`GreedyInsert.*`, `Subdivision.*`, `Heap.*`, `greedy.cc`) with exact integer predicates and
//! a deterministic point-location walk.
//!
//! The height field is a `width x height` grid of samples. The mesh starts as the two triangles
//! of the grid rectangle (diagonal from the south-west to the north-east corner) and repeatedly
//! inserts the unused grid point whose height differs most from the plane of the triangle it lies
//! in, keeping the triangulation Delaunay, until the largest such difference is at or below the
//! error threshold or the point limit is reached. Points can be pre-selected (forced into the
//! mesh) or ignored (never candidates).

use crate::quadedge::{EdgeId, FaceId, NO_FACE, QuadEdges, inv_rot, rot, sym};

pub const UNUSED: u8 = 0;
pub const USED: u8 = 1;
pub const IGNORED: u8 = 2;

const NOT_IN_HEAP: u32 = u32::MAX;
/// Candidates below this importance are not worth a heap entry (Terra: 1e-4).
const NEGLIGIBLE: f64 = 1e-4;

#[derive(Debug, Clone, Copy)]
struct Face {
    anchor: EdgeId,
    heap_pos: u32,
    /// Candidate grid point (the unused point with the largest error inside the face).
    cx: u16,
    cy: u16,
    import: f64,
    alive: bool,
}

/// Array max-heap keyed by candidate importance, with position tracking in the faces.
#[derive(Debug, Default)]
struct Heap {
    entries: Vec<(f64, FaceId)>,
}

pub struct Greedy<'a> {
    pub width: usize,
    pub height: usize,
    /// Heights, `z[y * width + x]`.
    z: &'a [f32],
    status: Vec<u8>,
    mesh: QuadEdges,
    faces: Vec<Face>,
    free_faces: Vec<FaceId>,
    heap: Heap,
    count: usize,
    start_edge: EdgeId,
    tie: bool,
}

/// Twice the signed area of (a, b, c): positive when counter-clockwise.
#[inline]
fn tri_area(a: (i64, i64), b: (i64, i64), c: (i64, i64)) -> i64 {
    (b.0 - a.0) * (c.1 - a.1) - (b.1 - a.1) * (c.0 - a.0)
}

/// `d` strictly inside the circle through the counter-clockwise triangle (a, b, c)
/// (Guibas & Stolfi, p. 107), evaluated exactly.
fn in_circle(a: (i64, i64), b: (i64, i64), c: (i64, i64), d: (i64, i64)) -> bool {
    let sq = |p: (i64, i64)| (p.0 * p.0 + p.1 * p.1) as i128;
    let v = sq(a) * tri_area(b, c, d) as i128 - sq(b) * tri_area(a, c, d) as i128
        + sq(c) * tri_area(a, b, d) as i128
        - sq(d) * tri_area(a, b, c) as i128;
    v > 0
}

impl<'a> Greedy<'a> {
    /// A `width x height` height field (`z[y * width + x]`), with its four corners selected.
    pub fn new(width: usize, height: usize, z: &'a [f32]) -> Self {
        assert!(width >= 2 && height >= 2 && z.len() == width * height);
        let mut g = Greedy {
            width,
            height,
            z,
            status: vec![UNUSED; width * height],
            mesh: QuadEdges::with_capacity(64),
            faces: Vec::with_capacity(64),
            free_faces: Vec::new(),
            heap: Heap::default(),
            count: 0,
            start_edge: 0,
            tie: false,
        };
        g.init_rectangle();
        g
    }

    #[inline]
    fn index(&self, x: usize, y: usize) -> u32 {
        (y * self.width + x) as u32
    }
    #[inline]
    fn point(&self, v: u32) -> (i64, i64) {
        (
            (v as usize % self.width) as i64,
            (v as usize / self.width) as i64,
        )
    }
    #[inline]
    fn height_of(&self, v: u32) -> f64 {
        f64::from(self.z[v as usize])
    }

    /// The rectangle a (SW) - b (NW) - c (NE) - d (SE) with the diagonal a-c, as Terra's
    /// `initMesh`.
    fn init_rectangle(&mut self) {
        let (w, h) = (self.width, self.height);
        let a = self.index(0, 0);
        let b = self.index(0, h - 1);
        let c = self.index(w - 1, h - 1);
        let d = self.index(w - 1, 0);
        let m = &mut self.mesh;
        let ea = m.make_edge();
        m.set_endpoints(ea, a, b);
        let eb = m.make_edge();
        m.splice(sym(ea), eb);
        m.set_endpoints(eb, b, c);
        let ec = m.make_edge();
        m.splice(sym(eb), ec);
        m.set_endpoints(ec, c, d);
        let ed = m.make_edge();
        m.splice(sym(ec), ed);
        m.set_endpoints(ed, d, a);
        m.splice(sym(ed), ea);
        let diag = m.make_edge();
        m.splice(sym(ed), diag);
        m.splice(sym(eb), sym(diag));
        m.set_endpoints(diag, a, c);
        self.start_edge = ea;
        for v in [a, b, c, d] {
            self.status[v as usize] = USED;
        }
        self.count = 4;
        let f1 = self.new_face(sym(ea));
        let f2 = self.new_face(sym(ec));
        self.scan(f1);
        self.scan(f2);
    }

    fn new_face(&mut self, anchor: EdgeId) -> FaceId {
        let face = Face {
            anchor,
            heap_pos: NOT_IN_HEAP,
            cx: 0,
            cy: 0,
            import: 0.0,
            alive: true,
        };
        let id = match self.free_faces.pop() {
            Some(id) => {
                self.faces[id as usize] = face;
                id
            }
            None => {
                self.faces.push(face);
                (self.faces.len() - 1) as FaceId
            }
        };
        self.mesh.label_triangle(anchor, id);
        id
    }

    fn free_face(&mut self, f: FaceId) {
        if f == NO_FACE || !self.faces[f as usize].alive {
            return;
        }
        if self.faces[f as usize].heap_pos != NOT_IN_HEAP {
            self.heap_remove(f);
        }
        self.faces[f as usize].alive = false;
        self.free_faces.push(f);
    }

    pub fn status(&self, x: usize, y: usize) -> u8 {
        self.status[y * self.width + x]
    }

    /// Marks a point as never-a-candidate (no effect on used points).
    pub fn ignore(&mut self, x: usize, y: usize) {
        let i = y * self.width + x;
        if self.status[i] == UNUSED {
            self.status[i] = IGNORED;
        }
    }

    /// Forces a grid point into the mesh. Returns false when it is already used.
    pub fn select(&mut self, x: usize, y: usize) -> bool {
        let v = self.index(x, y);
        if self.status[v as usize] == USED {
            return false;
        }
        self.status[v as usize] = USED;
        self.count += 1;
        let hint = self.start_edge;
        self.insert(v, hint);
        true
    }

    pub fn point_count(&self) -> usize {
        self.count
    }

    /// Largest candidate error in the mesh (0 when every triangle is within tolerance).
    pub fn max_error(&self) -> f64 {
        self.heap.entries.first().map_or(0.0, |e| e.0)
    }

    /// Inserts candidates while the largest error exceeds `threshold` and fewer than
    /// `point_limit` points are in the mesh.
    pub fn run(&mut self, threshold: f64, point_limit: usize) {
        while self.count < point_limit {
            let Some((import, f)) = self.heap.entries.first().copied() else {
                break;
            };
            if import <= threshold {
                break;
            }
            let face = self.faces[f as usize];
            let (x, y) = (usize::from(face.cx), usize::from(face.cy));
            let v = self.index(x, y);
            if self.status[v as usize] != UNUSED {
                // The candidate was ignored (or selected) after the face was scanned: rescan.
                self.scan(f);
                continue;
            }
            self.heap_remove(f);
            self.status[v as usize] = USED;
            self.count += 1;
            self.insert(v, face.anchor);
        }
    }

    /// Every selected grid point as (x, y), in grid order.
    pub fn used_points(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        let w = self.width;
        self.status
            .iter()
            .enumerate()
            .filter(|(_, s)| **s == USED)
            .map(move |(i, _)| (i % w, i / w))
    }

    /// Every triangle as its three grid points, counter-clockwise, in face-arena order.
    pub fn triangles(&self) -> Vec<[(usize, usize); 3]> {
        let mut out = Vec::with_capacity(self.faces.len());
        for f in &self.faces {
            if !f.alive {
                continue;
            }
            let e = f.anchor;
            let (a, b, c) = (
                self.mesh.org(e),
                self.mesh.dest(e),
                self.mesh.org(self.mesh.lprev(e)),
            );
            let p = |v: u32| (v as usize % self.width, v as usize / self.width);
            out.push([p(a), p(b), p(c)]);
        }
        out
    }

    // ---- point location (Terra's `locate`, with a deterministic tie-break) ----

    fn locate(&mut self, x: (i64, i64), start: EdgeId) -> EdgeId {
        let mut tie = self.tie;
        let m = &self.mesh;
        let width = self.width;
        let pt = |v: u32| ((v as usize % width) as i64, (v as usize / width) as i64);
        let mut e = start;
        let mut t = tri_area(x, pt(m.dest(e)), pt(m.org(e)));
        if t > 0 {
            t = -t;
            e = sym(e);
        }
        loop {
            let eo = m.onext(e);
            let ed = m.dprev(e);
            let to = tri_area(x, pt(m.dest(eo)), pt(m.org(eo)));
            let td = tri_area(x, pt(m.dest(ed)), pt(m.org(ed)));
            if td > 0 {
                if to > 0 || (to == 0 && t == 0) {
                    self.tie = tie;
                    self.start_edge = e;
                    return e;
                }
                t = to;
                e = eo;
            } else if to > 0 {
                if td == 0 && t == 0 {
                    self.tie = tie;
                    self.start_edge = e;
                    return e;
                }
                t = td;
                e = ed;
            } else if t == 0 && tri_area(pt(m.dest(eo)), pt(m.org(e)), pt(m.dest(e))) <= 0 {
                // x is on e but the subdivision lies to the right of it.
                e = sym(e);
            } else {
                tie = !tie;
                if tie {
                    t = to;
                    e = eo;
                } else {
                    t = td;
                    e = ed;
                }
            }
        }
    }

    fn on_edge(&self, x: (i64, i64), e: EdgeId) -> bool {
        let o = self.point(self.mesh.org(e));
        let d = self.point(self.mesh.dest(e));
        if tri_area(x, o, d) != 0 {
            return false;
        }
        // Collinear: between the endpoints (strictly, since x is not a vertex).
        let dot = (x.0 - o.0) * (d.0 - o.0) + (x.1 - o.1) * (d.1 - o.1);
        let len2 = (d.0 - o.0) * (d.0 - o.0) + (d.1 - o.1) * (d.1 - o.1);
        dot > 0 && dot < len2
    }

    // ---- insertion: spoke the located face (or the two faces of the located edge), restore
    //      Delaunay by swapping, rescan the faces around the new point ----

    fn insert(&mut self, v: u32, hint: EdgeId) {
        let x = self.point(v);
        let mut e = self.locate(x, hint);
        debug_assert!(
            self.mesh.org(e) != v && self.mesh.dest(e) != v,
            "reinsertion of a used point"
        );
        let mut boundary_edge = None;
        if self.on_edge(x, e) {
            if self.mesh.lface(e) == NO_FACE {
                e = sym(e);
            }
            if self.mesh.lface(sym(e)) == NO_FACE {
                // On a boundary edge: spoke the inner triangle as if x were inside it, then drop
                // the edge, which turns the degenerate (org, dest, x) face into outer face.
                boundary_edge = Some(e);
                self.free_face(self.mesh.lface(e));
            } else {
                self.free_face(self.mesh.lface(e));
                self.free_face(self.mesh.lface(sym(e)));
                e = self.mesh.oprev(e);
                let old = self.mesh.onext(e);
                self.mesh.delete_edge(old);
            }
        } else {
            self.free_face(self.mesh.lface(e));
        }
        // Spokes from every corner of the polygon around x to x (Guibas-Stolfi `InsertSite`).
        let base0 = self.mesh.make_edge();
        let org = self.mesh.org(e);
        self.mesh.set_endpoints(base0, org, v);
        self.mesh.splice(base0, e);
        let start = base0;
        let mut base = base0;
        loop {
            base = self.mesh.connect(e, sym(base));
            e = self.mesh.oprev(base);
            if self.mesh.lnext(e) == start {
                break;
            }
        }
        if let Some(be) = boundary_edge {
            self.mesh.delete_edge(be);
        }
        self.start_edge = start;
        // Faces around x.
        let first_spoke = sym(start);
        let mut s = first_spoke;
        loop {
            if self.mesh.left_is_triangle(s) {
                self.new_face(s);
            } else {
                self.mesh.set_lface(s, NO_FACE);
                let n = self.mesh.lnext(s);
                self.mesh.set_lface(n, NO_FACE);
            }
            s = self.mesh.onext(s);
            if s == first_spoke {
                break;
            }
        }
        // Delaunay: swap every interior edge opposite x that fails the circle test (Terra's
        // `optimize`: stay on the spoke after a swap, advance otherwise).
        let mut s = first_spoke;
        loop {
            let opp = self.mesh.lnext(s);
            if self.mesh.lface(opp) != NO_FACE
                && self.mesh.lface(sym(opp)) != NO_FACE
                && self.should_swap(x, opp)
            {
                let f1 = self.mesh.lface(opp);
                let f2 = self.mesh.lface(sym(opp));
                self.mesh.swap(opp);
                self.mesh.label_triangle(opp, f1);
                self.faces[f1 as usize].anchor = opp;
                self.mesh.label_triangle(sym(opp), f2);
                self.faces[f2 as usize].anchor = sym(opp);
            } else {
                s = self.mesh.onext(s);
                if s == first_spoke {
                    break;
                }
            }
        }
        // Rescan the faces around x.
        let mut s = first_spoke;
        loop {
            let f = self.mesh.lface(s);
            if f != NO_FACE {
                self.scan(f);
            }
            s = self.mesh.onext(s);
            if s == first_spoke {
                break;
            }
        }
    }

    /// Terra's `shouldSwap`: the vertex across `e` lies inside the circle through
    /// (org(e), that vertex, dest(e)) and x.
    fn should_swap(&self, x: (i64, i64), e: EdgeId) -> bool {
        let t = self.mesh.oprev(e);
        let a = self.point(self.mesh.org(e));
        let b = self.point(self.mesh.dest(t));
        let c = self.point(self.mesh.dest(e));
        tri_area(a, b, c) > 0 && in_circle(a, b, c, x)
    }

    // ---- scanning ----

    /// Finds the unused grid point inside face `f` with the largest |height - plane| and files
    /// it in the heap (Terra's `scanTriangle`).
    fn scan(&mut self, f: FaceId) {
        let anchor = self.faces[f as usize].anchor;
        let (va, vb, vc) = (
            self.mesh.org(anchor),
            self.mesh.dest(anchor),
            self.mesh.org(self.mesh.lprev(anchor)),
        );
        let mut pts = [
            (self.point(va), self.height_of(va)),
            (self.point(vb), self.height_of(vb)),
            (self.point(vc), self.height_of(vc)),
        ];
        // Plane z = a x + b y + c through the three corners.
        let (p, q, r) = (pts[0], pts[1], pts[2]);
        let (ux, uy, uz) = ((q.0.0 - p.0.0) as f64, (q.0.1 - p.0.1) as f64, q.1 - p.1);
        let (vx, vy, vz) = ((r.0.0 - p.0.0) as f64, (r.0.1 - p.0.1) as f64, r.1 - p.1);
        let den = ux * vy - uy * vx;
        if den == 0.0 {
            self.heap_remove(f);
            return;
        }
        let pa = (uz * vy - uy * vz) / den;
        let pb = (ux * vz - uz * vx) / den;
        let pc = p.1 - pa * p.0.0 as f64 - pb * p.0.1 as f64;
        pts.sort_by_key(|(pt, _)| (pt.1, pt.0));
        let (ymin, ymax) = (pts[0].0.1, pts[2].0.1);
        let mut best = (-1.0f64, 0usize, 0usize);
        let edges = [
            (pts[0].0, pts[1].0),
            (pts[1].0, pts[2].0),
            (pts[0].0, pts[2].0),
        ];
        for y in ymin..=ymax {
            // Exact x range of the triangle on this row.
            let (mut xl, mut xr) = (i64::MAX, i64::MIN);
            for &(s, t) in &edges {
                let (lo, hi) = if s.1 <= t.1 { (s, t) } else { (t, s) };
                if y < lo.1 || y > hi.1 {
                    continue;
                }
                if lo.1 == hi.1 {
                    xl = xl.min(lo.0.min(hi.0));
                    xr = xr.max(lo.0.max(hi.0));
                } else {
                    let num = (y - lo.1) * (hi.0 - lo.0);
                    let den = hi.1 - lo.1;
                    xl = xl.min(lo.0 + num.div_euclid(den) + i64::from(num.rem_euclid(den) != 0));
                    xr = xr.max(lo.0 + num.div_euclid(den));
                }
            }
            if xl > xr {
                continue;
            }
            let row = y as usize * self.width;
            let mut plane = pa * xl as f64 + pb * y as f64 + pc;
            for x in xl..=xr {
                let i = row + x as usize;
                if self.status[i] == UNUSED {
                    let diff = (f64::from(self.z[i]) - plane).abs();
                    if diff > best.0 {
                        best = (diff, x as usize, y as usize);
                    }
                }
                plane += pa;
            }
        }
        if best.0 < NEGLIGIBLE {
            self.heap_remove(f);
        } else {
            let face = &mut self.faces[f as usize];
            face.cx = best.1 as u16;
            face.cy = best.2 as u16;
            face.import = best.0;
            self.heap_set(f, best.0);
        }
    }

    // ---- heap ----

    fn heap_set(&mut self, f: FaceId, import: f64) {
        let pos = self.faces[f as usize].heap_pos;
        if pos == NOT_IN_HEAP {
            self.heap.entries.push((import, f));
            let i = self.heap.entries.len() - 1;
            self.faces[f as usize].heap_pos = i as u32;
            self.up_heap(i);
        } else {
            let i = pos as usize;
            let old = self.heap.entries[i].0;
            self.heap.entries[i].0 = import;
            if import < old {
                self.down_heap(i);
            } else {
                self.up_heap(i);
            }
        }
    }

    fn heap_remove(&mut self, f: FaceId) {
        let pos = self.faces[f as usize].heap_pos;
        if pos == NOT_IN_HEAP {
            return;
        }
        let i = pos as usize;
        let last = self.heap.entries.len() - 1;
        self.heap_swap(i, last);
        self.heap.entries.pop();
        self.faces[f as usize].heap_pos = NOT_IN_HEAP;
        if i < self.heap.entries.len() {
            let moved = self.heap.entries[i].0;
            let parent = if i == 0 {
                None
            } else {
                Some(self.heap.entries[(i - 1) / 2].0)
            };
            if parent.is_some_and(|p| moved > p) {
                self.up_heap(i);
            } else {
                self.down_heap(i);
            }
        }
    }

    fn heap_swap(&mut self, i: usize, j: usize) {
        self.heap.entries.swap(i, j);
        let fi = self.heap.entries[i].1;
        let fj = self.heap.entries[j].1;
        self.faces[fi as usize].heap_pos = i as u32;
        self.faces[fj as usize].heap_pos = j as u32;
    }

    fn up_heap(&mut self, mut i: usize) {
        while i > 0 {
            let p = (i - 1) / 2;
            if self.heap.entries[i].0 > self.heap.entries[p].0 {
                self.heap_swap(i, p);
                i = p;
            } else {
                break;
            }
        }
    }

    fn down_heap(&mut self, mut i: usize) {
        let n = self.heap.entries.len();
        loop {
            let (l, r) = (2 * i + 1, 2 * i + 2);
            let mut largest = i;
            if l < n && self.heap.entries[l].0 > self.heap.entries[largest].0 {
                largest = l;
            }
            if r < n && self.heap.entries[r].0 > self.heap.entries[largest].0 {
                largest = r;
            }
            if largest == i {
                break;
            }
            self.heap_swap(i, largest);
            i = largest;
        }
    }

    /// Mesh checks for tests: every face is a counter-clockwise triangle whose edges carry its
    /// label, every interior edge is locally Delaunay, and the outer boundary is the rectangle.
    #[cfg(test)]
    fn check_invariants(&self) {
        let m = &self.mesh;
        for (id, f) in self.faces.iter().enumerate() {
            if !f.alive {
                continue;
            }
            let e = f.anchor;
            assert!(m.left_is_triangle(e));
            let (a, b, c) = (
                self.point(m.org(e)),
                self.point(m.dest(e)),
                self.point(m.org(m.lprev(e))),
            );
            assert!(
                tri_area(a, b, c) > 0,
                "face {id} not ccw: {a:?} {b:?} {c:?}"
            );
            for x in [e, m.lnext(e), m.lprev(e)] {
                assert_eq!(m.lface(x), id as u32);
                if m.lface(sym(x)) != NO_FACE {
                    let d = self.point(m.dest(m.oprev(x)));
                    let (o, de) = (self.point(m.org(x)), self.point(m.dest(x)));
                    let apex = self.point(m.org(m.lprev(x)));
                    assert!(
                        !(tri_area(o, d, de) > 0 && in_circle(o, d, de, apex)),
                        "edge {o:?}-{de:?} not Delaunay"
                    );
                }
            }
        }
    }
}

// Silence the unused-import warning when the helpers are only used in tests.
#[allow(dead_code)]
fn _rot_helpers(e: EdgeId) -> (EdgeId, EdgeId) {
    (rot(e), inv_rot(e))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(w: usize, h: usize, f: impl Fn(usize, usize) -> f32) -> Vec<f32> {
        (0..w * h).map(|i| f(i % w, i / w)).collect()
    }

    fn max_error_of(g: &Greedy<'_>) -> f64 {
        // Brute force: every grid point against the triangle containing it.
        let tris = g.triangles();
        let mut worst = 0.0f64;
        for y in 0..g.height {
            for x in 0..g.width {
                let p = (x as i64, y as i64);
                let mut found = false;
                for t in &tris {
                    let pts = t.map(|(a, b)| (a as i64, b as i64));
                    let s = [
                        tri_area(pts[0], pts[1], p),
                        tri_area(pts[1], pts[2], p),
                        tri_area(pts[2], pts[0], p),
                    ];
                    if s.iter().all(|&v| v >= 0) {
                        found = true;
                        let zs = t.map(|(a, b)| f64::from(g.z[b * g.width + a]));
                        let area = tri_area(pts[0], pts[1], pts[2]) as f64;
                        let z = (s[1] as f64 * zs[0] + s[2] as f64 * zs[1] + s[0] as f64 * zs[2])
                            / area;
                        worst = worst.max((z - f64::from(g.z[y * g.width + x])).abs());
                        break;
                    }
                }
                assert!(found, "point {x},{y} not covered");
            }
        }
        worst
    }

    #[test]
    fn flat_field_stays_two_triangles() {
        let z = field(9, 9, |_, _| 5.0);
        let mut g = Greedy::new(9, 9, &z);
        g.run(0.5, 1000);
        assert_eq!(g.point_count(), 4);
        assert_eq!(g.triangles().len(), 2);
        g.check_invariants();
    }

    #[test]
    fn meets_threshold_and_stays_delaunay() {
        let z = field(33, 33, |x, y| {
            ((x as f32 * 0.4).sin() * 20.0 + (y as f32 * 0.25).cos() * 15.0 + (x * y) as f32 * 0.01)
        });
        let mut g = Greedy::new(33, 33, &z);
        g.run(2.0, 100_000);
        g.check_invariants();
        let worst = max_error_of(&g);
        assert!(worst <= 2.0 + 1e-6, "worst {worst}");
        assert!(g.point_count() > 4 && g.point_count() < 33 * 33);
        let n_used = g.used_points().count();
        assert_eq!(n_used, g.point_count());
        // Euler: triangles = 2 * points - boundary points - 2.
        let boundary = g
            .used_points()
            .filter(|&(x, y)| x == 0 || y == 0 || x == 32 || y == 32)
            .count();
        assert_eq!(g.triangles().len(), 2 * n_used - boundary - 2);
    }

    #[test]
    fn preselection_and_ignores() {
        let z = field(17, 17, |x, y| {
            if (x, y) == (8, 8) {
                100.0
            } else if x == 3 {
                50.0
            } else {
                0.0
            }
        });
        let mut g = Greedy::new(17, 17, &z);
        // Boundary and interior pre-insertions, incl. points on existing edges.
        assert!(g.select(0, 8));
        assert!(g.select(16, 8));
        assert!(g.select(8, 0));
        assert!(g.select(8, 8));
        assert!(!g.select(8, 8));
        g.check_invariants();
        for y in 0..17 {
            g.ignore(3, y);
        }
        g.run(0.1, 10_000);
        g.check_invariants();
        assert_eq!(g.max_error(), 0.0);
        // The ignored column is never selected (except the corners / pre-inserted points).
        assert!(g.used_points().all(|(x, _)| x != 3));
        assert!(g.used_points().any(|p| p == (8, 8)));
    }

    #[test]
    fn point_limit_stops() {
        let z = field(33, 33, |x, y| ((x * 7 + y * 13) % 11) as f32);
        let mut g = Greedy::new(33, 33, &z);
        g.run(0.0, 50);
        assert_eq!(g.point_count(), 50);
        g.check_invariants();
    }

    #[test]
    fn full_insertion_is_exact() {
        let z = field(9, 9, |x, y| ((x * 3 + y * 5) % 7) as f32);
        let mut g = Greedy::new(9, 9, &z);
        g.run(0.0, 10_000);
        g.check_invariants();
        assert_eq!(max_error_of(&g), 0.0);
    }
}
