//! Per-base LOD assignment: discovered model levels, the applicable mask rule and the LODGen
//! export columns (LOD4, LOD8, LOD16, LOD32) it produces.

use std::collections::HashMap;

use crate::bases::{Base, model_path};
use crate::lodmodels::{LodIndex, ParallaxGenDiff, Snow, crc32};
use crate::rules::{Level, Rule, select_mask_rule};

/// STAT record flag "Has Tree LOD".
pub const STAT_HAS_TREE_LOD: u32 = 0x40;

#[derive(Debug, Clone, Default)]
pub struct BaseLod {
    /// Data-relative full model path, lower-case (`meshes\...`).
    pub model: String,
    /// Model levels 0..3 from discovery (lower-case paths).
    pub levels: [Option<String>; 4],
    /// TexGen billboard texture for trees (lower-case).
    pub billboard: Option<String>,
    /// Index of the applicable mask rule.
    pub rule: Option<usize>,
    /// Export columns for LOD4, LOD8, LOD16, LOD32 (empty = none).
    pub columns: [String; 4],
}

pub struct Resolver<'a> {
    pub rules: &'a [Rule],
    pub index: &'a LodIndex,
    /// PGPatcher CRC map for `<stem>_<CRC32>_lod` discovery.
    pub parallax: &'a ParallaxGenDiff,
    pub materials: &'a HashMap<u32, String>,
    /// Plugin file names in load order (for billboard folder names).
    pub plugins: &'a [String],
    /// [`crate::bases::owner_key`] -> load-order index.
    pub owners: &'a HashMap<u32, usize>,
    pub read: &'a (dyn Fn(&str) -> Option<Vec<u8>> + Sync),
    pub exists: &'a (dyn Fn(&str) -> bool + Sync),
    pub level32: bool,
    /// DynDOLOD's vanilla mesh CRCs (replaced meshes only take `<stem>_<crc>` LOD models).
    pub mesh_crc: &'a crate::lodmodels::MeshCrcTable,
}

/// The `h` / `l` LOD model variant per level: what the base's MNAM assigns (`..._LOD_0H.nif`),
/// else heavy for a directional material and light for one whose name contains "light".
pub fn snow_of(b: &Base, materials: &HashMap<u32, String>) -> [Snow; 4] {
    let m = materials.get(&b.material).map(|s| s.to_ascii_lowercase());
    let fallback = match m {
        Some(m) if m.contains("light") => Snow::Light,
        Some(_) => Snow::Heavy,
        None => Snow::None,
    };
    let mut out = [fallback; 4];
    if let Some(mnam) = &b.mnam {
        for (k, s) in out.iter_mut().enumerate() {
            let f = mnam[k].to_ascii_lowercase();
            if f.ends_with(&format!("_lod_{k}h.nif")) {
                *s = Snow::Heavy;
            } else if f.ends_with(&format!("_lod_{k}l.nif")) {
                *s = Snow::Light;
            } else if !f.is_empty() {
                *s = Snow::None;
            }
        }
    }
    out
}

pub fn is_tree(b: &Base) -> bool {
    &b.signature == b"TREE" || (&b.signature == b"STAT" && b.flags & STAT_HAS_TREE_LOD != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_mnam_alias_is_a_fallback_not_a_discovery_override() {
        let paths = ["meshes\\other_lod.nif", "meshes\\hall_lod_0.nif"];
        let mut base = Base { flags: 0x8000, signature: *b"STAT", model: "hall.nif".into(),
            mnam: Some(["other_lod.nif".into(), String::new(), String::new(), String::new()]), ..Default::default() };
        let resolve = |base: &Base, discovered: bool, exists: bool| {
            let index = LodIndex::scan(paths.iter().copied().filter(|p| discovered && p.contains("hall_lod")), &["meshes".into()], &[], &Default::default());
            let parallax = Default::default();
            let materials = HashMap::new();
            let owners = HashMap::new();
            Resolver { rules: &[], index: &index, parallax: &parallax, materials: &materials,
                plugins: &[], owners: &owners, read: &|_| None, exists: &|p| exists && paths.contains(&p), level32: false, mesh_crc: &crate::lodmodels::MeshCrcTable::default() }.resolve(base)
        };
        assert_eq!(resolve(&base, false, true).levels[0].as_deref(), Some("meshes\\other_lod.nif"));
        assert_eq!(resolve(&base, true, true).levels[0].as_deref(), Some("meshes\\hall_lod_0.nif"));
        assert!(resolve(&base, false, false).levels.iter().all(Option::is_none));
        base.flags = 0;
        assert!(resolve(&base, false, true).levels.iter().all(Option::is_none));
    }
}

impl Resolver<'_> {
    /// `textures\terrain\lodgen\<plugin>\<model stem>_<formid8>.dds` when it exists.
    pub fn billboard(&self, b: &Base, stem: &str) -> Option<String> {
        let mi = *self.owners.get(&crate::bases::owner_key(b.form_id))?;
        let plugin = self.plugins.get(mi)?.to_ascii_lowercase();
        let local = if b.form_id >> 24 == 0xFE { b.form_id & 0xFFF } else { b.form_id & 0x00FF_FFFF };
        let path = format!("textures\\terrain\\lodgen\\{plugin}\\{stem}_{local:08x}.dds");
        (self.exists)(&path).then_some(path)
    }

    /// CRC32 of the installed full model as LOD model discovery uses it (the original CRC for a
    /// PGPatcher-patched mesh). None when the file cannot be read.
    pub fn model_crc(&self, model: &str) -> Option<u32> {
        (self.read)(model).map(|bytes| self.parallax.discovery_crc(model, crc32(&bytes)))
    }

    pub fn resolve(&self, b: &Base) -> BaseLod {
        let model = model_path(&b.model);
        let stem = model.rsplit('\\').next().unwrap_or(&model).trim_end_matches(".nif").to_string();
        let tree = is_tree(b);
        let crc = self.model_crc(&model);
        let mut levels = crate::lodmodels::lod_set_with(self.index, &stem, crc, snow_of(b, self.materials), true, !self.mesh_crc.replaced(&model, crc));
        // A full model that is itself named `_lod` is its own LOD model (the bare name may
        // resolve to a copy in a later LOD folder).
        if levels.iter().all(Option::is_none) && stem.contains("_lod")
            && let Some(p) = self.index.get(&format!("{stem}.nif"))
        {
            levels = [Some(p.to_string()), Some(p.to_string()), Some(p.to_string()), None];
        }
        // Native LOD assignments can intentionally use another model's stem (e.g. CC halls).
        // Keep filename/CRC discovery precedence, but do not drop valid MNAM-only LOD.
        if levels.iter().all(Option::is_none) && b.flags & 0x8000 != 0
            && let Some(mnam) = &b.mnam
        {
            for (k, m) in mnam.iter().enumerate() {
                let path = crate::bases::model_path(m);
                if !m.is_empty() && (self.exists)(&path) {
                    levels[k] = Some(path);
                }
            }
        }
        // The base record's own MNAM assignment wins where it names an existing file.
        if std::env::var_os("LODFORGE_NO_MNAM").is_none() && (b.material == 0 || std::env::var_os("LODFORGE_MNAM_ALL").is_some() || snow_of(b, self.materials)[0] == Snow::Light)
            && let Some(mnam) = &b.mnam
        {
            for (k, m) in mnam.iter().enumerate() {
                if m.is_empty() || k == 0 {
                    continue;
                }
                let p = crate::bases::model_path(m);
                let same_model = p.rsplit('\\').next().is_some_and(|n| n.starts_with(&format!("{stem}_lod")));
                let keep_passthru = levels[k].as_deref().is_some_and(|l| l.contains("passthru_lod"));
                if same_model && !keep_passthru && (self.exists)(&p) {
                    levels[k] = Some(p);
                }
            }
        }
        let billboard = if tree { self.billboard(b, &stem) } else { None };
        let below = model.strip_prefix("meshes\\").unwrap_or(&model);
        let available = |l: Level| match l {
            Level::None => false,
            Level::Model(k) => levels[usize::from(k)].is_some(),
            Level::Full => true,
            Level::Billboard(_) => billboard.is_some(),
        };
        let rule = select_mask_rule(self.rules, below, tree, &available).map(|r| self.rules.iter().position(|x| std::ptr::eq(x, r)).unwrap());
        let mut columns: [String; 4] = Default::default();
        if let Some(ri) = rule {
            let r = &self.rules[ri];
            let full = crate::export::export_model_text(&b.model);
            for (l, col) in columns.iter_mut().enumerate() {
                let choice = if l == 3 && r.levels[3] == Level::None { r.levels[2] } else { r.levels[l] };
                if l == 3 && !self.level32 {
                    continue;
                }
                *col = match choice {
                    Level::None => String::new(),
                    Level::Model(k) => levels[usize::from(k)].clone().unwrap_or_default(),
                    Level::Full => full.clone(),
                    Level::Billboard(_) => billboard.clone().unwrap_or_default(),
                };
            }
        }
        BaseLod { model, levels, billboard, rule, columns }
    }
}

impl Resolver<'_> {
    /// Glow window columns: for each level column the normal line uses, the smallest existing
    /// `<model>glow_lod_<j>.nif` with j at least the rule's model level there (golden: a base with
    /// only `glow_lod_0` glows in LOD4 only; `glow_lod_1` covers LOD4 and LOD8, and LOD16 where the
    /// rule asks for level 1 or less there). None without any.
    pub fn glow_columns(&self, r: &Rule, b: &Base, normal: &[String; 4]) -> Option<[String; 4]> {
        // Named after the LOD model the normal line uses, without its last `_lod...` suffix
        // (`sbardscollege_lod_lod.nif` -> `sbardscollege_lodglow_lod_2.nif`, `sbluepalace_lod.nif`
        // -> `sbluepalaceglow_lod_2.nif`).
        let _ = b;
        let first = normal.iter().find(|c| !c.is_empty())?.to_ascii_lowercase();
        let name = first.rsplit('\\').next().unwrap_or(&first).trim_end_matches(".nif");
        let stem = name.rsplit_once("_lod").map_or(name, |(s, _)| s);
        let glow: [Option<&str>; 4] = std::array::from_fn(|j| self.index.get(&format!("{stem}glow_lod_{j}.nif")));
        if glow.iter().all(Option::is_none) {
            return None;
        }
        let mut out: [String; 4] = Default::default();
        for (l, col) in out.iter_mut().enumerate() {
            if normal[l].is_empty() {
                continue;
            }
            let choice = if l == 3 && r.levels[3] == Level::None { r.levels[2] } else { r.levels[l] };
            let k = match choice {
                Level::Model(k) => usize::from(k),
                Level::Full => 0,
                _ => continue,
            };
            if let Some(p) = glow[k..].iter().flatten().next() {
                *col = p.to_string();
            }
        }
        out.iter().any(|c| !c.is_empty()).then_some(out)
    }

    /// The export columns a rule gives a base.
    pub fn columns(&self, r: &Rule, b: &Base, lod: &BaseLod) -> [String; 4] {
        let mut columns: [String; 4] = Default::default();
        for (l, col) in columns.iter_mut().enumerate() {
            if l == 3 && !self.level32 {
                continue;
            }
            let choice = if l == 3 && r.levels[3] == Level::None { r.levels[2] } else { r.levels[l] };
            *col = match choice {
                Level::None => String::new(),
                Level::Model(k) => lod.levels[usize::from(k)].clone().unwrap_or_default(),
                Level::Full => crate::export::export_model_text(&b.model),
                Level::Billboard(_) => lod.billboard.clone().unwrap_or_default(),
            };
        }
        columns
    }
}

/// The full model as DynDOLOD writes it in the export: as stored, with a `Meshes\` prefix.
pub fn full_model_text(model: &str) -> String {
    let m = model.replace('/', "\\");
    let m = m.trim_start_matches('\\');
    if m.to_ascii_lowercase().starts_with("meshes\\") { m.to_string() } else { format!("Meshes\\{m}") }
}
