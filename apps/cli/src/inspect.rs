//! `lodforge inspect`: load a profile and report what was found, with timings.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use serde::Serialize;

use lodforge_esp::scan_worldspaces;

use crate::profile::ProfileArgs;

#[derive(Serialize)]
struct Timings {
    vfs_ms: u128,
    plugins_ms: u128,
    archives_ms: u128,
    worldspaces_ms: u128,
    total_ms: u128,
}

#[derive(Serialize)]
struct WorldReport {
    form_id: String,
    editor_id: String,
    parent: Option<String>,
    plugins: Vec<String>,
    cells: usize,
    bounds: Option<((i32, i32), (i32, i32))>,
    lands: usize,
    refs: usize,
    persistent_refs: usize,
    lodsettings: bool,
}

#[derive(Serialize)]
struct Report {
    instance: PathBuf,
    profile: String,
    threads: usize,
    loose_files: usize,
    archives: usize,
    archived_files: usize,
    missing_archives: Vec<String>,
    plugins: Vec<String>,
    plugin_mb: f64,
    timings: Timings,
    worldspaces: Vec<WorldReport>,
}

pub fn run(args: &ProfileArgs, json: Option<PathBuf>) -> Result<()> {
    let total = Instant::now();
    let p = args.open()?;
    let (vfs, lo) = (&p.vfs, &p.load_order);

    let t = Instant::now();
    let worlds = scan_worldspaces(lo).context("scanning worldspaces")?;
    let worldspaces_ms = t.elapsed().as_millis();

    let plugin_name = |i: u16| lo.plugins[i as usize].plugin.name.clone();
    let [vfs_ms, plugins_ms, archives_ms] = p.timings_ms;
    let report = Report {
        instance: args.instance.clone(),
        profile: args.profile.clone(),
        threads: rayon::current_num_threads(),
        loose_files: vfs.loose_count(),
        archives: vfs.archives().len(),
        archived_files: vfs.archived_count(),
        missing_archives: p.missing_archives.clone(),
        plugin_mb: lo.plugins.iter().map(|p| p.plugin.bytes().len() as f64).sum::<f64>() / 1048576.0,
        plugins: p.plugin_names.clone(),
        timings: Timings { vfs_ms, plugins_ms, archives_ms, worldspaces_ms, total_ms: total.elapsed().as_millis() },
        worldspaces: worlds
            .iter()
            .map(|w| WorldReport {
                form_id: format!("{:08X}", w.form_id),
                editor_id: w.editor_id.clone(),
                parent: w.parent.map(|p| format!("{p:08X}")),
                plugins: w.plugins.iter().map(|&i| plugin_name(i)).collect(),
                cells: w.cells.len(),
                bounds: w.cell_bounds(),
                lands: w.lands.len(),
                refs: w.refs.len(),
                persistent_refs: w.refs.values().filter(|p| p.is_persistent()).count(),
                lodsettings: vfs.exists(&format!("lodsettings\\{}.lod", w.editor_id)),
            })
            .collect(),
    };

    println!("instance {} / profile {}  ({} threads)", report.instance.display(), report.profile, report.threads);
    println!("VFS: {} loose files, {} archives ({} files), missing archives: {:?}", report.loose_files, report.archives, report.archived_files, report.missing_archives);
    println!("plugins: {} ({:.0} MB)", report.plugins.len(), report.plugin_mb);
    let tm = &report.timings;
    println!("timings: vfs {} ms, plugins {} ms, archives {} ms, worldspaces {} ms, total {} ms", tm.vfs_ms, tm.plugins_ms, tm.archives_ms, tm.worldspaces_ms, tm.total_ms);
    println!("\n{:<10} {:<28} {:>3} {:>6} {:>6} {:>8} {:>6} {:<22} lod", "formid", "editor id", "pl", "cells", "lands", "refs", "pers", "bounds");
    for w in &report.worldspaces {
        let b = w.bounds.map(|((x0, y0), (x1, y1))| format!("{x0},{y0} .. {x1},{y1}")).unwrap_or_default();
        println!(
            "{:<10} {:<28} {:>3} {:>6} {:>6} {:>8} {:>6} {:<22} {}",
            w.form_id,
            w.editor_id,
            w.plugins.len(),
            w.cells,
            w.lands,
            w.refs,
            w.persistent_refs,
            b,
            if w.lodsettings { "yes" } else { "" }
        );
    }
    // Cells reached by more than one LAND record (different form ids at the same grid position).
    for w in &worlds {
        let mut by_cell: std::collections::HashMap<(i32, i32), Vec<(u32, &lodforge_esp::LandRef)>> = std::collections::HashMap::new();
        for (id, l) in &w.lands {
            by_cell.entry(l.cell).or_default().push((*id, l));
        }
        let mut dups: Vec<_> = by_cell.into_iter().filter(|(_, v)| v.len() > 1).collect();
        dups.sort_by_key(|(c, _)| *c);
        for (cell, v) in dups {
            let list: Vec<String> = v.iter().map(|(id, l)| format!("LAND {id:08X} in {}", plugin_name(l.plugin))).collect();
            println!("duplicate LAND in {} cell {},{}: {}", w.editor_id, cell.0, cell.1, list.join(", "));
        }
    }
    if let Some(path) = json {
        std::fs::write(&path, serde_json::to_vec_pretty(&report)?).with_context(|| format!("writing {}", path.display()))?;
        println!("\nreport: {}", path.display());
    }
    Ok(())
}
