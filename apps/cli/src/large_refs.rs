//! `lodforge large-refs`: large references (WRLD RNAM) of a worldspace per cell, and how a
//! DynDOLOD output's large-reference workarounds (`[Markers]`, `[Resets]`, DynDOLOD.esm) line up
//! with them (diagnostics for the workarounds).

use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::profile::ProfileArgs;

#[derive(clap::Args)]
pub struct Args {
    #[command(flatten)]
    pub profile: ProfileArgs,
    #[arg(long, default_value = "Tamriel")]
    pub world: String,
    /// A DynDOLOD (or LODForge) output Data folder to line up with.
    #[arg(long)]
    pub output: Option<PathBuf>,
    /// With --output: compare its workarounds with this DynDOLOD output instead.
    #[arg(long)]
    pub golden: Option<PathBuf>,
}

/// `[Markers]` of a data file: (world form text, x, y) -> (count, trailing items).
fn marker_map(text: &str) -> BTreeMap<(String, i32, i32), (String, Vec<String>)> {
    section(text, "Markers")
        .into_iter()
        .filter(|m| m.len() >= 7)
        .filter_map(|m| {
            let (x, y) = (m[2].parse().ok()?, m[3].parse().ok()?);
            let mut t: Vec<String> = m[7..].iter().map(|s| s.to_ascii_lowercase()).collect();
            t.sort();
            Some(((format!("{},{}", m[0], m[1].to_ascii_lowercase()), x, y), (m[6].clone(), t)))
        })
        .collect()
}

/// Ours (`out`) against a DynDOLOD output: marker cells per worldspace, `[Markers]` counts and
/// persistent lists, `[Resets]`.
fn compare(out: &std::path::Path, golden: &std::path::Path, world: &lodforge_esp::Worldspace, children: &[&lodforge_esp::Worldspace]) -> Result<()> {
    let txt = |root: &std::path::Path| lodforge_dyndolod::ini::decode(&std::fs::read(root.join("SKSE").join("Plugins").join("DynDOLOD_Data").join(format!("DynDOLOD_NG_{}.txt", world.editor_id))).unwrap_or_default());
    let (to, tg) = (txt(out), txt(golden));
    let mut total = [0usize; 3];
    for w in std::iter::once(world).chain(children.iter().copied()) {
        let o: HashSet<(i32, i32)> = disciple_cells(&out.join("DynDOLOD.esm"), &w.editor_id).into_iter().collect();
        let g: HashSet<(i32, i32)> = disciple_cells(&golden.join("DynDOLOD.esm"), &w.editor_id).into_iter().collect();
        if o.is_empty() && g.is_empty() {
            continue;
        }
        let common = o.intersection(&g).count();
        println!("  markers {:28} ours {:5} golden {:5} common {:5}", w.editor_id, o.len(), g.len(), common);
        total[0] += o.len();
        total[1] += g.len();
        total[2] += common;
    }
    println!("  markers total: ours {} golden {} common {}", total[0], total[1], total[2]);
    let (mo, mg) = (marker_map(&to), marker_map(&tg));
    let common: Vec<_> = mo.keys().filter(|k| mg.contains_key(*k)).collect();
    let same_count = common.iter().filter(|k| mo[**k].0 == mg[**k].0).count();
    let same_list = common.iter().filter(|k| mo[**k].1 == mg[**k].1).count();
    let items = |m: &BTreeMap<(String, i32, i32), (String, Vec<String>)>| m.values().map(|v| v.1.len()).sum::<usize>();
    println!("  [Markers] lines: ours {} golden {} common {}; same count {same_count}, same persistent list {same_list}; persistent items ours {} golden {}", mo.len(), mg.len(), common.len(), items(&mo), items(&mg));
    let resets = |t: &str| -> HashSet<String> { section(t, "Resets").into_iter().map(|r| r.join(",").to_ascii_lowercase()).collect() };
    let (ro, rg) = (resets(&to), resets(&tg));
    println!("  [Resets]: ours {} golden {} common {}", ro.len(), rg.len(), ro.intersection(&rg).count());
    Ok(())
}

/// `(form id, plugin)` items of a data file section.
pub fn section(text: &str, name: &str) -> Vec<Vec<String>> {
    let mut on = false;
    let mut out = Vec::new();
    for l in text.lines() {
        let l = l.trim();
        if l.starts_with('[') {
            on = l.eq_ignore_ascii_case(&format!("[{name}]"));
            continue;
        }
        if on && !l.is_empty() {
            out.push(l.split(',').map(str::to_string).collect());
        }
    }
    out
}

pub fn run(a: &Args) -> Result<()> {
    let p = a.profile.open()?;
    let lo = &p.load_order;
    let worlds = lodforge_esp::scan_worldspaces(lo)?;
    let world = worlds.iter().find(|w| w.editor_id.eq_ignore_ascii_case(&a.world)).context("no such worldspace")?;
    let gid = |plugin: &str, local: &str| local.parse::<u32>().ok().and_then(|l| lodforge_dyndolod::bases::global_id(lo, plugin, l));
    let is_master = |pi: u16| lo.plugins[pi as usize].plugin.is_master();
    // Cells with large references: every RNAM entry, the master-listed ones, and the ones
    // DynDOLOD treats as valid (master-listed, still in their cell, not overridden by an ESP).
    let valid = crate::dyndolod_build::large_refs(lo, world);
    let bases = lodforge_dyndolod::bases::read_bases(lo)?;
    let mut all: BTreeMap<(i32, i32), HashSet<u32>> = BTreeMap::new();
    let mut master: BTreeMap<(i32, i32), HashSet<u32>> = BTreeMap::new();
    let mut good: BTreeMap<(i32, i32), HashSet<u32>> = BTreeMap::new();
    for &(pi, g, r) in &world.large_refs {
        all.entry(g).or_default().insert(r);
        if is_master(pi) {
            master.entry(g).or_default().insert(r);
        }
        if valid.contains(&r) {
            good.entry(g).or_default().insert(r);
        }
    }
    println!("{}: {} RNAM entries in {} cells; master-listed in {} cells; valid {} refs in {} cells", world.editor_id, world.large_refs.len(), all.len(), master.len(), valid.len(), good.len());
    let Some(out) = &a.output else { return Ok(()) };
    if let Some(g) = &a.golden {
        let children: Vec<&lodforge_esp::Worldspace> = worlds.iter().filter(|w| w.parent == Some(world.form_id)).collect();
        return compare(out, g, world, &children);
    }
    let dir = out.join("SKSE").join("Plugins").join("DynDOLOD_Data");
    let text = lodforge_dyndolod::ini::decode(&std::fs::read(dir.join(format!("DynDOLOD_NG_{}.txt", world.editor_id))).unwrap_or_default());
    let markers = section(&text, "Markers");
    let mut cells: HashSet<(i32, i32)> = HashSet::new();
    let mut diff: BTreeMap<String, usize> = BTreeMap::new();
    let mut extra_props: BTreeMap<String, usize> = BTreeMap::new();
    for m in &markers {
        if m.len() < 7 || gid(&m[1], &m[0]) != Some(world.form_id) {
            continue;
        }
        let (Ok(x), Ok(y), Ok(n)) = (m[2].parse::<i32>(), m[3].parse::<i32>(), m[6].parse::<usize>()) else { continue };
        cells.insert((x, y));
        let c = |t: &BTreeMap<(i32, i32), HashSet<u32>>| t.get(&(x, y)).map_or(0, HashSet::len) as i64;
        let in_cell = world.refs.values().filter(|r| r.cell == Some((x, y))).count() as i64;
        *diff.entry(format!("n-all {:+}", n as i64 - c(&all))).or_default() += 1;
        *diff.entry(format!("n-master {:+}", n as i64 - c(&master))).or_default() += 1;
        *diff.entry(format!("n-valid {:+}", n as i64 - c(&good))).or_default() += 1;
        *diff.entry(format!("n-temporary refs {:+}", (n as i64 - in_cell).clamp(-3, 3))).or_default() += 1;
        // Trailing `plugin;id` items (tokenised loosely: every `;`-joined pair of tokens).
        let rest = m[7..].join(",");
        for item in rest.split(',').filter(|s| !s.is_empty()) {
            let parts: Vec<&str> = item.split(';').collect();
            if parts.len() != 2 {
                *extra_props.entry(format!("token {item:?}")).or_default() += 1;
                continue;
            }
            let id = gid(parts[0], parts[1]);
            let r = id.and_then(|i| world.refs.get(&i));
            let key = match (id, r) {
                (None, _) => "unresolved".to_string(),
                (Some(_), None) => "not in world".to_string(),
                (Some(i), Some(r)) => format!(
                    "large={} rnam={} same cell={} disabled={} parent={} persistent={} winner={}",
                    u8::from(valid.contains(&i)),
                    u8::from(world.large_refs.iter().any(|l| l.2 == i)),
                    u8::from(r.cell == Some((x, y))),
                    u8::from(r.flags & 0x800 != 0),
                    u8::from(r.enable_parent),
                    u8::from(r.is_persistent()),
                    if is_master(r.plugin) { "master" } else { "esp" }
                ),
            };
            *extra_props.entry(key).or_default() += 1;
        }
    }
    // Features per cell (by the references' positions) against marker membership.
    if std::env::var_os("LODFORGE_MARKERFEAT").is_some() {
        let mut ncount: std::collections::HashMap<(i32, i32), usize> = markers.iter().filter(|m| m.len() >= 7 && gid(&m[1], &m[0]) == Some(world.form_id)).filter_map(|m| Some(((m[2].parse().ok()?, m[3].parse().ok()?), m[6].parse().ok()?))).collect();
        for c in disciple_cells(&out.join("DynDOLOD.esm"), &world.editor_id) {
            ncount.entry(c).or_insert(0);
        }
        let ini = a.profile.instance.join(format!(r"tools\DynDOLOD\Edit Scripts\DynDOLOD\Configs\DynDOLOD_SSE_childworld_{}.ini", world.editor_id));
        if let Some(b) = std::fs::read_to_string(&ini).ok().and_then(|t| t.lines().find_map(|l| l.trim().strip_prefix("Bounds=").map(str::to_string))) {
            let v: Vec<i32> = b.split(',').filter_map(|s| s.trim().parse().ok()).collect();
            let rect: Vec<(i32, i32)> = (v[0] - 3..=v[2] + 3).flat_map(|x| (v[1] - 3..=v[3] + 3).map(move |y| (x, y))).collect();
            let pred: HashSet<(i32, i32)> = rect.iter().copied().filter(|c| !world.navmesh_cells.contains(c)).collect();
            let mk: HashSet<(i32, i32)> = ncount.keys().copied().collect();
            let mut only_p: Vec<_> = pred.difference(&mk).collect();
            only_p.sort();
            let mut only_m: Vec<_> = mk.difference(&pred).collect();
            only_m.sort();
            println!("  Bounds {b} +-3 minus NAVM cells: predicted {}, markers {}, common {}; predicted only {only_p:?}; markers only {only_m:?}", pred.len(), mk.len(), pred.intersection(&mk).count());
        }
        if let Some((lo_b, hi_b)) = world.object_bounds {
            let (x0, y0, x1, y1) = ((lo_b[0] / 4096.0).floor() as i32, (lo_b[1] / 4096.0).floor() as i32, (hi_b[0] / 4096.0).floor() as i32, (hi_b[1] / 4096.0).floor() as i32);
            let inside = ncount.keys().filter(|c| (x0..=x1).contains(&c.0) && (y0..=y1).contains(&c.1)).count();
            let mut missing: Vec<(i32, i32)> = (x0..=x1).flat_map(|x| (y0..=y1).map(move |y| (x, y))).filter(|c| !ncount.contains_key(c)).collect();
            missing.sort();
            let _ = missing;
            let ec: Vec<&(i32, i32)> = world.cells.keys().collect();
            let bb = |v: &[&(i32, i32)]| (v.iter().map(|c| c.0).min(), v.iter().map(|c| c.0).max(), v.iter().map(|c| c.1).min(), v.iter().map(|c| c.1).max());
            let landed: Vec<&(i32, i32)> = world.cells.keys().filter(|c| world.lands.values().any(|l| l.cell == **c)).collect();
            let mk: Vec<&(i32, i32)> = ncount.keys().collect();
            println!("  object bounds cells x {x0}..{x1} y {y0}..{y1}: markers inside {inside}; bbox cells {:?}, with land ({}) {:?}, markers {:?}", bb(&ec), landed.len(), bb(&landed), bb(&mk));
        }
        println!("  marker cells (txt + esm Disciple EditorIDs): {}; world cells {}, marker cells among them {}; parent {:?} flags {:02X}", ncount.len(), world.cells.len(), ncount.keys().filter(|c| world.cells.contains_key(c)).count(), world.parent, world.parent_flags);
        let pos_cell = |r: &lodforge_esp::Placement| ((r.position[0] / 4096.0).floor() as i32, (r.position[1] / 4096.0).floor() as i32);
        let mut feat: BTreeMap<(i32, i32), [usize; 6]> = BTreeMap::new();
        for (id, r) in &world.refs {
            let c = pos_cell(r);
            let e = feat.entry(c).or_default();
            let large = valid.contains(id);
            let rnam = world.large_refs.iter().any(|l| l.2 == *id);
            e[0] += usize::from(r.cell.is_some());
            e[1] += usize::from(large);
            e[2] += usize::from(rnam && !large);
            e[3] += usize::from(large && r.flags & 0x800 != 0);
            e[4] += usize::from(large && r.is_persistent());
            e[5] += usize::from(large && r.cell.is_some());
        }
        // By RNAM entry cell: [listed, missing from the world (deleted / not a REFR), not valid,
        // initially disabled, persistent, has an enable parent, winning override from an ESP]
        let mut ent: BTreeMap<(i32, i32), [usize; 7]> = BTreeMap::new();
        for (id, c) in &world.large_ref_cells {
            let e = ent.entry(*c).or_default();
            e[0] += 1;
            match world.refs.get(id) {
                None => e[1] += 1,
                Some(r) => {
                    e[2] += usize::from(!valid.contains(id));
                    e[3] += usize::from(r.flags & 0x800 != 0);
                    e[4] += usize::from(r.is_persistent() && r.flags & 0x10000 == 0);
                    e[5] += usize::from(r.enable_parent);
                    e[6] += usize::from(!is_master(r.plugin));
                }
            }
        }
        let mut he: BTreeMap<String, [usize; 2]> = BTreeMap::new();
        for (c, e) in &ent {
            let k = format!("missing={} invalid={} disabled={} persistent={} parent={} esp={}", u8::from(e[1] > 0), u8::from(e[2] > 0), u8::from(e[3] > 0), u8::from(e[4] > 0), u8::from(e[5] > 0), u8::from(e[6] > 0));
            he.entry(k).or_default()[usize::from(ncount.contains_key(c))] += 1;
        }
        for (k, v) in he {
            println!("  entries {k}: no marker {}, marker {}", v[0], v[1]);
        }
        // Unexplained cells (no parent / disabled / ESP among the listed refs): what DynDOLOD did
        // there, by golden section.
        let ids_of = |name: &str, f: usize| -> HashSet<u32> { section(&text, name).iter().filter(|r| r.len() > f + 1).filter_map(|r| gid(&r[f + 1], &r[f])).collect() };
        let (objs, dis, res) = (ids_of("Objects", 14), ids_of("Disabled", 0), ids_of("Resets", 0));
        let pos_of = |id: &u32| world.refs.get(id).map(pos_cell);
        let in_cells = |s: &HashSet<u32>| -> HashSet<(i32, i32)> { s.iter().filter_map(pos_of).collect() };
        let large_objs: HashSet<u32> = objs.iter().copied().filter(|i| valid.contains(i)).collect();
        let (c_obj, c_lobj, c_dis, c_res) = (in_cells(&objs), in_cells(&large_objs), in_cells(&dis), in_cells(&res));
        let mut hu: BTreeMap<String, [usize; 2]> = BTreeMap::new();
        for (c, e) in &ent {
            if e[3] + e[5] + e[6] > 0 {
                continue;
            }
            let k = format!("persistent={} objects={} large objects={} disabled={} resets={}", u8::from(e[4] > 0), u8::from(c_obj.contains(c)), u8::from(c_lobj.contains(c)), u8::from(c_dis.contains(c)), u8::from(c_res.contains(c)));
            hu.entry(k).or_default()[usize::from(ncount.contains_key(c))] += 1;
        }
        // Trailing items against persistent large references listed in the marker's cell.
        let mut trailing: HashSet<u32> = HashSet::new();
        for m in markers.iter().filter(|m| m.len() > 7 && gid(&m[1], &m[0]) == Some(world.form_id)) {
            for item in &m[7..] {
                if let Some((pl, id)) = item.split_once(';')
                    && let Some(g) = gid(pl, id)
                {
                    trailing.insert(g);
                }
            }
        }
        let mut ht: BTreeMap<String, [usize; 2]> = BTreeMap::new();
        for (id, c) in &world.large_ref_cells {
            let Some(r) = world.refs.get(id) else { continue };
            if !r.is_persistent() || !ncount.contains_key(c) {
                continue;
            }
            let k = format!("disabled={} parent={} fulllod={} esp={} valid={}", u8::from(r.flags & 0x800 != 0), u8::from(r.enable_parent), u8::from(r.flags & 0x10000 != 0), u8::from(!is_master(r.plugin)), u8::from(valid.contains(id)));
            let listed = trailing.contains(id);
            if !r.enable_parent && is_master(r.plugin) && r.flags & 0x10000 == 0 && (r.flags & 0x800 != 0) != listed {
                println!("    odd {} {c:?} {id:08X} {} [{}] flags {:08X} base flags {:08X}", if listed { "listed enabled " } else { "unlisted disabled" }, bases.get(&r.base).map(|b| b.editor_id.as_str()).unwrap_or("?"), p.plugin_names.get(r.plugin as usize).map(String::as_str).unwrap_or("?"), r.flags, bases.get(&r.base).map_or(0, |b| b.flags));
            }
            ht.entry(k).or_default()[usize::from(listed)] += 1;
        }
        println!("  trailing items {}; persistent large refs in marker cells:", trailing.len());
        for (k, v) in ht {
            println!("    {k}: not listed {}, listed {}", v[0], v[1]);
        }
        // Is this ref some reference's enable parent?
        let parents: HashSet<u32> = world.refs.values().filter(|r| r.enable_parent).map(|r| r.enable_parent_id).collect();
        let mut shown = [0usize; 2];
        for (c, e) in &ent {
            if e[3] + e[5] + e[6] > 0 || e[4] == 0 {
                continue;
            }
            let m = usize::from(ncount.contains_key(c));
            shown[m] += 1;
            if shown[m] > 25 {
                continue;
            }
            let pers: Vec<String> = world.large_ref_cells.iter().filter(|(_, g)| *g == c).filter_map(|(id, _)| world.refs.get(id).filter(|r| r.is_persistent()).map(|r| (id, r))).map(|(id, r)| {
                format!("{id:08X} {} [{}] parent-of={} flags {:08X}", bases.get(&r.base).map(|b| b.editor_id.as_str()).unwrap_or("?"), p.plugin_names.get(r.plugin as usize).map(String::as_str).unwrap_or("?"), u8::from(parents.contains(id)), r.flags)
            }).collect();
            println!("  pers {} {c:?}: {}", if m == 1 { "MARKER" } else { "none  " }, pers.join("; "));
        }
        for (k, v) in hu {
            println!("  rest {k}: no marker {}, marker {}", v[0], v[1]);
        }
        let mut h: BTreeMap<String, [usize; 2]> = BTreeMap::new();
        let mut nd: BTreeMap<String, usize> = BTreeMap::new();
        for (c, f) in &feat {
            let k = format!("invalid_rnam>0={} disabled_large>0={} persistent_large>0={} temp_large>0={}", u8::from(f[2] > 0), u8::from(f[3] > 0), u8::from(f[4] > 0), u8::from(f[5] > 0));
            h.entry(k).or_default()[usize::from(ncount.contains_key(c))] += 1;
            if let Some(&n) = ncount.get(c)
                && std::env::var_os("LODFORGE_MARKERROWS").is_some()
            {
                let rn = all.get(c).map_or(0, HashSet::len);
                let in_cell: Vec<&lodforge_esp::Placement> = world.refs.values().filter(|r| pos_cell(r) == *c).collect();
                let st = in_cell.iter().filter(|r| bases.get(&r.base).is_some_and(|b| &b.signature == b"STAT" || &b.signature == b"MSTT")).count();
                println!("  cell {c:?} n {n} rnam {rn} refs {} temp {} stat/mstt {st} f {f:?}", in_cell.len(), in_cell.iter().filter(|r| r.cell.is_some()).count());
            }
            if let Some(&n) = ncount.get(c) {
                *nd.entry(format!("n-temp_large {:+}", (n as i64 - f[5] as i64).clamp(-5, 5))).or_default() += 1;
                *nd.entry(format!("n-large {:+}", (n as i64 - f[1] as i64).clamp(-5, 5))).or_default() += 1;
                *nd.entry(format!("n-temp {:+}", (n as i64 - f[0] as i64).clamp(-5, 5))).or_default() += 1;
                let e = world.large_ref_cells.values().filter(|g| *g == c).count() as i64;
                *nd.entry(format!("n-entries {:+}", (n as i64 - e).clamp(-5, 5))).or_default() += 1;
            }
        }
        for (k, v) in h {
            println!("  feat {k}: no marker {}, marker {}", v[0], v[1]);
        }
        for (k, v) in nd {
            println!("  {k}: {v}");
        }
    }
    let m_cells: HashSet<(i32, i32)> = master.keys().copied().collect();
    let g_cells: HashSet<(i32, i32)> = good.keys().copied().collect();
    let a_cells: HashSet<(i32, i32)> = all.keys().copied().collect();
    println!("[Markers] {} lines for this world in {} cells", markers.len(), cells.len());
    for (name, s) in [("all RNAM", &a_cells), ("master RNAM", &m_cells), ("valid", &g_cells)] {
        println!("  vs cells with {name} large refs ({}): common {}, markers only {}, {name} only {}", s.len(), cells.intersection(s).count(), cells.difference(s).count(), s.difference(&cells).count());
    }
    for (k, v) in &diff {
        println!("  {k}: {v}");
    }
    println!("  trailing items:");
    for (k, v) in &extra_props {
        println!("    {v:5} {k}");
    }
    // Overrides of existing references in the output's plugins: global id -> (plugin, EditorID
    // suffix, flags).
    let mut overrides: std::collections::HashMap<u32, Vec<String>> = std::collections::HashMap::new();
    for name in ["DynDOLOD.esm", "DynDOLOD.esp"] {
        let Ok(pl) = lodforge_esp::Plugin::open(&out.join(name)) else { continue };
        let masters = pl.header.masters.clone();
        fn walk(es: lodforge_esp::Entries<'_>, f: &mut dyn FnMut(&lodforge_esp::Record<'_>)) {
            for e in es.flatten() {
                match e {
                    lodforge_esp::Entry::Group(g) => walk(g.entries(), f),
                    lodforge_esp::Entry::Record(r) => f(&r),
                }
            }
        }
        walk(pl.top_entries(), &mut |r| {
            let mi = (r.form_id >> 24) as usize;
            if &r.sig.0 != b"REFR" || mi >= masters.len() {
                return;
            }
            let Some(g) = lo.find(&masters[mi]).map(|m| m.index.global(r.form_id & 0x00FF_FFFF)) else { return };
            let edid = r.data().ok().and_then(|d| lodforge_esp::fields(&d).find(|f| &f.sig.0 == b"EDID").map(|f| f.zstring())).unwrap_or_default();
            let kind = edid.rsplit("_DynDOLOD_").next().filter(|_| edid.contains("_DynDOLOD_")).unwrap_or("(plain)").to_string();
            overrides.entry(g).or_default().push(format!("{name}:{kind}:{:08X}", r.flags));
        });
    }
    // Overrides that clear Is Full LOD, by persistence and kind; Is Full LOD refs without one.
    let mut cleared: BTreeMap<String, usize> = BTreeMap::new();
    for (id, r) in &world.refs {
        if r.flags & 0x10000 == 0 {
            continue;
        }
        let ov = overrides.get(id);
        let kind = ov.map_or("no override".to_string(), |v| v.iter().map(|s| {
            let fl = u32::from_str_radix(s.rsplit(':').next().unwrap_or("0"), 16).unwrap_or(0);
            format!("{}{}", s.rsplit_once(':').map_or(s.as_str(), |x| x.0), if fl & 0x10000 == 0 { " cleared" } else { " kept" })
        }).collect::<Vec<_>>().join("+"));
        *cleared.entry(format!("persistent={} large={} {kind}", u8::from(r.is_persistent()), u8::from(valid.contains(id)))).or_default() += 1;
    }
    println!("Is Full LOD references:");
    for (k, v) in &cleared {
        println!("    {v:5} {k}");
    }
    // [Resets] and [Disabled]: properties of the listed references.
    for name in ["Resets", "Disabled"] {
        let rows = section(&text, name);
        let mut props: BTreeMap<String, usize> = BTreeMap::new();
        let mut uniq = HashSet::new();
        for r in &rows {
            let Some(id) = r.get(1).and_then(|pl| gid(pl, &r[0])) else {
                *props.entry("unresolved".into()).or_default() += 1;
                continue;
            };
            uniq.insert(id);
            if std::env::var_os("LODFORGE_RESETROWS").is_some() && let Some(pl) = world.refs.get(&id) {
                println!("    {name} {id:08X} {} flags {:08X} scale {} [{}]", bases.get(&pl.base).map(|b| b.editor_id.as_str()).unwrap_or("?"), pl.flags, pl.scale, p.plugin_names.get(pl.plugin as usize).map(String::as_str).unwrap_or("?"));
            }
            let Some(pl) = world.refs.get(&id) else {
                let wn = worlds.iter().find(|w| w.refs.contains_key(&id)).map_or("none".to_string(), |w| w.editor_id.clone());
                *props.entry(format!("not in world (in {wn}) overrides {:?}", overrides.get(&id).map(|v| v.iter().map(|s| s.rsplit_once(':').map_or(s.as_str(), |x| x.0)).collect::<Vec<_>>()))).or_default() += 1;
                continue;
            };
            let overridden = world.plugins.iter().any(|&q| !is_master(q)) && !is_master(pl.plugin);
            let key = format!(
                "large={} rnam={} disabled={} parent={} persistent={} winner={} base={} overrides={:?}",
                u8::from(valid.contains(&id)),
                u8::from(world.large_refs.iter().any(|l| l.2 == id)),
                u8::from(pl.flags & 0x800 != 0),
                u8::from(pl.enable_parent),
                u8::from(pl.is_persistent()),
                if overridden { "esp" } else { "master" },
                String::from_utf8_lossy(&bases.get(&pl.base).map(|b| b.signature).unwrap_or(*b"????")),
                overrides.get(&id).map(|v| v.iter().map(|s| s.rsplit_once(':').map_or(s.as_str(), |x| x.0)).collect::<Vec<_>>())
            );
            *props.entry(key).or_default() += 1;
        }
        println!("[{name}] {} lines, {} distinct references:", rows.len(), uniq.len());
        for (k, v) in &props {
            println!("    {v:5} {k}");
        }
    }

    Ok(())
}

/// Cells of the `<World>_Disciple_<x>_<y>` marker references in a DynDOLOD.esm (`%` stands for
/// a minus sign in those EditorIDs).
pub fn disciple_cells(esm: &std::path::Path, world: &str) -> Vec<(i32, i32)> {
    let Ok(pl) = lodforge_esp::Plugin::open(esm) else { return Vec::new() };
    let prefix = format!("{}_disciple_", world.to_ascii_lowercase());
    let mut out = Vec::new();
    fn walk(es: lodforge_esp::Entries<'_>, prefix: &str, out: &mut Vec<(i32, i32)>) {
        for e in es.flatten() {
            match e {
                lodforge_esp::Entry::Group(g) => walk(g.entries(), prefix, out),
                lodforge_esp::Entry::Record(r) if &r.sig.0 == b"REFR" => {
                    let Ok(d) = r.data() else { continue };
                    let Some(edid) = lodforge_esp::fields(&d).find(|f| &f.sig.0 == b"EDID").map(|f| f.zstring().to_ascii_lowercase()) else { continue };
                    if let Some(rest) = edid.strip_prefix(prefix)
                        && let Some((x, y)) = rest.replace('%', "-").split_once('_').and_then(|(x, y)| Some((x.parse().ok()?, y.parse().ok()?)))
                    {
                        out.push((x, y));
                    }
                }
                lodforge_esp::Entry::Record(_) => {}
            }
        }
    }
    walk(pl.top_entries(), &prefix, &mut out);
    out
}
