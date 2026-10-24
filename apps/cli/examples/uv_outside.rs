//! Vertices of object LOD shapes on an atlas page whose UVs leave the page (0..1 plus `slack`),
//! with their world positions, to trace the references they came from.
//!
//! `cargo run --release --example uv_outside -- [--slack 0.001] <file.bto>...`
use anyhow::Result;
use lodforge_nif::Block;

fn main() -> Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut slack = 0.001f32;
    if let Some(i) = args.iter().position(|a| a == "--slack") {
        args.remove(i);
        slack = args.remove(i).parse()?;
    }
    for f in &args {
        let nif = lodforge_nif::parse(&std::fs::read(f)?)?;
        for b in &nif.blocks {
            let Block::TriShape(s) = b else { continue };
            let tex = match nif.get(s.shader_property) {
                Some(Block::LightingShader(l)) => match nif.get(l.texture_set) {
                    Some(Block::TextureSet(t)) => t.first().cloned().unwrap_or_default(),
                    _ => continue,
                },
                _ => continue,
            };
            if !tex.to_ascii_lowercase().contains("dyndolod\\lod\\") {
                continue;
            }
            let (t, sc) = (s.av.transform.translation, s.av.transform.scale);
            for (i, uv) in s.uvs.iter().enumerate() {
                if uv[0] < -slack || uv[0] > 1.0 + slack || uv[1] < -slack || uv[1] > 1.0 + slack {
                    let p = s.positions[i];
                    println!("{}\t{}\t{tex}\tuv {:.4},{:.4}\tworld {:.0},{:.0},{:.0}", std::path::Path::new(f).file_name().unwrap().to_string_lossy(), s.av.net.name.clone().unwrap_or_default(), uv[0], uv[1], t[0] + p[0] * sc, t[1] + p[1] * sc, t[2] + p[2] * sc);
                }
            }
        }
    }
    Ok(())
}
