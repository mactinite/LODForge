//! Per-attribute differences between two BTOs of the same quad: vertices are paired by shape
//! name + first texture and exact world position; for the pairs, the byte deltas of normals,
//! tangents, bitangents, the half-float UV deltas and the colour deltas are histogrammed.
//! `cargo run -p lodforge-nif --example bto_attr_diff -- <golden.bto> <ours.bto>`

use std::collections::{BTreeMap, HashMap};

use lodforge_nif::{Block, Nif, f32_to_half, float_to_byte, parse};

type Attr = ([u8; 3], [u8; 3], [u8; 3], f32, [u16; 2], [u8; 4]);

fn vertices(nif: &Nif) -> BTreeMap<String, HashMap<[u32; 3], Attr>> {
    let mut out: BTreeMap<String, HashMap<[u32; 3], Attr>> = BTreeMap::new();
    for b in &nif.blocks {
        let Block::TriShape(s) = b else { continue };
        let name = s.av.net.name.clone().unwrap_or_default();
        let tex = nif.shape_textures(s).first().cloned().unwrap_or_default().to_lowercase();
        let key = format!("{name} {tex}");
        let t = &s.av.transform;
        let map = out.entry(key).or_default();
        for (i, p) in s.positions.iter().enumerate() {
            let w = [p[0] * t.scale + t.translation[0], p[1] * t.scale + t.translation[1], p[2] * t.scale + t.translation[2]];
            let n = s.normals.get(i).map(|n| n.map(float_to_byte)).unwrap_or([0; 3]);
            let tg = s.tangents.get(i).map(|n| n.map(float_to_byte)).unwrap_or([0; 3]);
            let bt = s.bitangents.get(i).copied().unwrap_or([0.0; 3]);
            let bb = [float_to_byte(bt[0]), float_to_byte(bt[1]), float_to_byte(bt[2])];
            let uv = s.uvs.get(i).map(|u| [f32_to_half(u[0]), f32_to_half(u[1])]).unwrap_or([0; 2]);
            let c = s.colors.get(i).copied().unwrap_or([0; 4]);
            map.entry(w.map(f32::to_bits)).or_insert((n, tg, bb, bt[0], uv, c));
        }
    }
    out
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let g = vertices(&parse(&std::fs::read(&a[0]).unwrap()).unwrap());
    let o = vertices(&parse(&std::fs::read(&a[1]).unwrap()).unwrap());
    for (key, gv) in &g {
        let Some(ov) = o.get(key) else {
            println!("{key}: only golden ({} vertices)", gv.len());
            continue;
        };
        let mut paired = 0usize;
        let mut hist: [BTreeMap<i32, usize>; 3] = Default::default();
        let (mut uv_diff, mut col_diff, mut bx_max) = (0usize, 0usize, 0f32);
        let mut example = None;
        for (pos, ga) in gv {
            let Some(oa) = ov.get(pos) else { continue };
            paired += 1;
            for (k, (gb, ob)) in [(&ga.0, &oa.0), (&ga.1, &oa.1), (&ga.2, &oa.2)].into_iter().enumerate() {
                let d = (0..3).map(|i| (i32::from(gb[i]) - i32::from(ob[i])).abs()).max().unwrap();
                *hist[k].entry(d).or_default() += 1;
                if d > 2 && example.is_none() {
                    example = Some(format!("pos {:?} golden {:?} ours {:?}", pos.map(f32::from_bits), ga, oa));
                }
            }
            if ga.4 != oa.4 {
                uv_diff += 1;
                if uv_diff <= 2 {
                    println!("   uv differs at {:?}: golden {:?} ours {:?}", pos.map(f32::from_bits), ga.4.map(lodforge_nif::half_to_f32), oa.4.map(lodforge_nif::half_to_f32));
                }
            }
            if ga.5 != oa.5 {
                col_diff += 1;
                if col_diff <= 2 {
                    println!("   colour differs at {:?}: golden {:?} ours {:?}", pos.map(f32::from_bits), ga.5, oa.5);
                }
            }
            bx_max = bx_max.max((ga.3 - oa.3).abs());
        }
        println!("{key}: golden {} ours {} paired by position {paired}; uv differ {uv_diff}, colour differ {col_diff}, bitangent-x max |d| {bx_max:.4}", gv.len(), ov.len());
        for (k, n) in ["normal", "tangent", "bitangent"].iter().enumerate() {
            println!("   {n:9} max byte delta histogram: {:?}", hist[k]);
        }
        if let Some(e) = example {
            println!("   e.g. {e}");
        }
    }
    for key in o.keys().filter(|k| !g.contains_key(*k)) {
        println!("{key}: only ours ({} vertices)", o[key].len());
    }
}
