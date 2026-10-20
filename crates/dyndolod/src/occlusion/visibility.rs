//! Which LOD blocks can be seen from a cell: a line-of-sight model over the height map, packed
//! as a TVDT field.
//!
//! The model (docs/re/occlusion-spec.md, sections 1 and 10.5, fitted to the golden output): a
//! cell has `q x q` sample points (quality 2 = 2 x 2, quality 3 = 4 x 4), each at the highest
//! thing (terrain, water surface or object LOD) in its part of the cell. A block is visible when
//! some sample point of the observer cell, raised by `OcclusionHeight`, has a straight line to
//! some sample point of some cell of the block that is not blocked by the landscape between
//! them; the landscape (ground with its water surface, objects do not block) blocks when it
//! rises more than the clearance above the line. Cells farther than the radius (Euclidean, in
//! cells) are never looked at; the observer's own block is always visible; every node above the
//! blocks is the OR of its children. Beyond the height map the height is 0 with no clearance.
//!
//! Copyright (c) 2026 CageTV (AnvilLOD, `src/AnvilLOD.Core/Occlusion/OcclusionCalculator.cs`,
//! commit 606eece, GPL-3.0-only). Modified for LODForge: ported to Rust; sample points sit on
//! the highest sample of their sub-area instead of its centre; the clearance is refitted
//! (1,250 units) against DynDOLOD's output with the void heights of the spec; targets are
//! filtered per cell by the radius; the levels above the blocks are the plain OR of their
//! children (the golden output has no other bits set); sample points and block orders are
//! precomputed once per map; diagnostics for the fit added.
//! This file is part of LODForge and is distributed under the GNU General Public License,
//! version 3 only.

use super::grid::{HeightMap, SAMPLES};
use super::tvdt::{Layout, Tvdt};

/// What blocks a line of sight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Occluder {
    /// The landscape and its water surface (the fitted model: object LOD does not block).
    Ground,
    /// The highest thing in a sample, object LOD included.
    Top,
}

/// Clearance (game units) the landscape may rise above a line of sight without blocking it,
/// fitted to the golden output (300 Tamriel cells: block Jaccard 0.867 at 750, 0.873 at 1,000,
/// 0.870 at 1,250, 0.862 at 1,500; at 1,250 we hide 5.0% of the blocks the reference shows and
/// show 9.1% it hides).
pub const CLEARANCE: f32 = 1250.0;

/// The visibility model's settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Model {
    /// `OcclusionQuality`: 1, 2 or 3 (2^(q-1) sample points per cell side).
    pub quality: i32,
    /// `OcclusionRadius` in cells.
    pub radius: i32,
    /// `OcclusionHeight`: added to the observer's height.
    pub eye_add: f32,
    /// Height the landscape must rise above the line of sight to block it.
    pub clearance: f32,
    pub occluder: Occluder,
}

impl Model {
    pub fn new(quality: i32, radius: i32, eye_add: f32) -> Model {
        Model { quality, radius, eye_add, clearance: CLEARANCE, occluder: Occluder::Ground }
    }

    /// Sample points per cell side.
    pub fn samples_per_side(&self) -> usize {
        1usize << (self.quality.clamp(1, 6) - 1)
    }
}

/// A sample point: the highest sample of a sub-area, in map sample coordinates.
#[derive(Debug, Clone, Copy)]
struct Sample {
    x: i32,
    y: i32,
    h: f32,
}

pub struct Calculator<'a> {
    map: &'a HeightMap,
    origin: (i32, i32),
    stride: i32,
    layout: Layout,
    model: Model,
    /// Sample points per cell side and per cell.
    n: usize,
    nn: usize,
    /// The sample points of every grid cell, `nn` per cell, cell `(gy * stride + gx)`.
    samples: Vec<Sample>,
    /// Per leaf block (`by * nb + bx`): its cells' sample points as `ci * nn + si` (`ci` =
    /// `cj * 4 + ci_x` within the block), highest first.
    block_order: Vec<Vec<u16>>,
}

impl<'a> Calculator<'a> {
    pub fn new(map: &'a HeightMap, origin: (i32, i32), stride: i32, model: Model) -> Option<Calculator<'a>> {
        let layout = Layout::for_stride(stride)?;
        let n = model.samples_per_side().min(SAMPLES);
        let nn = n * n;
        let sub = (SAMPLES / n) as i32;
        let s = stride as usize;
        let mut samples = Vec::with_capacity(s * s * nn);
        for gy in 0..stride {
            for gx in 0..stride {
                let (cx, cy) = (origin.0 + gx, origin.1 + gy);
                let (sx0, sy0) = map.cell_sample(cx, cy);
                let inside = map.contains_cell(cx, cy);
                for j in 0..n as i32 {
                    for i in 0..n as i32 {
                        let (ax, ay) = (sx0 + i * sub, sy0 + j * sub);
                        let mut best = Sample { x: ax + sub / 2, y: ay + sub / 2, h: map.outside };
                        if inside {
                            best.h = f32::NEG_INFINITY;
                            for y in ay..ay + sub {
                                for x in ax..ax + sub {
                                    let h = map.top_at(x, y);
                                    if h > best.h {
                                        best = Sample { x, y, h };
                                    }
                                }
                            }
                        }
                        samples.push(best);
                    }
                }
            }
        }
        let nb = layout.nb;
        let mut block_order = Vec::with_capacity(nb * nb);
        for by in 0..nb {
            for bx in 0..nb {
                let mut v: Vec<(f32, u16)> = Vec::with_capacity(16 * nn);
                for cj in 0..4 {
                    for ci in 0..4 {
                        let (gx, gy) = (bx * 4 + ci, by * 4 + cj);
                        let base = (gy * s + gx) * nn;
                        for si in 0..nn {
                            v.push((samples[base + si].h, ((cj * 4 + ci) * nn + si) as u16));
                        }
                    }
                }
                v.sort_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
                block_order.push(v.into_iter().map(|(_, k)| k).collect());
            }
        }
        Some(Calculator { map, origin, stride, layout, model, n, nn, samples, block_order })
    }

    pub fn layout(&self) -> Layout {
        self.layout
    }

    pub fn model(&self) -> &Model {
        &self.model
    }

    /// Sample points per cell side.
    pub fn samples_per_side(&self) -> usize {
        self.n
    }

    #[inline]
    fn occluder_at(&self, x: i32, y: i32) -> f32 {
        match self.model.occluder {
            Occluder::Ground => self.map.ground_at(x, y),
            Occluder::Top => self.map.top_at(x, y),
        }
    }

    #[inline]
    fn target(&self, tx: i32, ty: i32, si: usize) -> Sample {
        self.samples[(ty as usize * self.stride as usize + tx as usize) * self.nn + si]
    }

    /// The observer cell's sample points raised by `OcclusionHeight`, highest first.
    fn eyes(&self, gx: i32, gy: i32) -> Vec<Sample> {
        let base = (gy as usize * self.stride as usize + gx as usize) * self.nn;
        let mut eyes: Vec<Sample> = self.samples[base..base + self.nn].iter().map(|p| Sample { h: p.h + self.model.eye_add, ..*p }).collect();
        eyes.sort_by(|a, b| b.h.total_cmp(&a.h));
        eyes
    }

    /// Whether the block's nearest cell is within the radius of the observer cell (grid
    /// coordinates).
    #[inline]
    fn block_in_radius(&self, gx: i32, gy: i32, bx: usize, by: usize) -> bool {
        let (x0, y0) = (bx as i32 * 4, by as i32 * 4);
        let ddx = i64::from((x0 - gx).max(gx - (x0 + 3)).max(0));
        let ddy = i64::from((y0 - gy).max(gy - (y0 + 3)).max(0));
        let r = i64::from(self.model.radius);
        ddx * ddx + ddy * ddy <= r * r
    }

    #[inline]
    fn cell_in_radius(&self, gx: i32, gy: i32, tx: i32, ty: i32) -> bool {
        let (ex, ey) = (i64::from(tx - gx), i64::from(ty - gy));
        let r = i64::from(self.model.radius);
        ex * ex + ey * ey <= r * r
    }

    /// The highest rise of the landscape above the straight line from `e` to `t` (negative when
    /// the line is clear); beyond the map a rise counts as infinite (no clearance applies).
    /// Stops early at `limit` when `stop_early`.
    fn obstruction(&self, e: &Sample, t: &Sample, limit: f32, stop_early: bool) -> f32 {
        let (dx, dy) = (t.x - e.x, t.y - e.y);
        let steps = dx.abs().max(dy.abs());
        let mut worst = f32::NEG_INFINITY;
        if steps <= 1 {
            return worst;
        }
        let inv = 1.0 / steps as f32;
        let (fx, fy) = (dx as f32 * inv, dy as f32 * inv);
        let dh = (t.h - e.h) * inv;
        let (ex, ey) = (e.x as f32, e.y as f32);
        for s in 1..steps {
            let sf = s as f32;
            let px = (ex + fx * sf).round() as i32;
            let py = (ey + fy * sf).round() as i32;
            let rise = self.occluder_at(px, py) - (e.h + dh * sf);
            let rise = if rise > 0.0 && !self.map.contains_sample(px, py) { f32::INFINITY } else { rise };
            if rise > worst {
                worst = rise;
                if stop_early && worst > limit {
                    return worst;
                }
            }
        }
        worst
    }

    #[inline]
    fn sees(&self, e: &Sample, t: &Sample) -> bool {
        self.obstruction(e, t, self.model.clearance, true) <= self.model.clearance
    }

    /// The TVDT of a cell inside the grid.
    pub fn compute(&self, cx: i32, cy: i32) -> Option<Tvdt> {
        let (gx, gy) = (cx - self.origin.0, cy - self.origin.1);
        if gx < 0 || gy < 0 || gx >= self.stride || gy >= self.stride {
            return None;
        }
        let nb = self.layout.nb;
        let own = ((gx / 4) as usize, (gy / 4) as usize);
        let eyes = self.eyes(gx, gy);
        let mut t = Tvdt::empty(self.layout);
        for by in 0..nb {
            for bx in 0..nb {
                if !self.block_in_radius(gx, gy, bx, by) {
                    continue;
                }
                if (bx, by) == own {
                    t.set_leaf(bx, by, true);
                    continue;
                }
                let (x0, y0) = (bx as i32 * 4, by as i32 * 4);
                let mut visible = false;
                'targets: for &k in &self.block_order[by * nb + bx] {
                    let (ci, si) = (usize::from(k) / self.nn, usize::from(k) % self.nn);
                    let (tx, ty) = (x0 + (ci % 4) as i32, y0 + (ci / 4) as i32);
                    if !self.cell_in_radius(gx, gy, tx, ty) {
                        continue;
                    }
                    let target = self.target(tx, ty, si);
                    for e in &eyes {
                        if self.sees(e, &target) {
                            visible = true;
                            break 'targets;
                        }
                    }
                }
                t.set_leaf(bx, by, visible);
            }
        }
        t.fill_parents();
        Some(t)
    }

    /// Diagnostic: the clearance a block would need to be visible from a cell, i.e. the least
    /// obstruction over all eye/target pairs (negative when the line is clear); `None` for a
    /// block out of the radius or the observer's own.
    pub fn needed_clearance(&self, cx: i32, cy: i32, bx: usize, by: usize) -> Option<f32> {
        let (gx, gy) = (cx - self.origin.0, cy - self.origin.1);
        if !self.block_in_radius(gx, gy, bx, by) || (bx, by) == ((gx / 4) as usize, (gy / 4) as usize) {
            return None;
        }
        let (x0, y0) = (bx as i32 * 4, by as i32 * 4);
        let eyes = self.eyes(gx, gy);
        let mut best = f32::INFINITY;
        for &k in &self.block_order[by * self.layout.nb + bx] {
            let (ci, si) = (usize::from(k) / self.nn, usize::from(k) % self.nn);
            let (tx, ty) = (x0 + (ci % 4) as i32, y0 + (ci / 4) as i32);
            if !self.cell_in_radius(gx, gy, tx, ty) {
                continue;
            }
            let target = self.target(tx, ty, si);
            for e in &eyes {
                best = best.min(self.obstruction(e, &target, f32::INFINITY, false));
            }
        }
        Some(best)
    }

    /// Diagnostic: the best line of sight from a cell to a block in words: eye and target
    /// (cell, height), the least obstruction and the sample that causes it.
    pub fn best_line(&self, cx: i32, cy: i32, bx: usize, by: usize) -> Option<String> {
        let (gx, gy) = (cx - self.origin.0, cy - self.origin.1);
        let (x0, y0) = (bx as i32 * 4, by as i32 * 4);
        let eyes = self.eyes(gx, gy);
        let mut best: Option<(f32, Sample, Sample, (i32, i32), f32)> = None;
        for &k in &self.block_order[by * self.layout.nb + bx] {
            let (ci, si) = (usize::from(k) / self.nn, usize::from(k) % self.nn);
            let (tx, ty) = (x0 + (ci % 4) as i32, y0 + (ci / 4) as i32);
            let target = self.target(tx, ty, si);
            for e in &eyes {
                let (dx, dy) = (target.x - e.x, target.y - e.y);
                let steps = dx.abs().max(dy.abs());
                let (mut worst, mut at, mut wh) = (f32::NEG_INFINITY, (0, 0), 0.0);
                if steps > 1 {
                    let inv = 1.0 / steps as f32;
                    let (fx, fy) = (dx as f32 * inv, dy as f32 * inv);
                    let dh = (target.h - e.h) * inv;
                    for st in 1..steps {
                        let sf = st as f32;
                        let (px, py) = ((e.x as f32 + fx * sf).round() as i32, (e.y as f32 + fy * sf).round() as i32);
                        let h = self.occluder_at(px, py);
                        let rise = h - (e.h + dh * sf);
                        if rise > worst {
                            worst = rise;
                            at = (px, py);
                            wh = h;
                        }
                    }
                }
                if best.as_ref().is_none_or(|b| worst < b.0) {
                    best = Some((worst, *e, target, at, wh));
                }
            }
        }
        let (worst, e, t, at, wh) = best?;
        let cell = |p: (i32, i32)| (self.map.x0 + p.0.div_euclid(SAMPLES as i32), self.map.y0 + p.1.div_euclid(SAMPLES as i32));
        Some(format!("eye {:?} h {:.0} -> target {:?} h {:.0}, {} samples; worst rise {:.0} at {:?} (h {:.0}, ground {:.0})", cell((e.x, e.y)), e.h, cell((t.x, t.y)), t.h, (t.x - e.x).abs().max((t.y - e.y).abs()), worst, cell(at), wh, self.map.ground_at(at.0, at.1)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::occlusion::grid::POINTS;

    fn flat(h: f32) -> Vec<f32> {
        vec![h; POINTS * POINTS]
    }

    fn plain(model: Model) -> Model {
        Model { clearance: 0.0, ..model }
    }

    /// A 16-cell grid (stride 16, origin 0): a ridge of height 4000 across the cells x = 6 hides
    /// what lies east of it from an observer at x = 2, except what stands above the ridge line.
    #[test]
    fn a_ridge_hides_the_blocks_behind_it() {
        let mut map = HeightMap::new(0, 0, 16, 16, -14000.0);
        for y in 0..16 {
            for x in 0..16 {
                map.set_land(x, y, &flat(if x == 6 { 4000.0 } else { 0.0 }), f32::NEG_INFINITY);
            }
        }
        let calc = Calculator::new(&map, (0, 0), 16, plain(Model::new(2, 100, 0.0))).unwrap();
        let t = calc.compute(2, 5).unwrap();
        assert!(t.leaf(0, 1), "own block");
        assert!(t.leaf(1, 1), "the ridge block itself is visible");
        assert!(!t.leaf(2, 1) && !t.leaf(3, 1), "blocks behind the ridge are hidden");
        assert!(t.leaf(0, 0) && t.leaf(0, 3), "blocks on this side are visible");
        assert!(t.is_consistent());
        // A tower in block (3, 1) (cell 13, 5) that stands high above the ridge becomes visible.
        let mut tall = flat(-20000.0);
        tall[16 * POINTS + 16] = 20000.0;
        map.raise_top(13, 5, &tall);
        let calc = Calculator::new(&map, (0, 0), 16, plain(Model::new(2, 100, 0.0))).unwrap();
        let t = calc.compute(2, 5).unwrap();
        assert!(t.leaf(3, 1) && !t.leaf(2, 1));
        // An object wall over cell (4, 5), higher than the tower, does not block (objects are
        // seen, not seen through); with objects as occluders it hides the tower again.
        map.raise_top(4, 5, &flat(30000.0));
        assert!(Calculator::new(&map, (0, 0), 16, plain(Model::new(2, 100, 0.0))).unwrap().compute(2, 5).unwrap().leaf(3, 1));
        let mut m = plain(Model::new(2, 100, 0.0));
        m.occluder = Occluder::Top;
        assert!(!Calculator::new(&map, (0, 0), 16, m).unwrap().compute(2, 5).unwrap().leaf(3, 1));
        // The clearance lets the line pass a ridge that rises less than it.
        let mut m = plain(Model::new(2, 100, 0.0));
        m.clearance = 4100.0;
        assert!(Calculator::new(&map, (0, 0), 16, m).unwrap().compute(2, 5).unwrap().leaf(2, 1));
    }

    #[test]
    fn the_radius_limits_the_targets_and_the_observer_height_raises_the_eye() {
        let mut map = HeightMap::new(0, 0, 16, 16, -14000.0);
        for y in 0..16 {
            for x in 0..16 {
                map.set_land(x, y, &flat(0.0), f32::NEG_INFINITY);
            }
        }
        // Radius 6 from cell (0, 0): block (1, 1) has its nearest cell (4, 4) at 5.66 cells, block
        // (2, 0) its nearest cell 8 cells away.
        let t = Calculator::new(&map, (0, 0), 16, plain(Model::new(2, 6, 0.0))).unwrap().compute(0, 0).unwrap();
        assert!(t.leaf(0, 0) && t.leaf(1, 0) && t.leaf(0, 1) && t.leaf(1, 1));
        assert!(!t.leaf(2, 0));
        assert_eq!(t.visible_leaves(), 4);
        // Looking from cell (0, 0) to block (3, 0) (cells 12..15, rows 0..3) over a wall of 300
        // across the cells (7, 0..3): an observer raised by 1000 sees the far end of the block
        // over it, one at 0 does not.
        for y in 0..4 {
            map.set_land(7, y, &flat(300.0), f32::NEG_INFINITY);
        }
        let see = |eye_add: f32| Calculator::new(&map, (0, 0), 16, plain(Model::new(2, 100, eye_add))).unwrap().compute(0, 0).unwrap().leaf(3, 0);
        assert!(!see(0.0));
        assert!(see(1000.0));
    }

    /// Beyond the map the height is 0 and no clearance applies: an observer below 0 never sees
    /// the blocks out there, one above 0 does (spec section 10.5).
    #[test]
    fn beyond_the_map_only_observers_above_zero_see() {
        // Map of 8 x 8 cells inside a 16-cell grid: everything at x >= 8 is outside.
        let mut map = HeightMap::new(0, 0, 8, 16, -14000.0);
        for y in 0..16 {
            for x in 0..8 {
                map.set_land(x, y, &flat(-14000.0), -14000.0);
            }
        }
        let calc = Calculator::new(&map, (0, 0), 16, Model::new(2, 100, 0.0)).unwrap();
        let t = calc.compute(1, 8).unwrap();
        assert!(t.leaf(1, 2), "the sea inside the map is visible");
        assert!(!t.leaf(2, 2) && !t.leaf(3, 2), "the void beyond the map is not, from sea level");
        let mut map2 = map.clone();
        map2.set_land(1, 8, &flat(500.0), f32::NEG_INFINITY);
        let calc = Calculator::new(&map2, (0, 0), 16, Model::new(2, 100, 0.0)).unwrap();
        let t = calc.compute(1, 8).unwrap();
        assert!(t.leaf(2, 2) && t.leaf(3, 2), "from above 0 it is");
    }
}
