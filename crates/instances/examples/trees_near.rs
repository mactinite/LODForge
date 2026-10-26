//! Prints the references of a `.trees.bin` nearest to a point: distance, plugin, form id, type.
use lodforge_instances::trees::TreeFile;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let f = TreeFile::from_bytes(&std::fs::read(&a[1]).unwrap()).unwrap();
    let (x, y, n): (f32, f32, usize) = (a[2].parse().unwrap(), a[3].parse().unwrap(), a.get(4).map_or(10, |s| s.parse().unwrap()));
    let mut v: Vec<(f32, String)> = Vec::new();
    for (&cell, recs) in &f.cells {
        for r in recs {
            let p = r.position(cell);
            let d = (p[0] - x).hypot(p[1] - y);
            v.push((d, format!("{d:7.0} ({:.0},{:.0},{:.0}) {} {:06X} {}", p[0], p[1], p[2], f.common.plugins[usize::from(r.plugin)], r.form, f.types[usize::from(r.type_)].name)));
        }
    }
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    for (_, s) in v.into_iter().take(n) {
        println!("{s}");
    }
}
