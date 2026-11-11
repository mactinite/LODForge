// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! Animation blocks, read only: the controller sequences of a model's NiControllerManager and the
//! pose each one ends in, so a LOD model can show an animated object in a chosen state (a door or
//! gate the reference flags open by default).
//!
//! The main parser keeps these blocks as [`crate::Block::Unknown`] with their bytes in
//! [`Nif::raw_blocks`]; this module decodes those bytes (Skyrim SE layout, 20.2.0.7 / BS 100):
//! NiControllerManager -> NiControllerSequence (name, controlled blocks: node name + interpolator)
//! -> NiTransformInterpolator (base transform, data) -> NiTransformData (rotation keys as a
//! quaternion or as XYZ Euler channels, translation keys, scale keys). Only transform
//! interpolators are read; other kinds (B-spline, float, visibility) leave the node as it is.

use crate::reader::Reader;
use crate::{Nif, Result};

/// Transform components a sequence leaves a node with when it ends: the last key of each component
/// that has keys, else the interpolator's own value when it is defined, else `None` (the node's
/// own value).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct EndPose {
    /// Controlled node (or shape) name as the sequence names it.
    pub node: String,
    pub translation: Option<[f32; 3]>,
    /// Row-major rotation, `v' = R v`.
    pub rotation: Option<[[f32; 3]; 3]>,
    pub scale: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Sequence {
    pub name: String,
    pub start: f32,
    pub stop: f32,
    pub channels: Vec<EndPose>,
}

/// Bethesda's marker for an undefined interpolator component (-FLT_MAX).
fn defined(v: f32) -> bool {
    v.is_finite() && v > -1.0e30 && v < 1.0e30
}

/// The sequences of every NiControllerManager in the file (normally one, on the root), in file
/// order. Blocks that do not decode are skipped.
pub fn sequences(nif: &Nif) -> Vec<Sequence> {
    let mut out = Vec::new();
    for i in 0..nif.blocks.len() {
        if nif.block_type(i) != "NiControllerManager" {
            continue;
        }
        let Some(body) = nif.raw_blocks.get(&i) else { continue };
        let Ok(seq_refs) = manager(body) else { continue };
        for r in seq_refs {
            let Some(idx) = r else { continue };
            if nif.block_type(idx as usize) != "NiControllerSequence" {
                continue;
            }
            if let Some(body) = nif.raw_blocks.get(&(idx as usize))
                && let Ok(s) = sequence(nif, body)
            {
                out.push(s);
            }
        }
    }
    out
}

fn string(r: &mut Reader<'_>, nif: &Nif) -> Result<String> {
    let i = r.i32()?;
    Ok(if i >= 0 { nif.header.strings.get(i as usize).cloned().unwrap_or_default() } else { String::new() })
}

fn block_ref(r: &mut Reader<'_>) -> Result<Option<u32>> {
    let i = r.i32()?;
    Ok((i >= 0).then_some(i as u32))
}

/// NiControllerManager: the sequence references.
fn manager(body: &[u8]) -> Result<Vec<Option<u32>>> {
    let mut r = Reader::new(body);
    // NiTimeController: next, flags, frequency, phase, start, stop, target.
    r.skip(4 + 2 + 4 + 4 + 4 + 4 + 4)?;
    let _cumulative = r.u8()?;
    let n = r.u32()? as usize;
    if n > body.len() / 4 {
        return Err(crate::Error::Truncated { offset: r.pos(), what: "controller sequences" });
    }
    (0..n).map(|_| block_ref(&mut r)).collect()
}

/// NiControllerSequence: name, times and the end pose of each transform channel.
fn sequence(nif: &Nif, body: &[u8]) -> Result<Sequence> {
    let mut r = Reader::new(body);
    let name = string(&mut r, nif)?;
    let n = r.u32()? as usize;
    let _grow = r.u32()?;
    if n > body.len() / 29 {
        return Err(crate::Error::Truncated { offset: r.pos(), what: "controlled blocks" });
    }
    let mut blocks = Vec::with_capacity(n);
    for _ in 0..n {
        let interpolator = block_ref(&mut r)?;
        let _controller = block_ref(&mut r)?;
        let _priority = r.u8()?;
        let node = string(&mut r, nif)?;
        // Property type, controller type, controller id, interpolator id.
        r.skip(16)?;
        blocks.push((interpolator, node));
    }
    let _weight = r.f32()?;
    let _text_keys = block_ref(&mut r)?;
    let _cycle = r.u32()?;
    let _frequency = r.f32()?;
    let start = r.f32()?;
    let stop = r.f32()?;
    let mut channels = Vec::new();
    for (interp, node) in blocks {
        let Some(ii) = interp else { continue };
        if nif.block_type(ii as usize) != "NiTransformInterpolator" {
            continue;
        }
        let Some(ib) = nif.raw_blocks.get(&(ii as usize)) else { continue };
        if let Ok(mut pose) = transform_interpolator(nif, ib) {
            pose.node = node;
            channels.push(pose);
        }
    }
    Ok(Sequence { name, start, stop, channels })
}

/// NiTransformInterpolator: the base transform (components left undefined by Bethesda's exporter)
/// and the data block's last keys.
fn transform_interpolator(nif: &Nif, body: &[u8]) -> Result<EndPose> {
    let mut r = Reader::new(body);
    let t = r.vec3()?;
    let q = [r.f32()?, r.f32()?, r.f32()?, r.f32()?];
    let s = r.f32()?;
    let data = block_ref(&mut r)?;
    let mut pose = EndPose {
        node: String::new(),
        translation: t.iter().all(|v| defined(*v)).then_some(t),
        rotation: q.iter().all(|v| defined(*v)).then(|| quat_matrix(q)),
        scale: defined(s).then_some(s),
    };
    if let Some(di) = data
        && nif.block_type(di as usize) == "NiTransformData"
        && let Some(db) = nif.raw_blocks.get(&(di as usize))
    {
        let (rot, trans, scale) = transform_data(db)?;
        if rot.is_some() {
            pose.rotation = rot;
        }
        if trans.is_some() {
            pose.translation = trans;
        }
        if scale.is_some() {
            pose.scale = scale;
        }
    }
    Ok(pose)
}

const KEY_LINEAR: u32 = 1;
const KEY_QUADRATIC: u32 = 2;
const KEY_TBC: u32 = 3;
const KEY_XYZ_ROTATION: u32 = 4;
const KEY_CONST: u32 = 5;

/// Extra floats per key beyond time and value for a key type with `dim` value components.
fn key_extra(kind: u32, dim: usize) -> Result<usize> {
    Ok(match kind {
        KEY_LINEAR | KEY_CONST => 0,
        KEY_QUADRATIC => 2 * dim,
        KEY_TBC => 3,
        _ => return Err(crate::Error::Truncated { offset: 0, what: "key type" }),
    })
}

/// The last key's value of a key group of `dim` floats (`None` for an empty group).
fn last_key(r: &mut Reader<'_>, dim: usize, total: usize) -> Result<Option<Vec<f32>>> {
    let n = r.u32()? as usize;
    if n == 0 {
        return Ok(None);
    }
    let kind = r.u32()?;
    let extra = key_extra(kind, dim)?;
    let per = 4 * (1 + dim + extra);
    if n > total / per {
        return Err(crate::Error::Truncated { offset: r.pos(), what: "keys" });
    }
    r.skip(per * (n - 1))?;
    let _time = r.f32()?;
    let v = (0..dim).map(|_| r.f32()).collect::<Result<Vec<_>>>()?;
    r.skip(4 * extra)?;
    Ok(Some(v))
}

/// NiTransformData: (rotation, translation, scale) at the last keys.
#[allow(clippy::type_complexity)]
fn transform_data(body: &[u8]) -> Result<(Option<[[f32; 3]; 3]>, Option<[f32; 3]>, Option<f32>)> {
    let mut r = Reader::new(body);
    let total = body.len();
    let n_rot = r.u32()? as usize;
    let mut rotation = None;
    if n_rot > 0 {
        let kind = r.u32()?;
        if kind == KEY_XYZ_ROTATION {
            let mut e = [0.0f32; 3];
            for v in &mut e {
                if let Some(k) = last_key(&mut r, 1, total)? {
                    *v = k[0];
                }
            }
            rotation = Some(euler_matrix(e));
        } else {
            let extra = key_extra(kind, 4)?;
            let per = 4 * (1 + 4 + extra);
            if n_rot > total / per {
                return Err(crate::Error::Truncated { offset: r.pos(), what: "rotation keys" });
            }
            r.skip(per * (n_rot - 1))?;
            let _time = r.f32()?;
            let q = [r.f32()?, r.f32()?, r.f32()?, r.f32()?];
            r.skip(4 * extra)?;
            rotation = Some(quat_matrix(q));
        }
    }
    let translation = last_key(&mut r, 3, total)?.map(|v| [v[0], v[1], v[2]]);
    let scale = last_key(&mut r, 1, total)?.map(|v| v[0]);
    Ok((rotation, translation, scale))
}

/// Rotation matrix of a (w, x, y, z) quaternion, `v' = R v`.
pub fn quat_matrix(q: [f32; 4]) -> [[f32; 3]; 3] {
    let [w, x, y, z] = q;
    let n = (w * w + x * x + y * y + z * z).sqrt();
    let (w, x, y, z) = if n > 1e-12 { (w / n, x / n, y / n, z / n) } else { (1.0, 0.0, 0.0, 0.0) };
    [
        [1.0 - 2.0 * (y * y + z * z), 2.0 * (x * y - w * z), 2.0 * (x * z + w * y)],
        [2.0 * (x * y + w * z), 1.0 - 2.0 * (x * x + z * z), 2.0 * (y * z - w * x)],
        [2.0 * (x * z - w * y), 2.0 * (y * z + w * x), 1.0 - 2.0 * (x * x + y * y)],
    ]
}

/// Rotation matrix of NIF XYZ Euler angles (radians): X applied first, then Y, then Z, as
/// NifSkope composes them (`Matrix::fromEuler`).
pub fn euler_matrix(e: [f32; 3]) -> [[f32; 3]; 3] {
    let (sx, cx) = e[0].sin_cos();
    let (sy, cy) = e[1].sin_cos();
    let (sz, cz) = e[2].sin_cos();
    [
        [cy * cz, -cy * sz, sy],
        [sx * sy * cz + sz * cx, cx * cz - sx * sy * sz, -sx * cy],
        [sx * sz - cx * sy * cz, cx * sy * sz + sx * cz, cx * cy],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Block, Header};

    fn put_i32(v: &mut Vec<u8>, x: i32) {
        v.extend_from_slice(&x.to_le_bytes());
    }
    fn put_u32(v: &mut Vec<u8>, x: u32) {
        v.extend_from_slice(&x.to_le_bytes());
    }
    fn put_f32(v: &mut Vec<u8>, x: f32) {
        v.extend_from_slice(&x.to_le_bytes());
    }

    /// A file like a vanilla gate's: manager on the root, Open and Close sequences, one XYZ
    /// rotation channel with quadratic keys, undefined interpolator base, Open also moving a
    /// second node by translation keys with a defined base scale.
    fn gate() -> Nif {
        let mut nif = Nif::new();
        nif.header.strings = vec!["Open".into(), "Door".into(), "NiTransformController".into(), "Close".into(), "Latch".into()];
        nif.header.block_types = vec!["BSFadeNode".into(), "NiControllerManager".into(), "NiControllerSequence".into(), "NiTransformInterpolator".into(), "NiTransformData".into()];
        nif.header.block_type_index = vec![0, 1, 2, 3, 4, 3, 4, 2, 3, 4];
        nif.blocks = vec![Block::Node(Default::default())];
        nif.blocks.extend(std::iter::repeat_n(Block::Unknown, 9));
        // 1: manager -> sequences 2 (Open), 7 (Close).
        let mut m = vec![0u8; 26];
        m.push(0);
        put_u32(&mut m, 2);
        put_i32(&mut m, 2);
        put_i32(&mut m, 7);
        put_i32(&mut m, -1);
        nif.raw_blocks.insert(1, m);
        // Sequences: (name, [(interpolator, node)], stop).
        let seq = |name: i32, blocks: &[(i32, i32)], stop: f32| {
            let mut s = Vec::new();
            put_i32(&mut s, name);
            put_u32(&mut s, blocks.len() as u32);
            put_u32(&mut s, 1);
            for &(i, n) in blocks {
                put_i32(&mut s, i);
                put_i32(&mut s, -1);
                s.push(0);
                put_i32(&mut s, n);
                for _ in 0..4 {
                    put_i32(&mut s, -1);
                }
            }
            put_f32(&mut s, 1.0);
            put_i32(&mut s, -1);
            put_u32(&mut s, 2);
            put_f32(&mut s, 1.0);
            put_f32(&mut s, 0.0);
            put_f32(&mut s, stop);
            put_i32(&mut s, 1);
            put_i32(&mut s, -1);
            s.extend_from_slice(&[0, 0]);
            s
        };
        nif.raw_blocks.insert(2, seq(0, &[(3, 1), (5, 4)], 2.0));
        nif.raw_blocks.insert(7, seq(3, &[(8, 1)], 1.6));
        let interp = |data: i32, scale: f32| {
            let mut v = Vec::new();
            for _ in 0..7 {
                put_f32(&mut v, -f32::MAX);
            }
            put_f32(&mut v, scale);
            put_i32(&mut v, data);
            v
        };
        nif.raw_blocks.insert(3, interp(4, -f32::MAX));
        nif.raw_blocks.insert(5, interp(6, 2.0));
        nif.raw_blocks.insert(8, interp(9, -f32::MAX));
        // XYZ rotation data: z from 0 to `z_end` with quadratic keys; no translation or scale.
        let rot = |z_end: f32| {
            let mut d = Vec::new();
            put_u32(&mut d, 1);
            put_u32(&mut d, KEY_XYZ_ROTATION);
            for axis in 0..3 {
                put_u32(&mut d, 2);
                put_u32(&mut d, KEY_QUADRATIC);
                for (t, v) in [(0.0f32, 0.0f32), (2.0, if axis == 2 { z_end } else { 0.0 })] {
                    put_f32(&mut d, t);
                    put_f32(&mut d, v);
                    put_f32(&mut d, 0.0);
                    put_f32(&mut d, 0.0);
                }
            }
            put_u32(&mut d, 0);
            put_u32(&mut d, 0);
            d
        };
        nif.raw_blocks.insert(4, rot(std::f32::consts::FRAC_PI_2));
        nif.raw_blocks.insert(9, rot(0.0));
        // Translation data with linear keys and a TBC scale key.
        let mut d = Vec::new();
        put_u32(&mut d, 0);
        put_u32(&mut d, 2);
        put_u32(&mut d, KEY_LINEAR);
        for (t, v) in [(0.0f32, [0.0f32, 0.0, 0.0]), (2.0, [10.0, 0.0, 5.0])] {
            put_f32(&mut d, t);
            for c in v {
                put_f32(&mut d, c);
            }
        }
        put_u32(&mut d, 1);
        put_u32(&mut d, KEY_TBC);
        put_f32(&mut d, 2.0);
        put_f32(&mut d, 3.0);
        for _ in 0..3 {
            put_f32(&mut d, 0.0);
        }
        nif.raw_blocks.insert(6, d);
        let _ = Header::default();
        nif
    }

    #[test]
    fn gate_sequences_end_poses() {
        let nif = gate();
        let s = sequences(&nif);
        assert_eq!(s.len(), 2);
        assert_eq!((s[0].name.as_str(), s[0].stop), ("Open", 2.0));
        assert_eq!((s[1].name.as_str(), s[1].stop), ("Close", 1.6));
        let door = &s[0].channels[0];
        assert_eq!(door.node, "Door");
        assert_eq!(door.translation, None);
        assert_eq!(door.scale, None);
        let r = door.rotation.unwrap();
        // 90 degrees about Z: x -> y.
        let v = [r[0][0] * 1.0 + r[0][1] * 0.0, r[1][0] * 1.0 + r[1][1] * 0.0, r[2][0]];
        assert!((v[0]).abs() < 1e-6 && (v[1] - 1.0).abs() < 1e-6 && v[2].abs() < 1e-6, "{v:?}");
        let latch = &s[0].channels[1];
        assert_eq!(latch.node, "Latch");
        assert_eq!(latch.translation, Some([10.0, 0.0, 5.0]));
        assert_eq!(latch.scale, Some(3.0), "the TBC scale key wins over the defined base scale");
        assert_eq!(latch.rotation, None);
        let close = &s[1].channels[0];
        let r = close.rotation.unwrap();
        assert!((r[0][0] - 1.0).abs() < 1e-6 && r[0][1].abs() < 1e-6);
    }

    #[test]
    fn quaternion_and_euler_agree_about_z() {
        let a = 0.7f32;
        let q = quat_matrix([(a / 2.0).cos(), 0.0, 0.0, (a / 2.0).sin()]);
        let e = euler_matrix([0.0, 0.0, a]);
        for i in 0..3 {
            for j in 0..3 {
                assert!((q[i][j] - e[i][j]).abs() < 1e-6, "{i}{j}: {} vs {}", q[i][j], e[i][j]);
            }
        }
    }
}
