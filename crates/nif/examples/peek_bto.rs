//! Prints the shapes of a BTO (object LOD) file: name, counts, vertex desc, segments, shader and
//! textures, bounds. `cargo run -p lodforge-nif --example peek_bto -- <file.bto>`

use lodforge_nif::{Block, parse};

fn main() {
    let path = std::env::args().nth(1).expect("usage: peek_bto <file>");
    let nif = parse(&std::fs::read(&path).unwrap()).unwrap();
    println!("{} blocks, strings {:?}", nif.blocks.len(), nif.header.strings);
    for (i, b) in nif.blocks.iter().enumerate() {
        match b {
            Block::Node(n) => println!("[{i}] {} name={:?} children={:?} mb={:?} cull={:?} flags={} t={:?} s={}", nif.block_type(i), n.av.net.name, n.children, n.multi_bound, n.culling_mode, n.av.flags, n.av.transform.translation, n.av.transform.scale),
            Block::TriShape(s) => {
                println!(
                    "[{i}] {} name={:?} flags={} t={:?} s={} v={} t={} desc={:#018x} center={:?} r={} shader={:?} alpha={:?} extra={:?}",
                    nif.block_type(i),
                    s.av.net.name,
                    s.av.flags,
                    s.av.transform.translation,
                    s.av.transform.scale,
                    s.positions.len(),
                    s.triangles.len(),
                    s.vertex_desc,
                    s.bound_center,
                    s.bound_radius,
                    s.shader_property,
                    s.alpha_property,
                    s.av.net.extra_data
                );
                let non_white = s.colors.iter().filter(|c| c[0] != 255 || c[1] != 255 || c[2] != 255).count();
                let (amin, amax) = s.colors.iter().fold((255u8, 0u8), |(lo, hi), v| (lo.min(v[3]), hi.max(v[3])));
                println!("      uv={} n={} tan={} bitan={} col={} non_white={} alpha={}..{} segs={:?}", s.uvs.len(), s.normals.len(), s.tangents.len(), s.bitangents.len(), s.colors.len(), non_white, amin, amax, s.segments);
            }
            Block::LightingShader(l) => println!("[{i}] lighting type={} name={:?} f1={:#x} f2={:#x} ts={:?} clamp={} gloss={} le={} {} spec={:?} {} emis={:?} {} uv={:?}/{:?}", l.shader_type, l.net.name, l.flags1, l.flags2, l.texture_set, l.texture_clamp_mode, l.glossiness, l.lighting_effect_1, l.lighting_effect_2, l.specular_color, l.specular_strength, l.emissive_color, l.emissive_multiple, l.uv_offset, l.uv_scale),
            Block::TextureSet(t) => println!("[{i}] textures {t:?}"),
            Block::MultiBound { data } => println!("[{i}] multibound {data:?}"),
            Block::MultiBoundAabb { position, extent } => println!("[{i}] aabb {position:?} {extent:?}"),
            Block::LargeRefExtra { name, large_ref } => println!("[{i}] largeref {name:?} {large_ref}"),
            other => println!("[{i}] {} {:?}", nif.block_type(i), std::mem::discriminant(other)),
        }
    }
}
