//! DynDOLOD's per-reference side files (docs/re/object-lod-spec.md 7.3), read as data:
//!
//! - `LODGen_SSE_AltTextures_<World>[_SEASON].txt`: `<ref id>\t<index>=<shape>=<t0>,...,<t9>,`
//!   entries, one to five per line: texture sets a reference's LOD shapes use instead of the
//!   model's own (alternate textures, PBR). Applied by [`AltTextures::textures_for`].
//! - `LODGen_SSE_VC_<World>[_SEASON].txt`: `<full model>\t<shape>\tTrue|False`, whether a full
//!   model's shape has non-white vertex colours (consumed in stage B2.5).

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub struct AltEntry {
    /// The 0-based shape position the entry names (spec 7.3: the LOD model's shape position for
    /// LOD-shape entries, the full model's for the base's `MODS` entries).
    pub index: i32,
    pub shape: String,
    /// Nine texture slots, as written (lower case, `textures\` prefix), the tenth field dropped.
    pub textures: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct AltTextures {
    pub refs: HashMap<String, Vec<AltEntry>>,
}

impl AltTextures {
    pub fn parse(text: &str) -> AltTextures {
        let mut a = AltTextures::default();
        for raw in text.lines() {
            let line = raw.trim_end_matches('\r');
            let mut cols = line.split('\t');
            let Some(id) = cols.next() else { continue };
            let id = id.trim().to_ascii_uppercase();
            if id.len() != 8 || !id.bytes().all(|c| c.is_ascii_hexdigit()) {
                continue;
            }
            let entries = a.refs.entry(id).or_default();
            for e in cols {
                let mut parts = e.splitn(3, '=');
                let (Some(i), Some(name), Some(tex)) = (parts.next(), parts.next(), parts.next()) else { continue };
                let Ok(index) = i.trim().parse::<i32>() else { continue };
                let mut textures: Vec<String> = tex.split(',').map(|t| t.trim().to_ascii_lowercase()).collect();
                textures.truncate(9);
                textures.resize(9, String::new());
                entries.push(AltEntry { index, shape: name.to_string(), textures });
            }
        }
        a
    }

    pub fn is_empty(&self) -> bool {
        self.refs.is_empty()
    }

    /// The texture set for shape `index` (named `shape`) of a reference's LOD model: an entry
    /// whose index and name both match, else one whose name matches (the index then refers to
    /// the full model's shapes, spec 7.3); `None` keeps the model's own textures.
    pub fn textures_for(&self, ref_id: &str, index: usize, shape: &str) -> Option<&[String]> {
        let entries = self.refs.get(&ref_id.to_ascii_uppercase())?;
        if let Some(e) = entries.iter().find(|e| e.index == index as i32 && e.shape.eq_ignore_ascii_case(shape)) {
            return Some(&e.textures);
        }
        entries.iter().find(|e| e.shape.eq_ignore_ascii_case(shape)).map(|e| &e.textures[..])
    }
}

/// `VC` side file: (full model lower case, shape name) -> has non-white vertex colours.
#[derive(Debug, Clone, Default)]
pub struct VcList {
    pub shapes: HashMap<(String, String), bool>,
    /// Per full model: whether any of its shapes is `True`.
    pub models: HashMap<String, bool>,
}

/// What the VC side file says about a LOD shape's colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VcVerdict {
    /// The full model is not in the file: keep the model's colours.
    Unknown,
    /// Whiten: the full model's shape of that name is listed `False`. A LOD shape whose name
    /// is not listed keeps its colours (DynDOLOD golden2 `Tamriel.4.16.12.bto`: the model
    /// `rockcaveentrance01.nif` lists `:0`, `:3`, `:8` as `False`, and the LOD shape
    /// `RockCaveEntrance01:1 - L2_Rocks:1` keeps its colours in the golden).
    White,
    /// Keep.
    Colored,
}

impl VcList {
    pub fn parse(text: &str) -> VcList {
        let mut v = VcList::default();
        for raw in text.lines() {
            let c: Vec<&str> = raw.trim_end_matches('\r').split('\t').collect();
            if c.len() < 3 {
                continue;
            }
            let model = c[0].trim().to_ascii_lowercase();
            let colored = c[2].trim().eq_ignore_ascii_case("true");
            *v.models.entry(model.clone()).or_insert(false) |= colored;
            v.shapes.insert((model, c[1].trim().to_string()), colored);
        }
        v
    }

    pub fn get(&self, model: &str, shape: &str) -> Option<bool> {
        self.shapes.get(&(model.to_ascii_lowercase(), shape.to_string())).copied()
    }

    pub fn verdict(&self, model: &str, shape: &str) -> VcVerdict {
        let m = model.to_ascii_lowercase();
        if !self.models.contains_key(&m) {
            return VcVerdict::Unknown;
        }
        match self.shapes.get(&(m, shape.to_string())) {
            Some(false) => VcVerdict::White,
            Some(true) => VcVerdict::Colored,
            None => VcVerdict::Unknown,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.shapes.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alt_textures() {
        let t = "00013304\t0=DirtCliffs01:0=textures\\a.dds,textures\\a_n.dds,,,,,,,,\t1=DirtCliffs01:1=textures\\b.dds,textures\\b_n.dds,,,,textures\\b_rmaos.dds,,,,\r\n00013AA4\t2=MountainTrim01Wet:2=textures\\w.dds,,,,,,,,,\t1=MountainTrim01Dirt_LOD_2:3=textures\\d.dds,,,,,,,,,\r\nnot a line\r\n";
        let a = AltTextures::parse(t);
        assert_eq!(a.refs.len(), 2);
        let e = a.textures_for("00013304", 1, "DirtCliffs01:1").unwrap();
        assert_eq!(e[0], "textures\\b.dds");
        assert_eq!(e[5], "textures\\b_rmaos.dds");
        assert_eq!(e.len(), 9);
        // Name match wins over a stale index.
        assert_eq!(a.textures_for("00013AA4", 1, "MountainTrim01Dirt_LOD_2:3").unwrap()[0], "textures\\d.dds");
        assert_eq!(a.textures_for("00013AA4", 0, "mountaintrim01dirt_lod_2:3").unwrap()[0], "textures\\d.dds");
        assert!(a.textures_for("00013AA4", 0, "Other").is_none());
        assert!(a.textures_for("00000001", 0, "x").is_none());
        let v = VcList::parse("meshes\\architecture\\farmhouse\\farmhouse02.nif\tL2_Rope:18\tTrue\r\nmeshes\\x.nif\tSkirt\tFalse\r\n");
        assert_eq!(v.get("Meshes\\Architecture\\Farmhouse\\Farmhouse02.nif", "L2_Rope:18"), Some(true));
        assert_eq!(v.get("meshes\\x.nif", "Skirt"), Some(false));
        assert_eq!(v.get("meshes\\x.nif", "Other"), None);
        assert_eq!(v.verdict("meshes\\x.nif", "Skirt"), VcVerdict::White);
        assert_eq!(v.verdict("meshes\\x.nif", "Other"), VcVerdict::Unknown);
        assert_eq!(v.verdict("meshes\\architecture\\farmhouse\\farmhouse02.nif", "L2_Rope:18"), VcVerdict::Colored);
        assert_eq!(v.verdict("meshes\\architecture\\farmhouse\\farmhouse02.nif", "Other"), VcVerdict::Unknown);
        assert_eq!(v.verdict("meshes\\none.nif", "x"), VcVerdict::Unknown);
    }
}
