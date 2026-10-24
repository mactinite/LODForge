//! Shapes (draw calls) and triangles of a worldspace's default-season object LOD, per level, and
//! how many shapes carry their own texture instead of an atlas page.
//!
//! `cargo run --release --example bto_shapes -- <objects dir> [World]`
use std::collections::BTreeMap;

use anyhow::Result;
use lodforge_nif::Block;

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = std::path::PathBuf::from(&args[0]);
    let world = args.get(1).cloned().unwrap_or_else(|| "Tamriel".into());
    // level -> (files, shapes, triangles, shapes on their own texture)
    let mut per: BTreeMap<i32, (usize, usize, usize, usize)> = BTreeMap::new();
    for e in std::fs::read_dir(&dir)?.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let parts: Vec<&str> = name.split('.').collect();
        // Default season only: World.L.X.Y.bto
        if parts.len() != 5 || !parts[0].eq_ignore_ascii_case(&world) || !parts[4].eq_ignore_ascii_case("bto") {
            continue;
        }
        let Ok(level) = parts[1].parse::<i32>() else { continue };
        let nif = lodforge_nif::parse(&std::fs::read(e.path())?)?;
        let s = per.entry(level).or_default();
        s.0 += 1;
        for b in &nif.blocks {
            let Block::TriShape(t) = b else { continue };
            s.1 += 1;
            s.2 += t.triangles.len();
            let own = match nif.get(t.shader_property) {
                Some(Block::LightingShader(l)) => match nif.get(l.texture_set) {
                    Some(Block::TextureSet(ts)) => !ts.first().is_some_and(|d| d.to_ascii_lowercase().contains("\\dyndolod\\lod\\") || d.to_ascii_lowercase().contains("objectslod")),
                    _ => true,
                },
                _ => false,
            };
            if own {
                s.3 += 1;
            }
        }
    }
    for (l, (f, s, t, own)) in per {
        println!("LOD{l}\t{f} files\t{s} shapes\t{t} triangles\t{own} shapes on their own texture");
    }
    Ok(())
}
