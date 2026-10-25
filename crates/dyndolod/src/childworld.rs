//! Child worldspace copies (`<plugin>&<id>_DynDOLOD_CHILD`): references of a child worldspace
//! (a walled city) copied into its parent so the parent's LOD shows them.
//!
//! Selection, as DynDOLOD 3 does it (golden: its debug log and `ChildworldCopies_<World>.txt`):
//! child worldspaces with a `Configs\DynDOLOD_SSE_childworld_<Child>.ini` section `[<Parent>]`
//! whose `ScanChild=1` (skipped when a `NoScanIfPluginExists` / `NoScanChildIfPluginExists`
//! plugin is loaded); every reference there except those whose base EditorID contains an
//! `IgnoreChildEDID<Preset>` entry, those initially disabled through their enable parents, those listed as the child side of a known pair in
//! `DynDOLOD_SSE_ChildworldMatches.txt`, duplicates inside the child and twins of a parent
//! reference (see [`ref_key`]). Whether a copy gets LOD at all is the export's rule resolution.
//!
//! Duplicates and twins (golden Ultra run, skyrim-cs-base 2026-10-22; derived from the debug log,
//! no reverse engineering): DynDOLOD keys every reference as
//! `<model>_<x/100>_<y/100>_<z/100>_<scale %>` (rounded). Inside one child worldspace a later
//! reference with the key and rotation of an earlier one is dropped ("Duplicate reference ignored
//! for LOD ... adds same model at same position, rotation and scale": Haelga's Bunkhouse
//! `FE063817` duplicates JKs Skyrim `13000EAA` in Riften). A child reference whose key equals a
//! parent reference's is a twin and not copied (Pawned Prawn `FE07E800` / Tamriel `FE07E80A`,
//! 9 units apart; JKs Skyrim `130147D2` / `13014D59`), but each parent reference takes at most
//! one child twin, and one taken by a known pair takes none (JK's Temple of the Divines
//! `FE0798F8` has the key of Tamriel `000F4FEE`, which `ChildworldMatches.txt` pairs with
//! Solitude's `000E73A4`, and is copied).

use std::collections::{HashMap, HashSet};
use std::path::Path;

use lodforge_esp::{LoadOrder, Placement, Worldspace};

use crate::bases::{Base, owner_key, owners};
use crate::ini::{Ini, decode};

/// Settings of one child worldspace for its parent.
#[derive(Debug, Clone, Default)]
pub struct ChildSettings {
    pub scan_child: bool,
    /// `IgnoreChildEDID<Preset>`, lower-case substrings of base EditorIDs.
    pub ignore_edid: Vec<String>,
    /// Plugins whose references `mod_world_ignore.txt` ignores in this child (lower-case).
    pub ignore_plugins: HashSet<String>,
}

/// `Configs\DynDOLOD_SSE_mod_world_ignore.txt` ("ignore all references added by mod to
/// worldspace"): `plugin=World,World` lines, `//` comments. A reference in a listed worldspace is
/// ignored when the listed plugin defines it or holds its winning record (golden Ultra run,
/// skyrim-cs-base 2026-10-22: `JKs Skyrim.esp=MarkarthWorld,SolitudeWorld,WindhelmWorld` skipped
/// 1329 references JK's Skyrim adds there, 24 Skyrim.esm ones it overrides and 6 of its own
/// that a patch overrides; its Whiterun and Riften references were copied).
#[derive(Debug, Clone, Default)]
pub struct ModWorldIgnore(pub HashMap<String, HashSet<String>>);

impl ModWorldIgnore {
    pub fn parse(text: &str) -> ModWorldIgnore {
        let mut m: HashMap<String, HashSet<String>> = HashMap::new();
        for line in text.lines() {
            let l = line.trim();
            if l.is_empty() || l.starts_with("//") || l.starts_with(';') {
                continue;
            }
            if let Some((plugin, worlds)) = l.split_once('=') {
                let set = m.entry(plugin.trim().to_ascii_lowercase()).or_default();
                set.extend(worlds.split(',').map(|w| w.trim().to_ascii_lowercase()).filter(|w| !w.is_empty()));
            }
        }
        ModWorldIgnore(m)
    }

    pub fn read(dir: &Path) -> ModWorldIgnore {
        std::fs::read(crate::overlay::path(dir, r"Configs\DynDOLOD_SSE_mod_world_ignore.txt")).map(|b| ModWorldIgnore::parse(&decode(&b))).unwrap_or_default()
    }

    /// The plugins (lower-case) whose references are ignored in worldspace `world` (EditorID).
    pub fn plugins_for(&self, world: &str) -> HashSet<String> {
        let w = world.to_ascii_lowercase();
        self.0.iter().filter(|(_, ws)| ws.contains(&w)).map(|(p, _)| p.clone()).collect()
    }
}

/// Whether a reference defined by `owner` whose winning record is in `winner` (both lower-case)
/// is ignored by a [`ModWorldIgnore::plugins_for`] set.
pub fn mod_world_ignored(ignore: &HashSet<String>, owner: &str, winner: &str) -> bool {
    !ignore.is_empty() && (ignore.contains(owner) || ignore.contains(winner))
}

/// One reference to copy into the parent worldspace.
#[derive(Debug, Clone)]
pub struct ChildCopy {
    /// The child worldspace's form id and the original reference.
    pub child_world: u32,
    pub ref_id: u32,
    pub placement: Placement,
    /// Id the copy uses in the parent's export (`COPY_ID_BASE | n`), and so in DynDOLOD.esm.
    pub copy_id: u32,
    /// Listed in `childworldfull.txt`: the copy keeps the original base (full model).
    pub full_model: bool,
    /// A large reference in DynDOLOD.esm (WRLD RNAM) and so `-LargeRef` in the export: not a
    /// tree, scaled bounds at least [`LARGE_MIN_EXTENT`] on some axis (golden: 30 of 199; a few
    /// Windhelm civil-war intact/destroyed pairs disagree).
    pub large: bool,
}

/// Smallest scaled bounds extent of a large copy (golden Tamriel: large from 928, small up to 886).
pub const LARGE_MIN_EXTENT: f32 = 900.0;

/// DynDOLOD's reference key: model path (lower case, without `meshes\`), position / 100 and
/// scale in percent, each rounded half to even (Delphi `Round`; the debug log prints
/// `meshes\architecture\riften\rtpawnedprawn01.nif_1716_-952_111_100`).
pub fn ref_key(model: &str, p: &Placement) -> (String, [i64; 4]) {
    let m = model.to_ascii_lowercase().replace('/', "\\");
    let m = m.strip_prefix("meshes\\").unwrap_or(&m).to_string();
    let scale = if p.has_scale { p.scale } else { 1.0 };
    let r = |v: f64| v.round_ties_even() as i64;
    (m, [r(f64::from(p.position[0]) / 100.0), r(f64::from(p.position[1]) / 100.0), r(f64::from(p.position[2]) / 100.0), r(f64::from(scale) * 100.0)])
}

/// Rotation in whole degrees, for the in-child duplicate check.
fn rot_key(p: &Placement) -> [i64; 3] {
    p.rotation.map(|v| (f64::from(v).to_degrees().rem_euclid(360.0)).round() as i64 % 360)
}

/// References (`plugin` lower-case, object id) of `Configs\DynDOLOD_SSE_childworldfull.txt`.
pub fn full_model_refs(dir: &Path) -> HashSet<(String, u32)> {
    let Ok(b) = std::fs::read(crate::overlay::path(dir, r"Configs\DynDOLOD_SSE_childworldfull.txt")) else { return HashSet::new() };
    decode(&b)
        .lines()
        .filter(|l| !l.trim_start().starts_with(';'))
        .filter_map(|l| {
            let mut p = l.split(';');
            let plugin = p.next()?.trim().to_ascii_lowercase();
            Some((plugin, u32::from_str_radix(p.next()?.trim(), 16).ok()? & 0x00FF_FFFF))
        })
        .collect()
}

/// Prefix of the ids copies use until they get their DynDOLOD.esm form id.
pub const COPY_ID_BASE: u32 = 0xFD00_0000;

pub fn is_copy(id: u32) -> bool {
    id & 0xFF00_0000 == COPY_ID_BASE
}

/// Reads `Configs\DynDOLOD_SSE_childworld_<child>.ini`, section `[<parent>]`; `None` when the
/// file or section is missing or a `NoScan...IfPluginExists` plugin is loaded.
pub fn settings(dir: &Path, child: &str, parent: &str, preset: &str, plugins: &[String]) -> Option<ChildSettings> {
    let ini = Ini::read(&crate::overlay::path(dir, &format!(r"Configs\DynDOLOD_SSE_childworld_{child}.ini"))).ok()?;
    let get = |k: &str| ini.get(parent, k).map(str::trim);
    get("ScanChild")?;
    let loaded: HashSet<String> = plugins.iter().map(|p| p.to_ascii_lowercase()).collect();
    for k in ["NoScanIfPluginExists", "NoScanChildIfPluginExists"] {
        if get(k).is_some_and(|v| v.split(',').any(|p| loaded.contains(&p.trim().to_ascii_lowercase()))) {
            return None;
        }
    }
    let list = |k: &str| -> Vec<String> { get(k).unwrap_or("").split(',').map(|s| s.trim().to_ascii_lowercase()).filter(|s| !s.is_empty()).collect() };
    Some(ChildSettings { scan_child: get("ScanChild") == Some("1"), ignore_edid: list(&format!("IgnoreChildEDID{preset}")), ignore_plugins: ModWorldIgnore::read(dir).plugins_for(child) })
}

/// Known child/parent pairs of `Configs\DynDOLOD_SSE_ChildworldMatches.txt`: child side
/// (`plugin` lower-case, object id) -> parent sides.
pub fn known_matches(dir: &Path) -> HashMap<(String, u32), Vec<(String, u32)>> {
    let mut out: HashMap<(String, u32), Vec<(String, u32)>> = HashMap::new();
    let Ok(b) = std::fs::read(crate::overlay::path(dir, r"Configs\DynDOLOD_SSE_ChildworldMatches.txt")) else { return out };
    for l in decode(&b).lines().filter(|l| !l.trim_start().starts_with(';')) {
        let p: Vec<&str> = l.splitn(5, ';').collect();
        if p.len() < 4 {
            continue;
        }
        let id = |s: &str| u32::from_str_radix(s.trim(), 16).ok().map(|v| v & 0x00FF_FFFF);
        if let (Some(c), Some(pa)) = (id(p[1]), id(p[3])) {
            out.entry((p[0].trim().to_ascii_lowercase(), c)).or_default().push((p[2].trim().to_ascii_lowercase(), pa));
        }
    }
    out
}

/// The references of `children` (with their settings) to copy into `parent`.
///
/// `has_lod` says whether a reference of a base gets any LOD (the export's rule resolution);
/// `parent_removed(plugin, object id)` whether DynDOLOD removes a parent reference (reference
/// rules), in which case a known pair no longer stops the copy (golden: Dragonsreach and the
/// Bards College tower, whose parent `_LOD` stand-ins are deleted).
#[allow(clippy::too_many_arguments)]
pub fn copies(
    lo: &LoadOrder,
    parent: &Worldspace,
    children: &[(&Worldspace, ChildSettings)],
    bases: &HashMap<u32, Base>,
    matches: &HashMap<(String, u32), Vec<(String, u32)>>,
    full: &HashSet<(String, u32)>,
    has_lod: &dyn Fn(u32, &Placement, &Base) -> bool,
    parent_removed: &dyn Fn(&str, u32) -> bool,
) -> Vec<ChildCopy> {
    let owner = owners(lo);
    let plugin_of = |id: u32| owner.get(&owner_key(id)).map(|&i| lo.plugins[i].plugin.name.to_ascii_lowercase()).unwrap_or_default();
    let model_of = |p: &Placement| bases.get(&p.base).map(|b| b.model.as_str()).unwrap_or("");
    // Parent twins: how many parent references carry each key, less those a known pair takes.
    let mut parent_free: HashMap<(String, [i64; 4]), usize> = HashMap::new();
    for p in parent.refs.values() {
        *parent_free.entry(ref_key(model_of(p), p)).or_default() += 1;
    }
    let wanted: HashSet<(String, u32)> = matches.values().flatten().cloned().collect();
    let parent_by_local: HashMap<(String, u32), &Placement> = parent
        .refs
        .iter()
        .filter_map(|(id, p)| {
            let k = (plugin_of(*id), if *id >> 24 == 0xFE { *id & 0xFFF } else { *id & 0x00FF_FFFF });
            wanted.contains(&k).then_some((k, p))
        })
        .collect();
    for ps in matches.values() {
        for (pp, pid) in ps {
            if parent_removed(pp, *pid) {
                continue;
            }
            if let Some(p) = parent_by_local.get(&(pp.clone(), *pid)) {
                if let Some(n) = parent_free.get_mut(&ref_key(model_of(p), p)) {
                    *n = n.saturating_sub(1);
                }
            }
        }
    }
    let lookup = |id: u32| children.iter().find_map(|(w, _)| w.refs.get(&id)).or_else(|| parent.refs.get(&id));
    let mut out = Vec::new();
    for (child, s) in children {
        if !s.scan_child {
            continue;
        }
        let mut ids: Vec<u32> = child.refs.keys().copied().collect();
        ids.sort_unstable();
        let mut seen: HashSet<((String, [i64; 4]), [i64; 3])> = HashSet::new();
        let trace = std::env::var("LODFORGE_CHILD_TRACE").ok().and_then(|s| u32::from_str_radix(s.trim(), 16).ok());
        for id in ids {
            let p = &child.refs[&id];
            let tr = |why: &str| {
                if trace == Some(id) {
                    eprintln!("child copy {id:08X}: {why}");
                }
            };
            // Only references that start enabled through their enable-parent chain (golden: the
            // intact Windhelm market is copied, its civil-war "Destroyed" state is not).
            if !initially_enabled(p, &lookup) {
                tr("initially disabled");
                continue;
            }
            let Some(b) = bases.get(&p.base) else {
                tr("no base");
                continue;
            };
            // Trees with an enable parent are not copied (golden: the Whiterun temple tree states
            // and two Solitude trees; every copied tree is unparented).
            if p.enable_parent && crate::assign::is_tree(b) {
                tr("tree with enable parent");
                continue;
            }
            let edid = b.editor_id.to_ascii_lowercase();
            if s.ignore_edid.iter().any(|i| edid.contains(i.as_str())) {
                tr("IgnoreChildEDID");
                continue;
            }
            let winner = lo.plugins.get(usize::from(p.plugin)).map(|lp| lp.plugin.name.to_ascii_lowercase()).unwrap_or_default();
            if mod_world_ignored(&s.ignore_plugins, &plugin_of(id), &winner) {
                tr("mod_world_ignore");
                continue;
            }
            let local = if id >> 24 == 0xFE { id & 0xFFF } else { id & 0x00FF_FFFF };
            let matched = matches.get(&(plugin_of(id), local)).is_some_and(|ps| ps.iter().any(|(pp, pid)| !parent_removed(pp, *pid)));
            if matched {
                tr("known pair");
                continue;
            }
            if !has_lod(id, p, b) {
                tr("no LOD");
                continue;
            }
            let k = ref_key(&b.model, p);
            if !seen.insert((k.clone(), rot_key(p))) {
                tr("duplicate in the child");
                continue;
            }
            if let Some(n) = parent_free.get_mut(&k).filter(|n| **n > 0) {
                *n -= 1;
                tr("parent twin");
                continue;
            }
            tr("copied");
            let scale = if p.has_scale { p.scale } else { 1.0 };
            let extent = (0..3).map(|k| f32::from(b.bounds[k + 3]) - f32::from(b.bounds[k])).fold(0.0, f32::max) * scale;
            let large = !crate::assign::is_tree(b) && extent >= LARGE_MIN_EXTENT;
            out.push(ChildCopy { child_world: child.form_id, ref_id: id, placement: p.clone(), copy_id: 0, full_model: full.contains(&(plugin_of(id), local)), large });
        }
    }
    for (n, c) in out.iter_mut().enumerate() {
        c.copy_id = COPY_ID_BASE | n as u32;
    }
    out
}

/// `parent` with the copies added as references (ids [`ChildCopy::copy_id`], cell from the position).
pub fn with_copies(parent: &Worldspace, copies: &[ChildCopy]) -> Worldspace {
    let mut w = parent.clone();
    for c in copies {
        let mut p = c.placement.clone();
        p.cell = Some(((p.position[0] / 4096.0).floor() as i32, (p.position[1] / 4096.0).floor() as i32));
        w.refs.insert(c.copy_id, p);
    }
    w
}

/// Persistent full-LOD originals in a child whose known parent counterpart already receives
/// LOD. These are excluded from child copies, but still need a REVERT and runtime reset.
pub fn matched_full_lod_refs(
    lo: &LoadOrder,
    child: &Worldspace,
    parent_lod_refs: &[u32],
    matches: &HashMap<(String, u32), Vec<(String, u32)>>,
    rules: &[crate::rules::Rule],
) -> Vec<u32> {
    let owner = owners(lo);
    let identity = |id: u32| {
        owner.get(&owner_key(id)).map(|&i| {
            let local = if id >> 24 == 0xFE { id & 0xFFF } else { id & 0x00FF_FFFF };
            (lo.plugins[i].plugin.name.to_ascii_lowercase(), local)
        })
    };
    let selected: HashSet<_> = parent_lod_refs.iter().filter_map(|&id| identity(id)).collect();
    let matched: Vec<u32> = child.refs.iter()
        .filter(|(_, r)| r.flags & lodforge_esp::record::FLAG_DELETED == 0)
        .filter_map(|(&id, _)| {
            let key = identity(id)?;
            if removed_by_rule(rules, &key.0, key.1) {
                return None;
            }
            matches.get(&key).is_some_and(|parents| parents.iter().any(|p| selected.contains(p))).then_some(id)
        })
        .collect();
    crate::largerefs::revert_refs(child, &matched)
}

/// Whether the export would give `p` (a reference of `b` defined by `plugin`) any LOD: not
/// disabled or deleted, an `Unchanged` reference rule with level columns or no reference rule and
/// a mask rule with level columns.
pub fn has_lod(r: &crate::assign::Resolver<'_>, rules: &[crate::rules::Rule], plugin: &str, local: u32, p: &Placement, b: &Base) -> bool {
    use lodforge_esp::record::{FLAG_DELETED, FLAG_INITIALLY_DISABLED};
    if p.flags & (FLAG_INITIALLY_DISABLED | FLAG_DELETED) != 0 {
        return false;
    }
    let lod = r.resolve(b);
    match crate::rules::select_reference_rule(rules, plugin, local) {
        Some(rule) => rule.reference_action.eq_ignore_ascii_case("Unchanged") && r.columns(rule, b, &lod).iter().any(|c| !c.is_empty()),
        None => lod.rule.is_some() && lod.columns.iter().any(|c| !c.is_empty()),
    }
}

/// Whether a reference rule removes the reference (`plugin` lower-case, object id).
pub fn removed_by_rule(rules: &[crate::rules::Rule], plugin: &str, local: u32) -> bool {
    crate::rules::select_reference_rule(rules, plugin, local).is_some_and(|r| !r.reference_action.eq_ignore_ascii_case("Unchanged"))
}

/// Initial enable state through the enable-parent chain: the top reference's own flag, inverted
/// at every "opposite of parent" link.
fn initially_enabled<'a>(p: &'a Placement, lookup: &dyn Fn(u32) -> Option<&'a Placement>) -> bool {
    use lodforge_esp::record::FLAG_INITIALLY_DISABLED;
    let mut cur = p;
    let mut opposite = false;
    for _ in 0..32 {
        if !cur.enable_parent {
            break;
        }
        let Some(next) = lookup(cur.enable_parent_id) else { break };
        opposite ^= cur.enable_parent_flags & 1 != 0;
        cur = next;
    }
    (cur.flags & FLAG_INITIALLY_DISABLED == 0) != opposite
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ref_key_matches_the_debug_log() {
        let p = Placement { position: [171_620.0, -95_210.0, 11_080.0], scale: 1.0, has_scale: false, ..Default::default() };
        assert_eq!(ref_key(r"Meshes\Architecture\Riften\RTPawnedPrawn01.nif", &p), (r"architecture\riften\rtpawnedprawn01.nif".to_string(), [1716, -952, 111, 100]));
        let q = Placement { position: [23_550.0, -1_450.0, -2_650.0], scale: 0.3, has_scale: true, ..Default::default() };
        // Half way rounds to even, like Delphi's Round.
        assert_eq!(ref_key("architecture\\sovengarde\\sovngardestatue01.nif", &q).1, [236, -14, -26, 30]);
    }

    #[test]
    fn mod_world_ignore_lists_plugins_per_worldspace() {
        let m = ModWorldIgnore::parse("// ignore all references added by mod to worldspace\n// mod file name=worldspace name,worldspace name,...\nJKs Skyrim.esp=MarkarthWorld,SolitudeWorld,WindhelmWorld\nELFX - Exteriors.esp=WhiterunWorld, WindhelmWorld\n");
        let windhelm = m.plugins_for("WindhelmWorld");
        assert!(windhelm.contains("jks skyrim.esp") && windhelm.contains("elfx - exteriors.esp"));
        assert!(!m.plugins_for("RiftenWorld").contains("jks skyrim.esp"));
        // Defined by the listed plugin (its patch wins), or a master's reference it overrides.
        assert!(mod_world_ignored(&windhelm, "jks skyrim.esp", "jks candlehearth hall - jks skyrim patch.esp"));
        assert!(mod_world_ignored(&windhelm, "skyrim.esm", "jks skyrim.esp"));
        assert!(!mod_world_ignored(&windhelm, "skyrim.esm", "jk's candlehearth hall.esp"));
        assert!(!mod_world_ignored(&HashSet::new(), "jks skyrim.esp", "jks skyrim.esp"));
    }
}

/// Parent references that duplicate content of `child` (the child has the object itself): the
/// parent sides of known pairs (`ChildworldMatches.txt`) whose child side is in `child`, and
/// parent twins, i.e. a parent reference with the [`ref_key`] (model, position / 100, scale) of
/// a child reference. Used by runtimes that draw the parent's LOD inside a child worldspace's
/// cells: everything else of the parent there is missing from the child (stables, farms,
/// outer walls), the duplicates would draw the city twice.
pub fn parent_duplicates(lo: &LoadOrder, parent: &Worldspace, child: &Worldspace, bases: &HashMap<u32, Base>, matches: &HashMap<(String, u32), Vec<(String, u32)>>) -> HashSet<u32> {
    let owner = owners(lo);
    let identity = |id: u32| {
        owner.get(&owner_key(id)).map(|&i| {
            let local = if id >> 24 == 0xFE { id & 0xFFF } else { id & 0x00FF_FFFF };
            (lo.plugins[i].plugin.name.to_ascii_lowercase(), local)
        })
    };
    let model_of = |p: &Placement| bases.get(&p.base).map(|b| b.model.as_str()).unwrap_or("");
    let live = |p: &Placement| p.flags & lodforge_esp::record::FLAG_DELETED == 0;
    let child_keys: HashSet<(String, [i64; 4])> = child.refs.values().filter(|p| live(p)).map(|p| ref_key(model_of(p), p)).collect();
    let paired: HashSet<(String, u32)> = child.refs.iter().filter(|(_, p)| live(p)).filter_map(|(&id, _)| identity(id)).filter_map(|k| matches.get(&k)).flatten().cloned().collect();
    parent
        .refs
        .iter()
        .filter(|(id, p)| !is_copy(**id) && (child_keys.contains(&ref_key(model_of(p), p)) || identity(**id).is_some_and(|k| paired.contains(&k))))
        .map(|(&id, _)| id)
        .collect()
}
