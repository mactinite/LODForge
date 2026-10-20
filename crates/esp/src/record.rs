//! Low-level TES4-format (Skyrim SE) record, group and subrecord parsing over a byte slice.
//!
//! Layout (all little-endian):
//! - record header, 24 bytes: signature[4], data_size u32, flags u32, form_id u32, vc_info u32,
//!   version u16, unknown u16; followed by `data_size` bytes of data.
//! - group header, 24 bytes: "GRUP", group_size u32 (includes the header), label[4], group_type i32,
//!   vc_info u32, unknown u32; followed by nested records/groups.
//! - compressed record (flag 0x0004_0000): data = decompressed_size u32 + zlib stream.
//! - subrecord: signature[4], size u16, data. An `XXXX` subrecord (size 4) carries a u32 size
//!   that replaces the u16 size of the subrecord that follows it.

use std::borrow::Cow;
use std::fmt;
use std::io::Read;

use crate::{Error, Result};

pub const HEADER_LEN: usize = 24;

pub const FLAG_COMPRESSED: u32 = 0x0004_0000;
pub const FLAG_DELETED: u32 = 0x0000_0020;
pub const FLAG_PERSISTENT: u32 = 0x0000_0400;
pub const FLAG_INITIALLY_DISABLED: u32 = 0x0000_0800;

/// A four-character record or subrecord signature.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Sig(pub [u8; 4]);

impl Sig {
    pub const GRUP: Sig = Sig(*b"GRUP");
    pub const fn new(s: &[u8; 4]) -> Self {
        Sig(*s)
    }
}

impl fmt::Debug for Sig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", String::from_utf8_lossy(&self.0))
    }
}

impl fmt::Display for Sig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self, f)
    }
}

/// Group types in the GRUP header.
pub mod group_type {
    pub const TOP: i32 = 0;
    pub const WORLD_CHILDREN: i32 = 1;
    pub const INTERIOR_BLOCK: i32 = 2;
    pub const INTERIOR_SUB_BLOCK: i32 = 3;
    pub const EXTERIOR_BLOCK: i32 = 4;
    pub const EXTERIOR_SUB_BLOCK: i32 = 5;
    pub const CELL_CHILDREN: i32 = 6;
    pub const TOPIC_CHILDREN: i32 = 7;
    pub const CELL_PERSISTENT: i32 = 8;
    pub const CELL_TEMPORARY: i32 = 9;
}

#[inline]
pub(crate) fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
#[inline]
pub(crate) fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
#[inline]
pub(crate) fn i32_at(b: &[u8], o: usize) -> i32 {
    u32_at(b, o) as i32
}
#[inline]
pub(crate) fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_bits(u32_at(b, o))
}

/// A record (not a group) borrowed from plugin bytes.
#[derive(Clone, Copy)]
pub struct Record<'a> {
    pub sig: Sig,
    pub flags: u32,
    /// Form ID as stored in the file (high byte indexes this plugin's master list).
    pub form_id: u32,
    pub version: u16,
    /// Absolute offset of the record header in the plugin.
    pub offset: usize,
    raw: &'a [u8],
}

impl fmt::Debug for Record<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} [{:08X}] @{:#x} ({} bytes)", self.sig, self.form_id, self.offset, self.raw.len())
    }
}

impl<'a> Record<'a> {
    pub fn is_compressed(&self) -> bool {
        self.flags & FLAG_COMPRESSED != 0
    }
    pub fn is_deleted(&self) -> bool {
        self.flags & FLAG_DELETED != 0
    }
    /// Raw (possibly compressed) record data, without the header.
    pub fn raw_data(&self) -> &'a [u8] {
        self.raw
    }

    /// Record data, decompressed if needed.
    pub fn data(&self) -> Result<Cow<'a, [u8]>> {
        if !self.is_compressed() {
            return Ok(Cow::Borrowed(self.raw));
        }
        if self.raw.len() < 4 {
            return Err(Error::Malformed { offset: self.offset, what: "compressed record shorter than 4 bytes" });
        }
        let size = u32_at(self.raw, 0) as usize;
        let mut out = Vec::with_capacity(size);
        flate2::read::ZlibDecoder::new(&self.raw[4..])
            .read_to_end(&mut out)
            .map_err(|e| Error::Decompress { offset: self.offset, source: e })?;
        if out.len() != size {
            return Err(Error::Malformed { offset: self.offset, what: "decompressed size mismatch" });
        }
        Ok(Cow::Owned(out))
    }
}

/// A group header borrowed from plugin bytes; `body` holds the nested entries.
#[derive(Clone, Copy)]
pub struct Group<'a> {
    pub label: [u8; 4],
    pub group_type: i32,
    pub offset: usize,
    body: &'a [u8],
}

impl fmt::Debug for Group<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "GRUP type {} label {:02X?} @{:#x} ({} bytes)", self.group_type, self.label, self.offset, self.body.len())
    }
}

impl<'a> Group<'a> {
    pub fn label_sig(&self) -> Sig {
        Sig(self.label)
    }
    pub fn label_u32(&self) -> u32 {
        u32::from_le_bytes(self.label)
    }
    /// Exterior block/sub-block label as (x, y); the file stores Y first, then X, as i16 each.
    pub fn label_grid(&self) -> (i16, i16) {
        let y = i16::from_le_bytes([self.label[0], self.label[1]]);
        let x = i16::from_le_bytes([self.label[2], self.label[3]]);
        (x, y)
    }
    pub fn entries(&self) -> Entries<'a> {
        Entries { bytes: self.body, base: self.offset + HEADER_LEN, pos: 0 }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Entry<'a> {
    Record(Record<'a>),
    Group(Group<'a>),
}

/// Iterator over sibling records/groups in a byte range.
#[derive(Clone)]
pub struct Entries<'a> {
    bytes: &'a [u8],
    base: usize,
    pos: usize,
}

impl<'a> Entries<'a> {
    /// Iterates entries in `bytes`, whose first byte sits at absolute plugin offset `base`.
    pub fn new(bytes: &'a [u8], base: usize) -> Self {
        Entries { bytes, base, pos: 0 }
    }
}

impl<'a> Iterator for Entries<'a> {
    type Item = Result<Entry<'a>>;

    fn next(&mut self) -> Option<Self::Item> {
        let b = self.bytes;
        let p = self.pos;
        if p >= b.len() {
            return None;
        }
        let offset = self.base + p;
        if b.len() - p < HEADER_LEN {
            self.pos = b.len();
            return Some(Err(Error::Malformed { offset, what: "truncated header" }));
        }
        let sig = Sig([b[p], b[p + 1], b[p + 2], b[p + 3]]);
        let size = u32_at(b, p + 4) as usize;
        if sig == Sig::GRUP {
            if size < HEADER_LEN || p + size > b.len() {
                self.pos = b.len();
                return Some(Err(Error::Malformed { offset, what: "group size out of range" }));
            }
            self.pos = p + size;
            Some(Ok(Entry::Group(Group {
                label: [b[p + 8], b[p + 9], b[p + 10], b[p + 11]],
                group_type: i32_at(b, p + 12),
                offset,
                body: &b[p + HEADER_LEN..p + size],
            })))
        } else {
            let end = p + HEADER_LEN + size;
            if end > b.len() {
                self.pos = b.len();
                return Some(Err(Error::Malformed { offset, what: "record size out of range" }));
            }
            self.pos = end;
            Some(Ok(Entry::Record(Record {
                sig,
                flags: u32_at(b, p + 8),
                form_id: u32_at(b, p + 12),
                version: u16_at(b, p + 20),
                offset,
                raw: &b[p + HEADER_LEN..end],
            })))
        }
    }
}

/// A subrecord within decompressed record data.
#[derive(Clone, Copy)]
pub struct Field<'a> {
    pub sig: Sig,
    pub data: &'a [u8],
}

impl fmt::Debug for Field<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({} bytes)", self.sig, self.data.len())
    }
}

impl<'a> Field<'a> {
    /// Zero-terminated string (Windows-1252 in practice; non-ASCII bytes are mapped lossily).
    pub fn zstring(&self) -> String {
        let end = self.data.iter().position(|&c| c == 0).unwrap_or(self.data.len());
        self.data[..end].iter().map(|&c| c as char).collect()
    }
    pub fn u32(&self) -> Option<u32> {
        (self.data.len() >= 4).then(|| u32_at(self.data, 0))
    }
}

/// Iterator over the subrecords of decompressed record data.
pub struct Fields<'a> {
    data: &'a [u8],
    pos: usize,
}

pub fn fields(data: &[u8]) -> Fields<'_> {
    Fields { data, pos: 0 }
}

impl<'a> Iterator for Fields<'a> {
    type Item = Field<'a>;

    fn next(&mut self) -> Option<Field<'a>> {
        let d = self.data;
        let mut override_size = None;
        loop {
            if self.pos + 6 > d.len() {
                return None;
            }
            let sig = Sig([d[self.pos], d[self.pos + 1], d[self.pos + 2], d[self.pos + 3]]);
            let small = u16_at(d, self.pos + 4) as usize;
            let start = self.pos + 6;
            if sig == Sig(*b"XXXX") && small == 4 && start + 4 <= d.len() {
                override_size = Some(u32_at(d, start) as usize);
                self.pos = start + 4;
                continue;
            }
            let size = override_size.unwrap_or(small);
            let end = (start + size).min(d.len());
            self.pos = end;
            return Some(Field { sig, data: &d[start..end] });
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn record(sig: &[u8; 4], flags: u32, form_id: u32, data: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(sig);
        v.extend_from_slice(&(data.len() as u32).to_le_bytes());
        v.extend_from_slice(&flags.to_le_bytes());
        v.extend_from_slice(&form_id.to_le_bytes());
        v.extend_from_slice(&[0; 4]);
        v.extend_from_slice(&44u16.to_le_bytes());
        v.extend_from_slice(&[0; 2]);
        v.extend_from_slice(data);
        v
    }

    pub fn group(label: [u8; 4], group_type: i32, body: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(b"GRUP");
        v.extend_from_slice(&((body.len() + HEADER_LEN) as u32).to_le_bytes());
        v.extend_from_slice(&label);
        v.extend_from_slice(&group_type.to_le_bytes());
        v.extend_from_slice(&[0; 8]);
        v.extend_from_slice(body);
        v
    }

    pub fn field(sig: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(sig);
        v.extend_from_slice(&(data.len() as u16).to_le_bytes());
        v.extend_from_slice(data);
        v
    }

    #[test]
    fn walks_records_and_groups() {
        let r1 = record(b"STAT", 0, 0x0100_0800, &field(b"EDID", b"Rock\0"));
        let inner = record(b"REFR", 0, 0x0100_0801, &[]);
        let g = group(*b"STAT", group_type::TOP, &[r1.clone(), group([1, 0, 0, 0], 9, &inner)].concat());
        let entries: Vec<_> = Entries::new(&g, 0).collect::<Result<_>>().unwrap();
        assert_eq!(entries.len(), 1);
        let Entry::Group(top) = entries[0] else { panic!() };
        assert_eq!(top.label_sig(), Sig(*b"STAT"));
        let kids: Vec<_> = top.entries().collect::<Result<_>>().unwrap();
        let Entry::Record(stat) = kids[0] else { panic!() };
        assert_eq!(stat.form_id, 0x0100_0800);
        assert_eq!(stat.offset, HEADER_LEN);
        let f: Vec<_> = fields(&stat.data().unwrap()).map(|f| (f.sig, f.zstring())).collect();
        assert_eq!(f, vec![(Sig(*b"EDID"), "Rock".to_string())]);
        let Entry::Group(temp) = kids[1] else { panic!() };
        assert_eq!(temp.group_type, 9);
        assert_eq!(temp.entries().count(), 1);
    }

    #[test]
    fn decompresses_and_reads_xxxx() {
        use flate2::{write::ZlibEncoder, Compression};
        use std::io::Write;
        let big = vec![7u8; 70_000];
        let payload = [field(b"XXXX", &(big.len() as u32).to_le_bytes()), {
            let mut v = b"OFST".to_vec();
            v.extend_from_slice(&0u16.to_le_bytes());
            v.extend_from_slice(&big);
            v
        }, field(b"EDID", b"X\0")]
        .concat();
        let mut enc = ZlibEncoder::new(Vec::new(), Compression::default());
        enc.write_all(&payload).unwrap();
        let mut data = (payload.len() as u32).to_le_bytes().to_vec();
        data.extend(enc.finish().unwrap());
        let bytes = record(b"WRLD", FLAG_COMPRESSED, 0x3C, &data);
        let Some(Ok(Entry::Record(r))) = Entries::new(&bytes, 0).next() else { panic!() };
        let d = r.data().unwrap();
        let f: Vec<_> = fields(&d).collect();
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].sig, Sig(*b"OFST"));
        assert_eq!(f[0].data.len(), 70_000);
        assert_eq!(f[1].zstring(), "X");
    }
}
