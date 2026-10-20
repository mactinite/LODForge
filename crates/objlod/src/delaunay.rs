//! The s-hull.org C# Delaunay triangulator (copyright s-hull.org 2011, released under the
//! contributors beerware license; contributors: Phil Atkin, Dr Sinclair), ported
//! with its f32 / f64 arithmetic and its quirks (seed triangle search, hull walk, flip passes).

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pt {
    pub x: f32,
    pub y: f32,
}

impl Pt {
    fn dist2(&self, o: &Pt) -> f32 {
        let (dx, dy) = (self.x - o.x, self.y - o.y);
        dx * dx + dy * dy
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Triad {
    pub a: i32,
    pub b: i32,
    pub c: i32,
    pub ab: i32,
    pub bc: i32,
    pub ac: i32,
    r2: f32,
    cx: f32,
    cy: f32,
}

impl Triad {
    fn new(a: i32, b: i32, c: i32) -> Triad {
        Triad { a, b, c, ab: -1, bc: -1, ac: -1, r2: -1.0, cx: 0.0, cy: 0.0 }
    }

    #[allow(clippy::too_many_arguments)]
    fn initialize(&mut self, a: i32, b: i32, c: i32, ab: i32, bc: i32, ac: i32, pts: &[Pt]) {
        (self.a, self.b, self.c, self.ab, self.bc, self.ac) = (a, b, c, ab, bc, ac);
        self.circumcircle(pts);
    }

    fn make_clockwise(&mut self, pts: &[Pt]) {
        let (pa, pb, pc) = (pts[self.a as usize], pts[self.b as usize], pts[self.c as usize]);
        let cx = (pa.x + pb.x + pc.x) / 3.0;
        let cy = (pa.y + pb.y + pc.y) / 3.0;
        let (dr0, dr1) = (pa.x - cx, pa.y - cy);
        let (dx01, dy01) = (pb.x - pa.x, pb.y - pa.y);
        let df = (0.0 - dx01) * dr1 + dy01 * dr0;
        if df > 0.0 {
            std::mem::swap(&mut self.b, &mut self.c);
            std::mem::swap(&mut self.ab, &mut self.ac);
        }
    }

    /// `FindCircumcirclePrecisely`.
    fn circumcircle(&mut self, pts: &[Pt]) -> bool {
        let (p0, p1, p2) = (pts[self.a as usize], pts[self.b as usize], pts[self.c as usize]);
        let ax = f64::from(p1.x - p0.x);
        let ay = f64::from(p1.y - p0.y);
        let bx = f64::from(p2.x - p0.x);
        let by = f64::from(p2.y - p0.y);
        let m = ax * ax + ay * ay;
        let u = bx * bx + by * by;
        let s = ax * by - ay * bx;
        if s == 0.0 {
            (self.cx, self.cy, self.r2) = (0.0, 0.0, -1.0);
            return false;
        }
        let s = 0.5 / s;
        let cx = (by * m - ay * u) * s;
        let cy = (ax * u - bx * m) * s;
        let r2 = cx * cx + cy * cy;
        if r2 > 10_000_000_000.0 * m || r2 > 10_000_000_000.0 * u {
            (self.cx, self.cy, self.r2) = (0.0, 0.0, -1.0);
            return false;
        }
        self.r2 = r2 as f32;
        self.cx = (f64::from(p0.x) + cx) as f32;
        self.cy = (f64::from(p0.y) + cy) as f32;
        true
    }

    fn inside(&self, p: &Pt) -> bool {
        let (dx, dy) = (self.cx - p.x, self.cy - p.y);
        dx * dx + dy * dy < self.r2
    }

    fn change_adjacent(&mut self, from: i32, to: i32) {
        if self.ab == from {
            self.ab = to;
        } else if self.bc == from {
            self.bc = to;
        } else if self.ac == from {
            self.ac = to;
        }
    }

    /// `FindAdjacency` -> (opposite, left, right).
    fn adjacency(&self, vertex: i32, triangle: i32) -> (i32, i32, i32) {
        if self.ab == triangle {
            if vertex == self.a { (self.c, self.ac, self.bc) } else { (self.c, self.bc, self.ac) }
        } else if self.ac == triangle {
            if vertex == self.a { (self.b, self.ab, self.bc) } else { (self.b, self.bc, self.ab) }
        } else if self.bc == triangle {
            if vertex == self.b { (self.a, self.ab, self.ac) } else { (self.a, self.ac, self.ab) }
        } else {
            (0, 0, 0)
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct HullV {
    x: f32,
    y: f32,
    point: i32,
    triad: i32,
}

struct Hull(Vec<HullV>);

impl Hull {
    fn next(&self, i: usize) -> usize {
        if i == self.0.len() - 1 { 0 } else { i + 1 }
    }
    fn to_next(&self, i: usize) -> (f32, f32) {
        let (a, b) = (self.0[i], self.0[self.next(i)]);
        (b.x - a.x, b.y - a.y)
    }
    fn visible_d(&self, i: usize, dx: f32, dy: f32) -> bool {
        let (ex, ey) = self.to_next(i);
        (0.0 - dy) * ex + dx * ey < 0.0
    }
    fn visible_p(&self, i: usize, p: &HullV) -> bool {
        let (ex, ey) = self.to_next(i);
        let (dx, dy) = (p.x - self.0[i].x, p.y - self.0[i].y);
        (0.0 - dy) * ex + dx * ey < 0.0
    }
}

/// Errors LODGen raises as exceptions (fewer than three distinct points).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TooFewPoints;

/// .NET `Array.Sort(keys, items, index, length)` for the sizes used here: insertion sort below 17
/// elements (stable); a stable sort is used beyond that too.
fn sort_by_keys(keys: &mut [f32], items: &mut [i32]) {
    let mut idx: Vec<usize> = (0..keys.len()).collect();
    idx.sort_by(|&a, &b| keys[a].partial_cmp(&keys[b]).unwrap_or(std::cmp::Ordering::Equal));
    let k: Vec<f32> = idx.iter().map(|&i| keys[i]).collect();
    let v: Vec<i32> = idx.iter().map(|&i| items[i]).collect();
    keys.copy_from_slice(&k);
    items.copy_from_slice(&v);
}

fn analyse(points: &[Pt], hull: &mut Hull, triads: &mut Vec<Triad>, reject_duplicates: bool, hull_only: bool) -> Result<(), TooFewPoints> {
    if points.len() < 3 {
        return Err(TooFewPoints);
    }
    let mut n = points.len();
    let mut d: Vec<f32> = (0..n).map(|i| points[0].dist2(&points[i])).collect();
    let mut ix: Vec<i32> = (0..n as i32).collect();
    sort_by_keys(&mut d, &mut ix);
    if reject_duplicates {
        let mut k = n as i32 - 2;
        while k >= 0 {
            let ku = k as usize;
            let (p, q) = (points[ix[ku] as usize], points[ix[ku + 1] as usize]);
            if p.x == q.x && p.y == q.y {
                ix.copy_within(ku + 2..n, ku + 1);
                d.copy_within(ku + 2..n, ku + 1);
                n -= 1;
            }
            k -= 1;
        }
    }
    if n < 3 {
        return Err(TooFewPoints);
    }
    let mut mid = -1i32;
    let mut romin2 = f32::MAX;
    let (mut cx, mut cy) = (0.0f32, 0.0f32);
    let mut tri = Triad::new(ix[0], ix[1], 2);
    for j in 2..n {
        tri.c = ix[j];
        if tri.circumcircle(points) && tri.r2 < romin2 {
            mid = j as i32;
            romin2 = tri.r2;
            cx = tri.cx;
            cy = tri.cy;
        } else if romin2 * 4.0 < d[j] {
            break;
        }
    }
    if mid != -1 && mid != 2 {
        let m = mid as usize;
        let (ii, dd) = (ix[m], d[m]);
        ix.copy_within(2..m, 3);
        d.copy_within(2..m, 3);
        ix[2] = ii;
        d[2] = dd;
    }
    tri.c = ix[2];
    tri.make_clockwise(points);
    tri.circumcircle(points);
    triads.push(tri);
    let hv = |i: i32| HullV { x: points[i as usize].x, y: points[i as usize].y, point: i, triad: 0 };
    hull.0.push(hv(tri.a));
    hull.0.push(hv(tri.b));
    hull.0.push(hv(tri.c));
    let centre = Pt { x: cx, y: cy };
    for k in 3..n {
        d[k] = points[ix[k] as usize].dist2(&centre);
    }
    sort_by_keys(&mut d[3..n], &mut ix[3..n]);
    for &pi in ix.iter().take(n).skip(3) {
        let pt = hv(pi);
        let dx = pt.x - hull.0[0].x;
        let dy = pt.y - hull.0[0].y;
        let mut numh = hull.0.len();
        let mut pidx: Vec<i32> = Vec::new();
        let mut tridx: Vec<i32> = Vec::new();
        let hidx: usize;
        if hull.visible_d(0, dx, dy) {
            if hull.visible_d(numh - 1, dx, dy) {
                hidx = 0;
                pidx.push(hull.0[numh - 1].point);
                tridx.push(hull.0[numh - 1].triad);
                let h = 0usize;
                while h + 1 < numh {
                    pidx.push(hull.0[h].point);
                    tridx.push(hull.0[h].triad);
                    if hull.visible_p(h, &pt) {
                        hull.0.remove(h);
                        numh -= 1;
                        continue;
                    }
                    hull.0.insert(0, pt);
                    numh += 1;
                    break;
                }
                let mut h = numh as i64 - 2;
                while h > 0 && hull.visible_p(h as usize, &pt) {
                    pidx.insert(0, hull.0[h as usize].point);
                    tridx.insert(0, hull.0[h as usize].triad);
                    hull.0.remove(h as usize + 1);
                    h -= 1;
                }
            } else {
                hidx = 1;
                tridx.push(hull.0[0].triad);
                pidx.push(hull.0[0].point);
                let h = 1usize;
                while h < numh {
                    pidx.push(hull.0[h].point);
                    tridx.push(hull.0[h].triad);
                    if hull.visible_p(h, &pt) {
                        hull.0.remove(h);
                        numh -= 1;
                        continue;
                    }
                    hull.0.insert(h, pt);
                    break;
                }
            }
        } else {
            let mut e1: i64 = -1;
            let mut e2 = numh;
            for h in 1..numh {
                if hull.visible_p(h, &pt) {
                    if e1 < 0 {
                        e1 = h as i64;
                    }
                } else if e1 > 0 {
                    e2 = h;
                    break;
                }
            }
            if e2 < numh {
                for e in e1..=e2 as i64 {
                    pidx.push(hull.0[e as usize].point);
                    tridx.push(hull.0[e as usize].triad);
                }
            } else {
                if e1 >= 0 {
                    for e in e1..e2 as i64 {
                        pidx.push(hull.0[e as usize].point);
                        tridx.push(hull.0[e as usize].triad);
                    }
                }
                pidx.push(hull.0[0].point);
            }
            if e1 < e2 as i64 - 1 {
                hull.0.drain((e1 + 1) as usize..e2);
            }
            hull.0.insert((e1 + 1) as usize, pt);
            hidx = (e1 + 1) as usize;
        }
        add_fan(points, hull, triads, pi, hidx, &pidx, &tridx, hull_only);
    }
    Ok(())
}

/// Adds the fan of new triads from point `pi` to the visible hull edges `pidx` (the tail of the
/// s-hull insertion loop) and fixes the hull's triad links.
#[allow(clippy::too_many_arguments)]
fn add_fan(points: &[Pt], hull: &mut Hull, triads: &mut Vec<Triad>, pi: i32, hidx: usize, pidx: &[i32], tridx: &[i32], hull_only: bool) {
    if hull_only {
        return;
    }
    let numt = triads.len() as i32;
    let mut num17 = numt;
    for i in 0..pidx.len().saturating_sub(1) {
        let mut t = Triad::new(pi, pidx[i], pidx[i + 1]);
        t.circumcircle(points);
        t.bc = tridx[i];
        if i > 0 {
            t.ab = num17 - 1;
        }
        t.ac = num17 + 1;
        let tx = &mut triads[tridx[i] as usize];
        if (t.b == tx.a && t.c == tx.b) | (t.b == tx.b && t.c == tx.a) {
            tx.ab = num17;
        } else if (t.b == tx.a && t.c == tx.c) | (t.b == tx.c && t.c == tx.a) {
            tx.ac = num17;
        } else if (t.b == tx.b && t.c == tx.c) | (t.b == tx.c && t.c == tx.b) {
            tx.bc = num17;
        }
        triads.push(t);
        num17 += 1;
    }
    triads[(num17 - 1) as usize].ac = -1;
    hull.0[hidx].triad = num17 - 1;
    if hidx > 0 {
        hull.0[hidx - 1].triad = numt;
    } else {
        let n = hull.0.len();
        hull.0[n - 1].triad = numt;
    }
}

/// `Triangulator.ConvexHull(points, rejectDuplicatePoints)`: the hull vertices.
pub fn convex_hull(points: &[Pt], reject_duplicates: bool) -> Result<Vec<Pt>, TooFewPoints> {
    let mut hull = Hull(Vec::new());
    let mut triads = Vec::new();
    analyse(points, &mut hull, &mut triads, reject_duplicates, true)?;
    Ok(hull.0.iter().map(|h| Pt { x: h.x, y: h.y }).collect())
}

/// `Triangulator.Triangulation(points, rejectDuplicatePoints)`.
pub fn triangulation(points: &[Pt], reject_duplicates: bool) -> Result<Vec<Triad>, TooFewPoints> {
    let mut triads = Vec::new();
    let mut hull = Hull(Vec::new());
    analyse(points, &mut hull, &mut triads, reject_duplicates, false)?;
    let count = triads.len();
    let mut a = vec![false; count];
    let mut b = vec![false; count];
    let fraction = 0.3f32;
    let mut num = flip_all(points, &mut triads, &mut a);
    let mut it = 1;
    while num > (fraction * count as f32) as usize && it < 1000 {
        num = if it & 1 == 1 { flip_marked(points, &mut triads, &a, &mut b) } else { flip_marked(points, &mut triads, &b, &mut a) };
        it += 1;
    }
    let mut s1 = std::collections::BTreeSet::new();
    let mut s2 = std::collections::BTreeSet::new();
    num = flip_marked_set(points, &mut triads, if it & 1 == 1 { &a } else { &b }, &mut s1);
    it = 1;
    while num > 0 && it < 2000 {
        num = if it & 1 == 1 { flip_set(points, &mut triads, &s1, &mut s2) } else { flip_set(points, &mut triads, &s2, &mut s1) };
        it += 1;
    }
    Ok(triads)
}

fn flip(points: &[Pt], triads: &mut [Triad], i: usize) -> Option<usize> {
    let t = triads[i];
    let ti = i as i32;
    if t.bc >= 0 {
        let f = t.bc as usize;
        let (opp, left, right) = triads[f].adjacency(t.b, ti);
        if t.inside(&points[opp as usize]) {
            let (ab, ac) = (t.ab, t.ac);
            if ab != left && ac != right {
                let (a, b, c) = (t.a, t.b, t.c);
                triads[i].initialize(a, b, opp, ab, left, f as i32, points);
                triads[f].initialize(a, c, opp, ac, right, ti, points);
                if left >= 0 {
                    triads[left as usize].change_adjacent(f as i32, ti);
                }
                if ac >= 0 {
                    triads[ac as usize].change_adjacent(ti, f as i32);
                }
                return Some(f);
            }
        }
    }
    let t = triads[i];
    if t.ab >= 0 {
        let f = t.ab as usize;
        let (opp, left, right) = triads[f].adjacency(t.a, ti);
        if t.inside(&points[opp as usize]) {
            let (ab, ac) = (t.ac, t.bc);
            if ab != left && ac != right {
                let (a, b, c) = (t.a, t.b, t.c);
                triads[i].initialize(c, a, opp, ab, left, f as i32, points);
                triads[f].initialize(c, b, opp, ac, right, ti, points);
                if left >= 0 {
                    triads[left as usize].change_adjacent(f as i32, ti);
                }
                if ac >= 0 {
                    triads[ac as usize].change_adjacent(ti, f as i32);
                }
                return Some(f);
            }
        }
    }
    let t = triads[i];
    if t.ac >= 0 {
        let f = t.ac as usize;
        let (opp, left, right) = triads[f].adjacency(t.a, ti);
        if t.inside(&points[opp as usize]) {
            let (ab, ac) = (t.ab, t.bc);
            if ab != left && ac != right {
                let (a, b, c) = (t.a, t.b, t.c);
                triads[i].initialize(b, a, opp, ab, left, f as i32, points);
                triads[f].initialize(b, c, opp, ac, right, ti, points);
                if left >= 0 {
                    triads[left as usize].change_adjacent(f as i32, ti);
                }
                if ac >= 0 {
                    triads[ac as usize].change_adjacent(ti, f as i32);
                }
                return Some(f);
            }
        }
    }
    None
}

fn flip_all(points: &[Pt], triads: &mut [Triad], flipped: &mut [bool]) -> usize {
    flipped.iter_mut().for_each(|v| *v = false);
    let mut n = 0;
    for i in 0..triads.len() {
        if let Some(f) = flip(points, triads, i) {
            n += 2;
            flipped[i] = true;
            flipped[f] = true;
        }
    }
    n
}

fn flip_marked(points: &[Pt], triads: &mut [Triad], test: &[bool], flipped: &mut [bool]) -> usize {
    flipped.iter_mut().for_each(|v| *v = false);
    let mut n = 0;
    for i in 0..triads.len() {
        if test[i]
            && let Some(f) = flip(points, triads, i)
        {
            n += 2;
            flipped[i] = true;
            flipped[f] = true;
        }
    }
    n
}

fn flip_marked_set(points: &[Pt], triads: &mut [Triad], test: &[bool], flipped: &mut std::collections::BTreeSet<usize>) -> usize {
    flipped.clear();
    let mut n = 0;
    for i in 0..triads.len() {
        if test[i]
            && let Some(f) = flip(points, triads, i)
        {
            n += 2;
            flipped.insert(i);
            flipped.insert(f);
        }
    }
    n
}

fn flip_set(points: &[Pt], triads: &mut [Triad], test: &std::collections::BTreeSet<usize>, flipped: &mut std::collections::BTreeSet<usize>) -> usize {
    flipped.clear();
    let mut n = 0;
    for &i in test {
        if let Some(f) = flip(points, triads, i) {
            n += 2;
            flipped.insert(i);
            flipped.insert(f);
        }
    }
    n
}
