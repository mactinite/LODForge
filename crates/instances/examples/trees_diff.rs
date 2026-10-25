//! Compares two `.trees.bin` files per reference (plugin + form): type name changes, added and
//! removed references, and pack-mesh model changes.
use std::collections::{BTreeMap, HashMap};

use lodforge_instances::trees::TreeFile;

fn by_ref(f: &TreeFile) -> HashMap<(String, u32), (String, String)> {
    let mut m = HashMap::new();
    for recs in f.cells.values() {
        for r in recs {
            let t = usize::from(r.type_);
            let mesh = f.meshes.get(t).map(|m| m.model.clone()).unwrap_or_default();
            m.insert((f.common.plugins[usize::from(r.plugin)].to_ascii_lowercase(), r.form), (f.types[t].name.clone(), mesh));
        }
    }
    m
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let x = by_ref(&TreeFile::from_bytes(&std::fs::read(&a[1]).unwrap()).unwrap());
    let y = by_ref(&TreeFile::from_bytes(&std::fs::read(&a[2]).unwrap()).unwrap());
    let mut changes: BTreeMap<String, usize> = BTreeMap::new();
    let (mut same, mut added, mut removed) = (0, 0, 0);
    for (k, v) in &x {
        match y.get(k) {
            Some(w) if w == v => same += 1,
            Some(w) => *changes.entry(format!("{} -> {}{}", v.0, w.0, if v.1 != w.1 { " (mesh changed)" } else { "" })).or_default() += 1,
            None => {
                removed += 1;
                *changes.entry(format!("{} -> (removed)", v.0)).or_default() += 1;
            }
        }
    }
    added += y.keys().filter(|k| !x.contains_key(*k)).count();
    println!("same {same}, changed+removed {}, added {added}, removed {removed}", changes.values().sum::<usize>());
    for (c, n) in changes {
        println!("  {n:6} {c}");
    }
}
