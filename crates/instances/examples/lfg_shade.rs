//! Mean shade of the records of a `.lfg`, and of the cells near a point: `lfg_shade <file> [x y]`.
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let f = lodforge_instances::grass::GrassFile::from_bytes(&std::fs::read(&a[1]).unwrap()).unwrap();
    let near = a.get(2).map(|x| (x.parse::<f32>().unwrap() / 4096.0).floor() as i16).zip(a.get(3).map(|y| (y.parse::<f32>().unwrap() / 4096.0).floor() as i16));
    let (mut n, mut s, mut hist) = (0usize, 0f64, [0usize; 7]);
    for ((x, y), c) in &f.cells {
        if let Some((cx, cy)) = near { if (x - cx).abs() > 4 || (y - cy).abs() > 4 { continue; } }
        for r in &c.records { n += 1; s += r.shade as f64 / 200.0; hist[(r.shade / 40).min(6) as usize] += 1; }
    }
    println!("{n} records, mean shade {:.3}; histogram per 0.2: {:?}", s / n.max(1) as f64, hist);
}
