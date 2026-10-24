//! Repeat tiles for generated LOD models: a shape whose UVs repeat its texture is drawn from an
//! atlas tile that holds the texture repeated `ru x rv` times, with its UVs shifted by whole
//! repeats and scaled by `1 / ru, 1 / rv` (an atlas map row with that scale), instead of being
//! re-cut along every repeat line ([`crate::reuv`]) or keeping a texture of its own.
//!
//! Generated models (`meshes\lod\lodforge\...`, made from full models) mostly use repeating
//! architecture textures; their simplified triangles span several repeats, so cutting at every
//! repeat line multiplied their triangles (x2.5 at LOD4, x3.9 at LOD16 on skyrim-cs-base) and
//! keeping their own textures made each a draw call of its own in every block. Shapes spanning
//! more than [`MAX_REPEATS`] repeats are cut every `MAX_REPEATS` repeats. LOD models installed
//! with the load order keep DynDOLOD's handling (atlas or own texture).
//!
//! Keys: the atlas key of the shape's textures with `*<ru>x<rv>` appended
//! (`textures\a.dds,textures\a_n.dds=8*3x2`); [`crate::atlas_map::AtlasMap`] reads the scale from
//! that suffix.

use crate::mesh::MeshPart;

/// The most repeats a tile holds per axis.
pub const MAX_REPEATS: u32 = 8;
/// How much a shape may grow when it still has to be cut (spans beyond [`MAX_REPEATS`]):
/// `MAX_GROWTH` times its triangles plus [`GROWTH_SLACK`]; beyond that it keeps its own texture.
pub const MAX_GROWTH: usize = 4;
pub const GROWTH_SLACK: usize = 64;

/// LOD models LODForge generated (`lod-models`, far models): the only ones that get repeat tiles.
pub fn is_generated_model(model: &str) -> bool {
    model.to_ascii_lowercase().replace('/', "\\").starts_with("meshes\\lod\\lodforge\\")
}

/// `key*<ru>x<rv>`.
pub fn tiled_key(key: &str, (ru, rv): (u32, u32)) -> String {
    format!("{key}*{ru}x{rv}")
}

/// A key's base and repeats (`None` for an ordinary key).
pub fn split_key(key: &str) -> (&str, Option<(u32, u32)>) {
    if let Some((base, r)) = key.rsplit_once('*')
        && let Some((u, v)) = r.split_once('x')
        && let (Ok(u), Ok(v)) = (u.parse::<u32>(), v.parse::<u32>())
        && u >= 1
        && v >= 1
    {
        return (base, Some((u, v)));
    }
    (key, None)
}

/// Whole repeats the UVs span per axis after shifting their minimum into 0..1, and that shift.
pub fn span(uvs: &[[f32; 2]]) -> ([f32; 2], [u32; 2]) {
    let mut shift = [0.0f32; 2];
    let mut reps = [1u32; 2];
    for a in 0..2 {
        let (lo, hi) = uvs.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p[a]), hi.max(p[a])));
        if lo > hi {
            continue;
        }
        shift[a] = -lo.floor();
        // A small tolerance: 1.0004 is one repeat, not two.
        reps[a] = ((hi + shift[a]) - 1e-3).ceil().max(1.0) as u32;
    }
    (shift, reps)
}

/// The repeats of the tile a shape with these UVs is drawn from (capped at [`MAX_REPEATS`]),
/// `None` when the UVs fit one repeat (the ordinary tile does).
pub fn tile_repeats(uvs: &[[f32; 2]]) -> Option<(u32, u32)> {
    let (shift, [ru, rv]) = span(uvs);
    if ru == 1 && rv == 1 && shift == [0.0; 2] {
        return None;
    }
    if ru == 1 && rv == 1 {
        // Inside one repeat once shifted: the ordinary tile with the shift applied would do, but
        // the ordinary row is matched on the unshifted UVs; a 1 x 1 repeat tile keeps it simple.
        return Some((1, 1));
    }
    Some((ru.min(MAX_REPEATS), rv.min(MAX_REPEATS)))
}

/// The most triangles a shape of `before` triangles may have after cutting.
pub fn growth_limit(before: usize) -> usize {
    before * MAX_GROWTH + GROWTH_SLACK
}

/// A lower bound of the triangles cutting `uvs` into cells of `cell` repeats makes: each
/// triangle becomes at least one piece per cell its UV box spans along its longer side.
fn cut_lower_bound(uvs: &[[f32; 2]], triangles: &[[u16; 3]], cell: [f32; 2]) -> usize {
    triangles
        .iter()
        .map(|t| {
            let n = |a: usize| {
                let (lo, hi) = t.iter().fold((f32::MAX, f32::MIN), |(lo, hi), &i| (lo.min(uvs[i as usize][a] / cell[a]), hi.max(uvs[i as usize][a] / cell[a])));
                (hi.floor() - lo.floor()).max(0.0) as usize + 1
            };
            n(0).max(n(1))
        })
        .sum()
}

/// The shape prepared for its repeat tile: UVs shifted by whole repeats into `0..ru x 0..rv`,
/// cut every [`MAX_REPEATS`] repeats where it spans more. `None` when cutting would grow it past
/// [`growth_limit`] (it then keeps its own texture).
pub fn prepare(part: &MeshPart, (ru, rv): (u32, u32)) -> Option<MeshPart> {
    let (shift, reps) = span(&part.uvs);
    let mut p = part.clone();
    for uv in &mut p.uvs {
        uv[0] += shift[0];
        uv[1] += shift[1];
    }
    if reps[0] <= ru && reps[1] <= rv {
        return Some(p);
    }
    // Cut into cells of ru x rv repeats: scale the UVs down, cut along the unit lines (each
    // piece lands in 0..1), scale back up.
    let cell = [ru as f32, rv as f32];
    if cut_lower_bound(&p.uvs, &p.triangles, cell) > growth_limit(p.triangles.len()) {
        return None;
    }
    for uv in &mut p.uvs {
        uv[0] /= cell[0];
        uv[1] /= cell[1];
    }
    let mut c = crate::reuv::recut(&p);
    // Cuts through a corner leave zero-area slivers; they draw nothing.
    let pos = c.positions.clone();
    c.triangles.retain(|t| {
        let [a, b, d] = t.map(|i| pos[i as usize]);
        let (e, f) = ([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [d[0] - a[0], d[1] - a[1], d[2] - a[2]]);
        let n = [e[1] * f[2] - e[2] * f[1], e[2] * f[0] - e[0] * f[2], e[0] * f[1] - e[1] * f[0]];
        n[0] * n[0] + n[1] * n[1] + n[2] * n[2] > 1e-6
    });
    if c.triangles.len() > growth_limit(part.triangles.len()) || c.positions.len() > usize::from(u16::MAX) {
        return None;
    }
    for uv in &mut c.uvs {
        uv[0] *= cell[0];
        uv[1] *= cell[1];
    }
    Some(c)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::{MeshPart, ShaderSource};

    fn quad(u: f32, v: f32, off: f32) -> MeshPart {
        MeshPart {
            name: "q".into(),
            index: 0,
            textures: Default::default(),
            shader: ShaderSource::Lighting(Default::default()),
            positions: vec![[0.0, 0.0, 0.0], [100.0, 0.0, 0.0], [100.0, 0.0, 100.0], [0.0, 0.0, 100.0]],
            uvs: vec![[off, off], [off + u, off], [off + u, off + v], [off, off + v]],
            normals: vec![[0.0, -1.0, 0.0]; 4],
            tangents: vec![[1.0, 0.0, 0.0]; 4],
            bitangents: vec![[0.0, 0.0, 1.0]; 4],
            colors: None,
            triangles: vec![[0, 1, 2], [0, 2, 3]],
            source_normals: true,
            source_tangents: true,
            legacy: false,
            alpha: None,
        }
    }

    #[test]
    fn keys() {
        assert_eq!(tiled_key("a.dds,a_n.dds=8", (3, 2)), "a.dds,a_n.dds=8*3x2");
        assert_eq!(split_key("a.dds,a_n.dds=8*3x2"), ("a.dds,a_n.dds=8", Some((3, 2))));
        assert_eq!(split_key("a.dds,a_n.dds"), ("a.dds,a_n.dds", None));
        assert_eq!(split_key("a*b.dds"), ("a*b.dds", None));
        assert!(is_generated_model(r"Meshes\LOD\LODForge\xjkcowmeshes\a_lod_0.nif"));
        assert!(!is_generated_model(r"meshes\lod\winterhold\a_lod.nif"));
    }

    #[test]
    fn repeats_and_shift() {
        assert_eq!(tile_repeats(&quad(1.0, 1.0, 0.0).uvs), None);
        assert_eq!(tile_repeats(&quad(3.0, 1.0, 0.0).uvs), Some((3, 1)));
        // 2.5 .. 4.5 spans three whole repeats once shifted by -2.
        assert_eq!(span(&quad(2.0, 0.5, 2.5).uvs), ([-2.0, -2.0], [3, 1]));
        assert_eq!(tile_repeats(&quad(20.0, 1.0, 0.0).uvs), Some((MAX_REPEATS, 1)));
    }

    #[test]
    fn prepared_shapes_fit_their_tile_without_cuts() {
        let p = prepare(&quad(3.0, 2.0, 5.25), (4, 3)).unwrap();
        // Shift only: same triangles, UVs within 0..ru x 0..rv.
        assert_eq!(p.triangles.len(), 2);
        assert!(p.uvs.iter().all(|uv| (0.0..=4.0).contains(&uv[0]) && (0.0..=3.0).contains(&uv[1])), "{:?}", p.uvs);
        assert_eq!(p.uvs[0], [0.25, 0.25]);
    }

    #[test]
    fn long_spans_are_cut_every_max_repeats() {
        // 20 repeats in u: cut into pieces of at most 8 repeats.
        let p = prepare(&quad(20.0, 1.0, 0.0), (MAX_REPEATS, 1)).unwrap();
        assert!(p.triangles.len() > 2 && p.triangles.len() <= growth_limit(2));
        for t in &p.triangles {
            let us = t.map(|i| p.uvs[i as usize][0]);
            let (lo, hi) = (us.iter().cloned().fold(f32::MAX, f32::min), us.iter().cloned().fold(f32::MIN, f32::max));
            assert!(lo >= -1e-4 && hi <= MAX_REPEATS as f32 + 1e-4, "{us:?}");
        }
        // A shape repeating hundreds of times is left alone.
        assert!(prepare(&quad(2000.0, 2000.0, 0.0), (MAX_REPEATS, MAX_REPEATS)).is_none());
    }
}
