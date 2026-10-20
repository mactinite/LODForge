//! Child worldspace copies (`<plugin>&<id>_DynDOLOD_CHILD`): references of a child worldspace
//! (a walled city) copied into its parent so the parent's LOD shows them.
//!
//! Selection, as DynDOLOD 3 does it (golden: its debug log and `ChildworldCopies_<World>.txt`):
//! child worldspaces with a `Configs\DynDOLOD_SSE_childworld_<Child>.ini` section `[<Parent>]`
//! whose `ScanChild=1` (skipped when a `NoScanIfPluginExists` / `NoScanChildIfPluginExists`
//! plugin is loaded); every reference there except those whose base EditorID contains an
//! `IgnoreChildEDID<Preset>` entry, those initially disabled through their enable parents, those listed as the child side of a known pair in
//! `DynDOLOD_SSE_ChildworldMatches.txt`, and duplicates of a parent reference (same base at the
//! same place). Whether a copy gets LOD at all is the export's rule resolution.

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

/// References (`plugin` lower-case, object id) of `Configs\DynDOLOD_SSE_childworldfull.txt`.
pub fn full_model_refs(dir: &Path) -> HashSet<(String, u32)> {
    let Ok(b) = std::fs::read(dir.join("Configs").join("DynDOLOD_SSE_childworldfull.txt")) else { return HashSet::new() };
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
    let ini = Ini::read(&dir.join("Configs").join(format!("DynDOLOD_SSE_childworld_{child}.ini"))).ok()?;
    let get = |k: &str| ini.get(parent, k).map(str::trim);
    get("ScanChild")?;
    let loaded: HashSet<String> = plugins.iter().map(|p| p.to_ascii_lowercase()).collect();
    for k in ["NoScanIfPluginExists", "NoScanChildIfPluginExists"] {
        if get(k).is_some_and(|v| v.split(',').any(|p| loaded.contains(&p.trim().to_ascii_lowercase()))) {
            return None;
        }
    }
    let list = |k: &str| -> Vec<String> { get(k).unwrap_or("").split(',').map(|s| s.trim().to_ascii_lowercase()).filter(|s| !s.is_empty()).collect() };
    Some(ChildSettings { scan_child: get("ScanChild") == Some("1"), ignore_edid: list(&format!("IgnoreChildEDID{preset}")) })
}

/// Known child/parent pairs of `Configs\DynDOLOD_SSE_ChildworldMatches.txt`: child side
/// (`plugin` lower-case, object id) -> parent sides.
pub fn known_matches(dir: &Path) -> HashMap<(String, u32), Vec<(String, u32)>> {
    let mut out: HashMap<(String, u32), Vec<(String, u32)>> = HashMap::new();
    let Ok(b) = std::fs::read(dir.join("Configs").join("DynDOLOD_SSE_ChildworldMatches.txt")) else { return out };
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
    // Parent references by (base, position rounded to a unit) for the duplicate check.
    let key = |p: &Placement| (p.base, p.position.map(|v| v.round() as i32));
    let parent_keys: HashSet<(u32, [i32; 3])> = parent.refs.values().map(key).collect();
    let lookup = |id: u32| children.iter().find_map(|(w, _)| w.refs.get(&id)).or_else(|| parent.refs.get(&id));
    let mut out = Vec::new();
    for (child, s) in children {
        if !s.scan_child {
            continue;
        }
        let mut ids: Vec<u32> = child.refs.keys().copied().collect();
        ids.sort_unstable();
        for id in ids {
            let p = &child.refs[&id];
            // Only references that start enabled through their enable-parent chain (golden: the
            // intact Windhelm market is copied, its civil-war "Destroyed" state is not).
            if !initially_enabled(p, &lookup) {
                continue;
            }
            let Some(b) = bases.get(&p.base) else { continue };
            // Trees with an enable parent are not copied (golden: the Whiterun temple tree states
            // and two Solitude trees; every copied tree is unparented).
            if p.enable_parent && crate::assign::is_tree(b) {
                continue;
            }
            let edid = b.editor_id.to_ascii_lowercase();
            if s.ignore_edid.iter().any(|i| edid.contains(i.as_str())) {
                continue;
            }
            let local = if id >> 24 == 0xFE { id & 0xFFF } else { id & 0x00FF_FFFF };
            let matched = matches.get(&(plugin_of(id), local)).is_some_and(|ps| ps.iter().any(|(pp, pid)| !parent_removed(pp, *pid)));
            if matched || parent_keys.contains(&key(p)) || !has_lod(id, p, b) {
                continue;
            }
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
