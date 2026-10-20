//! ReUV (docs/re/object-lod-spec.md section 5): a shape whose UVs tile beyond 0..1 is re-cut
//! along the integer UV lines so that every piece fits one texture repeat, and each piece's UVs
//! are shifted into 0..1. The rendered texture is unchanged (each piece shows the same part of
//! the repeating texture it showed before); the triangle count grows by the cuts. Afterwards the
//! piece can be remapped into an atlas tile like an untiled shape.
//!
//! Algorithm (ours): every triangle is clipped against the lines `u = k` and `v = k` that cross
//! it (Sutherland-Hodgman against each line, both sides kept), each resulting convex polygon is
//! fan-triangulated and wrapped by the integer cell of its centroid. New vertices interpolate
//! positions, UVs, colours and the direction vectors linearly (directions renormalized).

use crate::mesh::MeshPart;
use crate::transform::normalize;

/// A vertex with every attribute, for interpolation.
#[derive(Clone, Copy)]
struct V {
    p: [f32; 3],
    uv: [f32; 2],
    n: [f32; 3],
    t: [f32; 3],
    b: [f32; 3],
    c: [f32; 4],
}

fn lerp3(a: [f32; 3], b: [f32; 3], s: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * s, a[1] + (b[1] - a[1]) * s, a[2] + (b[2] - a[2]) * s]
}

impl V {
    fn lerp(&self, o: &V, s: f32) -> V {
        V {
            p: lerp3(self.p, o.p, s),
            uv: [self.uv[0] + (o.uv[0] - self.uv[0]) * s, self.uv[1] + (o.uv[1] - self.uv[1]) * s],
            n: normalize(lerp3(self.n, o.n, s), self.n),
            t: normalize(lerp3(self.t, o.t, s), self.t),
            b: normalize(lerp3(self.b, o.b, s), self.b),
            c: [self.c[0] + (o.c[0] - self.c[0]) * s, self.c[1] + (o.c[1] - self.c[1]) * s, self.c[2] + (o.c[2] - self.c[2]) * s, self.c[3] + (o.c[3] - self.c[3]) * s],
        }
    }
}

/// Splits a convex polygon at `coord(uv) = k`, returning the parts on each side (empty when the
/// line misses the polygon).
fn split(poly: &[V], axis: usize, k: f32) -> (Vec<V>, Vec<V>) {
    let mut below = Vec::new();
    let mut above = Vec::new();
    let n = poly.len();
    for i in 0..n {
        let a = &poly[i];
        let b = &poly[(i + 1) % n];
        let (da, db) = (a.uv[axis] - k, b.uv[axis] - k);
        if da <= 0.0 {
            below.push(*a);
        }
        if da >= 0.0 {
            above.push(*a);
        }
        if (da < 0.0 && db > 0.0) || (da > 0.0 && db < 0.0) {
            let s = da / (da - db);
            let mut m = a.lerp(b, s);
            m.uv[axis] = k;
            below.push(m);
            above.push(m);
        }
    }
    (below, above)
}

/// Whether the UVs tile beyond one repeat (any UV outside `[-tol, 1 + tol]`).
pub fn needs_recut(uvs: &[[f32; 2]], tol: f32) -> bool {
    uvs.iter().any(|uv| uv[0] < -tol || uv[0] > 1.0 + tol || uv[1] < -tol || uv[1] > 1.0 + tol)
}

/// The re-cut part: every triangle within one integer UV cell, UVs wrapped into 0..1.
pub fn recut(part: &MeshPart) -> MeshPart {
    let vert = |i: usize| V {
        p: part.positions[i],
        uv: part.uvs[i],
        n: part.normals[i],
        t: part.tangents[i],
        b: part.bitangents[i],
        c: part.colors.as_ref().map(|c| c[i].map(f32::from)).unwrap_or([255.0; 4]),
    };
    let mut out: Vec<V> = Vec::new();
    let mut tris: Vec<[u16; 3]> = Vec::new();
    let push_polygon = |poly: &[V], out: &mut Vec<V>, tris: &mut Vec<[u16; 3]>| {
        if poly.len() < 3 {
            return;
        }
        let cu = poly.iter().map(|v| v.uv[0]).sum::<f32>() / poly.len() as f32;
        let cv = poly.iter().map(|v| v.uv[1]).sum::<f32>() / poly.len() as f32;
        let (ou, ov) = (cu.floor(), cv.floor());
        let base = out.len();
        for v in poly {
            let mut w = *v;
            w.uv = [(v.uv[0] - ou).clamp(0.0, 1.0), (v.uv[1] - ov).clamp(0.0, 1.0)];
            out.push(w);
        }
        for i in 1..poly.len() - 1 {
            tris.push([base as u16, (base + i) as u16, (base + i + 1) as u16]);
        }
    };
    for tri in &part.triangles {
        let poly = vec![vert(tri[0] as usize), vert(tri[1] as usize), vert(tri[2] as usize)];
        // Cuts at the integer u lines strictly inside the triangle's u range, then the v lines.
        let mut pieces = vec![poly];
        for axis in 0..2 {
            let mut next = Vec::new();
            for piece in pieces {
                let lo = piece.iter().map(|v| v.uv[axis]).fold(f32::INFINITY, f32::min);
                let hi = piece.iter().map(|v| v.uv[axis]).fold(f32::NEG_INFINITY, f32::max);
                let mut current = vec![piece];
                let (k0, k1) = ((lo.floor() as i64) + 1, hi.ceil() as i64 - 1);
                for k in k0..=k1 {
                    let kf = k as f32;
                    let mut after = Vec::new();
                    for c in current {
                        let (below, above) = split(&c, axis, kf);
                        if below.len() >= 3 {
                            after.push(below);
                        }
                        if above.len() >= 3 {
                            after.push(above);
                        }
                    }
                    current = after;
                }
                next.extend(current);
            }
            pieces = next;
        }
        for piece in &pieces {
            push_polygon(piece, &mut out, &mut tris);
        }
        if out.len() > u16::MAX as usize - 8 {
            break;
        }
    }
    MeshPart {
        name: part.name.clone(),
        index: part.index,
        textures: part.textures.clone(),
        shader: part.shader.clone(),
        positions: out.iter().map(|v| v.p).collect(),
        uvs: out.iter().map(|v| v.uv).collect(),
        normals: out.iter().map(|v| v.n).collect(),
        tangents: out.iter().map(|v| v.t).collect(),
        bitangents: out.iter().map(|v| v.b).collect(),
        colors: part.colors.as_ref().map(|_| out.iter().map(|v| v.c.map(|x| x.round().clamp(0.0, 255.0) as u8)).collect()),
        triangles: tris,
        source_normals: part.source_normals,
        source_tangents: part.source_tangents,
        legacy: part.legacy,
        alpha: part.alpha,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::ShaderSource;

    fn quad(uvs: [[f32; 2]; 4]) -> MeshPart {
        MeshPart {
            name: "S".into(),
            index: 0,
            textures: Default::default(),
            shader: ShaderSource::Lighting(Default::default()),
            positions: vec![[0.0, 0.0, 0.0], [100.0, 0.0, 0.0], [100.0, 100.0, 0.0], [0.0, 100.0, 0.0]],
            uvs: uvs.to_vec(),
            normals: vec![[0.0, 0.0, 1.0]; 4],
            tangents: vec![[1.0, 0.0, 0.0]; 4],
            bitangents: vec![[0.0, 1.0, 0.0]; 4],
            colors: Some(vec![[0, 0, 0, 255], [255, 255, 255, 255], [255, 255, 255, 255], [0, 0, 0, 255]]),
            triangles: vec![[0, 1, 2], [0, 2, 3]],
            source_normals: true,
            source_tangents: true,
            legacy: false,
            alpha: None,
        }
    }

    #[test]
    fn untiled_quad_is_unchanged() {
        let p = quad([[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]]);
        assert!(!needs_recut(&p.uvs, 0.2));
        let r = recut(&p);
        assert_eq!(r.triangles.len(), 2);
        assert!(r.uvs.iter().all(|uv| (0.0..=1.0).contains(&uv[0]) && (0.0..=1.0).contains(&uv[1])));
    }

    #[test]
    fn tiled_quad_is_cut_and_wrapped() {
        // A quad tiled 2 x 2: four pieces of two triangles each, UVs inside 0..1, same area.
        let p = quad([[0.0, 0.0], [2.0, 0.0], [2.0, 2.0], [0.0, 2.0]]);
        assert!(needs_recut(&p.uvs, 0.2));
        let r = recut(&p);
        assert!(r.triangles.len() >= 8);
        assert!(r.uvs.iter().all(|uv| (0.0..=1.0).contains(&uv[0]) && (0.0..=1.0).contains(&uv[1])));
        let area = |t: &[u16; 3], pos: &[[f32; 3]]| {
            let (a, b, c) = (pos[t[0] as usize], pos[t[1] as usize], pos[t[2] as usize]);
            ((b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1])).abs() / 2.0
        };
        let total: f32 = r.triangles.iter().map(|t| area(t, &r.positions)).sum();
        assert!((total - 10_000.0).abs() < 0.5, "{total}");
        // The cut at x = 50 interpolated the colour.
        assert!(r.colors.as_ref().unwrap().iter().any(|c| c[0] > 100 && c[0] < 160));
        // Interpolated positions sit on the cut lines.
        assert!(r.positions.iter().any(|p| (p[0] - 50.0).abs() < 1e-3));
    }

    #[test]
    fn offset_tile_wraps() {
        let p = quad([[3.0, 1.0], [4.0, 1.0], [4.0, 2.0], [3.0, 2.0]]);
        let r = recut(&p);
        assert_eq!(r.triangles.len(), 2);
        assert!(r.uvs.iter().all(|uv| (0.0..=1.0).contains(&uv[0]) && (0.0..=1.0).contains(&uv[1])));
        assert_eq!(r.uvs[1], [1.0, 0.0]);
    }
}
