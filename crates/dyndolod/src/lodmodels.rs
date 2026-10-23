//! LOD model discovery (docs/re/dyndolod-spec.md "LOD model discovery and per-level choice").
//!
//! Every file with `_lod` in its name below `Folder1..5` is indexed by bare file name; a file
//! found in a later folder replaces an earlier one. A full model `<stem>.nif` then gets:
//! - `<stem>_lod_<N>.nif` at model level N (0..3), `_lod_4` at level 3;
//! - `<stem>_lod.nif` at level 0, and at levels 1 and 2 too when no numbered file exists;
//! - a missing level takes the next higher level's file;
//! - `<stem>_<CRC32>_lod...` variants win when the CRC32 matches the installed full model; a
//!   mesh PGPatcher / ParallaxGen patched counts with its CRC32 before patching
//!   ([`ParallaxGenDiff`]);
//! - `_lod_<N>h` / `_lod_<N>l` snow variants for heavy / light snow bases.

use std::collections::{HashMap, HashSet};

/// Bare file name (lower-case) -> Data-relative path (lower-case).
#[derive(Debug, Default, Clone)]
pub struct LodIndex {
    by_name: HashMap<String, String>,
}

impl LodIndex {
    /// Builds the index from all VFS paths. `folders` are Data-relative (`meshes`, `meshes\lod`,
    /// ...), `ignore` partial paths (IgnoreLODMesh) and `misnamed` full paths excluded.
    pub fn scan<'a>(paths: impl Iterator<Item = &'a str> + Clone, folders: &[String], ignore: &[String], misnamed: &std::collections::HashSet<String>) -> LodIndex {
        let mut by_name = HashMap::new();
        for folder in folders {
            let prefix = format!("{}\\", folder.trim_end_matches('\\').to_ascii_lowercase());
            let mut found: Vec<&str> = paths.clone().filter(|p| p.starts_with(&prefix) && p.ends_with(".nif")).collect();
            found.sort_unstable();
            for p in found {
                let name = p.rsplit('\\').next().unwrap_or(p);
                if !name.contains("_lod") || misnamed.contains(p) || ignore.iter().any(|i| p.contains(i.as_str())) {
                    continue;
                }
                by_name.insert(name.to_string(), p.to_string());
            }
        }
        LodIndex { by_name }
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.by_name.get(&name.to_ascii_lowercase()).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    /// True if any indexed name starts with `prefix` (lower-case).
    pub fn any_with_prefix(&self, prefix: &str) -> bool {
        self.by_name.keys().any(|k| k.starts_with(prefix))
    }
}

/// Snow variant of a base (from its directional material).
/// `Configs\DynDOLOD_<GAME>_mesh_crc32.txt`: the CRC32s a vanilla full mesh path can have.
/// A winning mesh at such a path with another CRC was replaced by a mod, so LOD models found
/// by its plain name (made for the vanilla mesh) do not fit it; only `<stem>_<crc>` models do
/// (DynDOLOD 3 Ultra golden, skyrim-cs-base: Ulvenwald's replaced vanilla pines got billboards,
/// not DynDOLOD Resources' `treepineforest03passthru_lod.nif`).
#[derive(Debug, Clone, Default)]
pub struct MeshCrcTable(pub HashMap<String, HashSet<u32>>);

impl MeshCrcTable {
    /// `path;CRC` lines (hex), `//` comments.
    pub fn parse(text: &str) -> MeshCrcTable {
        let mut m: HashMap<String, HashSet<u32>> = HashMap::new();
        for line in text.lines() {
            let l = line.trim();
            if l.is_empty() || l.starts_with("//") {
                continue;
            }
            if let Some((p, c)) = l.rsplit_once(';')
                && let Ok(c) = u32::from_str_radix(c.trim(), 16)
            {
                m.entry(p.trim().to_ascii_lowercase()).or_default().insert(c);
            }
        }
        MeshCrcTable(m)
    }

    /// True when `model` (a `meshes\...` path) is a known vanilla mesh whose CRC is not one of
    /// its vanilla CRCs.
    pub fn replaced(&self, model: &str, crc: Option<u32>) -> bool {
        match (self.0.get(&model.to_ascii_lowercase()), crc) {
            (Some(set), Some(c)) => !set.contains(&c),
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Snow {
    None,
    Heavy,
    Light,
}

/// Model levels 0..3 for a full model file stem (`farmhouse02`), lower-case paths.
/// `snow` gives the `h` / `l` variant per model level (from the base's MNAM).
pub fn lod_set(index: &LodIndex, stem: &str, crc: Option<u32>, snow: [Snow; 4], passthru_first: bool) -> [Option<String>; 4] {
    lod_set_with(index, stem, crc, snow, passthru_first, true)
}

/// [`lod_set`]; `allow_plain = false` only accepts `<stem>_<crc>` models (a full model that a
/// replacer mod changed, see [`MeshCrcTable`]).
pub fn lod_set_with(index: &LodIndex, stem: &str, crc: Option<u32>, snow: [Snow; 4], passthru_first: bool, allow_plain: bool) -> [Option<String>; 4] {
    let stem = stem.to_ascii_lowercase();
    let variants: Vec<String> = {
        let mut v = Vec::new();
        if let Some(c) = crc {
            v.push(format!("{stem}_{c:08x}"));
        }
        if allow_plain {
            v.push(stem.clone());
        }
        v
    };
    let lookup = |name: &str| -> Option<String> { index.get(name).map(str::to_string) };
    let numbered = |base: &str, n: usize| -> Option<String> {
        let suffix = match snow[n.min(3)] {
            Snow::Heavy => Some('h'),
            Snow::Light => Some('l'),
            Snow::None => None,
        };
        if let Some(s) = suffix
            && let Some(p) = lookup(&format!("{base}_lod_{n}{s}.nif"))
        {
            return Some(p);
        }
        lookup(&format!("{base}_lod_{n}.nif"))
    };
    for base in &variants {
        let mut bases = vec![base.clone()];
        if passthru_first {
            bases.insert(0, format!("{base}passthru"));
        }
        for b in &bases {
            let mut levels: [Option<String>; 4] = [numbered(b, 0), numbered(b, 1), numbered(b, 2), numbered(b, 3)];
            if levels[3].is_none() {
                levels[3] = numbered(b, 4);
            }
            // Levels 1 and 2 take the next higher numbered file; `_lod` then fills the first
            // free level (0..2) and the free levels right after it; level 0 finally falls back
            // to level 1.
            for k in (1..3).rev() {
                if levels[k].is_none() {
                    levels[k] = levels[k + 1].clone();
                }
            }
            if let Some(plain) = lookup(&format!("{b}_lod.nif"))
                && let Some(first) = (0..3).find(|&k| levels[k].is_none())
            {
                for l in levels.iter_mut().take(3).skip(first) {
                    if l.is_some() {
                        break;
                    }
                    *l = Some(plain.clone());
                }
            }
            if levels[0].is_none() {
                levels[0] = levels[1].clone();
            }
            if levels.iter().any(Option::is_some) {
                return levels;
            }
        }
    }
    [None, None, None, None]
}

/// `ParallaxGen_Diff.json` (PGPatcher / ParallaxGen output, Data root): per patched mesh the
/// CRC32 of the original and of the patched file,
/// `{ "meshes\\...nif": { "crc32original": n, "crc32patched": n } }`. DynDOLOD
/// (`LoadParallaxGen`) reads it so that `<stem>_<CRC32>_lod` models made for the original mesh
/// still match the patched copy.
#[derive(Debug, Default, Clone)]
pub struct ParallaxGenDiff {
    /// Data-relative mesh path (lower-case, `\`) -> (original, patched) CRC32.
    by_path: HashMap<String, (u32, u32)>,
}

impl ParallaxGenDiff {
    /// Data-relative file name.
    pub const FILE: &'static str = "ParallaxGen_Diff.json";

    /// Parses the file; entries without both CRCs are skipped, invalid JSON gives an empty map.
    pub fn parse(bytes: &[u8]) -> ParallaxGenDiff {
        let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
        let Ok(serde_json::Value::Object(m)) = serde_json::from_slice::<serde_json::Value>(bytes) else {
            return ParallaxGenDiff::default();
        };
        let crc = |v: &serde_json::Value, k: &str| v.get(k).and_then(serde_json::Value::as_u64).and_then(|n| u32::try_from(n).ok());
        let by_path = m
            .iter()
            .filter_map(|(path, v)| Some((path.replace('/', "\\").to_ascii_lowercase(), (crc(v, "crc32original")?, crc(v, "crc32patched")?))))
            .collect();
        ParallaxGenDiff { by_path }
    }

    pub fn len(&self) -> usize {
        self.by_path.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_path.is_empty()
    }

    /// The CRC32 LOD model discovery uses for `model` (Data-relative) with installed bytes of
    /// CRC32 `crc`: the original one when the installed file is the patched copy, else `crc`.
    pub fn discovery_crc(&self, model: &str, crc: u32) -> u32 {
        match self.by_path.get(&model.replace('/', "\\").to_ascii_lowercase()) {
            Some(&(original, patched)) if patched == crc => original,
            _ => crc,
        }
    }
}

/// zlib CRC-32 (the polynomial DynDOLOD prints as `[CRC32:...]`).
pub fn crc32(bytes: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let t = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, e) in t.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
            *e = c;
        }
        t
    });
    let mut c = !0u32;
    for &b in bytes {
        c = t[((c ^ u32::from(b)) & 0xFF) as usize] ^ (c >> 8);
    }
    !c
}

#[cfg(test)]
mod tests {
    use super::*;

    fn index(names: &[&str]) -> LodIndex {
        let paths: Vec<String> = names.iter().map(|n| format!("meshes\\lod\\{n}")).collect();
        LodIndex::scan(paths.iter().map(String::as_str), &["meshes".into()], &[], &Default::default())
    }

    #[test]
    fn level_fill() {
        let i = index(&["impext_lod.nif", "impext_lod_3.nif", "rock_lod_1.nif", "farm_lod.nif", "mc_lod_0h.nif", "mc_lod_0l.nif", "mc_lod_1.nif", "mc_lod_2.nif"]);
        let n = |s: &[Option<String>; 4]| s.iter().map(|v| v.as_deref().map(|p| p.rsplit('\\').next().unwrap().to_string()).unwrap_or_default()).collect::<Vec<_>>();
        assert_eq!(n(&lod_set(&i, "impext", None, [Snow::None; 4], false)), ["impext_lod.nif", "impext_lod_3.nif", "impext_lod_3.nif", "impext_lod_3.nif"]);
        assert_eq!(n(&lod_set(&i, "rock", None, [Snow::None; 4], false)), ["rock_lod_1.nif", "rock_lod_1.nif", "", ""]);
        assert_eq!(n(&lod_set(&i, "farm", None, [Snow::None; 4], false)), ["farm_lod.nif", "farm_lod.nif", "farm_lod.nif", ""]);
        assert_eq!(n(&lod_set(&i, "mc", None, [Snow::Light; 4], false)), ["mc_lod_0l.nif", "mc_lod_1.nif", "mc_lod_2.nif", ""]);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn replaced_meshes_only_take_crc_named_lod_models() {
        let t = MeshCrcTable::parse("// crc32 of meshes\nmeshes\\landscape\\trees\\treepineforest03.nif;9DD6760A\nmeshes\\landscape\\trees\\treepineforest03.nif;9C85B0A8\n");
        let model = "meshes\\landscape\\trees\\treepineforest03.nif";
        assert!(!t.replaced(model, Some(0x9DD6_760A)), "a vanilla CRC is not a replacement");
        assert!(t.replaced(model, Some(0x0B77_42F0)), "another CRC is");
        assert!(!t.replaced("meshes\\mod\\newtree.nif", Some(1)), "meshes the table does not know are never replaced");
        let i = index(&["treepineforest03passthru_lod.nif", "treepineforest03_0b7742f0passthru_lod.nif"]);
        let name = |s: &[Option<String>; 4]| s[0].as_deref().map(|p| p.rsplit('\\').next().unwrap().to_string());
        assert_eq!(name(&lod_set_with(&i, "treepineforest03", Some(0x0B77_42F0), [Snow::None; 4], true, false)).as_deref(), Some("treepineforest03_0b7742f0passthru_lod.nif"));
        assert_eq!(name(&lod_set_with(&i, "treepineforest03", Some(0x1234_5678), [Snow::None; 4], true, false)), None);
        assert_eq!(name(&lod_set_with(&i, "treepineforest03", Some(0x1234_5678), [Snow::None; 4], true, true)).as_deref(), Some("treepineforest03passthru_lod.nif"));
    }

    #[test]
    fn parallaxgen_diff() {
        let json = b"\xEF\xBB\xBF{\n  \"meshes\\\\Architecture\\\\Farm.nif\": { \"crc32original\": 65502835, \"crc32patched\": 400660282 },\n  \"meshes\\\\bad.nif\": { \"crc32original\": 1 }\n}";
        let d = ParallaxGenDiff::parse(json);
        assert_eq!(d.len(), 1);
        // Patched copy installed: the original CRC (0x03E77E73); any other file keeps its own.
        assert_eq!(d.discovery_crc("meshes\\architecture\\farm.nif", 400_660_282), 0x03E7_7E73);
        assert_eq!(d.discovery_crc("Meshes/Architecture/FARM.nif", 400_660_282), 0x03E7_7E73);
        assert_eq!(d.discovery_crc("meshes\\architecture\\farm.nif", 7), 7);
        assert_eq!(d.discovery_crc("meshes\\other.nif", 400_660_282), 400_660_282);
        assert!(ParallaxGenDiff::parse(b"not json").is_empty());
        // The original-CRC variant then wins over the plain one.
        let i = index(&["farm_03e77e73_lod_0.nif", "farm_lod_0.nif"]);
        let crc = d.discovery_crc("meshes\\architecture\\farm.nif", 400_660_282);
        assert_eq!(lod_set(&i, "farm", Some(crc), [Snow::None; 4], false)[0].as_deref(), Some("meshes\\lod\\farm_03e77e73_lod_0.nif"));
        assert_eq!(lod_set(&i, "farm", Some(400_660_282), [Snow::None; 4], false)[0].as_deref(), Some("meshes\\lod\\farm_lod_0.nif"));
    }
}
