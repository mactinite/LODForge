//! Diagnostic (ignored): the border point sets of two neighbouring Tamriel LOD4 quads built from
//! xLODGen's terrain dump. Run with `cargo test -p lodforge-btr --test diag_seam -- --ignored
//! --nocapture`.

use lodforge_btr::mesh::{build_quad, full_coverage, QuadOptions};
use lodforge_btr::{terrain_bin, Settings};

const BIN: &str = r"D:\Modlists\skyrim-cs\tools\xLODGen\Edit Scripts\LODGen_Terrain_Tamriel.bin";

#[test]
#[ignore]
fn east_west_border_of_36_16_and_40_16() {
    let Ok(bytes) = std::fs::read(BIN) else { return };
    let cells = terrain_bin::read_cells(&bytes).unwrap();
    let input = terrain_bin::terrain_input(&cells, "Tamriel", (-96, -96), 32, -27000.0, Some(-14000.0));
    let field = input.sampler();
    let opts = QuadOptions::for_level(&Settings::default(), 4);
    let a = build_quad(&field, &opts, 36, 16, &full_coverage).unwrap();
    let b = build_quad(&field, &opts, 40, 16, &full_coverage).unwrap();
    let east: Vec<(f32, f32)> = a.land.positions[..a.land.grid_vertices].iter().filter(|p| p[0] == 4096.0).map(|p| (p[1], p[2])).collect();
    let west: Vec<(f32, f32)> = b.land.positions[..b.land.grid_vertices].iter().filter(|p| p[0] == 0.0).map(|p| (p[1], p[2])).collect();
    let mut e = east.clone();
    e.sort_by(|x, y| x.partial_cmp(y).unwrap());
    let mut w = west.clone();
    w.sort_by(|x, y| x.partial_cmp(y).unwrap());
    for (y, z) in &e {
        if !w.iter().any(|(wy, _)| wy == y) {
            println!("only in A (east): y {y} z {z}");
        }
    }
    for (y, z) in &w {
        if !e.iter().any(|(ey, _)| ey == y) {
            println!("only in B (west): y {y} z {z}");
        }
    }
    // Heights along the shared column from the sampler, rows of cell y = 18.
    for gy in 576..=608 {
        let h = field.height(40 * 32, gy) / 4.0;
        let local_y = (gy - 16 * 32) as f32 * 32.0;
        let in_a = e.iter().any(|(y, _)| *y == local_y);
        let in_b = w.iter().any(|(y, _)| *y == local_y);
        println!("gy {gy} local y {local_y}: h {h} A {in_a} B {in_b}");
    }
    println!("east {} west {}", e.len(), w.len());
}

#[test]
#[ignore]
fn profile_of_the_shared_column() {
    let Ok(bytes) = std::fs::read(BIN) else { return };
    let cells = terrain_bin::read_cells(&bytes).unwrap();
    let input = terrain_bin::terrain_input(&cells, "Tamriel", (-96, -96), 32, -27000.0, Some(-14000.0));
    let field = input.sampler();
    let heights: Vec<f32> = (576..=608).map(|gy| field.height(40 * 32, gy) / 4.0).collect();
    let mut fixed = vec![false; 33];
    fixed[0] = true;
    fixed[32] = true;
    let sel = lodforge_btr::profile::simplify(&heights, &fixed, &vec![true; 33], 5.0);
    let rows: Vec<usize> = (0..33).filter(|&k| sel[k]).collect();
    println!("profile selects rows {rows:?} ({} points)", rows.len());
}
