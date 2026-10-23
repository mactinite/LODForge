//! Diagnostics: the meshes TexGen's scene builds from a NIF (after node transforms, root
//! ignored), with their diffuse and z range, plus the overall bounds the billboard size uses.
//! `cargo run -p lodforge-texgen --example scene_bounds -- <file.nif>...`

fn main() {
    for path in std::env::args().skip(1) {
        let bytes = std::fs::read(&path).expect("read");
        let nif = lodforge_nif::parse_all_geometry(&bytes).expect("parse");
        let scene = lodforge_texgen::Scene::from_nif(&nif);
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        println!("{path}: {} meshes", scene.meshes.len());
        for m in &scene.meshes {
            let (mut mlo, mut mhi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for p in &m.positions {
                for k in 0..3 {
                    mlo[k] = mlo[k].min(p[k]);
                    mhi[k] = mhi[k].max(p[k]);
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
            }
            println!("  {:>6} verts  z {:>8.1} .. {:>8.1}  xy {:>7.1}..{:>7.1} / {:>7.1}..{:>7.1}  {}", m.positions.len(), mlo[2], mhi[2], mlo[0], mhi[0], mlo[1], mhi[1], m.diffuse);
        }
        println!("  overall z {:.1} .. {:.1} (height {:.1}), x {:.1} .. {:.1}, y {:.1} .. {:.1}", lo[2], hi[2], hi[2] - lo[2], lo[0], hi[0], lo[1], hi[1]);
    }
}
