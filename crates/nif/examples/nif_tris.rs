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
