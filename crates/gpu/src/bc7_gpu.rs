//! Multi-mode BC7 on the GPU: `bc7_ispc.wgsl`, a port of the ISPC encoder [crate::bc7_cpu] uses,
//! run with the same profiles. One invocation per 4x4 block; all images of a call go in one
//! submission.

use std::sync::mpsc;

use wgpu::util::DeviceExt;

use crate::{Bc7Quality, Gpu, Result, RgbaImage, bc7_cpu, bc7_tables};

/// Whether [Gpu::encode_bc7] runs the `high`/`best` encoders on the GPU (process-wide default
/// for new contexts).
static GPU_ISPC: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_default_gpu(on: bool) {
    GPU_ISPC.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn default_gpu() -> bool {
    GPU_ISPC.load(std::sync::atomic::Ordering::Relaxed)
}

/// Uniform block of `bc7_ispc.wgsl` (`Params`).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    blocks_x: u32,
    blocks_y: u32,
    stride: u32,
    channels: i32,
    mode_sel: [i32; 4],
    refine: [i32; 8],
    skip_mode2: i32,
    fs1: i32,
    fs3: i32,
    fs7: i32,
    m45_channel0: i32,
    refine_channel: i32,
    groups_x: u32,
    _pad: u32,
    consts: [f32; 4],
}

pub(crate) struct Bc7Ispc {
    pipeline: wgpu::ComputePipeline,
    tables: wgpu::Buffer,
}

impl Bc7Ispc {
    pub(crate) fn new(device: &wgpu::Device) -> Bc7Ispc {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("bc7_ispc"), source: wgpu::ShaderSource::Wgsl(include_str!("bc7_ispc.wgsl").into()) });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("bc7_ispc"),
            layout: None,
            module: &module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let mut t: Vec<u32> = Vec::with_capacity(412);
        t.extend_from_slice(&bc7_tables::PATTERN);
        t.extend_from_slice(&bc7_tables::PATTERN_MASK);
        t.extend_from_slice(&bc7_tables::SKIP);
        t.extend_from_slice(&bc7_tables::UNQUANT);
        let tables = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("bc7_tables"), contents: bytemuck::cast_slice(&t), usage: wgpu::BufferUsages::STORAGE });
        Bc7Ispc { pipeline, tables }
    }
}

/// Blocks per dispatch (4 Mpx), so no single dispatch runs long enough to trip the Windows GPU
/// timeout on big pages.
const CHUNK_BLOCKS: u32 = 1 << 18;

impl Gpu {
    /// The ISPC BC7 encoder ([bc7_cpu::encode]) on the GPU: same profiles, same block data.
    pub fn encode_bc7_ispc(&self, images: &[RgbaImage<'_>], quality: Bc7Quality) -> Result<Vec<Vec<u8>>> {
        struct Job {
            out: wgpu::Buffer,
            readback: wgpu::Buffer,
            binds: Vec<(wgpu::BindGroup, u32)>,
        }
        let layout = self.bc7_ispc.pipeline.get_bind_group_layout(0);
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("bc7_ispc") });
        let mut jobs = Vec::with_capacity(images.len());
        for img in images {
            let (bx, by, padded) = bc7_cpu::padded(img);
            let opaque = padded.chunks_exact(4).all(|p| p[3] == 255);
            let s = bc7_cpu::settings(quality, opaque);
            let input = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: &padded, usage: wgpu::BufferUsages::STORAGE });
            let size = (bx * by * 16) as u64;
            let out = self.device.create_buffer(&wgpu::BufferDescriptor { label: None, size, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false });
            let readback = self.device.create_buffer(&wgpu::BufferDescriptor { label: None, size, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
            // Chunks of whole block rows, each with its own source/destination window.
            // Multiples of 16 rows keep both binding offsets 256-byte aligned.
            let rows_per_chunk = ((CHUNK_BLOCKS as usize / bx) & !15).max(16);
            let mut binds = Vec::new();
            let mut row = 0;
            while row < by {
                let rows = rows_per_chunk.min(by - row);
                let blocks = (bx * rows) as u32;
                let groups = blocks.div_ceil(64);
                let gx = groups.min(32768);
                let params = Params {
                    blocks_x: bx as u32,
                    blocks_y: rows as u32,
                    stride: (bx * 4) as u32,
                    channels: s.channels,
                    mode_sel: s.mode_selection.map(i32::from),
                    refine: s.refine_iterations.map(|v| v as i32),
                    skip_mode2: i32::from(s.skip_mode2),
                    fs1: s.fast_skip_threshold_mode1 as i32,
                    fs3: s.fast_skip_threshold_mode3 as i32,
                    fs7: s.fast_skip_threshold_mode7 as i32,
                    m45_channel0: s.mode45_channel0 as i32,
                    refine_channel: s.refine_iterations_channel as i32,
                    groups_x: gx,
                    _pad: 0,
                    consts: [255.0, 16.0, 0.0, 0.0],
                };
                let uni = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::bytes_of(&params), usage: wgpu::BufferUsages::UNIFORM });
                let src_off = (row * 4 * bx * 4 * 4) as u64;
                let src_len = (rows * 4 * bx * 4 * 4) as u64;
                let dst_off = (row * bx * 16) as u64;
                let dst_len = (rows * bx * 16) as u64;
                let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                    label: None,
                    layout: &layout,
                    entries: &[
                        wgpu::BindGroupEntry { binding: 0, resource: uni.as_entire_binding() },
                        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &input, offset: src_off, size: std::num::NonZeroU64::new(src_len) }) },
                        wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding { buffer: &out, offset: dst_off, size: std::num::NonZeroU64::new(dst_len) }) },
                        wgpu::BindGroupEntry { binding: 3, resource: self.bc7_ispc.tables.as_entire_binding() },
                    ],
                });
                binds.push((bind, groups));
                row += rows;
            }
            jobs.push(Job { out, readback, binds });
        }
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("bc7_ispc"), timestamp_writes: None });
            pass.set_pipeline(&self.bc7_ispc.pipeline);
            for j in &jobs {
                for (bind, groups) in &j.binds {
                    pass.set_bind_group(0, bind, &[]);
                    let x = (*groups).min(32768);
                    pass.dispatch_workgroups(x, groups.div_ceil(x), 1);
                }
            }
        }
        for j in &jobs {
            encoder.copy_buffer_to_buffer(&j.out, 0, &j.readback, 0, j.out.size());
        }
        self.queue.submit([encoder.finish()]);

        let (tx, rx) = mpsc::channel();
        for (i, j) in jobs.iter().enumerate() {
            let tx = tx.clone();
            j.readback.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send((i, r));
            });
        }
        drop(tx);
        self.device.poll(wgpu::PollType::Wait)?;
        for (_, r) in rx {
            r?;
        }
        Ok(jobs
            .iter()
            .map(|j| {
                let data = j.readback.slice(..).get_mapped_range().to_vec();
                j.readback.unmap();
                data
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic test image: gradients, noise, and (with `alpha`) cut-out and soft alpha.
    fn image(w: u32, h: u32, alpha: bool) -> Vec<u8> {
        let mut s = 0x9E3779B97F4A7C15u64;
        let mut out = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                let n = (s & 31) as u32;
                let r = (x * 255 / w.max(1) + n) as u8;
                let g = (y * 255 / h.max(1)).wrapping_add(n) as u8;
                let b = ((x ^ y) * 7) as u8;
                let a = if !alpha {
                    255
                } else if (x / 8 + y / 8) % 3 == 0 {
                    4
                } else {
                    (128 + (x * 3 + y) % 128) as u8
                };
                out.extend_from_slice(&[r, g, b, a]);
            }
        }
        out
    }

    /// The GPU port writes the CPU encoder's blocks (allowing a handful of ties that break the
    /// other way on some drivers).
    #[test]
    fn gpu_ispc_matches_cpu() {
        let gpu = match Gpu::new() {
            Ok(g) => g,
            Err(e) => {
                eprintln!("skipping: {e}");
                return;
            }
        };
        for (w, h, alpha) in [(256u32, 256u32, false), (256, 256, true), (61, 37, true), (4, 4, false)] {
            for quality in [Bc7Quality::High, Bc7Quality::Best] {
                let rgba = image(w, h, alpha);
                let img = RgbaImage { width: w, height: h, rgba: &rgba };
                let cpu = bc7_cpu::encode(&img, quality);
                let g = gpu.encode_bc7_ispc(&[img], quality).unwrap().remove(0);
                assert_eq!(cpu.len(), g.len());
                let same = cpu.chunks_exact(16).zip(g.chunks_exact(16)).filter(|(a, b)| a == b).count();
                let blocks = cpu.len() / 16;
                assert!(same * 1000 >= blocks * 999, "{w}x{h} alpha {alpha} {quality:?}: {same} of {blocks} blocks identical");
            }
        }
    }
}
