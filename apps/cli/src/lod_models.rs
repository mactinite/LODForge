//! `lodforge lod-models` and the `build --lod-models` stage: LOD models generated from the full
//! models (`lodforge-lodmodel`) for the bases DynDOLOD's rules would give object LOD but no LOD
//! model is installed for, so a build does not need DynDOLOD Resources
//! (docs/re/own-lod-resources.md).
//!
//! Output: `meshes\lod\lodforge\<folder of the full model>\<stem>_lod_<N>.nif` in a Data folder,
//! the names DynDOLOD's own LOD model discovery uses (`Folder4=meshes\lod`, bare file name), plus
//! `<stem>_<CRC32>_lod_<N>.nif` where several different full models share a file name. A cache
//! folder keyed by the full model's CRC32 and the generator version avoids regenerating.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use rayon::prelude::*;

use lodforge_dyndolod::lodmodels::{LodIndex, Snow, crc32, lod_set};
use lodforge_dyndolod::rules::{Level, select_mask_rule};
use lodforge_esp::Worldspace;
use lodforge_esp::record::FLAG_DELETED;

use crate::dyndolod_build::Config;
use crate::profile::{Profile, ProfileArgs};

/// Selection policy: the smallest base size (largest OBND extent times the largest reference
/// scale, game units) that gets a generated model level 0, 1, 2.
#[derive(Debug, Clone, Copy)]
pub struct Policy {
    pub min_size: [f32; 3],
    /// Generate every level the rules ask for, also where an installed LOD model exists (for
    /// comparisons; written under `meshes\lod\lodforge` all the same).
    pub all: bool,
}

pub const DEFAULT_MIN_SIZE: [f32; 3] = [256.0, 512.0, 1024.0];

#[derive(clap::Args)]
pub struct Args {
    #[command(flatten)]
    pub profile: ProfileArgs,
    #[arg(long, value_delimiter = ',', default_value = "Tamriel")]
    pub worlds: Vec<String>,
    /// DynDOLOD's `Edit Scripts\DynDOLOD` folder (default: `<instance>\tools\DynDOLOD\Edit Scripts\DynDOLOD`).
    #[arg(long)]
    pub dyndolod_config: Option<PathBuf>,
    #[arg(long, default_value = "Medium")]
    pub preset: String,
    /// Data folder to write the models into.
    #[arg(long)]
    pub out: PathBuf,
    /// Cache folder (default: none).
    #[arg(long)]
    pub cache: Option<PathBuf>,
    /// Smallest base size for levels 0, 1, 2 (comma separated).
    #[arg(long, value_delimiter = ',')]
    pub min_size: Option<Vec<f32>>,
    /// Generate every level the rules use, also where a LOD model is installed.
    #[arg(long)]
    pub all: bool,
    /// Compare triangle counts and coverage with the LOD models of this mod (e.g. DynDOLOD
    /// Resources), read from `<instance>\mods\<name>` (never copied).
    #[arg(long)]
    pub reference_mod: Option<String>,
    /// Geometric error limit per level 0, 1, 2 in game units (default: `lodforge_lodmodel::LEVELS`).
    #[arg(long, value_delimiter = ',')]
    pub max_error: Option<Vec<f32>>,
    /// Triangle cap per level as a share of the source.
    #[arg(long, value_delimiter = ',')]
    pub max_ratio: Option<Vec<f32>>,
    /// Write per-model results as JSON.
    #[arg(long)]
    pub json: Option<PathBuf>,
}

/// Level parameters with command-line overrides.
pub fn level_params(max_error: Option<&[f32]>, max_ratio: Option<&[f32]>) -> [lodforge_lodmodel::LevelParams; 3] {
    let mut l = lodforge_lodmodel::LEVELS;
    for (k, p) in l.iter_mut().enumerate() {
        if let Some(v) = max_error.and_then(|v| v.get(k).or(v.last())) {
            p.max_error = *v;
        }
        if let Some(v) = max_ratio.and_then(|v| v.get(k).or(v.last())) {
            p.max_ratio = *v;
        }
    }
    l
}

/// One full model to make LOD levels for.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Item {
    /// Data-relative full model path, lower-case.
    pub model: String,
    pub stem: String,
    pub crc: u32,
    /// Name prefix of the output files: `<stem>` or `<stem>_<CRC32>`.
    pub name: String,
    /// Model levels to generate.
    pub levels: Vec<u8>,
    /// Size used for the policy (game units).
    pub size: f32,
    /// OBND extent (x, y, z) times the largest reference scale.
    pub extent: [f32; 3],
    pub refs: usize,
    pub bases: Vec<String>,
}

/// Bases the rules give object LOD levels to that have no installed LOD model for them.
pub fn plan(p: &Profile, c: &Config, worlds: &[&Worldspace], policy: &Policy) -> Vec<Item> {
    // Per base: references and the largest scale.
    let mut used: HashMap<u32, (usize, f32)> = HashMap::new();
    for w in worlds {
        for q in w.refs.values() {
            if q.flags & FLAG_DELETED != 0 {
                continue;
            }
            let e = used.entry(q.base).or_insert((0, 0.0));
            e.0 += 1;
            e.1 = e.1.max(if q.has_scale { q.scale } else { 1.0 });
        }
    }
    // Per full model: the levels any of its bases needs.
    let mut by_model: BTreeMap<String, (HashSet<u8>, [f32; 3], usize, Vec<String>)> = BTreeMap::new();
    crate::dyndolod_build::with_resolver(p, c, |r| {
        for (id, (refs, scale)) in &used {
            let Some(b) = c.bases.get(id) else { continue };
            if lodforge_dyndolod::assign::is_tree(b) || b.model.is_empty() {
                continue;
            }
            let edid = b.editor_id.to_ascii_lowercase();
            if c.ignore_edid.iter().any(|e| edid.contains(e.as_str())) {
                continue;
            }
            let model = lodforge_dyndolod::bases::model_path(&b.model);
            if !policy.all && skip_model(&model) {
                continue;
            }
            let below = model.strip_prefix("meshes\\").unwrap_or(&model).to_string();
            // The rule as it would apply if every model level existed.
            let Some(rule) = select_mask_rule(&c.rules, &below, false, &|l| !matches!(l, Level::Billboard(_) | Level::None)) else { continue };
            let lod = r.resolve(b);
            if std::env::var("LODFORGE_PLAN_TRACE").is_ok_and(|t| edid.contains(&t.to_ascii_lowercase())) {
                eprintln!("plan-trace {} {model} rule {:?} lod {:?} scale {scale}", b.editor_id, rule.levels, lod.levels);
            }
            let ext = [0, 1, 2].map(|k| f32::from(b.bounds[k + 3]) - f32::from(b.bounds[k]));
            let size = ext.iter().copied().fold(0.0f32, f32::max) * scale;
            let mut need = HashSet::new();
            for l in &rule.levels[..3] {
                if let Level::Model(k) = *l
                    && k < 3
                    && (policy.all || lod.levels[usize::from(k)].is_none())
                    && size >= policy.min_size[usize::from(k)]
                {
                    need.insert(k);
                }
            }
            if need.is_empty() {
                continue;
            }
            let e = by_model.entry(model).or_insert((HashSet::new(), [0.0; 3], 0, Vec::new()));
            e.0.extend(need);
            for k in 0..3 {
                e.1[k] = e.1[k].max(ext[k] * scale);
            }
            e.2 += refs;
            e.3.push(b.editor_id.clone());
        }
    });
    // CRC32 of each full model (the original one for a PGPatcher-patched mesh, as discovery
    // uses it); several full models with one file name get CRC-named outputs.
    let models: Vec<(String, (HashSet<u8>, [f32; 3], usize, Vec<String>))> = by_model.into_iter().collect();
    let crcs: Vec<Option<u32>> = models.par_iter().map(|(m, _)| p.vfs.read(m).ok().flatten().map(|b| c.parallax.discovery_crc(m, crc32(&b)))).collect();
    let mut stems: HashMap<String, HashSet<u32>> = HashMap::new();
    for ((m, _), crc) in models.iter().zip(&crcs) {
        if let Some(c) = crc {
            stems.entry(stem_of(m)).or_default().insert(*c);
        }
    }
    let mut out = Vec::new();
    for ((model, (levels, extent, refs, bases)), crc) in models.into_iter().zip(crcs) {
        let size = extent.iter().copied().fold(0.0f32, f32::max);
        let Some(crc) = crc else { continue };
        let stem = stem_of(&model);
        let name = if stems[&stem].len() > 1 { format!("{stem}_{crc:08x}") } else { stem.clone() };
        let mut levels: Vec<u8> = levels.into_iter().collect();
        levels.sort_unstable();
        out.push(Item { model, stem, crc, name, levels, size, extent, refs, bases });
    }
    out
}

/// Models that never get a generated static LOD model: markers (invisible in game) and
/// effects (animated or effect-shader meshes, which belong to dynamic LOD).
fn skip_model(model: &str) -> bool {
    let name = model.rsplit('\\').next().unwrap_or(model);
    name.contains("marker") || model.contains("\\markers\\") || model.contains("\\effects\\")
}

fn stem_of(model: &str) -> String {
    model.rsplit('\\').next().unwrap_or(model).trim_end_matches(".nif").to_string()
}

/// `meshes\lod\lodforge\<folder of the full model below meshes>\`.
fn out_dir(model: &str) -> String {
    let below = model.strip_prefix("meshes\\").unwrap_or(model);
    let dir = below.rsplit_once('\\').map(|(d, _)| d).unwrap_or("");
    if dir.is_empty() { "meshes\\lod\\lodforge".into() } else { format!("meshes\\lod\\lodforge\\{dir}") }
}

/// One generated level.
#[derive(Debug, Clone, serde::Serialize)]
pub struct Made {
    pub path: String,
    pub level: u8,
    pub source_triangles: usize,
    /// Triangles written (after [`fit_uvs`]).
    pub triangles: usize,
    pub cached: bool,
    /// Shapes re-cut so they fit the object LOD atlas ([`fit_uvs`]).
    pub recut_shapes: usize,
    /// Shapes left tiled (on their own texture).
    pub tiled_shapes: usize,
}

#[derive(Debug, Default)]
pub struct Report {
    pub made: Vec<(usize, Made)>,
    pub failed: Vec<(String, String)>,
}

impl Report {
    /// One line on [`fit_uvs`]: shapes re-cut for the atlas, shapes left tiled.
    pub fn fit_summary(&self) -> String {
        let (r, t) = self.made.iter().fold((0, 0), |(r, t), (_, m)| (r + m.recut_shapes, t + m.tiled_shapes));
        format!("{r} shapes re-cut to fit the object LOD atlas, {t} left tiled (would grow past {MAX_RECUT_GROWTH}x + {RECUT_SLACK} triangles)")
    }
}

/// Generates the planned levels into `out` (a Data folder), through `cache` when given.
pub fn generate(p: &Profile, items: &[Item], out: &Path, cache: Option<&Path>, params: &[lodforge_lodmodel::LevelParams; 3]) -> Report {
    let results: Vec<(usize, Result<Vec<Made>, String>)> = items
        .par_iter()
        .enumerate()
        .map(|(i, it)| {
            let r = (|| -> Result<Vec<Made>, String> {
                let bytes = p.vfs.read(&it.model).ok().flatten().ok_or("full model missing")?;
                let mut made = Vec::new();
                let dir = out_dir(&it.model);
                for &k in &it.levels {
                    let rel = format!("{dir}\\{}_lod_{k}.nif", it.name);
                    let dest = out.join(&rel);
                    let pk = params[usize::from(k)];
                    let key = cache.map(|c| c.join(format!("v{}", lodforge_lodmodel::GENERATOR_VERSION)).join(format!("{:08x}_{k}_{:08x}.nif", it.crc, crc32(format!("{pk:?}").as_bytes()))));
                    let (data, st, cached) = match key.as_ref().and_then(|k| std::fs::read(k).ok()) {
                        Some(d) => {
                            let tris = lodforge_nif::parse(&d).map(|n| n.blocks.iter().map(|b| if let lodforge_nif::Block::TriShape(s) = b { s.triangles.len() } else { 0 }).sum()).unwrap_or(0);
                            (d, lodforge_lodmodel::Stats { triangles: tris, ..Default::default() }, true)
                        }
                        None => {
                            let (d, st) = lodforge_lodmodel::generate(&bytes, &params[usize::from(k)], &it.name).map_err(|e| format!("{e:?}"))?;
                            if let Some(k) = &key {
                                let _ = std::fs::create_dir_all(k.parent().unwrap()).and_then(|_| std::fs::write(k, &d));
                            }
                            (d, st, false)
                        }
                    };
                    // Atlas-ready UVs (after the cache, so cached models get them too).
                    let (data, fit) = fit_uvs(&data)?;
                    std::fs::create_dir_all(dest.parent().unwrap()).map_err(|e| e.to_string())?;
                    std::fs::write(&dest, &data).map_err(|e| e.to_string())?;
                    made.push(Made { path: rel, level: k, source_triangles: st.source_triangles, triangles: fit.triangles_after, cached, recut_shapes: fit.recut, tiled_shapes: fit.tiled });
                }
                Ok(made)
            })();
            (i, r)
        })
        .collect();
    let mut rep = Report::default();
    for (i, r) in results {
        match r {
            Ok(m) => rep.made.extend(m.into_iter().map(|m| (i, m))),
            Err(e) => rep.failed.push((items[i].model.clone(), e)),
        }
    }
    rep
}

/// How much a shape may grow when its repeating UVs are re-cut for the atlas ([`fit_uvs`]):
/// `MAX_RECUT_GROWTH` times its triangles plus [`RECUT_SLACK`] (small shapes cross a few repeat
/// lines however few triangles they have). A shape that would grow more keeps its tiled UVs (and
/// its own texture).
pub const MAX_RECUT_GROWTH: usize = 4;
pub const RECUT_SLACK: usize = 64;

/// The most triangles a re-cut shape of `before` triangles may have.
pub fn recut_limit(before: usize) -> usize {
    before * MAX_RECUT_GROWTH + RECUT_SLACK
}

/// A lower bound of the triangles re-cutting a shape makes: each triangle becomes at least one
/// piece per integer UV cell its UV box spans along its longer side. Cheap; lets [`fit_uvs`]
/// skip shapes whose UVs repeat hundreds of times without cutting them first.
pub fn recut_lower_bound(uvs: &[[f32; 2]], triangles: &[[u16; 3]]) -> usize {
    triangles
        .iter()
        .map(|t| {
            let span = |k: usize| {
                let (lo, hi) = t.iter().fold((f32::MAX, f32::MIN), |(lo, hi), &i| (lo.min(uvs[i as usize][k]), hi.max(uvs[i as usize][k])));
                (hi.floor() - lo.floor()).max(0.0) as usize + 1
            };
            span(0).max(span(1))
        })
        .sum()
}

/// What [`fit_uvs`] did to one model.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Fit {
    pub shapes: usize,
    /// Shapes re-cut so their UVs lie in 0..1.
    pub recut: usize,
    /// Shapes whose UVs still repeat (re-cutting would grow them past [`recut_limit`]).
    pub tiled: usize,
    pub triangles_before: usize,
    pub triangles_after: usize,
}

/// Makes a generated LOD model atlas-ready: every shape whose UVs (with its shader's UV
/// offset / scale applied) repeat beyond one texture is re-cut along the integer UV lines and
/// its UVs wrapped into 0..1 ([`lodforge_objlod::reuv`]), and the shader's UV transform is reset.
/// Generated models come from full models, whose textures mostly tile; with tiled UVs the atlas
/// planner keeps their full textures off the pages, so each such shape becomes a draw of its own
/// in every block instead of merging with the block's other LOD shapes.
pub fn fit_uvs(bytes: &[u8]) -> Result<(Vec<u8>, Fit), String> {
    use lodforge_nif::Block;
    let mut nif = lodforge_nif::parse(bytes).map_err(|e| e.to_string())?;
    let mesh = lodforge_objlod::mesh::import("", bytes).map_err(|e| e.to_string())?;
    // Generated models: a root node whose children are the shapes, in the importer's order.
    let shape_blocks: Vec<usize> = match nif.blocks.first() {
        Some(Block::Node(n)) => n.children.iter().flatten().map(|&c| c as usize).filter(|&c| matches!(nif.blocks.get(c), Some(Block::TriShape(_)))).collect(),
        _ => return Err("not a generated LOD model".into()),
    };
    let mut fit = Fit::default();
    if shape_blocks.len() != mesh.parts.len() {
        return Err(format!("{} shapes, {} imported parts", shape_blocks.len(), mesh.parts.len()));
    }
    for (&bi, part) in shape_blocks.iter().zip(&mesh.parts) {
        fit.shapes += 1;
        fit.triangles_before += part.triangles.len();
        if !lodforge_objlod::reuv::needs_recut(&part.uvs, 1e-3) {
            fit.triangles_after += part.triangles.len();
            continue;
        }
        // Shapes that would certainly grow too much are not cut at all (cutting one that repeats
        // its texture hundreds of times takes long and makes millions of pieces).
        if recut_lower_bound(&part.uvs, &part.triangles) > recut_limit(part.triangles.len()) {
            fit.tiled += 1;
            fit.triangles_after += part.triangles.len();
            continue;
        }
        let mut r = lodforge_objlod::reuv::recut(part);
        // Cuts through a corner leave zero-area slivers; they draw nothing.
        r.triangles.retain(|t| {
            let [a, b, c] = t.map(|i| r.positions[i as usize]);
            let (e, f) = ([b[0] - a[0], b[1] - a[1], b[2] - a[2]], [c[0] - a[0], c[1] - a[1], c[2] - a[2]]);
            let n = [e[1] * f[2] - e[2] * f[1], e[2] * f[0] - e[0] * f[2], e[0] * f[1] - e[1] * f[0]];
            n[0] * n[0] + n[1] * n[1] + n[2] * n[2] > 1e-6
        });
        if r.triangles.len() > recut_limit(part.triangles.len()) || r.positions.len() > usize::from(u16::MAX) {
            fit.tiled += 1;
            fit.triangles_after += part.triangles.len();
            continue;
        }
        fit.recut += 1;
        fit.triangles_after += r.triangles.len();
        let Some(Block::TriShape(s)) = nif.blocks.get_mut(bi) else { unreachable!() };
        // Only the attributes the shape stores (the importer fills in the others).
        let had = (!s.uvs.is_empty(), !s.normals.is_empty(), !s.tangents.is_empty(), !s.bitangents.is_empty(), !s.colors.is_empty());
        s.positions = r.positions.clone();
        s.uvs = r.uvs.clone();
        s.normals = if had.1 { r.normals.clone() } else { Vec::new() };
        s.tangents = if had.2 { r.tangents.clone() } else { Vec::new() };
        s.bitangents = if had.3 { r.bitangents.clone() } else { Vec::new() };
        s.colors = if had.4 { r.colors.clone().unwrap_or_else(|| vec![[255; 4]; r.positions.len()]) } else { Vec::new() };
        s.triangles = r.triangles.clone();
        let shader = s.shader_property;
        // The UV transform is in the vertices now.
        if let Some(Block::LightingShader(l)) = shader.and_then(|i| nif.blocks.get_mut(i as usize)) {
            l.uv_offset = [0.0; 2];
            l.uv_scale = [1.0; 2];
        }
    }
    if fit.recut == 0 {
        return Ok((bytes.to_vec(), fit));
    }
    Ok((nif.to_bytes().map_err(|e| e.to_string())?, fit))
}

/// LOD models of a mod folder (reference for comparisons): an index over its `meshes`.
fn reference_index(root: &Path) -> (LodIndex, Vec<String>) {
    let mut paths = Vec::new();
    let mut stack = vec![(root.join("meshes"), "meshes".to_string())];
    while let Some((dir, rel)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_ascii_lowercase();
            let r = format!("{rel}\\{n}");
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                stack.push((e.path(), r));
            } else if n.ends_with(".nif") {
                paths.push(r);
            }
        }
    }
    let idx = LodIndex::scan(paths.iter().map(String::as_str), &["meshes".to_string()], &[], &Default::default());
    (idx, paths)
}

fn find_dir_ci(root: &Path, name: &str) -> Option<PathBuf> {
    std::fs::read_dir(root).ok()?.flatten().find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(name)).map(|e| e.path())
}

fn nif_triangles(path: &Path) -> usize {
    std::fs::read(path).ok().and_then(|b| lodforge_nif::parse(&b).ok()).map(|n| n.blocks.iter().map(|b| if let lodforge_nif::Block::TriShape(s) = b { s.triangles.len() } else { 0 }).sum()).unwrap_or(0)
}

pub fn run(a: &Args) -> Result<()> {
    let t = Instant::now();
    let p = a.profile.open()?;
    let dir = a.dyndolod_config.clone().unwrap_or_else(|| a.profile.instance.join(r"tools\DynDOLOD\Edit Scripts\DynDOLOD"));
    let c = Config::load(&p, &dir, &a.preset, false)?;
    let all_worlds = lodforge_esp::scan_worldspaces(&p.load_order)?;
    let worlds: Vec<&Worldspace> = a.worlds.iter().filter_map(|n| all_worlds.iter().find(|w| w.editor_id.eq_ignore_ascii_case(n))).collect();
    let min_size = a.min_size.as_ref().map(|v| [0, 1, 2].map(|k| v.get(k).or(v.last()).copied().unwrap_or(0.0))).unwrap_or(DEFAULT_MIN_SIZE);
    let policy = Policy { min_size, all: a.all };
    let items = plan(&p, &c, &worlds, &policy);
    let n_levels: usize = items.iter().map(|i| i.levels.len()).sum();
    println!("plan: {} full models, {} levels, {} references (min size {:?}) in {} ms", items.len(), n_levels, items.iter().map(|i| i.refs).sum::<usize>(), min_size, t.elapsed().as_millis());
    let t = Instant::now();
    let params = level_params(a.max_error.as_deref(), a.max_ratio.as_deref());
    let rep = generate(&p, &items, &a.out, a.cache.as_deref(), &params);
    let gen_ms = t.elapsed().as_millis();
    let mut src = [0usize; 3];
    let mut ours = [0usize; 3];
    let mut count = [0usize; 3];
    for (_, m) in &rep.made {
        let k = usize::from(m.level);
        src[k] += m.source_triangles;
        ours[k] += m.triangles;
        count[k] += 1;
    }
    println!("generated {} files in {gen_ms} ms ({} from cache), {} models failed; {}", rep.made.len(), rep.made.iter().filter(|(_, m)| m.cached).count(), rep.failed.len(), rep.fit_summary());
    for k in 0..3 {
        println!("  level {k}: {:>5} models, {:>9} source triangles -> {:>8} ({:.1}%), {:.0} per model", count[k], src[k], ours[k], 100.0 * ours[k] as f64 / src[k].max(1) as f64, ours[k] as f64 / count[k].max(1) as f64);
    }
    for (m, e) in rep.failed.iter().take(10) {
        println!("  failed {m}: {e}");
    }
    let mut rows: Vec<serde_json::Value> = Vec::new();
    if let Some(name) = &a.reference_mod {
        let root = a.profile.instance.join("mods").join(name);
        let (ridx, _) = reference_index(&root);
        let mroot = find_dir_ci(&root, "meshes").unwrap_or(root.join("meshes"));
        // Per level: models where both exist, triangles ours vs reference; coverage both ways.
        let mut both = [0usize; 3];
        let mut ours_t = [0usize; 3];
        let mut ref_t = [0usize; 3];
        let mut only_ours = [0usize; 3];
        let mut covered = 0usize;
        let mut ref_items = 0usize;
        let mut sizes: BTreeMap<u32, [usize; 2]> = BTreeMap::new();
        let made_by: HashMap<(usize, u8), &Made> = rep.made.iter().map(|(i, m)| ((*i, m.level), m)).collect();
        for (i, it) in items.iter().enumerate() {
            let rl = lod_set(&ridx, &it.stem, Some(it.crc), [Snow::None; 4], false);
            let has_ref = rl.iter().any(Option::is_some);
            ref_items += usize::from(has_ref);
            let bucket = (it.size.max(1.0).log2().floor() as u32).min(16);
            let e = sizes.entry(bucket).or_default();
            e[0] += 1;
            e[1] += usize::from(has_ref);
            let mut rt = [0usize; 3];
            for &k in &it.levels {
                let ku = usize::from(k);
                let Some(m) = made_by.get(&(i, k)) else { continue };
                match &rl[ku] {
                    Some(r) => {
                        let rel = r.strip_prefix("meshes\\").unwrap_or(r);
                        let tris = nif_triangles(&mroot.join(rel));
                        rt[ku] = tris;
                        both[ku] += 1;
                        ours_t[ku] += m.triangles;
                        ref_t[ku] += tris;
                    }
                    None => only_ours[ku] += 1,
                }
            }
            if has_ref {
                covered += 1;
            }
            if a.json.is_some() {
                rows.push(serde_json::json!({ "item": it, "reference_levels": rl, "reference_triangles": rt }));
            }
        }
        println!("\nagainst {name}: {covered}/{} planned models have a reference LOD model", items.len());
        for k in 0..3 {
            println!(
                "  level {k}: {:>5} with both, triangles ours {:>8} vs reference {:>8} (x{:.2}); {:>5} ours only",
                both[k], ours_t[k], ref_t[k], ours_t[k] as f64 / ref_t[k].max(1) as f64, only_ours[k]
            );
        }
        println!("  planned models by size (2^n units): share with a reference model");
        for (b, [n, r]) in &sizes {
            println!("    2^{b:<2} {n:>5} models, {r:>5} with reference ({:.0}%)", 100.0 * *r as f64 / (*n).max(1) as f64);
        }
        let _ = ref_items;
    }
    if let Some(j) = &a.json {
        let made: Vec<serde_json::Value> = rep.made.iter().map(|(i, m)| serde_json::json!({ "model": items[*i].model, "made": m })).collect();
        std::fs::write(j, serde_json::to_vec_pretty(&serde_json::json!({ "items": rows, "made": made }))?).with_context(|| format!("writing {}", j.display()))?;
    }
    Ok(())
}

/// What [far_from_lod0] did.
#[derive(Debug, Default)]
pub struct FarReport {
    /// Reference lines (all seasons) whose LOD8/16 model columns were replaced.
    pub lines: usize,
    /// Generated models (data-relative paths).
    pub written: Vec<String>,
    pub failed: Vec<(String, String)>,
}

/// The diffuse textures (lower case, `textures\...`) of a model's visible lighting shapes.
fn model_diffuses(p: &Profile, model: &str) -> Option<(Vec<String>, usize)> {
    let bytes = p.vfs.read(&model.to_ascii_lowercase()).ok().flatten()?;
    let nif = lodforge_nif::parse(&bytes).ok()?;
    let mut out = Vec::new();
    let mut tris = 0;
    for b in &nif.blocks {
        let lodforge_nif::Block::TriShape(s) = b else { continue };
        if s.av.flags & 1 != 0 {
            continue;
        }
        tris += s.triangles.len();
        let Some(lodforge_nif::Block::LightingShader(l)) = s.shader_property.and_then(|r| nif.blocks.get(r as usize)) else { continue };
        let Some(lodforge_nif::Block::TextureSet(t)) = l.texture_set.and_then(|r| nif.blocks.get(r as usize)) else { continue };
        if let Some(d) = t.first().filter(|d| !d.is_empty()) {
            out.push(lodforge_objlod::export::texture_key(d));
        }
    }
    Some((out, tris))
}

/// Far LOD models rebuilt from the LOD4 model. A reference whose LOD8/16 model (`_lod_1`,
/// `_lod_2`) shows a pre-baked LOD texture (`textures\lod\...`) none of its LOD4 model's shapes
/// use switches texture, UV layout and normal map at the LOD4 -> LOD8 boundary: mountain cliffs
/// show the current (e.g. PBR) mountain texture up close and a vanilla-look bake from a LOD
/// texture pack beyond (`textures\lod\mtncliff04lod.dds`). For those, the LOD4 model is
/// simplified to the triangle budget of the shipped far model, keeping its textures, UVs and
/// shape names (so AltTextures apply as at LOD4), written to
/// `meshes\lod\lodforge\far\<stem>_<crc>_far<level>.nif`, and the export's columns are pointed at
/// it. Returns what was replaced; the caller adds `written` to the VFS.
pub fn far_from_lod0(p: &Profile, exports: &mut crate::dyndolod_build::WorldExport, out: &Path, cache: Option<&Path>) -> FarReport {
    let norm = |m: &str| m.trim().to_ascii_lowercase().replace('/', "\\");
    // Candidate (LOD4 model, level, far model) pairs.
    let mut pairs: HashSet<(String, usize, String)> = HashSet::new();
    for s in &exports.seasons {
        for l in &s.lines {
            let c0 = norm(&l.columns[0]);
            if !c0.ends_with(".nif") || c0.contains("passthru_lod") || c0.contains("glow_lod") {
                continue;
            }
            for k in 1..=2 {
                let ck = norm(&l.columns[k]);
                if ck.ends_with(".nif") && ck != c0 {
                    pairs.insert((c0.clone(), k, ck));
                }
            }
        }
    }
    let models: HashSet<String> = pairs.iter().flat_map(|(a, _, b)| [a.clone(), b.clone()]).collect();
    let info: HashMap<String, (Vec<String>, usize)> = models.into_par_iter().filter_map(|m| model_diffuses(p, &m).map(|d| (m, d))).collect();
    let baked = |t: &str| t.starts_with("textures\\lod\\");
    let wanted: Vec<(String, usize, String)> = pairs
        .into_iter()
        .filter(|(c0, _, ck)| {
            let (Some((d0, _)), Some((dk, _))) = (info.get(c0), info.get(ck)) else { return false };
            !d0.is_empty() && !dk.is_empty() && d0.iter().all(|t| !baked(t)) && dk.iter().all(|t| baked(t) && !d0.contains(t))
        })
        .collect();
    let mut rep = FarReport::default();
    // Generate per (LOD4 model, level, budget).
    let made: Vec<((String, usize, String), Result<String, String>)> = wanted
        .par_iter()
        .map(|(c0, k, ck)| {
            let r = (|| -> Result<String, String> {
                let bytes = p.vfs.read(c0).ok().flatten().ok_or("LOD4 model missing")?;
                let (_, t0) = info[c0];
                let (_, tk) = info[ck];
                let base = lodforge_lodmodel::LEVELS[*k];
                let params = lodforge_lodmodel::LevelParams { max_ratio: (tk as f32 / t0.max(1) as f32).clamp(0.02, 1.0), min_triangles: 8, permissive: false, ..base };
                let stem = c0.rsplit('\\').next().unwrap_or(c0).trim_end_matches(".nif").to_string();
                // The stem keeps `_lod_0`: the model stays an HD LOD model (DynDOLOD's `IsHDMeshMask`),
                // so its HD-style vertex alpha keeps meaning the snow mask of the HD snow shader.
                let rel = format!("meshes\\lod\\lodforge\\far\\{stem}_{:08x}_far{k}.nif", crc32(format!("{c0}|{ck}").as_bytes()));
                let key = cache.map(|c| c.join(format!("v{}", lodforge_lodmodel::GENERATOR_VERSION)).join("far").join(format!("{:08x}_{k}_{:08x}.nif", crc32(&bytes), crc32(format!("{params:?}").as_bytes()))));
                let data = match key.as_ref().and_then(|k| std::fs::read(k).ok()) {
                    Some(d) => d,
                    None => {
                        let (d, _) = lodforge_lodmodel::generate_keep_names(&bytes, &params, &stem).map_err(|e| format!("{e:?}"))?;
                        if let Some(k) = &key {
                            let _ = std::fs::create_dir_all(k.parent().unwrap()).and_then(|_| std::fs::write(k, &d));
                        }
                        d
                    }
                };
                let dest = out.join(rel.replace('\\', "/"));
                std::fs::create_dir_all(dest.parent().unwrap()).map_err(|e| e.to_string())?;
                std::fs::write(&dest, &data).map_err(|e| e.to_string())?;
                Ok(rel)
            })();
            ((c0.clone(), *k, ck.clone()), r)
        })
        .collect();
    let mut replace: HashMap<(String, usize, String), String> = HashMap::new();
    for (key, r) in made {
        match r {
            Ok(rel) => {
                rep.written.push(rel.clone());
                replace.insert(key, rel);
            }
            Err(e) => rep.failed.push((key.0, e)),
        }
    }
    rep.written.sort();
    // Columns and the export text (fields 13 + k of a reference line).
    for s in &mut exports.seasons {
        let mut by_ref: HashMap<String, [Option<String>; 3]> = HashMap::new();
        for l in &mut s.lines {
            let c0 = norm(&l.columns[0]);
            let mut new: [Option<String>; 3] = Default::default();
            for k in 1..=2 {
                if let Some(rel) = replace.get(&(c0.clone(), k, norm(&l.columns[k]))) {
                    l.columns[k] = rel.clone();
                    new[k] = Some(rel.clone());
                }
            }
            if new.iter().any(Option::is_some) {
                rep.lines += 1;
                by_ref.insert(format!("{:08X}", l.ref_id), new);
            }
        }
        if by_ref.is_empty() {
            continue;
        }
        let text: Vec<String> = s
            .text
            .split("\r\n")
            .map(|line| {
                let Some(new) = line.split('\t').next().and_then(|id| by_ref.get(id)) else { return line.to_string() };
                let mut f: Vec<String> = line.split('\t').map(str::to_string).collect();
                for (k, n) in new.iter().enumerate() {
                    if let (Some(n), Some(field)) = (n, f.get_mut(13 + k)) {
                        *field = n.clone();
                    }
                }
                f.join("\t")
            })
            .collect();
        s.text = text.join("\r\n");
    }
    rep
}

#[cfg(test)]
mod tests {
    use lodforge_nif::{AvObject, Block, LightingShader, Nif, Node, ObjectNet, TriShape, VertexDesc};

    use super::{fit_uvs, recut_limit, recut_lower_bound};

    #[test]
    fn recut_bound() {
        let uvs = [[0.0, 0.0], [0.5, 0.0], [0.0, 0.5], [300.0, 0.0], [0.0, 2.5]];
        // Inside one repeat: 1 piece; spanning 301 repeats in u: at least 301; 3 in v: 3.
        assert_eq!(recut_lower_bound(&uvs, &[[0, 1, 2]]), 1);
        assert_eq!(recut_lower_bound(&uvs, &[[0, 3, 2], [0, 1, 4]]), 301 + 3);
        assert!(recut_lower_bound(&uvs, &[[0, 3, 2]]) > recut_limit(1));
    }

    /// A generated-style model: a root node with one quad shape per (u repeats, v repeats,
    /// shader UV scale).
    fn model(quads: &[(f32, f32, f32)]) -> Vec<u8> {
        let mut nif = Nif::new();
        nif.add_block(Block::Node(Node { av: AvObject { net: ObjectNet::named("m"), ..Default::default() }, ..Default::default() }));
        let mut children = Vec::new();
        for &(u, v, scale) in quads {
            let ts = nif.add_block(Block::TextureSet(vec![r"textures\architecture\wall.dds".into(), r"textures\architecture\wall_n.dds".into()]));
            let sh = nif.add_block(Block::LightingShader(LightingShader { texture_set: Some(ts), uv_scale: [scale; 2], ..Default::default() }));
            let s = TriShape {
                av: AvObject { net: ObjectNet::named("m"), ..Default::default() },
                shader_property: Some(sh),
                vertex_desc: VertexDesc::with_attributes(true, true, true, false, false).0,
                positions: vec![[0.0, 0.0, 0.0], [100.0, 0.0, 0.0], [100.0, 0.0, 100.0], [0.0, 0.0, 100.0]],
                uvs: vec![[0.0, 0.0], [u, 0.0], [u, v], [0.0, v]],
                normals: vec![[0.0, -1.0, 0.0]; 4],
                triangles: vec![[0, 1, 2], [0, 2, 3]],
                ..Default::default()
            };
            children.push(Some(nif.add_block(Block::TriShape(s))));
        }
        if let Block::Node(r) = &mut nif.blocks[0] {
            r.children = children;
        }
        nif.to_bytes().unwrap()
    }

    #[test]
    fn repeating_uvs_are_recut_into_one_repeat() {
        // 3 x 1 repeats; 1 x 1 UVs drawn with a shader UV scale of 2 (2 x 2 repeats); 1 x 1.
        let (bytes, fit) = fit_uvs(&model(&[(3.0, 1.0, 1.0), (1.0, 1.0, 2.0), (1.0, 1.0, 1.0)])).unwrap();
        assert_eq!((fit.shapes, fit.recut, fit.tiled, fit.triangles_before), (3, 2, 0, 6));
        assert!(fit.triangles_after > 6);
        let nif = lodforge_nif::parse(&bytes).unwrap();
        for b in &nif.blocks {
            match b {
                Block::TriShape(s) => {
                    assert!(s.uvs.iter().all(|uv| (-1e-4..=1.0 + 1e-4).contains(&uv[0]) && (-1e-4..=1.0 + 1e-4).contains(&uv[1])), "{:?}", s.uvs);
                    assert_eq!(s.uvs.len(), s.positions.len());
                    assert_eq!(s.normals.len(), s.positions.len());
                    // The shape still covers the same quad.
                    let area: f32 = s.triangles.iter().map(|t| {
                        let [a, b, c] = t.map(|i| s.positions[i as usize]);
                        let (e, f) = ([b[0] - a[0], b[2] - a[2]], [c[0] - a[0], c[2] - a[2]]);
                        (e[0] * f[1] - e[1] * f[0]).abs() / 2.0
                    }).sum();
                    assert!((area - 10000.0).abs() < 1.0, "area {area}");
                    // No zero-area slivers from cuts through corners.
                    assert!(s.triangles.iter().all(|t| {
                        let [a, b, c] = t.map(|i| s.positions[i as usize]);
                        ((b[0] - a[0]) * (c[2] - a[2]) - (b[2] - a[2]) * (c[0] - a[0])).abs() > 1e-3
                    }));
                }
                Block::LightingShader(l) => assert_eq!((l.uv_offset, l.uv_scale), ([0.0; 2], [1.0; 2])),
                _ => {}
            }
        }
    }

    #[test]
    fn heavy_repeats_stay_tiled_and_fitting_models_are_unchanged() {
        // 20 x 20 repeats would turn 2 triangles into ~800: kept tiled.
        let src = model(&[(20.0, 20.0, 1.0)]);
        let (bytes, fit) = fit_uvs(&src).unwrap();
        assert_eq!((fit.recut, fit.tiled, fit.triangles_after), (0, 1, 2));
        assert!(fit.triangles_after <= recut_limit(2));
        assert_eq!(bytes, src);
        let src = model(&[(1.0, 1.0, 1.0)]);
        assert_eq!(fit_uvs(&src).unwrap().0, src);
    }
}
