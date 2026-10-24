// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! Impostor preview: the LODForge runtime's hemi-octahedral tree impostors
//! (`textures\LODForge\Impostors\<World>\impostors.json`, docs/runtime/impostors.md) drawn the
//! way the runtime shader must draw them (`vs_impostor` / `fs_impostor`: frame selection,
//! 3-frame blend, baked normals), for golden screenshots.
//!
//! Instances come from the temporary `preview-instances.bin` `lodforge impostors
//! --preview-instances` writes, until the runtime's instance files exist.

use lodforge_impostor::manifest::Index;

/// Index file of a worldspace's set (Data-relative, lower case).
pub fn index_path(world: &str) -> String {
    format!("textures\\lodforge\\impostors\\{}\\impostors.json", world.to_ascii_lowercase())
}

/// Preview instance file of a worldspace's set.
pub fn preview_path(world: &str) -> String {
    format!("textures\\lodforge\\impostors\\{}\\preview-instances.bin", world.to_ascii_lowercase())
}

/// One preview instance.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PreviewInstance {
    pub pos: [f32; 3],
    /// Z rotation (radians, as the REFR stores it): world = Rz(-yaw) * model.
    pub yaw: f32,
    pub scale: f32,
    pub type_index: u32,
}

/// Parses `preview-instances.bin` (`LFPI` v1).
pub fn parse_preview(b: &[u8]) -> Option<Vec<PreviewInstance>> {
    if b.get(0..4)? != b"LFPI" || u32::from_le_bytes(b.get(4..8)?.try_into().ok()?) != 1 {
        return None;
    }
    let n = u32::from_le_bytes(b.get(8..12)?.try_into().ok()?) as usize;
    let recs = b.get(12..12 + n * 24)?;
    Some(
        recs.chunks_exact(24)
            .map(|r| {
                let f = |o: usize| f32::from_le_bytes(r[o..o + 4].try_into().unwrap());
                PreviewInstance { pos: [f(0), f(4), f(8)], yaw: f(12), scale: f(16), type_index: u32::from_le_bytes(r[20..24].try_into().unwrap()) }
            })
            .collect(),
    )
}

pub fn parse_index(b: &[u8]) -> Option<Index> {
    serde_json::from_slice(b).ok()
}

/// One impostor instance for the GPU (the quad is built in `vs_impostor`).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ImpostorInstance {
    pub pos: [f32; 3],
    pub yaw: f32,
    /// Model origin -> impostor centre (model units).
    pub center: [f32; 3],
    pub scale: f32,
    /// The type's rectangle on its page (normalised `u0, v0, du, dv`).
    pub rect: [f32; 4],
    /// Quad size (model units), frames per side, frame size (px), flags.
    pub quad: f32,
    pub grid: f32,
    pub frame_px: f32,
    pub flags: u32,
}

/// GPU instances of `inst`, grouped by atlas page (instances of page k are
/// `ranges[k]`); instances of unknown types are counted and left out.
pub fn instances(inst: &[PreviewInstance], index: &Index) -> (Vec<ImpostorInstance>, Vec<std::ops::Range<u32>>, usize) {
    let mut by_page: Vec<Vec<ImpostorInstance>> = vec![Vec::new(); index.pages.len()];
    let mut unknown = 0;
    for i in inst {
        let Some(t) = index.types.get(i.type_index as usize) else {
            unknown += 1;
            continue;
        };
        let Some(page) = by_page.get_mut(t.page as usize) else {
            unknown += 1;
            continue;
        };
        page.push(ImpostorInstance { pos: i.pos, yaw: i.yaw, center: t.bounds_center_offset, scale: i.scale, rect: t.rect, quad: t.quad_size, grid: t.grid as f32, frame_px: t.frame_px as f32, flags: 0 });
    }
    let mut out = Vec::new();
    let mut ranges = Vec::new();
    for p in by_page {
        let s = out.len() as u32;
        out.extend(p);
        ranges.push(s..out.len() as u32);
    }
    (out, ranges, unknown)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lodforge_impostor::manifest::{PageInfo, TypeManifest};

    fn index() -> Index {
        let t = |page: u32| TypeManifest {
            version: 1,
            name: format!("t{page}"),
            model: String::new(),
            record_model: String::new(),
            model_crc32: String::new(),
            bases: Vec::new(),
            grid: 12,
            frame_px: 64,
            atlas_px: 768,
            quad_size: 500.0,
            bounds_center_offset: [0.0, 0.0, 250.0],
            bounds_min: [0.0; 3],
            bounds_max: [0.0; 3],
            page,
            albedo: String::new(),
            normal_depth: String::new(),
            rect_px: [0, 0, 768, 768],
            rect: [0.0, 0.0, 0.5, 0.5],
            coverage: 0.3,
            cache_key: String::new(),
        };
        let p = |index: u32| PageInfo { index, albedo: String::new(), normal_depth: String::new(), width: 1536, height: 1536, mips: 7 };
        Index { version: 1, baker_version: 1, layout: "hemi-oct-z-up".into(), worlds: vec![], pages: vec![p(0), p(1)], types: vec![t(1), t(0)] }
    }

    #[test]
    fn preview_round_trip_and_page_grouping() {
        let mut b = b"LFPI".to_vec();
        b.extend_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(&3u32.to_le_bytes());
        for (k, ti) in [(0u32, 0u32), (1, 1), (2, 7)] {
            for v in [k as f32, 2.0, 3.0, 0.5, 1.25] {
                b.extend_from_slice(&v.to_le_bytes());
            }
            b.extend_from_slice(&ti.to_le_bytes());
        }
        let inst = parse_preview(&b).unwrap();
        assert_eq!(inst.len(), 3);
        assert_eq!(inst[1], PreviewInstance { pos: [1.0, 2.0, 3.0], yaw: 0.5, scale: 1.25, type_index: 1 });
        let (gpu, ranges, unknown) = instances(&inst, &index());
        assert_eq!(unknown, 1);
        // Type 1 is on page 0, type 0 on page 1.
        assert_eq!(ranges, vec![0..1, 1..2]);
        assert_eq!(gpu[0].pos[0], 1.0);
        assert_eq!(gpu[1].pos[0], 0.0);
        assert_eq!(std::mem::size_of::<ImpostorInstance>(), 64);
        assert!(parse_preview(b"LFPI\x02\0\0\0\0\0\0\0").is_none());
    }
}
