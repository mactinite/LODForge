// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! `lodforge impostors` (and `build --impostors`): hemi-octahedral impostor atlases for the tree
//! types of a worldspace, for the LODForge runtime (docs/runtime/impostors.md).
//!
//! Output, per worldspace, under `textures\LODForge\Impostors\<World>\`: atlas pages
//! (`pageNN.dds` albedo + coverage, `pageNN_nd.dds` normal + depth, BC7 with mips), one manifest
//! per type (`types\<name>.json`) and the set index (`impostors.json`; a type's position in its
//! `types` list is the type index instance files use). Bakes are cached by model and texture
//! CRC, layout and baker version.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result, bail};

use lodforge_impostor::manifest::{BaseRef, Index, PageInfo, TypeManifest};
use lodforge_impostor::{atlas, bake};
use lodforge_texgen::{Renderer, Scene, TexData};

use crate::profile::{Profile, ProfileArgs};
use crate::texgen_billboards::Candidate;

/// Bake and layout options shared by the command and the build flag.
#[derive(clap::Args, Clone, Debug)]
pub struct Opts {
    /// Frames per side (12..16 recommended); 0 = 16 for frames up to 64 px, else 12.
    #[arg(long = "impostor-grid", default_value_t = 0)]
    pub grid: u32,
    /// Frame size in pixels; 0 = sized by the tree's height (see --impostor-units-per-px).
    #[arg(long = "impostor-frame", default_value_t = 0)]
    pub frame: u32,
    /// Auto frame size: next power of two of (height x scale) / this, clamped to the min/max.
    #[arg(long = "impostor-units-per-px", default_value_t = 12.0)]
    pub units_per_px: f32,
    #[arg(long = "impostor-min-frame", default_value_t = 32)]
    pub min_frame: u32,
    #[arg(long = "impostor-max-frame", default_value_t = 128)]
    pub max_frame: u32,
    /// Atlas page size (texels per side); 0 = one page per type.
    #[arg(long = "impostor-page", default_value_t = 4096)]
    pub page: u32,
    /// Bake cache folder; default `%LOCALAPPDATA%\LODForge\cache\impostors`.
    #[arg(long = "impostor-cache")]
    pub cache: Option<PathBuf>,
    /// Bake every type again (the cache is still written).
    #[arg(long = "impostor-no-cache")]
    pub no_cache: bool,
    /// Also write every page as PNG (albedo, normal, depth) into this folder, for inspection.
    #[arg(long = "impostor-png")]
    pub png: Option<PathBuf>,
    /// Large trees (world height = model height x reference scale up to 1.25, in units) at
    /// least this tall get --impostor-large-frame px frames.
    #[arg(long = "impostor-large-height", default_value_t = 2600.0)]
    pub large_height: f32,
    /// Frame size of large trees; 0 = no large tier.
    #[arg(long = "impostor-large-frame", default_value_t = 256)]
    pub large_frame: u32,
    /// Mip bias of the tree textures in the bake (negative keeps the foliage's alpha-tested
    /// coverage; 0 renders them as the billboards do).
    #[arg(long = "impostor-mip-bias", default_value_t = -2.0, allow_hyphen_values = true)]
    pub mip_bias: f32,
    /// Leave the albedo unlit (no baked light, transmittance or occlusion).
    #[arg(long = "impostor-unlit")]
    pub unlit: bool,
    /// Per-type colour correction: CSV `type,brightness,r,g,b` (type = manifest name, or `*` for
    /// every type; later lines win), written into the manifests.
    #[arg(long = "impostor-calibration")]
    pub calibration: Option<PathBuf>,
}

impl Opts {
    fn settings(&self) -> bake::Settings {
        let mut s = bake::Settings { grid: self.grid, frame_px: self.frame, units_per_px: self.units_per_px, min_frame: self.min_frame, max_frame: self.max_frame, large_height: self.large_height, large_frame: self.large_frame, mip_bias: self.mip_bias, ..Default::default() };
        s.shading.enabled = !self.unlit;
        s
    }

    /// `type -> (brightness, tint)` from --impostor-calibration (`*` = every type).
    fn calibration(&self) -> Result<Vec<(String, f32, [f32; 3])>> {
        let Some(f) = &self.calibration else { return Ok(Vec::new()) };
        let text = std::fs::read_to_string(f).with_context(|| format!("reading {}", f.display()))?;
        let mut out = Vec::new();
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with("type,")) {
            let c: Vec<&str> = line.split(',').map(str::trim).collect();
            let num = |i: usize, d: f32| c.get(i).and_then(|v| v.parse::<f32>().ok()).unwrap_or(d);
            out.push((c[0].to_ascii_lowercase(), num(1, 1.0), [num(2, 1.0), num(3, 1.0), num(4, 1.0)]));
        }
        Ok(out)
    }

    fn cache_dir(&self) -> PathBuf {
        self.cache.clone().unwrap_or_else(|| std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join(r"LODForge\cache\impostors"))
    }
}

#[derive(clap::Args)]
pub struct Args {
    #[command(flatten)]
    pub profile: ProfileArgs,
    /// Worldspaces whose tree types to bake (comma list); default Tamriel.
    #[arg(long, value_delimiter = ',')]
    pub worlds: Vec<String>,
    /// Extra seasons (SPR, SUM, AUT, WIN): their swapped tree bases are baked too.
    #[arg(long, value_delimiter = ',')]
    pub seasons: Vec<String>,
    /// Only types whose model path or base EditorID contains this (case-insensitive).
    #[arg(long)]
    pub filter: Option<String>,
    /// Bake these models instead of a worldspace's trees (Data-relative paths, comma list);
    /// written as the set `Models`.
    #[arg(long, value_delimiter = ',')]
    pub models: Vec<String>,
    /// Output Data folder.
    #[arg(long)]
    pub out: PathBuf,
    /// DynDOLOD folder (TexGen `_static` replacement models and billboard overrides); read only.
    /// Defaults to `<instance>\tools\DynDOLOD`.
    #[arg(long)]
    pub dyndolod: Option<PathBuf>,
    /// Also write `preview-instances.bin` (every tree reference with its type) for the viewer's
    /// impostor preview, until the runtime instance files exist.
    #[arg(long)]
    pub preview_instances: bool,
    /// Per-type report (CSV: time, layout, texels, bytes); default
    /// `<out>\LODForge_ImpostorReport_<set>.csv`.
    #[arg(long)]
    pub report: Option<PathBuf>,
    #[command(flatten)]
    pub opts: Opts,
}

/// One type to bake: its render model and the bases that use it.
struct TypeJob {
    name: String,
    /// Data-relative (lower case) model the bake renders.
    model: String,
    record_model: String,
    /// Replacement model file outside the VFS (TexGen `_static`), if any.
    file: Option<PathBuf>,
    bases: Vec<(u32, BaseRef)>,
    /// CRC-32 of the model file (set when loaded).
    crc: u32,
    /// Largest reference scale of its bases in the worldspace (frame sizing), 1..1.25.
    scale: f32,
}

/// Per-type result for the report.
pub struct TypeReport {
    pub name: String,
    pub model: String,
    pub refs: usize,
    pub grid: u32,
    pub frame_px: u32,
    pub atlas_px: u32,
    pub quad_size: f32,
    pub ms: u128,
    pub cached: bool,
    /// BC7 bytes of its two maps with mips (its share of the pages).
    pub bytes: u64,
    pub coverage: f32,
}

/// What a set bake did.
pub struct SetReport {
    pub types: Vec<TypeReport>,
    pub pages: usize,
    pub page_bytes: u64,
    pub bake_ms: u128,
    pub encode_ms: u128,
    pub failed: Vec<String>,
}

pub fn run(a: &Args) -> Result<()> {
    let t = Instant::now();
    // As `build`: earlier LOD output (installed billboards decide which bases are trees) is not
    // read.
    let mut prof = a.profile.clone();
    prof.exclude_mod.extend(crate::profile::generated_lod_mods(&prof.instance, &prof.profile));
    let p = prof.open()?;
    let dyndolod = a.dyndolod.clone().unwrap_or_else(|| a.profile.instance.join(r"tools\DynDOLOD"));
    let gpu = lodforge_gpu::Gpu::new()?;
    let renderer = Renderer::new(&gpu);
    println!("profile opened in {} ms", t.elapsed().as_millis());
    if !a.models.is_empty() {
        let jobs: Vec<TypeJob> = a.models.iter().map(|m| model_job(m, &dyndolod)).collect();
        let r = bake_set(&p, &gpu, &renderer, &a.out, "Models", &[], jobs, &a.opts)?;
        return finish(a, "Models", &r);
    }
    let worlds = if a.worlds.is_empty() { vec!["Tamriel".to_string()] } else { a.worlds.clone() };
    for w in &worlds {
        let r = bake_world(&p, &gpu, &renderer, &dyndolod, &a.out, w, &a.seasons, a.filter.as_deref(), a.preview_instances, &a.opts)?;
        finish(a, w, &r)?;
    }
    println!("impostors done in {:.1} s", t.elapsed().as_secs_f32());
    Ok(())
}

fn finish(a: &Args, set: &str, r: &SetReport) -> Result<()> {
    let report = a.report.clone().unwrap_or_else(|| report_path(&a.out, set));
    if let Some(d) = report.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(&report, report_csv(r))?;
    print_summary(set, r);
    println!("  report: {}", report.display());
    Ok(())
}

/// `textures\LODForge\Impostors\<set>` under `out`.
pub fn set_dir(out: &Path, set: &str) -> PathBuf {
    out.join(r"textures\LODForge\Impostors").join(set)
}

fn print_summary(set: &str, r: &SetReport) {
    let cached = r.types.iter().filter(|t| t.cached).count();
    let ms: Vec<u128> = r.types.iter().filter(|t| !t.cached).map(|t| t.ms).collect();
    let mean = if ms.is_empty() { 0 } else { ms.iter().sum::<u128>() / ms.len() as u128 };
    println!(
        "{set}: {} impostor types ({cached} from cache), {} pages, {:.1} MiB; bake {:.1} s (mean {mean} ms/type baked), encode {:.1} s",
        r.types.len(),
        r.pages,
        r.page_bytes as f64 / (1024.0 * 1024.0),
        r.bake_ms as f64 / 1000.0,
        r.encode_ms as f64 / 1000.0
    );
    let mut by_layout: BTreeMap<(u32, u32), (usize, u64)> = BTreeMap::new();
    for t in &r.types {
        let e = by_layout.entry((t.grid, t.frame_px)).or_default();
        e.0 += 1;
        e.1 += t.bytes;
    }
    for ((g, f), (n, b)) in by_layout {
        println!("  {g}x{g} x {f} px ({} px atlas): {n} types, {:.1} MiB", g * f, b as f64 / (1024.0 * 1024.0));
    }
    for f in r.failed.iter().take(20) {
        println!("  failed: {f}");
    }
}

pub fn report_csv(r: &SetReport) -> String {
    let mut s = String::from("type,model,references,grid,frame_px,atlas_px,quad_size,bake_ms,cached,bytes,coverage\n");
    for t in &r.types {
        s.push_str(&format!("{},{},{},{},{},{},{:.1},{},{},{},{:.3}\n", t.name, t.model, t.refs, t.grid, t.frame_px, t.atlas_px, t.quad_size, t.ms, t.cached, t.bytes, t.coverage));
    }
    s
}

fn model_job(model: &str, dyndolod: &Path) -> TypeJob {
    let record_model = lodforge_dyndolod::bases::model_path(model);
    let (file, rendered) = match lodforge_texgen::replacement_model(dyndolod, model) {
        Some((f, v)) => (Some(f), v.to_ascii_lowercase()),
        None => (None, record_model.clone()),
    };
    TypeJob { name: lodforge_impostor::manifest::type_name(&rendered), model: rendered, record_model, file, bases: Vec::new(), crc: 0, scale: 1.0 }
}

/// Never-enabled (enable parent = the player, opposite state), deleted, initially disabled, or
/// tilted more than 30 degrees: not drawn as a distant tree.
pub fn skip_ref(r: &lodforge_esp::world::Placement) -> bool {
    use lodforge_esp::record::{FLAG_DELETED, FLAG_INITIALLY_DISABLED};
    let never = r.enable_parent && r.enable_parent_id == lodforge_treelod::PLAYER_REF && r.enable_parent_flags & 1 != 0;
    let tilt = |a: f32| {
        let d = a.to_degrees().rem_euclid(360.0);
        d > 30.0 && d < 330.0
    };
    r.flags & (FLAG_DELETED | FLAG_INITIALLY_DISABLED) != 0 || never || tilt(r.rotation[0]) || tilt(r.rotation[1])
}

/// Bakes the tree types of worldspace `world` (TexGen's accepted billboard candidates its
/// references use, plus their season swaps) into its set.
#[allow(clippy::too_many_arguments)]
pub fn bake_world(p: &Profile, gpu: &lodforge_gpu::Gpu, renderer: &Renderer, dyndolod: &Path, out: &Path, world: &str, seasons: &[String], filter: Option<&str>, preview: bool, opts: &Opts) -> Result<SetReport> {
    let lo = &p.load_order;
    let t = Instant::now();
    let bases = lodforge_dyndolod::bases::read_bases(lo)?;
    let cands = crate::texgen_billboards::discover_trees(p, &bases, dyndolod);
    let accepted: HashMap<u32, &Candidate> = cands.iter().filter(|c| c.accepted).map(|c| (c.base, c)).collect();
    let worlds = lodforge_esp::scan_worldspaces(lo)?;
    let Some(ws) = worlds.iter().find(|w| w.editor_id.eq_ignore_ascii_case(world)) else { bail!("worldspace {world} not found") };
    // Season swaps of tree bases.
    let mut swaps: Vec<HashMap<u32, u32>> = Vec::new();
    if !seasons.is_empty() {
        let inputs = crate::object_export::load_inputs(p, world)?;
        for s in seasons {
            let season = crate::object_export::season_from_code(s).with_context(|| format!("unknown season {s}"))?;
            swaps.push(crate::object_export::base_swaps(&inputs, season));
        }
    }
    // References and largest scale per base.
    let mut refs: HashMap<u32, (usize, f32)> = HashMap::new();
    for r in ws.refs.values() {
        if skip_ref(r) {
            continue;
        }
        let scale = if r.has_scale { r.scale } else { 1.0 };
        for b in std::iter::once(r.base).chain(swaps.iter().filter_map(|m| m.get(&r.base).copied())) {
            if accepted.contains_key(&b) {
                let e = refs.entry(b).or_insert((0, 0.0));
                e.0 += 1;
                e.1 = e.1.max(scale);
            }
        }
    }
    // Types by rendered model.
    let mut types: BTreeMap<String, TypeJob> = BTreeMap::new();
    let mut ref_counts: HashMap<String, usize> = HashMap::new();
    let mut base_type: HashMap<u32, String> = HashMap::new();
    let flt = filter.map(str::to_ascii_lowercase);
    let mut ids: Vec<u32> = refs.keys().copied().collect();
    ids.sort_unstable();
    for id in ids {
        let c = accepted[&id];
        if let Some(f) = &flt
            && !c.model.to_ascii_lowercase().contains(f.as_str())
            && !c.editor_id.to_ascii_lowercase().contains(f.as_str())
        {
            continue;
        }
        let job = model_job(&c.model, dyndolod);
        let (n, scale) = refs[&id];
        let e = types.entry(job.name.clone()).or_insert(job);
        e.bases.push((id, BaseRef { plugin: c.plugin.clone(), form_id: format!("{:08X}", c.local), editor_id: c.editor_id.clone() }));
        e.scale = e.scale.max(scale.clamp(1.0, 1.25));
        *ref_counts.entry(e.name.clone()).or_default() += n;
        base_type.insert(id, e.name.clone());
    }
    println!("{world}: {} tree types from {} bases with references ({} accepted candidates) in {} ms", types.len(), refs.len(), accepted.len(), t.elapsed().as_millis());
    let jobs: Vec<TypeJob> = types.into_values().collect();
    let mut r = bake_set(p, gpu, renderer, out, world, &[world.to_string()], jobs, opts)?;
    for t in &mut r.types {
        t.refs = ref_counts.get(&t.name).copied().unwrap_or(0);
    }
    if preview {
        let index: HashMap<String, u32> = r.types.iter().enumerate().map(|(i, t)| (t.name.clone(), i as u32)).collect();
        let n = write_preview_instances(&set_dir(out, world).join("preview-instances.bin"), ws, &base_type, &index)?;
        println!("  preview-instances.bin: {n} references");
    }
    Ok(r)
}

/// Temporary instance list for the viewer (`LFPI` v1: count, then pos xyz, yaw (radians, as the
/// REFR stores it), scale, type index; 24 bytes each). Replaced by the runtime instance files.
fn write_preview_instances(path: &Path, ws: &lodforge_esp::Worldspace, base_type: &HashMap<u32, String>, index: &HashMap<String, u32>) -> Result<usize> {
    let mut ids: Vec<&u32> = ws.refs.keys().collect();
    ids.sort_unstable();
    let mut recs = Vec::new();
    for id in ids {
        let r = &ws.refs[id];
        if skip_ref(r) {
            continue;
        }
        let Some(ti) = base_type.get(&r.base).and_then(|n| index.get(n)) else { continue };
        let scale = if r.has_scale { r.scale } else { 1.0 };
        for v in [r.position[0], r.position[1], r.position[2], r.rotation[2], scale] {
            recs.extend_from_slice(&v.to_le_bytes());
        }
        recs.extend_from_slice(&ti.to_le_bytes());
    }
    let n = recs.len() / 24;
    let mut b = Vec::with_capacity(12 + recs.len());
    b.extend_from_slice(b"LFPI");
    b.extend_from_slice(&1u32.to_le_bytes());
    b.extend_from_slice(&(n as u32).to_le_bytes());
    b.extend_from_slice(&recs);
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(path, b)?;
    Ok(n)
}

/// A loaded type: scene, textures (decoded only when baking) and the cache key inputs.
struct Loaded {
    scene: Scene,
    tex_bytes: Vec<(String, Vec<u8>)>,
    model_crc: u32,
}

fn load(p: &Profile, j: &TypeJob) -> Result<Loaded> {
    let bytes = match &j.file {
        Some(f) => std::fs::read(f).with_context(|| format!("reading {}", f.display()))?,
        None => p.vfs.read(&j.record_model)?.with_context(|| format!("{} not in the VFS", j.record_model))?,
    };
    let nif = lodforge_nif::parse_all_geometry(&bytes).map_err(|e| anyhow::anyhow!("{}: {e}", j.model))?;
    let mut scene = Scene::from_nif(&nif);
    lodforge_texgen::resolve_textures(&mut scene, |t| p.vfs.exists(t));
    let mut tex_bytes: Vec<(String, Vec<u8>)> = Vec::new();
    for m in &scene.meshes {
        for path in [&m.diffuse, &m.normal, &m.glow] {
            if path.is_empty() || tex_bytes.iter().any(|(q, _)| q == path) {
                continue;
            }
            if let Some(b) = p.vfs.read(path).ok().flatten() {
                tex_bytes.push((path.clone(), b));
            }
        }
    }
    Ok(Loaded { scene, tex_bytes, model_crc: lodforge_impostor::manifest::crc32(&bytes) })
}

/// Bakes `jobs` (from the cache where it can) and writes the set `set`: pages, manifests, index.
#[allow(clippy::too_many_arguments)]
fn bake_set(p: &Profile, gpu: &lodforge_gpu::Gpu, renderer: &Renderer, out: &Path, set: &str, worlds: &[String], jobs: Vec<TypeJob>, opts: &Opts) -> Result<SetReport> {
    let settings = opts.settings();
    let calibration = opts.calibration()?;
    let cache = opts.cache_dir();
    std::fs::create_dir_all(&cache).with_context(|| format!("creating {}", cache.display()))?;
    let t_bake = Instant::now();
    let mut baked: Vec<(TypeJob, bake::Baked, String, u128, bool)> = Vec::new();
    let mut failed = Vec::new();
    let total = jobs.len();
    for (k, mut j) in jobs.into_iter().enumerate() {
        let t = Instant::now();
        let l = match load(p, &j) {
            Ok(l) => l,
            Err(e) => {
                failed.push(format!("{}: {e:#}", j.model));
                continue;
            }
        };
        let Some(fit) = bake::fit(&l.scene) else {
            failed.push(format!("{}: no geometry", j.model));
            continue;
        };
        let (grid, frame) = settings.layout((fit.max.z - fit.min.z) * j.scale);
        j.crc = l.model_crc;
        let crcs: Vec<(String, u32)> = l.tex_bytes.iter().map(|(n, b)| (n.clone(), lodforge_impostor::manifest::crc32(b))).collect();
        let key = bake::cache_key(l.model_crc, &crcs, grid, frame, &settings);
        let key_hex = format!("{key:016x}");
        let file = cache.join(format!("{key_hex}.lfic"));
        let hit = if opts.no_cache { None } else { std::fs::read(&file).ok().and_then(|d| bake::decode_cache(&d)) };
        let (b, cached) = match hit {
            Some(b) => (b, true),
            None => {
                let textures: HashMap<String, TexData> = l.tex_bytes.iter().filter_map(|(n, b)| TexData::for_gpu(gpu, b).map(|t| (n.clone(), t))).collect();
                let s = bake::Settings { grid, frame_px: frame, ..settings };
                let Some(b) = bake::bake(gpu, renderer, &l.scene, &textures, &s) else {
                    failed.push(format!("{}: nothing rendered (missing textures?)", j.model));
                    continue;
                };
                let _ = std::fs::write(&file, bake::encode_cache(&b));
                (b, false)
            }
        };
        let ms = t.elapsed().as_millis();
        if (k + 1) % 25 == 0 || k + 1 == total {
            println!("  {}/{total} types ({} ms for {}{})", k + 1, ms, j.name, if cached { ", cached" } else { "" });
        }
        baked.push((j, b, key_hex, ms, cached));
    }
    let bake_ms = t_bake.elapsed().as_millis();

    // Pages.
    let t_enc = Instant::now();
    let dir = set_dir(out, set);
    if dir.is_dir() {
        // Pages and manifests of an earlier run would no longer match the index.
        for sub in ["types"] {
            let _ = std::fs::remove_dir_all(dir.join(sub));
        }
        for e in std::fs::read_dir(&dir)?.flatten() {
            let n = e.file_name().to_string_lossy().to_ascii_lowercase();
            if n.starts_with("page") && n.ends_with(".dds") {
                std::fs::remove_file(e.path())?;
            }
        }
    }
    std::fs::create_dir_all(dir.join("types"))?;
    // Types with the same cache key (identical model and texture bytes under another path)
    // share one atlas rectangle.
    let mut first: HashMap<&str, usize> = HashMap::new();
    let mut unique: Vec<usize> = Vec::new();
    let slot_of: Vec<usize> = baked
        .iter()
        .enumerate()
        .map(|(k, (_, _, key, ..))| {
            *first.entry(key.as_str()).or_insert_with(|| {
                unique.push(k);
                unique.len() - 1
            })
        })
        .collect();
    let sizes: Vec<u32> = unique.iter().map(|&k| baked[k].1.size()).collect();
    let unique_slots = atlas::pack(&sizes, opts.page);
    let refs: Vec<&bake::Baked> = unique.iter().map(|&k| &baked[k].1).collect();
    let pages = atlas::compose(&refs, &unique_slots, settings.alpha_ref);
    let slots: Vec<atlas::Slot> = slot_of.iter().map(|&u| unique_slots[u]).collect();
    if let Some(png_dir) = &opts.png {
        dump_pngs(png_dir, set, &pages)?;
    }
    let rel = |name: &str| format!(r"textures\LODForge\Impostors\{set}\{name}");
    let mut page_infos = Vec::new();
    let mut page_bytes = 0u64;
    for (i, pg) in pages.iter().enumerate() {
        let (an, nn) = (format!("page{i:02}.dds"), format!("page{i:02}_nd.dds"));
        let a = atlas::page_dds(gpu, pg, atlas::Map::Albedo)?;
        let n = atlas::page_dds(gpu, pg, atlas::Map::NormalDepth)?;
        page_bytes += (a.len() + n.len()) as u64;
        std::fs::write(dir.join(&an), &a)?;
        std::fs::write(dir.join(&nn), &n)?;
        page_infos.push(PageInfo { index: i as u32, albedo: rel(&an), normal_depth: rel(&nn), width: pg.width, height: pg.height, mips: pg.mip_levels() });
    }
    let encode_ms = t_enc.elapsed().as_millis();

    let mut types = Vec::new();
    let mut reports = Vec::new();
    for (k, ((j, b, key, ms, cached), s)) in baked.iter().zip(&slots).enumerate() {
        let shared = unique[slot_of[k]] != k;
        let pi = &page_infos[s.page as usize];
        let size = b.size();
        // BC7: 1 byte per texel and map; the mip chain adds about a third.
        let levels = b.frame_px.max(1).ilog2() + 1;
        let bytes: u64 = if shared { 0 } else { (0..levels).map(|l| u64::from((size >> l).max(1).div_ceil(4) * 4).pow(2)).sum::<u64>() * 2 };
        let calib = calibration.iter().filter(|(n, ..)| n == "*" || *n == j.name).last();
        let m = TypeManifest {
            version: lodforge_impostor::manifest::MANIFEST_VERSION,
            name: j.name.clone(),
            model: j.model.clone(),
            record_model: j.record_model.clone(),
            model_crc32: format!("{:08X}", j.crc),
            bases: j.bases.iter().map(|(_, b)| b.clone()).collect(),
            grid: b.grid,
            frame_px: b.frame_px,
            atlas_px: size,
            quad_size: b.quad_size(),
            bounds_center_offset: b.fit.center.into(),
            bounds_min: b.fit.min.into(),
            bounds_max: b.fit.max.into(),
            page: s.page,
            albedo: pi.albedo.clone(),
            normal_depth: pi.normal_depth.clone(),
            rect_px: [s.x, s.y, size, size],
            rect: [s.x as f32 / pi.width as f32, s.y as f32 / pi.height as f32, size as f32 / pi.width as f32, size as f32 / pi.height as f32],
            coverage: b.coverage(),
            cache_key: key.clone(),
            brightness: calib.map_or(1.0, |c| c.1),
            tint: calib.map_or([1.0; 3], |c| c.2),
            prelit: settings.shading.enabled,
        };
        std::fs::write(dir.join("types").join(format!("{}.json", j.name)), serde_json::to_string_pretty(&m)?)?;
        reports.push(TypeReport { name: j.name.clone(), model: j.model.clone(), refs: 0, grid: b.grid, frame_px: b.frame_px, atlas_px: size, quad_size: b.quad_size(), ms: *ms, cached: *cached, bytes, coverage: m.coverage });
        types.push(m);
    }
    let index = Index { version: lodforge_impostor::manifest::MANIFEST_VERSION, baker_version: bake::BAKER_VERSION, layout: "hemi-oct-z-up".into(), worlds: worlds.to_vec(), pages: page_infos, types };
    std::fs::write(dir.join("impostors.json"), serde_json::to_string_pretty(&index)?)?;
    Ok(SetReport { types: reports, pages: pages.len(), page_bytes, bake_ms, encode_ms, failed })
}

/// `build --impostors`: the worldspace's set into the build output.
pub fn build_stage(p: &Profile, dyndolod: &Path, out: &Path, world: &str, seasons: &[String], opts: &Opts) -> Result<()> {
    let gpu = lodforge_gpu::Gpu::new()?;
    let renderer = Renderer::new(&gpu);
    let r = bake_world(p, &gpu, &renderer, dyndolod, out, world, seasons, None, false, opts)?;
    std::fs::write(report_path(out, world), report_csv(&r))?;
    print_summary(world, &r);
    Ok(())
}

/// `LODForge_ImpostorReport_<set>.csv` at the output root (next to the tree reports).
pub fn report_path(out: &Path, set: &str) -> PathBuf {
    out.join(format!("LODForge_ImpostorReport_{set}.csv"))
}

/// Writes `<set>_pageNN_{albedo,normal,depth}.png` for inspection.
fn dump_pngs(dir: &Path, set: &str, pages: &[atlas::Page]) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let write = |name: String, w: u32, h: u32, px: &[u8]| -> Result<()> {
        let f = std::fs::File::create(dir.join(name))?;
        let mut e = png::Encoder::new(std::io::BufWriter::new(f), w, h);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        e.write_header()?.write_image_data(px)?;
        Ok(())
    };
    for (i, p) in pages.iter().enumerate() {
        // Over a light grey background, coverage as the blend weight.
        let flat: Vec<u8> = p.albedo.chunks_exact(4).flat_map(|c| { let a = u32::from(c[3]); let m = |v: u8| ((u32::from(v) * a + 200 * (255 - a)) / 255) as u8; [m(c[0]), m(c[1]), m(c[2]), 255] }).collect();
        write(format!("{set}_page{i:02}_albedo.png"), p.width, p.height, &flat)?;
        let normal: Vec<u8> = p.normal_depth.chunks_exact(4).flat_map(|c| [c[0], c[1], c[2], 255]).collect();
        write(format!("{set}_page{i:02}_normal.png"), p.width, p.height, &normal)?;
        let depth: Vec<u8> = p.normal_depth.chunks_exact(4).zip(p.albedo.chunks_exact(4)).flat_map(|(c, a)| [c[3], c[3], c[3], a[3].max(64)]).collect();
        write(format!("{set}_page{i:02}_depth.png"), p.width, p.height, &depth)?;
    }
    Ok(())
}
