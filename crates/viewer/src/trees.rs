// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! Classic tree LOD: `<World>.lst` (billboard types with their atlas rectangles) and
//! `<World>.4.x.y.btt` (references per LOD4 block), as `lodforge-treelod` writes them.

use serde::Serialize;

/// LST entry (32 bytes): type index, billboard width and height (game units at scale 1), atlas
/// UV rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct TreeType {
    pub index: i32,
    pub width: f32,
    pub height: f32,
    pub uv_min: [f32; 2],
    pub uv_max: [f32; 2],
}

/// BTT reference (32 bytes): position, Z rotation (radians), scale, form id.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct TreeRef {
    pub pos: [f32; 3],
    pub rotation: f32,
    pub scale: f32,
    pub form_id: u32,
    pub type_index: i32,
}

fn f32_at(b: &[u8], o: usize) -> Option<f32> {
    Some(f32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

fn i32_at(b: &[u8], o: usize) -> Option<i32> {
    Some(i32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

/// Parses a `.lst`; `None` when it is not laid out as expected.
pub fn parse_lst(b: &[u8]) -> Option<Vec<TreeType>> {
    let n = usize::try_from(i32_at(b, 0)?).ok()?;
    if b.len() < 4 + n * 32 {
        return None;
    }
    (0..n)
        .map(|i| {
            let o = 4 + i * 32;
            Some(TreeType {
                index: i32_at(b, o)?,
                width: f32_at(b, o + 4)?,
                height: f32_at(b, o + 8)?,
                uv_min: [f32_at(b, o + 12)?, f32_at(b, o + 16)?],
                uv_max: [f32_at(b, o + 20)?, f32_at(b, o + 24)?],
            })
        })
        .collect()
}

/// Parses a `.btt`: every reference with its type index.
pub fn parse_btt(b: &[u8]) -> Option<Vec<TreeRef>> {
    let types = usize::try_from(i32_at(b, 0)?).ok()?;
    let mut o = 4;
    let mut out = Vec::new();
    for _ in 0..types {
        let index = i32_at(b, o)?;
        let count = usize::try_from(i32_at(b, o + 4)?).ok()?;
        o += 8;
        if b.len() < o + count * 32 {
            return None;
        }
        for _ in 0..count {
            out.push(TreeRef {
                pos: [f32_at(b, o)?, f32_at(b, o + 4)?, f32_at(b, o + 8)?],
                rotation: f32_at(b, o + 12)?,
                scale: f32_at(b, o + 16)?,
                form_id: u32::from_le_bytes(b[o + 20..o + 24].try_into().ok()?),
                type_index: index,
            });
            o += 32;
        }
    }
    Some(out)
}

/// One billboard instance for the GPU (the quad is built in the vertex shader).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct TreeInstance {
    pub pos: [f32; 3],
    pub rotation: f32,
    /// Width and height in game units (type size times reference scale).
    pub size: [f32; 2],
    pub uv_min: [f32; 2],
    pub uv_max: [f32; 2],
    /// Highlight / flags (bit 0 selected, bit 1 type unknown).
    pub flags: u32,
    pub _pad: u32,
}

/// Instances of a block's references; references whose type is not in the list get a unit
/// billboard with flag bit 1 and are counted in the second return value.
pub fn instances(refs: &[TreeRef], types: &[TreeType]) -> (Vec<TreeInstance>, usize) {
    let mut unknown = 0;
    let v = refs
        .iter()
        .map(|r| match types.iter().find(|t| t.index == r.type_index) {
            Some(t) => TreeInstance {
                pos: r.pos,
                rotation: r.rotation,
                size: [t.width * r.scale, t.height * r.scale],
                uv_min: t.uv_min,
                uv_max: t.uv_max,
                flags: 0,
                _pad: 0,
            },
            None => {
                unknown += 1;
                TreeInstance {
                    pos: r.pos,
                    rotation: r.rotation,
                    size: [256.0 * r.scale, 512.0 * r.scale],
                    uv_min: [0.0, 0.0],
                    uv_max: [1.0, 1.0],
                    flags: 2,
                    _pad: 0,
                }
            }
        })
        .collect();
    (v, unknown)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lst_bytes(types: &[TreeType]) -> Vec<u8> {
        let mut o = Vec::new();
        o.extend_from_slice(&(types.len() as i32).to_le_bytes());
        for t in types {
            o.extend_from_slice(&t.index.to_le_bytes());
            for v in [
                t.width,
                t.height,
                t.uv_min[0],
                t.uv_min[1],
                t.uv_max[0],
                t.uv_max[1],
            ] {
                o.extend_from_slice(&v.to_le_bytes());
            }
            o.extend_from_slice(&0i32.to_le_bytes());
        }
        o
    }

    fn btt_bytes(blocks: &[(i32, Vec<TreeRef>)]) -> Vec<u8> {
        let mut o = Vec::new();
        o.extend_from_slice(&(blocks.len() as i32).to_le_bytes());
        for (index, refs) in blocks {
            o.extend_from_slice(&index.to_le_bytes());
            o.extend_from_slice(&(refs.len() as i32).to_le_bytes());
            for r in refs {
                for v in [r.pos[0], r.pos[1], r.pos[2], r.rotation, r.scale] {
                    o.extend_from_slice(&v.to_le_bytes());
                }
                o.extend_from_slice(&r.form_id.to_le_bytes());
                o.extend_from_slice(&0i32.to_le_bytes());
                o.extend_from_slice(&0i32.to_le_bytes());
            }
        }
        o
    }

    #[test]
    fn round_trips_treelod_layout() {
        let types = vec![TreeType {
            index: 3,
            width: 300.0,
            height: 600.0,
            uv_min: [0.0, 0.5],
            uv_max: [0.25, 1.0],
        }];
        assert_eq!(parse_lst(&lst_bytes(&types)).unwrap(), types);
        let r = TreeRef {
            pos: [1.0, 2.0, 3.0],
            rotation: 0.5,
            scale: 1.2,
            form_id: 0x1234,
            type_index: 3,
        };
        let refs = parse_btt(&btt_bytes(&[
            (3, vec![r]),
            (7, vec![TreeRef { type_index: 7, ..r }]),
        ]))
        .unwrap();
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0], r);
        let (inst, unknown) = instances(&refs, &types);
        assert_eq!(unknown, 1);
        assert!((inst[0].size[1] - 720.0).abs() < 1e-3);
        assert_eq!(inst[1].flags, 2);
        assert!(parse_lst(&[1, 0, 0, 0]).is_none());
    }
}
