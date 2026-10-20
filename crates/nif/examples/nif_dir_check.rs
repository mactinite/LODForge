//! Block-type inventory and parse -> write round trip over every NIF-like file in a folder.
//!
//! `cargo run --release -p lodforge-nif --example nif_dir_check -- <dir> [ext]` (default ext `bto`).

use std::collections::BTreeMap;

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = args.next().expect("usage: nif_dir_check <dir> [ext]");
    let ext = args.next().unwrap_or_else(|| "bto".into());
    let mut files: Vec<_> = std::fs::read_dir(&dir).unwrap().filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case(&ext))).collect();
    files.sort();
    let (mut types, mut raw_types) = (BTreeMap::<String, usize>::new(), BTreeMap::<String, usize>::new());
    let (mut identical, mut errors) = (0usize, Vec::new());
    for p in &files {
        let bytes = std::fs::read(p).unwrap();
        let nif = match lodforge_nif::parse(&bytes) {
            Ok(n) => n,
            Err(e) => {
                errors.push(format!("{}: {e}", p.display()));
                continue;
            }
        };
        for i in 0..nif.blocks.len() {
            *types.entry(nif.block_type(i).to_string()).or_default() += 1;
        }
        for &i in nif.raw_blocks.keys() {
            *raw_types.entry(nif.block_type(i).to_string()).or_default() += 1;
        }
        match nif.to_bytes() {
            Ok(out) if out == bytes => identical += 1,
            Ok(_) => errors.push(format!("{}: written bytes differ", p.display())),
            Err(e) => errors.push(format!("{}: write: {e}", p.display())),
        }
    }
    println!("{} files, {identical} round-trip byte-identical", files.len());
    println!("block types: {types:?}");
    println!("raw (not re-encoded): {raw_types:?}");
    for e in errors.iter().take(10) {
        println!("  {e}");
    }
}
