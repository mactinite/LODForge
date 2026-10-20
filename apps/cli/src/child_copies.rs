//! `lodforge child-copies`: the child worldspace references DynDOLOD would copy into a parent
//! worldspace, optionally compared with a DynDOLOD `ChildworldCopies_<World>.txt` log.

use std::collections::HashSet;
use std::path::PathBuf;

use anyhow::{Context, Result};

use lodforge_dyndolod::childworld;
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
    /// DynDOLOD `Edit Scripts\DynDOLOD` folder (default: `<instance>\tools\DynDOLOD\Edit Scripts\DynDOLOD`).
    #[arg(long)]
    pub dyndolod: Option<PathBuf>,
    /// DynDOLOD's `DynDOLOD_SSE_ChildworldCopies_<World>.txt` to compare with.
    #[arg(long)]
    pub golden: Option<PathBuf>,
}

/// The copies for `parent` from every child worldspace with settings.
pub fn for_world(p: &Profile, c: &crate::dyndolod_build::Config, r: &lodforge_dyndolod::assign::Resolver<'_>, worlds: &[lodforge_esp::Worldspace], parent: &lodforge_esp::Worldspace, preset: &str) -> Vec<childworld::ChildCopy> {
    let (dir, bases) = (&c.dir, &c.bases);
    let children: Vec<(&lodforge_esp::Worldspace, childworld::ChildSettings)> = worlds
        .iter()
        .filter(|w| w.parent == Some(parent.form_id))
        .filter_map(|w| childworld::settings(dir, &w.editor_id, &parent.editor_id, preset, &p.plugin_names).map(|s| (w, s)))
        .collect();
    if children.is_empty() {
        return Vec::new();
    }
    let owners = lodforge_dyndolod::bases::owners(&p.load_order);
    let plugin_of = |id: u32| owners.get(&lodforge_dyndolod::bases::owner_key(id)).and_then(|&i| p.plugin_names.get(i)).map(|s| s.to_ascii_lowercase()).unwrap_or_default();
    let rules = &c.rules;
    let lod_of = |id: u32, pl: &lodforge_esp::Placement, b: &lodforge_dyndolod::bases::Base| {
        let local = if id >> 24 == 0xFE { id & 0xFFF } else { id & 0x00FF_FFFF };
        childworld::has_lod(r, rules, &plugin_of(id), local, pl, b)
    };
    childworld::copies(&p.load_order, parent, &children, bases, &childworld::known_matches(dir), &childworld::full_model_refs(dir), &lod_of, &|pl, id| childworld::removed_by_rule(rules, pl, id))
}

pub fn run(a: &Args) -> Result<()> {
    let p = a.profile.open()?;
    let dir = a.dyndolod.clone().unwrap_or_else(|| a.profile.instance.join(r"tools\DynDOLOD\Edit Scripts\DynDOLOD"));
    let worlds = scan_worldspaces(&p.load_order)?;
    let parent = worlds.iter().find(|w| w.editor_id.eq_ignore_ascii_case(&a.world)).context("worldspace not found")?;
    let c = crate::dyndolod_build::Config::load(&p, &dir, &a.preset, false)?;
    let copies = crate::dyndolod_build::with_resolver(&p, &c, |r| for_world(&p, &c, r, &worlds, parent, &a.preset));
    let bases = &c.bases;
    let name = |id: u32| worlds.iter().find(|w| w.form_id == id).map(|w| w.editor_id.clone()).unwrap_or_default();
    let mut by_world = std::collections::BTreeMap::new();
    for c in &copies {
        *by_world.entry(name(c.child_world)).or_insert(0) += 1;
    }
    println!("{} candidate copies: {by_world:?}", copies.len());
    if let Some(g) = &a.golden {
        let text = std::fs::read_to_string(g)?;
        let golden: HashSet<u32> = text.lines().filter_map(|l| l.split("[REFR:").nth(1).and_then(|s| u32::from_str_radix(&s[..8], 16).ok())).collect();
        let ours: HashSet<u32> = copies.iter().map(|c| c.ref_id).collect();
        let label = |id: &u32| {
            let c = copies.iter().find(|c| c.ref_id == *id);
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
    Ok(())
}
