// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! From a loaded scene and the viewing choices (levels, layers, shading, camera) to the draw
//! list of one frame; shared by the window and the headless renderer.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::camera::{ViewParams, aabb_in_frustum};
use crate::lodselect::{self, LodMode};
use crate::mesh::{FileKind, ShapeKind};
use crate::pick::Hit;
use crate::render::{
    DrawShape, Frame, GlobalsGpu, KIND_COLORS, LEVEL_COLORS, LineVertex, Shading, box_lines,
};
use crate::scene::Scene;
use crate::world::{QuadId, WorldFiles};

/// Which kinds of LOD are drawn.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Layers {
    pub terrain: bool,
    pub water: bool,
    pub objects: bool,
    pub tree_billboards: bool,
    pub grass: bool,
    /// Classic tree LOD (`.btt`).
    pub trees: bool,
    /// Only shapes flagged as large references.
    pub large_refs_only: bool,
}

impl Default for Layers {
    fn default() -> Self {
        Layers {
            terrain: true,
            water: true,
            objects: true,
            tree_billboards: true,
            grass: true,
            trees: true,
            large_refs_only: false,
        }
    }
}

impl Layers {
    pub fn shows(&self, kind: ShapeKind, large_ref: bool) -> bool {
        if self.large_refs_only && !large_ref {
            return false;
        }
        match kind {
            ShapeKind::Terrain => self.terrain,
            ShapeKind::Water => self.water,
            ShapeKind::Object | ShapeKind::Model => self.objects,
            ShapeKind::TreeBillboard => self.tree_billboards,
            ShapeKind::Grass => self.grass,
        }
    }

    /// `terrain,objects,trees,...` (names of the fields; `all`).
    pub fn parse(s: &str) -> Layers {
        let mut l = Layers {
            terrain: false,
            water: false,
            objects: false,
            tree_billboards: false,
            grass: false,
            trees: false,
            large_refs_only: false,
        };
        for part in s.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            match part.to_ascii_lowercase().as_str() {
                "all" => l = Layers::default(),
                "terrain" => l.terrain = true,
                "water" => l.water = true,
                "objects" => l.objects = true,
                "tree_billboards" | "tree-billboards" | "billboards" => l.tree_billboards = true,
                "grass" => l.grass = true,
                "trees" => l.trees = true,
                "large_refs" | "large-refs" | "largerefs" => l.large_refs_only = true,
                _ => {}
            }
        }
        l
    }
}

/// Viewing choices.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ViewSettings {
    pub mode: LodMode,
    pub layers: Layers,
    pub shading: Shading,
    pub normal_maps: bool,
    /// Alpha test threshold forced on every shape (`None`: as the material says).
    pub alpha_override: Option<f32>,
    /// Sun heading (degrees clockwise from north) and elevation above the horizon.
    pub sun_azimuth: f32,
    pub sun_elevation: f32,
    pub ambient: f32,
    /// Outline every drawn block, coloured by level.
    pub grid: bool,
    /// Draw the selected shape's stored bounds.
    pub bounds: bool,
    pub sky: [f32; 3],
}

impl Default for ViewSettings {
    fn default() -> Self {
        ViewSettings {
            mode: LodMode::default(),
            layers: Layers::default(),
            shading: Shading::Textured,
            normal_maps: true,
            alpha_override: None,
            sun_azimuth: 225.0,
            sun_elevation: 40.0,
            ambient: 0.35,
            grid: false,
            bounds: true,
            sky: [0.55, 0.68, 0.85],
        }
    }
}

/// Scene quad indices to draw for the camera's (x, y): terrain and object blocks of the levels
/// the mode selects.
pub fn visible_quads(
    scene: &Scene,
    world: &WorldFiles,
    mode: LodMode,
    cam_xy: [f32; 2],
) -> Vec<usize> {
    let sw = world.grid_origin();
    let mut out = Vec::new();
    for (present, kind) in [
        (&scene.present_terrain, FileKind::Terrain),
        (&scene.present_objects, FileKind::Objects),
    ] {
        if present.is_empty() {
            continue;
        }
        let top = present.iter().map(|q| q.level).max().unwrap_or(32);
        let all: HashSet<QuadId> = present.iter().copied().collect();
        for q in lodselect::visible(mode, cam_xy, &all, sw, top) {
            if let Some(&i) = scene.by_id.get(&(q, kind)) {
                out.push(i);
            }
        }
    }
    out
}

/// Draw statistics of a frame.
#[derive(Debug, Clone, Copy, Default)]
pub struct FrameStats {
    pub quads: usize,
    pub shapes: usize,
    pub triangles: usize,
    pub trees: u32,
}

fn level_color(level: i32) -> [u8; 4] {
    let c = LEVEL_COLORS[match level {
        4 => 0,
        8 => 1,
        16 => 2,
        _ => 3,
    }];
    [
        (c[0] * 255.0) as u8,
        (c[1] * 255.0) as u8,
        (c[2] * 255.0) as u8,
        220,
    ]
}

/// Builds the frame: frustum-culled shapes of the visible quads, the tree instances, grid and
/// selection overlays, and the shader constants.
pub fn build_frame<'a>(
    scene: &'a Scene,
    visible: &[usize],
    s: &ViewSettings,
    view: &ViewParams,
    selection: Option<Hit>,
) -> (Frame<'a>, GlobalsGpu, FrameStats) {
    let planes = view.frustum();
    let mut frame = Frame {
        clear: [
            f64::from(s.sky[0]),
            f64::from(s.sky[1]),
            f64::from(s.sky[2]),
            1.0,
        ],
        ..Default::default()
    };
    let mut stats = FrameStats::default();
    for &qi in visible {
        let q = &scene.quads[qi];
        if !aabb_in_frustum(&planes, q.aabb.min, q.aabb.max) {
            continue;
        }
        stats.quads += 1;
        if s.grid && !q.aabb.is_empty() {
            let [x0, y0, x1, y1] = q.data.quad.rect();
            let z = q.aabb.max[2] + 2.0;
            let c = level_color(q.data.quad.level);
            let p = |x: f32, y: f32| LineVertex {
                pos: [x, y, z],
                color: c,
            };
            frame.overlay.extend([
                p(x0, y0),
                p(x1, y0),
                p(x1, y0),
                p(x1, y1),
                p(x1, y1),
                p(x0, y1),
                p(x0, y1),
                p(x0, y0),
            ]);
        }
        for sh in &q.shapes {
            if !s.layers.shows(sh.kind, sh.large_ref)
                || sh.triangles == 0
                || !aabb_in_frustum(&planes, sh.aabb.min, sh.aabb.max)
            {
                continue;
            }
            stats.shapes += 1;
            stats.triangles += sh.triangles as usize;
            frame.shapes.push(DrawShape {
                vbuf: &q.vbuf,
                ibuf: &q.ibuf,
                materials: &q.materials_bg,
                textures: &sh.textures.bind_group,
                indices: sh.indices.clone(),
                material_index: sh.material_index,
                blend: sh.blend,
                double_sided: sh.double_sided,
            });
        }
    }
    // Fewer texture rebinds: group by texture bind group.
    frame.shapes.sort_by_key(|d| {
        (
            d.blend,
            d.double_sided,
            d.textures as *const _ as usize,
            d.vbuf as *const _ as usize,
        )
    });
    if s.layers.trees {
        if let Some(t) = &scene.trees {
            if !t.instances.is_empty() && !s.layers.large_refs_only {
                frame.trees = Some((&t.buffer, t.instances.len() as u32, &t.atlas.bind_group));
                stats.trees = t.instances.len() as u32;
            }
        }
    }
    if let Some(h) = selection {
        if let Some(q) = scene.quads.get(h.quad) {
            if let Some(sh) = q.shapes.get(h.shape) {
                frame.wire.push(DrawShape {
                    vbuf: &q.vbuf,
                    ibuf: &q.ibuf,
                    materials: &q.materials_bg,
                    textures: &sh.textures.bind_group,
                    indices: sh.indices.clone(),
                    material_index: sh.material_index,
                    blend: false,
                    double_sided: true,
                });
                if s.bounds {
                    box_lines(
                        sh.aabb.min,
                        sh.aabb.max,
                        [255, 200, 40, 255],
                        &mut frame.overlay,
                    );
                    if let Some(mb) = q.data.shapes.get(sh.cpu).and_then(|c| c.multibound) {
                        box_lines(mb.min, mb.max, [80, 200, 255, 255], &mut frame.overlay);
                    }
                }
            }
        }
    }
    let (sa, ca) = s.sun_azimuth.to_radians().sin_cos();
    let (se, ce) = s.sun_elevation.to_radians().sin_cos();
    let globals = GlobalsGpu {
        view_proj: view.view_proj().to_cols_array_2d(),
        cam_pos: [
            view.eye.x,
            view.eye.y,
            view.eye.z,
            if view.ortho { 1.0 } else { 0.0 },
        ],
        sun_dir: [sa * ce, ca * ce, se, s.ambient],
        params: [
            s.shading.index(),
            if s.normal_maps { 1.0 } else { 0.0 },
            s.alpha_override.unwrap_or(-1.0),
            0.0,
        ],
        level_colors: LEVEL_COLORS,
        kind_colors: KIND_COLORS,
    };
    (frame, globals, stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layer_parsing() {
        let l = Layers::parse("terrain, trees");
        assert!(l.terrain && l.trees && !l.objects);
        assert!(l.shows(ShapeKind::Terrain, false) && !l.shows(ShapeKind::Object, false));
        let l = Layers::parse("all,large_refs");
        assert!(
            l.large_refs_only
                && l.shows(ShapeKind::Object, true)
                && !l.shows(ShapeKind::Object, false)
        );
    }
}
