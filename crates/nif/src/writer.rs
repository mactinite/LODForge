//! NIF serializer for Skyrim SE files (Gamebryo 20.2.0.7, user version 12, BS version 100).
//!
//! Written from the niftools NIF format specification (`nif.xml`, niftoolsxml 0.10.0.0, commit
//! `028cb35b73f59f07acc39e7df7a89ecfda6439a1`, 2023-06-17) and from the block layouts observed in
//! xLODGen / DynDOLOD output (docs/re/terrain-lod-spec.md, section 1). It encodes every block type
//! the reader decodes ([`crate::WRITABLE_TYPES`]); any other block must be present in
//! [`Nif::raw_blocks`] and is copied verbatim.
//!
//! Header (nif.xml `Header`, SSE fields only): header line + `\n`, version u32, endian u8, user
//! version u32, block count u32, BS version u32, three export strings (u8 length including the
//! NUL, then the bytes), block type count u16, block types (u32-sized strings), block type index
//! (u16 per block), block sizes (u32 per block), string count u32, max string length u32, strings
//! (u32-sized), group count u32 (+ groups). Footer (nif.xml `Footer`): root count u32 + root refs.
//!
//! Block type and string tables: an existing table in [`Nif::header`] is kept (so a parsed file
//! round-trips byte for byte, unused entries included); names and types it lacks are appended in
//! the order the blocks first use them, which is also the order observed in generated files when
//! the tables are built from nothing.
//!
//! Serialization is two-phase: [`encode`] turns the blocks into byte vectors and [`Encoded::to_bytes`]
//! assembles the file around them, so a family of files that differ in one block (the season
//! variants of a terrain quad only change the texture set) encodes the geometry once.

use crate::{
    AvObject, Block, EffectShader, Error, Header, LightingExtra, LightingShader, Nif, Node,
    ObjectNet, Ref, Result, Segment, TriShape, VertexDesc, f32_to_half, float_to_byte, va,
};

/// Vertex attributes the writer can encode (nif.xml `BSVertexDataSSE`): position (+ bitangent X /
/// unused W), UVs, normals (+ bitangent Y), tangents (+ bitangent Z), colours; `Full_Precision` is
/// a flag without a layout effect in SSE.
const ENCODABLE_ATTRIBUTES: u16 =
    va::VERTEX | va::UVS | va::NORMALS | va::TANGENTS | va::COLORS | va::FULL_PRECISION;

/// Little-endian byte buffer with the NIF primitive encodings.
#[derive(Default)]
struct Buf(Vec<u8>);

impl Buf {
    fn u8(&mut self, v: u8) {
        self.0.push(v);
    }
    fn u16(&mut self, v: u16) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u32(&mut self, v: u32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn i32(&mut self, v: i32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn u64(&mut self, v: u64) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn f32(&mut self, v: f32) {
        self.0.extend_from_slice(&v.to_le_bytes());
    }
    fn vec3(&mut self, v: [f32; 3]) {
        v.iter().for_each(|&c| self.f32(c));
    }
    fn reference(&mut self, r: Ref) {
        self.i32(r.map_or(-1, |i| i as i32));
    }
    fn refs(&mut self, rs: &[Ref]) -> Result<()> {
        self.u32(u32::try_from(rs.len()).map_err(|_| Error::Write("too many references".into()))?);
        rs.iter().for_each(|&r| self.reference(r));
        Ok(())
    }
    /// nif.xml `SizedString`: u32 length + bytes (Latin-1, no terminator).
    fn sized_string(&mut self, s: &str) -> Result<()> {
        let bytes = latin1(s)?;
        self.u32(bytes.len() as u32);
        self.0.extend_from_slice(&bytes);
        Ok(())
    }
    /// nif.xml `ExportString`: u8 length including the NUL terminator, then the bytes and the NUL.
    fn export_string(&mut self, s: &str) -> Result<()> {
        let bytes = latin1(s)?;
        if bytes.len() >= 255 {
            return Err(Error::Write(format!(
                "export string too long ({} bytes)",
                bytes.len()
            )));
        }
        self.u8(bytes.len() as u8 + 1);
        self.0.extend_from_slice(&bytes);
        self.u8(0);
        Ok(())
    }
}

/// Strings are stored as Latin-1 (the reader maps each byte to the char of the same code point).
fn latin1(s: &str) -> Result<Vec<u8>> {
    s.chars()
        .map(|c| {
            u8::try_from(u32::from(c))
                .map_err(|_| Error::Write(format!("string {s:?} is not Latin-1")))
        })
        .collect()
}

/// The tables and encoded blocks of one file, ready to be assembled.
#[derive(Debug, Clone)]
pub struct Encoded {
    pub block_types: Vec<String>,
    pub block_type_index: Vec<u16>,
    pub strings: Vec<String>,
    pub blocks: Vec<Vec<u8>>,
}

/// Lookup/append tables for block type names and object names, in first-use order.
struct Tables {
    block_types: Vec<String>,
    strings: Vec<String>,
}

impl Tables {
    fn string_index(&mut self, name: Option<&str>) -> i32 {
        let Some(name) = name else { return -1 };
        match self.strings.iter().position(|s| s == name) {
            Some(i) => i as i32,
            None => {
                self.strings.push(name.to_string());
                (self.strings.len() - 1) as i32
            }
        }
    }
    fn type_index(&mut self, name: &str) -> u16 {
        match self.block_types.iter().position(|s| s == name) {
            Some(i) => i as u16,
            None => {
                self.block_types.push(name.to_string());
                (self.block_types.len() - 1) as u16
            }
        }
    }
}

/// Encodes every block. The header's existing block type and string tables are the starting
/// point of the tables; the blocks keep their positions.
pub fn encode(nif: &Nif) -> Result<Encoded> {
    let h = &nif.header;
    let tables_match = h.block_type_index.len() == nif.blocks.len()
        && h.block_type_index
            .iter()
            .all(|&t| (t as usize) < h.block_types.len());
    let mut tables = Tables {
        block_types: if tables_match {
            h.block_types.clone()
        } else {
            Vec::new()
        },
        strings: h.strings.clone(),
    };
    let mut blocks = Vec::with_capacity(nif.blocks.len());
    let mut type_index = Vec::with_capacity(nif.blocks.len());
    for (i, block) in nif.blocks.iter().enumerate() {
        let type_name: String = match h
            .block_type_index
            .get(i)
            .and_then(|&t| h.block_types.get(t as usize))
        {
            Some(t) if tables_match => t.clone(),
            _ => block
                .default_type_name()
                .ok_or_else(|| {
                    Error::Write(format!("block {i} has no type name and no raw bytes"))
                })?
                .to_string(),
        };
        type_index.push(tables.type_index(&type_name));
        let bytes = match nif.raw_blocks.get(&i) {
            Some(raw) => raw.clone(),
            None => encode_block(block, &type_name, &mut tables)
                .map_err(|e| Error::Write(format!("block {i} ({type_name}): {e}")))?,
        };
        blocks.push(bytes);
    }
    Ok(Encoded {
        block_types: tables.block_types,
        block_type_index: type_index,
        strings: tables.strings,
        blocks,
    })
}

/// Serializes a whole file: [`encode`] + [`Encoded::to_bytes`].
pub fn write(nif: &Nif) -> Result<Vec<u8>> {
    encode(nif)?.to_bytes(&nif.header, &nif.roots)
}

impl Encoded {
    /// Re-encodes block `index` of `nif` (after a change that touches no object name, such as new
    /// texture paths) and stores it in place.
    pub fn replace_block(&mut self, nif: &Nif, index: usize) -> Result<()> {
        let type_name = self
            .block_types
            .get(self.block_type_index[index] as usize)
            .cloned()
            .ok_or_else(|| Error::Write("block type index out of range".into()))?;
        let mut tables = Tables {
            block_types: self.block_types.clone(),
            strings: self.strings.clone(),
        };
        let bytes = match nif.raw_blocks.get(&index) {
            Some(raw) => raw.clone(),
            None => encode_block(&nif.blocks[index], &type_name, &mut tables)?,
        };
        if tables.strings.len() != self.strings.len() {
            return Err(Error::Write(
                "replace_block: the block introduces a new name".into(),
            ));
        }
        self.blocks[index] = bytes;
        Ok(())
    }

    /// Header + blocks + footer. `header` supplies the version fields and export strings; the
    /// tables come from the encoding.
    pub fn to_bytes(&self, header: &Header, roots: &[Ref]) -> Result<Vec<u8>> {
        if header.bs_version != 100 {
            return Err(Error::Write(format!(
                "BS version {} is not supported for writing",
                header.bs_version
            )));
        }
        let total: usize = self.blocks.iter().map(Vec::len).sum();
        let mut b = Buf(Vec::with_capacity(total + 512));
        b.0.extend_from_slice(&latin1(&header.header_line)?);
        b.u8(b'\n');
        b.u32(header.version);
        b.u8(header.endian);
        b.u32(header.user_version);
        b.u32(self.blocks.len() as u32);
        b.u32(header.bs_version);
        b.export_string(&header.author)?;
        b.export_string(&header.process_script)?;
        b.export_string(&header.export_script)?;
        b.u16(
            u16::try_from(self.block_types.len())
                .map_err(|_| Error::Write("too many block types".into()))?,
        );
        for t in &self.block_types {
            b.sized_string(t)?;
        }
        for &t in &self.block_type_index {
            b.u16(t);
        }
        for block in &self.blocks {
            b.u32(u32::try_from(block.len()).map_err(|_| Error::Write("block too large".into()))?);
        }
        b.u32(self.strings.len() as u32);
        b.u32(
            self.strings
                .iter()
                .map(|s| s.chars().count() as u32)
                .max()
                .unwrap_or(0),
        );
        for s in &self.strings {
            b.sized_string(s)?;
        }
        b.u32(header.groups.len() as u32);
        for &g in &header.groups {
            b.u32(g);
        }
        for block in &self.blocks {
            b.0.extend_from_slice(block);
        }
        b.refs(roots)?;
        Ok(b.0)
    }
}

fn object_net(b: &mut Buf, net: &ObjectNet, t: &mut Tables) -> Result<()> {
    b.i32(t.string_index(net.name.as_deref()));
    b.refs(&net.extra_data)?;
    b.reference(net.controller);
    Ok(())
}

fn av_object(b: &mut Buf, av: &AvObject, t: &mut Tables) -> Result<()> {
    object_net(b, &av.net, t)?;
    b.u32(av.flags);
    b.vec3(av.transform.translation);
    for row in &av.transform.rotation {
        b.vec3(*row);
    }
    b.f32(av.transform.scale);
    b.reference(av.collision);
    Ok(())
}

fn node(b: &mut Buf, n: &Node, type_name: &str, t: &mut Tables) -> Result<()> {
    av_object(b, &n.av, t)?;
    b.refs(&n.children)?;
    b.refs(&n.effects)?;
    if type_name == "BSMultiBoundNode" {
        b.reference(n.multi_bound);
        b.u32(
            n.culling_mode
                .ok_or_else(|| Error::Write("BSMultiBoundNode without a culling mode".into()))?,
        );
    }
    Ok(())
}

fn tri_shape(b: &mut Buf, s: &TriShape, type_name: &str, t: &mut Tables) -> Result<()> {
    if s.legacy {
        return Err(Error::Write(
            "Skyrim LE geometry cannot be written as a BSTriShape".into(),
        ));
    }
    av_object(b, &s.av, t)?;
    b.vec3(s.bound_center);
    b.f32(s.bound_radius);
    b.reference(s.skin);
    b.reference(s.shader_property);
    b.reference(s.alpha_property);
    let desc = VertexDesc(s.vertex_desc);
    let attrs = desc.attributes();
    if attrs & !ENCODABLE_ATTRIBUTES != 0 {
        return Err(Error::Write(format!(
            "vertex attributes {attrs:#05x} cannot be encoded"
        )));
    }
    let layout = desc.layout();
    let stride = desc.stride();
    if layout.size != stride {
        return Err(Error::VertexLayout {
            desc: s.vertex_desc,
            computed: layout.size,
            stride,
        });
    }
    let nv = s.positions.len();
    for (flag, len, what) in [
        (va::VERTEX, nv, "positions"),
        (va::UVS, s.uvs.len(), "uvs"),
        (va::NORMALS, s.normals.len(), "normals"),
        (va::TANGENTS, s.tangents.len(), "tangents"),
        (va::COLORS, s.colors.len(), "colors"),
    ] {
        let present = attrs & flag != 0;
        if present != (len > 0) && nv > 0 {
            return Err(Error::Write(format!(
                "{what}: {len} entries but the vertex desc flag is {}",
                if present { "set" } else { "clear" }
            )));
        }
        if present && len != nv {
            return Err(Error::Write(format!(
                "{what}: {len} entries for {nv} vertices"
            )));
        }
    }
    if !s.bitangents.is_empty() && s.bitangents.len() != nv {
        return Err(Error::Write(format!(
            "bitangents: {} entries for {nv} vertices",
            s.bitangents.len()
        )));
    }
    if nv > usize::from(u16::MAX) || s.triangles.len() > usize::from(u16::MAX) {
        return Err(Error::Write(format!(
            "{nv} vertices / {} triangles exceed the u16 limits",
            s.triangles.len()
        )));
    }
    if s.triangles.iter().flatten().any(|&i| usize::from(i) >= nv) {
        return Err(Error::Write("triangle index out of range".into()));
    }
    b.u64(s.vertex_desc);
    b.u16(s.triangles.len() as u16);
    b.u16(nv as u16);
    let data_size = nv * stride + s.triangles.len() * 6;
    b.u32(data_size as u32);
    if data_size > 0 {
        for i in 0..nv {
            let bt = s.bitangents.get(i).copied().unwrap_or([0.0; 3]);
            if layout.position.is_some() {
                b.vec3(s.positions[i]);
                // nif.xml: "Bitangent X" with tangents, "Unused W" otherwise (kept as read).
                b.f32(bt[0]);
            }
            if layout.uv.is_some() {
                b.u16(f32_to_half(s.uvs[i][0]));
                b.u16(f32_to_half(s.uvs[i][1]));
            }
            if layout.normal.is_some() {
                let n = s.normals[i];
                b.0.extend_from_slice(&[
                    float_to_byte(n[0]),
                    float_to_byte(n[1]),
                    float_to_byte(n[2]),
                    float_to_byte(bt[1]),
                ]);
            }
            if layout.tangent.is_some() {
                let n = s.tangents[i];
                b.0.extend_from_slice(&[
                    float_to_byte(n[0]),
                    float_to_byte(n[1]),
                    float_to_byte(n[2]),
                    float_to_byte(bt[2]),
                ]);
            }
            if layout.color.is_some() {
                b.0.extend_from_slice(&s.colors[i]);
            }
        }
        for tri in &s.triangles {
            tri.iter().for_each(|&i| b.u16(i));
        }
    }
    // Particle data (nif.xml, BS_SSE): none in generated files.
    b.u32(0);
    match type_name {
        "BSSubIndexTriShape" => {
            let segments: &[Segment] = s
                .segments
                .as_deref()
                .ok_or_else(|| Error::Write("BSSubIndexTriShape without segments".into()))?;
            b.u32(segments.len() as u32);
            for seg in segments {
                b.u8(seg.flags);
                b.u32(seg.start);
                b.u32(seg.count);
            }
        }
        "BSMeshLODTriShape" => {
            let sizes = s
                .lod_sizes
                .ok_or_else(|| Error::Write("BSMeshLODTriShape without LOD sizes".into()))?;
            sizes.iter().for_each(|&v| b.u32(v));
        }
        "BSTriShape" => {
            if s.segments.is_some() || s.lod_sizes.is_some() {
                return Err(Error::Write("BSTriShape with segments or LOD sizes".into()));
            }
        }
        other => {
            return Err(Error::Write(format!(
                "shape type {other} cannot be written"
            )));
        }
    }
    Ok(())
}

fn lighting_shader(b: &mut Buf, s: &LightingShader, t: &mut Tables) -> Result<()> {
    b.u32(s.shader_type);
    object_net(b, &s.net, t)?;
    b.u32(s.flags1);
    b.u32(s.flags2);
    b.f32(s.uv_offset[0]);
    b.f32(s.uv_offset[1]);
    b.f32(s.uv_scale[0]);
    b.f32(s.uv_scale[1]);
    b.reference(s.texture_set);
    b.vec3(s.emissive_color);
    b.f32(s.emissive_multiple);
    b.u32(s.texture_clamp_mode);
    b.f32(s.alpha);
    b.f32(s.refraction_strength);
    b.f32(s.glossiness);
    b.vec3(s.specular_color);
    b.f32(s.specular_strength);
    b.f32(s.lighting_effect_1);
    b.f32(s.lighting_effect_2);
    // Type-specific tail (nif.xml conditions on Shader Type).
    let needs_tail = matches!(s.shader_type, 1 | 5 | 6 | 7 | 11 | 14 | 16);
    match (&s.extra, s.extra.shader_type()) {
        (LightingExtra::None, _) if needs_tail => {
            return Err(Error::Write(format!(
                "shader type {} needs its type-specific fields",
                s.shader_type
            )));
        }
        (LightingExtra::None, _) => {}
        (_, Some(ty)) if ty != s.shader_type => {
            return Err(Error::Write(format!(
                "shader type {} with the fields of type {ty}",
                s.shader_type
            )));
        }
        _ => {}
    }
    match &s.extra {
        LightingExtra::None => {}
        LightingExtra::EnvironmentMap { scale } => b.f32(*scale),
        LightingExtra::SkinTint(c) | LightingExtra::HairTint(c) => b.vec3(*c),
        LightingExtra::ParallaxOcclusion { max_passes, scale } => {
            b.f32(*max_passes);
            b.f32(*scale);
        }
        LightingExtra::MultiLayerParallax {
            inner_layer_thickness,
            refraction_scale,
            inner_layer_texture_scale,
            envmap_strength,
        } => {
            b.f32(*inner_layer_thickness);
            b.f32(*refraction_scale);
            b.f32(inner_layer_texture_scale[0]);
            b.f32(inner_layer_texture_scale[1]);
            b.f32(*envmap_strength);
        }
        LightingExtra::SparkleSnow(v) => v.iter().for_each(|&c| b.f32(c)),
        LightingExtra::EyeEnvironment {
            cubemap_scale,
            left,
            right,
        } => {
            b.f32(*cubemap_scale);
            b.vec3(*left);
            b.vec3(*right);
        }
    }
    Ok(())
}

fn effect_shader(b: &mut Buf, s: &EffectShader, t: &mut Tables) -> Result<()> {
    object_net(b, &s.net, t)?;
    b.u32(s.flags1);
    b.u32(s.flags2);
    b.f32(s.uv_offset[0]);
    b.f32(s.uv_offset[1]);
    b.f32(s.uv_scale[0]);
    b.f32(s.uv_scale[1]);
    b.sized_string(&s.source_texture)?;
    b.0.extend_from_slice(&[
        s.texture_clamp_mode,
        s.lighting_influence,
        s.env_map_min_lod,
        s.unknown,
    ]);
    s.falloff.iter().for_each(|&v| b.f32(v));
    s.emissive_color.iter().for_each(|&v| b.f32(v));
    b.f32(s.emissive_multiple);
    b.f32(s.soft_falloff_depth);
    b.sized_string(&s.greyscale_texture)?;
    Ok(())
}

fn encode_block(block: &Block, type_name: &str, t: &mut Tables) -> Result<Vec<u8>> {
    let mut b = Buf::default();
    match block {
        Block::Node(n) => match type_name {
            "NiNode" | "BSFadeNode" | "BSLeafAnimNode" | "BSMultiBoundNode" => {
                node(&mut b, n, type_name, t)?
            }
            other => {
                return Err(Error::Write(format!(
                    "node type {other} cannot be written without its raw bytes"
                )));
            }
        },
        Block::TriShape(s) => tri_shape(&mut b, s, type_name, t)?,
        Block::LightingShader(s) => lighting_shader(&mut b, s, t)?,
        Block::EffectShader(s) => effect_shader(&mut b, s, t)?,
        Block::TextureSet(paths) => {
            b.u32(paths.len() as u32);
            for p in paths {
                b.sized_string(p)?;
            }
        }
        Block::Alpha {
            net,
            flags,
            threshold,
        } => {
            object_net(&mut b, net, t)?;
            b.u16(*flags);
            b.u8(*threshold);
        }
        Block::MultiBound { data } => b.reference(*data),
        Block::MultiBoundAabb { position, extent } => {
            b.vec3(*position);
            b.vec3(*extent);
        }
        Block::LargeRefExtra { name, large_ref } => {
            b.i32(t.string_index(name.as_deref()));
            b.u8(*large_ref);
        }
        Block::Unknown => return Err(Error::Write("unknown block without raw bytes".into())),
    }
    Ok(b.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Transform, parse};

    #[test]
    fn export_strings_carry_their_nul() {
        let mut b = Buf::default();
        b.export_string("").unwrap();
        b.export_string("ab").unwrap();
        assert_eq!(b.0, [1, 0, 3, b'a', b'b', 0]);
    }

    #[test]
    fn tables_are_first_use_ordered() {
        let mut nif = Nif::new();
        nif.add_block(Block::Node(Node {
            av: AvObject {
                net: ObjectNet::named("second"),
                ..Default::default()
            },
            children: vec![Some(1)],
            ..Default::default()
        }));
        nif.add_block(Block::Node(Node {
            av: AvObject {
                net: ObjectNet::named("first"),
                transform: Transform {
                    scale: 2.0,
                    ..Default::default()
                },
                ..Default::default()
            },
            culling_mode: Some(1),
            ..Default::default()
        }));
        nif.add_block(Block::MultiBoundAabb {
            position: [1.0, 2.0, 3.0],
            extent: [4.0, 5.0, 6.0],
        });
        let bytes = write(&nif).unwrap();
        let back = parse(&bytes).unwrap();
        assert_eq!(
            back.header.block_types,
            ["NiNode", "BSMultiBoundNode", "BSMultiBoundAABB"]
        );
        assert_eq!(back.header.strings, ["second", "first"]);
        assert_eq!(back.header.block_type_index, [0, 1, 2]);
        assert_eq!(back.header.block_sizes.len(), 3);
        assert_eq!(back.blocks, nif.blocks);
        assert_eq!(back.roots, nif.roots);
        assert!(back.raw_blocks.is_empty());
        assert_eq!(write(&back).unwrap(), bytes);
    }

    #[test]
    fn replace_block_keeps_the_rest() {
        let mut nif = Nif::new();
        nif.add_block(Block::TextureSet(vec!["a.dds".into(), "a_n.dds".into()]));
        nif.add_block(Block::MultiBound { data: None });
        let mut enc = encode(&nif).unwrap();
        let before = enc.to_bytes(&nif.header, &nif.roots).unwrap();
        nif.blocks[0] =
            Block::TextureSet(vec!["longer\\name.dds".into(), "longer\\name_n.dds".into()]);
        enc.replace_block(&nif, 0).unwrap();
        let after = enc.to_bytes(&nif.header, &nif.roots).unwrap();
        assert_ne!(before, after);
        assert_eq!(after, write(&nif).unwrap());
        let back = parse(&after).unwrap();
        assert_eq!(back.blocks, nif.blocks);
    }
}
