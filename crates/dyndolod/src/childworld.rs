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

/// Parent references that duplicate content of `child` (the child has the object itself), each
/// with its child twin: the parent sides of known pairs (`ChildworldMatches.txt`) whose child
/// side is in `child`, and parent twins, i.e. a parent reference with the [`ref_key`] (model,
/// position / 100, scale) of a child reference. Used by runtimes that draw the parent's LOD
/// inside a child worldspace's cells: such a duplicate is shown only while its twin's own 3D is
/// not loaded, everything else of the parent there (stables, farms) is missing from the child.
pub fn parent_duplicates(lo: &LoadOrder, parent: &Worldspace, child: &Worldspace, bases: &HashMap<u32, Base>, matches: &HashMap<(String, u32), Vec<(String, u32)>>) -> HashMap<u32, u32> {
    let owner = owners(lo);
    let identity = |id: u32| {
        owner.get(&owner_key(id)).map(|&i| {
            let local = if id >> 24 == 0xFE { id & 0xFFF } else { id & 0x00FF_FFFF };
            (lo.plugins[i].plugin.name.to_ascii_lowercase(), local)
        })
    };
    let model_of = |p: &Placement| bases.get(&p.base).map(|b| b.model.as_str()).unwrap_or("");
    let live = |p: &Placement| p.flags & lodforge_esp::record::FLAG_DELETED == 0;
    let mut child_ids: Vec<u32> = child.refs.iter().filter(|(_, p)| live(p)).map(|(&id, _)| id).collect();
    child_ids.sort_unstable();
    let mut child_keys: HashMap<(String, [i64; 4]), u32> = HashMap::new();
    let mut paired: HashMap<(String, u32), u32> = HashMap::new();
    for &id in &child_ids {
        child_keys.entry(ref_key(model_of(&child.refs[&id]), &child.refs[&id])).or_insert(id);
        if let Some(parents) = identity(id).and_then(|k| matches.get(&k)) {
            for pk in parents {
                paired.entry(pk.clone()).or_insert(id);
            }
        }
    }
    parent
        .refs
        .iter()
        .filter(|(id, _)| !is_copy(**id))
        .filter_map(|(&id, p)| identity(id).and_then(|k| paired.get(&k).copied()).or_else(|| child_keys.get(&ref_key(model_of(p), p)).copied()).map(|twin| (id, twin)))
        .collect()
}

// ---------------------------------------------------------------------------------------------
// Floating copies (a deliberate divergence from DynDOLOD; docs/re/dyndolod-spec.md "Child
// worldspace copies"): a small model that stands on something the parent world has no LOD for.
// ---------------------------------------------------------------------------------------------

use lodforge_objlod::transform::{Mat3, Xform};

/// The rule of [`drop_floating`]: a copy whose scaled OBND is under `max_extent` on every axis,
/// whose lowest point is more than `clearance` above the parent's ground and which no LOD
/// surface (another copy's or a parent LOD reference's LOD model) passes under, through or
/// just over is left out.
///
/// Whiterun (cs-base 2026-10-27): the brazier bowls `WRBrazierBowl01` (`082834`, `082837` of
/// Skyrim.esm) on the north wall stand on `WRIntCastleStoneColumn01` pieces that have no LOD
/// model, so DynDOLOD's own output (and ours before this rule) shows two bowls hanging in the
/// air above the city wall when looking from Tamriel's side. Large structures are never dropped:
/// at worst they are the support of something smaller.
#[derive(Debug, Clone, PartialEq)]
pub struct FloatingRule {
    /// Largest scaled OBND extent (on any axis) of a copy the rule looks at, game units.
    pub max_extent: f32,
    /// A copy whose lowest point is more than this above the parent's ground is not standing on
    /// it (the terrain of a walled city and the parent's land under it differ by a few units
    /// where they agree at all); also how far below the foot, or above the top, a LOD surface
    /// may pass and still hold the copy (the parent's terrain stand-ins are coarse meshes:
    /// JK's Whiterun braziers sit up to 30 units over `WRCloudDistrictTerrainLOD`, JK's Riften
    /// steps up to 48 over `RTLandC01`).
    pub clearance: f32,
    /// Slack around a support's OBND box when looking for the supports to test, game units.
    pub margin: f32,
}

impl Default for FloatingRule {
    fn default() -> Self {
        FloatingRule { max_extent: 150.0, clearance: 48.0, margin: 16.0 }
    }
}

/// A copy [`drop_floating`] left out, with what it measured.
#[derive(Debug, Clone)]
pub struct Floating {
    pub copy: ChildCopy,
    /// Largest scaled OBND extent.
    pub extent: f32,
    /// The copy's lowest point above the parent's ground.
    pub above_terrain: f32,
}

/// Ground heights of a worldspace: its winning LAND records (`Worldspace::land_by_cell`),
/// decoded per cell on first use and bilinear inside the cell's 33 x 33 grid, and its water
/// (the cell's XCLW, else the WRLD default; a one-byte CELL DATA means no water), where that
/// is higher: a model at the water surface does not float.
pub struct LandHeights<'a> {
    lo: &'a LoadOrder,
    world: &'a Worldspace,
    lands: HashMap<(i32, i32), lodforge_esp::LandRef>,
    default_water: Option<f32>,
    cache: std::cell::RefCell<HashMap<(i32, i32), Option<std::rc::Rc<Vec<f32>>>>>,
}

impl<'a> LandHeights<'a> {
    pub fn new(lo: &'a LoadOrder, world: &'a Worldspace) -> LandHeights<'a> {
        // WRLD DATA 0x08: no LOD water.
        let default_water = world.default_heights.filter(|_| world.flags & 0x08 == 0).map(|d| d.1);
        LandHeights { lo, world, lands: world.land_by_cell(), default_water, cache: Default::default() }
    }

    fn cell(&self, c: (i32, i32)) -> Option<std::rc::Rc<Vec<f32>>> {
        if let Some(h) = self.cache.borrow().get(&c) {
            return h.clone();
        }
        let h = self.lands.get(&c).and_then(|l| {
            let pl = self.lo.plugins.get(usize::from(l.plugin))?;
            let r = pl.record_at(l.offset).ok()?;
            pl.decode_land(&r).ok()?.heights.map(std::rc::Rc::new)
        });
        self.cache.borrow_mut().insert(c, h.clone());
        h
    }

    /// Terrain height under (x, y); `None` where the cell has no LAND.
    pub fn height_at(&self, x: f32, y: f32) -> Option<f32> {
        const GRID: usize = lodforge_esp::terrain::GRID;
        let (cx, cy) = ((x / 4096.0).floor() as i32, (y / 4096.0).floor() as i32);
        let h = self.cell((cx, cy))?;
        let gx = (x - cx as f32 * 4096.0) / 128.0;
        let gy = (y - cy as f32 * 4096.0) / 128.0;
        let (ix, iy) = (gx.floor().clamp(0.0, (GRID - 2) as f32), gy.floor().clamp(0.0, (GRID - 2) as f32));
        let (fx, fy) = ((gx - ix).clamp(0.0, 1.0), (gy - iy).clamp(0.0, 1.0));
        let (col, row) = (ix as usize, iy as usize);
        let at = |c: usize, r: usize| h[r * GRID + c];
        let south = at(col, row) * (1.0 - fx) + at(col + 1, row) * fx;
        let north = at(col, row + 1) * (1.0 - fx) + at(col + 1, row + 1) * fx;
        Some(south * (1.0 - fy) + north * fy)
    }

    /// Water height of the cell under (x, y), as the terrain export takes it.
    pub fn water_at(&self, x: f32, y: f32) -> Option<f32> {
        let cell = ((x / 4096.0).floor() as i32, (y / 4096.0).floor() as i32);
        if self.world.cell_flags.get(&cell).is_some_and(|f| f & lodforge_esp::CELL_SHORT_DATA != 0) {
            return None;
        }
        match self.world.cell_water.get(&cell) {
            Some(&h) if h.abs() < 16_777_216.0 => Some(h),
            _ => self.default_water,
        }
    }

    /// The ground under (x, y): the terrain, or the water where that is higher.
    pub fn ground_at(&self, x: f32, y: f32) -> Option<f32> {
        let land = self.height_at(x, y)?;
        Some(self.water_at(x, y).map_or(land, |w| land.max(w)))
    }
}

/// The placed transform of a reference (`T R S`, as the export's reference lines).
pub fn placed(p: &Placement) -> Xform {
    Xform::reference(p.position, p.rotation.map(f32::to_degrees), if p.has_scale && p.scale > 0.0 { p.scale } else { 1.0 })
}

/// The triangles of a loaded LOD model in model space (every part, as `mesh::import` bakes them).
pub fn mesh_triangles(m: &lodforge_objlod::mesh::LodMesh) -> Vec<[[f32; 3]; 3]> {
    m.parts.iter().flat_map(|p| p.triangles.iter().filter_map(|t| Some([*p.positions.get(usize::from(t[0]))?, *p.positions.get(usize::from(t[1]))?, *p.positions.get(usize::from(t[2]))?]))).collect()
}

/// The heights at which the surface of `triangles` (model space) placed by `xf` crosses the
/// vertical line through world (x, y): one per triangle whose XY projection holds the point.
pub fn surface_crossings(triangles: &[[[f32; 3]; 3]], xf: &Xform, x: f32, y: f32) -> Vec<f32> {
    let mut out = Vec::new();
    for t in triangles {
        let [a, b, c] = t.map(|v| xf.apply(v));
        let d = (b[1] - c[1]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[1] - c[1]);
        if d.abs() < 1e-9 {
            continue;
        }
        let l0 = ((b[1] - c[1]) * (x - c[0]) + (c[0] - b[0]) * (y - c[1])) / d;
        let l1 = ((c[1] - a[1]) * (x - c[0]) + (a[0] - c[0]) * (y - c[1])) / d;
        let l2 = 1.0 - l0 - l1;
        const EPS: f32 = -1e-4;
        if l0 >= EPS && l1 >= EPS && l2 >= EPS {
            out.push(l0 * a[2] + l1 * b[2] + l2 * c[2]);
        }
    }
    out
}

/// A base's OBND placed by a reference: the world hull of the rotated box and the inverse
/// transform for tests in the box's own frame.
#[derive(Clone)]
struct PlacedBox {
    bounds: [f32; 6],
    scale: f32,
    world_min: [f32; 3],
    world_max: [f32; 3],
    /// world -> local: `v = R^T (v' - t) / s`.
    inv: Mat3,
    t: [f32; 3],
}

impl PlacedBox {
    /// `None` for a base without OBND (nothing to measure).
    fn new(p: &Placement, b: &Base) -> Option<PlacedBox> {
        if b.bounds.iter().all(|&v| v == 0) {
            return None;
        }
        let bounds = b.bounds.map(f32::from);
        let scale = if p.has_scale && p.scale > 0.0 { p.scale } else { 1.0 };
        let xf = placed(p);
        let (mut world_min, mut world_max) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        for i in 0..8 {
            let c = xf.apply([bounds[if i & 1 == 0 { 0 } else { 3 }], bounds[if i & 2 == 0 { 1 } else { 4 }], bounds[if i & 4 == 0 { 2 } else { 5 }]]);
            for k in 0..3 {
                world_min[k] = world_min[k].min(c[k]);
                world_max[k] = world_max[k].max(c[k]);
            }
        }
        let r = xf.rotation_only().0;
        let inv = Mat3([[r[0][0], r[1][0], r[2][0]], [r[0][1], r[1][1], r[2][1]], [r[0][2], r[1][2], r[2][2]]]).scaled(1.0 / scale);
        Some(PlacedBox { bounds, scale, world_min, world_max, inv, t: p.position })
    }

    /// Largest scaled extent on any axis.
    fn extent(&self) -> f32 {
        (0..3).map(|k| (self.bounds[k + 3] - self.bounds[k]) * self.scale).fold(0.0, f32::max)
    }

    /// Where the model stands: the middle of its world hull at its lowest point.
    fn foot(&self) -> [f32; 3] {
        [(self.world_min[0] + self.world_max[0]) * 0.5, (self.world_min[1] + self.world_max[1]) * 0.5, self.world_min[2]]
    }

    /// Whether world point `w` lies inside the box widened by `margin` world units.
    fn holds(&self, w: [f32; 3], margin: f32) -> bool {
        let l = self.inv.apply([w[0] - self.t[0], w[1] - self.t[1], w[2] - self.t[2]]);
        let m = margin / self.scale;
        (0..3).all(|k| l[k] >= self.bounds[k] - m && l[k] <= self.bounds[k + 3] + m)
    }

    /// The cells the world hull covers.
    fn cells(&self) -> impl Iterator<Item = (i32, i32)> {
        let c = |v: f32| (v / 4096.0).floor() as i32;
        let (x0, x1, y0, y1) = (c(self.world_min[0]), c(self.world_max[0]), c(self.world_min[1]), c(self.world_max[1]));
        (x0..=x1).flat_map(move |x| (y0..=y1).map(move |y| (x, y)))
    }
}

/// What a copy may stand on.
struct Support<'a> {
    /// Index of the copy this is, `None` for a parent reference.
    owner: Option<usize>,
    id: u32,
    placement: &'a Placement,
    base: &'a Base,
    bx: PlacedBox,
}

/// Leaves the floating copies out of `copies` ([`FloatingRule`]): a small copy that stands
/// neither on the parent's ground (`ground(x, y)`; `None` where the parent has no land data,
/// and such a copy is kept) nor on a LOD surface. The supports looked at are the other copies
/// and the parent's references with LOD (`parent_lod`: reference id and placement) whose
/// placed OBND box, widened by `margin`, holds the copy's foot (the middle of its hull at its
/// lowest point); `lod_under(id, placement, base, [x, y])` gives the heights at which that
/// support's LOD surface crosses the vertical line through the foot, and one between
/// `clearance` below the foot and `clearance` above the model's top holds the copy (standing
/// on the surface, sunk into it, or hanging from it like icicles under an eave). A dropped copy
/// holds nothing (passes repeat until nothing changes). The kept copies are renumbered
/// (`COPY_ID_BASE | n`); returns them and the dropped ones with their measurements.
/// `LODFORGE_CHILD_TRACE=<ref id>` prints the decision for one reference.
pub fn drop_floating(copies: Vec<ChildCopy>, bases: &HashMap<u32, Base>, parent_lod: &[(u32, &Placement)], ground: &dyn Fn(f32, f32) -> Option<f32>, lod_under: &dyn Fn(u32, &Placement, &Base, [f32; 2]) -> Vec<f32>, rule: &FloatingRule) -> (Vec<ChildCopy>, Vec<Floating>) {
    let boxes: Vec<Option<PlacedBox>> = copies.iter().map(|c| bases.get(&c.placement.base).and_then(|b| PlacedBox::new(&c.placement, b))).collect();
    let mut supports: Vec<Support<'_>> = Vec::new();
    for (i, (c, bx)) in copies.iter().zip(&boxes).enumerate() {
        if let (Some(bx), Some(b)) = (bx, bases.get(&c.placement.base)) {
            supports.push(Support { owner: Some(i), id: c.ref_id, placement: &c.placement, base: b, bx: bx.clone() });
        }
    }
    for &(id, p) in parent_lod {
        if let Some((b, bx)) = bases.get(&p.base).and_then(|b| PlacedBox::new(p, b).map(|bx| (b, bx))) {
            supports.push(Support { owner: None, id, placement: p, base: b, bx });
        }
    }
    let mut grid: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
    for (j, s) in supports.iter().enumerate() {
        for c in s.bx.cells() {
            grid.entry(c).or_default().push(j);
        }
    }
    let trace = std::env::var("LODFORGE_CHILD_TRACE").ok().and_then(|s| u32::from_str_radix(s.trim(), 16).ok());
    let mut dropped: Vec<Option<Floating>> = vec![None; copies.len()];
    let mut pass = 0usize;
    loop {
        let mut changed = false;
        // The trace repeats a kept decision only when something changed since.
        let first = pass == 0;
        for (i, c) in copies.iter().enumerate() {
            if dropped[i].is_some() {
                continue;
            }
            let Some(bx) = &boxes[i] else { continue };
            let Some(b) = bases.get(&c.placement.base) else { continue };
            if c.large || c.full_model || crate::assign::is_tree(b) {
                continue;
            }
            let extent = bx.extent();
            if extent >= rule.max_extent {
                continue;
            }
            let foot = bx.foot();
            let top = bx.world_max[2];
            let tr = |why: String, always: bool| {
                if trace == Some(c.ref_id) && (first || always) {
                    eprintln!("child copy {:08X}: floating rule: extent {extent:.0}, foot ({:.0}, {:.0}, {:.0}), top {top:.0}: {why}", c.ref_id, foot[0], foot[1], foot[2]);
                }
            };
            let Some(level) = ground(foot[0], foot[1]) else {
                tr("no terrain data, kept".into(), false);
                continue;
            };
            let above = foot[2] - level;
            if above <= rule.clearance {
                tr(format!("{above:.0} above the ground, kept"), false);
                continue;
            }
            let (below, over) = (foot[2] - rule.clearance, top + rule.clearance);
            let cell = ((foot[0] / 4096.0).floor() as i32, (foot[1] / 4096.0).floor() as i32);
            let mut near = Vec::new();
            let held_by = grid.get(&cell).into_iter().flatten().copied().find(|&j| {
                let s = &supports[j];
                if s.owner == Some(i) || s.owner.is_some_and(|o| dropped[o].is_some()) || !s.bx.holds(foot, rule.margin) {
                    return false;
                }
                let zs = lod_under(s.id, s.placement, s.base, [foot[0], foot[1]]);
                let hit = zs.iter().any(|&z| z >= below && z <= over);
                if !hit {
                    near.push(format!("{} {:08X} (surface at {:?})", s.base.editor_id, s.id, zs.iter().map(|z| z.round()).collect::<Vec<_>>()));
                }
                hit
            });
            match held_by {
                Some(j) => tr(format!("{above:.0} above the ground, held by {} {:08X} at ({:.0}, {:.0}, {:.0}), kept", supports[j].base.editor_id, supports[j].id, supports[j].placement.position[0], supports[j].placement.position[1], supports[j].placement.position[2]), false),
                None => {
                    tr(format!("{above:.0} above the ground, no LOD surface under it (boxes holding the foot: {}): dropped", if near.is_empty() { "none".to_string() } else { near.join(", ") }), true);
                    dropped[i] = Some(Floating { copy: c.clone(), extent, above_terrain: above });
                    changed = true;
                }
            }
        }
        pass += 1;
        if !changed {
            break;
        }
    }
    let mut kept = Vec::with_capacity(copies.len());
    let mut out = Vec::new();
    for (c, d) in copies.into_iter().zip(dropped) {
        match d {
            Some(f) => out.push(f),
            None => kept.push(c),
        }
    }
    for (n, c) in kept.iter_mut().enumerate() {
        c.copy_id = COPY_ID_BASE | n as u32;
    }
    (kept, out)
}

#[cfg(test)]
mod floating_tests {
    use super::*;

    fn base(id: u32, edid: &str, bounds: [i16; 6]) -> (u32, Base) {
        (id, Base { signature: *b"STAT", form_id: id, editor_id: edid.into(), model: format!("{edid}.nif"), bounds, ..Default::default() })
    }

    fn at(base: u32, x: f32, y: f32, z: f32, yaw_deg: f32, scale: f32) -> Placement {
        Placement { base, position: [x, y, z], rotation: [0.0, 0.0, yaw_deg.to_radians()], scale, has_scale: scale != 1.0, ..Default::default() }
    }

    fn copy(n: u32, p: Placement) -> ChildCopy {
        ChildCopy { child_world: 1, ref_id: 0x1000 + n, placement: p, copy_id: COPY_ID_BASE | n, full_model: false, large: false }
    }

    /// A LOD model that is its OBND box: the top face as two triangles.
    fn box_top(b: &Base) -> Vec<[[f32; 3]; 3]> {
        let [x1, y1, _, x2, y2, z2] = b.bounds.map(f32::from);
        vec![[[x1, y1, z2], [x2, y1, z2], [x2, y2, z2]], [[x1, y1, z2], [x2, y2, z2], [x1, y2, z2]]]
    }

    fn top_surface(_: u32, p: &Placement, b: &Base, xy: [f32; 2]) -> Vec<f32> {
        surface_crossings(&box_top(b), &placed(p), xy[0], xy[1])
    }

    /// The Whiterun case in miniature: brazier bowls (80 x 80 x 90) on columns without LOD
    /// float; those on a copied wall, on a parent reference with LOD, on the ground, on the
    /// roof of a big copy, sunk into a wall or hanging under its top surface stay, one inside
    /// a big copy's box with no surface near it does not; big and scaled-up models are never
    /// dropped; no land data or no OBND keeps a copy.
    #[test]
    fn floating_small_copies_are_dropped() {
        let bases: HashMap<u32, Base> = [base(1, "WRBrazierBowl01", [-40, -40, 0, 40, 40, 90]), base(2, "WRCityWall", [-400, -40, 0, 400, 40, 500]), base(3, "Dragonsreach", [-1500, -1500, -100, 1500, 1500, 2000]), base(4, "NoBounds", [0; 6])].into_iter().collect();
        let ground = |x: f32, y: f32| if x < 100_000.0 { Some(if y > 50_000.0 { 1000.0 } else { 0.0 }) } else { None };
        let wall = copy(0, at(2, 0.0, 0.0, 0.0, 90.0, 1.0)); // runs along Y after the 90 degree yaw
        let big = copy(1, at(3, 20_000.0, 0.0, 0.0, 0.0, 1.0));
        let copies = vec![
            wall,
            big,
            copy(2, at(1, 0.0, 300.0, 500.0, 0.0, 1.0)),      // on the wall's top: kept
            copy(3, at(1, 300.0, 0.0, 500.0, 0.0, 1.0)),      // beside the wall, 500 up: dropped
            copy(4, at(1, 5000.0, 0.0, 10.0, 0.0, 1.0)),      // on the ground: kept
            copy(5, at(1, 5000.0, 0.0, 200.0, 0.0, 1.0)),     // on a column without LOD: dropped
            copy(6, at(1, 6000.0, 0.0, 200.0, 0.0, 2.0)),     // 180 units: not small, kept
            copy(7, at(1, 20_000.0, 0.0, 2010.0, 0.0, 1.0)),  // on Dragonsreach's roof: kept
            copy(8, at(1, 9000.0, 0.0, 500.0, 0.0, 1.0)),     // on a parent reference with LOD: kept
            copy(9, at(1, 200_000.0, 0.0, 500.0, 0.0, 1.0)),  // no land data there: kept
            copy(10, at(4, 5000.0, 0.0, 900.0, 0.0, 1.0)),    // no OBND: kept
            copy(11, at(1, 5000.0, 60_000.0, 1020.0, 0.0, 1.0)), // 20 over higher ground: kept
            copy(12, at(1, 7000.0, 0.0, 300.0, 0.0, 1.0)),    // on copy 13, which floats: both dropped
            copy(13, at(1, 7000.0, 0.0, 210.0, 0.0, 1.0)),
            copy(14, at(1, 20_500.0, 0.0, 800.0, 0.0, 1.0)),  // inside Dragonsreach's box, no surface near: dropped
            copy(15, at(1, 0.0, -300.0, 440.0, 0.0, 1.0)),    // sunk 60 into the wall's top: kept
            copy(16, at(1, 0.0, -100.0, 390.0, 0.0, 1.0)),    // hanging 20 under the wall's top surface: kept
        ];
        let parent_wall = at(2, 9000.0, 0.0, 0.0, 0.0, 1.0);
        let (kept, dropped) = drop_floating(copies, &bases, &[(0x2000, &parent_wall)], &ground, &top_surface, &FloatingRule::default());
        let mut gone: Vec<u32> = dropped.iter().map(|f| f.copy.ref_id - 0x1000).collect();
        gone.sort_unstable();
        assert_eq!(gone, vec![3, 5, 12, 13, 14]);
        let stayed: Vec<u32> = kept.iter().map(|c| c.ref_id - 0x1000).collect();
        assert_eq!(stayed, vec![0, 1, 2, 4, 6, 7, 8, 9, 10, 11, 15, 16]);
        // Renumbered without gaps.
        assert!(kept.iter().enumerate().all(|(n, c)| c.copy_id == COPY_ID_BASE | n as u32));
        let five = dropped.iter().find(|f| f.copy.ref_id == 0x1005).unwrap();
        assert_eq!(five.extent, 90.0);
        assert_eq!(five.above_terrain, 200.0);
        // A rule nothing matches changes nothing.
        let none = FloatingRule { max_extent: 0.0, ..Default::default() };
        let (k, d) = drop_floating(vec![copy(0, at(1, 5000.0, 0.0, 200.0, 0.0, 1.0))], &bases, &[], &ground, &top_surface, &none);
        assert!(d.is_empty() && k.len() == 1);
    }

    /// The support's box and surface are placed with its rotation and scale: a yawed wall
    /// holds what stands on its rotated top, not everything inside its axis-aligned hull.
    #[test]
    fn supports_are_placed_like_their_reference() {
        let bases: HashMap<u32, Base> = [base(1, "WRBrazierBowl01", [-40, -40, 0, 40, 40, 90]), base(2, "WRCityWall", [-400, -40, 0, 400, 40, 500])].into_iter().collect();
        let ground = |_: f32, _: f32| Some(0.0);
        let wall = at(2, 0.0, 0.0, 0.0, 45.0, 1.0);
        // `Xform::reference` turns local +X by -45 degrees: towards (+X, -Y).
        let d = 300.0 * std::f32::consts::FRAC_1_SQRT_2;
        let on = at(1, d, -d, 500.0, 0.0, 1.0);
        let off = at(1, d, d, 500.0, 0.0, 1.0);
        let (kept, dropped) = drop_floating(vec![copy(0, on), copy(1, off)], &bases, &[(0x2000, &wall)], &ground, &top_surface, &FloatingRule::default());
        assert_eq!(kept.iter().map(|c| c.ref_id).collect::<Vec<_>>(), vec![0x1000]);
        assert_eq!(dropped.iter().map(|f| f.copy.ref_id).collect::<Vec<_>>(), vec![0x1001]);
        let small_wall = at(2, 0.0, 0.0, 0.0, 0.0, 0.5);
        let on_small = at(1, 150.0, 0.0, 250.0, 0.0, 1.0);
        let past_small = at(1, 300.0, 0.0, 250.0, 0.0, 1.0);
        let (kept, dropped) = drop_floating(vec![copy(0, on_small), copy(1, past_small)], &bases, &[(0x2000, &small_wall)], &ground, &top_surface, &FloatingRule::default());
        assert_eq!((kept.len(), dropped.len()), (1, 1));
        assert_eq!(kept[0].ref_id, 0x1000);
        // The surface test itself: a sloped triangle, inside and outside its projection.
        let tri = [[[0.0, 0.0, 0.0], [100.0, 0.0, 0.0], [0.0, 100.0, 100.0]]];
        let z = surface_crossings(&tri, &Xform::IDENTITY, 10.0, 20.0);
        assert_eq!(z.len(), 1);
        assert!((z[0] - 20.0).abs() < 1e-3);
        assert!(surface_crossings(&tri, &Xform::IDENTITY, 60.0, 60.0).is_empty());
        let moved = Xform::reference([1000.0, 0.0, 50.0], [0.0, 0.0, 90.0], 2.0);
        // Local (10, 20) scaled by 2 and turned by -90 degrees lands at (1040, -20): z = 50 + 2 * 20.
        let z = surface_crossings(&tri, &moved, 1040.0, -20.0);
        assert_eq!(z.len(), 1);
        assert!((z[0] - 90.0).abs() < 1e-2);
    }
}
