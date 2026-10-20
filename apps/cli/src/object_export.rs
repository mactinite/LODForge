//! `lodforge object-export`: the LODGen object export (references, base models, materials) built
//! from the load order (`lodforge_objlod::source`, port of xEdit's `wbGenerateLODTES5`), diffed
//! against xLODGen's `LODGen[.<season>].txt`.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result, bail};

use lodforge_esp::scan_worldspaces;
use lodforge_objlod::source::{ExportOptions, export_from_load_order, read_bases, read_materials};
use lodforge_objlod::{ExportFile, StaticRef};
use lodforge_objlod::atlas_build::{AtlasLayout, layout, sorted_unique, texture_asset_name, untiled_textures};
use lodforge_seasons::{Season, load_swaps};

use crate::profile::{Profile, ProfileArgs};

#[derive(clap::Args)]
pub struct Args {
    #[command(flatten)]
    pub profile: ProfileArgs,
    #[arg(long, default_value = "Tamriel")]
    pub world: String,
    /// Also export these seasons (SPR, SUM, AUT, WIN).
    #[arg(long, value_delimiter = ',')]
    pub seasons: Vec<String>,
    /// Folder with xLODGen's LODGen[.<season>].txt to diff against
    /// (default `<instance>\tools\xLODGen\Edit Scripts`).
    #[arg(long)]
    pub golden: Option<PathBuf>,
    /// Render the atlas textures into this Data folder.
    #[arg(long)]
    pub atlas_out: Option<PathBuf>,
    /// xLODGen output Data folder to compare the atlas textures with (top-mip PSNR).
    #[arg(long)]
    pub golden_textures: Option<PathBuf>,
}

/// Everything the export needs from the profile, loaded once for all seasons.
pub struct ExportInputs {
    pub world: lodforge_esp::Worldspace,
    pub bases: HashMap<u32, lodforge_objlod::source::Base>,
    pub materials: HashMap<u32, String>,
    pub south_west: (i32, i32),
    pub swaps: lodforge_seasons::SeasonSwaps,
}

pub fn load_inputs(p: &Profile, world: &str) -> Result<ExportInputs> {
    let lo = &p.load_order;
    let worlds = scan_worldspaces(lo)?;
    let Some(w) = worlds.into_iter().find(|w| w.editor_id.eq_ignore_ascii_case(world)) else { bail!("worldspace {world} not found") };
    let bases = read_bases(lo)?;
    let materials = read_materials(lo)?;
    let path = format!("lodsettings\\{}.lod", w.editor_id);
    let lod = p.vfs.read(&path)?.with_context(|| format!("{path} not found"))?;
    let south_west = (i32::from(i16::from_le_bytes([lod[0], lod[1]])), i32::from(i16::from_le_bytes([lod[2], lod[3]])));
    let edids: HashMap<String, u32> = bases.iter().map(|(id, b)| (b.editor_id.to_ascii_lowercase(), *id)).collect();
    let swaps = load_swaps(&p.vfs, lo, &edids)?;
    Ok(ExportInputs { world: w, bases, materials, south_west, swaps })
}

/// Static and tree swaps of one season.
pub fn base_swaps(inputs: &ExportInputs, season: Season) -> HashMap<u32, u32> {
    let mut out = HashMap::new();
    if let Some(sections) = inputs.swaps.swaps.get(&season) {
        for (name, map) in sections {
            if name.eq_ignore_ascii_case("statics") || name.eq_ignore_ascii_case("trees") {
                out.extend(map.iter().map(|(a, b)| (*a, *b)));
            }
        }
    }
    out
}

pub fn season_from_code(code: &str) -> Option<Season> {
    Season::ALL.into_iter().find(|s| s.code().eq_ignore_ascii_case(code))
}

pub fn build_export(p: &Profile, inputs: &ExportInputs, season: Option<Season>) -> (ExportFile, lodforge_objlod::source::ExportStats) {
    let opts = ExportOptions {
        south_west: inputs.south_west,
        atlas_tolerance: 0.5,
        season: season.map(|s| s.code().to_string()),
        base_swaps: season.map(|s| base_swaps(inputs, s)).unwrap_or_default(),
        winter_snow: season == Some(Season::Winter),
    };
    let vfs = &p.vfs;
    let exists = |path: &str| vfs.read(path).ok().flatten().is_some();
    export_from_load_order(&p.load_order, &inputs.world, &inputs.bases, &inputs.materials, &opts, &exists)
}

pub fn run(a: &Args) -> Result<()> {
    let t = Instant::now();
    let p = a.profile.open()?;
    let inputs = load_inputs(&p, &a.world)?;
    println!("profile + records loaded in {} ms; {} STAT/TREE bases, {} references, {} RNAM entries", t.elapsed().as_millis(), inputs.bases.len(), inputs.world.refs.len(), inputs.world.large_refs.len());
    let golden = a.golden.clone().unwrap_or_else(|| a.profile.instance.join(r"tools\xLODGen\Edit Scripts"));
    let gpu = if a.atlas_out.is_some() || a.golden_textures.is_some() { Some(lodforge_gpu::Gpu::new()?) } else { None };
    let mut seasons: Vec<Option<Season>> = vec![None];
    for s in &a.seasons {
        seasons.push(Some(season_from_code(s).with_context(|| format!("unknown season {s}"))?));
    }
    for season in seasons {
        let t = Instant::now();
        let (ours, st) = build_export(&p, &inputs, season);
        let ms = t.elapsed().as_millis();
        let name = match season {
            Some(s) => format!("LODGen.{}.txt", s.code()),
            None => "LODGen.txt".into(),
        };
        println!("{name}: {} references from {} REFRs exported in {ms} ms (skipped: {} not STAT/TREE, {} disabled, {} enable parent, {} persistent, {} outside, {} no LOD mesh; {} large refs, {} LOD meshes)", ours.statics.len(), st.refs, st.not_static, st.disabled, st.enable_parent, st.persistent, st.out_of_area, st.no_lod, st.large_refs, st.lod_meshes.len());
        let path = golden.join(&name);
        let Ok(bytes) = std::fs::read(&path) else {
            println!("  no golden {}", path.display());
            continue;
        };
        let gold = ExportFile::parse(&lodforge_objlod::export::decode(&bytes));
        diff(&gold, &ours);
        let t = Instant::now();
        let atlas = atlas_layout(&p, &inputs, &st, season);
        let map_name = match season {
            Some(s) => format!("SSELODGen-AtlasMap{}.{}.txt", inputs.world.editor_id, s.code()),
            None => format!("SSELODGen-AtlasMap{}.txt", inputs.world.editor_id),
        };
        println!("  atlas: {} textures on {} pages in {} ms", atlas.entries.len(), atlas.pages.len(), t.elapsed().as_millis());
        if let Ok(bytes) = std::fs::read(golden.join(&map_name)) {
            diff_map(&lodforge_objlod::export::decode(&bytes), &atlas);
        }
        if let Some(gpu) = &gpu {
            let t = Instant::now();
            let pages = crate::object_atlas::render(&p.vfs, &atlas, gpu, &crate::object_atlas::Formats::BC7_FULL)?;
            println!("  atlas textures: {} pages rendered + BC7 in {} ms", pages.len(), t.elapsed().as_millis());
            if let Some(g) = &a.golden_textures {
                crate::object_atlas::compare_golden(g, &pages);
            }
            if let Some(out) = &a.atlas_out {
                crate::object_atlas::write(out, &pages)?;
            }
        }
    }
    Ok(())
}

fn diff(gold: &ExportFile, ours: &ExportFile) {
    let g: HashMap<&str, &StaticRef> = gold.statics.iter().map(|s| (s.ref_id.as_str(), s)).collect();
    let o: HashMap<&str, &StaticRef> = ours.statics.iter().map(|s| (s.ref_id.as_str(), s)).collect();
    let only_gold: Vec<&&str> = g.keys().filter(|k| !o.contains_key(*k)).collect();
    let only_ours: Vec<&&str> = o.keys().filter(|k| !g.contains_key(*k)).collect();
    let mut fields: BTreeMap<&str, (usize, Vec<String>)> = BTreeMap::new();
    let mut equal = 0;
    for (k, gs) in &g {
        let Some(os) = o.get(k) else { continue };
        let checks: [(&str, String, String); 12] = [
            ("ref_flags", format!("{:08X}", gs.ref_flags), format!("{:08X}", os.ref_flags)),
            ("pos", format!("{:?}", [gs.x, gs.y, gs.z]), format!("{:?}", [os.x, os.y, os.z])),
            ("rot", format!("{:?}", [gs.rot_x, gs.rot_y, gs.rot_z]), format!("{:?}", [os.rot_x, os.rot_y, os.rot_z])),
            ("scale", gs.scale.to_string(), os.scale.to_string()),
            ("static_name", gs.static_name.clone(), os.static_name.clone()),
            ("static_flags", format!("{:08X}", gs.static_flags), format!("{:08X}", os.static_flags)),
            ("material", gs.material_name.clone(), os.material_name.clone()),
            ("full_model", gs.full_model.clone(), os.full_model.clone()),
            ("lod4", gs.models[0].clone(), os.models[0].clone()),
            ("lod8", gs.models[1].clone(), os.models[1].clone()),
            ("lod16", gs.models[2].clone(), os.models[2].clone()),
            ("cell", format!("{},{}", gs.cell_x, gs.cell_y), format!("{},{}", os.cell_x, os.cell_y)),
        ];
        let mut same = true;
        for (f, a, b) in checks {
            if a != b {
                same = false;
                let e = fields.entry(f).or_default();
                e.0 += 1;
                if e.1.len() < 3 {
                    e.1.push(format!("{k} {}: golden {a:?} ours {b:?}", gs.static_name));
                }
            }
        }
        equal += usize::from(same);
    }
    println!("  vs golden: {} golden refs, {} ours; {equal} identical, {} only golden, {} only ours", g.len(), o.len(), only_gold.len(), only_ours.len());
    for (f, (n, ex)) in &fields {
        println!("    {f}: {n} differ");
        for e in ex {
            println!("      {e}");
        }
    }
    for k in only_gold.iter().take(5) {
        let s = g[**k];
        println!("    only golden: {k} {} {} flags {:08X}", s.static_name, s.models[0], s.ref_flags);
    }
    for k in only_ours.iter().take(5) {
        let s = o[**k];
        println!("    only ours: {k} {} {} flags {:08X}", s.static_name, s.models[0], s.ref_flags);
    }
    let h = |e: &ExportFile| (e.worldspace.clone(), e.south_west, e.hd_textures.clone(), e.atlas_tolerance, e.ignore_translation.clone());
    if h(gold) != h(ours) {
        println!("    header differs: golden {:?} ours {:?}", h(gold), h(ours));
    }
}

/// xLODGen's atlas settings for this setup (Plugins.sseviewsettings: AtlasWidth/Height 4096,
/// AtlasTextureSize 1024, AtlasTextureUVRange 1.5).
pub const ATLAS_SIZE: u32 = 4096;
pub const ATLAS_MAX_TEXTURE: u32 = 1024;
pub const ATLAS_UV_RANGE: f32 = 1.5;

/// The object LOD atlas layout for an export (`wbGetUVRangeTexturesList` + `wbBuildAtlas`).
pub fn atlas_layout(p: &Profile, inputs: &ExportInputs, st: &lodforge_objlod::source::ExportStats, season: Option<Season>) -> AtlasLayout {
    let vfs = &p.vfs;
    let mut list = Vec::new();
    for mesh in &st.lod_meshes {
        let Some(bytes) = vfs.read(&mesh.to_lowercase()).ok().flatten() else { continue };
        let Ok(nif) = lodforge_nif::parse(&bytes) else { continue };
        untiled_textures(mesh, &nif, ATLAS_UV_RANGE, &mut list);
    }
    let mut list = sorted_unique(list);
    let hd = texture_asset_name(&inputs.world.hd_lod_texture);
    list.retain(|t| !t.eq_ignore_ascii_case(&hd));
    let dims = |path: &str| -> Option<(u32, u32)> {
        let b = vfs.read(&path.to_lowercase()).ok().flatten()?;
        let info = lodforge_dds::parse(&b).ok()?;
        Some((info.width, info.height))
    };
    let mut tex = Vec::new();
    for t in list {
        let Some((w, h)) = dims(&t) else { continue };
        if w > ATLAS_MAX_TEXTURE || h > ATLAS_MAX_TEXTURE {
            continue;
        }
        let normal = normal_for(&t, |n| vfs.read(&n.to_lowercase()).ok().flatten().is_some());
        if dims(&normal).is_none() {
            continue;
        }
        tex.push((t, w, h));
    }
    let base = match season {
        Some(s) => format!("textures\\terrain\\{w}\\Objects\\{w}ObjectsLOD.{}", s.code(), w = inputs.world.editor_id),
        None => format!("textures\\terrain\\{w}\\Objects\\{w}ObjectsLOD", w = inputs.world.editor_id),
    };
    layout(&tex, ATLAS_SIZE, ATLAS_SIZE, &base)
}

/// The normal map xEdit pairs with an atlas diffuse (`_d` -> `_n`, else `_n` appended, else the
/// default flat normal).
pub fn normal_for(diffuse: &str, exists: impl Fn(&str) -> bool) -> String {
    let stem = |s: &str| s.rsplit_once('.').map(|(a, _)| a.to_string()).unwrap_or_else(|| s.to_string());
    let lower = diffuse.to_lowercase();
    let mut n = if lower.contains("_d.dds") {
        let i = lower.find("_d.dds").unwrap();
        let cand = format!("{}_n.dds{}", &diffuse[..i], &diffuse[i + 6..]);
        if exists(&cand) { cand } else { format!("{}_n.dds", stem(diffuse)) }
    } else {
        format!("{}_n.dds", stem(diffuse))
    };
    if !exists(&n) {
        n = "textures\\default_n.dds".into();
    }
    n
}

fn diff_map(golden: &str, ours: &AtlasLayout) {
    let ours_text = ours.map_text();
    let g: Vec<Vec<String>> = golden.lines().filter(|l| !l.trim().is_empty()).map(|l| l.split('\t').map(|c| c.to_lowercase()).collect()).collect();
    let o: Vec<Vec<String>> = ours_text.lines().map(|l| l.split('\t').map(|c| c.to_lowercase()).collect()).collect();
    let same_order = g.iter().map(|c| &c[0]).eq(o.iter().map(|c| &c[0]));
    let gm: HashMap<&String, &Vec<String>> = g.iter().map(|c| (&c[0], c)).collect();
    let om: HashMap<&String, &Vec<String>> = o.iter().map(|c| (&c[0], c)).collect();
    let mut equal = 0;
    let mut examples = Vec::new();
    for (k, gc) in &gm {
        match om.get(k) {
            Some(oc) if oc == gc => equal += 1,
            Some(oc) => {
                if examples.len() < 6 {
                    examples.push(format!("{k}: golden {:?} ours {:?}", &gc[1..], &oc[1..]));
                }
            }
            None => {
                if examples.len() < 6 {
                    examples.push(format!("{k}: only golden"));
                }
            }
        }
    }
    let only_ours: Vec<&&String> = om.keys().filter(|k| !gm.contains_key(**k)).collect();
    println!("  atlas map vs golden: {} golden entries, {} ours; {equal} identical; {} only ours; line order {}", g.len(), o.len(), only_ours.len(), if same_order { "same" } else { "differs" });
    for e in examples {
        println!("    {e}");
    }
    for k in only_ours.iter().take(4) {
        println!("    only ours: {k}");
    }
}