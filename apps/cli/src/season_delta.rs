//! `lodforge season-delta`: which terrain LOD tiles does each season change?

use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use rayon::prelude::*;

use lodforge_esp::{land_textures, scan_worldspaces};
use lodforge_seasons::{affected_tiles, cell_textures, effective_swaps, load_swaps, Season, TextureKind};

use crate::profile::ProfileArgs;

const LEVELS: [i32; 4] = [4, 8, 16, 32];

pub fn run(args: &ProfileArgs, world_name: &str, golden: Option<&Path>, margin: i32) -> Result<()> {
    let p = args.open()?;
    let lo = &p.load_order;
    let t = Instant::now();
    let worlds = scan_worldspaces(lo)?;
    let Some(world) = worlds.iter().find(|w| w.editor_id.eq_ignore_ascii_case(world_name)) else { bail!("worldspace {world_name} not found") };
    let ltex = land_textures(lo)?;
    let edids: HashMap<String, u32> = ltex.iter().map(|(id, t)| (t.editor_id.to_ascii_lowercase(), *id)).collect();
    let swaps = load_swaps(&p.vfs, lo, &edids)?;
    let cells = cell_textures(lo, world)?;
    let plan_ms = t.elapsed().as_millis();

    println!("{}: {} cells with LAND, {} LTEX in load order; planned in {plan_ms} ms", world.editor_id, cells.len(), ltex.len());
    for (s, files) in &swaps.files {
        let n = swaps.land_textures(*s).map_or(0, |m| m.len());
        println!("  {}: {n} LTEX swaps from {}", s.code(), files.join(", "));
    }
    if !swaps.unresolved.is_empty() {
        println!("  unresolved swap lines: {} (first: {})", swaps.unresolved.len(), swaps.unresolved[0]);
    }

    let golden_dir = golden.map(|g| g.join("output").join("Textures").join("Terrain").join(&world.editor_id));
    if let Some(d) = &golden_dir {
        if !d.is_dir() {
            bail!("golden texture folder not found: {}", d.display());
        }
    }

    println!("\n{:<4} {:>5} {:<8} {:>6} {:>9} {:>7} {:>7} {:>7}", "seas", "level", "kind", "tiles", "predicted", "actual", "missed", "extra");
    let (mut total_missed, mut total_extra) = (0usize, 0usize);
    for season in Season::ALL {
        let Some(swapped) = swaps.land_textures(season) else { continue };
        for level in LEVELS {
            for (kind, tk) in [("diffuse", TextureKind::Diffuse), ("normal", TextureKind::Normal)] {
                let predicted = affected_tiles(&cells, &effective_swaps(swapped, &ltex, tk), level, margin);
                let Some(dir) = &golden_dir else {
                    println!("{:<4} {:>5} {:<8} {:>6} {:>9}", season.code(), level, kind, "-", predicted.len());
                    continue;
                };
                let (tiles, actual) = golden_diffs(dir, &world.editor_id, level, kind, season).with_context(|| format!("comparing golden {} {level} {kind}", season.code()))?;
                let missed: Vec<_> = actual.difference(&predicted).copied().collect();
                let extra = predicted.intersection(&tiles).filter(|t| !actual.contains(t)).count();
                total_missed += missed.len();
                total_extra += extra;
                println!("{:<4} {:>5} {:<8} {:>6} {:>9} {:>7} {:>7} {:>7}", season.code(), level, kind, tiles.len(), predicted.intersection(&tiles).count(), actual.len(), missed.len(), extra);
                if !missed.is_empty() {
                    println!("       missed e.g. {:?}", &missed[..missed.len().min(6)]);
                }
            }
        }
    }
    if golden_dir.is_some() {
        println!("\ntotal missed {total_missed} (tiles that changed but were not predicted), extra {total_extra} (predicted but unchanged)");
    }
    Ok(())
}

/// All tiles of a level/kind in the golden default season, and those whose season file differs.
fn golden_diffs(dir: &Path, world: &str, level: i32, kind: &str, season: Season) -> Result<(BTreeSet<(i32, i32)>, BTreeSet<(i32, i32)>)> {
    let suffix = if kind == "normal" { "_n" } else { "" };
    let prefix = format!("{world}.{level}.").to_ascii_lowercase();
    let mut tiles = Vec::new();
    for e in std::fs::read_dir(dir)? {
        let name = e?.file_name().to_string_lossy().to_ascii_lowercase();
        let Some(rest) = name.strip_prefix(&prefix).and_then(|r| r.strip_suffix(&format!("{suffix}.dds"))) else { continue };
        let parts: Vec<&str> = rest.split('.').collect();
        // Default-season files only: "<x>.<y>" (season files have a third part).
        if parts.len() != 2 || (suffix.is_empty() && rest.ends_with("_n")) {
            continue;
        }
        if let (Ok(x), Ok(y)) = (parts[0].parse::<i32>(), parts[1].parse::<i32>()) {
            tiles.push((x, y));
        }
    }
    let changed: Vec<(i32, i32)> = tiles
        .par_iter()
        .filter(|&&(x, y)| {
            let a = dir.join(format!("{world}.{level}.{x}.{y}{suffix}.dds"));
            let b = dir.join(format!("{world}.{level}.{x}.{y}.{}{suffix}.dds", season.code()));
            match (std::fs::read(&a), std::fs::read(&b)) {
                (Ok(a), Ok(b)) => a != b,
                _ => true,
            }
        })
        .copied()
        .collect();
    Ok((tiles.into_iter().collect(), changed.into_iter().collect()))
}
