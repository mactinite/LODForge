//! Mean colour each object LOD shape samples: for every shape of the given `.bto` files, its first
//! texture, the mean sRGB colour of the texels inside the shape's UV bounding box (top mip and
//! mip 3), the mean vertex colour when the shader uses vertex colours, and the product the game
//! roughly shows. Grouped by texture and shape name, so the same cliff can be compared between
//! two outputs or between LOD levels.
//!
//! `cargo run --release --example shape_colors -- <Data dir> [--match <substring>] <file.bto>...`
use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::Result;
use lodforge_nif::Block;

fn luma(c: [f64; 3]) -> f64 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

fn main() -> Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let data = PathBuf::from(args.remove(0));
    let mut pat = String::new();
    if let Some(i) = args.iter().position(|a| a == "--match") {
        args.remove(i);
        pat = args.remove(i).to_lowercase();
    }
    let mut textures: BTreeMap<String, Option<(lodforge_dds::Image, lodforge_dds::Image)>> = BTreeMap::new();
    // (texture, name) -> (shapes, sum tex lum mip0, sum tex lum mip3, sum vc lum, vc shapes, sum rgb0)
    let mut groups: BTreeMap<(String, String), (usize, f64, f64, f64, usize, [f64; 3])> = BTreeMap::new();
    for f in &args {
        let bytes = std::fs::read(f)?;
        let nif = lodforge_nif::parse(&bytes)?;
        for b in &nif.blocks {
            let Block::TriShape(s) = b else { continue };
            let Some(Block::LightingShader(l)) = nif.get(s.shader_property) else { continue };
            let Some(Block::TextureSet(t)) = nif.get(l.texture_set) else { continue };
            let tex = t.first().cloned().unwrap_or_default().to_lowercase().replace('/', "\\");
            let tex = tex.trim_start_matches("data\\").to_string();
            let tex = if tex.starts_with("textures") { tex } else { format!("textures\\{tex}") };
            let name = s.av.net.name.clone().unwrap_or_default();
            if !pat.is_empty() && !name.to_lowercase().contains(&pat) && !tex.contains(&pat) {
                continue;
            }
            if s.uvs.is_empty() {
                continue;
            }
            let img = textures.entry(tex.clone()).or_insert_with(|| {
                // `root1;root2`: the first root that has the file wins.
                let b = data.to_string_lossy().split(';').map(|r| PathBuf::from(r).join(tex.replace('\\', "/"))).find_map(|p| std::fs::read(p).ok())?;
                let i = lodforge_dds::parse(&b).ok()?;
                let m0 = lodforge_dds::decode(&b, &i, 0).ok()?;
                let m3 = lodforge_dds::decode(&b, &i, 3.min(i.mip_count.saturating_sub(1))).ok()?;
                Some((m0, m3))
            });
            let Some((m0, m3)) = img else { continue };
            let (mut u0, mut v0, mut u1, mut v1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
            for uv in &s.uvs {
                u0 = u0.min(uv[0]);
                v0 = v0.min(uv[1]);
                u1 = u1.max(uv[0]);
                v1 = v1.max(uv[1]);
            }
            let mean = |im: &lodforge_dds::Image| -> [f64; 3] {
                let (w, h) = (im.width as f32, im.height as f32);
                let (x0, y0) = (((u0.clamp(0.0, 1.0)) * w) as u32, ((v0.clamp(0.0, 1.0)) * h) as u32);
                let (x1, y1) = (((u1.clamp(0.0, 1.0)) * w).ceil() as u32, ((v1.clamp(0.0, 1.0)) * h).ceil() as u32);
                let (x1, y1) = (x1.clamp(x0 + 1, im.width), y1.clamp(y0 + 1, im.height));
                let mut s = [0f64; 3];
                let mut n = 0f64;
                for y in y0..y1 {
                    for x in x0..x1 {
                        let p = ((y * im.width + x) * 4) as usize;
                        for c in 0..3 {
                            s[c] += f64::from(im.rgba[p + c]);
                        }
                        n += 1.0;
                    }
                }
                [s[0] / n, s[1] / n, s[2] / n]
            };
            let c0 = mean(m0);
            let c3 = mean(m3);
            let vc = l.flags2 & (1 << 5) != 0 && !s.colors.is_empty();
            let vcl = if vc { s.colors.iter().map(|c| luma([f64::from(c[0]), f64::from(c[1]), f64::from(c[2])])).sum::<f64>() / s.colors.len() as f64 } else { 0.0 };
            let e = groups.entry((tex.clone(), name)).or_insert((0, 0.0, 0.0, 0.0, 0, [0.0; 3]));
            e.0 += 1;
            e.1 += luma(c0);
            e.2 += luma(c3);
            if vc {
                e.3 += vcl;
                e.4 += 1;
            }
            for c in 0..3 {
                e.5[c] += c0[c];
            }
        }
    }
    println!("{:>3} {:>7} {:>7} {:>7} {:>8}  {:18} texture", "n", "lum0", "lum3", "vclum", "rgb0", "shape");
    for ((tex, name), (n, l0, l3, vl, nvc, rgb)) in &groups {
        let nf = *n as f64;
        let vcs = if *nvc > 0 { format!("{:7.1}", vl / *nvc as f64) } else { "      -".into() };
        println!("{n:>3} {:7.1} {:7.1} {vcs} ({:3.0},{:3.0},{:3.0})  {:18} {tex}", l0 / nf, l3 / nf, rgb[0] / nf, rgb[1] / nf, rgb[2] / nf, name.chars().take(18).collect::<String>());
    }
    Ok(())
}
