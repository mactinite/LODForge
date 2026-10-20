//! `lodforge e2e-compare`: a LODForge build (`--ours`, a Data folder) against a DynDOLOD output
//! (`--golden`) for one worldspace: file inventory, object LOD (full attributes and geometry only),
//! the object height map, plugin records, dynamic LOD references and occlusion (TVDT).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use anyhow::Result;
use rayon::prelude::*;

use crate::profile::ProfileArgs;

#[derive(clap::Args)]
pub struct Args {
    /// Profile the build read (for form ids in the dynamic LOD data); leave out earlier LOD
    /// output and the DynDOLOD plugins like the build did (`--exclude-mod`, `--exclude-plugin`).
    #[command(flatten)]
    pub profile: ProfileArgs,
    /// LODForge output Data folder.
    #[arg(long)]
    pub ours: PathBuf,
    /// DynDOLOD output Data folder.
    #[arg(long)]
    pub golden: PathBuf,
    #[arg(long, default_value = "Tamriel")]
    pub world: String,
    /// Object LOD comparison with position/bitangent tolerance (see `object-lod --tolerant`).
    #[arg(long)]
    pub tolerant: bool,
    /// Write a JSON summary here.
    #[arg(long)]
    pub json: Option<PathBuf>,
}

/// Case-insensitive path lookup under `root`.
fn find_ci(root: &Path, rel: &str) -> Option<PathBuf> {
    let mut cur = root.to_path_buf();
    for part in rel.split(['\\', '/']).filter(|s| !s.is_empty()) {
        let next = std::fs::read_dir(&cur).ok()?.filter_map(|e| e.ok()).find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(part))?;
        cur = next.path();
    }
    Some(cur)
}

/// Every file under `root` as (lower-case relative path, size).
fn inventory(root: &Path) -> BTreeMap<String, u64> {
    let mut out = BTreeMap::new();
    let mut stack = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, rel)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_ascii_lowercase();
            let r = if rel.is_empty() { name } else { format!("{rel}\\{name}") };
            match e.file_type() {
                Ok(t) if t.is_dir() => stack.push((e.path(), r)),
                Ok(_) => {
                    out.insert(r, e.metadata().map(|m| m.len()).unwrap_or(0));
                }
                Err(_) => {}
            }
        }
    }
    out
}

/// Inventory grouped by kind: (files, bytes) per bucket.
fn buckets(inv: &BTreeMap<String, u64>, world: &str) -> BTreeMap<String, (usize, u64)> {
    let w = world.to_ascii_lowercase();
    let mut out: BTreeMap<String, (usize, u64)> = BTreeMap::new();
    for (p, s) in inv {
        let key = if p.starts_with(&format!("meshes\\terrain\\{w}\\objects\\")) {
            if p.ends_with(".dat") { "object height map" } else { "object LOD (.bto)" }
        } else if p.starts_with(&format!("meshes\\terrain\\{w}\\trees\\")) {
            "tree LOD (.btt/.lst)"
        } else if p.starts_with(&format!("meshes\\terrain\\{w}\\")) {
            "terrain LOD (.btr)"
        } else if p.starts_with("meshes\\lod\\grass\\") {
            "grass LOD meshes"
        } else if p.starts_with("textures\\dyndolod\\lod\\") {
            "object LOD atlas"
        } else if p.starts_with(&format!("textures\\terrain\\{w}\\trees\\")) {
            "tree LOD atlas"
        } else if p.starts_with(&format!("textures\\terrain\\{w}\\")) {
            "terrain LOD textures"
        } else if p.starts_with("textures\\terrain\\lodgen\\") {
            "billboards"
        } else if p.starts_with("textures\\") {
            "other textures"
        } else if p.ends_with(".esp") || p.ends_with(".esm") {
            "plugins"
        } else if p.starts_with("skse\\") {
            "DLL NG data"
        } else {
            "other"
        }
        .to_string();
        let e = out.entry(key).or_default();
        e.0 += 1;
        e.1 += s;
    }
    out
}

type GeoTri = [[i32; 3]; 3];

/// World-space triangles of a BTO by position only (0.25-unit grid, vertex order normalised):
/// (grass shapes, all other shapes).
fn geometry(bytes: &[u8]) -> Option<(Vec<GeoTri>, Vec<GeoTri>)> {
    let nif = lodforge_nif::parse(bytes).ok()?;
    let (mut grass, mut out) = (Vec::new(), Vec::new());
    for b in &nif.blocks {
        let lodforge_nif::Block::TriShape(s) = b else { continue };
        let is_grass = s.av.net.name.as_deref().is_some_and(|n| n.to_ascii_lowercase().starts_with("grass"));
        let t = &s.av.transform;
        let world = |p: [f32; 3]| -> [i32; 3] {
            let r = t.rotation;
            let v = [r[0][0] * p[0] + r[0][1] * p[1] + r[0][2] * p[2], r[1][0] * p[0] + r[1][1] * p[1] + r[1][2] * p[2], r[2][0] * p[0] + r[2][1] * p[1] + r[2][2] * p[2]];
            [0, 1, 2].map(|k| ((v[k] * t.scale + t.translation[k]) * 4.0).round() as i32)
        };
        for tri in &s.triangles {
            let mut v = tri.map(|i| world(s.positions.get(i as usize).copied().unwrap_or_default()));
            v.sort();
            if is_grass { grass.push(v) } else { out.push(v) }
        }
    }
    out.sort();
    grass.sort();
    Some((grass, out))
}

/// TVDT per worldspace EditorID and cell.
fn tvdt_all(path: &Path) -> Result<BTreeMap<String, HashMap<(i32, i32), Vec<u8>>>> {
    use lodforge_esp::{Entries, Entry, fields};
    let pl = lodforge_esp::Plugin::open(path)?;
    let mut out: BTreeMap<String, HashMap<(i32, i32), Vec<u8>>> = BTreeMap::new();
    fn walk(es: Entries<'_>, cur: &mut String, out: &mut BTreeMap<String, HashMap<(i32, i32), Vec<u8>>>) -> Result<()> {
        for e in es {
            match e? {
                Entry::Record(r) if &r.sig.0 == b"WRLD" => {
                    let data = r.data()?;
                    *cur = fields(&data).find(|f| &f.sig.0 == b"EDID").map(|f| f.zstring()).unwrap_or_default();
                }
                Entry::Record(r) if &r.sig.0 == b"CELL" => {
                    let data = r.data()?;
                    let (mut grid, mut tvdt) = (None, None);
                    for f in fields(&data) {
                        match &f.sig.0 {
                            b"XCLC" if f.data.len() >= 8 => grid = Some((i32::from_le_bytes(f.data[0..4].try_into().unwrap()), i32::from_le_bytes(f.data[4..8].try_into().unwrap()))),
                            b"TVDT" => tvdt = Some(f.data.to_vec()),
                            _ => {}
                        }
                    }
                    if let (Some(g), Some(t)) = (grid, tvdt) {
                        out.entry(cur.clone()).or_default().insert(g, t);
                    }
                }
                Entry::Record(_) => {}
                Entry::Group(g) => walk(g.entries(), cur, out)?,
            }
        }
        Ok(())
    }
    walk(pl.top_entries(), &mut String::new(), &mut out)?;
    Ok(out)
}

/// `<World>.objects.heightmap.dat` cells.
fn heightmap(path: &Path) -> Option<HashMap<(i32, i32), Vec<f32>>> {
    let b = std::fs::read(path).ok()?;
    let n = i32::from_le_bytes(b.get(0..4)?.try_into().ok()?) as usize;
    let rec = 8 + 33 * 33 * 4;
    let mut out = HashMap::new();
    for i in 0..n {
        let r = b.get(4 + i * rec..4 + (i + 1) * rec)?;
        let x = i32::from_le_bytes(r[0..4].try_into().ok()?);
        let y = i32::from_le_bytes(r[4..8].try_into().ok()?);
        out.insert((x, y), r[8..].chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect());
    }
    Some(out)
}

pub fn run(a: &Args) -> Result<()> {
    let mut summary = serde_json::Map::new();
    let w = &a.world;
    // 1. Inventory.
    let (io, ig) = (inventory(&a.ours), inventory(&a.golden));
    let (bo, bg) = (buckets(&io, w), buckets(&ig, w));
    println!("files by kind (ours | golden):");
    let keys: std::collections::BTreeSet<&String> = bo.keys().chain(bg.keys()).collect();
    for k in keys {
        let (o, g) = (bo.get(k).copied().unwrap_or_default(), bg.get(k).copied().unwrap_or_default());
        println!("  {k:24} {:6} files {:9.1} MB | {:6} files {:9.1} MB", o.0, o.1 as f64 / 1e6, g.0, g.1 as f64 / 1e6);
    }
    // 2. Object LOD.
    let od = find_ci(&a.ours, &format!("meshes\\terrain\\{w}\\objects"));
    let gd = find_ci(&a.golden, &format!("meshes\\terrain\\{w}\\objects"));
    if let (Some(od), Some(gd)) = (&od, &gd) {
        let is_default = |n: &str| n.ends_with(".bto") && !n.split('.').any(|p| ["SPR", "SUM", "AUT", "WIN"].contains(&p));
        let names_o: HashSet<String> = std::fs::read_dir(od)?.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| is_default(n)).collect();
        let names_g: HashSet<String> = std::fs::read_dir(gd)?.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| is_default(n)).collect();
        let mut common: Vec<String> = names_o.intersection(&names_g).cloned().collect();
        common.sort();
        println!("object LOD (default season): ours {} files, golden {}, both {}", names_o.len(), names_g.len(), common.len());
        for level in [4, 8, 16, 32] {
            let n = |s: &HashSet<String>| s.iter().filter(|x| x.split('.').nth(1) == Some(&level.to_string())).count();
            println!("  LOD{level}: ours {}, golden {}", n(&names_o), n(&names_g));
        }
        crate::object_lod::compare_dir(od, gd, &common, crate::object_lod::CompareMode { tolerant: a.tolerant, geometry_only: false, grid: 0.1 })?;
        // Geometry only: per file, triangles by world position (atlas UVs and texture paths
        // cannot match: the atlases are packed differently).
        // (level, grass?) -> (golden, ours, common)
        let res: Vec<(i32, [(usize, usize, usize); 2])> = common
            .par_iter()
            .filter_map(|n| {
                let level: i32 = n.split('.').nth(1)?.parse().ok()?;
                let (o, g) = (geometry(&std::fs::read(od.join(n)).ok()?)?, geometry(&std::fs::read(gd.join(n)).ok()?)?);
                let m = |g: &Vec<GeoTri>, o: &Vec<GeoTri>| (g.len(), o.len(), crate::object_lod::multiset_common(g, o));
                Some((level, [m(&g.1, &o.1), m(&g.0, &o.0)]))
            })
            .collect();
        println!("  geometry only (positions, 0.25-unit grid; atlas UVs and texture paths cannot match):");
        let mut geo = serde_json::Map::new();
        for (kind, k) in [("objects", 0), ("grass", 1)] {
            for level in [4, 8, 16] {
                let (tg, to, tc) = res.iter().filter(|r| r.0 == level).fold((0, 0, 0), |a, r| (a.0 + r.1[k].0, a.1 + r.1[k].1, a.2 + r.1[k].2));
                if tg + to == 0 {
                    continue;
                }
                println!("    {kind:7} LOD{level:<2}: golden {tg:10} triangles, ours {to:10}, common {tc:10} ({:6.2}% of golden, {:6.2}% of ours)", 100.0 * tc as f64 / tg.max(1) as f64, 100.0 * tc as f64 / to.max(1) as f64);
                geo.insert(format!("{kind}_lod{level}"), serde_json::json!({"golden": tg, "ours": to, "common": tc}));
            }
        }
        // Grass by triangle centroid within a tolerance (LOD4 only).
        let grass4: Vec<&String> = common.iter().filter(|n| n.split('.').nth(1) == Some("4")).collect();
        let near: Vec<[usize; 3]> = grass4
            .par_iter()
            .filter_map(|n| {
                let (o, g) = (geometry(&std::fs::read(od.join(n)).ok()?)?, geometry(&std::fs::read(gd.join(n)).ok()?)?);
                (!g.0.is_empty() || !o.0.is_empty()).then(|| [g.0.len(), near_common(&g.0, &o.0, 1.0), near_common(&g.0, &o.0, 4.0)])
            })
            .collect();
        let tot = near.iter().fold([0usize; 3], |a, r| [a[0] + r[0], a[1] + r[1], a[2] + r[2]]);
        if tot[0] > 0 {
            let pc = |v: usize| 100.0 * v as f64 / tot[0] as f64;
            println!("    grass   LOD4 by centroid: within 1 unit {:.2}%, 4 units {:.2}% of golden", pc(tot[1]), pc(tot[2]));
            geo.insert("grass_lod4_near".into(), serde_json::json!({"golden": tot[0], "within_1": tot[1], "within_4": tot[2]}));
        }
        summary.insert("bto_files".into(), serde_json::json!({"ours": names_o.len(), "golden": names_g.len(), "both": common.len()}));
        summary.insert("bto_geometry".into(), serde_json::Value::Object(geo));
        // Height map.
        let hm_name = format!("{w}.objects.heightmap.dat");
        if let (Some(ho), Some(hg)) = (heightmap(&od.join(&hm_name)), find_ci(gd, &hm_name).and_then(|p| heightmap(&p))) {
            let (mut cells, mut same_cells, mut vals, mut same, mut near) = (0, 0, 0usize, 0usize, 0usize);
            for (c, g) in &hg {
                let Some(o) = ho.get(c) else { continue };
                cells += 1;
                same_cells += usize::from(o.iter().zip(g).all(|(a, b)| a.to_bits() == b.to_bits()));
                for (a, b) in o.iter().zip(g) {
                    vals += 1;
                    same += usize::from(a.to_bits() == b.to_bits());
                    near += usize::from((a - b).abs() <= 1.0 || (*a <= f32::MIN / 2.0 && *b <= f32::MIN / 2.0));
                }
            }
            println!("object height map: cells ours {} golden {} both {cells}; cells identical {same_cells}; values identical {same}/{vals} ({:.2}%), within 1 unit {near} ({:.3}%)", ho.len(), hg.len(), 100.0 * same as f64 / vals.max(1) as f64, 100.0 * near as f64 / vals.max(1) as f64);
            summary.insert("heightmap".into(), serde_json::json!({"cells_ours": ho.len(), "cells_golden": hg.len(), "cells_identical": same_cells, "values": vals, "values_identical": same, "values_within_1": near}));
        }
    }
    // 3. Plugins.
    for name in ["DynDOLOD.esm", "DynDOLOD.esp", "Occlusion.esp"] {
        let (po, pg) = (find_ci(&a.ours, name), find_ci(&a.golden, name));
        let counts = |p: &Option<PathBuf>| p.as_ref().and_then(|p| lodforge_esp::Plugin::open(p).ok()).and_then(|pl| crate::dyndolod_export::record_counts(&pl).ok().map(|c| (pl.header.masters.len(), c)));
        let (co, cg) = (counts(&po), counts(&pg));
        println!("{name}: ours {}, golden {}", fmt_counts(&co), fmt_counts(&cg));
    }
    // 4. Dynamic LOD references.
    let p = a.profile.open()?;
    let ng = format!("SKSE\\Plugins\\DynDOLOD_Data\\DynDOLOD_NG_{w}.txt");
    if let (Some(o), Some(g)) = (find_ci(&a.ours, &ng), find_ci(&a.golden, &ng)) {
        let dec = |p: &Path| lodforge_dyndolod::ini::decode(&std::fs::read(p).unwrap_or_default());
        let (so, sg) = (crate::dyndolod_export::golden_dynamic_originals(&dec(&o), &p.load_order), crate::dyndolod_export::golden_dynamic_originals(&dec(&g), &p.load_order));
        let both = so.intersection(&sg).count();
        println!("dynamic LOD [Objects] originals: ours {}, golden {}, both {both} ({:.1}% of golden)", so.len(), sg.len(), 100.0 * both as f64 / sg.len().max(1) as f64);
        summary.insert("dynamic".into(), serde_json::json!({"ours": so.len(), "golden": sg.len(), "both": both}));
        // Which bases the differences are (by base EditorID and plugin of the winning reference).
        if let Ok(worlds) = lodforge_esp::scan_worldspaces(&p.load_order)
            && let Some(world) = worlds.iter().find(|x| x.editor_id.eq_ignore_ascii_case(w))
            && let Ok(bases) = lodforge_dyndolod::bases::read_bases(&p.load_order)
        {
            let label = |id: &u32| -> String {
                let Some(r) = world.refs.get(id) else { return "(not in world)".into() };
                let b = bases.get(&r.base).map(|b| b.editor_id.as_str()).unwrap_or("?");
                let parent = if r.enable_parent {
                    let pr = world.refs.get(&r.enable_parent_id);
                    let pb = pr.and_then(|x| bases.get(&x.base)).map(|b| b.editor_id.as_str()).unwrap_or("?");
                    format!(", parent {pb}{}{}", if r.enable_parent_flags & 1 != 0 { " opposite" } else { "" }, if pr.is_some_and(|x| x.flags & 0x800 != 0) { " (disabled)" } else { "" })
                } else {
                    String::new()
                };
                format!("{b} [{}{}{parent}]", p.plugin_names.get(r.plugin as usize).map(String::as_str).unwrap_or("?"), if r.flags & 0x800 != 0 { ", initially disabled" } else { "" })
            };
            // LODFORGE_DYNFIELDS=1: golden [Objects] field values against reference properties.
            if std::env::var_os("LODFORGE_DYNFIELDS").is_some() {
                let text = lodforge_dyndolod::ini::decode(&std::fs::read(&g).unwrap_or_default());
                let mut table: BTreeMap<(usize, String, String), usize> = BTreeMap::new();
                let mut in_obj = false;
                for l in text.lines() {
                    if l.starts_with('[') {
                        in_obj = l.eq_ignore_ascii_case("[Objects]");
                        continue;
                    }
                    let f: Vec<&str> = l.split(',').collect();
                    if !in_obj || f.len() < 16 {
                        continue;
                    }
                    let Some(id) = f[14].parse::<u32>().ok().and_then(|local| lodforge_dyndolod::bases::global_id(&p.load_order, f[15], local)) else { continue };
                    let Some(r) = world.refs.get(&id) else { continue };
                    let b = bases.get(&r.base);
                    let props = [
                        ("disabled", r.flags & 0x800 != 0),
                        ("persistent", r.flags & 0x400 != 0),
                        ("parent", r.enable_parent),
                        ("opposite", r.enable_parent && r.enable_parent_flags & 1 != 0),
                        ("parent=self", f[7] == f[14] && f[8].eq_ignore_ascii_case(f[15])),
                        ("large", world.large_refs.iter().any(|x| x.2 == id)),
                        ("door", b.is_some_and(|b| &b.signature == b"DOOR")),
                        ("far", f[2] == "1"),
                    ];
                    if std::env::var_os("LODFORGE_DYNJOINT").is_some() {
                        let pf = world.refs.get(&r.enable_parent_id).map(|x| x.flags & 0x800 != 0);
                        let key = format!("f4={} f5={} f6={} f7={} f13={} f17={}", f[3], f[4], f[5], f[6], f[12], f[16]);
                        let pr = format!("dis={} par={} opp={} pdis={:?} pers={} far={}", u8::from(r.flags & 0x800 != 0), u8::from(r.enable_parent && !(f[7] == f[14])), u8::from(r.enable_parent && r.enable_parent_flags & 1 != 0), pf, u8::from(r.flags & 0x400 != 0), f[2]);
                        *table.entry((0, key, pr)).or_default() += 1;
                        continue;
                    }
                    for (i, v) in f.iter().enumerate() {
                        if matches!(i, 0 | 1 | 7 | 8 | 14 | 15) {
                            continue;
                        }
                        for (name, on) in props {
                            *table.entry((i + 1, format!("{v:?}"), format!("{name}={on}"))).or_default() += 1;
                        }
                    }
                }
                let mut last = (0, String::new());
                for ((i, v, prop), n) in &table {
                    if (*i, v.clone()) != last {
                        println!("  field {i} = {v}:");
                        last = (*i, v.clone());
                    }
                    println!("      {prop}: {n}");
                }
            }
            // LODFORGE_DYNBASE=<EditorID>: every reference of that base with its set and position.
            if let Ok(want) = std::env::var("LODFORGE_DYNBASE") {
                let mut rows: Vec<String> = world.refs.iter().filter(|(_, r)| bases.get(&r.base).is_some_and(|b| b.editor_id.eq_ignore_ascii_case(&want))).map(|(id, r)| {
                    let set = match (so.contains(id), sg.contains(id)) { (true, true) => "both", (true, false) => "ours", (false, true) => "golden", _ => "none" };
                    format!("  {set:6} {id:08X} cell {:4},{:4} pos {:9.1} {:9.1} {:8.1} flags {:08X} scale {:.2} large {}", (r.position[0] / 4096.0).floor(), (r.position[1] / 4096.0).floor(), r.position[0], r.position[1], r.position[2], r.flags, r.scale, world.large_refs.iter().any(|l| l.2 == *id))
                }).collect();
                rows.sort();
                println!("{}", rows.join("
"));
            }
            for (name, set) in [("ours only", so.difference(&sg).collect::<Vec<_>>()), ("golden only", sg.difference(&so).collect::<Vec<_>>())] {
                let mut by: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
                for id in set {
                    *by.entry(label(id)).or_default() += 1;
                }
                let mut v: Vec<_> = by.into_iter().collect();
                v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
                println!("  {name}: {}", v.iter().take(25).map(|(k, n)| format!("{n} {k}")).collect::<Vec<_>>().join("; "));
            }
        }
    }
    // 4b. Tree LOD: every .btt reference by form id, grouped by base.
    if let (Some(td_o), Some(td_g)) = (find_ci(&a.ours, &format!("meshes\\terrain\\{w}\\trees")), find_ci(&a.golden, &format!("meshes\\terrain\\{w}\\trees"))) {
        let refs = |dir: &Path| -> (HashMap<u32, (String, [f32; 3], f32, i32)>, usize, usize) {
            let mut m = HashMap::new();
            let (mut files, mut empty) = (0, 0);
            for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if !name.ends_with(".btt") || name.split('.').any(|p| ["SPR", "SUM", "AUT", "WIN"].contains(&p)) {
                    continue;
                }
                let Ok(b) = std::fs::read(e.path()) else { continue };
                files += 1;
                if b.len() <= 4 {
                    empty += 1;
                    continue;
                }
                let rd = |o: usize| i32::from_le_bytes(b[o..o + 4].try_into().unwrap());
                let rf = |o: usize| f32::from_le_bytes(b[o..o + 4].try_into().unwrap());
                let mut o = 4;
                for _ in 0..rd(0) {
                    let (ty, n) = (rd(o), rd(o + 4) as usize);
                    o += 8;
                    for _ in 0..n {
                        if o + 32 > b.len() {
                            break;
                        }
                        m.insert(rd(o + 20) as u32, (name.clone(), [rf(o), rf(o + 4), rf(o + 8)], rf(o + 16), ty));
                        o += 32;
                    }
                }
            }
            (m, files, empty)
        };
        let ((ro, fo, eo), (rg, fg, eg)) = (refs(&td_o), refs(&td_g));
        let both = ro.keys().filter(|k| rg.contains_key(k)).count();
        let same_block = ro.iter().filter(|(k, v)| rg.get(k).is_some_and(|g| g.0 == v.0)).count();
        let same_scale = ro.iter().filter(|(k, v)| rg.get(k).is_some_and(|g| (g.2 - v.2).abs() < 1e-3)).count();
        println!("tree LOD: ours {} references in {fo} files ({eo} empty), golden {} in {fg} files ({eg} empty); common {both}, same block {same_block}, same scale {same_scale}", ro.len(), rg.len());
        let lst = |dir: &Path| std::fs::read(dir.join(format!("{w}.lst"))).ok().map(|b| i32::from_le_bytes(b[0..4].try_into().unwrap()));
        println!("  LST types: ours {:?}, golden {:?}", lst(&td_o), lst(&td_g));
        if let Ok(worlds) = lodforge_esp::scan_worldspaces(&p.load_order)
            && let Some(world) = worlds.iter().find(|x| x.editor_id.eq_ignore_ascii_case(w))
            && let Ok(bases) = lodforge_dyndolod::bases::read_bases(&p.load_order)
        {
            let label = |id: &u32| -> String {
                let Some(r) = world.refs.get(id) else { return "(not in world)".into() };
                let b = bases.get(&r.base).map(|b| b.editor_id.as_str()).unwrap_or("?");
                let tilt = |rad: f32| { let d = f64::from(rad).to_degrees().rem_euclid(360.0); d.min(360.0 - d) };
                format!("{b} [{}{}{}{}]", p.plugin_names.get(r.plugin as usize).map(String::as_str).unwrap_or("?"), if r.flags & 0x800 != 0 { ", disabled" } else { "" }, if r.enable_parent { ", parent" } else { "" }, if tilt(r.rotation[0]).max(tilt(r.rotation[1])) > 30.0 { ", tilted" } else { "" })
            };
            // LODFORGE_TILT_IDS=<file with comma-separated ids DynDOLOD excluded for tilt>: rotation
            // statistics of those against the trees DynDOLOD kept.
            if let Ok(path) = std::env::var("LODFORGE_TILT_IDS") {
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                let excluded: Vec<u32> = text.split(',').filter_map(|t| u32::from_str_radix(t.trim(), 16).ok()).collect();
                let measures = |id: &u32| -> Option<(f64, f64, f64)> {
                    let r = world.refs.get(id)?;
                    let n = |rad: f32| { let d = f64::from(rad).to_degrees().rem_euclid(360.0); d.min(360.0 - d) };
                    let (x, y) = (n(r.rotation[0]), n(r.rotation[1]));
                    // Angle of the rotated up vector from vertical (Euler XYZ as the engine applies it).
                    let (cx, sx, cy, sy) = (f64::from(r.rotation[0]).cos(), f64::from(r.rotation[0]).sin(), f64::from(r.rotation[1]).cos(), f64::from(r.rotation[1]).sin());
                    let _ = (sx, sy);
                    let up = (cx * cy).clamp(-1.0, 1.0).acos().to_degrees();
                    Some((x, y, up))
                };
                let stats = |name: &str, ids: Vec<u32>| {
                    let v: Vec<(f64, f64, f64)> = ids.iter().filter_map(measures).collect();
                    let mut maxs: Vec<f64> = v.iter().map(|m| m.0.max(m.1)).collect();
                    let mut ups: Vec<f64> = v.iter().map(|m| m.2).collect();
                    maxs.sort_by(|a, b| a.partial_cmp(b).unwrap());
                    ups.sort_by(|a, b| a.partial_cmp(b).unwrap());
                    let q = |s: &Vec<f64>, f: f64| s.get(((s.len() as f64 - 1.0) * f) as usize).copied().unwrap_or(f64::NAN);
                    println!("  {name}: {} refs; max(|rx|,|ry|) min {:.2} p50 {:.2} max {:.2}; up-vector tilt min {:.2} p50 {:.2} max {:.2}", v.len(), q(&maxs, 0.0), q(&maxs, 0.5), q(&maxs, 1.0), q(&ups, 0.0), q(&ups, 0.5), q(&ups, 1.0));
                };
                stats("DynDOLOD excluded (tilt)", excluded.clone());
                let raw = |id: &u32| world.refs.get(id).map(|r| format!("{id:08X} rx {:.2} ry {:.2} rz {:.2}", f64::from(r.rotation[0]).to_degrees(), f64::from(r.rotation[1]).to_degrees(), f64::from(r.rotation[2]).to_degrees())).unwrap_or_default();
                for id in excluded.iter().take(12) {
                    println!("    excluded {}", raw(id));
                }
                // Rule: signed degrees of the stored rotation, rx > 30 or ry > 30.
                let norm = |rad: f32| { let d = f64::from(rad).to_degrees().rem_euclid(360.0); if d > 180.0 { d - 360.0 } else { d } };
                let signed = |id: &u32| world.refs.get(id).map(|r| norm(r.rotation[0]) > 30.0 || norm(r.rotation[1]) > 30.0);
                let ex_hit = excluded.iter().filter(|id| signed(id) == Some(true)).count();
                let kept_hit = rg.keys().filter(|id| signed(id) == Some(true)).count();
                println!("    signed rule: excluded predicted {ex_hit}/{}, kept wrongly predicted {kept_hit}/{}", excluded.len(), rg.len());
                for id in excluded.iter().filter(|id| signed(id) == Some(false)) {
                    println!("    excluded-not-signed {}", raw(id));
                }
                // Raw stored radians in degrees (no normalisation), as xEdit shows them.
                let rawd = |id: &u32| world.refs.get(id).map(|r| { let w = |rad: f32| { let d = lodforge_dyndolod::export::degrees(rad); d > 30.0 && d < 330.0 }; w(r.rotation[0]) || w(r.rotation[1]) });
                let up = |id: &u32| world.refs.get(id).map(|r| (f64::from(r.rotation[0]).cos() * f64::from(r.rotation[1]).cos()).clamp(-1.0, 1.0).acos().to_degrees()).unwrap_or(0.0);
                let mut kept_up: Vec<(f64, u32)> = rg.keys().map(|id| (up(id), *id)).collect();
                kept_up.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
                println!("    up-vector tilt of kept trees, largest: {}", kept_up.iter().take(8).map(|(t, id)| format!("{id:08X} {t:.2} [{}]", raw(id))).collect::<Vec<_>>().join("; "));
                println!("    up-vector tilt of the unexplained exclusions: {}", excluded.iter().filter(|id| signed(id) == Some(false)).map(|id| format!("{id:08X} {:.2}", up(id))).collect::<Vec<_>>().join(", "));
                for id in rg.keys().filter(|id| rawd(id) == Some(true)) { println!("    raw-kept {}", raw(id)); }
                println!("    raw rule: excluded predicted {}/{}, kept wrongly predicted {}", excluded.iter().filter(|id| rawd(id) == Some(true)).count(), excluded.len(), rg.keys().filter(|id| rawd(id) == Some(true)).count());
                let mut wrong: Vec<&u32> = rg.keys().filter(|id| signed(id) == Some(true)).collect();
                wrong.sort();
                for id in wrong.iter().take(14) {
                    let b = world.refs.get(id).and_then(|r| bases.get(&r.base)).map(|b| b.editor_id.clone()).unwrap_or_default();
                    println!("    kept-but-signed {} {b} plugin {}", raw(id), world.refs.get(id).map(|r| p.plugin_names[r.plugin as usize].clone()).unwrap_or_default());
                }
                let mut kept_tilted: Vec<&u32> = rg.keys().filter(|id| measures(id).is_some_and(|m| m.0.max(m.1) > 30.0)).collect();
                kept_tilted.sort();
                println!("    kept with max(|rx|,|ry|) > 30: {}", kept_tilted.len());
                for id in kept_tilted.iter().take(16) {
                    println!("    kept {}", raw(id));
                }
                stats("DynDOLOD kept", rg.keys().copied().collect());
            }
            if let Ok(path) = std::env::var("LODFORGE_CITY_IDS") {
                let city: HashSet<u32> = std::fs::read_to_string(&path).unwrap_or_default().split(',').filter_map(|t| u32::from_str_radix(t.trim(), 16).ok()).collect();
                let ours_only: Vec<u32> = ro.keys().filter(|k| !rg.contains_key(k)).copied().collect();
                let in_city = ours_only.iter().filter(|k| city.contains(k)).count();
                println!("  city bounds: {} DynDOLOD exclusions, {in_city} of our {} extra trees among them", city.len(), ours_only.len());
                let mut cells: BTreeMap<(i32, i32), usize> = BTreeMap::new();
                for id in &city {
                    if let Some(r) = world.refs.get(id) {
                        *cells.entry(((r.position[0] / 4096.0).floor() as i32, (r.position[1] / 4096.0).floor() as i32)).or_default() += 1;
                    }
                }
                println!("  city-bound cells: {cells:?}");
                let mut rest: BTreeMap<(i32, i32), usize> = BTreeMap::new();
                for id in ours_only.iter().filter(|k| !city.contains(k)) {
                    if let Some(r) = world.refs.get(id) {
                        *rest.entry(((r.position[0] / 4096.0).floor() as i32, (r.position[1] / 4096.0).floor() as i32)).or_default() += 1;
                    }
                }
                println!("  other extra trees by cell: {rest:?}");
            }
            for (name, ids) in [("ours only", ro.keys().filter(|k| !rg.contains_key(k)).collect::<Vec<_>>()), ("golden only", rg.keys().filter(|k| !ro.contains_key(k)).collect::<Vec<_>>())] {
                let mut by: HashMap<String, usize> = HashMap::new();
                for id in ids {
                    *by.entry(label(id)).or_default() += 1;
                }
                let mut v: Vec<_> = by.into_iter().collect();
                v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
                println!("  {name} ({}): {}", v.iter().map(|x| x.1).sum::<usize>(), v.iter().take(20).map(|(k, n)| format!("{n} {k}")).collect::<Vec<_>>().join("; "));
            }
        }
    }
    // 5. Occlusion.
    if let (Some(o), Some(g)) = (find_ci(&a.ours, "Occlusion.esp"), find_ci(&a.golden, "Occlusion.esp")) {
        let (to, tg) = (tvdt_all(&o)?, tvdt_all(&g)?);
        let (mut tot_same, mut tot_g, mut tot_o, mut both_bits, mut either_bits) = (0, 0, 0, 0usize, 0usize);
        let worlds: std::collections::BTreeSet<&String> = to.keys().chain(tg.keys()).collect();
        for wn in worlds {
            let empty = HashMap::new();
            let (mo, mg) = (to.get(wn).unwrap_or(&empty), tg.get(wn).unwrap_or(&empty));
            let mut same = 0;
            for (c, gt) in mg {
                if let Some(ot) = mo.get(c) {
                    same += usize::from(ot == gt);
                    for (x, y) in ot.iter().zip(gt) {
                        both_bits += (x & y).count_ones() as usize;
                        either_bits += (x | y).count_ones() as usize;
                    }
                }
            }
            println!("  occlusion {wn}: ours {} cells, golden {}, identical TVDT {same}", mo.len(), mg.len());
            tot_same += same;
            tot_g += mg.len();
            tot_o += mo.len();
        }
        println!("occlusion: ours {tot_o} cells, golden {tot_g}, identical {tot_same}; bit Jaccard on common cells {:.4}", both_bits as f64 / either_bits.max(1) as f64);
        summary.insert("occlusion".into(), serde_json::json!({"ours": tot_o, "golden": tot_g, "identical": tot_same, "jaccard": both_bits as f64 / either_bits.max(1) as f64}));
    }
    if let Some(j) = &a.json {
        std::fs::write(j, serde_json::to_string_pretty(&serde_json::Value::Object(summary))?)?;
    }
    Ok(())
}

fn fmt_counts(c: &Option<(usize, BTreeMap<String, usize>)>) -> String {
    match c {
        None => "-".into(),
        Some((m, c)) => format!("{m} masters, {}", c.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", ")),
    }
}

/// Triangles of `o` whose centroid lies within `tol` units of an unused centroid of `g` (greedy,
/// grid-hashed). For grass, where per-view billboard widths a few units apart move every vertex
/// off the exact 0.25-unit grid.
fn near_common(g: &[GeoTri], o: &[GeoTri], tol: f32) -> usize {
    let c = |t: &GeoTri| [0, 1, 2].map(|k| (t[0][k] + t[1][k] + t[2][k]) as f32 / 12.0);
    let key = |p: [f32; 3]| p.map(|v| (v / tol).floor() as i64);
    let mut grid: HashMap<[i64; 3], Vec<[f32; 3]>> = HashMap::new();
    for t in g {
        let p = c(t);
        grid.entry(key(p)).or_default().push(p);
    }
    let mut n = 0;
    for t in o {
        let p = c(t);
        let k = key(p);
        'search: for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    if let Some(v) = grid.get_mut(&[k[0] + dx, k[1] + dy, k[2] + dz])
                        && let Some(i) = v.iter().position(|q| (0..3).map(|j| (q[j] - p[j]).powi(2)).sum::<f32>() <= tol * tol)
                    {
                        v.swap_remove(i);
                        n += 1;
                        break 'search;
                    }
                }
            }
        }
    }
    n
}
