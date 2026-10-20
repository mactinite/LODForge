//! Read-only decoding of blocks the main parser keeps as raw bytes: SSE skinned geometry
//! (NiSkinInstance / BSDismemberSkinInstance -> NiSkinPartition with the shared vertex buffer)
//! and the active child of NiSwitchNode / NiLODNode.

use crate::reader::Reader;
use crate::{byte_to_float, f32_le, half_to_f32, u16_le, Block, Nif, Result, TriShape, VertexDesc};

/// Vertex data of a skinned BSTriShape (SSE keeps it in the NiSkinPartition, the shape itself has
/// a data size of 0). Positions are in the shape's own space (bind pose).
#[derive(Debug, Clone, Default)]
pub struct SkinGeometry {
    pub positions: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub normals: Vec<[f32; 3]>,
    pub colors: Vec<[u8; 4]>,
    /// Triangles over the whole vertex buffer (every partition, vertex maps applied).
    pub triangles: Vec<[u16; 3]>,
}

fn partition_geometry(body: &[u8]) -> Result<SkinGeometry> {
    let mut r = Reader::new(body);
    let num_partitions = r.u32()? as usize;
    let data_size = r.u32()? as usize;
    let vertex_size = r.u32()? as usize;
    let desc = VertexDesc(r.u64()?);
    let layout = desc.layout();
    let mut g = SkinGeometry::default();
    if data_size > 0 {
        if vertex_size == 0 || layout.size != vertex_size {
            return Err(crate::Error::VertexLayout { desc: desc.0, computed: layout.size, stride: vertex_size });
        }
        let vdata = r.bytes(data_size)?;
        for v in vdata.chunks_exact(vertex_size) {
            if let Some(o) = layout.position {
                g.positions.push([f32_le(v, o), f32_le(v, o + 4), f32_le(v, o + 8)]);
            }
            if let Some(o) = layout.uv {
                g.uvs.push([half_to_f32(u16_le(v, o)), half_to_f32(u16_le(v, o + 2))]);
            }
            if let Some(o) = layout.normal {
                g.normals.push([byte_to_float(v[o]), byte_to_float(v[o + 1]), byte_to_float(v[o + 2])]);
            }
            if let Some(o) = layout.color {
                g.colors.push([v[o], v[o + 1], v[o + 2], v[o + 3]]);
            }
        }
    }
    let n = g.positions.len();
    for _ in 0..num_partitions {
        let nv = r.u16()? as usize;
        let nt = r.u16()? as usize;
        let nb = r.u16()? as usize;
        let ns = r.u16()? as usize;
        let nw = r.u16()? as usize;
        r.skip(nb * 2)?;
        let map: Vec<u16> = if r.u8()? != 0 { (0..nv).map(|_| r.u16()).collect::<Result<_>>()? } else { (0..nv as u16).collect() };
        if r.u8()? != 0 {
            r.skip(nv * nw * 4)?;
        }
        let strip_lengths = (0..ns).map(|_| r.u16().map(usize::from)).collect::<Result<Vec<_>>>()?;
        let mut local: Vec<[u16; 3]> = Vec::new();
        if r.u8()? != 0 {
            if ns != 0 {
                for len in strip_lengths {
                    let p = (0..len).map(|_| r.u16()).collect::<Result<Vec<_>>>()?;
                    for k in 2..p.len() {
                        let t = if k % 2 == 0 { [p[k - 2], p[k - 1], p[k]] } else { [p[k - 2], p[k], p[k - 1]] };
                        if t[0] != t[1] && t[1] != t[2] && t[0] != t[2] {
                            local.push(t);
                        }
                    }
                }
            } else {
                local = (0..nt).map(|_| Ok([r.u16()?, r.u16()?, r.u16()?])).collect::<Result<_>>()?;
            }
        }
        if r.u8()? != 0 {
            r.skip(nv * nw)?;
        }
        // SSE: LOD level u8, global VB bool, vertex desc u64, triangles over the global buffer.
        let _lod_level = r.u8()?;
        let _global_vb = r.u8()?;
        let _desc = r.u64()?;
        let copy = (0..nt).map(|_| Ok([r.u16()?, r.u16()?, r.u16()?])).collect::<Result<Vec<_>>>()?;
        if copy.len() == nt && copy.iter().flatten().all(|&i| (i as usize) < n) {
            g.triangles.extend(copy);
        } else {
            g.triangles.extend(local.iter().filter_map(|t| {
                let m = [*map.get(t[0] as usize)?, *map.get(t[1] as usize)?, *map.get(t[2] as usize)?];
                m.iter().all(|&i| (i as usize) < n).then_some(m)
            }));
        }
    }
    Ok(g)
}

impl Nif {
    /// The vertex data of a skinned SSE BSTriShape, read from its NiSkinPartition. `None` when the
    /// shape has no skin instance or the partition carries no vertex buffer.
    pub fn skin_geometry(&self, shape: &TriShape) -> Option<SkinGeometry> {
        let skin = shape.skin? as usize;
        let ty = self.block_type(skin);
        if ty != "NiSkinInstance" && ty != "BSDismemberSkinInstance" {
            return None;
        }
        let mut r = Reader::new(self.raw_blocks.get(&skin)?);
        let _data = r.i32().ok()?;
        let partition = usize::try_from(r.i32().ok()?).ok()?;
        if self.block_type(partition) != "NiSkinPartition" {
            return None;
        }
        let g = partition_geometry(self.raw_blocks.get(&partition)?).ok()?;
        (!g.positions.is_empty()).then_some(g)
    }

    /// Index of the active child of a NiSwitchNode or NiLODNode (`None` for other blocks).
    pub fn switch_index(&self, index: usize) -> Option<u32> {
        let body = self.raw_blocks.get(&index)?;
        let end = match self.block_type(index) {
            "NiSwitchNode" => body.len(),
            "NiLODNode" => body.len().checked_sub(4)?,
            _ => return None,
        };
        let b = body.get(end.checked_sub(4)?..end)?;
        matches!(self.blocks.get(index), Some(Block::Node(_))).then(|| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
}
