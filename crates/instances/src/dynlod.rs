//! `<World>.dyn.bin` (docs/runtime/instance-formats.md section 7b): dynamic LOD items, their
//! models, the reset / disabled / base lists, child worlds, the terrain underside and the large
//! references of a worldspace, for LODForge Runtime's DynDOLOD replacement (M5).

use std::collections::BTreeMap;

use half::f16;

use crate::container::{Buf, Chunk, Common, Container, Cur, Kind, RawSection, WriteOptions, write};
use crate::hash::{angle_rad, angle_u16, ref_hash};
use crate::{Error, Result};

pub const EXT: &str = "dyn.bin";

/// `parent_plugin` / `PLUG` index meaning "none".
pub const NONE: u16 = 0xFFFF;

pub const GRID_NEAR: u32 = 0;
pub const GRID_FAR: u32 = 1;
pub const GRID_NEVER_FADE: u32 = 2;
pub const GRID_MASK: u32 = 3;
pub const LARGE: u32 = 1 << 2;
pub const OPPOSITE: u32 = 1 << 3;
pub const INITIALLY_ENABLED: u32 = 1 << 4;
pub const REPLACE: u32 = 1 << 5;
pub const PERSISTENT: u32 = 1 << 6;
pub const DOOR: u32 = 1 << 7;
pub const FULL: u32 = 1 << 8;
pub const ANIMATED: u32 = 1 << 9;
pub const GLOW: u32 = 1 << 10;
pub const GLOW_LEVEL_SHIFT: u32 = 11;
/// Glow level field value for "not a `glow_lod_N` model".
pub const GLOW_LEVEL_NONE: u32 = 3;
pub const CHILD_COPY: u32 = 1 << 13;
pub const IN_CHILD_WORLDS: u32 = 1 << 14;
pub const FROM_CHILD_WORLD: u32 = 1 << 15;
/// A parent-world reference copied into a child worldspace that uses the parent's LOD (in a
/// `<Child>.dyn.bin`): the engine hides the parent's LOD in the child's cells, and the child has no
/// object of its own there (stables, farms, outer walls). Drawn while the player is in the child
/// world, at its LOD models (`DMDL` slots 1-3; slot 0 = the LOD4 model).
pub const PARENT_COPY: u32 = 1 << 17;
/// The base is a tree (`TREE`, or a `STAT` with tree LOD); its LOD4 may be a billboard (no pack
/// mesh): draw the parent's impostor of the same reference (`.trees.bin`, same plugin and form).
pub const TREE: u32 = 1 << 18;
/// A `parent_copy` that duplicates a child-world reference (same model and place, or a known
/// pair): the twin is in the `direct_parent` fields (54, 60). Hide the item while the twin's 3D
/// is loaded.
pub const TWIN: u32 = 1 << 19;
/// The direct enable parent's own XESP opposite flag.
pub const DIRECT_OPPOSITE: u32 = 1 << 16;

/// `DHDR.flags`: every eligible large reference was instanced (a large reference missing from
/// `.statics.bin` is in the `.bto` or has no LOD; see `DLRG`).
pub const HDR_LARGE_REFS_INSTANCED: u16 = 1;

/// `DMDL.flags`.
pub const MODEL_FULL: u32 = 1;
pub const MODEL_ANIMATED: u32 = 2;
pub const MODEL_GLOW: u32 = 4;

/// `DLRG` entry `lod` bits.
pub const LRG_BTO4: u16 = 1;
pub const LRG_BTO8: u16 = 2;
pub const LRG_BTO16: u16 = 4;
pub const LRG_INST4: u16 = 8;
pub const LRG_INST8: u16 = 16;
pub const LRG_INST16: u16 = 32;
pub const LRG_DYN: u16 = 64;
pub const LRG_PERSISTENT: u16 = 128;
pub const LRG_DISABLED: u16 = 256;

/// No pack mesh.
pub const NO_MESH: u32 = 0xFFFF_FFFF;

/// A form named by `PLUG` index and local form id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct FormRef {
    pub plugin: u16,
    pub form: u32,
}

/// One dynamic item (64-byte record).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DynItem {
    pub pos: [f32; 3],
    pub angles: [u16; 3],
    pub scale: f16,
    pub original: FormRef,
    /// `plugin == NONE`: no enable parent.
    pub parent: FormRef,
    pub model: u32,
    pub flags: u32,
    pub hash: u32,
    pub destruction_stage: u8,
    pub destruction_type: u8,
    /// Offset 46 (since 1.7, `tree` items): the parent's impostor type index + 1 (the
    /// `impostors.json` type of the base; 0 = none), for trees without a `.trees.bin` record.
    pub impostor: u16,
    pub cell: (i16, i16),
    pub base: FormRef,
    /// The direct enable parent when it differs from `parent` (the top of the chain); `plugin ==
    /// NONE` otherwise. Its own opposite flag is [`DIRECT_OPPOSITE`].
    pub direct_parent: FormRef,
}

impl DynItem {
    pub fn angles_rad(&self) -> [f32; 3] {
        self.angles.map(angle_rad)
    }

    pub fn grid(&self) -> u32 {
        self.flags & GRID_MASK
    }

    pub fn has_parent(&self) -> bool {
        self.parent.plugin != NONE
    }

    fn write(&self, b: &mut Buf) {
        for v in self.pos {
            b.f32(v);
        }
        for a in self.angles {
            b.u16(a);
        }
        b.u16(self.scale.to_bits());
        b.u16(self.original.plugin);
        b.u16(self.parent.plugin);
        b.u32(self.original.form);
        b.u32(self.parent.form);
        b.u32(self.model);
        b.u32(self.flags);
        b.u32(self.hash);
        b.u8(self.destruction_stage);
        b.u8(self.destruction_type);
        b.u16(self.impostor);
        b.i16(self.cell.0);
        b.i16(self.cell.1);
        b.u16(self.base.plugin);
        b.u16(self.direct_parent.plugin);
        b.u32(self.base.form);
        b.u32(self.direct_parent.form);
    }

    fn read(r: &mut Cur<'_>) -> Result<DynItem> {
        let pos = [r.f32()?, r.f32()?, r.f32()?];
        let angles = [r.u16()?, r.u16()?, r.u16()?];
        let scale = f16::from_bits(r.u16()?);
        let (op, pp) = (r.u16()?, r.u16()?);
        let (of, pf) = (r.u32()?, r.u32()?);
        let (model, flags, hash) = (r.u32()?, r.u32()?, r.u32()?);
        let (destruction_stage, destruction_type) = (r.u8()?, r.u8()?);
        let impostor = r.u16()?;
        let cell = (r.i16()?, r.i16()?);
        let bp = r.u16()?;
        let dp = r.u16()?;
        let bf = r.u32()?;
        let df = r.u32()?;
        Ok(DynItem { pos, angles, scale, original: FormRef { plugin: op, form: of }, parent: FormRef { plugin: pp, form: pf }, model, flags, hash, destruction_stage, destruction_type, cell, base: FormRef { plugin: bp, form: bf }, direct_parent: FormRef { plugin: dp, form: df }, impostor })
    }
}

/// One `DMDL` entry.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DynModel {
    pub flags: u32,
    /// `StaticMeshes.bin` mesh of the dynamic model and of the LOD4, LOD8, LOD16 models.
    pub pack: [u32; 4],
    pub bounds: [f32; 6],
    pub model: String,
    /// LOD4, LOD8, LOD16 models of the item's rule (`""` = none).
    pub lod: [String; 3],
    /// `DMVT` (since 1.7): per slot (dynamic model, LOD4, LOD8, LOD16), the (diffuse, normal) of
    /// each part when the reference draws other textures than the model's (AltTextures; empty:
    /// the model's own) ...
    pub textures: [Vec<(String, String)>; 4],
    /// ... and the per-part LOD material class with the whitening bit (`MCLS` coding; empty: all
    /// plain). The pack mesh of a slot is built for exactly this variant.
    pub classes: [Vec<u8>; 4],
}

/// `DHDR`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DynHeader {
    pub world: FormRef,
    pub south_west: (i16, i16),
    pub near_grid: u8,
    pub far_grid: u8,
    pub glow_multiple: f32,
    pub lod_world: String,
    pub flags: u16,
}

/// `DCHW` entry.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ChildWorld {
    pub world: FormRef,
    /// Bit 0 uses this world's LOD, bit 1 has bounds.
    pub flags: u16,
    pub bounds: [i16; 4],
    pub editor_id: String,
}

pub const CHILD_USES_PARENT_LOD: u16 = 1;
pub const CHILD_HAS_BOUNDS: u16 = 2;
/// The child uses the parent's land (terrain, so also its grass: draw the parent's `.lfg`).
pub const CHILD_USES_PARENT_LAND: u16 = 4;

/// `DUND`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Underside {
    pub model: String,
    pub height: f32,
    pub worlds: Vec<FormRef>,
}

/// `DLRG` entry.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct LargeRef {
    pub reference: FormRef,
    pub lod: u16,
    pub bounds: [f32; 6],
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DynFile {
    pub common: Common,
    pub header: DynHeader,
    pub models: Vec<DynModel>,
    /// `DSEA`: per season code, (original, model) pairs.
    pub seasons: Vec<(String, Vec<(FormRef, u32)>)>,
    pub resets: Vec<FormRef>,
    pub disabled: Vec<FormRef>,
    /// `DMIG`: persistent large references DynDOLOD's large-reference workaround toggles (its
    /// `[Markers]` entries, this world and its own-LOD child worlds), for save migration.
    pub migration: Vec<FormRef>,
    pub bases: Vec<FormRef>,
    pub child_worlds: Vec<ChildWorld>,
    pub underside: Option<Underside>,
    /// `DLRG`, keyed by large reference cell.
    pub large: BTreeMap<(i16, i16), Vec<LargeRef>>,
    /// Items keyed by the cell of their position.
    pub cells: BTreeMap<(i16, i16), Vec<DynItem>>,
    model_index: std::collections::HashMap<String, u32>,
}

/// The cell of a world position.
pub fn cell_of(p: [f32; 3]) -> (i16, i16) {
    ((p[0] / 4096.0).floor() as i16, (p[1] / 4096.0).floor() as i16)
}

fn form_list(b: &mut Buf, v: &[FormRef]) {
    b.u32(v.len() as u32);
    for f in v {
        b.u16(f.plugin);
        b.u16(0);
        b.u32(f.form);
    }
}

fn read_form(r: &mut Cur<'_>) -> Result<FormRef> {
    let plugin = r.u16()?;
    r.u16()?;
    Ok(FormRef { plugin, form: r.u32()? })
}

fn read_form_list(r: &mut Cur<'_>) -> Result<Vec<FormRef>> {
    let n = r.u32()?;
    (0..n).map(|_| read_form(r)).collect()
}

impl DynFile {
    pub fn new(common: Common) -> DynFile {
        DynFile { common, ..Default::default() }
    }

    /// `(plugin name, local form)` -> `FormRef`, adding the plugin to `PLUG`.
    pub fn form(&mut self, plugin: &str, form: u32) -> FormRef {
        FormRef { plugin: self.common.plugin_index(plugin), form }
    }

    /// The `DMDL` index of a model entry (`key` names model, columns and flags), added with
    /// `make` when new.
    pub fn model(&mut self, key: &str, make: impl FnOnce() -> DynModel) -> u32 {
        if let Some(&i) = self.model_index.get(key) {
            return i;
        }
        self.models.push(make());
        let i = (self.models.len() - 1) as u32;
        self.model_index.insert(key.to_string(), i);
        i
    }

    /// Adds an item. `angles` in radians as the plugin stores them; `parent` `None` without an
    /// enable parent; `direct` the direct enable parent when it differs from `parent`.
    #[allow(clippy::too_many_arguments)]
    pub fn add(&mut self, pos: [f32; 3], angles: [f32; 3], scale: f32, original: (&str, u32), parent: Option<(&str, u32)>, model: u32, flags: u32, cell: (i16, i16), base: (&str, u32), direct: Option<(&str, u32)>) -> &mut DynItem {
        let o = self.form(original.0, original.1);
        let p = match parent {
            Some((n, f)) => self.form(n, f),
            None => FormRef { plugin: NONE, form: 0 },
        };
        let b = self.form(base.0, base.1);
        let d = match direct {
            Some((n, f)) => self.form(n, f),
            None => FormRef { plugin: NONE, form: 0 },
        };
        let item = DynItem { pos, angles: angles.map(angle_u16), scale: f16::from_f32(scale), original: o, parent: p, model, flags, hash: ref_hash(original.0, original.1), destruction_stage: 0xFF, destruction_type: 0, cell, base: b, direct_parent: d, impostor: 0 };
        let v = self.cells.entry(cell_of(pos)).or_default();
        v.push(item);
        v.last_mut().unwrap()
    }

    pub fn items(&self) -> usize {
        self.cells.values().map(Vec::len).sum()
    }

    pub fn to_bytes(&self, opts: WriteOptions) -> Result<Vec<u8>> {
        let mut chunks = self.common.chunks();
        let h = &self.header;
        let mut b = Buf::default();
        b.u16(h.world.plugin);
        b.u16(0);
        b.u32(h.world.form);
        b.i16(h.south_west.0);
        b.i16(h.south_west.1);
        b.u8(h.near_grid);
        b.u8(h.far_grid);
        b.u16(h.flags);
        b.f32(h.glow_multiple);
        b.str(&h.lod_world);
        chunks.push(Chunk { tag: *b"DHDR", data: b.0 });
        let mut b = Buf::default();
        b.u32(self.models.len() as u32);
        for m in &self.models {
            b.u32(m.flags);
            for v in m.pack {
                b.u32(v);
            }
            for v in m.bounds {
                b.f32(v);
            }
            b.str(&m.model);
            for l in &m.lod {
                b.str(l);
            }
        }
        chunks.push(Chunk { tag: *b"DMDL", data: b.0 });
        if self.models.iter().any(|m| m.textures.iter().any(|t| !t.is_empty()) || m.classes.iter().any(|c| !c.is_empty())) {
            let mut b = Buf::default();
            b.u32(self.models.len() as u32);
            for m in &self.models {
                for s in 0..4 {
                    b.u16(m.textures[s].len() as u16);
                    for (d, n) in &m.textures[s] {
                        b.str(d);
                        b.str(n);
                    }
                    b.u16(m.classes[s].len() as u16);
                    b.0.extend(&m.classes[s]);
                }
            }
            chunks.push(Chunk { tag: *b"DMVT", data: b.0 });
        }
        if !self.seasons.is_empty() {
            let mut b = Buf::default();
            b.u32(self.seasons.len() as u32);
            for (code, v) in &self.seasons {
                b.str(code);
                b.u32(v.len() as u32);
                for (f, m) in v {
                    b.u16(f.plugin);
                    b.u16(0);
                    b.u32(f.form);
                    b.u32(*m);
                }
            }
            chunks.push(Chunk { tag: *b"DSEA", data: b.0 });
        }
        for (tag, list) in [(b"DRST", &self.resets), (b"DDIS", &self.disabled), (b"DBAS", &self.bases), (b"DMIG", &self.migration)] {
            let mut b = Buf::default();
            form_list(&mut b, list);
            chunks.push(Chunk { tag: *tag, data: b.0 });
        }
        if !self.child_worlds.is_empty() {
            let mut b = Buf::default();
            b.u32(self.child_worlds.len() as u32);
            for c in &self.child_worlds {
                b.u16(c.world.plugin);
                b.u16(c.flags);
                b.u32(c.world.form);
                for v in c.bounds {
                    b.i16(v);
                }
                b.str(&c.editor_id);
            }
            chunks.push(Chunk { tag: *b"DCHW", data: b.0 });
        }
        if let Some(u) = &self.underside {
            let mut b = Buf::default();
            b.str(&u.model);
            b.f32(u.height);
            form_list(&mut b, &u.worlds);
            chunks.push(Chunk { tag: *b"DUND", data: b.0 });
        }
        if !self.large.is_empty() {
            let mut b = Buf::default();
            b.u32(self.large.len() as u32);
            for (&(x, y), v) in &self.large {
                b.i16(x);
                b.i16(y);
                b.u32(v.len() as u32);
                for l in v {
                    b.u16(l.reference.plugin);
                    b.u16(l.lod);
                    b.u32(l.reference.form);
                    for f in l.bounds {
                        b.f32(f);
                    }
                }
            }
            chunks.push(Chunk { tag: *b"DLRG", data: b.0 });
        }
        let sections = self
            .cells
            .iter()
            .map(|(&(x, y), items)| {
                let mut sorted = items.clone();
                sorted.sort_by_key(|r| (r.hash, r.original.plugin, r.original.form, r.flags & CHILD_COPY));
                let mut b = Buf(Vec::with_capacity(sorted.len() * 64));
                for r in &sorted {
                    r.write(&mut b);
                }
                RawSection { level: 0, x, y, data: b.0, records: sorted.len() as u32 }
            })
            .collect();
        write(Kind::Dyn, &chunks, sections, opts)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<DynFile> {
        let c = Container::parse(bytes, Kind::Dyn)?;
        c.verify()?;
        let mut f = DynFile { common: c.common()?, ..Default::default() };
        if let Some(ch) = c.chunk(b"DHDR") {
            let mut r = Cur::new(&ch.data);
            let world = read_form(&mut r)?;
            let south_west = (r.i16()?, r.i16()?);
            let (near_grid, far_grid, flags) = (r.u8()?, r.u8()?, r.u16()?);
            f.header = DynHeader { world, south_west, near_grid, far_grid, flags, glow_multiple: r.f32()?, lod_world: r.str()? };
        }
        if let Some(ch) = c.chunk(b"DMDL") {
            let mut r = Cur::new(&ch.data);
            let n = r.u32()?;
            for _ in 0..n {
                let flags = r.u32()?;
                let pack = [r.u32()?, r.u32()?, r.u32()?, r.u32()?];
                let mut bounds = [0f32; 6];
                for v in &mut bounds {
                    *v = r.f32()?;
                }
                let model = r.str()?;
                let lod = [r.str()?, r.str()?, r.str()?];
                f.models.push(DynModel { flags, pack, bounds, model, lod, ..Default::default() });
            }
        }
        if let Some(ch) = c.chunk(b"DMVT") {
            let mut r = Cur::new(&ch.data);
            let n = r.u32()? as usize;
            for i in 0..n {
                for s in 0..4 {
                    let k = usize::from(r.u16()?);
                    let t = (0..k).map(|_| Ok((r.str()?, r.str()?))).collect::<Result<Vec<_>>>()?;
                    let k = usize::from(r.u16()?);
                    let cl = r.take(k)?.to_vec();
                    if let Some(m) = f.models.get_mut(i) {
                        m.textures[s] = t;
                        m.classes[s] = cl;
                    }
                }
            }
        }
        if let Some(ch) = c.chunk(b"DSEA") {
            let mut r = Cur::new(&ch.data);
            let n = r.u32()?;
            for _ in 0..n {
                let code = r.str()?;
                let k = r.u32()?;
                let v = (0..k).map(|_| Ok((read_form(&mut r)?, r.u32()?))).collect::<Result<Vec<_>>>()?;
                f.seasons.push((code, v));
            }
        }
        for (tag, list) in [(b"DRST", &mut f.resets), (b"DDIS", &mut f.disabled), (b"DBAS", &mut f.bases), (b"DMIG", &mut f.migration)] {
            if let Some(ch) = c.chunk(tag) {
                *list = read_form_list(&mut Cur::new(&ch.data))?;
            }
        }
        if let Some(ch) = c.chunk(b"DCHW") {
            let mut r = Cur::new(&ch.data);
            let n = r.u32()?;
            for _ in 0..n {
                let plugin = r.u16()?;
                let flags = r.u16()?;
                let form = r.u32()?;
                let bounds = [r.i16()?, r.i16()?, r.i16()?, r.i16()?];
                f.child_worlds.push(ChildWorld { world: FormRef { plugin, form }, flags, bounds, editor_id: r.str()? });
            }
        }
        if let Some(ch) = c.chunk(b"DUND") {
            let mut r = Cur::new(&ch.data);
            f.underside = Some(Underside { model: r.str()?, height: r.f32()?, worlds: read_form_list(&mut r)? });
        }
        if let Some(ch) = c.chunk(b"DLRG") {
            let mut r = Cur::new(&ch.data);
            let n = r.u32()?;
            for _ in 0..n {
                let key = (r.i16()?, r.i16()?);
                let k = r.u32()?;
                let mut v = Vec::with_capacity(k as usize);
                for _ in 0..k {
                    let plugin = r.u16()?;
                    let lod = r.u16()?;
                    let form = r.u32()?;
                    let mut bounds = [0f32; 6];
                    for b in &mut bounds {
                        *b = r.f32()?;
                    }
                    v.push(LargeRef { reference: FormRef { plugin, form }, lod, bounds });
                }
                f.large.insert(key, v);
            }
        }
        let stride = c.header.record_size as usize;
        if stride < 64 {
            return Err(Error::Invalid("dyn record size"));
        }
        for e in &c.index {
            let raw = c.payload(e)?;
            let mut v = Vec::with_capacity(e.record_count as usize);
            for i in 0..e.record_count as usize {
                let mut r = Cur::new(raw.get(i * stride..(i + 1) * stride).ok_or(Error::Truncated)?);
                v.push(DynItem::read(&mut r)?);
            }
            f.cells.insert((e.x, e.y), v);
        }
        for (i, m) in f.models.iter().enumerate() {
            f.model_index.insert(format!("{}#{i}", m.model), i as u32);
        }
        Ok(f)
    }

    /// Sorts every cell as the writer stores it.
    pub fn finish(&mut self) {
        for v in self.cells.values_mut() {
            v.sort_by_key(|r| (r.hash, r.original.plugin, r.original.form, r.flags & CHILD_COPY));
        }
    }
}
