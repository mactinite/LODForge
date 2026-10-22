//! `lodforge btr-compare`: the tolerant terrain LOD comparison of docs/re/terrain-lod-spec.md 5.1
//! between two folders of .btr files (per level: land vertex / triangle set agreement within a
//! tolerance, water shapes and segments, root AABB, shader, textures, shape headers).

use std::path::PathBuf;

use anyhow::Result;

#[derive(clap::Args)]
pub struct Args {
    /// First folder of .btr files (e.g. our output's meshes\terrain\<World>).
    pub a: PathBuf,
    /// Second folder (e.g. the golden run's).
    pub b: PathBuf,
    /// Tolerance in quad-local units (0 = exact bits).
    #[arg(long, default_value_t = 0.0)]
    pub tol: f32,
}

pub fn run(a: &Args) -> Result<()> {
    for r in lodforge_btr::compare::compare_dirs(&a.a, &a.b, a.tol).map_err(anyhow::Error::msg)? {
        println!("{}", r.line());
        for e in &r.examples {
            println!("  {e}");
        }
    }
    for r in lodforge_btr::compare::compare_heights(&a.a, &a.b).map_err(anyhow::Error::msg)? {
        println!("{}", r.line());
    }
    Ok(())
}
