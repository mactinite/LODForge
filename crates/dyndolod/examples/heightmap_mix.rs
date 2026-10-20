//! Diagnostics for LODGen object height maps (`<World>.objects.heightmap.dat`).
//!
//! `heightmap_mix <ours> <golden>`: per-cell difference report.
//! `heightmap_mix <ours> <golden> <out> x0 x1 y0 y1`: writes a map that takes our cells inside the
//! rectangle and the golden ones elsewhere, to bisect which cells change an occlusion result.

use std::collections::BTreeMap;

type Map = BTreeMap<(i32, i32), Vec<f32>>;

fn read(path: &str) -> Map {
    let b = std::fs::read(path).expect(path);
    let i = |o: usize| i32::from_le_bytes(b[o..o + 4].try_into().unwrap());
    let mut m = Map::new();
    let mut o = 4;
    for _ in 0..i(0) {
        let (x, y) = (i(o), i(o + 4));
        let v = b[o + 8..o + 8 + 33 * 33 * 4].chunks(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect();
        m.insert((x, y), v);
        o += 8 + 33 * 33 * 4;
    }
    m
}

fn write(path: &str, m: &Map) {
    let mut out = Vec::with_capacity(4 + m.len() * (8 + 33 * 33 * 4));
    out.extend_from_slice(&(m.len() as i32).to_le_bytes());
    for ((x, y), v) in m {
        out.extend_from_slice(&x.to_le_bytes());
        out.extend_from_slice(&y.to_le_bytes());
        for f in v {
            out.extend_from_slice(&f.to_le_bytes());
        }
    }
    std::fs::write(path, out).expect(path);
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (ours, golden) = (read(&a[0]), read(&a[1]));
    if a.len() >= 7 {
        let n = |k: usize| a[k].parse::<i32>().unwrap();
        let (x0, x1, y0, y1) = (n(3), n(4), n(5), n(6));
        let mut mix = golden.clone();
        let mut taken = 0;
        for (&(x, y), v) in &ours {
            if x >= x0 && x <= x1 && y >= y0 && y <= y1 {
                mix.insert((x, y), v.clone());
                taken += 1;
            }
        }
        write(&a[2], &mix);
        println!("{taken} cells from ours in x {x0}..{x1}, y {y0}..{y1}");
        return;
    }
    let only_ours = ours.keys().filter(|k| !golden.contains_key(k)).count();
    let only_golden = golden.keys().filter(|k| !ours.contains_key(k)).count();
    let mut cells_diff = 0;
    let mut values_diff = 0;
    let mut worst: Vec<(f32, (i32, i32), usize, f32, f32)> = Vec::new();
    for (k, va) in &ours {
        let Some(vb) = golden.get(k) else { continue };
        let mut any = false;
        for (i, (p, q)) in va.iter().zip(vb).enumerate() {
            if p.to_bits() != q.to_bits() {
                any = true;
                values_diff += 1;
                worst.push(((p - q).abs(), *k, i, *p, *q));
            }
        }
        cells_diff += usize::from(any);
    }
    // HM_CELL=x,y lists every differing value of one cell.
    if let Ok(c) = std::env::var("HM_CELL") {
        let mut p = c.split(',').map(|s| s.trim().parse::<i32>().unwrap());
        let cell = (p.next().unwrap(), p.next().unwrap());
        for (d, k, i, p, q) in worst.iter().filter(|w| w.1 == cell) {
            println!("  {k:?} [{},{}] ours {p} ({:08X}) golden {q} ({:08X}) |diff| {d}", i / 33, i % 33, p.to_bits(), q.to_bits());
        }
        return;
    }
    worst.sort_by(|x, y| y.0.total_cmp(&x.0));
    println!("cells: ours {}, golden {}, only ours {only_ours}, only golden {only_golden}; cells with differences {cells_diff}; values differing {values_diff}", ours.len(), golden.len());
    for (d, (x, y), i, p, q) in worst.iter().take(10) {
        println!("  ({x},{y}) [{},{}] ours {p} golden {q} |diff| {d}", i / 33, i % 33);
    }
}
