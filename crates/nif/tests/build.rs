//! Writer tests that need no golden data: building a small file from scratch, field round trips
//! and rejection of inconsistent input.

use lodforge_nif::{
    AvObject, Block, LightingExtra, LightingShader, Nif, Node, ObjectNet, Segment, Transform,
    TriShape, VertexDesc, parse, va,
};

fn quad(z: f32) -> TriShape {
    let positions = vec![
        [0.0, 0.0, z],
        [1024.0, 0.0, z],
        [0.0, 1024.0, z],
        [1024.0, 1024.0, z + 10.0],
    ];
    let uvs = positions
        .iter()
        .map(|p| [p[0] / 4096.0, 1.0 - p[1] / 4096.0])
        .collect();
    let mut s = TriShape {
        av: AvObject {
            net: ObjectNet::named("land"),
            transform: Transform {
                scale: 4.0,
                ..Default::default()
            },
            ..Default::default()
        },
        bound_center: [512.0, 512.0, z + 5.0],
        bound_radius: 724.1,
        positions,
        uvs,
        triangles: vec![[0, 1, 2], [2, 1, 3]],
        ..Default::default()
    };
    s.update_vertex_desc();
    s
}

#[test]
fn build_file_from_scratch() {
    let mut nif = Nif::new();
    nif.add_block(Block::Node(Node {
        av: AvObject {
            net: ObjectNet::named("chunk"),
            ..Default::default()
        },
        children: vec![Some(1), Some(4)],
        multi_bound: Some(6),
        culling_mode: Some(1),
        ..Default::default()
    }));
    let mut land = quad(100.0);
    land.shader_property = Some(2);
    nif.add_block(Block::TriShape(land.clone()));
    nif.add_block(Block::LightingShader(LightingShader {
        shader_type: 18,
        texture_set: Some(3),
        flags1: 0x8040_1000,
        flags2: 3,
        texture_clamp_mode: 0,
        glossiness: 1.0,
        lighting_effect_1: 0.0,
        lighting_effect_2: 0.0,
        ..Default::default()
    }));
    nif.add_block(Block::TextureSet(vec![
        r"Data\Textures\Terrain\Tamriel\Tamriel.4.0.0.dds".into(),
        r"Data\Textures\Terrain\Tamriel\Tamriel.4.0.0_n.dds".into(),
    ]));
    let mut water = TriShape {
        positions: vec![[0.0, 0.0, -5.0], [512.0, 0.0, -5.0], [0.0, 512.0, -5.0]],
        triangles: vec![[0, 1, 2]],
        segments: Some(vec![Segment {
            flags: 0,
            start: 0,
            count: 1,
        }]),
        ..Default::default()
    };
    water.update_vertex_desc();
    nif.add_block(Block::Node(Node {
        av: AvObject {
            net: ObjectNet::named("WATER"),
            ..Default::default()
        },
        children: vec![Some(5)],
        multi_bound: Some(8),
        culling_mode: Some(0),
        ..Default::default()
    }));
    nif.add_block(Block::TriShape(water.clone()));
    nif.add_block(Block::MultiBound { data: Some(7) });
    nif.add_block(Block::MultiBoundAabb {
        position: [2048.0, 2048.0, 420.0],
        extent: [2048.0, 2048.0, 20.0],
    });
    nif.add_block(Block::MultiBound { data: Some(9) });
    nif.add_block(Block::MultiBoundAabb {
        position: [1024.0, 1024.0, -20.0],
        extent: [1024.0, 1024.0, 0.0],
    });

    let bytes = nif.to_bytes().unwrap();
    let back = parse(&bytes).unwrap();
    assert!(back.raw_blocks.is_empty());
    let types: Vec<&str> = (0..back.blocks.len()).map(|i| back.block_type(i)).collect();
    assert_eq!(
        types,
        [
            "BSMultiBoundNode",
            "BSTriShape",
            "BSLightingShaderProperty",
            "BSShaderTextureSet",
            "BSMultiBoundNode",
            "BSSubIndexTriShape",
            "BSMultiBound",
            "BSMultiBoundAABB",
            "BSMultiBound",
            "BSMultiBoundAABB"
        ]
    );
    assert_eq!(
        back.header.block_types,
        [
            "BSMultiBoundNode",
            "BSTriShape",
            "BSLightingShaderProperty",
            "BSShaderTextureSet",
            "BSSubIndexTriShape",
            "BSMultiBound",
            "BSMultiBoundAABB"
        ]
    );
    assert_eq!(back.header.strings, ["chunk", "land", "WATER"]);
    assert_eq!(back.header.author, lodforge_nif::CREATOR);
    assert_eq!(back.roots, [Some(0)]);
    assert_eq!(back.blocks, nif.blocks);
    let Block::TriShape(s) = &back.blocks[1] else {
        panic!()
    };
    assert_eq!(s.vertex_desc, 0x0000_3000_0000_0405);
    assert_eq!((s.av.flags, s.av.transform.scale), (14, 4.0));
    let Block::TriShape(w) = &back.blocks[5] else {
        panic!()
    };
    assert_eq!(w.vertex_desc, VertexDesc::sse(va::VERTEX).0);
    assert_eq!(back.shape_textures(s).len(), 2);
    // Re-writing is stable.
    assert_eq!(back.to_bytes().unwrap(), bytes);
}

fn single_block_round_trip(block: Block) -> Block {
    let mut nif = Nif::new();
    nif.add_block(block);
    let bytes = nif.to_bytes().unwrap();
    let parsed = parse(&bytes).unwrap();
    assert!(parsed.raw_blocks.is_empty());
    assert_eq!(parsed.to_bytes().unwrap(), bytes);
    parsed.blocks.into_iter().next().unwrap()
}

#[test]
fn lighting_shader_tails_round_trip() {
    for (t, extra) in [
        (0, LightingExtra::None),
        (1, LightingExtra::EnvironmentMap { scale: 0.75 }),
        (5, LightingExtra::SkinTint([0.1, 0.2, 0.3])),
        (6, LightingExtra::HairTint([0.4, 0.5, 0.6])),
        (
            7,
            LightingExtra::ParallaxOcclusion {
                max_passes: 4.0,
                scale: 0.5,
            },
        ),
        (
            11,
            LightingExtra::MultiLayerParallax {
                inner_layer_thickness: 1.0,
                refraction_scale: 2.0,
                inner_layer_texture_scale: [3.0, 4.0],
                envmap_strength: 5.0,
            },
        ),
        (14, LightingExtra::SparkleSnow([1.0, 2.0, 3.0, 4.0])),
        (
            16,
            LightingExtra::EyeEnvironment {
                cubemap_scale: 1.5,
                left: [1.0, 2.0, 3.0],
                right: [4.0, 5.0, 6.0],
            },
        ),
        (18, LightingExtra::None),
    ] {
        let s = LightingShader {
            shader_type: t,
            net: ObjectNet::named("shader"),
            extra,
            emissive_color: [0.5, 0.25, 0.125],
            ..Default::default()
        };
        assert_eq!(
            single_block_round_trip(Block::LightingShader(s.clone())),
            Block::LightingShader(s)
        );
    }
}

#[test]
fn effect_shader_and_alpha_round_trip() {
    let e = lodforge_nif::EffectShader {
        net: ObjectNet::named("fx"),
        flags1: 0x8000_0000,
        flags2: 0x20,
        uv_scale: [1.0, 1.0],
        source_texture: "textures\\a.dds".into(),
        texture_clamp_mode: 3,
        lighting_influence: 255,
        falloff: [1.0, 1.0, 0.0, 0.0],
        emissive_color: [1.0; 4],
        emissive_multiple: 1.0,
        soft_falloff_depth: 100.0,
        ..Default::default()
    };
    assert_eq!(
        single_block_round_trip(Block::EffectShader(e.clone())),
        Block::EffectShader(e)
    );
    let a = Block::Alpha {
        net: ObjectNet::default(),
        flags: 4844,
        threshold: 128,
    };
    assert_eq!(single_block_round_trip(a.clone()), a);
    let l = Block::LargeRefExtra {
        name: Some("DOLRD".into()),
        large_ref: 1,
    };
    assert_eq!(single_block_round_trip(l.clone()), l);
}

#[test]
fn full_vertex_round_trip() {
    let n = 3;
    let q = |v: f32| lodforge_nif::byte_to_float(lodforge_nif::float_to_byte(v));
    let mut s = TriShape {
        positions: (0..n).map(|i| [i as f32, 2.0 * i as f32, -1.5]).collect(),
        uvs: (0..n)
            .map(|i| [i as f32 * 0.25, 1.0 - i as f32 * 0.125])
            .collect(),
        normals: (0..n)
            .map(|i| [lodforge_nif::byte_to_float(i as u8 * 40), q(0.0), 1.0])
            .collect(),
        tangents: (0..n).map(|_| [1.0, q(0.0), q(-0.3)]).collect(),
        bitangents: (0..n)
            .map(|i| [0.5 * i as f32, lodforge_nif::byte_to_float(7), -1.0])
            .collect(),
        colors: (0..n).map(|i| [i as u8, 10, 20, 255]).collect(),
        triangles: vec![[0, 1, 2]],
        ..Default::default()
    };
    s.update_vertex_desc();
    assert_eq!(VertexDesc(s.vertex_desc).stride(), 32);
    let Block::TriShape(back) = single_block_round_trip(Block::TriShape(s.clone())) else {
        panic!()
    };
    assert_eq!(back, s);
}

#[test]
fn rejects_inconsistent_shapes() {
    let mut s = TriShape {
        positions: vec![[0.0; 3]; 3],
        uvs: vec![[0.0; 2]; 3],
        triangles: vec![[0, 1, 2]],
        ..Default::default()
    };
    // Desc without the UV flag while UVs are present.
    s.vertex_desc = VertexDesc::sse(va::VERTEX).0;
    let mut nif = Nif::new();
    nif.add_block(Block::TriShape(s.clone()));
    assert!(nif.to_bytes().is_err());
    // Out-of-range index.
    s.update_vertex_desc();
    s.triangles = vec![[0, 1, 3]];
    nif.blocks[0] = Block::TriShape(s);
    assert!(nif.to_bytes().is_err());
    // Shader type 1 needs its env map scale.
    nif.blocks[0] = Block::LightingShader(LightingShader {
        shader_type: 1,
        ..Default::default()
    });
    assert!(nif.to_bytes().is_err());
    // Skinned data cannot be encoded.
    let mut skinned = TriShape::default();
    skinned.vertex_desc = u64::from(va::SKINNED) << 44 | 3;
    nif.blocks[0] = Block::TriShape(skinned);
    assert!(nif.to_bytes().is_err());
    // Unknown block without raw bytes.
    nif.blocks[0] = Block::Unknown;
    assert!(nif.to_bytes().is_err());
}

#[test]
fn unknown_blocks_are_kept_verbatim() {
    let mut nif = Nif::new();
    nif.add_block(Block::Node(Node {
        av: AvObject {
            net: ObjectNet::named("Scene Root"),
            ..Default::default()
        },
        ..Default::default()
    }));
    nif.add_block(Block::Unknown);
    nif.header.block_types = vec!["BSFadeNode".into(), "NiStringExtraData".into()];
    nif.header.block_type_index = vec![0, 1];
    // NiStringExtraData: name string index -1, string data "abc".
    nif.raw_blocks.insert(
        1,
        [
            (-1i32).to_le_bytes().as_slice(),
            &3u32.to_le_bytes(),
            b"abc",
        ]
        .concat(),
    );
    let bytes = nif.to_bytes().unwrap();
    let back = parse(&bytes).unwrap();
    assert_eq!(
        (back.block_type(0), back.block_type(1)),
        ("BSFadeNode", "NiStringExtraData")
    );
    assert!(matches!(back.blocks[1], Block::Unknown));
    assert_eq!(back.raw_blocks.keys().collect::<Vec<_>>(), [&1]);
    assert_eq!(back.to_bytes().unwrap(), bytes);
}
