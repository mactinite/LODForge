//! Far grass, `<World>[.<SEASON>].lfg` (docs/runtime/instance-formats.md section 7), and its
//! conversion from NGIO `.cgid` grass cache cells (format interoperability through
//! `lodforge_grass::cgid`; no grass is generated here).

use std::collections::{BTreeMap, HashMap};

use half::f16;

use crate::container::{Buf, Chunk, Common, Container, Cur, Kind, RawSection, WriteOptions, write};
use crate::hash::{dequantize_rel, grass_hash, quantize_rel};
use crate::{Error, Result};

pub const EXT: &str = "lfg";
/// Density levels per cell (`prefix[k]`: records with `hash < 2^(32-k)`).
pub const LEVELS: usize = 8;
/// Bytes per record.
pub const RECORD: usize = 10;

pub const FLAG_FITS_TO_SLOPE: u16 = 1;
pub const FLAG_UNIFORM_SCALING: u16 = 2;
pub const FLAG_VERTEX_LIGHTING: u16 = 4;
/// `GTYP.plugin` when the GRAS record was not found in the build's load order.
pub const UNRESOLVED: u16 = 0xFFFF;

/// One `GTYP` entry.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GrassType {
    pub plugin: u16,
    pub flags: u16,
    pub form: u32,
    pub wave_period: f32,
    pub model: String,
}

/// One instance before quantisation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Instance {
    pub pos: [f32; 3],
    pub yaw: f32,
    pub scale: f32,
    /// Unit surface normal (z up).
    pub normal: [f32; 3],
    /// `GTYP` index.
    pub type_: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GrassRecord {
    pub qx: u16,
    pub qy: u16,
    pub qz: u16,
    pub type_: u8,
    pub yaw: u8,
    pub scale: u8,
    pub tilt: u8,
}

impl GrassRecord {
    pub fn yaw_rad(&self) -> f32 {
        f32::from(self.yaw) * std::f32::consts::TAU / 256.0
    }

    pub fn scale_f32(&self) -> f32 {
        f32::from(self.scale) / 64.0
    }

    pub fn normal(&self) -> [f32; 3] {
        let nib = |v: u8| -> f32 { f32::from(((v << 4) as i8) >> 4) / 7.0 };
        let nx = nib(self.tilt >> 4);
        let ny = nib(self.tilt & 15);
        [nx, ny, (1.0 - nx * nx - ny * ny).max(0.0).sqrt()]
    }
}

fn tilt_of(n: [f32; 3]) -> u8 {
    let q = |v: f32| -> u8 { ((v.clamp(-1.0, 1.0) * 7.0).round() as i8 as u8) & 15 };
    (q(n[0]) << 4) | q(n[1])
}

/// One cell section.
#[derive(Debug, Clone, PartialEq)]
pub struct GrassCell {
    pub x: i16,
    pub y: i16,
    pub z_min: f32,
    pub z_step: f32,
    /// Cell type index -> `GTYP` index.
    pub types: Vec<u16>,
    pub prefix: [u32; LEVELS],
    /// Sorted by [`grass_hash`].
    pub records: Vec<GrassRecord>,
}

impl GrassCell {
    /// Quantises and hash-sorts the instances of cell `(x, y)` (instances outside the cell are
    /// clamped to its edge). At most 255 types per cell.
    pub fn build(x: i16, y: i16, instances: &[Instance]) -> Result<GrassCell> {
        let (z_min, z_max) = instances.iter().fold((f32::MAX, f32::MIN), |(lo, hi), i| (lo.min(i.pos[2]), hi.max(i.pos[2])));
        let (z_min, z_step) = if instances.is_empty() { (0.0, 1.0) } else { (z_min, if z_max > z_min { (z_max - z_min) / 65535.0 } else { 1.0 }) };
        let mut types: Vec<u16> = Vec::new();
        let mut local: HashMap<u16, u8> = HashMap::new();
        let mut keyed: Vec<(u32, usize, GrassRecord)> = Vec::with_capacity(instances.len());
        for (k, i) in instances.iter().enumerate() {
            let t = match local.get(&i.type_) {
                Some(&t) => t,
                None => {
                    if types.len() == 255 {
                        return Err(Error::Invalid("more than 255 grass types in a cell"));
                    }
                    types.push(i.type_);
                    let t = (types.len() - 1) as u8;
                    local.insert(i.type_, t);
                    t
                }
            };
            let qx = quantize_rel(i.pos[0], i32::from(x));
            let qy = quantize_rel(i.pos[1], i32::from(y));
            let qz = (((i.pos[2] - z_min) / z_step).round()).clamp(0.0, 65535.0) as u16;
            let yaw = ((i.yaw.rem_euclid(std::f32::consts::TAU) / std::f32::consts::TAU * 256.0).round() as u32 % 256) as u8;
            let scale = (i.scale * 64.0).round().clamp(0.0, 255.0) as u8;
            keyed.push((grass_hash(x, y, qx, qy), k, GrassRecord { qx, qy, qz, type_: t, yaw, scale, tilt: tilt_of(i.normal) }));
        }
        keyed.sort_by_key(|(h, k, _)| (*h, *k));
        let mut prefix = [0u32; LEVELS];
        for (k, p) in prefix.iter_mut().enumerate() {
            let limit = 1u64 << (32 - k);
            *p = keyed.partition_point(|(h, _, _)| u64::from(*h) < limit) as u32;
        }
        Ok(GrassCell { x, y, z_min, z_step, types, prefix, records: keyed.into_iter().map(|(_, _, r)| r).collect() })
    }

    pub fn header_size(&self) -> usize {
        16 + 4 * LEVELS + 2 * self.types.len()
    }

    /// World position of a record.
    pub fn position(&self, r: &GrassRecord) -> [f32; 3] {
        [dequantize_rel(r.qx, i32::from(self.x)), dequantize_rel(r.qy, i32::from(self.y)), self.z_min + f32::from(r.qz) * self.z_step]
    }

    pub fn to_payload(&self) -> Vec<u8> {
        let mut b = Buf(Vec::with_capacity(self.header_size() + self.records.len() * RECORD));
        b.i16(self.x);
        b.i16(self.y);
        b.f32(self.z_min);
        b.f32(self.z_step);
        b.u16(self.types.len() as u16);
        b.u8(LEVELS as u8);
        b.u8(0);
        for p in self.prefix {
            b.u32(p);
        }
        for t in &self.types {
            b.u16(*t);
        }
        for r in &self.records {
            b.u16(r.qx);
            b.u16(r.qy);
            b.u16(r.qz);
            b.u8(r.type_);
            b.u8(r.yaw);
            b.u8(r.scale);
            b.u8(r.tilt);
        }
        b.0
    }

    /// Parses one section payload; `stride` is the file's record size.
    pub fn parse(raw: &[u8], stride: usize, record_count: usize) -> Result<GrassCell> {
        let mut r = Cur::new(raw);
        let x = r.i16()?;
        let y = r.i16()?;
        let z_min = r.f32()?;
        let z_step = r.f32()?;
        let nt = usize::from(r.u16()?);
        let nl = usize::from(r.u8()?);
        let _ = r.u8()?;
        let mut prefix = [0u32; LEVELS];
        for k in 0..nl {
            let v = r.u32()?;
            if k < LEVELS {
                prefix[k] = v;
            }
        }
        let types = (0..nt).map(|_| r.u16()).collect::<Result<Vec<_>>>()?;
        let base = r.o;
        let mut records = Vec::with_capacity(record_count);
        for i in 0..record_count {
            let mut c = Cur::new(raw.get(base + i * stride..base + (i + 1) * stride).ok_or(Error::Truncated)?);
            records.push(GrassRecord { qx: c.u16()?, qy: c.u16()?, qz: c.u16()?, type_: c.u8()?, yaw: c.u8()?, scale: c.u8()?, tilt: c.u8()? });
        }
        Ok(GrassCell { x, y, z_min, z_step, types, prefix, records })
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct GrassFile {
    pub common: Common,
    pub types: Vec<GrassType>,
    pub cells: BTreeMap<(i16, i16), GrassCell>,
}

impl GrassFile {
    fn type_chunk(&self) -> Chunk {
        let mut b = Buf::default();
        b.u32(self.types.len() as u32);
        for t in &self.types {
            b.u16(t.plugin);
            b.u16(t.flags);
            b.u32(t.form);
            b.f32(t.wave_period);
            b.str(&t.model);
        }
        Chunk { tag: *b"GTYP", data: b.0 }
    }

    pub fn records(&self) -> usize {
        self.cells.values().map(|c| c.records.len()).sum()
    }

    pub fn to_bytes(&self, opts: WriteOptions) -> Result<Vec<u8>> {
        let mut chunks = self.common.chunks();
        chunks.push(self.type_chunk());
        let sections = self.cells.values().map(|c| RawSection { level: 0, x: c.x, y: c.y, data: c.to_payload(), records: c.records.len() as u32 }).collect();
        write(Kind::Grass, &chunks, sections, opts)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<GrassFile> {
        let c = Container::parse(bytes, Kind::Grass)?;
        c.verify()?;
        let common = c.common()?;
        let mut types = Vec::new();
        if let Some(ch) = c.chunk(b"GTYP") {
            let mut r = Cur::new(&ch.data);
            for _ in 0..r.u32()? {
                types.push(GrassType { plugin: r.u16()?, flags: r.u16()?, form: r.u32()?, wave_period: r.f32()?, model: r.str()? });
            }
        }
        let mut cells = BTreeMap::new();
        for e in &c.index {
            let raw = c.payload(e)?;
            let cell = GrassCell::parse(&raw, c.header.record_size as usize, e.record_count as usize)?;
            cells.insert((e.x, e.y), cell);
        }
        Ok(GrassFile { common, types, cells })
    }
}

/// The instances of one `.cgid` cell. `type_of` maps a cache type to its `GTYP` index (`None`
/// skips the type). Positions are made absolute with the 12x12 block origin of `cell`.
pub fn instances_from_cgid(cell: (i32, i32), cache: &lodforge_grass::cgid::CellCache, mut type_of: impl FnMut(&lodforge_grass::cgid::TypeCache) -> Option<u16>) -> Vec<Instance> {
    let block = |c: i32| (12 * (c / 12)) as f32 * 4096.0;
    let (bx, by) = (block(cell.0), block(cell.1));
    let h = |v: u16| f16::from_bits(v).to_f32();
    let mut out = Vec::with_capacity(cache.instance_count());
    for t in &cache.types {
        let Some(ti) = type_of(t) else { continue };
        for g in &t.groups {
            for i in &g.instances {
                // Stored order r0 r1 r2 r7 r3 r4 r5 r8 r6.
                let s = &i[4..13];
                let (r0, r2, r3, r5, r8) = (h(s[0]), h(s[2]), h(s[4]), h(s[6]), h(s[7]));
                let n = [r2, r5, r8];
                let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                let normal = if len > 1e-3 && n[2] > 0.0 { n.map(|v| v / len) } else { [0.0, 0.0, 1.0] };
                out.push(Instance { pos: [h(i[0]) + bx, h(i[1]) + by, h(i[2])], yaw: r3.atan2(r0), scale: 1.0 + h(i[13]), normal, type_: ti });
            }
        }
    }
    out
}
