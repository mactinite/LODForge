//! The game's view of `Data\`: loose files layered over BSAs, the way Skyrim resolves them, with
//! the loose layer assembled the way MO2's virtual file system would present it.
//!
//! Resolution order (highest wins):
//! 1. loose files: MO2 overwrite > enabled mods (top of modlist.txt = highest) > game Data folder;
//! 2. archives: later-loaded wins; INI archive lists load first, then each active plugin's
//!    `<name>.bsa` and `<name> - Textures.bsa` in plugin load order.

use std::collections::{HashMap, HashSet};
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
    /// Files [`Vfs::add_loose_root_since`] left out, kept only with `LODFORGE_TRACE_STALE` set:
    /// each lookup that would have found one of them is reported once on stderr.
    stale: HashMap<String, PathBuf>,
    stale_seen: std::sync::Mutex<HashSet<String>>,
}

impl Vfs {
    /// Builds the loose layer from `roots` (lowest priority first), walking them in parallel.
    pub fn from_roots(roots: Vec<PathBuf>) -> Result<Vfs> {
        let listings: Vec<Vec<(String, PathBuf)>> = roots.par_iter().map(|r| list_files(r)).collect::<Result<_>>()?;
        let mut loose = HashMap::with_capacity(listings.iter().map(Vec::len).sum());
        for list in listings {
            loose.extend(list);
        }
        Ok(Vfs { loose, archives: Vec::new(), archived: HashMap::new(), roots, stale: HashMap::new(), stale_seen: std::sync::Mutex::new(HashSet::new()) })
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

    /// Adds the files of `root` written since `before` was taken (new files, and files whose
    /// size or modification time changed) as the highest-priority loose layer. What the folder
    /// held before stays invisible: a build run into the output folder of an earlier build reads
    /// nothing of that output through the VFS, only what it wrote itself. Returns the number of
    /// files added and the number left out.
    pub fn add_loose_root_since(&mut self, root: &Path, before: &DirSnapshot) -> Result<(usize, usize)> {
        if !root.is_dir() {
            return Ok((0, 0));
        }
        let (mut added, mut stale) = (0, 0);
        let trace = std::env::var_os("LODFORGE_TRACE_STALE").is_some();
        for (rel, path) in list_files(root)? {
            if before.is_unchanged(&rel, &path) {
                stale += 1;
                if trace && !self.loose.contains_key(&rel) {
                    self.stale.insert(rel, path);
                }
            } else {
                self.loose.insert(rel, path);
                added += 1;
            }
        }
        if !self.roots.iter().any(|r| r == root) {
            self.roots.push(root.to_path_buf());
        }
        Ok((added, stale))
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
        let n = normalize(rel);
        let p = self.loose.get(&n).map(PathBuf::as_path);
        if p.is_none() {
            self.trace_stale(&n);
        }
        p
    }

    pub fn exists(&self, rel: &str) -> bool {
        let n = normalize(rel);
        let hit = self.loose.contains_key(&n) || self.archived.contains_key(&n);
        if !hit {
            self.trace_stale(&n);
        }
        hit
    }

    /// `LODFORGE_TRACE_STALE`: a lookup missed `n`, which an earlier build's output in a loose
    /// root holds (see [`Vfs::add_loose_root_since`]); reported once per path.
    fn trace_stale(&self, n: &str) {
        if !self.stale.is_empty() && self.stale.contains_key(n) && self.stale_seen.lock().unwrap().insert(n.to_string()) {
            eprintln!("stale output lookup: {n}");
        }
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
            None => {
                self.trace_stale(&n);
                Ok(None)
            }
        }
    }

    /// Reads the archived copy of a file (the BSA layer alone), whether or not a loose file
    /// overrides it: vanilla data even when a generated loose file sits on top.
    pub fn read_archived(&self, rel: &str) -> Result<Option<Vec<u8>>> {
        let n = normalize(rel);
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
        if !self.stale.is_empty() {
            for k in self.stale.keys().filter(|k| k.starts_with(&p) && !self.loose.contains_key(*k) && !self.archived.contains_key(*k)) {
                self.trace_stale(k);
            }
        }
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

/// What a folder held before a build wrote into it: each file's size and modification time by
/// relative path (as [`Vfs`] keys it), so that [`Vfs::add_loose_root_since`] can tell the files
/// the build wrote from the files an earlier build left in the same folder.
#[derive(Debug, Default, Clone)]
pub struct DirSnapshot {
    files: HashMap<String, (u64, Option<std::time::SystemTime>)>,
}

impl DirSnapshot {
    /// Snapshot of `root` (empty when it does not exist).
    pub fn take(root: &Path) -> Result<DirSnapshot> {
        let mut files = HashMap::new();
        if root.is_dir() {
            for (rel, path) in list_files(root)? {
                files.insert(rel, file_stamp(&path));
            }
        }
        Ok(DirSnapshot { files })
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }

    /// Whether `path`, at `rel` below the snapshot's root, is still the file the snapshot saw
    /// (same size and modification time). A file the snapshot does not know is new.
    pub fn is_unchanged(&self, rel: &str, path: &Path) -> bool {
        self.files.get(rel).is_some_and(|&stamp| stamp == file_stamp(path))
    }
}

fn file_stamp(path: &Path) -> (u64, Option<std::time::SystemTime>) {
    std::fs::metadata(path).map(|m| (m.len(), m.modified().ok())).unwrap_or((0, None))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lodforge-vfs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write(root: &Path, rel: &str, bytes: &[u8]) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    }

    /// Two builds into one output folder: the second registers only what it wrote (a rewritten
    /// file of the same size, a new file), never the first build's leftovers.
    #[test]
    fn second_build_sees_only_its_own_output() {
        let out = scratch("rebuild");
        // Build 1.
        let before1 = DirSnapshot::take(&out).unwrap();
        assert!(before1.is_empty());
        write(&out, "meshes/terrain/W/Objects/W.4.0.0.bto", b"bto-1");
        write(&out, "textures/lod/a_lod.dds", b"atlas-1");
        write(&out, "textures/lod/stale_lod.dds", b"left behind");
        let mut vfs1 = Vfs::from_roots(Vec::new()).unwrap();
        assert_eq!(vfs1.add_loose_root_since(&out, &before1).unwrap(), (3, 0));
        assert!(vfs1.exists("textures\\lod\\stale_lod.dds"));
        // Make the first build's files old enough that a rewrite is a visible timestamp change.
        let old = SystemTime::now() - Duration::from_secs(3600);
        for rel in ["meshes/terrain/W/Objects/W.4.0.0.bto", "textures/lod/a_lod.dds", "textures/lod/stale_lod.dds"] {
            std::fs::File::options().write(true).open(out.join(rel)).unwrap().set_modified(old).unwrap();
        }
        // Build 2 into the same folder: one file rewritten with the same size, one new file.
        let before2 = DirSnapshot::take(&out).unwrap();
        assert_eq!(before2.len(), 3);
        write(&out, "meshes/terrain/W/Objects/W.4.0.0.bto", b"bto-2");
        write(&out, "textures/lod/b_lod.dds", b"atlas-2");
        let mut vfs2 = Vfs::from_roots(Vec::new()).unwrap();
        assert_eq!(vfs2.add_loose_root_since(&out, &before2).unwrap(), (2, 2));
        assert_eq!(vfs2.read("meshes\\terrain\\W\\Objects\\W.4.0.0.bto").unwrap().as_deref(), Some(&b"bto-2"[..]));
        assert!(vfs2.exists("textures\\lod\\b_lod.dds"));
        assert!(!vfs2.exists("textures\\lod\\a_lod.dds"), "untouched file of the earlier build must stay invisible");
        assert!(!vfs2.exists("textures\\lod\\stale_lod.dds"));
        assert_eq!(vfs2.list_prefix("textures\\lod\\"), vec!["textures\\lod\\b_lod.dds".to_string()]);
        assert_eq!(vfs2.roots, vec![out.clone()]);
        // A second registration later in the same build (more files written) adds the new ones
        // and still leaves the old ones out.
        write(&out, "textures/lod/c_lod.dds", b"atlas-2c");
        assert_eq!(vfs2.add_loose_root_since(&out, &before2).unwrap(), (3, 2));
        assert!(vfs2.exists("textures\\lod\\c_lod.dds"));
        assert!(!vfs2.exists("textures\\lod\\a_lod.dds"));
        assert_eq!(vfs2.roots.len(), 1);
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn snapshot_of_missing_folder_is_empty_and_registration_is_a_no_op() {
        let out = scratch("missing").join("not-there");
        let before = DirSnapshot::take(&out).unwrap();
        assert!(before.is_empty());
        let mut vfs = Vfs::from_roots(Vec::new()).unwrap();
        assert_eq!(vfs.add_loose_root_since(&out, &before).unwrap(), (0, 0));
        assert!(vfs.roots.is_empty());
        let _ = std::fs::remove_dir_all(out.parent().unwrap());
    }
}
