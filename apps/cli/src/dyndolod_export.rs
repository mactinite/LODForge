//! `lodforge dyndolod-export`: the DynDOLOD-compatible object LOD export built by
//! `lodforge-dyndolod` from the load order, the user's DynDOLOD configuration and a preset,
//! compared with a golden DynDOLOD run (`--golden`, the folder holding its saved preset and
//! `export\`).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use anyhow::{Context, Result};

use lodforge_dyndolod::ini::Ini;
use lodforge_dyndolod::lodmodels::LodIndex;
use lodforge_dyndolod::rules;

use crate::profile::ProfileArgs;

#[derive(clap::Args)]
pub struct Args {
    #[command(flatten)]
    pub profile: ProfileArgs,
    /// DynDOLOD's `Edit Scripts\DynDOLOD` folder (default: `<instance>\tools\DynDOLOD\Edit Scripts\DynDOLOD`).
    #[arg(long)]
    pub dyndolod_dir: Option<PathBuf>,
    /// Rule preset: Low, Medium or High.
    #[arg(long, default_value = "Medium")]
    pub preset: String,
    /// Static instead of dynamic LOD rules (`_all_static`).
    #[arg(long)]
    pub static_lod: bool,
    #[arg(long, default_value = "Tamriel")]
    pub world: String,
    /// Golden DynDOLOD run folder (DynDOLOD_SSE_Default.ini, export\).
    #[arg(long)]
    pub golden: Option<PathBuf>,
    /// Write the reference lines (and later the whole export) here.
    #[arg(long)]
    pub out: Option<PathBuf>,
    /// `PathOutput=` of the export (default: `<instance>\..\LODForge-DynDOLOD\meshes\Terrain\<World>\Objects\`).
    #[arg(long)]
    pub path_output: Option<String>,
    /// Write DynDOLOD.esm/.esp and the DLL NG data files (dynamic LOD) into this folder.
    #[arg(long)]
    pub plugins: Option<PathBuf>,
    /// Write source identities, reference state, rules and dynamic selection as diagnostic JSON.
    #[arg(long)]
    pub runtime_report: Option<PathBuf>,
    /// Also build these season exports (WIN, SPR, SUM, AUT).
    #[arg(long, value_delimiter = ',')]
    pub seasons: Vec<String>,
    /// Compare the AltTextures and VC side files with the ones in this folder (DynDOLOD's
    /// `Edit Scripts\Export`).
    #[arg(long)]
    pub side_golden: Option<PathBuf>,
}

pub fn run(a: &Args) -> Result<()> {
    let p = a.profile.open()?;
    let dir = a.dyndolod_dir.clone().unwrap_or_else(|| a.profile.instance.join(r"tools\DynDOLOD\Edit Scripts\DynDOLOD"));
    let t = std::time::Instant::now();
    let c = crate::dyndolod_build::Config::load(&p, &dir, &a.preset, a.static_lod)?;
    let rules = &c.rules;
    println!("rules: {} from the rule files", rules.len());
    if let Some(g) = &a.golden {
        let preset = Ini::read(&g.join("DynDOLOD_SSE_Default.ini")).context("reading the golden preset")?;
        let gold = rules::rules_from_preset(&preset, "Skyrim");
        let ours: Vec<String> = rules.iter().map(|r| r.preset_value()).collect();
        let theirs: Vec<String> = gold.iter().map(|r| r.preset_value()).collect();
        let same = ours.iter().zip(&theirs).filter(|(a, b)| a.eq_ignore_ascii_case(b)).count();
        println!("rules vs golden preset: {same}/{} identical lines (ours {})", theirs.len(), ours.len());
        for (i, (o, t)) in ours.iter().zip(&theirs).enumerate() {
            if !o.eq_ignore_ascii_case(t) {
                println!("  #{}: ours   {o}\n        golden {t}", i + 1);
                break;
            }
        }
    }
    println!("configuration: {} LOD models from {} mesh paths, {} ParallaxGen CRCs, bases and swaps in {} ms", c.index.len(), c.mesh_paths, c.parallax.len(), t.elapsed().as_millis());
    let (bases, materials, index) = (&c.bases, &c.materials, &c.index);
    if let Some(g) = &a.golden {
        let report = std::fs::read(g.join(r"logs\DynDOLOD_SSE_Object_Report.txt")).map(|b| lodforge_dyndolod::ini::decode(&b)).unwrap_or_default();
        compare_report(&report, bases, materials, index, &c.parallax, &p.vfs);
    }
    if let Ok(pat) = std::env::var("LODFORGE_BASES") {
        for b in bases.values().filter(|b| b.editor_id.to_ascii_lowercase().contains(&pat.to_ascii_lowercase())) {
            println!("  base {} {:08X} {} flags {:08X} bounds {:?} material {:?} model {} mnam {:?} mods {:?}", String::from_utf8_lossy(&b.signature), b.form_id, b.editor_id, b.flags, b.bounds, materials.get(&b.material), b.model, b.mnam, b.alt_textures);
        }
    }

    // Reference lines.
    let lo = &p.load_order;
    let worlds = lodforge_esp::scan_worldspaces(lo)?;
    let all_worlds = worlds.clone();
    let world = worlds.into_iter().find(|w| w.editor_id.eq_ignore_ascii_case(&a.world)).with_context(|| format!("worldspace {} not found", a.world))?;
    if let Ok(ids) = std::env::var("LODFORGE_REFS") {
        for id in ids.split(',').filter_map(|s| u32::from_str_radix(s.trim(), 16).ok()) {
            if let Some(state) = c.reference_states.get(&id) {
                println!("  state {id:08X} flags {:08X} parent {:08X} opposite {}", state.flags, state.parent, state.opposite);
            }
            if let Some(r) = world.refs.get(&id) {
                println!("  ref {id:08X} base {:08X} pos {:?} rot {:?} scale {} flags {:08X} plugin {} xesp {:08X}/{}", r.base, r.position, r.rotation, r.scale, r.flags, p.plugin_names[r.plugin as usize], r.enable_parent_id, r.enable_parent_flags);
            }
        }
    }
    crate::dyndolod_build::with_resolver(&p, &c, |resolver| -> Result<()> {
        // FlatTextures: every tree billboard (TexGen .dds + .txt) of a loaded tree base.
        let flat = crate::dyndolod_build::flat_textures(&p, &c, resolver);
        if let Some(o) = &a.out {
            std::fs::write(o.with_file_name("LODGen_SSE_FlatTextures.txt"), flat.iter().map(|l| format!("{l}\r\n")).collect::<String>())?;
        }
        if let Some(g) = &a.golden {
            let gold = lodforge_dyndolod::ini::decode(&std::fs::read(g.join("export").join("LODGen_SSE_FlatTextures.txt"))?);
            let gl: Vec<&str> = gold.lines().collect();
            let same = gl.iter().filter(|l| flat.iter().any(|o| o == *l)).count();
            println!("flat textures: {same}/{} golden lines identical (ours {})", gl.len(), flat.len());
            if let Some(l) = gl.iter().find(|l| !flat.iter().any(|o| o == *l)) {
                let key = l.split('\t').next().unwrap_or("");
                println!("  golden {:.200}\n  ours   {:.200}", l, flat.iter().find(|o| o.starts_with(key)).map(String::as_str).unwrap_or("-"));
            }
        }
        // Header and every season.
        let export_dir = format!("{}\\", dir.parent().unwrap_or(&dir).join("Export").display()).replace('/', "\\");
        let path_output = a.path_output.clone().unwrap_or_else(|| format!("{}\\meshes\\Terrain\\{}\\Objects\\", a.profile.instance.parent().unwrap_or(&a.profile.instance).join("LODForge-DynDOLOD").display(), world.editor_id));
        let header = crate::dyndolod_build::header(&p, &c, &world, &export_dir, &a.profile.instance.join("Stock Game").join("Data"), &path_output);
        let sw = header.south_west;
        let seasons: Vec<lodforge_seasons::Season> = a.seasons.iter().filter_map(|code| lodforge_seasons::Season::ALL.into_iter().find(|s| s.code().eq_ignore_ascii_case(code))).collect();
        let t = std::time::Instant::now();
        // Child worldspace copies only for the side-file comparison (the export comparison predates them).
        let copies = if a.side_golden.is_some() { crate::child_copies::for_world(&p, &c, resolver, &all_worlds, &world, &a.preset) } else { Vec::new() };
        let we = crate::dyndolod_build::export_world(&p, &c, resolver, &world, &seasons, &header, &copies)?;
        println!("exports: {} season(s) in {} ms", we.seasons.len(), t.elapsed().as_millis());
        let dynamic = &we.dynamic;
        println!("dynamic LOD: {} references", dynamic.len());
        if let Some(path) = &a.runtime_report {
            use serde_json::json;
            use std::collections::BTreeMap;
            use std::io::Write;
            let large = crate::dyndolod_build::large_refs(lo, &world);
            let referenced: HashSet<u32> = world.refs.values().map(|r| r.base).collect();
            let base_rows: BTreeMap<u32, _> = bases.iter().filter(|(id, _)| referenced.contains(id)).map(|(&id, b)| {
                let lod = resolver.resolve(b);
                let rule = lod.rule.map(|i| &c.rules[i]);
                (id, json!({"edid": b.editor_id, "signature": String::from_utf8_lossy(&b.signature),
                    "flags": b.flags, "bounds": b.bounds, "model": b.model, "mnam": b.mnam,
                    "levels": lod.levels, "columns": lod.columns,
                    "rule": rule.map(|r| r.preset_value())}))
            }).collect();
            let references: BTreeMap<u32, _> = world.refs.iter().map(|(&id, r)| (id,
                json!({"base": r.base, "flags": r.flags, "plugin": p.plugin_names[r.plugin as usize],
                    "parent": r.enable_parent_id, "parent_flags": r.enable_parent_flags,
                    "position": r.position, "scale": r.scale, "persistent": r.is_persistent(),
                    "large": large.contains(&id), "rnam": world.large_ref_cells.get(&id)}))).collect();
            let model_paths: std::collections::BTreeSet<String> = dynamic.iter().flat_map(|d| [
                lodforge_dyndolod::bases::model_path(&bases[&d.base].model),
                lodforge_dyndolod::bases::model_path(&d.model)]).collect();
            let mut models = BTreeMap::new();
            for model in model_paths {
                let bytes = p.vfs.read(&model)?.with_context(|| format!("runtime report: missing {model}"))?;
                let nif = lodforge_nif::parse(&bytes).with_context(|| format!("runtime report: parsing {model}"))?;
                let roots: Vec<_> = nif.roots.iter().flatten().map(|&i| nif.block_type(i as usize)).collect();
                let shaders: Vec<_> = nif.blocks.iter().filter_map(|b| match b {
                    lodforge_nif::Block::LightingShader(s) => Some((s.flags1, s.flags2)),
                    lodforge_nif::Block::EffectShader(s) => Some((s.flags1, s.flags2)),
                    _ => None,
                }).collect();
                models.insert(model, json!({"roots": roots, "shaders": shaders, "types": nif.header.block_types, "strings": nif.header.strings}));
            }
            let dynamic: Vec<_> = dynamic.iter().map(|d| json!({"id": d.ref_id, "base": d.base,
                "grid": d.grid.name(), "large": d.large, "persistent": d.persistent,
                "parent": d.enable_parent, "initially_enabled": d.initially_enabled, "replace": d.replace, "model": d.model})).collect();
            let objects: BTreeMap<u32, _> = we.seasons[0].lines.iter().map(|l| (l.ref_id, json!({"base": l.base, "columns": l.columns}))).collect();
            let plugins: Vec<_> = lo.plugins.iter().map(|lp| json!({"name": lp.plugin.name, "index": lp.index.global(0)})).collect();
            let mut file = std::io::BufWriter::new(std::fs::File::create(path).with_context(|| format!("creating {}", path.display()))?);
            serde_json::to_writer(&mut file, &json!({"world": world.editor_id, "plugins": plugins,
                "bases": base_rows, "references": references, "dynamic": dynamic, "objects": objects, "models": models}))?;
            file.flush()?;
        }
        if let Some(dir) = &a.plugins {
            let near: u32 = c.num("NearGrid", 11);
            let far: u32 = c.num("FarGrid", 21);
            let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1);
            let (prev_esm, prev_esp) = crate::dyndolod_build::previous_plugins(dir);
            let token = prev_esp.as_ref().and_then(|pr| pr.token.clone()).unwrap_or_else(|| lodforge_dyndolod::dynplugins::bunch_of_numbers(seed));
            let rolling = lodforge_dyndolod::dynplugins::Rolling { esm: prev_esm.as_ref(), esp: prev_esp.as_ref() };
            let base_data = crate::dyndolod_build::base_lod_data(&p, &c, we.lod_bases.iter().copied())?;
            let mut out = lodforge_dyndolod::dynplugins::build(lo, &world, bases, dynamic, sw, near, far, &token, &rolling).map_err(anyhow::Error::msg)?;
            out.worlds_txt.push_str(&base_data);
            let data = dir.join("SKSE").join("Plugins").join("DynDOLOD_Data");
            std::fs::create_dir_all(&data)?;
            std::fs::write(dir.join("DynDOLOD.esm"), &out.esm)?;
            std::fs::write(dir.join("DynDOLOD.esp"), &out.esp)?;
            std::fs::write(data.join("DynDOLOD_NG_Worlds.txt"), &out.worlds_txt)?;
            std::fs::write(data.join(format!("DynDOLOD_NG_{}.txt", world.editor_id)), &out.world_txt)?;
            // Read the plugins back as a structural check.
            for name in ["DynDOLOD.esm", "DynDOLOD.esp"] {
                let pl = lodforge_esp::Plugin::open(&dir.join(name))?;
                println!("  {name}: {} bytes, masters {:?}, records {:?}", std::fs::metadata(dir.join(name))?.len(), pl.header.masters, record_counts(&pl)?);
            }
        }
        if let Some(g) = &a.golden {
            compare_dynamic(g, lo, &world, bases, dynamic);
        }
        let model_cache = lodforge_dyndolod::sidefiles::ModelCache::default();
        for s in &we.seasons {
            let suffix = s.suffix();
            println!("export{suffix}: {} reference lines", s.lines.len());
            let t = std::time::Instant::now();
            let (vc, alt) = crate::dyndolod_build::side_texts(&p, &c, &model_cache, &s.lines);
            println!("side files{suffix}: VC {} lines, AltTextures {} lines in {} ms", vc.lines().count(), alt.lines().count(), t.elapsed().as_millis());
            if let Some(o) = &a.out {
                std::fs::write(o.with_file_name(format!("LODGen_SSE_VC_{}{suffix}.txt", world.editor_id)), &vc)?;
                std::fs::write(o.with_file_name(format!("LODGen_SSE_AltTextures_{}{suffix}.txt", world.editor_id)), &alt)?;
            }
            if let Some(g) = &a.side_golden {
                let read = |name: &str| std::fs::read(g.join(format!("LODGen_SSE_{name}_{}{suffix}.txt", world.editor_id))).map(|b| lodforge_dyndolod::ini::decode(&b)).unwrap_or_default();
                compare_side("VC", &read("VC"), &vc);
                compare_side("AltTextures", &read("AltTextures"), &alt);
                // The same from DynDOLOD's own export lines (side-file logic alone).
                let ours_base: HashMap<u32, u32> = s.lines.iter().map(|l| (l.ref_id, l.base)).collect();
                let gold_lines: Vec<lodforge_dyndolod::export::RefLine> = read("Export")
                    .lines()
                    .filter(|l| l.len() > 9 && l.as_bytes()[8] == b'\t')
                    .filter_map(|l| {
                        let f: Vec<&str> = l.split('\t').collect();
                        let id = u32::from_str_radix(f[0], 16).ok()?;
                        let base = ours_base.get(&id).copied().or_else(|| world.refs.get(&id).map(|r| r.base))?;
                        let col = |i: usize| f.get(i).map(|s| s.to_string()).unwrap_or_default();
                        Some(lodforge_dyndolod::export::RefLine { ref_id: id, full_model: col(12), columns: [col(13), col(14), col(15), col(16)], base, ..Default::default() })
                    })
                    .collect();
                let (gvc, galt) = crate::dyndolod_build::side_texts(&p, &c, &model_cache, &gold_lines);
                println!("  from DynDOLOD's export lines ({}):", gold_lines.len());
                compare_side("VC", &read("VC"), &gvc);
                compare_side("AltTextures", &read("AltTextures"), &galt);
            }
            if let Some(o) = &a.out {
                std::fs::write(o.with_file_name(format!("{}{suffix}.txt", o.file_stem().unwrap_or_default().to_string_lossy())), &s.text)?;
            }
            if let Ok(path) = std::env::var("LODFORGE_RAWDUMP") {
                let mut text = String::new();
                for l in &s.lines {
                    let r = &world.refs[&l.ref_id];
                    text.push_str(&format!("{:08X}\t{:08x}\t{:08x}\t{:08x}\n", l.ref_id, r.rotation[0].to_bits(), r.rotation[1].to_bits(), r.rotation[2].to_bits()));
                }
                std::fs::write(path, text)?;
            }
            if let Some(g) = &a.golden {
                let gold = lodforge_dyndolod::ini::decode(&std::fs::read(g.join(format!(r"export\LODGen_SSE_Export_{}{suffix}.txt", a.world)))?);
                diff_lines(&gold, &s.lines, &s.skipped, &world, bases);
                // Header lines (everything before the first reference line), compared one by one.
                let gh: Vec<&str> = gold.lines().take_while(|l| !(l.len() > 9 && l.as_bytes()[8] == b'\t')).collect();
                let oh: Vec<&str> = s.text.lines().take_while(|l| !(l.len() > 9 && l.as_bytes()[8] == b'\t')).collect();
                let same = gh.iter().filter(|l| oh.contains(l)).count();
                println!("header: {same}/{} golden lines present in ours ({} lines), order {}", gh.len(), oh.len(), if gh == oh { "identical" } else { "differs" });
                for g in gh.iter().filter(|l| !oh.contains(l)).take(6) {
                    println!("  golden only {:.140}", g);
                }
                for o in oh.iter().filter(|l| !gh.contains(l)).take(6) {
                    println!("  ours only   {:.140}", o);
                }
            }
        }
        Ok(())
    })
}

/// Records per signature of a plugin.
pub fn record_counts(pl: &lodforge_esp::Plugin) -> Result<std::collections::BTreeMap<String, usize>> {
    let mut counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    fn walk(es: lodforge_esp::Entries<'_>, counts: &mut std::collections::BTreeMap<String, usize>) -> Result<()> {
        for e in es {
            match e? {
                lodforge_esp::Entry::Record(r) => *counts.entry(r.sig.to_string()).or_default() += 1,
                lodforge_esp::Entry::Group(g) => walk(g.entries(), counts)?,
            }
        }
        Ok(())
    }
    walk(pl.top_entries(), &mut counts)?;
    Ok(counts)
}

/// Dynamic LOD references against a golden run's `DynDOLOD_NG_<World>.txt` `[Objects]` originals.
fn compare_dynamic(g: &std::path::Path, lo: &lodforge_esp::LoadOrder, world: &lodforge_esp::Worldspace, bases: &HashMap<u32, lodforge_dyndolod::bases::Base>, dynamic: &[lodforge_dyndolod::export::DynamicRef]) {
    let path = g.join("output").join("SKSE").join("plugins").join("DynDOLOD_Data").join(format!("DynDOLOD_NG_{}.txt", world.editor_id));
    let Ok(t) = std::fs::read(&path) else { return };
    let gold = golden_dynamic_originals(&lodforge_dyndolod::ini::decode(&t), lo);
    let ours: HashSet<u32> = dynamic.iter().map(|d| d.ref_id).collect();
    println!("  vs golden [Objects] originals: golden {}, ours {}, common {}", gold.len(), ours.len(), gold.intersection(&ours).count());
    let all = std::env::var_os("LODFORGE_DYNALL").is_some();
    let ex: Vec<String> = gold.difference(&ours).take(if all { 100000 } else { 5 }).map(|id| format!("{id:08X} {}", world.refs.get(id).and_then(|r| bases.get(&r.base)).map(|b| b.editor_id.as_str()).unwrap_or("?"))).collect();
    println!("  golden only e.g. {}", ex.join(", "));
    let ex: Vec<String> = ours.difference(&gold).take(if all { 100000 } else { 5 }).map(|id| format!("{id:08X} {}", world.refs.get(id).and_then(|r| bases.get(&r.base)).map(|b| b.editor_id.as_str()).unwrap_or("?"))).collect();
    println!("  ours only e.g. {}", ex.join(", "));
}

/// Global form ids of the original references in a `DynDOLOD_NG_<World>.txt` `[Objects]` section.
pub fn golden_dynamic_originals(text: &str, lo: &lodforge_esp::LoadOrder) -> HashSet<u32> {
    let mut gold: HashSet<u32> = HashSet::new();
    let mut in_obj = false;
    for l in text.lines() {
        if l.starts_with('[') {
            in_obj = l.eq_ignore_ascii_case("[Objects]");
            continue;
        }
        let f: Vec<&str> = l.split(',').collect();
        if in_obj && f.len() >= 16
            && let Ok(local) = f[14].parse::<u32>()
            && let Some(gid) = lodforge_dyndolod::bases::global_id(lo, f[15], local)
        {
            gold.insert(gid);
        }
    }
    gold
}

fn diff_lines(gold: &str, ours: &[lodforge_dyndolod::export::RefLine], skipped: &HashMap<u32, lodforge_dyndolod::export::Skip>, world: &lodforge_esp::Worldspace, bases: &HashMap<u32, lodforge_dyndolod::bases::Base>) {
    const NAMES: [&str; 17] = ["ref", "refflags", "x", "y", "z", "rx", "ry", "rz", "scale", "edid", "baseflags", "material", "model", "lod4", "lod8", "lod16", "lod32"];
    let gold_lines: Vec<&str> = gold.lines().filter(|l| l.len() > 9 && l.as_bytes()[8] == b'\t').collect();
    let gmap: HashMap<u32, &str> = gold_lines.iter().filter_map(|l| u32::from_str_radix(&l[..8], 16).ok().map(|id| (id, *l))).collect();
    let omap: HashMap<u32, String> = ours.iter().map(|l| (l.ref_id, l.text())).collect();
    let exact = gold_lines.iter().filter(|l| omap.get(&u32::from_str_radix(&l[..8], 16).unwrap_or(0)).is_some_and(|o| o == **l)).count();
    println!("vs golden: {} golden lines, ours {}, identical {exact}", gold_lines.len(), ours.len());
    let mut missing: HashMap<String, Vec<u32>> = HashMap::new();
    for id in gmap.keys() {
        if !omap.contains_key(id) {
            let why = match skipped.get(id) {
                Some(s) => format!("{s:?}"),
                None => if world.refs.contains_key(id) { "not visited".into() } else { "not in world".into() },
            };
            missing.entry(why).or_default().push(*id);
        }
    }
    for (why, ids) in &missing {
        let mut ids = ids.clone();
        ids.sort_unstable();
        let ex: Vec<String> = ids.iter().take(4).map(|id| {
            let base = world.refs.get(id).and_then(|p| bases.get(&p.base)).map(|b| b.editor_id.clone()).unwrap_or_default();
            format!("{id:08X} {base}")
        }).collect();
        println!("  missing in ours ({why}): {} e.g. {}", ids.len(), ex.join(", "));
    }
    let extra: Vec<u32> = omap.keys().filter(|id| !gmap.contains_key(id)).copied().collect();
    let ex: Vec<String> = extra.iter().take(6).map(|id| format!("{id:08X} {}", omap[id].split('\t').nth(9).unwrap_or(""))).collect();
    println!("  extra in ours: {} e.g. {}", extra.len(), ex.join(", "));
    let mut col_diff = [0usize; 17];
    let mut col_ex: Vec<Option<(String, String)>> = vec![None; 17];
    for (id, g) in &gmap {
        let Some(o) = omap.get(id) else { continue };
        let (gf, of): (Vec<&str>, Vec<&str>) = (g.split('\t').collect(), o.split('\t').collect());
        for c in 0..17 {
            let (a, b) = (gf.get(c).copied().unwrap_or(""), of.get(c).copied().unwrap_or(""));
            if a != b {
                col_diff[c] += 1;
                if col_ex[c].is_none() {
                    col_ex[c] = Some((format!("{id:08X} gold {a:?}"), format!("ours {b:?}")));
                }
            }
        }
    }
    for c in 0..17 {
        if col_diff[c] > 0 {
            let (g, o) = col_ex[c].clone().unwrap();
            println!("  column {} differs on {} lines, e.g. {g} {o}", NAMES[c], col_diff[c]);
        }
    }
    // Per-base view of LOD column differences (LODFORGE_LODDIFF=1).
    if std::env::var_os("LODFORGE_LODDIFF").is_some() {
        let mut seen: HashMap<String, (usize, String)> = HashMap::new();
        for (id, g) in &gmap {
            let Some(o) = omap.get(id) else { continue };
            let (gf, of): (Vec<&str>, Vec<&str>) = (g.split('\t').collect(), o.split('\t').collect());
            if gf[13..16] == of[13..16] {
                continue;
            }
            let b = world.refs.get(id).and_then(|p| bases.get(&p.base));
            let key = b.map(|b| b.editor_id.clone()).unwrap_or_default();
            let e = seen.entry(key).or_insert((0, String::new()));
            e.0 += 1;
            if e.1.is_empty() {
                e.1 = format!("gold {:?}\n      ours {:?}\n      mnam {:?}", &gf[13..16], &of[13..16], b.and_then(|b| b.mnam.clone()));
            }
        }
        let mut v: Vec<_> = seen.into_iter().collect();
        v.sort_by(|a, b| b.1.0.cmp(&a.1.0));
        for (k, (n, s)) in v.iter().take(25) {
            println!("  {n:5} {k}\n      {s}");
        }
    }
}

/// Compares discovered model levels with the golden `DynDOLOD_SSE_Object_Report.txt`.
fn compare_report(report: &str, bases: &HashMap<u32, lodforge_dyndolod::bases::Base>, materials: &HashMap<u32, String>, index: &LodIndex, parallax: &lodforge_dyndolod::lodmodels::ParallaxGenDiff, vfs: &lodforge_archive::Vfs) {
    let by_edid: HashMap<String, &lodforge_dyndolod::bases::Base> = bases.values().map(|b| (b.editor_id.to_ascii_lowercase(), b)).collect();
    let (mut entries, mut same, mut missing) = (0, 0, 0);
    let mut shown = 0;
    let mut cur: Option<(String, [Option<String>; 4])> = None;
    let check = |e: Option<(String, [Option<String>; 4])>, same: &mut usize, missing: &mut usize, shown: &mut usize| {
        let Some((edid, gold)) = e else { return };
        let Some(b) = by_edid.get(&edid.to_ascii_lowercase()) else {
            *missing += 1;
            return;
        };
        let model = lodforge_dyndolod::bases::model_path(&b.model);
        let stem = model.rsplit('\\').next().unwrap_or(&model).trim_end_matches(".nif").to_string();
        let crc = vfs.read(&model).ok().flatten().map(|bytes| parallax.discovery_crc(&model, lodforge_dyndolod::lodmodels::crc32(&bytes)));
        let ours = lodforge_dyndolod::lodmodels::lod_set(index, &stem, crc, lodforge_dyndolod::assign::snow_of(b, materials), false);
        if ours == gold {
            *same += 1;
        } else if *shown < 15 && std::env::var_os("LODFORGE_REPORT_DIFF").is_some() {
            *shown += 1;
            println!("  {edid} ({}) mat {:?}\n    ours   {ours:?}\n    golden {gold:?}", model, materials.get(&b.material));
        }
    };
    for line in report.lines() {
        if !line.starts_with('\t') {
            check(cur.take(), &mut same, &mut missing, &mut shown);
            if let Some((edid, _)) = line.split_once(" [") {
                entries += 1;
                cur = Some((edid.to_string(), Default::default()));
            }
            continue;
        }
        let l = line.trim_start();
        if let Some((k, v)) = l.split_once(": ")
            && let Some(n) = k.strip_prefix("Level").and_then(|n| n.parse::<usize>().ok())
            && let Some((_, levels)) = &mut cur
            && n < 4
            && levels[n].is_none()
        {
            let path = v.split(" [").next().unwrap_or("").to_ascii_lowercase();
            if !path.contains("glow_lod") {
                levels[n] = Some(path);
            }
        }
    }
    check(cur.take(), &mut same, &mut missing, &mut shown);
    println!("object report: {entries} bases, model levels identical for {same}, base not found {missing}");
}

/// Line-level comparison of a side file with DynDOLOD's (order ignored). AltTextures lines are also
/// compared per reference and per entry. `LODFORGE_SIDEDIFF=n` prints n examples each way.
fn compare_side(what: &str, gold: &str, ours: &str) {
    let gl: Vec<&str> = gold.lines().filter(|l| !l.is_empty()).collect();
    let ol: Vec<&str> = ours.lines().filter(|l| !l.is_empty()).collect();
    let gs: HashSet<&str> = gl.iter().copied().collect();
    let os: HashSet<&str> = ol.iter().copied().collect();
    let same = gl.iter().filter(|l| os.contains(*l)).count();
    println!("  {what}: {same}/{} golden lines identical ({:.2}%), ours {} lines", gl.len(), 100.0 * same as f64 / gl.len().max(1) as f64, ol.len());
    let n: usize = std::env::var("LODFORGE_SIDEDIFF").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    if what == "AltTextures" {
        let split = |ls: &[&str]| -> HashMap<String, HashSet<String>> { ls.iter().filter_map(|l| l.split_once('\t')).map(|(id, rest)| (id.to_string(), rest.split('\t').map(str::to_string).collect())).collect() };
        let (g, o) = (split(&gl), split(&ol));
        let (ge, oe): (usize, usize) = (g.values().map(HashSet::len).sum(), o.values().map(HashSet::len).sum());
        let common = g.iter().map(|(id, es)| o.get(id).map(|oes| es.intersection(oes).count()).unwrap_or(0)).sum::<usize>();
        let sets = g.iter().filter(|(id, es)| o.get(*id) == Some(*es)).count();
        println!("    references: golden {}, ours {}, both {}, same entry set {sets}; entries: golden {ge}, ours {oe}, common {common}", g.len(), o.len(), g.keys().filter(|k| o.contains_key(*k)).count());
        let mut shown = 0;
        let mut ids: Vec<&String> = g.keys().collect();
        ids.sort();
        for id in ids {
            if shown >= n {
                break;
            }
            let es = &g[id];
            let oes = o.get(id).cloned().unwrap_or_default();
            if *es == oes {
                continue;
            }
            shown += 1;
            println!("    {id}");
            for e in es.difference(&oes) {
                println!("      golden {e:.220}");
            }
            for e in oes.difference(es) {
                println!("      ours   {e:.220}");
            }
        }
        let extra: Vec<&String> = o.keys().filter(|k| !g.contains_key(*k)).take(n).collect();
        for id in extra {
            println!("    ours only {id}: {:.220}", o[id].iter().next().map(String::as_str).unwrap_or(""));
        }
    } else {
        for l in gl.iter().filter(|l| !os.contains(*l)).take(n) {
            println!("    golden only {l:.200}");
        }
        for l in ol.iter().filter(|l| !gs.contains(*l)).take(n) {
            println!("    ours only   {l:.200}");
        }
    }
}
