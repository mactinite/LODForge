//! LOD model import (docs/re/object-lod-spec.md 3.1, 3.2): the drawable shapes of a NIF, walked
//! from the footer roots through the node tree with the node transforms composed, baked into
//! model space with normals, tangents and bitangents on every vertex.
//!
//! Derived from AnvilLOD `AnvilLOD.Meshes/Nif/NifGeometryReader.cs` (Copyright (c) 2026 CageTV,
//! GPL-3.0-only, commit 606eece): the walk (hidden nodes and shapes skipped, skinned shapes
//! skipped, shapes without a shader skipped), the bake of positions by the full transform and of
//! the direction vectors by its rotation part, face normals for shapes without normals and
//! tangents from the UV layout for shapes without tangents. Modified: the file is decoded by the
//! `lodforge-nif` reader (SSE and LE geometry alike) instead of AnvilLOD's span reader, effect
//! shaders are kept (passthru models carry them), the material reduction lives in
//! [`crate::material`], AnvilLOD's fake-water replacement and alpha handling are not ported
//! (spec 3.4: no NiAlphaProperty in the output), and editor markers are skipped.

use std::collections::HashSet;

use lodforge_nif::{Block, EffectShader, LightingShader, Nif, TriShape};

use crate::transform::{Mat3, Xform, cross, dot, normalize, sub};

/// The shader of a source shape, as the file has it.
#[derive(Debug, Clone, PartialEq)]
pub enum ShaderSource {
    /// `BSLightingShaderProperty` (its texture set is in [`MeshPart::textures`]).
    Lighting(LightingShader),
    /// `BSEffectShaderProperty`.
    Effect(EffectShader),
}

/// One source shape in model space.
#[derive(Debug, Clone)]
pub struct MeshPart {
    /// Shape name (spec 3.1 lists the terms LODGen reads from it).
    pub name: String,
    /// Depth-first position of the shape among the model's shapes, the index the AltTextures
    /// side file uses (spec 7.3).
    pub index: usize,
    /// Nine texture slots as stored (lighting shader: the texture set; effect shader: the source
    /// texture in slot 0).
    pub textures: [String; 9],
    pub shader: ShaderSource,
    pub positions: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub normals: Vec<[f32; 3]>,
    pub tangents: Vec<[f32; 3]>,
    pub bitangents: Vec<[f32; 3]>,
    /// RGBA8 vertex colours when the shape has them.
    pub colors: Option<Vec<[u8; 4]>>,
    pub triangles: Vec<[u16; 3]>,
    /// Whether the normals / tangents came from the file (else generated).
    pub source_normals: bool,
    pub source_tangents: bool,
    /// The shape came from Skyrim LE geometry data (NiTriShape / NiTriStrips).
    pub legacy: bool,
    /// The shape's NiAlphaProperty (flags, threshold), if any: the atlas key carries the
    /// threshold / 16 when it is not 128 (spec 1.2).
    pub alpha: Option<(u16, u8)>,
}

/// A loaded LOD model.
#[derive(Debug, Clone)]
pub struct LodMesh {
    pub path: String,
    pub parts: Vec<MeshPart>,
    pub warnings: Vec<String>,
    /// Source shapes counted in the depth-first index but not usable.
    pub skipped: usize,
}

/// Parses and bakes a model. Files the reader rejects are an error; a file without usable
/// shapes is `Ok` with no parts (DynDOLOD Resources ships such "no LOD" models on purpose).
pub fn import(path: &str, bytes: &[u8]) -> Result<LodMesh, lodforge_nif::Error> {
    import_with(path, bytes, true)
}

/// [`import`] with a choice: with `node_transforms` false the node and shape transforms of the
/// file are ignored and the vertices are taken as stored (xLODGen's `IgnoreTranslation` meshes,
/// spec 2.2, open question 7: settled on the Solitude quad `Tamriel.4.-20.24.bto`, where the
/// listed meshes land in place only this way while the reference transform still applies).
pub fn import_with(path: &str, bytes: &[u8], node_transforms: bool) -> Result<LodMesh, lodforge_nif::Error> {
    let nif = lodforge_nif::parse_all_geometry(bytes)?;
    let mut mesh = LodMesh { path: path.to_string(), parts: Vec::new(), warnings: Vec::new(), skipped: 0 };
    let mut visited = HashSet::new();
    let mut index = 0usize;
    for root in nif.roots.clone() {
        walk(&nif, root, &Xform::IDENTITY, &mut mesh, &mut visited, &mut index, node_transforms);
    }
    Ok(mesh)
}

fn walk(nif: &Nif, r: lodforge_nif::Ref, parent: &Xform, mesh: &mut LodMesh, visited: &mut HashSet<u32>, index: &mut usize, node_transforms: bool) {
    let Some(i) = r else { return };
    if !visited.insert(i) {
        return;
    }
    match nif.get(Some(i)) {
        Some(Block::Node(n)) => {
            if n.av.flags & 1 != 0 {
                return;
            }
            let world = if node_transforms { parent.then(&Xform::from_nif(&n.av.transform)) } else { Xform::IDENTITY };
            for c in n.children.clone() {
                walk(nif, c, &world, mesh, visited, index, node_transforms);
            }
        }
        Some(Block::TriShape(s)) => {
            let my_index = *index;
            *index += 1;
            match bake(nif, s, parent, my_index, node_transforms) {
                Ok(p) => mesh.parts.push(p),
                Err(why) => {
                    mesh.skipped += 1;
                    mesh.warnings.push(format!("{}: shape {} {:?} skipped: {why}", mesh.path, i, s.av.net.name));
                }
            }
        }
        _ => {}
    }
}

fn bake(nif: &Nif, s: &TriShape, parent: &Xform, index: usize, node_transforms: bool) -> Result<MeshPart, &'static str> {
    if s.av.flags & 1 != 0 {
        return Err("hidden");
    }
    if s.skin.is_some() {
        return Err("skinned");
    }
    let name = s.av.net.name.clone().unwrap_or_default();
    if name.to_ascii_lowercase().contains("editormarker") {
        return Err("editor marker");
    }
    let n = s.positions.len();
    if n == 0 || s.triangles.is_empty() {
        return Err("no geometry");
    }
    if s.triangles.iter().flatten().any(|&i| i as usize >= n) {
        return Err("triangle index out of range");
    }
    let mut textures: [String; 9] = Default::default();
    let shader = match nif.get(s.shader_property) {
        Some(Block::LightingShader(l)) => {
            if let Some(Block::TextureSet(t)) = nif.get(l.texture_set) {
                for (slot, tex) in textures.iter_mut().zip(t) {
                    *slot = tex.clone();
                }
            }
            let mut l = l.clone();
            l.texture_set = None;
            ShaderSource::Lighting(l)
        }
        Some(Block::EffectShader(e)) => {
            textures[0] = e.source_texture.clone();
            ShaderSource::Effect(e.clone())
        }
        _ => return Err("no lighting or effect shader"),
    };
    let alpha = match nif.get(s.alpha_property) {
        Some(Block::Alpha { flags, threshold, .. }) => Some((*flags, *threshold)),
        _ => None,
    };
    let world = if node_transforms { parent.then(&Xform::from_nif(&s.av.transform)) } else { Xform::IDENTITY };
    let rot = world.rotation_only();
    let positions: Vec<[f32; 3]> = s.positions.iter().map(|&p| world.apply(p)).collect();
    let uvs: Vec<[f32; 2]> = if s.uvs.len() == n { s.uvs.clone() } else { vec![[0.0; 2]; n] };
    let source_normals = s.normals.len() == n;
    let colors_placeholder = ();
    let _ = colors_placeholder;
    let normals: Vec<[f32; 3]> = if source_normals { s.normals.iter().map(|&v| normalize(rot.apply(v), [0.0, 0.0, 1.0])).collect() } else { face_normals(&positions, &s.triangles) };
    let source_tangents = s.tangents.len() == n && s.bitangents.len() == n;
    let (tangents, bitangents) = if source_tangents {
        (s.tangents.iter().map(|&v| normalize(rot.apply(v), [1.0, 0.0, 0.0])).collect(), s.bitangents.iter().map(|&v| normalize(rot.apply(v), [0.0, 1.0, 0.0])).collect())
    } else {
        tangents_from_uv(&positions, &uvs, &normals, &s.triangles)
    };
    // The colour array as stored; whether it counts is decided per placement
    // (`crate::build`: VC whitening, white arrays, the snow shader's alpha).
    let colors = (s.colors.len() == n).then(|| s.colors.clone());
    // The shader's UV offset and scale are baked into the vertices (the written shader has
    // 0 / 1): golden passthru water shapes carry UVs scaled by their source shader's UV scale.
    let (uv_offset, uv_scale) = match &shader {
        ShaderSource::Lighting(l) => (l.uv_offset, l.uv_scale),
        ShaderSource::Effect(e) => (e.uv_offset, e.uv_scale),
    };
    let uvs: Vec<[f32; 2]> = if uv_offset != [0.0; 2] || uv_scale != [1.0; 2] { uvs.iter().map(|uv| [uv[0] * uv_scale[0] + uv_offset[0], uv[1] * uv_scale[1] + uv_offset[1]]).collect() } else { uvs };
    Ok(MeshPart { name, index, textures, shader, positions, uvs, normals, tangents, bitangents, colors, triangles: s.triangles.clone(), source_normals, source_tangents, legacy: s.legacy, alpha })
}

/// Area-weighted vertex normals from the triangles (unit +Z where a vertex has none).
pub fn face_normals(pos: &[[f32; 3]], tris: &[[u16; 3]]) -> Vec<[f32; 3]> {
    let mut n = vec![[0f32; 3]; pos.len()];
    for t in tris {
        let (a, b, c) = (pos[t[0] as usize], pos[t[1] as usize], pos[t[2] as usize]);
        let fnormal = cross(sub(b, a), sub(c, a));
        for &i in t {
            for k in 0..3 {
                n[i as usize][k] += fnormal[k];
            }
        }
    }
    n.into_iter().map(|v| normalize(v, [0.0, 0.0, 1.0])).collect()
}

/// Per-vertex tangent and bitangent from the UV layout, orthogonal to the normal; the bitangent
/// is `normal x tangent` flipped to the UV winding.
pub fn tangents_from_uv(pos: &[[f32; 3]], uv: &[[f32; 2]], nrm: &[[f32; 3]], tris: &[[u16; 3]]) -> (Vec<[f32; 3]>, Vec<[f32; 3]>) {
    let mut tan = vec![[0f32; 3]; pos.len()];
    let mut bit = vec![[0f32; 3]; pos.len()];
    for t in tris {
        let (a, b, c) = (t[0] as usize, t[1] as usize, t[2] as usize);
        let (e1, e2) = (sub(pos[b], pos[a]), sub(pos[c], pos[a]));
        let d1 = [uv[b][0] - uv[a][0], uv[b][1] - uv[a][1]];
        let d2 = [uv[c][0] - uv[a][0], uv[c][1] - uv[a][1]];
        let det = d1[0] * d2[1] - d2[0] * d1[1];
        if det.abs() < 1e-12 {
            continue;
        }
        let inv = 1.0 / det;
        let tv = [(e1[0] * d2[1] - e2[0] * d1[1]) * inv, (e1[1] * d2[1] - e2[1] * d1[1]) * inv, (e1[2] * d2[1] - e2[2] * d1[1]) * inv];
        let bv = [(e2[0] * d1[0] - e1[0] * d2[0]) * inv, (e2[1] * d1[0] - e1[1] * d2[0]) * inv, (e2[2] * d1[0] - e1[2] * d2[0]) * inv];
        for &i in &[a, b, c] {
            for k in 0..3 {
                tan[i][k] += tv[k];
                bit[i][k] += bv[k];
            }
        }
    }
    for i in 0..pos.len() {
        let n = nrm[i];
        let d = dot(n, tan[i]);
        let t = normalize([tan[i][0] - n[0] * d, tan[i][1] - n[1] * d, tan[i][2] - n[2] * d], [1.0, 0.0, 0.0]);
        let mut b = normalize(cross(n, t), [0.0, 1.0, 0.0]);
        if dot(b, bit[i]) < 0.0 {
            b = [-b[0], -b[1], -b[2]];
        }
        tan[i] = t;
        bit[i] = b;
    }
    (tan, bit)
}

/// Applies the rotation part of a transform to a direction and renormalizes.
pub fn rotate_dir(rot: &Mat3, v: [f32; 3], fallback: [f32; 3]) -> [f32; 3] {
    normalize(rot.apply(v), fallback)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lodforge_nif::{AvObject, Node, ObjectNet, Transform};

    fn shape(name: &str, hidden: bool) -> TriShape {
        let mut s = TriShape {
            av: AvObject { net: ObjectNet::named(name), flags: if hidden { 15 } else { 14 }, transform: Transform { translation: [0.0, 0.0, 10.0], ..Default::default() }, collision: None },
            positions: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
            uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
            triangles: vec![[0, 1, 2]],
            ..Default::default()
        };
        s.update_vertex_desc();
        s
    }

    #[test]
    fn walk_and_bake() {
        let mut nif = Nif::new();
        let root = Node { av: AvObject { net: ObjectNet::named("Root"), transform: Transform { translation: [100.0, 0.0, 0.0], scale: 2.0, ..Default::default() }, ..Default::default() }, children: vec![Some(1), Some(4), Some(5), Some(6)], ..Default::default() };
        nif.add_block(Block::Node(root));
        let mut s = shape("A", false);
        s.shader_property = Some(2);
        nif.add_block(Block::TriShape(s));
        nif.add_block(Block::LightingShader(LightingShader { texture_set: Some(3), ..Default::default() }));
        nif.add_block(Block::TextureSet(vec!["textures\\d.dds".into(), "textures\\d_n.dds".into()]));
        let mut h = shape("Hidden", true);
        h.shader_property = Some(2);
        nif.add_block(Block::TriShape(h));
        nif.add_block(Block::TriShape(shape("NoShader", false)));
        let mut e = shape("EditorMarker", false);
        e.shader_property = Some(2);
        nif.add_block(Block::TriShape(e));
        let bytes = nif.to_bytes().unwrap();
        let m = import("meshes\\x.nif", &bytes).unwrap();
        assert_eq!(m.parts.len(), 1);
        assert_eq!(m.skipped, 3);
        let p = &m.parts[0];
        assert_eq!(p.index, 0);
        assert_eq!(p.textures[0], "textures\\d.dds");
        // Root scale 2 and translation, shape translation 10 in z: (1,0,0) -> 2*(1,0,10) + (100,0,0).
        assert_eq!(p.positions[1], [102.0, 0.0, 20.0]);
        assert!(!p.source_normals && !p.source_tangents);
        assert_eq!(p.normals[0], [0.0, 0.0, 1.0]);
        assert_eq!(p.tangents[0], [1.0, 0.0, 0.0]);
        assert_eq!(p.bitangents[0], [0.0, 1.0, 0.0]);
        assert!(p.colors.is_none());
        assert!(matches!(p.shader, ShaderSource::Lighting(_)));
    }
}
