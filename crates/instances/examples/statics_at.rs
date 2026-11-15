//! Every reference of a `.statics.bin` within a radius of a point: plugin:form, position, and its
//! model, flags and triangles per level.
//! `cargo run --release -p lodforge-instances --example statics_at -- <file> <x> <y> <radius> [model substring]`
//! With `@<model substring>` for x: centred on the first record whose model contains it
//! (`statics_at <file> @wrcastleentrance01 <radius>`).
use std::collections::BTreeMap;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let f = lodforge_instances::statics::StaticsFile::from_bytes(&std::fs::read(&a[1]).unwrap()).unwrap();
    let (cx, cy, radius, filt) = match a[2].strip_prefix('@') {
        Some(q) => {
            let q = q.to_ascii_lowercase();
            let r = f.blocks.values().flatten().find(|r| f.meshes[r.mesh as usize].model.to_ascii_lowercase().contains(&q)).expect("no record with that model");
            (r.pos[0], r.pos[1], a[3].parse::<f32>().unwrap(), a.get(4))
        }
        None => (a[2].parse::<f32>().unwrap(), a[3].parse::<f32>().unwrap(), a[4].parse::<f32>().unwrap(), a.get(5)),
    };
    let filt = filt.map(|s| s.to_ascii_lowercase());
    println!("centre {cx:.0} {cy:.0} (cell {} {}), radius {radius}", (cx / 4096.0).floor(), (cy / 4096.0).floor());
    // Reference -> (position, level -> text).
    // Synthetic records (child-world copies, no plugin) are told apart by their hash.
    let mut refs: BTreeMap<(u16, u32), ([f32; 3], BTreeMap<u8, Vec<String>>)> = BTreeMap::new();
    for (&(level, _, _), recs) in &f.blocks {
        for r in recs {
            if (r.pos[0] - cx).hypot(r.pos[1] - cy) > radius {
                continue;
            }
            let m = &f.meshes[r.mesh as usize];
            if filt.as_ref().is_some_and(|q| !m.model.to_ascii_lowercase().contains(q.as_str())) {
                continue;
            }
            let key = if r.plugin == lodforge_instances::statics::NO_PLUGIN { (r.plugin, r.hash) } else { (r.plugin, r.form) };
            let e = refs.entry(key).or_insert((r.pos, BTreeMap::new()));
            e.1.entry(level).or_default().push(format!("{} ({} tris, flags {:#x})", m.model.rsplit('\\').next().unwrap_or(&m.model), m.triangles, m.flags));
        }
    }
    for ((plugin, form), (pos, levels)) in &refs {
        let name = f.common.plugins.get(*plugin as usize).map(String::as_str).unwrap_or("-");
        println!("{name}:{form:06X}  at {:.0} {:.0} {:.0}", pos[0], pos[1], pos[2]);
        for (l, m) in levels {
            println!("    LOD{l:<2} {}", m.join(" + "));
        }
    }
    println!("{} references", refs.len());
}
