//! Parser smoke tests over real files: every NIF/DDS under a folder.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::Result;
use rayon::prelude::*;

fn files_with(dir: &Path, exts: &[&str]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_owned()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| exts.iter().any(|w| x.eq_ignore_ascii_case(w))) {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

fn report_failures(failures: &[(PathBuf, String)]) {
    println!("failures: {}", failures.len());
    // Group by error kind (text before the first digit-bearing detail) so unexpected ones stand out.
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    for (_, e) in failures {
        let kind = if e.starts_with("unsupported NIF version") { e.clone() } else { e.split([':', '(']).next().unwrap_or(e).to_string() };
        *kinds.entry(kind).or_default() += 1;
    }
    for (k, n) in &kinds {
        println!("  {n:>6}  {k}");
    }
    for (p, e) in failures.iter().filter(|(_, e)| !e.starts_with("unsupported NIF version")).take(15) {
        println!("  {}: {e}", p.display());
    }
}

pub fn nif_scan(dir: &Path) -> Result<()> {
    let files = files_with(dir, &["nif", "btr", "bto"]);
    let t = Instant::now();
    let roundtrip_bad = std::sync::atomic::AtomicUsize::new(0);
    let results: Vec<(PathBuf, Result<lodforge_nif::Nif, String>, u64)> = files
        .par_iter()
        .map(|p| {
            let bytes = std::fs::read(p).unwrap_or_default();
            let r = lodforge_nif::parse(&bytes).map_err(|e| e.to_string());
            if let Ok(n) = &r
                && n.header.bs_version != lodforge_nif::LE_BS_VERSION
                && n.to_bytes().ok().as_deref() != Some(&bytes[..])
            {
                roundtrip_bad.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if std::env::var_os("LODFORGE_ROUNDTRIP").is_some() {
                    eprintln!("round-trip differs: {}", p.display());
                }
            }
            (p.clone(), r, bytes.len() as u64)
        })
        .collect();
    let elapsed = t.elapsed();

    let mut types: BTreeMap<String, (usize, bool)> = BTreeMap::new();
    let (mut shapes, mut verts, mut tris, mut bytes) = (0usize, 0usize, 0usize, 0u64);
    let mut failures = Vec::new();
    for (p, r, len) in &results {
        bytes += len;
        match r {
            Ok(nif) => {
                for (i, b) in nif.blocks.iter().enumerate() {
                    let e = types.entry(nif.block_type(i).to_string()).or_insert((0, false));
                    e.0 += 1;
                    e.1 = !matches!(b, lodforge_nif::Block::Unknown);
                    if let lodforge_nif::Block::TriShape(s) = b {
                        shapes += 1;
                        verts += s.positions.len();
                        tris += s.triangles.len();
                    }
                }
            }
            Err(e) => failures.push((p.clone(), e.clone())),
        }
    }
    println!("{} files, {:.1} MB parsed in {} ms", results.len(), bytes as f64 / 1048576.0, elapsed.as_millis());
    println!("shapes {shapes}, vertices {verts}, triangles {tris}");
    println!("re-encoded differently (SSE files): {}", roundtrip_bad.load(std::sync::atomic::Ordering::Relaxed));
    println!("block types (count, decoded):");
    for (t, (n, decoded)) in &types {
        println!("  {t:<28} {n:>8} {}", if *decoded { "decoded" } else { "skipped" });
    }
    report_failures(&failures);
    Ok(())
}

pub fn bsa_scan(vfs: &lodforge_archive::Vfs) -> Result<()> {
    let paths: Vec<&str> = vfs.archived_paths().filter(|p| p.ends_with(".nif") || p.ends_with(".dds")).collect();
    let t = Instant::now();
    let results: Vec<(PathBuf, Result<u64, String>)> = paths
        .par_iter()
        .map(|&p| {
            let r = (|| -> Result<u64, String> {
                let bytes = vfs.read(p).map_err(|e| e.to_string())?.ok_or("vanished")?;
                if p.ends_with(".nif") {
                    lodforge_nif::parse(&bytes).map_err(|e| e.to_string())?;
                } else {
                    lodforge_dds::parse(&bytes).map_err(|e| e.to_string())?;
                }
                Ok(bytes.len() as u64)
            })();
            (PathBuf::from(p), r)
        })
        .collect();
    let elapsed = t.elapsed();
    let bytes: u64 = results.iter().filter_map(|(_, r)| r.as_ref().ok()).sum();
    let (nifs, ddss) = (paths.iter().filter(|p| p.ends_with(".nif")).count(), paths.iter().filter(|p| p.ends_with(".dds")).count());
    println!("{} archived files ({nifs} nif, {ddss} dds), {:.1} MB extracted + parsed in {} ms", results.len(), bytes as f64 / 1048576.0, elapsed.as_millis());
    let failures: Vec<(PathBuf, String)> = results.into_iter().filter_map(|(p, r)| r.err().map(|e| (p, e))).collect();
    report_failures(&failures);
    Ok(())
}

/// Decodes up to `limit` DDS files of `size`x`size` under `dir`, re-encodes them with the GPU BC7
/// encoder in batches and reports throughput and round-trip PSNR against the decoded source.
pub fn bc7_bench(dir: &Path, size: u32, limit: usize, batch: usize) -> Result<()> {
    let gpu = lodforge_gpu::Gpu::new()?;
    println!("GPU: {} ({:?})", gpu.adapter_info.name, gpu.adapter_info.backend);
    let mut sources: Vec<(u32, u32, Vec<u8>)> = files_with(dir, &["dds"])
        .par_iter()
        .filter_map(|p| {
            let bytes = std::fs::read(p).ok()?;
            let info = lodforge_dds::parse(&bytes).ok()?;
            if info.width != size || info.height != size {
                return None;
            }
            let img = lodforge_dds::decode(&bytes, &info, 0).ok()?;
            Some((img.width, img.height, img.rgba))
        })
        .collect();
    sources.truncate(limit);
    if sources.is_empty() {
        anyhow::bail!("no {size}x{size} DDS files under {}", dir.display());
    }
    let mpix = sources.iter().map(|s| f64::from(s.0 * s.1)).sum::<f64>() / 1e6;
    // Warm-up (pipeline compile, driver caches).
    gpu.encode_bc7(&[lodforge_gpu::RgbaImage { width: sources[0].0, height: sources[0].1, rgba: &sources[0].2 }])?;
    let t = Instant::now();
    let mut encoded = Vec::with_capacity(sources.len());
    for chunk in sources.chunks(batch) {
        let imgs: Vec<_> = chunk.iter().map(|(w, h, rgba)| lodforge_gpu::RgbaImage { width: *w, height: *h, rgba }).collect();
        encoded.extend(gpu.encode_bc7(&imgs)?);
    }
    let secs = t.elapsed().as_secs_f64();
    println!("{} images, {mpix:.1} Mpixel encoded in {:.0} ms ({:.0} Mpixel/s, incl. upload + readback)", sources.len(), secs * 1000.0, mpix / secs);

    let psnrs: Vec<f64> = sources
        .par_iter()
        .zip(&encoded)
        .map(|((w, h, rgba), blocks)| {
            let info = lodforge_dds::DdsInfo { width: *w, height: *h, mip_count: 1, format: lodforge_dds::Format::Bc7, layers: 1, srgb: false, data_offset: 0 };
            let dec = lodforge_dds::decode(blocks, &info, 0).expect("decode own BC7");
            let mse = rgba.iter().zip(&dec.rgba).map(|(a, b)| (f64::from(*a) - f64::from(*b)).powi(2)).sum::<f64>() / rgba.len() as f64;
            if mse == 0.0 { 99.0 } else { 10.0 * (255.0 * 255.0 / mse).log10() }
        })
        .collect();
    let mean = psnrs.iter().sum::<f64>() / psnrs.len() as f64;
    let min = psnrs.iter().cloned().fold(f64::INFINITY, f64::min);
    println!("round-trip PSNR vs decoded source: mean {mean:.2} dB, min {min:.2} dB");
    Ok(())
}

/// Decodes two DDS files (top mip) and reports differing pixels per `block`-sized square.
pub fn dds_diff(a: &Path, b: &Path, block: u32) -> Result<()> {
    let load = |p: &Path| -> Result<lodforge_dds::Image> {
        let bytes = std::fs::read(p)?;
        let info = lodforge_dds::parse(&bytes)?;
        Ok(lodforge_dds::decode(&bytes, &info, 0)?)
    };
    let (x, y) = (load(a)?, load(b)?);
    if (x.width, x.height) != (y.width, y.height) {
        anyhow::bail!("sizes differ: {}x{} vs {}x{}", x.width, x.height, y.width, y.height);
    }
    let mut blocks: BTreeMap<(u32, u32), (usize, u8)> = BTreeMap::new();
    for py in 0..x.height {
        for px in 0..x.width {
            let i = ((py * x.width + px) * 4) as usize;
            let d = (0..4).map(|c| x.rgba[i + c].abs_diff(y.rgba[i + c])).max().unwrap_or(0);
            if d > 0 {
                let e = blocks.entry((px / block, py / block)).or_default();
                e.0 += 1;
                e.1 = e.1.max(d);
            }
        }
    }
    let (rgb, alpha) = crate::texgen_stitched::psnr(&x.rgba, &y.rgba);
    println!("PSNR top mip: rgb {rgb:.2} dB, alpha {alpha:.2} dB");
    println!("{} of {} blocks ({block}px) differ", blocks.len(), (x.width / block) * (x.height / block));
    for ((bx, by), (n, max)) in &blocks {
        println!("  block col {bx} row {by} (row 0 = north): {n} px, max diff {max}");
    }
    Ok(())
}

pub fn dds_scan(dir: &Path) -> Result<()> {
    let files = files_with(dir, &["dds"]);
    let t = Instant::now();
    let results: Vec<(PathBuf, Result<(lodforge_dds::Format, u32, u32, u32), String>)> = files
        .par_iter()
        .map(|p| {
            let r = (|| -> Result<_, String> {
                let bytes = std::fs::read(p).map_err(|e| e.to_string())?;
                let info = lodforge_dds::parse(&bytes).map_err(|e| e.to_string())?;
                let img = lodforge_dds::decode(&bytes, &info, 0).map_err(|e| e.to_string())?;
                if img.rgba.len() != (img.width * img.height * 4) as usize {
                    return Err("decoded size mismatch".into());
                }
                Ok((info.format, info.width, info.height, info.mip_count))
            })();
            (p.clone(), r)
        })
        .collect();
    let elapsed = t.elapsed();
    let mut formats: BTreeMap<String, usize> = BTreeMap::new();
    let mut failures = Vec::new();
    for (p, r) in &results {
        match r {
            Ok((f, w, h, m)) => *formats.entry(format!("{f:?} {w}x{h} mips {m}")).or_default() += 1,
            Err(e) => failures.push((p.clone(), e.clone())),
        }
    }
    println!("{} files parsed + top mip decoded in {} ms", results.len(), elapsed.as_millis());
    for (f, n) in &formats {
        println!("  {f:<34} {n:>6}");
    }
    report_failures(&failures);
    Ok(())
}
