//! Nearest-vertex distances between two BTOs for shapes of one name: for every vertex of the
//! first file's shapes named `<name>`, the distance to the nearest vertex of the second file's
//! shapes of that name (hash grid of 4 units, exact beyond that is reported as "> 16"), as a
//! histogram, plus the farthest cases with their positions and colours.
//! `cargo run -p lodforge-nif --example bto_nearest -- <a.bto> <b.bto> <shape name> [examples]`

use std::collections::HashMap;

use lodforge_nif::{Block, parse};

fn verts(path: &str, name: &str) -> Vec<([f32; 3], [u8; 4], [f32; 2])> {
    let nif = parse(&std::fs::read(path).unwrap()).unwrap();
    let mut out = Vec::new();
    for b in &nif.blocks {
        let Block::TriShape(s) = b else { continue };
        if s.av.net.name.as_deref() != Some(name) {
            continue;
        }
        let t = &s.av.transform;
        for (i, p) in s.positions.iter().enumerate() {
            let w = [p[0] * t.scale + t.translation[0], p[1] * t.scale + t.translation[1], p[2] * t.scale + t.translation[2]];
            out.push((w, s.colors.get(i).copied().unwrap_or([0; 4]), s.uvs.get(i).copied().unwrap_or([0.0; 2])));
        }
    }
    out
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let examples: usize = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(8);
    let va = verts(&a[0], &a[2]);
    let vb = verts(&a[1], &a[2]);
    let cell = 4.0f32;
    let key = |p: [f32; 3]| [(p[0] / cell).floor() as i64, (p[1] / cell).floor() as i64, (p[2] / cell).floor() as i64];
    let mut grid: HashMap<[i64; 3], Vec<usize>> = HashMap::new();
    for (i, v) in vb.iter().enumerate() {
        grid.entry(key(v.0)).or_default().push(i);
    }
    let bins = [0.1f32, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0];
    let mut hist = vec![0usize; bins.len() + 1];
    let mut far: Vec<(f32, usize)> = Vec::new();
    for (i, v) in va.iter().enumerate() {
        let k = key(v.0);
        let mut best = f32::MAX;
        for dx in -4..=4 {
            for dy in -4..=4 {
                for dz in -4..=4 {
                    if let Some(list) = grid.get(&[k[0] + dx, k[1] + dy, k[2] + dz]) {
                        for &j in list {
                            let q = vb[j].0;
                            let d = ((q[0] - v.0[0]).powi(2) + (q[1] - v.0[1]).powi(2) + (q[2] - v.0[2]).powi(2)).sqrt();
                            if d < best {
                                best = d;
                            }
                        }
                    }
                }
            }
        }
        let bin = bins.iter().position(|&b| best <= b).unwrap_or(bins.len());
        hist[bin] += 1;
        if best > 4.0 {
            far.push((best, i));
        }
    }
    println!("{} vertices in A, {} in B", va.len(), vb.len());
    let mut lo = 0.0;
    for (i, &b) in bins.iter().enumerate() {
        println!("  {lo:>5} .. {b:>5}: {:8} ({:.1}%)", hist[i], 100.0 * hist[i] as f64 / va.len() as f64);
        lo = b;
    }
    println!("  > {:>5}      : {:8} ({:.1}%)", bins[bins.len() - 1], hist[bins.len()], 100.0 * hist[bins.len()] as f64 / va.len() as f64);
    far.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    for (d, i) in far.iter().take(examples) {
        let v = &va[*i];
        println!("  far {d:.1}: A v{i} at ({:.1}, {:.1}, {:.1}) colour {:?} uv ({:.4}, {:.4})", v.0[0], v.0[1], v.0[2], v.1, v.2[0], v.2[1]);
    }
}
