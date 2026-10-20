//! Dumps the vertices of the shapes of a BTO whose name contains a substring: world position,
//! UV, normal, tangent, bitangent, colour, plus the triangles, up to a limit.
//! `cargo run -p lodforge-nif --example dump_shape -- <file> <name substring> [max vertices] [skip vertices]`

use lodforge_nif::{Block, parse};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let nif = parse(&std::fs::read(&a[0]).unwrap()).unwrap();
    let needle = a[1].to_ascii_lowercase();
    let max: usize = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(16);
    let skip: usize = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(0);
    for (i, b) in nif.blocks.iter().enumerate() {
        let Block::TriShape(s) = b else { continue };
        let name = s.av.net.name.clone().unwrap_or_default();
        if !name.to_ascii_lowercase().contains(&needle) {
            continue;
        }
        let t = &s.av.transform;
        println!("[{i}] {name} v={} t={} translation={:?} scale={} textures={:?}", s.positions.len(), s.triangles.len(), t.translation, t.scale, nif.shape_textures(s));
        for (vi, p) in s.positions.iter().enumerate().skip(skip).take(max) {
            let w = [p[0] * t.scale + t.translation[0], p[1] * t.scale + t.translation[1], p[2] * t.scale + t.translation[2]];
            let uv = s.uvs.get(vi).copied().unwrap_or([0.0; 2]);
            let n = s.normals.get(vi).copied().unwrap_or([0.0; 3]);
            let tg = s.tangents.get(vi).copied().unwrap_or([0.0; 3]);
            let bt = s.bitangents.get(vi).copied().unwrap_or([0.0; 3]);
            let c = s.colors.get(vi).copied();
            println!("  v{vi}: w=({:.3}, {:.3}, {:.3}) uv=({:.5}, {:.5}) n=({:.3}, {:.3}, {:.3}) t=({:.3}, {:.3}, {:.3}) b=({:.3}, {:.3}, {:.3}) c={:?}", w[0], w[1], w[2], uv[0], uv[1], n[0], n[1], n[2], tg[0], tg[1], tg[2], bt[0], bt[1], bt[2], c);
        }
        let tris: Vec<String> = s.triangles.iter().skip(skip / 2).take(max / 2).map(|t| format!("{:?}", t)).collect();
        println!("  triangles: {}", tris.join(" "));
    }
}
