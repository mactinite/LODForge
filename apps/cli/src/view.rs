//! `lodforge view`: inspect generated LOD as the game arranges it. Opens the viewer window, or
//! renders a screenshot / runs the checks headlessly for automation.

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};

use lodforge_viewer::camera::FreeCam;
use lodforge_viewer::checks::Tolerances;
use lodforge_viewer::files::{DirFiles, Files, Layered};
use lodforge_viewer::lodselect::LodMode;
use lodforge_viewer::render::Shading;
use lodforge_viewer::scene::LoadOptions;
use lodforge_viewer::viewstate::{Layers, ViewSettings};
use lodforge_viewer::{app, world};

#[derive(clap::Args)]
pub struct Args {
    /// The generated output: a Data-style folder (the `--out` of a build, or an MO2 mod folder).
    /// With --instance it defaults to the profile's generated LOD mod (the highest-priority
    /// enabled mod LODForge or DynDOLOD wrote).
    #[arg(long, required_unless_present = "instance")]
    pub out: Option<PathBuf>,
    /// Worldspace (as spelled in the file names); default: Tamriel when present, else the first.
    #[arg(long)]
    pub world: Option<String>,
    /// Season code (SPR, SUM, AUT, WIN); default: the default season's files.
    #[arg(long)]
    pub season: Option<String>,
    /// MO2 instance: textures and models the output refers to but does not carry, and the
    /// lodsettings grid, resolve through the profile's load order (as in game).
    #[arg(long)]
    pub instance: Option<PathBuf>,
    #[arg(long, default_value = "Default")]
    pub profile: String,
    /// Game folder (with --instance); defaults to `<instance>\Stock Game`.
    #[arg(long)]
    pub game: Option<PathBuf>,
    /// With --instance: also show the load order's own LOD files for blocks the output lacks (what
    /// the game would display), instead of the output alone.
    #[arg(long)]
    pub merge_load_order: bool,
    /// Run the checks without a window and print a summary; with --json, write the report.
    #[arg(long)]
    pub check: bool,
    /// Report file for --check.
    #[arg(long)]
    pub json: Option<PathBuf>,
    /// Render one PNG without a window and exit.
    #[arg(long)]
    pub screenshot: Option<PathBuf>,
    /// Camera for --screenshot (and the window's start): `x,y,z[,yaw,pitch]`; yaw in degrees
    /// clockwise from north, pitch up. Without it the screenshot is a top-down map.
    #[arg(long, allow_hyphen_values = true)]
    pub camera: Option<String>,
    /// Horizontal field of view (degrees), as the game's `fov` console command takes it.
    #[arg(long, default_value_t = 70.0)]
    pub fov: f32,
    /// Centre of the game-mode LOD arrangement `x,y` for --screenshot (default: the camera). The
    /// game picks LOD levels around the player, so a free-camera capture whose player stands
    /// elsewhere needs the player's position here.
    #[arg(long, allow_hyphen_values = true)]
    pub lod_center: Option<String>,
    /// Screenshot size `WxH`.
    #[arg(long, default_value = "1920x1080")]
    pub size: String,
    /// Levels to show: `game` (the game's distance-based arrangement) or `4`, `8`, `16`, `32`.
    #[arg(long, default_value = "game")]
    pub levels: String,
    /// Multiplier on the game's LOD switch distances in `game` mode.
    #[arg(long, default_value_t = 1.0)]
    pub lod_distance: f32,
    /// Layers: `all` or a comma list of terrain, water, objects, tree_billboards, grass, trees,
    /// large_refs.
    #[arg(long, default_value = "all")]
    pub layers: String,
    /// Shading: textured, unlit, normals, flat, level, kind.
    #[arg(long, default_value = "textured")]
    pub shading: String,
    /// VRAM for terrain textures (MB); top mips are dropped per level to fit.
    #[arg(long, default_value_t = 3072)]
    pub texture_budget_mb: u64,
    /// Skip normal maps.
    #[arg(long)]
    pub no_normals: bool,
    /// Load only these levels (comma list), e.g. `4` to save time and memory.
    #[arg(long, value_delimiter = ',')]
    pub load_levels: Vec<i32>,
    /// MSAA samples for --screenshot.
    #[arg(long, default_value_t = 4)]
    pub samples: u32,
}

/// `Files` over the profile's VFS.
struct VfsFiles(lodforge_archive::Vfs);

impl Files for VfsFiles {
    fn read(&self, rel: &str) -> Option<Vec<u8>> {
        self.0.read(rel).ok().flatten()
    }
    fn exists(&self, rel: &str) -> bool {
        self.0.exists(rel)
    }
    fn origin(&self, rel: &str) -> Option<String> {
        self.0.origin(rel).map(|o| match o {
            lodforge_archive::vfs::Origin::Loose(p) => p.display().to_string(),
            lodforge_archive::vfs::Origin::Archive(p) => p.display().to_string(),
        })
    }
    fn list(&self, prefix: &str) -> Vec<String> {
        self.0.list_prefix(prefix)
    }
}

fn settings(a: &Args) -> Result<ViewSettings> {
    let mode = match a.levels.to_ascii_lowercase().as_str() {
        "game" => LodMode::Game { distance_mult: a.lod_distance },
        l => LodMode::Single(l.trim_start_matches("lod").parse().with_context(|| format!("--levels {l}: expected game, 4, 8, 16 or 32"))?),
    };
    Ok(ViewSettings { mode, lod_distances: lod_distances(a), layers: Layers::parse(&a.layers), shading: Shading::parse(&a.shading).ok_or_else(|| anyhow!("--shading {}: expected textured, unlit, normals, flat, level or kind", a.shading))?, normal_maps: !a.no_normals, ..ViewSettings::default() })
}

/// The game arrangement's switch distances: the profile's SkyrimPrefs.ini (profile-local INIs
/// first, then the one in Documents\My Games), else the game's defaults.
fn lod_distances(a: &Args) -> lodforge_viewer::lodselect::LodDistances {
    use lodforge_viewer::lodselect::LodDistances;
    let mut candidates = Vec::new();
    if let Some(inst) = &a.instance {
        candidates.push(inst.join("profiles").join(&a.profile).join("skyrimprefs.ini"));
    }
    if let Some(home) = std::env::var_os("USERPROFILE") {
        candidates.push(std::path::PathBuf::from(home).join(r"Documents\My Games\Skyrim Special Edition\SkyrimPrefs.ini"));
    }
    for p in candidates {
        if let Ok(text) = std::fs::read_to_string(&p) {
            let d = LodDistances::from_prefs_ini(&text);
            eprintln!("LOD distances {} / {} / {} from {}", d.level0, d.level1, d.maximum, p.display());
            return d;
        }
    }
    LodDistances::default()
}

/// The worldspace to start on: Tamriel when present, else the first listed.
fn default_world(worlds: &[String]) -> Option<String> {
    worlds.iter().find(|w| w.eq_ignore_ascii_case("Tamriel")).or_else(|| worlds.first()).cloned()
}

/// Started from a GUI program (an MO2 tool entry): the console was created for this process alone
/// and nobody reads it, so release it rather than leave a console window beside the viewer. A
/// console shared with a shell or a wrapper script (more than one process on it) is kept.
#[cfg(windows)]
fn release_private_console() {
    use windows_sys::Win32::System::Console::{FreeConsole, GetConsoleProcessList};
    let mut pids = [0u32; 2];
    // SAFETY: the buffer holds two entries and the call is told so; it writes at most that many.
    let n = unsafe { GetConsoleProcessList(pids.as_mut_ptr(), pids.len() as u32) };
    if n == 1 {
        // SAFETY: detaching from a console this process owns alone.
        unsafe { FreeConsole() };
    }
}

#[cfg(not(windows))]
fn release_private_console() {}

pub fn run(a: &Args) -> Result<()> {
    let out_dir = match (&a.out, &a.instance) {
        (Some(o), _) => o.clone(),
        (None, Some(inst)) => {
            let mods = crate::profile::generated_lod_mods(inst, &a.profile);
            let m = mods.last().cloned().ok_or_else(|| anyhow!("no generated LOD mod is enabled in profile {} of {}; pass --out", a.profile, inst.display()))?;
            eprintln!("output: mod \"{m}\"");
            inst.join("mods").join(m)
        }
        (None, None) => return Err(anyhow!("pass --out <folder> or --instance <MO2 instance>")),
    };
    let out = DirFiles::new(&out_dir, "output").with_context(|| format!("reading {}", out_dir.display()))?;
    if out.is_empty() {
        return Err(anyhow!("{} has no files", out_dir.display()));
    }
    let out: Arc<dyn Files> = Arc::new(out);
    let mut layered = Layered::new().with(out.clone());
    if let Some(inst) = &a.instance {
        let game = a.game.clone().unwrap_or_else(|| inst.join("Stock Game"));
        let t = std::time::Instant::now();
        let p = crate::profile::open(inst, &a.profile, &game).context("opening the MO2 profile")?;
        eprintln!("profile {}: {} loose files, {} archives in {} ms", a.profile, p.vfs.loose_count(), p.vfs.archives().len(), t.elapsed().as_millis());
        layered.push(Arc::new(VfsFiles(p.vfs)));
    }
    let files: Arc<dyn Files> = Arc::new(layered);
    let lister: Arc<dyn Files> = if a.merge_load_order && a.instance.is_some() { files.clone() } else { out.clone() };
    let worlds = world::list_worlds(lister.as_ref());
    if worlds.is_empty() {
        return Err(anyhow!("no terrain or object LOD files under {}\\meshes\\terrain", out_dir.display()));
    }
    let world_name = match &a.world {
        Some(w) => worlds.iter().find(|x| x.eq_ignore_ascii_case(w)).cloned().ok_or_else(|| anyhow!("worldspace {w} not found; available: {}", worlds.join(", ")))?,
        None => default_world(&worlds).expect("worlds is not empty"),
    };
    let load = LoadOptions { texture_budget: a.texture_budget_mb << 20, normal_maps: !a.no_normals, levels: a.load_levels.clone(), ..LoadOptions::default() };
    let settings = settings(a)?;
    let camera = a.camera.as_deref().map(|c| FreeCam::parse(c).ok_or_else(|| anyhow!("--camera {c}: expected x,y,z[,yaw,pitch]"))).transpose()?.map(|c| FreeCam { fov: a.fov, ..c });

    let lod_center = a
        .lod_center
        .as_deref()
        .map(|c| {
            let v: Vec<f32> = c.split(',').filter_map(|p| p.trim().parse().ok()).collect();
            if v.len() == 2 { Ok([v[0], v[1]]) } else { Err(anyhow!("--lod-center {c}: expected x,y")) }
        })
        .transpose()?;

    if a.check {
        let w = world::discover(files.as_ref(), lister.as_ref(), &world_name, a.season.as_deref());
        let t = std::time::Instant::now();
        let (report, messages) = lodforge_viewer::check(files.as_ref(), &w, &load, &Tolerances::default(), a.instance.is_some());
        for m in &messages {
            eprintln!("{m}");
        }
        let s = &report.summary;
        println!("{}{}: {} files, {} shapes, {} triangles; {} errors, {} warnings, {} notes ({} ms)", w.world, w.season.as_ref().map(|s| format!(" [{s}]")).unwrap_or_default(), s.quads, s.shapes, s.triangles, s.errors, s.warnings, s.infos, t.elapsed().as_millis());
        for i in report.issues.iter().filter(|i| i.severity != lodforge_viewer::checks::Severity::Info).take(200) {
            println!("{:<8} {:<24} {}", i.severity.label(), i.kind, i.message);
        }
        if let Some(p) = &a.json {
            std::fs::write(p, serde_json::to_vec_pretty(&report)?).with_context(|| format!("writing {}", p.display()))?;
            println!("report: {}", p.display());
        }
        if s.errors > 0 {
            std::process::exit(2);
        }
        return Ok(());
    }

    if let Some(png) = &a.screenshot {
        let (w, h) = a.size.split_once('x').and_then(|(w, h)| Some((w.parse::<u32>().ok()?, h.parse::<u32>().ok()?))).ok_or_else(|| anyhow!("--size {}: expected WxH", a.size))?;
        let wf = world::discover(files.as_ref(), lister.as_ref(), &world_name, a.season.as_deref());
        let t = std::time::Instant::now();
        let (bytes, messages) = lodforge_viewer::screenshot(lodforge_viewer::ScreenshotConfig { files, world: wf, load, settings, camera, lod_center, width: w, height: h, samples: a.samples })?;
        for m in &messages {
            eprintln!("{m}");
        }
        std::fs::write(png, bytes).with_context(|| format!("writing {}", png.display()))?;
        println!("{} ({w}x{h}) in {} ms", png.display(), t.elapsed().as_millis());
        return Ok(());
    }

    release_private_console();
    app::run(app::ViewerConfig {
        files,
        lister,
        world: world_name.clone(),
        season: a.season.clone(),
        worlds,
        load,
        settings,
        camera,
        title: format!("LODForge viewer - {} - {}", world_name, out_dir.display()),
        screenshot_dir: std::env::current_dir().unwrap_or_else(|_| out_dir.clone()),
        load_order: a.instance.is_some(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tamriel_first() {
        let w = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(default_world(&w(&["Blackreach", "tamriel", "Sovngarde"])).as_deref(), Some("tamriel"));
        assert_eq!(default_world(&w(&["Blackreach", "Sovngarde"])).as_deref(), Some("Blackreach"));
        assert_eq!(default_world(&[]), None);
    }
}
