//! Which triangles of placed LOD models made it into object LOD blocks: every triangle of each
//! listed model, placed by its reference transform, is looked up in the given `.bto` files (all
//! three corners within `--tol` game units of one block triangle's corners). Prints matched and
//! missing counts per model part with the height range of the missing ones; `--obj` writes the
//! missing triangles.
//!
//! `cargo run --release --example ref_coverage -- --refs refs.txt [--tol 2] [--obj missing.obj] <file.bto>...`
//! where each line of refs.txt is `<model.nif> x y z rx ry rz scale` (degrees, as in the export).
use std::collections::HashMap;
use std::fmt::Write as _;

use anyhow::Result;
use lodforge_nif::Block;
use lodforge_objlod::transform::Xform;

fn key(p: [f32; 3], tol: f32) -> [i64; 3] {
    p.map(|v| (v / tol).round() as i64)
}

fn main() -> Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let take = |args: &mut Vec<String>, flag: &str| args.iter().position(|a| a == flag).map(|i| {
        args.remove(i);
        args.remove(i)
    });
    let refs = take(&mut args, "--refs").expect("--refs");
    let tol: f32 = take(&mut args, "--tol").map(|v| v.parse().unwrap()).unwrap_or(2.0);
    let obj = take(&mut args, "--obj");
    // Block triangles keyed by their sorted, snapped corners (each corner also tried on the
    // neighbouring grid points via the lookup below).
    let mut corners: HashMap<[i64; 3], Vec<usize>> = HashMap::new();
    let mut tris: Vec<[[f32; 3]; 3]> = Vec::new();
    for f in &args {
        let nif = lodforge_nif::parse(&std::fs::read(f)?)?;
        for b in &nif.blocks {
            let Block::TriShape(s) = b else { continue };
            let t = s.av.transform.translation;
            let sc = s.av.transform.scale;
            let w = |i: u16| {
                let p = s.positions[i as usize];
                [t[0] + p[0] * sc, t[1] + p[1] * sc, t[2] + p[2] * sc]
            };
            for tri in &s.triangles {
                let v = [w(tri[0]), w(tri[1]), w(tri[2])];
                corners.entry(key(v[0], tol)).or_default().push(tris.len());
                tris.push(v);
            }
        }
    }
    let near = |a: [f32; 3], b: [f32; 3]| (0..3).all(|k| (a[k] - b[k]).abs() <= tol);
    let found = |v: &[[f32; 3]; 3]| {
        let k = key(v[0], tol);
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let Some(list) = corners.get(&[k[0] + dx, k[1] + dy, k[2] + dz]) else { continue };
                    for &i in list {
                        let t = &tris[i];
                        if near(t[0], v[0]) && ((near(t[1], v[1]) && near(t[2], v[2])) || (near(t[1], v[2]) && near(t[2], v[1]))) {
                            return true;
                        }
                    }
                }
            }
        }
        // Any rotation of the corner order.
        false
    };
    let found_any = |v: &[[f32; 3]; 3]| found(v) || found(&[v[1], v[2], v[0]]) || found(&[v[2], v[0], v[1]]);
    let mut out = String::new();
    let mut base = 1usize;
    for line in std::fs::read_to_string(&refs)?.lines() {
        let p: Vec<&str> = line.split_whitespace().collect();
        if p.len() < 8 {
            continue;
        }
        let n: Vec<f32> = p[1..8].iter().map(|v| v.parse().unwrap()).collect();
        let xf = Xform::reference([n[0], n[1], n[2]], [n[3], n[4], n[5]], n[6]);
        let mesh = lodforge_objlod::mesh::import(p[0], &std::fs::read(p[0])?)?;
        for part in &mesh.parts {
            let (mut hit, mut miss) = (0usize, 0usize);
            let (mut zmin, mut zmax) = (f32::INFINITY, f32::NEG_INFINITY);
            for tri in &part.triangles {
                let v = tri.map(|i| xf.apply(part.positions[i as usize]));
                if found_any(&v) {
                    hit += 1;
                } else {
                    miss += 1;
                    for c in &v {
                        zmin = zmin.min(c[2]);
                        zmax = zmax.max(c[2]);
                        let _ = writeln!(out, "v {} {} {}", c[0], c[1], c[2]);
                    }
                    let _ = writeln!(out, "f {} {} {}", base, base + 1, base + 2);
                    base += 3;
                }
            }
            let file = std::path::Path::new(p[0]).file_name().unwrap().to_string_lossy();
            println!("{file}\trot={}\t{}\ttris={}\tfound={hit}\tmissing={miss}\t{}", n[5], part.name, part.triangles.len(), if miss > 0 { format!("z={zmin:.0}..{zmax:.0}") } else { String::new() });
        }
    }
    if let Some(o) = obj {
        std::fs::write(o, out)?;
    }
    Ok(())
}
