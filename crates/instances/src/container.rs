//! The container all three files share (docs/runtime/instance-formats.md section 3): a 64-byte
//! header, a meta block of tagged chunks, section payloads and a sorted section index.

use std::borrow::Cow;

use crate::{Error, Result};

pub const MAJOR: u16 = 1;
pub const MINOR: u16 = 7;
pub const HEADER_SIZE: usize = 64;
pub const INDEX_ENTRY: usize = 32;
pub const FLAG_ZSTD: u8 = 1;

/// The three file kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Trees,
    Statics,
    Grass,
    MeshPack,
    Dyn,
}

impl Kind {
    pub fn magic(self) -> [u8; 4] {
        match self {
            Kind::Trees => *b"LFTR",
            Kind::Statics => *b"LFST",
            Kind::Grass => *b"LFGR",
            Kind::MeshPack => *b"LFMP",
            Kind::Dyn => *b"LFDY",
        }
    }

    pub fn record_size(self) -> u32 {
        match self {
            Kind::Trees => 24,
            Kind::Statics => 32,
            Kind::Grass => 10,
            Kind::MeshPack => 28,
            Kind::Dyn => 64,
        }
    }

    pub fn from_magic(m: &[u8]) -> Option<Kind> {
        match m {
            b"LFTR" => Some(Kind::Trees),
            b"LFST" => Some(Kind::Statics),
            b"LFGR" => Some(Kind::Grass),
            b"LFMP" => Some(Kind::MeshPack),
            b"LFDY" => Some(Kind::Dyn),
            _ => None,
        }
    }
}

/// The file header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub kind: Kind,
    pub major: u16,
    pub minor: u16,
    pub header_size: u32,
    pub flags: u32,
    pub record_size: u32,
    pub section_count: u32,
    pub index_offset: u64,
    pub meta_offset: u64,
    pub meta_size: u32,
    pub min: (i16, i16),
    pub max: (i16, i16),
    pub data_crc32: u32,
    pub total_records: u64,
}

/// One section index entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    pub x: i16,
    pub y: i16,
    pub level: u8,
    pub flags: u8,
    pub offset: u64,
    pub stored_size: u32,
    pub raw_size: u32,
    pub record_count: u32,
    pub raw_crc32: u32,
}

impl Entry {
    pub fn key(&self) -> (u8, i16, i16) {
        (self.level, self.y, self.x)
    }
}

/// Little-endian byte writer.
#[derive(Default)]
pub struct Buf(pub Vec<u8>);

impl Buf {
    pub fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    pub fn u16(&mut self, v: u16) {
        self.0.extend(v.to_le_bytes());
    }
    pub fn i16(&mut self, v: i16) {
        self.0.extend(v.to_le_bytes());
    }
    pub fn u32(&mut self, v: u32) {
        self.0.extend(v.to_le_bytes());
    }
    pub fn u64(&mut self, v: u64) {
        self.0.extend(v.to_le_bytes());
    }
    pub fn f32(&mut self, v: f32) {
        self.0.extend(v.to_le_bytes());
    }
    pub fn str(&mut self, s: &str) {
        let b = s.as_bytes();
        let n = b.len().min(usize::from(u16::MAX));
        self.u16(n as u16);
        self.0.extend(&b[..n]);
    }
    pub fn pad4(&mut self) {
        while !self.0.len().is_multiple_of(4) {
            self.0.push(0);
        }
    }
}

/// Little-endian byte reader.
pub struct Cur<'a> {
    pub d: &'a [u8],
    pub o: usize,
}

impl<'a> Cur<'a> {
    pub fn new(d: &'a [u8]) -> Cur<'a> {
        Cur { d, o: 0 }
    }
    pub fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let s = self.d.get(self.o..self.o + n).ok_or(Error::Truncated)?;
        self.o += n;
        Ok(s)
    }
    pub fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    pub fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    pub fn i16(&mut self) -> Result<i16> {
        Ok(i16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    pub fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
    pub fn f32(&mut self) -> Result<f32> {
        Ok(f32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn str(&mut self) -> Result<String> {
        let n = usize::from(self.u16()?);
        String::from_utf8(self.take(n)?.to_vec()).map_err(|_| Error::Invalid("string is not UTF-8"))
    }
}

/// A meta chunk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub tag: [u8; 4],
    pub data: Vec<u8>,
}

/// What every file's meta block holds besides its kind's tables.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Common {
    pub world: String,
    /// `""` for the default season, else `SPR`, `SUM`, `AUT` or `WIN`.
    pub season: String,
    pub generator: String,
    /// `PLUG`: plugin file names.
    pub plugins: Vec<String>,
    /// `PARM`: `key=value` lines.
    pub params: String,
    /// `BTOK` (since 1.7): the build's run token; empty = no chunk.
    pub token: String,
}

impl Common {
    pub fn chunks(&self) -> Vec<Chunk> {
        let mut w = Buf::default();
        w.str(&self.world);
        w.str(&self.season);
        w.str(&self.generator);
        let mut p = Buf::default();
        p.u32(self.plugins.len() as u32);
        for n in &self.plugins {
            p.str(n);
        }
        let mut m = Buf::default();
        m.str(&self.params);
        let mut v = vec![Chunk { tag: *b"WRLD", data: w.0 }, Chunk { tag: *b"PLUG", data: p.0 }, Chunk { tag: *b"PARM", data: m.0 }];
        if !self.token.is_empty() {
            let mut t = Buf::default();
            t.str(&self.token);
            v.push(Chunk { tag: *b"BTOK", data: t.0 });
        }
        v
    }

    pub fn from_chunks(chunks: &[Chunk]) -> Result<Common> {
        let mut c = Common::default();
        for ch in chunks {
            let mut r = Cur::new(&ch.data);
            match &ch.tag {
                b"WRLD" => {
                    c.world = r.str()?;
                    c.season = r.str()?;
                    c.generator = r.str()?;
                }
                b"PLUG" => {
                    let n = r.u32()? as usize;
                    c.plugins = (0..n).map(|_| r.str()).collect::<Result<_>>()?;
                }
                b"PARM" => c.params = r.str()?,
                b"BTOK" => c.token = r.str()?,
                _ => {}
            }
        }
        Ok(c)
    }

    /// The `PLUG` index of `name` (case-insensitive), added when missing.
    pub fn plugin_index(&mut self, name: &str) -> u16 {
        if let Some(i) = self.plugins.iter().position(|p| p.eq_ignore_ascii_case(name)) {
            return i as u16;
        }
        self.plugins.push(name.to_string());
        (self.plugins.len() - 1) as u16
    }

    /// The file name of this world / season: `<World>[.<SEASON>].<ext>`.
    pub fn file_name(&self, ext: &str) -> String {
        if self.season.is_empty() { format!("{}.{ext}", self.world) } else { format!("{}.{}.{ext}", self.world, self.season) }
    }
}

/// A section to write: its key, uncompressed payload and record count.
pub struct RawSection {
    pub level: u8,
    pub x: i16,
    pub y: i16,
    pub data: Vec<u8>,
    pub records: u32,
}

/// Writer options.
#[derive(Debug, Clone, Copy, Default)]
pub struct WriteOptions {
    /// zstd level for section payloads; `None` stores them raw. A section is stored raw when
    /// compression does not make it smaller.
    pub zstd: Option<i32>,
    /// Fill in `data_crc32` and the per-section `raw_crc32`.
    pub crc: bool,
}

/// Lays out a file: header, meta, payloads (4-byte aligned), index.
pub fn write(kind: Kind, chunks: &[Chunk], mut sections: Vec<RawSection>, opts: WriteOptions) -> Result<Vec<u8>> {
    sections.sort_by_key(|s| (s.level, s.y, s.x));
    for w in sections.windows(2) {
        if (w[0].level, w[0].y, w[0].x) == (w[1].level, w[1].y, w[1].x) {
            return Err(Error::Invalid("two sections with the same key"));
        }
    }
    let mut out = Buf(vec![0u8; HEADER_SIZE]);
    let meta_offset = out.0.len() as u64;
    for c in chunks {
        out.0.extend(c.tag);
        out.u32(c.data.len() as u32);
        out.0.extend(&c.data);
        out.pad4();
    }
    let meta_size = (out.0.len() as u64 - meta_offset) as u32;
    let mut entries = Vec::with_capacity(sections.len());
    let mut any_zstd = false;
    let mut total = 0u64;
    for s in &sections {
        out.pad4();
        let offset = out.0.len() as u64;
        let raw_crc32 = if opts.crc { crc32fast::hash(&s.data) } else { 0 };
        let mut flags = 0u8;
        let stored: Cow<[u8]> = match opts.zstd {
            Some(level) if !s.data.is_empty() => {
                let z = zstd::bulk::compress(&s.data, level).map_err(Error::Io)?;
                if z.len() < s.data.len() {
                    flags |= FLAG_ZSTD;
                    any_zstd = true;
                    Cow::Owned(z)
                } else {
                    Cow::Borrowed(&s.data)
                }
            }
            _ => Cow::Borrowed(&s.data),
        };
        out.0.extend(stored.iter());
        total += u64::from(s.records);
        entries.push(Entry { x: s.x, y: s.y, level: s.level, flags, offset, stored_size: stored.len() as u32, raw_size: s.data.len() as u32, record_count: s.records, raw_crc32 });
    }
    out.pad4();
    let index_offset = out.0.len() as u64;
    for e in &entries {
        out.i16(e.x);
        out.i16(e.y);
        out.u8(e.level);
        out.u8(e.flags);
        out.u16(0);
        out.u64(e.offset);
        out.u32(e.stored_size);
        out.u32(e.raw_size);
        out.u32(e.record_count);
        out.u32(e.raw_crc32);
    }
    let (min, max) = if entries.is_empty() {
        ((0, 0), (0, 0))
    } else {
        let f = |g: fn(&Entry) -> i16, m: fn(i16, i16) -> i16| entries.iter().map(g).reduce(m).unwrap();
        ((f(|e| e.x, i16::min), f(|e| e.y, i16::min)), (f(|e| e.x, i16::max), f(|e| e.y, i16::max)))
    };
    let data_crc32 = if opts.crc { crc32fast::hash(&out.0[HEADER_SIZE..]) } else { 0 };
    let mut h = Buf::default();
    h.0.extend(kind.magic());
    h.u16(MAJOR);
    h.u16(MINOR);
    h.u32(HEADER_SIZE as u32);
    h.u32(u32::from(any_zstd));
    h.u32(kind.record_size());
    h.u32(entries.len() as u32);
    h.u64(index_offset);
    h.u64(meta_offset);
    h.u32(meta_size);
    h.i16(min.0);
    h.i16(min.1);
    h.i16(max.0);
    h.i16(max.1);
    h.u32(data_crc32);
    h.u64(total);
    debug_assert_eq!(h.0.len(), HEADER_SIZE);
    out.0[..HEADER_SIZE].copy_from_slice(&h.0);
    Ok(out.0)
}

/// A parsed file: header, meta chunks and index; payloads are read on demand.
pub struct Container<'a> {
    pub bytes: &'a [u8],
    pub header: Header,
    pub chunks: Vec<Chunk>,
    pub index: Vec<Entry>,
}

impl<'a> Container<'a> {
    pub fn parse(bytes: &'a [u8], expect: Kind) -> Result<Container<'a>> {
        let mut r = Cur::new(bytes);
        let kind = Kind::from_magic(r.take(4)?).ok_or(Error::Invalid("not a LODForge instance file"))?;
        if kind != expect {
            return Err(Error::Invalid("wrong file kind"));
        }
        let major = r.u16()?;
        let minor = r.u16()?;
        if major != MAJOR {
            return Err(Error::Version(major, minor));
        }
        let header = Header {
            kind,
            major,
            minor,
            header_size: r.u32()?,
            flags: r.u32()?,
            record_size: r.u32()?,
            section_count: r.u32()?,
            index_offset: r.u64()?,
            meta_offset: r.u64()?,
            meta_size: r.u32()?,
            min: (r.i16()?, r.i16()?),
            max: (r.i16()?, r.i16()?),
            data_crc32: r.u32()?,
            total_records: r.u64()?,
        };
        if (header.header_size as usize) < HEADER_SIZE || header.record_size < kind.record_size() {
            return Err(Error::Invalid("header or record size too small"));
        }
        let mo = header.meta_offset as usize;
        let meta = bytes.get(mo..mo + header.meta_size as usize).ok_or(Error::Truncated)?;
        let mut chunks = Vec::new();
        let mut m = Cur::new(meta);
        while m.o + 8 <= meta.len() {
            let tag: [u8; 4] = m.take(4)?.try_into().unwrap();
            let n = m.u32()? as usize;
            let data = m.take(n)?.to_vec();
            m.o = (m.o + 3) & !3;
            chunks.push(Chunk { tag, data });
        }
        let mut index = Vec::with_capacity(header.section_count as usize);
        let mut ir = Cur::new(bytes);
        ir.o = header.index_offset as usize;
        for _ in 0..header.section_count {
            let x = ir.i16()?;
            let y = ir.i16()?;
            let level = ir.u8()?;
            let flags = ir.u8()?;
            let _ = ir.u16()?;
            index.push(Entry { x, y, level, flags, offset: ir.u64()?, stored_size: ir.u32()?, raw_size: ir.u32()?, record_count: ir.u32()?, raw_crc32: ir.u32()? });
        }
        Ok(Container { bytes, header, chunks, index })
    }

    pub fn chunk(&self, tag: &[u8; 4]) -> Option<&Chunk> {
        self.chunks.iter().find(|c| &c.tag == tag)
    }

    pub fn common(&self) -> Result<Common> {
        Common::from_chunks(&self.chunks)
    }

    /// Binary search of the sorted index.
    pub fn find(&self, level: u8, x: i16, y: i16) -> Option<&Entry> {
        self.index.binary_search_by_key(&(level, y, x), Entry::key).ok().map(|i| &self.index[i])
    }

    /// A section's uncompressed payload.
    pub fn payload(&self, e: &Entry) -> Result<Cow<'a, [u8]>> {
        let o = e.offset as usize;
        let stored = self.bytes.get(o..o + e.stored_size as usize).ok_or(Error::Truncated)?;
        let raw: Cow<[u8]> = if e.flags & FLAG_ZSTD != 0 {
            let v = zstd::bulk::decompress(stored, e.raw_size as usize).map_err(Error::Io)?;
            if v.len() != e.raw_size as usize {
                return Err(Error::Invalid("decompressed size differs from raw_size"));
            }
            Cow::Owned(v)
        } else {
            Cow::Borrowed(stored)
        };
        if e.raw_crc32 != 0 && crc32fast::hash(&raw) != e.raw_crc32 {
            return Err(Error::Invalid("section CRC mismatch"));
        }
        Ok(raw)
    }

    /// Checks `data_crc32` when it is set.
    pub fn verify(&self) -> Result<()> {
        if self.header.data_crc32 != 0 && crc32fast::hash(&self.bytes[self.header.header_size as usize..]) != self.header.data_crc32 {
            return Err(Error::Invalid("file CRC mismatch"));
        }
        Ok(())
    }
}
