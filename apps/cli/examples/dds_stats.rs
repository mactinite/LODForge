//! Coverage and colour statistics of DDS textures: per file, the share of texels with alpha at or
//! above a threshold and the mean RGB (and mean alpha) of those texels, plus the mean RGB of every
//! texel. For checking baked LOD textures (crown atlases, billboards) against their sources.
//!
//! `cargo run --release --example dds_stats -- [--threshold 128] [--mip 0] a.dds b.dds ...`
use anyhow::{Result, anyhow};

fn main() -> Result<()> {
    let mut threshold = 128u32;
    let mut mip = 0u32;
    let mut files = Vec::new();
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < a.len() {
        match a[i].as_str() {
            "--threshold" => {
                threshold = a.get(i + 1).ok_or_else(|| anyhow!("--threshold needs a value"))?.parse()?;
                i += 1;
            }
            "--mip" => {
                mip = a.get(i + 1).ok_or_else(|| anyhow!("--mip needs a value"))?.parse()?;
                i += 1;
            }
            f => files.push(f.to_string()),
        }
        i += 1;
    }
    for f in &files {
        let bytes = std::fs::read(f)?;
        let info = lodforge_dds::parse(&bytes).map_err(|e| anyhow!("{f}: {e}"))?;
        let img = lodforge_dds::decode(&bytes, &info, mip).map_err(|e| anyhow!("{f}: {e}"))?;
        let n = (img.width * img.height) as usize;
        let (mut cov, mut sum, mut suma, mut all) = (0u64, [0u64; 3], 0u64, [0u64; 3]);
        for px in img.rgba.chunks_exact(4) {
            for k in 0..3 {
                all[k] += u64::from(px[k]);
            }
            if u32::from(px[3]) >= threshold {
                cov += 1;
                suma += u64::from(px[3]);
                for k in 0..3 {
                    sum[k] += u64::from(px[k]);
                }
            }
        }
        let name = f.rsplit(['\\', '/']).next().unwrap_or(f);
        println!(
            "{name}: {}x{} mip {mip}, alpha>={threshold}: {:.1}% of texels, mean RGB {:?}, mean alpha {}; all texels mean RGB {:?}",
            img.width,
            img.height,
            100.0 * cov as f64 / n.max(1) as f64,
            sum.map(|v| (v / cov.max(1)) as u32),
            (suma / cov.max(1)) as u32,
            all.map(|v| (v / n.max(1) as u64) as u32)
        );
    }
    Ok(())
}
