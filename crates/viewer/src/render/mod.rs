// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! wgpu rendering of loaded LOD: mesh pipelines (opaque, double-sided, blended), classic tree
//! billboards, overlay lines and the selection wireframe, over a reversed-Z depth buffer so the
//! whole of Tamriel fits one frustum.

pub mod textures;

use std::ops::Range;
use std::sync::Arc;

use wgpu::util::DeviceExt;

use crate::mesh::{ShapeKind, ShapeMesh};
use textures::{GpuTexture, TexCache};
use crate::files::Files;

pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// Mesh vertex (36 bytes): world position, UV, packed normal / tangent / bitangent, colour.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Vertex {
    pub pos: [f32; 3],
    pub uv: [f32; 2],
    pub normal: [i8; 4],
    pub tangent: [i8; 4],
    pub bitangent: [i8; 4],
    pub color: [u8; 4],
}

impl Vertex {
    pub const LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
        array_stride: 36,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &[
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x3,
                offset: 0,
                shader_location: 0,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x2,
                offset: 12,
                shader_location: 1,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Snorm8x4,
                offset: 20,
                shader_location: 2,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Snorm8x4,
                offset: 24,
                shader_location: 3,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Snorm8x4,
                offset: 28,
                shader_location: 4,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Unorm8x4,
                offset: 32,
                shader_location: 5,
            },
        ],
    };
}

fn snorm(v: [f32; 3]) -> [i8; 4] {
    let q = |x: f32| (x.clamp(-1.0, 1.0) * 127.0).round() as i8;
    [q(v[0]), q(v[1]), q(v[2]), 0]
}

/// Packs a shape's vertices.
pub fn vertices_of(s: &ShapeMesh) -> Vec<Vertex> {
    (0..s.positions.len())
        .map(|i| Vertex {
            pos: s.positions[i],
            uv: s.uvs.get(i).copied().unwrap_or([0.0, 0.0]),
            normal: snorm(s.normals.get(i).copied().unwrap_or([0.0, 0.0, 1.0])),
            tangent: snorm(s.tangents.get(i).copied().unwrap_or([1.0, 0.0, 0.0])),
            bitangent: snorm(s.bitangents.get(i).copied().unwrap_or([0.0, 1.0, 0.0])),
            color: s.colors.get(i).copied().unwrap_or([255; 4]),
        })
        .collect()
}

/// Material as the shaders read it (64 bytes).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct MaterialGpu {
    pub flags: u32,
    pub threshold: f32,
    pub emissive_mult: f32,
    pub kind_level: u32,
    pub emissive: [f32; 4],
    pub uv: [f32; 4],
    pub tint: [f32; 4],
}

pub const F_ALPHA_TEST: u32 = 1;
pub const F_DOUBLE_SIDED: u32 = 2;
pub const F_VERTEX_COLORS: u32 = 4;
pub const F_NORMAL_MAP: u32 = 8;
pub const F_EMIT: u32 = 16;
pub const F_UNLIT: u32 = 32;
pub const F_TERRAIN: u32 = 64;
pub const F_WATER: u32 = 128;
pub const F_LARGE_REF: u32 = 256;
pub const F_HAS_NORMALS: u32 = 1024;
pub const F_HAS_TANGENTS: u32 = 2048;
pub const F_CLAMP: u32 = 4096;

pub fn kind_index(k: ShapeKind) -> u32 {
    match k {
        ShapeKind::Terrain => 0,
        ShapeKind::Water => 1,
        ShapeKind::Object => 2,
        ShapeKind::TreeBillboard => 3,
        ShapeKind::Grass => 4,
        ShapeKind::Model => 5,
    }
}

impl MaterialGpu {
    pub fn of(s: &ShapeMesh, level_index: u32, has_normal_map: bool) -> MaterialGpu {
        let m = &s.material;
        let mut flags = 0;
        if m.alpha_test {
            flags |= F_ALPHA_TEST;
        }
        if m.double_sided {
            flags |= F_DOUBLE_SIDED;
        }
        if m.vertex_colors && !s.colors.is_empty() {
            flags |= F_VERTEX_COLORS;
        }
        if has_normal_map {
            flags |= F_NORMAL_MAP;
        }
        if (m.own_emit || m.glow_map) && m.emissive_multiple > 0.0 {
            flags |= F_EMIT;
        }
        if m.effect {
            flags |= F_UNLIT;
        }
        if s.kind == ShapeKind::Terrain {
            flags |= F_TERRAIN;
        }
        if s.kind == ShapeKind::Water {
            flags |= F_WATER;
        }
        if m.large_ref {
            flags |= F_LARGE_REF;
        }
        if !s.normals.is_empty() {
            flags |= F_HAS_NORMALS;
        }
        if !s.tangents.is_empty() && !s.bitangents.is_empty() {
            flags |= F_HAS_TANGENTS;
        }
        if m.clamp {
            flags |= F_CLAMP;
        }
        MaterialGpu {
            flags,
            threshold: f32::from(m.threshold) / 255.0,
            emissive_mult: m.emissive_multiple,
            kind_level: kind_index(s.kind) | (level_index.min(3) << 8) | (m.snow.index() << 16),
            emissive: [m.emissive[0], m.emissive[1], m.emissive[2], 0.0],
            uv: [m.uv_scale[0], m.uv_scale[1], m.uv_offset[0], m.uv_offset[1]],
            tint: [0.0; 4],
        }
    }
}

/// Per-frame constants (matches `Globals` in the shader).
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct GlobalsGpu {
    pub view_proj: [[f32; 4]; 4],
    pub cam_pos: [f32; 4],
    pub sun_dir: [f32; 4],
    pub params: [f32; 4],
    pub level_colors: [[f32; 4]; 4],
    pub kind_colors: [[f32; 4]; 6],
    /// Snow / ash LOD materials, see `snow::SnowMaterials::gpu`.
    pub snow: [[f32; 4]; 12],
    /// Inverse of `view_proj` (sky directions).
    pub inv_view_proj: [[f32; 4]; 4],
    /// Weather and water, see `atmosphere::Atmosphere::gpu`; `atmo_on.x` = 1 when used.
    pub atmo: [[f32; 4]; 11],
    pub atmo_on: [f32; 4],
}

/// Shading modes (`params.x`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Shading {
    Textured,
    Unlit,
    Normals,
    Flat,
    Level,
    Kind,
    /// The sampled normal map, as stored.
    NormalMap,
}

impl Shading {
    pub const ALL: [Shading; 7] = [
        Shading::Textured,
        Shading::Unlit,
        Shading::Normals,
        Shading::Flat,
        Shading::Level,
        Shading::Kind,
        Shading::NormalMap,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Shading::Textured => "textured",
            Shading::Unlit => "unlit",
            Shading::Normals => "normals",
            Shading::Flat => "flat",
            Shading::Level => "by level",
            Shading::Kind => "by kind",
            Shading::NormalMap => "normal map",
        }
    }

    pub fn index(self) -> f32 {
        Shading::ALL.iter().position(|s| *s == self).unwrap_or(0) as f32
    }

    pub fn parse(s: &str) -> Option<Shading> {
        Shading::ALL.into_iter().find(|m| {
            m.label().eq_ignore_ascii_case(s)
                || m.label().replace(' ', "_").eq_ignore_ascii_case(s)
                || m.label().replace("by ", "").eq_ignore_ascii_case(s)
                || m.label().replace(' ', "").eq_ignore_ascii_case(s)
        })
    }
}

pub const LEVEL_COLORS: [[f32; 4]; 4] = [
    [0.35, 0.75, 0.35, 1.0],
    [0.85, 0.75, 0.3, 1.0],
    [0.85, 0.45, 0.25, 1.0],
    [0.6, 0.3, 0.7, 1.0],
];
pub const KIND_COLORS: [[f32; 4]; 6] = [
    [0.55, 0.5, 0.4, 1.0],
    [0.2, 0.4, 0.8, 1.0],
    [0.75, 0.75, 0.75, 1.0],
    [0.2, 0.6, 0.2, 1.0],
    [0.6, 0.85, 0.3, 1.0],
    [0.9, 0.5, 0.9, 1.0],
];

/// Overlay line vertex.
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LineVertex {
    pub pos: [f32; 3],
    pub color: [u8; 4],
}

impl LineVertex {
    pub const LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
        array_stride: 16,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &[
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Float32x3,
                offset: 0,
                shader_location: 0,
            },
            wgpu::VertexAttribute {
                format: wgpu::VertexFormat::Unorm8x4,
                offset: 12,
                shader_location: 1,
            },
        ],
    };
}

const TREE_INSTANCE_LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
    array_stride: std::mem::size_of::<crate::trees::TreeInstance>() as u64,
    step_mode: wgpu::VertexStepMode::Instance,
    attributes: &[
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x3,
            offset: 0,
            shader_location: 0,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32,
            offset: 12,
            shader_location: 1,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x2,
            offset: 16,
            shader_location: 2,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x2,
            offset: 24,
            shader_location: 3,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x2,
            offset: 32,
            shader_location: 4,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Uint32,
            offset: 40,
            shader_location: 5,
        },
    ],
};

/// Bind group layouts, sampler and placeholder textures, shared with the loader threads.
#[derive(Clone)]
pub struct Layouts {
    pub globals: wgpu::BindGroupLayout,
    pub materials: wgpu::BindGroupLayout,
    pub textures: wgpu::BindGroupLayout,
    pub sampler: wgpu::Sampler,
    /// Clamp-to-edge twin of `sampler` for shapes with `TextureClampMode` 0.
    pub sampler_clamp: wgpu::Sampler,
    pub white: Arc<GpuTexture>,
    /// 50% grey (the projected noise before the game's texture is loaded).
    pub grey: Arc<GpuTexture>,
    pub flat_normal: Arc<GpuTexture>,
    pub missing: Arc<GpuTexture>,
}

/// A diffuse + normal pair bound together.
pub struct TextureBindGroup {
    pub bind_group: wgpu::BindGroup,
    pub diffuse: Option<Arc<GpuTexture>>,
    pub normal: Option<Arc<GpuTexture>>,
}

impl Layouts {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Layouts {
        let globals_tex = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let globals = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // The game's projected-material textures (snow / ash LOD).
                globals_tex(1),
                globals_tex(2),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let materials = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("materials"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let tex = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let textures = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("textures"),
            entries: &[
                tex(0),
                tex(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("lod"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            anisotropy_clamp: 8,
            ..Default::default()
        });
        let sampler_clamp = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("lod clamp"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            anisotropy_clamp: 8,
            ..Default::default()
        });
        Layouts {
            globals,
            materials,
            textures,
            sampler,
            sampler_clamp,
            white: Arc::new(textures::solid(
                device,
                queue,
                [255, 255, 255, 255],
                true,
                "white",
            )),
            grey: Arc::new(textures::solid(device, queue, [128, 128, 128, 255], false, "grey")),
            flat_normal: Arc::new(textures::solid(
                device,
                queue,
                [128, 128, 255, 255],
                false,
                "flat normal",
            )),
            missing: Arc::new(textures::missing_checker(device, queue)),
        }
    }

    /// Binds a diffuse / normal pair; `None` diffuse shows the missing checker, `None` normal a
    /// flat normal.
    pub fn texture_bind_group(
        &self,
        device: &wgpu::Device,
        diffuse: Option<Arc<GpuTexture>>,
        normal: Option<Arc<GpuTexture>>,
        missing_is_white: bool,
    ) -> TextureBindGroup {
        let d = diffuse.clone().unwrap_or_else(|| {
            if missing_is_white {
                self.white.clone()
            } else {
                self.missing.clone()
            }
        });
        let n = normal.clone().unwrap_or_else(|| self.flat_normal.clone());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("textures"),
            layout: &self.textures,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&d.view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&n.view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&self.sampler_clamp),
                },
            ],
        });
        TextureBindGroup {
            bind_group,
            diffuse,
            normal,
        }
    }

    pub fn materials_bind_group(
        &self,
        device: &wgpu::Device,
        buffer: &wgpu::Buffer,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("materials"),
            layout: &self.materials,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        })
    }
}

/// One shape to draw.
pub struct DrawShape<'a> {
    pub vbuf: &'a wgpu::Buffer,
    pub ibuf: &'a wgpu::Buffer,
    pub materials: &'a wgpu::BindGroup,
    pub textures: &'a wgpu::BindGroup,
    pub indices: Range<u32>,
    pub material_index: u32,
    pub blend: bool,
    pub double_sided: bool,
}

/// Everything one frame draws.
#[derive(Default)]
pub struct Frame<'a> {
    pub shapes: Vec<DrawShape<'a>>,
    /// Tree billboard instances, their count and the atlas bind group.
    pub trees: Option<(&'a wgpu::Buffer, u32, &'a wgpu::BindGroup)>,
    /// Lines tested against depth.
    pub lines: Vec<LineVertex>,
    /// Lines drawn over everything.
    pub overlay: Vec<LineVertex>,
    /// Shapes drawn again as a wireframe (selection); needs the line polygon mode.
    pub wire: Vec<DrawShape<'a>>,
    pub clear: [f64; 4],
    /// Draw the weather's sky gradient behind everything (instead of the clear colour alone).
    pub sky: bool,
}

struct Targets {
    width: u32,
    height: u32,
    msaa: Option<wgpu::TextureView>,
    depth: wgpu::TextureView,
}

pub struct Renderer {
    pub layouts: Layouts,
    pub samples: u32,
    pub color_format: wgpu::TextureFormat,
    pub wireframe_supported: bool,
    mesh_cull: wgpu::RenderPipeline,
    mesh_nocull: wgpu::RenderPipeline,
    mesh_blend: wgpu::RenderPipeline,
    tree: wgpu::RenderPipeline,
    sky: wgpu::RenderPipeline,
    line_depth: wgpu::RenderPipeline,
    line_overlay: wgpu::RenderPipeline,
    wire: Option<wgpu::RenderPipeline>,
    globals_buf: wgpu::Buffer,
    globals_bg: wgpu::BindGroup,
    tree_quad: wgpu::Buffer,
    targets: Option<Targets>,
}

impl Renderer {
    pub fn new(
        device: &wgpu::Device,
        layouts: Layouts,
        color_format: wgpu::TextureFormat,
        samples: u32,
    ) -> Renderer {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("viewer"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders.wgsl").into()),
        });
        let mesh_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mesh"),
            bind_group_layouts: &[&layouts.globals, &layouts.materials, &layouts.textures],
            push_constant_ranges: &[],
        });
        let tree_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("tree"),
            bind_group_layouts: &[&layouts.globals, &layouts.textures],
            push_constant_ranges: &[],
        });
        let line_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("line"),
            bind_group_layouts: &[&layouts.globals],
            push_constant_ranges: &[],
        });
        let depth = |write: bool, compare: wgpu::CompareFunction| {
            Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: write,
                depth_compare: compare,
                stencil: Default::default(),
                bias: Default::default(),
            })
        };
        let multisample = wgpu::MultisampleState {
            count: samples,
            mask: !0,
            alpha_to_coverage_enabled: false,
        };
        let target = |blend: Option<wgpu::BlendState>| {
            Some(wgpu::ColorTargetState {
                format: color_format,
                blend,
                write_mask: wgpu::ColorWrites::ALL,
            })
        };
        let mesh = |label: &str,
                    cull: Option<wgpu::Face>,
                    blend: bool,
                    polygon: wgpu::PolygonMode,
                    fs: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&mesh_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_mesh"),
                    buffers: &[Vertex::LAYOUT],
                    compilation_options: Default::default(),
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: cull,
                    polygon_mode: polygon,
                    unclipped_depth: false,
                    conservative: false,
                },
                depth_stencil: depth(
                    !blend,
                    if polygon == wgpu::PolygonMode::Line {
                        wgpu::CompareFunction::Always
                    } else {
                        wgpu::CompareFunction::GreaterEqual
                    },
                ),
                multisample,
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(fs),
                    targets: &[target(blend.then_some(wgpu::BlendState::ALPHA_BLENDING))],
                    compilation_options: Default::default(),
                }),
                multiview: None,
                cache: None,
            })
        };
        let wireframe_supported = device
            .features()
            .contains(wgpu::Features::POLYGON_MODE_LINE);
        let mesh_cull = mesh(
            "mesh cull",
            Some(wgpu::Face::Back),
            false,
            wgpu::PolygonMode::Fill,
            "fs_mesh",
        );
        let mesh_nocull = mesh(
            "mesh double-sided",
            None,
            false,
            wgpu::PolygonMode::Fill,
            "fs_mesh",
        );
        let mesh_blend = mesh("mesh blend", None, true, wgpu::PolygonMode::Fill, "fs_mesh");
        let wire = wireframe_supported
            .then(|| mesh("wire", None, false, wgpu::PolygonMode::Line, "fs_solid"));
        let tree = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("tree"),
            layout: Some(&tree_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_tree"),
                buffers: &[TREE_INSTANCE_LAYOUT],
                compilation_options: Default::default(),
            },
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: depth(true, wgpu::CompareFunction::GreaterEqual),
            multisample,
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_tree"),
                targets: &[target(None)],
                compilation_options: Default::default(),
            }),
            multiview: None,
            cache: None,
        });
        let line = |label: &str, compare: wgpu::CompareFunction| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&line_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs_line"),
                    buffers: &[LineVertex::LAYOUT],
                    compilation_options: Default::default(),
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::LineList,
                    strip_index_format: None,
                    front_face: wgpu::FrontFace::Ccw,
                    cull_mode: None,
                    polygon_mode: wgpu::PolygonMode::Fill,
                    unclipped_depth: false,
                    conservative: false,
                },
                depth_stencil: depth(false, compare),
                multisample,
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs_line"),
                    targets: &[target(Some(wgpu::BlendState::ALPHA_BLENDING))],
                    compilation_options: Default::default(),
                }),
                multiview: None,
                cache: None,
            })
        };
        let sky = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sky"),
            layout: Some(&line_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs_sky"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: depth(false, wgpu::CompareFunction::Always),
            multisample,
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs_sky"),
                targets: &[target(None)],
                compilation_options: Default::default(),
            }),
            multiview: None,
            cache: None,
        });
        let line_depth = line("line depth", wgpu::CompareFunction::GreaterEqual);
        let line_overlay = line("line overlay", wgpu::CompareFunction::Always);
        let globals_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<GlobalsGpu>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_bg = globals_bind_group(device, &layouts, &globals_buf, &layouts.white, &layouts.grey);
        let tree_quad = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("tree quad"),
            contents: bytemuck::cast_slice(&[0u16, 1, 2, 2, 1, 3]),
            usage: wgpu::BufferUsages::INDEX,
        });
        Renderer {
            layouts,
            samples,
            color_format,
            wireframe_supported,
            mesh_cull,
            mesh_nocull,
            mesh_blend,
            tree,
            sky,
            line_depth,
            line_overlay,
            wire,
            globals_buf,
            globals_bg,
            tree_quad,
            targets: None,
        }
    }

    /// Binds the game's projected diffuse and noise textures (snow / ash LOD) from the load
    /// order; keeps the placeholders when they are absent.
    pub fn set_projected(&mut self, device: &wgpu::Device, cache: &TexCache, files: &dyn Files) {
        let diffuse = cache.get(files, crate::snow::PROJECTED_DIFFUSE, true);
        let noise = cache.get(files, crate::snow::PROJECTED_NOISE, false);
        let d = diffuse.unwrap_or_else(|| self.layouts.white.clone());
        let n = noise.unwrap_or_else(|| self.layouts.grey.clone());
        self.globals_bg = globals_bind_group(device, &self.layouts, &self.globals_buf, &d, &n);
    }

    fn targets(&mut self, device: &wgpu::Device, width: u32, height: u32) -> &Targets {
        if self
            .targets
            .as_ref()
            .is_none_or(|t| t.width != width || t.height != height)
        {
            let size = wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            };
            let msaa = (self.samples > 1).then(|| {
                device
                    .create_texture(&wgpu::TextureDescriptor {
                        label: Some("msaa"),
                        size,
                        mip_level_count: 1,
                        sample_count: self.samples,
                        dimension: wgpu::TextureDimension::D2,
                        format: self.color_format,
                        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                        view_formats: &[],
                    })
                    .create_view(&Default::default())
            });
            let depth = device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("depth"),
                    size,
                    mip_level_count: 1,
                    sample_count: self.samples,
                    dimension: wgpu::TextureDimension::D2,
                    format: DEPTH_FORMAT,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                })
                .create_view(&Default::default());
            self.targets = Some(Targets {
                width,
                height,
                msaa,
                depth,
            });
        }
        self.targets.as_ref().unwrap()
    }

    /// Draws a frame into `target` (resolving MSAA), clearing colour and depth first.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        width: u32,
        height: u32,
        globals: &GlobalsGpu,
        frame: &Frame<'_>,
    ) {
        queue.write_buffer(&self.globals_buf, 0, bytemuck::bytes_of(globals));
        let line_buf = |v: &[LineVertex]| {
            (!v.is_empty()).then(|| {
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: Some("lines"),
                    contents: bytemuck::cast_slice(v),
                    usage: wgpu::BufferUsages::VERTEX,
                })
            })
        };
        let (lines, overlay) = (line_buf(&frame.lines), line_buf(&frame.overlay));
        let t = self.targets(device, width.max(1), height.max(1));
        let (view, resolve) = match &t.msaa {
            Some(m) => (m, Some(target)),
            None => (target, None),
        };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("scene"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: resolve,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color {
                        r: frame.clear[0],
                        g: frame.clear[1],
                        b: frame.clear[2],
                        a: frame.clear[3],
                    }),
                    store: if resolve.is_some() {
                        wgpu::StoreOp::Discard
                    } else {
                        wgpu::StoreOp::Store
                    },
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &t.depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Clear(0.0),
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        pass.set_bind_group(0, &self.globals_bg, &[]);
        if frame.sky {
            pass.set_pipeline(&self.sky);
            pass.draw(0..3, 0..1);
        }
        // Opaque: single-sided, then double-sided; blended last.
        for (pipeline, pick) in [
            (&self.mesh_cull, 0u8),
            (&self.mesh_nocull, 1),
            (&self.mesh_blend, 2),
        ] {
            pass.set_pipeline(pipeline);
            let mut last_tex: *const wgpu::BindGroup = std::ptr::null();
            let mut last_mat: *const wgpu::BindGroup = std::ptr::null();
            let mut last_v: *const wgpu::Buffer = std::ptr::null();
            for s in frame.shapes.iter().filter(|s| match pick {
                0 => !s.blend && !s.double_sided,
                1 => !s.blend && s.double_sided,
                _ => s.blend,
            }) {
                if !std::ptr::eq(s.materials, last_mat) {
                    pass.set_bind_group(1, s.materials, &[]);
                    last_mat = s.materials;
                }
                if !std::ptr::eq(s.textures, last_tex) {
                    pass.set_bind_group(2, s.textures, &[]);
                    last_tex = s.textures;
                }
                if !std::ptr::eq(s.vbuf, last_v) {
                    pass.set_vertex_buffer(0, s.vbuf.slice(..));
                    pass.set_index_buffer(s.ibuf.slice(..), wgpu::IndexFormat::Uint32);
                    last_v = s.vbuf;
                }
                pass.draw_indexed(s.indices.clone(), 0, s.material_index..s.material_index + 1);
            }
            if pick == 1 {
                if let Some((inst, count, atlas)) = frame.trees {
                    if count > 0 {
                        pass.set_pipeline(&self.tree);
                        pass.set_bind_group(1, atlas, &[]);
                        pass.set_vertex_buffer(0, inst.slice(..));
                        pass.set_index_buffer(self.tree_quad.slice(..), wgpu::IndexFormat::Uint16);
                        pass.draw_indexed(0..6, 0, 0..count);
                    }
                }
            }
        }
        if let Some(b) = &lines {
            pass.set_pipeline(&self.line_depth);
            pass.set_vertex_buffer(0, b.slice(..));
            pass.draw(0..frame.lines.len() as u32, 0..1);
        }
        if let (Some(w), false) = (&self.wire, frame.wire.is_empty()) {
            pass.set_pipeline(w);
            for s in &frame.wire {
                pass.set_bind_group(1, s.materials, &[]);
                pass.set_bind_group(2, s.textures, &[]);
                pass.set_vertex_buffer(0, s.vbuf.slice(..));
                pass.set_index_buffer(s.ibuf.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(s.indices.clone(), 0, s.material_index..s.material_index + 1);
            }
        }
        if let Some(b) = &overlay {
            pass.set_pipeline(&self.line_overlay);
            pass.set_vertex_buffer(0, b.slice(..));
            pass.draw(0..frame.overlay.len() as u32, 0..1);
        }
    }
}

/// Line segments of an axis-aligned box.
fn globals_bind_group(
    device: &wgpu::Device,
    layouts: &Layouts,
    buffer: &wgpu::Buffer,
    projected_diffuse: &GpuTexture,
    projected_noise: &GpuTexture,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("globals"),
        layout: &layouts.globals,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&projected_diffuse.view),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::TextureView(&projected_noise.view),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::Sampler(&layouts.sampler),
            },
        ],
    })
}

pub fn box_lines(min: [f32; 3], max: [f32; 3], color: [u8; 4], out: &mut Vec<LineVertex>) {
    let c = |x: usize, y: usize, z: usize| LineVertex {
        pos: [
            if x == 0 { min[0] } else { max[0] },
            if y == 0 { min[1] } else { max[1] },
            if z == 0 { min[2] } else { max[2] },
        ],
        color,
    };
    for z in 0..2 {
        out.extend([
            c(0, 0, z),
            c(1, 0, z),
            c(1, 0, z),
            c(1, 1, z),
            c(1, 1, z),
            c(0, 1, z),
            c(0, 1, z),
            c(0, 0, z),
        ]);
    }
    for (x, y) in [(0, 0), (1, 0), (1, 1), (0, 1)] {
        out.extend([c(x, y, 0), c(x, y, 1)]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_match_struct_sizes() {
        assert_eq!(std::mem::size_of::<Vertex>(), 36);
        assert_eq!(std::mem::size_of::<MaterialGpu>(), 64);
        assert_eq!(std::mem::size_of::<LineVertex>(), 16);
        assert_eq!(std::mem::size_of::<crate::trees::TreeInstance>(), 48);
        assert_eq!(std::mem::size_of::<GlobalsGpu>() % 16, 0);
        assert_eq!(snorm([1.0, -1.0, 0.0]), [127, -127, 0, 0]);
        assert_eq!(Shading::parse("by level"), Some(Shading::Level));
        assert_eq!(Shading::parse("kind"), Some(Shading::Kind));
    }
}
