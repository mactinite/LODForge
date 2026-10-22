//! wgpu renderer for billboards: orthographic views of a [`Scene`] into an MSAA target,
//! resolved to RGBA8 (alpha = sample coverage).

use std::collections::HashMap;

use wgpu::util::DeviceExt;

use crate::scene::{Mesh, Scene};

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    rx: [f32; 4],
    ry: [f32; 4],
    rz: [f32; 4],
    /// Light direction (model space, toward the light) and ambient.
    light: [f32; 4],
    /// Direction toward the camera (model space) and direct. The normal pass derives its view
    /// basis from the camera direction (image right = up x toward-camera, image up = +z).
    eye: [f32; 4],
    /// Alpha-test threshold (0..1, < 0 none), pass (0 diffuse, 1 normal bake), PBR scale, flags.
    params: [f32; 4],
    /// Specular colour and strength.
    spec: [f32; 4],
    /// Glossiness, lighting effect 1, lighting effect 2.
    spec2: [f32; 4],
    /// Emissive colour times multiple (own emit only).
    emit: [f32; 4],
}

/// Material flags in `Uniforms::params.w` (mirrored by the `FLAG_*` constants of
/// `billboard.wgsl`).
mod flag {
    pub const SPECULAR: u32 = 1;
    /// Soft lighting with the slot-2 texture as its mask.
    pub const SOFT: u32 = 2;
    /// Rim lighting with the slot-2 texture as its mask.
    pub const RIM: u32 = 4;
    /// The slot-2 texture is a glow map (Glow_Map shader flag) and own-emit applies through it.
    pub const GLOW_MAP: u32 = 8;
    pub const TANGENTS: u32 = 16;
    pub const PBR: u32 = 32;
    pub const COMPLEX: u32 = 64;
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    pos: [f32; 3],
    uv: [f32; 2],
    n: [f32; 3],
    t: [f32; 3],
    b: [f32; 3],
    c: [f32; 4],
}

/// Decoded texture: RGBA8 mip chain (level 0 first), plus the DDS's own BC blocks when it is block
/// compressed (uploaded as is, so the GPU decodes it like TexGen's compressed GL upload).
pub struct TexData {
    pub width: u32,
    pub height: u32,
    pub mips: Vec<Vec<u8>>,
    pub bc: Option<(wgpu::TextureFormat, Vec<Vec<u8>>)>,
}

impl TexData {
    /// Loads a DDS: every consistent mip level (halving sizes), decoded to RGBA8 only when it is
    /// not block compressed (or `decode` is set).
    pub fn from_dds(bytes: &[u8], decode: bool) -> Option<TexData> {
        let info = lodforge_dds::parse(bytes).ok()?;
        let bc_format = match info.format {
            lodforge_dds::Format::Bc1 => Some(wgpu::TextureFormat::Bc1RgbaUnorm),
            lodforge_dds::Format::Bc2 => Some(wgpu::TextureFormat::Bc2RgbaUnorm),
            lodforge_dds::Format::Bc3 => Some(wgpu::TextureFormat::Bc3RgbaUnorm),
            lodforge_dds::Format::Bc4 => Some(wgpu::TextureFormat::Bc4RUnorm),
            lodforge_dds::Format::Bc5 => Some(wgpu::TextureFormat::Bc5RgUnorm),
            lodforge_dds::Format::Bc7 => Some(wgpu::TextureFormat::Bc7RgbaUnorm),
            _ => None,
        }
        .filter(|_| info.width % 4 == 0 && info.height % 4 == 0 && !decode);
        let (w, h) = (info.width, info.height);
        let levels: Vec<(usize, usize, u32, u32)> = info.mips().into_iter().enumerate().take_while(|(i, m)| m.2 == (w >> i).max(1) && m.3 == (h >> i).max(1) && m.0 + m.1 <= bytes.len()).map(|(_, m)| m).collect();
        if levels.is_empty() {
            return None;
        }
        if let Some(f) = bc_format {
            let blocks = levels.iter().map(|&(o, s, _, _)| bytes[o..o + s].to_vec()).collect();
            return Some(TexData { width: w, height: h, mips: Vec::new(), bc: Some((f, blocks)) });
        }
        let mut mips = Vec::new();
        for lvl in 0..levels.len() as u32 {
            mips.push(lodforge_dds::decode(bytes, &info, lvl).ok()?.rgba);
        }
        Some(TexData { width: w, height: h, mips, bc: None })
    }

    /// [`TexData::from_dds`], keeping BC blocks when `gpu` can sample them.
    pub fn for_gpu(gpu: &lodforge_gpu::Gpu, bytes: &[u8]) -> Option<TexData> {
        TexData::from_dds(bytes, !gpu.device.features().contains(wgpu::Features::TEXTURE_COMPRESSION_BC))
    }

    fn level_count(&self) -> u32 {
        self.bc.as_ref().map(|b| b.1.len()).unwrap_or(self.mips.len()) as u32
    }
}

/// Orthographic view: image right = -X (camera on +Y looking at -Y) rotated by `yaw`
/// degrees about Z, image up = +Z. `x0..x1` is the horizontal range in view units (left to
/// right), `z0..z1` the vertical range.
#[derive(Debug, Clone, Copy)]
pub struct View {
    pub yaw_deg: f32,
    pub x0: f32,
    pub x1: f32,
    pub z0: f32,
    pub z1: f32,
    pub width: u32,
    pub height: u32,
}

/// Lighting of one billboard type (TexGen `*BillboardsDirect`, `Ambient`, light direction).
#[derive(Debug, Clone, Copy)]
pub struct Light {
    pub direct: f32,
    pub ambient: f32,
    /// Toward the light, model space (z up).
    pub dir: [f32; 3],
    /// Colour scale for PBR materials (`BillboardPBRScale`).
    pub pbr_scale: f32,
}

/// What a render produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Pass {
    /// Lit colour (TexGen `bakeDiffuse`).
    #[default]
    Diffuse,
    /// Normal texture of the view for `_n` textures: red = right, green = down, blue = toward
    /// the viewer, as the game's normal maps (`billboard.wgsl` has the exact encoding);
    /// independent of the billboard's lighting (HD grass has direct 0 and still gets normals).
    Normal,
}

/// TexGen's `MaxMultiSamples` default; capped by what the adapter supports.
pub const MAX_SAMPLES: u32 = 16;

/// Billboards are rendered at this many times their size per axis and averaged down. The alpha
/// test discards whole pixels, so MSAA alone leaves foliage edges inside a texture hard and
/// stair-stepped (thin needles turn into pixel noise); averaging SUPERSAMPLE^2 samples gives every
/// edge texel its real coverage. MSAA is off in this mode (the supersampling covers triangle edges
/// too).
pub const SUPERSAMPLE: u32 = 4;

pub struct Renderer {
    /// MSAA sample count of the render target.
    pub samples: u32,
    pipelines: [wgpu::RenderPipeline; 2],
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    flat: wgpu::TextureView,
    black: wgpu::TextureView,
    /// Uploaded source textures by path, and their size in bytes.
    cache: std::cell::RefCell<(HashMap<String, wgpu::TextureView>, u64)>,
}

/// Horizontal view coordinate of a model-space point for a yaw (degrees).
pub fn view_x(p: [f32; 3], yaw_deg: f32) -> f32 {
    let (s, c) = yaw_deg.to_radians().sin_cos();
    // yaw 0: view x = -X; yaw 90: view x = -Y (side view).
    -(p[0] * c + p[1] * s)
}

fn view_depth(p: [f32; 3], yaw_deg: f32) -> f32 {
    let (s, c) = yaw_deg.to_radians().sin_cos();
    // Toward the camera = larger.
    p[1] * c - p[0] * s
}

fn tex_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry { binding, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None }
}

fn upload(gpu: &lodforge_gpu::Gpu, t: &TexData) -> wgpu::Texture {
    let tex = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("source"),
        size: wgpu::Extent3d { width: t.width, height: t.height, depth_or_array_layers: 1 },
        mip_level_count: t.level_count(),
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: t.bc.as_ref().map(|b| b.0).unwrap_or(wgpu::TextureFormat::Rgba8Unorm),
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for lvl in 0..t.level_count() as usize {
        let (mw, mh) = ((t.width >> lvl).max(1), (t.height >> lvl).max(1));
        let (data, bpr, rows, ew, eh) = match &t.bc {
            // Block rows; the copy covers the physical (block-rounded) size.
            Some((f, blocks)) => {
                let unit = f.block_copy_size(None).unwrap_or(16);
                (&blocks[lvl], mw.div_ceil(4) * unit, mh.div_ceil(4), mw.div_ceil(4) * 4, mh.div_ceil(4) * 4)
            }
            None => (&t.mips[lvl], mw * 4, mh, mw, mh),
        };
        gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo { texture: &tex, mip_level: lvl as u32, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            data,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(bpr), rows_per_image: Some(rows) },
            wgpu::Extent3d { width: ew, height: eh, depth_or_array_layers: 1 },
        );
    }
    tex
}

fn solid(gpu: &lodforge_gpu::Gpu, rgba: [u8; 4]) -> wgpu::TextureView {
    upload(gpu, &TexData { width: 1, height: 1, mips: vec![rgba.to_vec()], bc: None }).create_view(&Default::default())
}

/// Material uniforms of a mesh (TexGen's `LoadScene` shader parameters): specular, glossiness and
/// lighting effects, own-emit colour, flags.
fn material(m: &Mesh, textures: &HashMap<String, TexData>) -> ([f32; 4], [f32; 4], [f32; 4], u32) {
    let mut flags = 0;
    let (mut spec, mut spec2, mut emit) = ([0.0; 4], [0.0; 4], [0.0; 4]);
    let has_slot2 = !m.glow.is_empty() && textures.contains_key(&m.glow);
    if let Some(l) = &m.shader {
        if l.flags1 & 1 != 0 {
            flags |= flag::SPECULAR;
        }
        // Own_Emit: emissive colour times multiple is added to the light.
        if l.flags1 & (1 << 22) != 0 {
            emit = [l.emissive_color[0] * l.emissive_multiple, l.emissive_color[1] * l.emissive_multiple, l.emissive_color[2] * l.emissive_multiple, 0.0];
        }
        // Glow_Map: the slot-2 texture is the emissive map.
        if l.flags2 & (1 << 6) != 0 && has_slot2 {
            flags |= flag::GLOW_MAP;
        }
        if l.flags2 & (1 << 25) != 0 {
            flags |= flag::SOFT;
        }
        if l.flags2 & (1 << 26) != 0 {
            flags |= flag::RIM;
        }
        spec = [l.specular_color[0], l.specular_color[1], l.specular_color[2], l.specular_strength];
        spec2 = [l.glossiness, l.lighting_effect_1, l.lighting_effect_2, 0.0];
    }
    if !m.tangents.is_empty() && !m.bitangents.is_empty() {
        flags |= flag::TANGENTS;
    }
    if m.diffuse.contains(r"\pbr\") {
        flags |= flag::PBR;
    }
    if m.complex {
        flags |= flag::COMPLEX;
    }
    // `TG_MAT=1`: the material values of every draw (shading diagnostics, as `TG_FIT` / `TG_PNG`
    // in the CLI).
    if std::env::var_os("TG_MAT").is_some() {
        eprintln!("MAT\t{}\tflags {flags:#b}\tspec {spec:?}\tgloss/e1/e2 {spec2:?}\temit {emit:?}\tglow {}\talpha {:?}\tdouble {}", m.diffuse, m.glow, m.alpha_test, m.double_sided);
    }
    (spec, spec2, emit, flags)
}

impl Renderer {
    pub fn new(gpu: &lodforge_gpu::Gpu) -> Renderer {
        let d = &gpu.device;
        let samples = if SUPERSAMPLE > 1 { 1 } else { gpu.max_msaa.min(MAX_SAMPLES) };
        // `LODFORGE_BILLBOARD_WGSL=<file>` replaces the built-in shader at run time (tuning the
        // shading against golden billboards without a rebuild).
        let source = std::env::var_os("LODFORGE_BILLBOARD_WGSL").and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_else(|| include_str!("billboard.wgsl").to_string());
        let module = d.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("billboard"), source: wgpu::ShaderSource::Wgsl(source.into()) });
        let layout = d.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("billboard"),
            entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::VERTEX_FRAGMENT, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
                tex_entry(1),
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
                tex_entry(3),
                tex_entry(4),
            ],
        });
        let pl = d.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("billboard"), bind_group_layouts: &[&layout], push_constant_ranges: &[] });
        let make = |cull_mode: Option<wgpu::Face>, front_face: wgpu::FrontFace| {
            d.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("billboard"),
                layout: Some(&pl),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some("vs"),
                    compilation_options: Default::default(),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32x3, 3 => Float32x3, 4 => Float32x3, 5 => Float32x4],
                    }],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rgba8Unorm, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                }),
                primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, cull_mode, front_face, ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState { format: wgpu::TextureFormat::Depth32Float, depth_write_enabled: true, depth_compare: wgpu::CompareFunction::Less, stencil: Default::default(), bias: Default::default() }),
                multisample: wgpu::MultisampleState { count: samples, mask: !0, alpha_to_coverage_enabled: false },
                multiview: None,
                cache: None,
            })
        };
        // [0] two-sided, [1] back faces culled: TexGen culls shapes without the Double_Sided flag
        // (counter-clockwise front faces in this projection).
        let pipelines = [make(None, wgpu::FrontFace::Ccw), make(Some(wgpu::Face::Back), wgpu::FrontFace::Ccw)];
        let sampler = d.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("billboard"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            // TexGen samples the source mips with anisotropic filtering: without it the alpha-tested
            // foliage of the golden billboards is matched only with a ~-1 mip bias, with it the mean
            // width and height ratios are 1.000 (docs/baseline.md P8 step 2).
            anisotropy_clamp: 16,
            ..Default::default()
        });
        // Missing normal map: flat (+z); missing glow map: black.
        let flat = solid(gpu, [128, 128, 255, 255]);
        let black = solid(gpu, [0, 0, 0, 255]);
        Renderer { samples, pipelines, layout, sampler, flat, black, cache: Default::default() }
    }

    /// Renders `scene` for `view` ([`Pass::Diffuse`]).
    pub fn render(&self, gpu: &lodforge_gpu::Gpu, scene: &Scene, textures: &HashMap<String, TexData>, view: View, light: Light) -> Vec<u8> {
        self.render_pass(gpu, scene, textures, view, light, Pass::Diffuse)
    }

    /// One render of `kind`; see [`Renderer::render_many`].
    pub fn render_pass(&self, gpu: &lodforge_gpu::Gpu, scene: &Scene, textures: &HashMap<String, TexData>, view: View, light: Light, kind: Pass) -> Vec<u8> {
        self.render_many(gpu, &[Job { scene, textures, view, light, pass: kind }]).pop().unwrap_or_default()
    }

    /// GPU views of the jobs' textures, uploading the ones not cached yet. The cache is keyed by
    /// Data-relative path and emptied when it grows past [`TEXTURE_CACHE_BYTES`].
    fn texture_views(&self, gpu: &lodforge_gpu::Gpu, jobs: &[Job]) -> HashMap<String, wgpu::TextureView> {
        let mut cache = self.cache.borrow_mut();
        let mut out = HashMap::new();
        for job in jobs {
            for m in &job.scene.meshes {
                for path in [&m.diffuse, &m.normal, &m.glow] {
                    if path.is_empty() || out.contains_key(path) {
                        continue;
                    }
                    if let Some(v) = cache.0.get(path) {
                        out.insert(path.clone(), v.clone());
                    } else if let Some(t) = job.textures.get(path) {
                        let bytes: u64 = t.bc.as_ref().map(|b| b.1.iter().map(|l| l.len() as u64).sum()).unwrap_or_else(|| t.mips.iter().map(|l| l.len() as u64).sum());
                        if cache.1 + bytes > TEXTURE_CACHE_BYTES {
                            cache.0.clear();
                            cache.1 = 0;
                        }
                        let v = upload(gpu, t).create_view(&Default::default());
                        cache.0.insert(path.clone(), v.clone());
                        cache.1 += bytes;
                        out.insert(path.clone(), v);
                    }
                }
            }
        }
        out
    }

    /// Renders every job in one command buffer and one readback wait (one GPU round trip).
    /// Each result is RGBA8 rows (top row first) where alpha is the MSAA coverage of opaque samples
    /// and RGB the coverage-weighted colour of the job's pass.
    pub fn render_many(&self, gpu: &lodforge_gpu::Gpu, jobs: &[Job]) -> Vec<Vec<u8>> {
        let d = &gpu.device;
        let q = &gpu.queue;
        let views = self.texture_views(gpu, jobs);
        let mut enc = d.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("billboards") });
        let mut readbacks = Vec::with_capacity(jobs.len());
        for job in jobs {
            let (scene, view, light, kind) = (job.scene, job.view, job.light, job.pass);
            let ss = SUPERSAMPLE.max(1);
            let (w, h) = (view.width.max(1) * ss, view.height.max(1) * ss);
            let size = wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 };
            let msaa = (self.samples > 1).then(|| d.create_texture(&wgpu::TextureDescriptor { label: Some("msaa"), size, mip_level_count: 1, sample_count: self.samples, dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Rgba8Unorm, usage: wgpu::TextureUsages::RENDER_ATTACHMENT, view_formats: &[] }));
            let resolve = d.create_texture(&wgpu::TextureDescriptor { label: Some("resolve"), size, mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Rgba8Unorm, usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, view_formats: &[] });
            let depth = d.create_texture(&wgpu::TextureDescriptor { label: Some("depth"), size, mip_level_count: 1, sample_count: self.samples, dimension: wgpu::TextureDimension::D2, format: wgpu::TextureFormat::Depth32Float, usage: wgpu::TextureUsages::RENDER_ATTACHMENT, view_formats: &[] });
            let (mv, rv, dv) = (msaa.as_ref().map(|m| m.create_view(&Default::default())), resolve.create_view(&Default::default()), depth.create_view(&Default::default()));

            // Depth range over the whole scene.
            let (mut dmin, mut dmax) = (f32::MAX, f32::MIN);
            for m in &scene.meshes {
                for p in &m.positions {
                    let z = view_depth(*p, view.yaw_deg);
                    dmin = dmin.min(z);
                    dmax = dmax.max(z);
                }
            }
            let dspan = (dmax - dmin).max(1.0);
            let (s, c) = view.yaw_deg.to_radians().sin_cos();
            let sx = 2.0 / (view.x1 - view.x0);
            let sz = 2.0 / (view.z1 - view.z0);
            // clip x = (view_x - x0) * sx - 1, view_x = -(X c + Y s)
            let rx = [-c * sx, -s * sx, 0.0, -view.x0 * sx - 1.0];
            let ry = [0.0, 0.0, sz, -view.z0 * sz - 1.0];
            // depth: 0 nearest (largest view depth) .. 1 farthest
            let rz = [s / dspan, -c / dspan, 0.0, dmax / dspan];
            // Toward the camera (the direction of increasing view depth).
            let eye = [-s, c, 0.0, light.direct];

            struct Draw {
                vb: wgpu::Buffer,
                ib: wgpu::Buffer,
                count: u32,
                bind: wgpu::BindGroup,
                two_sided: bool,
            }
            let mut draws = Vec::new();
            for m in &scene.meshes {
                let Some(tv) = views.get(&m.diffuse) else { continue };
                let nv = views.get(&m.normal).unwrap_or(&self.flat);
                let gv = views.get(&m.glow).unwrap_or(&self.black);
                let zero = [0.0; 3];
                let verts: Vec<Vertex> = (0..m.positions.len())
                    .map(|i| Vertex { pos: m.positions[i], uv: m.uvs.get(i).copied().unwrap_or_default(), n: m.normals[i], t: m.tangents.get(i).copied().unwrap_or(zero), b: m.bitangents.get(i).copied().unwrap_or(zero), c: m.colors[i] })
                    .collect();
                let vb = d.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::cast_slice(&verts), usage: wgpu::BufferUsages::VERTEX });
                let ib = d.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::cast_slice(&m.indices), usage: wgpu::BufferUsages::INDEX });
                let (spec, spec2, emit, flags) = material(m, job.textures);
                let u = Uniforms {
                    rx,
                    ry,
                    rz,
                    light: [light.dir[0], light.dir[1], light.dir[2], light.ambient],
                    eye,
                    params: [m.alpha_test.map(|t| f32::from(t) / 255.0).unwrap_or(-1.0), if kind == Pass::Normal { 1.0 } else { 0.0 }, light.pbr_scale, flags as f32],
                    spec,
                    spec2,
                    emit,
                };
                let ub = d.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::bytes_of(&u), usage: wgpu::BufferUsages::UNIFORM });
                let bind = d.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout: &self.layout,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: ub.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(tv) },
                        wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                        wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(nv) },
                        wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(gv) },
                    ],
                });
                draws.push(Draw { vb, ib, count: m.indices.len() as u32, bind, two_sided: m.double_sided });
            }

            let row = (w * 4).div_ceil(256) * 256;
            let readback = d.create_buffer(&wgpu::BufferDescriptor { label: Some("readback"), size: u64::from(row * h), usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });
            {
                let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("billboard"),
                    color_attachments: &[Some(match &mv {
                        Some(mv) => wgpu::RenderPassColorAttachment { view: mv, resolve_target: Some(&rv), ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store } },
                        None => wgpu::RenderPassColorAttachment { view: &rv, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store } },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: &dv, depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }), stencil_ops: None }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                });
                for dr in &draws {
                    pass.set_pipeline(&self.pipelines[usize::from(!dr.two_sided)]);
                    pass.set_bind_group(0, &dr.bind, &[]);
                    pass.set_vertex_buffer(0, dr.vb.slice(..));
                    pass.set_index_buffer(dr.ib.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..dr.count, 0, 0..1);
                }
            }
            enc.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo { texture: &resolve, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                wgpu::TexelCopyBufferInfo { buffer: &readback, layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(h) } },
                size,
            );
            readbacks.push((readback, row, w, h));
        }
        q.submit([enc.finish()]);
        let (tx, rx_) = std::sync::mpsc::channel();
        for (k, (buf, ..)) in readbacks.iter().enumerate() {
            let tx = tx.clone();
            buf.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send((k, r));
            });
        }
        let _ = d.poll(wgpu::PollType::Wait);
        for _ in 0..readbacks.len() {
            let _ = rx_.recv();
        }
        readbacks
            .iter()
            .map(|(buf, row, w, h)| {
                let data = buf.slice(..).get_mapped_range();
                let mut out = vec![0u8; (w * h * 4) as usize];
                for y in 0..*h as usize {
                    // Row 0 of the texture is the top of the image (clip y = +1).
                    out[y * *w as usize * 4..(y + 1) * *w as usize * 4].copy_from_slice(&data[y * *row as usize..y * *row as usize + *w as usize * 4]);
                }
                downsample(&out, *w, *h, SUPERSAMPLE.max(1))
            })
            .collect()
    }
}

/// Averages `ss` x `ss` blocks of a supersampled render, as an MSAA resolve does: alpha is the
/// mean (the texel's coverage) and RGB the mean of colour times alpha (premultiplied; the
/// background clears to transparent black), which [crate::finish_alpha] un-premultiplies.
pub fn downsample(px: &[u8], w: u32, h: u32, ss: u32) -> Vec<u8> {
    if ss <= 1 {
        return px.to_vec();
    }
    let (ow, oh) = (w / ss, h / ss);
    let n = (ss * ss) as u32;
    let mut out = vec![0u8; (ow * oh * 4) as usize];
    for oy in 0..oh {
        for ox in 0..ow {
            let mut acc = [0u32; 4];
            for y in oy * ss..(oy + 1) * ss {
                for x in ox * ss..(ox + 1) * ss {
                    let i = ((y * w + x) * 4) as usize;
                    let a = u32::from(px[i + 3]);
                    for c in 0..3 {
                        acc[c] += u32::from(px[i + c]) * a;
                    }
                    acc[3] += a;
                }
            }
            let o = ((oy * ow + ox) * 4) as usize;
            for c in 0..3 {
                out[o + c] = ((acc[c] + n * 255 / 2) / (n * 255)) as u8;
            }
            out[o + 3] = ((acc[3] + n / 2) / n) as u8;
        }
    }
    out
}

/// Upper bound of the renderer's texture cache.
pub const TEXTURE_CACHE_BYTES: u64 = 1 << 30;

/// One render of a [`Renderer::render_many`] batch.
#[derive(Clone, Copy)]
pub struct Job<'a> {
    pub scene: &'a Scene,
    pub textures: &'a HashMap<String, TexData>,
    pub view: View,
    pub light: Light,
    pub pass: Pass,
}
