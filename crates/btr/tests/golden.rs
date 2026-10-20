//! Golden terrain BTRs against the BTR assembly: reading a golden file into the [`Btr`]
//! description and assembling it again reproduces the bytes (so the block layout, names, tables
//! and field values of docs/re/terrain-lod-spec.md section 1 are the ones we write); the bound
//! rules of 1.4 / 1.7 / 1.8 reproduce the stored bounds; the UV halves are what `f32_to_half`
//! produces. Skipped when the golden folder is missing.

use std::path::{Path, PathBuf};

use lodforge_btr::bounds::BBox;
use lodforge_btr::btr::{Btr, parse_file_name, texture_paths};
use lodforge_nif::{f32_to_half, half_to_f32, parse};

const DEFAULT_DIR: &str =
    r"D:\Modlists\LODForge-golden\no-seasons-20261017-031139\output\meshes\terrain\Tamriel";

fn golden_files() -> Option<Vec<PathBuf>> {
    let dir = std::env::var("LODFORGE_GOLDEN_BTR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_DIR));
    if !dir.is_dir() {
        eprintln!("golden BTR folder {} missing, skipping", dir.display());
        return None;
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("btr")))
        .collect();
    files.sort();
    Some(files)
}

fn name(p: &Path) -> String {
    p.file_name().unwrap().to_string_lossy().into_owned()
}

/// Golden bytes -> parse -> `Btr::from_nif` -> `to_nif` -> bytes, with the golden creator string.
#[test]
fn golden_btr_assembly_round_trip() {
    let Some(files) = golden_files() else { return };
    let mut failures = Vec::new();
    for p in &files {
        let bytes = std::fs::read(p).unwrap();
        let nif = parse(&bytes).unwrap();
        let (world, level, x, y, season) = parse_file_name(&name(p)).unwrap();
        let Some(mut btr) = Btr::from_nif(&nif) else {
            failures.push(format!("{}: not a BTR layout", name(p)));
            continue;
        };
        btr.x = x;
        btr.y = y;
        assert_eq!(btr.level, level);
        assert_eq!(
            Btr::textures(&nif),
            texture_paths(&world, level, x, y, season.as_deref())
        );
        let mut ours = btr.to_nif(&world, season.as_deref());
        ours.header.author = nif.header.author.clone();
        let out = ours.to_bytes().unwrap();
        if out != bytes {
            let off = out
                .iter()
                .zip(&bytes)
                .position(|(a, b)| a != b)
                .unwrap_or(out.len().min(bytes.len()));
            failures.push(format!(
                "{}: first mismatch at {off:#x} (golden {} bytes, ours {})",
                name(p),
                bytes.len(),
                out.len()
            ));
        }
    }
    println!(
        "assembly round trip: {}/{} byte-identical",
        files.len() - failures.len(),
        files.len()
    );
    for f in failures.iter().take(20) {
        println!("  {f}");
    }
    assert!(failures.is_empty());
}

/// Bound sphere and multibound rules reproduce the stored values.
#[test]
fn golden_btr_bounds() {
    let Some(files) = golden_files() else { return };
    let (
        mut land,
        mut land_ok,
        mut water,
        mut water_ok,
        mut aabb_ok,
        mut water_aabb_ok,
        mut waters,
    ) = (0, 0, 0, 0, 0, 0, 0);
    let close = |a: [f32; 3], b: [f32; 3], tol: f32| (0..3).all(|i| (a[i] - b[i]).abs() <= tol);
    for p in &files {
        let nif = parse(&std::fs::read(p).unwrap()).unwrap();
        let btr = Btr::from_nif(&nif).unwrap();
        let lvl = btr.level as f32;
        land += 1;
        let lb = BBox::from_points(&btr.land.positions);
        land_ok += usize::from(
            close(lb.center(false), btr.land.bound_center, 0.05)
                && (lb.radius() - btr.land.bound_radius).abs() <= 0.05,
        );
        let la = lb.lod_aabb(lvl);
        aabb_ok += usize::from(
            close(la.position, btr.land_aabb.position, 1.0)
                && close(la.extent, btr.land_aabb.extent, 1.0),
        );
        if let Some(wa) = btr.water_aabb {
            waters += 1;
            let mut wb = BBox::default();
            for s in &btr.water {
                for v in &s.positions {
                    wb.add(*v);
                }
            }
            let ours = wb.lod_aabb(lvl);
            water_aabb_ok += usize::from(
                close(ours.position, wa.position, 1.0) && close(ours.extent, wa.extent, 1.0),
            );
            for s in &btr.water {
                water += 1;
                let b = BBox::from_points(&s.positions);
                water_ok += usize::from(
                    close(b.center(false), s.bound_center, 0.05)
                        && (b.max_radius(&s.positions) - s.bound_radius).abs() <= 0.05,
                );
            }
        }
    }
    println!(
        "land bounds {land_ok}/{land}, land AABB {aabb_ok}/{land}, water bounds {water_ok}/{water}, water AABB {water_aabb_ok}/{waters}"
    );
    assert!(land > 0);
    assert_eq!(
        (land_ok, aabb_ok, water_ok, water_aabb_ok),
        (land, land, water, waters)
    );
}

/// Land UVs are (x / 4096, 1 - y / 4096) of the quad-local position; the stored halves are what
/// `f32_to_half` gives for grid vertices.
#[test]
fn golden_uv_half_rounding() {
    let Some(files) = golden_files() else { return };
    let (mut total, mut matched) = (0usize, 0usize);
    for p in &files {
        let nif = parse(&std::fs::read(p).unwrap()).unwrap();
        let btr = Btr::from_nif(&nif).unwrap();
        let step = 128.0 / btr.level as f32;
        for (pos, uv) in btr.land.positions.iter().zip(&btr.land.uvs) {
            let (gx, gy) = (pos[0] / step, pos[1] / step);
            if gx.fract() != 0.0 || gy.fract() != 0.0 {
                continue;
            }
            for (f, stored) in [(pos[0] / 4096.0, uv[0]), (1.0 - pos[1] / 4096.0, uv[1])] {
                total += 1;
                matched += usize::from(half_to_f32(f32_to_half(f)) == stored);
            }
        }
    }
    println!("UV halves: {matched}/{total} match");
    assert!(total > 0);
    assert_eq!(matched, total);
}
