//! `<World>[.<SEASON>].statics.bin` (docs/runtime/instance-formats.md section 6).

use std::collections::BTreeMap;

use half::f16;

use crate::container::{Buf, Chunk, Common, Container, Cur, Kind, RawSection, WriteOptions, write};
use crate::hash::{angle_rad, angle_u16, position_hash, ref_hash};
use crate::{Error, Result};

pub const EXT: &str = "statics.bin";

pub const MESH_TREE: u32 = 1;
pub const MESH_PASSTHRU: u32 = 2;
pub const MESH_VERTEX_ALPHA: u32 = 4;

/// One `MESH` entry.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Mesh {
    pub bounds: [f32; 6],
    pub flags: u32,
    pub triangles: u32,
    pub model: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StaticRecord {
    pub pos: [f32; 3],
    pub angles: [u16; 3],
    pub scale: f16,
    pub mesh: u16,
    pub plugin: u16,
    pub form: u32,
    pub hash: u32,
}

impl StaticRecord {
    pub fn angles_rad(&self) -> [f32; 3] {
        self.angles.map(angle_rad)
    }

    fn write(&self, b: &mut Buf) {
        for v in self.pos {
            b.f32(v);
        }
        for a in self.angles {
            b.u16(a);
        }
        b.u16(self.scale.to_bits());
        b.u16(self.mesh);
        b.u16(self.plugin);
        b.u32(self.form);
        b.u32(self.hash);
    }

    fn read(r: &mut Cur<'_>) -> Result<StaticRecord> {
        Ok(StaticRecord { pos: [r.f32()?, r.f32()?, r.f32()?], angles: [r.u16()?, r.u16()?, r.u16()?], scale: f16::from_bits(r.u16()?), mesh: r.u16()?, plugin: r.u16()?, form: r.u32()?, hash: r.u32()? })
    }
}

/// Plugin index of a synthetic reference.
pub const NO_PLUGIN: u16 = 0xFFFF;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct StaticsFile {
    pub common: Common,
    pub meshes: Vec<Mesh>,
    /// Keyed by (level, block x, block y).
    pub blocks: BTreeMap<(u8, i16, i16), Vec<StaticRecord>>,
    mesh_index: std::collections::HashMap<String, u16>,
}

impl StaticsFile {
    pub fn new(common: Common) -> StaticsFile {
        StaticsFile { common, ..Default::default() }
    }

    /// The `MESH` index of `model`, added with `make` when new.
    pub fn mesh(&mut self, model: &str, make: impl FnOnce() -> Mesh) -> u16 {
        let key = model.to_ascii_lowercase();
        if let Some(&i) = self.mesh_index.get(&key) {
            return i;
        }
        let mut m = make();
        m.model = key.clone();
        self.meshes.push(m);
        let i = (self.meshes.len() - 1) as u16;
        self.mesh_index.insert(key, i);
        i
    }

    /// Adds an instance. `angles` in radians as the plugin stores them; `plugin` `None` for a
    /// synthetic reference.
    #[allow(clippy::too_many_arguments)]
    pub fn add(&mut self, level: u8, block: (i16, i16), pos: [f32; 3], angles: [f32; 3], scale: f32, mesh: u16, plugin: Option<&str>, form: u32) {
        let (p, hash) = match plugin {
            Some(name) => (self.common.plugin_index(name), ref_hash(name, form)),
            None => (NO_PLUGIN, position_hash(pos)),
        };
        let rec = StaticRecord { pos, angles: angles.map(angle_u16), scale: f16::from_f32(scale), mesh, plugin: p, form, hash };
        self.blocks.entry((level, block.0, block.1)).or_default().push(rec);
    }

    pub fn records(&self) -> usize {
        self.blocks.values().map(Vec::len).sum()
    }

    fn mesh_chunk(&self) -> Chunk {
        let mut b = Buf::default();
        b.u32(self.meshes.len() as u32);
        for m in &self.meshes {
            for v in m.bounds {
                b.f32(v);
            }
            b.u32(m.flags);
            b.u32(m.triangles);
            b.str(&m.model);
        }
        Chunk { tag: *b"MESH", data: b.0 }
    }

    pub fn to_bytes(&self, opts: WriteOptions) -> Result<Vec<u8>> {
        let mut chunks = self.common.chunks();
        chunks.push(self.mesh_chunk());
        let sections = self
            .blocks
            .iter()
            .map(|(&(level, x, y), recs)| {
                let mut sorted = recs.clone();
                sorted.sort_by_key(|r| (r.mesh, r.hash, r.plugin, r.form));
                let mut b = Buf(Vec::with_capacity(sorted.len() * 32));
                for r in &sorted {
                    r.write(&mut b);
                }
                RawSection { level, x, y, data: b.0, records: sorted.len() as u32 }
            })
            .collect();
        write(Kind::Statics, &chunks, sections, opts)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<StaticsFile> {
        let c = Container::parse(bytes, Kind::Statics)?;
        c.verify()?;
        let common = c.common()?;
        let mut f = StaticsFile { common, ..Default::default() };
        if let Some(ch) = c.chunk(b"MESH") {
            let mut r = Cur::new(&ch.data);
            let n = r.u32()?;
            for _ in 0..n {
                let mut bounds = [0f32; 6];
                for v in &mut bounds {
                    *v = r.f32()?;
                }
                let m = Mesh { bounds, flags: r.u32()?, triangles: r.u32()?, model: r.str()? };
                f.mesh_index.insert(m.model.clone(), f.meshes.len() as u16);
                f.meshes.push(m);
            }
        }
        let stride = c.header.record_size as usize;
        for e in &c.index {
            let raw = c.payload(e)?;
            let mut v = Vec::with_capacity(e.record_count as usize);
            for i in 0..e.record_count as usize {
                let mut r = Cur::new(raw.get(i * stride..(i + 1) * stride).ok_or(Error::Truncated)?);
                v.push(StaticRecord::read(&mut r)?);
            }
            f.blocks.insert((e.level, e.x, e.y), v);
        }
        Ok(f)
    }

    /// Sorts every block as the writer stores it (mesh, then hash).
    pub fn finish(&mut self) {
        for v in self.blocks.values_mut() {
            v.sort_by_key(|r| (r.mesh, r.hash, r.plugin, r.form));
        }
    }
}
