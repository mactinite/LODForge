//! Triangle and vertex counts per file and in all: `cargo run -p lodforge-nif --example nif_tris -- <file.nif>...`
fn main() {
    let (mut tt, mut tv) = (0usize, 0usize);
    for f in std::env::args().skip(1) {
        let Ok(bytes) = std::fs::read(&f) else { continue };
        let Ok(nif) = lodforge_nif::parse_all_geometry(&bytes) else {
            println!("{f}: parse error");
            continue;
        };
        let (mut t, mut v, mut s) = (0usize, 0usize, 0usize);
        for b in &nif.blocks {
            if let lodforge_nif::Block::TriShape(sh) = b {
                if std::env::var_os("NIF_DETAIL").is_some() {
                    let alpha = sh.alpha_property.and_then(|r| match nif.blocks.get(r as usize) {
                        Some(lodforge_nif::Block::Alpha { flags, threshold, .. }) => Some((*flags, *threshold)),
                        _ => None,
                    });
                    let shader = sh.shader_property.and_then(|r| match nif.blocks.get(r as usize) {
                        Some(lodforge_nif::Block::LightingShader(l)) => Some((l.shader_type, l.flags1, l.flags2)),
                        _ => None,
                    });
                    println!("    {:?} tris {} alpha {:?} shader {:?}", sh.av.net.name, sh.triangles.len(), alpha.map(|(f, t)| format!("{f:#06x}/{t}")), shader.map(|(ty, f1, f2)| format!("type {ty} f1 {f1:#010x} f2 {f2:#010x}")));
                }
                t += sh.triangles.len();
                v += sh.positions.len();
                s += 1;
            }
        }
        println!("{t:>7} tris {v:>7} verts {s:>3} shapes {:>8} bytes  {f}", bytes.len());
        tt += t;
        tv += v;
    }
    println!("{tt} tris, {tv} verts in all");
}
