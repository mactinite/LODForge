//! Shape naming, grouping key and shader values (docs/re/object-lod-spec.md 3.3, 3.4).
//!
//! Every ordinary shape gets the standard LOD shader seen in the golden files: type 0, flags1
//! `ZBuffer_Test | Receive_Shadows | Cast_Shadows`, flags2 `ZBuffer_Write | LOD_Objects`
//! (`HD_LOD_Objects` instead on snow / ash HD shapes) plus `Vertex_Colors` when the shape has
//! colours and `Double_Sided` when the source had it, glossiness 1, white specular, no emissive,
//! and a texture set of the diffuse, the normal map and (for PBR sets) the slot-5 map. Passthru
//! shapes keep their source shader block. The flag reduction is the one AnvilLOD's `ShaderGuard`
//! (Copyright (c) 2026 CageTV, GPL-3.0-only, commit 606eece) applies (`0x80000300 | keep`,
//! `0x5 | keep`), narrowed to the bits the golden files show (only `Double_Sided` is kept).
//!
//! Vertex colours (open question 4, settled on the DynDOLOD golden2 at LOD 4): colour presence
//! is not part of the grouping key; the VC side file whitens the LOD shapes it lists `False`
//! (RGB 255, alpha kept); a merged plain shape is coloured when some part has non-white RGB
//! after that, a snow / ash shape when some part has a colour array at all; the parts without
//! an array get opaque white vertices, and an uncoloured shape drops every array (spec 12.2).
//!
//! HD classification (open question 5 of the spec, settled on the golden statistics of
//! 2026-10-20, 2 of ~700 xLODGen LOD4 shapes unexplained): a shape is HD when its diffuse is an
//! HD texture (xLODGen: `TextureDiffuseHD`; DynDOLOD: an `IsHDTextureMask` substring) and the
//! object is snow / ash, or a large reference at LOD4; snow / ash HD shapes carry the
//! `HD_LOD_Objects` flag, plain `objHD` shapes keep `LOD_Objects`.

use lodforge_nif::{EffectShader, LightingShader, LightingExtra};

use crate::atlas_map::AtlasRow;
use crate::mesh::{MeshPart, ShaderSource};

pub const SLSF1_STANDARD: u32 = 0x8000_0300;
pub const SLSF1_SPECULAR: u32 = 0x1;
pub const SLSF1_RECEIVE_SHADOWS: u32 = 0x100;
pub const SLSF1_CAST_SHADOWS: u32 = 0x200;
pub const SLSF1_EXTERNAL_EMITTANCE: u32 = 0x2000_0000;
pub const SLSF2_ENVMAP_LIGHT_FADE: u32 = 0x8000;
pub const SLSF2_ZBUFFER_WRITE: u32 = 0x1;
pub const SLSF2_LOD_OBJECTS: u32 = 0x4;
pub const SLSF2_DOUBLE_SIDED: u32 = 0x10;
pub const SLSF2_VERTEX_COLORS: u32 = 0x20;
pub const SLSF2_HD_LOD_OBJECTS: u32 = 0x8000_0000;
pub const SLSF2_GLOW_MAP: u32 = 0x40;

/// The flat normal map used when a source has none.
pub const DEFAULT_NORMAL: &str = "textures\\default_n.dds";

/// The material column of a reference, classified (spec 1.1.3 column 12).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MaterialClass {
    Plain,
    Snow,
    Ash,
    PassThru,
    /// Any other MATO EditorID, lower case.
    Other(String),
}

impl MaterialClass {
    pub fn parse(material: &str) -> MaterialClass {
        let m = material.trim();
        if m.is_empty() {
            MaterialClass::Plain
        } else if m.eq_ignore_ascii_case("snow") {
            MaterialClass::Snow
        } else if m.eq_ignore_ascii_case("ash") {
            MaterialClass::Ash
        } else if m.eq_ignore_ascii_case("passthru") {
            MaterialClass::PassThru
        } else {
            MaterialClass::Other(m.to_ascii_lowercase())
        }
    }

    /// The name part after `obj` / `objpassthru`.
    pub fn suffix(&self) -> &str {
        match self {
            MaterialClass::Plain | MaterialClass::PassThru => "",
            MaterialClass::Snow => "Snow",
            MaterialClass::Ash => "Ash",
            MaterialClass::Other(s) => s,
        }
    }

    pub fn is_snow_or_ash(&self) -> bool {
        matches!(self, MaterialClass::Snow | MaterialClass::Ash)
    }
}

/// How HD textures are recognised.
#[derive(Debug, Clone, Default)]
pub struct HdRules {
    /// `UseHDFlag` / "Use HD Flag": when false nothing is HD.
    pub enabled: bool,
    /// xLODGen: the worldspace's `TextureDiffuseHD` (exact, lower-case `textures\...`).
    pub exact: Vec<String>,
    /// DynDOLOD `IsHDTextureMask` entries (lower-case substrings of the texture path).
    pub is_hd: Vec<String>,
    /// DynDOLOD `NotHDTextureMask` entries (recorded; not used by the settled rule).
    pub not_hd: Vec<String>,
    /// DynDOLOD `IsHDMeshMask` entries (lower-case substrings of the LOD model path): with
    /// masks, only models matching one can be HD (`_lod_0` in the golden run); without masks
    /// (xLODGen) the snow / ash or LOD4-large-reference rule applies.
    pub mesh_masks: Vec<String>,
}

impl HdRules {
    /// Whether a diffuse path (as stored in the model, or an AltTextures replacement) is an HD
    /// texture; a PBR `<stem>linear.dds` counts as its `<stem>.dds`.
    pub fn is_hd_texture(&self, diffuse: &str) -> bool {
        if !self.enabled || diffuse.is_empty() {
            return false;
        }
        let key = crate::export::texture_key(diffuse);
        let key = match key.strip_suffix("linear.dds") {
            Some(stem) => format!("{stem}.dds"),
            None => key,
        };
        self.exact.iter().any(|t| *t == key) || self.is_hd.iter().any(|m| !m.is_empty() && key.contains(m))
    }
}

/// What decides a shape's name and shader besides the geometry.
#[derive(Debug, Clone)]
pub struct Context<'a> {
    pub class: MaterialClass,
    pub large_ref: bool,
    pub level: i32,
    pub passthru: bool,
    pub hd: &'a HdRules,
    /// The LOD model path matches an `IsHDMeshMask` entry (DynDOLOD).
    pub model_hd: bool,
}

/// The shape name of spec 3.3.
pub fn shape_name(class: &MaterialClass, hd: bool, passthru: bool, large_ref: bool) -> String {
    let mut n = String::from(if passthru { "objpassthru" } else { "obj" });
    n.push_str(class.suffix());
    if hd {
        n.push_str("HD");
    }
    if large_ref {
        n.push_str("-LargeRef");
    }
    n
}

/// Whether a shape with these (alt-texture applied) textures is HD (module doc). An HD shape
/// keeps its full texture: the DynDOLOD golden2 has no HD shape on an atlas page.
pub fn is_hd(ctx: &Context<'_>, textures: &[String]) -> bool {
    let large_ref = ctx.large_ref && ctx.level == 4;
    !ctx.passthru && ctx.hd.is_hd_texture(&textures[0]) && if ctx.hd.mesh_masks.is_empty() { ctx.class.is_snow_or_ash() || large_ref } else { ctx.model_hd }
}

/// A written texture path: lower case, `Data\` + `textures\...`; empty stays empty.
pub fn written_texture(path: &str) -> String {
    let k = crate::export::texture_key(path);
    if k.is_empty() { String::new() } else { format!("Data\\{k}") }
}

/// The grouping key and shader of one output shape.
#[derive(Debug, Clone, PartialEq)]
pub struct Material {
    pub name: String,
    /// Nine slots as written.
    pub textures: Vec<String>,
    pub lighting: Option<LightingShader>,
    pub effect: Option<EffectShader>,
    pub colors: bool,
    pub large_ref: bool,
    pub hd: bool,
    pub passthru: bool,
    /// Sort / grouping key: name, textures, shader values, colour presence.
    pub key: String,
}

impl Material {
    /// The material of a source shape placed by a reference. `textures` are the shape's slots
    /// after any AltTextures substitution (as stored in the model / side file); `atlas` is the
    /// atlas-map row the shape is remapped into, if any: then the written textures are the row's
    /// page, its `_n` and (for a glow row) its `_g`, with clamp mode 0 (spec 5).
    pub fn for_part(part: &MeshPart, textures: &[String], ctx: &Context<'_>, atlas: Option<&AtlasRow>) -> Material {
        // Large references matter in the LOD4 grid only: no `-LargeRef` shapes above LOD4 (3.3).
        let large_ref = ctx.large_ref && ctx.level == 4;
        let hd = is_hd(ctx, textures);
        let name = shape_name(&ctx.class, hd, ctx.passthru, large_ref);
        let page_slots = |row: &AtlasRow| -> Vec<String> {
            let page = crate::export::texture_key(&row.page);
            let stem = page.trim_end_matches(".dds").to_string();
            let mut w = vec![String::new(); 9];
            w[0] = format!("Data\\{page}");
            w[1] = format!("Data\\{stem}_n.dds");
            if row.key.split(',').count() >= 3 {
                w[2] = format!("Data\\{stem}_g.dds");
            }
            w
        };
        let (lighting, effect, written) = if ctx.passthru {
            match &part.shader {
                ShaderSource::Lighting(l) => {
                    let mut l = l.clone();
                    l.texture_set = None;
                    l.uv_offset = [0.0; 2];
                    l.uv_scale = [1.0; 2];
                    // A default-type passthru shader loses what LOD never renders (DynDOLOD
                    // golden2, 91 of 109 LOD4 passthru lighting shapes: source `0xae400301 /
                    // 0x8069` written as `0x8c400000 / 0x69`): Specular, Receive_Shadows,
                    // Cast_Shadows, External_Emittance and EnvMap_Light_Fade; the glow shader
                    // (type 2) likewise. The environment-map type (the water tiles, type 1)
                    // keeps every bit.
                    if l.shader_type != 1 {
                        l.flags1 &= !(SLSF1_SPECULAR | SLSF1_RECEIVE_SHADOWS | SLSF1_CAST_SHADOWS | SLSF1_EXTERNAL_EMITTANCE);
                        l.flags2 &= !SLSF2_ENVMAP_LIGHT_FADE;
                    }
                    let w = match atlas {
                        Some(row) => {
                            l.texture_clamp_mode = 0;
                            page_slots(row)
                        }
                        None => textures.iter().map(|t| written_texture(t)).collect::<Vec<_>>(),
                    };
                    (Some(l), None, w)
                }
                ShaderSource::Effect(e) => {
                    let mut e = e.clone();
                    e.source_texture = written_texture(&e.source_texture);
                    e.uv_offset = [0.0; 2];
                    e.uv_scale = [1.0; 2];
                    (None, Some(e), Vec::new())
                }
            }
        } else {
            let double_sided = match &part.shader {
                ShaderSource::Lighting(l) => l.flags2 & SLSF2_DOUBLE_SIDED != 0,
                ShaderSource::Effect(e) => e.flags2 & SLSF2_DOUBLE_SIDED != 0,
            };
            let mut flags2 = SLSF2_ZBUFFER_WRITE | if hd && ctx.class.is_snow_or_ash() { SLSF2_HD_LOD_OBJECTS } else { SLSF2_LOD_OBJECTS };
            if double_sided {
                flags2 |= SLSF2_DOUBLE_SIDED;
            }
            let l = LightingShader {
                shader_type: 0,
                flags1: SLSF1_STANDARD,
                flags2,
                uv_offset: [0.0; 2],
                uv_scale: [1.0; 2],
                texture_set: None,
                emissive_color: [0.0; 3],
                emissive_multiple: 1.0,
                texture_clamp_mode: if atlas.is_some() { 0 } else { 3 },
                alpha: 1.0,
                refraction_strength: 0.0,
                glossiness: 1.0,
                specular_color: [1.0; 3],
                specular_strength: 1.0,
                lighting_effect_1: 0.0,
                lighting_effect_2: 0.0,
                extra: LightingExtra::None,
                net: Default::default(),
            };
            let w = match atlas {
                Some(row) => page_slots(row),
                None => {
                    let mut w = vec![String::new(); 9];
                    w[0] = written_texture(&textures[0]);
                    w[1] = if textures[1].trim().is_empty() { written_texture(DEFAULT_NORMAL) } else { written_texture(&textures[1]) };
                    if textures.len() > 5 && !textures[5].trim().is_empty() {
                        w[5] = written_texture(&textures[5]);
                    }
                    w
                }
            };
            (Some(l), None, w)
        };
        let mut m = Material { name, textures: written, lighting, effect, colors: false, large_ref, hd, passthru: ctx.passthru, key: String::new() };
        m.key = m.make_key();
        m
    }

    /// A material with fixed shader values and written textures (tree billboards, grass LOD),
    /// never HD, never a large reference; the `Vertex_Colors` flag follows the colour decision.
    pub fn fixed(name: String, textures: Vec<String>, lighting: LightingShader) -> Material {
        let mut m = Material { name, textures, lighting: Some(lighting), effect: None, colors: false, large_ref: false, hd: false, passthru: false, key: String::new() };
        m.key = m.make_key();
        m
    }

    /// Records whether the merged shape carries vertex colours; ordinary shapes get the
    /// `Vertex_Colors` flag accordingly, passthru shapes keep their source flags.
    pub fn set_colors(&mut self, has: bool) {
        self.colors = has;
        if !self.passthru
            && let Some(l) = &mut self.lighting
        {
            if has {
                l.flags2 |= SLSF2_VERTEX_COLORS;
            } else {
                l.flags2 &= !SLSF2_VERTEX_COLORS;
            }
        }
    }

    fn make_key(&self) -> String {
        let mut k = format!("{}|{}", self.name, self.textures.join(","));
        if let Some(l) = &self.lighting {
            k.push_str(&format!("|L{}|{:#x}|{:#x}|{}|{:?}|{:?}|{}|{}|{}|{:?}|{}|{}|{}|{:?}", l.shader_type, l.flags1, l.flags2, l.texture_clamp_mode, l.uv_offset, l.uv_scale, l.alpha, l.refraction_strength, l.glossiness, l.specular_color, l.specular_strength, l.lighting_effect_1, l.lighting_effect_2, l.emissive_color));
            k.push_str(&format!("|{}|{:?}", l.emissive_multiple, l.extra));
        }
        if let Some(e) = &self.effect {
            k.push_str(&format!("|E{:#x}|{:#x}|{}|{}|{:?}|{:?}|{:?}|{}|{}|{}", e.flags1, e.flags2, e.source_texture, e.greyscale_texture, e.uv_offset, e.uv_scale, e.falloff, e.emissive_multiple, e.soft_falloff_depth, e.texture_clamp_mode));
        }
        k
    }
}

/// Whether a LOD model path is a passthru model (`PassThruMeshMask` substrings).
pub fn is_passthru_model(model: &str, masks: &[String]) -> bool {
    let m = model.to_ascii_lowercase();
    masks.iter().any(|k| !k.is_empty() && m.contains(k))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(textures: [&str; 2], colors: bool, double_sided: bool) -> MeshPart {
        let mut t: [String; 9] = Default::default();
        t[0] = textures[0].into();
        t[1] = textures[1].into();
        MeshPart {
            name: "S".into(),
            index: 0,
            textures: t,
            shader: ShaderSource::Lighting(LightingShader { flags2: if double_sided { 0x11 } else { 0x1 }, ..Default::default() }),
            positions: vec![[0.0; 3]],
            uvs: vec![[0.0; 2]],
            normals: vec![[0.0, 0.0, 1.0]],
            tangents: vec![[1.0, 0.0, 0.0]],
            bitangents: vec![[0.0, 1.0, 0.0]],
            colors: colors.then(|| vec![[255; 4]]),
            triangles: vec![[0, 0, 0]],
            source_normals: true,
            source_tangents: true,
            legacy: false,
            alpha: None,
        }
    }

    #[test]
    fn names_flags_and_textures() {
        let hd = HdRules { enabled: true, exact: vec!["textures\\landscape\\mountains\\mountainslab02.dds".into()], is_hd: vec![], not_hd: vec![], mesh_masks: vec![] };
        let slab = "Textures\\Landscape\\Mountains\\MountainSlab02.dds";
        let ctx = |class: MaterialClass, large: bool, level: i32, passthru: bool| Context { class, large_ref: large, level, passthru, hd: &hd, model_hd: false };
        let p = part([slab, ""], true, false);
        let mut m = Material::for_part(&p, &p.textures, &ctx(MaterialClass::Snow, true, 4, false), None);
        assert_eq!(m.name, "objSnowHD-LargeRef");
        assert_eq!(m.lighting.as_ref().unwrap().flags2, 0x8000_0001);
        m.set_colors(true);
        assert_eq!(m.lighting.as_ref().unwrap().flags2, 0x8000_0021);
        assert!(m.colors);
        m.set_colors(false);
        assert_eq!(m.lighting.as_ref().unwrap().flags2, 0x8000_0001);
        assert_eq!(m.textures[0], "Data\\textures\\landscape\\mountains\\mountainslab02.dds");
        assert_eq!(m.textures[1], "Data\\textures\\default_n.dds");
        let m = Material::for_part(&p, &p.textures, &ctx(MaterialClass::Plain, true, 4, false), None);
        assert_eq!(m.name, "objHD-LargeRef");
        assert_eq!(m.lighting.as_ref().unwrap().flags2, 0x5);
        let m = Material::for_part(&p, &p.textures, &ctx(MaterialClass::Plain, false, 4, false), None);
        assert_eq!(m.name, "obj");
        let m = Material::for_part(&p, &p.textures, &ctx(MaterialClass::Plain, true, 8, false), None);
        assert_eq!(m.name, "obj");
        let m = Material::for_part(&p, &p.textures, &ctx(MaterialClass::Snow, false, 8, false), None);
        assert_eq!(m.name, "objSnowHD");
        assert_eq!(m.lighting.as_ref().unwrap().flags2, 0x8000_0001);
        let q = part(["textures\\dungeons\\ridgedstone05.dds", "textures\\dungeons\\ridgedstone04_n.dds"], false, true);
        let m = Material::for_part(&q, &q.textures, &ctx(MaterialClass::Other("volcanicsulfurcrystalslite".into()), false, 16, false), None);
        assert_eq!(m.name, "objvolcanicsulfurcrystalslite");
        let l = m.lighting.as_ref().unwrap();
        assert_eq!((l.flags1, l.flags2, l.texture_clamp_mode, l.glossiness), (0x8000_0300, 0x15, 3, 1.0));
        assert_eq!(m.textures[1], "Data\\textures\\dungeons\\ridgedstone04_n.dds");
        let m = Material::for_part(&q, &q.textures, &ctx(MaterialClass::Snow, true, 4, true), None);
        assert_eq!(m.name, "objpassthruSnow-LargeRef");
        assert_eq!(m.lighting.as_ref().unwrap().flags2, 0x11);
        let row = AtlasRow { key: "textures\\dungeons\\ridgedstone05.dds".into(), width: 512, height: 512, x: 0, y: 0, page: "textures\\terrain\\Tamriel\\Objects\\TamrielObjectsLOD01.dds".into(), page_width: 4096, page_height: 2048, scale: None };
        let m = Material::for_part(&q, &q.textures, &ctx(MaterialClass::Plain, false, 4, false), Some(&row));
        assert_eq!(m.textures[0], "Data\\textures\\terrain\\tamriel\\objects\\tamrielobjectslod01.dds");
        assert_eq!(m.textures[1], "Data\\textures\\terrain\\tamriel\\objects\\tamrielobjectslod01_n.dds");
        assert_eq!(m.textures[2], "");
        assert_eq!(m.lighting.as_ref().unwrap().texture_clamp_mode, 0);
        let glow = AtlasRow { key: "a.dds,a_n.dds,a_g.dds".into(), page: "textures\\DynDOLOD\\LOD\\DynDOLOD_Tamriel_Glow.dds".into(), ..row.clone() };
        let m = Material::for_part(&q, &q.textures, &ctx(MaterialClass::Plain, false, 4, true), Some(&glow));
        assert_eq!(m.textures[2], "Data\\textures\\dyndolod\\lod\\dyndolod_tamriel_glow_g.dds");
        assert_eq!(m.lighting.as_ref().unwrap().texture_clamp_mode, 0);
        assert!(is_passthru_model("meshes\\lod\\x\\farmhouse02glow_lod_0.nif", &["glow_lod".into(), "passthru_lod".into()]));
        assert!(!is_passthru_model("meshes\\lod\\x\\farmhouse02_lod.nif", &["glow_lod".into()]));
        assert_eq!(MaterialClass::parse("Snow"), MaterialClass::Snow);
        assert_eq!(MaterialClass::parse("volcanicSulfur").suffix(), "volcanicsulfur");
    }
}
