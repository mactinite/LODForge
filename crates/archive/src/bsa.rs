//! Bethesda archive (BSA) reader for Skyrim LE (v104, zlib) and SE (v105, LZ4 frame).
//!
//! Layout: 36-byte header; folder records (v104: hash u64, count u32, offset u32; v105: hash u64,
//! count u32, pad u32, offset u64); per folder an optional bzstring name followed by file records
//! (hash u64, size u32, offset u32); then the file-name block (zstrings in file order).
//! File size bit 0x4000_0000 inverts the archive's default compression. With archive flag 0x100
//! each file's data starts with a bstring of its full path. Compressed data starts with the
//! uncompressed size (u32).

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use memmap2::Mmap;

use crate::{normalize, Error, Result};

const FLAG_DIR_NAMES: u32 = 0x1;
const FLAG_FILE_NAMES: u32 = 0x2;
const FLAG_COMPRESSED: u32 = 0x4;
const FLAG_EMBED_NAMES: u32 = 0x100;
const SIZE_TOGGLE_COMPRESSION: u32 = 0x4000_0000;
const SIZE_MASK: u32 = 0x3FFF_FFFF;

#[derive(Debug, Clone)]
pub struct BsaFile {
    /// Normalized path (lowercase, backslashes), e.g. `meshes\terrain\tamriel\objects\x.bto`.
    pub path: String,
    pub offset: u64,
    pub size: u32,
    pub compressed: bool,
}

pub struct Bsa {
    pub path: PathBuf,
    pub version: u32,
    pub flags: u32,
    pub files: Vec<BsaFile>,
    map: Mmap,
}

impl std::fmt::Debug for Bsa {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Bsa({}, v{}, {} files)", self.path.display(), self.version, self.files.len())
    }
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

impl Bsa {
    pub fn open(path: &Path) -> Result<Bsa> {
        let file = File::open(path).map_err(|e| Error::Io { path: path.to_owned(), source: e })?;
        // SAFETY: archives are read-only inputs for the duration of a build.
        let map = unsafe { Mmap::map(&file) }.map_err(|e| Error::Io { path: path.to_owned(), source: e })?;
        let bad = |what: &'static str| Error::Malformed { path: path.to_owned(), what };
        let b: &[u8] = &map;
        if b.len() < 36 || &b[0..4] != b"BSA\0" {
            return Err(bad("not a BSA"));
        }
        let version = u32_at(b, 4);
        if version != 104 && version != 105 {
            return Err(bad("unsupported BSA version"));
        }
        let header_size = u32_at(b, 8) as usize;
        let flags = u32_at(b, 12);
        let folder_count = u32_at(b, 16) as usize;
        let file_count = u32_at(b, 20) as usize;
        let folder_rec = if version == 105 { 24 } else { 16 };
        if flags & FLAG_FILE_NAMES == 0 {
            return Err(bad("archive has no file names"));
        }

        let mut pos = header_size + folder_count * folder_rec;
        let mut folders = Vec::with_capacity(folder_count);
        let mut records: Vec<(usize, u32, u32)> = Vec::with_capacity(file_count); // (folder, size, offset)
        for fi in 0..folder_count {
            let count = u32_at(b, header_size + fi * folder_rec + 8) as usize;
            let name = if flags & FLAG_DIR_NAMES != 0 {
                let len = *b.get(pos).ok_or_else(|| bad("truncated folder name"))? as usize;
                let raw = b.get(pos + 1..pos + 1 + len).ok_or_else(|| bad("truncated folder name"))?;
                pos += 1 + len;
                let end = raw.iter().position(|&c| c == 0).unwrap_or(raw.len());
                normalize(&String::from_utf8_lossy(&raw[..end]))
            } else {
                String::new()
            };
            folders.push(name);
            if pos + count * 16 > b.len() {
                return Err(bad("truncated file records"));
            }
            for k in 0..count {
                let r = pos + k * 16;
                records.push((fi, u32_at(b, r + 8), u32_at(b, r + 12)));
            }
            pos += count * 16;
        }
        if records.len() != file_count {
            return Err(bad("file count mismatch"));
        }

        let default_compressed = flags & FLAG_COMPRESSED != 0;
        let mut files = Vec::with_capacity(file_count);
        for (folder, size, offset) in records {
            let end = b[pos..].iter().position(|&c| c == 0).ok_or_else(|| bad("truncated file names"))?;
            let name = String::from_utf8_lossy(&b[pos..pos + end]).to_ascii_lowercase();
            pos += end + 1;
            let dir = &folders[folder];
            let path = if dir.is_empty() || dir == "." { name } else { format!("{dir}\\{name}") };
            files.push(BsaFile {
                path,
                offset: u64::from(offset),
                size: size & SIZE_MASK,
                compressed: default_compressed != (size & SIZE_TOGGLE_COMPRESSION != 0),
            });
        }
        Ok(Bsa { path: path.to_owned(), version, flags, files, map })
    }

    /// Extracts one file's contents.
    pub fn read(&self, index: usize) -> Result<Vec<u8>> {
        let f = &self.files[index];
        let bad = |what: &'static str| Error::Malformed { path: self.path.clone(), what };
        let start = f.offset as usize;
        let mut data = self.map.get(start..start + f.size as usize).ok_or_else(|| bad("file data out of range"))?;
        if self.flags & FLAG_EMBED_NAMES != 0 {
            let len = *data.first().ok_or_else(|| bad("missing embedded name"))? as usize;
            data = data.get(1 + len..).ok_or_else(|| bad("embedded name out of range"))?;
        }
        if !f.compressed {
            return Ok(data.to_vec());
        }
        if data.len() < 4 {
            return Err(bad("compressed file shorter than 4 bytes"));
        }
        let original = u32_at(data, 0) as usize;
        let mut out = Vec::with_capacity(original);
        let res = if self.version == 105 {
            lz4_flex::frame::FrameDecoder::new(&data[4..]).read_to_end(&mut out)
        } else {
            flate2::read::ZlibDecoder::new(&data[4..]).read_to_end(&mut out)
        };
        res.map_err(|e| Error::Io { path: self.path.clone(), source: e })?;
        if out.len() != original {
            return Err(bad("decompressed size mismatch"));
        }
        Ok(out)
    }
}
