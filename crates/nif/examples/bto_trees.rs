//! Diagnostics: tree billboards placed in an object LOD file (shapes whose name contains
//! "tree" or "passthru"), grouped per quad (two triangles sharing a diagonal), with each quad's
//! centre, height and width, sorted by distance to a point in the file's coordinates.
//! `cargo run -p lodforge-nif --example bto_trees -- <file.bto> <x> <y> <radius>`

use lodforge_nif::{Block, parse};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (path, x, y, r) = (&a[1], a[2].parse::<f32>().unwrap(), a[3].parse::<f32>().unwrap(), a[4].parse::<f32>().unwrap());
    let nif = parse(&std::fs::read(path).unwrap()).unwrap();
    let mut quads = Vec::new();
    for b in &nif.blocks {
        let Block::TriShape(s) = b else { continue };
        let name = s.av.net.name.clone().unwrap_or_default();
        let lname = name.to_ascii_lowercase();
        if !(lname.contains("tree") || lname.contains("passthru")) {
            continue;
        }
        let t = &s.av.transform;
        let w = |i: u16| {
            let p = s.positions[i as usize];
            [p[0] * t.scale + t.translation[0], p[1] * t.scale + t.translation[1], p[2] * t.scale + t.translation[2]]
        };
        for pair in s.triangles.chunks(2) {
            let mut pts = Vec::new();
            for tri in pair {
                for &i in tri {
                    pts.push(w(i));
                }
            }
            let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
            for p in &pts {
                for k in 0..3 {
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
            }
            let c = [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0];
            let d = ((c[0] - x).powi(2) + (c[1] - y).powi(2)).sqrt();
            if d <= r {
                let width = ((hi[0] - lo[0]).powi(2) + (hi[1] - lo[1]).powi(2)).sqrt();
                quads.push((d, name.clone(), c, lo[2], hi[2] - lo[2], width));
            }
        }
    }
    quads.sort_by(|a, b| a.0.total_cmp(&b.0));
    for q in &quads {
        println!("dist {:7.0}  {:<24} centre ({:.0}, {:.0})  base z {:.0}  height {:6.0}  width {:6.0}", q.0, q.1, q.2[0], q.2[1], q.3, q.4, q.5);
    }
    println!("{} tree quads within {r}", quads.len());
}
