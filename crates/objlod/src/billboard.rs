//! Tree billboards as object LOD (docs/re/object-lod-spec.md 6.1): a reference whose LOD model
//! column names a billboard texture becomes a `treepassthru` shape of two crossed quads, settled
//! on the DynDOLOD golden2 (`Tamriel.4.0.-16.bto`, reference `170A4ACF`, rotation 137.51
//! degrees, billboard `alder_slim_side01_summer`: width 623.58, depth 585.79, height 1209.37,
//! shift Z -36.94):
//!
//! - the first quad runs along the reference's local Y axis (world `(sin r, cos r)` for the
//!   export's rotation `r` about Z) with the billboard **depth**, the second along local X
//!   (world `(-cos r, sin r)`) with its **width**, both times the reference scale, centred on the
//!   reference position plus the billboard's shift X / Y; the bottom edge sits at `z + shiftZ
//!   * scale`, the top `height * scale` above it;
//! - vertices per quad: `+axis` bottom, `-axis` bottom, `-axis` top, `+axis` top, triangles
//!   `[0 1 2] [2 3 0]`; UVs `(u1, v1) (u0, v1) (u0, v0) (u1, v0)` of the billboard's tile on the
//!   `_Alpha` atlas page (the whole texture when the map has no row);
//! - normals `(0, 0, 1)`, tangents `(0, 0, -1)`, bitangent = the quad's axis
//!   (`DynDOLOD_SSE_flat_normals.txt` rotated into the quad);
//! - vertex colour = grey `FlatLODVertexColor` (field 9) varied by `FlatLODColorVariance` (field
//!   10) per quad (golden: 116 and 144 for 0.55 / 0.1), alpha 255. LODGen draws the variance
//!   from a time-seeded random; ours is a hash of the reference id and the quad, so a rebuild
//!   is reproducible and the tolerant comparison ignores billboard colours either way;
//! - shader: lighting type 0, flags1 `ZBuffer_Test` only, flags2 `ZBuffer_Write | Double_Sided |
//!   Vertex_Colors` (`LOD_Objects` added from `FlatLODLevelLODFlag`), glossiness 80, specular
//!   black, emissive black times 0, lighting effect 1 = field 6, clamp 0 on the page (3 off it),
//!   textures: the page, its `_n`, and the page again in the glow slot when the billboard has a
//!   glow (field 8 other than black).
//!
//! The quads follow the reference's whole rotation (x, y and z; 444 of golden2's 546 billboard
//! references are tilted, trees on cliffs by 100 degrees and more, and the golden tilts their
//! quads with them: a conifer rotated 35.6 degrees about x in `Tamriel.16.-32.-16.bto` lies
//! exactly on the fully rotated quads, so `TreeLODMaxTilt=30` does not cap the geometry). The
//! golden's height variance (field 14) is 0 in every line and is not applied. No `-LargeRef` suffix; the hidden-face test applies (the golden's LOD16 tree shapes
//! lose single triangles under the terrain); present at LOD 4, 8 and 16 alike (the export
//! names the billboard in all three columns).

use lodforge_nif::{LightingExtra, LightingShader, ObjectNet};

use crate::atlas_map::AtlasMap;
use crate::export::StaticRef;
use crate::flat::FlatTexture;
use crate::material::{self, Material, SLSF2_DOUBLE_SIDED, SLSF2_LOD_OBJECTS, SLSF2_VERTEX_COLORS, SLSF2_ZBUFFER_WRITE};
use crate::mesh::{MeshPart, ShaderSource};
use crate::transform::Xform;

/// The shape name of tree billboards.
pub const NAME: &str = "treepassthru";

/// The geometry (world space) and material of one tree billboard.
pub struct Billboard {
    pub part: MeshPart,
    pub material: Material,
}

/// Grey level of a billboard quad: `vc` varied by `variance` through a hash of the reference
/// id and the quad index, clamped to 0..1, as a byte.
pub fn grey(vc: f32, variance: f32, ref_id: &str, quad: usize) -> u8 {
    let mut h: u32 = 0x811c_9dc5;
    for b in ref_id.bytes().chain([b'#', quad as u8]) {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    let unit = (h % 2001) as f32 / 1000.0 - 1.0;
    ((vc + variance * unit).clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Builds the billboard of a reference: `texture` is the LOD model column (lower case), `ft`
/// its FlatTextures line, `lod_flag` whether `LOD_Objects` is set at this level.
pub fn build(r: &StaticRef, ft: &FlatTexture, texture: &str, atlas: &AtlasMap, lod_flag: bool) -> Billboard {
    // The reference's full transform (rotation about x, y and z, then scale): the quads are
    // built in the reference's local frame, as a LOD model's shapes would be.
    let xform = Xform::reference([r.x, r.y, r.z], [r.rot_x, r.rot_y, r.rot_z], r.scale);
    let rot = xform.rotation_only();
    let row = atlas.lookup(texture, "", "", None);
    let (u0, v0, u1, v1) = match row {
        Some(row) => {
            let a = row.remap([0.0, 0.0]);
            let b = row.remap([1.0, 1.0]);
            (a[0], a[1], b[0], b[1])
        }
        None => (0.0, 0.0, 1.0, 1.0),
    };
    let mut positions = Vec::with_capacity(8);
    let mut uvs = Vec::with_capacity(8);
    let mut normals = Vec::with_capacity(8);
    let mut tangents = Vec::with_capacity(8);
    let mut bitangents = Vec::with_capacity(8);
    let mut colors = Vec::with_capacity(8);
    let mut triangles = Vec::with_capacity(4);
    // Quad 1 along local +Y (depth), quad 2 along local -X (width); bottom at shift Z, the
    // shift X / Y in the local frame; the first vertex at the +axis end.
    let (z0, z1) = (ft.shift_z, ft.shift_z + ft.height);
    let quads = [([0.0, 1.0, 0.0], ft.depth * 0.5), ([-1.0, 0.0, 0.0], ft.width * 0.5)];
    for (qi, (axis, half)) in quads.into_iter().enumerate() {
        let base = positions.len() as u16;
        let p = |sign: f32, z: f32| xform.apply([ft.shift_x + sign * half * axis[0], ft.shift_y + sign * half * axis[1], z]);
        positions.extend([p(1.0, z0), p(-1.0, z0), p(-1.0, z1), p(1.0, z1)]);
        uvs.extend([[u1, v1], [u0, v1], [u0, v0], [u1, v0]]);
        let g = grey(ft.vertex_color, ft.color_variance, &r.ref_id, qi);
        let tangent = crate::transform::normalize(rot.apply([0.0, 0.0, -1.0]), [0.0, 0.0, -1.0]);
        let bitangent = crate::transform::normalize(rot.apply(axis), [1.0, 0.0, 0.0]);
        for _ in 0..4 {
            // The flat normals file: the normal stays world up, tangent and bitangent follow
            // the quad (golden `Tamriel.16.-16.-16.bto`, a tilted tree: n (0, 0, 1), t (-0.18,
            // -0.51, -0.84)).
            normals.push([0.0, 0.0, 1.0]);
            tangents.push(tangent);
            bitangents.push(bitangent);
            colors.push([g, g, g, 255]);
        }
        triangles.extend([[base, base + 1, base + 2], [base + 2, base + 3, base]]);
    }
    // Textures: the page (or the billboard itself), its normal map, the glow slot.
    let glow = !ft.glow.is_empty() && !ft.glow.to_ascii_lowercase().contains("black");
    let stem = texture.trim_end_matches(".dds").to_string();
    let written: Vec<String> = match row {
        Some(row) => {
            let page = crate::export::texture_key(&row.page);
            let pstem = page.trim_end_matches(".dds").to_string();
            let mut w = vec![String::new(); 9];
            w[0] = format!("Data\\{page}");
            w[1] = format!("Data\\{pstem}_n.dds");
            if glow {
                w[2] = format!("Data\\{page}");
            }
            w
        }
        None => {
            let mut w = vec![String::new(); 9];
            w[0] = material::written_texture(texture);
            w[1] = material::written_texture(&format!("{stem}_n.dds"));
            if glow {
                w[2] = material::written_texture(&ft.glow);
            }
            w
        }
    };
    let lighting = LightingShader {
        shader_type: 0,
        net: ObjectNet::default(),
        flags1: 0x8000_0000,
        flags2: SLSF2_ZBUFFER_WRITE | SLSF2_DOUBLE_SIDED | SLSF2_VERTEX_COLORS | if lod_flag { SLSF2_LOD_OBJECTS } else { 0 },
        uv_offset: [0.0; 2],
        uv_scale: [1.0; 2],
        texture_set: None,
        emissive_color: [0.0; 3],
        emissive_multiple: 0.0,
        texture_clamp_mode: if row.is_some() { 0 } else { 3 },
        alpha: 1.0,
        refraction_strength: 0.0,
        glossiness: 80.0,
        specular_color: [0.0; 3],
        specular_strength: 1.0,
        lighting_effect_1: ft.lighting_effect,
        lighting_effect_2: 0.0,
        extra: LightingExtra::None,
    };
    let material = Material::fixed(NAME.to_string(), written, lighting.clone());
    let mut textures: [String; 9] = Default::default();
    textures[0] = texture.to_string();
    let part = MeshPart {
        name: NAME.to_string(),
        index: 0,
        textures,
        shader: ShaderSource::Lighting(lighting),
        positions,
        uvs,
        normals,
        tangents,
        bitangents,
        colors: Some(colors),
        triangles,
        source_normals: true,
        source_tangents: true,
        legacy: false,
        alpha: None,
    };
    Billboard { part, material }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flat::FlatList;

    #[test]
    fn golden_tree() {
        // DynDOLOD golden2, Tamriel.4.0.-16.bto, reference 170A4ACF.
        let mut r = StaticRef::default();
        r.ref_id = "170A4ACF".into();
        r.x = 3332.517090;
        r.y = -51423.910156;
        r.z = 1372.252808;
        r.rot_z = 137.509995;
        r.scale = 1.0;
        let flat = FlatList::parse("textures\\terrain\\lodgen\\x.esp\\alder.dds\t623.583923339844\t1209.37487792969\t-36.9375\t1\t0\tn;n;n;n\ttextures\\white.dds\t0.55\t0.1\t585.790954589844\t0\t0\t0\t\r\n");
        let ft = flat.get("textures\\terrain\\lodgen\\x.esp\\alder.dds").unwrap();
        let b = build(&r, ft, "textures\\terrain\\lodgen\\x.esp\\alder.dds", &AtlasMap::default(), false);
        let p = &b.part.positions;
        let close = |a: [f32; 3], b: [f32; 3]| (0..3).all(|i| (a[i] - b[i]).abs() < 0.01);
        assert!(close(p[0], [3530.357, -51639.891, 1335.315]), "{:?}", p[0]);
        assert!(close(p[1], [3134.677, -51207.930, 1335.315]), "{:?}", p[1]);
        assert!(close(p[2], [3134.677, -51207.930, 2544.690]), "{:?}", p[2]);
        assert!(close(p[4], [3562.431, -51213.305, 1335.315]), "{:?}", p[4]);
        assert!(close(p[5], [3102.603, -51634.516, 1335.315]), "{:?}", p[5]);
        assert_eq!(b.part.triangles, vec![[0, 1, 2], [2, 3, 0], [4, 5, 6], [6, 7, 4]]);
        assert_eq!(b.part.uvs[0], [1.0, 1.0]);
        assert_eq!(b.part.uvs[2], [0.0, 0.0]);
        let l = b.material.lighting.as_ref().unwrap();
        assert_eq!((l.flags1, l.flags2, l.glossiness, l.emissive_multiple, l.texture_clamp_mode), (0x8000_0000, 0x31, 80.0, 0.0, 3));
        assert_eq!(b.material.textures[2], "Data\\textures\\white.dds");
        let c = b.part.colors.as_ref().unwrap();
        assert!(c[0][0] >= 115 && c[0][0] <= 166 && c[0][3] == 255);
        assert_eq!(b.material.name, "treepassthru");
    }
}
