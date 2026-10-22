// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! Decoded LOD geometry: every shape of a NIF with its node transforms applied (world space) and
//! the material facts the renderer and the checks need.

use glam::{Mat3, Vec3};
use lodforge_nif::{Block, Nif, Node, Ref, TriShape};
use serde::Serialize;

use crate::files::normalize;

/// SLSF1 Own_Emit.
pub const SLSF1_OWN_EMIT: u32 = 1 << 22;
/// SLSF2 Double_Sided.
pub const SLSF2_DOUBLE_SIDED: u32 = 1 << 4;
/// SLSF2 Vertex_Colors.
pub const SLSF2_VERTEX_COLORS: u32 = 1 << 5;
/// SLSF2 Glow_Map.
pub const SLSF2_GLOW_MAP: u32 = 1 << 6;

/// What a shape is, from where it sits and what it is called.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum ShapeKind {
    /// Terrain LOD land (`.btr`, first shape).
    Terrain,
    /// Terrain LOD water (`.btr`, shapes under the water node).
    Water,
    /// Object LOD (`.bto`, `obj*` shapes).
    Object,
    /// Tree billboards inside object LOD (`treepassthru`).
    TreeBillboard,
    /// Grass billboards inside object LOD (`grasspassthru`).
    Grass,
    /// A model viewed on its own (grass `_lod1.nif`, LOD models).
    Model,
}

impl ShapeKind {
    pub fn label(self) -> &'static str {
        match self {
            ShapeKind::Terrain => "terrain",
            ShapeKind::Water => "water",
            ShapeKind::Object => "object",
            ShapeKind::TreeBillboard => "tree billboard",
            ShapeKind::Grass => "grass",
            ShapeKind::Model => "model",
        }
    }
}

/// Material facts of one shape.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Material {
    pub name: String,
    /// Texture set as written (slot order of BSShaderTextureSet), empty slots kept.
    pub textures: Vec<String>,
    /// Normalized diffuse / normal / glow paths (`None` when the slot is empty).
    pub diffuse: Option<String>,
    pub normal: Option<String>,
    pub glow: Option<String>,
    pub shader_type: u32,
    pub flags1: u32,
    pub flags2: u32,
    /// BSEffectShaderProperty rather than BSLightingShaderProperty.
    pub effect: bool,
    pub alpha_test: bool,
    pub alpha_blend: bool,
    pub threshold: u8,
    pub double_sided: bool,
    pub vertex_colors: bool,
    pub glow_map: bool,
    pub own_emit: bool,
    pub emissive: [f32; 3],
    pub emissive_multiple: f32,
    pub uv_scale: [f32; 2],
    pub uv_offset: [f32; 2],
    pub large_ref: bool,
}

/// Axis-aligned box.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Aabb {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Default for Aabb {
    fn default() -> Self {
        Aabb::EMPTY
    }
}

impl Aabb {
    pub const EMPTY: Aabb = Aabb {
        min: [f32::INFINITY; 3],
        max: [f32::NEG_INFINITY; 3],
    };

    pub fn add(&mut self, p: [f32; 3]) {
        for i in 0..3 {
            self.min[i] = self.min[i].min(p[i]);
            self.max[i] = self.max[i].max(p[i]);
        }
    }

    pub fn from_points<'a>(pts: impl IntoIterator<Item = &'a [f32; 3]>) -> Aabb {
        let mut b = Aabb::EMPTY;
        for p in pts {
            b.add(*p);
        }
        b
    }

    pub fn is_empty(&self) -> bool {
        self.min[0] > self.max[0]
    }

    pub fn center(&self) -> [f32; 3] {
        [
            (self.min[0] + self.max[0]) * 0.5,
            (self.min[1] + self.max[1]) * 0.5,
            (self.min[2] + self.max[2]) * 0.5,
        ]
    }

    pub fn contains(&self, p: [f32; 3], margin: f32) -> bool {
        (0..3).all(|i| p[i] >= self.min[i] - margin && p[i] <= self.max[i] + margin)
    }

    pub fn union(&self, o: &Aabb) -> Aabb {
        let mut b = *self;
        if !o.is_empty() {
            b.add(o.min);
            b.add(o.max);
        }
        b
    }
}

/// One shape in world space.
#[derive(Debug, Clone)]
pub struct ShapeMesh {
    /// Block index in its file.
    pub block: u32,
    pub kind: ShapeKind,
    pub material: Material,
    pub positions: Vec<[f32; 3]>,
    /// Empty when the shape carries no normals (terrain LOD).
    pub normals: Vec<[f32; 3]>,
    pub tangents: Vec<[f32; 3]>,
    pub bitangents: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub colors: Vec<[u8; 4]>,
    pub indices: Vec<u32>,
    pub aabb: Aabb,
    /// Bounding sphere as stored, transformed to world space.
    pub bound_center: [f32; 3],
    pub bound_radius: f32,
    /// BSMultiBoundAABB of the nearest enclosing BSMultiBoundNode (world space, as stored).
    pub multibound: Option<Aabb>,
    /// Transform scale that was applied (the LOD level for generated files).
    pub scale: f32,
    /// Triangle indices that were dropped (out of range vertex index).
    pub bad_indices: usize,
    /// Vertices with a non-finite coordinate.
    pub non_finite: usize,
    /// Segments (BSSubIndexTriShape) as stored: (start index, index count).
    pub segments: Vec<(u32, u32)>,
}

impl ShapeMesh {
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }
}

/// Rigid transform with uniform scale: `p' = t + r * (s * p)`.
#[derive(Debug, Clone, Copy)]
pub struct Xform {
    pub t: Vec3,
    pub r: Mat3,
    pub s: f32,
}

impl Xform {
    pub const IDENTITY: Xform = Xform {
        t: Vec3::ZERO,
        r: Mat3::IDENTITY,
        s: 1.0,
    };

    pub fn from_nif(t: &lodforge_nif::Transform) -> Xform {
        // nif.xml rotation is row-major; glam builds from columns.
        let m = t.rotation;
        Xform {
            t: Vec3::from(t.translation),
            r: Mat3::from_cols(
                Vec3::new(m[0][0], m[1][0], m[2][0]),
                Vec3::new(m[0][1], m[1][1], m[2][1]),
                Vec3::new(m[0][2], m[1][2], m[2][2]),
            ),
            s: t.scale,
        }
    }

    /// `self` applied after `child` (parent * child).
    pub fn then(&self, child: &Xform) -> Xform {
        Xform {
            t: self.t + self.r * (self.s * child.t),
            r: self.r * child.r,
            s: self.s * child.s,
        }
    }

    pub fn point(&self, p: [f32; 3]) -> [f32; 3] {
        (self.t + self.r * (self.s * Vec3::from(p))).to_array()
    }

    pub fn dir(&self, d: [f32; 3]) -> [f32; 3] {
        (self.r * Vec3::from(d)).normalize_or_zero().to_array()
    }
}

/// The BSMultiBoundAABB behind a node's `multi_bound`, placed by the file's base transform (the
/// box is stored in the file's own space: world space for object LOD, block space for terrain).
fn node_aabb(nif: &Nif, n: &Node, base: &Xform) -> Option<Aabb> {
    let Block::MultiBound { data } = nif.get(n.multi_bound)? else {
        return None;
    };
    let Block::MultiBoundAabb { position, extent } = nif.get(*data)? else {
        return None;
    };
    let c = base.point(*position);
    let e = [
        extent[0] * base.s.abs(),
        extent[1] * base.s.abs(),
        extent[2] * base.s.abs(),
    ];
    Some(Aabb {
        min: [c[0] - e[0], c[1] - e[1], c[2] - e[2]],
        max: [c[0] + e[0], c[1] + e[1], c[2] + e[2]],
    })
}

fn slot(textures: &[String], i: usize) -> Option<String> {
    textures
        .get(i)
        .filter(|t| !t.trim().is_empty())
        .map(|t| normalize(t))
}

fn material_of(nif: &Nif, s: &TriShape) -> Material {
    let name = s.av.net.name.clone().unwrap_or_default();
    let mut m = Material {
        name,
        emissive_multiple: 1.0,
        uv_scale: [1.0, 1.0],
        ..Default::default()
    };
    match nif.get(s.shader_property) {
        Some(Block::LightingShader(l)) => {
            if let Some(Block::TextureSet(t)) = nif.get(l.texture_set) {
                m.textures = t.clone();
            }
            m.shader_type = l.shader_type;
            m.flags1 = l.flags1;
            m.flags2 = l.flags2;
            m.double_sided = l.flags2 & SLSF2_DOUBLE_SIDED != 0;
            m.vertex_colors = l.flags2 & SLSF2_VERTEX_COLORS != 0;
            m.glow_map = l.flags2 & SLSF2_GLOW_MAP != 0;
            m.own_emit = l.flags1 & SLSF1_OWN_EMIT != 0;
            m.emissive = l.emissive_color;
            m.emissive_multiple = l.emissive_multiple;
            m.uv_scale = l.uv_scale;
            m.uv_offset = l.uv_offset;
        }
        Some(Block::EffectShader(e)) => {
            m.effect = true;
            m.textures = vec![e.source_texture.clone()];
            m.flags1 = e.flags1;
            m.flags2 = e.flags2;
            m.double_sided = e.flags2 & SLSF2_DOUBLE_SIDED != 0;
            m.vertex_colors = e.flags2 & SLSF2_VERTEX_COLORS != 0;
            m.emissive = [
                e.emissive_color[0],
                e.emissive_color[1],
                e.emissive_color[2],
            ];
            m.emissive_multiple = e.emissive_multiple;
            m.uv_scale = e.uv_scale;
            m.uv_offset = e.uv_offset;
        }
        _ => {}
    }
    m.diffuse = slot(&m.textures, 0);
    m.normal = slot(&m.textures, 1);
    m.glow = slot(&m.textures, 2);
    if let Some(Block::Alpha {
        flags, threshold, ..
    }) = nif.get(s.alpha_property)
    {
        // NiAlphaProperty: bit 0 blending, bit 9 testing.
        m.alpha_blend = flags & 1 != 0;
        m.alpha_test = flags & (1 << 9) != 0;
        m.threshold = *threshold;
    }
    m.large_ref = s.av.net.extra_data.iter().any(
        |r| matches!(nif.get(*r), Some(Block::LargeRefExtra { large_ref, .. }) if *large_ref != 0),
    );
    m
}

/// How shapes of a file are classified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum FileKind {
    /// `.btr`: first root child is land, the rest water.
    Terrain,
    /// `.bto`: by shape name.
    Objects,
    /// A standalone model.
    Model,
}

fn classify(kind: FileKind, name: &str, under_root_index: usize) -> ShapeKind {
    match kind {
        FileKind::Terrain => {
            if under_root_index == 0 {
                ShapeKind::Terrain
            } else {
                ShapeKind::Water
            }
        }
        FileKind::Objects => {
            let n = name.to_ascii_lowercase();
            if n.starts_with("grasspassthru") {
                ShapeKind::Grass
            } else if n.starts_with("treepassthru") {
                ShapeKind::TreeBillboard
            } else {
                ShapeKind::Object
            }
        }
        FileKind::Model => ShapeKind::Model,
    }
}

/// Every shape of a parsed NIF in world space.
pub fn shapes_of(nif: &Nif, kind: FileKind) -> Vec<ShapeMesh> {
    shapes_with(nif, kind, Xform::IDENTITY)
}

/// Every shape of a parsed NIF, placed by `base` before the file's own transforms. Terrain LOD
/// files (`.btr`) carry no block translation (the game places them by file name), so their base
/// is the block's south-west corner; object LOD files translate their shapes themselves.
pub fn shapes_with(nif: &Nif, kind: FileKind, base: Xform) -> Vec<ShapeMesh> {
    let mut out = Vec::new();
    for &root in &nif.roots {
        walk(nif, root, base, &base, None, kind, 0, &mut out, 0);
    }
    out
}

/// Base transform of a terrain LOD block: its south-west corner.
pub fn terrain_base(block_x: i32, block_y: i32) -> Xform {
    Xform {
        t: Vec3::new(block_x as f32 * 4096.0, block_y as f32 * 4096.0, 0.0),
        r: Mat3::IDENTITY,
        s: 1.0,
    }
}

#[allow(clippy::too_many_arguments)]
fn walk(
    nif: &Nif,
    r: Ref,
    xf: Xform,
    base: &Xform,
    aabb: Option<Aabb>,
    kind: FileKind,
    root_child: usize,
    out: &mut Vec<ShapeMesh>,
    depth: u32,
) {
    let Some(i) = r else { return };
    if depth > 64 {
        return;
    }
    match nif.get(r) {
        Some(Block::Node(n)) => {
            let xf = xf.then(&Xform::from_nif(&n.av.transform));
            let aabb = node_aabb(nif, n, base).or(aabb);
            for (ci, &c) in n.children.iter().enumerate() {
                let rc = if depth == 0 { ci } else { root_child };
                walk(nif, c, xf, base, aabb, kind, rc, out, depth + 1);
            }
        }
        Some(Block::TriShape(s)) => {
            let xf = xf.then(&Xform::from_nif(&s.av.transform));
            let material = material_of(nif, s);
            let shape_kind = classify(kind, &material.name, root_child);
            out.push(shape_mesh(i, s, &xf, aabb, shape_kind, material));
        }
        _ => {}
    }
}

fn shape_mesh(
    block: u32,
    s: &TriShape,
    xf: &Xform,
    multibound: Option<Aabb>,
    kind: ShapeKind,
    material: Material,
) -> ShapeMesh {
    let mut non_finite = 0usize;
    let positions: Vec<[f32; 3]> = s
        .positions
        .iter()
        .map(|p| {
            let w = xf.point(*p);
            if !w.iter().all(|v| v.is_finite()) {
                non_finite += 1;
            }
            w
        })
        .collect();
    let n = positions.len() as u32;
    let mut bad_indices = 0usize;
    let mut indices = Vec::with_capacity(s.triangles.len() * 3);
    for t in &s.triangles {
        if t.iter().all(|&i| u32::from(i) < n) {
            indices.extend(t.iter().map(|&i| u32::from(i)));
        } else {
            bad_indices += 1;
        }
    }
    let aabb = Aabb::from_points(positions.iter().filter(|p| p.iter().all(|v| v.is_finite())));
    let dirs = |v: &[[f32; 3]]| -> Vec<[f32; 3]> {
        if v.len() == s.positions.len() {
            v.iter().map(|d| xf.dir(*d)).collect()
        } else {
            Vec::new()
        }
    };
    let segments = s
        .segments
        .as_ref()
        .map(|v| v.iter().map(|g| (g.start, g.count * 3)).collect())
        .unwrap_or_default();
    ShapeMesh {
        block,
        kind,
        material,
        normals: dirs(&s.normals),
        tangents: dirs(&s.tangents),
        bitangents: dirs(&s.bitangents),
        uvs: if s.uvs.len() == s.positions.len() {
            s.uvs.clone()
        } else {
            Vec::new()
        },
        colors: if s.colors.len() == s.positions.len() {
            s.colors.clone()
        } else {
            Vec::new()
        },
        positions,
        indices,
        aabb,
        bound_center: xf.point(s.bound_center),
        bound_radius: s.bound_radius * xf.s.abs(),
        multibound,
        scale: xf.s,
        bad_indices,
        non_finite,
        segments,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lodforge_nif::{AvObject, Node, ObjectNet, Transform, VertexDesc};

    /// A BTO-like file: root -> BSMultiBoundNode (AABB) -> shape at (bx*4096, by*4096) scale 4.
    fn bto_like() -> Nif {
        let mut nif = Nif::new();
        let root = nif.add_block(Block::Node(Node {
            av: AvObject {
                net: ObjectNet::named("obj"),
                ..Default::default()
            },
            ..Default::default()
        }));
        let mbn = nif.add_block(Block::Node(Node::multi_bound_node(None, 1)));
        let shape = TriShape {
            av: AvObject {
                net: ObjectNet::named("objSnow-LargeRef"),
                transform: Transform {
                    translation: [8192.0, -4096.0, 0.0],
                    scale: 4.0,
                    ..Default::default()
                },
                ..Default::default()
            },
            bound_center: [1.0, 1.0, 0.0],
            bound_radius: 2.0,
            positions: vec![[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 10.0, 5.0]],
            uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
            normals: vec![[0.0, 0.0, 1.0]; 3],
            triangles: vec![[0, 1, 2], [0, 1, 9]],
            vertex_desc: VertexDesc::with_attributes(true, true, true, false, false).0,
            ..Default::default()
        };
        let si = nif.add_block(Block::TriShape(shape));
        let extra = nif.add_block(Block::LargeRefExtra {
            name: Some("DOLRED".into()),
            large_ref: 1,
        });
        if let Block::TriShape(s) = &mut nif.blocks[si as usize] {
            s.av.net.extra_data = vec![Some(extra)];
        }
        let mb = nif.add_block(Block::MultiBound { data: Some(5) });
        nif.add_block(Block::MultiBoundAabb {
            position: [8212.0, -4076.0, 10.0],
            extent: [20.0, 20.0, 10.0],
        });
        if let Block::Node(n) = &mut nif.blocks[mbn as usize] {
            n.children = vec![Some(si)];
            n.multi_bound = Some(mb);
        }
        if let Block::Node(n) = &mut nif.blocks[root as usize] {
            n.children = vec![Some(mbn)];
        }
        nif
    }

    #[test]
    fn applies_transforms_and_reads_material() {
        let nif = bto_like();
        let shapes = shapes_of(&nif, FileKind::Objects);
        assert_eq!(shapes.len(), 1);
        let s = &shapes[0];
        assert_eq!(s.positions[1], [8232.0, -4096.0, 0.0]);
        assert_eq!(s.positions[2], [8192.0, -4056.0, 20.0]);
        assert_eq!(s.indices, vec![0, 1, 2]);
        assert_eq!(s.bad_indices, 1);
        assert_eq!(s.kind, ShapeKind::Object);
        assert!(s.material.large_ref);
        assert_eq!(s.bound_center, [8196.0, -4092.0, 0.0]);
        assert_eq!(s.bound_radius, 8.0);
        assert_eq!(s.multibound.unwrap().min, [8192.0, -4096.0, 0.0]);
        assert_eq!(s.scale, 4.0);
        assert_eq!(s.normals[0], [0.0, 0.0, 1.0]);
    }

    #[test]
    fn terrain_base_places_the_block() {
        // A BTR-like land shape: scale 4, translation (0, 0, z_shift), vertices in block units.
        let mut nif = Nif::new();
        let root = nif.add_block(Block::Node(Node::multi_bound_node(None, 1)));
        let shape = TriShape {
            av: AvObject {
                transform: Transform {
                    translation: [0.0, 0.0, 1.5],
                    scale: 4.0,
                    ..Default::default()
                },
                ..Default::default()
            },
            positions: vec![[0.0, 0.0, 10.0], [4096.0, 0.0, 10.0], [0.0, 4096.0, 10.0]],
            triangles: vec![[0, 1, 2]],
            vertex_desc: VertexDesc::with_attributes(true, false, false, false, false).0,
            ..Default::default()
        };
        let si = nif.add_block(Block::TriShape(shape));
        let mb = nif.add_block(Block::MultiBound { data: Some(3) });
        nif.add_block(Block::MultiBoundAabb {
            position: [8192.0, 8192.0, 41.5],
            extent: [8192.0, 8192.0, 10.0],
        });
        if let Block::Node(n) = &mut nif.blocks[root as usize] {
            n.children = vec![Some(si)];
            n.multi_bound = Some(mb);
        }
        let s = shapes_with(&nif, FileKind::Terrain, terrain_base(-8, 12));
        assert_eq!(s[0].kind, ShapeKind::Terrain);
        assert_eq!(s[0].positions[0], [-32768.0, 49152.0, 41.5]);
        assert_eq!(s[0].positions[1], [-32768.0 + 16384.0, 49152.0, 41.5]);
        // The block-space AABB moves with the block.
        let b = s[0].multibound.unwrap();
        assert_eq!(b.min, [-32768.0, 49152.0, 31.5]);
        assert_eq!(b.max, [-32768.0 + 16384.0, 49152.0 + 16384.0, 51.5]);
    }

    #[test]
    fn classifies_by_name() {
        assert_eq!(
            classify(FileKind::Objects, "grasspassthru", 0),
            ShapeKind::Grass
        );
        assert_eq!(
            classify(FileKind::Objects, "treepassthru", 3),
            ShapeKind::TreeBillboard
        );
        assert_eq!(classify(FileKind::Objects, "objHD", 0), ShapeKind::Object);
        assert_eq!(classify(FileKind::Terrain, "", 0), ShapeKind::Terrain);
        assert_eq!(classify(FileKind::Terrain, "", 1), ShapeKind::Water);
    }

    #[test]
    fn rotation_is_row_major() {
        // 90 degrees about Z as nif.xml stores it (row-major): x -> y.
        let t = Transform {
            translation: [0.0; 3],
            rotation: [[0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]],
            scale: 1.0,
        };
        let p = Xform::from_nif(&t).point([1.0, 0.0, 0.0]);
        assert!((p[0]).abs() < 1e-6 && (p[1] - 1.0).abs() < 1e-6, "{p:?}");
    }
}
