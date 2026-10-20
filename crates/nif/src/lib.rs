// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! Skyrim SE NIF reader and writer (file version 20.2.0.7, user version 12, BS version 100),
//! written from the niftools NIF format specification (`nif.xml`, niftoolsxml 0.10.0.0, commit
//! `028cb35b`, 2023-06-17) and the layouts observed in generated LOD files.
//!
//! Skyrim LE files (BS version 83) are read too, for LOD meshes that mods and DynDOLOD Resources
//! still ship in that format: NiTriShape / NiTriStrips with their NiTriShapeData /
//! NiTriStripsData decode into [`Block::TriShape`] (geometry pulled from the data block, flagged
//! [`TriShape::legacy`]). LE files are read-only: [`Nif::to_bytes`] refuses them.
//!
//! The header lists every block's type and byte size, so blocks LOD does not need are kept as
//! [`Block::Unknown`] and skipped exactly (their bytes are kept in [`Nif::raw_blocks`] so they can
//! be written back verbatim). Decoded blocks: NiNode family, BSTriShape family (incl.
//! BSSubIndexTriShape segments), BSLightingShaderProperty (all SSE fields), BSEffectShaderProperty
//! (leading fields only), BSShaderTextureSet, NiAlphaProperty, BSMultiBound, BSMultiBoundAABB.
//!
//! Header layout (nif.xml `Header`): text line, version u32, endian u8, user version u32, block
//! count u32, BS version u32, 3 export strings (u8 length incl. NUL), block type count u16, block
//! types (u32-sized strings), block type index u16[n], block sizes u32[n], string count u32,
//! max string length u32, strings (u32-sized), group count u32, groups u32[]. Footer: root count
//! u32 + root refs.
//!
//! Writing: [`Nif::to_bytes`] re-encodes every decoded block (see [`writer`]) and rebuilds the
//! block type, block size and string tables (kept when present, else in first-use order), so a
//! parsed file round-trips byte for byte. Terrain LOD files are assembled by the `lodforge-btr`
//! crate on top of this writer.

mod reader;
pub mod skin;
pub use skin::SkinGeometry;
pub mod writer;

use std::collections::BTreeMap;

use reader::Reader;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not a NIF file")]
    NotNif,
    #[error("unsupported NIF version {version:#010x} / user {user_version} / BS {bs_version}")]
    Unsupported { version: u32, user_version: u32, bs_version: u32 },
    #[error("truncated NIF at {offset:#x} while reading {what}")]
    Truncated { offset: usize, what: &'static str },
    #[error("vertex attributes need {computed} bytes but the stride is {stride} (desc {desc:#018x})")]
    VertexLayout { desc: u64, computed: usize, stride: usize },
    #[error("block {index} ({type_name}): {what}")]
    BadBlock { index: usize, type_name: String, what: String },
    #[error("cannot write: {0}")]
    Write(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Block reference: index into [`Nif::blocks`], `None` for -1.
pub type Ref = Option<u32>;

fn to_ref(v: i32) -> Ref {
    (v >= 0).then_some(v as u32)
}

/// Header line of SSE files (nif.xml `HeaderString` for version 20.2.0.7).
pub const HEADER_LINE: &str = "Gamebryo File Format, Version 20.2.0.7";
/// Creator export string LODForge writes into the files it generates.
pub const CREATOR: &str = concat!("LODForge ", env!("CARGO_PKG_VERSION"));

#[derive(Debug, Clone)]
pub struct Header {
    /// First line without the trailing `\n`.
    pub header_line: String,
    pub version: u32,
    pub endian: u8,
    pub user_version: u32,
    pub bs_version: u32,
    /// First export string ("creator"; generators write their own name and version here).
    pub author: String,
    pub process_script: String,
    pub export_script: String,
    pub block_types: Vec<String>,
    /// Per block: index into `block_types`.
    pub block_type_index: Vec<u16>,
    pub block_sizes: Vec<u32>,
    pub strings: Vec<String>,
    pub groups: Vec<u32>,
}

impl Default for Header {
    /// An empty SSE header with LODForge's creator string.
    fn default() -> Self {
        Header {
            header_line: HEADER_LINE.to_string(),
            version: 0x1402_0007,
            endian: 1,
            user_version: 12,
            bs_version: 100,
            author: CREATOR.to_string(),
            process_script: String::new(),
            export_script: String::new(),
            block_types: Vec::new(),
            block_type_index: Vec::new(),
            block_sizes: Vec::new(),
            strings: Vec::new(),
            groups: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ObjectNet {
    pub name: Option<String>,
    pub extra_data: Vec<Ref>,
    pub controller: Ref,
}

impl ObjectNet {
    pub fn named(name: &str) -> Self {
        ObjectNet { name: Some(name.to_string()), ..Default::default() }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Transform {
    pub translation: [f32; 3],
    /// Row-major 3x3 rotation.
    pub rotation: [[f32; 3]; 3],
    pub scale: f32,
}

impl Default for Transform {
    fn default() -> Self {
        Transform { translation: [0.0; 3], rotation: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], scale: 1.0 }
    }
}

/// NiAVObject fields. SSE (BS version above 26) stores `flags` as one u32 (nif.xml `NiAVObject`).
#[derive(Debug, Clone, PartialEq)]
pub struct AvObject {
    pub net: ObjectNet,
    pub flags: u32,
    pub transform: Transform,
    pub collision: Ref,
}

impl Default for AvObject {
    /// Flags 0xE (nif.xml's SSE default for BSMultiBoundNode / BSTriShape, observed on every AV
    /// object of the golden terrain files), identity transform, no collision.
    fn default() -> Self {
        AvObject { net: ObjectNet::default(), flags: 14, transform: Transform::default(), collision: None }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Node {
    pub av: AvObject,
    pub children: Vec<Ref>,
    pub effects: Vec<Ref>,
    /// BSMultiBoundNode only.
    pub multi_bound: Ref,
    /// BSMultiBoundNode only (`Some` marks the node as a BSMultiBoundNode when building).
    pub culling_mode: Option<u32>,
}

impl Node {
    /// A BSMultiBoundNode with the observed defaults (flags 0xE, identity transform).
    pub fn multi_bound_node(name: Option<&str>, culling_mode: u32) -> Self {
        Node {
            av: AvObject { net: ObjectNet { name: name.map(str::to_string), ..Default::default() }, ..Default::default() },
            culling_mode: Some(culling_mode),
            ..Default::default()
        }
    }
}

/// Vertex attribute flags (vertex desc bits 44..55).
pub mod va {
    pub const VERTEX: u16 = 0x001;
    pub const UVS: u16 = 0x002;
    pub const UVS_2: u16 = 0x004;
    pub const NORMALS: u16 = 0x008;
    pub const TANGENTS: u16 = 0x010;
    pub const COLORS: u16 = 0x020;
    pub const SKINNED: u16 = 0x040;
    pub const LAND_DATA: u16 = 0x080;
    pub const EYE_DATA: u16 = 0x100;
    pub const FULL_PRECISION: u16 = 0x400;
}

/// Decoded BSVertexDesc (nif.xml bitfield `BSVertexDesc`, u64): nibbles for the vertex data size
/// in dwords (bits 0..3), dynamic vertex size (4..7), UV1 / UV2 / normal / tangent / colour /
/// skinning / landscape / eye data offsets in dwords (8..39), and the attribute flags
/// (`VertexAttribute`) in bits 44..55.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VertexDesc(pub u64);

impl VertexDesc {
    pub fn stride(self) -> usize {
        (self.0 & 0xF) as usize * 4
    }
    fn offset(self, shift: u32) -> usize {
        ((self.0 >> shift) & 0xF) as usize * 4
    }
    /// Stored offset nibbles; may be stale in shipped files, prefer [VertexDesc::layout].
    pub fn uv_offset(self) -> usize {
        self.offset(8)
    }
    pub fn normal_offset(self) -> usize {
        self.offset(16)
    }
    pub fn tangent_offset(self) -> usize {
        self.offset(20)
    }
    pub fn color_offset(self) -> usize {
        self.offset(24)
    }
    /// Attribute offsets derived from the flags, in SSE order: position+W (16), UV (4), UV2 (4),
    /// normal+bitangent Y (4), tangent+bitangent Z (4), colour (4), skin (12), eye (4).
    /// Vanilla files exist whose stored offset nibbles are stale (e.g. a UV slot counted though
    /// the UV flag is clear), so readers must use this rather than the stored nibbles.
    pub fn layout(self) -> VertexLayout {
        let a = self.attributes();
        let mut off = 0;
        let mut take = |flag: u16, len: usize| -> Option<usize> {
            (a & flag != 0).then(|| {
                let o = off;
                off += len;
                o
            })
        };
        let position = take(va::VERTEX, 16);
        let uv = take(va::UVS, 4);
        let _uv2 = take(va::UVS_2, 4);
        let normal = take(va::NORMALS, 4);
        let tangent = take(va::TANGENTS, 4);
        let color = take(va::COLORS, 4);
        let _skin = take(va::SKINNED, 12);
        let _eye = take(va::EYE_DATA, 4);
        VertexLayout { position, uv, normal, tangent, color, size: off }
    }

    pub fn attributes(self) -> u16 {
        ((self.0 >> 44) & 0xFFF) as u16
    }
    pub fn has(self, flag: u16) -> bool {
        self.attributes() & flag != 0
    }

    /// The SSE vertex desc for a set of attribute flags (`va`), filled per the nif.xml bitfield:
    /// vertex data size = stride in dwords (position 4, every other attribute 1), the UV1 offset
    /// nibble = 4 whenever positions are present (observed also on position-only shapes), the
    /// normal / tangent / colour offset nibbles = the attribute's dword offset when present.
    /// Observed values: terrain land shapes (position + UV) `0x0000_3000_0000_0405`, terrain water
    /// shapes (position) `0x0000_1000_0000_0404`, object LOD shapes with every attribute
    /// `0x0003_b000_0765_0408`.
    pub fn sse(attributes: u16) -> Self {
        let a = attributes & (va::VERTEX | va::UVS | va::NORMALS | va::TANGENTS | va::COLORS);
        let mut size = 0u64;
        let mut desc = 0u64;
        if a & va::VERTEX != 0 {
            size += 4;
            desc |= 4 << 8;
        }
        for (flag, shift) in [(va::UVS, None), (va::NORMALS, Some(16)), (va::TANGENTS, Some(20)), (va::COLORS, Some(24))] {
            if a & flag != 0 {
                if let Some(sh) = shift {
                    desc |= size << sh;
                }
                size += 1;
            }
        }
        VertexDesc(desc | size | (u64::from(a) << 44))
    }

    /// [`VertexDesc::sse`] from presence flags.
    pub fn with_attributes(positions: bool, uvs: bool, normals: bool, tangents: bool, colors: bool) -> Self {
        let mut a = 0;
        for (present, flag) in [(positions, va::VERTEX), (uvs, va::UVS), (normals, va::NORMALS), (tangents, va::TANGENTS), (colors, va::COLORS)] {
            if present {
                a |= flag;
            }
        }
        Self::sse(a)
    }
}

/// Byte offsets of each present vertex attribute and the total vertex size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VertexLayout {
    pub position: Option<usize>,
    pub uv: Option<usize>,
    pub normal: Option<usize>,
    pub tangent: Option<usize>,
    pub color: Option<usize>,
    pub size: usize,
}

/// One BSSubIndexTriShape segment as SSE (BS version 100) stores it (nif.xml
/// `BSGeometrySegmentData`): flags u8, start index u32 (= 3 x the segment's first triangle),
/// num primitives u32 (its triangle count); 9 bytes. The sub-segment fields are FO4-only.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Segment {
    pub flags: u8,
    pub start: u32,
    pub count: u32,
}

/// BSTriShape / BSSubIndexTriShape / BSMeshLODTriShape (BS version 100).
///
/// Layout after NiAVObject: bound center vec3, radius f32, skin ref, shader ref, alpha ref,
/// vertex desc u64, triangle count u16, vertex count u16, data size u32 (= verts * stride +
/// tris * 6), vertex data, triangles u16[3][], particle data size u32 (+ data), then subclass
/// fields (BSSubIndexTriShape: segment count u32 + [`Segment`]s; BSMeshLODTriShape: 3 u32).
///
/// SSE vertex data per vertex (nif.xml `BSVertexDataSSE`, see [`VertexDesc::layout`]): position
/// f32[3] + bitangent X f32, UV half[2], normal u8[3] + bitangent Y u8, tangent u8[3] + bitangent
/// Z u8, colour u8[4]. Bytes map to floats as `b / 255 * 2 - 1` ([`byte_to_float`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TriShape {
    pub av: AvObject,
    pub bound_center: [f32; 3],
    pub bound_radius: f32,
    pub skin: Ref,
    pub shader_property: Ref,
    pub alpha_property: Ref,
    /// Written as stored; must agree with which attribute vectors are filled.
    pub vertex_desc: u64,
    pub positions: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub normals: Vec<[f32; 3]>,
    pub tangents: Vec<[f32; 3]>,
    /// Per-vertex bitangent stored in the W slots of position (X, f32), normal (Y, byte) and
    /// tangent (Z, byte). Empty means all zero (what shapes without tangents carry).
    pub bitangents: Vec<[f32; 3]>,
    /// RGBA8 vertex colours.
    pub colors: Vec<[u8; 4]>,
    pub triangles: Vec<[u16; 3]>,
    /// BSMeshLODTriShape: triangle counts of LOD0..2.
    pub lod_sizes: Option<[u32; 3]>,
    /// BSSubIndexTriShape: segments (`Some` marks the shape as a BSSubIndexTriShape when building).
    pub segments: Option<Vec<Segment>>,
    /// Skyrim LE NiTriShape / NiTriStrips: the geometry came from a NiTriShapeData /
    /// NiTriStripsData block. Normals, tangents and bitangents are the stored floats (tangents
    /// and bitangents only when the data has them, not one per vertex), and vertex colours are
    /// float RGBA in [`TriShape::colors_f32`] (`colors` holds them rounded to bytes).
    pub legacy: bool,
    /// LE vertex colours as stored (RGBA floats); empty for SSE shapes.
    pub colors_f32: Vec<[f32; 4]>,
}

impl TriShape {
    /// Recomputes `vertex_desc` from the filled attribute vectors ([`VertexDesc::with_attributes`]).
    pub fn update_vertex_desc(&mut self) {
        self.vertex_desc = VertexDesc::with_attributes(!self.positions.is_empty(), !self.uvs.is_empty(), !self.normals.is_empty(), !self.tangents.is_empty(), !self.colors.is_empty()).0;
    }
}

/// Shader-type-specific tail of BSLightingShaderProperty (SSE).
#[derive(Debug, Clone, Default, PartialEq)]
pub enum LightingExtra {
    /// Shader types without extra fields (0 default, 2 glow, 18 LOD landscape, ...).
    #[default]
    None,
    /// Type 1: environment map scale.
    EnvironmentMap { scale: f32 },
    /// Type 5: skin tint colour.
    SkinTint([f32; 3]),
    /// Type 6: hair tint colour.
    HairTint([f32; 3]),
    /// Type 7: max passes, scale.
    ParallaxOcclusion { max_passes: f32, scale: f32 },
    /// Type 11.
    MultiLayerParallax { inner_layer_thickness: f32, refraction_scale: f32, inner_layer_texture_scale: [f32; 2], envmap_strength: f32 },
    /// Type 14.
    SparkleSnow([f32; 4]),
    /// Type 16: cubemap scale, left and right eye reflection centres.
    EyeEnvironment { cubemap_scale: f32, left: [f32; 3], right: [f32; 3] },
}

impl LightingExtra {
    /// Shader type whose tail this is; `None` for [`LightingExtra::None`].
    pub fn shader_type(&self) -> Option<u32> {
        Some(match self {
            LightingExtra::None => return None,
            LightingExtra::EnvironmentMap { .. } => 1,
            LightingExtra::SkinTint(_) => 5,
            LightingExtra::HairTint(_) => 6,
            LightingExtra::ParallaxOcclusion { .. } => 7,
            LightingExtra::MultiLayerParallax { .. } => 11,
            LightingExtra::SparkleSnow(_) => 14,
            LightingExtra::EyeEnvironment { .. } => 16,
        })
    }
}

/// BSLightingShaderProperty (SSE). Layout: shader type u32, NiObjectNET, flags1 u32, flags2 u32,
/// UV offset f32[2], UV scale f32[2], texture set ref, emissive colour f32[3], emissive multiple,
/// texture clamp mode u32, alpha, refraction strength, glossiness, specular colour f32[3],
/// specular strength, lighting effect 1, lighting effect 2, then [`LightingExtra`] by shader type.
#[derive(Debug, Clone, PartialEq)]
pub struct LightingShader {
    pub shader_type: u32,
    pub net: ObjectNet,
    pub flags1: u32,
    pub flags2: u32,
    pub uv_offset: [f32; 2],
    pub uv_scale: [f32; 2],
    pub texture_set: Ref,
    pub emissive_color: [f32; 3],
    pub emissive_multiple: f32,
    pub texture_clamp_mode: u32,
    pub alpha: f32,
    pub refraction_strength: f32,
    pub glossiness: f32,
    pub specular_color: [f32; 3],
    pub specular_strength: f32,
    pub lighting_effect_1: f32,
    pub lighting_effect_2: f32,
    pub extra: LightingExtra,
}

impl Default for LightingShader {
    /// Starting values for a shader built from scratch: ZBuffer_Test / ZBuffer_Write, wrap clamp
    /// mode, nif.xml's glossiness 80 and white specular, lighting effects 1. Generated files set
    /// every field they care about explicitly (the terrain LOD shader is in `lodforge-btr`).
    fn default() -> Self {
        LightingShader {
            shader_type: 0,
            net: ObjectNet::default(),
            flags1: 0x8000_0000,
            flags2: 1,
            uv_offset: [0.0; 2],
            uv_scale: [1.0; 2],
            texture_set: None,
            emissive_color: [0.0; 3],
            emissive_multiple: 1.0,
            texture_clamp_mode: 3,
            alpha: 1.0,
            refraction_strength: 0.0,
            glossiness: 80.0,
            specular_color: [1.0; 3],
            specular_strength: 1.0,
            lighting_effect_1: 1.0,
            lighting_effect_2: 1.0,
            extra: LightingExtra::None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct EffectShader {
    pub net: ObjectNet,
    pub flags1: u32,
    pub flags2: u32,
    pub uv_offset: [f32; 2],
    pub uv_scale: [f32; 2],
    pub source_texture: String,
    pub texture_clamp_mode: u8,
    pub lighting_influence: u8,
    pub env_map_min_lod: u8,
    pub unknown: u8,
    /// Falloff start angle, stop angle, start opacity, stop opacity.
    pub falloff: [f32; 4],
    pub emissive_color: [f32; 4],
    pub emissive_multiple: f32,
    pub soft_falloff_depth: f32,
    pub greyscale_texture: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    /// NiNode, BSFadeNode, BSLeafAnimNode, BSMultiBoundNode, BSOrderedNode, ...
    Node(Node),
    /// BSTriShape, BSSubIndexTriShape, BSMeshLODTriShape, BSDynamicTriShape (static part).
    TriShape(TriShape),
    LightingShader(LightingShader),
    EffectShader(EffectShader),
    TextureSet(Vec<String>),
    Alpha { net: ObjectNet, flags: u16, threshold: u8 },
    MultiBound { data: Ref },
    MultiBoundAabb { position: [f32; 3], extent: [f32; 3] },
    /// BSDistantObjectLargeRefExtraData: NiExtraData name + large-reference byte.
    LargeRefExtra { name: Option<String>, large_ref: u8 },
    Unknown,
}

impl Block {
    /// Block type name used when the header has no entry for the block (blocks built from
    /// scratch): BSMultiBoundNode if `culling_mode` is set, else NiNode; BSSubIndexTriShape if
    /// `segments` is set, BSMeshLODTriShape if `lod_sizes` is set, else BSTriShape.
    pub fn default_type_name(&self) -> Option<&'static str> {
        Some(match self {
            Block::Node(n) if n.culling_mode.is_some() => "BSMultiBoundNode",
            Block::Node(_) => "NiNode",
            Block::TriShape(s) if s.segments.is_some() => "BSSubIndexTriShape",
            Block::TriShape(s) if s.lod_sizes.is_some() => "BSMeshLODTriShape",
            Block::TriShape(_) => "BSTriShape",
            Block::LightingShader(_) => "BSLightingShaderProperty",
            Block::EffectShader(_) => "BSEffectShaderProperty",
            Block::TextureSet(_) => "BSShaderTextureSet",
            Block::Alpha { .. } => "NiAlphaProperty",
            Block::MultiBound { .. } => "BSMultiBound",
            Block::MultiBoundAabb { .. } => "BSMultiBoundAABB",
            Block::LargeRefExtra { .. } => "BSDistantObjectLargeRefExtraData",
            Block::Unknown => return None,
        })
    }
}

#[derive(Debug, Clone)]
pub struct Nif {
    pub header: Header,
    pub blocks: Vec<Block>,
    pub roots: Vec<Ref>,
    /// Verbatim bytes of blocks the writer cannot re-encode (unknown types, partially decoded
    /// types such as BSDynamicTriShape, skinned shapes, blocks with undecoded trailing bytes).
    /// [`Nif::to_bytes`] emits these unchanged; remove an entry to force re-encoding.
    pub raw_blocks: BTreeMap<usize, Vec<u8>>,
}

impl Default for Nif {
    fn default() -> Self {
        Nif::new()
    }
}

impl Nif {
    /// An empty SSE NIF with LODForge's creator string and the footer root `[0]`.
    pub fn new() -> Self {
        Nif { header: Header::default(), blocks: Vec::new(), roots: vec![Some(0)], raw_blocks: BTreeMap::new() }
    }

    /// Appends a block and returns its index.
    pub fn add_block(&mut self, block: Block) -> u32 {
        self.blocks.push(block);
        (self.blocks.len() - 1) as u32
    }

    /// Type name of block `index`: the header's entry if the block came from a parsed file,
    /// otherwise [`Block::default_type_name`]. Panics if `index` is out of range.
    pub fn block_type(&self, index: usize) -> &str {
        match self.header.block_type_index.get(index).and_then(|&t| self.header.block_types.get(t as usize)) {
            Some(t) => t,
            None => self.blocks[index].default_type_name().unwrap_or("Unknown"),
        }
    }

    pub fn get(&self, r: Ref) -> Option<&Block> {
        r.and_then(|i| self.blocks.get(i as usize))
    }

    /// Texture paths of a shape's shader (lighting: texture set slots; effect: source texture).
    pub fn shape_textures(&self, shape: &TriShape) -> Vec<String> {
        match self.get(shape.shader_property) {
            Some(Block::LightingShader(s)) => match self.get(s.texture_set) {
                Some(Block::TextureSet(t)) => t.clone(),
                _ => Vec::new(),
            },
            Some(Block::EffectShader(s)) => vec![s.source_texture.clone()],
            _ => Vec::new(),
        }
    }

    /// Serializes the file. See [`writer::write`]. Skyrim LE (BS 83) files cannot be written.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        if self.header.bs_version == LE_BS_VERSION {
            return Err(Error::Write("Skyrim LE (BS 83) files are read-only".into()));
        }
        writer::write(self)
    }
}

const NODE_TYPES: &[&str] = &[
    "NiNode",
    "BSFadeNode",
    "BSLeafAnimNode",
    "BSTreeNode",
    "BSOrderedNode",
    "BSValueNode",
    "BSMultiBoundNode",
    "NiBillboardNode",
    "NiSwitchNode",
    "NiLODNode",
    "BSRangeNode",
    "BSBlastNode",
    "BSDamageStage",
    "BSDebrisNode",
];

/// Block types the writer re-encodes from the decoded fields (when decoding consumed the whole
/// block). Everything else round-trips through [`Nif::raw_blocks`].
pub const WRITABLE_TYPES: &[&str] = &[
    "NiNode",
    "BSFadeNode",
    "BSLeafAnimNode",
    "BSMultiBoundNode",
    "BSTriShape",
    "BSSubIndexTriShape",
    "BSMeshLODTriShape",
    "BSLightingShaderProperty",
    "BSEffectShaderProperty",
    "BSShaderTextureSet",
    "NiAlphaProperty",
    "BSMultiBound",
    "BSMultiBoundAABB",
    "BSDistantObjectLargeRefExtraData",
];

/// BS version of Skyrim LE files (read-only support).
pub const LE_BS_VERSION: u32 = 83;

pub fn parse(bytes: &[u8]) -> Result<Nif> {
    parse_impl(bytes, false)
}

/// Like [`parse`], but also decodes NiTriShape / NiTriStrips geometry in SSE (BS 100) files. Vanilla
/// SSE trees keep such shapes (the low-detail child of their NiSwitchNode) next to the skinned
/// BSTriShapes; renderers that cannot skin draw these instead.
pub fn parse_all_geometry(bytes: &[u8]) -> Result<Nif> {
    parse_impl(bytes, true)
}

fn parse_impl(bytes: &[u8], ni_geometry: bool) -> Result<Nif> {
    let mut r = Reader::new(bytes);
    let header = parse_header(&mut r)?;
    let legacy = header.bs_version == LE_BS_VERSION;
    let ni_geometry = ni_geometry || legacy;
    let mut blocks = Vec::with_capacity(header.block_sizes.len());
    let mut raw_blocks = BTreeMap::new();
    // LE: geometry blocks and the data blocks they reference, joined after all blocks are read.
    let mut le_shapes: Vec<(usize, Ref)> = Vec::new();
    let mut le_data: BTreeMap<usize, LeGeometryData> = BTreeMap::new();
    for (i, &size) in header.block_sizes.iter().enumerate() {
        let start = r.pos();
        let end = start + size as usize;
        let body = bytes.get(start..end).ok_or(Error::Truncated { offset: start, what: "block" })?;
        let type_name = header.block_types[header.block_type_index[i] as usize].as_str();
        let mut br = Reader::with_base(body, start);
        let bad = |e: Error| Error::BadBlock { index: i, type_name: type_name.to_string(), what: e.to_string() };
        let block = match type_name {
            "NiTriShape" | "NiTriStrips" | "BSLODTriShape" if ni_geometry => {
                let (s, data) = le_tri_shape(&mut br, &header).map_err(bad)?;
                le_shapes.push((i, data));
                Block::TriShape(s)
            }
            "NiTriShapeData" | "NiTriStripsData" if ni_geometry => {
                le_data.insert(i, le_geometry_data(&mut br, type_name == "NiTriStripsData").map_err(bad)?);
                Block::Unknown
            }
            _ => {
                let (block, faithful) = parse_block(&mut br, type_name, &header).map_err(bad)?;
                if !faithful || !br.rest().is_empty() || !WRITABLE_TYPES.contains(&type_name) {
                    raw_blocks.insert(i, body.to_vec());
                }
                block
            }
        };
        if legacy || (ni_geometry && (type_name.starts_with("NiTri") || type_name == "BSLODTriShape")) {
            raw_blocks.insert(i, body.to_vec());
        }
        blocks.push(block);
        r.seek(end);
    }
    for (i, data) in le_shapes {
        let (Some(d), Block::TriShape(s)) = (data.and_then(|d| le_data.get(&(d as usize))), &mut blocks[i]) else { continue };
        s.positions = d.vertices.clone();
        s.normals = d.normals.clone();
        s.tangents = d.tangents.clone();
        s.bitangents = d.bitangents.clone();
        s.uvs = d.uvs.clone();
        s.colors_f32 = d.colors.clone();
        s.colors = d.colors.iter().map(|c| c.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)).collect();
        s.triangles = d.triangles.clone();
        s.bound_center = d.center;
        s.bound_radius = d.radius;
    }
    let roots = match r.u32() {
        Ok(n) => (0..n).map(|_| r.i32().map(to_ref)).collect::<Result<_>>()?,
        Err(_) => Vec::new(),
    };
    Ok(Nif { header, blocks, roots, raw_blocks })
}

fn parse_header(r: &mut Reader<'_>) -> Result<Header> {
    let line_end = r.rest().iter().position(|&c| c == b'\n').ok_or(Error::NotNif)?;
    let header_line = reader::latin1(&r.rest()[..line_end]);
    if !header_line.starts_with("Gamebryo File Format") && !header_line.starts_with("NetImmerse File Format") {
        return Err(Error::NotNif);
    }
    r.skip(line_end + 1)?;
    let version = r.u32()?;
    let endian = r.u8()?;
    let user_version = r.u32()?;
    let num_blocks = r.u32()? as usize;
    let bs_version = r.u32()?;
    if version != 0x1402_0007 || user_version != 12 || (bs_version != 100 && bs_version != LE_BS_VERSION) {
        return Err(Error::Unsupported { version, user_version, bs_version });
    }
    let author = r.export_string()?;
    let process_script = r.export_string()?;
    let export_script = r.export_string()?;
    let num_types = r.u16()? as usize;
    let block_types = (0..num_types).map(|_| r.sized_string()).collect::<Result<Vec<_>>>()?;
    let block_type_index = (0..num_blocks).map(|_| r.u16().map(|v| v & 0x7FFF)).collect::<Result<Vec<_>>>()?;
    if block_type_index.iter().any(|&t| t as usize >= block_types.len()) {
        return Err(Error::Truncated { offset: r.pos(), what: "block type index out of range" });
    }
    let block_sizes = (0..num_blocks).map(|_| r.u32()).collect::<Result<Vec<_>>>()?;
    let num_strings = r.u32()? as usize;
    let _max_len = r.u32()?;
    let strings = (0..num_strings).map(|_| r.sized_string()).collect::<Result<Vec<_>>>()?;
    let num_groups = r.u32()? as usize;
    if num_groups > r.rest().len() / 4 {
        return Err(Error::Truncated { offset: r.pos(), what: "groups" });
    }
    let groups = (0..num_groups).map(|_| r.u32()).collect::<Result<Vec<_>>>()?;
    Ok(Header { header_line, version, endian, user_version, bs_version, author, process_script, export_script, block_types, block_type_index, block_sizes, strings, groups })
}

fn string_ref(r: &mut Reader<'_>, h: &Header) -> Result<Option<String>> {
    let i = r.i32()?;
    Ok((i >= 0).then(|| h.strings.get(i as usize).cloned().unwrap_or_default()))
}

fn refs(r: &mut Reader<'_>) -> Result<Vec<Ref>> {
    let n = r.u32()? as usize;
    if n > r.rest().len() / 4 {
        return Err(Error::Truncated { offset: r.pos(), what: "ref list" });
    }
    (0..n).map(|_| r.i32().map(to_ref)).collect()
}

fn object_net(r: &mut Reader<'_>, h: &Header) -> Result<ObjectNet> {
    Ok(ObjectNet { name: string_ref(r, h)?, extra_data: refs(r)?, controller: to_ref(r.i32()?) })
}

fn av_object(r: &mut Reader<'_>, h: &Header) -> Result<AvObject> {
    let net = object_net(r, h)?;
    let flags = r.u32()?;
    let translation = r.vec3()?;
    let mut rotation = [[0.0; 3]; 3];
    for row in &mut rotation {
        *row = r.vec3()?;
    }
    let scale = r.f32()?;
    let collision = to_ref(r.i32()?);
    Ok(AvObject { net, flags, transform: Transform { translation, rotation, scale }, collision })
}

/// Decodes one block. The flag is false when the decoded fields do not capture the block
/// completely (the caller then keeps the raw bytes for writing).
fn parse_block(r: &mut Reader<'_>, type_name: &str, h: &Header) -> Result<(Block, bool)> {
    let mut faithful = true;
    let block = match type_name {
        t if NODE_TYPES.contains(&t) => {
            let av = av_object(r, h)?;
            let children = refs(r)?;
            let effects = refs(r)?;
            let (multi_bound, culling_mode) = if t == "BSMultiBoundNode" { (to_ref(r.i32()?), Some(r.u32()?)) } else { (None, None) };
            Block::Node(Node { av, children, effects, multi_bound, culling_mode })
        }
        "BSTriShape" | "BSMeshLODTriShape" | "BSDynamicTriShape" | "BSSubIndexTriShape" => {
            let (s, f) = tri_shape(r, h, type_name)?;
            faithful = f;
            Block::TriShape(s)
        }
        "BSLightingShaderProperty" => {
            let shader_type = r.u32()?;
            let net = object_net(r, h)?;
            let mut s = LightingShader { shader_type, net, ..Default::default() };
            s.flags1 = r.u32()?;
            s.flags2 = r.u32()?;
            s.uv_offset = [r.f32()?, r.f32()?];
            s.uv_scale = [r.f32()?, r.f32()?];
            s.texture_set = to_ref(r.i32()?);
            s.emissive_color = r.vec3()?;
            s.emissive_multiple = r.f32()?;
            s.texture_clamp_mode = r.u32()?;
            s.alpha = r.f32()?;
            s.refraction_strength = r.f32()?;
            s.glossiness = r.f32()?;
            s.specular_color = r.vec3()?;
            s.specular_strength = r.f32()?;
            s.lighting_effect_1 = r.f32()?;
            s.lighting_effect_2 = r.f32()?;
            s.extra = match shader_type {
                1 => LightingExtra::EnvironmentMap { scale: r.f32()? },
                5 => LightingExtra::SkinTint(r.vec3()?),
                6 => LightingExtra::HairTint(r.vec3()?),
                7 => LightingExtra::ParallaxOcclusion { max_passes: r.f32()?, scale: r.f32()? },
                11 => LightingExtra::MultiLayerParallax { inner_layer_thickness: r.f32()?, refraction_scale: r.f32()?, inner_layer_texture_scale: [r.f32()?, r.f32()?], envmap_strength: r.f32()? },
                14 => LightingExtra::SparkleSnow([r.f32()?, r.f32()?, r.f32()?, r.f32()?]),
                16 => LightingExtra::EyeEnvironment { cubemap_scale: r.f32()?, left: r.vec3()?, right: r.vec3()? },
                _ => LightingExtra::None,
            };
            Block::LightingShader(s)
        }
        "BSEffectShaderProperty" => {
            let net = object_net(r, h)?;
            let flags1 = r.u32()?;
            let flags2 = r.u32()?;
            let uv_offset = [r.f32()?, r.f32()?];
            let uv_scale = [r.f32()?, r.f32()?];
            let source_texture = r.sized_string()?;
            let mut e = EffectShader { net, flags1, flags2, uv_offset, uv_scale, source_texture, ..Default::default() };
            // SSE / LE tail (nif.xml: four bytes where LE files store a u32 clamp mode).
            if r.rest().len() >= 4 + 16 + 16 + 12 {
                e.texture_clamp_mode = r.u8()?;
                e.lighting_influence = r.u8()?;
                e.env_map_min_lod = r.u8()?;
                e.unknown = r.u8()?;
                e.falloff = [r.f32()?, r.f32()?, r.f32()?, r.f32()?];
                e.emissive_color = [r.f32()?, r.f32()?, r.f32()?, r.f32()?];
                e.emissive_multiple = r.f32()?;
                e.soft_falloff_depth = r.f32()?;
                e.greyscale_texture = r.sized_string()?;
            }
            Block::EffectShader(e)
        }
        "BSShaderTextureSet" => {
            let n = r.u32()? as usize;
            if n > r.rest().len() / 4 {
                return Err(Error::Truncated { offset: r.pos(), what: "texture count" });
            }
            Block::TextureSet((0..n).map(|_| r.sized_string()).collect::<Result<_>>()?)
        }
        "NiAlphaProperty" => {
            let net = object_net(r, h)?;
            Block::Alpha { net, flags: r.u16()?, threshold: r.u8()? }
        }
        "BSMultiBound" => Block::MultiBound { data: to_ref(r.i32()?) },
        "BSMultiBoundAABB" => Block::MultiBoundAabb { position: r.vec3()?, extent: r.vec3()? },
        "BSDistantObjectLargeRefExtraData" => Block::LargeRefExtra { name: string_ref(r, h)?, large_ref: r.u8()? },
        _ => Block::Unknown,
    };
    Ok((block, faithful))
}

/// NiGeometryData + NiTriShapeData / NiTriStripsData (Skyrim LE).
#[derive(Debug, Default)]
struct LeGeometryData {
    vertices: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    tangents: Vec<[f32; 3]>,
    bitangents: Vec<[f32; 3]>,
    center: [f32; 3],
    radius: f32,
    colors: Vec<[f32; 4]>,
    uvs: Vec<[f32; 2]>,
    triangles: Vec<[u16; 3]>,
}

/// NiTriShape / NiTriStrips (NiGeometry, Skyrim LE): NiAVObject, data ref, skin ref, material
/// data (count, names, extra data, active material, dirty flag), shader and alpha property refs.
fn le_tri_shape(r: &mut Reader<'_>, h: &Header) -> Result<(TriShape, Ref)> {
    let mut s = TriShape { av: av_object(r, h)?, legacy: true, ..Default::default() };
    let data = to_ref(r.i32()?);
    s.skin = to_ref(r.i32()?);
    let n = r.u32()? as usize;
    if n > r.rest().len() / 8 {
        return Err(Error::Truncated { offset: r.pos(), what: "material count" });
    }
    r.skip(n * 8)?;
    let _active_material = r.i32()?;
    let _dirty = r.u8()?;
    s.shader_property = to_ref(r.i32()?);
    s.alpha_property = to_ref(r.i32()?);
    Ok((s, data))
}

fn le_vec3s(r: &mut Reader<'_>, n: usize, what: &'static str) -> Result<Vec<[f32; 3]>> {
    if n * 12 > r.rest().len() {
        return Err(Error::Truncated { offset: r.pos(), what });
    }
    (0..n).map(|_| r.vec3()).collect()
}

/// NiGeometryData + NiTriShapeData / NiTriStripsData per nif.xml; strips are unrolled into
/// triangles (alternating winding, degenerate triangles skipped).
fn le_geometry_data(r: &mut Reader<'_>, strips: bool) -> Result<LeGeometryData> {
    let mut d = LeGeometryData::default();
    let _group_id = r.i32()?;
    let n = r.u16()? as usize;
    let _keep = r.u8()?;
    let _compress = r.u8()?;
    if r.u8()? != 0 {
        d.vertices = le_vec3s(r, n, "vertices")?;
    }
    let uv_sets = r.u8()?;
    let extra_vector_flags = r.u8()?;
    let _material_crc = r.u32()?;
    if r.u8()? != 0 {
        d.normals = le_vec3s(r, n, "normals")?;
        if extra_vector_flags & 0x10 != 0 {
            d.tangents = le_vec3s(r, n, "tangents")?;
            d.bitangents = le_vec3s(r, n, "bitangents")?;
        }
    }
    d.center = r.vec3()?;
    d.radius = r.f32()?;
    if r.u8()? != 0 {
        if n * 16 > r.rest().len() {
            return Err(Error::Truncated { offset: r.pos(), what: "vertex colours" });
        }
        d.colors = (0..n).map(|_| Ok([r.f32()?, r.f32()?, r.f32()?, r.f32()?])).collect::<Result<_>>()?;
    }
    if uv_sets & 1 == 1 {
        if n * 8 > r.rest().len() {
            return Err(Error::Truncated { offset: r.pos(), what: "uvs" });
        }
        d.uvs = (0..n).map(|_| Ok([r.f32()?, r.f32()?])).collect::<Result<_>>()?;
    }
    let _consistency = r.u16()?;
    let _additional = r.i32()?;
    let num_tris = r.u16()? as usize;
    if strips {
        let num_strips = r.u16()? as usize;
        let lengths = (0..num_strips).map(|_| r.u16().map(usize::from)).collect::<Result<Vec<_>>>()?;
        if r.u8()? != 0 {
            for len in lengths {
                let p = (0..len).map(|_| r.u16()).collect::<Result<Vec<_>>>()?;
                if p.len() < 2 {
                    continue;
                }
                let (mut a, mut b) = (p[0], p[1]);
                let mut flip = false;
                for &c in &p[2..] {
                    let o = a;
                    a = b;
                    b = c;
                    if o != a && a != b && b != o {
                        d.triangles.push(if flip { [o, b, a] } else { [o, a, b] });
                    }
                    flip = !flip;
                }
            }
        }
    } else {
        let _num_points = r.u32()?;
        if r.u8()? != 0 {
            if num_tris * 6 > r.rest().len() {
                return Err(Error::Truncated { offset: r.pos(), what: "triangles" });
            }
            d.triangles = (0..num_tris).map(|_| Ok([r.u16()?, r.u16()?, r.u16()?])).collect::<Result<_>>()?;
        }
        let groups = r.u16()? as usize;
        for _ in 0..groups {
            let m = r.u16()? as usize;
            r.skip(m * 2)?;
        }
    }
    if d.triangles.iter().flatten().any(|&i| i as usize >= n) {
        return Err(Error::Truncated { offset: r.pos(), what: "triangle index out of range" });
    }
    Ok(d)
}

/// Attribute flags the writer can encode.
const ENCODABLE_ATTRIBUTES: u16 = va::VERTEX | va::UVS | va::NORMALS | va::TANGENTS | va::COLORS | va::FULL_PRECISION;

fn tri_shape(r: &mut Reader<'_>, h: &Header, type_name: &str) -> Result<(TriShape, bool)> {
    let mut s = TriShape { av: av_object(r, h)?, ..Default::default() };
    s.bound_center = r.vec3()?;
    s.bound_radius = r.f32()?;
    s.skin = to_ref(r.i32()?);
    s.shader_property = to_ref(r.i32()?);
    s.alpha_property = to_ref(r.i32()?);
    s.vertex_desc = r.u64()?;
    let desc = VertexDesc(s.vertex_desc);
    let mut faithful = desc.attributes() & !ENCODABLE_ATTRIBUTES == 0;
    let num_tris = r.u16()? as usize;
    let num_verts = r.u16()? as usize;
    let data_size = r.u32()? as usize;
    if data_size > 0 {
        let stride = desc.stride();
        let layout = desc.layout();
        if layout.size != stride {
            return Err(Error::VertexLayout { desc: s.vertex_desc, computed: layout.size, stride });
        }
        faithful &= data_size == num_verts * stride + num_tris * 6;
        let vdata = r.bytes(num_verts * stride)?;
        let mut bitangent = false;
        for v in vdata.chunks_exact(stride.max(1)) {
            let mut b = [0.0f32; 3];
            if let Some(o) = layout.position {
                s.positions.push([f32_le(v, o), f32_le(v, o + 4), f32_le(v, o + 8)]);
                b[0] = f32_le(v, o + 12);
            }
            if let Some(o) = layout.uv {
                s.uvs.push([half_to_f32(u16_le(v, o)), half_to_f32(u16_le(v, o + 2))]);
            }
            if let Some(o) = layout.normal {
                s.normals.push([byte_to_float(v[o]), byte_to_float(v[o + 1]), byte_to_float(v[o + 2])]);
                b[1] = byte_to_float(v[o + 3]);
            }
            if let Some(o) = layout.tangent {
                s.tangents.push([byte_to_float(v[o]), byte_to_float(v[o + 1]), byte_to_float(v[o + 2])]);
                b[2] = byte_to_float(v[o + 3]);
            }
            if let Some(o) = layout.color {
                s.colors.push([v[o], v[o + 1], v[o + 2], v[o + 3]]);
            }
            s.bitangents.push(b);
            // A missing normal/tangent slot is written back as FloatToByte(0) = 128; anything that
            // would not reproduce the stored bytes keeps the bitangents.
            bitangent |= b[0].to_bits() != 0 || (layout.normal.is_some() && v[layout.normal.unwrap() + 3] != 128) || (layout.tangent.is_some() && v[layout.tangent.unwrap() + 3] != 128);
        }
        if !bitangent {
            s.bitangents.clear();
        }
        s.triangles = (0..num_tris).map(|_| Ok([r.u16()?, r.u16()?, r.u16()?])).collect::<Result<_>>()?;
        if s.triangles.iter().flatten().any(|&i| i as usize >= num_verts) {
            return Err(Error::Truncated { offset: r.pos(), what: "triangle index out of range" });
        }
    } else {
        faithful &= num_verts == 0 && num_tris == 0;
    }
    // SSE: particle data size u32 (+ data), then subclass fields.
    let particle_size = r.u32()? as usize;
    if particle_size > 0 {
        faithful = false;
        r.skip(num_verts * 6 + num_tris * 6)?;
    }
    match type_name {
        "BSMeshLODTriShape" => s.lod_sizes = Some([r.u32()?, r.u32()?, r.u32()?]),
        "BSSubIndexTriShape" => {
            let n = r.u32()? as usize;
            if n > r.rest().len() / 9 {
                return Err(Error::Truncated { offset: r.pos(), what: "segment count" });
            }
            s.segments = Some((0..n).map(|_| Ok(Segment { flags: r.u8()?, start: r.u32()?, count: r.u32()? })).collect::<Result<_>>()?);
        }
        "BSDynamicTriShape" => faithful = false,
        _ => {}
    }
    Ok((s, faithful))
}

fn u16_le(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn f32_le(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// nif.xml `normbyte` ("a float in the range -1.0:1.0, stored as a byte"): `b / 255 * 2 - 1`.
pub fn byte_to_float(b: u8) -> f32 {
    f32::from(b) / 255.0 * 2.0 - 1.0
}

/// Inverse of [`byte_to_float`]: clamp to [-1, 1], then `round((f + 1) / 2 * 255)` (midpoints
/// away from zero). Reproduces every byte that went through [`byte_to_float`].
pub fn float_to_byte(f: f32) -> u8 {
    let f = f.clamp(-1.0, 1.0);
    let v = (f + 1.0) / 2.0 * 255.0;
    f64::from(v).round() as u8
}

/// IEEE 754 half to f32.
pub fn half_to_f32(h: u16) -> f32 {
    let sign = u32::from(h >> 15) << 31;
    let exp = u32::from((h >> 10) & 0x1F);
    let mant = u32::from(h & 0x3FF);
    let bits = match (exp, mant) {
        (0, 0) => sign,
        (0, m) => {
            // subnormal: normalize
            let mut e = 127 - 15 + 1;
            let mut m = m;
            while m & 0x400 == 0 {
                m <<= 1;
                e -= 1;
            }
            sign | (e << 23) | ((m & 0x3FF) << 13)
        }
        (31, m) => sign | 0x7F80_0000 | (m << 13),
        (e, m) => sign | ((e + 127 - 15) << 23) | (m << 13),
    };
    f32::from_bits(bits)
}

/// f32 to IEEE 754 half, round to nearest, ties to even (the `half` crate). Every value a half
/// can hold converts back to the same bits; the terrain UVs (`k / 4096` with k a multiple of
/// 128 / level, and `1 - k / 4096`) are all exactly representable, so no rounding occurs for them.
pub fn f32_to_half(f: f32) -> u16 {
    half::f16::from_f32(f).to_bits()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halves() {
        assert_eq!(half_to_f32(0x3C00), 1.0);
        assert_eq!(half_to_f32(0xC000), -2.0);
        assert_eq!(half_to_f32(0x3800), 0.5);
        assert_eq!(half_to_f32(0x0001), 5.960_464_5e-8);
    }

    #[test]
    fn half_round_trip_all() {
        // Every finite half decodes to an f32 that encodes back to the same bits.
        for h in 0..=u16::MAX {
            if (h >> 10) & 0x1F == 0x1F {
                continue;
            }
            assert_eq!(f32_to_half(half_to_f32(h)), h, "half {h:#06x}");
        }
    }

    #[test]
    fn half_rounding_is_ieee() {
        assert_eq!(f32_to_half(1.0), 0x3C00);
        assert_eq!(f32_to_half(-2.0), 0xC000);
        // 1 + 2^-11 is exactly between 0x3C00 and 0x3C01: ties go to even.
        assert_eq!(f32_to_half(1.0 + 1.0 / 2048.0), 0x3C00);
        assert_eq!(f32_to_half(1.0 + 3.0 / 2048.0), 0x3C02);
        assert_eq!(f32_to_half(1.0 + 1.0 / 4096.0), 0x3C00);
        // Values beyond the largest finite half become infinity.
        assert_eq!(f32_to_half(65519.0), 0x7BFF);
        assert_eq!(f32_to_half(65520.0), 0x7C00);
        assert_eq!(f32_to_half(70000.0), 0x7C00);
        assert_eq!(f32_to_half(f32::INFINITY), 0x7C00);
        assert_eq!(f32_to_half(-0.0), 0x8000);
        assert_eq!(f32_to_half(1e-9), 0x0000);
        // Subnormal halves: 2^-24 and its rounding neighbourhood.
        assert_eq!(f32_to_half(5.960_464_5e-8), 0x0001);
        assert_eq!(f32_to_half(2.0f32.powi(-25) * 1.5), 0x0001);
        // Terrain UVs: 1/32 steps across a quad.
        assert_eq!(f32_to_half(0.03125), 0x2800);
        assert_eq!(f32_to_half(0.5), 0x3800);
        assert_eq!(f32_to_half(1.0 / 3.0), 0x3555);
    }

    #[test]
    fn normal_bytes_round_trip() {
        for b in 0..=255u8 {
            assert_eq!(float_to_byte(byte_to_float(b)), b);
        }
        assert_eq!(float_to_byte(0.0), 128);
        assert_eq!(float_to_byte(-1.0), 0);
        assert_eq!(float_to_byte(1.0), 255);
        assert_eq!(float_to_byte(7.0), 255);
    }

    #[test]
    fn vertex_desc() {
        // pos(16) uv@16 normal@20 tangent@24 color@28, stride 32: nibbles 8 | uv 4 | normal 5 | tangent 6 | color 7.
        let d = VertexDesc(0x0765_0408 | (u64::from(va::VERTEX | va::UVS | va::NORMALS | va::TANGENTS | va::COLORS) << 44));
        assert_eq!(d.stride(), 32);
        assert_eq!(d.color_offset(), 28);
        assert_eq!(d.uv_offset(), 16);
        assert_eq!(d.normal_offset(), 20);
        assert_eq!(d.tangent_offset(), 24);
        assert!(d.has(va::COLORS));
        let l = d.layout();
        assert_eq!((l.uv, l.normal, l.tangent, l.color, l.size), (Some(16), Some(20), Some(24), Some(28), 32));
    }

    #[test]
    fn sse_desc_matches_observed_values() {
        let d = VertexDesc::sse(va::VERTEX | va::UVS);
        assert_eq!(d.0, 0x0000_3000_0000_0405);
        assert_eq!(d.stride(), 20);
        assert_eq!(d.layout().size, 20);
        let d = VertexDesc::sse(va::VERTEX);
        assert_eq!(d.0, 0x0000_1000_0000_0404);
        assert_eq!((d.stride(), d.layout().size), (16, 16));
        let d = VertexDesc::with_attributes(true, true, true, true, true);
        assert_eq!(d.0, 0x0003_b000_0765_0408);
        assert_eq!(d.layout().size, d.stride());
    }

    /// NiGeometryData header + per-vertex arrays for an LE data block test.
    fn le_data_bytes(verts: &[[f32; 3]], with_tangents: bool, colors: bool) -> Vec<u8> {
        let mut b = Vec::new();
        let f = |b: &mut Vec<u8>, v: f32| b.extend_from_slice(&v.to_le_bytes());
        b.extend_from_slice(&0i32.to_le_bytes()); // group id
        b.extend_from_slice(&(verts.len() as u16).to_le_bytes());
        b.extend_from_slice(&[0, 0, 1]); // keep, compress, has vertices
        for v in verts {
            v.iter().for_each(|&c| f(&mut b, c));
        }
        b.extend_from_slice(&[1, if with_tangents { 0x10 } else { 0 }]); // uv sets, extra vector flags
        b.extend_from_slice(&0u32.to_le_bytes()); // material crc
        b.push(1); // has normals
        for _ in verts {
            [0.0, 0.0, 1.0].iter().for_each(|&c| f(&mut b, c));
        }
        if with_tangents {
            for _ in 0..2 * verts.len() {
                [1.0, 0.0, 0.0].iter().for_each(|&c| f(&mut b, c));
            }
        }
        [0.5, 0.5, 0.0, 2.0].iter().for_each(|&c| f(&mut b, c)); // center, radius
        b.push(u8::from(colors));
        if colors {
            for _ in verts {
                [0.25, 0.5, 1.0, 1.0].iter().for_each(|&c| f(&mut b, c));
            }
        }
        for v in verts {
            f(&mut b, v[0]);
            f(&mut b, v[1]);
        }
        b.extend_from_slice(&0u16.to_le_bytes()); // consistency
        b.extend_from_slice(&(-1i32).to_le_bytes()); // additional data
        b
    }

    #[test]
    fn le_tri_shape_data() {
        let verts = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let mut b = le_data_bytes(&verts, true, true);
        b.extend_from_slice(&1u16.to_le_bytes()); // triangles
        b.extend_from_slice(&3u32.to_le_bytes()); // points
        b.push(1);
        [0u16, 1, 2].iter().for_each(|i| b.extend_from_slice(&i.to_le_bytes()));
        b.extend_from_slice(&0u16.to_le_bytes()); // match groups
        let mut r = Reader::new(&b);
        let d = le_geometry_data(&mut r, false).unwrap();
        assert!(r.rest().is_empty());
        assert_eq!(d.vertices, verts);
        assert_eq!((d.tangents.len(), d.bitangents.len()), (3, 3));
        assert_eq!(d.colors[0], [0.25, 0.5, 1.0, 1.0]);
        assert_eq!(d.uvs[1], [1.0, 0.0]);
        assert_eq!(d.triangles, vec![[0, 1, 2]]);
    }

    #[test]
    fn le_strips_become_triangles() {
        let verts = [[0.0; 3]; 5];
        let mut b = le_data_bytes(&verts, false, false);
        b.extend_from_slice(&3u16.to_le_bytes()); // triangles
        b.extend_from_slice(&1u16.to_le_bytes()); // strips
        b.extend_from_slice(&6u16.to_le_bytes()); // strip length
        b.push(1);
        // 0 1 2 3 3 4: the repeated 3 makes two degenerate triangles that are skipped.
        [0u16, 1, 2, 3, 3, 4].iter().for_each(|i| b.extend_from_slice(&i.to_le_bytes()));
        let mut r = Reader::new(&b);
        let d = le_geometry_data(&mut r, true).unwrap();
        assert!(d.tangents.is_empty() && d.colors.is_empty());
        assert_eq!(d.triangles, vec![[0, 1, 2], [1, 3, 2]]);
    }

    #[test]
    fn stale_offsets_use_flags() {
        // From vanilla dlc2horker01.nif: no UV flag, but the stored nibbles still count a UV slot.
        let d = VertexDesc(0x0001_9000_0065_0406);
        assert_eq!(d.tangent_offset(), 24); // stored, wrong
        let l = d.layout();
        assert_eq!((l.position, l.uv, l.normal, l.tangent, l.size), (Some(0), None, Some(16), Some(20), 24));
        assert_eq!(l.size, d.stride());
    }
}
