//! Opening an MO2 profile: VFS (loose + archives) and load order.

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};

use lodforge_archive::{archive_load_order, ini_archive_list, Mo2Layout, Vfs};
use lodforge_esp::{mo2_active_plugins, LoadOrder};

pub struct Profile {
    pub game_dir: PathBuf,
    pub vfs: Vfs,
    pub plugin_names: Vec<String>,
    pub load_order: LoadOrder,
    pub missing_archives: Vec<String>,
    pub timings_ms: [u128; 3],
}

#[derive(clap::Args, Clone)]
pub struct ProfileArgs {
    /// MO2 instance root (contains ModOrganizer.exe, mods\, profiles\).
    #[arg(long)]
    pub instance: PathBuf,
    #[arg(long, default_value = "Default")]
    pub profile: String,
    /// Game folder (contains Data\ and Skyrim.ccc). Defaults to `<instance>\Stock Game`.
    #[arg(long)]
    pub game: Option<PathBuf>,
    /// Leave these enabled mods out of the input (comma separated), e.g. earlier LOD output.
    #[arg(long, value_delimiter = ',')]
    pub exclude_mod: Vec<String>,
    /// Leave these active plugins out of the load order (comma separated).
    #[arg(long, value_delimiter = ',')]
    pub exclude_plugin: Vec<String>,
}

impl ProfileArgs {
    pub fn game_dir(&self) -> PathBuf {
        self.game.clone().unwrap_or_else(|| self.instance.join("Stock Game"))
    }

    pub fn open(&self) -> Result<Profile> {
        open_filtered(&self.instance, &self.profile, &self.game_dir(), &self.exclude_mod, &self.exclude_plugin)
    }
}

pub fn open(instance: &Path, profile: &str, game: &Path) -> Result<Profile> {
    open_filtered(instance, profile, game, &[], &[])
}

/// Opens a profile without the named mods (loose files and their plugins' archives) and plugins.
pub fn open_filtered(instance: &Path, profile: &str, game: &Path, exclude_mods: &[String], exclude_plugins: &[String]) -> Result<Profile> {
    // Edited DynDOLOD config files from the UI (lodforge_dyndolod::overlay), unless excluded.
    let overlay = lodforge_dyndolod::overlay::for_instance(instance);
    let excluded = exclude_mods.iter().any(|m| m.eq_ignore_ascii_case(lodforge_dyndolod::overlay::MOD_NAME));
    lodforge_dyndolod::overlay::set((overlay.is_dir() && !excluded).then_some(overlay));
    let layout = Mo2Layout::portable(instance, profile, &game.join("Data"));

    let t = Instant::now();
    let mut vfs = if exclude_mods.is_empty() {
        Vfs::mo2(&layout).context("building VFS")?
    } else {
        let mut roots = vec![layout.game_data.clone()];
        let mods = layout.enabled_mods().context("reading modlist.txt")?;
        roots.extend(mods.into_iter().filter(|m| !exclude_mods.iter().any(|x| x.eq_ignore_ascii_case(m))).map(|m| layout.mods_dir.join(m)).filter(|p| p.is_dir()));
        if layout.overwrite_dir.is_dir() {
            roots.push(layout.overwrite_dir.clone());
        }
        Vfs::from_roots(roots).context("building VFS")?
    };
    let vfs_ms = t.elapsed().as_millis();

    let t = Instant::now();
    let mut plugin_names = mo2_active_plugins(&layout.profile_dir, Some(&game.join("Skyrim.ccc"))).context("reading load order")?;
    // Plugins that only an excluded mod provided are gone from the VFS too.
    plugin_names.retain(|n| !exclude_plugins.iter().any(|x| x.eq_ignore_ascii_case(n)) && vfs.loose_path(n).is_some());
    let (skipped, kept): (Vec<String>, Vec<String>) = plugin_names.into_iter().partition(|n| is_generated_lod_plugin(n));
    plugin_names = kept;
    if !skipped.is_empty() {
        eprintln!("note: ignoring generated LOD plugins in the load order: {} (output of an earlier LOD run, never an input)", skipped.join(", "));
    }
    let load_order = LoadOrder::open(&plugin_names, |n| vfs.loose_path(n).map(|p| p.to_owned())).context("opening plugins")?;
    let plugins_ms = t.elapsed().as_millis();

    let t = Instant::now();
    // MO2 profiles with local INIs keep them in the profile folder.
    let inis: Vec<PathBuf> = ["skyrim.ini", "skyrimcustom.ini"].iter().map(|n| layout.profile_dir.join(n)).collect();
    let order = archive_load_order(&ini_archive_list(&inis), &plugin_names, &vfs);
    let missing_archives = vfs.add_archives(&order).context("opening archives")?;
    let archives_ms = t.elapsed().as_millis();

    Ok(Profile { game_dir: game.to_path_buf(), vfs, plugin_names, load_order, missing_archives, timings_ms: [vfs_ms, plugins_ms, archives_ms] })
}

/// Enabled mods of a profile that hold earlier LOD output: mods LODForge created (meta.ini
/// comment) and DynDOLOD output (DynDOLOD.esm or Occlusion.esp in the mod root). A build must not
/// read its own previous results.
pub fn generated_lod_mods(instance: &Path, profile: &str) -> Vec<String> {
    let layout = Mo2Layout::portable(instance, profile, Path::new(""));
    let Ok(mods) = layout.enabled_mods() else { return Vec::new() };
    mods.into_iter()
        .filter(|m| {
            let dir = layout.mods_dir.join(m);
            let ours = std::fs::read_to_string(dir.join("meta.ini")).is_ok_and(|t| t.lines().any(|l| l.trim() == "comments=Generated by LODForge"));
            ours || ["DynDOLOD.esm", "Occlusion.esp"].iter().any(|n| dir.join(n).is_file())
        })
        .collect()
}

/// Plugins a LOD generator writes (DynDOLOD's, or LODForge's own in DynDOLOD mode). DynDOLOD refuses
/// to run while its previous output is active; LODForge leaves them out of the load order it reads,
/// so a profile with an old LOD output enabled builds the same LOD as one without (in LODForge-E2E,
/// DynDOLOD.esm's tree copies otherwise showed up as 3 trees without billboards).
pub fn is_generated_lod_plugin(name: &str) -> bool {
    ["DynDOLOD.esm", "DynDOLOD.esp", "Occlusion.esp"].iter().any(|g| name.eq_ignore_ascii_case(g))
}

#[cfg(test)]
mod tests {
    #[test]
    fn generated_lod_plugins() {
        assert!(super::is_generated_lod_plugin("dyndolod.esm"));
        assert!(super::is_generated_lod_plugin("Occlusion.esp"));
        assert!(!super::is_generated_lod_plugin("DynDOLOD Resources.esp"));
        assert!(!super::is_generated_lod_plugin("Skyrim.esm"));
    }
}
