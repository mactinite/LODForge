//! Prints a `.trees.bin` file's types (index, name, model, references, bases, pack mesh) and,
//! with `x y radius`, the per-type reference counts inside that circle.
use lodforge_instances::trees::TreeFile;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let f = TreeFile::from_bytes(&std::fs::read(&args[1]).expect("read")).expect("parse");
    let near: Option<(f32, f32, f32)> = (args.len() >= 5).then(|| (args[2].parse().unwrap(), args[3].parse().unwrap(), args[4].parse().unwrap()));
    let mut counts = vec![0usize; f.types.len()];
    for (&cell, recs) in &f.cells {
        for r in recs {
            let p = r.position(cell);
            if near.is_none_or(|(x, y, rad)| (p[0] - x).hypot(p[1] - y) <= rad) {
                counts[usize::from(r.type_)] += 1;
            }
        }
    }
    for (i, t) in f.types.iter().enumerate() {
        if counts[i] == 0 {
            continue;
        }
        let bases: Vec<String> = t.bases.iter().map(|&(p, b)| format!("{}:{b:06X}", f.common.plugins.get(usize::from(p)).map(String::as_str).unwrap_or("?"))).collect();
        let pack = f.pack_meshes.get(i).copied().unwrap_or(u32::MAX);
        let mesh = f.meshes.get(i).map(|m| m.shapes.iter().map(|s| s.diffuse.clone()).collect::<Vec<_>>().join(";")).unwrap_or_default();
        println!("{i:4} {:6} {} {} pack={pack} bases=[{}] mesh={mesh}", counts[i], t.name, t.model, bases.join(" "));
    }
}
