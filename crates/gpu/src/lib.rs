// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! GPU compute via wgpu (D3D12/Vulkan on Windows): BC7 encoding and terrain LOD baking.

pub mod bc7_cpu;
pub mod terrain_spec;

pub use bc7_cpu::Bc7Quality;

use std::sync::mpsc;

use wgpu::util::DeviceExt;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("no suitable GPU adapter: {0}")]
    NoAdapter(String),
    #[error("GPU device request failed: {0}")]
    Device(#[from] wgpu::RequestDeviceError),
    #[error("GPU buffer mapping failed: {0}")]
    Map(#[from] wgpu::BufferAsyncError),
    #[error("GPU poll failed: {0}")]
    Poll(#[from] wgpu::PollError),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct Bc7Params {
    width: u32,
    height: u32,
    blocks_x: u32,
    blocks_y: u32,
}

pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub adapter: wgpu::Adapter,
    pub adapter_info: wgpu::AdapterInfo,
    /// Largest MSAA sample count usable for an Rgba8Unorm + Depth32Float render target (4 without
    /// adapter-specific format features, up to 16).
    pub max_msaa: u32,
    pub(crate) bc7: wgpu::ComputePipeline,
    pub(crate) spec_render: wgpu::ComputePipeline,
    pub(crate) spec_down_h: wgpu::ComputePipeline,
    pub(crate) spec_down_v: wgpu::ComputePipeline,
    pub(crate) level_h: wgpu::ComputePipeline,
    pub(crate) level_v: wgpu::ComputePipeline,
    pub(crate) level_copy: wgpu::ComputePipeline,
    /// [Bc7Quality] as u8: which encoder [Gpu::encode_bc7] uses.
    bc7_quality: std::sync::atomic::AtomicU8,
}

/// An RGBA8 image to encode (`rgba.len() == width * height * 4`).
pub struct RgbaImage<'a> {
    pub width: u32,
    pub height: u32,
    pub rgba: &'a [u8],
}

impl Gpu {
    pub fn new() -> Result<Gpu> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
        Self::with_instance(&instance, None, wgpu::Features::empty())
    }

    /// A device on `instance`, able to present to `compatible_surface` when one is given, with
    /// `extra` features requested on top of the compute ones (those the adapter lacks are left
    /// out; check `device.features()`).
    pub fn with_instance(instance: &wgpu::Instance, compatible_surface: Option<&wgpu::Surface<'_>>, extra: wgpu::Features) -> Result<Gpu> {
        pollster::block_on(Self::new_async(instance, compatible_surface, extra))
    }

    async fn new_async(instance: &wgpu::Instance, compatible_surface: Option<&wgpu::Surface<'_>>, extra: wgpu::Features) -> Result<Gpu> {
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions { power_preference: wgpu::PowerPreference::HighPerformance, compatible_surface, ..Default::default() })
            .await
            .map_err(|e| Error::NoAdapter(e.to_string()))?;
        let adapter_info = adapter.get_info();
        let features = adapter.features() & (wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES | wgpu::Features::TEXTURE_COMPRESSION_BC | extra);
        let max_msaa = if !features.contains(wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES) {
            4
        } else {
            let (c, d) = (adapter.get_texture_format_features(wgpu::TextureFormat::Rgba8Unorm).flags, adapter.get_texture_format_features(wgpu::TextureFormat::Depth32Float).flags);
            [16, 8, 4].into_iter().find(|&n| c.sample_count_supported(n) && d.sample_count_supported(n)).unwrap_or(4)
        };
        let limits = wgpu::Limits { max_storage_buffer_binding_size: adapter.limits().max_storage_buffer_binding_size, max_buffer_size: adapter.limits().max_buffer_size, ..wgpu::Limits::default() };
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor { label: Some("lodforge"), required_features: features, required_limits: limits, ..Default::default() })
            .await?;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("bc7_mode6"), source: wgpu::ShaderSource::Wgsl(include_str!("bc7_mode6.wgsl").into()) });
        let bc7 = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("bc7_mode6"),
            layout: None,
            module: &module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let spec = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("terrain_spec"), source: wgpu::ShaderSource::Wgsl(include_str!("terrain_spec.wgsl").into()) });
        let entry = |name: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(name),
                layout: None,
                module: &spec,
                entry_point: Some(name),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let (spec_render, spec_down_h, spec_down_v) = (entry("render_cell"), entry("downscale_h"), entry("downscale_v"));
        let levels = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("terrain_levels"), source: wgpu::ShaderSource::Wgsl(include_str!("terrain_levels.wgsl").into()) });
        let level_entry = |name: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(name),
                layout: None,
                module: &levels,
                entry_point: Some(name),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let (level_h, level_v, level_copy) = (level_entry("level_h"), level_entry("level_v"), level_entry("level_copy"));
        Ok(Gpu { device, queue, adapter, adapter_info, max_msaa, bc7, spec_render, spec_down_h, spec_down_v, level_h, level_v, level_copy, bc7_quality: std::sync::atomic::AtomicU8::new(bc7_cpu::default_quality().as_u8()) })
    }

    /// Chooses the BC7 encoder for later [Gpu::encode_bc7] calls (default [Bc7Quality::High]).
    pub fn set_bc7_quality(&self, q: Bc7Quality) {
        self.bc7_quality.store(q.as_u8(), std::sync::atomic::Ordering::Relaxed);
    }

    pub fn bc7_quality(&self) -> Bc7Quality {
        Bc7Quality::from_u8(self.bc7_quality.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// BC7 encodes RGBA8 images; returns each image's block data (16 bytes per 4x4 block,
    /// row-major), ready to follow a DDS header. [Bc7Quality::Fast] uses the GPU mode-6 encoder,
    /// the others the multi-mode CPU encoder ([bc7_cpu]).
    pub fn encode_bc7(&self, images: &[RgbaImage<'_>]) -> Result<Vec<Vec<u8>>> {
        match self.bc7_quality() {
            Bc7Quality::Fast => self.encode_bc7_mode6(images),
            q => {
                use rayon::prelude::*;
                Ok(images.par_iter().map(|img| bc7_cpu::encode(img, q)).collect())
            }
        }
    }

    /// BC7 mode 6 on the GPU (one round trip for all images).
    pub fn encode_bc7_mode6(&self, images: &[RgbaImage<'_>]) -> Result<Vec<Vec<u8>>> {
        struct Job {
            out: wgpu::Buffer,
            readback: wgpu::Buffer,
            bind: wgpu::BindGroup,
            blocks: u32,
        }
        let layout = self.bc7.get_bind_group_layout(0);
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("bc7") });
        let mut jobs = Vec::with_capacity(images.len());
        for img in images {
            let (bx, by) = (img.width.div_ceil(4), img.height.div_ceil(4));
            let params = Bc7Params { width: img.width, height: img.height, blocks_x: bx, blocks_y: by };
            let uni = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: bytemuck::bytes_of(&params), usage: wgpu::BufferUsages::UNIFORM });
            let input = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: None, contents: img.rgba, usage: wgpu::BufferUsages::STORAGE });
            let size = u64::from(bx * by) * 16;
            let out = self.device.create_buffer(&wgpu::BufferDescriptor { label: None, size, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false });
            let readback = self.device.create_buffer(&wgpu::BufferDescriptor { label: None, size, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
            let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: uni.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: input.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: out.as_entire_binding() },
                ],
            });
            jobs.push(Job { out, readback, bind, blocks: bx * by });
        }
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("bc7"), timestamp_writes: None });
            pass.set_pipeline(&self.bc7);
            for j in &jobs {
                pass.set_bind_group(0, &j.bind, &[]);
                // 64 blocks per group; more than 65535 groups (8192x8192 and up) wrap into rows.
                let groups = j.blocks.div_ceil(64);
                let x = groups.min(32768);
                pass.dispatch_workgroups(x, groups.div_ceil(x), 1);
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

    fn dds_bc7(width: u32, height: u32, blocks: &[u8]) -> Vec<u8> {
        let mut b = vec![0u8; 148];
        b[0..4].copy_from_slice(b"DDS ");
        b[4..8].copy_from_slice(&124u32.to_le_bytes());
        b[12..16].copy_from_slice(&height.to_le_bytes());
        b[16..20].copy_from_slice(&width.to_le_bytes());
        b[28..32].copy_from_slice(&1u32.to_le_bytes());
        b[76..80].copy_from_slice(&32u32.to_le_bytes());
        b[80..84].copy_from_slice(&4u32.to_le_bytes());
        b[84..88].copy_from_slice(b"DX10");
        b[128..132].copy_from_slice(&98u32.to_le_bytes()); // BC7_UNORM
        b[132..136].copy_from_slice(&3u32.to_le_bytes());
        b[140..144].copy_from_slice(&1u32.to_le_bytes());
        b.extend_from_slice(blocks);
        b
    }

    fn psnr(a: &[u8], b: &[u8]) -> f64 {
        let mse: f64 = a.iter().zip(b).map(|(x, y)| (f64::from(*x) - f64::from(*y)).powi(2)).sum::<f64>() / a.len() as f64;
        if mse == 0.0 { 99.0 } else { 10.0 * (255.0 * 255.0 / mse).log10() }
    }

    /// Round trip through the GPU encoder and the CPU BC7 decoder.
    #[test]
    fn bc7_roundtrip_quality() {
        let gpu = match Gpu::new() {
            Ok(g) => g,
            Err(e) => {
                eprintln!("skipping: {e}");
                return;
            }
        };
        let (w, h) = (64u32, 36u32);
        let mut rgba = Vec::new();
        for y in 0..h {
            for x in 0..w {
                // Smooth gradients plus a hard edge.
                let edge = if x > 40 { 60 } else { 0 };
                rgba.extend([(x * 4) as u8, (y * 7) as u8, (128 + edge) as u8, 255]);
            }
        }
        let out = gpu.encode_bc7(&[RgbaImage { width: w, height: h, rgba: &rgba }]).unwrap();
        let dds = dds_bc7(w, h, &out[0]);
        let info = lodforge_dds::parse(&dds).unwrap();
        let dec = lodforge_dds::decode(&dds, &info, 0).unwrap();
        let p = psnr(&rgba, &dec.rgba);
        eprintln!("BC7 mode 6 round trip on {}: PSNR {p:.2} dB", gpu.adapter_info.name);
        assert!(p > 40.0, "PSNR {p:.2}");
    }

    /// More blocks than one dispatch dimension holds (8192x4100: 32800 groups of 64 blocks, past
    /// the 32768-group row): the last rows are encoded too.
    #[test]
    fn bc7_large_image_rows() {
        let gpu = match Gpu::new() {
            Ok(g) => g,
            Err(e) => {
                eprintln!("skipping: {e}");
                return;
            }
        };
        let (w, h) = (8192u32, 4100u32);
        let mut rgba = vec![0u8; (w * h * 4) as usize];
        for px in rgba.chunks_exact_mut(4) {
            px.copy_from_slice(&[200, 100, 50, 255]);
        }
        let out = gpu.encode_bc7(&[RgbaImage { width: w, height: h, rgba: &rgba }]).unwrap();
        let dds = dds_bc7(w, h, &out[0]);
        let info = lodforge_dds::parse(&dds).unwrap();
        let dec = lodforge_dds::decode(&dds, &info, 0).unwrap();
        let last = &dec.rgba[dec.rgba.len() - 4..];
        assert!(last.iter().zip([200u8, 100, 50, 255]).all(|(a, b)| (i32::from(*a) - i32::from(b)).abs() <= 2), "{last:?}");
    }
}
