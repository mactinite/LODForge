//! `<World>[.<SEASON>].trees.bin` (docs/runtime/instance-formats.md section 5).

use std::collections::BTreeMap;

use half::f16;

use crate::container::{Buf, Chunk, Common, Container, Cur, Kind, RawSection, WriteOptions, write};
use crate::hash::{angle_rad, angle_u16, cell_of, dequantize_rel, quantize_rel, ref_hash};
use crate::{Error, Result};

pub const EXT: &str = "trees.bin";

/// One tree type (`TTYP` entry).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TreeType {
    pub base_plugin: u16,
    /// The base is a `TREE` record (else a `STAT` with the has-tree-LOD flag).
    pub tree_record: bool,
    pub base_form: u32,
    pub model_crc32: u32,
    pub bounds: [i16; 6],
    pub references: u32,
    pub model: String,
    pub lod4: String,
}

/// One record as stored.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TreeRecord {
    pub qx: u16,
    pub qy: u16,
    pub z: f32,
    pub yaw: u16,
    pub scale: f16,
    pub type_: u16,
    pub plugin: u16,
    pub form: u32,
    pub hash: u32,
}

impl TreeRecord {
    pub fn position(&self, cell: (i16, i16)) -> [f32; 3] {
        [dequantize_rel(self.qx, i32::from(cell.0)), dequantize_rel(self.qy, i32::from(cell.1)), self.z]
    }

    pub fn yaw_rad(&self) -> f32 {
        angle_rad(self.yaw)
    }

    fn write(&self, b: &mut Buf) {
        b.u16(self.qx);
        b.u16(self.qy);
        b.f32(self.z);
        b.u16(self.yaw);
        b.u16(self.scale.to_bits());
        b.u16(self.type_);
        b.u16(self.plugin);
        b.u32(self.form);
        b.u32(self.hash);
    }

    fn read(r: &mut Cur<'_>) -> Result<TreeRecord> {
        Ok(TreeRecord { qx: r.u16()?, qy: r.u16()?, z: r.f32()?, yaw: r.u16()?, scale: f16::from_bits(r.u16()?), type_: r.u16()?, plugin: r.u16()?, form: r.u32()?, hash: r.u32()? })
    }
}

/// A whole trees file.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TreeFile {
    pub common: Common,
    pub types: Vec<TreeType>,
    /// Per cell, sorted by hash once [`TreeFile::finish`] ran (the writer sorts too).
    pub cells: BTreeMap<(i16, i16), Vec<TreeRecord>>,
}

impl TreeFile {
    /// Adds a tree reference: world position, z rotation (radians, as the plugin stores it),
    /// scale, type, the defining plugin's file name and the local form id.
    pub fn add(&mut self, pos: [f32; 3], yaw: f32, scale: f32, type_: u16, plugin: &str, form: u32) -> Result<()> {
        let (cx, cy) = (cell_of(pos[0]), cell_of(pos[1]));
        let cell = (i16::try_from(cx).map_err(|_| Error::Invalid("cell out of range"))?, i16::try_from(cy).map_err(|_| Error::Invalid("cell out of range"))?);
        let p = self.common.plugin_index(plugin);
        let rec = TreeRecord { qx: quantize_rel(pos[0], cx), qy: quantize_rel(pos[1], cy), z: pos[2], yaw: angle_u16(yaw), scale: f16::from_f32(scale), type_, plugin: p, form, hash: ref_hash(plugin, form) };
        if let Some(t) = self.types.get_mut(usize::from(type_)) {
            t.references += 1;
        }
        self.cells.entry(cell).or_default().push(rec);
        Ok(())
    }

    /// Sorts every cell by hash (form as the tie-break).
    pub fn finish(&mut self) {
        for v in self.cells.values_mut() {
            v.sort_by_key(|r| (r.hash, r.plugin, r.form));
        }
    }

    pub fn records(&self) -> usize {
        self.cells.values().map(Vec::len).sum()
    }

    fn type_chunk(&self) -> Chunk {
        let mut b = Buf::default();
        b.u32(self.types.len() as u32);
        for t in &self.types {
            b.u16(t.base_plugin);
            b.u16(u16::from(t.tree_record));
            b.u32(t.base_form);
            b.u32(t.model_crc32);
            for v in t.bounds {
                b.i16(v);
            }
            b.u32(t.references);
            b.str(&t.model);
            b.str(&t.lod4);
        }
        Chunk { tag: *b"TTYP", data: b.0 }
    }

    pub fn to_bytes(&self, opts: WriteOptions) -> Result<Vec<u8>> {
        let mut chunks = self.common.chunks();
        chunks.push(self.type_chunk());
        let sections = self
            .cells
            .iter()
            .map(|(&(x, y), recs)| {
                let mut sorted = recs.clone();
                sorted.sort_by_key(|r| (r.hash, r.plugin, r.form));
                let mut b = Buf(Vec::with_capacity(sorted.len() * 24));
                for r in &sorted {
                    r.write(&mut b);
                }
                RawSection { level: 0, x, y, data: b.0, records: sorted.len() as u32 }
            })
            .collect();
        write(Kind::Trees, &chunks, sections, opts)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<TreeFile> {
        let c = Container::parse(bytes, Kind::Trees)?;
        c.verify()?;
        let common = c.common()?;
        let mut types = Vec::new();
        if let Some(ch) = c.chunk(b"TTYP") {
            let mut r = Cur::new(&ch.data);
            let n = r.u32()?;
            for _ in 0..n {
                let base_plugin = r.u16()?;
                let flags = r.u16()?;
                let base_form = r.u32()?;
                let model_crc32 = r.u32()?;
                let mut bounds = [0i16; 6];
                for v in &mut bounds {
                    *v = r.i16()?;
                }
                types.push(TreeType { base_plugin, tree_record: flags & 1 != 0, base_form, model_crc32, bounds, references: r.u32()?, model: r.str()?, lod4: r.str()? });
            }
        }
        let stride = c.header.record_size as usize;
        let mut cells = BTreeMap::new();
        for e in &c.index {
            let raw = c.payload(e)?;
            let mut v = Vec::with_capacity(e.record_count as usize);
            for i in 0..e.record_count as usize {
                let mut r = Cur::new(raw.get(i * stride..(i + 1) * stride).ok_or(Error::Truncated)?);
                v.push(TreeRecord::read(&mut r)?);
            }
            cells.insert((e.x, e.y), v);
        }
        Ok(TreeFile { common, types, cells })
    }
}
