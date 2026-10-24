//! Repeat-tile decisions (`lodforge_objlod::tiling`) for the shapes of LOD models: UV range,
//! repeats, and the UV range after `prepare`.
//!
//! `cargo run --release --example tiling_probe -- <model.nif>...`
use anyhow::Result;

fn range(uvs: &[[f32; 2]]) -> [f32; 4] {
    uvs.iter().fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |r, p| [r[0].min(p[0]), r[1].min(p[1]), r[2].max(p[0]), r[3].max(p[1])])
}

fn main() -> Result<()> {
    for f in std::env::args().skip(1) {
        let mesh = lodforge_objlod::mesh::import(&f, &std::fs::read(&f)?)?;
        for part in &mesh.parts {
            let reps = lodforge_objlod::tiling::tile_repeats(&part.uvs);
            let prepared = reps.and_then(|r| lodforge_objlod::tiling::prepare(part, r));
            println!(
                "{}\t{}\ttris {}\tuv {:?}\tspan {:?}\trepeats {:?}\tprepared {:?}",
                std::path::Path::new(&f).file_name().unwrap().to_string_lossy(),
                part.textures[0],
                part.triangles.len(),
                range(&part.uvs),
                lodforge_objlod::tiling::span(&part.uvs),
                reps,
                prepared.map(|p| (p.triangles.len(), range(&p.uvs)))
            );
        }
    }
    Ok(())
}
