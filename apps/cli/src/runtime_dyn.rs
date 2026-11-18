//! Maintenance of a `build --runtime lodforge` output without a full build: `lodforge
//! runtime-occlusion` (Occlusion.esp alone) and `lodforge runtime-retype` (tree items of the
//! `.dyn.bin` files against the current impostor sets, new build token), plus the impostor
//! fingerprint and type checks the build and `runtime-trees` share.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use lodforge_instances::WriteOptions;

use crate::profile::ProfileArgs;

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

/// `lodforge runtime-occlusion`: Occlusion.esp alone into an output folder, as the build's
/// occlusion stage writes it, against the profile's load order (use `--exclude-plugin` to leave
/// DynDOLOD.esm/.esp and an old Occlusion.esp out: then the plugin has no DynDOLOD master).
/// Object height maps are read from `--out`, terrain LOD through the profile.
#[derive(clap::Args)]
pub struct OcclusionArgs {
    #[command(flatten)]
    pub profile: ProfileArgs,
    /// The output folder (gets `Occlusion.esp`; holds `meshes\terrain\<World>\Objects\*.objects.heightmap.dat`).
    #[arg(long)]
    pub out: PathBuf,
    /// Worldspaces; default: every worldspace with object LOD files in `--out`.
    #[arg(long, value_delimiter = ',')]
    pub worlds: Vec<String>,
    #[arg(long, default_value = "High")]
    pub preset: String,
}

pub fn occlusion(a: &OcclusionArgs) -> Result<()> {
    let t0 = Instant::now();
    let mut p = a.profile.open()?;
    // Testing the build's occlusion lane: LODFORGE_OCC_OUT=1 reads `--out` as part of the load
    // order (as the build does with its own output); LODFORGE_OCC_LANE=1 also computes through
    // the lane first, as a build does.
    if std::env::var_os("LODFORGE_OCC_OUT").is_some() {
        p.vfs.add_loose_root(&a.out)?;
    }
    let dir = a.profile.instance.join("tools").join("DynDOLOD").join("Edit Scripts").join("DynDOLOD");
    let c = crate::dyndolod_build::Config::load(&p, &dir, &a.preset, false)?;
    let worlds: Vec<String> = if a.worlds.is_empty() {
        let terrain = a.out.join("meshes").join("terrain");
        let mut v: Vec<String> = std::fs::read_dir(&terrain).with_context(|| format!("listing {}", terrain.display()))?.flatten().filter(|e| e.path().join("Objects").is_dir()).filter_map(|e| e.file_name().to_str().map(str::to_string)).collect();
        v.sort();
        v
    } else {
        a.worlds.clone()
    };
    for n in ["DynDOLOD.esm", "DynDOLOD.esp"] {
        if p.load_order.plugins.iter().any(|lp| lp.plugin.name.eq_ignore_ascii_case(n)) {
            println!("warning: {n} is in the load order; Occlusion.esp may get it as a master (--exclude-plugin {n})");
        }
    }
    let mut pre = std::collections::HashMap::new();
    if std::env::var_os("LODFORGE_OCC_LANE").is_some() {
        let lane = crate::occlusion_lane::OcclusionLane::start(a.profile.clone(), a.out.clone(), worlds.clone());
        for w in &worlds {
            lane.send(w, crate::dyndolod_build::OcclusionInputs::new(&c, w));
        }
        pre = lane.finish();
    }
    let done = crate::dyndolod_build::write_occlusion_with(&p, &c, &worlds, &a.out, &mut pre)?;
    let cells: usize = done.iter().map(|d| d.1).sum();
    println!("Occlusion.esp: {cells} cells in {} worldspace(s) in {:.1} s", done.len(), t0.elapsed().as_secs_f64());
    Ok(())
}

/// Fingerprint of an impostor type list (`impostors.json` type names and model CRCs, in order):
/// what an item's impostor type index (`.dyn.bin` offset 46, `.trees.bin` type) refers to.
/// Recorded in a `.dyn.bin`'s `PARM` as `impostor_set=<hex>`.
pub fn impostor_fingerprint(set: &crate::impostors::SetReport) -> String {
    fingerprint_of(set.types.iter().map(|t| (t.name.as_str(), t.model_crc32)))
}

/// [`impostor_fingerprint`] of (type name, model CRC-32) pairs in order (`impostors.json` or a
/// `.trees.bin` `TTYP`).
pub fn fingerprint_of<'a>(types: impl Iterator<Item = (&'a str, u32)>) -> String {
    let mut s = String::new();
    for (name, crc) in types {
        s.push_str(&format!("{name}|{crc:08X}\n"));
    }
    format!("{:08X}", lodforge_instances::hash::fnv1a32(s.as_bytes()))
}

/// The impostor set of `world` under `root` (`textures\LODForge\Impostors\<World>\impostors.json`).
pub fn impostor_set(p: &crate::profile::Profile, root: &Path, world: &str) -> Option<crate::impostors::SetReport> {
    crate::runtime_trees::set_from_index(p, &root.join("textures").join("LODForge").join("Impostors").join(world).join("impostors.json")).ok()
}

fn param<'a>(params: &'a str, key: &str) -> Option<&'a str> {
    params.lines().find_map(|l| l.strip_prefix(key).and_then(|r| r.strip_prefix('=')))
}

fn set_param(params: &mut String, key: &str, value: &str) {
    let mut lines: Vec<String> = params.lines().filter(|l| !l.starts_with(&format!("{key}="))).map(str::to_string).collect();
    lines.push(format!("{key}={value}"));
    *params = lines.join("\n");
}

/// After the impostor type lists changed (`runtime-trees`): every `.dyn.bin` tree item gets the
/// type of its own base in the new set (offset 46, from `root`'s `impostors.json`), the files
/// record the new fingerprint, every runtime file in `out` is stamped with `token` and
/// `Build.txt` is rewritten. Returns (dyn files rewritten, items retyped, files stamped).
pub fn retype_and_restamp(p: &crate::profile::Profile, root: &Path, out: &Path, token: &str) -> Result<(usize, usize, usize)> {
    use lodforge_instances::dynlod::{DynFile, TREE};
    let dir = crate::instances::out_dir(out);
    let mut sets: HashMap<String, Option<(String, crate::impostors::SetReport)>> = HashMap::new();
    let (mut files, mut retyped, mut stamped) = (0, 0, 0);
    let mut dyn_files: Vec<PathBuf> = std::fs::read_dir(&dir)?.flatten().map(|e| e.path()).filter(|q| q.to_string_lossy().to_ascii_lowercase().ends_with(".dyn.bin")).collect();
    dyn_files.sort();
    for f in &dyn_files {
        let bytes = std::fs::read(f)?;
        let mut d = DynFile::from_bytes(&bytes).with_context(|| format!("reading {}", f.display()))?;
        let world = d.header.lod_world.clone();
        let entry = sets.entry(world.clone()).or_insert_with(|| impostor_set(p, root, &world).map(|s| (impostor_fingerprint(&s), s)));
        let mut changed = false;
        if let Some((fp, set)) = entry.as_ref() {
            for it in d.cells.values_mut().flatten().filter(|it| it.flags & TREE != 0) {
                let base = d.common.plugins.get(usize::from(it.base.plugin)).and_then(|n| lodforge_dyndolod::bases::global_id(&p.load_order, n, it.base.form));
                let t = base.and_then(|b| set.base_index.get(&b)).map_or(0, |&t| t as u16 + 1);
                if t != it.impostor {
                    it.impostor = t;
                    retyped += 1;
                    changed = true;
                }
            }
            if d.cells.values().flatten().any(|it| it.flags & TREE != 0) && param(&d.common.params, "impostor_set") != Some(fp.as_str()) {
                set_param(&mut d.common.params, "impostor_set", fp);
                changed = true;
            }
        }
        if changed || d.common.token != token {
            d.common.token = token.to_string();
            let opts = WriteOptions { zstd: has_zstd(&bytes).then_some(3), crc: true };
            std::fs::write(f, d.to_bytes(opts)?)?;
            files += 1;
        }
    }
    for e in std::fs::read_dir(&dir)?.flatten() {
        let n = e.file_name().to_string_lossy().to_ascii_lowercase();
        if (n.ends_with(".bin") || n.ends_with(".lfg")) && !n.ends_with(".dyn.bin") && stamp(&e.path(), token)? {
            stamped += 1;
        }
    }
    crate::instances::write_build_txt_with(out, token)?;
    Ok((files, retyped, stamped))
}

/// `runtime-check --types`: every `.dyn.bin` tree item's impostor type (offset 46) names a type
/// of its `lod_world`'s current `impostors.json` whose base list holds the item's base, and the
/// file's `impostor_set` fingerprint is the current one. Returns the number of problems.
pub fn check_types(root: &Path, dir: &Path) -> Result<usize> {
    use lodforge_instances::dynlod::{DynFile, TREE};
    let mut problems = 0;
    let mut sets: HashMap<String, Option<(String, lodforge_impostor::manifest::Index)>> = HashMap::new();
    let mut names: Vec<PathBuf> = std::fs::read_dir(dir)?.flatten().map(|e| e.path()).filter(|q| q.to_string_lossy().to_ascii_lowercase().ends_with(".dyn.bin")).collect();
    names.sort();
    for f in names {
        let d = DynFile::from_bytes(&std::fs::read(&f)?)?;
        let world = d.header.lod_world.clone();
        let entry = sets.entry(world.clone()).or_insert_with(|| {
            let path = root.join("textures").join("LODForge").join("Impostors").join(&world).join("impostors.json");
            let index: lodforge_impostor::manifest::Index = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
            let fp = fingerprint_of(index.types.iter().map(|t| (t.name.as_str(), u32::from_str_radix(&t.model_crc32, 16).unwrap_or(0))));
            Some((fp, index))
        });
        let trees: Vec<_> = d.cells.values().flatten().filter(|it| it.flags & TREE != 0).collect();
        let name = f.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
        if trees.is_empty() {
            continue;
        }
        let Some((fp, index)) = entry.as_ref() else {
            println!("{name}: {} tree items but no impostor set for {world}", trees.len());
            problems += 1;
            continue;
        };
        let recorded = param(&d.common.params, "impostor_set").unwrap_or("none");
        if recorded != fp {
            println!("{name}: built against impostor set {recorded}, current {world} set is {fp}");
            problems += 1;
        }
        let (mut bad, mut none) = (0usize, 0usize);
        let mut examples = Vec::new();
        for it in &trees {
            let plugin = d.common.plugins.get(usize::from(it.base.plugin)).map(|s| s.to_ascii_lowercase()).unwrap_or_default();
            if it.impostor == 0 {
                none += 1;
                continue;
            }
            let ok = index.types.get(usize::from(it.impostor - 1)).is_some_and(|t| t.bases.iter().any(|b| b.plugin.eq_ignore_ascii_case(&plugin) && u32::from_str_radix(&b.form_id, 16).is_ok_and(|v| v & 0x00FF_FFFF == it.base.form)));
            if !ok {
                bad += 1;
                if examples.len() < 4 {
                    let tname = index.types.get(usize::from(it.impostor - 1)).map_or("<out of range>", |t| t.name.as_str());
                    examples.push(format!("{}:{:06X} base {plugin}:{:06X} -> {tname}", d.common.plugins[usize::from(it.original.plugin)], it.original.form, it.base.form));
                }
            }
        }
        println!("{name}: {} tree items, {bad} with an impostor type that does not hold their base, {none} without a type{}", trees.len(), if examples.is_empty() { String::new() } else { format!("; e.g. {}", examples.join(", ")) });
        problems += bad;
    }
    Ok(problems)
}

/// `lodforge runtime-retype`: the dependants step of an impostor type list change on its own:
/// `.dyn.bin` tree items retyped from their bases against the current `impostors.json`, a new
/// token stamped into every runtime file, `Build.txt` rewritten.
#[derive(clap::Args)]
pub struct RetypeArgs {
    #[command(flatten)]
    pub profile: ProfileArgs,
    /// Output folder (Data or MO2 mod folder) whose runtime files are updated in place.
    #[arg(long)]
    pub out: PathBuf,
    /// Folder with `textures\LODForge\Impostors` (default: `--out`).
    #[arg(long)]
    pub impostors: Option<PathBuf>,
}

pub fn retype(a: &RetypeArgs) -> Result<()> {
    let p = a.profile.open()?;
    let token = crate::instances::new_token();
    let root = a.impostors.clone().unwrap_or_else(|| a.out.clone());
    let (files, retyped, stamped) = retype_and_restamp(&p, &root, &a.out, &token)?;
    println!("runtime-retype: new token {token}; {files} .dyn.bin rewritten ({retyped} tree items retyped), {stamped} other files stamped");
    Ok(())
}
