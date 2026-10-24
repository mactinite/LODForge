//! `lodforge runtime-strip-statics`: removes from `.statics.bin` files every instance whose
//! reference is a record of the matching `.trees.bin` (same world and season). Since format 1.2
//! the trees file's `TMSH` mesh is the near tier of those trees, so a statics file from an older
//! build would draw them twice.

use std::collections::HashSet;
use std::path::PathBuf;

use anyhow::{Context, Result};

use lodforge_instances::statics::StaticsFile;
use lodforge_instances::trees::TreeFile;

#[derive(clap::Args)]
pub struct Args {
    /// Folder with the `.trees.bin` files.
    #[arg(long)]
    pub trees: PathBuf,
    /// Folder with the `.statics.bin` files to clean.
    #[arg(long)]
    pub statics: PathBuf,
    /// Where to write the cleaned statics files.
    #[arg(long)]
    pub out: PathBuf,
    #[arg(long, default_value_t = 3)]
    pub zstd: i32,
}

pub fn run(a: &Args) -> Result<()> {
    std::fs::create_dir_all(&a.out)?;
    let write = lodforge_instances::WriteOptions { zstd: (a.zstd > 0).then_some(a.zstd), crc: true };
    let mut names: Vec<PathBuf> = std::fs::read_dir(&a.statics).with_context(|| format!("listing {}", a.statics.display()))?.flatten().map(|e| e.path()).filter(|p| p.to_string_lossy().to_ascii_lowercase().ends_with(".statics.bin")).collect();
    names.sort();
    let (mut kept_total, mut dropped_total) = (0usize, 0usize);
    for path in names {
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
        let stem = &file_name[..file_name.len() - ".statics.bin".len()];
        let trees_path = a.trees.join(format!("{stem}.trees.bin"));
        let mut st = StaticsFile::from_bytes(&std::fs::read(&path)?).with_context(|| format!("reading {}", path.display()))?;
        let refs: HashSet<(String, u32)> = match std::fs::read(&trees_path) {
            Ok(b) => {
                let t = TreeFile::from_bytes(&b).with_context(|| format!("reading {}", trees_path.display()))?;
                t.cells.values().flatten().filter_map(|r| t.common.plugins.get(usize::from(r.plugin)).map(|n| (n.to_ascii_lowercase(), r.form))).collect()
            }
            Err(_) => HashSet::new(),
        };
        let before = st.records();
        let plugins = st.common.plugins.clone();
        for recs in st.blocks.values_mut() {
            recs.retain(|r| !plugins.get(usize::from(r.plugin)).is_some_and(|n| refs.contains(&(n.to_ascii_lowercase(), r.form))));
        }
        st.blocks.retain(|_, v| !v.is_empty());
        let after = st.records();
        st.common.params.push_str("\ntree_refs=0");
        std::fs::write(a.out.join(&file_name), st.to_bytes(write)?)?;
        println!("  {file_name}: {before} -> {after} instances");
        kept_total += after;
        dropped_total += before - after;
    }
    println!("runtime-strip-statics: {dropped_total} tree instances removed, {kept_total} kept");
    Ok(())
}
