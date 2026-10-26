//! `<World>[.<SEASON>].trees.bin` (docs/runtime/instance-formats.md section 5).

use std::collections::BTreeMap;

use half::f16;

use crate::container::{Buf, Chunk, Common, Container, Cur, Kind, RawSection, WriteOptions, write};
use crate::hash::{angle_rad, angle_u16, cell_of, dequantize_rel, quantize_rel, ref_hash};
use crate::{Error, Result};

pub const EXT: &str = "trees.bin";

/// One tree type (`TTYP` entry): an impostor type of the worldspace's impostor set, at the same
/// index as in its `impostors.json`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TreeType {
    /// Some base of this type is a `TREE` record (else all are `STAT`s with the has-tree-LOD flag).
    pub tree_record: bool,
    /// CRC-32 of the rendered model file.
    pub model_crc32: u32,
    /// `OBND` of the first base in this file (zero when no base of the type is used here).
    pub bounds: [i16; 6],
    pub references: u32,
    /// The impostor type name (`impostors.json` `types[i].name`): the join key.
    pub name: String,
    /// Rendered model (`impostors.json` `model`).
    pub model: String,
    /// LOD4 column the object LOD would have used for the first reference.
    pub lod4: String,
    /// Bases drawn with this type in this file: (`PLUG` index, local form id).
    pub bases: Vec<(u16, u32)>,
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
    /// `TMSH`: the near-tier LOD mesh per type (same order as `types`; empty: no chunk).
    pub meshes: Vec<TreeMesh>,
    /// `TMPK`: per type, its mesh index in `TreeMeshes.bin` (`meshpack::NO_MESH` for none); empty: no chunk.
    pub pack_meshes: Vec<u32>,
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
            b.u32(t.model_crc32);
            for v in t.bounds {
                b.i16(v);
            }
            b.u32(t.references);
            b.u16(t.bases.len() as u16);
            b.u16(u16::from(t.tree_record));
            b.str(&t.name);
            b.str(&t.model);
            b.str(&t.lod4);
            for &(p, f) in &t.bases {
                b.u16(p);
                b.u32(f);
            }
        }
        Chunk { tag: *b"TTYP", data: b.0 }
    }

    pub fn to_bytes(&self, opts: WriteOptions) -> Result<Vec<u8>> {
        let mut chunks = self.common.chunks();
        chunks.push(self.type_chunk());
        if let Some(m) = self.mesh_chunk() {
            chunks.push(m);
        }
        if !self.pack_meshes.is_empty() {
            let mut b = Buf::default();
            b.u32(self.pack_meshes.len() as u32);
            for &m in &self.pack_meshes {
                b.u32(m);
            }
            chunks.push(Chunk { tag: *b"TMPK", data: b.0 });
        }
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
                let model_crc32 = r.u32()?;
                let mut bounds = [0i16; 6];
                for v in &mut bounds {
                    *v = r.i16()?;
                }
                let references = r.u32()?;
                let nb = usize::from(r.u16()?);
                let flags = r.u16()?;
                let (name, model, lod4) = (r.str()?, r.str()?, r.str()?);
                let bases = (0..nb).map(|_| Ok((r.u16()?, r.u32()?))).collect::<Result<Vec<_>>>()?;
                types.push(TreeType { tree_record: flags & 1 != 0, model_crc32, bounds, references, name, model, lod4, bases });
            }
        }
        let pack_meshes = match c.chunk(b"TMPK") {
            Some(ch) => {
                let mut r = Cur::new(&ch.data);
                let n = r.u32()?;
                (0..n).map(|_| r.u32()).collect::<Result<Vec<_>>>()?
            }
            None => Vec::new(),
        };
        let meshes = match c.chunk(b"TMSH") {
            Some(ch) => Self::read_meshes(&ch.data)?,
            None => Vec::new(),
        };
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
        Ok(TreeFile { common, types, meshes, pack_meshes, cells })
    }
}

/// One shape of a tree type's LOD mesh (`TMSH`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MeshShape {
    pub name: String,
    /// Diffuse and normal map, Data-relative, lower case, resolved against the load order
    /// (`""` when the shape has none or it could not be found).
    pub diffuse: String,
    pub normal: String,
    /// `NiAlphaProperty` flags and threshold (`has_alpha` false: no alpha property).
    pub has_alpha: bool,
    pub alpha_flags: u16,
    pub alpha_threshold: u8,
    pub vertex_colors: bool,
    pub triangles: u32,
}

/// The near-tier mesh of one tree type (`TMSH` entry, same index as `TTYP`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TreeMesh {
    /// LOD4 3D model, Data-relative, lower case; `""` when the type has none (billboard or no
    /// LOD4 in object LOD).
    pub model: String,
    /// Model-space AABB of all shapes (min xyz, max xyz), node transforms applied.
    pub bounds: [f32; 6],
    pub triangles: u32,
    /// The model came from LODForge's generated tree models (`meshes\lod\lodforge\...`).
    pub generated: bool,
    pub passthru: bool,
    pub shapes: Vec<MeshShape>,
}

impl TreeFile {
    fn mesh_chunk(&self) -> Option<Chunk> {
        if self.meshes.is_empty() {
            return None;
        }
        let mut b = Buf::default();
        b.u32(self.meshes.len() as u32);
        for m in &self.meshes {
            let flags = u16::from(!m.model.is_empty()) | (u16::from(m.generated) << 1) | (u16::from(m.passthru) << 2);
            b.u16(flags);
            b.u16(m.shapes.len() as u16);
            b.u32(m.triangles);
            for v in m.bounds {
                b.f32(v);
            }
            b.str(&m.model);
            for s in &m.shapes {
                b.u16(s.alpha_flags);
                b.u8(s.alpha_threshold);
                b.u8(u8::from(s.has_alpha) | (u8::from(s.vertex_colors) << 1));
                b.u32(s.triangles);
                b.str(&s.name);
                b.str(&s.diffuse);
                b.str(&s.normal);
            }
        }
        Some(Chunk { tag: *b"TMSH", data: b.0 })
    }

    fn read_meshes(data: &[u8]) -> Result<Vec<TreeMesh>> {
        let mut r = Cur::new(data);
        let n = r.u32()?;
        let mut out = Vec::with_capacity(n as usize);
        for _ in 0..n {
            let flags = r.u16()?;
            let ns = r.u16()?;
            let triangles = r.u32()?;
            let mut bounds = [0f32; 6];
            for v in &mut bounds {
                *v = r.f32()?;
            }
            let model = r.str()?;
            let mut shapes = Vec::with_capacity(usize::from(ns));
            for _ in 0..ns {
                let alpha_flags = r.u16()?;
                let alpha_threshold = r.u8()?;
                let f = r.u8()?;
                let triangles = r.u32()?;
                shapes.push(MeshShape { has_alpha: f & 1 != 0, vertex_colors: f & 2 != 0, alpha_flags, alpha_threshold, triangles, name: r.str()?, diffuse: r.str()?, normal: r.str()? });
            }
            out.push(TreeMesh { model, bounds, triangles, generated: flags & 2 != 0, passthru: flags & 4 != 0, shapes });
        }
        Ok(out)
    }
}
