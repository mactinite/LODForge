// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! Tree impostors of the LODForge runtime: the hemi-octahedral impostor set
//! (`textures\LODForge\Impostors\<World>\impostors.json`, docs/runtime/impostors.md) placed at
//! the trees of `<World>.trees.bin` ([`crate::runtime`]) and drawn as the runtime draws them
//! (`vs_impostor` / `fs_impostor`: nearest frame with a jittered switch, or the contract's
//! 3-frame blend; distance fade, thinning and the hand-over to the mesh tier).

use lodforge_impostor::manifest::Index;

use crate::runtime::Tree;

/// Index file of a worldspace's set (Data-relative, lower case).
pub fn index_path(world: &str) -> String {
    format!("textures\\lodforge\\impostors\\{}\\impostors.json", world.to_ascii_lowercase())
}

pub fn parse_index(b: &[u8]) -> Option<Index> {
    serde_json::from_slice(b).ok()
}

/// Instance flag: the tree has a mesh-tier model (the impostor hands over to it near).
pub const FLAG_MESH: u32 = 1;

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
    /// Quad size (model units), frames per side, frame size (px), unused.
    pub quad: f32,
    pub grid: f32,
    pub frame_px: f32,
    pub _pad: f32,
    /// Colour correction `brightness * tint` (rgb); w = 1 when the albedo is pre-lit.
    pub tint: [f32; 4],
    /// Reference hash, flags ([`FLAG_MESH`]), unused.
    pub ids: [u32; 4],
}

/// GPU instances of `trees`, grouped by atlas page (instances of page k are `ranges[k]`);
/// trees of unknown types are counted and left out.
pub fn instances(trees: &[Tree], index: &Index) -> (Vec<ImpostorInstance>, Vec<std::ops::Range<u32>>, usize) {
    let mut by_page: Vec<Vec<ImpostorInstance>> = vec![Vec::new(); index.pages.len()];
    let mut unknown = 0;
    for t in trees {
        let Some(ty) = index.types.get(usize::from(t.type_)) else {
            unknown += 1;
            continue;
        };
        let Some(page) = by_page.get_mut(ty.page as usize) else {
            unknown += 1;
            continue;
        };
        let flags = if t.mesh != lodforge_instances::meshpack::NO_MESH { FLAG_MESH } else { 0 };
        page.push(ImpostorInstance {
            pos: t.pos,
            yaw: t.yaw,
            center: ty.bounds_center_offset,
            scale: t.scale,
            rect: ty.rect,
            quad: ty.quad_size,
            grid: ty.grid as f32,
            frame_px: ty.frame_px as f32,
            _pad: 0.0,
            tint: [ty.tint[0] * ty.brightness, ty.tint[1] * ty.brightness, ty.tint[2] * ty.brightness, if ty.prelit { 1.0 } else { 0.0 }],
            ids: [t.hash, flags, 0, 0],
        });
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
            brightness: 1.0,
            tint: [1.0; 3],
            prelit: true,
            cache_key: String::new(),
        };
        let p = |index: u32| PageInfo { index, albedo: String::new(), normal_depth: String::new(), width: 1536, height: 1536, mips: 7 };
        Index { version: 1, baker_version: 1, layout: "hemi-oct-z-up".into(), worlds: vec![], pages: vec![p(0), p(1)], types: vec![t(1), t(0)], snow_grade: None, alpha: None }
    }

    #[test]
    fn page_grouping_and_flags() {
        let tree = |x: f32, type_: u16, mesh: u32| Tree { pos: [x, 2.0, 3.0], yaw: 0.5, scale: 1.25, type_, hash: 7, mesh, center_z: 250.0 };
        let trees = [tree(0.0, 0, 3), tree(1.0, 1, lodforge_instances::meshpack::NO_MESH), tree(2.0, 7, 0)];
        let (gpu, ranges, unknown) = instances(&trees, &index());
        assert_eq!(unknown, 1);
        // Type 1 is on page 0, type 0 on page 1.
        assert_eq!(ranges, vec![0..1, 1..2]);
        assert_eq!(gpu[0].pos[0], 1.0);
        assert_eq!(gpu[0].ids, [7, 0, 0, 0]);
        assert_eq!(gpu[1].ids[1], FLAG_MESH);
        assert_eq!(std::mem::size_of::<ImpostorInstance>(), 96);
    }
}
