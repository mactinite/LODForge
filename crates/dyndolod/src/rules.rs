//! DynDOLOD mesh-mask / reference rules (docs/re/dyndolod-spec.md "Rules and mesh masks").
//!
//! A rule is `mask,LOD4,LOD8,LOD16[,LOD32],Grid,Reference,Flags`. Rule files are merged by plain
//! concatenation: `_all` → per-plugin files in load order (`<plugin>_<Preset>.ini`, else
//! `<plugin>.ini`) → `_all_dynamic` / `_all_static` → the base preset. The first applicable rule
//! wins; reference (FormID) rules beat every mask rule.

use std::path::{Path, PathBuf};

use crate::ini::Ini;

/// What a rule puts into one LOD level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    None,
    /// Model level 0..3 from LOD model discovery.
    Model(u8),
    /// The full model.
    Full,
    /// Tree billboard template 1..6.
    Billboard(u8),
}

impl Level {
    pub fn parse(s: &str) -> Level {
        let t = s.trim();
        let lower = t.to_ascii_lowercase();
        let compact: String = lower.chars().filter(|c| !c.is_whitespace()).collect();
        match compact.as_str() {
            "" | "none" => Level::None,
            "staticlod4" | "level0" => Level::Model(0),
            "staticlod8" | "level1" => Level::Model(1),
            "staticlod16" | "level2" => Level::Model(2),
            "staticlod32" | "level3" => Level::Model(3),
            "full" | "fullmodel" => Level::Full,
            "billboard" => Level::Billboard(1),
            _ => match compact.strip_prefix("billboard").and_then(|n| n.parse::<u8>().ok()) {
                Some(n @ 1..=6) => Level::Billboard(n),
                _ => Level::None,
            },
        }
    }

    /// The preset file's spelling.
    pub fn name(self) -> String {
        match self {
            Level::None => "None".into(),
            Level::Model(k) => format!("Level{k}"),
            Level::Full => "Full".into(),
            Level::Billboard(n) => format!("Billboard{n}"),
        }
    }
}

/// Dynamic LOD grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grid {
    None,
    NearLod,
    NearFull,
    FarLod,
    FarFull,
    NeverFadeLod,
    NeverFadeFull,
}

impl Grid {
    pub fn parse(s: &str) -> Grid {
        let c: String = s.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_ascii_lowercase();
        match c.as_str() {
            "nearlod" => Grid::NearLod,
            "nearfull" => Grid::NearFull,
            "farlod" => Grid::FarLod,
            "farfull" => Grid::FarFull,
            "neverfadelod" => Grid::NeverFadeLod,
            "neverfadefull" => Grid::NeverFadeFull,
            _ => Grid::None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Grid::None => "None",
            Grid::NearLod => "NearLOD",
            Grid::NearFull => "NearFull",
            Grid::FarLod => "FarLOD",
            Grid::FarFull => "FarFull",
            Grid::NeverFadeLod => "NeverFadeLOD",
            Grid::NeverFadeFull => "NeverFadeFull",
        }
    }
}

pub const FLAG_VWD: u32 = 1;
pub const FLAG_NO_GLOW: u32 = 2;
pub const FLAG_NO_MATO: u32 = 4;
pub const FLAG_DYNAMIC: u32 = 8;
pub const FLAG_TREE: u32 = 16;
pub const FLAG_MAP: u32 = 32;

#[derive(Debug, Clone, PartialEq)]
pub struct Rule {
    /// Mesh mask as written (substring of the model path below `meshes\`), or the reference
    /// rule's `plugin;FORMID` text.
    pub mask: String,
    /// Lower-case mask for matching.
    pub mask_lc: String,
    /// Reference rule target: (plugin file name lower-case, local form id).
    pub reference: Option<(String, u32)>,
    pub levels: [Level; 4],
    pub grid: Grid,
    pub reference_action: String,
    pub flags: u32,
    /// The rule file it came from.
    pub source: String,
}

impl Rule {
    /// Parses a rule value (`mask,LOD4,LOD8,LOD16[,LOD32],Grid,Reference,Flags`).
    pub fn parse(value: &str, source: &str) -> Option<Rule> {
        let f: Vec<&str> = value.split(',').collect();
        if f.len() < 7 {
            return None;
        }
        let (levels, rest) = if f.len() >= 8 {
            ([Level::parse(f[1]), Level::parse(f[2]), Level::parse(f[3]), Level::parse(f[4])], &f[5..])
        } else {
            let l16 = Level::parse(f[3]);
            ([Level::parse(f[1]), Level::parse(f[2]), l16, Level::None], &f[4..])
        };
        let flags_text = rest.get(2).copied().unwrap_or("0");
        let digits: String = flags_text.trim().chars().take_while(|c| c.is_ascii_digit()).collect();
        let mask_raw = f[0].trim();
        let reference = mask_raw.split_once(';').and_then(|(p, id)| u32::from_str_radix(id.trim(), 16).ok().map(|id| (p.trim().to_ascii_lowercase(), id & 0x00FF_FFFF)));
        Some(Rule {
            mask: mask_raw.to_string(),
            mask_lc: mask_raw.to_ascii_lowercase(),
            reference,
            levels,
            grid: Grid::parse(rest[0]),
            reference_action: if rest[1].trim().is_empty() { "Unchanged".into() } else { rest[1].trim().to_string() },
            flags: digits.parse().unwrap_or(0),
            source: source.to_string(),
        })
    }

    /// The preset file line value (`mask,LOD4,LOD8,LOD16,LOD32,Grid,Reference,Flags,source`).
    pub fn preset_value(&self) -> String {
        format!(
            "{},{},{},{},{},{},{},{},{}",
            self.mask,
            self.levels[0].name(),
            self.levels[1].name(),
            self.levels[2].name(),
            self.levels[3].name(),
            self.grid.name(),
            self.reference_action,
            self.flags,
            self.source
        )
    }

    /// True when the mask matches a model path relative to `meshes\` (lower-case, backslashes).
    pub fn matches_model(&self, model_lower: &str) -> bool {
        self.reference.is_none() && model_lower.contains(&self.mask_lc)
    }

    pub fn has_levels(&self) -> bool {
        self.levels.iter().any(|l| *l != Level::None)
    }
}

/// The first mask rule that applies to a base: its mask is a substring of the model path below
/// `meshes\` (lower-case), or it has the TREE flag and the base is a tree; a VWD rule applies
/// only when `has_lod` (a LOD model or billboard exists for the base).
///
/// `available(level)` says whether the resource a level choice needs exists (a model level, the
/// billboard); a VWD rule applies only if one of its level choices is available. A no-level/
/// no-grid rule blocks full-model fallback, but does not suppress discovered LOD resources.
///
/// A grid-only rule (dynamic LOD, no levels, `Unchanged`) passes a base that has LOD models on to
/// the next matching rule for its levels (golden Ultra report, skyrim-cs-base 2026-10-22:
/// `SovngardeStatue01` matches `sovngardestatue0,,,,Far LOD,Unchanged` and then the catch-all
/// `\,Static LOD4,Static LOD8,Static LOD16,Far LOD`, and its references are static object LOD
/// with `sovngardestatue01_lod.nif`; `ShipCaptainsBroken01`, without LOD models, stays on its
/// grid-only rule as dynamic LOD with the full model).
pub fn select_mask_rule<'a>(rules: &'a [Rule], model_below_meshes: &str, is_tree: bool, available: &dyn Fn(Level) -> bool) -> Option<&'a Rule> {
    let has_models = [Level::Model(0), Level::Model(1), Level::Model(2), Level::Model(3)].into_iter().any(available);
    let applies = |r: &Rule| -> bool {
        if r.reference.is_some() {
            return false;
        }
        if !(r.matches_model(model_below_meshes) || (is_tree && r.flags & FLAG_TREE != 0)) {
            return false;
        }
        if r.flags & FLAG_VWD != 0 {
            return r.levels.iter().any(|&l| l != Level::None && available(l)) || (!r.has_levels() && [Level::Model(0), Level::Model(1), Level::Model(2), Level::Model(3)].into_iter().any(available));
        }
        r.has_levels() || r.grid != Grid::None
            || ![Level::Model(0), Level::Model(1), Level::Model(2), Level::Model(3), Level::Billboard(1)].into_iter().any(available)
    };
    let first = rules.iter().position(applies)?;
    let r = &rules[first];
    if !r.has_levels() && r.grid != Grid::None && has_models && r.flags & FLAG_VWD == 0 && r.reference_action.eq_ignore_ascii_case("Unchanged")
        && let Some(next) = rules[first + 1..].iter().find(|n| applies(n) && n.has_levels())
    {
        return Some(next);
    }
    Some(r)
}

/// The reference rule for a reference (plugin that defines it, lower-case, and local form id).
pub fn select_reference_rule<'a>(rules: &'a [Rule], plugin: &str, local_id: u32) -> Option<&'a Rule> {
    rules.iter().find(|r| r.reference.as_ref().is_some_and(|(p, id)| p == plugin && *id == local_id))
}

/// Plugin file name as it appears in per-plugin rule file names: lower-case, everything but
/// ASCII letters, digits and `_` removed.
pub fn plugin_key(name: &str) -> String {
    name.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').collect::<String>().to_ascii_lowercase()
}

/// Where rule files are read from: `Data\DynDOLOD\<name>` (through the VFS) before
/// `Edit Scripts\DynDOLOD\Rules\<name>`.
pub struct RuleSource<'a> {
    pub rules_dir: PathBuf,
    pub data_override: &'a dyn Fn(&str) -> Option<String>,
}

impl RuleSource<'_> {
    /// (display path, content) of a rule file, matched case-insensitively.
    fn open(&self, name: &str) -> Option<(String, String)> {
        if let Some(t) = (self.data_override)(&format!("dyndolod\\{name}")) {
            return Some((format!("Data\\DynDOLOD\\{name}"), t));
        }
        let path = self.rules_dir.join(name);
        let found = if path.exists() { Some(path) } else { find_ci(&self.rules_dir, name) };
        let p = found?;
        let bytes = std::fs::read(&p).ok()?;
        Some((self.rules_dir.join(name).to_string_lossy().replace('/', "\\"), crate::ini::decode(&bytes)))
    }
}

fn find_ci(dir: &Path, name: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir).ok()?.filter_map(|e| e.ok()).find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(name)).map(|e| e.path())
}

/// The merged rule table for a game (`SSE`), preset (`Medium`) and load order.
pub fn build_rules(src: &RuleSource<'_>, game: &str, preset: &str, dynamic: bool, plugins: &[String]) -> Vec<Rule> {
    let loaded: std::collections::HashSet<String> = plugins.iter().map(|p| p.to_ascii_lowercase()).collect();
    let mut rules: Vec<Rule> = Vec::new();
    let add_file = |name: &str, rules: &mut Vec<Rule>| {
        let Some((display, text)) = src.open(name) else { return };
        let ini = Ini::parse(&text);
        for s in &ini.sections {
            let Some(target) = s.name.strip_prefix("Skyrim ") else { continue };
            if !(target.eq_ignore_ascii_case("LODGen") || loaded.contains(&target.to_ascii_lowercase())) {
                continue;
            }
            for (k, v) in &s.entries {
                if !k.to_ascii_lowercase().starts_with("lodgen") {
                    continue;
                }
                if let Some(r) = Rule::parse(v, &display) {
                    rules.push(r);
                }
            }
        }
    };
    add_file(&format!("DynDOLOD_{game}_all.ini"), &mut rules);
    for p in plugins {
        let key = plugin_key(p);
        let with_preset = format!("DynDOLOD_{game}_{key}_{preset}.ini");
        if src.open(&with_preset).is_some() {
            add_file(&with_preset, &mut rules);
        } else {
            add_file(&format!("DynDOLOD_{game}_{key}.ini"), &mut rules);
        }
    }
    add_file(&format!("DynDOLOD_{game}_all_{}.ini", if dynamic { "dynamic" } else { "static" }), &mut rules);
    add_file(&format!("DynDOLOD_{game}_{preset}.ini"), &mut rules);
    rules
}

/// Rules as saved in a preset file's `[<Game> LODGen]` section.
pub fn rules_from_preset(ini: &Ini, game_name: &str) -> Vec<Rule> {
    let mut out = Vec::new();
    for s in ini.sections_named(&format!("{game_name} LODGen")) {
        for (_, v) in &s.entries {
            let (value, source) = match v.match_indices(',').nth(7) {
                Some((i, _)) => (&v[..i], &v[i + 1..]),
                None => (v.as_str(), ""),
            };
            if let Some(r) = Rule::parse(value, source) {
                out.push(r);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_seven_and_eight_fields() {
        let r = Rule::parse("tree,Billboard,Billboard,Billboard,Near LOD,Unchanged,17", "x").unwrap();
        assert_eq!(r.preset_value(), "tree,Billboard1,Billboard1,Billboard1,None,NearLOD,Unchanged,17,x");
        let r = Rule::parse("dbm resources\\landscape\\slabpillar01.nif,Full model,Full model,,Far LOD,Unchanged,0", "x").unwrap();
        assert_eq!(r.levels, [Level::Full, Level::Full, Level::None, Level::None]);
        let r = Rule::parse("skyrim.esm;000EE66D,,,,,Delete,0", "x").unwrap();
        assert_eq!(r.reference, Some(("skyrim.esm".into(), 0xEE66D)));
        assert_eq!(r.grid, Grid::None);
        assert_eq!(plugin_key("Unofficial Skyrim Special Edition Patch.esp"), "unofficialskyrimspecialeditionpatchesp");
        assert_eq!(plugin_key("_ResourcePack.esl"), "_resourcepackesl");
    }
}
