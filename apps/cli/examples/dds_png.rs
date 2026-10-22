//! Writes one mip of DDS files as PNG, optionally cropped and enlarged with nearest-neighbour
//! sampling, side by side when several files are given (before/after texture comparisons).
//!
//! `cargo run --release --example dds_png -- out.png a.dds b.dds --mip 0 --crop 0,0,128,128 --zoom 4`

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;

#[derive(Parser)]
struct Args {
    out: PathBuf,
    files: Vec<PathBuf>,
    #[arg(long, default_value_t = 0)]
    mip: u32,
    /// `x,y,w,h` in pixels of the chosen mip.
    #[arg(long)]
    crop: Option<String>,
    #[arg(long, default_value_t = 1)]
    zoom: u32,
    /// Show the alpha channel as grey instead of the colour.
    #[arg(long)]
    alpha: bool,
}

fn main() -> Result<()> {
    let a = Args::parse();
    let mut panels: Vec<(u32, u32, Vec<u8>)> = Vec::new();
    for f in &a.files {
        let bytes = std::fs::read(f).with_context(|| f.display().to_string())?;
        let info = lodforge_dds::parse(&bytes)?;
        let img = lodforge_dds::decode(&bytes, &info, a.mip.min(info.mip_count - 1))?;
        let (x0, y0, w, h) = match &a.crop {
            Some(c) => {
                let v: Vec<u32> = c.split(',').map(|s| s.trim().parse().unwrap_or(0)).collect();
                (v[0], v[1], v[2].min(img.width - v[0]), v[3].min(img.height - v[1]))
            }
            None => (0, 0, img.width, img.height),
        };
        let z = a.zoom.max(1);
        let (ow, oh) = (w * z, h * z);
        let mut px = vec![0u8; (ow * oh * 4) as usize];
        for y in 0..oh {
            for x in 0..ow {
                let s = (((y0 + y / z) * img.width + x0 + x / z) * 4) as usize;
                let d = ((y * ow + x) * 4) as usize;
                if a.alpha {
                    px[d..d + 3].fill(img.rgba[s + 3]);
                } else {
                    px[d..d + 3].copy_from_slice(&img.rgba[s..s + 3]);
                }
                px[d + 3] = 255;
            }
        }
        panels.push((ow, oh, px));
    }
    let gap = 8;
    let width: u32 = panels.iter().map(|p| p.0).sum::<u32>() + gap * (panels.len() as u32).saturating_sub(1);
    let height = panels.iter().map(|p| p.1).max().unwrap_or(1);
    let mut img = lodforge_terrain::Image::new(width, height);
    let mut x0 = 0;
    for (w, h, px) in &panels {
        for y in 0..*h {
            for x in 0..*w {
                let s = ((y * w + x) * 4) as usize;
                img.px[(y * width + x0 + x) as usize] = [f32::from(px[s]) / 255.0, f32::from(px[s + 1]) / 255.0, f32::from(px[s + 2]) / 255.0, 1.0];
            }
        }
        x0 += w + gap;
    }
    img.write_png(&a.out)?;
    println!("{} ({width}x{height})", a.out.display());
    Ok(())
}
