//! The LODForge Config overlay: edited copies of DynDOLOD's configuration files, kept in an MO2
//! mod (`<instance>\mods\LODForge Config\DynDOLOD\`) with the same layout as DynDOLOD's
//! `Edit Scripts\DynDOLOD` folder (`Configs\...`, `Rules\...`, `DynDOLOD_SSE.ini`). Every config
//! read goes through [path]: the overlay's copy wins when it exists, so the UI can edit the lists
//! without touching DynDOLOD's install. The folder is read directly (enabled in the profile or
//! not).

use std::path::{Path, PathBuf};
use std::sync::RwLock;

static OVERLAY: RwLock<Option<PathBuf>> = RwLock::new(None);

/// Name of the overlay mod the UI writes.
pub const MOD_NAME: &str = "LODForge Config";

/// Sets the overlay folder (the `DynDOLOD` folder inside the mod); `None` turns it off.
pub fn set(dir: Option<PathBuf>) {
    *OVERLAY.write().unwrap() = dir;
}

/// The overlay folder for an MO2 instance.
pub fn for_instance(instance: &Path) -> PathBuf {
    instance.join("mods").join(MOD_NAME).join("DynDOLOD")
}

pub fn current() -> Option<PathBuf> {
    OVERLAY.read().unwrap().clone()
}

/// `dir\rel` (DynDOLOD's config folder), or the overlay's copy of `rel` when it exists.
pub fn path(dir: &Path, rel: &str) -> PathBuf {
    if let Some(o) = current() {
        let p = o.join(rel);
        if p.is_file() {
            return p;
        }
    }
    dir.join(rel)
}

/// The files of a sub-folder (`Configs`), DynDOLOD's and the overlay's merged by name (the
/// overlay's copy wins).
pub fn list(dir: &Path, sub: &str) -> Vec<PathBuf> {
    let mut by_name: std::collections::BTreeMap<String, PathBuf> = std::collections::BTreeMap::new();
    let mut add = |root: &Path| {
        if let Ok(rd) = std::fs::read_dir(root.join(sub)) {
            for e in rd.flatten() {
                if e.path().is_file() {
                    by_name.insert(e.file_name().to_string_lossy().to_ascii_lowercase(), e.path());
                }
            }
        }
    };
    add(dir);
    if let Some(o) = current() {
        add(&o);
    }
    by_name.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_copy_wins() {
        let base = std::env::temp_dir().join(format!("lodforge-overlay-{}", std::process::id()));
        let (dd, ov) = (base.join("dd"), base.join("ov"));
        std::fs::create_dir_all(dd.join("Configs")).unwrap();
        std::fs::create_dir_all(ov.join("Configs")).unwrap();
        std::fs::write(dd.join(r"Configs\a.txt"), "dd").unwrap();
        std::fs::write(dd.join(r"Configs\b.txt"), "dd").unwrap();
        std::fs::write(ov.join(r"Configs\b.txt"), "ov").unwrap();
        std::fs::write(ov.join(r"Configs\c.txt"), "ov").unwrap();
        set(Some(ov.clone()));
        assert_eq!(path(&dd, r"Configs\a.txt"), dd.join(r"Configs\a.txt"));
        assert_eq!(path(&dd, r"Configs\b.txt"), ov.join(r"Configs\b.txt"));
        let l = list(&dd, "Configs");
        assert_eq!(l.len(), 3);
        assert!(l.contains(&ov.join(r"Configs\b.txt")));
        set(None);
        assert_eq!(path(&dd, r"Configs\b.txt"), dd.join(r"Configs\b.txt"));
        let _ = std::fs::remove_dir_all(base);
    }
}
