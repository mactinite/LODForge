//! `lodforge runtime-dyn`: brings an earlier `build --runtime lodforge` output up to format 1.7
//! without a full build. From a fresh DynDOLOD-mode export it writes `<World>.dyn.bin` per
//! worldspace, rebuilds `StaticMeshes.bin` (+ static atlas pages) so the dynamic LOD models have
//! pack meshes, stamps the build token (`BTOK`) into every runtime file and writes `Build.txt`.
//! No texgen, object LOD, impostors or occlusion run; `.bto` files are not touched, so large
//! references keep whatever routing the earlier build gave them (`DHDR` flag off).
//!
//! The profile must show the earlier output (its generated LOD models decide the export's LOD
//! columns, as in the build). With `--out` the changed files go to a staging folder (the runtime
//! files and the static atlas pages), to be copied over the output.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use lodforge_instances::WriteOptions;

use crate::profile::ProfileArgs;

#[derive(clap::Args)]
pub struct Args {
    #[command(flatten)]
    pub profile: ProfileArgs,
    /// The earlier build output (Data folder or MO2 mod folder).
    #[arg(long)]
    pub data: PathBuf,
    /// Where to write the changed files (default: `--data`, in place).
    #[arg(long)]
    pub out: Option<PathBuf>,
    /// Worldspaces; default: every worldspace with a `.statics.bin` or `.trees.bin` in `--data`.
    #[arg(long, value_delimiter = ',')]
    pub worlds: Vec<String>,
    #[arg(long, value_delimiter = ',', default_value = "SPR,SUM,AUT,WIN")]
    pub seasons: Vec<String>,
    #[arg(long, default_value = "High")]
    pub preset: String,
    #[arg(long, default_value = "Low")]
    pub child_parent: String,
    /// The earlier build ran without DynDOLOD's large-reference workarounds.
    #[arg(long)]
    pub no_large_ref_workarounds: bool,
}

fn has_zstd(bytes: &[u8]) -> bool {
    bytes.len() >= 16 && u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]) & 1 != 0
}

/// Rewrites one runtime file with `token` in its `BTOK` (keeping its compression). Returns false
/// when it already had it.
fn stamp(path: &Path, token: &str) -> Result<bool> {
    use lodforge_instances::{grass::GrassFile, meshpack::MeshPack, statics::StaticsFile, trees::TreeFile};
    let bytes = std::fs::read(path)?;
    let opts = WriteOptions { zstd: has_zstd(&bytes).then_some(3), crc: true };
    let out = match bytes.get(..4) {
        Some(b"LFTR") => {
            let mut f = TreeFile::from_bytes(&bytes)?;
            if f.common.token == token {
                return Ok(false);
            }
            f.common.token = token.into();
            f.to_bytes(opts)?
        }
        Some(b"LFST") => {
            let mut f = StaticsFile::from_bytes(&bytes)?;
            if f.common.token == token {
                return Ok(false);
            }
            f.common.token = token.into();
            f.to_bytes(opts)?
        }
        Some(b"LFGR") => {
            let mut f = GrassFile::from_bytes(&bytes)?;
            if f.common.token == token {
                return Ok(false);
            }
            f.common.token = token.into();
            f.to_bytes(opts)?
        }
        Some(b"LFMP") => {
            let mut f = MeshPack::from_bytes(&bytes)?;
            if f.common.token == token {
                return Ok(false);
            }
            f.common.token = token.into();
            f.to_bytes(opts)?
        }
        _ => return Ok(false),
    };
    std::fs::write(path, out)?;
    Ok(true)
}

pub fn run(a: &Args) -> Result<()> {
    let t0 = Instant::now();
    let p = a.profile.open()?;
    let out = a.out.clone().unwrap_or_else(|| a.data.clone());
    let src_dir = crate::instances::out_dir(&a.data);
    let dst_dir = crate::instances::out_dir(&out);
    std::fs::create_dir_all(&dst_dir)?;
    // The staging folder starts from the earlier runtime files.
    if dst_dir != src_dir {
        for e in std::fs::read_dir(&src_dir).with_context(|| format!("listing {}", src_dir.display()))?.flatten() {
            let n = e.file_name().to_string_lossy().to_ascii_lowercase();
            if e.path().is_file() && !n.ends_with(".dyn.bin") && n != crate::instances::BUILD_TXT.to_ascii_lowercase() {
                std::fs::copy(e.path(), dst_dir.join(e.file_name()))?;
            }
        }
    }
    let token = crate::instances::init_build_token(&out);
    let worlds: Vec<String> = if a.worlds.is_empty() {
        let mut v: Vec<String> = std::fs::read_dir(&src_dir)?
            .flatten()
            .filter_map(|e| e.file_name().to_str().and_then(|n| n.strip_suffix(".statics.bin").or_else(|| n.strip_suffix(".trees.bin")).map(str::to_string)))
            .filter(|n| !n.contains('.'))
            .collect();
        v.sort();
        v.dedup();
        v
    } else {
        a.worlds.clone()
    };
    let dir = a.profile.instance.join("tools").join("DynDOLOD").join("Edit Scripts").join("DynDOLOD");
    let mut c = crate::dyndolod_build::Config::load(&p, &dir, &a.preset, false)?;
    c.ultra = true;
    let all = lodforge_esp::scan_worldspaces(&p.load_order)?;
    let seasons: Vec<lodforge_seasons::Season> = a.seasons.iter().filter_map(|s| crate::object_export::season_from_code(s)).collect();
    let names = crate::instances::FormNames::new(&p);
    let write = WriteOptions { zstd: Some(3), crc: true };
    let mut exports = Vec::new();
    let mut large_columns: HashMap<String, HashMap<u32, u16>> = HashMap::new();
    let mut undersides = HashSet::new();
    for world in &worlds {
        let Some(ws) = all.iter().find(|w| w.editor_id.eq_ignore_ascii_case(world)) else {
            println!("{world}: not in the load order, skipped");
            continue;
        };
        let t = Instant::now();
        let h = crate::dyndolod_build::header(&p, &c, ws, "", &a.profile.game_dir().join("Data"), "");
        let we = crate::dyndolod_build::with_resolver(&p, &c, |r| -> Result<_> {
            let copies = crate::child_copies::for_world(&p, &c, r, &all, ws, &a.child_parent);
            crate::dyndolod_build::export_world(&p, &c, r, ws, &seasons, &h, &copies)
        })?;
        let lc = large_columns.entry(ws.editor_id.clone()).or_default();
        for l in &we.seasons[0].lines {
            if ws.large_ref_cells.contains_key(&l.ref_id) {
                let bits = (0..3).filter(|&i| !l.columns[i].is_empty()).fold(0u16, |b, i| b | (1 << i));
                *lc.entry(l.ref_id).or_default() |= bits;
            }
        }
        let e = &ws.editor_id;
        if a.data.join("meshes").join("terrain").join(e).join(format!("{e}_Underside.nif")).is_file() {
            undersides.insert(e.clone());
        }
        println!("{world}: export {} dynamic, {} child copies in {:.1} s", we.dynamic.len(), we.child_copies.len(), t.elapsed().as_secs_f64());
        exports.push((ws.editor_id.clone(), we));
    }
    let inputs: Vec<crate::dyn_export::DynWorld<'_>> = exports
        .iter()
        .filter_map(|(w, we)| {
            all.iter().find(|x| &x.editor_id == w).map(|world| crate::dyn_export::DynWorld { world, dynamic: &we.dynamic, seasonal: &we.seasonal_dynamic, child_copies: &we.child_copies, lod_refs: &we.lod_refs, lod_bases: &we.lod_bases, large_columns: large_columns.get(w) })
        })
        .collect();
    let workarounds = !a.no_large_ref_workarounds && c.num("WorkaroundLargeReferencesBugs", 1) == 1 && c.num("IgnoreLargeReferences", 0) == 0;
    let rep = crate::dyn_export::write(&p, &c, &inputs, &all, &undersides, &names, &out, write, crate::dyn_export::WriteFlags { large_refs_instanced: false, workarounds })?;
    println!("{}", rep.summary());
    // The static pack with the dynamic LOD models (reads the VFS: the profile shows the output).
    let t = Instant::now();
    let s = crate::tree_pack::build_statics(&p, &out, &crate::tree_pack::Settings::default(), write)?;
    println!("static {} ({:.1} s)", s.summary(), t.elapsed().as_secs_f64());
    let mut stamped = 0;
    for e in std::fs::read_dir(&dst_dir)?.flatten() {
        if e.path().extension().is_some_and(|x| x.eq_ignore_ascii_case("bin") || x.eq_ignore_ascii_case("lfg")) && stamp(&e.path(), &token)? {
            stamped += 1;
        }
    }
    crate::instances::write_build_txt(&out)?;
    println!("token {token}: {stamped} files stamped; Build.txt written; runtime-dyn finished in {:.1} s", t0.elapsed().as_secs_f64());
    Ok(())
}
