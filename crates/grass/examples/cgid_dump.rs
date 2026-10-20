//! Dumps every instance of a grass cache file: model, unwrapped position, and the remaining
//! thirteen halves as floats (scale, then what AnvilLOD's `GrassCache.cs` calls the rotation
//! matrix, then three more). `cargo run -p lodforge-grass --example cgid_dump -- <file.cgid>`

use half::f16;
use lodforge_grass::cgid::CellCache;

fn unwrap(v: f32, center: f32) -> f32 {
    v + ((center - v) / 49152.0).round() * 49152.0
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let bytes = std::fs::read(&a[0]).unwrap();
    let c = CellCache::parse(&bytes).expect("parse");
    for t in &c.types {
        for g in &t.groups {
            for inst in &g.instances {
                let h: Vec<f32> = inst.iter().map(|&v| f16::from_bits(v).to_f32()).collect();
                let p = [unwrap(h[0], g.center[0]), unwrap(h[1], g.center[1]), unwrap(h[2], g.center[2])];
                let rest: Vec<String> = h[3..].iter().map(|v| format!("{v:.4}")).collect();
                println!("{}\t{:08X}\t{:.2}\t{:.2}\t{:.2}\t{}", t.model, t.form_id, p[0], p[1], p[2], rest.join("\t"));
            }
        }
    }
}
