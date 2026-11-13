//! Measures the shading check of `lodforge_lodmodel` (`LevelParams::shade_tol`) and the checked
//! triangle cap (`checked_cap`) on full models: for every model below a folder and each level,
//! the silhouette and shading difference of today's result, of the result when the cap gives way
//! to the silhouette check, and of that with a shading tolerance enforced as well, with what each
//! costs in triangles and time.
//!
//! `cargo run --release --example lodmodel_shade -- <folder> [--tol 0.15] [--max 300]
//!  [--csv out.csv] [--skip interior,lod,...]`
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use lodforge_lodmodel::{LEVELS, LevelParams, Stats};
use rayon::prelude::*;

fn walk(dir: &Path, skip: &[String], out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let lower = p.to_string_lossy().to_ascii_lowercase();
        if skip.iter().any(|s| lower.contains(s.as_str())) {
            continue;
        }
        if p.is_dir() {
            walk(&p, skip, out);
        } else if lower.ends_with(".nif") {
            out.push(p);
        }
    }
}

const VARIANTS: [&str; 3] = ["before", "checked cap", "checked cap + shading"];

struct Row {
    path: String,
    level: usize,
    /// Per variant: stats and seconds.
    v: [(Stats, f32); 3],
}

fn main() -> Result<()> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut dir = None;
    let (mut tol, mut max, mut csv) = (0.15f32, 300usize, None);
    let mut skip: Vec<String> = ["interior", "lod", r"\clutter\", r"\doors\"].iter().map(|s| s.to_string()).collect();
    let mut i = 0;
    while i < a.len() {
        let next = || a.get(i + 1).with_context(|| format!("{} needs a value", a[i]));
        match a[i].as_str() {
            "--tol" => (tol, i) = (next()?.parse()?, i + 1),
            "--max" => (max, i) = (next()?.parse()?, i + 1),
            "--csv" => (csv, i) = (Some(PathBuf::from(next()?)), i + 1),
            "--skip" => (skip, i) = (next()?.split(',').map(|s| s.to_ascii_lowercase()).collect(), i + 1),
            s => dir = Some(PathBuf::from(s)),
        }
        i += 1;
    }
    let dir = dir.context("usage: lodmodel_shade <folder> [--tol t] [--max n] [--csv file] [--skip a,b]")?;
    let mut files = Vec::new();
    walk(&dir, &skip, &mut files);
    files.sort();
    // An even sample over the sorted list.
    let step = files.len().div_ceil(max.max(1)).max(1);
    let files: Vec<PathBuf> = files.into_iter().step_by(step).collect();
    println!("{} models, shading tolerance {tol}", files.len());

    let rows: Vec<Row> = files
        .par_iter()
        .flat_map_iter(|f| {
            let mut rows = Vec::new();
            let Ok(bytes) = std::fs::read(f) else { return rows };
            let Ok(nif) = lodforge_nif::parse(&bytes) else { return rows };
            for (level, p) in LEVELS.iter().enumerate() {
                // 1e6: the shading is measured, never enforced.
                let variants = [LevelParams { shade_tol: 1e6, checked_cap: false, keep_glow: false, ..*p }, LevelParams { shade_tol: 1e6, checked_cap: true, ..*p }, LevelParams { shade_tol: tol, checked_cap: true, ..*p }];
                let mut v: Vec<(Stats, f32)> = Vec::new();
                for q in &variants {
                    let t = Instant::now();
                    if let Ok((_, st)) = lodforge_lodmodel::generate_nif(&nif, q, "m") {
                        v.push((st, t.elapsed().as_secs_f32()));
                    }
                }
                if let Ok(v) = <[(Stats, f32); 3]>::try_from(v) {
                    rows.push(Row { path: f.strip_prefix(&dir).unwrap_or(f).to_string_lossy().to_string(), level, v });
                }
            }
            rows
        })
        .collect();

    if let Some(csv) = &csv {
        let mut s = String::from("model,level,source_triangles");
        for n in VARIANTS {
            s += &format!(",{n} triangles,{n} fill,{n} loss,{n} shade,{n} error limit,{n} seconds");
        }
        s.push('\n');
        for r in &rows {
            s += &format!("{},{},{}", r.path, r.level, r.v[0].0.source_triangles);
            for (st, t) in &r.v {
                s += &format!(",{},{:.3},{:.3},{:.3},{},{t:.3}", st.triangles, st.fill, st.loss, st.shade, st.error_limit);
            }
            s.push('\n');
        }
        std::fs::write(csv, s)?;
    }
    for (level, p) in LEVELS.iter().enumerate() {
        let rs: Vec<&Row> = rows.iter().filter(|r| r.level == level).collect();
        if rs.is_empty() {
            continue;
        }
        println!("level {level}: {} models, {} source triangles", rs.len(), rs.iter().map(|r| r.v[0].0.source_triangles).sum::<usize>());
        for (k, name) in VARIANTS.iter().enumerate() {
            let mut shades: Vec<f32> = rs.iter().map(|r| r.v[k].0.shade).collect();
            shades.sort_by(f32::total_cmp);
            let q = |f: f32| shades[((shades.len() - 1) as f32 * f) as usize];
            let outline = rs.iter().filter(|r| r.v[k].0.fill > p.fill_tol || r.v[k].0.loss > p.loss_tol).count();
            let lit = rs.iter().filter(|r| r.v[k].0.shade > tol).count();
            let tris: usize = rs.iter().map(|r| r.v[k].0.triangles).sum();
            let secs: f32 = rs.iter().map(|r| r.v[k].1).sum();
            println!("  {name:<22} {tris:>8} triangles, {secs:>6.1} s; silhouette out of tolerance {outline:>3}, shading over {tol} {lit:>3}; shading median {:.3} p90 {:.3} max {:.3}", q(0.5), q(0.9), q(1.0));
        }
        let mut worst: Vec<&&Row> = rs.iter().collect();
        worst.sort_by(|x, y| y.v[0].0.shade.total_cmp(&x.v[0].0.shade));
        for r in worst.iter().take(5) {
            let [a, b, c] = &r.v;
            println!("    shade {:.2} / {:.2} / {:.2}, triangles {} / {} / {} of {}  {}", a.0.shade, b.0.shade, c.0.shade, a.0.triangles, b.0.triangles, c.0.triangles, a.0.source_triangles, r.path);
        }
    }
    Ok(())
}
