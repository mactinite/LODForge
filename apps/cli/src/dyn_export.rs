//! `--runtime lodforge`: `<World>.dyn.bin` per worldspace (docs/runtime/instance-formats.md
//! section 7b), from the same export that writes DynDOLOD.esp: the dynamic LOD items, child
//! world copies, reset / disabled / base lists, child worlds, terrain underside and the large
//! references with where their LOD lives. LODForge Runtime draws all of it itself, so it needs
//! no DynDOLOD.esm markers and no DynDOLOD.esp references.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result};

use lodforge_dyndolod::bases::Base;
use lodforge_dyndolod::childworld::ChildCopy;
use lodforge_dyndolod::export::DynamicRef;
use lodforge_dyndolod::rules::Grid;
use lodforge_esp::Worldspace;
use lodforge_instances::Common;
use lodforge_instances::dynlod::*;
use lodforge_instances::statics::StaticsFile;

use crate::dyndolod_build::Config;
use crate::instances::FormNames;
use crate::profile::Profile;

/// One worldspace's input, from its export.
pub struct DynWorld<'a> {
    pub world: &'a Worldspace,
    pub dynamic: &'a [DynamicRef],
    pub seasonal: &'a [(&'static str, HashMap<u32, (u32, String)>)],
    pub child_copies: &'a [(ChildCopy, String)],
    /// References with object or tree LOD (for the Is Full LOD resets).
    pub lod_refs: &'a [u32],
    pub lod_bases: &'a HashSet<u32>,
    /// Large references of the export: load-order form id -> `.bto` columns used (bit 0-2 =
    /// LOD4/8/16 has a model or billboard).
    pub large_columns: Option<&'a HashMap<u32, u16>>,
    /// Export lines of the default season (for the parent copies of child worldspaces); only
    /// lines inside some child's bounds are used.
    pub lines: &'a [lodforge_dyndolod::export::RefLine],
    /// Draw variants of `lines` ([`variants`]).
    pub variants: Option<&'a HashMap<u32, Variant>>,
}

/// What the build did that the `.dyn.bin` files record.
#[derive(Debug, Clone, Copy, Default)]
pub struct WriteFlags {
    /// Every eligible large reference is instanced (`DHDR` flag).
    pub large_refs_instanced: bool,
    /// DynDOLOD's large-reference workarounds apply (its `[Markers]` would be written): fill `DMIG`.
    pub workarounds: bool,
}

#[derive(Default, Debug)]
pub struct Report {
    pub files: usize,
    pub items: usize,
    pub copies: usize,
    pub models: usize,
    pub large: usize,
    pub large_instanced: usize,
    pub large_merged: usize,
    pub resets: usize,
    pub disabled: usize,
    pub migration: usize,
    pub direct_parents: usize,
    /// Per child worldspace: (EditorID, items, twins, city copies, billboard trees with an
    /// impostor type, billboard trees without one).
    pub child_copies_per_world: Vec<(String, usize, usize, usize, usize, usize, usize)>,
    /// Items whose original or base has no defining plugin (left out).
    pub unnamed: usize,
    pub bytes: usize,
}

impl Report {
    pub fn summary(&self) -> String {
        format!(
            "runtime dynamic LOD: {} files, {} items ({} with a direct enable parent below the chain top) + {} child-world copies, {} models; {} large references ({} instanced, {} merged in .bto only); {} resets, {} disabled, {} migration large refs; {} without a plugin left out; {:.2} MB",
            self.files, self.items, self.direct_parents, self.copies, self.models, self.large, self.large_instanced, self.large_merged, self.resets, self.disabled, self.migration, self.unnamed, self.bytes as f64 / 1e6
        ) + &self.child_copies_per_world.iter().map(|(w, n, t, cc, ti, tn, ob)| format!("
  parent copies in {w}: {n} items ({t} twins of child references, {cc} copies of the child's own content); billboard trees: {ti} with an impostor type, {tn} without; {ob} in child cells outside the bounds")).collect::<String>()
    }
}

/// World AABB of `bounds` (OBND, model space) under a reference transform (section 1:
/// `R = Rx(-ax) * Ry(-ay) * Rz(-az)`, uniform scale).
pub fn world_bounds(bounds: [i16; 6], pos: [f32; 3], rot: [f32; 3], scale: f32) -> [f32; 6] {
    let (sx, cx) = (-rot[0]).sin_cos();
    let (sy, cy) = (-rot[1]).sin_cos();
    let (sz, cz) = (-rot[2]).sin_cos();
    let rx = [[1.0, 0.0, 0.0], [0.0, cx, -sx], [0.0, sx, cx]];
    let ry = [[cy, 0.0, sy], [0.0, 1.0, 0.0], [-sy, 0.0, cy]];
    let rz = [[cz, -sz, 0.0], [sz, cz, 0.0], [0.0, 0.0, 1.0]];
    let mul = |a: [[f32; 3]; 3], b: [[f32; 3]; 3]| -> [[f32; 3]; 3] { std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum())) };
    let r = mul(mul(rx, ry), rz);
    let mut out = [f32::MAX, f32::MAX, f32::MAX, f32::MIN, f32::MIN, f32::MIN];
    for c in 0..8 {
        let v = [f32::from(bounds[if c & 1 == 0 { 0 } else { 3 }]), f32::from(bounds[if c & 2 == 0 { 1 } else { 4 }]), f32::from(bounds[if c & 4 == 0 { 2 } else { 5 }])];
        for i in 0..3 {
            let w = pos[i] + scale * (r[i][0] * v[0] + r[i][1] * v[1] + r[i][2] * v[2]);
            out[i] = out[i].min(w);
            out[i + 3] = out[i + 3].max(w);
        }
    }
    out
}

fn grid_flags(g: Grid, large: bool) -> u32 {
    match g {
        // Only large references stay on FarGrid; the others are downgraded to NearGrid.
        Grid::FarLod | Grid::FarFull if large => GRID_FAR,
        Grid::NeverFadeLod | Grid::NeverFadeFull => GRID_NEVER_FADE,
        _ => GRID_NEAR,
    }
}

fn glow_level(model: &str) -> u32 {
    let m = model.to_ascii_lowercase();
    (0..3).find(|j| m.contains(&format!("glow_lod_{j}"))).unwrap_or(GLOW_LEVEL_NONE)
}

fn model_flags(model: &str, full: bool) -> u32 {
    let m = model.to_ascii_lowercase();
    let mut f = if full { MODEL_FULL } else { 0 };
    if m.contains("_dyndolod_lod") {
        f |= MODEL_ANIMATED;
    }
    if m.contains("glow") {
        f |= MODEL_GLOW;
    }
    f
}

/// Data-relative, lower case, `meshes\` prefixed.
fn mesh_path(m: &str) -> String {
    let m = m.trim().replace('/', "\\").to_ascii_lowercase();
    if m.is_empty() || m.starts_with("meshes\\") || m.ends_with(".dds") { m } else { format!("meshes\\{m}") }
}

fn model_entry(f: &mut DynFile, model: &str, columns: &[String; 4], full: bool, base: Option<&Base>, variant: Option<&Variant>) -> u32 {
    let model = mesh_path(model);
    let lod: [String; 3] = std::array::from_fn(|i| mesh_path(&columns[i]));
    let flags = model_flags(&model, full);
    let key = format!("{model}|{}|{}|{}|{flags}|{variant:?}", lod[0], lod[1], lod[2]);
    let bounds = base.map(|b| b.bounds.map(f32::from)).unwrap_or_default();
    let (textures, classes) = variant.map(|v| (v.clone().map(|s| s.0), v.clone().map(|s| s.1))).unwrap_or_default();
    f.model(&key, || DynModel { flags, pack: [NO_MESH; 4], bounds, model: model.clone(), lod, textures, classes })
}

/// (lower-case plugin, local form) -> instanced levels (bit 3-5) from `<World>.statics.bin`.
fn instanced(out: &Path, world: &str) -> Result<HashMap<(String, u32), u16>> {
    let path = crate::instances::out_dir(out).join(format!("{world}.statics.bin"));
    let mut m = HashMap::new();
    let Ok(bytes) = std::fs::read(&path) else { return Ok(m) };
    let st = StaticsFile::from_bytes(&bytes).with_context(|| format!("reading {}", path.display()))?;
    for (&(level, _, _), recs) in &st.blocks {
        let bit = match level {
            4 => LRG_INST4,
            8 => LRG_INST8,
            16 => LRG_INST16,
            _ => continue,
        };
        for r in recs {
            if let Some(n) = st.common.plugins.get(usize::from(r.plugin)) {
                *m.entry((n.to_ascii_lowercase(), r.form)).or_default() |= bit;
            }
        }
    }
    Ok(m)
}

#[allow(clippy::too_many_arguments)]
pub fn write(p: &Profile, c: &Config, worlds: &[DynWorld<'_>], all_worlds: &[Worldspace], undersides: &HashSet<String>, names: &FormNames, out: &Path, opts: lodforge_instances::WriteOptions, wf: WriteFlags, impostor_root: &Path) -> Result<Report> {
    let dir = crate::instances::out_dir(out);
    std::fs::create_dir_all(&dir)?;
    let mut rep = Report::default();
    let near: u32 = c.num("NearGrid", 11);
    let far: u32 = c.num("FarGrid", 21);
    let height: f32 = c.num("TerrainUndersideHeight", 500.0);
    let matches = lodforge_dyndolod::childworld::known_matches(&c.dir);
    for w in worlds {
        let world = w.world;
        let mut f = DynFile::new(Common { world: world.editor_id.clone(), season: String::new(), generator: crate::instances::generator(), plugins: Vec::new(), params: format!("near_grid={near}\nfar_grid={far}"), token: crate::instances::build_token() });
        let sw = crate::dyndolod_build::south_west(p, &world.editor_id);
        let world_ref = names.split(world.form_id).map(|(n, l)| f.form(n, l)).unwrap_or_default();
        let lod_world = if world.parent_flags & 0x02 != 0 { world.parent.and_then(|pid| all_worlds.iter().find(|x| x.form_id == pid)).map_or(world.editor_id.clone(), |x| x.editor_id.clone()) } else { world.editor_id.clone() };
        f.header = DynHeader { world: world_ref, south_west: (sw.0 as i16, sw.1 as i16), near_grid: near as u8, far_grid: far as u8, glow_multiple: 1.75, lod_world, flags: if wf.large_refs_instanced { HDR_LARGE_REFS_INSTANCED } else { 0 } };
        let in_child = all_worlds.iter().any(|x| x.parent == Some(world.form_id) && x.parent_flags & 0x02 != 0);
        let mut dyn_ids = HashSet::new();
        for d in w.dynamic {
            let Some(r) = world.refs.get(&d.ref_id) else { continue };
            let (Some((on, ol)), Some((bn, bl))) = (names.split(d.ref_id), names.split(d.base)) else {
                rep.unnamed += 1;
                continue;
            };
            dyn_ids.insert(d.ref_id);
            let full = d.replace || matches!(d.grid, Grid::NearFull | Grid::FarFull | Grid::NeverFadeFull);
            let model = model_entry(&mut f, &d.model, &d.columns, full, c.bases.get(&d.base), None);
            let mf = f.models[model as usize].flags;
            let mut flags = grid_flags(d.grid, d.large);
            for (on_, bit) in [(d.large, LARGE), (d.enable_parent != 0 && d.opposite_parent, OPPOSITE), (d.initially_enabled, INITIALLY_ENABLED), (d.replace, REPLACE), (d.persistent, PERSISTENT), (full, FULL), (mf & MODEL_ANIMATED != 0, ANIMATED), (mf & MODEL_GLOW != 0, GLOW), (in_child && !d.replace, IN_CHILD_WORLDS)] {
                if on_ {
                    flags |= bit;
                }
            }
            if c.bases.get(&d.base).is_some_and(|b| &b.signature == b"DOOR") {
                flags |= DOOR;
            }
            flags |= glow_level(&d.model) << GLOW_LEVEL_SHIFT;
            if world.parent.is_some() {
                flags |= FROM_CHILD_WORLD;
            }
            let parent = if d.enable_parent != 0 { names.split(d.enable_parent) } else { None };
            // The direct parent too when the chain has more links (a script can toggle a middle one).
            let direct = if d.enable_parent != 0 && r.enable_parent && r.enable_parent_id != d.enable_parent { names.split(r.enable_parent_id) } else { None };
            if direct.is_some() && r.enable_parent_flags & 1 != 0 {
                flags |= DIRECT_OPPOSITE;
            }
            let cell = r.cell.map_or_else(|| cell_of(r.position), |(x, y)| (x as i16, y as i16));
            f.add(r.position, r.rotation, if r.has_scale { r.scale } else { 1.0 }, (on, ol), parent, model, flags, cell, (bn, bl), direct);
            if direct.is_some() {
                rep.direct_parents += 1;
            }
            if d.replace {
                let o = f.form(on, ol);
                f.disabled.push(o);
            }
            rep.items += 1;
        }
        // Seasonal models: the DMDL entry each original uses in a season where it changes.
        for (code, map) in w.seasonal {
            let mut v = Vec::new();
            for d in w.dynamic {
                let Some((b2, m2)) = map.get(&d.ref_id) else { continue };
                if *b2 == d.base && m2.eq_ignore_ascii_case(&d.model) {
                    continue;
                }
                let Some((on, ol)) = names.split(d.ref_id) else { continue };
                let full = d.replace || matches!(d.grid, Grid::NearFull | Grid::FarFull | Grid::NeverFadeFull);
                let m = model_entry(&mut f, m2, &Default::default(), full, c.bases.get(b2), None);
                v.push((f.form(on, ol), m));
            }
            if !v.is_empty() {
                f.seasons.push((code.to_string(), v));
            }
        }
        // Child-world copies: drawn while their cell is attached in this world.
        for (cp, model) in w.child_copies {
            let pl = &cp.placement;
            let (Some((on, ol)), Some((bn, bl))) = (names.split(cp.ref_id), names.split(pl.base)) else {
                rep.unnamed += 1;
                continue;
            };
            let full = cp.full_model;
            let m = model_entry(&mut f, if full { c.bases.get(&pl.base).map_or(model.as_str(), |b| b.model.as_str()) } else { model }, &Default::default(), full, c.bases.get(&pl.base), None);
            let mut flags = GRID_NEAR | CHILD_COPY | INITIALLY_ENABLED;
            if cp.large {
                flags |= LARGE;
            }
            if full {
                flags |= FULL;
            }
            f.add(pl.position, pl.rotation, if pl.has_scale { pl.scale } else { 1.0 }, (on, ol), None, m, flags, cell_of(pl.position), (bn, bl), None);
            rep.copies += 1;
        }
        // Persistent Is Full LOD references whose LOD this build provides: the runtime clears the
        // flag (DynDOLOD does it with plugin overrides) and resets them.
        let mut lod_refs: Vec<u32> = w.lod_refs.to_vec();
        lod_refs.extend(w.dynamic.iter().filter(|d| !d.replace).map(|d| d.ref_id));
        for id in lodforge_dyndolod::largerefs::revert_refs(world, &lod_refs) {
            if let Some((n, l)) = names.split(id) {
                let r = f.form(n, l);
                f.resets.push(r);
            }
        }
        let mut bases: Vec<u32> = w.lod_bases.iter().copied().filter(|b| c.bases.get(b).is_some_and(|b| b.flags & 0x8000 == 0)).collect();
        bases.sort_unstable();
        for b in bases {
            if let Some((n, l)) = names.split(b) {
                let r = f.form(n, l);
                f.bases.push(r);
            }
        }
        for x in all_worlds.iter().filter(|x| x.parent == Some(world.form_id)) {
            let Some((n, l)) = names.split(x.form_id) else { continue };
            let (bounds, _) = crate::dyndolod_build::child_config(c, &x.editor_id, &world.editor_id);
            let mut flags = if x.parent_flags & 0x02 != 0 { CHILD_USES_PARENT_LOD } else { 0 };
            if x.parent_flags & 0x01 != 0 {
                flags |= CHILD_USES_PARENT_LAND;
            }
            if bounds.is_some() {
                flags |= CHILD_HAS_BOUNDS;
            }
            let b = bounds.map_or([0; 4], |b| [b.0 as i16, b.1 as i16, b.2 as i16, b.3 as i16]);
            let wr = f.form(n, l);
            f.child_worlds.push(ChildWorld { world: wr, flags, bounds: b, editor_id: x.editor_id.clone() });
        }
        if undersides.contains(&world.editor_id) {
            let e = &world.editor_id;
            let mut list = vec![world.form_id];
            list.extend(all_worlds.iter().filter(|x| x.parent == Some(world.form_id) && x.parent_flags & 1 != 0).map(|x| x.form_id));
            let worlds_ = list.into_iter().filter_map(|id| names.split(id)).map(|(n, l)| (n.to_string(), l)).collect::<Vec<_>>();
            let worlds_ = worlds_.into_iter().map(|(n, l)| f.form(&n, l)).collect();
            f.underside = Some(Underside { model: format!("meshes\\terrain\\{e}\\{e}_underside.nif").to_ascii_lowercase(), height, worlds: worlds_ });
        }
        // Large references and where their LOD lives.
        let inst = instanced(out, &world.editor_id)?;
        let mut large: Vec<(&u32, &(i32, i32))> = world.large_ref_cells.iter().collect();
        large.sort();
        for (&id, &cell) in large {
            let Some(r) = world.refs.get(&id) else { continue };
            let Some((n, l)) = names.split(id) else { continue };
            let i = inst.get(&(n.to_ascii_lowercase(), l)).copied().unwrap_or(0);
            let cols = w.large_columns.and_then(|m| m.get(&id)).copied().unwrap_or(0);
            // A level is merged when the export has a column there and it was not instanced.
            let mut lod = i | (cols & !(i >> 3) & 7);
            if dyn_ids.contains(&id) {
                lod |= LRG_DYN;
            }
            if r.is_persistent() {
                lod |= LRG_PERSISTENT;
            }
            if r.flags & 0x800 != 0 || r.enable_parent {
                lod |= LRG_DISABLED;
            }
            if i != 0 {
                rep.large_instanced += 1;
            } else if lod & 7 != 0 {
                rep.large_merged += 1;
            }
            let base = c.bases.get(&r.base);
            let bounds = base.map_or([r.position[0], r.position[1], r.position[2], r.position[0], r.position[1], r.position[2]], |b| world_bounds(b.bounds, r.position, r.rotation, if r.has_scale { r.scale } else { 1.0 }));
            let reference = f.form(n, l);
            f.large.entry((cell.0 as i16, cell.1 as i16)).or_default().push(LargeRef { reference, lod, bounds });
            rep.large += 1;
        }
        // DynDOLOD's large-reference workaround: the persistent large references its markers toggle
        // (`[Markers]`), in this world and its child worlds with their own LOD.
        if wf.workarounds {
            let mut ids: Vec<u32> = lodforge_dyndolod::largerefs::marker_cells(&p.load_order, world, &c.bases).into_iter().flat_map(|m| m.persistent).collect();
            for x in all_worlds.iter().filter(|x| x.parent == Some(world.form_id) && x.parent_flags & 0x02 == 0) {
                if crate::dyndolod_build::child_config(c, &x.editor_id, &world.editor_id).0.is_some() {
                    ids.extend(lodforge_dyndolod::largerefs::marker_cells(&p.load_order, x, &c.bases).into_iter().flat_map(|m| m.persistent));
                }
            }
            ids.sort_unstable();
            ids.dedup();
            for id in ids {
                if let Some((n, l)) = names.split(id) {
                    let r = f.form(n, l);
                    f.migration.push(r);
                }
            }
        }
        rep.migration += f.migration.len();
        rep.resets += f.resets.len();
        rep.disabled += f.disabled.len();
        // Parent copies for the child worldspaces that use this world's LOD (walled cities).
        // Billboard trees there are in no `.trees.bin` (they stay in the parent's `.bto`): their
        // items carry the impostor type of their base.
        // The parent's trees drawn as impostors (`.trees.bin`, default season).
        let parent_trees: HashSet<(String, u32)> = std::fs::read(dir.join(format!("{}.trees.bin", world.editor_id))).ok().and_then(|b| lodforge_instances::trees::TreeFile::from_bytes(&b).ok()).map(|t| t.cells.values().flatten().filter_map(|r| t.common.plugins.get(usize::from(r.plugin)).map(|n| (n.to_ascii_lowercase(), r.form))).collect()).unwrap_or_default();
        let impostors: HashMap<u32, u32> = crate::runtime_trees::set_from_index(p, &impostor_root.join("textures").join("LODForge").join("Impostors").join(&world.editor_id).join("impostors.json")).map(|s| s.base_index).unwrap_or_default();
        let in_child_dynamic: HashSet<u32> = if in_child { w.dynamic.iter().filter(|d| !d.replace).map(|d| d.ref_id).collect() } else { HashSet::new() };
        for x in all_worlds.iter().filter(|x| x.parent == Some(world.form_id) && x.parent_flags & 0x02 != 0) {
            let Some(b) = crate::dyndolod_build::child_config(c, &x.editor_id, &world.editor_id).0 else { continue };
            let dups = lodforge_dyndolod::childworld::parent_duplicates(&p.load_order, world, x, &c.bases, &matches);
            let mut cf = DynFile::new(Common { world: x.editor_id.clone(), season: String::new(), generator: crate::instances::generator(), plugins: Vec::new(), params: format!("parent_copies_of={}\nbounds={},{},{},{}", world.editor_id, b.0, b.1, b.2, b.3), token: crate::instances::build_token() });
            let xr = names.split(x.form_id).map(|(n, l)| cf.form(n, l)).unwrap_or_default();
            cf.header = DynHeader { world: xr, south_west: f.header.south_west, near_grid: near as u8, far_grid: far as u8, glow_multiple: 1.75, lod_world: world.editor_id.clone(), flags: 0 };
            // (twins, city copies, trees with an impostor type, trees without a mesh or type,
            // copies in child cells outside the bounds)
            let mut counts = (0usize, 0usize, 0usize, 0usize, 0usize);
            for l in w.lines {
                let cell = ((l.position[0] / 4096.0).floor() as i32, (l.position[1] / 4096.0).floor() as i32);
                let inside = cell.0 >= b.0 && cell.0 <= b.2 && cell.1 >= b.1 && cell.1 <= b.3;
                if (!inside && !x.cells.contains_key(&cell)) || in_child_dynamic.contains(&l.ref_id) || l.columns[..3].iter().all(|c| c.is_empty()) {
                    continue;
                }
                // Outside the bounds (a cell the child has beyond them) the runtime still draws the
                // parent's own instances and impostors; only the levels merged into the parent's
                // `.bto` (which the engine hides in the child's cells) need a copy.
                let mut columns = l.columns.clone();
                if !inside {
                    let key = names.split(l.ref_id).map(|(n, lf)| (n.to_ascii_lowercase(), lf));
                    if key.as_ref().is_some_and(|k| parent_trees.contains(k)) {
                        continue;
                    }
                    let bits = key.and_then(|k| inst.get(&k).copied()).unwrap_or(0);
                    for (k, bit) in [(0, LRG_INST4), (1, LRG_INST8), (2, LRG_INST16)] {
                        if bits & bit != 0 {
                            columns[k].clear();
                        }
                    }
                    if columns[..3].iter().all(|c| c.is_empty()) {
                        continue;
                    }
                    counts.4 += 1;
                }
                let l = &lodforge_dyndolod::export::RefLine { columns, ..l.clone() };
                // The original: a parent reference, or for the city's own content copied into the
                // parent, the child reference itself (hidden while its 3D is loaded, as any item).
                let (orig, base_id) = if lodforge_dyndolod::childworld::is_copy(l.ref_id) {
                    match w.child_copies.iter().find(|(cp, _)| cp.copy_id == l.ref_id && cp.child_world == x.form_id) {
                        Some((cp, _)) => (cp.ref_id, cp.placement.base),
                        None => continue,
                    }
                } else {
                    (l.ref_id, l.base)
                };
                let (Some((on, ol)), Some((bn, bl))) = (names.split(orig), names.split(base_id)) else { continue };
                let twin = dups.get(&l.ref_id).and_then(|&t| names.split(t));
                let base = c.bases.get(&base_id);
                let first = if l.columns[0].ends_with(".nif") { l.columns[0].as_str() } else { "" };
                let m = model_entry(&mut cf, first, &l.columns, false, base, w.variants.and_then(|v| v.get(&l.ref_id)));
                let mut flags = GRID_NEAR | PARENT_COPY | INITIALLY_ENABLED;
                let is_tree = base.is_some_and(lodforge_dyndolod::assign::is_tree);
                if is_tree {
                    flags |= TREE;
                }
                if world.large_ref_cells.contains_key(&l.ref_id) || x.large_ref_cells.contains_key(&orig) {
                    flags |= LARGE;
                }
                if twin.is_some() {
                    flags |= TWIN;
                    counts.0 += 1;
                }
                if orig != l.ref_id {
                    counts.1 += 1;
                }
                let rot = l.rotation.map(|d| d.to_radians() as f32);
                let rc = world.refs.get(&l.ref_id).or_else(|| x.refs.get(&orig)).and_then(|r| r.cell).map_or(cell_of(l.position), |(x, y)| (x as i16, y as i16));
                let impostor = if is_tree { impostors.get(&base_id).map_or(0, |&t| t as u16 + 1) } else { 0 };
                let it = cf.add(l.position, rot, l.scale, (on, ol), None, m, flags, rc, (bn, bl), twin);
                it.impostor = impostor;
                if is_tree && first.is_empty() {
                    if impostor > 0 {
                        counts.2 += 1;
                    } else {
                        counts.3 += 1;
                    }
                }
            }
            rep.child_copies_per_world.push((x.editor_id.clone(), cf.items(), counts.0, counts.1, counts.2, counts.3, counts.4));
            if cf.items() == 0 {
                continue;
            }
            cf.finish();
            rep.models += cf.models.len();
            let bytes = cf.to_bytes(opts)?;
            let path = dir.join(cf.common.file_name(EXT));
            std::fs::write(&path, &bytes).with_context(|| format!("writing {}", path.display()))?;
            rep.files += 1;
            rep.bytes += bytes.len();
        }
        if f.items() == 0 && f.large.is_empty() && f.underside.is_none() && f.resets.is_empty() && f.migration.is_empty() {
            continue;
        }
        f.finish();
        rep.models += f.models.len();
        let bytes = f.to_bytes(opts)?;
        anyhow::ensure!(DynFile::from_bytes(&bytes)?.items() == f.items(), "dyn file reads back differently");
        let path = dir.join(f.common.file_name(EXT));
        std::fs::write(&path, &bytes).with_context(|| format!("writing {}", path.display()))?;
        rep.files += 1;
        rep.bytes += bytes.len();
    }
    crate::instances::write_build_txt(out)?;
    Ok(rep)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_under_yaw_and_scale() {
        // 90 degrees about z: R = Rz(-pi/2) maps x to -y.
        let b = world_bounds([0, 0, 0, 10, 20, 30], [100.0, 200.0, 0.0], [0.0, 0.0, std::f32::consts::FRAC_PI_2], 2.0);
        let r: Vec<i32> = b.iter().map(|v| v.round() as i32).collect();
        assert_eq!(r, vec![100, 180, 0, 140, 200, 60]);
    }

    #[test]
    fn grids_and_glow() {
        assert_eq!(grid_flags(Grid::FarLod, false), GRID_NEAR);
        assert_eq!(grid_flags(Grid::FarFull, true), GRID_FAR);
        assert_eq!(grid_flags(Grid::NeverFadeLod, false), GRID_NEVER_FADE);
        assert_eq!(glow_level(r"meshes\x\windowglow_lod_1.nif"), 1);
        assert_eq!(glow_level(r"meshes\x\rock.nif"), GLOW_LEVEL_NONE);
        assert_eq!(mesh_path("Architecture\\A.nif"), r"meshes\architecture\a.nif");
        assert_eq!(mesh_path(r"textures\x.dds"), r"textures\x.dds");
    }
}

/// `lodforge runtime-dyn-info`.
#[derive(clap::Args)]
pub struct InfoArgs {
    /// A `.dyn.bin` file.
    pub file: std::path::PathBuf,
    /// Also print this many items.
    #[arg(long, default_value_t = 0)]
    pub items: usize,
    /// A `.statics.bin` (the parent's): print items whose reference is instanced there at LOD4
    /// with other textures or a snow/ash class than the item's plain model.
    #[arg(long)]
    pub statics: Option<std::path::PathBuf>,
    /// Print these items in full (`<form hex>`, any plugin), with their model slots and variants.
    #[arg(long, value_delimiter = ',')]
    pub item: Vec<String>,
}

pub fn info(a: &InfoArgs) -> Result<()> {
    let f = DynFile::from_bytes(&std::fs::read(&a.file)?).with_context(|| format!("reading {}", a.file.display()))?;
    let h = &f.header;
    println!("{} (lod world {}), token {:?}, {} plugins, SW ({}, {}), grids {}/{}, glow {}", f.common.world, h.lod_world, f.common.token, f.common.plugins.len(), h.south_west.0, h.south_west.1, h.near_grid, h.far_grid, h.glow_multiple);
    let items: Vec<&DynItem> = f.cells.values().flatten().collect();
    let count = |m: u32| items.iter().filter(|i| i.flags & m != 0).count();
    let grid = |g: u32| items.iter().filter(|i| i.grid() == g).count();
    println!("items {} in {} cells: near {}, far {}, never-fade {}; large {}, parent {} (opposite {}), initially enabled {}, replace {}, persistent {}, door {}, full {}, animated {}, glow {}, child copies {}, in child worlds {}", items.len(), f.cells.len(), grid(GRID_NEAR), grid(GRID_FAR), grid(GRID_NEVER_FADE), count(LARGE), items.iter().filter(|i| i.has_parent()).count(), count(OPPOSITE), count(INITIALLY_ENABLED), count(REPLACE), count(PERSISTENT), count(DOOR), count(FULL), count(ANIMATED), count(GLOW), count(CHILD_COPY), count(IN_CHILD_WORLDS));
    let no_pack = f.models.iter().filter(|m| m.pack[0] == NO_MESH && m.flags & (MODEL_ANIMATED | MODEL_GLOW) == 0).count();
    let with_lod = f.models.iter().filter(|m| m.pack[1..].iter().any(|&p| p != NO_MESH)).count();
    let variant = f.models.iter().filter(|m| m.textures.iter().any(|t| !t.is_empty()) || m.classes.iter().any(|c| !c.is_empty())).count();
    println!("models {}: {} without a pack mesh for the dynamic model (not animated/glow), {} with some LOD4/8/16 pack mesh, {variant} with a draw variant (AltTextures / snow / whitening)", f.models.len(), no_pack, with_lod);
    for m in f.models.iter().filter(|m| m.pack[0] == NO_MESH && m.flags & (MODEL_ANIMATED | MODEL_GLOW) == 0).take(8) {
        println!("  no pack: {}", m.model);
    }
    let lr: Vec<&LargeRef> = f.large.values().flatten().collect();
    let lc = |m: u16| lr.iter().filter(|l| l.lod & m != 0).count();
    println!("large refs {} in {} cells: instanced {}, merged {}, dynamic {}, persistent {}, disabled/parent {}, no LOD {}", lr.len(), f.large.len(), lr.iter().filter(|l| l.lod & (LRG_INST4 | LRG_INST8 | LRG_INST16) != 0).count(), lc(LRG_BTO4 | LRG_BTO8 | LRG_BTO16), lc(LRG_DYN), lc(LRG_PERSISTENT), lc(LRG_DISABLED), lr.iter().filter(|l| l.lod & 0x7F == 0).count());
    println!("seasons {:?}; resets {}, disabled {}, bases {}; child worlds {}; underside {:?}", f.seasons.iter().map(|(c, v)| format!("{c}:{}", v.len())).collect::<Vec<_>>(), f.resets.len(), f.disabled.len(), f.bases.len(), f.child_worlds.iter().map(|c| format!("{} {:#x} {:?}", c.editor_id, c.flags, c.bounds)).collect::<Vec<_>>().join(", "), f.underside.as_ref().map(|u| (&u.model, u.height, u.worlds.len())));
    if let Some(sp) = &a.statics {
        let st = StaticsFile::from_bytes(&std::fs::read(sp)?)?;
        let mut inst: HashMap<(String, u32), usize> = HashMap::new();
        for ((level, _, _), recs) in &st.blocks {
            if *level == 4 {
                for r in recs {
                    if let Some(n) = st.common.plugins.get(usize::from(r.plugin)) {
                        inst.insert((n.to_ascii_lowercase(), r.form), usize::from(r.mesh));
                    }
                }
            }
        }
        let (mut found, mut variant) = (0, 0);
        for i in &items {
            let k = (f.common.plugins[usize::from(i.original.plugin)].to_ascii_lowercase(), i.original.form);
            let Some(&m) = inst.get(&k) else { continue };
            found += 1;
            let me = &st.meshes[m];
            if !me.textures.is_empty() || me.classes.iter().any(|&c| c != 0) {
                variant += 1;
                if variant <= 12 {
                    println!("  variant: {}:{:06X} {} textures {:?} classes {:?}", k.0, k.1, me.model, me.textures.first(), me.classes);
                }
            }
        }
        println!("statics: {found} items instanced there at LOD4, {variant} with textures or classes the item does not carry");
    }
    for want in &a.item {
        let form = u32::from_str_radix(want.trim().trim_start_matches("0x"), 16).unwrap_or(0) & 0x00FF_FFFF;
        for i in items.iter().filter(|i| i.original.form == form) {
            let m = &f.models[i.model as usize];
            println!("item {}:{:06X} flags {:#x} twin {}:{:06X} impostor {} model #{} {:?} lod {:?} pack {:?}", f.common.plugins[usize::from(i.original.plugin)], i.original.form, i.flags, f.common.plugins.get(usize::from(i.direct_parent.plugin)).map_or("-", String::as_str), i.direct_parent.form, i.impostor, i.model, m.model, m.lod, m.pack);
            for s in 0..4 {
                println!("  slot {s}: textures {:?} classes {:?}", m.textures[s], m.classes[s]);
            }
        }
    }
    for i in items.iter().take(a.items) {
        let m = &f.models[i.model as usize];
        println!("  {}:{:06X} at ({:.0},{:.0},{:.0}) flags {:#x} parent {}:{:06X} model {} pack {:?}", f.common.plugins[usize::from(i.original.plugin)], i.original.form, i.pos[0], i.pos[1], i.pos[2], i.flags, f.common.plugins.get(usize::from(i.parent.plugin)).map_or("-", String::as_str), i.parent.form, m.model, m.pack);
    }
    Ok(())
}

/// Per slot (dynamic model = LOD4, LOD4, LOD8, LOD16): (AltTextures per part, part classes).
pub type Variant = [(Vec<(String, String)>, Vec<u8>); 4];

/// The draw variants (AltTextures, snow/ash class, VC whitening) of `lines` (default season),
/// exactly as object LOD would draw them, from the season export's header and side files.
pub fn variants(p: &Profile, s: &crate::dyndolod_build::SeasonExport, lines: &[lodforge_dyndolod::export::RefLine]) -> HashMap<u32, Variant> {
    let mut out = HashMap::new();
    if lines.is_empty() {
        return out;
    }
    let export = lodforge_objlod::ExportFile::parse_with(&s.text, lodforge_objlod::Dialect::Dyndolod);
    let mut opts = lodforge_objlod::Options::from_export(&export);
    opts.alt_textures = lodforge_objlod::sidefiles::AltTextures::parse(&s.alt_textures);
    opts.vc = lodforge_objlod::sidefiles::VcList::parse(&s.vc);
    let by_id: HashMap<&str, &lodforge_objlod::StaticRef> = export.statics.iter().map(|r| (r.ref_id.as_str(), r)).collect();
    let mut meshes: HashMap<String, Option<lodforge_objlod::mesh::LodMesh>> = HashMap::new();
    for l in lines {
        let id = format!("{:08X}", l.ref_id);
        let Some(r) = by_id.get(id.as_str()) else { continue };
        let mut v: Variant = Default::default();
        for (k, level) in [(0usize, 4i32), (1, 8), (2, 16)] {
            let Some(model) = r.model(k).map(|m| m.to_ascii_lowercase().replace('/', "\\")) else { continue };
            if !model.ends_with(".nif") {
                continue;
            }
            let mesh = meshes.entry(model.clone()).or_insert_with(|| crate::tree_pack::vfs_mesh(p, &model));
            let Some(mesh) = mesh.as_ref() else { continue };
            let (t, c) = opts.instance_variant(r, level, &model, mesh);
            v[k + 1] = (t.unwrap_or_default(), c);
        }
        v[0] = v[1].clone();
        if v.iter().any(|(t, c)| !t.is_empty() || !c.is_empty()) {
            out.insert(l.ref_id, v);
        }
    }
    out
}

/// The export lines of `world` inside the bounds of a child worldspace that uses its LOD (the
/// parent copies' candidates).
pub fn child_bound_lines(c: &Config, all_worlds: &[Worldspace], world: &Worldspace, lines: &[lodforge_dyndolod::export::RefLine]) -> Vec<lodforge_dyndolod::export::RefLine> {
    let children: Vec<(Option<(i32, i32, i32, i32)>, &Worldspace)> = all_worlds.iter().filter(|x| x.parent == Some(world.form_id) && x.parent_flags & 0x02 != 0).map(|x| (crate::dyndolod_build::child_config(c, &x.editor_id, &world.editor_id).0, x)).filter(|(b, _)| b.is_some()).collect();
    // Inside a child's bounds, or in any cell the child has (the engine hides the parent's LOD there).
    lines
        .iter()
        .filter(|l| {
            let (x, y) = ((l.position[0] / 4096.0).floor() as i32, (l.position[1] / 4096.0).floor() as i32);
            children.iter().any(|(b, w)| b.is_some_and(|b| x >= b.0 && x <= b.2 && y >= b.1 && y <= b.3) || w.cells.contains_key(&(x, y)))
        })
        .cloned()
        .collect()
}
