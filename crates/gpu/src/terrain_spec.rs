//! Host side of terrain_spec.wgsl: render cells once (diffuse or normal), Lanczos-downscale them
//! into every LOD level of one top-level (LOD32) tile.

use std::sync::mpsc;

use wgpu::util::DeviceExt;

use crate::{Gpu, Result};

pub const NONE: u32 = 0xFFFF_FFFF;

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SpecCell {
    pub vc_offset: u32,
    /// Normal kind: offset of the cell's 64x64 terrain normal in CellBatch::terrain.
    pub terrain_offset: u32,
    pub _pad: [u32; 2],
    /// Texture index per quadrant (default already substituted).
    pub base: [u32; 4],
    pub layer_start: [u32; 4],
    pub layer_count: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SpecLayer {
    pub tex: u32,
    /// Offset of the layer's 33x33 splat mask in `CellBatch::splats`.
    pub splat_offset: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Placement {
    /// Cell position inside the top tile, in cells (0..32).
    pub x: u32,
    pub y: u32,
    /// Rendered cell index within the batch.
    pub slot: u32,
    pub _pad: u32,
}

/// Cells to render plus where their results go.
#[derive(Debug, Default, Clone)]
pub struct CellBatch {
    pub cells: Vec<SpecCell>,
    pub layers: Vec<SpecLayer>,
    pub splats: Vec<f32>,
    /// 33x33 RGBA8 per cell with vertex colours.
    pub vcolors: Vec<u32>,
    /// Normal kind: 64x64 RGBA8 terrain normal per rendered cell (R = x, G = z, B = y), north-up.
    pub terrain: Vec<u32>,
    pub placements: Vec<Placement>,
}

#[derive(Debug, Clone, Copy)]
pub struct LevelSpec {
    pub level: u32,
    pub ppc: u32,
    pub gamma: f32,
    pub brightness: f32,
    pub contrast: f32,
}

/// One level's tiles for a top tile, RGBA8, tile after tile (row-major by tile, south first).
pub struct LevelImage {
    pub level: u32,
    pub tile_size: u32,
    pub tiles_per_side: u32,
    pub rgba: Vec<u8>,
}

impl LevelImage {
    /// RGBA of tile (tx, ty) (in tiles from the top tile's south-west corner).
    pub fn tile(&self, tx: u32, ty: u32) -> &[u8] {
        let n = (self.tile_size * self.tile_size * 4) as usize;
        let i = (ty * self.tiles_per_side + tx) as usize;
        &self.rgba[i * n..(i + 1) * n]
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct RenderParams {
    cell_px: u32,
    tex_px: u32,
    vc_mult: f32,
    kind: u32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct LevelParams {
    src_side: u32,
    dst_side: u32,
    level: u32,
    tile_size: u32,
    tiles_per_side: u32,
    kind: u32,
    gamma_inv: f32,
    brightness: f32,
    contrast: f32,
    _p: [u32; 3],
}

/// How the cells of a top tile become level images.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LevelFilter {
    /// Every cell is resized into every level on its own with clamped edges (xLODGen). The
    /// kernel never sees the neighbouring cells, which leaves a cell grid that grows with the
    /// level. 8-bit gamma-space values throughout.
    PerCell,
    /// Cells are resized to the finest level and stitched; every level is resized from the
    /// stitched top tile (diffuse in linear light, normals renormalised), so no cell grid.
    #[default]
    Stitched,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct DownParams {
    cell_px: u32,
    ppc: u32,
    level: u32,
    tile_size: u32,
    tiles_per_side: u32,
    round_down: u32,
    gamma_inv: f32,
    brightness: f32,
    contrast: f32,
    _p1: [u32; 3],
}

pub struct SpecTextures {
    buf: wgpu::Buffer,
    pub tex_px: u32,
    pub count: usize,
}

pub const TOP_LEVEL: u32 = 32;

/// Which LOD texture a bake produces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextureKind {
    Diffuse = 0,
    Normal = 1,
}

impl Gpu {
    /// Resizes the stitched `base` (side `side`, RGBA8, north-up) into every level buffer.
    fn levels_from_base(&self, kind: TextureKind, base: &wgpu::Buffer, side: u32, level_bufs: &[(LevelSpec, u32, u32, wgpu::Buffer)]) -> Result<()> {
        let max_tmp = level_bufs.iter().map(|(_, tps, ts, _)| tps * ts).filter(|&d| d != side).max().unwrap_or(0);
        let tmp = (max_tmp > 0).then(|| self.device.create_buffer(&wgpu::BufferDescriptor { label: Some("level tmp"), size: u64::from(side) * u64::from(max_tmp) * 8, usage: wgpu::BufferUsages::STORAGE, mapped_at_creation: false }));
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("levels") });
        for (l, tps, ts, level_buf) in level_bufs {
            let dst = tps * ts;
            let p = LevelParams {
                src_side: side,
                dst_side: dst,
                level: l.level,
                tile_size: *ts,
                tiles_per_side: *tps,
                kind: kind as u32,
                gamma_inv: if kind == TextureKind::Diffuse { 1.0 / l.gamma } else { 1.0 },
                brightness: if kind == TextureKind::Diffuse { l.brightness / 100.0 } else { 0.0 },
                contrast: if kind == TextureKind::Diffuse { l.contrast / 100.0 } else { 0.0 },
                _p: [0; 3],
            };
            let uni = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::bytes_of(&p), usage: wgpu::BufferUsages::UNIFORM });
            let bind = |pipe: &wgpu::ComputePipeline, with_tmp: bool, with_level: bool| {
                let mut e = vec![wgpu::BindGroupEntry { binding: 0, resource: uni.as_entire_binding() }];
                if !with_level || dst == side {
                    e.push(wgpu::BindGroupEntry { binding: 1, resource: base.as_entire_binding() });
                }
                if with_tmp {
                    e.push(wgpu::BindGroupEntry { binding: 2, resource: tmp.as_ref().unwrap().as_entire_binding() });
                }
                if with_level {
                    e.push(wgpu::BindGroupEntry { binding: 3, resource: level_buf.as_entire_binding() });
                }
                self.device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &pipe.get_bind_group_layout(0), entries: &e })
            };
            if dst == side {
                let g = bind(&self.level_copy, false, true);
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("level_copy"), timestamp_writes: None });
                pass.set_pipeline(&self.level_copy);
                pass.set_bind_group(0, &g, &[]);
                pass.dispatch_workgroups(dst.div_ceil(8), dst.div_ceil(8), 1);
                continue;
            }
            let gh = bind(&self.level_h, true, false);
            let gv = bind(&self.level_v, true, true);
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("level_h"), timestamp_writes: None });
                pass.set_pipeline(&self.level_h);
                pass.set_bind_group(0, &gh, &[]);
                pass.dispatch_workgroups(dst.div_ceil(64), side, 1);
            }
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("level_v"), timestamp_writes: None });
            pass.set_pipeline(&self.level_v);
            pass.set_bind_group(0, &gv, &[]);
            pass.dispatch_workgroups(dst.div_ceil(8), dst.div_ceil(8), 1);
        }
        self.queue.submit([encoder.finish()]);
        Ok(())
    }

    /// Uploads `tex_px`-square RGBA8 textures (index = position).
    pub fn upload_spec_textures(&self, tex_px: u32, textures: &[Vec<u8>]) -> SpecTextures {
        let mut data: Vec<u8> = textures.concat();
        if data.is_empty() {
            data = vec![128; (tex_px * tex_px * 4) as usize];
        }
        let buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("spec textures"), contents: &data, usage: wgpu::BufferUsages::STORAGE });
        SpecTextures { buf, tex_px, count: textures.len() }
    }

    /// Renders and downscales `batches` into the levels of one top tile; returns each level's
    /// RGBA (tone curve applied, 8-bit).
    pub fn bake_top_tile(&self, tex: &SpecTextures, kind: TextureKind, vc_mult: f32, levels: &[LevelSpec], batches: &[CellBatch], filter: LevelFilter) -> Result<Vec<LevelImage>> {
        let n = 24 * tex.tex_px;
        let storage = |label: &str, bytes: &[u8]| {
            let contents: &[u8] = if bytes.is_empty() { &[0u8; 16] } else { bytes };
            self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(label), contents, usage: wgpu::BufferUsages::STORAGE })
        };
        let level_bufs: Vec<(LevelSpec, u32, u32, wgpu::Buffer)> = levels
            .iter()
            .map(|l| {
                let tps = TOP_LEVEL / l.level;
                let ts = l.ppc * l.level;
                let size = u64::from(tps * tps) * u64::from(ts * ts) * 4;
                let b = self.device.create_buffer(&wgpu::BufferDescriptor { label: Some("level"), size, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false });
                (*l, tps, ts, b)
            })
            .collect();
        let max_ppc = levels.iter().map(|l| l.ppc).max().unwrap_or(1);
        // Stitched: the cells go once into a base image at the finest level's pixels per cell
        // (pre tone curve, rounded), a single 32-cell "tile".
        let base_spec = LevelSpec { level: TOP_LEVEL, ppc: max_ppc, gamma: 1.0, brightness: 0.0, contrast: 0.0 };
        let base = (filter == LevelFilter::Stitched).then(|| {
            let side = TOP_LEVEL * max_ppc;
            let b = self.device.create_buffer(&wgpu::BufferDescriptor { label: Some("base"), size: u64::from(side) * u64::from(side) * 4, usage: wgpu::BufferUsages::STORAGE, mapped_at_creation: false });
            (base_spec, 1u32, side, b)
        });
        let render_layout = self.spec_render.get_bind_group_layout(0);
        let down_h_layout = self.spec_down_h.get_bind_group_layout(0);
        let down_v_layout = self.spec_down_v.get_bind_group_layout(0);
        let rp = RenderParams { cell_px: n, tex_px: tex.tex_px, vc_mult, kind: kind as u32 };
        let rp_buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::bytes_of(&rp), usage: wgpu::BufferUsages::UNIFORM });
        let targets: Vec<&(LevelSpec, u32, u32, wgpu::Buffer)> = match &base {
            Some(b) => vec![b],
            None => level_bufs.iter().collect(),
        };
        let down_params: Vec<wgpu::Buffer> = targets
            .iter()
            .map(|(l, tps, ts, _)| {
                let stitched = base.is_some();
                let p = DownParams {
                    cell_px: n,
                    ppc: l.ppc,
                    level: l.level,
                    tile_size: *ts,
                    tiles_per_side: *tps,
                    // The base rounds for both kinds (truncating normals tilts them).
                    round_down: (kind == TextureKind::Diffuse || stitched) as u32,
                    gamma_inv: 1.0 / l.gamma,
                    brightness: l.brightness / 100.0,
                    contrast: l.contrast / 100.0,
                    _p1: [0; 3],
                };
                self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::bytes_of(&p), usage: wgpu::BufferUsages::UNIFORM })
            })
            .collect();

        for batch in batches {
            let slots = batch.cells.len() as u32;
            if slots == 0 || batch.placements.is_empty() {
                continue;
            }
            let cells = storage("cells", bytemuck::cast_slice(&batch.cells));
            let layers = storage("layers", bytemuck::cast_slice(&batch.layers));
            let splats = storage("splats", bytemuck::cast_slice(&batch.splats));
            let vcolors = storage("vcolors", bytemuck::cast_slice(&batch.vcolors));
            let terrain = storage("terrain", bytemuck::cast_slice(&batch.terrain));
            let placements = storage("placements", bytemuck::cast_slice(&batch.placements));
            let cell_px = self.device.create_buffer(&wgpu::BufferDescriptor { label: Some("cell px"), size: u64::from(slots) * u64::from(n * n) * 4, usage: wgpu::BufferUsages::STORAGE, mapped_at_creation: false });
            let tmp = self.device.create_buffer(&wgpu::BufferDescriptor { label: Some("tmp"), size: u64::from(slots) * u64::from(n * max_ppc) * 16, usage: wgpu::BufferUsages::STORAGE, mapped_at_creation: false });
            let render_bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &render_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: rp_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: tex.buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: cells.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 3, resource: layers.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 4, resource: splats.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 5, resource: vcolors.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 6, resource: cell_px.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 7, resource: terrain.as_entire_binding() },
                ],
            });
            let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("spec") });
            {
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("render"), timestamp_writes: None });
                pass.set_pipeline(&self.spec_render);
                pass.set_bind_group(0, &render_bind, &[]);
                pass.dispatch_workgroups(n.div_ceil(8), n.div_ceil(8), slots);
            }
            for ((l, _, _, level_buf), dp) in targets.iter().zip(&down_params) {
                let entries = |layout: &wgpu::BindGroupLayout, with_placement: bool| {
                    let mut e = vec![
                        wgpu::BindGroupEntry { binding: 0, resource: dp.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 1, resource: cell_px.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 2, resource: tmp.as_entire_binding() },
                    ];
                    if with_placement {
                        e.retain(|x| x.binding != 1);
                        e.push(wgpu::BindGroupEntry { binding: 3, resource: placements.as_entire_binding() });
                        e.push(wgpu::BindGroupEntry { binding: 4, resource: level_buf.as_entire_binding() });
                    }
                    self.device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout, entries: &e })
                };
                let h_bind = entries(&down_h_layout, false);
                let v_bind = entries(&down_v_layout, true);
                // Separate passes: tmp is written by the first and read by the second.
                {
                    let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("downscale_h"), timestamp_writes: None });
                    pass.set_pipeline(&self.spec_down_h);
                    pass.set_bind_group(0, &h_bind, &[]);
                    pass.dispatch_workgroups(l.ppc.div_ceil(64), n, slots);
                }
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("downscale_v"), timestamp_writes: None });
                pass.set_pipeline(&self.spec_down_v);
                pass.set_bind_group(0, &v_bind, &[]);
                pass.dispatch_workgroups(l.ppc.div_ceil(8), l.ppc.div_ceil(8), batch.placements.len() as u32);
            }
            self.queue.submit([encoder.finish()]);
        }

        if let Some((_, _, side, base_buf)) = &base {
            self.levels_from_base(kind, base_buf, *side, &level_bufs)?;
        }

        // Read back every level image.
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("readback") });
        let reads: Vec<wgpu::Buffer> = level_bufs
            .iter()
            .map(|(_, _, _, b)| {
                let r = self.device.create_buffer(&wgpu::BufferDescriptor { label: None, size: b.size(), usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
                encoder.copy_buffer_to_buffer(b, 0, &r, 0, b.size());
                r
            })
            .collect();
        self.queue.submit([encoder.finish()]);
        let (txc, rx) = mpsc::channel();
        for r in &reads {
            let t = txc.clone();
            r.slice(..).map_async(wgpu::MapMode::Read, move |res| {
                let _ = t.send(res);
            });
        }
        drop(txc);
        self.device.poll(wgpu::PollType::Wait)?;
        for res in rx {
            res?;
        }
        Ok(level_bufs
            .iter()
            .zip(&reads)
            .map(|((l, tps, ts, _), r)| {
                let rgba = r.slice(..).get_mapped_range().to_vec();
                r.unmap();
                LevelImage { level: l.level, tile_size: *ts, tiles_per_side: *tps, rgba }
            })
            .collect())
    }
}
