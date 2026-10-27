//! Statics records of a `.statics.bin` inside a box: level, model, flags, plugin:form.
//! `cargo run --release -p lodforge-instances --example statics_near -- <file> <x0> <y0> <x1> <y1> [model substring]`
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let f = lodforge_instances::statics::StaticsFile::from_bytes(&std::fs::read(&a[1]).unwrap()).unwrap();
    let n = |i: usize| a[i].parse::<f32>().unwrap();
    let (x0, y0, x1, y1) = (n(2), n(3), n(4), n(5));
    let filt = a.get(6).map(|s| s.to_ascii_lowercase());
    let mut by: std::collections::BTreeMap<(u8, String, u32), usize> = Default::default();
    for (&(level, _, _), recs) in &f.blocks {
        for r in recs {
            if r.pos[0] < x0 || r.pos[0] > x1 || r.pos[1] < y0 || r.pos[1] > y1 { continue; }
            let m = &f.meshes[r.mesh as usize];
            if filt.as_ref().is_some_and(|q| !m.model.to_ascii_lowercase().contains(q.as_str())) { continue; }
            *by.entry((level, m.model.clone(), m.flags)).or_default() += 1;
        }
    }
    for ((l, m, fl), c) in by { println!("LOD{l}\t{c}\tflags {fl:#x}\t{m}"); }
}
