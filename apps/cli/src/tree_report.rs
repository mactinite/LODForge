//! Tree report: what LOD every tree base got in a DynDOLOD-mode export, like the tree part of
//! DynDOLOD's object report, as `LODForge_TreeReport_<World>.csv` next to the build output.
//!
//! One row per tree base placed in the worldspace (after season swaps of the default season):
//! how many references use it, what each LOD level shows (a 3D LOD model, the billboard, the full
//! model or nothing) and, when LOD4 is not 3D, why. Sorted by reference count, so the trees that
//! would gain most from a generated 3D LOD model (Ultra phase 3) come first. Also available as
//! `lodforge tree-report`.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use anyhow::{Context, Result};

use lodforge_dyndolod::assign::Resolver;
use lodforge_dyndolod::export::{RefLine, Skip};
use lodforge_esp::Worldspace;

use crate::profile::{Profile, ProfileArgs};

#[derive(clap::Args)]
pub struct Args {
    #[command(flatten)]
    pub profile: ProfileArgs,
    #[arg(long, default_value = "Tamriel")]
    pub world: String,
    /// DynDOLOD's `Edit Scripts\DynDOLOD` folder (default: `<instance>\tools\DynDOLOD\Edit Scripts\DynDOLOD`).
    #[arg(long)]
    pub dyndolod_config: Option<PathBuf>,
    #[arg(long, default_value = "High")]
    pub preset: String,
    /// Classic tree LOD (billboards in the game's tree LOD) instead of Ultra.
    #[arg(long)]
    pub classic: bool,
    /// Write the CSV here (default: print the summary only).
    #[arg(long)]
    pub out: Option<PathBuf>,
    /// The mod whose files count as DynDOLOD Resources.
    #[arg(long, default_value = "DynDOLOD Resources SE 3")]
    pub resources_mod: String,
}

/// What one LOD level of a tree shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Model3d,
    Billboard,
    Full,
    None,
}

impl Kind {
    fn of(column: &str, full: &str) -> Kind {
        let c = column.to_ascii_lowercase();
        if c.is_empty() {
            Kind::None
        } else if c.ends_with(".dds") {
            Kind::Billboard
        } else if c == full.to_ascii_lowercase() {
            Kind::Full
        } else {
            Kind::Model3d
        }
    }

    fn label(self) -> &'static str {
        match self {
            Kind::Model3d => "3d",
            Kind::Billboard => "billboard",
            Kind::Full => "full",
            Kind::None => "none",
        }
    }
}

/// One tree base.
pub struct Row {
    pub plugin: String,
    pub form_id: u32,
    pub editor_id: String,
    pub model: String,
    pub crc: Option<u32>,
    pub refs: usize,
    pub kinds: [Kind; 3],
    pub lod4: String,
    pub source: String,
    pub note: String,
    /// Big enough for a generated 3D model ([generator_candidate]).
    pub candidate: bool,
    /// References drawn by the LODForge Runtime (`build --runtime lodforge`; out of `.bto`).
    pub runtime_refs: usize,
}

/// TreeLODGen's test for trees worth a 3D model (docs/re/treelodgen-algorithm.md): the model
/// path names a tree, the bounds' diagonal is at least 512 units and their top at least 256.
/// Bushes, ferns and ground cover stay billboards.
pub fn generator_candidate(model: &str, bounds: [i16; 6]) -> bool {
    let d = |k: usize| f64::from(bounds[k + 3]) - f64::from(bounds[k]);
    let diag = (d(0) * d(0) + d(1) * d(1) + d(2) * d(2)).sqrt();
    model.to_ascii_lowercase().contains("tree") && diag >= 512.0 && bounds[5] >= 256
}

/// Rows for the tree references of one export (the default season's lines and skips).
#[allow(clippy::too_many_arguments)]
pub fn rows(p: &Profile, c: &crate::dyndolod_build::Config, r: &Resolver<'_>, world: &Worldspace, lines: &[RefLine], skipped: &HashMap<u32, Skip>, mods_dir: &std::path::Path, resources_mod: &str, runtime: &HashMap<u32, usize>) -> Vec<Row> {
    // (base) -> (references, columns of the first line)
    let mut per_base: HashMap<u32, (usize, Option<[String; 4]>)> = HashMap::new();
    for l in lines {
        let Some(b) = c.bases.get(&l.base) else { continue };
        if !lodforge_dyndolod::assign::is_tree(b) {
            continue;
        }
        let e = per_base.entry(l.base).or_insert((0, None));
        e.0 += 1;
        e.1.get_or_insert_with(|| l.columns.clone());
    }
    // References that got nothing (or classic tree LOD) count under their base too.
    for (id, why) in skipped {
        let Some(pl) = world.refs.get(id) else { continue };
        let Some(b) = c.bases.get(&pl.base) else { continue };
        if !lodforge_dyndolod::assign::is_tree(b) || matches!(why, Skip::Disabled | Skip::NoBase) {
            continue;
        }
        per_base.entry(pl.base).or_insert((0, None)).0 += 1;
    }
    let mut out = Vec::new();
    for (base, (refs, columns)) in per_base {
        let b = &c.bases[&base];
        let model = lodforge_dyndolod::bases::model_path(&b.model);
        let full = lodforge_dyndolod::assign::full_model_text(&b.model);
        let crc = r.model_crc(&model);
        let cols = columns.clone().unwrap_or_default();
        let kinds = [Kind::of(&cols[0], &full), Kind::of(&cols[1], &full), Kind::of(&cols[2], &full)];
        let lod4 = cols[0].clone();
        let source = if lod4.is_empty() {
            String::new()
        } else {
            let s = crate::lod_sources::source(p, mods_dir, resources_mod, &lod4.to_ascii_lowercase());
            format!("{:?}: {}", s.group, s.from)
        };
        let stem = model.rsplit('\\').next().unwrap_or(&model).trim_end_matches(".nif").to_string();
        let note = if columns.is_none() {
            if lodforge_dyndolod::assign::is_tree(b) && r.billboard(b, &stem).is_none() { "no LOD (no billboard)".into() } else { "no LOD".into() }
        } else if kinds[0] == Kind::Model3d {
            String::new()
        } else if r.mesh_crc.replaced(&model, crc) && r.index.get(&format!("{stem}passthru_lod.nif")).is_some() {
            "full mesh replaced by a mod: the plain-named 3D model does not fit it".into()
        } else {
            "no 3D LOD model installed".into()
        };
        let owner = c.owners.get(&lodforge_dyndolod::bases::owner_key(base)).and_then(|&i| p.plugin_names.get(i)).cloned().unwrap_or_default();
        let local = if base >> 24 == 0xFE { base & 0xFFF } else { base & 0x00FF_FFFF };
        let candidate = generator_candidate(&model, b.bounds);
        out.push(Row { plugin: owner, form_id: local, editor_id: b.editor_id.clone(), model: full, crc, refs, kinds, lod4, source, note, candidate, runtime_refs: runtime.get(&base).copied().unwrap_or(0) });
    }
    out.sort_by(|a, b| b.refs.cmp(&a.refs).then_with(|| a.editor_id.cmp(&b.editor_id)));
    out
}

fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n']) { format!("\"{}\"", s.replace('"', "\"\"")) } else { s.to_string() }
}

pub fn csv(rows: &[Row]) -> String {
    let mut s = String::from("plugin,form_id,editor_id,model,model_crc32,references,lod4,lod8,lod16,lod4_model,lod4_source,note,generator_candidate,runtime_refs\r\n");
    for r in rows {
        let f = [
            csv_field(&r.plugin),
            format!("{:06X}", r.form_id),
            csv_field(&r.editor_id),
            csv_field(&r.model),
            r.crc.map(|c| format!("{c:08X}")).unwrap_or_default(),
            r.refs.to_string(),
            r.kinds[0].label().into(),
            r.kinds[1].label().into(),
            r.kinds[2].label().into(),
            csv_field(&r.lod4),
            csv_field(&r.source),
            csv_field(&r.note),
            if r.candidate { "yes".into() } else { "no".into() },
            r.runtime_refs.to_string(),
        ];
        s.push_str(&f.join(","));
        s.push_str("\r\n");
    }
    s
}

/// Summary lines: references and bases per LOD4 kind, and the most placed trees without 3D LOD.
pub fn summary(world: &str, rows: &[Row]) -> String {
    let mut by_kind: BTreeMap<Kind, (usize, usize)> = BTreeMap::new();
    for r in rows {
        let e = by_kind.entry(r.kinds[0]).or_default();
        e.0 += 1;
        e.1 += r.refs;
    }
    let total: usize = rows.iter().map(|r| r.refs).sum();
    let mut s = format!("  tree report {world}: {} tree bases, {total} references; LOD4:", rows.len());
    for (k, (bases, refs)) in &by_kind {
        s.push_str(&format!(" {} {refs} refs ({:.0}%) / {bases} bases;", k.label(), 100.0 * *refs as f64 / total.max(1) as f64));
    }
    s.push('\n');
    let runtime: usize = rows.iter().map(|r| r.runtime_refs).sum();
    if runtime > 0 {
        s.push_str(&format!("  LODForge Runtime draws {runtime} of them ({:.0}%; out of .bto)\n", 100.0 * runtime as f64 / total.max(1) as f64));
    }
    let cand: Vec<&Row> = rows.iter().filter(|r| r.kinds[0] != Kind::Model3d && r.candidate).collect();
    s.push_str(&format!("  generator candidates without a 3D LOD4 model: {} bases, {} references\n", cand.len(), cand.iter().map(|r| r.refs).sum::<usize>()));
    let top: Vec<&Row> = cand.into_iter().take(8).collect();
    if !top.is_empty() {
        s.push_str("  most placed of them:\n");
        for r in top {
            s.push_str(&format!("    {:>6}  {} ({} {:06X}) {}  [{}]\n", r.refs, r.editor_id, r.plugin, r.form_id, r.model, r.note));
        }
    }
    s
}

pub fn run(a: &Args) -> Result<()> {
    let p = a.profile.open()?;
    let dir = a.dyndolod_config.clone().unwrap_or_else(|| a.profile.instance.join(r"tools\DynDOLOD\Edit Scripts\DynDOLOD"));
    let worlds = lodforge_esp::scan_worldspaces(&p.load_order)?;
    let ws = worlds.iter().find(|w| w.editor_id.eq_ignore_ascii_case(&a.world)).context("worldspace not found")?;
    let mut c = crate::dyndolod_build::Config::load(&p, &dir, &a.preset, false)?;
    c.ultra = !a.classic;
    let h = crate::dyndolod_build::header(&p, &c, ws, "", &a.profile.game_dir().join("Data"), "");
    let rows = crate::dyndolod_build::with_resolver(&p, &c, |r| -> Result<Vec<Row>> {
        let copies = crate::child_copies::for_world(&p, &c, r, &worlds, ws, "Low");
        let we = crate::dyndolod_build::export_world(&p, &c, r, ws, &[], &h, &copies)?;
        let s = &we.seasons[0];
        let world = lodforge_dyndolod::childworld::with_copies(ws, &copies);
        Ok(rows(&p, &c, r, &world, &s.lines, &s.skipped, &a.profile.instance.join("mods"), &a.resources_mod, &HashMap::new()))
    })?;
    print!("{}", summary(&ws.editor_id, &rows));
    if let Some(o) = &a.out {
        std::fs::write(o, csv(&rows))?;
        println!("  wrote {}", o.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_from_columns() {
        let full = r"Meshes\landscape\trees\treepineforest01.nif";
        assert_eq!(Kind::of(r"meshes\dyndolod\lod\trees\treepineforest01passthru_lod.nif", full), Kind::Model3d);
        assert_eq!(Kind::of(r"textures\terrain\lodgen\skyrim.esm\treepineforest01_0006f7dc.dds", full), Kind::Billboard);
        assert_eq!(Kind::of(r"meshes\landscape\trees\TreePineForest01.nif", full), Kind::Full);
        assert_eq!(Kind::of("", full), Kind::None);
    }

    #[test]
    fn shrubs_are_not_candidates() {
        assert!(generator_candidate(r"landscape\trees\treepineforest01.nif", [-300, -300, -20, 300, 300, 1800]));
        assert!(!generator_candidate(r"landscape\plants\thicket01.nif", [-300, -300, -20, 300, 300, 400]));
        assert!(!generator_candidate(r"landscape\trees\treesapling01.nif", [-60, -60, 0, 60, 60, 200]));
    }
}
