//! Diagnostics: for the given global form ids, the plugin at their mod index and every plugin
//! that carries a record for them in the worldspace scan.
//! `cargo run --release -p lodforge-treelod --example tree_refs -- <MO2 instance> <hex id>...`

use std::path::Path;

use lodforge_archive::{Mo2Layout, Vfs};
use lodforge_esp::{mo2_active_plugins, LoadOrder, ModIndex};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let instance = Path::new(&args[0]);
    let game = instance.join("Stock Game");
    let layout = Mo2Layout::portable(instance, "Default", &game.join("Data"));
    let vfs = Vfs::mo2(&layout).expect("vfs");
    let names = mo2_active_plugins(&layout.profile_dir, Some(&game.join("Skyrim.ccc"))).expect("load order");
    let lo = LoadOrder::open(&names, |n| vfs.loose_path(n).map(|p| p.to_owned())).expect("plugins");
    for h in &args[1..] {
        let id = u32::from_str_radix(h, 16).expect("hex");
        let owner = lo.plugins.iter().find(|p| match p.index {
            ModIndex::Full(i) => id >> 24 == u32::from(i),
            ModIndex::Light(i) => id >> 24 == 0xFE && (id >> 12) & 0xFFF == u32::from(i),
        });
        println!("{id:08X}: owner {:?}", owner.map(|p| (&p.plugin.name, p.plugin.header.masters.len())));
    }
}
