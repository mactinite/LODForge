//! Texture tiling of the shapes in NIF files: for each shape with a lighting shader, its first
//! texture and the world units one texture repeat covers (square root of the shape's world
//! area over its UV area, both summed over its triangles, the shape's own transform applied).
//! Compares how often a full model and its LOD model repeat the same texture.
//!
//! `cargo run --release --example uv_density -- <file.nif>...`

use anyhow::Result;
use lodforge_nif::Block;

fn tri_area3(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> f64 {
    let u = [f64::from(b[0] - a[0]), f64::from(b[1] - a[1]), f64::from(b[2] - a[2])];
    let v = [f64::from(c[0] - a[0]), f64::from(c[1] - a[1]), f64::from(c[2] - a[2])];
    let x = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
    0.5 * (x[0] * x[0] + x[1] * x[1] + x[2] * x[2]).sqrt()
}

fn tri_area2(a: [f32; 2], b: [f32; 2], c: [f32; 2]) -> f64 {
    0.5 * (f64::from(b[0] - a[0]) * f64::from(c[1] - a[1]) - f64::from(c[0] - a[0]) * f64::from(b[1] - a[1])).abs()
}

fn main() -> Result<()> {
    for f in std::env::args().skip(1) {
        let bytes = std::fs::read(&f)?;
        let nif = lodforge_nif::parse(&bytes)?;
        println!("{f}");
        for b in &nif.blocks {
            let Block::TriShape(s) = b else { continue };
            let Some(Block::LightingShader(l)) = nif.get(s.shader_property) else { continue };
            let tex = match nif.get(l.texture_set) {
                Some(Block::TextureSet(t)) => t.first().cloned().unwrap_or_default(),
                _ => String::new(),
            };
            let scale = f64::from(s.av.transform.scale);
            let (mut world, mut uv) = (0.0, 0.0);
            for t in &s.triangles {
                let [i, j, k] = [t[0] as usize, t[1] as usize, t[2] as usize];
                let (Some(&pa), Some(&pb), Some(&pc)) = (s.positions.get(i), s.positions.get(j), s.positions.get(k)) else { continue };
                let (Some(&ua), Some(&ub), Some(&uc)) = (s.uvs.get(i), s.uvs.get(j), s.uvs.get(k)) else { continue };
                world += tri_area3(pa, pb, pc) * scale * scale;
                uv += tri_area2(ua, ub, uc);
            }
            if uv <= 0.0 {
                continue;
            }
            let per_repeat = (world / uv).sqrt();
            let uvs = [l.uv_scale[0], l.uv_scale[1]];
            println!(
                "  {:<28} {:>6} tris  {:>8.1} units per repeat  (shader uv scale {:.2},{:.2})  {}",
                s.av.net.name.as_deref().unwrap_or(""),
                s.triangles.len(),
                per_repeat / f64::from(uvs[0].max(1e-3)),
                uvs[0],
                uvs[1],
                tex
            );
        }
    }
    Ok(())
}
