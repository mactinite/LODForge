//! BC7 block artefacts in DDS files: the mean luminance step across 4x4 block borders divided by
//! the mean step inside blocks, over texels both sides of which are opaque (alpha >= 128).
//! 1.0 = no block structure; mode-6-only encoding of detailed or smooth content goes above it.
//!
//! `cargo run --release --example bc7_blocks -- color|normal|all <dds or folder>...` (`normal` = `*_n.dds`)

use std::path::{Path, PathBuf};

use anyhow::Result;

fn files(p: &Path, out: &mut Vec<PathBuf>) {
    if p.is_dir() {
        for e in std::fs::read_dir(p).into_iter().flatten().flatten() {
            files(&e.path(), out);
        }
    } else if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("dds")) {
        out.push(p.to_path_buf());
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let which = args.first().cloned().unwrap_or_default();
    let mut all = Vec::new();
    for a in &args[1..] {
        files(Path::new(a), &mut all);
    }
    all.retain(|f| {
        let n = f.to_string_lossy().to_lowercase().ends_with("_n.dds");
        match which.as_str() {
            "color" => !n,
            "normal" => n,
            _ => true,
        }
    });
    let (mut edge, mut ne, mut inner, mut ni) = (0f64, 0u64, 0f64, 0u64);
    for f in &all {
        let Ok(bytes) = std::fs::read(f) else { continue };
        let Ok(info) = lodforge_dds::parse(&bytes) else { continue };
        let Ok(img) = lodforge_dds::decode(&bytes, &info, 0) else { continue };
        let (w, h) = (img.width as usize, img.height as usize);
        let px = |x: usize, y: usize| &img.rgba[(y * w + x) * 4..(y * w + x) * 4 + 4];
        let luma = |p: &[u8]| 0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2]);
        for y in 0..h {
            for x in 0..w {
                for (dx, dy) in [(1usize, 0usize), (0, 1)] {
                    let (x2, y2) = (x + dx, y + dy);
                    if x2 >= w || y2 >= h {
                        continue;
                    }
                    let (a, b) = (px(x, y), px(x2, y2));
                    if a[3] < 128 || b[3] < 128 {
                        continue;
                    }
                    let d = (luma(a) - luma(b)).abs();
                    let border = if dx == 1 { x2 % 4 == 0 } else { y2 % 4 == 0 };
                    if border {
                        edge += d;
                        ne += 1;
                    } else {
                        inner += d;
                        ni += 1;
                    }
                }
            }
        }
    }
    let ratio = (edge / ne.max(1) as f64) / (inner / ni.max(1) as f64).max(1e-9);
    println!("{} files: block-border step {:.2}, inside {:.2}, ratio {ratio:.3}", all.len(), edge / ne.max(1) as f64, inner / ni.max(1) as f64);
    Ok(())
}
