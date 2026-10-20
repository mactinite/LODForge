//! Quad-edge planar subdivision (Guibas & Stolfi 1985) in an arena, with the face labels the
//! greedy mesher needs. This is the structure Terra 0.7 (Garland & Heckbert, public domain,
//! `Quadedge.*` / `Subdivision.*`) builds its Delaunay triangulation on; the primitives here are
//! the textbook ones: `Onext`, `Rot`, `Sym`, `splice`, `make_edge`, `delete_edge`, `connect`,
//! `swap`.
//!
//! An edge id is `quad_edge_index * 4 + rotation`; rotations 0 and 2 are the primal directed
//! edge and its symmetric, 1 and 3 the duals. Vertices are plain `u32` labels chosen by the
//! caller (the mesher uses grid indices); faces are `u32` labels with [`NO_FACE`] for the outer
//! face.

pub type EdgeId = u32;
pub type FaceId = u32;
pub const NO_FACE: FaceId = u32::MAX;
pub const NO_EDGE: EdgeId = u32::MAX;

#[derive(Debug, Default)]
pub struct QuadEdges {
    /// `Onext` per edge id.
    next: Vec<EdgeId>,
    /// Origin vertex per edge id (meaningful for rotations 0 and 2).
    org: Vec<u32>,
    /// Left face per edge id (rotations 0 and 2).
    lface: Vec<FaceId>,
    free: Vec<u32>,
}

#[inline]
pub fn rot(e: EdgeId) -> EdgeId {
    (e & !3) | ((e + 1) & 3)
}
#[inline]
pub fn sym(e: EdgeId) -> EdgeId {
    e ^ 2
}
#[inline]
pub fn inv_rot(e: EdgeId) -> EdgeId {
    (e & !3) | ((e + 3) & 3)
}

impl QuadEdges {
    pub fn with_capacity(edges: usize) -> Self {
        QuadEdges {
            next: Vec::with_capacity(edges * 4),
            org: Vec::with_capacity(edges * 4),
            lface: Vec::with_capacity(edges * 4),
            free: Vec::new(),
        }
    }

    /// A new isolated edge (its own `Onext`, duals forming a loop), rotation 0.
    pub fn make_edge(&mut self) -> EdgeId {
        let q = match self.free.pop() {
            Some(q) => q,
            None => {
                let q = (self.next.len() / 4) as u32;
                self.next.extend_from_slice(&[0; 4]);
                self.org.extend_from_slice(&[u32::MAX; 4]);
                self.lface.extend_from_slice(&[NO_FACE; 4]);
                q
            }
        };
        let e0 = q * 4;
        self.next[e0 as usize] = e0;
        self.next[e0 as usize + 1] = e0 + 3;
        self.next[e0 as usize + 2] = e0 + 2;
        self.next[e0 as usize + 3] = e0 + 1;
        self.org[e0 as usize..e0 as usize + 4].fill(u32::MAX);
        self.lface[e0 as usize..e0 as usize + 4].fill(NO_FACE);
        e0
    }

    #[inline]
    pub fn onext(&self, e: EdgeId) -> EdgeId {
        self.next[e as usize]
    }
    #[inline]
    pub fn oprev(&self, e: EdgeId) -> EdgeId {
        rot(self.onext(rot(e)))
    }
    #[inline]
    pub fn lnext(&self, e: EdgeId) -> EdgeId {
        rot(self.onext(inv_rot(e)))
    }
    #[inline]
    pub fn lprev(&self, e: EdgeId) -> EdgeId {
        sym(self.onext(e))
    }
    #[inline]
    pub fn dprev(&self, e: EdgeId) -> EdgeId {
        inv_rot(self.onext(inv_rot(e)))
    }
    #[inline]
    pub fn rprev(&self, e: EdgeId) -> EdgeId {
        self.onext(sym(e))
    }
    #[inline]
    pub fn org(&self, e: EdgeId) -> u32 {
        self.org[e as usize]
    }
    #[inline]
    pub fn dest(&self, e: EdgeId) -> u32 {
        self.org[sym(e) as usize]
    }
    #[inline]
    pub fn set_endpoints(&mut self, e: EdgeId, org: u32, dest: u32) {
        self.org[e as usize] = org;
        self.org[sym(e) as usize] = dest;
    }
    #[inline]
    pub fn lface(&self, e: EdgeId) -> FaceId {
        self.lface[e as usize]
    }
    #[inline]
    pub fn set_lface(&mut self, e: EdgeId, f: FaceId) {
        self.lface[e as usize] = f;
    }

    /// The fundamental operator: swaps the `Onext` rings of `a` and `b` (and of their duals).
    pub fn splice(&mut self, a: EdgeId, b: EdgeId) {
        let alpha = rot(self.onext(a));
        let beta = rot(self.onext(b));
        let t1 = self.onext(b);
        let t2 = self.onext(a);
        let t3 = self.onext(beta);
        let t4 = self.onext(alpha);
        self.next[a as usize] = t1;
        self.next[b as usize] = t2;
        self.next[alpha as usize] = t3;
        self.next[beta as usize] = t4;
    }

    /// Removes `e` from the subdivision and frees its quad edge.
    pub fn delete_edge(&mut self, e: EdgeId) {
        let a = self.oprev(e);
        self.splice(e, a);
        let b = self.oprev(sym(e));
        self.splice(sym(e), b);
        self.free.push(e >> 2);
    }

    /// New edge from `dest(a)` to `org(b)` inside the left face of `a` (Guibas-Stolfi `Connect`).
    pub fn connect(&mut self, a: EdgeId, b: EdgeId) -> EdgeId {
        let e = self.make_edge();
        let ln = self.lnext(a);
        self.splice(e, ln);
        self.splice(sym(e), b);
        let (org, dest) = (self.dest(a), self.org(b));
        self.set_endpoints(e, org, dest);
        e
    }

    /// Rotates `e` inside the quadrilateral formed by its two faces (Guibas-Stolfi `Swap`): it
    /// then joins the two vertices opposite its old endpoints. Face labels are not updated.
    pub fn swap(&mut self, e: EdgeId) {
        let a = self.oprev(e);
        let b = self.oprev(sym(e));
        self.splice(e, a);
        self.splice(sym(e), b);
        let la = self.lnext(a);
        self.splice(e, la);
        let lb = self.lnext(b);
        self.splice(sym(e), lb);
        let (org, dest) = (self.dest(a), self.dest(b));
        self.set_endpoints(e, org, dest);
    }

    /// Labels the three edges of the triangle to the left of `e` with `f`.
    pub fn label_triangle(&mut self, e: EdgeId, f: FaceId) {
        let n = self.lnext(e);
        let p = self.lprev(e);
        self.set_lface(e, f);
        self.set_lface(n, f);
        self.set_lface(p, f);
    }

    /// True when the left face of `e` is a triangle (`Lnext` three times returns to `e`).
    pub fn left_is_triangle(&self, e: EdgeId) -> bool {
        let n = self.lnext(e);
        let nn = self.lnext(n);
        self.lnext(nn) == e && n != e
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn isolated_edge_rings() {
        let mut q = QuadEdges::default();
        let e = q.make_edge();
        assert_eq!(q.onext(e), e);
        assert_eq!(sym(e), e + 2);
        assert_eq!(q.onext(sym(e)), sym(e));
        assert_eq!(rot(sym(e)), sym(rot(e)));
        assert_eq!(q.lnext(e), sym(e));
        assert_eq!(q.lprev(e), sym(e));
    }

    #[test]
    fn connect_makes_a_triangle() {
        let mut q = QuadEdges::default();
        let a = q.make_edge();
        q.set_endpoints(a, 0, 1);
        let b = q.make_edge();
        q.set_endpoints(b, 1, 2);
        q.splice(sym(a), b);
        let c = q.connect(b, a);
        assert_eq!((q.org(c), q.dest(c)), (2, 0));
        assert_eq!(q.lnext(a), b);
        assert_eq!(q.lnext(b), c);
        assert_eq!(q.lnext(c), a);
        assert!(q.left_is_triangle(a));
        assert!(q.left_is_triangle(sym(a)));
        q.label_triangle(a, 7);
        assert_eq!(
            (q.lface(a), q.lface(b), q.lface(c), q.lface(sym(a))),
            (7, 7, 7, NO_FACE)
        );
    }
}
