//! The game's view of `Data\`: loose files layered over BSAs, the way Skyrim resolves them, with
//! the loose layer assembled the way MO2's virtual file system would present it.
//!
//! Resolution order (highest wins):
//! 1. loose files: MO2 overwrite > enabled mods (top of modlist.txt = highest) > game Data folder;
//! 2. archives: later-loaded wins; INI archive lists load first, then each active plugin's
//!    `<name>.bsa` and `<name> - Textures.bsa` in plugin load order.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rayon::prelude::*;

use crate::bsa::Bsa;
use crate::{normalize, Error, Result};

/// Paths an MO2 instance uses.
#[derive(Debug, Clone)]
pub struct Mo2Layout {
    pub game_data: PathBuf,
    pub mods_dir: PathBuf,
    pub overwrite_dir: PathBuf,
    pub profile_dir: PathBuf,
}

impl Mo2Layout {
    /// Default layout of a portable instance: `<root>\mods`, `<root>\overwrite`, `<root>\profiles\<profile>`.
    pub fn portable(instance_root: &Path, profile: &str, game_data: &Path) -> Mo2Layout {
        Mo2Layout {
            game_data: game_data.to_owned(),
            mods_dir: instance_root.join("mods"),
            overwrite_dir: instance_root.join("overwrite"),
            profile_dir: instance_root.join("profiles").join(profile),
        }
    }

    /// Enabled mods from modlist.txt, lowest priority first. Separators are skipped.
    pub fn enabled_mods(&self) -> Result<Vec<String>> {
        let p = self.profile_dir.join("modlist.txt");
        let text = std::fs::read(&p).map_err(|e| Error::Io { path: p.clone(), source: e })?;
        let mut mods: Vec<String> = String::from_utf8_lossy(&text)
            .lines()
            .filter_map(|l| l.trim_end().strip_prefix('+'))
            .filter(|n| !n.ends_with("_separator"))
            .map(str::to_string)
            .collect();
        mods.reverse();
        Ok(mods)
    }
}

pub struct Vfs {
    /// Normalized relative path -> absolute file path of the winning loose file.
    loose: HashMap<String, PathBuf>,
    archives: Vec<Bsa>,
    /// Normalized path -> (archive index, file index) of the winning archived file.
    archived: HashMap<String, (u32, u32)>,
    /// Loose-layer roots, lowest priority first (for diagnostics).
    pub roots: Vec<PathBuf>,
}

impl Vfs {
    /// Builds the loose layer from `roots` (lowest priority first), walking them in parallel.
    pub fn from_roots(roots: Vec<PathBuf>) -> Result<Vfs> {
        let listings: Vec<Vec<(String, PathBuf)>> = roots.par_iter().map(|r| list_files(r)).collect::<Result<_>>()?;
        let mut loose = HashMap::with_capacity(listings.iter().map(Vec::len).sum());
        for list in listings {
            loose.extend(list);
        }
        Ok(Vfs { loose, archives: Vec::new(), archived: HashMap::new(), roots })
    }

    /// Loose layer for an MO2 profile: game Data, then enabled mods, then overwrite.
    pub fn mo2(layout: &Mo2Layout) -> Result<Vfs> {
        let mut roots = vec![layout.game_data.clone()];
        roots.extend(layout.enabled_mods()?.into_iter().map(|m| layout.mods_dir.join(m)).filter(|p| p.is_dir()));
        if layout.overwrite_dir.is_dir() {
            roots.push(layout.overwrite_dir.clone());
        }
        Vfs::from_roots(roots)
    }

    /// Adds `root` as the highest-priority loose layer (e.g. a build's own output folder, so later
    /// stages see the files earlier stages wrote).
    pub fn add_loose_root(&mut self, root: &Path) -> Result<()> {
        if root.is_dir() {
            self.loose.extend(list_files(root)?);
            self.roots.push(root.to_path_buf());
        }
        Ok(())
    }

    /// Adds the files `rels` (relative to `root`, which should already be a loose root) to the
    /// loose layer, for files written after `root` was listed.
    pub fn add_loose_files<'a>(&mut self, root: &Path, rels: impl IntoIterator<Item = &'a str>) {
        for r in rels {
            self.loose.insert(normalize(r), root.join(r.replace('\\', "/")));
        }
    }

    /// Opens archives (names relative to Data, in load order; missing ones are skipped and
    /// returned) and layers them under the loose files. Archives are indexed in parallel.
    pub fn add_archives(&mut self, names: &[String]) -> Result<Vec<String>> {
        let mut missing = Vec::new();
        let mut paths = Vec::new();
        for n in names {
            match self.loose_path(n) {
                Some(p) => paths.push(p.to_owned()),
                None => missing.push(n.clone()),
            }
        }
        let opened: Vec<Bsa> = paths.par_iter().map(|p| Bsa::open(p)).collect::<Result<_>>()?;
        for bsa in opened {
            let ai = self.archives.len() as u32;
            for (fi, f) in bsa.files.iter().enumerate() {
                self.archived.insert(f.path.clone(), (ai, fi as u32));
            }
            self.archives.push(bsa);
        }
        Ok(missing)
    }

    pub fn archives(&self) -> &[Bsa] {
        &self.archives
    }
    pub fn loose_count(&self) -> usize {
        self.loose.len()
    }
    pub fn archived_count(&self) -> usize {
        self.archived.len()
    }

    /// Absolute path of a loose file, if the loose layer has it.
    pub fn loose_path(&self, rel: &str) -> Option<&Path> {
        self.loose.get(&normalize(rel)).map(PathBuf::as_path)
    }

    pub fn exists(&self, rel: &str) -> bool {
        let n = normalize(rel);
        self.loose.contains_key(&n) || self.archived.contains_key(&n)
    }

    /// Where a file comes from: loose path or archive path.
    pub fn origin(&self, rel: &str) -> Option<Origin<'_>> {
        let n = normalize(rel);
        if let Some(p) = self.loose.get(&n) {
            return Some(Origin::Loose(p));
        }
        self.archived.get(&n).map(|&(a, _)| Origin::Archive(&self.archives[a as usize].path))
    }

    /// Reads a file through the VFS.
    pub fn read(&self, rel: &str) -> Result<Option<Vec<u8>>> {
        let n = normalize(rel);
        if let Some(p) = self.loose.get(&n) {
            return std::fs::read(p).map(Some).map_err(|e| Error::Io { path: p.clone(), source: e });
        }
        match self.archived.get(&n) {
            Some(&(a, f)) => self.archives[a as usize].read(f as usize).map(Some),
            None => Ok(None),
        }
    }

    /// Paths whose winning copy is in an archive (not overridden by a loose file).
    pub fn archived_paths(&self) -> impl Iterator<Item = &str> {
        self.archived.keys().filter(|k| !self.loose.contains_key(*k)).map(String::as_str)
    }

    /// All visible files whose normalized path starts with `prefix` (e.g. `lodsettings\`).
    pub fn list_prefix(&self, prefix: &str) -> Vec<String> {
        let p = normalize(prefix);
        let mut v: Vec<String> = self.loose.keys().chain(self.archived.keys()).filter(|k| k.starts_with(&p)).cloned().collect();
        v.sort();
        v.dedup();
        v
    }
}

#[derive(Debug)]
pub enum Origin<'a> {
    Loose(&'a Path),
    Archive(&'a Path),
}

fn list_files(root: &Path) -> Result<Vec<(String, PathBuf)>> {
    let mut out = Vec::new();
    let mut stack = vec![(root.to_owned(), String::new())];
    while let Some((dir, rel)) = stack.pop() {
        let rd = std::fs::read_dir(&dir).map_err(|e| Error::Io { path: dir.clone(), source: e })?;
        for entry in rd {
            let entry = entry.map_err(|e| Error::Io { path: dir.clone(), source: e })?;
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            let child_rel = if rel.is_empty() { name.clone() } else { format!("{rel}\\{name}") };
            let ft = entry.file_type().map_err(|e| Error::Io { path: entry.path(), source: e })?;
            if ft.is_dir() {
                stack.push((entry.path(), child_rel));
            } else if !(rel.is_empty() && name == "meta.ini") {
                out.push((child_rel, entry.path()));
            }
        }
    }
    Ok(out)
}

/// Archive load order the game uses: INI lists first, then per-plugin archives in plugin order.
pub fn archive_load_order(ini_archives: &[String], plugins: &[String], vfs: &Vfs) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let push = |n: String, out: &mut Vec<String>| {
        if !out.iter().any(|e| e.eq_ignore_ascii_case(&n)) {
            out.push(n);
        }
    };
    for a in ini_archives {
        push(a.clone(), &mut out);
    }
    for p in plugins {
        let stem = match p.rfind('.') {
            Some(i) => &p[..i],
            None => p,
        };
        for cand in [format!("{stem}.bsa"), format!("{stem} - Textures.bsa")] {
            if vfs.loose_path(&cand).is_some() {
                push(cand, &mut out);
            }
        }
    }
    out
}

/// `sResourceArchiveList` + `sResourceArchiveList2` from game INIs (later INIs override keys).
pub fn ini_archive_list(inis: &[PathBuf]) -> Vec<String> {
    let mut keys: HashMap<String, String> = HashMap::new();
    for ini in inis {
        let Ok(bytes) = std::fs::read(ini) else { continue };
        let mut section = String::new();
        for line in String::from_utf8_lossy(&bytes).lines() {
            let line = line.trim();
            if line.starts_with('[') {
                section = line.trim_matches(|c| c == '[' || c == ']').to_ascii_lowercase();
            } else if section == "archive" {
                if let Some((k, v)) = line.split_once('=') {
                    keys.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
                }
            }
        }
    }
    ["sresourcearchivelist", "sresourcearchivelist2"]
        .iter()
        .filter_map(|k| keys.get(*k))
        .flat_map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect::<Vec<_>>())
        .collect()
}
