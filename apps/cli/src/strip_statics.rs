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

/// `lodforge runtime-check`: statics `MESH` entries without a pack mesh (`SMPK` = none), whose
/// instances would draw nothing although they left `.bto`, with their instance counts.
#[derive(clap::Args)]
pub struct CheckArgs {
    /// Folder with the `.statics.bin` files.
    #[arg(long)]
    pub dir: PathBuf,
    /// Also print the default-season LOD4 instances whose model path contains this, as
    /// `ref_coverage` reference lines (`<model> x y z rx ry rz scale`, degrees).
    #[arg(long)]
    pub dump: Option<String>,
    /// Folder the dumped model paths are relative to (prefixed to them).
    #[arg(long, default_value = "")]
    pub model_root: String,
    /// Check the `.dyn.bin` tree items' impostor types (offset 46) against the current
    /// `impostors.json` of their LOD world (base lists and the `impostor_set` fingerprint);
    /// fails when one is stale.
    #[arg(long)]
    pub types: bool,
    /// Data folder with `textures\LODForge\Impostors` (default: three levels above `--dir`).
    #[arg(long)]
    pub data: Option<PathBuf>,
}

pub fn check(a: &CheckArgs) -> Result<()> {
    if a.types {
        let root = a.data.clone().unwrap_or_else(|| a.dir.ancestors().nth(3).map(std::path::Path::to_path_buf).unwrap_or_default());
        let n = crate::runtime_dyn::check_types(&root, &a.dir)?;
        println!("runtime-check --types: {n} problem(s)");
        anyhow::ensure!(n == 0, "{n} stale impostor type reference(s); run runtime-dyn (or runtime-trees, which now retypes)");
        return Ok(());
    }
    if let Some(filter) = &a.dump {
        let f = filter.to_ascii_lowercase();
        for path in std::fs::read_dir(&a.dir)?.flatten().map(|e| e.path()) {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
            if !name.ends_with(".statics.bin") || name.matches('.').count() != 2 {
                continue;
            }
            let st = StaticsFile::from_bytes(&std::fs::read(&path)?)?;
            for ((level, _, _), recs) in &st.blocks {
                if *level != 4 {
                    continue;
                }
                for r in recs {
                    let m = &st.meshes[usize::from(r.mesh)].model;
                    if m.contains(&f) {
                        let d = r.angles_rad().map(f32::to_degrees);
                        println!("{}{m} {} {} {} {} {} {} {}", a.model_root, r.pos[0], r.pos[1], r.pos[2], d[0], d[1], d[2], r.scale.to_f32());
                    }
                }
            }
        }
        return Ok(());
    }
    let mut names: Vec<PathBuf> = std::fs::read_dir(&a.dir)?.flatten().map(|e| e.path()).filter(|p| p.to_string_lossy().to_ascii_lowercase().ends_with(".statics.bin")).collect();
    names.sort();
    let mut bad_total = 0usize;
    for path in names {
        let st = StaticsFile::from_bytes(&std::fs::read(&path)?)?;
        let mut per: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
        for recs in st.blocks.values() {
            for r in recs {
                let m = usize::from(r.mesh);
                let none = st.pack_meshes.get(m).is_none_or(|&p| p == lodforge_instances::meshpack::NO_MESH);
                if none {
                    *per.entry(st.meshes[m].model.clone()).or_default() += 1;
                }
            }
        }
        if !per.is_empty() {
            let n: usize = per.values().sum();
            bad_total += n;
            println!("{}: {n} instances without a pack mesh: {}", path.file_name().and_then(|n| n.to_str()).unwrap_or_default(), per.iter().map(|(m, c)| format!("{m} x{c}")).collect::<Vec<_>>().join(", "));
        }
    }
    println!("runtime-check: {bad_total} instances without a pack mesh");
    Ok(())
}
