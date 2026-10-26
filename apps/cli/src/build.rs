//! `lodforge build`: the whole xLODGen job in one command. For every selected worldspace:
//! terrain meshes, terrain textures (diffuse + normal; seasons as deltas of the default),
//! object LOD (export, atlas + textures, BTOs, clipped against the fresh terrain meshes) and tree
//! LOD, for the default season and any extra seasons. Output goes to a Data folder or straight
//! into an MO2 mod (`--mo2-mod`). `--json` prints one JSON object per finished stage.
//!
//! `--mode dyndolod` replaces the object LOD stage with DynDOLOD's (its object LOD export and
//! LODGen 3.1.8, see `dyndolod_build`) and adds DynDOLOD.esm/.esp and Occlusion.esp at the end.
//! Earlier LOD output enabled in the profile (mods LODForge generated, DynDOLOD output) and the
//! DynDOLOD plugins are left out of the input. So is an earlier build's output in the output
//! folder itself: the stages see only the files this run wrote (`own_output`), plus
//! DynDOLOD.esm/.esp for their form ids and run token, so a rebuild in place writes the same
//! files as a fresh build.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result, bail};

use lodforge_esp::scan_worldspaces;

use crate::profile::{Profile, ProfileArgs};

#[derive(clap::Args)]
pub struct Args {
    #[command(flatten)]
    pub profile: ProfileArgs,
    /// Worldspaces (EditorIDs); default: every worldspace with a lodsettings file.
    #[arg(long, value_delimiter = ',')]
    pub worlds: Vec<String>,
    /// Extra seasons (SPR, SUM, AUT, WIN) besides the default one.
    #[arg(long, value_delimiter = ',')]
    pub seasons: Vec<String>,
    /// Output Data folder.
    #[arg(long, conflicts_with = "mo2_mod")]
    pub out: Option<PathBuf>,
    /// Write into `<instance>\mods\<name>` (created with a meta.ini; enable it in MO2 yourself,
    /// or pass --enable).
    #[arg(long)]
    pub mo2_mod: Option<String>,
    /// With --mo2-mod: also enable the mod at the top of the profile's modlist.txt (MO2 must be
    /// closed).
    #[arg(long)]
    pub enable: bool,
    #[arg(long)]
    pub no_terrain: bool,
    /// Leave cells without LAND flat instead of filling them from the terrain LOD meshes the game
    /// ships (terrain meshes and underside; docs/re/terrain-lod-spec.md 2.3).
    #[arg(long)]
    pub no_lod_fill: bool,
    #[arg(long)]
    pub no_objects: bool,
    #[arg(long)]
    pub no_trees: bool,
    /// Run a TexGen stage first: tree billboards (and the rendered trunk billboards DynDOLOD uses)
    /// from the load order into the output, used by the tree LOD stage.
    #[arg(long)]
    pub texgen: bool,
    /// DynDOLOD folder for the TexGen stage (`_static` replacements, overrides, trunk models);
    /// defaults to `<instance>	oolsDynDOLOD`. Read only.
    #[arg(long)]
    pub dyndolod: Option<PathBuf>,
    /// Print one JSON line per finished stage.
    #[arg(long)]
    pub json: bool,
    /// Object LOD flavour: xLODGen's, or DynDOLOD's with dynamic LOD plugins and occlusion.
    #[arg(long, value_enum, default_value = "xlodgen")]
    pub mode: Mode,
    #[command(flatten)]
    pub dyn_opts: crate::dyndolod_build::Opts,
    /// Read earlier LOD output enabled in the profile too (by default mods LODForge generated and
    /// DynDOLOD output are left out).
    #[arg(long)]
    pub keep_lod_mods: bool,
    /// Terrain LOD texture mip chains: `full` (default), `golden` (xLODGen's saved settings: LOD4
    /// diffuse only) or `none`.
    #[arg(long, value_enum, default_value = "full")]
    pub terrain_mips: crate::terrain_textures::TerrainMips,
    /// Terrain LOD texture resampling: `hq` (default; no cell grid, linear-light diffuse,
    /// renormalised normals) or `xlodgen` (as the golden xLODGen run).
    #[arg(long, value_enum, default_value = "hq")]
    pub terrain_filter: crate::terrain_textures::TerrainFilter,
    /// LOD32 (world map) terrain settings: `step` (2048 px textures, gamma 1.25, mesh quality 0;
    /// the default) or `golden` (the golden xLODGen run: 512 px, gamma 1.0, quality 10).
    #[arg(long, value_enum, default_value = "step")]
    pub lod32: crate::terrain_textures::Lod32,
    /// Colour scale for PBR landscape albedo in the terrain LOD textures: 0.65 (default,
    /// DynDOLOD's PBR scale, so terrain LOD matches the darker in-game PBR terrain and the object
    /// LOD) or 1 (the albedo unchanged, as xLODGen).
    #[arg(long, default_value_t = crate::terrain_textures::DEFAULT_PBR_SCALE)]
    pub terrain_pbr_scale: f32,
    /// Tone of the stitched object LOD textures (the LOD8/16 mini atlases): `flat` (the default;
    /// every texture keeps its source's brightness, so distant rocks and buildings match the
    /// full textures LOD4 shows) or `dyndolod` (TexGen's rule brightness and gamma columns,
    /// which darken e.g. mountain textures by 14% below LOD4).
    #[arg(long, value_enum, default_value = "flat")]
    pub lod_texture_tone: LodTextureTone,
    /// With `--lod-texture-tone flat`: how far (0..1) a pre-baked LOD texture that only LOD8+
    /// models use (e.g. `textures\lod\mtncliff04lod.dds` from a LOD texture pack) is moved
    /// toward the mean colour of the LOD4 textures of the same references. 0 leaves such bakes
    /// as shipped; 1 matches the texture means outright, which looks too bright because the
    /// LOD4 shapes also carry darkening vertex colours.
    #[arg(long, default_value_t = 0.5)]
    pub far_lod_match: f32,
    /// Keep the shipped LOD8/16 models everywhere. By default a reference whose LOD8/16 model
    /// shows a pre-baked LOD texture its LOD4 model does not use (mountain cliffs with a LOD
    /// texture pack) gets LOD8/16 models simplified from its LOD4 model instead, so texture,
    /// UVs and normal map stay the same across the LOD4 -> LOD8 switch.
    #[arg(long)]
    pub keep_far_lod_models: bool,
    /// Material setup of the list. `pbr` (Community Shaders with PBR): pbr_lod texture twins,
    /// PBR texture sets and linear copies from the textures\pbr\ sources. `standard` (non-PBR:
    /// ENB complex material, parallax or vanilla): plain textures only, textures\pbr\ sources
    /// ignored. LOD carries no complex-material or parallax maps in either case, so `standard`
    /// is simply "not PBR" (`complex` is accepted as an alias). Complex grass is detected per
    /// shader either way.
    #[arg(long, value_enum, default_value = "pbr")]
    pub materials: Materials,
    #[command(flatten)]
    pub rt: crate::instances::Opts,
    /// Also bake hemi-octahedral tree impostors for the LODForge runtime, per worldspace under
    /// `textures\LODForge\Impostors\<World>\` (docs/runtime/impostors.md).
    #[arg(long)]
    pub impostors: bool,
    #[command(flatten)]
    pub impostor_opts: crate::impostors::Opts,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, clap::ValueEnum)]
pub enum Mode {
    Xlodgen,
    Dyndolod,
}

/// `--lod-texture-tone`: whether the stitched LOD textures take TexGen's per-rule tone.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, clap::ValueEnum)]
pub enum LodTextureTone {
    #[default]
    Flat,
    Dyndolod,
}

/// `--materials`: which texture variants the DynDOLOD path makes and names.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, clap::ValueEnum)]
pub enum Materials {
    /// Community Shaders with PBR (`textures\pbr\` sources, `pbr_lod` twins, `linear.dds` copies).
    #[default]
    Pbr,
    /// Non-PBR (ENB complex material, parallax or vanilla shading): plain textures only.
    #[value(alias = "complex")]
    Standard,
}

impl Materials {
    pub fn pbr(self) -> bool {
        self == Materials::Pbr
    }
}

/// `--json`: one line when a stage starts, so a front end can show what is running.
pub(crate) fn begin(json: bool, world: &str, stage: &str, season: Option<&str>) {
    if json {
        println!("{{\"event\":\"begin\",\"world\":\"{world}\",\"stage\":\"{stage}\",\"season\":{}}}", season.map(|s| format!("\"{s}\"")).unwrap_or_else(|| "null".into()));
    }
}

pub(crate) static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

pub(crate) fn event(json: bool, world: &str, stage: &str, season: Option<&str>, ms: u128) {
    if json {
        let (bs, bpx, bn) = lodforge_gpu::bc7_stats::snapshot();
        let t = START.get_or_init(Instant::now).elapsed().as_millis();
        println!("{{\"event\":\"stage\",\"world\":\"{world}\",\"stage\":\"{stage}\",\"season\":{},\"ms\":{ms},\"t\":{t},\"bc7_s\":{bs:.2},\"bc7_mpx\":{},\"bc7_calls\":{bn}}}", season.map(|s| format!("\"{s}\"")).unwrap_or_else(|| "null".into()), bpx / 1_000_000);
    } else {
        println!("== {world} {stage}{} done in {ms} ms", season.map(|s| format!(" [{s}]")).unwrap_or_default());
    }
}

/// Worldspaces xLODGen can generate: those with `lodsettings\<world>.lod`.
pub fn lod_worlds(p: &Profile) -> Result<Vec<String>> {
    let worlds = scan_worldspaces(&p.load_order)?;
    let mut out = Vec::new();
    for w in worlds {
        if !w.editor_id.is_empty() && p.vfs.read(&format!("lodsettings\\{}.lod", w.editor_id))?.is_some() {
            out.push(w.editor_id);
        }
    }
    Ok(out)
}

fn ensure_mo2_mod(a: &Args, name: &str) -> Result<PathBuf> {
    let dir = a.profile.instance.join("mods").join(name);
    std::fs::create_dir_all(&dir)?;
    let meta = dir.join("meta.ini");
    if !meta.exists() {
        std::fs::write(&meta, "[General]\r\nmodid=0\r\nversion=\r\nnewestVersion=\r\ncategory=\r\ninstallationFile=\r\ncomments=Generated by LODForge\r\n")?;
    }
    if a.enable {
        let running = std::process::Command::new("tasklist").args(["/FI", "IMAGENAME eq ModOrganizer.exe", "/NH"]).output().map(|o| String::from_utf8_lossy(&o.stdout).contains("ModOrganizer.exe")).unwrap_or(false);
        if running {
            bail!("MO2 is running; close it before --enable (it rewrites modlist.txt on exit)");
        }
        let ml = a.profile.instance.join("profiles").join(&a.profile.profile).join("modlist.txt");
        let text = std::fs::read_to_string(&ml).unwrap_or_default();
        let line = format!("+{name}");
        if !text.lines().any(|l| l.trim_end() == line) {
            let mut out = String::new();
            let mut done = false;
            for l in text.lines() {
                if !done && !l.starts_with('#') && l.trim_end() != format!("-{name}") {
                    out.push_str(&line);
                    out.push_str("\r\n");
                    done = true;
                }
                if l.trim_end() != format!("-{name}") {
                    out.push_str(l);
                    out.push_str("\r\n");
                }
            }
            if !done {
                out.push_str(&line);
                out.push_str("\r\n");
            }
            std::fs::write(&ml, out)?;
        }
    }
    Ok(dir)
}

pub fn run(a: &Args) -> Result<()> {
    let total = Instant::now();
    START.get_or_init(Instant::now);
    let out = match (&a.out, &a.mo2_mod) {
        (Some(o), _) => o.clone(),
        (None, Some(name)) => ensure_mo2_mod(a, name)?,
        (None, None) => bail!("pass --out <Data folder> or --mo2-mod <name>"),
    };
    // Stages write into the output from the start (the tree report of the object stage comes
    // before any LOD file when the terrain stage is off).
    std::fs::create_dir_all(&out).with_context(|| format!("creating {}", out.display()))?;
    // Inputs: the profile without earlier LOD output (the generated plugins are always left out,
    // see `profile::is_generated_lod_plugin`).
    let mut prof = a.profile.clone();
    if !a.keep_lod_mods {
        let generated = crate::profile::generated_lod_mods(&prof.instance, &prof.profile);
        prof.exclude_mod.extend(generated);
    }
    if let Some(name) = &a.mo2_mod {
        prof.exclude_mod.push(name.clone());
    }
    prof.exclude_mod.sort();
    prof.exclude_mod.dedup();
    if !a.json && !prof.exclude_mod.is_empty() {
        println!("not read: {}", prof.exclude_mod.join(", "));
    }
    if a.dyn_opts.grass == crate::dyndolod_build::GrassMode::Bto && a.mode != Mode::Dyndolod {
        bail!("--grass bto puts grass into DynDOLOD's object LOD; use it with --mode dyndolod (or --grass go)");
    }
    if a.rt.on() && a.mode != Mode::Dyndolod {
        bail!("--runtime lodforge needs --mode dyndolod (the trees and repeated statics come from its export)");
    }
    if a.rt.on() && a.dyn_opts.grass == crate::dyndolod_build::GrassMode::Bto {
        bail!("--runtime lodforge draws far grass from .lfg files; leave --grass bto off");
    }
    let mut p = prof.open()?;
    // What the output folder holds before this build writes into it (an earlier build's output,
    // or anything else). The stages see their own files through the VFS (`own_output` below),
    // never these: a rebuild into the same folder reads the load order, the DynDOLOD.esm/.esp
    // kept for their form ids (`dyndolod_build::previous_plugins`) and, with --no-terrain, the
    // terrain LOD already in the output; nothing else of the earlier output.
    let before = lodforge_archive::DirSnapshot::take(&out).with_context(|| format!("listing {}", out.display()))?;
    if !a.json && !before.is_empty() {
        println!("output folder already holds {} files: not read (only DynDOLOD.esm/.esp for their form ids{})", before.len(), if a.no_terrain { ", and the terrain LOD with --no-terrain" } else { "" });
    }
    let worlds = if a.worlds.is_empty() { lod_worlds(&p)? } else { a.worlds.clone() };
    if !a.json {
        println!("LODForge build: {} worldspace(s) {:?}, seasons {:?}, output {}", worlds.len(), worlds, a.seasons, out.display());
        if a.materials == Materials::Standard {
            println!(r"materials: standard (non-PBR; plain textures, textures\pbr\ sources ignored)");
        }
    }
    let mut failed = Vec::new();
    // Next to the output (same volume, so grass cache files can be hard-linked).
    let work = a.dyn_opts.work_dir.clone().unwrap_or_else(|| out.parent().unwrap_or(&out).join(format!(".lodforge-work-{}", std::process::id())));
    // Terrain meshes and textures of every worldspace and season run on their own thread from
    // here on; the object stages below wait per worldspace for its meshes.
    let lane = (!a.no_terrain).then(|| crate::terrain_lane::TerrainLane::start(crate::terrain_lane::LaneArgs { profile: a.profile.clone(), out: out.clone(), worlds: worlds.clone(), seasons: a.seasons.clone(), mips: a.terrain_mips, filter: a.terrain_filter, lod32: a.lod32, pbr_scale: a.terrain_pbr_scale, json: a.json, lod_fill: !a.no_lod_fill }));
    if a.texgen {
        let t = Instant::now();
        begin(a.json, "all", "texgen", None);
        match texgen_stage(a, &p, &out) {
            Ok(()) => event(a.json, "all", "texgen", None, t.elapsed().as_millis()),
            Err(e) => {
                eprintln!("texgen: {e:#}");
                failed.push(format!("texgen: {e}"));
            }
        }
        if a.mode == Mode::Dyndolod {
            // The mini atlases DynDOLOD's LOD models use (textures\lod\..., stitched from full
            // textures by the TexGen rules), so no installed TexGen output is needed.
            let t = Instant::now();
            begin(a.json, "all", "texgen-stitched", None);
            let dyndolod = a.dyndolod.clone().unwrap_or_else(|| a.profile.instance.join(r"tools\DynDOLOD"));
            let reuv = work.join("DynDOLOD_SSE_re-uv-miniatlas.txt");
            match lodforge_gpu::Gpu::new().map_err(anyhow::Error::from).and_then(|gpu| crate::texgen_stitched::stitch_into(&p, &gpu, &dyndolod, &out, Some(&reuv), a.materials.pbr(), a.lod_texture_tone == LodTextureTone::Dyndolod)) {
                Ok(m) => {
                    if !a.json {
                        println!("texgen-stitched: {} atlases, {} PBR twins, {} full textures, {} temporaries, {} rendered; {} skipped", m.atlases, m.pbr, m.full, m.temps, m.rendered, m.skipped.len());
                        for s in m.skipped.iter().take(10) {
                            println!("  skipped {s}");
                        }
                    }
                    event(a.json, "all", "texgen-stitched", None, t.elapsed().as_millis());
                }
                Err(e) => {
                    eprintln!("texgen-stitched: {e:#}");
                    failed.push(format!("texgen-stitched: {e}"));
                }
            }
        }
        // Later stages read the new billboards and textures through the VFS.
        own_output(&mut p, &out, &before)?;
    }
    if a.no_terrain {
        // Object LOD clipping and occlusion use the terrain LOD already in the output (the
        // object LOD stage reads the BTRs from the output folder directly, occlusion through
        // the VFS).
        let rels: Vec<String> = worlds
            .iter()
            .flat_map(|w| {
                let dir = out.join("meshes").join("terrain").join(w);
                std::fs::read_dir(&dir).ok().into_iter().flatten().flatten().filter_map(|e| e.file_name().to_str().map(str::to_string)).filter(|f| f.to_ascii_lowercase().ends_with(".btr")).map(move |f| format!("meshes\\terrain\\{w}\\{f}")).collect::<Vec<_>>()
            })
            .collect();
        p.vfs.add_loose_files(&out, rels.iter().map(String::as_str));
    }
    // DynDOLOD mode: configuration, worldspaces and the work folder for LODGen's input files.
    let dyn_mode = a.mode == Mode::Dyndolod;
    let mut dyn_cfg: Option<crate::dyndolod_build::Config> = None;
    // PBR linear textures named by the AltTextures, and those already written.
    let mut linear: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut linear_done: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut dyn_worlds: Vec<lodforge_esp::Worldspace> = Vec::new();
    #[allow(clippy::type_complexity)]
    let mut dynamic = Vec::new();
    let mut tree_exclude: std::collections::HashMap<String, std::collections::HashSet<u32>> = std::collections::HashMap::new();
    let mut undersides: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut mini_rows: Vec<lodforge_texgen::stitch::MiniAtlasRow> = Vec::new();
    let mut tree_billboards: Vec<String> = Vec::new();
    if dyn_mode && !a.no_objects {
        let t = Instant::now();
        begin(a.json, "all", "dyndolod-config", None);
        let dir = a.dyn_opts.dyndolod_config.clone().unwrap_or_else(|| a.profile.instance.join(r"tools\DynDOLOD\Edit Scripts\DynDOLOD"));
        match crate::dyndolod_build::Config::load(&p, &dir, &a.dyn_opts.preset, a.dyn_opts.static_lod) {
            Ok(mut c) => {
                c.windows = a.dyn_opts.windows;
                // Runtime mode keeps every tree in the export (as Ultra does); the object LOD stage then
                // leaves the runtime trees out of the merged shapes.
                c.ultra = a.dyn_opts.tree_lod == crate::dyndolod_build::TreeLodMode::Ultra || a.rt.on();
                c.pbr = a.materials.pbr();
                for (key, v) in [("ObjectLODMaxTileSize", a.dyn_opts.atlas_max_tile), ("FullTextureMaxTileSize", a.dyn_opts.atlas_full_max_tile)] {
                    if let Some(v) = v {
                        c.ini.set("DynDOLOD", key, &v.next_power_of_two().clamp(64, 4096).to_string());
                    }
                }
                if !a.json {
                    println!("DynDOLOD configuration: {} rules, {} LOD models, preset {}", c.rules.len(), c.index.len(), a.dyn_opts.preset);
                }
                dyn_worlds = scan_worldspaces(&p.load_order)?;
                if a.dyn_opts.lod_models {
                    let t = Instant::now();
                    begin(a.json, "all", "lod-models", None);
                    let selected: Vec<&lodforge_esp::Worldspace> = dyn_worlds.iter().filter(|w| worlds.iter().any(|n| n.eq_ignore_ascii_case(&w.editor_id))).collect();
                    let min_size = a.dyn_opts.lod_model_min_size.as_ref().map(|v| [0, 1, 2].map(|k| v.get(k).or(v.last()).copied().unwrap_or(0.0))).unwrap_or(crate::lod_models::DEFAULT_MIN_SIZE);
                    let items = crate::lod_models::plan(&p, &c, &selected, &crate::lod_models::Policy { min_size, all: false });
                    let cache = a.dyn_opts.lod_model_cache.clone().unwrap_or_else(|| out.parent().unwrap_or(&out).join(".lodforge-cache"));
                    let rep = crate::lod_models::generate(&p, &items, &out, Some(&cache), &lodforge_lodmodel::LEVELS);
                    // Discovery and the object LOD stage read them through the VFS.
                    own_output(&mut p, &out, &before)?;
                    c.rescan(&p);
                    if !a.json {
                        println!("LOD models: {} generated for {} full models ({} from cache, {} without geometry), {} LOD models now", rep.made.len(), items.len(), rep.made.iter().filter(|(_, m)| m.cached).count(), rep.failed.len(), c.index.len());
                    }
                    event(a.json, "all", "lod-models", None, t.elapsed().as_millis());
                }
                if c.ultra && a.dyn_opts.tree_models != crate::dyndolod_build::TreeModelsMode::Off {
                    let t = Instant::now();
                    begin(a.json, "all", "tree-models", None);
                    let all = a.dyn_opts.tree_models == crate::dyndolod_build::TreeModelsMode::All;
                    match crate::dyndolod_build::with_resolver(&p, &c, |r| crate::tree_models::levels_arg(a.dyn_opts.tree_model_levels.as_deref()).and_then(|lv| crate::tree_models::generate(&p, &c, r, &out, &lv, all, &[], a.dyn_opts.tree_model_crown, &lodforge_treemodel::cloud::CloudParams::default()))) {
                        Ok(rep) => {
                            // LOD discovery and the object LOD stage read them through the VFS.
                            own_output(&mut p, &out, &before)?;
                            c.rescan(&p);
                            if !a.json {
                                println!("{}", rep.summary());
                            }
                            event(a.json, "all", "tree-models", None, t.elapsed().as_millis());
                        }
                        Err(e) => {
                            eprintln!("tree-models: {e:#}");
                            failed.push(format!("tree-models: {e}"));
                        }
                    }
                }
                let flat = crate::dyndolod_build::with_resolver(&p, &c, |r| crate::dyndolod_build::flat_textures(&p, &c, r));
                tree_billboards = flat.iter().filter_map(|l| l.split('\t').next()).map(str::to_string).collect();
                std::fs::create_dir_all(&work)?;
                std::fs::write(work.join("LODGen_SSE_FlatTextures.txt"), flat.iter().map(|l| format!("{l}\r\n")).collect::<String>())?;
                // TexGen's mini atlases (installed, or made by --texgen above) for the atlas maps.
                let dyndolod = a.dyndolod.clone().unwrap_or_else(|| a.profile.instance.join(r"tools\DynDOLOD"));
                // (LODFORGE_NO_MINIATLAS=1: left out, for A/B comparisons.)
                if std::env::var_os("LODFORGE_NO_MINIATLAS").is_none() {
                    mini_rows = crate::texgen_stitched::miniatlas_rows(&p, &dyndolod, a.materials.pbr());
                }
                if !a.json {
                    println!("  {} TexGen mini-atlas rows for the atlas maps", mini_rows.len());
                }
                dyn_cfg = Some(c);
                event(a.json, "all", "dyndolod-config", None, t.elapsed().as_millis());
            }
            Err(e) => {
                eprintln!("dyndolod-config: {e:#}");
                failed.push(format!("dyndolod-config: {e}"));
            }
        }
    }
    // `--runtime lodforge`: load-order form ids -> plugin names for the instance files.
    let form_names = a.rt.on().then(|| std::sync::Arc::new(crate::instances::FormNames::new(&p)));
    if a.rt.on() {
        crate::instances::init_build_token(&out);
    }
    // Runtime mode: per worldspace, the export's large references -> `.bto` levels with a column.
    let mut large_columns: std::collections::HashMap<String, std::collections::HashMap<u32, u16>> = std::collections::HashMap::new();
    // Runtime mode: per worldspace, its export lines inside the bounds of a child worldspace that
    // uses its LOD (the parent copies of the `<Child>.dyn.bin` files).
    let mut child_lines: std::collections::HashMap<String, Vec<lodforge_dyndolod::export::RefLine>> = std::collections::HashMap::new();
    let mut child_variants: std::collections::HashMap<String, std::collections::HashMap<u32, crate::dyn_export::Variant>> = std::collections::HashMap::new();
    let mut tree_mesh_cache: std::collections::HashMap<String, lodforge_instances::trees::TreeMesh> = std::collections::HashMap::new();
    let mut tex_index = crate::instances::TextureIndex::default();
    for world in &worlds {
        let mut step = |stage: &str, season: Option<&str>, f: &mut dyn FnMut() -> Result<()>| {
            let t = Instant::now();
            begin(a.json, world, stage, season);
            match f() {
                Ok(()) => event(a.json, world, stage, season, t.elapsed().as_millis()),
                Err(e) => {
                    eprintln!("{world} {stage}: {e:#}");
                    failed.push(format!("{world} {stage}: {e}"));
                }
            }
        };
        // Impostors without the runtime files: TexGen's tree candidates. (Runtime mode bakes them
        // after the export, for every tree the runtime draws; see `runtime-trees` below.)
        let impostor_dir = a.dyndolod.clone().unwrap_or_else(|| a.profile.instance.join(r"tools\DynDOLOD"));
        if a.impostors && !a.rt.on() {
            step("impostors", None, &mut || crate::impostors::build_stage(&p, &impostor_dir, &out, world, &a.seasons, &a.impostor_opts, &std::collections::HashSet::new()).map(|_| ()));
        }
        if !a.no_objects && dyn_mode {
            let Some(c) = &dyn_cfg else { continue };
            let Some(ws) = dyn_worlds.iter().find(|w| w.editor_id.eq_ignore_ascii_case(world)) else { continue };
            let parent = ws.parent.and_then(|pid| dyn_worlds.iter().find(|w| w.form_id == pid));
            let seasons: Vec<lodforge_seasons::Season> = a.seasons.iter().filter_map(|s| crate::object_export::season_from_code(s)).collect();
            let export_dir = work.join(world);
            let mut exports = None;
            let mut layout = None;
            let mut pending: Option<crate::dyndolod_build::WorldExport> = None;
            // The mini atlases DynDOLOD makes itself (MiniAtlasExport): written to the output and
            // added to the VFS before the atlas reads them.
            let mut world_mini = mini_rows.clone();
            let mut mini_written: Vec<String> = Vec::new();
            let mut rt_refs: std::collections::HashMap<String, std::sync::Arc<std::collections::HashSet<String>>> = std::collections::HashMap::new();
            let mut rt_bases: std::collections::HashSet<u32> = std::collections::HashSet::new();
            let mut rt_rows: Vec<crate::tree_report::Row> = Vec::new();
            step("dyndolod-export", None, &mut || {
                // LODGen's input files are all written below; nothing of an earlier run stays.
                if export_dir.is_dir() {
                    std::fs::remove_dir_all(&export_dir).with_context(|| format!("clearing {}", export_dir.display()))?;
                }
                let export_text = format!("{}\\", export_dir.display()).replace('/', "\\");
                let path_output = format!("{}\\", out.join("meshes").join("terrain").join(world).join("Objects").display()).replace('/', "\\");
                let h = crate::dyndolod_build::header(&p, c, ws, &export_text, &a.profile.game_dir().join("Data"), &path_output);
                let (mut we, tree_rows) = crate::dyndolod_build::with_resolver(&p, c, |r| -> Result<_> {
                    let copies = crate::child_copies::for_world(&p, c, r, &dyn_worlds, ws, &a.dyn_opts.child_parent);
                    let we = crate::dyndolod_build::export_world(&p, c, r, ws, &seasons, &h, &copies)?;
                    // What every tree got (docs: tree_report.rs), from the default season.
                    let with = lodforge_dyndolod::childworld::with_copies(ws, &copies);
                    // Runtime mode: every tree base the runtime will draw, for the impostor stage.
                    if a.rt.on() {
                        rt_bases = crate::instances::runtime_bases(c, &with, &we.seasons);
                    }
                    let s = &we.seasons[0];
                    let rows = crate::tree_report::rows(&p, c, r, &with, &s.lines, &s.skipped, &a.profile.instance.join("mods"), "DynDOLOD Resources SE 3", &std::collections::HashMap::new());
                    Ok((we, rows))
                })?;
                if a.rt.on() {
                    // Written by `runtime-trees` with the runtime column.
                    rt_rows = tree_rows;
                } else if !tree_rows.is_empty() {
                    std::fs::write(out.join(format!("LODForge_TreeReport_{world}.csv")), crate::tree_report::csv(&tree_rows))?;
                    if !a.json {
                        print!("{}", crate::tree_report::summary(world, &tree_rows));
                    }
                }
                if !a.json && !we.child_copies.is_empty() {
                    println!("  {} child worldspace references copied in", we.child_copies.len());
                }
                if a.dyn_opts.grass == crate::dyndolod_build::GrassMode::Bto {
                    let keys = crate::dyndolod_build::grass_header(c);
                    for s in &mut we.seasons {
                        if let Some(i) = s.text.find("\r\n") {
                            s.text.insert_str(i + 2, &keys);
                        }
                    }
                }
                let requests = crate::dyndolod_build::add_side_files(&p, c, &mut we);
                linear.extend(we.seasons.iter().flat_map(|s| crate::pbr_linear::names_in(&s.alt_textures)));
                if !mini_rows.is_empty() {
                    let dyndolod = a.dyndolod.clone().unwrap_or_else(|| a.profile.instance.join(r"tools\DynDOLOD"));
                    let has_row: std::collections::HashSet<String> = mini_rows.iter().map(|r| r.source.to_lowercase()).collect();
                    let per_season: Vec<Vec<lodforge_texgen::stitch::Atlas>> = requests.iter().map(|q| crate::texgen_stitched::mini_atlas_export(&p, &dyndolod, q, |t| has_row.contains(t))).collect();
                    std::fs::create_dir_all(&export_dir)?;
                    for (s, atlases) in we.seasons.iter().zip(&per_season) {
                        if !atlases.is_empty() {
                            std::fs::write(export_dir.join(format!("LODGen_SSE_MiniAtlasExport_{world}{}.txt", s.suffix())), crate::texgen_stitched::mini_atlas_export_text(atlases, &out))?;
                        }
                    }
                    let mut all: Vec<lodforge_texgen::stitch::Atlas> = Vec::new();
                    for x in per_season.into_iter().flatten() {
                        if !all.iter().any(|y| y.dest == x.dest) {
                            all.push(x);
                        }
                    }
                    if !all.is_empty() {
                        let gpu = lodforge_gpu::Gpu::new()?;
                        let (written, skipped) = crate::texgen_stitched::stitch_atlases(&p, &gpu, &all, &out, a.materials.pbr())?;
                        let made: Vec<lodforge_texgen::stitch::Rule> = all.iter().filter(|x| written.contains(&x.dest)).flat_map(|x| x.rules.iter().cloned()).collect();
                        world_mini.extend(lodforge_texgen::stitch::atlas_map_rows(&made, lodforge_texgen::stitch::StitchParams::default().base, |_| false, |_| None));
                        if !a.json {
                            println!("  MiniAtlasExport: {} mini atlases ({} textures written)", all.len(), written.len());
                            for s in &skipped {
                                println!("    skipped {s}");
                            }
                        }
                        mini_written = written;
                    }
                }
                pending = Some(we);
                Ok(())
            });
            // Runtime mode: impostors for every tree the runtime draws, then the trees files typed
            // by the impostor set (docs/runtime/instance-formats.md section 5).
            if let Some(names) = &form_names
                && pending.is_some()
            {
                let mut set: Option<crate::impostors::SetReport> = None;
                step("impostors", None, &mut || {
                    set = Some(crate::impostors::build_stage(&p, &impostor_dir, &out, world, &a.seasons, &a.impostor_opts, &rt_bases)?);
                    Ok(())
                });
                step("runtime-trees", None, &mut || {
                    let Some(we) = pending.as_ref() else { return Ok(()) };
                    let empty = crate::impostors::SetReport::default();
                    let set = set.as_ref().unwrap_or(&empty);
                    let mut runtime_counts = std::collections::HashMap::new();
                    for (k, s) in we.seasons.iter().enumerate() {
                        let mut t = crate::instances::select_trees(c, ws, s, names, set);
                        crate::instances::fill_meshes(&p, &out, &mut t.file, &mut tree_mesh_cache, &mut tex_index);
                        if !a.json {
                            let used: Vec<usize> = (0..t.file.types.len()).filter(|&i| t.file.types[i].references > 0).collect();
                            let with = used.iter().filter(|&&i| !t.file.meshes[i].model.is_empty()).count();
                            let refs_with: u32 = used.iter().filter(|&&i| !t.file.meshes[i].model.is_empty()).map(|&i| t.file.types[i].references).sum();
                            let generated = used.iter().filter(|&&i| t.file.meshes[i].generated).count();
                            let unresolved = used.iter().flat_map(|&i| &t.file.meshes[i].shapes).filter(|s| s.diffuse.is_empty()).count();
                            println!("  near-tier meshes ({}): {with} of {} used types have a LOD4 3D model ({generated} generated), covering {refs_with} of {} trees; {unresolved} shapes without a diffuse", if t.file.common.season.is_empty() { "default" } else { &t.file.common.season }, used.len(), t.file.records());
                        }
                        let (path, size) = crate::instances::write_trees(&out, &t, a.rt.write_options())?;
                        if !a.json {
                            println!("  runtime trees: {} references, {} types in {} cells; {} ({:.2} MB); kept in object LOD: {} tilted, {} large, {} child copies, {} in child-world bounds, {} without a plugin, {} without an impostor", t.file.records(), t.file.types.len(), t.file.cells.len(), path.file_name().and_then(|n| n.to_str()).unwrap_or_default(), size as f64 / 1e6, t.kept.tilted, t.kept.large, t.kept.copies, t.kept.bounds, t.kept.no_plugin, t.kept.no_impostor);
                            if t.kept.no_impostor > 0 {
                                println!("    without an impostor: {}", t.top_no_impostor(6));
                            }
                        }
                        if k == 0 {
                            runtime_counts = crate::instances::count_by_base(&s.lines, &t.refs);
                        }
                        rt_refs.insert(t.file.common.season.clone(), std::sync::Arc::new(t.ref_columns()));
                    }
                    for r in &mut rt_rows {
                        r.runtime_refs = runtime_counts.get(&r.base).copied().unwrap_or(0);
                    }
                    if !rt_rows.is_empty() {
                        std::fs::write(out.join(format!("LODForge_TreeReport_{world}.csv")), crate::tree_report::csv(&rt_rows))?;
                        if !a.json {
                            print!("{}", crate::tree_report::summary(world, &rt_rows));
                        }
                    }
                    Ok(())
                });
            }
            if !mini_written.is_empty() {
                p.vfs.add_loose_files(&out, mini_written.iter().map(String::as_str));
            }
            // LOD8/16 models rebuilt from the LOD4 model where the shipped far models switch to
            // a pre-baked texture of their own (mountain cliffs).
            if !a.keep_far_lod_models && pending.is_some() {
                let mut far_written: Vec<String> = Vec::new();
                step("far-lod-models", None, &mut || {
                    let Some(we) = pending.as_mut() else { return Ok(()) };
                    let cache = a.dyn_opts.lod_model_cache.clone().unwrap_or_else(|| out.parent().unwrap_or(&out).join(".lodforge-cache"));
                    let rep = crate::lod_models::far_from_lod0(&p, we, &out, Some(&cache));
                    if !a.json {
                        println!("  far LOD models from the LOD4 model: {} models, {} reference lines; {} failed", rep.written.len(), rep.lines, rep.failed.len());
                        for (m, e) in rep.failed.iter().take(5) {
                            println!("    {m}: {e}");
                        }
                    }
                    far_written = rep.written;
                    Ok(())
                });
                if !far_written.is_empty() {
                    p.vfs.add_loose_files(&out, far_written.iter().map(String::as_str));
                }
            }
            // Grass placements and billboards before the atlas: the billboard views go on the
            // `_Alpha` page like the tree billboards (DynDOLOD's map), so the atlas has to know
            // them and the VFS has to hold them when the pages are rendered.
            let mut grass_inputs: Option<crate::dyndolod_build::GrassInputs> = None;
            if a.dyn_opts.grass == crate::dyndolod_build::GrassMode::Bto {
                step("grass", None, &mut || {
                    let g = crate::dyndolod_build::prepare_grass(&p, c, &prof, ws, &work, &out)?;
                    if !a.json {
                        println!("  grass placements: {} cache files ({} with form ids fixed for this load order)", g.cached, g.remapped);
                    }
                    grass_inputs = Some(g);
                    Ok(())
                });
                if let Some(g) = &grass_inputs {
                    p.vfs.add_loose_files(&out, g.files.iter().map(String::as_str));
                }
            }
            let grass_views: Vec<String> = grass_inputs.as_ref().map(|g| crate::dyndolod_build::grass_billboard_views(&g.files)).unwrap_or_default();
            step("dyndolod-atlas", None, &mut || {
                let Some(we) = pending.take() else { return Ok(()) };
                let atlas = crate::dyndolod_build::atlas(&p, c, ws, &we, &world_mini, &grass_views, &tree_billboards, (a.lod_texture_tone == LodTextureTone::Flat && a.far_lod_match > 0.0).then_some((out.as_path(), a.far_lod_match.min(1.0))))?;
                let bin = crate::dyndolod_build::terrain_bin(&p.load_order, ws, parent)?;
                let paths = crate::dyndolod_build::write_world_files(&export_dir, world, &we, &bin, &atlas)?;
                std::fs::copy(work.join("LODGen_SSE_FlatTextures.txt"), export_dir.join("LODGen_SSE_FlatTextures.txt"))?;
                if !a.json {
                    println!("  {} reference lines, {} dynamic LOD references, atlas {} textures on {} page(s)", we.seasons[0].lines.len(), we.dynamic.len(), atlas.entries.len(), atlas.pages.len());
                }
                let skipped = we.seasons[0].skipped.iter().filter(|(_, s)| matches!(s, lodforge_dyndolod::export::Skip::ReferenceRule)).map(|(id, _)| *id);
                // Tree LOD leaves out what the export keeps in object or dynamic LOD (tilted, large, child-world bounds).
                if a.rt.on() {
                    let cl = crate::dyn_export::child_bound_lines(c, &dyn_worlds, ws, &we.seasons[0].lines);
                    child_variants.insert(world.clone(), crate::dyn_export::variants(&p, &we.seasons[0], &cl));
                    child_lines.insert(world.clone(), cl);
                    let lc = large_columns.entry(world.clone()).or_default();
                    for l in &we.seasons[0].lines {
                        if ws.large_ref_cells.contains_key(&l.ref_id) {
                            let bits = (0..3).filter(|&i| !l.columns[i].is_empty()).fold(0u16, |b, i| b | (1 << i));
                            *lc.entry(l.ref_id).or_default() |= bits;
                        }
                    }
                }
                let in_object_lod = we.seasons[0].lines.iter().map(|l| l.ref_id);
                tree_exclude.insert(world.clone(), we.dynamic.iter().map(|d| d.ref_id).chain(skipped).chain(in_object_lod).collect());
                dynamic.push((world.clone(), we.dynamic, we.seasonal_dynamic, we.child_copies, we.lod_refs, we.lod_bases));
                exports = Some(paths);
                layout = Some(atlas);
                Ok(())
            });
            let Some(paths) = exports else { continue };
            // The `...linear.dds` copies of the PBR diffuses the AltTextures name (shared by every
            // worldspace; each is written once).
            linear.retain(|n| !linear_done.contains(n));
            if a.materials.pbr() && !linear.is_empty() {
                step("pbr-linear", None, &mut || {
                    let gpu = lodforge_gpu::Gpu::new()?;
                    let n = crate::pbr_linear::write(&p.vfs, &gpu, &linear, &out)?;
                    if !a.json {
                        println!("  {n} of {} PBR linear textures written", linear.len());
                    }
                    Ok(())
                });
                linear_done.append(&mut linear);
            }
            if let Some(atlas) = &layout
                && !atlas.pages.is_empty()
            {
                step("objects-atlas", None, &mut || {
                    let gpu = lodforge_gpu::Gpu::new()?;
                    let pages = crate::object_atlas::render(&p.vfs, atlas, &gpu, &crate::dyndolod_build::AtlasSettings::from_config(c).formats())?;
                    crate::object_atlas::write(&out, &pages)?;
                    // The new pages invalidate season BTOs an earlier run left for seasons this
                    // run does not build.
                    let stale = crate::object_atlas::remove_stale_season_btos(&out, world, &a.seasons)?;
                    if stale > 0 {
                        println!("  removed {stale} season object LOD files of an earlier build (seasons not built now; they used the previous atlas layout)");
                    }
                    Ok(())
                });
            }
            let mut args = object_lod_args(a, world, &out, paths, crate::object_lod::FlavorArg::Dyndolod);
            if let Some(g) = grass_inputs {
                args.grass = Some(g.dir);
                args.grass_map = Some(g.map);
                args.grass_flat = Some(g.flat);
                args.grass_density = a.dyn_opts.grass_density;
            }
            // Clipped against this worldspace's fresh terrain meshes.
            if let Some(l) = &lane {
                l.wait_meshes(world);
            }
            if let Some(names) = &form_names {
                args.runtime = Some(crate::instances::RuntimeObjects { refs: std::mem::take(&mut rt_refs), tree_models: a.rt.runtime_tree_statics, snow: !a.rt.runtime_no_static_snow, min_copies: a.rt.runtime_min_copies, large_refs_always: !a.rt.runtime_no_large_refs, model_masks: a.rt.runtime_model_masks.clone(), names: names.clone(), write: a.rt.write_options() });
            }
            step("objects", None, &mut || crate::object_lod::run_with(&p, &args));
            if a.rt.on() && !a.rt.runtime_no_grass {
                step("runtime-grass", None, &mut || {
                    for r in crate::grass_far::build(&p, world, &a.seasons, a.rt.runtime_grass_cache.as_deref(), &out, a.rt.write_options())? {
                        if !a.json {
                            print!("{}", r.summary());
                        }
                    }
                    Ok(())
                });
            }
        } else if !a.no_objects {
            let args = crate::object_lod::Args {
                profile: a.profile.clone(),
                export: Vec::new(),
                seasons: a.seasons.clone(),
                atlas: None,
                terrain: None,
                hidden_rule: None,
                no_hidden: false,
                reuv: false,
                loose_atlas_uv: false,
                hidden_study: false,
                out: out.clone(),
                golden: None,
                level: None,
                only: None,
                explain: Vec::new(),
                compare_only: None,
                dry_run: false,
                from_load_order: true,
                world: world.clone(),
                atlas_textures: true,
                flavor: crate::object_lod::FlavorArg::Xlodgen,
                tolerant: false,
                geometry_only: false,
                side_files: None,
                grid: 0.1,
                grass: None,
                grass_map: None,
                grass_flat: None,
                grass_density: None,
                runtime: None,
            };
            if let Some(l) = &lane {
                l.wait_meshes(world);
            }
            step("objects", None, &mut || crate::object_lod::run_with(&p, &args));
        }
        if !a.no_trees {
            let dyndolod = dyn_cfg.as_ref().filter(|_| dyn_mode).map(|c| c.dir.clone());
            let exclude = tree_exclude.remove(world).unwrap_or_default();
            let args = crate::tree_lod::Args { profile: a.profile.clone(), world: world.clone(), seasons: a.seasons.clone(), out: out.clone(), placeholder_billboards: None, golden: None, list_missing: false, dyndolod, exclude };
            if dyn_cfg.as_ref().is_some_and(|c| c.ultra && dyn_mode) {
                // Ultra: the trees are in object LOD; empty lists switch the game's tree LOD off.
                step("trees", None, &mut || crate::tree_lod::write_empty_lists(&p, &args));
            } else {
                step("trees", None, &mut || crate::tree_lod::run_with(&p, &args));
            }
        }
        if dyn_mode
            && !a.no_objects
            && !a.dyn_opts.no_underside
            && let Some(c) = &dyn_cfg
            && crate::dyndolod_build::wants_underside(c, world)
        {
            let args = crate::terrain_underside::Args { profile: a.profile.clone(), world: world.clone(), out: out.clone(), quality: 10.0, from_export: None, golden: None, dyndolod: Some(c.dir.clone()), no_lod_fill: a.no_lod_fill };
            let mut built = false;
            if let Some(l) = &lane {
                l.wait_meshes(world);
            }
            step("underside", None, &mut || {
                crate::terrain_underside::run_with(&p, &args)?;
                built = true;
                Ok(())
            });
            if built {
                undersides.insert(world.clone());
            }
        }
    }
    if a.rt.on()
        && let (Some(c), Some(names)) = (&dyn_cfg, &form_names)
    {
        // Dynamic LOD for LODForge Runtime (`.dyn.bin`), before the static pack takes its models.
        let t = Instant::now();
        begin(a.json, "all", "runtime-dyn", None);
        let inputs: Vec<crate::dyn_export::DynWorld<'_>> = dynamic
            .iter()
            .filter_map(|(w, dynamic, seasonal, child_copies, lod_refs, lod_bases)| {
                dyn_worlds.iter().find(|x| x.editor_id.eq_ignore_ascii_case(w)).map(|world| crate::dyn_export::DynWorld { world, dynamic, seasonal, child_copies, lod_refs, lod_bases, large_columns: large_columns.get(w), lines: child_lines.get(w).map_or(&[][..], Vec::as_slice), variants: child_variants.get(w) })
            })
            .collect();
        match crate::dyn_export::write(&p, c, &inputs, &dyn_worlds, &undersides, names, &out, a.rt.write_options(), crate::dyn_export::WriteFlags { large_refs_instanced: !a.rt.runtime_no_large_refs, workarounds: crate::dyndolod_build::workarounds_on(c, &a.dyn_opts) }, &out) {
            Ok(r) => {
                if !a.json {
                    println!("  {}", r.summary());
                }
                event(a.json, "all", "runtime-dyn", None, t.elapsed().as_millis());
            }
            Err(e) => {
                eprintln!("runtime-dyn: {e:#}");
                failed.push(format!("runtime-dyn: {e}"));
            }
        }
    }
    if a.rt.on() {
        // The near-tier mesh pack of every worldspace's trees files (shared atlas + clusters).
        let t = Instant::now();
        begin(a.json, "all", "tree-pack", None);
        // Every texture this build wrote (mini atlases, PBR copies, ...) is a pack input.
        if let Err(e) = own_output(&mut p, &out, &before) {
            eprintln!("tree-pack: {e:#}");
        }
        let packs = crate::tree_pack::build(&p, &out, &crate::tree_pack::Settings::default(), a.rt.write_options()).and_then(|t| {
            // Static pack: instanced statics, large references and the dynamic LOD models.
            let s = if a.rt.runtime_min_copies > 0 || !a.rt.runtime_no_large_refs || dyn_cfg.is_some() { Some(crate::tree_pack::build_statics(&p, &out, &crate::tree_pack::Settings::default(), a.rt.write_options())?) } else { None };
            // Far grass: the grass types of the `.lfg` files this build wrote.
            let has_lfg = std::fs::read_dir(crate::instances::out_dir(&out)).map(|d| d.flatten().any(|e| e.file_name().to_string_lossy().to_ascii_lowercase().ends_with(".lfg"))).unwrap_or(false);
            if has_lfg {
                let g = crate::grass_pack::build(&mut p, &out, &crate::tree_pack::Settings::default(), a.rt.write_options())?;
                if !a.json {
                    println!("  grass {} ({} cards rendered as billboards)", g.summary(), g.rendered_cards);
                }
            }
            Ok((t, s))
        });
        match packs {
            Ok((rep, statics)) => {
                if !a.json {
                    println!("  tree {}", rep.summary());
                    if let Some(s) = statics {
                        println!("  static {}", s.summary());
                    }
                }
                event(a.json, "all", "tree-pack", None, t.elapsed().as_millis());
            }
            Err(e) => {
                eprintln!("tree-pack: {e:#}");
                failed.push(format!("tree-pack: {e}"));
            }
        }
    }
    if a.dyn_opts.grass == crate::dyndolod_build::GrassMode::Go {
        // Grass LOD meshes are per grass model, not per worldspace.
        let t = Instant::now();
        begin(a.json, "all", "grass-lod1", None);
        let r = (|| -> Result<(usize, usize)> {
            let all = if dyn_worlds.is_empty() { scan_worldspaces(&p.load_order)? } else { Vec::new() };
            let list = if dyn_worlds.is_empty() { &all } else { &dyn_worlds };
            let ws = list.iter().find(|w| worlds.first().is_some_and(|n| w.editor_id.eq_ignore_ascii_case(n))).context("no worldspace for the grass tables")?;
            crate::dyndolod_build::write_grass_lod1(&p, &prof, ws, &work, &out)
        })();
        match r {
            Ok((n, present)) => {
                if !a.json {
                    println!("  grass LOD meshes (Grass Optimizations _lod1): {n} written, {present} already in the load order");
                }
                event(a.json, "all", "grass-lod1", None, t.elapsed().as_millis());
            }
            Err(e) => {
                eprintln!("grass-lod1: {e:#}");
                failed.push(format!("grass-lod1: {e}"));
            }
        }
    }
    // Everything after this reads the terrain LOD through the VFS: the lane must be done.
    if let Some(l) = lane {
        l.finish(&mut failed);
    }
    if dyn_mode && let Some(c) = &dyn_cfg {
        // The DynDOLOD stages read the build's own terrain LOD and plugins.
        own_output(&mut p, &out, &before)?;
        if !a.dyn_opts.no_plugins && !a.dyn_opts.static_lod && !(a.rt.on() && a.rt.runtime_no_dyndolod_plugins) {
            let t = Instant::now();
            begin(a.json, "all", "plugins", None);
            let refs: Vec<crate::dyndolod_build::PluginWorld<'_>> = dynamic
                .drain(..)
                .filter_map(|(w, dynamic, seasonal, child_copies, lod_refs, lod_bases)| dyn_worlds.iter().find(|x| x.editor_id.eq_ignore_ascii_case(&w)).map(|world| crate::dyndolod_build::PluginWorld { world, dynamic, seasonal, child_copies, lod_refs, lod_bases }))
                .collect();
            match crate::dyndolod_build::write_plugins(&p, c, &a.dyn_opts, &refs, &dyn_worlds, &undersides, crate::dyndolod_build::large_ref_margin(&a.profile), &out) {
                Ok((n, w, mut used)) => {
                    if !a.json {
                        println!("  DynDOLOD.esm/.esp: {n} dynamic LOD references in {w} worldspace(s)");
                    }
                    // Linear copies only the plugins' texture sets use (child copies, dynamic LOD).
                    used.retain(|n| !linear_done.contains(n));
                    if a.materials.pbr() && !used.is_empty() {
                        let written = (|| -> Result<usize> {
                            let gpu = lodforge_gpu::Gpu::new()?;
                            crate::pbr_linear::write(&p.vfs, &gpu, &used, &out)
                        })();
                        match written {
                            Ok(k) => {
                                if !a.json {
                                    println!("  {k} of {} PBR linear textures written for the plugins' texture sets", used.len());
                                }
                                linear_done.append(&mut used);
                            }
                            Err(e) => {
                                eprintln!("plugins: linear textures: {e:#}");
                                failed.push(format!("plugins: linear textures: {e}"));
                            }
                        }
                    }
                    event(a.json, "all", "plugins", None, t.elapsed().as_millis());
                }
                Err(e) => {
                    eprintln!("plugins: {e:#}");
                    failed.push(format!("plugins: {e}"));
                }
            }
        }
        if !a.dyn_opts.no_occlusion {
            let t = Instant::now();
            begin(a.json, "all", "occlusion", None);
            match crate::dyndolod_build::write_occlusion(&p, c, &worlds, &out) {
                Ok(done) => {
                    if !a.json {
                        let cells: usize = done.iter().map(|d| d.1).sum();
                        println!("  Occlusion.esp: {cells} cells in {} worldspace(s)", done.len());
                    }
                    event(a.json, "all", "occlusion", None, t.elapsed().as_millis());
                }
                Err(e) => {
                    eprintln!("occlusion: {e:#}");
                    failed.push(format!("occlusion: {e}"));
                }
            }
        }
    }
    if a.dyn_opts.work_dir.is_none() {
        std::fs::remove_dir_all(&work).ok();
    }
    if a.json {
        println!("{{\"event\":\"done\",\"ms\":{},\"failed\":{}}}", total.elapsed().as_millis(), failed.len());
    } else {
        println!("LODForge build finished in {:.1} s; {} failed stage(s)", total.elapsed().as_secs_f64(), failed.len());
        for f in &failed {
            println!("  {f}");
        }
    }
    if !failed.is_empty() {
        bail!("{} stage(s) failed", failed.len());
    }
    Ok(())
}

/// Makes the files this build has written into `out` so far visible to the later stages: those
/// new or changed since `before` (the folder's state when the build started). An earlier
/// build's output in the same folder is never an input (see [`run`]).
fn own_output(p: &mut Profile, out: &std::path::Path, before: &lodforge_archive::DirSnapshot) -> Result<()> {
    p.vfs.add_loose_root_since(out, before).with_context(|| format!("listing {}", out.display()))?;
    Ok(())
}

/// TexGen stage: tree billboards, their report and the rendered trunk billboards, written into
/// `out` (`textures\terrain\lodgen\...`, `textures\DynDOLOD\LOD\trees\...`).
fn texgen_stage(a: &Args, p: &Profile, out: &std::path::Path) -> Result<()> {
    use crate::texgen_billboards as tg;
    let gpu = lodforge_gpu::Gpu::new()?;
    let renderer = lodforge_texgen::Renderer::new(&gpu);
    let dyndolod = a.dyndolod.clone().unwrap_or_else(|| a.profile.instance.join(r"tools\DynDOLOD"));
    let lo = &p.load_order;
    let bases = lodforge_dyndolod::bases::read_bases(lo)?;
    let max_scale = tg::max_ref_scales(lo)?;
    let cands = tg::discover_trees(p, &bases, &dyndolod);
    let accepted: Vec<&tg::Candidate> = cands.iter().filter(|c| c.accepted).collect();
    let report = out.join(r"textures\terrain\lodgen\TexGen_SSE_Tree_Billboards.txt");
    std::fs::create_dir_all(report.parent().unwrap())?;
    std::fs::write(&report, tg::tree_report(&cands))?;
    let trees = tg::tree_billboards(p, &gpu, &renderer, &dyndolod, &accepted, &max_scale, false, |tb| tg::write_tree_billboard(&gpu, out, &tb))?;
    let trunks = tg::trunk_billboards(p, &gpu, &renderer, &dyndolod, None, |tb| tg::write_rendered(&gpu, &out.join(&tb.out_rel), &tb.stem, &tb.billboard, None, &tb.source))?;
    if !a.json {
        println!("texgen: {} candidates, {trees} tree billboards, {trunks} trunk billboards", cands.len());
    }
    Ok(())
}

/// `object-lod` arguments for one worldspace of the build.
fn object_lod_args(a: &Args, world: &str, out: &std::path::Path, export: Vec<PathBuf>, flavor: crate::object_lod::FlavorArg) -> crate::object_lod::Args {
    crate::object_lod::Args {
        profile: a.profile.clone(),
        export,
        seasons: Vec::new(),
        atlas: None,
        terrain: None,
        hidden_rule: None,
        no_hidden: false,
        reuv: false,
        loose_atlas_uv: false,
        hidden_study: false,
        out: out.to_path_buf(),
        golden: None,
        level: None,
        only: None,
        explain: Vec::new(),
        compare_only: None,
        dry_run: false,
        from_load_order: false,
        world: world.to_string(),
        atlas_textures: false,
        flavor,
        tolerant: false,
        geometry_only: false,
        side_files: None,
        grid: 0.1,
        grass: None,
        grass_map: None,
        grass_flat: None,
        grass_density: None,
        runtime: None,
    }
}
