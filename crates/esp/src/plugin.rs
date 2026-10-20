//! A memory-mapped plugin file and its TES4 header.

use std::fs::File;
use std::path::{Path, PathBuf};

use memmap2::Mmap;

use crate::record::{fields, Entries, Entry, Record, Sig, HEADER_LEN};
use crate::{Error, Result};

pub const HEADER_FLAG_MASTER: u32 = 0x0000_0001;
pub const HEADER_FLAG_LOCALIZED: u32 = 0x0000_0080;
pub const HEADER_FLAG_LIGHT: u32 = 0x0000_0200;

#[derive(Debug, Clone)]
pub struct PluginHeader {
    pub flags: u32,
    pub version: f32,
    pub num_records: u32,
    pub masters: Vec<String>,
}

impl PluginHeader {
    pub fn is_master_flagged(&self) -> bool {
        self.flags & HEADER_FLAG_MASTER != 0
    }
    pub fn is_localized(&self) -> bool {
        self.flags & HEADER_FLAG_LOCALIZED != 0
    }
    pub fn is_light_flagged(&self) -> bool {
        self.flags & HEADER_FLAG_LIGHT != 0
    }
}

pub struct Plugin {
    /// File name as listed in the load order (case preserved), e.g. `Skyrim.esm`.
    pub name: String,
    pub path: PathBuf,
    pub header: PluginHeader,
    map: Mmap,
}

impl std::fmt::Debug for Plugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Plugin").field("name", &self.name).field("bytes", &self.map.len()).field("header", &self.header).finish()
    }
}

impl Plugin {
    pub fn open(path: &Path) -> Result<Plugin> {
        let file = File::open(path).map_err(|e| Error::Io { path: path.to_owned(), source: e })?;
        // SAFETY: plugin files are treated as read-only input; concurrent modification while
        // mapped is outside what this tool supports.
        let map = unsafe { Mmap::map(&file) }.map_err(|e| Error::Io { path: path.to_owned(), source: e })?;
        let header = parse_header(&map).map_err(|e| e.in_file(path))?;
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        Ok(Plugin { name, path: path.to_owned(), header, map })
    }

    pub fn bytes(&self) -> &[u8] {
        &self.map
    }

    /// `.esl` extension or the light flag (only honoured on .esm/.esp in SSE 1.6+).
    pub fn is_light(&self) -> bool {
        self.name.to_ascii_lowercase().ends_with(".esl") || self.header.is_light_flagged()
    }

    /// `.esm`/`.esl` extension or the master flag.
    pub fn is_master(&self) -> bool {
        let n = self.name.to_ascii_lowercase();
        n.ends_with(".esm") || n.ends_with(".esl") || self.header.is_master_flagged()
    }

    /// Top-level entries after the TES4 header record.
    pub fn top_entries(&self) -> Entries<'_> {
        let b = self.bytes();
        let header_len = HEADER_LEN + crate::record::u32_at(b, 4) as usize;
        Entries::new(&b[header_len..], header_len)
    }

    /// The top-level group for one record type (e.g. `WRLD`), if present.
    pub fn top_group(&self, sig: Sig) -> Result<Option<crate::record::Group<'_>>> {
        for e in self.top_entries() {
            if let Entry::Group(g) = e? {
                if g.label_sig() == sig {
                    return Ok(Some(g));
                }
            }
        }
        Ok(None)
    }
}

fn parse_header(b: &[u8]) -> Result<PluginHeader> {
    let first = Entries::new(b, 0).next().ok_or(Error::Malformed { offset: 0, what: "empty file" })??;
    let Entry::Record(rec) = first else {
        return Err(Error::Malformed { offset: 0, what: "file does not start with a TES4 record" });
    };
    if rec.sig != Sig(*b"TES4") {
        return Err(Error::Malformed { offset: 0, what: "file does not start with a TES4 record" });
    }
    header_from_record(&rec)
}

fn header_from_record(rec: &Record<'_>) -> Result<PluginHeader> {
    let data = rec.data()?;
    let mut h = PluginHeader { flags: rec.flags, version: 0.0, num_records: 0, masters: Vec::new() };
    for f in fields(&data) {
        match &f.sig.0 {
            b"HEDR" if f.data.len() >= 8 => {
                h.version = crate::record::f32_at(f.data, 0);
                h.num_records = crate::record::u32_at(f.data, 4);
            }
            b"MAST" => h.masters.push(f.zstring()),
            _ => {}
        }
    }
    Ok(h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::record::tests::{field, record};

    #[test]
    fn parses_header() {
        let mut hedr = 1.71f32.to_le_bytes().to_vec();
        hedr.extend(5u32.to_le_bytes());
        hedr.extend(0x800u32.to_le_bytes());
        let data = [field(b"HEDR", &hedr), field(b"MAST", b"Skyrim.esm\0"), field(b"DATA", &[0; 8]), field(b"MAST", b"Update.esm\0")].concat();
        let bytes = record(b"TES4", HEADER_FLAG_LIGHT, 0, &data);
        let h = parse_header(&bytes).unwrap();
        assert_eq!(h.masters, ["Skyrim.esm", "Update.esm"]);
        assert_eq!(h.num_records, 5);
        assert!(h.is_light_flagged());
    }
}
