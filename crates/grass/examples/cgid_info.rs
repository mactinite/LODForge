//! Prints a grass cache file's types and instance counts, and the first instances of each type;
//! with several files, one line per file and the total.
//! `cargo run -p lodforge-grass --example cgid_info -- <file.cgid> [max instances per type]`
//! `cargo run -p lodforge-grass --example cgid_info -- <file.cgid> <file.cgid> ...`

use lodforge_grass::cgid::CellCache;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.len() > 2 || (a.len() == 2 && a[1].parse::<usize>().is_err()) {
        let mut total = 0;
        for f in &a {
            let n = std::fs::read(f).ok().and_then(|b| CellCache::parse(&b)).map(|c| c.instance_count()).unwrap_or(0);
            println!("{n:8} {f}");
            total += n;
        }
        println!("total instances: {total}");
        return;
    }
    let max: usize = a.get(1).and_then(|s| s.parse().ok()).unwrap_or(2);
    let bytes = std::fs::read(&a[0]).unwrap();
    let c = CellCache::parse(&bytes).expect("parse");
    println!("{} types, {} instances", c.types.len(), c.instance_count());
    for t in &c.types {
        let n: usize = t.groups.iter().map(|g| g.instances.len()).sum();
        println!("type gras={} form={:08X} model={} wave={} slope={} uniform={} vlight={} groups={} instances={}", t.gras, t.form_id, t.model, t.wave_period, t.fits_to_slope, t.uniform_scaling, t.vertex_lighting, t.groups.len(), n);
        for g in t.groups.iter().take(1) {
            println!("  group center={:?} half={:?} instances={}", g.center, g.half_extent, g.instances.len());
            for inst in g.instances.iter().take(max) {
                let h: Vec<String> = inst.iter().map(|&v| format!("{:.3}", half::f16::from_bits(v).to_f32())).collect();
                println!("    raw={:?}\n    f16={}", inst, h.join(" "));
            }
        }
    }
}
