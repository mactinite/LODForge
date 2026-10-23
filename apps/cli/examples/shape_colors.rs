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
    let mut tri_counts: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut uv_sum: BTreeMap<(String, String), (f64, f64, f64)> = BTreeMap::new();
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
            *tri_counts.entry((tex.clone(), name.clone())).or_insert(0) += s.triangles.len();
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
            let Some((m0, m3)) = img else {
                // Texture not on disk: still list the shape (flags and vertex colours).
                let vc = l.flags2 & (1 << 5) != 0 && !s.colors.is_empty();
                let e = groups.entry((format!("{tex} [not found; flags2 {:#x}]", l.flags2), name)).or_insert((0, 0.0, 0.0, 0.0, 0, [0.0; 3]));
                e.0 += 1;
                if vc {
                    e.3 += s.colors.iter().map(|c| luma([f64::from(c[0]), f64::from(c[1]), f64::from(c[2])])).sum::<f64>() / s.colors.len() as f64;
                    e.4 += 1;
                }
                continue;
            };
            let (mut u0, mut v0, mut u1, mut v1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
            for uv in &s.uvs {
                u0 = u0.min(uv[0]);
                v0 = v0.min(uv[1]);
                u1 = u1.max(uv[0]);
                v1 = v1.max(uv[1]);
            }
            // Mean over the texels the triangles cover: 10 barycentric samples per triangle,
            // weighted by the triangle's world area (an atlased merged shape's UV bounding box
            // would span the whole page).
            let mean = |im: &lodforge_dds::Image| -> [f64; 5] {
                let (w, h) = (im.width as f32, im.height as f32);
                let mut acc = [0f64; 5];
                let mut n = 0f64;
                let _ = (u0, v0, u1, v1);
                for tri in &s.triangles {
                    let p: Vec<[f32; 3]> = tri.iter().map(|&i| s.positions[i as usize]).collect();
                    let u: Vec<[f32; 2]> = tri.iter().map(|&i| s.uvs[i as usize]).collect();
                    let e1 = [p[1][0] - p[0][0], p[1][1] - p[0][1], p[1][2] - p[0][2]];
                    let e2 = [p[2][0] - p[0][0], p[2][1] - p[0][1], p[2][2] - p[0][2]];
                    let c = [e1[1] * e2[2] - e1[2] * e2[1], e1[2] * e2[0] - e1[0] * e2[2], e1[0] * e2[1] - e1[1] * e2[0]];
                    let area = f64::from((c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt());
                    if area <= 0.0 {
                        continue;
                    }
                    let wgt = area / 10.0;
                    for (a, b) in [(0.0, 0.0), (1.0, 0.0), (0.0, 1.0), (0.5, 0.0), (0.0, 0.5), (0.5, 0.5), (0.33, 0.33), (0.25, 0.25), (0.5, 0.25), (0.25, 0.5)] {
                        let (a, b, g): (f32, f32, f32) = (a, b, 1.0 - a - b);
                        let uu = g * u[0][0] + a * u[1][0] + b * u[2][0];
                        let vv = g * u[0][1] + a * u[1][1] + b * u[2][1];
                        let x = ((uu.rem_euclid(1.0)) * w) as u32 % im.width.max(1);
                        let y = ((vv.rem_euclid(1.0)) * h) as u32 % im.height.max(1);
                        let i = ((y * im.width + x) * 4) as usize;
                        for k in 0..3 {
                            acc[k] += f64::from(im.rgba[i + k]) * wgt;
                        }
                        acc[3] += f64::from(uu.rem_euclid(1.0)) * wgt;
                        acc[4] += f64::from(vv.rem_euclid(1.0)) * wgt;
                        n += wgt;
                    }
                }
                if n <= 0.0 {
                    return [0.0; 5];
                }
                [acc[0] / n, acc[1] / n, acc[2] / n, acc[3] / n, acc[4] / n]
            };
            let m = mean(m0);
            let c0 = [m[0], m[1], m[2]];
            let m3v = mean(m3);
            let c3 = [m3v[0], m3v[1], m3v[2]];
            uv_sum.entry((tex.clone(), name.clone())).and_modify(|e| { e.0 += m[3]; e.1 += m[4]; e.2 += 1.0; }).or_insert((m[3], m[4], 1.0));
            let vc = l.flags2 & (1 << 5) != 0 && !s.colors.is_empty();
            let vcl = if vc { s.colors.iter().map(|c| luma([f64::from(c[0]), f64::from(c[1]), f64::from(c[2])])).sum::<f64>() / s.colors.len() as f64 } else { 0.0 };
            if vc && std::env::var_os("VC_HIST").is_some() {
                // Vertex colour histogram (red channel, 0..255 in buckets of 8).
                let mut h = [0usize; 32];
                for c in &s.colors {
                    h[(f64::from(c[0]) * if f64::from(c[0]) <= 1.0 { 255.0 } else { 1.0 }).round().clamp(0.0, 255.0) as usize / 8] += 1;
                }
                let lo = s.colors.iter().map(|c| f64::from(c[0])).fold(f64::MAX, f64::min);
                let hi = s.colors.iter().map(|c| f64::from(c[0])).fold(f64::MIN, f64::max);
                println!("  vc {name}: min {lo:.3} max {hi:.3} buckets(/8) {:?}", h.iter().enumerate().filter(|(_, n)| **n > 0).map(|(i, n)| (i * 8, *n)).collect::<Vec<_>>());
            }
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
    println!("{:>3} {:>8} {:>7} {:>7} {:>7} {:>8}  {:18} texture", "n", "tris", "lum0", "lum3", "vclum", "rgb0", "shape");
    for ((tex, name), (n, l0, l3, vl, nvc, rgb)) in &groups {
        let nf = *n as f64;
        let vcs = if *nvc > 0 { format!("{:7.1}", vl / *nvc as f64) } else { "      -".into() };
        let tris = tri_counts.get(&(tex.clone(), name.clone())).copied().unwrap_or(0);
        let uv = uv_sum.get(&(tex.clone(), name.clone())).map(|(u, v, k)| format!("uv {:.3},{:.3}", u / k, v / k)).unwrap_or_default();
        println!("{n:>3} {tris:>8} {:7.1} {:7.1} {vcs} ({:3.0},{:3.0},{:3.0})  {:18} {tex}  {uv}", l0 / nf, l3 / nf, rgb[0] / nf, rgb[1] / nf, rgb[2] / nf, name.chars().take(18).collect::<String>());
    }
    Ok(())
}
