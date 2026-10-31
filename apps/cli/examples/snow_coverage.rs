//! Snow-class statics entries by treatment: baked with the base's snow material, baked without
//! one (base material not found), and not baked (old LOD snow), counted per LOD4 reference.
//! `snow_coverage <LODForgeRuntime dir>`
use std::collections::BTreeMap;

fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).unwrap());
    let snowy = |c: &u8| matches!(*c, lodforge_instances::meshpack::CLASS_SNOW | lodforge_instances::meshpack::CLASS_SNOW_HD);
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut unbaked: BTreeMap<String, usize> = BTreeMap::new();
    let mut nosnow: BTreeMap<String, usize> = BTreeMap::new();
    for e in std::fs::read_dir(&dir).unwrap().flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if !name.starts_with("Tamriel.") || !name.ends_with(".SUM.statics.bin") {
            continue;
        }
        let f = lodforge_instances::statics::StaticsFile::from_bytes(&std::fs::read(e.path()).unwrap()).unwrap();
        for ((level, _, _), recs) in &f.blocks {
            if *level != 4 {
                continue;
            }
            for r in recs {
                let m = &f.meshes[usize::from(r.mesh)];
                if !m.classes.iter().any(snowy) {
                    continue;
                }
                let baked = m.model.to_ascii_lowercase().contains(r"\lodforge\baked\");
                let k = match (baked, m.snow.is_some()) {
                    (true, true) => "baked, own snow",
                    (true, false) => "baked, no snow material",
                    (false, true) => "not baked, has a snow material",
                    (false, false) => "not baked, no snow material",
                };
                *counts.entry(k).or_default() += 1;
                if !baked {
                    *unbaked.entry(m.model.to_ascii_lowercase()).or_default() += 1;
                } else if m.snow.is_none() {
                    *nosnow.entry(m.model.to_ascii_lowercase()).or_default() += 1;
                }
            }
        }
    }
    println!("Tamriel SUM LOD4 references with snow parts: {counts:?}");
    let top = |t: &str, m: &BTreeMap<String, usize>| {
        let mut v: Vec<_> = m.iter().collect();
        v.sort_by(|a, b| b.1.cmp(a.1));
        println!("{t}:");
        for (k, n) in v.iter().take(25) {
            println!("  {n:>6} {k}");
        }
    };
    top("not baked, most used", &unbaked);
    top("baked without a snow material, most used", &nosnow);
}
