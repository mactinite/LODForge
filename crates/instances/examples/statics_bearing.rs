//! Records of a `.statics.bin` matching a model substring, by bearing (deg, 0 = +Y, clockwise) and
//! distance from a point: `statics_bearing <file> <x> <y> <maxdist> <substr>`.
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let f = lodforge_instances::statics::StaticsFile::from_bytes(&std::fs::read(&a[1]).unwrap()).unwrap();
    let (x, y, maxd): (f32, f32, f32) = (a[2].parse().unwrap(), a[3].parse().unwrap(), a[4].parse().unwrap());
    let q = a[5].to_ascii_lowercase();
    let mut rows = Vec::new();
    for (&(level, _, _), recs) in &f.blocks {
        for r in recs {
            let m = &f.meshes[r.mesh as usize];
            if !m.model.to_ascii_lowercase().contains(&q) { continue; }
            let (dx, dy) = (r.pos[0] - x, r.pos[1] - y);
            let d = (dx * dx + dy * dy).sqrt();
            if d > maxd { continue; }
            let b = dx.atan2(dy).to_degrees().rem_euclid(360.0);
            let plugin = f.common.plugins.get(r.plugin as usize).cloned().unwrap_or_default();
            rows.push((level, b, d, r.pos[2], m.flags, m.model.clone(), plugin, r.form));
        }
    }
    rows.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    for (l, b, d, z, fl, m, p, id) in rows { println!("LOD{l}\tbear {b:5.0}\tdist {d:6.0}\tz {z:6.0}\tflags {fl:#x}\t{p}:{id:06X}\t{m}"); }
}
