//! `.cgid` files (the engine's `.gid` layout, which NGIO writes with the instance data the
//! vanilla writer omits). Little-endian; see the spec §3.

use half::f16;

/// A GRAS record as the cache types need it.
#[derive(Debug, Clone)]
pub struct Gras {
    pub form_id: u32,
    /// MODL as authored.
    pub model: String,
    pub density: u8,
    pub min_slope: u8,
    pub max_slope: u8,
    pub units_from_water: u16,
    pub water_state: u32,
    pub position_range: f32,
    pub height_range: f32,
    pub color_range: f32,
    pub wave_period: f32,
    pub flags: u8,
}

impl Gras {
    pub fn vertex_lighting(&self) -> bool {
        self.flags & 1 != 0
    }
    pub fn uniform_scaling(&self) -> bool {
        self.flags & 2 != 0
    }
    pub fn fits_to_slope(&self) -> bool {
        self.flags & 4 != 0
    }
}

pub struct Group {
    pub center: [f32; 3],
    pub half_extent: [f32; 3],
    pub instances: Vec<[u16; 16]>,
}

impl Group {
    /// Bounds as AddGroup computes them: decoded instance AABB (x/y relative to the 12x12 block
    /// origin `block`) padded by 30.
    pub fn new(instances: Vec<[u16; 16]>, block: (f32, f32)) -> Group {
        let mut lo = [f32::MAX; 3];
        let mut hi = [-f32::MAX; 3];
        for i in &instances {
            let p = [f16::from_bits(i[0]).to_f32() + block.0, f16::from_bits(i[1]).to_f32() + block.1, f16::from_bits(i[2]).to_f32()];
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        let mut center = [0f32; 3];
        let mut half_extent = [0f32; 3];
        for k in 0..3 {
            let l = lo[k] - 30.0;
            let h = hi[k] + 30.0;
            center[k] = l + (h - l) * 0.5;
            half_extent[k] = h - center[k];
        }
        Group { center, half_extent, instances }
    }
}

pub struct TypeCache {
    /// Index into the caller's GRAS table (0 when parsed).
    pub gras: usize,
    /// Model path without the leading `meshes\`.
    pub model: String,
    pub wave_period: f32,
    pub form_id: u32,
    pub fits_to_slope: bool,
    pub uniform_scaling: bool,
    pub vertex_lighting: bool,
    pub groups: Vec<Group>,
}

impl TypeCache {
    pub fn new(g: &Gras) -> TypeCache {
        TypeCache {
            gras: 0,
            model: strip_meshes(&g.model).to_owned(),
            wave_period: g.wave_period,
            form_id: g.form_id,
            fits_to_slope: g.fits_to_slope(),
            uniform_scaling: g.uniform_scaling(),
            vertex_lighting: g.vertex_lighting(),
            groups: Vec::new(),
        }
    }
}

/// LoadGrassType builds `meshes\<model>` and the writer skips 7 characters.
pub fn strip_meshes(model: &str) -> &str {
    if model.len() >= 7 && model[..7].eq_ignore_ascii_case("meshes\\") { &model[7..] } else { model }
}

pub struct CellCache {
    pub types: Vec<TypeCache>,
}

impl CellCache {
    pub fn instance_count(&self) -> usize {
        self.types.iter().flat_map(|t| &t.groups).map(|g| g.instances.len()).sum()
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(64 + self.instance_count() * 32);
        b.extend((self.types.len() as u32).to_le_bytes());
        for t in &self.types {
            b.extend((t.model.len() as u32 + 1).to_le_bytes());
            b.extend(t.model.as_bytes());
            b.push(0);
            b.extend(t.wave_period.to_le_bytes());
            b.extend(t.form_id.to_le_bytes());
            b.extend([u8::from(t.fits_to_slope), u8::from(t.uniform_scaling), u8::from(t.vertex_lighting)]);
            b.extend((t.groups.len() as u32).to_le_bytes());
            for g in &t.groups {
                for v in g.center.iter().chain(&g.half_extent) {
                    b.extend(v.to_le_bytes());
                }
                // triCount: the loader recomputes it.
                b.extend(0u32.to_le_bytes());
                b.extend((g.instances.len() as u32).to_le_bytes());
                b.extend(16u32.to_le_bytes());
                for i in &g.instances {
                    for h in i {
                        b.extend(h.to_le_bytes());
                    }
                }
            }
        }
        b
    }

    /// Parses a `.cgid` (for comparisons). Groups keep the stored bounds.
    pub fn parse(d: &[u8]) -> Option<CellCache> {
        Self::parse_prefix(d).and_then(|(c, used)| (used == d.len()).then_some(c))
    }

    /// Like [`CellCache::parse`] but ignores trailing bytes, as the game's loader does (NGIO 1.6
    /// rewrites cache files in place without truncating them).
    pub fn parse_prefix(d: &[u8]) -> Option<(CellCache, usize)> {
        let mut o = 0usize;
        let u32_at = |o: &mut usize| -> Option<u32> {
            let v = u32::from_le_bytes(d.get(*o..*o + 4)?.try_into().ok()?);
            *o += 4;
            Some(v)
        };
        let n = u32_at(&mut o)?;
        let mut types = Vec::new();
        for _ in 0..n {
            let len = u32_at(&mut o)? as usize;
            let name = d.get(o..o + len)?;
            o += len;
            let model = String::from_utf8_lossy(&name[..len.saturating_sub(1)]).into_owned();
            let wave_period = f32::from_bits(u32_at(&mut o)?);
            let form_id = u32_at(&mut o)?;
            let f = d.get(o..o + 3)?;
            o += 3;
            let ng = u32_at(&mut o)?;
            let mut groups = Vec::new();
            for _ in 0..ng {
                let mut v = [0f32; 6];
                for x in &mut v {
                    *x = f32::from_bits(u32_at(&mut o)?);
                }
                let _tri = u32_at(&mut o)?;
                let count = u32_at(&mut o)? as usize;
                let shorts = u32_at(&mut o)? as usize;
                let mut instances = Vec::with_capacity(count);
                for _ in 0..count {
                    let mut i = [0u16; 16];
                    let raw = d.get(o..o + shorts * 2)?;
                    for (k, c) in raw.chunks_exact(2).take(16).enumerate() {
                        i[k] = u16::from_le_bytes([c[0], c[1]]);
                    }
                    o += shorts * 2;
                    instances.push(i);
                }
                groups.push(Group { center: [v[0], v[1], v[2]], half_extent: [v[3], v[4], v[5]], instances });
            }
            types.push(TypeCache { gras: 0, model, wave_period, form_id, fits_to_slope: f[0] != 0, uniform_scaling: f[1] != 0, vertex_lighting: f[2] != 0, groups });
        }
        Some((CellCache { types }, o))
    }
}
