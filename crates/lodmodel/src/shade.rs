// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! Shading check: what the lit surface of a result looks like against the source's, in the
//! silhouette frames ([`Frames`]).
//!
//! The silhouette check sees only the outline. Inside it a collapse can still change what the
//! model looks like: a roof ridge moved sideways turns half a roof slope toward the other side, a
//! porch folded into the wall behind it loses its lit side faces, a round tower that becomes a
//! prism keeps its outline within tolerance but shows three flat bands. Diffuse lighting is linear
//! in the surface normal, so the difference of two renders under any light is bounded by the
//! difference of the visible normals; that is what is compared here, without choosing a light.
//!
//! Per view the nearest surface from the front and from the back is rasterized with its
//! interpolated vertex normal (the one the game shades with), and the normals are summed per
//! `BLOCK` x `BLOCK` texel block, about a screen pixel at the level's distance: detail below a
//! block (relief on a wall, the planks of a deck) averages out on screen and so here, in the
//! source as in the result. Only blocks the source covers fully count: a block on the outline,
//! or one a rope or a rail thinner than a block passes through, holds whatever part of a tube
//! happens to face the view, and its coverage is the silhouette check's business. The error of a
//! view side is the summed length of the per-block differences over the texels of those blocks:
//! 0 for the same picture, 1 where everything is gone, 2 for a surface turned around.

use super::{BLOCK, Frame, Frames, cross3, dot3};

/// One view: per block, the summed unit normals of the texels' nearest surface from the front
/// (along `right` x `up`) and from the back, and the covered texels.
pub(crate) struct ShadedView {
    front: Vec<[f32; 3]>,
    back: Vec<[f32; 3]>,
    /// Per block, the covered texels.
    count: Vec<u16>,
}

impl Frames {
    /// One [`ShadedView`] per view. `normal(v)` is the normal of vertex `v`.
    pub(crate) fn shaded(&self, positions: &[[f32; 3]], normal: &dyn Fn(u32) -> [f32; 3], indices: &[u32]) -> Vec<ShadedView> {
        self.views.iter().map(|f| f.shaded(positions, normal, indices)).collect()
    }
}

struct Layers {
    w: usize,
    h: usize,
    near: Vec<f32>,
    far: Vec<f32>,
    near_n: Vec<[f32; 3]>,
    far_n: Vec<[f32; 3]>,
}

impl Layers {
    fn put(&mut self, x: f32, y: f32, depth: f32, n: [f32; 3]) {
        if !(x >= 0.0 && y >= 0.0 && (x as usize) < self.w && (y as usize) < self.h) {
            return;
        }
        let i = y as usize * self.w + x as usize;
        if depth < self.near[i] {
            self.near[i] = depth;
            self.near_n[i] = n;
        }
        if depth > self.far[i] {
            self.far[i] = depth;
            self.far_n[i] = n;
        }
    }
}

fn unit(v: [f32; 3]) -> [f32; 3] {
    let l = dot3(v, v).sqrt();
    if l > 1e-12 { [v[0] / l, v[1] / l, v[2] / l] } else { [0.0; 3] }
}

impl Frame {
    /// Same coverage rule as [`Frame::mask`] (interior at texel centres plus the edges as lines),
    /// with depth along the view direction deciding which surface a texel shows.
    fn shaded(&self, positions: &[[f32; 3]], normal: &dyn Fn(u32) -> [f32; 3], indices: &[u32]) -> ShadedView {
        let toward = cross3(self.right, self.up);
        let n = self.w * self.h;
        let mut l = Layers { w: self.w, h: self.h, near: vec![f32::MAX; n], far: vec![f32::MIN; n], near_n: vec![[0.0; 3]; n], far_n: vec![[0.0; 3]; n] };
        let proj = |p: [f32; 3]| [(dot3(p, self.right) - self.x0) / self.texel, (dot3(p, self.up) - self.z0) / self.texel, dot3(p, toward)];
        let mix = |a: [f32; 3], b: [f32; 3], c: [f32; 3], wa: f32, wb: f32, wc: f32| [a[0] * wa + b[0] * wb + c[0] * wc, a[1] * wa + b[1] * wb + c[1] * wc, a[2] * wa + b[2] * wb + c[2] * wc];
        for t in indices.chunks_exact(3) {
            let p = [proj(positions[t[0] as usize]), proj(positions[t[1] as usize]), proj(positions[t[2] as usize])];
            let nn = [normal(t[0]), normal(t[1]), normal(t[2])];
            for (i, j) in [(0, 1), (1, 2), (2, 0)] {
                let (a, b) = (p[i], p[j]);
                let steps = ((b[0] - a[0]).abs().max((b[1] - a[1]).abs()).ceil() as usize).clamp(1, 4096);
                for s in 0..=steps {
                    let f = s as f32 / steps as f32;
                    let q = mix(a, b, a, 1.0 - f, f, 0.0);
                    l.put(q[0], q[1], q[2], unit(mix(nn[i], nn[j], nn[i], 1.0 - f, f, 0.0)));
                }
            }
            let [a, b, c] = p;
            let area = (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
            let i0 = a[0].min(b[0]).min(c[0]).floor().max(0.0) as usize;
            let i1 = a[0].max(b[0]).max(c[0]).ceil().min(self.w as f32) as usize;
            let j0 = a[1].min(b[1]).min(c[1]).floor().max(0.0) as usize;
            let j1 = a[1].max(b[1]).max(c[1]).ceil().min(self.h as f32) as usize;
            if i0 >= i1 || j0 >= j1 || area.abs() < 1e-12 {
                continue;
            }
            let inv = 1.0 / area;
            for j in j0..j1 {
                let py = j as f32 + 0.5;
                for i in i0..i1 {
                    let px = i as f32 + 0.5;
                    // Barycentric weights of c, a, b (signed areas over the triangle's).
                    let wc = ((b[0] - a[0]) * (py - a[1]) - (b[1] - a[1]) * (px - a[0])) * inv;
                    let wa = ((c[0] - b[0]) * (py - b[1]) - (c[1] - b[1]) * (px - b[0])) * inv;
                    let wb = ((a[0] - c[0]) * (py - c[1]) - (a[1] - c[1]) * (px - c[0])) * inv;
                    if wa >= 0.0 && wb >= 0.0 && wc >= 0.0 {
                        l.put(px, py, a[2] * wa + b[2] * wb + c[2] * wc, unit(mix(nn[0], nn[1], nn[2], wa, wb, wc)));
                    }
                }
            }
        }
        let mut out = ShadedView { front: vec![[0.0; 3]; self.bw * self.bh], back: vec![[0.0; 3]; self.bw * self.bh], count: vec![0; self.bw * self.bh] };
        for j in 0..self.h {
            for i in 0..self.w {
                let k = j * self.w + i;
                if l.near[k] == f32::MAX {
                    continue;
                }

                let b = (j / BLOCK) * self.bw + i / BLOCK;
                out.count[b] += 1;
                for c in 0..3 {
                    out.front[b][c] += l.near_n[k][c];
                    out.back[b][c] += l.far_n[k][c];
                }
            }
        }
        out
    }
}

/// Shading of a result against the source's: over the views and their two sides, the largest
/// summed per-block normal difference over the blocks the source covers fully, as a share of
/// their texels (0 for a source without one).
pub(crate) fn shading_error(source: &[ShadedView], result: &[ShadedView]) -> f32 {
    let mut worst = 0.0f32;
    for (s, r) in source.iter().zip(result) {
        let full = (BLOCK * BLOCK) as u16;
        let texels = s.count.iter().filter(|&&c| c == full).count() * BLOCK * BLOCK;
        if texels == 0 {
            continue;
        }
        for (a, b) in [(&s.front, &r.front), (&s.back, &r.back)] {
            let sum: f32 = a.iter().zip(b).zip(&s.count).filter(|(_, c)| **c == full).map(|((x, y), _)| ((x[0] - y[0]).powi(2) + (x[1] - y[1]).powi(2) + (x[2] - y[2]).powi(2)).sqrt()).sum();
            worst = worst.max(sum / texels as f32);
        }
    }
    worst
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quad(z: f32) -> (Vec<[f32; 3]>, Vec<u32>) {
        (vec![[0.0, 0.0, z], [100.0, 0.0, z], [100.0, 100.0, z], [0.0, 100.0, z]], vec![0, 1, 2, 0, 2, 3])
    }

    /// The same surface scores 0, a missing one 1, one lit from the opposite side 2, and relief
    /// smaller than a block (normals tilted left and right in alternate texel columns) scores as
    /// the flat surface it averages to.
    #[test]
    fn shading_error_scale() {
        let (pos, idx) = quad(0.0);
        let frames = Frames::new(&pos, &idx, 2.0);
        let up = |_: u32| [0.0, 0.0, 1.0];
        let down = |_: u32| [0.0, 0.0, -1.0];
        let source = frames.shaded(&pos, &up, &idx);
        assert_eq!(shading_error(&source, &frames.shaded(&pos, &up, &idx)), 0.0);
        let gone = shading_error(&source, &frames.shaded(&pos, &up, &[]));
        assert!((gone - 1.0).abs() < 1e-4, "{gone}");
        let turned = shading_error(&source, &frames.shaded(&pos, &down, &idx));
        assert!((turned - 2.0).abs() < 1e-4, "{turned}");
        // Moved by less than a texel along the view: the same picture.
        let (near, _) = quad(0.5);
        assert!(shading_error(&source, &frames.shaded(&near, &up, &idx)) < 1e-4);
    }
}
