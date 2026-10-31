//! Which references lose (or gain) object LOD between two runtime outputs: per world (summer)
//! and LOD level, the references (plugin, form) that have a statics record in `a` but none in `b`,
//! grouped by `a`'s model, and the counts both ways.
//! `statics_diff <runtime dir a> <runtime dir b> [world] [count]`
use std::collections::{BTreeMap, HashMap, HashSet};

type Refs = HashMap<(u8, String, u32), String>;

fn load(dir: &std::path::Path, world: &str) -> Refs {
    let mut out = Refs::new();
    for e in std::fs::read_dir(dir).unwrap().flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if !name.starts_with(&format!("{world}.")) || !name.ends_with(".SUM.statics.bin") {
            continue;
        }
        let f = lodforge_instances::statics::StaticsFile::from_bytes(&std::fs::read(e.path()).unwrap()).unwrap();
        for ((level, _, _), recs) in &f.blocks {
            for r in recs {
                let Some(plugin) = f.common.plugins.get(usize::from(r.plugin)) else { continue };
                let model = f.meshes[usize::from(r.mesh)].model.to_ascii_lowercase();
                out.insert((*level, plugin.to_ascii_lowercase(), r.form), model);
            }
        }
    }
    out
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let world = a.get(3).cloned().unwrap_or_else(|| "Tamriel".into());
    let count: usize = a.get(4).and_then(|s| s.parse().ok()).unwrap_or(25);
    let (ra, rb) = (load(std::path::Path::new(&a[1]), &world), load(std::path::Path::new(&a[2]), &world));
    for level in [4u8, 8, 16] {
        let ka: HashSet<_> = ra.keys().filter(|k| k.0 == level).map(|k| (k.1.clone(), k.2)).collect();
        let kb: HashSet<_> = rb.keys().filter(|k| k.0 == level).map(|k| (k.1.clone(), k.2)).collect();
        let lost: Vec<_> = ka.difference(&kb).collect();
        let gained = kb.difference(&ka).count();
        println!("{world} LOD{level}: a {} refs, b {} refs; {} only in a (lose LOD), {} only in b (gain LOD)", ka.len(), kb.len(), lost.len(), gained);
        let mut by_model: BTreeMap<String, usize> = BTreeMap::new();
        for (p, f) in &lost {
            *by_model.entry(ra[&(level, p.clone(), *f)].clone()).or_default() += 1;
        }
        let mut v: Vec<_> = by_model.into_iter().collect();
        v.sort_by(|x, y| y.1.cmp(&x.1));
        for (m, n) in v.iter().take(count) {
            println!("  {n:>6} {m}");
        }
    }
}
