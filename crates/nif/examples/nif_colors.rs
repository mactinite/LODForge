//! Vertex colour statistics per shape of a NIF: whether colours are stored, how many vertices
//! are not white in RGB, how many have alpha below 255, and the alpha range.
//! `cargo run -p lodforge-nif --example nif_colors -- <file.nif> [more files]`

use lodforge_nif::{Block, parse_all_geometry};

fn main() {
    for path in std::env::args().skip(1) {
        let Ok(bytes) = std::fs::read(&path) else {
            println!("{path}: unreadable");
            continue;
        };
        let nif = match parse_all_geometry(&bytes) {
            Ok(n) => n,
            Err(e) => {
                println!("{path}: {e}");
                continue;
            }
        };
        println!("{path}:");
        for (i, b) in nif.blocks.iter().enumerate() {
            let Block::TriShape(s) = b else { continue };
            let n = s.positions.len();
            if s.colors.is_empty() {
                println!("  [{i}] {:?} v={n} no colours (desc {:#x})", s.av.net.name, s.vertex_desc);
                continue;
            }
            let non_white = s.colors.iter().filter(|c| c[0] != 255 || c[1] != 255 || c[2] != 255).count();
            let alpha_low = s.colors.iter().filter(|c| c[3] != 255).count();
            let (amin, amax) = s.colors.iter().fold((255u8, 0u8), |(lo, hi), c| (lo.min(c[3]), hi.max(c[3])));
            let (rmin, rmax) = s.colors.iter().fold((255u8, 0u8), |(lo, hi), c| (lo.min(c[0]), hi.max(c[0])));
            println!("  [{i}] {:?} v={n} colours: rgb non-white {non_white}, alpha<255 {alpha_low}, alpha {amin}..{amax}, r {rmin}..{rmax} (desc {:#x})", s.av.net.name, s.vertex_desc);
        }
    }
}
