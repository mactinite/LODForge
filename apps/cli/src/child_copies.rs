//! `lodforge child-copies`: the child worldspace references DynDOLOD would copy into a parent
//! worldspace, optionally compared with a DynDOLOD `ChildworldCopies_<World>.txt` log, and the
//! floating ones LODForge leaves out of that selection (`childworld::drop_floating`).

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

use anyhow::{Context, Result};

use lodforge_dyndolod::childworld::{self, ChildCopy, Floating};
use lodforge_esp::scan_worldspaces;

use crate::profile::{Profile, ProfileArgs};

#[derive(clap::Args)]
pub struct Args {
    #[command(flatten)]
    pub profile: ProfileArgs,
    #[arg(long, default_value = "Tamriel")]
    pub world: String,
    #[arg(long, default_value = "Medium")]
    pub preset: String,
    /// DynDOLOD's "Child > parent" setting (`IgnoreChildEDID<value>`; default: the preset), as
    /// `build --child-parent`.
    #[arg(long)]
    pub child_parent: Option<String>,
    /// Keep the floating copies (small models with no LOD surface under them), as
    /// `build --child-keep-floating`.
    #[arg(long)]
    pub keep_floating: bool,
    /// Floating rule: largest model extent looked at (default 150), clearance above the ground and
    /// around a LOD surface (default 48), slack around a support's box (default 16), game units.
    #[arg(long)]
    pub max_extent: Option<f32>,
    #[arg(long)]
    pub clearance: Option<f32>,
    #[arg(long)]
    pub margin: Option<f32>,
    /// DynDOLOD `Edit Scripts\DynDOLOD` folder (default: `<instance>\tools\DynDOLOD\Edit Scripts\DynDOLOD`).
    #[arg(long)]
    pub dyndolod: Option<PathBuf>,
    /// DynDOLOD's `DynDOLOD_SSE_ChildworldCopies_<World>.txt` to compare with (against the
    /// selection before the floating rule, which is DynDOLOD's own).
    #[arg(long)]
    pub golden: Option<PathBuf>,
    /// A DynDOLOD.esm to resolve the copies' `_DynDOLOD_CHILD` references against, as the
    /// runtime statics export does when the build writes no plugins (instance-formats.md
    /// section 6): prints how many copies it names and which it lacks.
    #[arg(long)]
    pub esm: Option<PathBuf>,
}

/// The copies for `parent` from every child worldspace with settings, less the floating ones
/// (`Config::child_floating`).
pub fn for_world(p: &Profile, c: &crate::dyndolod_build::Config, r: &lodforge_dyndolod::assign::Resolver<'_>, worlds: &[lodforge_esp::Worldspace], parent: &lodforge_esp::Worldspace, preset: &str) -> Vec<ChildCopy> {
    for_world_report(p, c, r, worlds, parent, preset).0
}

/// [`for_world`] with the floating copies it left out.
pub fn for_world_report(p: &Profile, c: &crate::dyndolod_build::Config, r: &lodforge_dyndolod::assign::Resolver<'_>, worlds: &[lodforge_esp::Worldspace], parent: &lodforge_esp::Worldspace, preset: &str) -> (Vec<ChildCopy>, Vec<Floating>) {
    let (dir, bases) = (&c.dir, &c.bases);
    let children: Vec<(&lodforge_esp::Worldspace, childworld::ChildSettings)> = worlds
        .iter()
        .filter(|w| w.parent == Some(parent.form_id))
        .filter_map(|w| childworld::settings(dir, &w.editor_id, &parent.editor_id, preset, &p.plugin_names).map(|s| (w, s)))
        .collect();
    if children.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let owners = lodforge_dyndolod::bases::owners(&p.load_order);
    let plugin_of = |id: u32| owners.get(&lodforge_dyndolod::bases::owner_key(id)).and_then(|&i| p.plugin_names.get(i)).map(|s| s.to_ascii_lowercase()).unwrap_or_default();
    let local_of = |id: u32| if id >> 24 == 0xFE { id & 0xFFF } else { id & 0x00FF_FFFF };
    let rules = &c.rules;
    let lod_of = |id: u32, pl: &lodforge_esp::Placement, b: &lodforge_dyndolod::bases::Base| {
        let local = local_of(id);
        let ok = childworld::has_lod(r, rules, &plugin_of(id), local, pl, b);
        if std::env::var("LODFORGE_CHILD_TRACE").ok().and_then(|s| u32::from_str_radix(s.trim(), 16).ok()) == Some(id) {
            let lod = r.resolve(b);
            let rr = lodforge_dyndolod::rules::select_reference_rule(rules, &plugin_of(id), local);
            eprintln!("child copy {id:08X}: model {} flags {:X} reference rule {:?} base rule {:?} columns {:?}", b.model, pl.flags, rr.map(|x| &x.reference_action), lod.rule.map(|i| (&rules[i].mask, &rules[i].levels, &rules[i].source)), lod.columns);
        }
        ok
    };
    let full = childworld::full_model_refs(dir);
    let copies = childworld::copies(&p.load_order, parent, &children, bases, &childworld::known_matches(dir), &full, &lod_of, &|pl, id| childworld::removed_by_rule(rules, pl, id));
    let Some(rule) = &c.child_floating else { return (copies, Vec::new()) };
    if copies.is_empty() {
        return (copies, Vec::new());
    }
    // What a copy may stand on besides another copy: the parent's own references with LOD in the
    // cells the copies touch and their neighbours (city walls, the parent's stand-ins of the city).
    let cell_of = |pos: [f32; 3]| ((pos[0] / 4096.0).floor() as i32, (pos[1] / 4096.0).floor() as i32);
    let mut near: HashSet<(i32, i32)> = HashSet::new();
    for cp in &copies {
        let (x, y) = cell_of(cp.placement.position);
        for dx in -2..=2 {
            for dy in -2..=2 {
                near.insert((x + dx, y + dy));
            }
        }
    }
    let parent_lod: Vec<(u32, &lodforge_esp::Placement)> = parent
        .refs
        .iter()
        .filter(|(_, pl)| near.contains(&cell_of(pl.position)))
        .filter(|(id, pl)| bases.get(&pl.base).is_some_and(|b| lod_of(**id, pl, b)))
        .map(|(id, pl)| (*id, pl))
        .collect();
    let heights = childworld::LandHeights::new(&p.load_order, parent);
    // The LOD surface of a support: its LOD4 model (the first level with one; the full model of
    // a `childworldfull.txt` copy) placed like the reference, triangles cached per model.
    let meshes: RefCell<HashMap<String, Option<Rc<Vec<[[f32; 3]; 3]>>>>> = RefCell::default();
    let triangles = |path: &str| -> Option<Rc<Vec<[[f32; 3]; 3]>>> {
        let key = path.to_ascii_lowercase().replace('/', "\\");
        if let Some(t) = meshes.borrow().get(&key) {
            return t.clone();
        }
        let t = p.vfs.read(&key).ok().flatten().and_then(|bytes| lodforge_objlod::mesh::import(&key, &bytes).ok()).map(|m| Rc::new(childworld::mesh_triangles(&m)));
        meshes.borrow_mut().insert(key, t.clone());
        t
    };
    let lod_under = |id: u32, pl: &lodforge_esp::Placement, b: &lodforge_dyndolod::bases::Base, xy: [f32; 2]| -> Vec<f32> {
        if lodforge_dyndolod::assign::is_tree(b) {
            return Vec::new();
        }
        let (plugin, local) = (plugin_of(id), local_of(id));
        let model = if full.contains(&(plugin.clone(), local)) {
            let m = b.model.replace('/', "\\");
            Some(if m.to_ascii_lowercase().starts_with("meshes\\") { m } else { format!("meshes\\{m}") })
        } else {
            let lod = r.resolve(b);
            let cols = match lodforge_dyndolod::rules::select_reference_rule(rules, &plugin, local) {
                Some(rule) => r.columns(rule, b, &lod),
                None => lod.columns.clone(),
            };
            cols.into_iter().find(|c| !c.is_empty())
        };
        match model.and_then(|m| triangles(&m)) {
            Some(t) => childworld::surface_crossings(&t, &childworld::placed(pl), xy[0], xy[1]),
            None => Vec::new(),
        }
    };
    childworld::drop_floating(copies, bases, &parent_lod, &|x, y| heights.height_at(x, y), &lod_under, rule)
}

pub fn run(a: &Args) -> Result<()> {
    let p = a.profile.open()?;
    let dir = a.dyndolod.clone().unwrap_or_else(|| a.profile.instance.join(r"tools\DynDOLOD\Edit Scripts\DynDOLOD"));
    let worlds = scan_worldspaces(&p.load_order)?;
    let parent = worlds.iter().find(|w| w.editor_id.eq_ignore_ascii_case(&a.world)).context("worldspace not found")?;
    let mut c = crate::dyndolod_build::Config::load(&p, &dir, &a.preset, false)?;
    if a.keep_floating {
        c.child_floating = None;
    }
    if let Some(rule) = c.child_floating.as_mut() {
        rule.max_extent = a.max_extent.unwrap_or(rule.max_extent);
        rule.clearance = a.clearance.unwrap_or(rule.clearance);
        rule.margin = a.margin.unwrap_or(rule.margin);
    }
    let (copies, dropped) = crate::dyndolod_build::with_resolver(&p, &c, |r| for_world_report(&p, &c, r, &worlds, parent, a.child_parent.as_deref().unwrap_or(&a.preset)));
    let bases = &c.bases;
    let name = |id: u32| worlds.iter().find(|w| w.form_id == id).map(|w| w.editor_id.clone()).unwrap_or_default();
    let mut by_world = BTreeMap::new();
    for c in &copies {
        *by_world.entry(name(c.child_world)).or_insert(0) += 1;
    }
    println!("{} candidate copies: {by_world:?}", copies.len());
    if c.child_floating.is_some() {
        let mut gone = BTreeMap::new();
        for f in &dropped {
            *gone.entry(name(f.copy.child_world)).or_insert(0) += 1;
        }
        println!("{} floating copies left out (small models with no LOD surface under them, docs/re/dyndolod-spec.md; --keep-floating keeps them): {gone:?}", dropped.len());
        for f in dropped.iter().take(60) {
            let pos = f.copy.placement.position;
            println!("  {:08X} {} ({}) extent {:.0}, {:.0} above the terrain, at ({:.0}, {:.0}, {:.0})", f.copy.ref_id, bases.get(&f.copy.placement.base).map(|b| b.editor_id.as_str()).unwrap_or(""), name(f.copy.child_world), f.extent, f.above_terrain, pos[0], pos[1], pos[2]);
        }
    }
    // DynDOLOD's own selection: the floating copies included.
    let selected: Vec<&ChildCopy> = copies.iter().chain(dropped.iter().map(|f| &f.copy)).collect();
    if let Some(g) = &a.golden {
        let text = std::fs::read_to_string(g)?;
        let golden: HashSet<u32> = text.lines().filter_map(|l| l.split("[REFR:").nth(1).and_then(|s| u32::from_str_radix(&s[..8], 16).ok())).collect();
        let ours: HashSet<u32> = selected.iter().map(|c| c.ref_id).collect();
        let label = |id: &u32| {
            let c = selected.iter().find(|c| c.ref_id == *id);
            let base = worlds.iter().find_map(|w| w.refs.get(id)).map(|r| r.base);
            let pl = worlds.iter().find_map(|w| w.refs.get(id));
            format!("{id:08X} {} xesp={:?} flags={:X} {}", c.map(|c| name(c.child_world)).unwrap_or_default(), pl.map(|p| (p.enable_parent_id, p.enable_parent_flags)), pl.map_or(0, |p| p.flags), base.and_then(|b| bases.get(&b)).map(|b| format!("{} {}", String::from_utf8_lossy(&b.signature), b.editor_id)).unwrap_or_default())
        };
        let mut only_g: Vec<&u32> = golden.difference(&ours).collect();
        let mut only_o: Vec<&u32> = ours.difference(&golden).collect();
        only_g.sort();
        only_o.sort();
        println!("golden {}, ours {}, both {}", golden.len(), ours.len(), golden.intersection(&ours).count());
        println!("golden only ({}): {}", only_g.len(), only_g.iter().take(30).map(|i| label(i)).collect::<Vec<_>>().join("; "));
        println!("ours only ({}): {}", only_o.len(), only_o.iter().take(60).map(|i| label(i)).collect::<Vec<_>>().join("; "));
    }
    if let Some(esm) = &a.esm {
        use lodforge_dyndolod::dynplugins::{ChildCopyIds, child_copy_editor_id, plugin_name_of};
        let bytes = std::fs::read(esm).with_context(|| format!("reading {}", esm.display()))?;
        let prev = lodforge_dyndolod::plugin::Previous::read(&bytes).context("not a plugin")?;
        let mut ids = ChildCopyIds::new(Some(&prev), false);
        let map = ids.assign(&p.load_order, parent, &copies).clone();
        let no_cell = copies.iter().filter(|c| !parent.cell_records.contains_key(&((c.placement.position[0] / 4096.0).floor() as i32, (c.placement.position[1] / 4096.0).floor() as i32))).count();
        println!(
            "{}: author {}, {} records, next object id {:X}; {} copies named by a _DynDOLOD_CHILD record, {} without one, {} in cells the worldspace has no record for (no copy there)",
            esm.display(),
            prev.author,
            prev.records,
            prev.next_object_id,
            ids.kept,
            ids.unresolved,
            no_cell
        );
        let owners = lodforge_dyndolod::bases::owners(&p.load_order);
        let mut missing: Vec<String> = copies
            .iter()
            .filter(|c| !map.contains_key(&c.copy_id))
            .map(|c| {
                let (local, plugin) = plugin_name_of(&p.load_order, &owners, c.ref_id);
                format!("{:08X} {} ({}{})", c.ref_id, child_copy_editor_id(&plugin, local), name(c.child_world), if c.large { ", large" } else { "" })
            })
            .collect();
        missing.sort();
        if !missing.is_empty() {
            println!("without a record ({}): {}", missing.len(), missing.iter().take(40).cloned().collect::<Vec<_>>().join("; "));
        }
        let mut sample: Vec<(u32, u32)> = map.iter().map(|(&c, &o)| (c, o)).collect();
        sample.sort_unstable();
        println!("first ids: {}", sample.iter().take(8).map(|(c, o)| format!("{c:08X}->{o:06X}")).collect::<Vec<_>>().join(", "));
    }
    Ok(())
}
