//! `lodforge object-lod`: object LOD meshes (.bto) for a worldspace from a LODGen export file,
//! built with `lodforge-objlod` (the stage B2 rewrite), optionally compared with golden BTOs.
//!
//! LODGen's shape order inside a .bto depends on thread timing (two xLODGen runs on the same input
//! differ byte-wise in ~65% of files), so the comparison is order-independent: shapes are matched
//! by name, textures and shader flags, and each shape's triangles are compared as a multiset of
//! fully attributed vertices.
//!
//! Stage B2.5 state: hidden-face removal (B2.3), the atlas UV remap (B2.4), tree billboards
//! and grass LOD (B2.5) are in; the object height map (B2.6) too. `--geometry-only` compares
//! positions, normals, tangents and colours only (no UVs, textures or clamp mode), which
//! isolates the geometry from the UV remap and the texture pages.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use rayon::prelude::*;

use lodforge_nif::{Block, Nif, f32_to_half, float_to_byte};
use lodforge_objlod::hidden::{BtrTerrain, HiddenRule, Samples, Terrain, TerrainKind, rule_hidden};
use lodforge_objlod::sidefiles::{AltTextures, VcList};
use lodforge_objlod::stats::Stats;
use lodforge_objlod::terrain::TerrainHeights;
use lodforge_objlod::BuiltQuad;
use lodforge_objlod::{AtlasMap, Dialect, ExportFile, LodGrid, LodSettings, ObjectLod, Options, file_name};

use crate::profile::{Profile, ProfileArgs};

/// LODForge's grass density in percent when neither `--grass-density` nor the export sets one.
/// DynDOLOD defaults to 100% (8.7 GB of Tamriel LOD4 with HD grass); its docs advise 50% or less
/// for HD grass, and the size scales about linearly with density.
#[allow(dead_code)]
pub const DEFAULT_GRASS_DENSITY: f32 = 50.0;

#[derive(clap::Args)]
pub struct Args {
    #[command(flatten)]
    pub profile: ProfileArgs,
    /// LODGen export file(s) (default: `<instance>\tools\xLODGen\Edit Scripts\LODGen.txt`).
    #[arg(long, value_delimiter = ',')]
    pub export: Vec<PathBuf>,
    /// Also build these seasons from `LODGen.<SEASON>.txt` next to the default export file.
    #[arg(long, value_delimiter = ',')]
    pub seasons: Vec<String>,
    /// Atlas map (default: the export file's `TextureAtlasMap`).
    #[arg(long)]
    pub atlas: Option<PathBuf>,
    /// Folder with the worldspace's terrain .btr files for the hidden-face test (default: the
    /// output's `meshes\terrain\<world>`, then the VFS).
    #[arg(long)]
    pub terrain: Option<PathBuf>,
    /// Hidden-face rule `btr|land:v3|v3c|s7:<shift>:water|nowater` (terrain source, sample
    /// points, margin in game units, water test); default: the rule settled on the golden files.
    #[arg(long)]
    pub hidden_rule: Option<String>,
    /// Keep every triangle (no hidden-face removal).
    #[arg(long)]
    pub no_hidden: bool,
    /// Re-cut tiled shapes along their UV tile edges so they fit a mini-atlas tile (off: such
    /// shapes keep their own texture, as LODGen does).
    #[arg(long)]
    pub reuv: bool,
    /// Build without removal and score candidate hidden-face rules against --golden (the
    /// triangles the golden files lack are the ones LODGen removed).
    #[arg(long)]
    pub hidden_study: bool,
    /// Output Data folder (`meshes\terrain\<world>\Objects\` is created inside).
    #[arg(long, default_value = "bench/out")]
    pub out: PathBuf,
    /// Golden folder of .bto files to compare with.
    #[arg(long)]
    pub golden: Option<PathBuf>,
    /// Only build this level (4, 8, 16).
    #[arg(long)]
    pub level: Option<i32>,
    /// Only build these quads (`4.0.-16,8.0.-16`, comma separated).
    #[arg(long)]
    pub only: Option<String>,
    /// Print the per-shape comparison of these output files.
    #[arg(long, value_delimiter = ',')]
    pub explain: Vec<String>,
    /// Skip building: compare this folder's default-season .bto files with --golden.
    #[arg(long)]
    pub compare_only: Option<PathBuf>,
    /// Build and serialize but do not write files (timing).
    #[arg(long)]
    pub dry_run: bool,
    /// Build the export and atlas from the load order (xEdit's object LOD export port) instead of
    /// reading xLODGen's LODGen*.txt and atlas maps.
    #[arg(long)]
    pub from_load_order: bool,
    /// Worldspace for --from-load-order.
    #[arg(long, default_value = "Tamriel")]
    pub world: String,
    /// With --from-load-order: also render the atlas textures into the output folder.
    #[arg(long)]
    pub atlas_textures: bool,
    /// Which export dialect to read: xLODGen's (16 columns, options on the command line) or
    /// DynDOLOD's (17 columns, options in the header); `auto` detects it from the header keys.
    #[arg(long, value_enum, default_value = "auto")]
    pub flavor: FlavorArg,
    /// Golden comparison with tolerance: positions on a 0.1-unit grid; normals, tangents,
    /// bitangents and UVs left out of the triangle key (the rewrite's arithmetic differs from
    /// LODGen's by one byte or one half-float step on many vertices); their agreement (one
    /// byte, 1/1024 of the UV range) is reported separately for the vertices paired by position.
    #[arg(long)]
    pub tolerant: bool,
    /// Golden comparison of the geometry only: shapes matched by name and flags without their
    /// textures, clamp mode or colour presence, vertices compared without UVs or colours.
    #[arg(long)]
    pub geometry_only: bool,
    /// DynDOLOD side files (AltTextures, VC, FlatTextures) from this folder by file name instead
    /// of the absolute paths in the export header (golden copies).
    #[arg(long)]
    pub side_files: Option<PathBuf>,
    /// Position grid of the tolerant comparison in game units (default 0.1).
    #[arg(long, default_value_t = 0.1)]
    pub grid: f32,
    /// Grass LOD: folder with the grass cache (`<World>x<XXXX>y<YYYY>.cgid`); with `--grass-map`.
    #[arg(long)]
    pub grass: Option<PathBuf>,
    /// Grass map (`MODELSTEM_FORMID8.DDS<TAB>billboard`, see `grass-billboards`).
    #[arg(long)]
    pub grass_map: Option<PathBuf>,
    /// Extra FlatTextures file with the grass billboards (appended to the export's).
    #[arg(long)]
    pub grass_flat: Option<PathBuf>,
    /// Grass density in percent (default: the export's `GrassDensity`, else 50).
    #[arg(long)]
    pub grass_density: Option<f32>,
}

#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum FlavorArg {
    Auto,
    Xlodgen,
    Dyndolod,
}

impl FlavorArg {
    fn dialect(self) -> Option<Dialect> {
        match self {
            FlavorArg::Auto => None,
            FlavorArg::Xlodgen => Some(Dialect::XLodgen),
            FlavorArg::Dyndolod => Some(Dialect::Dyndolod),
        }
    }
}

/// How two files are compared.
#[derive(Clone, Copy)]
pub struct CompareMode {
    pub tolerant: bool,
    pub geometry_only: bool,
    /// Position grid of the tolerant key, game units.
    pub grid: f32,
}

impl Default for CompareMode {
    fn default() -> Self {
        CompareMode { tolerant: false, geometry_only: false, grid: 0.1 }
    }
}

impl CompareMode {
    fn from_args(a: &Args) -> CompareMode {
        CompareMode { tolerant: a.tolerant, geometry_only: a.geometry_only, grid: if a.grid > 0.0 { a.grid } else { 0.1 } }
    }
}

pub fn run(a: &Args) -> Result<()> {
    let mode = CompareMode::from_args(a);
    if let (Some(dir), Some(g)) = (&a.compare_only, &a.golden) {
        let mut names: Vec<String> = std::fs::read_dir(dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".bto") && !n.split('.').any(|p| ["SPR", "SUM", "AUT", "WIN"].contains(&p)))
            .collect();
        names.sort();
        compare_dir(dir, g, &names, mode)?;
        for name in &a.explain {
            explain(&dir.join(name), &g.join(name), mode)?;
        }
        return Ok(());
    }
    let p = a.profile.open()?;
    run_with(&p, a)
}

fn read_text(path: &Path) -> Result<String> {
    Ok(lodforge_objlod::export::decode(&std::fs::read(path).with_context(|| format!("reading {}", path.display()))?))
}

pub fn run_with(p: &Profile, a: &Args) -> Result<()> {
    let total = Instant::now();
    let vfs = &p.vfs;
    let mode = CompareMode::from_args(a);
    // (export, atlas) per season, from the load order or from the export files.
    let mut jobs: Vec<(ExportFile, AtlasMap)> = Vec::new();
    if a.from_load_order {
        let t = Instant::now();
        let inputs = crate::object_export::load_inputs(p, &a.world)?;
        let gpu = if a.atlas_textures && !a.dry_run { Some(lodforge_gpu::Gpu::new()?) } else { None };
        let mut seasons = vec![None];
        for s in &a.seasons {
            seasons.push(Some(crate::object_export::season_from_code(s).with_context(|| format!("unknown season {s}"))?));
        }
        for season in seasons {
            let (export, st) = crate::object_export::build_export(p, &inputs, season);
            let layout = crate::object_export::atlas_layout(p, &inputs, &st, season);
            let atlas = AtlasMap::parse(&layout.map_text()).map_err(anyhow::Error::msg)?;
            if let Some(gpu) = &gpu {
                let pages = crate::object_atlas::render(&p.vfs, &layout, gpu, &crate::object_atlas::Formats::BC7_FULL)?;
                crate::object_atlas::write(&a.out, &pages)?;
            }
            jobs.push((export, atlas));
        }
        println!("export + atlas{} from the load order: {} ms", if gpu.is_some() { " (with textures)" } else { "" }, t.elapsed().as_millis());
    } else {
        let default_dir = a.profile.instance.join(r"tools\xLODGen\Edit Scripts");
        let mut exports: Vec<PathBuf> = if a.export.is_empty() { vec![default_dir.join("LODGen.txt")] } else { a.export.clone() };
        for s in &a.seasons {
            exports.push(default_dir.join(format!("LODGen.{s}.txt")));
        }
        for export_path in &exports {
            let text = read_text(export_path)?;
            let export = match a.flavor.dialect() {
                Some(d) => ExportFile::parse_with(&text, d),
                None => ExportFile::parse(&text),
            };
            if export.worldspace.is_empty() {
                bail!("no Worldspace= in {}", export_path.display());
            }
            let atlas_path = a.atlas.clone().unwrap_or_else(|| PathBuf::from(&export.atlas_map));
            let atlas = if atlas_path.as_os_str().is_empty() { AtlasMap::default() } else { AtlasMap::parse(&read_text(&atlas_path)?).map_err(anyhow::Error::msg)? };
            jobs.push((export, atlas));
        }
    }
    if a.grass.is_some() != a.grass_map.is_some() {
        bail!("grass LOD needs both --grass (the cache folder) and --grass-map");
    }
    // LOD models can be absolute paths (DynDOLOD's templates); everything else comes from the VFS.
    let read = |path: &str| -> Option<Vec<u8>> { if Path::new(path).is_absolute() { std::fs::read(path).ok() } else { vfs.read(path).ok().flatten() } };
    let mut written_total = 0;
    let job_count = jobs.len();
    for (export, atlas) in &jobs {
        let t0 = Instant::now();
        let world = export.worldspace.clone();
        let out_dir = a.out.join("meshes").join("terrain").join(&world).join("Objects");
        std::fs::create_dir_all(&out_dir)?;
        let grid = match vfs.read(&format!("lodsettings\\{world}.lod")).ok().flatten().and_then(|b| LodSettings::parse(&b)) {
            Some(s) if s.south_west == export.south_west => LodGrid::from_settings(&s),
            Some(s) => {
                println!("  warning: lodsettings\\{world}.lod south-west cell {:?} differs from the export's CellSW {:?}; using the export's", s.south_west, export.south_west);
                LodGrid::new(export.south_west, Some(s.stride))
            }
            None => LodGrid::new(export.south_west, None),
        };
        let mut opts = Options::from_export(export);
        // Side files: the header's absolute paths, or the same file names under --side-files.
        let side = |header_path: &str| -> PathBuf {
            match (&a.side_files, Path::new(header_path).file_name()) {
                (Some(dir), Some(name)) if dir.join(name).is_file() => dir.join(name),
                _ => PathBuf::from(header_path),
            }
        };
        if export.dialect == Dialect::Dyndolod {
            if !export.alt_textures_file.is_empty()
                && let Ok(text) = read_text(&side(&export.alt_textures_file))
            {
                opts.alt_textures = AltTextures::parse(&text);
            }
            if !export.vc_file.is_empty()
                && let Ok(text) = read_text(&side(&export.vc_file))
            {
                opts.vc = VcList::parse(&text);
            }
            if !export.flat_textures_file.is_empty()
                && let Ok(mut text) = read_text(&side(&export.flat_textures_file))
            {
                if let Some(gf) = &a.grass_flat {
                    text.push_str(&read_text(gf)?);
                }
                opts.flat = lodforge_objlod::flat::FlatList::parse(&text);
            }
        }
        if let (Some(dir), Some(map)) = (&a.grass, &a.grass_map) {
            let read = |path: &str| -> Option<Vec<u8>> { vfs.read(path).ok().flatten() };
            let mut g = lodforge_objlod::grass::GrassInputs::from_map(dir.clone(), &world, &read_text(map)?, &opts.flat, &read);
            let key = |n: &str, d: f32| export.number(n).unwrap_or(d);
            g.top = [key("GrassBrightnessTopR", 0.4), key("GrassBrightnessTopG", 0.445), key("GrassBrightnessTopB", 0.45)];
            g.bottom = [key("GrassBrightnessBottomR", 0.2), key("GrassBrightnessBottomG", 0.223), key("GrassBrightnessBottomB", 0.225)];
            g.complex_top = [key("ComplexGrassBrightnessTopR", 0.7), key("ComplexGrassBrightnessTopG", 0.75), key("ComplexGrassBrightnessTopB", 0.75)];
            g.complex_bottom = [key("ComplexGrassBrightnessBottomR", 0.7), key("ComplexGrassBrightnessBottomG", 0.75), key("ComplexGrassBrightnessBottomB", 0.75)];
            g.density = a.grass_density.or_else(|| export.number("GrassDensity")).unwrap_or(DEFAULT_GRASS_DENSITY);
            let without = g.types.values().filter(|t| t.width_1 == 0.0).count();
            println!("  grass: {} types in the map, {} without a sidecar or FlatTextures size, density {}%", g.types.len(), without, g.density);
            opts.grass = Some(std::sync::Arc::new(g));
        }
        // Terrain sources for the hidden-face test: the .btr files of the same level (--terrain,
        // else the output's terrain folder, else the VFS) and the LAND heights of the load order
        // (water, and the `land` rules).
        let terrain_dir = a.terrain.clone().unwrap_or_else(|| a.out.join("meshes").join("terrain").join(&world));
        let world_l = world.to_ascii_lowercase();
        let btr_read = {
            let dir = terrain_dir.clone();
            move |name: &str| std::fs::read(dir.join(name)).ok().or_else(|| vfs.read(&format!("meshes\\terrain\\{world_l}\\{name}")).ok().flatten())
        };
        let land = match land_heights(p, &world, export.south_west) {
            Ok(l) => Some(l),
            Err(e) => {
                println!("  warning: no LAND heights for the water test: {e}");
                None
            }
        };
        opts.terrain = Some(std::sync::Arc::new(Terrain { btr: Some(BtrTerrain::new(&world, grid, Box::new(btr_read))), land }));
        if let Some(r) = &a.hidden_rule {
            opts.hidden_rule = HiddenRule::parse(r).with_context(|| format!("bad --hidden-rule {r}"))?;
        }
        if a.no_hidden || a.hidden_study {
            opts.remove_unseen_faces = false;
        }
        if a.reuv {
            opts.reuv = true;
        }
        let lod = ObjectLod::new(export, atlas, opts, grid, &read);
        let load_ms = t0.elapsed().as_millis();
        if !lod.missing_models.is_empty() {
            println!("  {} LOD models not found, e.g. {}", lod.missing_models.len(), lod.missing_models.iter().take(3).cloned().collect::<Vec<_>>().join(", "));
        }
        if !lod.warnings.is_empty() {
            println!("  {} model import warnings, e.g. {}", lod.warnings.len(), lod.warnings[0]);
        }

        let t = Instant::now();
        let mut built = Vec::new();
        let mut copied: Vec<String> = Vec::new();
        let quads = lod.quads();
        for level in lod.levels() {
            if a.level.is_some_and(|l| l != level) {
                continue;
            }
            let only: Vec<String> = a.only.as_deref().map(|s| s.split(',').map(|q| q.trim().to_string()).filter(|q| !q.is_empty()).collect()).unwrap_or_default();
            let mut qs: Vec<_> = quads.iter().filter(|q| q.level == level && (only.is_empty() || only.iter().any(|o| *o == format!("{}.{}.{}", q.level, q.x, q.y)))).copied().collect();
            // DynDOLOD season exports: unchanged quads are copies of the default season.
            qs.retain(|q| {
                if !lod.season_copy(q) || a.dry_run {
                    return true;
                }
                let (from, to) = (out_dir.join(file_name(&world, q, None)), out_dir.join(file_name(&world, q, Some(&export.season))));
                if std::fs::copy(&from, &to).is_ok() {
                    copied.push(file_name(&world, q, Some(&export.season)));
                    false
                } else {
                    true
                }
            });
            built.extend(lod.build_level(&qs));
        }
        let build_ms = t.elapsed().as_millis();

        let t = Instant::now();
        let season = (!export.season.is_empty()).then_some(export.season.as_str());
        built.par_iter().try_for_each(|b| -> Result<()> {
            let bytes = b.nif.to_bytes()?;
            if !a.dry_run {
                std::fs::write(out_dir.join(file_name(&world, &b.quad, season)), bytes)?;
            }
            Ok(())
        })?;
        // The object height map (spec 7.1 / 12.4): the LOD4 quads' cells merged by maximum,
        // default season only (`CreateHeightMap` in the export header).
        if season.is_none() && built.iter().any(|b| b.heights.is_some()) {
            let mut cells = lodforge_objlod::heightmap::Cells::new();
            for b in &mut built {
                if let Some(h) = b.heights.take() {
                    lodforge_objlod::heightmap::merge(&mut cells, h);
                }
            }
            let cells = lodforge_objlod::heightmap::finish(&cells);
            let name = format!("{world}.objects.heightmap.dat");
            if !a.dry_run {
                std::fs::write(out_dir.join(&name), lodforge_objlod::heightmap::encode(&cells))?;
            }
            println!("  object height map: {} cells", cells.len());
            if let Some(g) = &a.golden
                && let Ok(bytes) = std::fs::read(g.join(&name))
            {
                let golden = lodforge_objlod::heightmap::decode(&bytes);
                let c = lodforge_objlod::heightmap::compare(&cells, &golden);
                println!(
                    "  vs golden height map: cells ours {} golden {} common {}; points of the common cells {}: identical {:.2}%, within 1 unit {:.2}%, empty in both {}, only ours {}, only golden {}",
                    c.cells_ours,
                    c.cells_golden,
                    c.cells_common,
                    c.points,
                    100.0 * c.identical as f64 / c.points.max(1) as f64,
                    100.0 * c.within_1 as f64 / c.points.max(1) as f64,
                    c.both_none,
                    c.only_ours,
                    c.only_golden
                );
            }
        }
        let write_ms = t.elapsed().as_millis();
        written_total += built.len();
        let s = &lod.stats;
        println!(
            "{world}{} [{:?}]: {} references ({} outside the grid, {} without a model), atlas map {} rows, models + buckets {load_ms} ms; {} BTOs built{} in {build_ms} ms, written in {write_ms} ms",
            season.map(|s| format!(" [{s}]")).unwrap_or_default(),
            export.dialect,
            export.statics.len(),
            lod.buckets.outside,
            lod.buckets.without_model,
            atlas.len(),
            built.len(),
            if copied.is_empty() { String::new() } else { format!(" + {} copied from the default season", copied.len()) },
        );
        for (i, level) in [4, 8, 16, 32].iter().enumerate() {
            let tot = Stats::get(&s.total_tris[i]);
            if tot > 0 {
                println!("  LOD level {level} total triangles {tot} reduced to {} ({} hidden, rule {})", Stats::get(&s.reduced_tris[i]), Stats::get(&s.culled_tris[i]), if lod.opts.remove_unseen_faces { lod.opts.hidden_rule.text() } else { "off".into() });
            }
        }
        if let Some(t) = lod.opts.terrain.as_deref()
            && let Some(b) = &t.btr
        {
            let missing = b.missing.lock().unwrap().len();
            if missing > 0 {
                println!("  terrain: {} .btr files used, {missing} not found (triangles over them are kept)", b.loaded());
            }
        }
        if a.hidden_study {
            if let Some(g) = &a.golden {
                hidden_study(&lod, &built, g, &world)?;
            } else {
                bail!("--hidden-study needs --golden");
            }
        }
        println!(
            "  placements {}, missing meshes {}, tree billboards {} (without a FlatTextures line {}), grass instances {} (unmapped {}), passthru shapes {}, HD shapes {}, shapes split at 65,535 {}, AltTextures applied {}, parts atlased {} (re-cut {}), VC-whitened {}",
            Stats::get(&s.placed),
            Stats::get(&s.missing_meshes),
            Stats::get(&s.billboards),
            Stats::get(&s.billboards_missing),
            Stats::get(&s.grass_instances),
            Stats::get(&s.grass_unmapped),
            Stats::get(&s.passthru),
            Stats::get(&s.hd_shapes),
            Stats::get(&s.splits),
            Stats::get(&s.alt_textures),
            Stats::get(&s.atlased_parts),
            Stats::get(&s.reuv_parts),
            Stats::get(&s.vc_whitened)
        );
        if let Some(g) = &a.golden {
            compare_dir(&out_dir, g, &built.iter().map(|b| file_name(&world, &b.quad, season)).chain(copied.iter().cloned()).collect::<Vec<_>>(), mode)?;
            for name in &a.explain {
                explain(&out_dir.join(name), &g.join(name), mode)?;
            }
        }
    }
    println!("{job_count} export(s), {written_total} BTOs; total {} ms", total.elapsed().as_millis());
    Ok(())
}

/// LAND heights and water of a worldspace from the profile's load order.
fn land_heights(p: &Profile, world: &str, south_west: (i32, i32)) -> Result<TerrainHeights> {
    let worlds = lodforge_esp::scan_worldspaces(&p.load_order)?;
    let w = worlds.iter().find(|w| w.editor_id.eq_ignore_ascii_case(world)).with_context(|| format!("worldspace {world} not in the load order"))?;
    let parent = w.parent.and_then(|pid| worlds.iter().find(|x| x.form_id == pid));
    Ok(TerrainHeights::from_load_order(&p.load_order, w, parent, south_west, 32)?)
}

/// A world-space triangle key for the hidden-face study: positions on a 0.1-unit grid, canonical
/// rotation, nothing else.
fn position_key(tri: &[[f32; 3]; 3]) -> [[i32; 3]; 3] {
    // LODFORGE_STUDY_GRID = grid size in game units (default 0.1).
    let scale = 1.0 / std::env::var("LODFORGE_STUDY_GRID").ok().and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.1);
    let v = tri.map(|p| p.map(|c| (c * scale).round() as i32));
    let r = (0..3).min_by_key(|&k| v[k]).unwrap();
    [v[r], v[(r + 1) % 3], v[(r + 2) % 3]]
}

/// Scores candidate hidden-face rules against the golden files: every triangle of the build
/// (made without removal) is labelled "removed" when the golden file of the same quad has no
/// triangle at its positions, and each rule's decision is counted against that label.
fn hidden_study(lod: &ObjectLod<'_>, built: &[BuiltQuad], gold_dir: &Path, world: &str) -> Result<()> {
    let Some(terrain) = lod.opts.terrain.as_deref() else { bail!("--hidden-study needs a terrain source") };
    let mut rules = Vec::new();
    for kind in [TerrainKind::Btr, TerrainKind::Land] {
        if !terrain.has(kind) {
            continue;
        }
        for samples in [Samples::Vertices, Samples::VerticesCentroid, Samples::Seven, Samples::Centroid, Samples::AnyCorner, Samples::Majority] {
            for shift in [-10.0f32, 0.0, 10.0, 32.0] {
                for water in [true] {
                    for outside_hidden in [false] {
                        rules.push(HiddenRule { terrain: kind, samples, shift, water, outside_hidden });
                    }
                }
            }
        }
    }
    // Residue classes of the reference rule (golden removed, rule kept): by level, samples
    // without terrain data, duplicates in our own file, degenerate triangles, height of the
    // highest corner above the terrain, water state.
    let reference = lodforge_objlod::build::DEFAULT_HIDDEN_RULE;
    const CLASSES: [&str; 16] = ["lod4", "lod8", "lod16", "no-terrain-data", "dup-in-ours", "degenerate", "top<=8", "top<=32", "top<=128", "top>128", "water: all below", "water: some below", "no water data", "all corners above", "3 corners near golden vertices", "0 corners near golden vertices"];
    // LODFORGE_HIDDEN_RESIDUE=<file>: one line per residue triangle (file, level, centroid, top, shape).
    let residue_dump = std::env::var_os("LODFORGE_HIDDEN_RESIDUE").map(|_| std::sync::Mutex::new(Vec::<String>::new()));
    let per_quad: Vec<(usize, usize, usize, Vec<[usize; 3]>, [usize; 16])> = built
        .par_iter()
        .map(|b| {
            let mut golden: std::collections::HashMap<[[i32; 3]; 3], i32> = std::collections::HashMap::new();
            let name = file_name(world, &b.quad, None);
            if let Ok(bytes) = std::fs::read(gold_dir.join(&name))
                && let Ok(g) = lodforge_nif::parse(&bytes)
            {
                for blk in &g.blocks {
                    let Block::TriShape(s) = blk else { continue };
                    let t = &s.av.transform;
                    for tri in &s.triangles {
                        let w = tri.map(|i| {
                            let p = s.positions[i as usize];
                            [p[0] * t.scale + t.translation[0], p[1] * t.scale + t.translation[1], p[2] * t.scale + t.translation[2]]
                        });
                        *golden.entry(position_key(&w)).or_default() += 1;
                    }
                }
            } else {
                return (0, 0, 0, vec![[0; 3]; rules.len()], [0; 16]);
            }
            // Golden vertex positions on a 1-unit grid, for the "near a golden vertex" class.
            let mut golden_verts: std::collections::HashSet<[i32; 3]> = std::collections::HashSet::new();
            if let Ok(bytes) = std::fs::read(gold_dir.join(&name))
                && let Ok(g) = lodforge_nif::parse(&bytes)
            {
                for blk in &g.blocks {
                    let Block::TriShape(s) = blk else { continue };
                    let t = &s.av.transform;
                    for p in &s.positions {
                        golden_verts.insert([(p[0] * t.scale + t.translation[0]).round() as i32, (p[1] * t.scale + t.translation[1]).round() as i32, (p[2] * t.scale + t.translation[2]).round() as i32]);
                    }
                }
            }
            let near_golden = |p: [f32; 3]| -> bool {
                let c = [p[0].round() as i32, p[1].round() as i32, p[2].round() as i32];
                (-1..=1).any(|dx| (-1..=1).any(|dy| (-1..=1).any(|dz| golden_verts.contains(&[c[0] + dx, c[1] + dy, c[2] + dz]))))
            };
            let mut ours_count: std::collections::HashMap<[[i32; 3]; 3], u32> = std::collections::HashMap::new();
            for blk in &b.nif.blocks {
                let Block::TriShape(s) = blk else { continue };
                let t = &s.av.transform;
                for tri in &s.triangles {
                    let w = tri.map(|i| {
                        let p = s.positions[i as usize];
                        [p[0] * t.scale + t.translation[0], p[1] * t.scale + t.translation[1], p[2] * t.scale + t.translation[2]]
                    });
                    *ours_count.entry(position_key(&w)).or_default() += 1;
                }
            }
            let mut classes = [0usize; 16];
            let mut counts = vec![[0usize; 3]; rules.len()];
            let (mut total, mut removed) = (0usize, 0usize);
            for blk in &b.nif.blocks {
                let Block::TriShape(s) = blk else { continue };
                let t = &s.av.transform;
                for tri in &s.triangles {
                    let w = tri.map(|i| {
                        let p = s.positions[i as usize];
                        [p[0] * t.scale + t.translation[0], p[1] * t.scale + t.translation[1], p[2] * t.scale + t.translation[2]]
                    });
                    let key = position_key(&w);
                    let label = match golden.get_mut(&key) {
                        Some(c) if *c > 0 => {
                            *c -= 1;
                            false
                        }
                        _ => true,
                    };
                    total += 1;
                    removed += usize::from(label);
                    let (tb, wb) = if terrain.has(TerrainKind::Btr) { terrain.depths(TerrainKind::Btr, &b.quad, &w) } else { ([None; 7], [None; 7]) };
                    let (to, _) = if terrain.has(TerrainKind::BtrOwn) { terrain.depths(TerrainKind::BtrOwn, &b.quad, &w) } else { ([None; 7], [None; 7]) };
                    let (tc, _) = if terrain.has(TerrainKind::BtrClamp) { terrain.depths(TerrainKind::BtrClamp, &b.quad, &w) } else { ([None; 7], [None; 7]) };
                    let (tl, wl) = if terrain.has(TerrainKind::Land) { terrain.depths(TerrainKind::Land, &b.quad, &w) } else { ([None; 7], [None; 7]) };
                    for (ri, r) in rules.iter().enumerate() {
                        let (td, wd) = match r.terrain {
                            TerrainKind::Btr => (&tb, &wb),
                            TerrainKind::BtrOwn => (&to, &wb),
                            TerrainKind::BtrClamp => (&tc, &wb),
                            TerrainKind::Land => (&tl, &wl),
                        };
                        let pred = rule_hidden(r, td, wd);
                        match (label, pred) {
                            (true, true) => counts[ri][0] += 1,
                            (false, true) => counts[ri][1] += 1,
                            (true, false) => counts[ri][2] += 1,
                            _ => {}
                        }
                    }
                    let pred_ref = rule_hidden(&reference, &tb, &wb);
                    if let Some(dump) = &residue_dump
                        && ((label && !pred_ref) || (!label && !pred_ref && total % 53 == 0))
                    {
                        // Residue line, or every 53rd kept triangle as the baseline.
                        let top = tb[..3].iter().filter_map(|d| *d).fold(f32::NEG_INFINITY, f32::max);
                        let bottom = tb[..3].iter().filter_map(|d| *d).fold(f32::INFINITY, f32::min);
                        let e1 = [w[1][0] - w[0][0], w[1][1] - w[0][1], w[1][2] - w[0][2]];
                        let e2 = [w[2][0] - w[0][0], w[2][1] - w[0][1], w[2][2] - w[0][2]];
                        let n = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
                        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                        dump.lock().unwrap().push(format!("{}	{}	{}	{:.1}	{:.1}	{:.1}	{:.1}	{:.1}	{:.3}	{:.1}	{}	{:?}", if label { "res" } else { "kept" }, name, b.quad.level, (w[0][0] + w[1][0] + w[2][0]) / 3.0, (w[0][1] + w[1][1] + w[2][1]) / 3.0, (w[0][2] + w[1][2] + w[2][2]) / 3.0, top, bottom, if len > 0.0 { n[2] / len } else { 0.0 }, len / 2.0, s.av.net.name.as_deref().unwrap_or(""), w));
                    }
                    if label && !pred_ref {
                        let tb = tb;
                        classes[match b.quad.level {
                            4 => 0,
                            8 => 1,
                            _ => 2,
                        }] += 1;
                        let corners = &tb[..3];
                        if corners.iter().any(|d| d.is_none()) {
                            classes[3] += 1;
                        }
                        if ours_count.get(&key).copied().unwrap_or(0) > 1 {
                            classes[4] += 1;
                        }
                        let e1 = [w[1][0] - w[0][0], w[1][1] - w[0][1], w[1][2] - w[0][2]];
                        let e2 = [w[2][0] - w[0][0], w[2][1] - w[0][1], w[2][2] - w[0][2]];
                        let cx = e1[1] * e2[2] - e1[2] * e2[1];
                        let cy = e1[2] * e2[0] - e1[0] * e2[2];
                        let cz = e1[0] * e2[1] - e1[1] * e2[0];
                        if (cx * cx + cy * cy + cz * cz).sqrt() < 1e-3 {
                            classes[5] += 1;
                        }
                        if corners.iter().all(|d| d.is_some()) {
                            let top = corners.iter().map(|d| d.unwrap()).fold(f32::NEG_INFINITY, f32::max);
                            classes[if top <= 8.0 {
                                6
                            } else if top <= 32.0 {
                                7
                            } else if top <= 128.0 {
                                8
                            } else {
                                9
                            }] += 1;
                            if corners.iter().all(|d| d.unwrap() > 0.0) {
                                classes[13] += 1;
                            }
                        }
                        let wc = &wb[..3];
                        if wc.iter().all(|d| d.is_some_and(|v| v < 0.0)) {
                            classes[10] += 1;
                        } else if wc.iter().any(|d| d.is_some_and(|v| v < 0.0)) {
                            classes[11] += 1;
                        }
                        if wc.iter().any(|d| d.is_none()) {
                            classes[12] += 1;
                        }
                        let near = w.iter().filter(|p| near_golden(**p)).count();
                        if near == 3 {
                            classes[14] += 1;
                        } else if near == 0 {
                            classes[15] += 1;
                        }
                    }
                }
            }
            (1, total, removed, counts, classes)
        })
        .collect();
    let files: usize = per_quad.iter().map(|q| q.0).sum();
    let total: usize = per_quad.iter().map(|q| q.1).sum();
    let removed: usize = per_quad.iter().map(|q| q.2).sum();
    let mut agg = vec![[0usize; 3]; rules.len()];
    let mut classes = [0usize; 16];
    for q in &per_quad {
        for (i, c) in q.3.iter().enumerate() {
            for k in 0..3 {
                agg[i][k] += c[k];
            }
        }
        for k in 0..16 {
            classes[k] += q.4[k];
        }
    }
    println!("hidden-face study: {files} files, {total} triangles built, {removed} not in the golden files ({:.2}%)", 100.0 * removed as f64 / total.max(1) as f64);
    let mut order: Vec<usize> = (0..rules.len()).collect();
    order.sort_by_key(|&i| agg[i][1] + agg[i][2]);
    println!("  {:<22} {:>9} {:>9} {:>9} {:>8} {:>8}", "rule", "both", "rule-only", "gold-only", "prec%", "recall%");
    for &i in order.iter().take(24) {
        let [tp, fp, fnn] = agg[i];
        println!("  {:<22} {:>9} {:>9} {:>9} {:>8.2} {:>8.2}", rules[i].text(), tp, fp, fnn, 100.0 * tp as f64 / (tp + fp).max(1) as f64, 100.0 * tp as f64 / (tp + fnn).max(1) as f64);
    }
    if let (Some(d), Ok(path)) = (residue_dump, std::env::var("LODFORGE_HIDDEN_RESIDUE")) {
        let mut lines = d.into_inner().unwrap();
        lines.sort();
        std::fs::write(path, lines.join("
"))?;
    }
    println!("  residue of {} (golden removed, rule kept), overlapping classes:", reference.text());
    for (k, name) in CLASSES.iter().enumerate() {
        println!("    {:<20} {:>9}", name, classes[k]);
    }
    Ok(())
}

/// One vertex with every stored attribute in its on-disk encoding, in world space.
type VertexKey = ([u32; 3], [u16; 2], [u8; 4], [u8; 4], [u8; 4], u32);

#[derive(Default)]
struct ShapeSet {
    tris: Vec<[VertexKey; 3]>,
    /// Tolerant mode: world position (0.1 grid) -> normal and tangent bytes and the UV, for
    /// the direction / UV agreement figures.
    dirs: std::collections::HashMap<[u32; 3], ([u8; 3], [u8; 3], [f32; 2])>,
}

fn shape_sets(nif: &Nif, mode: CompareMode) -> BTreeMap<String, ShapeSet> {
    let mut out: BTreeMap<String, ShapeSet> = BTreeMap::new();
    for b in &nif.blocks {
        let Block::TriShape(s) = b else { continue };
        let mut name = s.av.net.name.clone().unwrap_or_default();
        if mode.tolerant {
            // Whether a merged group is named "...HD" follows its first member's vertex-alpha
            // flag, and LODGen's member order depends on thread timing.
            name = name.replace("HD", "");
        }
        let mut key = if mode.geometry_only { format!("{name}|{:#x}", s.vertex_desc & !(0x20u64 << 44) & 0xFFFF_0000_0000_0000) } else { format!("{name}|{:#x}", s.vertex_desc) };
        if let Some(Block::LightingShader(l)) = nif.get(s.shader_property) {
            if mode.geometry_only {
                key.push_str(&format!("|{:#x}|{:#x}", l.flags1, l.flags2 & !0x8000_0000 & !0x20));
            } else {
                key.push_str(&format!("|{:#x}|{:#x}|{}", l.flags1, l.flags2, l.texture_clamp_mode));
                if let Some(Block::TextureSet(t)) = nif.get(l.texture_set) {
                    key.push_str(&format!("|{}", t.join(",").to_lowercase()));
                }
            }
        }
        key.push_str(&format!("|x{}", s.av.net.extra_data.len()));
        let t = &s.av.transform;
        // Billboard colours come from LODGen's time-seeded Random (FlatLODColorVariance).
        let name = s.av.net.name.as_deref().unwrap_or("");
        let random_colors = mode.tolerant && (name.starts_with("tree") || name.starts_with("grass"));
        let vk = |i: usize| -> VertexKey {
            let p = s.positions[i];
            let w = [p[0] * t.scale + t.translation[0], p[1] * t.scale + t.translation[1], p[2] * t.scale + t.translation[2]];
            // Tolerant: UVs are left out of the key (LODGen's remapped halves differ from ours
            // by one 1/2048 step) and compared separately.
            let uv = if mode.geometry_only || mode.tolerant {
                [0, 0]
            } else {
                s.uvs.get(i).map(|u| [f32_to_half(u[0]), f32_to_half(u[1])]).unwrap_or_default()
            };
            let b = s.bitangents.get(i).copied().unwrap_or([0.0; 3]);
            let n = s.normals.get(i).map(|n| [float_to_byte(n[0]), float_to_byte(n[1]), float_to_byte(n[2]), float_to_byte(b[1])]).unwrap_or_default();
            let tg = s.tangents.get(i).map(|n| [float_to_byte(n[0]), float_to_byte(n[1]), float_to_byte(n[2]), float_to_byte(b[2])]).unwrap_or_default();
            let mut c = s.colors.get(i).copied().unwrap_or_default();
            if random_colors || mode.geometry_only {
                c = [0, 0, 0, if mode.geometry_only { 0 } else { c[3] }];
            }
            if mode.tolerant {
                (w.map(|v| (v / mode.grid).round() as i32 as u32), uv, [0; 4], [0; 4], c, 0)
            } else {
                (w.map(f32::to_bits), uv, n, tg, c, b[0].to_bits())
            }
        };
        let set = out.entry(key).or_default();
        if mode.tolerant {
            for i in 0..s.positions.len() {
                let p = s.positions[i];
                let w = [p[0] * t.scale + t.translation[0], p[1] * t.scale + t.translation[1], p[2] * t.scale + t.translation[2]].map(|v| (v / mode.grid).round() as i32 as u32);
                let n = s.normals.get(i).map(|n| n.map(float_to_byte)).unwrap_or([0; 3]);
                let tg = s.tangents.get(i).map(|n| n.map(float_to_byte)).unwrap_or([0; 3]);
                let uv = s.uvs.get(i).copied().unwrap_or([0.0; 2]);
                set.dirs.entry(w).or_insert((n, tg, uv));
            }
        }
        for tri in &s.triangles {
            let v = tri.map(|i| vk(i as usize));
            // Canonical rotation (keeps the winding).
            let r = (0..3).min_by_key(|&k| v[k]).unwrap();
            set.tris.push([v[r], v[(r + 1) % 3], v[(r + 2) % 3]]);
        }
    }
    for s in out.values_mut() {
        s.tris.sort_unstable();
    }
    out
}

#[derive(Default)]
struct Totals {
    /// Tolerant mode: vertices paired by position, and those whose normal / tangent bytes agree
    /// within one unit on every component.
    paired: usize,
    normals_close: usize,
    tangents_close: usize,
    uvs_close: usize,
    files: usize,
    identical_bytes: usize,
    canonical_equal: usize,
    shape_keys_differ: usize,
    tris_ours: usize,
    tris_gold: usize,
    tris_common: usize,
    missing: usize,
}

pub fn compare_dir(ours_dir: &Path, gold_dir: &Path, names: &[String], mode: CompareMode) -> Result<()> {
    let shape_diff = std::env::var("LODFORGE_SHAPE_DIFF").ok();
    let diffs = std::sync::Mutex::new(Vec::<String>::new());
    let results: Vec<(String, Option<(bool, bool, bool, usize, usize, usize, [usize; 4])>)> = names
        .par_iter()
        .map(|name| {
            let (Ok(gold), Ok(ours)) = (std::fs::read(gold_dir.join(name)), std::fs::read(ours_dir.join(name))) else { return (name.clone(), None) };
            let same_bytes = gold == ours;
            let (Ok(g), Ok(o)) = (lodforge_nif::parse(&gold), lodforge_nif::parse(&ours)) else { return (name.clone(), None) };
            let (gs, os) = (shape_sets(&g, mode), shape_sets(&o, mode));
            let keys_equal = gs.keys().eq(os.keys());
            let (mut common, mut tg, mut to) = (0, 0, 0);
            let mut all_equal = keys_equal;
            let mut dirs = [0usize; 4];
            let close = |a: &[u8; 3], b: &[u8; 3]| (0..3).all(|i| (i32::from(a[i]) - i32::from(b[i])).abs() <= 1);
            for (k, gv) in &gs {
                tg += gv.tris.len();
                if let Some(ov) = os.get(k) {
                    common += multiset_common(&gv.tris, &ov.tris);
                    all_equal &= gv.tris == ov.tris;
                    for (p, (gn, gt, guv)) in &gv.dirs {
                        if let Some((on, ot, ouv)) = ov.dirs.get(p) {
                            dirs[0] += 1;
                            dirs[1] += usize::from(close(gn, on));
                            dirs[2] += usize::from(close(gt, ot));
                            dirs[3] += usize::from((guv[0] - ouv[0]).abs() <= 1.0 / 1024.0 && (guv[1] - ouv[1]).abs() <= 1.0 / 1024.0);
                        }
                    }
                }
            }
            for ov in os.values() {
                to += ov.tris.len();
            }
            if shape_diff.is_some() {
                let mut d = diffs.lock().unwrap();
                for (k, gv) in &gs {
                    match os.get(k) {
                        None => d.push(format!("{name}\tgold\t{}\t{k}", gv.tris.len())),
                        Some(ov) if gv.tris != ov.tris => d.push(format!("{name}\tboth\t{} {} {}\t{k}", gv.tris.len(), ov.tris.len(), multiset_common(&gv.tris, &ov.tris))),
                        _ => {}
                    }
                }
                for (k, ov) in &os {
                    if !gs.contains_key(k) {
                        d.push(format!("{name}\tours\t{}\t{k}", ov.tris.len()));
                    }
                }
            }
            (name.clone(), Some((same_bytes, all_equal, keys_equal, tg, to, common, dirs)))
        })
        .collect();
    let mut t = Totals::default();
    let mut worst: Vec<(f64, String)> = Vec::new();
    let gold_names: Vec<String> = std::fs::read_dir(gold_dir)?.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.ends_with(".bto")).collect();
    let ours: std::collections::HashSet<&String> = names.iter().collect();
    let season_of = |n: &str| n.split('.').find(|p| ["SPR", "SUM", "AUT", "WIN"].contains(p)).map(str::to_string);
    let our_season = names.first().and_then(|n| season_of(n));
    let gold_only: Vec<&String> = gold_names.iter().filter(|n| !ours.contains(n) && season_of(n) == our_season).collect();
    for (name, r) in &results {
        let Some((bytes, eq, keys, tg, to, common, dirs)) = r else {
            t.missing += 1;
            continue;
        };
        t.files += 1;
        t.paired += dirs[0];
        t.normals_close += dirs[1];
        t.tangents_close += dirs[2];
        t.uvs_close += dirs[3];
        t.identical_bytes += usize::from(*bytes);
        t.canonical_equal += usize::from(*eq);
        t.shape_keys_differ += usize::from(!*keys);
        t.tris_gold += tg;
        t.tris_ours += to;
        t.tris_common += common;
        if !*eq {
            let score = 1.0 - *common as f64 / (*tg).max(*to).max(1) as f64;
            worst.push((score, format!("{name}: shapes {} / tris gold {tg} ours {to} common {common}", if *keys { "same" } else { "DIFFER" })));
        }
    }
    println!(
        "vs golden{}: {} files compared ({} missing in golden, {} golden-only); {} byte-identical; {} equal up to order; {} with different shape sets",
        match (mode.tolerant, mode.geometry_only) {
            (true, true) => format!(" (tolerant {} units, geometry only)", mode.grid),
            (true, false) => format!(" (tolerant {} units)", mode.grid),
            (false, true) => " (geometry only)".to_string(),
            (false, false) => String::new(),
        },
        t.files,
        t.missing,
        gold_only.len(),
        t.identical_bytes,
        t.canonical_equal,
        t.shape_keys_differ
    );
    println!(
        "  triangles: golden {}, ours {}, identical (all attributes) {} ({:.3}% of golden, {:.3}% of ours)",
        t.tris_gold,
        t.tris_ours,
        t.tris_common,
        100.0 * t.tris_common as f64 / t.tris_gold.max(1) as f64,
        100.0 * t.tris_common as f64 / t.tris_ours.max(1) as f64
    );
    if mode.tolerant && t.paired > 0 {
        println!(
            "  attributes: {} vertices paired by position in matching shapes; normals within 1 byte {:.2}%, tangents {:.2}%, UVs within 1/1024 {:.2}%",
            t.paired,
            100.0 * t.normals_close as f64 / t.paired as f64,
            100.0 * t.tangents_close as f64 / t.paired as f64,
            100.0 * t.uvs_close as f64 / t.paired as f64
        );
    }
    if let Some(path) = &shape_diff {
        let mut d = diffs.into_inner().unwrap();
        d.sort();
        std::fs::write(path, d.join("\n"))?;
    }
    if let Ok(path) = std::env::var("LODFORGE_DIFF_LIST") {
        let mut names: Vec<&str> = results.iter().filter(|(_, r)| !r.as_ref().is_some_and(|r| r.1)).map(|(n, _)| n.as_str()).collect();
        names.sort();
        std::fs::write(path, names.join("\n"))?;
    }
    worst.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    for (_, w) in worst.iter().take(12) {
        println!("  {w}");
    }
    for n in gold_only.iter().take(5) {
        println!("  golden only: {n}");
    }
    Ok(())
}

fn explain(ours: &Path, gold: &Path, mode: CompareMode) -> Result<()> {
    let o = shape_sets(&lodforge_nif::parse(&std::fs::read(ours)?)?, mode);
    let g = shape_sets(&lodforge_nif::parse(&std::fs::read(gold)?)?, mode);
    println!("{}:", ours.display());
    let keys: std::collections::BTreeSet<&String> = o.keys().chain(g.keys()).collect();
    for k in keys {
        let (a, b) = (g.get(k).map_or(&[][..], |s| &s.tris[..]), o.get(k).map_or(&[][..], |s| &s.tris[..]));
        let c = multiset_common(a, b);
        if c != a.len() || c != b.len() {
            println!("  gold {:6} ours {:6} common {:6}  {k}", a.len(), b.len(), c);
            let only_gold: Vec<_> = a.iter().filter(|t| b.binary_search(t).is_err()).take(2).collect();
            let only_ours: Vec<_> = b.iter().filter(|t| a.binary_search(t).is_err()).take(2).collect();
            for t in only_gold {
                println!("    gold: {:?}", t.map(|v| (v.0.map(f32::from_bits), v.1, v.2, v.3, v.4, f32::from_bits(v.5))));
            }
            for t in only_ours {
                println!("    ours: {:?}", t.map(|v| (v.0.map(f32::from_bits), v.1, v.2, v.3, v.4, f32::from_bits(v.5))));
            }
        }
    }
    Ok(())
}

/// Size of the multiset intersection of two sorted lists.
pub fn multiset_common<T: Ord>(a: &[T], b: &[T]) -> usize {
    let (mut i, mut j, mut n) = (0, 0, 0);
    while i < a.len() && j < b.len() {
        match a[i].cmp(&b[j]) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                n += 1;
                i += 1;
                j += 1;
            }
        }
    }
    n
}
