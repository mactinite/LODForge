//! The BTO block layout (docs/re/object-lod-spec.md 3.3), as observed in the golden files:
//!
//! ```text
//! NiNode "obj"                              flags 14, identity
//! └─ BSMultiBoundNode (one per shape)      flags 14, culling mode 1, no name
//!    ├─ BSSubIndexTriShape "<name>"        flags 14, translation (bx·4096, by·4096, 0), scale L,
//!    │   │                                 bound centre/radius in local units, segments,
//!    │   │                                 extra data [BSDistantObjectLargeRefExtraData "DOLRED" 1]
//!    │   ├─ BSLightingShaderProperty       (or BSEffectShaderProperty for passthru shapes)
//!    │   │   └─ BSShaderTextureSet         9 slots, shared between shapes with the same set
//!    └─ BSMultiBound → BSMultiBoundAABB    world-space centre and half extents
//! ```
//!
//! Block order per shape as in the golden files: node, shape, [large-ref extra data], shader,
//! [texture set], multibound, AABB. The bound radius is the half diagonal of the local box
//! (`Tamriel.4.0.-16.bto`: centre (3413.6, 1717.3, 852.7), radius 1871.1); the AABB is the plain
//! world box (no clamp). Serialized by the `lodforge-nif` writer (stage B1).

use std::collections::HashMap;

use lodforge_nif::{AvObject, Block, Nif, Node, ObjectNet, Transform, TriShape};

use crate::build::BuiltShape;
use crate::grid::Quad;

/// Assembles the NIF of one quad from its shapes (in the order given).
pub fn assemble(q: &Quad, shapes: &[BuiltShape]) -> Nif {
    let mut nif = Nif::new();
    let root = nif.add_block(Block::Node(Node { av: AvObject { net: ObjectNet::named("obj"), ..Default::default() }, ..Default::default() }));
    let origin = q.origin();
    let level = q.level as f32;
    let mut texture_sets: HashMap<Vec<String>, u32> = HashMap::new();
    let mut children = Vec::with_capacity(shapes.len());
    for s in shapes {
        let node = nif.add_block(Block::Node(Node::multi_bound_node(None, 1)));
        children.push(Some(node));
        let shape_index = nif.add_block(Block::Unknown);
        let mut extra = Vec::new();
        if s.material.large_ref {
            extra.push(Some(nif.add_block(Block::LargeRefExtra { name: Some("DOLRED".into()), large_ref: 1 })));
        }
        // The shader block sits before its texture set, which is shared by every shape with the
        // same slots (as in the golden files).
        let shader = nif.add_block(Block::Unknown);
        let block = if let Some(l) = &s.material.lighting {
            let mut l = l.clone();
            let set = &s.material.textures;
            let ts = match texture_sets.get(set) {
                Some(&i) => i,
                None => {
                    let i = nif.add_block(Block::TextureSet(set.clone()));
                    texture_sets.insert(set.clone(), i);
                    i
                }
            };
            l.texture_set = Some(ts);
            Block::LightingShader(l)
        } else if let Some(e) = &s.material.effect {
            Block::EffectShader(e.clone())
        } else {
            Block::Unknown
        };
        nif.blocks[shader as usize] = block;
        finish_shape(&mut nif, s, shape_index, node, &extra, shader, origin, level);
    }
    if let Block::Node(r) = &mut nif.blocks[root as usize] {
        r.children = children;
    }
    nif.roots = vec![Some(root)];
    nif
}

#[allow(clippy::too_many_arguments)]
fn finish_shape(nif: &mut Nif, s: &BuiltShape, shape_index: u32, node: u32, extra: &[lodforge_nif::Ref], shader: u32, origin: [f32; 3], level: f32) {
    let (min, max) = bounds(&s.positions);
    let center = [(min[0] + max[0]) / 2.0, (min[1] + max[1]) / 2.0, (min[2] + max[2]) / 2.0];
    let radius = ((max[0] - center[0]).powi(2) + (max[1] - center[1]).powi(2) + (max[2] - center[2]).powi(2)).sqrt();
    let mut shape = TriShape {
        av: AvObject { net: ObjectNet { name: Some(s.material.name.clone()), extra_data: extra.to_vec(), controller: None }, flags: 14, transform: Transform { translation: origin, rotation: Transform::default().rotation, scale: level }, collision: None },
        bound_center: center,
        bound_radius: radius,
        skin: None,
        shader_property: Some(shader),
        alpha_property: None,
        vertex_desc: 0,
        positions: s.positions.clone(),
        uvs: s.uvs.clone(),
        normals: s.normals.clone(),
        tangents: s.tangents.clone(),
        bitangents: s.bitangents.clone(),
        colors: s.colors.clone().unwrap_or_default(),
        triangles: s.triangles.clone(),
        lod_sizes: None,
        segments: Some(s.segments.clone()),
        legacy: false,
        colors_f32: Vec::new(),
    };
    shape.update_vertex_desc();
    nif.blocks[shape_index as usize] = Block::TriShape(shape);
    let aabb = Block::MultiBoundAabb {
        position: [origin[0] + center[0] * level, origin[1] + center[1] * level, origin[2] + center[2] * level],
        extent: [(max[0] - center[0]) * level, (max[1] - center[1]) * level, (max[2] - center[2]) * level],
    };
    let mb = nif.add_block(Block::Unknown);
    let aabb_index = nif.add_block(aabb);
    nif.blocks[mb as usize] = Block::MultiBound { data: Some(aabb_index) };
    if let Block::Node(n) = &mut nif.blocks[node as usize] {
        n.children = vec![Some(shape_index)];
        n.multi_bound = Some(mb);
    }
}

fn bounds(p: &[[f32; 3]]) -> ([f32; 3], [f32; 3]) {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for v in p {
        for k in 0..3 {
            min[k] = min[k].min(v[k]);
            max[k] = max[k].max(v[k]);
        }
    }
    if p.is_empty() { ([0.0; 3], [0.0; 3]) } else { (min, max) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material::Material;
    use lodforge_nif::{LightingShader, Segment};

    fn shape(name: &str, large: bool, tex: &str) -> BuiltShape {
        let textures: Vec<String> = (0..9).map(|i| if i == 0 { format!("Data\\{tex}") } else { String::new() }).collect();
        let material = Material { name: name.into(), textures, lighting: Some(LightingShader::default()), effect: None, colors: false, large_ref: large, hd: false, passthru: false, key: name.into() };
        BuiltShape {
            material,
            positions: vec![[0.0, 0.0, -10.0], [100.0, 0.0, 0.0], [0.0, 100.0, 30.0]],
            uvs: vec![[0.0; 2]; 3],
            normals: vec![[0.0, 0.0, 1.0]; 3],
            tangents: vec![[1.0, 0.0, 0.0]; 3],
            bitangents: vec![[0.0, 1.0, 0.0]; 3],
            colors: None,
            triangles: vec![[0, 1, 2]],
            segments: vec![Segment { flags: 0, start: 0, count: 1 }],
        }
    }

    #[test]
    fn layout_bounds_and_shared_texture_sets() {
        let q = Quad { level: 4, x: 0, y: -16 };
        let nif = assemble(&q, &[shape("obj-LargeRef", true, "textures\\a.dds"), shape("obj", false, "textures\\a.dds"), shape("objSnow", false, "textures\\b.dds")]);
        let types: Vec<&str> = (0..nif.blocks.len()).map(|i| nif.block_type(i)).collect();
        assert_eq!(
            types,
            [
                "NiNode",
                "BSMultiBoundNode",
                "BSSubIndexTriShape",
                "BSDistantObjectLargeRefExtraData",
                "BSLightingShaderProperty",
                "BSShaderTextureSet",
                "BSMultiBound",
                "BSMultiBoundAABB",
                "BSMultiBoundNode",
                "BSSubIndexTriShape",
                "BSLightingShaderProperty",
                "BSMultiBound",
                "BSMultiBoundAABB",
                "BSMultiBoundNode",
                "BSSubIndexTriShape",
                "BSLightingShaderProperty",
                "BSShaderTextureSet",
                "BSMultiBound",
                "BSMultiBoundAABB",
            ]
        );
        let Block::Node(root) = &nif.blocks[0] else { panic!() };
        assert_eq!(root.children, [Some(1), Some(8), Some(13)]);
        let Block::Node(mb) = &nif.blocks[1] else { panic!() };
        assert_eq!((mb.culling_mode, mb.multi_bound, &mb.children[..]), (Some(1), Some(6), &[Some(2)][..]));
        let Block::TriShape(s) = &nif.blocks[2] else { panic!() };
        assert_eq!(s.av.transform.translation, [0.0, -65536.0, 0.0]);
        assert_eq!(s.av.transform.scale, 4.0);
        assert_eq!(s.av.net.extra_data, [Some(3)]);
        assert_eq!(s.bound_center, [50.0, 50.0, 10.0]);
        assert!((s.bound_radius - (50f32.powi(2) * 2.0 + 20f32.powi(2)).sqrt()).abs() < 1e-3);
        assert_eq!(s.vertex_desc, 0x0001_b000_0065_0407);
        let Block::LightingShader(l) = &nif.blocks[4] else { panic!() };
        assert_eq!(l.texture_set, Some(5));
        let Block::LightingShader(l2) = &nif.blocks[10] else { panic!() };
        assert_eq!(l2.texture_set, Some(5));
        let Block::MultiBoundAabb { position, extent } = &nif.blocks[7] else { panic!() };
        assert_eq!(position, &[200.0, -65536.0 + 200.0, 40.0]);
        assert_eq!(extent, &[200.0, 200.0, 80.0]);
        let bytes = nif.to_bytes().unwrap();
        let back = lodforge_nif::parse(&bytes).unwrap();
        assert_eq!(back.header.strings, ["obj", "obj-LargeRef", "DOLRED", "objSnow"]);
    }
}
