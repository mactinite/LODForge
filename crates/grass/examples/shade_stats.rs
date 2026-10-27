//! Shade (instance half [3]) statistics of `.cgid` files: `shade_stats <file.cgid>...`
fn main() {
    let mut h = [0usize; 24];
    let (mut n, mut lo, mut hi, mut sum) = (0usize, f32::MAX, f32::MIN, 0f64);
    for p in std::env::args().skip(1) {
        let Some(c) = lodforge_grass::cgid::CellCache::parse(&std::fs::read(&p).unwrap()) else { eprintln!("bad {p}"); continue };
        for t in &c.types {
            for g in &t.groups {
                for i in &g.instances {
                    let s = half::f16::from_bits(i[3]).to_f32();
                    n += 1; lo = lo.min(s); hi = hi.max(s); sum += s as f64;
                    h[((s * 10.0).clamp(0.0, 23.0)) as usize] += 1;
                }
            }
        }
    }
    println!("{n} instances, shade min {lo:.3} max {hi:.3} mean {:.3}", sum / n.max(1) as f64);
    for (k, c) in h.iter().enumerate() { if *c > 0 { println!("  {:.1}..{:.1}: {:5.1}%", k as f32 / 10.0, (k + 1) as f32 / 10.0, 100.0 * *c as f64 / n as f64); } }
}
