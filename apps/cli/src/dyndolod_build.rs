//! The DynDOLOD part of `lodforge build --mode dyndolod`, chained without DynDOLOD's own files:
//!
//! 1. per worldspace, the DynDOLOD object LOD export from the load order (`lodforge-dyndolod`)
//!    for every season, with the side files LODGen reads (terrain `.bin`, atlas map,
//!    FlatTextures) written to a work folder;
//! 2. the object LOD atlas (`textures\DynDOLOD\LOD\DynDOLOD_<World>[NN][_n].dds`);
//! 3. object LOD with the LODGen 3.1.8 port, which also writes `<World>.objects.heightmap.dat`;
//! 4. after all worldspaces: DynDOLOD.esm/.esp and the DLL NG data files for the dynamic LOD
//!    references of every worldspace;
//! 5. Occlusion.esp from the fresh terrain LOD, the object height maps and the new plugins.
//!
//! What the build reads from DynDOLOD's install is its configuration only (`Edit Scripts\
//! DynDOLOD`: INI, rules, configs), like DynDOLOD itself.

// The atlas planning (glow page included) lives in `object_atlas_plan` (object LOD stage
// B2.4), the FlatTextures list, the grass header and the complex-grass lines below (B2.5 /
// B2.6); `grass_lod1_nif` (`--grass go`, the Grass Optimizations far-LOD meshes) was rewritten
// from Community Shaders' loader requirements (object-lod-spec 11.9). The allow below still
// covers test-only helpers and atlas imports kept for the B2 stages.
#![allow(dead_code, unused_imports)]

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use lodforge_dyndolod::assign::Resolver;
use lodforge_dyndolod::bases::Base;
use lodforge_dyndolod::plugin::Previous;
use lodforge_dyndolod::export::{DynamicRef, ExportInput, Header, RefLine, Skip};
use lodforge_dyndolod::ini::Ini;
use lodforge_dyndolod::lodmodels::{LodIndex, ParallaxGenDiff};
use lodforge_dyndolod::rules::{self, Rule, RuleSource};
use lodforge_esp::Worldspace;
use lodforge_objlod::atlas_build::{AtlasLayout, layout, sorted_unique, texture_asset_name};
use lodforge_seasons::{Season, SeasonSwaps};

use crate::profile::Profile;

/// DynDOLOD options of `lodforge build`.
#[derive(clap::Args, Clone, Debug)]
pub struct Opts {
    /// DynDOLOD rule preset: Low, Medium or High.
    #[arg(long, default_value = "Medium")]
    pub preset: String,
    /// DynDOLOD's "Child > parent" setting: which `IgnoreChildEDID<Low|Medium|High>` list of the
    /// child worldspace configs filters the references copied from cities into their parent
    /// (default Low, DynDOLOD's default; independent of the rule preset: the High golden run
    /// still ignored `stonewall` / `treepine` bases, which only the Low list names).
    #[arg(long, default_value = "Low")]
    pub child_parent: String,
    /// DynDOLOD's `Edit Scripts\DynDOLOD` folder (configuration only; default:
    /// `<instance>\tools\DynDOLOD\Edit Scripts\DynDOLOD`).
    #[arg(long)]
    pub dyndolod_config: Option<PathBuf>,
    /// Static LOD rules only (no dynamic LOD references, no DynDOLOD.esm/.esp).
    #[arg(long)]
    pub static_lod: bool,
    /// Generate LOD models from the full models where the rules want object LOD and no LOD
    /// model is installed (so DynDOLOD Resources is not needed); written to
    /// `meshes\lod\lodforge\` in the output.
    #[arg(long)]
    pub lod_models: bool,
    /// Cache for generated LOD models (default: `.lodforge-cache` next to the output).
    #[arg(long)]
    pub lod_model_cache: Option<PathBuf>,
    /// Smallest object size (game units) that gets a generated model level 0, 1, 2.
    #[arg(long, value_delimiter = ',')]
    pub lod_model_min_size: Option<Vec<f32>>,
    /// Skip DynDOLOD.esm/.esp and the DLL NG data files.
    #[arg(long)]
    pub no_plugins: bool,
    /// Skip Occlusion.esp.
    #[arg(long)]
    pub no_occlusion: bool,
    /// Largest atlas tile of a LOD texture in pixels (`ObjectLODMaxTileSize`; default: the
    /// DynDOLOD INI, else 1024). Bigger tiles keep more texture detail on object LOD.
    #[arg(long)]
    pub atlas_max_tile: Option<u32>,
    /// Largest atlas tile of a full (non-LOD) texture (`FullTextureMaxTileSize`; default: the
    /// DynDOLOD INI, else 256).
    #[arg(long)]
    pub atlas_full_max_tile: Option<u32>,
    /// Folder for LODGen's input files (export, atlas map, terrain .bin, grass); default:
    /// `.lodforge-work-<pid>` next to the output, removed after the build.
    #[arg(long)]
    pub work_dir: Option<PathBuf>,
    /// Grass LOD: off, DynDOLOD-style grass in the LOD4 object LOD (`bto`, needs `--mode
    /// dyndolod`), or LOD meshes for Community Shaders' Grass Optimizations (`go`). `bto` reads
    /// NGIO's grass cache only: cells without a cache file get no grass (it no longer generates
    /// placements from the load order); `go` needs no cache.
    #[arg(long, value_enum, default_value = "off")]
    pub grass: GrassMode,
    /// Tree LOD (`--mode dyndolod`): `classic` (default; billboards in the game's tree LOD,
    /// at most 256 tree types per list) or `ultra` (DynDOLOD's Ultra tree LOD: every tree in
    /// object LOD, with the rule's LOD models where the preset asks for them (High: a 3D model
    /// at LOD4) and its billboard otherwise, and empty tree LOD lists).
    #[arg(long, value_enum, default_value = "classic")]
    pub tree_lod: TreeLodMode,
    /// With `--tree-lod ultra`: generate 3D tree LOD models for the trees big enough for them
    /// (TreeLODGen's method, docs/ultra-tree-models-plan.md) into the output before the export, so
    /// those trees get a 3D model where the preset asks for one. `generate` skips trees with an
    /// installed CRC-named 3D model, `all` replaces those too; `off` (default) uses installed
    /// models only.
    #[arg(long, value_enum, default_value = "off")]
    pub tree_models: TreeModelsMode,
    /// Level budgets of generated tree models, `keep:area:bark:floor[:max_scale]` for
    /// LOD4,LOD8,LOD16 (see `lodforge tree-models --levels`).
    #[arg(long)]
    pub tree_model_levels: Option<String>,
    /// Crown of generated tree models: `cloud` (default, billboard cloud over a 3D trunk) or
    /// `prune` (TreeLODGen's pruned leaf cards).
    #[arg(long, value_enum, default_value = "cloud")]
    pub tree_model_crown: crate::tree_models::CrownMode,
    /// Grass placements for `--grass bto`: always the NGIO grass cache (precache in game with NGIO).
    /// Cells without a cache file get no grass (one warning per worldspace); nothing is generated
    /// from the load order any more. Kept so existing command lines keep working.
    #[arg(long, value_enum, default_value = "auto")]
    pub grass_source: GrassSource,
    /// Grass LOD density in percent (default 50; DynDOLOD's default is 100).
    #[arg(long)]
    pub grass_density: Option<f32>,
    /// A new run token (`bunchofnumbers`) even when the output already holds a LODForge
    /// DynDOLOD.esp (rolling updates keep its token by default).
    #[arg(long)]
    pub new_token: bool,
    /// Number DynDOLOD.esm/.esp from scratch instead of keeping the form ids of the plugins
    /// already in the output (breaks saves made with them; implies --new-token).
    #[arg(long)]
    pub renumber: bool,
    /// Leave out DynDOLOD's large-reference workarounds (DynDOLOD.esm markers, RNAM, Is Full
    /// LOD reverts, `[Markers]` / `[Resets]`).
    #[arg(long)]
    pub no_large_ref_workarounds: bool,
    /// Glow LOD windows (DynDOLOD's `Windows`): `<model>glow_lod_N.nif` overlays in object LOD.
    #[arg(long)]
    pub windows: bool,
    /// Skip the terrain underside (`<World>_Underside.nif` and its DynDOLOD.esm/.esp records).
    #[arg(long)]
    pub no_underside: bool,
}

/// `Bounds` (SW x, SW y, NE x, NE y) and `NoCellsWithNAVM` of a child worldspace from
/// `Configs\DynDOLOD_SSE_childworld_<Child>.ini`, section `[<Parent>]`.
pub fn child_config(c: &Config, child: &str, parent: &str) -> (Option<(i32, i32, i32, i32)>, bool) {
    let Ok(ini) = Ini::read(&lodforge_dyndolod::overlay::path(&c.dir, &format!(r"Configs\DynDOLOD_SSE_childworld_{child}.ini"))) else { return (None, false) };
    let v: Vec<i32> = ini.get(parent, "Bounds").unwrap_or_default().split(',').filter_map(|s| s.trim().parse().ok()).collect();
    ((v.len() >= 4).then(|| (v[0], v[1], v[2], v[3])), ini.get(parent, "NoCellsWithNAVM").is_some_and(|s| s.trim() == "1"))
}

/// Cells between the loaded grid and the large-reference grid: (uLargeRefLODGridSize -
/// uGridsToLoad) / 2 from the profile's INIs (defaults 11 and 5).
pub fn large_ref_margin(prof: &crate::profile::ProfileArgs) -> i32 {
    let dir = prof.instance.join("profiles").join(&prof.profile);
    let read = |file: &str, key: &str, default: i32| -> i32 {
        std::fs::read_to_string(dir.join(file)).ok().and_then(|t| t.lines().find_map(|l| l.trim().strip_prefix(key).and_then(|v| v.trim().parse().ok()))).unwrap_or(default)
    };
    let large = read("skyrimprefs.ini", "uLargeRefLODGridSize=", 11);
    let grids = read("skyrim.ini", "uGridsToLoad=", 5);
    ((large - grids) / 2).max(0)
}

/// Whether DynDOLOD builds a terrain underside for `world` (`TerrainUndersideIgnoreWorlds`).
pub fn wants_underside(c: &Config, world: &str) -> bool {
    !c.cfg("TerrainUndersideIgnoreWorlds").split(',').any(|w| w.trim().eq_ignore_ascii_case(world))
}

/// DynDOLOD's configuration and everything the export needs that does not depend on the
/// worldspace.
pub struct Config {
    pub dir: PathBuf,
    pub ini: Ini,
    pub rules: Vec<Rule>,
    pub index: LodIndex,
    /// `ParallaxGen_Diff.json` (last winner in the VFS; empty without one).
    pub parallax: ParallaxGenDiff,
    pub mesh_paths: usize,
    pub bases: HashMap<u32, Base>,
    pub materials: HashMap<u32, String>,
    pub owners: HashMap<u32, usize>,
    pub ignore_edid: Vec<String>,
    pub xesp_lod: HashSet<u32>,
    pub swaps: SeasonSwaps,
    /// `--materials pbr` (default): PBR texture swaps in the side files and base copies.
    pub pbr: bool,
    /// `Windows` (glow LOD windows).
    pub windows: bool,
    /// Ultra tree LOD: trees in object LOD (`--tree-lod ultra`).
    pub ultra: bool,
    /// `Configs\DynDOLOD_SSE_mesh_crc32.txt` (empty when missing).
    pub mesh_crc: lodforge_dyndolod::lodmodels::MeshCrcTable,
    /// Winning TXST texture sets (MODS alternate textures).
    pub texture_sets: HashMap<u32, Vec<String>>,
    pub reference_states: HashMap<u32, lodforge_dyndolod::export::ReferenceState>,
}

impl Config {
    pub fn load(p: &Profile, dir: &Path, preset: &str, static_lod: bool) -> Result<Config> {
        let data_override = |path: &str| p.vfs.read(path).ok().flatten().map(|b| lodforge_dyndolod::ini::decode(&b));
        let src = RuleSource { rules_dir: dir.join("Rules"), data_override: &data_override };
        let rules = rules::build_rules(&src, "SSE", preset, !static_lod, &p.plugin_names);
        let mesh_crc = std::fs::read(lodforge_dyndolod::overlay::path(dir, r"Configs\DynDOLOD_SSE_mesh_crc32.txt")).map(|b| lodforge_dyndolod::lodmodels::MeshCrcTable::parse(&String::from_utf8_lossy(&b))).unwrap_or_default();
        let ini_path = lodforge_dyndolod::overlay::path(dir, "DynDOLOD_SSE.ini");
        let ini = Ini::read(&ini_path).with_context(|| format!("reading {}", ini_path.display()))?;
        let cfg = |k: &str| ini.get("DynDOLOD", k).unwrap_or("").to_string();
        let list = |k: &str| -> Vec<String> { cfg(k).split(',').map(|s| s.trim().to_ascii_lowercase()).filter(|s| !s.is_empty()).collect() };
        let (index, mesh_paths) = lod_index(p, &ini, dir);
        let parallax = parallax_diff(p);
        let lo = &p.load_order;
        let bases = lodforge_dyndolod::bases::read_bases(lo)?;
        let materials = lodforge_dyndolod::bases::read_materials(lo)?;
        let owners = lodforge_dyndolod::bases::owners(lo);
        let xesp_lod = std::fs::read(lodforge_dyndolod::overlay::path(dir, r"Configs\DynDOLOD_SSE_xesplod.txt")).map(|b| lodforge_dyndolod::bases::read_form_list(lo, &lodforge_dyndolod::ini::decode(&b))).unwrap_or_default();
        let edids: HashMap<String, u32> = bases.values().map(|b| (b.editor_id.to_ascii_lowercase(), b.form_id)).collect();
        let swaps = lodforge_seasons::load_swaps(&p.vfs, lo, &edids)?;
        let texture_sets = lodforge_dyndolod::sidefiles::read_texture_sets(lo)?;
        let reference_states = lodforge_dyndolod::export::read_reference_states(lo)?;
        Ok(Config { dir: dir.to_path_buf(), rules, index, parallax, mesh_paths, bases, materials, owners, ignore_edid: list("IgnoreBaseEDID"), xesp_lod, swaps, ini, windows: false, ultra: false, mesh_crc, pbr: true, texture_sets, reference_states })
    }

    /// Re-reads LOD model discovery after files were added to the VFS (generated LOD models).
    pub fn rescan(&mut self, p: &Profile) {
        (self.index, self.mesh_paths) = lod_index(p, &self.ini, &self.dir);
    }

    /// `[DynDOLOD]` value, empty when missing.
    pub fn cfg(&self, k: &str) -> String {
        self.ini.get("DynDOLOD", k).unwrap_or("").trim().to_string()
    }

    pub fn num<T: std::str::FromStr>(&self, k: &str, default: T) -> T {
        self.cfg(k).parse().unwrap_or(default)
    }
}

/// LOD model discovery over the VFS (`Folder1..5`, `IgnoreLODMesh`, `misnamed.txt`) and the
/// number of mesh paths seen.
fn lod_index(p: &Profile, ini: &Ini, dir: &Path) -> (LodIndex, usize) {
    let cfg = |k: &str| ini.get("DynDOLOD", k).unwrap_or("").to_string();
    let folders: Vec<String> = (1..=5).map(|i| cfg(&format!("Folder{i}"))).filter(|f| !f.is_empty()).collect();
    let ignore: Vec<String> = cfg("IgnoreLODMesh").split(',').map(|s| s.trim().to_ascii_lowercase()).filter(|s| !s.is_empty()).collect();
    let misnamed: HashSet<String> = std::fs::read(lodforge_dyndolod::overlay::path(dir, r"Configs\DynDOLOD_SSE_misnamed.txt"))
        .map(|b| lodforge_dyndolod::ini::decode(&b).lines().map(|l| l.trim().to_ascii_lowercase()).filter(|l| !l.is_empty() && !l.starts_with("//")).collect())
        .unwrap_or_default();
    let meshes = p.vfs.list_prefix("meshes\\");
    (LodIndex::scan(meshes.iter().map(String::as_str), &folders, &ignore, &misnamed), meshes.len())
}

/// PGPatcher's `ParallaxGen_Diff.json` from the VFS (loose in Data or in any mod, last winner).
/// `LODFORGE_NO_PARALLAXGEN` ignores it (for comparisons).
fn parallax_diff(p: &Profile) -> ParallaxGenDiff {
    if std::env::var_os("LODFORGE_NO_PARALLAXGEN").is_some() {
        return ParallaxGenDiff::default();
    }
    p.vfs.read(ParallaxGenDiff::FILE).ok().flatten().map(|b| ParallaxGenDiff::parse(&b)).unwrap_or_default()
}

/// Runs `f` with a model/billboard resolver over the profile's VFS.
pub fn with_resolver<R>(p: &Profile, c: &Config, f: impl FnOnce(&Resolver<'_>) -> R) -> R {
    let read = |path: &str| p.vfs.read(path).ok().flatten();
    let exists = |path: &str| p.vfs.exists(path);
    let r = Resolver { rules: &c.rules, index: &c.index, parallax: &c.parallax, materials: &c.materials, plugins: &p.plugin_names, owners: &c.owners, read: &read, exists: &exists, level32: false, mesh_crc: &c.mesh_crc };
    f(&r)
}

/// `LODGen_SSE_FlatTextures.txt` lines (docs/re/object-lod-spec.md 1.1.4, 15 tab-separated
/// fields): every tree billboard (TexGen `.dds` + `.txt` sidecar) of a loaded tree base, sorted
/// by texture. Fields: texture, width, height, shift Z, scale, `FlatLODLightingEffect`, the flat
/// normals file four times (`;`), `FlatLODGlowMap`, the vertex colour
/// (`FlatLODWithNormalMapVertexColor` when the billboard has an `_n.dds`, else
/// `FlatLODVertexColor`), `FlatLODColorVariance`, depth, shift X, shift Y,
/// `FlatLODHeightVariance`, and an empty last field, numbers in Delphi's `FloatToStr` form.
pub fn flat_textures(p: &Profile, c: &Config, r: &Resolver<'_>) -> Vec<String> {
    let _ = p;
    let normals = lodforge_dyndolod::overlay::path(&c.dir, r"Configs\DynDOLOD_SSE_flat_normals.txt");
    let normals = normals.display().to_string().replace('/', "\\").to_ascii_lowercase();
    let normals = [normals.as_str(); 4].join(";");
    let glow = {
        let g = c.cfg("FlatLODGlowMap");
        if g.is_empty() { "textures\\white.dds".to_string() } else { g }
    };
    let (light, vc, vc_n, variance, hvar) = (c.num("FlatLODLightingEffect", 0.0f64), c.num("FlatLODVertexColor", 0.55f64), c.num("FlatLODWithNormalMapVertexColor", 0.5f64), c.num("FlatLODColorVariance", 0.1f64), c.num("FlatLODHeightVariance", 0.0f64));
    let f = lodforge_dyndolod::export::float_to_str;
    let mut lines: Vec<(String, String)> = Vec::new();
    let mut seen = HashSet::new();
    for b in c.bases.values() {
        if !lodforge_dyndolod::assign::is_tree(b) {
            continue;
        }
        let Some(tex) = r.resolve(b).billboard else { continue };
        if !seen.insert(tex.clone()) {
            continue;
        }
        let stem = tex.trim_end_matches(".dds");
        let Some(txt) = (r.read)(&format!("{stem}.txt")) else { continue };
        let d = BillboardDims::parse(&String::from_utf8_lossy(&txt));
        let colour = if (r.exists)(&format!("{stem}_n.dds")) { vc_n } else { vc };
        let line = [
            tex.clone(),
            f(f64::from(d.width)),
            f(f64::from(d.height)),
            f(f64::from(d.shift[2])),
            f(f64::from(d.scale)),
            f(light),
            normals.clone(),
            glow.clone(),
            f(colour),
            f(variance),
            f(f64::from(d.depth)),
            f(f64::from(d.shift[0])),
            f(f64::from(d.shift[1])),
            f(hvar),
            String::new(),
        ]
        .join("\t");
        lines.push((tex, line));
    }
    lines.sort();
    lines.into_iter().map(|(_, l)| l).collect()
}

/// Large references of a worldspace as DynDOLOD treats them: listed by a master plugin, still in
/// their grid cell, and not overridden by a non-master plugin.
pub fn large_refs(lo: &lodforge_esp::LoadOrder, world: &Worldspace) -> HashSet<u32> {
    let mut large = HashSet::new();
    for &(pi, grid, r) in &world.large_refs {
        if !lo.plugins[pi as usize].plugin.is_master() {
            continue;
        }
        if let Some(pl) = world.refs.get(&r) {
            let cell = ((pl.position[0] / 4096.0).floor() as i32, (pl.position[1] / 4096.0).floor() as i32);
            if cell == grid && lo.plugins[pl.plugin as usize].plugin.is_master() {
                large.insert(r);
            }
        }
    }
    large
}

/// `Bounds` of child worldspaces (`Configs\DynDOLOD_SSE_childworld_<Child>.ini`, section
/// `[<World>]`) that do not copy parent references (`ScanParent=0`): there the parent's trees
/// use object LOD.
pub fn tree_object_bounds(c: &Config, world: &str) -> Vec<(i32, i32, i32, i32)> {
    tree_object_bounds_in(&c.dir, world)
}

/// `TreeLODMaxTilt` from `DynDOLOD_SSE.ini` in `dir` (default 30).
pub fn tree_max_tilt(dir: &Path) -> f64 {
    Ini::read(&lodforge_dyndolod::overlay::path(dir, "DynDOLOD_SSE.ini")).ok().and_then(|i| i.get("DynDOLOD", "TreeLODMaxTilt").and_then(|v| v.trim().parse().ok())).unwrap_or(30.0)
}

/// [`tree_object_bounds`] from a configuration folder.
pub fn tree_object_bounds_in(dir: &Path, world: &str) -> Vec<(i32, i32, i32, i32)> {
    let mut bounds = Vec::new();
    for path in lodforge_dyndolod::overlay::list(dir, "Configs") {
        let name = path.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        if !(name.starts_with("dyndolod_sse_childworld_") && name.ends_with(".ini")) {
            continue;
        }
        let Ok(ini) = Ini::read(&path) else { continue };
        let scan_parent = ini.get(world, "ScanParent").is_some_and(|v| v.trim() == "1");
        if !scan_parent && let Some(b) = ini.get(world, "Bounds") {
            let v: Vec<i32> = b.split(',').filter_map(|s| s.trim().parse().ok()).collect();
            if v.len() >= 4 {
                bounds.push((v[0], v[1], v[2], v[3]));
            }
        }
    }
    bounds
}

/// Lodsettings south-west cell of a worldspace (0,0 without a lodsettings file).
pub fn south_west(p: &Profile, world: &str) -> (i32, i32) {
    let l = p.vfs.read(&format!("lodsettings\\{world}.lod")).ok().flatten().unwrap_or_default();
    if l.len() >= 4 { (i32::from(i16::from_le_bytes([l[0], l[1]])), i32::from(i16::from_le_bytes([l[2], l[3]]))) } else { (0, 0) }
}

/// Crown, Trunk and FlatTrunk vertex colour multipliers (R, G, B) for a season. The exe's keys
/// are `CrownBrightness` and `CrownBrightnessR/G/B`; a season suffix (`CrownBrightness_WIN`,
/// `CrownBrightnessR_WIN`) gives a per-season value. Most specific key wins: channel+season,
/// season, channel, base, then 1.
pub fn tree_brightness(ini: &Ini, season: Option<Season>) -> [[f64; 3]; 3] {
    let get = |k: &str| ini.get("DynDOLOD", k).and_then(|v| v.trim().parse::<f64>().ok());
    let sfx = season.map(|s| format!("_{}", s.code()));
    ["Crown", "Trunk", "FlatTrunk"].map(|n| {
        ["R", "G", "B"].map(|ch| {
            let seasonal = |k: String| sfx.as_ref().and_then(|s| get(&format!("{k}{s}")));
            seasonal(format!("{n}Brightness{ch}"))
                .or_else(|| seasonal(format!("{n}Brightness")))
                .or_else(|| get(&format!("{n}Brightness{ch}")))
                .or_else(|| get(&format!("{n}Brightness")))
                .unwrap_or(1.0)
        })
    })
}

/// The export header (without season keys). `export_dir` holds the side files (with a trailing
/// backslash), `game_data` is the game's Data folder.
pub fn header(p: &Profile, c: &Config, world: &Worldspace, export_dir: &str, game_data: &Path, path_output: &str) -> Header {
    let game_data = format!("{}\\", game_data.display()).replace('/', "\\");
    Header {
        world: world.editor_id.clone(),
        south_west: south_west(p, &world.editor_id),
        export_dir: export_dir.to_string(),
        season: String::new(),
        lod_coords: None,
        limit_coords4: Vec::new(),
        limit_coords8: Vec::new(),
        resources: p.vfs.archives().iter().map(|b| format!("{game_data}{}", b.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())).collect(),
        hd_lod_texture: world.hd_lod_texture.clone(),
        ignore_textures: c.cfg("IgnoreTexture").split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
        remove_unseen_z_shift: c.num("RemoveUnseenZShift", 10.0),
        flat_lod_level_flag: c.num("FlatLODLevelLODFlag", 0),
        brightness: tree_brightness(&c.ini, None),
        path_data: game_data.clone(),
        path_output: path_output.to_string(),
    }
}

/// One season's export.
pub struct SeasonExport {
    pub season: Option<Season>,
    pub lines: Vec<RefLine>,
    pub skipped: HashMap<u32, Skip>,
    /// The whole export file.
    pub text: String,
    /// `LODGen_SSE_VC_*` and `LODGen_SSE_AltTextures_*` (see [`add_side_files`]).
    pub vc: String,
    pub alt_textures: String,
}

impl SeasonExport {
    /// `_SPR` etc., empty for the default season.
    pub fn suffix(&self) -> String {
        self.season.map(|s| format!("_{}", s.code())).unwrap_or_default()
    }
}

/// A worldspace's export: the default season first, then the requested seasons; plus the
/// dynamic LOD references (default season).
pub struct WorldExport {
    pub seasons: Vec<SeasonExport>,
    pub dynamic: Vec<DynamicRef>,
    /// References with object LOD (default season) or tree LOD.
    pub lod_refs: Vec<u32>,
    /// Original bases used by emitted large-reference object LOD.
    pub lod_bases: HashSet<u32>,
    /// Per season: (base, model) of each dynamic LOD reference in that season.
    pub seasonal_dynamic: Vec<(&'static str, HashMap<u32, (u32, String)>)>,
    /// Child worldspace references copied into this worldspace that got object LOD, with the model
    /// of their DynDOLOD.esm base copy.
    pub child_copies: Vec<(lodforge_dyndolod::childworld::ChildCopy, String)>,
}

/// The part of a reference line that decides its LOD meshes (the model columns).
fn lod_key(l: &RefLine) -> String {
    let mut k = String::new();
    for c in &l.columns {
        k.push_str(c);
        k.push('|');
    }
    k
}

/// Builds the export of every season. Season exports carry `LODCoords`: the quads whose lines
/// differ from the default season (LODGen copies the rest).
pub fn export_world(p: &Profile, c: &Config, r: &Resolver<'_>, world: &Worldspace, seasons: &[Season], header: &Header, copies: &[lodforge_dyndolod::childworld::ChildCopy]) -> Result<WorldExport> {
    let lo = &p.load_order;
    // Child worldspace copies are exported as references of this worldspace.
    let with_copies;
    let world = if copies.is_empty() {
        world
    } else {
        with_copies = lodforge_dyndolod::childworld::with_copies(world, copies);
        &with_copies
    };
    let mut large = large_refs(lo, world);
    large.extend(copies.iter().filter(|c| c.large).map(|c| c.copy_id));
    let bounds = tree_object_bounds(c, &world.editor_id);
    let tilt: f64 = c.num("TreeLODMaxTilt", 30.0);
    let sw = header.south_west;
    let mut default_lines: HashMap<u32, String> = HashMap::new();
    let mut out = WorldExport { seasons: Vec::new(), dynamic: Vec::new(), seasonal_dynamic: Vec::new(), child_copies: Vec::new(), lod_refs: Vec::new(), lod_bases: HashSet::new() };
    let ignore_plugins = lodforge_dyndolod::childworld::ModWorldIgnore::read(&c.dir).plugins_for(&world.editor_id);
    for season in std::iter::once(None).chain(seasons.iter().copied().map(Some)) {
        let season_swaps: HashMap<u32, u32> = match season {
            Some(s) => c.swaps.swaps.get(&s).map(|secs| secs.values().flat_map(|m| m.iter().map(|(a, b)| (*a, *b))).collect()).unwrap_or_default(),
            None => HashMap::new(),
        };
        let inp = ExportInput { swaps: &season_swaps, world, bases: &c.bases, materials: &c.materials, resolver: r, rules: &c.rules, plugins: &p.plugin_names, owners: &c.owners, ignore_base_edid: &c.ignore_edid, large_refs: &large, xesp_lod: &c.xesp_lod, tree_max_tilt: tilt, tree_object_bounds: &bounds, windows: c.windows, ignore_plugins: &ignore_plugins, ultra: c.ultra, reference_states: &c.reference_states };
        let (lines, skipped) = lodforge_dyndolod::export::build_lines(&inp);
        out.lod_bases.extend(lodforge_dyndolod::export::large_object_bases(&lines, &large));
        let mut h = header.clone();
        if let Some(s) = season {
            h.season = format!("_{}", s.code());
            h.brightness = tree_brightness(&c.ini, Some(s));
            let mut lc: [std::collections::BTreeSet<(i32, i32)>; 4] = Default::default();
            let ours: HashMap<u32, String> = lines.iter().map(|l| (l.ref_id, lod_key(l))).collect();
            let mut changed: Vec<u32> = ours.iter().filter(|(id, t)| default_lines.get(id) != Some(t)).map(|(id, _)| *id).collect();
            changed.extend(default_lines.keys().filter(|id| !ours.contains_key(id)));
            for id in changed {
                let Some(rf) = world.refs.get(&id) else { continue };
                let cell = ((rf.position[0] / 4096.0).floor() as i32, (rf.position[1] / 4096.0).floor() as i32);
                for (i, l) in [4, 8, 16, 32].into_iter().enumerate() {
                    let q = |c: i32, s: i32| s + (c - s) / l * l;
                    lc[i].insert((q(cell.0, sw.0), q(cell.1, sw.1)));
                }
            }
            h.lod_coords = Some(lc.map(|s| s.into_iter().collect()));
            if let Some(s) = season {
                let dynamic = lodforge_dyndolod::export::build_dynamic(&inp).map_err(anyhow::Error::msg)?;
                let map = dynamic.into_iter().map(|d| (d.ref_id, (d.base, d.model))).collect();
                out.seasonal_dynamic.push((s.code(), map));
            }
        } else {
            default_lines = lines.iter().map(|l| (l.ref_id, lod_key(l))).collect();
            out.dynamic = lodforge_dyndolod::export::build_dynamic(&inp).map_err(anyhow::Error::msg)?;
            out.lod_refs = lines.iter().map(|l| l.ref_id).chain(skipped.iter().filter(|(_, s)| matches!(s, Skip::TreeLod)).map(|(id, _)| *id)).collect();
            // The base copy of a copied reference shows its LOD4 model (trees: the full model).
            for cp in copies {
                // The normal line, not the glow window line before it.
                let Some(l) = lines.iter().find(|l| l.ref_id == cp.copy_id && l.columns.iter().any(|c| !c.is_empty()) && !l.columns.iter().any(|c| c.to_ascii_lowercase().contains("glow_lod"))) else { continue };
                let Some(b) = c.bases.get(&cp.placement.base) else { continue };
                let model = if lodforge_dyndolod::assign::is_tree(b) { b.model.clone() } else { l.columns.iter().find(|c| !c.is_empty()).cloned().unwrap_or_default() };
                out.child_copies.push((cp.clone(), model));
            }
        }
        let text = h.text() + &lines.iter().map(|l| l.text() + "\r\n").collect::<String>() + &iclod_line(p, world);
        out.seasons.push(SeasonExport { season, lines, skipped, text, vc: String::new(), alt_textures: String::new() });
    }
    Ok(out)
}

/// The Imperial City LOD reference DynDOLOD appends to every Tamriel export
/// (`Configs\DynDOLOD_SSE_manual_static_Tamriel.txt`, docs/re/object-lod-spec.md 1.1.3): form
/// 0, position (98400, -237948, -7100), `ICLOD`, `meshes\lod\iclod_lod.nif` for LOD4/8/16, as
/// the golden `LODGen_SSE_Export_Tamriel.txt` has it. Only when the model is installed.
pub fn iclod_line(p: &Profile, world: &Worldspace) -> String {
    if !world.editor_id.eq_ignore_ascii_case("Tamriel") || !p.vfs.exists("meshes\\lod\\iclod_lod.nif") {
        return String::new();
    }
    "00000000\t00000000\t98400.0000\t-237948.0000\t-7100.0000\t0.0000\t0.0000\t0.0000\t1.0\tICLOD\t00008000\t\tmeshes\\lod\\iclod_lod.nif\tmeshes\\lod\\iclod_lod.nif\tmeshes\\lod\\iclod_lod.nif\t\r\n".to_string()
}

/// DynDOLOD's terrain input for LODGen (`LODGen_SSE_LODGen_Terrain_<World>.bin`) from the load
/// order: the cell dump of docs/re/terrain-lod-spec.md 2.5 (x, y, water, hide flags, VHGT per
/// cell) as `lodforge_btr::terrain_bin` writes it, over the cells `lodforge_btr::export` exports
/// (every CELL inside the LAND rectangle, defaults from the worldspace's DNAM with the parent
/// rules). Only kept for LODGen compatibility: the object LOD builder reads LAND heights itself.
pub fn terrain_bin(lo: &lodforge_esp::LoadOrder, world: &Worldspace, parent: Option<&Worldspace>) -> Result<Vec<u8>> {
    let input = lodforge_btr::export::terrain_input(lo, world, parent, (0, 0), 32)?;
    Ok(lodforge_btr::terrain_bin::write_cells(&input))
}

/// DynDOLOD's object LOD atlas settings (`DynDOLOD_SSE.ini`): page cap, tile caps, formats and
/// mip count. Format codes are DynDOLOD's: 200 BC1, 201 BC2, 202 BC3, 225/226 BC7, 87/88
/// uncompressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AtlasSettings {
    /// `ObjectLODMaxAtlasSize`.
    pub max_atlas: u32,
    /// `ObjectLODMaxTileSize`: largest tile of a LOD texture.
    pub max_tile: u32,
    /// `FullTextureMaxTileSize`: largest tile of a full (non-LOD) texture.
    pub full_max_tile: u32,
    /// `ObjectLODDiffuseFormat`, `ObjectLODAlphaDiffuseFormat`, `ObjectLODNormalFormat`.
    pub diffuse_format: u32,
    pub alpha_format: u32,
    pub normal_format: u32,
    /// `ObjectLODMinMipMaps` (hidden key): mip levels per atlas page. DynDOLOD 3's pages have 8
    /// (`DynDOLOD_Tamriel.dds` 8192x4096, `_Alpha.dds` 2048x2048).
    pub mips: u32,
}

impl AtlasSettings {
    pub fn from_config(c: &Config) -> AtlasSettings {
        AtlasSettings {
            max_atlas: c.num("ObjectLODMaxAtlasSize", 8192u32).clamp(256, 16384),
            max_tile: c.num("ObjectLODMaxTileSize", 1024u32).max(1),
            full_max_tile: c.num("FullTextureMaxTileSize", 256u32).max(1),
            diffuse_format: c.num("ObjectLODDiffuseFormat", 200),
            alpha_format: c.num("ObjectLODAlphaDiffuseFormat", 225),
            normal_format: c.num("ObjectLODNormalFormat", 200),
            mips: c.num("ObjectLODMinMipMaps", 8u32).max(1),
        }
    }

    /// Different diffuse formats for alpha and opaque shapes: two atlas families, the alpha one
    /// `DynDOLOD_<World>_Alpha.dds` (DynDOLOD's `SeparateAlphaAtlas`).
    pub fn separate_alpha(&self) -> bool {
        self.diffuse_format != self.alpha_format
    }

    /// The page formats for [crate::object_atlas::render].
    pub fn formats(&self) -> crate::object_atlas::Formats {
        use crate::object_atlas::Encoding;
        let enc = |code: u32| if code == 200 { Encoding::Bc1 } else { Encoding::Bc7 };
        crate::object_atlas::Formats { diffuse: enc(self.diffuse_format), alpha_diffuse: enc(self.alpha_format), normal: enc(self.normal_format), mips: self.mips, alpha_fill: true }
    }
}

/// LOD textures (TexGen's `textures\lod\`, billboards under `textures\terrain\lodgen\`,
/// DynDOLOD's `textures\dyndolod\lod\`) get tiles up to `ObjectLODMaxTileSize`; any other texture
/// is a full texture, cut to `FullTextureMaxTileSize` (DynDOLOD 3 Tamriel: 2048 and 4096 full
/// textures become 256 tiles, 508x512 billboards stay as they are).
pub fn is_lod_texture(texture: &str) -> bool {
    let t = texture.to_ascii_lowercase();
    t.contains("\\lod\\") || t.contains("\\lodgen\\")
}

/// Tile size of a `w` x `h` source: halved until both sides fit `cap`.
pub fn tile_size(mut w: u32, mut h: u32, cap: u32) -> (u32, u32) {
    while w > cap || h > cap {
        w = (w / 2).max(1);
        h = (h / 2).max(1);
    }
    (w, h)
}

/// Whether `texture` matches one of DynDOLOD's `IgnoreTexture` masks (file names or parts of the
/// path, case-insensitive; `*` wildcards).
pub(crate) fn ignored_texture(texture: &str, masks: &[String]) -> bool {
    let t = texture.to_ascii_lowercase();
    let file = t.rsplit('\\').next().unwrap_or(&t);
    masks.iter().any(|m| {
        let m = m.to_ascii_lowercase();
        if m.contains('*') {
            let parts: Vec<&str> = m.split('*').collect();
            let mut at = 0;
            parts.iter().enumerate().all(|(i, p)| {
                if p.is_empty() {
                    return true;
                }
                match file[at..].find(p) {
                    Some(k) if i > 0 || k == 0 => {
                        at += k + p.len();
                        true
                    }
                    _ => false,
                }
            }) && (parts.last().is_some_and(|l| l.is_empty()) || file.ends_with(parts.last().unwrap()))
        } else {
            file == m || t.contains(&m)
        }
    })
}

/// The object LOD atlas for a worldspace (all seasons share one, as DynDOLOD's default
/// `SeasonsCombineAtlas=1`): the textures the LOD model shapes of every reference end up with
/// and the billboards the exports place directly, without `IgnoreTexture` matches and the worldspace's HD LOD
/// texture, packed with xEdit's atlas packer.
///
/// As DynDOLOD 3 (Tamriel E2E reference, `LODGen_SSE_ObjectAtlasMap_Tamriel.lst`): textures with
/// see-through texels ([has_transparency]; billboards among them) go to
/// `textures\DynDOLOD\LOD\DynDOLOD_<World>_Alpha.dds` when the alpha diffuse format differs,
/// everything else to `DynDOLOD_<World>.dds`; pages are sized by [square_layout]; keys are `diffuse,normal` (just `diffuse`
/// when there is no normal map, `=N` for alpha thresholds other than 128); tiles are cut to [`AtlasSettings::max_tile`] /
/// [`AtlasSettings::full_max_tile`].
///
/// `mini` are the TexGen mini-atlas rows ([crate::texgen_stitched::miniatlas_rows]). All of them
/// go into the map as they are (DynDOLOD's map has every row of its re-uv list); a model texture
/// with a row is re-UVed by LODGen onto its mini atlas, so the mini atlas takes its place on the
/// atlas pages (tiled shapes included). `grass` are the grass billboard views of the grass
/// stage ([grass_billboard_views]; the files must already be in the VFS), empty without
/// `--grass bto`.
pub fn atlas(p: &Profile, c: &Config, world: &Worldspace, exports: &WorldExport, mini: &[lodforge_texgen::stitch::MiniAtlasRow], grass: &[String], trees: &[String], match_far: Option<(&Path, f32)>) -> Result<AtlasLayout> {
    let settings = AtlasSettings::from_config(c);
    // Never atlased: `IgnoreTexture` masks and the vanilla HD LOD texture pair the export
    // header names (`IsHDTextureMask`).
    let mut masks: Vec<String> = c.cfg("IgnoreTexture").split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
    masks.push("mountainslab02.dds".into());
    masks.push("mountainslab02_n.dds".into());
    // Tree billboards (FlatTextures) and grass views go on the `_Alpha` page as one-part keys.
    let mut billboards: Vec<String> = grass.to_vec();
    billboards.extend(trees.iter().cloned());
    let input = crate::object_atlas_plan::PlanInput { settings, tolerance: 0.2, ignore_masks: &masks, mini, billboards: &billboards, base: format!("textures\\DynDOLOD\\LOD\\DynDOLOD_{}", world.editor_id), match_far };
    let (layout, st) = crate::object_atlas_plan::plan(p, exports, &input);
    println!("  atlas plan: {} models, {} keys: {} main, {} alpha, {} glow tiles ({} repeat tiles for generated models), {} mini rows; skipped {} tiled, {} missing, {} ignored; {} far-LOD bakes matched to LOD4", st.models, st.keys, st.main, st.alpha, st.glow, st.repeat_tiles, st.mini, st.skipped_tiled, st.skipped_missing, st.skipped_ignored, st.matched);
    Ok(layout)
}

/// Atlas pages for `tiles` as DynDOLOD sizes them: a square bin as wide as the power of two
/// covering the square root of the tiles' area (at least the widest tile), doubled until every
/// tile fits on one page or `max` is reached, then [layout] (which crops each page to its tiles
/// and spills the rest onto `<base>01.dds`, ...). DynDOLOD 3 Tamriel: 8192x4096 and 2048x2048.
pub fn square_layout(tiles: &[(String, u32, u32)], max: u32, base: &str) -> AtlasLayout {
    let area: u64 = tiles.iter().map(|(_, w, h)| u64::from(*w) * u64::from(*h)).sum();
    let widest = tiles.iter().map(|(_, w, h)| (*w).max(*h)).max().unwrap_or(1);
    let mut side = 1u32;
    while u64::from(side) * u64::from(side) < area {
        side *= 2;
    }
    let mut side = side.max(widest.next_power_of_two()).min(max);
    loop {
        let l = layout(tiles, side, side, base);
        if l.pages.len() <= 1 || side >= max {
            return l;
        }
        side = (side * 2).min(max);
    }
}

/// Whether any RGBA8 texel is below LOD's alpha threshold (128): the texture needs a format with
/// alpha (DynDOLOD's `ObjectLODAlphaDiffuseFormat` page).
pub fn has_transparency(rgba: &[u8]) -> bool {
    rgba.chunks_exact(4).any(|p| p[3] < 128)
}

/// The diffuse of an atlas map key (`diffuse[,normal[,glow]][=alpha]`), lower case.
pub fn atlas_key_diffuse(key: &str) -> String {
    key.split(['=', ',']).next().unwrap_or("").to_lowercase()
}

/// `LODGen_SSE_VC_*` and `LODGen_SSE_AltTextures_*` text for one season's reference lines.
pub fn side_texts(p: &Profile, c: &Config, cache: &lodforge_dyndolod::sidefiles::ModelCache, lines: &[RefLine]) -> (String, String) {
    let read = |path: &str| p.vfs.read(path).ok().flatten();
    let exists = |path: &str| p.vfs.exists(path);
    let src = lodforge_dyndolod::sidefiles::Sources { bases: &c.bases, texture_sets: &c.texture_sets, read: &read, exists: &exists, pbr: c.pbr };
    (lodforge_dyndolod::sidefiles::vc_text(lines, cache, &src), lodforge_dyndolod::sidefiles::alt_textures_text(lines, cache, &src))
}

/// Fills every season's VC and AltTextures text (models read once per worldspace); returns each
/// season's mini-atlas requests (DynDOLOD's `AddMiniAtlas`).
pub fn add_side_files(p: &Profile, c: &Config, exports: &mut WorldExport) -> Vec<Vec<lodforge_dyndolod::sidefiles::MiniAtlasRequest>> {
    let cache = lodforge_dyndolod::sidefiles::ModelCache::default();
    let read = |path: &str| p.vfs.read(path).ok().flatten();
    let exists = |path: &str| p.vfs.exists(path);
    let src = lodforge_dyndolod::sidefiles::Sources { bases: &c.bases, texture_sets: &c.texture_sets, read: &read, exists: &exists, pbr: c.pbr };
    let mut requests = Vec::new();
    for s in &mut exports.seasons {
        (s.vc, s.alt_textures) = side_texts(p, c, &cache, &s.lines);
        requests.push(lodforge_dyndolod::sidefiles::mini_atlas_requests(&s.lines, &cache, &src));
    }
    requests
}

/// Side files and exports of one worldspace in `export_dir`; returns the export file paths,
/// default season first.
pub fn write_world_files(export_dir: &Path, world: &str, exports: &WorldExport, terrain_bin: &[u8], atlas: &AtlasLayout) -> Result<Vec<PathBuf>> {
    std::fs::create_dir_all(export_dir)?;
    std::fs::write(export_dir.join(format!("LODGen_SSE_LODGen_Terrain_{world}.bin")), terrain_bin)?;
    std::fs::write(export_dir.join(format!("LODGen_SSE_ObjectAtlasMap_{world}.txt")), atlas.map_text())?;
    let mut paths = Vec::new();
    for s in &exports.seasons {
        let suffix = s.suffix();
        std::fs::write(export_dir.join(format!("LODGen_SSE_VC_{world}{suffix}.txt")), &s.vc)?;
        std::fs::write(export_dir.join(format!("LODGen_SSE_AltTextures_{world}{suffix}.txt")), &s.alt_textures)?;
        let path = export_dir.join(format!("LODGen_SSE_Export_{world}{suffix}.txt"));
        std::fs::write(&path, &s.text).with_context(|| format!("writing {}", path.display()))?;
        paths.push(path);
    }
    Ok(paths)
}

/// DynDOLOD.esm and DynDOLOD.esp of an earlier LODForge build in `out`, for rolling updates
/// (plugins written by DynDOLOD itself or anything else are not continued).
pub fn previous_plugins(out: &Path) -> (Option<Previous>, Option<Previous>) {
    let read = |name: &str| std::fs::read(out.join(name)).ok().and_then(|b| Previous::read(&b)).filter(|p| p.author == "LODForge");
    (read("DynDOLOD.esm"), read("DynDOLOD.esp"))
}

/// One worldspace's input to [`write_plugins`], from its export.
pub struct PluginWorld<'a> {
    pub world: &'a Worldspace,
    pub dynamic: Vec<DynamicRef>,
    /// Per season: (base, model) of each dynamic LOD reference in that season.
    pub seasonal: Vec<(&'static str, HashMap<u32, (u32, String)>)>,
    /// References with object or tree LOD (for the Is Full LOD reverts).
    pub lod_refs: Vec<u32>,
    pub lod_bases: HashSet<u32>,
    /// Child worldspace references copied in, with the model of their DynDOLOD.esm base copy.
    pub child_copies: Vec<(lodforge_dyndolod::childworld::ChildCopy, String)>,
}

/// BaseLODDLL defaults to runtime detection. A forced request on an unsupported setup fails.
pub fn base_lod_data(p: &Profile, c: &Config, ids: impl IntoIterator<Item = u32>) -> Result<String> {
    let has_dll = p.vfs.loose_path(r"SKSE\Plugins\DynDOLOD.DLL").is_some();
    let Some(version) = crate::runtime::base_lod_runtime(&c.cfg("BaseLODDLL"), has_dll, || crate::runtime::file_version(&p.game_dir.join("SkyrimSE.exe")))? else {
        return Ok(String::new());
    };
    let data = lodforge_dyndolod::dynplugins::base_lod_section(&p.load_order, &c.bases, ids).map_err(anyhow::Error::msg)?;
    println!("  BaseLODDLL: {} base flag updates (Skyrim {version:?})", data.lines().count() - 1);
    Ok(data)
}

/// Plugin stage: DynDOLOD.esm, DynDOLOD.esp and `SKSE\Plugins\DynDOLOD_Data\` in `out`. When
/// `out` already holds plugins of an earlier LODForge build they are updated in place: same form
/// ids for the same records, new ids after theirs, same run token (unless `opts` says otherwise).
/// Returns (dynamic references, worldspaces).
/// Returns (dynamic references, worldspaces, the `...linear.dds` copies the base copies' texture
/// sets point at, lower-case).
pub fn write_plugins(p: &Profile, c: &Config, opts: &Opts, worlds: &[PluginWorld<'_>], all_worlds: &[Worldspace], undersides: &HashSet<String>, margin: i32, out: &Path) -> Result<(usize, usize, std::collections::BTreeSet<String>)> {
    let (prev_esm, prev_esp) = if opts.renumber { (None, None) } else { previous_plugins(out) };
    for (name, prev) in [("DynDOLOD.esm", &prev_esm), ("DynDOLOD.esp", &prev_esp)] {
        if let Some(pr) = prev {
            println!("  loading {name} next object ID {:X} from {} for rolling updates ({} records)", pr.next_object_id, out.join(name).display(), pr.records);
        }
    }
    let token = match prev_esp.as_ref().and_then(|pr| pr.token.clone()) {
        Some(t) if !opts.new_token && !opts.renumber => t,
        _ => {
            let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1);
            lodforge_dyndolod::dynplugins::bunch_of_numbers(seed)
        }
    };
    let rolling = lodforge_dyndolod::dynplugins::Rolling { esm: prev_esm.as_ref(), esp: prev_esp.as_ref() };
    let height: f32 = c.num("TerrainUndersideHeight", 500.0);
    // The underside is enabled in the worldspace and in the child worldspaces that use its land.
    let underside = |w: &Worldspace| -> Option<(f32, Vec<u32>)> {
        undersides.contains(&w.editor_id).then(|| (height, std::iter::once(w.form_id).chain(all_worlds.iter().filter(|x| x.parent == Some(w.form_id) && x.parent_flags & 1 != 0).map(|x| x.form_id)).collect()))
    };
    let child_matches = lodforge_dyndolod::childworld::known_matches(&c.dir);
    let inputs: Vec<lodforge_dyndolod::dynplugins::WorldDynamic<'_>> = worlds
        .iter()
        .filter(|pw| !pw.dynamic.is_empty() || !pw.child_copies.is_empty() || undersides.contains(&pw.world.editor_id))
        .map(|pw| {
            let w = pw.world;
            let parent_lod_refs: Vec<u32> = pw.lod_refs.iter().copied()
                .chain(pw.dynamic.iter().map(|d| d.ref_id)).collect();
            let children = all_worlds
                .iter()
                .filter(|x| x.parent == Some(w.form_id))
                .map(|x| {
                    let (bounds, no_navmesh_cells) = child_config(c, &x.editor_id, &w.editor_id);
                    let revert_refs = lodforge_dyndolod::childworld::matched_full_lod_refs(&p.load_order, x, &parent_lod_refs, &child_matches, &c.rules);
                    lodforge_dyndolod::dynplugins::ChildWorld { world: x, revert_refs, bounds, no_navmesh_cells }
                })
                .collect();
            lodforge_dyndolod::dynplugins::WorldDynamic { world: w, dynamic: &pw.dynamic, south_west: south_west(p, &w.editor_id), lod_refs: &pw.lod_refs, children, underside: underside(w), seasonal: pw.seasonal.clone(), child_copies: pw.child_copies.clone() }
        })
        .collect();
    // WorkaroundLargeReferencesBugs (on with DLL NG) unless large references are not used.
    let workarounds = (!opts.no_large_ref_workarounds && c.num("WorkaroundLargeReferencesBugs", 1) == 1 && c.num("IgnoreLargeReferences", 0) == 0).then_some(lodforge_dyndolod::dynplugins::Workarounds { margin });
    // PBR (and other) texture swaps on the dynamic LOD base copies, as in the AltTextures.
    let cache = lodforge_dyndolod::sidefiles::ModelCache::default();
    let read = |path: &str| p.vfs.read(path).ok().flatten();
    let exists = |path: &str| p.vfs.exists(path);
    let src = lodforge_dyndolod::sidefiles::Sources { bases: &c.bases, texture_sets: &c.texture_sets, read: &read, exists: &exists, pbr: c.pbr };
    // Every linear copy a texture set points at; the build writes the ones the object LOD did not
    // need (child copies and dynamic LOD bases), or those TXSTs show a missing texture in game.
    let linear = std::sync::Mutex::new(std::collections::BTreeSet::new());
    let swaps = |b: &Base, model: &str| {
        let v = lodforge_dyndolod::sidefiles::lod_model_swaps(b, model, &cache, &src);
        for s in v.iter().filter(|s| s.diffuse.to_ascii_lowercase().ends_with("linear.dds")) {
            linear.lock().unwrap().insert(s.diffuse.to_ascii_lowercase());
        }
        v
    };
    let base_data = base_lod_data(p, c, worlds.iter().flat_map(|w| w.lod_bases.iter().copied()))?;
    let mut m = lodforge_dyndolod::dynplugins::build_worlds(&p.load_order, &inputs, &c.bases, c.num("NearGrid", 11), c.num("FarGrid", 21), &token, &rolling, workarounds, Some(&swaps)).map_err(anyhow::Error::msg)?;
    m.worlds_txt.push_str(&base_data);
    let data = out.join("SKSE").join("Plugins").join("DynDOLOD_Data");
    std::fs::create_dir_all(&data)?;
    std::fs::write(out.join("DynDOLOD.esm"), &m.esm)?;
    std::fs::write(out.join("DynDOLOD.esp"), &m.esp)?;
    std::fs::write(data.join("DynDOLOD_NG_Worlds.txt"), &m.worlds_txt)?;
    for (w, text) in &m.world_txts {
        std::fs::write(data.join(format!("DynDOLOD_NG_{w}.txt")), text)?;
    }
    if !m.seasons.is_empty() {
        std::fs::create_dir_all(out.join("Seasons"))?;
    }
    for (code, text) in &m.seasons {
        std::fs::write(out.join("Seasons").join(format!("DynDOLOD_{code}.ini")), text)?;
    }
    Ok((inputs.iter().map(|w| w.dynamic.len()).sum(), inputs.len(), linear.into_inner().unwrap()))
}

/// Occlusion stage: Occlusion.esp in `out` for every main worldspace in `worlds` (with their
/// child worldspaces), computed against the load order plus the build's DynDOLOD.esm/.esp.
/// The VFS must already see the build's terrain LOD; object height maps are read from `out`.
/// Returns (worldspace, cells) per worldspace with occlusion data.
pub fn write_occlusion(p: &Profile, c: &Config, worlds: &[String], out: &Path) -> Result<Vec<(String, usize)>> {
    let extra: Vec<PathBuf> = ["DynDOLOD.esm", "DynDOLOD.esp"].iter().map(|n| out.join(n)).filter(|q| q.is_file()).collect();
    let lo = crate::occlusion::extend_load_order(p, &extra)?;
    let all = lodforge_esp::scan_worldspaces(&lo)?;
    let read = |path: &str| p.vfs.read(path).ok().flatten();
    let params = lodforge_dyndolod::occlusion::Params { quality: c.num("OcclusionQuality", 2), radius: c.num("OcclusionRadius", 100), height: c.num("OcclusionHeight", 0.0) };
    let base_mode = c.ini.get("DynDOLOD", "OcclusionMode").map(str::to_string).unwrap_or_else(|| "-Flat +Border".into());
    let mut pb = lodforge_dyndolod::occlusion::plugin_builder();
    let mut done = Vec::new();
    for name in worlds {
        let Some(w) = all.iter().find(|w| w.editor_id.eq_ignore_ascii_case(name)) else { continue };
        // Child worldspaces that share their parent's LOD are done with the parent.
        if w.parent_flags & 0x02 != 0 && w.parent.is_some_and(|pid| all.iter().any(|x| x.form_id == pid && worlds.iter().any(|n| n.eq_ignore_ascii_case(&x.editor_id)))) {
            continue;
        }
        let hm_path = out.join("meshes").join("terrain").join(&w.editor_id).join("Objects").join(format!("{}.objects.heightmap.dat", w.editor_id));
        let heightmap = std::fs::read(&hm_path).ok();
        // The main worldspace's `OcclusionMode_<World>` (else `OcclusionMode`); its child
        // worldspaces are computed in the same pass with the same mode minus `+Border`.
        let main_mode = c.ini.get("DynDOLOD", &format!("OcclusionMode_{}", w.editor_id)).map(str::to_string).unwrap_or_else(|| base_mode.clone());
        let mode = |child: &str, is_main: bool| -> Option<String> {
            if c.ini.get("DynDOLOD", &format!("Occlusion_{child}")).is_some_and(|v| v.trim() == "0") {
                return None;
            }
            Some(if is_main { main_mode.clone() } else { lodforge_dyndolod::occlusion::Mode::parse(&main_mode).without_border().text })
        };
        done.extend(crate::occlusion::build_worlds(&lo, &read, &w.editor_id, heightmap.as_deref(), params, &mode, &mut pb)?);
    }
    if !pb.worlds.is_empty() {
        std::fs::write(out.join("Occlusion.esp"), pb.write(&lo))?;
    }
    Ok(done)
}

/// Whether DynDOLOD would treat the profile's grass as complex (HD) grass: TexGen offers only
/// HD grass billboards when Community Shaders is installed.
pub fn complex_grass(p: &Profile) -> bool {
    p.vfs.exists("skse\\plugins\\communityshaders.dll")
}

/// Grass export keys DynDOLOD writes (the grass golden's header, docs/re/object-lod-spec.md 6.2):
/// the brightness pairs from `DynDOLOD_SSE.ini`, the placement-brightness switches,
/// `GrassLargeReference`, `RemoveUnseenGrass` and `GrassGID`; `GrassMap` / `PathGrass` /
/// `GrassDensity` are passed to the builder directly. One `Key=Value\r\n` per line.
pub fn grass_header(c: &Config) -> String {
    let f = lodforge_dyndolod::export::float_to_str;
    let num = |k: &str, d: f64| f(c.num(k, d));
    let flag = |k: &str, d: bool| if c.num::<i32>(k, i32::from(d)) != 0 { "True" } else { "False" };
    let mut out = String::new();
    for (k, v) in [
        ("IgnoreGrassPlacementBrightness", flag("IgnoreGrassPlacementBrightness", false).to_string()),
        ("IgnoreComplexGrassPlacementBrightness", flag("IgnoreComplexGrassPlacementBrightness", false).to_string()),
        ("GrassBrightnessTopR", num("GrassBrightnessTopR", 0.4)),
        ("GrassBrightnessTopG", num("GrassBrightnessTopG", 0.445)),
        ("GrassBrightnessTopB", num("GrassBrightnessTopB", 0.45)),
        ("GrassBrightnessBottomR", num("GrassBrightnessBottomR", 0.2)),
        ("GrassBrightnessBottomG", num("GrassBrightnessBottomG", 0.223)),
        ("GrassBrightnessBottomB", num("GrassBrightnessBottomB", 0.225)),
        ("ComplexGrassBrightnessTopR", num("ComplexGrassBrightnessTopR", 0.7)),
        ("ComplexGrassBrightnessTopG", num("ComplexGrassBrightnessTopG", 0.75)),
        ("ComplexGrassBrightnessTopB", num("ComplexGrassBrightnessTopB", 0.75)),
        ("ComplexGrassBrightnessBottomR", num("ComplexGrassBrightnessBottomR", 0.7)),
        ("ComplexGrassBrightnessBottomG", num("ComplexGrassBrightnessBottomG", 0.75)),
        ("ComplexGrassBrightnessBottomB", num("ComplexGrassBrightnessBottomB", 0.75)),
        ("GrassLargeReference", flag("GrassLargeReference", false).to_string()),
        ("RemoveUnseenGrass", flag("RemoveUnseenGrass", false).to_string()),
        ("GrassGID", { let g = c.cfg("GrassGID"); if g.is_empty() { "cgid".to_string() } else { g } }),
    ] {
        out.push_str(&format!("{k}={v}\r\n"));
    }
    out
}

/// Rewrites grass FlatTextures lines (as `grass-billboards` writes them) the way DynDOLOD writes
/// them for complex grass: the `ComplexGrassBillboard` template (`GrassBillboard<N>`, e.g.
/// `DynDOLOD_flat_4x2alt1_lod.nif`, 4 planes) with the flat normals file, black glow and back
/// textures, vertex colour `FlatLODWithNormalMapVertexColor` (0.5; complex grass billboards have
/// normal maps) and colour variance 0.1 (the values in DynDOLOD's E2E export).
/// Normal grass keeps its lines (`GrassBillboard=1` selects the built-in two-plane billboard the golden
/// files show).
pub fn complex_grass_flat(c: &Config, flat: &str) -> String {
    // The template of `ComplexGrassBillboard` (index into `GrassBillboard1..6`), the flat normals
    // file, the with-normal-map vertex colour and the colour variance, as the grass golden's
    // lines have them (spec 6.2 / 12.4).
    let n = c.num::<usize>("ComplexGrassBillboard", 4).clamp(1, 6);
    let template = c.cfg(&format!("GrassBillboard{n}"));
    let template = if template.is_empty() || template.eq_ignore_ascii_case("internal") { "DynDOLOD_flat_4x2alt1_lod.nif".to_string() } else { template };
    let lower = |p: &std::path::Path| p.display().to_string().replace('/', "\\").to_ascii_lowercase();
    let template = lower(&c.dir.join(template));
    let normals = lower(&lodforge_dyndolod::overlay::path(&c.dir, r"Configs\DynDOLOD_SSE_flat_normals.txt"));
    let f = lodforge_dyndolod::export::float_to_str;
    let vc = f(c.num("FlatLODWithNormalMapVertexColor", 0.5));
    let variance = f(c.num("FlatLODColorVariance", 0.1));
    let mut out = String::new();
    for line in flat.lines() {
        let line = line.trim_end_matches('\r');
        if line.trim().is_empty() {
            continue;
        }
        let mut c: Vec<String> = line.split('\t').map(str::to_string).collect();
        c.resize(15, String::new());
        c[6] = format!("{template};{normals};{normals};{normals}");
        c[7] = "textures\\black.dds".to_string();
        c[8] = vc.clone();
        c[9] = variance.clone();
        c[14] = "textures\\black.dds".to_string();
        out.push_str(&c.join("\t"));
        out.push_str("\r\n");
    }
    out
}

/// Generated 3D tree LOD models (see `Opts::tree_models`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, clap::ValueEnum)]
pub enum TreeModelsMode {
    #[default]
    Off,
    Generate,
    All,
}

/// Where trees get their LOD (see `Opts::tree_lod`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, clap::ValueEnum)]
pub enum TreeLodMode {
    #[default]
    Classic,
    Ultra,
}

/// Grass LOD flavour.
#[derive(Clone, Copy, PartialEq, Eq, Debug, clap::ValueEnum)]
pub enum GrassMode {
    /// No grass LOD.
    Off,
    /// DynDOLOD's classic grass LOD: grass billboards in the LOD4 object LOD (`--mode dyndolod`).
    Bto,
    /// Grass LOD meshes for Community Shaders' Grass Optimizations (`meshes\LOD\Grass\*_lod1.nif`,
    /// crossed planes per grass model). Needs no grass placements, so no cache.
    Go,
}

/// Where `--grass bto` takes grass placements from. Only the NGIO grass cache is supported; the
/// option is kept so existing command lines keep working.
#[derive(Clone, Copy, PartialEq, Eq, Debug, clap::ValueEnum)]
pub enum GrassSource {
    /// The NGIO grass cache (`Grass\*.cgid` in the profile). Cells without a cache file get no
    /// grass.
    Auto,
    /// Same as `auto`.
    Cache,
}

/// What [`prepare_grass`] made for one worldspace.
pub struct GrassInputs {
    /// Folder with one `.cgid` per cell (links to the cache files).
    pub dir: PathBuf,
    /// LODGen `GrassMap` and FlatTextures lines for the grass billboards.
    pub map: PathBuf,
    pub flat: PathBuf,
    /// Cache files (all seasons) used, those whose form ids were rewritten for the current load
    /// order.
    pub cached: usize,
    pub remapped: usize,
    /// Every billboard file written into the output (Data-relative, `textures\terrain\lodgen\
    /// <plugin>\<stem>[_1|_2][_n].dds` and the `.txt` sidecars), for the VFS and the atlas.
    pub files: Vec<String>,
}

/// Every file under `root`, as Data-relative paths with backslashes, sorted.
fn relative_files(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let path = e.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(rel) = path.strip_prefix(root) {
                out.push(rel.to_string_lossy().replace('/', "\\"));
            }
        }
    }
    out.sort();
    out
}

/// The diffuse textures grass LOD shapes end up with: every billboard view the grass stage wrote
/// (`<stem>_1.dds` and `_2.dds` of complex grass, `<stem>.dds` otherwise), without the normal
/// maps and `.txt` sidecars. These go on the object atlas like the tree billboards (DynDOLOD's
/// `_Alpha` page); LODGen then re-UVs the grass quads onto their tiles.
pub fn grass_billboard_views(files: &[String]) -> Vec<String> {
    files
        .iter()
        .filter(|f| {
            let l = f.to_ascii_lowercase();
            l.ends_with(".dds") && !l.ends_with("_n.dds")
        })
        .cloned()
        .collect()
}

/// Collects the grass placements of `world` for `--grass bto` into `<work>\grass\<World>`
/// (NGIO cache files hard-linked; cells without one get no grass) and renders the grass billboards into
/// `out` with the grass map next to them in the work folder. With complex grass the FlatTextures
/// lines use DynDOLOD's complex grass template ([`complex_grass_flat`]).
pub fn prepare_grass(p: &Profile, c: &Config, prof: &crate::profile::ProfileArgs, world: &Worldspace, work: &Path, out: &Path) -> Result<GrassInputs> {
    let w = &world.editor_id;
    let dir = work.join("grass").join(w);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    use rayon::prelude::*;
    let src = crate::grass_types::grass_types(p)?;
    let prefix = format!("grass\\{}x", w.to_ascii_lowercase());
    let (mut cached, mut remapped) = (0, 0);
    let current: HashMap<(String, u32), u32> = src.gras.iter().map(|g| ((lodforge_grass::cgid::strip_meshes(&g.model).to_ascii_lowercase(), object_id(g.form_id)), g.form_id)).collect();
    let files: Vec<(String, PathBuf)> = p.vfs.list_prefix(&prefix).into_iter().filter_map(|rel| Some((rel.strip_prefix("grass\\")?.to_string(), p.vfs.loose_path(&rel)?.to_path_buf()))).collect();
    let done: Vec<Result<bool>> = files
        .par_iter()
        .map(|(name, from)| {
            let dst = dir.join(name);
            let bytes = std::fs::read(from).with_context(|| format!("reading {}", from.display()))?;
            match remap_cache_form_ids(&bytes, &current) {
                Some(fixed) => {
                    std::fs::write(&dst, fixed)?;
                    Ok(true)
                }
                None => {
                    if std::fs::hard_link(from, &dst).is_err() {
                        std::fs::write(&dst, &bytes)?;
                    }
                    Ok(false)
                }
            }
        })
        .collect();
    for d in done {
        remapped += usize::from(d?);
        cached += 1;
    }
    // Cells with LAND but no cache file are skipped (nothing is generated).
    let have: HashSet<(i32, i32)> = world.lands.values().map(|l| l.cell).collect();
    let missing = have.iter().filter(|c| !dir.join(lodforge_grass::file_name(w, c.0, c.1)).exists()).count();
    if missing > 0 {
        eprintln!("warning: {w}: {missing} of {} cells have no NGIO grass cache file and get no grass (cells without grass never get one; precache the worldspace in game with NGIO to fill the rest)", have.len());
    }
    // Billboards (TexGen's grass rule) and the grass map, rendered into a scratch Data folder and
    // copied into the output.
    let bb = work.join("grass-billboards").join(w);
    if bb.exists() {
        std::fs::remove_dir_all(&bb)?;
    }
    let bb_data = bb.join("Data");
    let args = crate::texgen_billboards::GrassArgs { profile: prof.clone(), grass: dir.clone(), world: w.clone(), out: bb_data.clone(), all_types: false, hd: complex_grass(p) };
    crate::texgen_billboards::run_grass(&args)?;
    copy_tree(&bb_data, out)?;
    let files = relative_files(&bb_data);
    if complex_grass(p) {
        let flat = bb.join("grass_flat.txt");
        std::fs::write(&flat, complex_grass_flat(c, &std::fs::read_to_string(&flat).unwrap_or_default()))?;
    }
    Ok(GrassInputs { dir, map: bb.join("grass_map.txt"), flat: bb.join("grass_flat.txt"), cached, remapped, files })
}

/// Object id of a form id without its load-order slot (light plugins keep the low 12 bits).
fn object_id(id: u32) -> u32 {
    if id >> 24 == 0xFE { id & 0xFFF } else { id & 0x00FF_FFFF }
}

/// Rewrites the GRAS form ids of a `.cgid` for the current load order. NGIO stores load-order
/// form ids, so a cache made with other plugins active (e.g. before DynDOLOD.esm was added) names
/// the wrong grass. Each type is matched by model and object id. Returns `None` when nothing
/// changes (or the file does not parse); trailing bytes are kept.
pub fn remap_cache_form_ids(d: &[u8], current: &HashMap<(String, u32), u32>) -> Option<Vec<u8>> {
    let mut out = d.to_vec();
    let mut o = 0usize;
    let u32_at = |o: &mut usize| -> Option<u32> {
        let v = u32::from_le_bytes(d.get(*o..*o + 4)?.try_into().ok()?);
        *o += 4;
        Some(v)
    };
    let mut changed = false;
    let n = u32_at(&mut o)?;
    for _ in 0..n {
        let len = u32_at(&mut o)? as usize;
        let name = d.get(o..o + len)?;
        o += len;
        let model = String::from_utf8_lossy(&name[..len.saturating_sub(1)]).to_ascii_lowercase();
        o += 4; // wave period
        let at = o;
        let form_id = u32_at(&mut o)?;
        if let Some(&now) = current.get(&(model, object_id(form_id)))
            && now != form_id
        {
            out[at..at + 4].copy_from_slice(&now.to_le_bytes());
            changed = true;
        }
        o += 3;
        let groups = u32_at(&mut o)?;
        for _ in 0..groups {
            o += 24 + 4;
            let count = u32_at(&mut o)? as usize;
            let shorts = u32_at(&mut o)? as usize;
            o += count * shorts * 2;
        }
        if o > d.len() {
            return None;
        }
    }
    changed.then_some(out)
}

/// `[LOD]` billboard sidecar values LODGen reads (`Utils.GetBillboardDimensions`).
#[derive(Debug, Clone, Copy, Default)]
pub struct BillboardDims {
    pub width: f32,
    pub depth: f32,
    pub height: f32,
    pub shift: [f32; 3],
    pub scale: f32,
}

impl BillboardDims {
    pub fn parse(txt: &str) -> BillboardDims {
        let mut d = BillboardDims { scale: 1.0, ..Default::default() };
        for line in txt.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let v: String = v.chars().filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-').collect();
            let Ok(v) = v.parse::<f32>() else { continue };
            match k.trim().to_ascii_uppercase().as_str() {
                "WIDTH" => d.width = v,
                "DEPTH" => d.depth = v,
                "HEIGHT" => d.height = v,
                "SHIFTX" => d.shift[0] = v,
                "SHIFTY" => d.shift[1] = v,
                "SHIFTZ" => d.shift[2] = v,
                "SCALE" => d.scale = v,
                _ => {}
            }
        }
        if d.depth == 0.0 {
            d.depth = d.width;
        }
        d
    }
}

/// A Grass Optimizations far-LOD grass mesh (`_LOD1`): two crossed vertical quads textured with
/// the grass billboard, laid out like the built-in billboards in the golden files (quad 0 along X at `ShiftY`,
/// quad 1 rotated -90 degrees at `ShiftX`, bottom at `ShiftZ`; u = 0 at +X, v = 0 at the top),
/// normals straight up like DynDOLOD's fixed-normal grass billboards, alpha tested, two-sided,
/// under a BSFadeNode.
///
/// Every size and shift is multiplied by the sidecar's `Scale`. File layout (written from the
/// requirements of Community Shaders' Grass Optimizations, `GrassMeshLibrary::LoadLODMesh` /
/// `FindLODShape`, and the RunGrass vertex input; see object-lod-spec 11.9):
///
/// ```text
/// BSFadeNode "<texture stem>"          identity transform
/// â””â”€ BSTriShape "GrassLOD1"            identity transform, 8 vertices / 4 triangles,
///    â”‚                                 position + UV + normal + colour (white, alpha 255)
///    â”œâ”€ BSLightingShaderProperty       type 0, flags1 ZBuffer_Test, flags2 ZBuffer_Write |
///    â”‚  â””â”€ BSShaderTextureSet          Double_Sided | Vertex_Colors; slot 0 `texture`, slot 1 `normal`
///    â””â”€ NiAlphaProperty                alpha test (> 128), no blending
/// ```
///
/// Grass Optimizations accepts a LOD file only with exactly one BSTriShape and identity
/// transforms on every object on the way to it, and draws it with the source grass shape's
/// material (the LOD file's shader and textures are not used there; they matter to other
/// readers of the file). The grass vertex shader reads POSITION, TEXCOORD0, NORMAL and COLOR,
/// with colour alpha both scaling the texture alpha and weighting the wind, so the shape carries
/// opaque white vertex colours.
pub fn grass_lod1_nif(texture: &str, normal: Option<&str>, d: &BillboardDims) -> Result<Vec<u8>> {
    use lodforge_nif::{AvObject, Block, LightingShader, Nif, Node, ObjectNet, TriShape, VertexDesc};

    let s = if d.scale > 0.0 { d.scale } else { 1.0 };
    let (hw, hd, h) = (d.width * s / 2.0, d.depth * s / 2.0, d.height * s);
    let (sx, sy, z0) = (d.shift[0] * s, d.shift[1] * s, d.shift[2] * s);
    let z1 = z0 + h;
    anyhow::ensure!(hw > 0.0 && hd > 0.0 && h > 0.0, "grass LOD1 billboard without a size ({d:?})");
    // Quad 0 in the XZ plane at y = ShiftY, u running from +X to -X; quad 1 is quad 0 turned
    // -90 degrees about Z ((x, y) -> (y, -x)) at x = ShiftX, so its u runs from -Y to +Y.
    // Per quad: top u0, top u1, bottom u1, bottom u0.
    let positions = vec![
        [hw, sy, z1],
        [-hw, sy, z1],
        [-hw, sy, z0],
        [hw, sy, z0],
        [sx, -hd, z1],
        [sx, hd, z1],
        [sx, hd, z0],
        [sx, -hd, z0],
    ];
    let uvs = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]].repeat(2);
    let triangles = vec![[0, 1, 2], [0, 2, 3], [4, 5, 6], [4, 6, 7]];
    let (lo, hi) = positions.iter().fold(([f32::MAX; 3], [f32::MIN; 3]), |(lo, hi), p| {
        (std::array::from_fn(|k| lo[k].min(p[k])), std::array::from_fn(|k| hi[k].max(p[k])))
    });
    let center: [f32; 3] = std::array::from_fn(|k| (lo[k] + hi[k]) / 2.0);
    let radius = positions.iter().map(|p| (0..3).map(|k| (p[k] - center[k]).powi(2)).sum::<f32>().sqrt()).fold(0.0, f32::max);

    let file = texture.rsplit(['\\', '/']).next().unwrap_or(texture);
    let stem = file.rsplit_once('.').map_or(file, |(a, _)| a);
    let mut nif = Nif::new();
    let root = nif.add_block(Block::Node(Node { av: AvObject { net: ObjectNet::named(stem), ..Default::default() }, ..Default::default() }));
    let shape = TriShape {
        av: AvObject { net: ObjectNet::named("GrassLOD1"), ..Default::default() },
        bound_center: center,
        bound_radius: radius,
        shader_property: Some(root + 2),
        alpha_property: Some(root + 4),
        vertex_desc: VertexDesc::with_attributes(true, true, true, false, true).0,
        normals: vec![[0.0, 0.0, 1.0]; 8],
        colors: vec![[255; 4]; 8],
        positions,
        uvs,
        triangles,
        ..Default::default()
    };
    let shape = nif.add_block(Block::TriShape(shape));
    nif.add_block(Block::LightingShader(LightingShader {
        flags1: 0x8000_0000,
        flags2: 0x31,
        texture_set: Some(root + 3),
        emissive_multiple: 0.0,
        texture_clamp_mode: 0,
        specular_color: [0.0; 3],
        lighting_effect_1: 0.0,
        lighting_effect_2: 0.0,
        ..Default::default()
    }));
    let mut slots = vec![String::new(); 9];
    slots[0] = texture.to_string();
    slots[1] = normal.unwrap_or_default().to_string();
    nif.add_block(Block::TextureSet(slots));
    nif.add_block(Block::Alpha { net: ObjectNet::default(), flags: 0x12EC, threshold: 128 });
    if let Some(Block::Node(n)) = nif.blocks.get_mut(root as usize) {
        n.children.push(Some(shape));
    }
    // The reader's Node carries no type of its own, and the writer takes the header's type
    // table only when it covers every block: name all five, one block each.
    nif.header.block_types = ["BSFadeNode", "BSTriShape", "BSLightingShaderProperty", "BSShaderTextureSet", "NiAlphaProperty"].map(String::from).to_vec();
    nif.header.block_type_index = (0..nif.blocks.len() as u16).collect();
    Ok(nif.to_bytes()?)
}

/// `--grass go`: a `meshes\LOD\Grass\<model>_lod1.nif` for every grass model a landscape texture
/// uses and TexGen would make a billboard for, unless the load order already has one (authored
/// LODs win). Placement-free, so it needs no grass cache. Billboards go into `out` next to the
/// meshes. Returns (written, already present).
pub fn write_grass_lod1(p: &Profile, prof: &crate::profile::ProfileArgs, world: &Worldspace, work: &Path, out: &Path) -> Result<(usize, usize)> {
    let src = crate::grass_types::grass_types(p)?;
    // One synthetic cache cell holding every used grass type: the billboard stage discovers
    // types from cache files.
    let dir = work.join("go-types");
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    std::fs::create_dir_all(&dir)?;
    let mut used: Vec<usize> = src.used.iter().copied().filter(|&i| src.model_ok[i]).collect();
    used.sort_unstable();
    let types: Vec<lodforge_grass::cgid::TypeCache> = used
        .iter()
        .map(|&i| {
            let mut t = lodforge_grass::cgid::TypeCache::new(&src.gras[i]);
            t.groups.push(lodforge_grass::cgid::Group::new(vec![[0u16; 16]], (0.0, 0.0)));
            t
        })
        .collect();
    std::fs::write(dir.join(lodforge_grass::file_name(&world.editor_id, 0, 0)), lodforge_grass::cgid::CellCache { types }.to_bytes())?;
    let bb = work.join("go-billboards");
    let bb_data = bb.join("Data");
    let args = crate::texgen_billboards::GrassArgs { profile: prof.clone(), grass: dir, world: world.editor_id.clone(), out: bb_data.clone(), all_types: false, hd: false };
    crate::texgen_billboards::run_grass(&args)?;
    copy_tree(&bb_data, out)?;
    // Grass map: `MODELSTEM_FORMID8.DDS<TAB>billboard`.
    let map = std::fs::read_to_string(bb.join("grass_map.txt")).unwrap_or_default();
    let by_key: HashMap<String, String> = map.lines().filter_map(|l| l.split_once('\t')).map(|(k, v)| (k.to_ascii_uppercase(), v.to_string())).collect();
    let (mut written, mut present) = (0, 0);
    let mut done: HashSet<String> = HashSet::new();
    for &i in &used {
        let g = &src.gras[i];
        let model = lodforge_grass::cgid::strip_meshes(&g.model).replace('/', "\\");
        let file = model.rsplit('\\').next().unwrap_or(&model).to_string();
        let stem = file.rsplit_once('.').map_or(file.as_str(), |s| s.0).to_string();
        let key = format!("{}_{:08X}.DDS", stem.to_ascii_uppercase(), object_id(g.form_id));
        let Some(texture) = by_key.get(&key) else { continue };
        let rel = format!("meshes\\lod\\grass\\{}_lod1.nif", stem.to_ascii_lowercase());
        if !done.insert(rel.clone()) {
            continue;
        }
        if p.vfs.exists(&rel) {
            present += 1;
            continue;
        }
        let txt = std::fs::read_to_string(bb_data.join(texture.replace(".dds", ".txt"))).unwrap_or_default();
        let dims = BillboardDims::parse(&txt);
        if dims.width <= 0.0 || dims.height <= 0.0 {
            continue;
        }
        let normal = texture.replace(".dds", "_n.dds");
        let has_normal = bb_data.join(&normal).is_file();
        let bytes = grass_lod1_nif(texture, has_normal.then_some(normal.as_str()), &dims)?;
        let path = out.join(&rel);
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(&path, bytes)?;
        written += 1;
    }
    Ok((written, present))
}

/// Copies every file under `from` to the same relative path under `to`.
pub fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    if !from.is_dir() {
        return Ok(());
    }
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let target = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            std::fs::create_dir_all(&target)?;
            copy_tree(&e.path(), &target)?;
        } else {
            std::fs::copy(e.path(), &target).with_context(|| format!("copying {}", e.path().display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {

    #[test]
    fn atlas_tile_sizes() {
        // DynDOLOD 3 Tamriel: full textures to 256, LOD textures and billboards up to 1024.
        assert!(!is_lod_texture("textures\\architecture\\highhrothgar\\hhcol01.dds"));
        assert!(is_lod_texture("textures\\lod\\falmerhutlod.dds"));
        assert!(is_lod_texture("textures\\terrain\\lodgen\\skyrim.esm\\reach_tree_rock01_000b8f53.dds"));
        assert!(is_lod_texture("textures\\dyndolod\\lod\\trees\\reachtree02_trunk_1.dds"));
        assert_eq!(tile_size(2048, 2048, 256), (256, 256));
        assert_eq!(tile_size(2048, 4096, 256), (128, 256));
        assert_eq!(tile_size(508, 512, 1024), (508, 512));
        assert_eq!(atlas_key_diffuse("Textures\\A.dds,textures\\a_n.dds=4"), "textures\\a.dds");
    }

    #[test]
    fn square_pages() {
        // 12 tiles of 512: 3 MB of area -> a 2048 bin, cropped to 2048x1536 -> 2048x2048.
        let tiles: Vec<(String, u32, u32)> = (0..12).map(|i| (format!("t{i:02}"), 512, 512)).collect();
        let l = square_layout(&tiles, 8192, "x");
        assert_eq!(l.pages.len(), 1);
        assert_eq!((l.pages[0].width, l.pages[0].height), (2048, 2048));
        // Too much for one page at the cap: spills.
        let l = square_layout(&tiles, 1024, "x");
        assert_eq!(l.pages.len(), 3);
        assert_eq!(l.pages[1].name, "x01.dds");
    }

    use super::*;

    #[test]
    fn tree_brightness_keys() {
        let ini = Ini::parse("[DynDOLOD]\nCrownBrightness=0.9\nCrownBrightnessG=0.8\nCrownBrightness_WIN=0.7\nCrownBrightnessB_WIN=0.6\nTrunkBrightnessR=0.5\n");
        let [crown, trunk, flat] = tree_brightness(&ini, None);
        assert_eq!(crown, [0.9, 0.8, 0.9]);
        assert_eq!(trunk, [0.5, 1.0, 1.0]);
        assert_eq!(flat, [1.0; 3]);
        // Season: channel+season, then season, then the plain keys.
        assert_eq!(tree_brightness(&ini, Some(Season::Winter))[0], [0.7, 0.7, 0.6]);
        assert_eq!(tree_brightness(&ini, Some(Season::Summer))[0], [0.9, 0.8, 0.9]);
    }

    #[test]
    fn grass_views_for_the_atlas() {
        let files: Vec<String> = [
            "textures\\terrain\\lodgen\\seasonal landscapes.esp\\fern_brown_00000816.txt",
            "textures\\terrain\\lodgen\\seasonal landscapes.esp\\fern_brown_00000816_1.dds",
            "textures\\terrain\\lodgen\\seasonal landscapes.esp\\fern_brown_00000816_1_n.dds",
            "textures\\terrain\\lodgen\\seasonal landscapes.esp\\fern_brown_00000816_2.dds",
            "textures\\terrain\\lodgen\\seasonal landscapes.esp\\fern_brown_00000816_2_N.DDS",
            "textures\\terrain\\lodgen\\skyrim.esm\\fieldgrass01_000135be.dds",
            "textures\\terrain\\lodgen\\skyrim.esm\\fieldgrass01_000135be.txt",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(
            grass_billboard_views(&files),
            [
                "textures\\terrain\\lodgen\\seasonal landscapes.esp\\fern_brown_00000816_1.dds",
                "textures\\terrain\\lodgen\\seasonal landscapes.esp\\fern_brown_00000816_2.dds",
                "textures\\terrain\\lodgen\\skyrim.esm\\fieldgrass01_000135be.dds",
            ]
        );
        assert!(grass_billboard_views(&[]).is_empty());
    }

    #[test]
    fn ignore_masks() {
        let m: Vec<String> = ["mountainslab01.dds", "dyndolod_atlas", "treelod*.dds"].iter().map(|s| s.to_string()).collect();
        assert!(ignored_texture("textures\\landscape\\mountains\\MountainSlab01.dds", &m));
        assert!(ignored_texture("textures\\lod\\x_dyndolod_atlas.dds", &m));
        assert!(ignored_texture("textures\\terrain\\tamriel\\trees\\treelod02.dds", &m));
        assert!(!ignored_texture("textures\\lod\\farmhouselod01.dds", &m));
    }

    #[test]
    fn grass_lod1_mesh() {
        use lodforge_nif::{Block, VertexDesc, va};
        let d = BillboardDims::parse("[LOD]\r\nWidth=40.0\r\nDepth=30.0\r\nHeight=50.0\r\nShiftX=2.0\r\nShiftY=-3.0\r\nShiftZ=-1.5\r\nScale=2.0\r\n");
        let tex = "textures\\terrain\\lodgen\\skyrim.esm\\fieldgrass01_0000abcd.dds";
        let nrm = "textures\\terrain\\lodgen\\skyrim.esm\\fieldgrass01_0000abcd_n.dds";
        let nif = lodforge_nif::parse(&grass_lod1_nif(tex, Some(nrm), &d).unwrap()).unwrap();
        assert_eq!(nif.block_type(0), "BSFadeNode");
        assert_eq!(nif.roots, vec![Some(0)]);
        let Block::Node(root) = &nif.blocks[0] else { panic!("root") };
        assert_eq!(root.av.net.name.as_deref(), Some("fieldgrass01_0000abcd"));
        assert_eq!(root.children.len(), 1);
        // Grass Optimizations: exactly one BSTriShape, identity transforms.
        let shapes: Vec<_> = nif.blocks.iter().filter_map(|b| if let Block::TriShape(s) = b { Some(s) } else { None }).collect();
        assert_eq!(shapes.len(), 1);
        let s = shapes[0];
        assert_eq!(nif.block_type(root.children[0].unwrap() as usize), "BSTriShape");
        assert_eq!(root.av.transform, lodforge_nif::Transform::default());
        assert_eq!(s.av.transform, lodforge_nif::Transform::default());
        assert_eq!((s.positions.len(), s.triangles.len()), (8, 4));
        let desc = VertexDesc(s.vertex_desc);
        for a in [va::VERTEX, va::UVS, va::NORMALS, va::COLORS] {
            assert!(desc.has(a), "{a:#x}");
        }
        assert_eq!(nif.shape_textures(s)[..2], [tex.to_string(), nrm.to_string()]);
        assert!(s.normals.iter().all(|n| n[2] > 0.99 && n[0].abs() < 0.01 && n[1].abs() < 0.01), "{:?}", s.normals);
        assert!(s.colors.iter().all(|c| *c == [255; 4]));
        for uv in &s.uvs {
            assert!((0.0..=1.0).contains(&uv[0]) && (0.0..=1.0).contains(&uv[1]));
        }
        let (umin, umax) = s.uvs.iter().fold((1f32, 0f32), |(a, b), uv| (a.min(uv[0]), b.max(uv[0])));
        assert_eq!((umin, umax), (0.0, 1.0));
        // Scaled sizes: bottom at ShiftZ x Scale, top Height x Scale above it.
        let zmin = s.positions.iter().map(|p| p[2]).fold(f32::MAX, f32::min);
        let zmax = s.positions.iter().map(|p| p[2]).fold(f32::MIN, f32::max);
        assert!((zmin - -3.0).abs() < 1e-4 && (zmax - 97.0).abs() < 1e-4, "{zmin} {zmax}");
        // v = 0 at the top, u = 0 at +X (quad 0, y = ShiftY) / -Y (quad 1, x = ShiftX).
        for (p, uv) in s.positions.iter().zip(&s.uvs) {
            assert_eq!(uv[1] == 0.0, p[2] > 0.0);
        }
        for (i, (p, uv)) in s.positions.iter().zip(&s.uvs).enumerate() {
            if i < 4 {
                assert!((p[1] - -6.0).abs() < 1e-4 && (p[0].abs() - 40.0).abs() < 1e-4);
                assert_eq!(uv[0] == 0.0, p[0] > 0.0);
            } else {
                assert!((p[0] - 4.0).abs() < 1e-4 && (p[1].abs() - 30.0).abs() < 1e-4);
                assert_eq!(uv[0] == 0.0, p[1] < 0.0);
            }
        }
        let Some(Block::LightingShader(sh)) = nif.get(s.shader_property) else { panic!("shader") };
        assert_eq!((sh.shader_type, sh.flags2 & 0x10, sh.flags2 & 0x20), (0, 0x10, 0x20));
        let Some(Block::Alpha { flags, threshold, .. }) = nif.get(s.alpha_property) else { panic!("alpha") };
        assert_eq!((flags & 0x201, *threshold), (0x200, 128));
        // Without a normal map slot 1 stays empty.
        let nif = lodforge_nif::parse(&grass_lod1_nif(tex, None, &d).unwrap()).unwrap();
        let s = nif.blocks.iter().find_map(|b| if let Block::TriShape(s) = b { Some(s) } else { None }).unwrap();
        assert_eq!(nif.shape_textures(s)[1], "");
    }
}
