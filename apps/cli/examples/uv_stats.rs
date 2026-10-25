//! UV ranges of NIF shapes as stored (no transforms, no shader UV scale): per shape the vertex
//! count, the UV bounding box and how many UVs are non-finite or beyond `--huge` (default 1000).
//! Given a folder, scans every `.nif` below it and prints only shapes with bad UVs.
//!
//! `cargo run --release --example uv_stats -- [--huge 1000] <file.nif | folder>...`
use anyhow::Result;
use lodforge_nif::Block;

fn shapes(path: &std::path::Path, huge: f32, only_bad: bool) -> Result<usize> {
    let nif = lodforge_nif::parse(&std::fs::read(path)?)?;
    let mut bad_shapes = 0;
    for b in &nif.blocks {
        let Block::TriShape(s) = b else { continue };
        let mut r = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        let (mut nonfinite, mut big) = (0usize, 0usize);
        for uv in &s.uvs {
            if !uv[0].is_finite() || !uv[1].is_finite() {
                nonfinite += 1;
                continue;
            }
            if uv[0].abs() > huge || uv[1].abs() > huge {
                big += 1;
            }
            r = [r[0].min(uv[0]), r[1].min(uv[1]), r[2].max(uv[0]), r[3].max(uv[1])];
        }
        let bad = nonfinite + big > 0;
        bad_shapes += usize::from(bad);
        if bad || !only_bad {
            println!("{}\t{:?}\tverts {}\ttris {}\tuv {r:?}\tnon-finite {nonfinite}\tbeyond {huge} {big}", path.display(), s.av.net.name, s.positions.len(), s.triangles.len());
        }
    }
    Ok(bad_shapes)
}

fn main() -> Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut huge = 1000.0f32;
    if let Some(i) = args.iter().position(|a| a == "--huge") {
        args.remove(i);
        huge = args.remove(i).parse()?;
    }
    for a in &args {
        let p = std::path::PathBuf::from(a);
        if p.is_dir() {
            let (mut files, mut bad_files, mut bad_shapes) = (0usize, 0usize, 0usize);
            let mut stack = vec![p];
            while let Some(d) = stack.pop() {
                for e in std::fs::read_dir(&d)?.flatten() {
                    let q = e.path();
                    if q.is_dir() {
                        stack.push(q);
                    } else if q.extension().is_some_and(|x| x.eq_ignore_ascii_case("nif")) {
                        files += 1;
                        let n = shapes(&q, huge, true).unwrap_or(0);
                        bad_files += usize::from(n > 0);
                        bad_shapes += n;
                    }
                }
            }
            println!("{a}: {files} models, {bad_files} with bad UVs ({bad_shapes} shapes)");
        } else {
            shapes(&p, huge, false)?;
        }
    }
    Ok(())
}
