//! `lodforge lod-sources`: where the LOD models of a DynDOLOD-mode build come from. Builds the
//! worldspace's DynDOLOD export and dynamic LOD selection the way `build --mode dyndolod` does and
//! classifies every LOD model (and the textures of the models a given mod provides) by origin:
//! the "resources" mod (default `DynDOLOD Resources SE 3`), vanilla archives, other mods' loose
//! files or archives. Used to size the work of replacing DynDOLOD Resources with generated models
//! (docs/re/own-lod-resources.md).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;

use anyhow::{Context, Result};
use rayon::prelude::*;

use lodforge_archive::Origin;

use crate::profile::{Profile, ProfileArgs};

#[derive(clap::Args)]
pub struct Args {
    #[command(flatten)]
    pub profile: ProfileArgs,
    #[arg(long, default_value = "Tamriel")]
    pub world: String,
    /// DynDOLOD's `Edit Scripts\DynDOLOD` folder (default: `<instance>\tools\DynDOLOD\Edit Scripts\DynDOLOD`).
    #[arg(long)]
    pub dyndolod_config: Option<PathBuf>,
    #[arg(long, default_value = "Medium")]
    pub preset: String,
    /// The mod whose files count as "resources".
    #[arg(long, default_value = "DynDOLOD Resources SE 3")]
    pub resources_mod: String,
    /// Write every LOD model with its origin, uses and triangle count as JSON.
    #[arg(long)]
    pub json: Option<PathBuf>,
}

/// Where a file comes from.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]
pub enum Group {
    Resources,
    Vanilla,
    Mod,
    Missing,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Source {
    pub group: Group,
    /// Mod folder or archive file name.
    pub from: String,
}

/// Vanilla archives: the game's own BSAs (base game, DLC, Creation Club).
fn vanilla_archive(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.starts_with("skyrim - ") || n.starts_with("cc") || ["update.bsa", "dawnguard.bsa", "hearthfires.bsa", "dragonborn.bsa", "_resourcepack.bsa"].contains(&n.as_str())
}

pub fn source(p: &Profile, mods_dir: &std::path::Path, resources_mod: &str, path: &str) -> Source {
    match p.vfs.origin(path) {
        None => Source { group: Group::Missing, from: String::new() },
        Some(Origin::Archive(a)) => {
            let name = a.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            Source { group: if vanilla_archive(&name) { Group::Vanilla } else { Group::Mod }, from: name }
        }
        Some(Origin::Loose(l)) => match l.strip_prefix(mods_dir).ok().and_then(|r| r.components().next()) {
            Some(m) => {
                let m = m.as_os_str().to_string_lossy().into_owned();
                Source { group: if m.eq_ignore_ascii_case(resources_mod) { Group::Resources } else { Group::Mod }, from: m }
            }
            None => Source { group: Group::Vanilla, from: "Data (loose)".into() },
        },
    }
}

/// `lod\rocks`, `dyndolod\lod\clutter`, ...: the folder below `meshes\` that names a category.
pub fn category(path: &str) -> String {
    let parts: Vec<&str> = path.split('\\').collect();
    let take = match parts.get(1) {
        Some(&"dyndolod") => 4,
        Some(&"lod") => 3,
        _ => parts.iter().position(|s| *s == "lod").map(|i| i + 2).unwrap_or(3),
    };
    parts[1..parts.len().saturating_sub(1).min(take)].join("\\")
}

/// Triangles and the textures of a NIF (all shapes, including SSE NiTriShape geometry).
fn nif_stats(bytes: &[u8]) -> (usize, Vec<String>) {
    let Ok(nif) = lodforge_nif::parse_all_geometry(bytes) else { return (0, Vec::new()) };
    let mut tris = 0;
    let mut tex = Vec::new();
    for b in &nif.blocks {
        if let lodforge_nif::Block::TriShape(s) = b {
            tris += s.triangles.len();
            for t in nif.shape_textures(s) {
                let t = t.trim().replace('/', "\\").to_ascii_lowercase();
                if t.ends_with(".dds") {
                    tex.push(if t.starts_with("textures\\") { t } else { format!("textures\\{}", t.trim_start_matches("data\\")) });
                }
            }
        }
    }
    (tris, tex)
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ModelUse {
    pub path: String,
    pub group: Option<Group>,
    pub from: String,
    pub category: String,
    pub triangles: usize,
    /// References using it at LOD4, LOD8, LOD16.
    pub refs: [usize; 3],
    /// Dynamic LOD references using it.
    pub dynamic: usize,
    /// Bases whose discovered level set uses it.
    pub bases: usize,
    pub textures: Vec<String>,
}

pub fn run(a: &Args) -> Result<()> {
    let p = a.profile.open()?;
    let mods_dir = a.profile.instance.join("mods");
    let dir = a.dyndolod_config.clone().unwrap_or_else(|| a.profile.instance.join(r"tools\DynDOLOD\Edit Scripts\DynDOLOD"));
    let c = crate::dyndolod_build::Config::load(&p, &dir, &a.preset, false)?;
    let worlds = lodforge_esp::scan_worldspaces(&p.load_order)?;
    let ws = worlds.iter().find(|w| w.editor_id.eq_ignore_ascii_case(&a.world)).with_context(|| format!("no worldspace {}", a.world))?;
    let h = crate::dyndolod_build::header(&p, &c, ws, "x\\", &a.profile.game_dir().join("Data"), "y\\");
    let (we, base_levels) = crate::dyndolod_build::with_resolver(&p, &c, |r| -> Result<_> {
        // Same child worldspace copies as the build, so the counts match its export.
        let copies = crate::child_copies::for_world(&p, &c, r, &worlds, ws, &a.preset);
        let we = crate::dyndolod_build::export_world(&p, &c, r, ws, &[], &h, &copies)?;
        // Discovered level sets of the bases the export uses.
        let mut used: HashSet<u32> = we.seasons[0].lines.iter().filter_map(|l| ws.refs.get(&l.ref_id).map(|q| q.base)).collect();
        used.extend(we.dynamic.iter().map(|d| d.base));
        let levels: Vec<(u32, [Option<String>; 4])> = used.iter().filter_map(|id| c.bases.get(id).map(|b| (*id, r.resolve(b).levels))).collect();
        Ok((we, levels))
    })?;
    let lines = &we.seasons[0].lines;

    let mut uses: BTreeMap<String, ModelUse> = BTreeMap::new();
    let mut kinds: BTreeMap<(&str, usize), usize> = BTreeMap::new();
    for l in lines {
        for (lv, col) in l.columns.iter().take(3).enumerate() {
            if col.is_empty() {
                *kinds.entry(("none", lv)).or_default() += 1;
            } else if col.ends_with(".dds") {
                *kinds.entry(("billboard", lv)).or_default() += 1;
            } else if col.eq_ignore_ascii_case(&l.full_model) {
                *kinds.entry(("full", lv)).or_default() += 1;
            } else {
                *kinds.entry(("lod-model", lv)).or_default() += 1;
                uses.entry(col.to_ascii_lowercase()).or_default().refs[lv] += 1;
            }
        }
    }
    let mut dyn_kinds: BTreeMap<&str, usize> = BTreeMap::new();
    for d in &we.dynamic {
        let m = d.model.to_ascii_lowercase();
        let kind = if m.contains("_dyndolod_lod") { "dyndolod_lod" } else if m.contains("_lod") { "lod-model" } else { "full" };
        *dyn_kinds.entry(kind).or_default() += 1;
        if kind != "full" {
            uses.entry(m).or_default().dynamic += 1;
        }
    }
    for (_, levels) in &base_levels {
        let set: HashSet<&String> = levels.iter().flatten().collect();
        for m in set {
            uses.entry(m.clone()).or_default().bases += 1;
        }
    }
    let paths: Vec<String> = uses.keys().cloned().collect();
    let stats: Vec<(String, Source, usize, Vec<String>)> = paths
        .par_iter()
        .map(|path| {
            let s = source(&p, &mods_dir, &a.resources_mod, path);
            let (tris, tex) = p.vfs.read(path).ok().flatten().map(|b| nif_stats(&b)).unwrap_or_default();
            (path.clone(), s, tris, tex)
        })
        .collect();
    for (path, s, tris, tex) in stats {
        let u = uses.get_mut(&path).unwrap();
        u.path = path.clone();
        u.group = Some(s.group);
        u.from = s.from;
        u.category = category(&path);
        u.triangles = tris;
        u.textures = tex;
    }

    println!("{}: {} reference lines, {} dynamic LOD references, {} bases with LOD", a.world, lines.len(), we.dynamic.len(), base_levels.len());
    println!("\nobject LOD columns (references):");
    for kind in ["lod-model", "full", "billboard", "none"] {
        println!("  {kind:<10} LOD4 {:>6}  LOD8 {:>6}  LOD16 {:>6}", kinds.get(&(kind, 0)).unwrap_or(&0), kinds.get(&(kind, 1)).unwrap_or(&0), kinds.get(&(kind, 2)).unwrap_or(&0));
    }
    println!("\nLOD model references by origin (object LOD):");
    let mut by_group: BTreeMap<Group, ([usize; 3], [usize; 3], usize, usize)> = BTreeMap::new();
    for u in uses.values() {
        let e = by_group.entry(u.group.clone().unwrap_or(Group::Missing)).or_default();
        for lv in 0..3 {
            e.0[lv] += u.refs[lv];
            e.1[lv] += u.refs[lv] * u.triangles;
        }
        if u.refs.iter().any(|&n| n > 0) {
            e.2 += 1;
        }
        e.3 += u.dynamic;
    }
    let total: [usize; 3] = std::array::from_fn(|lv| by_group.values().map(|e| e.0[lv]).sum());
    let total_tris: [usize; 3] = std::array::from_fn(|lv| by_group.values().map(|e| e.1[lv]).sum());
    for (g, (refs, tris, models, _)) in &by_group {
        let pct = |n: usize, d: usize| 100.0 * n as f64 / d.max(1) as f64;
        println!(
            "  {:<9} {:>4} models; refs LOD4 {:>6} ({:4.1}%) LOD8 {:>6} ({:4.1}%) LOD16 {:>6} ({:4.1}%); placed triangles LOD4 {:>9} ({:4.1}%) LOD8 {:>9} LOD16 {:>9}",
            format!("{g:?}"), models, refs[0], pct(refs[0], total[0]), refs[1], pct(refs[1], total[1]), refs[2], pct(refs[2], total[2]), tris[0], pct(tris[0], total_tris[0]), tris[1], tris[2]
        );
    }
    println!("\ndynamic LOD models: {}", dyn_kinds.iter().map(|(k, n)| format!("{k} {n}")).collect::<Vec<_>>().join(", "));
    for (g, (_, _, _, d)) in &by_group {
        if *d > 0 {
            println!("  {g:?}: {d} dynamic references, {} models", uses.values().filter(|u| u.dynamic > 0 && u.group.as_ref() == Some(g)).count());
        }
    }
    let from_res = |u: &&ModelUse| u.group == Some(Group::Resources);
    println!("\nresources models by category (object LOD refs at LOD4/8/16, dynamic refs, models, triangles per model):");
    let mut cats: BTreeMap<String, ([usize; 3], usize, usize, usize)> = BTreeMap::new();
    for u in uses.values().filter(from_res) {
        let e = cats.entry(u.category.clone()).or_default();
        for lv in 0..3 {
            e.0[lv] += u.refs[lv];
        }
        e.1 += u.dynamic;
        e.2 += 1;
        e.3 += u.triangles;
    }
    let mut cats: Vec<_> = cats.into_iter().collect();
    cats.sort_by_key(|(_, e)| std::cmp::Reverse(e.0[0] + e.1));
    for (cat, (refs, d, n, tris)) in &cats {
        println!("  {cat:<32} {:>6} {:>6} {:>6}  dyn {:>4}  {:>4} models  {:>6.0} tris/model", refs[0], refs[1], refs[2], d, n, *tris as f64 / (*n).max(1) as f64);
    }
    // Textures of resources models and where they come from.
    let tex: HashSet<&String> = uses.values().filter(from_res).flat_map(|u| u.textures.iter()).collect();
    let mut tex_groups: BTreeMap<Group, usize> = BTreeMap::new();
    for t in &tex {
        *tex_groups.entry(source(&p, &mods_dir, &a.resources_mod, t).group).or_default() += 1;
    }
    println!("\ntextures of resources models: {} distinct; by origin {:?}", tex.len(), tex_groups);
    // Mod/other origins, biggest first.
    let mut froms: HashMap<(Group, String), usize> = HashMap::new();
    for u in uses.values() {
        *froms.entry((u.group.clone().unwrap_or(Group::Missing), u.from.clone())).or_default() += u.refs[0] + u.dynamic;
    }
    let mut froms: Vec<_> = froms.into_iter().collect();
    froms.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    println!("\nLOD4 + dynamic references per source (top 15):");
    for ((g, f), n) in froms.iter().take(15) {
        println!("  {n:>6}  {g:?}  {f}");
    }
    if let Some(j) = &a.json {
        // Per base: full model and its origin, discovered levels, references.
        let mut line_refs: HashMap<u32, usize> = HashMap::new();
        for l in lines {
            if let Some(q) = ws.refs.get(&l.ref_id) {
                *line_refs.entry(q.base).or_default() += 1;
            }
        }
        let mut dyn_refs: HashMap<u32, usize> = HashMap::new();
        for d in &we.dynamic {
            *dyn_refs.entry(d.base).or_default() += 1;
        }
        let bases: Vec<serde_json::Value> = base_levels
            .par_iter()
            .filter_map(|(id, levels)| {
                let b = c.bases.get(id)?;
                let model = lodforge_dyndolod::bases::model_path(&b.model);
                let s = source(&p, &mods_dir, &a.resources_mod, &model);
                let tris = p.vfs.read(&model).ok().flatten().map(|b| nif_stats(&b).0).unwrap_or(0);
                Some(serde_json::json!({
                    "form_id": format!("{id:08X}"), "editor_id": b.editor_id, "signature": String::from_utf8_lossy(&b.signature),
                    "model": model, "model_group": s.group, "model_from": s.from, "model_triangles": tris,
                    "levels": levels, "refs": line_refs.get(id).copied().unwrap_or(0), "dynamic": dyn_refs.get(id).copied().unwrap_or(0),
                }))
            })
            .collect();
        std::fs::write(j.with_extension("bases.json"), serde_json::to_vec_pretty(&bases)?)?;
        std::fs::write(j, serde_json::to_vec_pretty(&uses.values().collect::<Vec<_>>())?)?;
        println!("\nwrote {}", j.display());
    }
    Ok(())
}
