//! Derived from TreeLODGen, Copyright (C) 2026 Clemus (GPL-3.0, commit dc4cc8a); ported to Rust and
//! modified for LODForge 2026-10-23. See THIRD-PARTY-NOTICES.txt part 1.7.
//!
//! The LOD model of one level (TreeLODGen `Split.WriteStaticNif`, `Split.LodShapeName`,
//! `NifUtil.BakeTransform`, `NifUtil.PlainLodShader`; spec 7, 8.1):
//!
//! - a `BSFadeNode` named `Scene Root`, every part a `BSTriShape` child, crown parts first;
//! - transforms baked into the vertices (positions; normals, tangents and bitangents rotated and
//!   renormalised), so every shape has the identity transform;
//! - shape names DynDOLOD can pair with the full model's shapes and read `Crown` / `Trunk` from:
//!   `<own> Crown` / `<own> Trunk` (the name as it is when it already says so), else `Crown <i>` /
//!   `Trunk <i>`, `Leaves <i>` / `Bark <i>`, `LOD shape <i>`, skipping a name that contains another
//!   source shape's name with a different diffuse; bark whose source UVs tile (outside
//!   [-0.05, 1.05]) gets ` noatlas`, so it keeps its own texture instead of an atlas tile;
//! - the source's shader, texture set and alpha property; PBR, environment map, parallax and glow
//!   shaders become a plain diffuse + normal shader (copied PBR made distant trees glow).
//!
//! Divergences: the vertex format is chosen from the attributes present (TreeLODGen keeps the
//! source's descriptor); baked tangents are rotated instead of recomputed from the UVs (the same
//! result for a rotation with uniform scale).

use lodforge_nif::{AvObject, Block, LightingExtra, Nif, Node, ObjectNet, TriShape, VertexDesc};

use crate::collect::SourceShape;
use crate::levels::Level;
use crate::split::Kind;

/// The material of a generated part (billboard-cloud crown): written as is.
#[derive(Debug, Clone)]
pub struct Material {
    pub name: String,
    pub shader: lodforge_nif::LightingShader,
    pub textures: Vec<String>,
    /// NiAlphaProperty (flags, threshold).
    pub alpha: Option<(u16, u8)>,
}

/// UV range beyond which bark counts as tiling (`noatlas`).
pub const TILING_TOLERANCE: f32 = 0.05;

/// File name of a level's model (`<stem>_<crc>passthru_lod[_1|_2].nif`, lower case).
pub fn file_name(stem: &str, crc: u32, level: usize) -> String {
    let suffix = if level == 0 { String::new() } else { format!("_{level}") };
    format!("{}_{crc:08x}passthru_lod{suffix}.nif", stem.to_ascii_lowercase())
}

/// Folder of the generated models below the output's `meshes\`.
pub const FOLDER: &str = "dyndolod\\lod\\trees";

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > 1e-6 { [v[0] / l, v[1] / l, v[2] / l] } else { v }
}

fn bounds(points: &[[f32; 3]]) -> ([f32; 3], f32) {
    if points.is_empty() {
        return ([0.0; 3], 0.0);
    }
    let mut lo = [f32::MAX; 3];
    let mut hi = [f32::MIN; 3];
    for p in points {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let c = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, (lo[2] + hi[2]) / 2.0];
    let r = points.iter().map(|p| ((p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2) + (p[2] - c[2]).powi(2)).sqrt()).fold(0.0f32, f32::max);
    (c, r)
}

const SLSF1_SPECULAR: u32 = 1;
const SLSF1_SKINNED: u32 = 1 << 1;
const SLSF1_VERTEX_ALPHA: u32 = 1 << 3;

fn ai_source(src: &Nif, s: &TriShape) -> bool {
    matches!(s.alpha_property.and_then(|r| src.blocks.get(r as usize)), Some(Block::Alpha { .. }))
}
const SLSF1_ENVMAP: u32 = 1 << 7;
const SLSF1_PARALLAX: u32 = 1 << 11;
const SLSF1_EYE_ENVMAP: u32 = 1 << 17;
const SLSF1_PARALLAX_OCC: u32 = 1 << 28;
const SLSF2_GLOW: u32 = 1 << 6;
const SLSF2_ENVMAP_LIGHT_FADE: u32 = 1 << 15;
const SLSF2_PBR: u32 = 1 << 23;
const SLSF2_MULTI_LAYER_PARALLAX: u32 = 1 << 24;
const SLSF2_SOFT_LIGHTING: u32 = 1 << 25;
const SLSF2_RIM_LIGHTING: u32 = 1 << 26;
const SLSF2_BACK_LIGHTING: u32 = 1 << 27;

/// TreeLODGen `PlainLodShader`: returns what was stripped ("PBR+parallax"), empty when nothing.
pub fn plain_lod_shader(sh: &mut lodforge_nif::LightingShader, textures: &mut [String]) -> String {
    let t = sh.shader_type;
    let pbr = sh.flags2 & SLSF2_PBR != 0;
    let env = sh.flags1 & (SLSF1_ENVMAP | SLSF1_EYE_ENVMAP) != 0 || t == 1 || t == 16;
    let parallax = sh.flags1 & (SLSF1_PARALLAX | SLSF1_PARALLAX_OCC) != 0 || sh.flags2 & SLSF2_MULTI_LAYER_PARALLAX != 0 || [3, 7, 11].contains(&t);
    let glow = sh.flags2 & SLSF2_GLOW != 0 || t == 2;
    let what: Vec<&str> = [(pbr, "PBR"), (env, "envmap"), (parallax, "parallax"), (glow, "glow")].iter().filter(|(b, _)| *b).map(|(_, n)| *n).collect();
    if what.is_empty() {
        return String::new();
    }
    sh.flags1 &= !(SLSF1_ENVMAP | SLSF1_PARALLAX | SLSF1_EYE_ENVMAP | SLSF1_PARALLAX_OCC);
    if pbr {
        sh.flags1 &= !SLSF1_SPECULAR;
    }
    sh.flags2 &= !(SLSF2_GLOW | SLSF2_ENVMAP_LIGHT_FADE | SLSF2_PBR | SLSF2_MULTI_LAYER_PARALLAX);
    if [1, 2, 3, 7, 11, 16].contains(&t) {
        sh.shader_type = 0;
        sh.extra = LightingExtra::None;
    }
    for (i, tex) in textures.iter_mut().enumerate().skip(2) {
        let keep = !pbr && ((i == 2 && sh.flags2 & (SLSF2_SOFT_LIGHTING | SLSF2_RIM_LIGHTING) != 0) || (i == 7 && sh.flags2 & SLSF2_BACK_LIGHTING != 0));
        if !keep {
            tex.clear();
        }
    }
    what.join("+")
}

fn shape_of<'a>(nif: &'a Nif, s: &SourceShape) -> Option<&'a TriShape> {
    match nif.blocks.get(s.block) {
        Some(Block::TriShape(t)) => Some(t),
        _ => None,
    }
}

fn tiling(s: &TriShape) -> bool {
    s.uvs.iter().any(|uv| uv[0] < -TILING_TOLERANCE || uv[0] > 1.0 + TILING_TOLERANCE || uv[1] < -TILING_TOLERANCE || uv[1] > 1.0 + TILING_TOLERANCE)
}

/// TreeLODGen `LodShapeName`.
fn shape_name(nif: &Nif, shapes: &[SourceShape], all_names: &[(String, String)], shape: usize, kind: Kind, index: usize) -> String {
    let own = &shapes[shape].name;
    let own_tex = shape_of(nif, &shapes[shape]).and_then(|s| crate::split::diffuse(nif, s)).unwrap_or_default();
    let (word, alt) = match kind {
        Kind::Crown => ("Crown", "Leaves"),
        Kind::Trunk => ("Trunk", "Bark"),
    };
    let first = if own.to_ascii_lowercase().contains(&word.to_ascii_lowercase()) { own.clone() } else { format!("{own} {word}") };
    let mistaken = |cand: &str| {
        let c = cand.to_ascii_lowercase();
        all_names.iter().any(|(n, tex)| !n.is_empty() && !n.eq_ignore_ascii_case(own) && c.contains(&n.to_ascii_lowercase()) && !tex.eq_ignore_ascii_case(&own_tex))
    };
    let mut name = [first, format!("{word} {index}"), format!("{alt} {index}")].into_iter().find(|c| !mistaken(c)).unwrap_or_else(|| format!("LOD shape {index}"));
    if kind == Kind::Trunk && shape_of(nif, &shapes[shape]).is_some_and(tiling) {
        name.push_str(" noatlas");
    }
    name
}

/// Writes one level as a NIF. `notes` collects stripped shaders (`shader:PBR`).
pub fn level_nif(src: &Nif, shapes: &[SourceShape], level: &Level, notes: &mut Vec<String>) -> Option<Nif> {
    // Every named shape of the source file with its diffuse, for the name test.
    let all_names: Vec<(String, String)> = src
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::TriShape(s) => Some((s.av.net.name.clone().unwrap_or_default(), crate::split::diffuse(src, s).unwrap_or_default())),
            _ => None,
        })
        .collect();
    let mut out = Nif::new();
    out.header.block_types = vec!["BSFadeNode".into()];
    out.header.block_type_index = vec![0];
    out.add_block(Block::Node(Node { av: AvObject { net: ObjectNet::named("Scene Root"), ..Default::default() }, ..Default::default() }));
    let mut children = Vec::new();
    for (index, part) in level.parts.iter().enumerate() {
        let m = &part.mesh;
        if m.triangles.is_empty() || m.positions.len() > 65535 {
            continue;
        }
        if let Some(mat) = &part.material {
            let n = m.positions.len();
            let (center, radius) = bounds(&m.positions);
            let ts = out.add_block(Block::TextureSet(mat.textures.clone()));
            let mut shader = mat.shader.clone();
            shader.texture_set = Some(ts);
            let si = out.add_block(Block::LightingShader(shader));
            let ai = mat.alpha.map(|(flags, threshold)| out.add_block(Block::Alpha { net: ObjectNet::default(), flags, threshold }));
            let tang = m.tangents.len() == n && m.bitangents.len() == n;
            let shape = TriShape {
                av: AvObject { net: ObjectNet::named(&mat.name), ..Default::default() },
                bound_center: center,
                bound_radius: radius,
                shader_property: Some(si),
                alpha_property: ai,
                vertex_desc: VertexDesc::with_attributes(true, !m.uvs.is_empty(), m.normals.len() == n, tang, m.colors.len() == n).0,
                colors: if m.colors.len() == n { m.colors.clone() } else { Vec::new() },
                uvs: m.uvs.clone(),
                normals: m.normals.clone(),
                tangents: if tang { m.tangents.clone() } else { Vec::new() },
                bitangents: if tang { m.bitangents.clone() } else { Vec::new() },
                positions: m.positions.clone(),
                triangles: m.triangles.iter().map(|t| t.map(|v| v as u16)).collect(),
                ..Default::default()
            };
            children.push(Some(out.add_block(Block::TriShape(shape))));
            continue;
        }
        let Some(s) = shape_of(src, &shapes[part.shape]) else { continue };
        let xf = part.xf;
        let bake = !xf.is_identity();
        let positions: Vec<[f32; 3]> = if bake { m.positions.iter().map(|p| xf.point(*p)).collect() } else { m.positions.clone() };
        let dir = |v: &Vec<[f32; 3]>| -> Vec<[f32; 3]> { if bake { v.iter().map(|d| normalize(xf.rot(*d))).collect() } else { v.clone() } };
        let (center, radius) = bounds(&positions);
        let mut textures: Vec<String> = match s.shader_property.and_then(|r| src.blocks.get(r as usize)) {
            Some(Block::LightingShader(sh)) => match sh.texture_set.and_then(|r| src.blocks.get(r as usize)) {
                Some(Block::TextureSet(t)) => t.clone(),
                _ => Vec::new(),
            },
            _ => Vec::new(),
        };
        let mut shader = match s.shader_property.and_then(|r| src.blocks.get(r as usize)) {
            Some(Block::LightingShader(sh)) => sh.clone(),
            _ => continue,
        };
        let what = plain_lod_shader(&mut shader, &mut textures);
        if !what.is_empty() && !notes.iter().any(|n| n == &format!("shader:{what}")) {
            notes.push(format!("shader:{what}"));
        }
        if s.skin.is_some() {
            shader.flags1 &= !SLSF1_SKINNED;
        }
        // Vertex_Alpha takes the alpha from vertex colours; in object LOD that left alpha-tested
        // shapes unclipped (the cloud crowns drew as squares until it went).
        if ai_source(src, s) {
            shader.flags1 &= !SLSF1_VERTEX_ALPHA;
        }
        shader.net = ObjectNet { name: shader.net.name.clone(), ..Default::default() };
        let ts = out.add_block(Block::TextureSet(textures));
        shader.texture_set = Some(ts);
        let si = out.add_block(Block::LightingShader(shader));
        let ai = match s.alpha_property.and_then(|r| src.blocks.get(r as usize)) {
            Some(Block::Alpha { flags, threshold, .. }) => Some(out.add_block(Block::Alpha { net: ObjectNet::default(), flags: *flags, threshold: *threshold })),
            _ => None,
        };
        let n = m.positions.len();
        let has = [!m.uvs.is_empty(), m.normals.len() == n, m.tangents.len() == n && m.bitangents.len() == n, m.colors.len() == n];
        let name = shape_name(src, shapes, &all_names, part.shape, part.kind, index);
        let shape = TriShape {
            av: AvObject { net: ObjectNet::named(&name), ..Default::default() },
            bound_center: center,
            bound_radius: radius,
            shader_property: Some(si),
            alpha_property: ai,
            vertex_desc: VertexDesc::with_attributes(true, has[0], has[1], has[2], has[3]).0,
            uvs: if has[0] { m.uvs.clone() } else { Vec::new() },
            normals: if has[1] { dir(&m.normals) } else { Vec::new() },
            tangents: if has[2] { dir(&m.tangents) } else { Vec::new() },
            bitangents: if has[2] { dir(&m.bitangents) } else { Vec::new() },
            colors: if has[3] { m.colors.clone() } else { Vec::new() },
            positions,
            triangles: m.triangles.iter().map(|t| t.map(|v| v as u16)).collect(),
            ..Default::default()
        };
        children.push(Some(out.add_block(Block::TriShape(shape))));
    }
    if children.is_empty() {
        return None;
    }
    if let Block::Node(r) = &mut out.blocks[0] {
        r.children = children;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_follow_treelodgen() {
        assert_eq!(file_name("TreePineForest01", 0x0A1B2C3D, 0), "treepineforest01_0a1b2c3dpassthru_lod.nif");
        assert_eq!(file_name("treepineforest01", 0x0A1B2C3D, 2), "treepineforest01_0a1b2c3dpassthru_lod_2.nif");
    }

    #[test]
    fn pbr_becomes_a_plain_shader() {
        let mut sh = lodforge_nif::LightingShader { shader_type: 3, flags1: SLSF1_SPECULAR | SLSF1_PARALLAX, flags2: SLSF2_PBR | SLSF2_SOFT_LIGHTING, ..Default::default() };
        let mut tex: Vec<String> = (0..8).map(|i| format!("t{i}.dds")).collect();
        assert_eq!(plain_lod_shader(&mut sh, &mut tex), "PBR+parallax");
        assert_eq!(sh.shader_type, 0);
        assert_eq!(sh.flags1 & (SLSF1_SPECULAR | SLSF1_PARALLAX), 0);
        assert_eq!(sh.flags2, SLSF2_SOFT_LIGHTING);
        assert_eq!(&tex[..3], &["t0.dds".to_string(), "t1.dds".into(), String::new()]);
        // A plain shader stays as it is.
        let mut plain = lodforge_nif::LightingShader { flags1: SLSF1_SPECULAR, ..Default::default() };
        let mut tex2 = vec!["a.dds".to_string(), "a_n.dds".into(), "x.dds".into()];
        assert_eq!(plain_lod_shader(&mut plain, &mut tex2), "");
        assert_eq!(tex2[2], "x.dds");
    }
}
