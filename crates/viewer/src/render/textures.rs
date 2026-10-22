// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! DDS files as GPU textures: block-compressed data is uploaded as is (sRGB views for colour),
//! the rest is decoded to RGBA8. A texture cache shares pages between shapes and keeps the VRAM
//! total; top mip levels can be dropped to fit a budget.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use lodforge_dds::Format;

use crate::files::{Files, normalize};

pub struct GpuTexture {
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
    pub mips: u32,
    pub format: wgpu::TextureFormat,
    pub bytes: u64,
    /// Mip levels dropped from the file to fit the budget.
    pub dropped: u32,
}

fn bc_format(f: Format, srgb: bool) -> Option<wgpu::TextureFormat> {
    use wgpu::TextureFormat as T;
    Some(match (f, srgb) {
        (Format::Bc1, false) => T::Bc1RgbaUnorm,
        (Format::Bc1, true) => T::Bc1RgbaUnormSrgb,
        (Format::Bc2, false) => T::Bc2RgbaUnorm,
        (Format::Bc2, true) => T::Bc2RgbaUnormSrgb,
        (Format::Bc3, false) => T::Bc3RgbaUnorm,
        (Format::Bc3, true) => T::Bc3RgbaUnormSrgb,
        (Format::Bc4, _) => T::Bc4RUnorm,
        (Format::Bc5, _) => T::Bc5RgUnorm,
        (Format::Bc7, false) => T::Bc7RgbaUnorm,
        (Format::Bc7, true) => T::Bc7RgbaUnormSrgb,
        _ => return None,
    })
}

/// Size in bytes of a DDS's mip chain on the GPU after dropping `drop` levels (0 when the file
/// is not a readable DDS).
pub fn dds_gpu_bytes(bytes: &[u8], drop: u32) -> u64 {
    let Ok(info) = lodforge_dds::parse(bytes) else {
        return 0;
    };
    let drop = drop.min(info.mip_count.saturating_sub(1));
    let bc = info.format.is_block_compressed();
    info.mips()
        .iter()
        .skip(drop as usize)
        .map(|m| {
            if bc {
                info.format.level_size(m.2, m.3) as u64
            } else {
                u64::from(m.2) * u64::from(m.3) * 4
            }
        })
        .sum()
}

/// Uploads a DDS. `srgb` marks colour data; `drop` top mip levels are skipped when the file has
/// them; `bc` says the device samples block-compressed formats (else they are decoded).
pub fn upload_dds(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    bytes: &[u8],
    srgb: bool,
    drop: u32,
    bc: bool,
    label: &str,
) -> Result<GpuTexture, String> {
    let info = lodforge_dds::parse(bytes).map_err(|e| e.to_string())?;
    let (w, h) = (info.width, info.height);
    // Consistent mip chain (halving sizes, inside the file).
    let levels: Vec<(usize, usize, u32, u32)> = info
        .mips()
        .into_iter()
        .enumerate()
        .take_while(|(i, m)| {
            m.2 == (w >> i).max(1) && m.3 == (h >> i).max(1) && m.0 + m.1 <= bytes.len()
        })
        .map(|(_, m)| m)
        .collect();
    if levels.is_empty() {
        return Err("no complete mip level".into());
    }
    let drop = drop.min(levels.len() as u32 - 1) as usize;
    let use_bc = bc && info.format.is_block_compressed() && w % 4 == 0 && h % 4 == 0;
    let (format, data): (wgpu::TextureFormat, Vec<Vec<u8>>) = if use_bc {
        (
            bc_format(info.format, srgb).ok_or("unknown BC format")?,
            levels[drop..]
                .iter()
                .map(|&(o, s, _, _)| bytes[o..o + s].to_vec())
                .collect(),
        )
    } else {
        let mut v = Vec::new();
        for lvl in drop..levels.len() {
            v.push(
                lodforge_dds::decode(bytes, &info, lvl as u32)
                    .map_err(|e| e.to_string())?
                    .rgba,
            );
        }
        (
            if srgb {
                wgpu::TextureFormat::Rgba8UnormSrgb
            } else {
                wgpu::TextureFormat::Rgba8Unorm
            },
            v,
        )
    };
    let (tw, th) = (levels[drop].2, levels[drop].3);
    let mips = data.len() as u32;
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: tw,
            height: th,
            depth_or_array_layers: 1,
        },
        mip_level_count: mips,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let mut total = 0u64;
    for (lvl, d) in data.iter().enumerate() {
        let (mw, mh) = ((tw >> lvl).max(1), (th >> lvl).max(1));
        let (bpr, rows, ew, eh) = if use_bc {
            let unit = format.block_copy_size(None).unwrap_or(16);
            (
                mw.div_ceil(4) * unit,
                mh.div_ceil(4),
                mw.div_ceil(4) * 4,
                mh.div_ceil(4) * 4,
            )
        } else {
            (mw * 4, mh, mw, mh)
        };
        total += d.len() as u64;
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: lvl as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            d,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bpr),
                rows_per_image: Some(rows),
            },
            wgpu::Extent3d {
                width: ew,
                height: eh,
                depth_or_array_layers: 1,
            },
        );
    }
    let view = texture.create_view(&Default::default());
    Ok(GpuTexture {
        texture,
        view,
        width: tw,
        height: th,
        mips,
        format,
        bytes: total,
        dropped: drop as u32,
    })
}

/// A 1x1 texture of one colour.
pub fn solid(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    rgba: [u8; 4],
    srgb: bool,
    label: &str,
) -> GpuTexture {
    let format = if srgb {
        wgpu::TextureFormat::Rgba8UnormSrgb
    } else {
        wgpu::TextureFormat::Rgba8Unorm
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(4),
            rows_per_image: Some(1),
        },
        wgpu::Extent3d {
            width: 1,
            height: 1,
            depth_or_array_layers: 1,
        },
    );
    let view = texture.create_view(&Default::default());
    GpuTexture {
        texture,
        view,
        width: 1,
        height: 1,
        mips: 1,
        format,
        bytes: 4,
        dropped: 0,
    }
}

/// An 8x8 magenta/black checkerboard for missing textures.
pub fn missing_checker(device: &wgpu::Device, queue: &wgpu::Queue) -> GpuTexture {
    let mut px = Vec::with_capacity(64 * 4);
    for y in 0..8 {
        for x in 0..8 {
            let on = (x + y) % 2 == 0;
            px.extend(if on {
                [255, 0, 255, 255]
            } else {
                [20, 20, 20, 255]
            });
        }
    }
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("missing"),
        size: wgpu::Extent3d {
            width: 8,
            height: 8,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &px,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(32),
            rows_per_image: Some(8),
        },
        wgpu::Extent3d {
            width: 8,
            height: 8,
            depth_or_array_layers: 1,
        },
    );
    let view = texture.create_view(&Default::default());
    GpuTexture {
        texture,
        view,
        width: 8,
        height: 8,
        mips: 1,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        bytes: 256,
        dropped: 0,
    }
}

type Slot = Arc<OnceLock<Option<Arc<GpuTexture>>>>;

/// Shared texture loader: each path is read and uploaded once; failures and absent files are
/// remembered as missing.
pub struct TexCache {
    device: wgpu::Device,
    queue: wgpu::Queue,
    bc: bool,
    slots: Mutex<HashMap<String, Slot>>,
    missing: Mutex<HashSet<String>>,
    failed: Mutex<HashMap<String, String>>,
    bytes: AtomicU64,
    /// Mip levels to drop, by path prefix match (`textures\terrain\<world>\<world>.<level>.`).
    drops: Mutex<Vec<(String, u32)>>,
}

impl TexCache {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue, bc: bool) -> TexCache {
        TexCache {
            device,
            queue,
            bc,
            slots: Mutex::new(HashMap::new()),
            missing: Mutex::new(HashSet::new()),
            failed: Mutex::new(HashMap::new()),
            bytes: AtomicU64::new(0),
            drops: Mutex::new(Vec::new()),
        }
    }

    /// Drops `levels` top mips of every texture whose normalized path starts with `prefix`.
    pub fn set_drop(&self, prefix: &str, levels: u32) {
        self.drops.lock().unwrap().push((normalize(prefix), levels));
    }

    fn drop_for(&self, path: &str) -> u32 {
        self.drops
            .lock()
            .unwrap()
            .iter()
            .filter(|(p, _)| path.starts_with(p.as_str()))
            .map(|(_, d)| *d)
            .max()
            .unwrap_or(0)
    }

    pub fn bytes(&self) -> u64 {
        self.bytes.load(Ordering::Relaxed)
    }

    pub fn missing(&self) -> Vec<String> {
        let mut v: Vec<String> = self.missing.lock().unwrap().iter().cloned().collect();
        v.sort();
        v
    }

    pub fn failed(&self) -> Vec<(String, String)> {
        let mut v: Vec<(String, String)> = self
            .failed
            .lock()
            .unwrap()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        v.sort();
        v
    }

    /// Loads (once) and returns the texture at `path`; `None` when the file is absent or broken.
    pub fn get(&self, files: &dyn Files, path: &str, srgb: bool) -> Option<Arc<GpuTexture>> {
        let key = normalize(path);
        let slot = {
            let mut slots = self.slots.lock().unwrap();
            slots
                .entry(key.clone())
                .or_insert_with(|| Arc::new(OnceLock::new()))
                .clone()
        };
        slot.get_or_init(|| {
            let Some(bytes) = files.read(&key) else {
                self.missing.lock().unwrap().insert(key.clone());
                return None;
            };
            match upload_dds(
                &self.device,
                &self.queue,
                &bytes,
                srgb,
                self.drop_for(&key),
                self.bc,
                &key,
            ) {
                Ok(t) => {
                    self.bytes.fetch_add(t.bytes, Ordering::Relaxed);
                    Some(Arc::new(t))
                }
                Err(e) => {
                    self.failed.lock().unwrap().insert(key.clone(), e);
                    None
                }
            }
        })
        .clone()
    }

    pub fn count(&self) -> usize {
        self.slots.lock().unwrap().len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_estimate() {
        // A 64x64 BC7 with 7 mips: 4096 + 1024 + 256 + 64 + 16 + 16 + 16 bytes.
        let mut b = vec![0u8; 148];
        b[0..4].copy_from_slice(b"DDS ");
        b[4..8].copy_from_slice(&124u32.to_le_bytes());
        b[8..12].copy_from_slice(&(0x1u32 | 0x2 | 0x4 | 0x1000 | 0x20000).to_le_bytes());
        b[12..16].copy_from_slice(&64u32.to_le_bytes());
        b[16..20].copy_from_slice(&64u32.to_le_bytes());
        b[28..32].copy_from_slice(&7u32.to_le_bytes());
        b[76..80].copy_from_slice(&32u32.to_le_bytes());
        b[80..84].copy_from_slice(&4u32.to_le_bytes());
        b[84..88].copy_from_slice(b"DX10");
        b[128..132].copy_from_slice(&98u32.to_le_bytes());
        b[132..136].copy_from_slice(&3u32.to_le_bytes());
        b[140..144].copy_from_slice(&1u32.to_le_bytes());
        b.resize(148 + 4096 + 1024 + 256 + 64 + 16 + 16 + 16, 0);
        assert_eq!(dds_gpu_bytes(&b, 0), 4096 + 1024 + 256 + 64 + 16 + 16 + 16);
        assert_eq!(dds_gpu_bytes(&b, 1), 1024 + 256 + 64 + 16 + 16 + 16);
        assert_eq!(dds_gpu_bytes(&b, 99), 16);
    }
}
