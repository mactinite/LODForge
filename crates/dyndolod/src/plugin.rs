//! Writing Skyrim SE plugins (DynDOLOD.esm/.esp, Occlusion.esp): records are built and copied in
//! load-order form-id space and translated to the new plugin's master list when written.
//!
//! Copied overrides (WRLD, CELL) have their form-id subrecords translated through a small schema;
//! other subrecords are copied verbatim.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use lodforge_esp::record::{FLAG_COMPRESSED, Sig};
use lodforge_esp::{Entries, Entry, LoadOrder, RecordRef, fields};

/// A subrecord whose data holds form ids at the given positions.
enum Ids {
    /// One form id at offset 0.
    One,
    /// A list of form ids filling the subrecord.
    List,
    /// One form id at this offset.
    At(usize),
    /// WRLD RNAM: y i16, x i16, count u32, count x (form id, i16, i16).
    Rnam,
}

fn schema(record: &[u8; 4], field: &[u8; 4]) -> Option<Ids> {
    match (record, field) {
        (b"CELL", b"XCLR") => Some(Ids::List),
        (b"CELL", b"XCWT" | b"XOWN" | b"LTMP" | b"XLCN" | b"XEZN" | b"XCMO" | b"XCIM" | b"XCAS" | b"XCCM" | b"XILL") => Some(Ids::One),
        (b"WRLD", b"XLCN" | b"WNAM" | b"CNAM" | b"NAM2" | b"NAM3" | b"ZNAM" | b"XEZN" | b"INAM" | b"XLTW") => Some(Ids::One),
        (b"WRLD", b"RNAM") => Some(Ids::Rnam),
        (b"REFR", b"NAME" | b"XLRL" | b"XEMI" | b"XOWN" | b"XLCN" | b"XLIB" | b"XEZN" | b"XMBR" | b"XLTW" | b"INAM" | b"XCZC" | b"XATR") => Some(Ids::One),
        (b"REFR", b"XESP" | b"XTEL" | b"XNDP" | b"XPWR") => Some(Ids::At(0)),
        (b"REFR", b"XLOC") => Some(Ids::At(4)),
        (b"REFR", b"XLKR" | b"XLRT" | b"XPOD" | b"XLRM") => Some(Ids::List),
        (b"FLST", b"LNAM") => Some(Ids::One),
        (b"STAT", b"DNAM") => Some(Ids::At(4)),
        _ => None,
    }
}

/// Form-id positions inside a subrecord.
fn id_offsets(record: &[u8; 4], field: &[u8; 4], data: &[u8]) -> Vec<usize> {
    match schema(record, field) {
        None => Vec::new(),
        Some(Ids::One) => if data.len() >= 4 { vec![0] } else { Vec::new() },
        Some(Ids::At(o)) => if data.len() >= o + 4 { vec![o] } else { Vec::new() },
        Some(Ids::List) => (0..data.len() / 4).map(|i| i * 4).collect(),
        Some(Ids::Rnam) => {
            if data.len() < 8 {
                return Vec::new();
            }
            let n = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;
            (0..n).map(|k| 8 + k * 8).filter(|o| o + 4 <= data.len()).collect()
        }
    }
}

/// A record with its fields in load-order form-id space.
#[derive(Debug, Clone)]
pub struct Rec {
    pub sig: [u8; 4],
    pub flags: u32,
    /// Load-order form id (for new records: `NEW | object id`, see [`PluginBuilder::new_id`]).
    pub form_id: u32,
    pub fields: Vec<([u8; 4], Vec<u8>)>,
    /// Extra form-id positions (field index, byte offset) outside the schema, e.g. object
    /// properties inside VMAD.
    pub id_at: Vec<(usize, usize)>,
}

/// Marker for form ids of records the new plugin itself defines (`NEW | object id`).
pub const NEW: u32 = 0xFF00_0000;
/// Marker for form ids of records the companion master defines ([`PluginBuilder::companion`],
/// DynDOLOD.esm for DynDOLOD.esp): `COMPANION | object id`.
pub const COMPANION: u32 = 0xFD00_0000;

impl Rec {
    pub fn new(sig: &[u8; 4], flags: u32, form_id: u32) -> Rec {
        Rec { sig: *sig, flags, form_id, fields: Vec::new(), id_at: Vec::new() }
    }

    pub fn field(mut self, sig: &[u8; 4], data: impl Into<Vec<u8>>) -> Rec {
        self.fields.push((*sig, data.into()));
        self
    }

    /// A field with load-order form ids at the given byte offsets.
    pub fn field_ids(mut self, sig: &[u8; 4], data: impl Into<Vec<u8>>, offsets: &[usize]) -> Rec {
        let i = self.fields.len();
        self.fields.push((*sig, data.into()));
        self.id_at.extend(offsets.iter().map(|&o| (i, o)));
        self
    }

    /// Form-id offsets of field `i`: the schema's plus the explicit ones.
    fn offsets(&self, i: usize) -> Vec<usize> {
        let (s, d) = &self.fields[i];
        let mut v = id_offsets(&self.sig, s, d);
        v.extend(self.id_at.iter().filter(|(f, o)| *f == i && o + 4 <= d.len()).map(|(_, o)| *o));
        v
    }

    /// Copies a winning record of the load order, form ids translated to load-order ids.
    pub fn copy(lo: &LoadOrder, at: RecordRef) -> Option<Rec> {
        let lp = lo.plugins.get(usize::from(at.plugin))?;
        let r = lp.record_at(at.offset).ok()?;
        let data = r.data().ok()?;
        // The compressed flag is kept: xEdit writes a copied override compressed when its source
        // record was (golden Occlusion.esp: 6,047 of 6,285 CELLs, exactly the compressed sources).
        let mut rec = Rec { sig: r.sig.0, flags: r.flags, form_id: lp.global_form_id(r.form_id)?, fields: Vec::new(), id_at: Vec::new() };
        for f in fields(&data) {
            let mut d = f.data.to_vec();
            for o in id_offsets(&rec.sig, &f.sig.0, &d) {
                let local = u32::from_le_bytes(d[o..o + 4].try_into().unwrap());
                let global = if local == 0 { 0 } else { lp.global_form_id(local).unwrap_or(0) };
                d[o..o + 4].copy_from_slice(&global.to_le_bytes());
            }
            rec.fields.push((f.sig.0, d));
        }
        Some(rec)
    }

    /// Whether a copied REFR can be written as an override: every subrecord is either free of
    /// form ids or has its form ids in the schema (no scripts, no unknown subrecords).
    pub fn copyable_ref(&self) -> bool {
        const PLAIN: &[&[u8; 4]] = &[
            b"EDID", b"DATA", b"XSCL", b"XPRM", b"XRDS", b"XMRK", b"FNAM", b"FULL", b"TNAM", b"XRGB", b"XRGD", b"XHTW", b"XFVC", b"XCNT", b"XACT", b"XLIG", b"XALP", b"XTRI", b"XLOD", b"XMBO", b"XRNK", b"XIS2",
            b"XCHG", b"XHLT", b"XOCP", b"XORD", b"XLCM",
        ];
        &self.sig == b"REFR" && self.fields.iter().all(|(s, _)| PLAIN.contains(&s) || schema(&self.sig, s).is_some())
    }

    /// Load-order form ids this record refers to (header and schema fields).
    fn referenced(&self) -> impl Iterator<Item = u32> + '_ {
        std::iter::once(self.form_id).chain((0..self.fields.len()).flat_map(move |i| {
            let d = &self.fields[i].1;
            self.offsets(i).into_iter().map(move |o| u32::from_le_bytes(d[o..o + 4].try_into().unwrap()))
        }))
    }
}

/// Exterior cell with its override record and temporary children.
#[derive(Debug, Clone)]
pub struct CellContent {
    pub grid: (i32, i32),
    pub cell: Rec,
    pub temporary: Vec<Rec>,
}

/// A worldspace override with its persistent cell and exterior cells.
#[derive(Debug, Clone)]
pub struct WorldContent {
    pub world: Rec,
    pub persistent_cell: Option<Rec>,
    pub persistent: Vec<Rec>,
    pub cells: Vec<CellContent>,
}

#[derive(Debug, Clone, Default)]
pub struct PluginBuilder {
    pub esm: bool,
    pub esl: bool,
    pub author: String,
    pub description: String,
    /// HEDR version (1.71 lets new form ids start below 0x800 on new runtimes).
    pub version: f32,
    /// Top-level groups by signature (records in insertion order).
    pub top: BTreeMap<[u8; 4], Vec<Rec>>,
    pub top_order: Vec<[u8; 4]>,
    pub worlds: Vec<WorldContent>,
    pub next_object_id: u32,
    /// TES4 INCC (interior cell count) written after the masters when set.
    pub incc: Option<u32>,
    /// Object ids of an earlier build by record key, handed out by [`PluginBuilder::keyed_id`].
    pub reuse: HashMap<String, VecDeque<u32>>,
    /// A master that is not in the load order yet (built alongside, e.g. DynDOLOD.esm), referred
    /// to with [`COMPANION`] ids; listed after the load-order masters with the master flag.
    pub companion: Option<String>,
}

/// Key under which a new record keeps its form id across rebuilds: signature, EditorID and model
/// (MODL for base records; none for REFRs, whose base may sit in DynDOLOD.esm), case-insensitive.
pub fn record_key(sig: &[u8; 4], editor_id: &str, model: &str) -> String {
    format!("{}|{}|{}", String::from_utf8_lossy(sig), editor_id.to_ascii_lowercase(), model.to_ascii_lowercase())
}

/// The new records of an earlier build of a plugin, for rolling updates: DynDOLOD loads the next
/// object id from the old plugin ("Loading %s next object ID from %s for rolling updates") so
/// existing form ids, and the saves that store them, stay valid.
#[derive(Debug, Default, Clone)]
pub struct Previous {
    /// Object ids by [`record_key`], in file order.
    pub ids: HashMap<String, VecDeque<u32>>,
    /// HEDR next object id (at least one past the highest new record).
    pub next_object_id: u32,
    /// CNAM.
    pub author: String,
    /// EditorID of KYWD 0x910 (the run token).
    pub token: Option<String>,
    /// New records with an EditorID.
    pub records: usize,
}

/// (signature, object id, EditorID, MODL)
type PrevRecord = ([u8; 4], u32, String, String);

impl Previous {
    pub fn read(bytes: &[u8]) -> Option<Previous> {
        if bytes.len() < 24 || &bytes[..4] != b"TES4" {
            return None;
        }
        let size = u32::from_le_bytes(bytes[4..8].try_into().ok()?) as usize;
        let body = bytes.get(24 + size..)?;
        let mut prev = Previous::default();
        let mut masters = 0u32;
        for f in fields(&bytes[24..24 + size]) {
            match &f.sig.0 {
                b"HEDR" if f.data.len() >= 12 => prev.next_object_id = u32::from_le_bytes(f.data[8..12].try_into().ok()?),
                b"CNAM" => prev.author = f.zstring(),
                b"MAST" => masters += 1,
                _ => {}
            }
        }
        fn walk(es: Entries<'_>, masters: u32, out: &mut Vec<PrevRecord>) {
            for e in es.flatten() {
                match e {
                    Entry::Group(g) => walk(g.entries(), masters, out),
                    Entry::Record(r) if r.form_id >> 24 == masters => {
                        let Ok(data) = r.data() else { continue };
                        let (mut edid, mut modl) = (String::new(), String::new());
                        for f in fields(&data) {
                            match &f.sig.0 {
                                b"EDID" => edid = f.zstring(),
                                b"MODL" => modl = f.zstring(),
                                _ => {}
                            }
                        }
                        out.push((r.sig.0, r.form_id & 0x00FF_FFFF, edid, modl));
                    }
                    Entry::Record(_) => {}
                }
            }
        }
        let mut recs = Vec::new();
        walk(Entries::new(body, 24 + size), masters, &mut recs);
        for (sig, id, edid, modl) in &recs {
            if sig == b"KYWD" && *id == 0x910 {
                prev.token = Some(edid.clone()).filter(|t| !t.is_empty());
            }
            prev.next_object_id = prev.next_object_id.max(id + 1);
            if edid.is_empty() {
                continue;
            }
            // REFRs by EditorID alone: their base can sit in another plugin (DynDOLOD.esm).
            let model = if sig == b"REFR" { "" } else { modl.as_str() };
            prev.ids.entry(record_key(sig, edid, model)).or_default().push_back(*id);
            prev.records += 1;
        }
        Some(prev)
    }
}

fn owner_key(form_id: u32) -> u32 {
    crate::bases::owner_key(form_id)
}

impl PluginBuilder {
    pub fn new(first_object_id: u32) -> PluginBuilder {
        PluginBuilder { version: 1.71, next_object_id: first_object_id, ..Default::default() }
    }

    /// A new form id of this plugin.
    pub fn new_id(&mut self) -> u32 {
        let id = NEW | self.next_object_id;
        self.next_object_id += 1;
        id
    }

    /// Continues an earlier build: its records keep their object ids ([`PluginBuilder::keyed_id`])
    /// and new records get ids from its next object id on.
    pub fn resume(&mut self, prev: &Previous) {
        self.next_object_id = self.next_object_id.max(prev.next_object_id);
        self.reuse = prev.ids.clone();
    }

    /// The earlier build's form id for the record with this [`record_key`], else a new one.
    pub fn keyed_id(&mut self, key: &str) -> u32 {
        match self.reuse.get_mut(key).and_then(VecDeque::pop_front) {
            Some(id) => NEW | id,
            None => self.new_id(),
        }
    }

    pub fn add(&mut self, rec: Rec) {
        if !self.top.contains_key(&rec.sig) {
            self.top_order.push(rec.sig);
        }
        self.top.entry(rec.sig).or_default().push(rec);
    }

    fn all_records(&self) -> impl Iterator<Item = &Rec> {
        self.top.values().flatten().chain(self.worlds.iter().flat_map(|w| {
            std::iter::once(&w.world).chain(w.persistent_cell.iter()).chain(w.persistent.iter()).chain(w.cells.iter().flat_map(|c| std::iter::once(&c.cell).chain(c.temporary.iter())))
        }))
    }

    /// Serialises the plugin. Masters are every load-order plugin a record refers to, in load
    /// order.
    pub fn write(&self, lo: &LoadOrder) -> Vec<u8> {
        let owners = crate::bases::owners(lo);
        let mut needed: BTreeSet<usize> = BTreeSet::new();
        let mut uses_companion = false;
        for r in self.all_records() {
            for id in r.referenced() {
                if id & 0xFF00_0000 == COMPANION {
                    uses_companion = true;
                } else if id != 0 && id & 0xFF00_0000 != NEW
                    && let Some(&i) = owners.get(&owner_key(id))
                {
                    needed.insert(i);
                }
            }
        }
        let masters: Vec<usize> = needed.into_iter().collect();
        let companion = self.companion.as_deref().filter(|_| uses_companion);
        // The companion master goes after the master-flagged plugins, as it loads.
        let companion_pos = masters.iter().take_while(|&&i| lo.plugins[i].plugin.header.flags & 1 != 0).count() as u32;
        let self_index = masters.len() as u32 + u32::from(companion.is_some());
        let pos: HashMap<usize, u32> = masters.iter().enumerate().map(|(k, &i)| (i, k as u32 + u32::from(companion.is_some() && k as u32 >= companion_pos))).collect();
        let local = |id: u32| -> u32 {
            if id == 0 {
                return 0;
            }
            if id & 0xFF00_0000 == NEW {
                return (self_index << 24) | (id & 0x00FF_FFFF);
            }
            if id & 0xFF00_0000 == COMPANION {
                return (companion_pos << 24) | (id & 0x00FF_FFFF);
            }
            let Some(&i) = owners.get(&owner_key(id)) else { return 0 };
            let m = pos[&i];
            if id >> 24 == 0xFE { (m << 24) | (id & 0xFFF) } else { (m << 24) | (id & 0x00FF_FFFF) }
        };
        let rec_bytes = |r: &Rec| -> Vec<u8> {
            let mut data = Vec::new();
            for (i, (s, d)) in r.fields.iter().enumerate() {
                let mut d = d.clone();
                for o in r.offsets(i) {
                    let g = u32::from_le_bytes(d[o..o + 4].try_into().unwrap());
                    d[o..o + 4].copy_from_slice(&local(g).to_le_bytes());
                }
                field_bytes(&mut data, s, &d);
            }
            record_bytes(&r.sig, r.flags, local(r.form_id), &data)
        };
        let mut count = 0u32;
        let mut body = Vec::new();
        for sig in &self.top_order {
            let recs = &self.top[sig];
            let mut g = Vec::new();
            for r in recs {
                g.extend(rec_bytes(r));
                count += 1;
            }
            body.extend(group_bytes(*sig, 0, &g));
            count += 1;
        }
        if !self.worlds.is_empty() {
            let mut top = Vec::new();
            for w in &self.worlds {
                let world_start = top.len();
                top.extend(rec_bytes(&w.world));
                count += 1;
                let wid = local(w.world.form_id);
                let mut children = Vec::new();
                if let Some(pc) = &w.persistent_cell {
                    children.extend(rec_bytes(pc));
                    count += 1;
                    let pid = local(pc.form_id);
                    let mut refs = Vec::new();
                    for r in &w.persistent {
                        refs.extend(rec_bytes(r));
                        count += 1;
                    }
                    if !refs.is_empty() {
                        let persistent = group_bytes(pid.to_le_bytes(), 8, &refs);
                        children.extend(group_bytes(pid.to_le_bytes(), 6, &persistent));
                        count += 2;
                    }
                }
                // Exterior cells by block (32x32) and sub-block (8x8). Groups are ordered by their
                // label as xEdit writes them (x, then y, both as unsigned 16-bit), cells by their
                // form id in this plugin.
                let key = |(x, y): (i32, i32)| (x as u16, y as u16);
                let mut blocks: BTreeMap<(u16, u16), ((i32, i32), BTreeMap<(u16, u16), ((i32, i32), Vec<&CellContent>)>)> = BTreeMap::new();
                for c in &w.cells {
                    let b = (c.grid.0.div_euclid(32), c.grid.1.div_euclid(32));
                    let sb = (c.grid.0.div_euclid(8), c.grid.1.div_euclid(8));
                    blocks.entry(key(b)).or_insert_with(|| (b, BTreeMap::new())).1.entry(key(sb)).or_insert_with(|| (sb, Vec::new())).1.push(c);
                }
                for (_, (b, subs)) in blocks {
                    let mut bb = Vec::new();
                    for (_, (sb, mut cells)) in subs {
                        cells.sort_by_key(|c| local(c.cell.form_id));
                        let mut sbb = Vec::new();
                        for c in cells {
                            sbb.extend(rec_bytes(&c.cell));
                            count += 1;
                            if !c.temporary.is_empty() {
                                let cid = local(c.cell.form_id);
                                let mut t = Vec::new();
                                for r in &c.temporary {
                                    t.extend(rec_bytes(r));
                                    count += 1;
                                }
                                let temp = group_bytes(cid.to_le_bytes(), 9, &t);
                                sbb.extend(group_bytes(cid.to_le_bytes(), 6, &temp));
                                count += 2;
                            }
                        }
                        bb.extend(group_bytes(grid_label(sb), 5, &sbb));
                        count += 1;
                    }
                    children.extend(group_bytes(grid_label(b), 4, &bb));
                    count += 1;
                }
                top.extend(group_bytes(wid.to_le_bytes(), 1, &children));
                count += 1;
                fix_ofst(&mut top[world_start..]);
            }
            body.extend(group_bytes(*b"WRLD", 0, &top));
            count += 1;
        }
        // TES4 header.
        let mut h = Vec::new();
        let mut hedr = self.version.to_le_bytes().to_vec();
        hedr.extend_from_slice(&count.to_le_bytes());
        hedr.extend_from_slice(&self.next_object_id.to_le_bytes());
        field_bytes(&mut h, b"HEDR", &hedr);
        if !self.author.is_empty() {
            field_bytes(&mut h, b"CNAM", &zstring(&self.author));
        }
        if !self.description.is_empty() {
            field_bytes(&mut h, b"SNAM", &zstring(&self.description));
        }
        for (k, &i) in masters.iter().enumerate() {
            if k as u32 == companion_pos && let Some(c) = companion {
                field_bytes(&mut h, b"MAST", &zstring(c));
                field_bytes(&mut h, b"DATA", &0u64.to_le_bytes());
            }
            field_bytes(&mut h, b"MAST", &zstring(&lo.plugins[i].plugin.name));
            field_bytes(&mut h, b"DATA", &0u64.to_le_bytes());
        }
        if let Some(c) = companion && companion_pos as usize == masters.len() {
            field_bytes(&mut h, b"MAST", &zstring(c));
            field_bytes(&mut h, b"DATA", &0u64.to_le_bytes());
        }
        if self.esm {
            // Skyrim needs ONAM to apply temporary-reference overrides from a master.
            let overridden: BTreeSet<u32> = self.all_records()
                .filter(|r| r.form_id & 0xFF00_0000 != NEW && r.flags & lodforge_esp::record::FLAG_PERSISTENT == 0)
                .filter(|r| matches!(&r.sig, b"REFR" | b"ACHR" | b"LAND" | b"NAVM" | b"PGRE" | b"PMIS" | b"PARW" | b"PBEA" | b"PFLA" | b"PCON" | b"PBAR" | b"PHZD"))
                .map(|r| local(r.form_id))
                .collect();
            if !overridden.is_empty() {
                let ids: Vec<u8> = overridden.iter().flat_map(|id| id.to_le_bytes()).collect();
                field_bytes(&mut h, b"ONAM", &ids);
            }
        }
        if let Some(n) = self.incc {
            field_bytes(&mut h, b"INCC", &n.to_le_bytes());
        }
        let flags = u32::from(self.esm) | if self.esl { 0x200 } else { 0 };
        let mut out = record_bytes(b"TES4", flags, 0, &h);
        out.extend(body);
        out
    }
}

/// Rewrites a WRLD record's OFST (offset data) for this file, as xEdit does when saving: one u32
/// per cell of the NAM0..NAM9 rectangle (row-major from the south-west cell), the offset of that
/// cell's CELL record from the start of the WRLD record, 0 for cells not in the file. `bytes`
/// starts with the (uncompressed) WRLD record, followed by its children group.
fn fix_ofst(bytes: &mut [u8]) {
    let rd = |b: &[u8], o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    let size = rd(bytes, 4) as usize;
    let (mut nam0, mut nam9, mut ofst) = (None, None, None);
    let mut o = 24;
    while o + 6 <= 24 + size {
        let sig = &bytes[o..o + 4];
        let n = u16::from_le_bytes([bytes[o + 4], bytes[o + 5]]) as usize;
        let f32_at = |b: &[u8], k: usize| f32::from_le_bytes(b[k..k + 4].try_into().unwrap());
        match sig {
            b"NAM0" if n >= 8 => nam0 = Some(((f32_at(bytes, o + 6) / 4096.0).round() as i32, (f32_at(bytes, o + 10) / 4096.0).round() as i32)),
            b"NAM9" if n >= 8 => nam9 = Some(((f32_at(bytes, o + 6) / 4096.0).round() as i32, (f32_at(bytes, o + 10) / 4096.0).round() as i32)),
            b"OFST" => ofst = Some((o + 6, n)),
            _ => {}
        }
        o += 6 + n;
    }
    let (Some((x0, y0)), Some((x1, _)), Some((at, len))) = (nam0, nam9, ofst) else { return };
    let width = (x1 - x0 + 1).max(0) as usize;
    let mut cells: Vec<((i32, i32), usize)> = Vec::new();
    fn walk(b: &[u8], mut o: usize, end: usize, out: &mut Vec<((i32, i32), usize)>) {
        while o + 24 <= end {
            let size = u32::from_le_bytes(b[o + 4..o + 8].try_into().unwrap()) as usize;
            if &b[o..o + 4] == b"GRUP" {
                walk(b, o + 24, o + size, out);
                o += size;
                continue;
            }
            if &b[o..o + 4] == b"CELL" {
                let flags = u32::from_le_bytes(b[o + 8..o + 12].try_into().unwrap());
                let raw = &b[o + 24..o + 24 + size];
                let unpacked;
                let data = if flags & FLAG_COMPRESSED != 0 {
                    let mut d = Vec::new();
                    std::io::Read::read_to_end(&mut flate2::read::ZlibDecoder::new(&raw[4..]), &mut d).ok();
                    unpacked = d;
                    &unpacked[..]
                } else {
                    raw
                };
                let mut f = 0;
                while f + 6 <= data.len() {
                    let n = u16::from_le_bytes([data[f + 4], data[f + 5]]) as usize;
                    if &data[f..f + 4] == b"XCLC" && n >= 8 {
                        let x = i32::from_le_bytes(data[f + 6..f + 10].try_into().unwrap());
                        let y = i32::from_le_bytes(data[f + 10..f + 14].try_into().unwrap());
                        out.push(((x, y), o));
                    }
                    f += 6 + n;
                }
            }
            o += 24 + size;
        }
    }
    walk(bytes, 24 + size, bytes.len(), &mut cells);
    bytes[at..at + len].fill(0);
    for ((x, y), pos) in cells {
        if x < x0 || y < y0 || x > x1 {
            continue;
        }
        let k = ((y - y0) as usize * width + (x - x0) as usize) * 4;
        if k + 4 <= len {
            bytes[at + k..at + k + 4].copy_from_slice(&(pos as u32).to_le_bytes());
        }
    }
}

fn zstring(s: &str) -> Vec<u8> {
    let mut v: Vec<u8> = s.chars().map(|c| if (c as u32) < 256 { c as u8 } else { b'?' }).collect();
    v.push(0);
    v
}

fn grid_label((x, y): (i32, i32)) -> [u8; 4] {
    let mut l = [0u8; 4];
    l[..2].copy_from_slice(&(y as i16).to_le_bytes());
    l[2..].copy_from_slice(&(x as i16).to_le_bytes());
    l
}

fn field_bytes(out: &mut Vec<u8>, sig: &[u8; 4], data: &[u8]) {
    if data.len() > 0xFFFF {
        out.extend_from_slice(b"XXXX");
        out.extend_from_slice(&4u16.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(sig);
        out.extend_from_slice(&0u16.to_le_bytes());
    } else {
        out.extend_from_slice(sig);
        out.extend_from_slice(&(data.len() as u16).to_le_bytes());
    }
    out.extend_from_slice(data);
}

/// Record data as xEdit stores a compressed record: the uncompressed size, then a zlib stream from
/// libdeflate at level 12 (the encoder that reproduces xEdit's bytes; zlib, zlib-ng and zopfli at
/// any setting do not).
fn compress_record(data: &[u8]) -> Vec<u8> {
    let mut c = libdeflater::Compressor::new(libdeflater::CompressionLvl::new(12).unwrap());
    let mut out = vec![0u8; 4 + c.zlib_compress_bound(data.len())];
    out[..4].copy_from_slice(&(data.len() as u32).to_le_bytes());
    let n = c.zlib_compress(data, &mut out[4..]).expect("zlib bound");
    out.truncate(4 + n);
    out
}

fn record_bytes(sig: &[u8; 4], flags: u32, form_id: u32, data: &[u8]) -> Vec<u8> {
    let packed;
    let data = if flags & FLAG_COMPRESSED != 0 {
        packed = compress_record(data);
        &packed[..]
    } else {
        data
    };
    let mut v = Vec::with_capacity(24 + data.len());
    v.extend_from_slice(sig);
    v.extend_from_slice(&(data.len() as u32).to_le_bytes());
    v.extend_from_slice(&flags.to_le_bytes());
    v.extend_from_slice(&form_id.to_le_bytes());
    v.extend_from_slice(&[0; 4]);
    v.extend_from_slice(&44u16.to_le_bytes());
    v.extend_from_slice(&[0; 2]);
    v.extend_from_slice(data);
    v
}

fn group_bytes(label: [u8; 4], group_type: i32, body: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(24 + body.len());
    v.extend_from_slice(b"GRUP");
    v.extend_from_slice(&((body.len() + 24) as u32).to_le_bytes());
    v.extend_from_slice(&label);
    v.extend_from_slice(&group_type.to_le_bytes());
    v.extend_from_slice(&[0; 8]);
    v.extend_from_slice(body);
    v
}

/// `Sig` helper for callers.
pub fn sig(s: &[u8; 4]) -> Sig {
    Sig(*s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn esm_onam_registers_only_temporary_overrides_in_master_id_space() {
        let dir = std::env::temp_dir().join(format!("lodforge-onam-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let empty = LoadOrder::open(&[], |_| None).unwrap();
        for (name, light) in [("Base.esm", false), ("Unused.esm", false), ("Light.esl", true)] {
            let mut source = PluginBuilder::new(0x900);
            source.esm = true;
            source.esl = light;
            std::fs::write(dir.join(name), source.write(&empty)).unwrap();
        }
        let lo = LoadOrder::open(&["Base.esm".into(), "Unused.esm".into(), "Light.esl".into()], |n| Some(dir.join(n))).unwrap();
        let mut pb = PluginBuilder::new(0x900);
        pb.esm = true;
        pb.worlds.push(WorldContent {
            world: Rec::new(b"WRLD", 0, 0x800),
            persistent_cell: Some(Rec::new(b"CELL", 0, 0x801)),
            persistent: vec![Rec::new(b"REFR", lodforge_esp::record::FLAG_PERSISTENT, 0x811)],
            cells: vec![CellContent {
                grid: (0, 0),
                cell: Rec::new(b"CELL", 0, 0x802),
                temporary: vec![
                    Rec::new(b"REFR", 0, 0xFE00_0812),
                    Rec::new(b"REFR", 0, NEW | 0x850),
                    Rec::new(b"REFR", 0, 0x810),
                ],
            }],
        });
        pb.add(Rec::new(b"STAT", 0, 0x803));
        let onam = |bytes: &[u8]| {
            let end = 24 + u32::from_le_bytes(bytes[4..8].try_into().unwrap()) as usize;
            fields(&bytes[24..end]).filter(|f| f.sig.0 == *b"ONAM")
                .flat_map(|f| f.data.chunks_exact(4).map(|id| u32::from_le_bytes(id.try_into().unwrap())))
                .collect::<Vec<_>>()
        };
        assert_eq!(onam(&pb.write(&lo)), vec![0x810, 0x0100_0812]);
        pb.esm = false;
        assert!(onam(&pb.write(&lo)).is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
