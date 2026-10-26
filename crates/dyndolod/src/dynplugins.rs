//! DynDOLOD.esm / DynDOLOD.esp and the DLL NG data files (`SKSE\Plugins\DynDOLOD_Data\
//! DynDOLOD_NG_Worlds.txt`, `DynDOLOD_NG_<World>.txt`) for the dynamic LOD references
//! (docs/re/dyndolod-spec.md "Plugin records" and "DLL NG and scripts").
//!
//! Includes the runtime framework, base copies, dynamic references, replacement originals and
//! large-reference workarounds. See the qualification reports for remaining runtime gaps.

use std::collections::HashMap;

use lodforge_esp::{LoadOrder, Worldspace};

use crate::bases::Base;
use crate::export::DynamicRef;
use crate::plugin::{COMPANION, CellContent, NEW, PluginBuilder, Previous, Rec, WorldContent, record_key};

pub const KEYWORD_ID: u32 = 0x910;
/// KYWD whose EditorID is the plugin format version the DLL compares (`3.07`).
pub const VERSION_KEYWORD_ID: u32 = 0x912;
pub const VERSION: &str = "3.07";
/// Framework records at DynDOLOD's fixed object ids.
pub const MASTER_ACTIVATOR_ID: u32 = 0x901;
pub const OBJECT_ACTIVATOR_ID: u32 = 0x905;
pub const MCM_QUEST_ID: u32 = 0x90F;
pub const QUEST_ID: u32 = 0x911;
/// First object id for generated records (after the fixed framework ids).
pub const FIRST_OBJECT_ID: u32 = 0x1000;
/// The player reference (Skyrim.esm).
const PLAYER_REF: u32 = 0x14;

/// A papyrus script property value (VMAD object format 2).
pub enum Prop<'a> {
    Object(u32),
    Str(&'a str),
}

/// VMAD bytes being built, with the offsets of the form ids inside.
#[derive(Default)]
pub struct Vmad {
    pub bytes: Vec<u8>,
    pub ids: Vec<usize>,
}

impl Vmad {
    fn wstr(&mut self, s: &str) {
        self.bytes.extend((s.len() as u16).to_le_bytes());
        self.bytes.extend(s.as_bytes());
    }

    fn object(&mut self, alias: i16, form_id: u32) {
        self.bytes.extend(0u16.to_le_bytes());
        self.bytes.extend(alias.to_le_bytes());
        self.ids.push(self.bytes.len());
        self.bytes.extend(form_id.to_le_bytes());
    }

    /// `version 5, object format 2`, then the scripts with their properties.
    pub fn scripts(&mut self, scripts: &[(&str, &[(&str, Prop<'_>)])]) {
        self.bytes.extend(5i16.to_le_bytes());
        self.bytes.extend(2i16.to_le_bytes());
        self.script_list(scripts);
    }

    fn script_list(&mut self, scripts: &[(&str, &[(&str, Prop<'_>)])]) {
        self.bytes.extend((scripts.len() as u16).to_le_bytes());
        for (name, props) in scripts {
            self.wstr(name);
            self.bytes.push(0);
            self.bytes.extend((props.len() as u16).to_le_bytes());
            for (pname, value) in props.iter() {
                self.wstr(pname);
                match value {
                    Prop::Object(id) => {
                        self.bytes.extend([1, 1]);
                        self.object(-1, *id);
                    }
                    Prop::Str(v) => {
                        self.bytes.extend([2, 1]);
                        self.wstr(v);
                    }
                }
            }
        }
    }

    /// Quest part: no fragments, one alias (id 0 of `quest`) running `scripts`.
    pub fn quest_alias(&mut self, quest: u32, scripts: &[(&str, &[(&str, Prop<'_>)])]) {
        self.bytes.push(2);
        self.bytes.extend(0u16.to_le_bytes());
        self.bytes.extend(0u16.to_le_bytes());
        self.bytes.extend(1u16.to_le_bytes());
        self.object(0, quest);
        self.bytes.extend(5i16.to_le_bytes());
        self.bytes.extend(2i16.to_le_bytes());
        self.script_list(scripts);
    }
}

/// A start-game-enabled quest with a player alias, as DynDOLOD writes its two framework quests.
fn quest(id: u32, edid: &str, full: &str, vmad: Vmad) -> Rec {
    let mut alfr = Vec::new();
    alfr.extend(PLAYER_REF.to_le_bytes());
    Rec::new(b"QUST", 0, NEW | id)
        .field(b"EDID", zs(edid))
        .field_ids(b"VMAD", vmad.bytes, &vmad.ids)
        .field(b"FULL", zs(full))
        .field(b"DNAM", vec![0x11, 0x01, 0x00, 0xFF, 0, 0, 0, 0, 0, 0, 0, 0])
        .field(b"NEXT", Vec::new())
        .field(b"ANAM", 1u32.to_le_bytes().to_vec())
        .field(b"ALST", 0u32.to_le_bytes().to_vec())
        .field(b"ALID", zs("PlayerAlias"))
        .field(b"FNAM", 0u32.to_le_bytes().to_vec())
        .field_ids(b"ALFR", alfr, &[0])
        .field(b"ALED", Vec::new())
}

/// The records DynDOLOD DLL NG and its scripts expect in DynDOLOD.esp: the version keyword, the
/// master and object activators and the init and MCM quests (each with a player alias), at
/// DynDOLOD's fixed object ids.
fn framework(esp: &mut PluginBuilder) {
    esp.add(Rec::new(b"KYWD", 0, NEW | VERSION_KEYWORD_ID).field(b"EDID", zs(VERSION)).field(b"CNAM", vec![0u8; 4]));
    let quest_id = NEW | QUEST_ID;
    let mut v = Vmad::default();
    v.scripts(&[("SHESON_DynDOLOD_Master", &[("MyQuest", Prop::Object(quest_id)), ("PlayerRef", Prop::Object(PLAYER_REF))])]);
    esp.add(
        Rec::new(b"ACTI", 0x0400_0200, NEW | MASTER_ACTIVATOR_ID)
            .field(b"EDID", zs("DynDOLOD_MasterActivator"))
            .field_ids(b"VMAD", v.bytes, &v.ids)
            .field(b"OBND", vec![0x00, 0xF8, 0x00, 0xF8, 0x00, 0x83, 0x00, 0x08, 0x00, 0x08, 0x00, 0x7D])
            .field(b"FULL", zs("ThouShaltBowToSheson"))
            .field(b"PNAM", vec![0u8; 4])
            .field(b"FNAM", vec![0u8; 2]),
    );
    let mut v = Vmad::default();
    v.scripts(&[("SHESON_DynDOLOD_ObjectActivator", &[("PlayerRef", Prop::Object(PLAYER_REF))])]);
    esp.add(
        Rec::new(b"ACTI", 0x0480_0200, NEW | OBJECT_ACTIVATOR_ID)
            .field(b"EDID", zs("DynDOLOD_ObjectActivator"))
            .field_ids(b"VMAD", v.bytes, &v.ids)
            .field(b"OBND", vec![0u8; 12])
            .field(b"FULL", zs("ThouShaltBowToObject"))
            .field(b"MODL", zs("markerx.nif"))
            .field(b"PNAM", vec![0u8; 4])
            .field(b"FNAM", vec![0u8; 2]),
    );
    let mcm_id = NEW | MCM_QUEST_ID;
    let mut v = Vmad::default();
    v.scripts(&[("SHESON_DynDOLOD_MCM", &[("ModName", Prop::Str("DynDOLOD")), ("MyQuest", Prop::Object(quest_id)), ("PlayerRef", Prop::Object(PLAYER_REF))])]);
    v.quest_alias(mcm_id, &[("SKI_PlayerLoadGameAlias", &[])]);
    esp.add(quest(MCM_QUEST_ID, "DynDOLOD_MCM", "ThouShaltBowToUser", v));
    let mut v = Vmad::default();
    v.scripts(&[("SHESON_DynDOLOD_Quest", &[("PlayerRef", Prop::Object(PLAYER_REF))])]);
    v.quest_alias(quest_id, &[("SHESON_DynDOLOD_Alias", &[("Script", Prop::Object(quest_id))])]);
    esp.add(quest(QUEST_ID, "DynDOLOD_Quest", "ThouShaltBowToQuest", v));
}

/// TES4 description as DynDOLOD writes it; the version is its last word.
fn description(file: &str, role: &str) -> String {
    format!("LODForge {file} SSE Generated Plugin\r{role}\rPlugin generated by DynDOLODVersion {VERSION}")
}

/// A 32-character run token (`bunchofnumbers`): digits and upper-case letters.
pub fn bunch_of_numbers(seed: u64) -> String {
    let mut s = seed ^ 0x9E37_79B9_7F4A_7C15;
    (0..32)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            let v = (s % 36) as u8;
            if v < 10 { (b'0' + v) as char } else { (b'A' + v - 10) as char }
        })
        .collect()
}

fn zs(s: &str) -> Vec<u8> {
    let mut v = s.as_bytes().to_vec();
    v.push(0);
    v
}

pub struct Output {
    pub esm: Vec<u8>,
    pub esp: Vec<u8>,
    pub worlds_txt: String,
    pub world_txt: String,
}

/// Global DLL base-flag updates for emitted large-reference LOD. The DLL sets HasDistantLOD;
/// it does not assign MNAM paths. Existing flags need no update.
pub fn base_lod_section(lo: &LoadOrder, bases: &HashMap<u32, Base>, ids: impl IntoIterator<Item = u32>) -> Result<String, String> {
    let owners = crate::bases::owners(lo);
    let mut lines = String::new();
    for id in ids.into_iter().collect::<std::collections::BTreeSet<_>>() {
        let base = bases.get(&id).ok_or_else(|| format!("missing LOD base {id:08X}"))?;
        if base.flags & 0x8000 != 0 {
            continue;
        }
        let owner = owners.get(&crate::bases::owner_key(id)).ok_or_else(|| format!("missing owner of LOD base {id:08X}"))?;
        let local = if id >> 24 == 0xfe { id & 0xfff } else { id & 0xffffff };
        if local == 0 {
            return Err(format!("invalid zero local ID for LOD base {id:08X}"));
        }
        lines.push_str(&format!("{local},{}\r\n", lo.plugins[*owner].plugin.name));
    }
    Ok(format!("[Bases]\r\n{lines}"))
}

/// One worldspace's dynamic LOD input for [`build_worlds`].
pub struct WorldDynamic<'a> {
    pub world: &'a Worldspace,
    pub dynamic: &'a [DynamicRef],
    /// Lodsettings south-west cell.
    pub south_west: (i32, i32),
    /// References that get object, tree or dynamic LOD (for the Is Full LOD reverts).
    pub lod_refs: &'a [u32],
    /// Child worldspaces for the large-reference workarounds.
    pub children: Vec<ChildWorld<'a>>,
    /// Terrain underside: (`TerrainUndersideHeight`, the worldspace and its child worldspaces
    /// for the enabler list) when `<World>_Underside.nif` was built.
    pub underside: Option<(f32, Vec<u32>)>,
    /// Per season (`SPR`, ...): the (base, model) of each dynamic reference in that season, for
    /// the Seasons of Skyrim swap lists.
    pub seasonal: Vec<(&'static str, HashMap<u32, (u32, String)>)>,
    /// Child worldspace references copied in, with the model of their base copy.
    pub child_copies: Vec<(crate::childworld::ChildCopy, String)>,
}

/// A child worldspace of a [`WorldDynamic`].
pub struct ChildWorld<'a> {
    pub world: &'a Worldspace,
    /// Full-LOD child originals represented by selected parent-world LOD.
    pub revert_refs: Vec<u32>,
    /// Childworld configuration `Bounds` (SW x, SW y, NE x, NE y); children without a childworld
    /// configuration get no workarounds.
    pub bounds: Option<(i32, i32, i32, i32)>,
    /// `NoCellsWithNAVM=1`: no markers in cells with a navmesh.
    pub no_navmesh_cells: bool,
}

/// DynDOLOD's large-reference workarounds (`WorkaroundLargeReferencesBugs=1`).
#[derive(Clone, Copy, Debug)]
pub struct Workarounds {
    /// Child-world marker margin in cells: (uLargeRefLODGridSize - uGridsToLoad) / 2.
    pub margin: i32,
}

/// The `_Disciple_` marker base in DynDOLOD.esm.
pub const DISCIPLE_MARKER_ID: u32 = 0x928;
const MARKER_MODEL: &str = "MarkerX.nif";

fn i16le(v: i32) -> [u8; 2] {
    (v as i16).to_le_bytes()
}

/// Child worldspace copies for a parent's DynDOLOD.esm content: temporary REFRs per parent cell,
/// and the large ones among them per cell.
#[derive(Default)]
struct CopiedRefs {
    cells: std::collections::BTreeMap<(i32, i32), Vec<Rec>>,
    large: std::collections::BTreeMap<(i32, i32), Vec<u32>>,
}

/// DynDOLOD.esm content for one worldspace: a WRLD override whose RNAM lists, per cell, the
/// cell's marker and its large child-world copies; the cells (overrides of the winning CELL, or
/// new ones for markers where the worldspace has none) with their `<World>_Disciple_<x>_<y>`
/// marker and copies; and persistent references (large-reference reverts). Returns the marker
/// form id per cell.
fn esm_world(lo: &LoadOrder, esm: &mut PluginBuilder, world: &Worldspace, parent: Option<&Worldspace>, cells: &[crate::largerefs::MarkerCell], persistent: Vec<Rec>, copies: CopiedRefs) -> HashMap<(i32, i32), u32> {
    let mut ids = HashMap::new();
    let Some(mut w) = world.world_record.and_then(|wr| Rec::copy(lo, wr)) else { return ids };
    if cells.is_empty() && persistent.is_empty() && copies.cells.is_empty() {
        return ids;
    }
    // grid -> (cell record, temporary references, RNAM entries)
    let mut grids: std::collections::BTreeMap<(i32, i32), (Rec, Vec<Rec>, Vec<u32>)> = std::collections::BTreeMap::new();
    for m in cells {
        let (x, y) = m.grid;
        let cell = match world.cell_records.get(&m.grid).and_then(|&c| Rec::copy(lo, c)) {
            Some(c) => c,
            None => {
                let edid = format!("{}&DynDOLOD&CELL&{x}&{y}", world.editor_id);
                let id = esm.keyed_id(&record_key(b"CELL", &edid, ""));
                let flags = parent.and_then(|p| p.cell_flags.get(&m.grid)).map_or(0x02, |f| f & !lodforge_esp::CELL_SHORT_DATA);
                let mut xclc = Vec::new();
                xclc.extend(x.to_le_bytes());
                xclc.extend(y.to_le_bytes());
                xclc.extend(0u32.to_le_bytes());
                Rec::new(b"CELL", 0, id).field(b"EDID", zs(&edid)).field(b"DATA", flags.to_le_bytes().to_vec()).field(b"XCLC", xclc).field(b"LTMP", vec![0u8; 4]).field(b"XCLW", vec![0xFF, 0xFF, 0x7F, 0x7F])
            }
        };
        let edid = crate::largerefs::marker_editor_id(&world.editor_id, m.grid);
        let rid = esm.keyed_id(&record_key(b"REFR", &edid, ""));
        let mut data = Vec::with_capacity(24);
        for v in [(x as f32 + 0.5) * 4096.0, (y as f32 + 0.5) * 4096.0, 0.0, 0.0, 0.0, 0.0] {
            data.extend(v.to_le_bytes());
        }
        let marker = Rec::new(b"REFR", 0x4000_0200, rid).field(b"EDID", zs(&edid)).field(b"NAME", (NEW | DISCIPLE_MARKER_ID).to_le_bytes().to_vec()).field(b"DATA", data);
        grids.insert(m.grid, (cell, vec![marker], vec![rid]));
        ids.insert(m.grid, rid);
    }
    let CopiedRefs { cells: copy_cells, mut large } = copies;
    for (grid, refs) in copy_cells {
        let entry = match grids.entry(grid) {
            std::collections::btree_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::btree_map::Entry::Vacant(e) => {
                let Some(cell) = world.cell_records.get(&grid).and_then(|&c| Rec::copy(lo, c)) else { continue };
                e.insert((cell, Vec::new(), Vec::new()))
            }
        };
        entry.1.extend(refs);
        entry.2.extend(large.remove(&grid).unwrap_or_default());
    }
    // RNAM right after the EditorID, one per cell: y, x (i16), count, then (reference, y, x) per
    // reference. The copied ones are left out (the engine merges the RNAM of every master).
    w.fields.retain(|(s, _)| s != b"RNAM");
    w.id_at.clear();
    let at = w.fields.iter().position(|(s, _)| s == b"EDID").map_or(0, |i| i + 1);
    let mut k = 0;
    let mut content = Vec::new();
    for (grid, (cell, temporary, entries)) in grids {
        if !entries.is_empty() {
            let g = [i16le(grid.1), i16le(grid.0)].concat();
            let mut f = g.clone();
            f.extend((entries.len() as u32).to_le_bytes());
            for id in entries {
                f.extend(id.to_le_bytes());
                f.extend(&g);
            }
            w.fields.insert(at + k, (*b"RNAM", f));
            k += 1;
        }
        content.push(CellContent { grid, cell, temporary });
    }
    let persistent_cell = if persistent.is_empty() { None } else { world.persistent_cell.and_then(|(_, pc)| Rec::copy(lo, pc)) };
    esm.worlds.push(WorldContent { world: w, persistent_cell, persistent, cells: content });
    ids
}

/// An override of a persistent Is Full LOD reference without the flag (`_DynDOLOD_REVERT`).
fn revert_record(lo: &LoadOrder, world: &Worldspace, id: u32, edid: &str) -> Option<Rec> {
    let r = world.refs.get(&id)?;
    let mut rec = Rec::copy(lo, lodforge_esp::RecordRef { plugin: r.plugin, offset: r.offset })?;
    if !rec.copyable_ref() || rec.form_id != id {
        return None;
    }
    rec.flags &= !0x10000;
    rec.fields.retain(|(s, _)| s != b"EDID");
    rec.fields.insert(0, (*b"EDID", zs(edid)));
    Some(rec)
}

fn reference_editor_id(plugin: &str, local: u32, world: &str) -> String {
    let tag: String = plugin.to_ascii_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    format!("{tag}&{local:06X}_{world}_DynDOLOD_REFERENCE")
}


/// Seasons of Skyrim section for a base record type.
fn season_section(sig: &[u8; 4]) -> &'static str {
    match sig {
        b"TREE" => "Trees",
        b"MSTT" => "MovableStatics",
        b"ACTI" => "Activators",
        b"FURN" => "Furniture",
        b"FLOR" => "Florae",
        _ => "Statics",
    }
}

/// The texture swaps a base copy showing a model needs (from the AltTextures logic,
/// [`crate::sidefiles::lod_model_swaps`]).
pub type SwapFn<'a> = dyn Fn(&Base, &str) -> Vec<crate::sidefiles::TextureSwap> + Sync + 'a;

/// Texture swaps for base copies and the `<texture>_DynDOLOD_TXST` records made so far.
struct TexSets<'a> {
    swaps: Option<&'a SwapFn<'a>>,
    /// Lower-case diffuse -> TXST form id.
    ids: HashMap<String, u32>,
}

/// `textures\pbr\x\ylinear.dds` -> `ylinear` -> EditorID `y_DynDOLOD_TXST` (DynDOLOD names the
/// texture set after the diffuse without `linear`).
fn txst_editor_id(diffuse: &str) -> String {
    let file = diffuse.rsplit('\\').next().unwrap_or(diffuse);
    let stem = file.strip_suffix(".dds").unwrap_or(file);
    format!("{}_DynDOLOD_TXST", stem.strip_suffix("linear").unwrap_or(stem))
}

/// The TXST for a swap (shared by diffuse): TX00/TX01 diffuse and normal without `textures\`,
/// TX02-TX07 empty, as DynDOLOD's `CreatePBRBaseRecord` writes it.
fn texture_set(esm: &mut PluginBuilder, tex: &mut TexSets<'_>, s: &crate::sidefiles::TextureSwap) -> u32 {
    let key = s.diffuse.to_ascii_lowercase();
    if let Some(&id) = tex.ids.get(&key) {
        return id;
    }
    let edid = txst_editor_id(&key);
    let id = esm.keyed_id(&record_key(b"TXST", &edid, ""));
    let rel = |t: &str| zs(t.strip_prefix("textures\\").unwrap_or(t));
    let mut rec = Rec::new(b"TXST", 0, id).field(b"EDID", zs(&edid)).field(b"OBND", vec![0u8; 12]).field(b"TX00", rel(&s.diffuse)).field(b"TX01", rel(&s.normal));
    for f in [b"TX02", b"TX03", b"TX04", b"TX05", b"TX06", b"TX07"] {
        rec = rec.field(f, vec![0u8]);
    }
    esm.add(rec);
    tex.ids.insert(key, id);
    id
}

/// MODS of a base copy: count, then per swap the 3D name (length-prefixed), the TXST and the
/// shape index; the TXST offsets for the form-id fixup.
fn mods_field(entries: &[(String, u32, i32)]) -> (Vec<u8>, Vec<usize>) {
    let mut d = (entries.len() as u32).to_le_bytes().to_vec();
    let mut ids = Vec::new();
    for (name, txst, index) in entries {
        d.extend((name.len() as u32).to_le_bytes());
        d.extend(name.as_bytes());
        ids.push(d.len());
        d.extend(txst.to_le_bytes());
        d.extend(index.to_le_bytes());
    }
    (d, ids)
}

/// The DynDOLOD.esm copy of `base` showing `model` (`<EDID>_DynDOLOD_BASERECORD`, same record
/// type, bounds and type data), shared by all worldspaces and seasons. Shapes of the model whose
/// textures the AltTextures would swap get MODS alternate textures (PBR load orders).
fn base_copy(esm: &mut PluginBuilder, copies: &mut HashMap<(u32, String), u32>, tex: &mut TexSets<'_>, b: &Base, model: &str) -> u32 {
    let key = (b.form_id, model.to_ascii_lowercase());
    if let Some(&id) = copies.get(&key) {
        return id;
    }
    let swaps = tex.swaps.map(|f| f(b, model)).unwrap_or_default();
    let mods: Vec<(String, u32, i32)> = swaps.iter().map(|s| (s.shape.clone(), texture_set(esm, tex, s), s.index)).collect();
    let model = model.strip_prefix("Meshes\\").or_else(|| model.strip_prefix("meshes\\")).unwrap_or(model);
    let edid = format!("{}_DynDOLOD_BASERECORD", b.editor_id);
    let id = esm.keyed_id(&record_key(&b.signature, &edid, model));
    let obnd: Vec<u8> = b.bounds.iter().flat_map(|v| v.to_le_bytes()).collect();
    // Has Distant LOD (0x8000) and 0x10000 / 0x800 carry over from the original base.
    let mut rec = Rec::new(&b.signature, b.flags & 0x0001_8800, id).field(b"EDID", zs(&edid)).field(b"OBND", obnd).field(b"MODL", zs(model));
    if !mods.is_empty() {
        let (d, offsets) = mods_field(&mods);
        rec = rec.field_ids(b"MODS", d, &offsets);
    }
    rec = match &b.signature {
        b"STAT" => rec.field(b"DNAM", vec![0u8; 12]),
        b"TREE" if !b.type_data.is_empty() => rec.field(b"CNAM", b.type_data.clone()),
        b"MSTT" => rec.field(b"DATA", if b.type_data.is_empty() { vec![0] } else { b.type_data.clone() }),
        // As in the reference DynDOLOD.esm: marker colour and flags zeroed, and the water type of
        // water activators kept (Water1024Creek_DynDOLOD_BASERECORD). Without WNAM the engine
        // draws the water LOD mesh black.
        b"ACTI" => {
            let rec = rec.field(b"PNAM", vec![0u8; 4]);
            let rec = if b.water_type != 0 { rec.field(b"WNAM", b.water_type.to_le_bytes().to_vec()) } else { rec };
            rec.field(b"FNAM", vec![0u8; 2])
        }
        _ => rec,
    };
    esm.add(rec);
    copies.insert(key, id);
    id
}

/// Plugins and data files for several worldspaces.
pub struct MultiOutput {
    pub esm: Vec<u8>,
    pub esp: Vec<u8>,
    /// `DynDOLOD_NG_Worlds.txt`.
    pub worlds_txt: String,
    /// `(EditorID, DynDOLOD_NG_<EditorID>.txt)` per worldspace.
    pub world_txts: Vec<(String, String)>,
    /// `(SPR, Seasons\DynDOLOD_SPR.ini)` per season with swaps.
    pub seasons: Vec<(String, String)>,
}

/// Earlier builds of DynDOLOD.esm and DynDOLOD.esp for rolling updates: their records keep their
/// form ids and new ones continue from their next object ids.
#[derive(Default, Clone, Copy)]
pub struct Rolling<'a> {
    pub esm: Option<&'a Previous>,
    pub esp: Option<&'a Previous>,
}

/// Builds both plugins and the data files for one worldspace.
#[allow(clippy::too_many_arguments)]
pub fn build(lo: &LoadOrder, world: &Worldspace, bases: &HashMap<u32, Base>, dynamic: &[DynamicRef], south_west: (i32, i32), near_grid: u32, far_grid: u32, token: &str, rolling: &Rolling<'_>) -> Result<Output, String> {
    let mut m = build_worlds(lo, &[WorldDynamic { world, dynamic, south_west, lod_refs: &[], children: Vec::new(), underside: None, seasonal: Vec::new(), child_copies: Vec::new() }], bases, near_grid, far_grid, token, rolling, None, None)?;
    let world_txt = m.world_txts.pop().map(|w| w.1).unwrap_or_default();
    Ok(Output { esm: m.esm, esp: m.esp, worlds_txt: m.worlds_txt, world_txt })
}

/// Builds both plugins and the data files for every worldspace in `worlds` (one DynDOLOD.esm,
/// one DynDOLOD.esp, one data file per worldspace).
#[allow(clippy::too_many_arguments)]
pub fn build_worlds(lo: &LoadOrder, worlds: &[WorldDynamic<'_>], bases: &HashMap<u32, Base>, near_grid: u32, far_grid: u32, token: &str, rolling: &Rolling<'_>, workarounds: Option<Workarounds>, swaps: Option<&SwapFn<'_>>) -> Result<MultiOutput, String> {
    // DynDOLOD.esm: the keywords and the large-reference workarounds.
    let mut esm = PluginBuilder::new(FIRST_OBJECT_ID);
    esm.esm = true;
    esm.author = "LODForge".into();
    esm.description = description("DynDOLOD.esm", "Master to work around game breaking bugs.");
    esm.add(Rec::new(b"KYWD", 0, NEW | KEYWORD_ID).field(b"EDID", zs(token)).field(b"CNAM", vec![0u8; 4]));
    esm.add(Rec::new(b"KYWD", 0, NEW | VERSION_KEYWORD_ID).field(b"EDID", zs(VERSION)).field(b"CNAM", vec![0u8; 4]));
    if let Some(p) = rolling.esm {
        esm.resume(p);
    }
    if workarounds.is_some() {
        // The marker base: an Is Marker STAT with MarkerX.nif, as DynDOLOD writes it.
        esm.add(
            Rec::new(b"STAT", 0x0080_0000, NEW | DISCIPLE_MARKER_ID)
                .field(b"EDID", zs("DynDOLOD_DiscipleMarker"))
                .field(b"OBND", vec![0xEE, 0xFF, 0xEC, 0xFF, 0x00, 0x00, 0x12, 0x00, 0x12, 0x00, 0x10, 0x00])
                .field(b"MODL", zs(MARKER_MODEL))
                .field(b"DNAM", vec![0x00, 0x00, 0xB4, 0x42, 0, 0, 0, 0, 0, 0, 0, 0]),
        );
    }
    // DynDOLOD.esp: keywords, framework, base copies, then the worldspaces.
    let mut esp = PluginBuilder::new(FIRST_OBJECT_ID);
    if let Some(p) = rolling.esp {
        esp.resume(p);
    }
    esp.companion = Some("DynDOLOD.esm".into());
    esp.author = "LODForge".into();
    esp.description = description("DynDOLOD.esp", "Plugin to work around game breaking bugs.");
    esp.add(Rec::new(b"KYWD", 0, NEW | KEYWORD_ID).field(b"EDID", zs(token)).field(b"CNAM", vec![0u8; 4]));
    framework(&mut esp);
    let master_base = NEW | MASTER_ACTIVATOR_ID;
    let owners = crate::bases::owners(lo);
    let plugin_name = |id: u32| -> (u32, String) {
        let name = owners.get(&crate::bases::owner_key(id)).map(|&i| lo.plugins[i].plugin.name.clone()).unwrap_or_default();
        let local = if id >> 24 == 0xFE { id & 0xFFF } else { id & 0x00FF_FFFF };
        (local, name)
    };
    let mut replacement_ids = HashMap::new();
    for wd in worlds {
        for d in wd.dynamic.iter().filter(|d| d.replace) {
            let (local, plugin) = plugin_name(d.ref_id);
            let edid = reference_editor_id(&plugin, local, &wd.world.editor_id);
            let id = esp.keyed_id(&record_key(b"REFR", &edid, ""));
            if replacement_ids.insert(d.ref_id, id).is_some() {
                return Err(format!("replacement {:08X}: selected in more than one world", d.ref_id));
            }
        }
    }
    // Base copies per (base, model) in DynDOLOD.esm, shared by all worldspaces and seasons.
    let mut copies: HashMap<(u32, String), u32> = HashMap::new();
    let mut tex = TexSets { swaps, ids: HashMap::new() };
    // Season -> section -> (default copy, seasonal copy) swap lines.
    let mut swaps: std::collections::BTreeMap<&'static str, std::collections::BTreeMap<&'static str, Vec<(u32, u32, String)>>> = std::collections::BTreeMap::new();
    let mut world_txts = Vec::new();
    let mut child_reverts = std::collections::BTreeMap::<(bool, u32), (&Worldspace, Vec<Rec>)>::new();
    for wd in worlds {
        let (world, dynamic, south_west) = (wd.world, wd.dynamic, wd.south_west);
        let mut persistent = Vec::new();
        let master_edid = format!("{}_ShesonObject", world.editor_id);
        let master_ref = esp.keyed_id(&record_key(b"REFR", &master_edid, ""));
        persistent.push(Rec::new(b"REFR", 0x4001_0400, master_ref).field(b"EDID", zs(&master_edid)).field(b"NAME", master_base.to_le_bytes().to_vec()).field(b"DATA", vec![0u8; 24]));
        // Terrain underside (golden): DynDOLOD.esm MSTT `<W>_UNDERSIDE_DynDOLOD_BASERECORD` with the
        // mesh; DynDOLOD.esp FLST `<W>_UNDERSIDE_DynDOLOD_LIST` (the worldspace and its child
        // worldspaces) and a persistent, initially disabled, Is Full LOD REFR
        // `<W>_UNDERSIDE_DynDOLOD_NOLOD` at (0, 0, -height) whose ObjectEnabler script enables it
        // in those worldspaces.
        if let Some((height, list_worlds)) = &wd.underside {
            let w = &world.editor_id;
            let (base_edid, base_model) = (format!("{w}_UNDERSIDE_DynDOLOD_BASERECORD"), format!("Terrain\\{w}\\{w}_Underside.nif"));
            let base = esm.keyed_id(&record_key(b"MSTT", &base_edid, &base_model));
            let obnd: Vec<u8> = [-4096i16, -4096, -4096, 4096, 4096, 4096].iter().flat_map(|v| v.to_le_bytes()).collect();
            esm.add(Rec::new(b"MSTT", 0x4, base).field(b"EDID", zs(&base_edid)).field(b"OBND", obnd).field(b"MODL", zs(&base_model)).field(b"DATA", vec![0]));
            let list_edid = format!("{w}_UNDERSIDE_DynDOLOD_LIST");
            let list = esp.keyed_id(&record_key(b"FLST", &list_edid, ""));
            let mut flst = Rec::new(b"FLST", 0, list).field(b"EDID", zs(&list_edid));
            for id in list_worlds {
                flst = flst.field(b"LNAM", id.to_le_bytes().to_vec());
            }
            esp.add(flst);
            let mut vmad = Vmad::default();
            vmad.scripts(&[("SHESON_DynDOLOD_ObjectEnabler", &[("MyWorldspaces", Prop::Object(list))])]);
            let mut data = vec![0u8; 24];
            data[8..12].copy_from_slice(&(-height).to_le_bytes());
            let nolod_edid = format!("{w}_UNDERSIDE_DynDOLOD_NOLOD");
            let nolod = esp.keyed_id(&record_key(b"REFR", &nolod_edid, ""));
            persistent.push(
                Rec::new(b"REFR", 0x0001_0C00, nolod)
                    .field(b"EDID", zs(&nolod_edid))
                    .field_ids(b"VMAD", vmad.bytes, &vmad.ids)
                    .field(b"NAME", (COMPANION | (base & 0x00FF_FFFF)).to_le_bytes().to_vec())
                    .field(b"DATA", data),
            );
        }
        let mut objects = String::new();
        let mut disabled = String::new();
        let mut copied = CopiedRefs::default();
        let mut esm_persistent = Vec::new();
        for d in dynamic {
            let b = bases.get(&d.base).ok_or_else(|| format!("dynamic reference {:08X}: missing base {:08X}", d.ref_id, d.base))?;
            let r = world.refs.get(&d.ref_id).ok_or_else(|| format!("dynamic reference {:08X}: missing placement", d.ref_id))?;
            let base_id = base_copy(&mut esm, &mut copies, &mut tex, b, &d.model);
            for (code, map) in &wd.seasonal {
                let Some((b2, m2)) = map.get(&d.ref_id) else { continue };
                if *b2 == d.base && m2.eq_ignore_ascii_case(&d.model) {
                    continue;
                }
                let Some(sb) = bases.get(b2) else { continue };
                let id2 = base_copy(&mut esm, &mut copies, &mut tex, sb, m2);
                let list = swaps.entry(code).or_default().entry(season_section(&b.signature)).or_default();
                if !list.iter().any(|(a, c, _)| *a == base_id && *c == id2) {
                    list.push((base_id, id2, format!("{}_DynDOLOD_BASERECORD|{}_DynDOLOD_BASERECORD", b.editor_id, sb.editor_id)));
                }
            }
            let base_id = COMPANION | (base_id & 0x00FF_FFFF);
            let mut data = Vec::with_capacity(24);
            for v in r.position.iter().chain(r.rotation.iter()) {
                data.extend_from_slice(&v.to_le_bytes());
            }
            // Persistent, initially disabled, Is Full LOD (and 0x40000000, as DynDOLOD writes them);
            // EditorID `<plugin without punctuation>&<object id>_<World>_DynDOLOD_REFERENCE`.
            let (ol, op) = plugin_name(d.ref_id);
            let edid = reference_editor_id(&op, ol, &world.editor_id);
            let rid = replacement_ids.get(&d.ref_id).copied().unwrap_or_else(|| esp.keyed_id(&record_key(b"REFR", &edid, "")));
            let mut rec = Rec::new(b"REFR", 0x4001_0C00, rid).field(b"EDID", zs(&edid)).field(b"NAME", base_id.to_le_bytes().to_vec());
            if r.has_scale && r.scale != 1.0 {
                rec = rec.field(b"XSCL", r.scale.to_le_bytes().to_vec());
            }
            if d.replace {
                if world.world_record.and_then(|at| Rec::copy(lo, at)).is_none()
                    || world.persistent_cell.and_then(|(_, at)| Rec::copy(lo, at)).is_none()
                {
                    return Err(format!("replacement {:08X}: missing world or persistent cell record", d.ref_id));
                }
                // Keep the original identity for saves/quests, but remove its geometry and parent.
                // The DLL also disables existing saved instances through [Disabled].
                let mut hidden = data.clone();
                hidden[8..12].copy_from_slice(&(-30000.0f32).to_le_bytes());
                let replaced_edid = format!("dyndolodesp&{:06X}_{edid}_DynDOLOD_REPLACED", rid & 0x00FF_FFFF);
                let mut original = Rec::new(b"REFR", r.flags & lodforge_esp::record::FLAG_PERSISTENT, d.ref_id)
                    .field(b"EDID", zs(&replaced_edid))
                    .field(b"NAME", 0x3Bu32.to_le_bytes().to_vec());
                if r.has_scale && r.scale != 1.0 {
                    original = original.field(b"XSCL", r.scale.to_le_bytes().to_vec());
                }
                original = original.field(b"DATA", hidden);
                if r.is_persistent() {
                    esm_persistent.push(original);
                } else {
                    let grid = r.cell.ok_or_else(|| format!("replacement {:08X}: missing source cell", d.ref_id))?;
                    if world.cell_records.get(&grid).and_then(|&at| Rec::copy(lo, at)).is_none() {
                        return Err(format!("replacement {:08X}: missing source cell record {grid:?}", d.ref_id));
                    }
                    copied.cells.entry(grid).or_default().push(original);
                }
                disabled.push_str(&format!("{ol},{op}\r\n"));
            }
            persistent.push(rec.field(b"DATA", data));
            // [Objects], 17 comma-separated fields as DynDOLOD.DLL's LoadData reads them:
            // 1-2 dynamic reference, 3 Level (0 NearGrid, 1 FarGrid), 4 Delay, 5 InitiallyEnabled,
            // 6 SwitchParentWorld, 7 OppositeParent, 8-9 enable parent (the original itself when it
            // has none), 10-11 destruction stage index/type, 12 unused, 13 SwitchChildWorld,
            // 14 Names3D (`;`-separated), 15-16 original, 17 LargeReference. Booleans are "1" or
            // empty. Door/Fade come from the base type and Persistent from the parent record.
            // `DowngradeNormalReferences=1`: only large references stay on FarGrid; the others are
            // downgraded to NearGrid and marked Delay (E2E golden: FarGrid = the 363 large refs).
            let rule_far = matches!(d.grid, crate::rules::Grid::FarLod | crate::rules::Grid::FarFull);
            let far = rule_far && d.large;
            let near_tree = b.signature == *b"TREE" && d.grid == crate::rules::Grid::NearLod;
            let delay = d.replace || (rule_far && !d.large) || (d.persistent && !near_tree);
            let (pl, pp) = if let Some(parent) = replacement_ids.get(&d.enable_parent) {
                (parent & 0x00FF_FFFF, "DynDOLOD.esp".into())
            } else if d.enable_parent != 0 {
                plugin_name(d.enable_parent)
            } else if d.replace {
                (rid & 0x00FF_FFFF, "DynDOLOD.esp".into())
            } else {
                (ol, op.clone())
            };
            let flag = |b: bool| if b { "1" } else { "" };
            objects.push_str(&format!(
                "{},DynDOLOD.esp,{},{},{},{},{},{pl},{pp},-1,-1,,{},,{ol},{op},{}\r\n",
                rid & 0x00FF_FFFF,
                u8::from(far),
                flag(delay),
                flag(d.initially_enabled),
                flag(!d.replace),
                flag(d.enable_parent != 0 && d.opposite_parent),
                flag(!d.replace),
                flag(d.large),
            ));
        }
        // Child worldspace copies (golden: DynDOLOD.esm REFR `<plugin>&<id>_DynDOLOD_CHILD`, a
        // temporary reference in the parent's exterior cell at its position, of a base copy that
        // shows the LOD model; no script, Visible When Distant kept). They go into the same
        // DynDOLOD.esm WRLD override as the markers.
        for (cp, model) in &wd.child_copies {
            let p = &cp.placement;
            let Some(b) = bases.get(&p.base) else { continue };
            let grid = ((p.position[0] / 4096.0).floor() as i32, (p.position[1] / 4096.0).floor() as i32);
            if !world.cell_records.contains_key(&grid) {
                continue;
            }
            // `childworldfull.txt` references keep their own base (the full model).
            let base_id = if cp.full_model { p.base } else { base_copy(&mut esm, &mut copies, &mut tex, b, model) };
            let (ol, op) = plugin_name(cp.ref_id);
            let tag: String = op.to_ascii_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect();
            let mut data = Vec::with_capacity(24);
            for v in p.position.iter().chain(p.rotation.iter()) {
                data.extend_from_slice(&v.to_le_bytes());
            }
            // Visible When Distant on everything but upright trees (golden: STAT copies and
            // tilted trees), unless the base has 0x10000.
            let vwd = (!crate::assign::is_tree(b) || crate::export::tree_tilted(p.rotation, 30.0)) && b.flags & 0x0001_0000 == 0;
            let edid = format!("{tag}&{ol:06X}_DynDOLOD_CHILD");
            let id = esm.keyed_id(&record_key(b"REFR", &edid, ""));
            let mut rec = Rec::new(b"REFR", if vwd { 0x8000 } else { 0 }, id).field(b"EDID", zs(&edid)).field(b"NAME", base_id.to_le_bytes().to_vec());
            if p.has_scale && p.scale != 1.0 {
                rec = rec.field(b"XSCL", p.scale.to_le_bytes().to_vec());
            }
            if cp.large {
                copied.large.entry(grid).or_default().push(id);
            }
            copied.cells.entry(grid).or_default().push(rec.field(b"DATA", data));
        }
        let mut copied = Some(copied);
        // Large-reference workarounds: Is Full LOD reverts ([Resets]) and the markers ([Markers]).
        let mut resets = String::new();
        let mut marker_lines: Vec<String> = Vec::new();
        if let Some(wa) = workarounds {
            let mut lod_refs: Vec<u32> = wd.lod_refs.to_vec();
            lod_refs.extend(dynamic.iter().filter(|d| !d.replace).map(|d| d.ref_id));
            for id in crate::largerefs::revert_refs(world, &lod_refs) {
                let (ol, op) = plugin_name(id);
                let tag: String = op.to_ascii_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect();
                let Some(rec) = revert_record(lo, world, id, &format!("{tag}&{ol:06X}_DynDOLOD_REVERT")) else { continue };
                if world.large_ref_cells.contains_key(&id) {
                    esm_persistent.push(rec);
                } else {
                    persistent.push(rec);
                }
                resets.push_str(&format!("{ol},{op}\r\n"));
            }
            // `world,plugin,x,y,marker,DynDOLOD.esm,count[,plugin;id]*`, one line per marker of
            // this worldspace and of its child worldspaces with their own LOD.
            let mut line = |w: &Worldspace, cells: &[crate::largerefs::MarkerCell], ids: &HashMap<(i32, i32), u32>| {
                let (wl, wp) = plugin_name(w.form_id);
                for m in cells {
                    let Some(&mid) = ids.get(&m.grid) else { continue };
                    let mut s = format!("{wl},{wp},{},{},{},DynDOLOD.esm,{}", m.grid.0, m.grid.1, mid & 0x00FF_FFFF, m.count);
                    for &p in &m.persistent {
                        let (pl, pp) = plugin_name(p);
                        s.push_str(&format!(",{pp};{pl}"));
                    }
                    marker_lines.push(s);
                }
            };
            let cells = crate::largerefs::marker_cells(lo, world, bases);
            let ids = esm_world(lo, &mut esm, world, None, &cells, std::mem::take(&mut esm_persistent), copied.take().unwrap_or_default());
            line(world, &cells, &ids);
            for ch in &wd.children {
                for id in crate::largerefs::revert_refs(ch.world, &ch.revert_refs) {
                    if replacement_ids.contains_key(&id) {
                        continue;
                    }
                    let (ol, op) = plugin_name(id);
                    let tag: String = op.to_ascii_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect();
                    let rec = revert_record(lo, ch.world, id, &format!("{tag}&{ol:06X}_DynDOLOD_REVERT"))
                        .ok_or_else(|| format!("child-world reset {id:08X}: cannot copy original reference"))?;
                    let large = ch.world.large_ref_cells.contains_key(&id);
                    child_reverts.entry((large, ch.world.form_id)).or_insert_with(|| (ch.world, Vec::new())).1.push(rec);
                    resets.push_str(&format!("{ol},{op}\r\n"));
                }
                if ch.bounds.is_none() {
                    continue;
                }
                if ch.world.parent_flags & 0x02 == 0 {
                    let cells = crate::largerefs::marker_cells(lo, ch.world, bases);
                    let ids = esm_world(lo, &mut esm, ch.world, Some(world), &cells, Vec::new(), CopiedRefs::default());
                    line(ch.world, &cells, &ids);
                } else if let Some(b) = ch.bounds {
                    // Children on the parent's LOD: markers the DLL is not told about (they make
                    // the cells exist in DynDOLOD.esm, as DynDOLOD's do).
                    let cells = crate::largerefs::child_marker_cells(ch.world, b, wa.margin, ch.no_navmesh_cells);
                    esm_world(lo, &mut esm, ch.world, Some(world), &cells, Vec::new(), CopiedRefs::default());
                }
            }
            marker_lines.sort();
        }
        if let (Some(wr), Some((_, pc))) = (world.world_record, world.persistent_cell)
            && let (Some(w), Some(c)) = (Rec::copy(lo, wr), Rec::copy(lo, pc))
        {
            esp.worlds.push(WorldContent { world: w, persistent_cell: Some(c), persistent, cells: Vec::new() });
        }
        // Without the workarounds the copies still need their WRLD override.
        if let Some(c) = copied.take()
            && (!c.cells.is_empty() || !esm_persistent.is_empty())
        {
            esm_world(lo, &mut esm, world, None, &[], esm_persistent, c);
        }
        let markers: String = marker_lines.iter().map(|l| format!("{l}\r\n")).collect();
        let world_txt = format!(
            "[Config]\r\nbunchofnumbers={token}\r\nmaster={},DynDOLOD.esp\r\nx={}\r\ny={}\r\n[Objects]\r\n{objects}[Markers]\r\n{markers}[Disabled]\r\n{disabled}[Resets]\r\n{resets}",
            master_ref & 0x00FF_FFFF,
            south_west.0,
            south_west.1
        );
        world_txts.push((world.editor_id.clone(), world_txt));
    }
    for ((large, _), (world, records)) in child_reverts {
        let target = if large { &mut esm } else { &mut esp };
        let cell = world.persistent_cell.and_then(|(_, at)| Rec::copy(lo, at))
            .ok_or_else(|| format!("child-world reset {}: missing persistent cell record", world.editor_id))?;
        let content = if let Some(index) = target.worlds.iter().position(|w| w.world.form_id == world.form_id) {
            &mut target.worlds[index]
        } else {
            let wr = world.world_record.and_then(|at| Rec::copy(lo, at))
                .ok_or_else(|| format!("child-world reset {}: missing world record", world.editor_id))?;
            target.worlds.push(WorldContent { world: wr, persistent_cell: None, persistent: Vec::new(), cells: Vec::new() });
            target.worlds.last_mut().unwrap()
        };
        content.persistent_cell.get_or_insert(cell);
        for rec in records {
            if !content.persistent.iter().any(|r| r.form_id == rec.form_id) {
                content.persistent.push(rec);
            }
        }
    }
    let names: String = worlds.iter().map(|w| format!("{}\r\n", w.world.editor_id.to_ascii_lowercase())).collect();
    let worlds_txt = format!(
        "[Config]\r\nbunchofnumbers={token}\r\nesmplugin=dyndolod.esm\r\nespplugin=dyndolod.esp\r\nglowmultiple=1.75\r\nneargrid={near_grid}\r\nfargrid={far_grid}\r\ndebug=false\r\n[Worlds]\r\n{names}"
    );
    // Seasons of Skyrim: `;<default EDID>|<seasonal EDID>` then `0x<id>~DynDOLOD.esm|0x<id>~DynDOLOD.esm`.
    let seasons = swaps
        .into_iter()
        .map(|(code, sections)| {
            let mut text = String::new();
            for (section, lines) in sections {
                text.push_str(&format!("[{section}]\r\n"));
                for (a, c, comment) in lines {
                    text.push_str(&format!(";{comment}\r\n0x{:X}~DynDOLOD.esm|0x{:X}~DynDOLOD.esm\r\n", a & 0x00FF_FFFF, c & 0x00FF_FFFF));
                }
            }
            (code.to_string(), text)
        })
        .collect();
    Ok(MultiOutput { esm: esm.write(lo), esp: esp.write(lo), worlds_txt, world_txts, seasons })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::Previous;

    #[test]
    fn texture_sets_for_base_copies() {
        assert_eq!(txst_editor_id("textures\\pbr\\landscape\\mountains\\mountainslab01linear.dds"), "mountainslab01_DynDOLOD_TXST");
        assert_eq!(txst_editor_id("textures\\lod\\mrkbuildingspbr_lod03.dds"), "mrkbuildingspbr_lod03_DynDOLOD_TXST");
        // As in the reference DynDOLOD.esm (RockCairnEvil01_DynDOLOD_BASERECORD), TXST 0x0D013DCD.
        let (d, ids) = mods_field(&[("RockCairnEvil01:1 - L1_Rocks04:1".into(), 0x0D01_3DCD, 0)]);
        assert_eq!(d.len(), 48);
        assert_eq!(&d[..8], &[1, 0, 0, 0, 0x20, 0, 0, 0]);
        assert_eq!(ids, vec![40]);
        assert_eq!(&d[40..], &[0xCD, 0x3D, 0x01, 0x0D, 0, 0, 0, 0]);
    }

    /// A load order of one generated `Test.esm`: STAT 0x800, worldspace 0x801 with persistent
    /// cell 0x802 and references 0x810.. at x = 1000 * n.
    fn test_load_order(dir: &std::path::Path, refs: u32) -> LoadOrder {
        let mut pb = PluginBuilder::new(0x900);
        pb.esm = true;
        pb.author = "test".into();
        pb.add(Rec::new(b"STAT", 0, NEW | 0x800).field(b"EDID", zs("Rock")).field(b"OBND", vec![0u8; 12]).field(b"MODL", zs("rock.nif")));
        let world = Rec::new(b"WRLD", 0, NEW | 0x801).field(b"EDID", zs("TestWorld"));
        let cell = Rec::new(b"CELL", 0, NEW | 0x802).field(b"DATA", vec![2, 0]);
        let persistent = (0..refs)
            .map(|n| {
                let mut data = Vec::new();
                for v in [1000.0f32 * n as f32, 0.0, 0.0, 0.0, 0.0, 0.0] {
                    data.extend(v.to_le_bytes());
                }
                Rec::new(b"REFR", 0x400, NEW | (0x810 + n)).field(b"NAME", (NEW | 0x800).to_le_bytes().to_vec()).field(b"DATA", data)
            })
            .collect();
        pb.worlds.push(WorldContent { world, persistent_cell: Some(cell), persistent, cells: Vec::new() });
        let empty = LoadOrder::open(&[], |_| None).unwrap();
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("Test.esm"), pb.write(&empty)).unwrap();
        LoadOrder::open(&["Test.esm".to_string()], |n| Some(dir.join(n))).unwrap()
    }

    /// Subrecords of the first `sig` record in an uncompressed plugin, as (type, data).
    fn first_record_fields(bytes: &[u8], sig: &[u8; 4], edid: &str) -> Vec<([u8; 4], Vec<u8>)> {
        let mut i = 0;
        while i + 24 <= bytes.len() {
            let size = u32::from_le_bytes(bytes[i + 4..i + 8].try_into().unwrap()) as usize;
            if &bytes[i..i + 4] == b"GRUP" {
                i += 24;
                continue;
            }
            let body = &bytes[i + 24..i + 24 + size];
            let mut fields = Vec::new();
            let mut k = 0;
            while k + 6 <= body.len() {
                let len = u16::from_le_bytes([body[k + 4], body[k + 5]]) as usize;
                fields.push((body[k..k + 4].try_into().unwrap(), body[k + 6..k + 6 + len].to_vec()));
                k += 6 + len;
            }
            if &bytes[i..i + 4] == sig && fields.iter().any(|(s, d)| s == b"EDID" && d.starts_with(edid.as_bytes())) {
                return fields;
            }
            i += 24 + size;
        }
        Vec::new()
    }

    #[test]
    fn water_activator_copies_keep_their_water_type() {
        // As in the reference DynDOLOD.esm (Water1024CreekFlowSE_DynDOLOD_BASERECORD): EDID, OBND,
        // MODL, PNAM 0, WNAM = the original's water type, FNAM 0. Without WNAM the water is black.
        let dir = std::env::temp_dir().join(format!("lodforge-water-acti-{}", std::process::id()));
        let lo = test_load_order(&dir, 0);
        let mut esm = PluginBuilder::new(0x800);
        esm.esm = true;
        let mut copies = HashMap::new();
        let mut tex = TexSets { swaps: None, ids: HashMap::new() };
        // Any record of Test.esm stands in for the water type: global 0x00000800.
        let water = Base { signature: *b"ACTI", form_id: 0x900, editor_id: "Water1024Creek".into(), model: "Water\\Water1024.nif".into(), water_type: 0x800, ..Default::default() };
        let plain = Base { signature: *b"ACTI", form_id: 0x901, editor_id: "DragonMound".into(), model: "dragonmound.nif".into(), ..Default::default() };
        base_copy(&mut esm, &mut copies, &mut tex, &water, "dyndolod\\lod\\water\\water1024_dyndolod_lod.nif");
        base_copy(&mut esm, &mut copies, &mut tex, &plain, "lod\\dragonmound_lod_0.nif");
        let bytes = esm.write(&lo);
        let _ = std::fs::remove_dir_all(&dir);

        let f = first_record_fields(&bytes, b"ACTI", "Water1024Creek_DynDOLOD_BASERECORD");
        let sigs: Vec<&[u8; 4]> = f.iter().map(|(s, _)| s).collect();
        assert_eq!(sigs, [b"EDID", b"OBND", b"MODL", b"PNAM", b"WNAM", b"FNAM"]);
        let get = |s: &[u8; 4]| f.iter().find(|(k, _)| k == s).unwrap().1.clone();
        assert_eq!(get(b"PNAM"), vec![0; 4]);
        assert_eq!(get(b"FNAM"), vec![0; 2]);
        // Test.esm is master 0 of the written plugin.
        assert_eq!(get(b"WNAM"), 0x0000_0800u32.to_le_bytes().to_vec());

        let f = first_record_fields(&bytes, b"ACTI", "DragonMound_DynDOLOD_BASERECORD");
        let sigs: Vec<&[u8; 4]> = f.iter().map(|(s, _)| s).collect();
        assert_eq!(sigs, [b"EDID", b"OBND", b"MODL", b"PNAM", b"FNAM"]);
    }

    fn dynamic(id: u32, model: &str) -> DynamicRef {
        DynamicRef { ref_id: id, base: 0x800, grid: crate::rules::Grid::NearLod, initially_enabled: true, enable_parent: 0, opposite_parent: false, large: false, persistent: false, replace: false, model: model.into(), columns: Default::default() }
    }

    #[test]
    fn near_tree_delay_does_not_follow_original_persistence() {
        use crate::rules::Grid;
        let dir = std::env::temp_dir().join(format!("lodforge-tree-delay-{}", std::process::id()));
        let lo = test_load_order(&dir, 1);
        let worlds = lodforge_esp::scan_worldspaces(&lo).unwrap();
        let world = &worlds[0];
        let mut bases = crate::bases::read_bases(&lo).unwrap();
        for (sig, grid, persistent, large, replace, expected) in [
            (*b"TREE", Grid::NearLod, true, false, false, false),
            (*b"TREE", Grid::NearLod, false, false, false, false),
            (*b"TREE", Grid::NearLod, true, true, false, false),
            (*b"TREE", Grid::NearLod, true, false, true, true),
            (*b"TREE", Grid::NearFull, true, false, false, true),
            (*b"TREE", Grid::FarLod, true, false, false, true),
            (*b"TREE", Grid::FarLod, true, true, false, true),
            (*b"STAT", Grid::NearLod, true, false, false, true),
            (*b"ACTI", Grid::NearLod, true, false, false, true),
            (*b"MSTT", Grid::FarFull, false, true, false, false),
        ] {
            bases.get_mut(&0x800).unwrap().signature = sig;
            let mut d = dynamic(0x810, "rock.nif");
            d.grid = grid;
            d.persistent = persistent;
            d.large = large;
            d.replace = replace;
            let out = build(&lo, world, &bases, &[d], (0, 0), 11, 21, "TOKEN", &Rolling::default()).unwrap();
            let row = out.world_txt.split("[Objects]\r\n").nth(1).unwrap().lines().next().unwrap();
            assert_eq!(row.split(',').nth(3).unwrap() == "1", expected, "{sig:?} {grid:?} {persistent} {large} {replace}");
        }
        drop(lo);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn matched_child_full_lod_reverts_stay_in_the_source_world() {
        let dir = std::env::temp_dir().join(format!("lodforge-child-resets-{}", std::process::id()));
        let original = test_load_order(&dir, 2);
        let mut source = PluginBuilder::new(0xa00);
        source.esm = true;
        let data: Vec<u8> = [100.0f32, 200.0, 300.0, 0.1, 0.2, 0.3].iter().flat_map(|v| v.to_le_bytes()).collect();
        source.worlds.push(WorldContent {
            world: Rec::new(b"WRLD", 0, NEW | 0x901).field(b"EDID", zs("ChildWorld")).field(b"WNAM", 0x801u32.to_le_bytes().to_vec()),
            persistent_cell: Some(Rec::new(b"CELL", 0, NEW | 0x902).field(b"DATA", vec![2, 0])),
            persistent: [0x920, 0x921, 0x922].into_iter().map(|id| {
                Rec::new(b"REFR", if id == 0x922 { 0x400 } else { 0x10400 }, NEW | id)
                    .field(b"NAME", 0x800u32.to_le_bytes().to_vec()).field(b"XSCL", 1.5f32.to_le_bytes().to_vec())
                    .field(b"DATA", data.clone())
            }).collect(),
            cells: Vec::new(),
        });
        std::fs::write(dir.join("Child.esm"), source.write(&original)).unwrap();
        let lo = LoadOrder::open(&["Test.esm".into(), "Child.esm".into()], |n| Some(dir.join(n))).unwrap();
        let worlds = lodforge_esp::scan_worldspaces(&lo).unwrap();
        let parent = worlds.iter().find(|w| w.editor_id == "TestWorld").unwrap();
        let child = worlds.iter().find(|w| w.editor_id == "ChildWorld").unwrap();
        let matches = HashMap::from([
            (("child.esm".into(), 0x920), vec![("test.esm".into(), 0x810), ("test.esm".into(), 0x811)]),
            (("child.esm".into(), 0x921), vec![("test.esm".into(), 0x811)]),
            (("child.esm".into(), 0x922), vec![("test.esm".into(), 0x810)]),
        ]);
        let selected = crate::childworld::matched_full_lod_refs(&lo, child, &[0x810, 0x810], &matches, &[]);
        assert_eq!(selected, vec![0x0100_0920]);
        assert!(crate::childworld::matched_full_lod_refs(&lo, child, &[], &matches, &[]).is_empty());
        for action in ["Delete", "Disable", "Replace"] {
            let rule = crate::rules::Rule::parse(&format!("Child.esm;00000920,None,None,None,None,{action},0"), "test").unwrap();
            assert!(crate::childworld::matched_full_lod_refs(&lo, child, &[0x810], &matches, &[rule]).is_empty());
        }
        let mut temporary = child.clone();
        temporary.refs.get_mut(&0x0100_0920).unwrap().flags &= !0x400;
        assert!(crate::childworld::matched_full_lod_refs(&lo, &temporary, &[0x810], &matches, &[]).is_empty());
        let bases = crate::bases::read_bases(&lo).unwrap();
        let dynamic = [dynamic(0x810, "rock_lod.nif")];
        let build = |child: &Worldspace, large: bool, wa: Option<Workarounds>, rolling: &Rolling<'_>| {
            let mut child = child.clone();
            if large {
                child.large_ref_cells.insert(0x0100_0920, (0, 0));
            }
            let inputs = [
                WorldDynamic {
                    world: parent, dynamic: &dynamic, south_west: (0, 0), lod_refs: &[],
                    children: vec![ChildWorld { world: &child, revert_refs: selected.clone(), bounds: None, no_navmesh_cells: false }],
                    underside: None, seasonal: Vec::new(), child_copies: Vec::new(),
                },
                // Also selected independently: the child's existing WRLD must be merged, not duplicated.
                WorldDynamic {
                    world: &child, dynamic: &[], south_west: (0, 0), lod_refs: &[],
                    children: Vec::new(), underside: None, seasonal: Vec::new(), child_copies: Vec::new(),
                },
            ];
            build_worlds(&lo, &inputs, &bases, 11, 21, "TOKEN", rolling, wa, None)
        };
        for large in [false, true] {
            let wa = Some(Workarounds { margin: 3 });
            let out = build(child, large, wa, &Rolling::default()).unwrap();
            assert!(out.world_txts[0].1.ends_with("[Resets]\r\n2336,Child.esm\r\n"));
            std::fs::write(dir.join("DynDOLOD.esm"), &out.esm).unwrap();
            std::fs::write(dir.join("DynDOLOD.esp"), &out.esp).unwrap();
            let generated = LoadOrder::open(&["Test.esm".into(), "Child.esm".into(), "DynDOLOD.esm".into(), "DynDOLOD.esp".into()], |n| Some(dir.join(n))).unwrap();
            let after = lodforge_esp::scan_worldspaces(&generated).unwrap();
            let child_after = after.iter().find(|w| w.editor_id == "ChildWorld").unwrap();
            let reference = &child_after.refs[&0x0100_0920];
            assert_eq!(reference.flags, 0x400);
            assert_eq!(reference.position, [100.0, 200.0, 300.0]);
            assert_eq!(reference.rotation, [0.1, 0.2, 0.3]);
            assert_eq!(reference.scale, 1.5);
            assert_eq!(reference.plugin, if large { 2 } else { 3 });
            assert_eq!(child_after.refs[&0x0100_0921].flags, 0x10400);
            assert!(!after.iter().find(|w| w.editor_id == "TestWorld").unwrap().refs.contains_key(&0x0100_0920));
            let (esm, esp) = (Previous::read(&out.esm).unwrap(), Previous::read(&out.esp).unwrap());
            let again = build(child, large, wa, &Rolling { esm: Some(&esm), esp: Some(&esp) }).unwrap();
            assert_eq!(out.esm, again.esm);
            assert_eq!(out.esp, again.esp);
            let mut missing = child.clone();
            missing.persistent_cell = None;
            assert!(build(&missing, large, wa, &Rolling::default()).err().unwrap().contains("missing persistent cell"));
            drop(generated);
        }
        let off = build(child, false, None, &Rolling::default()).unwrap();
        assert!(off.world_txts[0].1.ends_with("[Resets]\r\n"));
        drop(lo);
        drop(original);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn global_base_updates_use_owners_and_selected_unflagged_bases() {
        let dir = std::env::temp_dir().join(format!("lodforge-base-lod-{}", std::process::id()));
        let original = test_load_order(&dir, 0);
        let mut light = PluginBuilder::new(0xe00);
        light.esl = true;
        light.add(Rec::new(b"STAT", 0, 0x800).field(b"EDID", zs("OverriddenRock")));
        for (id, flags) in [(0xaaa, 0), (0xabc, 0x8000), (0xdef, 0)] {
            light.add(Rec::new(b"STAT", flags, NEW | id).field(b"EDID", zs("LightRock")));
        }
        std::fs::write(dir.join("Light.esl"), light.write(&original)).unwrap();
        let lo = LoadOrder::open(&["Test.esm".into(), "Light.esl".into()], |n| Some(dir.join(n))).unwrap();
        let mut bases = crate::bases::read_bases(&lo).unwrap();
        assert_eq!(bases[&0x800].winning_plugin, 1);
        let ids = [0xfe00_0def, 0x800, 0xfe00_0abc, 0x800];
        let expected = "[Bases]\r\n2048,Test.esm\r\n3567,Light.esl\r\n";
        assert_eq!(base_lod_section(&lo, &bases, ids).unwrap(), expected);
        assert_eq!(base_lod_section(&lo, &bases, ids.into_iter().rev()).unwrap(), expected);
        assert_eq!(base_lod_section(&lo, &bases, []).unwrap(), "[Bases]\r\n");
        assert!(base_lod_section(&lo, &bases, [0x900]).unwrap_err().contains("missing LOD base"));
        bases.insert(0x0200_0800, Base::default());
        assert!(base_lod_section(&lo, &bases, [0x0200_0800]).unwrap_err().contains("missing owner"));
        bases.insert(0, Base::default());
        assert!(base_lod_section(&lo, &bases, [0]).unwrap_err().contains("zero local ID"));
        drop(lo);
        drop(original);
        std::fs::remove_dir_all(dir).unwrap();
    }

    fn run(lo: &LoadOrder, dynamic: &[DynamicRef], token: &str, rolling: &Rolling<'_>) -> MultiOutput {
        let worlds = lodforge_esp::scan_worldspaces(lo).unwrap();
        let world = worlds.iter().find(|w| w.editor_id == "TestWorld").unwrap();
        let mut bases = HashMap::new();
        bases.insert(0x800, Base { signature: *b"STAT", form_id: 0x800, editor_id: "Rock".into(), flags: 0, model: "rock.nif".into(), mnam: None, material: 0, winning_plugin: 0, dnam: Vec::new(), bounds: [0; 6], type_data: Vec::new(), water_type: 0, alt_textures: Vec::new() });
        build_worlds(lo, &[WorldDynamic { world, dynamic, south_west: (0, 0), lod_refs: &[], children: Vec::new(), underside: None, seasonal: Vec::new(), child_copies: Vec::new() }], &bases, 11, 21, token, rolling, Some(Workarounds { margin: 3 }), None).unwrap()
    }

    #[test]
    fn replacements_retire_originals_and_keep_runtime_relationships() {
        let dir = std::env::temp_dir().join(format!("lodforge-replacements-{}", std::process::id()));
        let mut pb = PluginBuilder::new(0x900);
        pb.esm = true;
        pb.add(Rec::new(b"STAT", 0, NEW | 0x3b).field(b"EDID", zs("XMarker")).field(b"MODL", zs("MarkerX.nif")));
        pb.add(Rec::new(b"MSTT", 0, NEW | 0x800).field(b"EDID", zs("Wheel")).field(b"MODL", zs("wheel.nif")).field(b"DATA", vec![4]));
        let data: Vec<u8> = [1024.0f32, 2048.0, 100.0, 0.1, 0.2, 0.3].iter().flat_map(|v| v.to_le_bytes()).collect();
        let parent = [ (NEW | 0x811).to_le_bytes(), 1u32.to_le_bytes() ].concat();
        pb.worlds.push(WorldContent {
            world: Rec::new(b"WRLD", 0, NEW | 0x801).field(b"EDID", zs("TestWorld")),
            persistent_cell: Some(Rec::new(b"CELL", 0, NEW | 0x802).field(b"DATA", vec![2, 0])),
            persistent: vec![Rec::new(b"REFR", 0x10400, NEW | 0x811).field(b"NAME", (NEW | 0x800).to_le_bytes().to_vec()).field(b"DATA", data.clone())],
            cells: vec![CellContent {
                grid: (0, 0),
                cell: Rec::new(b"CELL", 0, NEW | 0x803).field(b"DATA", vec![0, 0]).field(b"XCLC", vec![0; 12]),
                temporary: vec![Rec::new(b"REFR", 0, NEW | 0x810).field(b"NAME", (NEW | 0x800).to_le_bytes().to_vec())
                    .field(b"XESP", parent).field(b"XSCL", 1.5f32.to_le_bytes().to_vec()).field(b"DATA", data)],
            }],
        });
        std::fs::create_dir_all(&dir).unwrap();
        let empty = LoadOrder::open(&[], |_| None).unwrap();
        std::fs::write(dir.join("Test.esm"), pb.write(&empty)).unwrap();
        let lo = LoadOrder::open(&["Test.esm".into()], |n| Some(dir.join(n))).unwrap();
        let bases = crate::bases::read_bases(&lo).unwrap();
        let mut world = lodforge_esp::scan_worldspaces(&lo).unwrap().remove(0);
        let mut child = dynamic(0x810, "wheel.nif");
        child.replace = true;
        child.grid = crate::rules::Grid::FarLod;
        child.large = true;
        child.enable_parent = 0x811;
        child.opposite_parent = true;
        let mut parent = dynamic(0x811, "wheel.nif");
        parent.replace = true;
        let dynamic = [child, parent];
        for wa in [None, Some(Workarounds { margin: 3 })] {
            let build = |world: &Worldspace, rolling: &Rolling<'_>| build_worlds(
                &lo, &[WorldDynamic { world, dynamic: &dynamic, south_west: (0, 0), lod_refs: &[],
                    children: Vec::new(), underside: None, seasonal: Vec::new(), child_copies: Vec::new() }],
                &bases, 11, 21, "TOKEN", rolling, wa, None,
            );
            let out = build(&world, &Rolling::default()).unwrap();
            let header_end = 24 + u32::from_le_bytes(out.esm[4..8].try_into().unwrap()) as usize;
            let onam: Vec<u32> = lodforge_esp::fields(&out.esm[24..header_end])
                .filter(|f| f.sig.0 == *b"ONAM")
                .flat_map(|f| f.data.chunks_exact(4).map(|id| u32::from_le_bytes(id.try_into().unwrap())))
                .collect();
            assert_eq!(onam, vec![0x810]);
            let text = &out.world_txts[0].1;
            assert!(text.contains("[Disabled]\r\n2064,Test.esm\r\n2065,Test.esm\r\n[Resets]\r\n"));
            let rows: Vec<Vec<&str>> = text.split("[Objects]\r\n").nth(1).unwrap().split("[Markers]").next().unwrap()
                .lines().map(|l| l.split(',').collect()).collect();
            assert_eq!(rows.len(), 2);
            assert!(rows.iter().all(|r| r.len() == 17 && r[3] == "1" && r[5].is_empty() && r[12].is_empty()));
            assert_eq!(&rows[0][2..7], &["1", "1", "1", "", "1"]);
            assert_eq!(rows[0][7], rows[1][0]);
            assert_eq!(rows[0][8], "DynDOLOD.esp");
            assert_eq!(rows[1][7], rows[1][0]);
            assert_eq!(rows[1][8], "DynDOLOD.esp");
            std::fs::write(dir.join("DynDOLOD.esm"), &out.esm).unwrap();
            std::fs::write(dir.join("DynDOLOD.esp"), &out.esp).unwrap();
            let generated = LoadOrder::open(&["Test.esm".into(), "DynDOLOD.esm".into(), "DynDOLOD.esp".into()], |n| Some(dir.join(n))).unwrap();
            let generated_world = lodforge_esp::scan_worldspaces(&generated).unwrap().remove(0);
            for id in [0x810, 0x811] {
                let original = &generated_world.refs[&id];
                assert_eq!(original.base, 0x3b);
                assert_eq!(original.position, [1024.0, 2048.0, -30000.0]);
                assert!(!original.enable_parent);
                assert_eq!(original.flags & 0x10000, 0);
            }
            assert_eq!(generated_world.refs[&0x810].scale, 1.5);
            assert!(!generated_world.refs[&0x810].is_persistent());
            assert!(generated_world.refs[&0x811].is_persistent());
            let copy_id = 0x0200_0000 | rows[0][0].parse::<u32>().unwrap();
            let copy = &generated_world.refs[&copy_id];
            assert_eq!(copy.position, [1024.0, 2048.0, 100.0]);
            assert_eq!(copy.scale, 1.5);
            let (esm, esp) = (Previous::read(&out.esm).unwrap(), Previous::read(&out.esp).unwrap());
            let again = build(&world, &Rolling { esm: Some(&esm), esp: Some(&esp) }).unwrap();
            assert_eq!(out.esm, again.esm);
            assert_eq!(out.esp, again.esp);
            assert_eq!(out.world_txts, again.world_txts);
            let cell = world.cell_records.remove(&(0, 0)).unwrap();
            assert!(build(&world, &Rolling::default()).err().unwrap().contains("missing source cell record"));
            world.cell_records.insert((0, 0), cell);
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn rolling_updates_keep_form_ids() {
        let dir = std::env::temp_dir().join(format!("lodforge-rolling-{}", std::process::id()));
        let lo = test_load_order(&dir, 4);
        let first: Vec<DynamicRef> = vec![dynamic(0x811, "lod\\rock_a.nif"), dynamic(0x812, "lod\\rock_b.nif"), dynamic(0x813, "lod\\rock_a.nif")];
        let a = run(&lo, &first, "TOKEN", &Rolling::default());

        // Same input on top of the first build: identical plugins and data files.
        let (pm, pp) = (Previous::read(&a.esm).unwrap(), Previous::read(&a.esp).unwrap());
        assert_eq!(pp.token.as_deref(), Some("TOKEN"));
        let b = run(&lo, &first, "TOKEN", &Rolling { esm: Some(&pm), esp: Some(&pp) });
        assert!(a.esm == b.esm && a.esp == b.esp, "rebuild changed the plugins");
        assert_eq!(a.worlds_txt, b.worlds_txt);
        assert_eq!(a.world_txts, b.world_txts);

        // One added dynamic reference (sorting first, with a new model): every old record keeps
        // its id, the new ones continue after the old next object id.
        let mut more = vec![dynamic(0x810, "lod\\rock_c.nif")];
        more.extend(first.iter().map(|d| dynamic(d.ref_id, &d.model)));
        let c = run(&lo, &more, "TOKEN", &Rolling { esm: Some(&pm), esp: Some(&pp) });
        // (old, new, added count): the reference in DynDOLOD.esp, its base copy in DynDOLOD.esm.
        for (old, new, n) in [(&pp, Previous::read(&c.esp).unwrap(), 1), (&pm, Previous::read(&c.esm).unwrap(), 1)] {
            for (key, ids) in &old.ids {
                assert_eq!(new.ids.get(key), Some(ids), "{key} moved");
            }
            let mut added: Vec<u32> = new.ids.iter().filter(|(k, _)| !old.ids.contains_key(*k)).flat_map(|(_, v)| v.iter().copied()).collect();
            added.sort_unstable();
            assert_eq!(added, (old.next_object_id..old.next_object_id + n).collect::<Vec<_>>());
            assert_eq!(new.next_object_id, old.next_object_id + n);
        }
        std::fs::remove_dir_all(dir).ok();
    }
}
