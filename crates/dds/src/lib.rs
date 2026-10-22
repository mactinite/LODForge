// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! DDS container parsing and decoding of the formats Skyrim LOD uses.
//!
//! Layout: `"DDS "` magic, 124-byte `DDS_HEADER` (with a 32-byte pixel format at offset 72),
//! then an optional 20-byte `DDS_HEADER_DXT10` when the FourCC is `DX10`, then mip data
//! (largest first; for cubemaps/arrays, each face's full mip chain in turn).

use texture2ddecoder as t2d;

pub mod bc1;
pub mod mips;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("not a DDS file")]
    NotDds,
    #[error("truncated DDS: {0}")]
    Truncated(&'static str),
    #[error("unsupported DDS format: {0}")]
    Unsupported(String),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Bc1,
    Bc2,
    Bc3,
    Bc4,
    Bc5,
    Bc7,
    /// 8-bit B, G, R, A in memory (D3DFMT_A8R8G8B8 / DXGI B8G8R8A8).
    Bgra8,
    /// 8-bit B, G, R, unused in memory (D3DFMT_X8R8G8B8 / DXGI B8G8R8X8).
    Bgrx8,
    /// 8-bit R, G, B, A in memory (DXGI R8G8B8A8).
    Rgba8,
    /// 24-bit B, G, R (D3DFMT_R8G8B8).
    Bgr8,
    /// 8-bit luminance (D3DFMT_L8).
    L8,
}

impl Format {
    pub fn is_block_compressed(self) -> bool {
        matches!(self, Format::Bc1 | Format::Bc2 | Format::Bc3 | Format::Bc4 | Format::Bc5 | Format::Bc7)
    }
    /// Bytes per 4x4 block, or per pixel for uncompressed formats.
    pub fn unit_bytes(self) -> usize {
        match self {
            Format::Bc1 | Format::Bc4 => 8,
            Format::Bc2 | Format::Bc3 | Format::Bc5 | Format::Bc7 => 16,
            Format::Bgra8 | Format::Bgrx8 | Format::Rgba8 => 4,
            Format::Bgr8 => 3,
            Format::L8 => 1,
        }
    }
    /// Byte size of one mip level of `w` x `h`.
    pub fn level_size(self, w: u32, h: u32) -> usize {
        let (w, h) = (w.max(1) as usize, h.max(1) as usize);
        if self.is_block_compressed() { w.div_ceil(4) * h.div_ceil(4) * self.unit_bytes() } else { w * h * self.unit_bytes() }
    }
}

#[derive(Debug, Clone)]
pub struct DdsInfo {
    pub width: u32,
    pub height: u32,
    pub mip_count: u32,
    pub format: Format,
    /// 6 for cubemaps, else the DX10 array size (1 when absent).
    pub layers: u32,
    pub srgb: bool,
    /// Offset of the first mip's data.
    pub data_offset: usize,
}

impl DdsInfo {
    /// (offset, size, width, height) of each mip of the first layer.
    pub fn mips(&self) -> Vec<(usize, usize, u32, u32)> {
        let mut off = self.data_offset;
        (0..self.mip_count)
            .map(|i| {
                let (w, h) = ((self.width >> i).max(1), (self.height >> i).max(1));
                let size = self.format.level_size(w, h);
                let m = (off, size, w, h);
                off += size;
                m
            })
            .collect()
    }
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

const DDPF_ALPHAPIXELS: u32 = 0x1;
const DDPF_FOURCC: u32 = 0x4;
const DDPF_RGB: u32 = 0x40;
const DDPF_LUMINANCE: u32 = 0x2_0000;
const DDSCAPS2_CUBEMAP: u32 = 0x200;

/// Parses the header(s) of a DDS file.
pub fn parse(b: &[u8]) -> Result<DdsInfo> {
    if b.len() < 128 || &b[0..4] != b"DDS " {
        return Err(Error::NotDds);
    }
    let h = 4; // DDS_HEADER starts after the magic
    let height = u32_at(b, h + 8);
    let width = u32_at(b, h + 12);
    let mip_count = u32_at(b, h + 24).max(1);
    let pf = h + 72;
    let pf_flags = u32_at(b, pf + 4);
    let four_cc = &b[pf + 8..pf + 12];
    let bit_count = u32_at(b, pf + 12);
    let (r_mask, a_mask) = (u32_at(b, pf + 16), u32_at(b, pf + 28));
    let caps2 = u32_at(b, h + 108);
    let mut layers = if caps2 & DDSCAPS2_CUBEMAP != 0 { 6 } else { 1 };
    let mut data_offset = 128;
    let mut srgb = false;

    let format = if pf_flags & DDPF_FOURCC != 0 {
        match four_cc {
            b"DXT1" => Format::Bc1,
            b"DXT2" | b"DXT3" => Format::Bc2,
            b"DXT4" | b"DXT5" => Format::Bc3,
            b"ATI1" | b"BC4U" => Format::Bc4,
            b"ATI2" | b"BC5U" => Format::Bc5,
            b"DX10" => {
                if b.len() < 148 {
                    return Err(Error::Truncated("DX10 header"));
                }
                data_offset = 148;
                let dxgi = u32_at(b, 128);
                layers = layers.max(u32_at(b, 128 + 12).max(1));
                srgb = matches!(dxgi, 72 | 75 | 78 | 99 | 29 | 91);
                match dxgi {
                    70..=72 => Format::Bc1,
                    73..=75 => Format::Bc2,
                    76..=78 => Format::Bc3,
                    79..=81 => Format::Bc4,
                    82..=84 => Format::Bc5,
                    97..=99 => Format::Bc7,
                    27..=29 => Format::Rgba8,
                    87 | 90 | 91 => Format::Bgra8,
                    88 | 92 | 93 => Format::Bgrx8,
                    other => return Err(Error::Unsupported(format!("DXGI format {other}"))),
                }
            }
            other => return Err(Error::Unsupported(format!("FourCC {:?}", String::from_utf8_lossy(other)))),
        }
    } else if pf_flags & DDPF_RGB != 0 {
        match (bit_count, r_mask) {
            (32, 0x00FF_0000) if pf_flags & DDPF_ALPHAPIXELS != 0 && a_mask != 0 => Format::Bgra8,
            (32, 0x00FF_0000) => Format::Bgrx8,
            (32, 0x0000_00FF) => Format::Rgba8,
            (24, 0x00FF_0000) => Format::Bgr8,
            _ => return Err(Error::Unsupported(format!("RGB {bit_count}-bit, R mask {r_mask:#x}"))),
        }
    } else if pf_flags & DDPF_LUMINANCE != 0 && bit_count == 8 {
        Format::L8
    } else {
        return Err(Error::Unsupported(format!("pixel format flags {pf_flags:#x}")));
    };

    let info = DdsInfo { width, height, mip_count, format, layers, srgb, data_offset };
    let needed = info.mips().last().map(|&(o, s, _, _)| o + s).unwrap_or(data_offset);
    if b.len() < needed {
        return Err(Error::Truncated("mip data"));
    }
    Ok(info)
}

/// Writes a BC7_UNORM DDS (DX10 header) from per-mip block data, largest mip first.
pub fn write_bc7(width: u32, height: u32, mips: &[Vec<u8>]) -> Vec<u8> {
    write_blocks(width, height, mips, b"DX10")
}

/// Writes a BC1 (`DXT1` FourCC, no DX10 header) DDS from per-mip block data, largest mip first.
pub fn write_dxt1(width: u32, height: u32, mips: &[Vec<u8>]) -> Vec<u8> {
    write_blocks(width, height, mips, b"DXT1")
}

fn write_blocks(width: u32, height: u32, mips: &[Vec<u8>], fourcc: &[u8; 4]) -> Vec<u8> {
    const DDSD_CAPS: u32 = 0x1;
    const DDSD_HEIGHT: u32 = 0x2;
    const DDSD_WIDTH: u32 = 0x4;
    const DDSD_PIXELFORMAT: u32 = 0x1000;
    const DDSD_MIPMAPCOUNT: u32 = 0x2_0000;
    const DDSD_LINEARSIZE: u32 = 0x8_0000;
    const DDSCAPS_COMPLEX: u32 = 0x8;
    const DDSCAPS_TEXTURE: u32 = 0x1000;
    const DDSCAPS_MIPMAP: u32 = 0x40_0000;
    let mut b = Vec::with_capacity(148 + mips.iter().map(Vec::len).sum::<usize>());
    let put = |b: &mut Vec<u8>, v: u32| b.extend_from_slice(&v.to_le_bytes());
    b.extend_from_slice(b"DDS ");
    put(&mut b, 124);
    let mut flags = DDSD_CAPS | DDSD_HEIGHT | DDSD_WIDTH | DDSD_PIXELFORMAT | DDSD_LINEARSIZE;
    if mips.len() > 1 {
        flags |= DDSD_MIPMAPCOUNT;
    }
    put(&mut b, flags);
    put(&mut b, height);
    put(&mut b, width);
    put(&mut b, mips.first().map_or(0, |m| m.len() as u32));
    put(&mut b, 0); // depth
    put(&mut b, mips.len() as u32);
    b.extend_from_slice(&[0u8; 44]); // reserved
    put(&mut b, 32); // pixel format size
    put(&mut b, DDPF_FOURCC);
    b.extend_from_slice(fourcc);
    b.extend_from_slice(&[0u8; 20]); // bit count + masks
    let mut caps = DDSCAPS_TEXTURE;
    if mips.len() > 1 {
        caps |= DDSCAPS_COMPLEX | DDSCAPS_MIPMAP;
    }
    put(&mut b, caps);
    b.extend_from_slice(&[0u8; 16]); // caps2..4, reserved
    if fourcc == b"DX10" {
        put(&mut b, 98); // DXGI_FORMAT_BC7_UNORM
        put(&mut b, 3); // D3D10_RESOURCE_DIMENSION_TEXTURE2D
        put(&mut b, 0);
        put(&mut b, 1); // array size
        put(&mut b, 0);
    }
    for m in mips {
        b.extend_from_slice(m);
    }
    b
}

/// RGBA8 pixels of one mip level.
#[derive(Debug, Clone)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Decodes mip `level` of the first layer to RGBA8. BC4 decodes to grey, BC5 to (R, G, 0, 255).
pub fn decode(b: &[u8], info: &DdsInfo, level: u32) -> Result<Image> {
    let &(off, size, w, h) = info.mips().get(level as usize).ok_or(Error::Truncated("mip level out of range"))?;
    let src = &b[off..off + size];
    let (wu, hu) = (w as usize, h as usize);
    let mut rgba = vec![0u8; wu * hu * 4];
    if info.format.is_block_compressed() {
        let block_fn: fn(&[u8], &mut [u32]) = match info.format {
            Format::Bc1 => t2d::decode_bc1a_block,
            Format::Bc2 => t2d::decode_bc2_block,
            Format::Bc3 => t2d::decode_bc3_block,
            Format::Bc4 => t2d::decode_bc4_block,
            Format::Bc5 => t2d::decode_bc5_block,
            Format::Bc7 => t2d::decode_bc7_block,
            _ => unreachable!(),
        };
        let unit = info.format.unit_bytes();
        let (bw, bh) = (wu.div_ceil(4), hu.div_ceil(4));
        let mut px = [0u32; 16];
        for by in 0..bh {
            for bx in 0..bw {
                let i = (by * bw + bx) * unit;
                block_fn(&src[i..i + unit], &mut px);
                for y in 0..4 {
                    let py = by * 4 + y;
                    if py >= hu {
                        break;
                    }
                    for x in 0..4 {
                        let pxx = bx * 4 + x;
                        if pxx >= wu {
                            break;
                        }
                        // texture2ddecoder packs pixels as B, G, R, A bytes.
                        let [bb, g, r, a] = px[y * 4 + x].to_le_bytes();
                        let o = (py * wu + pxx) * 4;
                        rgba[o..o + 4].copy_from_slice(&[r, g, bb, a]);
                    }
                }
            }
        }
    } else {
        let unit = info.format.unit_bytes();
        for (i, p) in src.chunks_exact(unit).enumerate() {
            let o = i * 4;
            let px = match info.format {
                Format::Bgra8 => [p[2], p[1], p[0], p[3]],
                Format::Bgrx8 => [p[2], p[1], p[0], 255],
                Format::Rgba8 => [p[0], p[1], p[2], p[3]],
                Format::Bgr8 => [p[2], p[1], p[0], 255],
                Format::L8 => [p[0], p[0], p[0], 255],
                _ => unreachable!(),
            };
            rgba[o..o + 4].copy_from_slice(&px);
        }
    }
    Ok(Image { width: w, height: h, rgba })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(w: u32, h: u32, mips: u32, four_cc: &[u8; 4]) -> Vec<u8> {
        let mut b = vec![0u8; 128];
        b[0..4].copy_from_slice(b"DDS ");
        b[4..8].copy_from_slice(&124u32.to_le_bytes());
        b[12..16].copy_from_slice(&h.to_le_bytes());
        b[16..20].copy_from_slice(&w.to_le_bytes());
        b[28..32].copy_from_slice(&mips.to_le_bytes());
        b[76..80].copy_from_slice(&32u32.to_le_bytes());
        b[80..84].copy_from_slice(&DDPF_FOURCC.to_le_bytes());
        b[84..88].copy_from_slice(four_cc);
        b
    }

    #[test]
    fn bc1_solid_red_with_mips() {
        // 8x8, 4 mips (8, 4, 2, 1) -> 4 + 1 + 1 + 1 blocks of 8 bytes.
        let mut b = header(8, 8, 4, b"DXT1");
        let red565: u16 = 0xF800;
        let block = [red565.to_le_bytes(), red565.to_le_bytes()].concat().into_iter().chain([0u8; 4]).collect::<Vec<_>>();
        for _ in 0..7 {
            b.extend_from_slice(&block);
        }
        let info = parse(&b).unwrap();
        assert_eq!((info.width, info.height, info.mip_count, info.format), (8, 8, 4, Format::Bc1));
        assert_eq!(info.mips().iter().map(|m| m.1).collect::<Vec<_>>(), [32, 8, 8, 8]);
        let img = decode(&b, &info, 0).unwrap();
        assert_eq!(&img.rgba[0..4], &[255, 0, 0, 255]);
        let tiny = decode(&b, &info, 3).unwrap();
        assert_eq!((tiny.width, tiny.height, tiny.rgba.len()), (1, 1, 4));
    }

    #[test]
    fn writes_bc7_with_mips() {
        let mips = vec![vec![0u8; 4 * 16], vec![0u8; 16], vec![0u8; 16]]; // 8x8, 4x4, 2x2
        let b = write_bc7(8, 8, &mips);
        let info = parse(&b).unwrap();
        assert_eq!((info.width, info.height, info.mip_count, info.format, info.data_offset), (8, 8, 3, Format::Bc7, 148));
        assert_eq!(b.len(), 148 + 96);
    }

    #[test]
    fn rejects_truncated() {
        let b = header(8, 8, 1, b"DXT5");
        assert!(matches!(parse(&b), Err(Error::Truncated(_))));
    }
}
