//! `lodforge runtime-trees`: rewrites only the trees files (`<World>[.<SEASON>].trees.bin`) of an
//! earlier `build --runtime lodforge` output, from a fresh DynDOLOD-mode export and the impostor
//! sets already in that output (`textures\LODForge\Impostors\<World>\impostors.json`). No texgen,
//! impostor bake, object LOD or occlusion runs, so it takes minutes, not a full build.
//!
//! The profile must show the earlier output (its generated LOD models and billboards decide the
//! export's LOD4 columns, as in the build), e.g. a profile with that mod enabled.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};

use crate::profile::ProfileArgs;

#[derive(clap::Args)]
pub struct Args {
    #[command(flatten)]
    pub profile: ProfileArgs,
    /// The earlier build output (Data folder or MO2 mod folder) with the impostor sets.
    #[arg(long)]
    pub data: PathBuf,
    /// Where to write `SKSE\Plugins\LODForgeRuntime\*.trees.bin` (default: `--data`).
    #[arg(long)]
    pub out: Option<PathBuf>,
    /// Worldspaces; default: every worldspace with an impostor set in `--data`.
    #[arg(long, value_delimiter = ',')]
    pub worlds: Vec<String>,
    #[arg(long, value_delimiter = ',')]
    pub seasons: Vec<String>,
    #[arg(long, default_value = "High")]
    pub preset: String,
    #[arg(long, default_value = "Low")]
    pub child_parent: String,
    /// zstd level for the sections (0 = uncompressed).
    #[arg(long, default_value_t = 3)]
    pub zstd: i32,
    /// Re-bake the impostor sets into `--out` first (every runtime tree base, as `build
    /// --runtime lodforge` does) instead of reading the ones in `--data`.
    #[arg(long)]
    pub bake: bool,
    /// DynDOLOD folder for the bake (`_static` replacements); default `<instance>\tools\DynDOLOD`.
    #[arg(long)]
    pub dyndolod: Option<PathBuf>,
    #[command(flatten)]
    pub impostor_opts: crate::impostors::Opts,
    /// Skip the near-tier mesh pack (`TreeMeshes.bin` + tree atlas pages).
    #[arg(long)]
    pub no_pack: bool,
    /// Largest tree atlas tile per source texture.
    #[arg(long, default_value_t = 512)]
    pub tree_atlas_tile: u32,
}

/// An impostor set read back from `impostors.json` as the trees selection needs it.
pub fn set_from_index(p: &crate::profile::Profile, path: &std::path::Path) -> Result<crate::impostors::SetReport> {
    let index: lodforge_impostor::manifest::Index = serde_json::from_slice(&std::fs::read(path).with_context(|| format!("reading {}", path.display()))?)?;
    let mut set = crate::impostors::SetReport::default();
    for (i, t) in index.types.iter().enumerate() {
        for b in &t.bases {
            if let Ok(local) = u32::from_str_radix(&b.form_id, 16)
                && let Some(id) = lodforge_dyndolod::bases::global_id(&p.load_order, &b.plugin, local)
            {
                set.base_index.insert(id, i as u32);
            }
        }
        set.types.push(crate::impostors::TypeReport { name: t.name.clone(), model: t.model.clone(), model_crc32: u32::from_str_radix(&t.model_crc32, 16).unwrap_or(0), ..Default::default() });
    }
    Ok(set)
}

pub fn run(a: &Args) -> Result<()> {
    let t0 = Instant::now();
    let p = a.profile.open()?;
    let out = a.out.clone().unwrap_or_else(|| a.data.clone());
    // Keep the earlier build's token, so the rewritten files match the ones left in place.
    let has_build_txt = |d: &std::path::Path| crate::instances::out_dir(d).join(crate::instances::BUILD_TXT).is_file();
    crate::instances::init_build_token(if has_build_txt(&out) { &out } else { &a.data });
    let sets_dir = a.data.join("textures").join("LODForge").join("Impostors");
    let worlds: Vec<String> = if a.worlds.is_empty() {
        let mut v: Vec<String> = std::fs::read_dir(&sets_dir).with_context(|| format!("listing {}", sets_dir.display()))?.flatten().filter(|e| e.path().join("impostors.json").is_file()).filter_map(|e| e.file_name().to_str().map(str::to_string)).collect();
        v.sort();
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
    let write = lodforge_instances::WriteOptions { zstd: (a.zstd > 0).then_some(a.zstd), crc: true };
    let mut mesh_cache = HashMap::new();
    let mut textures = crate::instances::TextureIndex::default();
    for world in &worlds {
        let t = Instant::now();
        let Some(ws) = all.iter().find(|w| w.editor_id.eq_ignore_ascii_case(world)) else {
            println!("{world}: not in the load order, skipped");
            continue;
        };
        let h = crate::dyndolod_build::header(&p, &c, ws, "", &a.profile.game_dir().join("Data"), "");
        let we = crate::dyndolod_build::with_resolver(&p, &c, |r| -> Result<_> {
            let copies = crate::child_copies::for_world(&p, &c, r, &all, ws, &a.child_parent);
            crate::dyndolod_build::export_world(&p, &c, r, ws, &seasons, &h, &copies)
        })?;
        let set = if a.bake {
            let bases = crate::instances::runtime_bases(&c, ws, &we.seasons);
            let dyndolod = a.dyndolod.clone().unwrap_or_else(|| a.profile.instance.join("tools").join("DynDOLOD"));
            crate::impostors::build_stage(&p, &dyndolod, &out, world, &a.seasons, &a.impostor_opts, &bases)?
        } else {
            set_from_index(&p, &sets_dir.join(world).join("impostors.json"))?
        };
        for s in &we.seasons {
            let mut st = crate::instances::select_trees(&c, ws, s, &names, &set);
            crate::instances::fill_meshes(&p, &a.data, &mut st.file, &mut mesh_cache, &mut textures);
            let (path, size) = crate::instances::write_trees(&out, &st, write)?;
            let used: Vec<usize> = (0..st.file.types.len()).filter(|&i| st.file.types[i].references > 0).collect();
            let with: Vec<&usize> = used.iter().filter(|&&i| !st.file.meshes[i].model.is_empty()).collect();
            let refs_with: u32 = with.iter().map(|&&i| st.file.types[i].references).sum();
            let generated = with.iter().filter(|&&&i| st.file.meshes[i].generated).count();
            let unresolved = with.iter().flat_map(|&&i| &st.file.meshes[i].shapes).filter(|s| s.diffuse.is_empty()).count();
            println!(
                "  {}: {} trees, {} without an impostor; near-tier meshes for {} of {} used types ({generated} generated), {refs_with} trees; {unresolved} shapes without a diffuse; {:.2} MB",
                path.file_name().and_then(|n| n.to_str()).unwrap_or_default(),
                st.file.records(),
                st.kept.no_impostor,
                with.len(),
                used.len(),
                size as f64 / 1e6
            );
        }
        println!("{world} done in {:.1} s", t.elapsed().as_secs_f64());
    }
    if !a.no_pack {
        let rep = crate::tree_pack::build(&p, &out, &crate::tree_pack::Settings { tile_cap: a.tree_atlas_tile, ..Default::default() }, write)?;
        println!("tree {}", rep.summary());
    }
    println!("runtime-trees finished in {:.1} s", t0.elapsed().as_secs_f64());
    Ok(())
}

/// `lodforge runtime-statics-plan`: from a fresh export only, how many LOD4 references and
/// source triangles static instancing would take out of `.bto` per copy threshold, plain
/// materials alone and with snow/ash (projection; no object LOD build).
#[derive(clap::Args)]
pub struct PlanArgs {
    #[command(flatten)]
    pub profile: ProfileArgs,
    #[arg(long, default_value = "Tamriel")]
    pub world: String,
    #[arg(long, default_value = "High")]
    pub preset: String,
}

pub fn plan(a: &PlanArgs) -> Result<()> {
    let p = a.profile.open()?;
    let dir = a.profile.instance.join("tools").join("DynDOLOD").join("Edit Scripts").join("DynDOLOD");
    let mut c = crate::dyndolod_build::Config::load(&p, &dir, &a.preset, false)?;
    c.ultra = true;
    let all = lodforge_esp::scan_worldspaces(&p.load_order)?;
    let ws = all.iter().find(|w| w.editor_id.eq_ignore_ascii_case(&a.world)).context("worldspace not found")?;
    let h = crate::dyndolod_build::header(&p, &c, ws, "", &a.profile.game_dir().join("Data"), "");
    let we = crate::dyndolod_build::with_resolver(&p, &c, |r| -> Result<_> {
        let copies = crate::child_copies::for_world(&p, &c, r, &all, ws, "Low");
        crate::dyndolod_build::export_world(&p, &c, r, ws, &[], &h, &copies)
    })?;
    let lines = &we.seasons[0].lines;
    let tris_cache = std::cell::RefCell::new(HashMap::<String, usize>::new());
    let tris = |m: &str| -> usize {
        if let Some(&t) = tris_cache.borrow().get(m) {
            return t;
        }
        let t = p.vfs.read(m).ok().flatten().and_then(|b| lodforge_objlod::mesh::import(m, &b).ok()).map(|lm| lm.parts.iter().map(|p| p.triangles.len()).sum()).unwrap_or(0);
        tris_cache.borrow_mut().insert(m.to_string(), t);
        t
    };
    // LOD4 model of each non-tree, non-billboard, non-glow reference, with its material kind.
    let mut refs: Vec<(String, bool)> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for l in lines {
        let Some(b) = c.bases.get(&l.base) else { continue };
        if lodforge_dyndolod::assign::is_tree(b) || !seen.insert(l.ref_id) {
            continue;
        }
        let m = l.columns[0].to_ascii_lowercase();
        if m.is_empty() || m.ends_with(".dds") || m.contains("glow") {
            continue;
        }
        let class = lodforge_objlod::material::MaterialClass::parse(l.material.trim_end_matches("-LargeRef"));
        let snow = class.is_snow_or_ash();
        if !snow && !matches!(class, lodforge_objlod::material::MaterialClass::Plain | lodforge_objlod::material::MaterialClass::PassThru) {
            continue;
        }
        refs.push((m, snow));
    }
    let total: usize = refs.iter().map(|(m, _)| tris(m)).sum();
    println!("{}: {} non-tree LOD4 references with plain or snow/ash material, {:.2} M source triangles", a.world, refs.len(), total as f64 / 1e6);
    for snow_too in [false, true] {
        let mut counts: HashMap<&str, usize> = HashMap::new();
        for (m, s) in &refs {
            if snow_too || !s {
                *counts.entry(m.as_str()).or_default() += 1;
            }
        }
        let mut line = format!("  {}:", if snow_too { "plain + snow/ash" } else { "plain only" });
        for nmin in [1usize, 2, 4, 8, 16, 32] {
            let (mut r, mut t, mut sr, mut st) = (0usize, 0usize, 0usize, 0usize);
            for (m, s) in &refs {
                if (snow_too || !s) && counts.get(m.as_str()).copied().unwrap_or(0) >= nmin {
                    let x = tris(m);
                    r += 1;
                    t += x;
                    if *s {
                        sr += 1;
                        st += x;
                    }
                }
            }
            line.push_str(&format!(" N>={nmin}: {r} refs {:.2}M tris (snow {sr} refs {:.2}M);", t as f64 / 1e6, st as f64 / 1e6));
        }
        println!("{line}");
    }
    // Vertex-colour overrides: references whose LOD4 model has a coloured part the VC side file
    // whitens; the build keeps them merged (their colours would differ per full model).
    let mut we = we;
    crate::dyndolod_build::add_side_files(&p, &c, &mut we);
    let s = &we.seasons[0];
    let vc = lodforge_objlod::sidefiles::VcList::parse(&s.vc);
    let mut parts_cache: HashMap<String, Vec<(String, bool)>> = HashMap::new();
    let (mut n, mut t, mut nl, mut tl, mut models) = (0usize, 0usize, 0usize, 0usize, std::collections::HashSet::new());
    let mut seen = std::collections::HashSet::new();
    for l in &s.lines {
        let Some(b) = c.bases.get(&l.base) else { continue };
        if lodforge_dyndolod::assign::is_tree(b) || !seen.insert(l.ref_id) {
            continue;
        }
        let m = l.columns[0].to_ascii_lowercase();
        if m.is_empty() || m.ends_with(".dds") || m.contains("glow") {
            continue;
        }
        let parts = parts_cache.entry(m.clone()).or_insert_with(|| p.vfs.read(&m).ok().flatten().and_then(|b| lodforge_objlod::mesh::import(&m, &b).ok()).map(|lm| lm.parts.iter().map(|p| (p.name.clone(), p.colors.is_some())).collect()).unwrap_or_default());
        let whitened = parts.iter().any(|(name, colors)| *colors && vc.verdict(&l.full_model, name) == lodforge_objlod::sidefiles::VcVerdict::White);
        if whitened {
            let x = tris(&m);
            n += 1;
            t += x;
            models.insert((m.clone(), l.full_model.to_ascii_lowercase()));
            if l.material.ends_with("-LargeRef") {
                nl += 1;
                tl += x;
            }
        }
    }
    println!("  held back by VC whitening: {n} refs {:.2}M tris ({nl} large refs {:.2}M); {} (LOD model, full model) pairs", t as f64 / 1e6, tl as f64 / 1e6, models.len());
    Ok(())
}
