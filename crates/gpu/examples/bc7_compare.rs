//! Compares the GPU port of the ISPC BC7 encoder with the CPU encoder, block by block.
//!
//! `cargo run --release -p lodforge-gpu --example bc7_compare -- [--best] [--mips] [--limit N] <dds or dir>...`
//!
//! Every input DDS is decoded (top mip, or every mip with --mips) and encoded by both. Per file:
//! blocks, identical blocks, and for the differing blocks the summed squared error against the
//! source of each encoder; at the end the totals and the encode times.

use std::path::{Path, PathBuf};
use std::time::Instant;

use lodforge_gpu::{Bc7Quality, Gpu, RgbaImage, bc7_cpu};

fn decode_blocks(w: u32, h: u32, blocks: &[u8]) -> Vec<u8> {
    let dds = lodforge_dds::write_bc7(w, h, &[blocks.to_vec()]);
    let info = lodforge_dds::parse(&dds).unwrap();
    lodforge_dds::decode(&dds, &info, 0).unwrap().rgba
}

/// Squared error of one 4x4 block of `a` against `src`.
fn block_err(w: u32, h: u32, src: &[u8], a: &[u8], bx: u32, by: u32) -> u64 {
    let mut e = 0u64;
    for y in by * 4..(by * 4 + 4).min(h) {
        for x in bx * 4..(bx * 4 + 4).min(w) {
            let i = ((y * w + x) * 4) as usize;
            for c in 0..4 {
                let d = i64::from(src[i + c]) - i64::from(a[i + c]);
                e += (d * d) as u64;
            }
        }
    }
    e
}

fn collect(p: &Path, out: &mut Vec<PathBuf>) {
    if p.is_dir() {
        let mut v: Vec<PathBuf> = std::fs::read_dir(p).unwrap().flatten().map(|e| e.path()).collect();
        v.sort();
        for q in v {
            collect(&q, out);
        }
    } else if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("dds")) {
        out.push(p.to_path_buf());
    }
}

fn main() {
    let mut quality = Bc7Quality::High;
    let mut mips = false;
    let mut limit = usize::MAX;
    let mut show = 0usize;
    let mut shown = 0usize;
    let mut oracle = false;
    let (mut o_cpu, mut o_gpu, mut o_n) = (0u64, 0u64, 0u64);
    let mut files = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--best" => quality = Bc7Quality::Best,
            "--mips" => mips = true,
            "--limit" => limit = args.next().unwrap().parse().unwrap(),
            "--show" => show = args.next().unwrap().parse().unwrap(),
            "--oracle" => oracle = true,
            _ => collect(Path::new(&a), &mut files),
        }
    }
    files.truncate(limit);
    let gpu = Gpu::new().expect("gpu");
    println!("GPU {} ({:?}); {} files, quality {quality:?}", gpu.adapter_info.name, gpu.adapter_info.backend, files.len());
    // Warm up the pipeline.
    let warm = vec![128u8; 64 * 64 * 4];
    gpu.encode_bc7_ispc(&[RgbaImage { width: 64, height: 64, rgba: &warm }], quality).unwrap();

    let (mut blocks, mut same, mut cpu_err, mut gpu_err, mut cpu_t, mut gpu_t, mut px) = (0u64, 0u64, 0u64, 0u64, 0f64, 0f64, 0u64);
    let (mut worse, mut better) = (0u64, 0u64);
    for f in &files {
        let bytes = std::fs::read(f).unwrap();
        let Ok(info) = lodforge_dds::parse(&bytes) else { continue };
        let levels = if mips { info.mip_count.max(1) } else { 1 };
        let mut imgs = Vec::new();
        for l in 0..levels {
            if let Ok(img) = lodforge_dds::decode(&bytes, &info, l) {
                imgs.push(img);
            }
        }
        let refs: Vec<RgbaImage> = imgs.iter().map(|i| RgbaImage { width: i.width, height: i.height, rgba: &i.rgba }).collect();
        let t = Instant::now();
        let cpu: Vec<Vec<u8>> = refs.iter().map(|r| bc7_cpu::encode(r, quality)).collect();
        let ct = t.elapsed().as_secs_f64();
        let t = Instant::now();
        let gpu_out = gpu.encode_bc7_ispc(&refs, quality).unwrap();
        let gt = t.elapsed().as_secs_f64();
        cpu_t += ct;
        gpu_t += gt;
        let (mut fb, mut fs, mut fce, mut fge) = (0u64, 0u64, 0u64, 0u64);
        for ((img, c), g) in imgs.iter().zip(&cpu).zip(&gpu_out) {
            px += u64::from(img.width) * u64::from(img.height);
            let (bw, bh) = (img.width.div_ceil(4), img.height.div_ceil(4));
            let dc = decode_blocks(img.width, img.height, c);
            let dg = decode_blocks(img.width, img.height, g);
            // The scalar oracle (same arithmetic as the shader, exact on the CPU).
            let orc: Vec<[u8; 16]> = if oracle {
                use rayon::prelude::*;
                let opaque = img.rgba.chunks_exact(4).all(|p| p[3] == 255);
                let s = lodforge_gpu::bc7_ref::settings(quality, opaque);
                (0..bw * bh)
                    .into_par_iter()
                    .map(|b| {
                        let (bx, by) = (b % bw, b / bw);
                        let px: [[u8; 4]; 16] = std::array::from_fn(|k| {
                            let (x, y) = ((bx * 4 + k as u32 % 4).min(img.width - 1), (by * 4 + k as u32 / 4).min(img.height - 1));
                            let j = ((y * img.width + x) * 4) as usize;
                            [img.rgba[j], img.rgba[j + 1], img.rgba[j + 2], img.rgba[j + 3]]
                        });
                        lodforge_gpu::bc7_ref::encode_block(&px, &s)
                    })
                    .collect()
            } else {
                Vec::new()
            };
            for (b, o) in orc.iter().enumerate() {
                o_n += 1;
                o_cpu += u64::from(c[b * 16..b * 16 + 16] == o[..]);
                o_gpu += u64::from(g[b * 16..b * 16 + 16] == o[..]);
            }
            for by in 0..bh {
                for bx in 0..bw {
                    let i = ((by * bw + bx) * 16) as usize;
                    fb += 1;
                    if c[i..i + 16] == g[i..i + 16] {
                        fs += 1;
                        continue;
                    }
                    let ec = block_err(img.width, img.height, &img.rgba, &dc, bx, by);
                    let eg = block_err(img.width, img.height, &img.rgba, &dg, bx, by);
                    if shown < show {
                        shown += 1;
                        let mode = |b: &[u8]| b[0].trailing_zeros();
                        println!("  block ({bx},{by}) {}x{}: cpu mode {} err {ec} {:02x?}", img.width, img.height, mode(&c[i..]), &c[i..i + 16]);
                        println!("  {:>width$}gpu mode {} err {eg} {:02x?}", "", mode(&g[i..]), &g[i..i + 16], width = 0);
                        let mut px = Vec::new();
                        for y in by * 4..by * 4 + 4 {
                            for x in bx * 4..bx * 4 + 4 {
                                let j = ((y.min(img.height - 1) * img.width + x.min(img.width - 1)) * 4) as usize;
                                px.push(format!("{:02x}{:02x}{:02x}{:02x}", img.rgba[j], img.rgba[j + 1], img.rgba[j + 2], img.rgba[j + 3]));
                            }
                        }
                        println!("    src {}", px.join(" "));
                        if let Some(o) = orc.get((by * bw + bx) as usize) {
                            println!("  oracle mode {} {:02x?}", o[0].trailing_zeros(), o);
                        }
                    }
                    fce += ec;
                    fge += eg;
                    if eg > ec {
                        worse += 1;
                    } else if eg < ec {
                        better += 1;
                    }
                }
            }
        }
        blocks += fb;
        same += fs;
        cpu_err += fce;
        gpu_err += fge;
        println!(
            "{:6.2}% same  {:>8} blocks  diff-block err cpu {:>9} gpu {:>9}  cpu {:6.3}s gpu {:6.3}s  {}",
            100.0 * fs as f64 / fb.max(1) as f64,
            fb,
            fce,
            fge,
            ct,
            gt,
            f.file_name().unwrap().to_string_lossy()
        );
    }
    println!(
        "TOTAL {:.4}% identical of {} blocks ({:.1} Mpx); differing blocks: gpu worse {} better {}; their error cpu {} gpu {} ({:+.3}%); encode cpu {:.2}s ({:.1} Mpx/s) gpu {:.2}s ({:.1} Mpx/s)",
        100.0 * same as f64 / blocks.max(1) as f64,
        blocks,
        px as f64 / 1e6,
        worse,
        better,
        cpu_err,
        gpu_err,
        100.0 * (gpu_err as f64 - cpu_err as f64) / cpu_err.max(1) as f64,
        cpu_t,
        px as f64 / 1e6 / cpu_t,
        gpu_t,
        px as f64 / 1e6 / gpu_t
    );
    if o_n > 0 {
        println!("ORACLE vs cpu {:.4}%, vs gpu {:.4}% identical of {} blocks", 100.0 * o_cpu as f64 / o_n as f64, 100.0 * o_gpu as f64 / o_n as f64, o_n);
    }
}
