//! Texture density of the shapes in object LOD files: for each shape, its first texture, the
//! texels its UVs cover on that texture (page size from the DDS header) and its world size, as
//! texels per 100 world units. Shapes are grouped by texture and name (billboard vs 3D).
//!
//! `cargo run --release --example bto_texels -- <Data dir> <file.bto>...`

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::Result;
use lodforge_nif::Block;

fn dds_size(data: &std::path::Path, tex: &str) -> Option<(u32, u32)> {
    let p = data.join(tex.replace('\\', "/"));
    let b = std::fs::read(&p).ok()?;
    let i = lodforge_dds::parse(&b).ok()?;
    Some((i.width, i.height))
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let data = PathBuf::from(&args[0]);
    // (texture, kind) -> (shapes, sum of texels per 100 units, min, max)
    let mut groups: BTreeMap<(String, String), (usize, f64, f64, f64)> = BTreeMap::new();
    for f in &args[1..] {
        let bytes = std::fs::read(f)?;
        let nif = lodforge_nif::parse(&bytes)?;
        for b in &nif.blocks {
            let Block::TriShape(s) = b else { continue };
            let Some(Block::LightingShader(l)) = nif.get(s.shader_property) else { continue };
            let Some(Block::TextureSet(t)) = nif.get(l.texture_set) else { continue };
            let tex = t.first().cloned().unwrap_or_default().to_lowercase().trim_start_matches("data\\").to_string();
            if s.uvs.is_empty() || s.positions.is_empty() {
                continue;
            }
            let tex = if tex.starts_with("textures") { tex } else { format!("textures/{tex}") };
            let Some((pw, ph)) = dds_size(&data, &tex) else {
                eprintln!("no texture {tex}");
                continue;
            };
            // Per triangle: sqrt(texel area / world area), averaged by world area.
            let (mut wsum, mut asum) = (0f64, 0f64);
            for tri in &s.triangles {
                let p: Vec<[f32; 3]> = tri.iter().map(|&i| s.positions[i as usize]).collect();
                let u: Vec<[f32; 2]> = tri.iter().map(|&i| s.uvs[i as usize]).collect();
                let e1 = [p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2]];
                let e2 = [p[2][0] - p[0][0], p[2][1] - p[0][1], p[2][2] - p[0][2]];
                let c = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
                let wa = 0.5 * f64::from((c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt());
                let ua = 0.5 * f64::from(((u[1][0] - u[0][0]) * pw as f32 * (u[2][1] - u[0][1]) * ph as f32 - (u[2][0] - u[0][0]) * pw as f32 * (u[1][1] - u[0][1]) * ph as f32).abs());
                if wa > 1e-3 {
                    wsum += (ua / wa).sqrt() * 100.0 * wa;
                    asum += wa;
                }
            }
            if asum <= 0.0 {
                continue;
            }
            let d = wsum / asum;
            let name = s.av.net.name.clone().unwrap_or_default().to_lowercase();
            let kind = if name.contains("billboard") || l.flags2 & 0x0000_0010 != 0 && tex.contains("alpha") { "billboard/alpha".to_string() } else { "3d".to_string() };
            let e = groups.entry((tex, kind)).or_insert((0, 0.0, f64::MAX, 0.0));
            e.0 += 1;
            e.1 += d;
            e.2 = e.2.min(d);
            e.3 = e.3.max(d);
        }
    }
    for ((tex, kind), (n, sum, lo, hi)) in &groups {
        println!("{:>5} shapes  {:16}  texels/100u mean {:6.2} (min {:6.2}, max {:6.2})  {tex}", n, kind, sum / *n as f64, lo, hi);
    }
    Ok(())
}
