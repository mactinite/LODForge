// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! Where the viewer reads from: a generated output folder first, then (optionally) the load
//! order behind it, so textures and models that LOD files reference but the output does not
//! carry resolve the way the game resolves them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Normalizes a Data-relative path: lower case, backslashes, no leading `data\` or slash (the
/// rule of `lodforge-archive`; repeated here so the viewer has no archive dependency).
pub fn normalize(p: &str) -> String {
    let mut s = p.replace('/', "\\").to_ascii_lowercase();
    while let Some(rest) = s.strip_prefix('\\') {
        s = rest.to_string();
    }
    if let Some(rest) = s.strip_prefix("data\\") {
        s = rest.to_string();
    }
    s
}

/// A source of Data-relative files.
pub trait Files: Send + Sync {
    /// Reads a file; `None` when the source has no such file.
    fn read(&self, rel: &str) -> Option<Vec<u8>>;
    /// Whether the source has the file (without reading it).
    fn exists(&self, rel: &str) -> bool;
    /// Where the file comes from, for the inspector ("output", "mods\\X\\...", "archive.bsa").
    fn origin(&self, rel: &str) -> Option<String>;
    /// Normalized paths under a prefix (`meshes\terrain\tamriel\`), sorted.
    fn list(&self, prefix: &str) -> Vec<String>;
}

/// A folder laid out like `Data\`, indexed once (case-insensitive lookups; listings keep the
/// names as spelled on disk).
pub struct DirFiles {
    root: PathBuf,
    /// Normalized path -> (absolute path, relative path as spelled).
    index: HashMap<String, (PathBuf, String)>,
    label: String,
}

impl DirFiles {
    /// Indexes every file under `root`; `label` names the source in origins.
    pub fn new(root: &Path, label: &str) -> std::io::Result<DirFiles> {
        let mut index = HashMap::new();
        let mut stack = vec![(root.to_path_buf(), String::new())];
        while let Some((dir, rel)) = stack.pop() {
            let rd = match std::fs::read_dir(&dir) {
                Ok(rd) => rd,
                Err(e) if dir == root => return Err(e),
                Err(_) => continue,
            };
            for entry in rd.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                let child = if rel.is_empty() {
                    name.clone()
                } else {
                    format!("{rel}\\{name}")
                };
                match entry.file_type() {
                    Ok(t) if t.is_dir() => stack.push((entry.path(), child)),
                    Ok(_) => {
                        index.insert(normalize(&child), (entry.path(), child));
                    }
                    Err(_) => {}
                }
            }
        }
        Ok(DirFiles {
            root: root.to_path_buf(),
            index,
            label: label.to_string(),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    /// Absolute path of a file, if present.
    pub fn path(&self, rel: &str) -> Option<&Path> {
        self.index.get(&normalize(rel)).map(|(p, _)| p.as_path())
    }
}

impl Files for DirFiles {
    fn read(&self, rel: &str) -> Option<Vec<u8>> {
        std::fs::read(self.path(rel)?).ok()
    }

    fn exists(&self, rel: &str) -> bool {
        self.index.contains_key(&normalize(rel))
    }

    fn origin(&self, rel: &str) -> Option<String> {
        self.path(rel).map(|_| self.label.clone())
    }

    fn list(&self, prefix: &str) -> Vec<String> {
        let p = normalize(prefix);
        let mut v: Vec<String> = self
            .index
            .iter()
            .filter(|(k, _)| k.starts_with(&p))
            .map(|(_, (_, spelled))| spelled.clone())
            .collect();
        v.sort();
        v
    }
}

/// Sorted, without case/separator duplicates; the first spelling of a path wins.
fn dedup_normalized(v: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out: Vec<String> = v
        .into_iter()
        .filter(|p| seen.insert(normalize(p)))
        .collect();
    out.sort_by_key(|p| normalize(p));
    out
}

/// Several sources, the first that has a file wins.
#[derive(Default)]
pub struct Layered {
    layers: Vec<Arc<dyn Files>>,
}

impl Layered {
    pub fn new() -> Layered {
        Layered::default()
    }

    pub fn push(&mut self, f: Arc<dyn Files>) {
        self.layers.push(f);
    }

    pub fn with(mut self, f: Arc<dyn Files>) -> Layered {
        self.push(f);
        self
    }
}

impl Files for Layered {
    fn read(&self, rel: &str) -> Option<Vec<u8>> {
        self.layers.iter().find_map(|l| l.read(rel))
    }

    fn exists(&self, rel: &str) -> bool {
        self.layers.iter().any(|l| l.exists(rel))
    }

    fn origin(&self, rel: &str) -> Option<String> {
        self.layers.iter().find_map(|l| l.origin(rel))
    }

    fn list(&self, prefix: &str) -> Vec<String> {
        dedup_normalized(self.layers.iter().flat_map(|l| l.list(prefix)).collect())
    }
}

/// In-memory files, for tests.
#[derive(Default)]
pub struct MemFiles {
    /// Normalized path -> (path as given, bytes).
    pub files: HashMap<String, (String, Vec<u8>)>,
}

impl MemFiles {
    pub fn insert(&mut self, rel: &str, bytes: Vec<u8>) {
        self.files.insert(normalize(rel), (rel.to_string(), bytes));
    }
}

impl Files for MemFiles {
    fn read(&self, rel: &str) -> Option<Vec<u8>> {
        self.files.get(&normalize(rel)).map(|(_, b)| b.clone())
    }

    fn exists(&self, rel: &str) -> bool {
        self.files.contains_key(&normalize(rel))
    }

    fn origin(&self, rel: &str) -> Option<String> {
        self.exists(rel).then(|| "memory".to_string())
    }

    fn list(&self, prefix: &str) -> Vec<String> {
        let p = normalize(prefix);
        dedup_normalized(
            self.files
                .iter()
                .filter(|(k, _)| k.starts_with(&p))
                .map(|(_, (s, _))| s.clone())
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_and_layers() {
        assert_eq!(
            normalize("Data/Textures\\Terrain/X.DDS"),
            "textures\\terrain\\x.dds"
        );
        let mut a = MemFiles::default();
        a.insert("meshes\\a.nif", vec![1]);
        let mut b = MemFiles::default();
        b.insert("Meshes\\A.nif", vec![2]);
        b.insert("meshes\\b.nif", vec![3]);
        let l = Layered::new().with(Arc::new(a)).with(Arc::new(b));
        assert_eq!(l.read("MESHES\\a.nif"), Some(vec![1]));
        assert_eq!(l.read("meshes\\b.nif"), Some(vec![3]));
        // Listings keep the first layer's spelling and drop case duplicates.
        assert_eq!(l.list("MESHES\\"), vec!["meshes\\a.nif", "meshes\\b.nif"]);
        assert!(!l.exists("meshes\\c.nif"));
    }
}
