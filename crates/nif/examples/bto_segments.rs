//! Prints, for every BSSubIndexTriShape of a BTO, which 4x4 cell of the quad each segment's
//! triangles fall into (by triangle centroid, in world units), to read the segment order off a
//! golden file. `cargo run -p lodforge-nif --example bto_segments -- <file.bto>`

use lodforge_nif::{Block, parse};

fn main() {
    let path = std::env::args().nth(1).expect("usage: bto_segments <file>");
    let nif = parse(&std::fs::read(&path).unwrap()).unwrap();
    let name = std::path::Path::new(&path).file_name().unwrap().to_string_lossy().into_owned();
    let parts: Vec<&str> = name.split('.').collect();
    let level: f32 = parts[1].parse().unwrap();
    let (bx, by): (i32, i32) = (parts[2].parse().unwrap(), parts[3].parse().unwrap());
    for b in &nif.blocks {
        let Block::TriShape(s) = b else { continue };
        let Some(segs) = &s.segments else { continue };
        let t = &s.av.transform;
        println!("{:?} v={} t={}", s.av.net.name, s.positions.len(), s.triangles.len());
        for (k, seg) in segs.iter().enumerate() {
            if seg.count == 0 {
                continue;
            }
            let first = (seg.start / 3) as usize;
            let mut cells = std::collections::BTreeMap::new();
            for tri in &s.triangles[first..first + seg.count as usize] {
                let mut c = [0f32; 2];
                for &i in tri {
                    let p = s.positions[i as usize];
                    c[0] += (p[0] * t.scale + t.translation[0]) / 3.0;
                    c[1] += (p[1] * t.scale + t.translation[1]) / 3.0;
                }
                let cx = (c[0] / 4096.0).floor() as i32 - bx;
                let cy = (c[1] / 4096.0).floor() as i32 - by;
                *cells.entry((cx, cy)).or_insert(0usize) += 1;
            }
            println!("  seg {k:2} (x*4+y -> ({},{}), y*4+x -> ({},{})): {} tris, centroid cells {:?}", k / 4, k % 4, k % 4, k / 4, seg.count, cells);
        }
        let _ = level;
    }
}
