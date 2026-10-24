//! Object LOD triangles inside a world-space box: for every shape of the given `.bto` files, the
//! triangles whose centroid lies in the box (x0..x1, y0..y1), with their height range and first
//! texture; optionally written as an OBJ (one group per shape) for side-by-side checks.
//!
//! `cargo run --release --example bto_region -- --box x0,y0,x1,y1 [--obj out.obj] <file.bto>...`
use std::fmt::Write as _;

use anyhow::Result;
use lodforge_nif::Block;

fn main() -> Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let take = |args: &mut Vec<String>, flag: &str| args.iter().position(|a| a == flag).map(|i| {
        args.remove(i);
        args.remove(i)
    });
    let bx: Vec<f32> = take(&mut args, "--box").expect("--box x0,y0,x1,y1").split(',').map(|v| v.parse().unwrap()).collect();
    let obj = take(&mut args, "--obj");
    let mut out = String::new();
    let mut base = 1usize;
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
            let tex = match nif.get(s.shader_property) {
                Some(Block::LightingShader(l)) => match nif.get(l.texture_set) {
                    Some(Block::TextureSet(t)) => t.first().cloned().unwrap_or_default(),
                    _ => String::new(),
                },
                Some(Block::EffectShader(_)) => "(effect)".into(),
                _ => String::new(),
            };
            let mut n = 0usize;
            let (mut zmin, mut zmax) = (f32::INFINITY, f32::NEG_INFINITY);
            let mut group = String::new();
            for tri in &s.triangles {
                let v = [w(tri[0]), w(tri[1]), w(tri[2])];
                let c = [(v[0][0] + v[1][0] + v[2][0]) / 3.0, (v[0][1] + v[1][1] + v[2][1]) / 3.0];
                if c[0] < bx[0] || c[0] > bx[2] || c[1] < bx[1] || c[1] > bx[3] {
                    continue;
                }
                n += 1;
                for p in &v {
                    zmin = zmin.min(p[2]);
                    zmax = zmax.max(p[2]);
                    let _ = writeln!(group, "v {} {} {}", p[0], p[1], p[2]);
                }
                let _ = writeln!(group, "f {} {} {}", base, base + 1, base + 2);
                base += 3;
            }
            if n > 0 {
                let name = s.av.net.name.clone().unwrap_or_default();
                println!("{}\t{name}\tverts={}\ttris={}\tin_box={n}\tz={zmin:.0}..{zmax:.0}\t{tex}", std::path::Path::new(f).file_name().unwrap().to_string_lossy(), s.positions.len(), s.triangles.len());
                let _ = writeln!(out, "g {name}_{}", tex.replace(['\\', '/', ' '], "_"));
                out.push_str(&group);
            }
        }
    }
    if let Some(o) = obj {
        std::fs::write(o, out)?;
    }
    Ok(())
}
