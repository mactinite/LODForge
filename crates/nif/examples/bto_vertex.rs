//! Prints the attributes of the vertices nearest to a world position in every shape of a BTO (or
//! of a plain NIF, positions as stored), to compare one vertex between two files.
//! `cargo run -p lodforge-nif --example bto_vertex -- <file> <x> <y> <z> [radius]`

use lodforge_nif::{Block, parse};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let nif = parse(&std::fs::read(&a[0]).unwrap()).unwrap();
    let p: Vec<f32> = a[1..4].iter().map(|s| s.parse().unwrap()).collect();
    let radius: f32 = a.get(4).and_then(|s| s.parse().ok()).unwrap_or(1.0);
    for b in &nif.blocks {
        let Block::TriShape(s) = b else { continue };
        let t = &s.av.transform;
        let desc = s.vertex_desc;
        for (i, v) in s.positions.iter().enumerate() {
            let w = [v[0] * t.scale + t.translation[0], v[1] * t.scale + t.translation[1], v[2] * t.scale + t.translation[2]];
            let d = ((w[0] - p[0]).powi(2) + (w[1] - p[1]).powi(2) + (w[2] - p[2]).powi(2)).sqrt();
            if d <= radius {
                println!(
                    "{:?} desc={desc:#x} v{i} d={d:.3} pos={w:?} uv={:?} n={:?} t={:?} b={:?} c={:?}",
                    s.av.net.name,
                    s.uvs.get(i),
                    s.normals.get(i),
                    s.tangents.get(i),
                    s.bitangents.get(i),
                    s.colors.get(i)
                );
            }
        }
    }
}
