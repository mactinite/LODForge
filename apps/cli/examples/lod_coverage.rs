//! How much of a full model's silhouette its LOD model keeps: for the LOD4 models a detail bake
//! report lists, the triangle counts and the silhouette overlap (intersection over union) of full
//! and LOD model from five views (top, four sides at 30 degrees above the horizon), plus the share
//! of the full silhouette the LOD misses (lost volume) and adds (bloat).
//! `lod_coverage <Data folder of the output> <MO2 mods folder> [count]`
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use glam::{Mat3, Vec3};

const RES: usize = 192;

fn find(mods: &Path, out: &Path, rel: &str) -> Option<PathBuf> {
    let o = out.join(rel);
    if o.exists() {
        return Some(o);
    }
    // Loose files in any mod (first match; good enough for a survey).
    for e in std::fs::read_dir(mods).ok()?.flatten() {
        let p = e.path().join(rel);
        if p.exists() {
            return Some(p);
        }
    }
    None
}

fn tris(path: &Path, rel: &str) -> Option<Vec<[Vec3; 3]>> {
    let m = lodforge_objlod::mesh::import(rel, &std::fs::read(path).ok()?).ok()?;
    let mut t = Vec::new();
    for p in &m.parts {
        for tri in &p.triangles {
            t.push(tri.map(|i| Vec3::from(p.positions[i as usize])));
        }
    }
    Some(t)
}

/// Rasterises triangles projected by `rot` (x, y kept) into a RES x RES mask over `lo..hi`.
fn mask(t: &[[Vec3; 3]], rot: Mat3, lo: [f32; 2], hi: [f32; 2]) -> Vec<bool> {
    let mut m = vec![false; RES * RES];
    let sx = RES as f32 / (hi[0] - lo[0]).max(1e-3);
    let sy = RES as f32 / (hi[1] - lo[1]).max(1e-3);
    for tri in t {
        let p = tri.map(|v| {
            let q = rot * v;
            [(q.x - lo[0]) * sx, (q.y - lo[1]) * sy]
        });
        let (x0, x1) = (p.iter().map(|q| q[0]).fold(f32::MAX, f32::min).floor().max(0.0) as usize, p.iter().map(|q| q[0]).fold(f32::MIN, f32::max).ceil().min(RES as f32 - 1.0) as usize);
        let (y0, y1) = (p.iter().map(|q| q[1]).fold(f32::MAX, f32::min).floor().max(0.0) as usize, p.iter().map(|q| q[1]).fold(f32::MIN, f32::max).ceil().min(RES as f32 - 1.0) as usize);
        let e = |a: [f32; 2], b: [f32; 2], c: [f32; 2]| (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]);
        let area = e(p[0], p[1], p[2]);
        if area.abs() < 1e-9 {
            continue;
        }
        for y in y0..=y1.max(y0) {
            for x in x0..=x1.max(x0) {
                let c = [x as f32 + 0.5, y as f32 + 0.5];
                let (w0, w1, w2) = (e(p[1], p[2], c), e(p[2], p[0], c), e(p[0], p[1], c));
                if (w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0) || (w0 <= 0.0 && w1 <= 0.0 && w2 <= 0.0) {
                    m[y * RES + x] = true;
                }
            }
        }
    }
    m
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let out = PathBuf::from(&a[1]);
    let mods = PathBuf::from(&a[2]);
    let count: usize = a.get(3).and_then(|s| s.parse().ok()).unwrap_or(25);
    let rep = std::fs::read_to_string(out.join(r"SKSE\Plugins\LODForgeRuntime\DetailBake.txt")).unwrap();
    // LOD4 models (DynDOLOD's own `_lod_0` / `_lod_0h` / `_lod_0l`, not LODForge's far tiers), one per LOD model.
    let mut pairs: HashMap<String, String> = HashMap::new();
    for l in rep.lines().filter(|l| !l.starts_with('#')) {
        let c: Vec<&str> = l.split('\t').collect();
        if c.len() < 3 || c[0].contains(r"\lodforge\far\") {
            continue;
        }
        let full = c[1].split(" (alt ").next().unwrap().to_string();
        pairs.entry(c[0].to_string()).or_insert(full);
    }
    let mut pairs: Vec<(String, String)> = pairs.into_iter().filter(|(l, _)| l.contains("_lod_0")).collect();
    pairs.sort();
    let views: Vec<(&str, Mat3)> = {
        let tilt = Mat3::from_rotation_x(-60f32.to_radians());
        vec![
            ("top", Mat3::IDENTITY),
            ("N", tilt),
            ("E", tilt * Mat3::from_rotation_z(90f32.to_radians())),
            ("S", tilt * Mat3::from_rotation_z(180f32.to_radians())),
            ("W", tilt * Mat3::from_rotation_z(270f32.to_radians())),
        ]
    };
    println!("{:<52} {:>6} {:>5}  {:>5} {:>5} {:>5}  (IoU mean / worst view, lost, added: share of the full silhouette)", "LOD model", "full", "lod", "IoU", "worst", "lost");
    let mut rows = Vec::new();
    for (lod, full) in pairs {
        let (Some(fp), Some(lp)) = (find(&mods, &out, &full), find(&mods, &out, &lod)) else { continue };
        let (Some(ft), Some(lt)) = (tris(&fp, &full), tris(&lp, &lod)) else { continue };
        let (mut ious, mut lost_sum, mut add_sum) = (Vec::new(), 0.0, 0.0);
        for (_, rot) in &views {
            let pts = ft.iter().chain(lt.iter()).flatten().map(|v| *rot * *v);
            let (mut lo, mut hi) = ([f32::MAX; 2], [f32::MIN; 2]);
            for q in pts {
                lo = [lo[0].min(q.x), lo[1].min(q.y)];
                hi = [hi[0].max(q.x), hi[1].max(q.y)];
            }
            let (fm, lm) = (mask(&ft, *rot, lo, hi), mask(&lt, *rot, lo, hi));
            let (mut i, mut u, mut f, mut lost, mut add) = (0usize, 0usize, 0usize, 0usize, 0usize);
            for k in 0..fm.len() {
                i += usize::from(fm[k] && lm[k]);
                u += usize::from(fm[k] || lm[k]);
                f += usize::from(fm[k]);
                lost += usize::from(fm[k] && !lm[k]);
                add += usize::from(lm[k] && !fm[k]);
            }
            ious.push(i as f64 / u.max(1) as f64);
            lost_sum += lost as f64 / f.max(1) as f64;
            add_sum += add as f64 / f.max(1) as f64;
        }
        let mean = ious.iter().sum::<f64>() / ious.len() as f64;
        let worst = ious.iter().cloned().fold(1.0, f64::min);
        rows.push((lod.clone(), ft.len(), lt.len(), mean, worst, lost_sum / views.len() as f64, add_sum / views.len() as f64));
    }
    rows.sort_by(|a, b| a.3.total_cmp(&b.3));
    for r in rows.iter().take(count) {
        let name = r.0.rsplit('\\').next().unwrap();
        println!("{:<52} {:>6} {:>5}  {:>5.2} {:>5.2} {:>5.2} {:>5.2}", name, r.1, r.2, r.3, r.4, r.5, r.6);
    }
    let n = rows.len().max(1) as f64;
    println!("{} LOD4 models: mean IoU {:.3}, mean lost {:.3}, mean added {:.3}", rows.len(), rows.iter().map(|r| r.3).sum::<f64>() / n, rows.iter().map(|r| r.5).sum::<f64>() / n, rows.iter().map(|r| r.6).sum::<f64>() / n);
}
