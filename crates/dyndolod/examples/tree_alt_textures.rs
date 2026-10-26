//! Lists tree bases (TREE, or STAT with the has-tree-LOD flag) of an MO2 profile that have MODS
//! alternate textures, and models shared by bases whose alternate textures differ (variants a
//! per-model tree type would merge). Usage: `tree_alt_textures <instance> <profile>`.
use std::collections::BTreeMap;
use std::path::Path;

use lodforge_archive::{Mo2Layout, Vfs};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let instance = Path::new(&a[1]);
    let ini = std::fs::read_to_string(instance.join("ModOrganizer.ini")).unwrap_or_default();
    let game = ini.lines().find_map(|l| l.strip_prefix("gamePath=")).map(|s| s.trim_start_matches("@ByteArray(").trim_end_matches(')').replace("\\\\", "\\")).expect("gamePath");
    let layout = Mo2Layout::portable(instance, &a[2], &Path::new(&game).join("Data"));
    let vfs = Vfs::mo2(&layout).expect("vfs");
    let names = lodforge_esp::mo2_active_plugins(&layout.profile_dir, Some(&Path::new(&game).join("Skyrim.ccc"))).expect("plugins");
    let names: Vec<String> = names.into_iter().filter(|n| vfs.loose_path(n).is_some() && !n.to_ascii_lowercase().starts_with("dyndolod.")).collect();
    let lo = lodforge_esp::LoadOrder::open(&names, |n| vfs.loose_path(n).map(|p| p.to_owned())).expect("load order");
    let bases = lodforge_dyndolod::bases::read_bases(&lo).expect("bases");
    let mut by_model: BTreeMap<String, Vec<&lodforge_dyndolod::bases::Base>> = BTreeMap::new();
    for b in bases.values().filter(|b| lodforge_dyndolod::assign::is_tree(b)) {
        by_model.entry(b.model.to_ascii_lowercase()).or_default().push(b);
    }
    let mut with = 0;
    for (m, bs) in &by_model {
        let alts: Vec<_> = bs.iter().filter(|b| !b.alt_textures.is_empty()).collect();
        with += alts.len();
        for b in &alts {
            println!("MODS {:08X} {} {m} {:?}", b.form_id, b.editor_id, b.alt_textures);
        }
        let mut sets: Vec<String> = bs.iter().map(|b| format!("{:?}", b.alt_textures)).collect();
        sets.sort();
        sets.dedup();
        if sets.len() > 1 {
            println!("MERGED {m}: {} bases, {} texture variants", bs.len(), sets.len());
        }
    }
    println!("{} tree bases, {} models, {with} with MODS", by_model.values().map(Vec::len).sum::<usize>(), by_model.len());
}
