//! Terrain LOD texture quality metrics, for before/after comparisons of filter changes.
//!
//! - grid: mean luminance step across cell borders divided by the mean step inside cells
//!   (1.0 = no cell grid; per-cell resizing with clamped edges pushes it above 1).
//! - normal length: mean length of the decoded `_n` vectors per mip (1.0 = unit normals;
//!   averaging without renormalising shortens them and flattens distant lighting).
//! - mip luminance: mean linear-light luminance of each mip relative to the top mip (a mip chain
//!   filtered in gamma space darkens contrasty textures).
//!
//! `cargo run --release --example lod_quality -- --dir <Data>\textures\terrain\Tamriel --world Tamriel`

use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use rayon::prelude::*;

#[derive(Parser)]
struct Args {
    /// Folder with the `<World>.<level>.<x>.<y>[_n].dds` tiles.
    #[arg(long)]
    dir: PathBuf,
    #[arg(long, default_value = "Tamriel")]
    world: String,
    #[arg(long, value_delimiter = ',', default_value = "4,8,16,32")]
    levels: Vec<u32>,
    /// At most this many tiles per level and kind (evenly spread).
    #[arg(long, default_value_t = 200)]
    sample: usize,
    /// Also print the mean luminance step by position inside the cell (0 = the cell border).
    #[arg(long)]
    profile: bool,
}

/// Mean step by (position % ppc), both axes.
fn step_profile(rgba: &[u8], size: usize, ppc: usize) -> Vec<f64> {
    let y: Vec<f32> = rgba.chunks_exact(4).map(luma).collect();
    let mut sum = vec![0f64; ppc];
    let mut n = vec![0u64; ppc];
    for r in 0..size {
        for c in 1..size {
            let h = f64::from((y[r * size + c] - y[r * size + c - 1]).abs());
            let v = f64::from((y[c * size + r] - y[(c - 1) * size + r]).abs());
            sum[c % ppc] += h + v;
            n[c % ppc] += 2;
        }
    }
    sum.iter().zip(&n).map(|(s, n)| s / (*n).max(1) as f64).collect()
}

fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

fn luma(p: &[u8]) -> f32 {
    (0.2126 * f32::from(p[0]) + 0.7152 * f32::from(p[1]) + 0.0722 * f32::from(p[2])) / 255.0
}

/// (border step, interior step, quadrant-border step) of a `size` square image with `ppc` pixels
/// per cell. Quadrant borders (half a cell) are where the land's own texture layers change too,
/// so their step is the content's own level of discontinuity.
fn grid_steps(rgba: &[u8], size: usize, ppc: usize) -> (f64, f64, f64) {
    let y: Vec<f32> = rgba.chunks_exact(4).map(luma).collect();
    let (mut b, mut nb, mut i, mut ni, mut q, mut nq) = (0f64, 0u64, 0f64, 0u64, 0f64, 0u64);
    for r in 0..size {
        for c in 1..size {
            let h = f64::from((y[r * size + c] - y[r * size + c - 1]).abs());
            let v = f64::from((y[c * size + r] - y[(c - 1) * size + r]).abs());
            if c % ppc == 0 {
                b += h + v;
                nb += 2;
            } else if ppc >= 4 && c % ppc == ppc / 2 {
                q += h + v;
                nq += 2;
            } else {
                i += h + v;
                ni += 2;
            }
        }
    }
    (b / nb.max(1) as f64, i / ni.max(1) as f64, q / nq.max(1) as f64)
}

#[derive(Default, Clone)]
struct Acc {
    n: usize,
    border: f64,
    interior: f64,
    quadrant: f64,
    /// Per mip: (sum of normal lengths or relative luminance, count).
    mips: Vec<(f64, f64)>,
    profile: Vec<f64>,
}

fn main() -> Result<()> {
    let a = Args::parse();
    let names: Vec<String> = std::fs::read_dir(&a.dir)?.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    for &level in &a.levels {
        for normal in [false, true] {
            let prefix = format!("{}.{level}.", a.world).to_lowercase();
            let mut files: Vec<&String> = names
                .iter()
                .filter(|n| {
                    let l = n.to_lowercase();
                    let parts = l.trim_end_matches(".dds").split('.').count();
                    // `<World>.<level>.<x>.<y>[_n].dds`: no season part.
                    l.starts_with(&prefix) && l.ends_with(".dds") && parts == 4 && l.ends_with("_n.dds") == normal
                })
                .collect();
            files.sort();
            if files.is_empty() {
                continue;
            }
            let step = (files.len() / a.sample.max(1)).max(1);
            let picked: Vec<&String> = files.iter().step_by(step).copied().collect();
            let accs: Vec<Acc> = picked
                .par_iter()
                .filter_map(|name| {
                    let bytes = std::fs::read(a.dir.join(name)).ok()?;
                    let info = lodforge_dds::parse(&bytes).ok()?;
                    let top = lodforge_dds::decode(&bytes, &info, 0).ok()?;
                    let size = top.width as usize;
                    // Default tiles are shrunk (32 px) and carry no cell structure worth measuring.
                    if size < 256 {
                        return None;
                    }
                    let ppc = size / level as usize;
                    let (border, interior, quadrant) = grid_steps(&top.rgba, size, ppc);
                    let top_luma: f64 = top.rgba.chunks_exact(4).map(|p| f64::from(srgb_to_linear(luma(p)))).sum::<f64>() / (size * size) as f64;
                    let mut mips = Vec::new();
                    for m in 0..info.mip_count.min(8) {
                        let Ok(img) = lodforge_dds::decode(&bytes, &info, m) else { break };
                        let px = img.rgba.chunks_exact(4);
                        let count = px.len() as f64;
                        let v = if normal {
                            img.rgba.chunks_exact(4).map(|p| {
                                let f = |c: u8| f64::from(c) / 255.0 * 2.0 - 1.0;
                                (f(p[0]).powi(2) + f(p[1]).powi(2) + f(p[2]).powi(2)).sqrt()
                            }).sum::<f64>()
                        } else {
                            img.rgba.chunks_exact(4).map(|p| f64::from(srgb_to_linear(luma(p)))).sum::<f64>() / top_luma.max(1e-6)
                        };
                        mips.push((v, count));
                    }
                    let profile = if a.profile { step_profile(&top.rgba, size, ppc) } else { Vec::new() };
                    Some(Acc { n: 1, border, interior, quadrant, mips, profile })
                })
                .collect();
            if accs.is_empty() {
                continue;
            }
            let n = accs.len() as f64;
            let border = accs.iter().map(|x| x.border).sum::<f64>() / n;
            let interior = accs.iter().map(|x| x.interior).sum::<f64>() / n;
            let quadrant = accs.iter().map(|x| x.quadrant).sum::<f64>() / n;
            let max_mips = accs.iter().map(|x| x.mips.len()).max().unwrap_or(0);
            let mips: Vec<String> = (0..max_mips)
                .map(|m| {
                    let (s, c) = accs.iter().filter_map(|x| x.mips.get(m)).fold((0.0, 0.0), |a, b| (a.0 + b.0, a.1 + b.1));
                    format!("{:.3}", s / c.max(1.0))
                })
                .collect();
            println!(
                "LOD{level:<2} {:7} {:4} tiles  grid {:.3} (quadrant {:.3})  {} per mip: {}",
                if normal { "normal" } else { "diffuse" },
                accs.iter().map(|x| x.n).sum::<usize>(),
                border / interior.max(1e-9),
                quadrant / interior.max(1e-9),
                if normal { "length" } else { "rel. luminance" },
                mips.join(" ")
            );
            if a.profile {
                let len = accs.iter().map(|x| x.profile.len()).max().unwrap_or(0);
                let prof: Vec<f64> = (0..len).map(|i| accs.iter().filter_map(|x| x.profile.get(i)).sum::<f64>() / n).collect();
                let mean = prof.iter().skip(1).sum::<f64>() / (len.max(2) - 1) as f64;
                println!("      step by position in cell / mean: {}", prof.iter().map(|v| format!("{:.2}", v / mean.max(1e-9))).collect::<Vec<_>>().join(" "));
            }
        }
    }
    Ok(())
}
