//! `lodforge runtime-check`: consistency checks over the runtime files of an output.

use std::path::PathBuf;

use anyhow::Result;

use lodforge_instances::statics::StaticsFile;

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
        anyhow::ensure!(n == 0, "{n} stale impostor type reference(s); run runtime-retype (or runtime-trees, which retypes too)");
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
    let mut synthetic_total = (0usize, 0usize);
    for path in names {
        let st = StaticsFile::from_bytes(&std::fs::read(&path)?)?;
        // Synthetic instances (plugin 0xFFFF): the runtime's per-reference large-reference
        // hand-off cannot follow those on large-reference meshes (child-world copies without a
        // `_DynDOLOD_CHILD` reference in the output's DynDOLOD.esm).
        let (synthetic, synthetic_large) = crate::instances::synthetic_counts(&st);
        synthetic_total.0 += synthetic;
        synthetic_total.1 += synthetic_large;
        if synthetic > 0 {
            println!("{}: {synthetic} synthetic instances, {synthetic_large} of them on large-reference meshes", path.file_name().and_then(|n| n.to_str()).unwrap_or_default());
        }
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
    println!("runtime-check: {bad_total} instances without a pack mesh; {} synthetic instances, {} of them on large-reference meshes", synthetic_total.0, synthetic_total.1);
    Ok(())
}
