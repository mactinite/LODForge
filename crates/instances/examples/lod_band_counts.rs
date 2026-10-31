//! What the LODForge Runtime draws from one camera position, by distance alone (no frustum or
//! occlusion): tree impostors kept by the fade and thinning rules, and statics records per LOD
//! level inside their level's band, with their triangles.
//!
//! `cargo run --release -p lodforge-instances --example lod_band_counts -- <runtime dir> <World[.SEASON]> <x> <y> <z>
//!   [--ends l0,l1,max[,l32]] [--trees fade_start,fade_end,thin_start,thin_end,thin_keep]`
//!
//! Defaults: the user's cs-base distances 57344,147456,327680 (LOD32 band empty), the runtime's
//! tree defaults before 2026-10-29 (200000,240000,60000,200000,0.35).
use lodforge_instances::statics::StaticsFile;
use lodforge_instances::trees::TreeFile;

fn rank(hash: u32, k: u32) -> f32 {
    (hash.wrapping_mul(k) >> 8) as f32 * (1.0 / 16_777_216.0)
}

fn floats(s: &str) -> Vec<f32> {
    s.split(',').map(|v| v.trim().parse().expect("number")).collect()
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let dir = std::path::Path::new(&a[1]);
    let stem = &a[2];
    let cam = [a[3].parse::<f32>().unwrap(), a[4].parse::<f32>().unwrap(), a[5].parse::<f32>().unwrap()];
    let opt = |k: &str| a.iter().position(|x| x == k).and_then(|i| a.get(i + 1)).map(|s| floats(s));
    let mut ends = opt("--ends").unwrap_or_else(|| vec![57344.0, 147456.0, 327680.0]);
    if ends.len() < 4 {
        ends.push(ends[2]);
    }
    let tr = opt("--trees").unwrap_or_else(|| vec![200000.0, 240000.0, 60000.0, 200000.0, 0.35]);
    let band = 2048.0f32;
    let dist = |p: [f32; 3]| ((p[0] - cam[0]).powi(2) + (p[1] - cam[1]).powi(2) + (p[2] - cam[2]).powi(2)).sqrt();
    let rings = [0.0f32, 57344.0, 100000.0, 147456.0, 200000.0, 240000.0, 327680.0, 1e9];
    let ring = |d: f32| rings.windows(2).position(|w| d >= w[0] && d < w[1]).unwrap_or(rings.len() - 2);

    // Trees: expected drawn count (the fade band dithers per pixel; counted by its weight).
    if let Ok(bytes) = std::fs::read(dir.join(format!("{stem}.trees.bin"))) {
        let f = TreeFile::from_bytes(&bytes).expect("trees");
        let (mut all, mut drawn) = (vec![0usize; rings.len() - 1], vec![0.0f64; rings.len() - 1]);
        for (&cell, recs) in &f.cells {
            for r in recs {
                let d = dist(r.position(cell));
                let k = ring(d);
                all[k] += 1;
                let mut fade = 1.0f32;
                if tr[1] > tr[0] {
                    fade *= ((tr[1] - d) / (tr[1] - tr[0])).clamp(0.0, 1.0);
                }
                let t = ((d - tr[2]) / (tr[3] - tr[2]).max(1.0)).clamp(0.0, 1.0);
                let keep = 1.0 + (tr[4] - 1.0) * t;
                if fade > 0.0 && rank(r.hash, 1) < keep {
                    drawn[k] += f64::from(fade);
                }
            }
        }
        let total: f64 = drawn.iter().sum();
        println!("trees {stem}: {} records, drawn {:.0} (impostor quads, 2 triangles each = {:.0} triangles)", all.iter().sum::<usize>(), total, total * 2.0);
        for k in 0..rings.len() - 1 {
            println!("  {:>7.0}..{:<7.0} trees {:7} drawn {:9.0} ({:5.1} %)", rings[k], rings[k + 1].min(9.99e5), all[k], drawn[k], if all[k] > 0 { drawn[k] * 100.0 / all[k] as f64 } else { 0.0 });
        }
    }

    // Statics: a record of level k draws between its band's start and end (the switch at the band
    // edge spread over `band` by hash rank, as StaticCull.hlsl does).
    if let Ok(bytes) = std::fs::read(dir.join(format!("{stem}.statics.bin"))) {
        let f = StaticsFile::from_bytes(&bytes).expect("statics");
        let switch_at = |edge: f32, r: f32| edge - band + band * r;
        let mut per_level = [[0usize; 4]; 2];
        let mut tris = [0u64; 4];
        let mut by_ring = vec![[0usize; 4]; rings.len() - 1];
        let mut all = [0usize; 4];
        // LOD16 records (and their large-ref share) past the LOD16 band: what a LOD32 band would add.
        let mut beyond = [0usize; 2];
        for (&(level, _, _), recs) in &f.blocks {
            let li = match level { 4 => 0, 8 => 1, 16 => 2, _ => 3 };
            for r in recs {
                all[li] += 1;
                let d = dist(r.pos);
                let rk = rank(r.hash, 2_654_435_761);
                if li > 0 && d < switch_at(ends[li - 1], rk) {
                    continue;
                }
                if d >= switch_at(ends[li], rk) {
                    if li == 2 {
                        beyond[0] += 1;
                        if f.meshes[usize::from(r.mesh)].bounds.chunks(3).collect::<Vec<_>>().windows(2).any(|w| (0..3).any(|k| (w[1][k] - w[0][k]) * f32::from(r.scale) >= 2048.0)) {
                            beyond[1] += 1;
                        }
                    }
                    continue;
                }
                per_level[0][li] += 1;
                let m = &f.meshes[usize::from(r.mesh)];
                tris[li] += u64::from(m.triangles);
                by_ring[ring(d)][li] += 1;
                if m.flags & lodforge_instances::statics::MESH_LARGE_REF != 0 {
                    per_level[1][li] += 1;
                }
            }
        }
        println!("statics {stem}: {} meshes, records LOD4/8/16/32 {:?}", f.meshes.len(), all);
        println!("  drawn per level {:?} (large refs {:?}), triangles per level {:?}, total {} instances / {} triangles", per_level[0], per_level[1], tris, per_level[0].iter().sum::<usize>(), tris.iter().sum::<u64>());
        for k in 0..rings.len() - 1 {
            println!("  {:>7.0}..{:<7.0} drawn {:?}", rings[k], rings[k + 1].min(9.99e5), by_ring[k]);
        }
        println!("  LOD16 records past the LOD16 band: {} ({} of them 2048 units or larger)", beyond[0], beyond[1]);
    }
}
