//! Generates one LOD level of a full model with `lodforge_lodmodel` (the `lod-models` generator)
//! into a file, without a profile: for checking the generator on single models.
//!
//! `cargo run --release --example lodmodel_one -- <full.nif> <level 0|1|2> <out.nif> [shading tolerance]`
use anyhow::{Result, anyhow};

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let level: usize = a[1].parse()?;
    let name = std::path::Path::new(&a[0]).file_stem().unwrap().to_string_lossy().to_string();
    let mut p = lodforge_lodmodel::LEVELS[level];
    if let Some(t) = a.get(3) {
        p.shade_tol = t.parse()?;
    }
    let (bytes, st) = lodforge_lodmodel::generate(&std::fs::read(&a[0])?, &p, &name).map_err(|e| anyhow!("{e:?}"))?;
    std::fs::write(&a[2], bytes)?;
    println!("{}: {} -> {} triangles, shade {:.3}, error limit {}", a[0], st.source_triangles, st.triangles, st.shade, st.error_limit);
    Ok(())
}
