//! The LODGen export built straight from the load order: a port of the object LOD branch of
//! xEdit's `wbGenerateLODTES5` (`Core\wbLOD.pas`, MPL), which xLODGen runs before LODGen.
//!
//! Reference selection: winning REFRs of the worldspace whose base is a STAT or TREE, not
//! initially disabled or deleted, without an enable parent unless visible-when-distant, not a
//! persistent "never fades" / "is full LOD" reference, inside the LOD area; bases need the "Has
//! Distant LOD" flag (or be a TREE) and at least one existing LOD mesh (MNAM). Values go through
//! xEdit's text formatting, which LODGen parses back: rotations in degrees with 4 decimals, a
//! present XSCL with 6 decimals.

use std::collections::{HashMap, HashSet};

use lodforge_esp::record::{FLAG_DELETED, FLAG_INITIALLY_DISABLED, FLAG_PERSISTENT};
use lodforge_esp::{LoadOrder, Sig, Worldspace, fields, winning_records};

use crate::export::{ExportFile, StaticRef};

/// The LOD-relevant parts of a STAT / TREE base record (winning override).
#[derive(Debug, Clone, Default)]
pub struct Base {
    pub signature: [u8; 4],
    pub editor_id: String,
    pub flags: u32,
    pub model: String,
    /// MNAM LOD meshes, levels 0..3 (as stored, without `Meshes\`).
    pub lod: [String; 4],
    pub has_mnam: bool,
    /// DNAM material (MATO global form id), 0 for none.
    pub material: u32,
    /// DNAM "Considered Snow" (SSE).
    pub considered_snow: Option<u8>,
}

/// STAT "Never Fades", "Has Distant LOD".
pub const STAT_NEVER_FADES: u32 = 0x4;
pub const STAT_HAS_DISTANT_LOD: u32 = 0x8000;
/// REFR "Visible When Distant", "Is Full LOD".
pub const REFR_VISIBLE_WHEN_DISTANT: u32 = 0x8000;
pub const REFR_IS_FULL_LOD: u32 = 0x10000;

/// Meshes xEdit tells LODGen to place without the mesh's own translation/rotation.
pub const IGNORE_TRANSLATION_TES5: &[&str] = &[
    "meshes\\lod\\solitude\\cwtower01_lod.nif",
    "meshes\\lod\\solitude\\sfarmhousesilo_lod.nif",
    "meshes\\lod\\solitude\\slumbermill01_lod.nif",
    "meshes\\lod\\solitude\\spatiowall02_lod.nif",
    "meshes\\lod\\solitude\\spatiowall03_lod.nif",
    "meshes\\lod\\solitude\\spatiowall30_lod.nif",
    "meshes\\lod\\solitude\\spatiowallsteps01_lod.nif",
    "meshes\\lod\\solitude\\spatiowallsteps02_lod.nif",
    "meshes\\lod\\solitude\\sstyrrshouse_lod.nif",
    "meshes\\lod\\solitude\\sthe winking skeever_lod.nif",
    "meshes\\lod\\windhelm\\wharena_lod.nif",
    "meshes\\lod\\windhelm\\whbrunwulfsq_lod.nif",
    "meshes\\lod\\windhelm\\whgrayquarter_lod.nif",
    "meshes\\lod\\windhelm\\whinnerwall01_lod.nif",
    "meshes\\lod\\windhelm\\whinnerwall02_lod.nif",
    "meshes\\lod\\windhelm\\whinnland_lod.nif",
    "meshes\\lod\\windhelm\\whmaingate_lod.nif",
    "meshes\\lod\\windhelm\\whmarket01_lod.nif",
    "meshes\\lod\\windhelm\\whouterwall3_lod.nif",
    "meshes\\lod\\windhelm\\whpalace_lod.nif",
    "meshes\\lod\\windhelm\\whtempletalos_lod.nif",
    "meshes\\lod\\windhelm\\whvalunstrad_lod.nif",
];

fn zstr(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    b[..end].iter().map(|&c| c as char).collect()
}

/// Winning STAT and TREE records.
pub fn read_bases(lo: &LoadOrder) -> lodforge_esp::Result<HashMap<u32, Base>> {
    let mut out = HashMap::new();
    for sig in [*b"STAT", *b"TREE"] {
        for (id, (pi, r)) in winning_records(lo, Sig(sig))? {
            let lp = &lo.plugins[pi as usize];
            let data = r.data()?;
            let mut b = Base { signature: sig, flags: r.flags, ..Default::default() };
            for f in fields(&data) {
                match &f.sig.0 {
                    b"EDID" => b.editor_id = f.zstring(),
                    b"MODL" => b.model = f.zstring(),
                    b"DNAM" if f.data.len() >= 8 => {
                        let m = u32::from_le_bytes([f.data[4], f.data[5], f.data[6], f.data[7]]);
                        b.material = if m == 0 { 0 } else { lp.global_form_id(m).unwrap_or(0) };
                        b.considered_snow = f.data.get(8).copied();
                    }
                    b"MNAM" => {
                        b.has_mnam = true;
                        for (k, l) in b.lod.iter_mut().enumerate() {
                            if let Some(s) = f.data.get(k * 260..(k + 1) * 260) {
                                *l = zstr(s);
                            }
                        }
                    }
                    _ => {}
                }
            }
            out.insert(id, b);
        }
    }
    Ok(out)
}

/// Editor IDs of the winning MATO (material object) records.
pub fn read_materials(lo: &LoadOrder) -> lodforge_esp::Result<HashMap<u32, String>> {
    let mut out = HashMap::new();
    for (id, (_, r)) in winning_records(lo, Sig(*b"MATO"))? {
        let data = r.data()?;
        let e = fields(&data).find(|f| &f.sig.0 == b"EDID").map(|f| f.zstring()).unwrap_or_default();
        out.insert(id, e);
    }
    Ok(out)
}

/// `TwbAsset.GetAssetName(name, '', atMesh)` for mesh paths: the part from `meshes\` on (or
/// after a `data\` folder), else `Meshes\` + the path.
pub fn mesh_asset_name(name: &str) -> String {
    if name.trim().len() < 2 {
        return String::new();
    }
    let path = format!("\\{}", name.to_lowercase().replace('/', "\\"));
    if let Some(i) = path.find("\\data\\") {
        return name[i + 5..].to_string();
    }
    if let Some(i) = path.find("\\meshes\\") {
        return name[i..].to_string();
    }
    format!("Meshes\\{}", name.trim_start_matches(['\\', '/']))
}

/// Delphi `SingleSameValue`.
fn single_same(a: f64, b: f64) -> bool {
    const RES: f64 = 0.000_000_5;
    let (a, b) = (f64::from(a as f32), f64::from(b as f32));
    (a - b).abs() <= (a.abs().min(b.abs()) * RES).max(RES)
}

/// xEdit's `RoundToEx(v, -digits)` = `Round(v / IntPower(10, -digits)) * IntPower(...)`, where
/// the factor ends up in single precision (the golden exports only fit with it), with banker's
/// rounding.
fn round_to(v: f64, digits: i32) -> f64 {
    let f = f64::from(10f32.powi(-digits));
    (v / f).round_ties_even() * f
}

/// xEdit's rotation text (radians normalised to 0..2pi with a single-precision 2pi, degrees
/// rounded to 4 decimals) parsed back as LODGen does. Matches 99.7 % of the golden Tamriel export's
/// angles exactly; the rest differ by 0.0001 degree.
pub fn angle_as_lodgen(rad: f32) -> f32 {
    let two_pi = std::f64::consts::TAU;
    let tp = f64::from(std::f32::consts::TAU);
    let mut r = f64::from(rad);
    if (r / two_pi).abs() > 100.0 {
        r -= r.signum() * two_pi * ((r / two_pi).abs() - 100.0).trunc();
        if (r / two_pi).abs() > 101.0 {
            return f32::NAN;
        }
    }
    while r < 0.0 {
        r += tp;
    }
    while r > two_pi {
        r -= tp;
    }
    if single_same(r, 0.0) || r < 0.0 || single_same(r, two_pi) || r > two_pi {
        r = 0.0;
    }
    let deg = round_to(r * (180.0 / std::f64::consts::PI), 4);
    format!("{deg:.4}").parse().unwrap_or(0.0)
}

/// xEdit's position text: rounded to 6 decimals and stored as single (printed with 15 digits,
/// so LODGen parses back the same single).
pub fn position_as_lodgen(v: f32) -> f32 {
    round_to(f64::from(v), 6) as f32
}

/// A present XSCL as xEdit prints it (6 decimals), parsed back.
pub fn scale_as_lodgen(s: f32) -> f32 {
    format!("{:.6}", round_to(f64::from(s), 6)).parse().unwrap_or(1.0)
}
/// Bases xLODGen's winter pass leaves without the snow shader. The 4.1.5o beta's rule is not in
/// the public source; on Tamriel it skips exactly the material-less bases whose EditorID contains
/// "ice" or "frozen" (icebergs, ice floes, `ImpExtIce*_NoIce`, frozen-marsh mountain pieces).
pub fn snow_exempt(editor_id: &str) -> bool {
    let e = editor_id.to_ascii_lowercase();
    e.contains("ice") || e.contains("frozen")
}

/// Options of the export that are not in the load order.
#[derive(Debug, Clone)]
pub struct ExportOptions {
    /// LOD area south-west cell (lodsettings).
    pub south_west: (i32, i32),
    /// `UVRange - 1` (xLODGen default 1.5 -> 0.5).
    pub atlas_tolerance: f32,
    /// Season code written as `Season=` (output file suffix).
    pub season: Option<String>,
    /// Base record swaps for the season (`[Statics]` / `[Trees]` of `Seasons\*_<SEASON>.ini`).
    pub base_swaps: HashMap<u32, u32>,
    /// Winter: references whose base has no material get `Snow` (see [snow_exempt]).
    pub winter_snow: bool,
}

/// Why references were left out (for the report).
#[derive(Debug, Default, Clone)]
pub struct ExportStats {
    pub refs: usize,
    pub not_static: usize,
    pub disabled: usize,
    pub enable_parent: usize,
    pub persistent: usize,
    pub out_of_area: usize,
    pub no_lod: usize,
    pub large_refs: usize,
    /// LOD meshes of the exported bases (xEdit's `slLODMeshes`), for the atlas texture list.
    pub lod_meshes: std::collections::BTreeSet<String>,
}

/// Builds the export for `world`. `exists` answers whether a game path (`Meshes\...`) exists.
pub fn export_from_load_order(lo: &LoadOrder, world: &Worldspace, bases: &HashMap<u32, Base>, materials: &HashMap<u32, String>, opts: &ExportOptions, exists: &(dyn Fn(&str) -> bool + Sync)) -> (ExportFile, ExportStats) {
    let mut st = ExportStats::default();
    let sw = opts.south_west;
    // Large references: RNAM lists of ESM plugins whose grid matches the reference's own cell.
    let mut large: HashSet<u32> = HashSet::new();
    for &(pi, grid, r) in &world.large_refs {
        if !lo.plugins[pi as usize].plugin.is_master() {
            continue;
        }
        if let Some(p) = world.refs.get(&r) {
            let cell = ((p.position[0] / 4096.0).floor() as i32, (p.position[1] / 4096.0).floor() as i32);
            if cell == grid {
                large.insert(r);
            }
        }
    }
    st.large_refs = large.len();

    // Per-base export columns (LODGen: static name, flags, material, full model, LOD4/8/16).
    let mut cache: HashMap<u32, Option<(String, u32, String, String, [String; 3])>> = HashMap::new();
    let mut base_line = |id: u32, b: &Base| -> Option<(String, u32, String, String, [String; 3])> {
        cache
            .entry(id)
            .or_insert_with(|| {
                if !(&b.signature == b"TREE" || b.flags & STAT_HAS_DISTANT_LOD != 0) {
                    return None;
                }
                let lod_name = |k: usize| -> String {
                    if !b.has_mnam {
                        return String::new();
                    }
                    let n = mesh_asset_name(&b.lod[k]);
                    if !n.is_empty() && exists(&n) { n } else { String::new() }
                };
                let m = [lod_name(0), lod_name(1), lod_name(2)];
                if m.iter().all(String::is_empty) {
                    return None;
                }
                let mut mat = if b.material != 0 { materials.get(&b.material).map(|e| e.to_lowercase()).unwrap_or_default() } else { String::new() };
                if mat.contains("snow") {
                    mat = "Snow".into();
                } else if mat.contains("ash") {
                    mat = "Ash".into();
                } else if mat.contains("passthru") {
                    mat = "PassThru".into();
                }
                Some((b.editor_id.clone(), b.flags, mat, mesh_asset_name(&b.model), m))
            })
            .clone()
    };

    let mut ids: Vec<u32> = world.refs.keys().copied().collect();
    ids.sort_unstable();
    let mut statics = Vec::new();
    for id in ids {
        st.refs += 1;
        let p = &world.refs[&id];
        let base_id = opts.base_swaps.get(&p.base).copied().unwrap_or(p.base);
        let Some(b) = bases.get(&base_id) else {
            st.not_static += 1;
            continue;
        };
        if p.flags & (FLAG_INITIALLY_DISABLED | FLAG_DELETED) != 0 {
            st.disabled += 1;
            continue;
        }
        if p.flags & REFR_VISIBLE_WHEN_DISTANT == 0 && p.enable_parent {
            st.enable_parent += 1;
            continue;
        }
        if p.flags & FLAG_PERSISTENT != 0 && (b.flags & STAT_NEVER_FADES != 0 || p.flags & REFR_IS_FULL_LOD != 0) {
            st.persistent += 1;
            continue;
        }
        let cell = ((p.position[0] / 4096.0).floor() as i32, (p.position[1] / 4096.0).floor() as i32);
        let block = (sw.0 + (cell.0 - sw.0) / 4 * 4, sw.1 + (cell.1 - sw.1) / 4 * 4);
        if block.0 < sw.0 || block.1 < sw.1 {
            st.out_of_area += 1;
            continue;
        }
        let line = base_line(base_id, b);
        if let Some((_, _, _, _, m)) = &line {
            st.lod_meshes.extend(m.iter().filter(|x| !x.is_empty()).cloned());
        }
        let Some((edid, flags, mut mat, full, m)) = line else {
            st.no_lod += 1;
            continue;
        };
        if opts.winter_snow && mat.is_empty() && !snow_exempt(&edid) {
            mat = "Snow".into();
        }
        if large.contains(&id) {
            mat.push_str("-LargeRef");
        }
        statics.push(StaticRef {
            ref_id: format!("{id:08X}"),
            ref_flags: p.flags,
            x: position_as_lodgen(p.position[0]),
            y: position_as_lodgen(p.position[1]),
            z: position_as_lodgen(p.position[2]),
            cell_x: (p.position[0] / 4096.0).floor() as i32,
            cell_y: (p.position[1] / 4096.0).floor() as i32,
            rot_x: angle_as_lodgen(p.rotation[0]),
            rot_y: angle_as_lodgen(p.rotation[1]),
            rot_z: angle_as_lodgen(p.rotation[2]),
            scale: if p.has_scale { scale_as_lodgen(p.scale) } else { 1.0 },
            static_name: edid,
            static_flags: flags,
            material_name: mat,
            full_model: full,
            models: [m[0].to_lowercase(), m[1].to_lowercase(), m[2].to_lowercase(), String::new()],
            line: statics.len(),
        });
    }

    let hd = mask_texture(&world.hd_lod_texture);
    let export = ExportFile {
        dialect: crate::export::Dialect::XLodgen,
        game_mode: "SSE".into(),
        worldspace: world.editor_id.clone(),
        south_west: sw,
        season: opts.season.clone().unwrap_or_default(),
        atlas_tolerance: opts.atlas_tolerance,
        hd_textures: if hd.is_empty() { Vec::new() } else { vec![hd.clone(), format!("{}_n.dds", hd.trim_end_matches(".dds"))] },
        ignore_translation: IGNORE_TRANSLATION_TES5.iter().map(|s| s.to_string()).collect(),
        statics,
        ..Default::default()
    };
    (export, st)
}

/// WRLD TNAM as the export's `TextureDiffuseHD` mask (lower-case `textures\...`).
fn mask_texture(t: &str) -> String {
    if t.trim().is_empty() {
        return String::new();
    }
    let l = t.to_lowercase().replace('/', "\\");
    let l = l.trim_start_matches('\\');
    let l = match lodforge_nif::data_folder_start(l, "textures") {
        Some(i) => l[i..].to_string(),
        None => format!("textures\\{l}"),
    };
    l
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xedit_formatting() {
        assert_eq!(mesh_asset_name("LOD\\Farmhouse\\Farmhouse02_LOD.nif"), "Meshes\\LOD\\Farmhouse\\Farmhouse02_LOD.nif");
        assert_eq!(mesh_asset_name("meshes\\a\\b.nif"), "meshes\\a\\b.nif");
        assert_eq!(angle_as_lodgen(std::f32::consts::FRAC_PI_4 * 3.0), 135.0);
        assert_eq!(angle_as_lodgen(-0.1), "354.2704".parse::<f32>().unwrap());
        assert_eq!(scale_as_lodgen(0.69), 0.69);
        assert_eq!(mask_texture("Landscape\\Mountains\\MountainSlab02.dds"), "textures\\landscape\\mountains\\mountainslab02.dds");
        assert_eq!(mask_texture("MyTextures\\Slab.dds"), "textures\\mytextures\\slab.dds");
    }
}
