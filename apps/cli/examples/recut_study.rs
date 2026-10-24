//! Where re-cutting generated LOD models for the atlas adds triangles: over a LOD model cache
//! (`<crc>_<level>_<params>.nif`, the generator's output before `fit_uvs`), per level, the
//! triangles of shapes whose UVs leave 0..1, what a full re-cut makes of them, and how many of
//! the added triangles come from triangles that span at most one texture repeat (they only
//! straddle a repeat line and would fit a tile holding the texture 2 x 2 with a per-triangle
//! integer shift, uncut) versus triangles spanning more than one repeat.
//!
//! `cargo run --release --example recut_study -- <cache dir>`
use std::collections::BTreeMap;

use anyhow::Result;
use lodforge_objlod::mesh::MeshPart;

#[derive(Default)]
struct Level {
    models: usize,
    tris: usize,
    /// Triangles of shapes that need a cut.
    tiled_tris: usize,
    /// Those shapes after a full re-cut.
    recut_tris: usize,
    /// Triangles spanning <= 1 repeat that cross a line, and what cutting them adds.
    straddle: usize,
    straddle_added: usize,
    /// Triangles spanning > 1 repeat, and what cutting them adds.
    multi: usize,
    multi_added: usize,
    /// Per-shape growth histogram of the full re-cut (ratio buckets).
    growth: BTreeMap<u32, usize>,
    /// Shapes left tiled by the current policy (not cut).
    heavy_shapes: usize,
    heavy_tris: usize,
    /// Triangles of shapes needing a cut by the larger repeat count of their UV box.
    span: BTreeMap<u32, usize>,
}

fn one(part: &MeshPart, tri: &[u16; 3]) -> MeshPart {
    let idx = tri.map(usize::from);
    let pick3 = |v: &Vec<[f32; 3]>| idx.iter().map(|&i| v[i]).collect::<Vec<_>>();
    MeshPart {
        positions: pick3(&part.positions),
        uvs: idx.iter().map(|&i| part.uvs[i]).collect(),
        normals: pick3(&part.normals),
        tangents: pick3(&part.tangents),
        bitangents: pick3(&part.bitangents),
        colors: part.colors.as_ref().map(|c| idx.iter().map(|&i| c[i]).collect()),
        triangles: vec![[0, 1, 2]],
        ..part.clone()
    }
}

fn main() -> Result<()> {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).expect("cache dir"));
    let mut levels: BTreeMap<u8, Level> = BTreeMap::new();
    for e in std::fs::read_dir(&dir)?.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some(k) = name.split('_').nth(1).and_then(|s| s.parse::<u8>().ok()) else { continue };
        let bytes = std::fs::read(e.path())?;
        let Ok(mesh) = lodforge_objlod::mesh::import(&name, &bytes) else { continue };
        let l = levels.entry(k).or_default();
        l.models += 1;
        for part in &mesh.parts {
            l.tris += part.triangles.len();
            if !lodforge_objlod::reuv::needs_recut(&part.uvs, 1e-3) {
                continue;
            }
            // Shapes the current policy leaves tiled (lower bound past 4x + 64) are not cut here.
            let bound: usize = part
                .triangles
                .iter()
                .map(|t| {
                    let span = |a: usize| {
                        let (lo, hi) = t.iter().fold((f32::MAX, f32::MIN), |(lo, hi), &i| (lo.min(part.uvs[i as usize][a]), hi.max(part.uvs[i as usize][a])));
                        (hi.floor() - lo.floor()).max(0.0) as usize + 1
                    };
                    span(0).max(span(1))
                })
                .sum();
            if bound > part.triangles.len() * 4 + 64 {
                l.heavy_shapes += 1;
                l.heavy_tris += part.triangles.len();
                continue;
            }
            l.tiled_tris += part.triangles.len();
            // The shape's whole repeat span after an integer shift (u, v repeats).
            let reps = |a: usize| {
                let (lo, hi) = part.uvs.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p[a]), hi.max(p[a])));
                ((hi - lo.floor()) - 1e-4).ceil().max(1.0) as u32
            };
            let r = reps(0).max(reps(1));
            *l.span.entry(r.min(17)).or_default() += part.triangles.len();
            let r = lodforge_objlod::reuv::recut(part);
            l.recut_tris += r.triangles.len();
            let ratio = (r.triangles.len() as f32 / part.triangles.len() as f32 * 4.0).round() as u32;
            *l.growth.entry(ratio.min(40)).or_default() += 1;
            for t in &part.triangles {
                let uv = t.map(|i| part.uvs[i as usize]);
                let span = |a: usize| {
                    let (lo, hi) = uv.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p[a]), hi.max(p[a])));
                    (hi - lo, hi.floor() - lo.floor() >= 1.0 && hi.fract() > 1e-4 || (hi.floor() - lo.floor()) >= 2.0)
                };
                let ((su, cu), (sv, cv)) = (span(0), span(1));
                if !cu && !cv {
                    continue;
                }
                let added = lodforge_objlod::reuv::recut(&one(part, t)).triangles.len().saturating_sub(1);
                if su <= 1.0 && sv <= 1.0 {
                    l.straddle += 1;
                    l.straddle_added += added;
                } else {
                    l.multi += 1;
                    l.multi_added += added;
                }
            }
        }
    }
    for (k, l) in &levels {
        println!(
            "level {k}: {} models, {} triangles; shapes needing a cut: {} triangles -> {} re-cut (+{}); straddling (span <= 1) {} triangles add {}, spanning > 1 repeat {} triangles add {}",
            l.models, l.tris, l.tiled_tris, l.recut_tris, l.recut_tris - l.tiled_tris, l.straddle, l.straddle_added, l.multi, l.multi_added
        );
        println!("  left tiled (not cut): {} shapes, {} triangles", l.heavy_shapes, l.heavy_tris);
        println!("  triangles by repeats of the shape UV box (max of u, v; 17 = more): {:?}", l.span);
        println!("  growth per shape (x0.25 buckets): {:?}", l.growth.iter().map(|(b, n)| (*b as f32 / 4.0, *n)).collect::<Vec<_>>());
    }
    Ok(())
}
