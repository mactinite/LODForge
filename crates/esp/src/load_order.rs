//! Load order: which plugins are active, in what order, and how their file-local form IDs map to
//! load-order (global) form IDs.
//!
//! Global form IDs follow the game: full plugins get mod index 0x00..=0xFD, light plugins share
//! 0xFE with a 12-bit light index, giving `FE LLL OOO`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rayon::prelude::*;

use crate::plugin::Plugin;
use crate::{Error, Result};

/// Masters every SSE install loads first, whether or not plugins.txt lists them.
pub const IMPLICIT_MASTERS: [&str; 5] = ["Skyrim.esm", "Update.esm", "Dawnguard.esm", "HearthFires.esm", "Dragonborn.esm"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModIndex {
    Full(u8),
    Light(u16),
}

impl ModIndex {
    pub fn global(self, local_object: u32) -> u32 {
        match self {
            ModIndex::Full(i) => (u32::from(i) << 24) | (local_object & 0x00FF_FFFF),
            ModIndex::Light(i) => 0xFE00_0000 | (u32::from(i) << 12) | (local_object & 0xFFF),
        }
    }
}

/// Active plugin names in load order for an MO2 profile.
///
/// Order comes from `loadorder.txt` (falling back to implicit masters + `plugins.txt` order);
/// a plugin is active when it is an implicit master, listed in the game's `Skyrim.ccc`, or marked
/// `*` in `plugins.txt`.
pub fn mo2_active_plugins(profile_dir: &Path, ccc_file: Option<&Path>) -> Result<Vec<String>> {
    let read = |p: PathBuf| -> Result<Vec<String>> {
        let text = std::fs::read(&p).map_err(|e| Error::Io { path: p.clone(), source: e })?;
        Ok(String::from_utf8_lossy(&text).lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty() && !l.starts_with('#')).collect())
    };
    let plugins_txt = read(profile_dir.join("plugins.txt"))?;
    let mut active: HashMap<String, bool> = HashMap::new();
    for l in &plugins_txt {
        match l.strip_prefix('*') {
            Some(n) => active.insert(n.to_ascii_lowercase(), true),
            None => active.insert(l.to_ascii_lowercase(), false),
        };
    }
    let mut implicit: Vec<String> = IMPLICIT_MASTERS.iter().map(|s| s.to_string()).collect();
    if let Some(ccc) = ccc_file.filter(|p| p.exists()) {
        implicit.extend(read(ccc.to_owned())?);
    }
    for n in &implicit {
        active.entry(n.to_ascii_lowercase()).or_insert(true);
    }

    let order = match read(profile_dir.join("loadorder.txt")) {
        Ok(o) => o,
        Err(_) => implicit.iter().cloned().chain(plugins_txt.iter().map(|l| l.trim_start_matches('*').to_string())).collect(),
    };
    let mut seen = std::collections::HashSet::new();
    Ok(order.into_iter().filter(|n| active.get(&n.to_ascii_lowercase()).copied().unwrap_or(false) && seen.insert(n.to_ascii_lowercase())).collect())
}

pub struct LoadedPlugin {
    pub plugin: Plugin,
    pub index: ModIndex,
    /// For each master (by position in the header), its global mod index; the final entry is the
    /// plugin itself. `None` when the master is not in the load order.
    master_map: Vec<Option<ModIndex>>,
}

impl LoadedPlugin {
    /// Maps a form ID as stored in this plugin to its load-order form ID.
    pub fn global_form_id(&self, local: u32) -> Option<u32> {
        let file_index = (local >> 24) as usize;
        let slot = file_index.min(self.master_map.len() - 1);
        self.master_map[slot].map(|m| m.global(local))
    }

    /// True when a stored form ID refers to a record this plugin itself introduces.
    pub fn is_own(&self, local: u32) -> bool {
        (local >> 24) as usize >= self.master_map.len() - 1
    }
}

pub struct LoadOrder {
    pub plugins: Vec<LoadedPlugin>,
}

impl LoadOrder {
    /// Opens `names` (in load order) using `resolve` to find each file, in parallel.
    pub fn open(names: &[String], resolve: impl Fn(&str) -> Option<PathBuf> + Sync) -> Result<LoadOrder> {
        let opened: Vec<Plugin> = names
            .par_iter()
            .map(|n| {
                let path = resolve(n).ok_or_else(|| Error::MissingPlugin(n.clone()))?;
                let mut p = Plugin::open(&path)?;
                p.name = n.clone();
                Ok(p)
            })
            .collect::<Result<_>>()?;

        let (mut full, mut light) = (0u32, 0u32);
        let mut indices = Vec::with_capacity(opened.len());
        for p in &opened {
            let idx = if p.is_light() {
                light += 1;
                if light > 4096 {
                    return Err(Error::TooManyPlugins);
                }
                ModIndex::Light((light - 1) as u16)
            } else {
                full += 1;
                if full > 0xFD {
                    return Err(Error::TooManyPlugins);
                }
                ModIndex::Full((full - 1) as u8)
            };
            indices.push(idx);
        }
        let by_name: HashMap<String, ModIndex> = opened.iter().zip(&indices).map(|(p, i)| (p.name.to_ascii_lowercase(), *i)).collect();

        let plugins = opened
            .into_iter()
            .zip(indices)
            .map(|(plugin, index)| {
                let mut master_map: Vec<Option<ModIndex>> = plugin.header.masters.iter().map(|m| by_name.get(&m.to_ascii_lowercase()).copied()).collect();
                master_map.push(Some(index));
                LoadedPlugin { plugin, index, master_map }
            })
            .collect();
        Ok(LoadOrder { plugins })
    }

    pub fn find(&self, name: &str) -> Option<&LoadedPlugin> {
        self.plugins.iter().find(|p| p.plugin.name.eq_ignore_ascii_case(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_ids() {
        assert_eq!(ModIndex::Full(2).global(0x0100_1408), 0x0200_1408);
        assert_eq!(ModIndex::Light(0x3E).global(0x01_000A5F), 0xFE03_EA5F);
    }

    #[test]
    fn mo2_order_and_activity() {
        let dir = std::env::temp_dir().join(format!("lodforge-lo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("plugins.txt"), "# x\n*A.esp\nB.esp\n*cc.esl\n").unwrap();
        std::fs::write(dir.join("loadorder.txt"), "Skyrim.esm\nUpdate.esm\nDawnguard.esm\nHearthFires.esm\nDragonborn.esm\nccImplicit.esm\ncc.esl\nB.esp\nA.esp\n").unwrap();
        let ccc = dir.join("Skyrim.ccc");
        std::fs::write(&ccc, "ccImplicit.esm\n").unwrap();
        let names = mo2_active_plugins(&dir, Some(&ccc)).unwrap();
        assert_eq!(names, ["Skyrim.esm", "Update.esm", "Dawnguard.esm", "HearthFires.esm", "Dragonborn.esm", "ccImplicit.esm", "cc.esl", "A.esp"]);
        std::fs::remove_dir_all(dir).ok();
    }
}
