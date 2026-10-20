//! Reference -> quad assignment (docs/re/object-lod-spec.md 2.3, 1.4): a reference goes into the
//! block of its position at every level it has a model for; quads outside `LimitCoords<L>` are
//! not generated; in a DynDOLOD season export the quads not listed in `LODCoords<L>` are copies
//! of the default season.
//!
//! The bucketing follows AnvilLOD `AnvilLOD.Core/World/QuadBucketer.cs` (Copyright (c) 2026
//! CageTV, GPL-3.0-only, commit 606eece): inside-grid test, one bucket per (level, block) for
//! every level the reference has a mesh for, a deterministic order inside each bucket.
//! Modified: single-threaded over the parsed export, ordered by line number instead of form key,
//! plus the `LimitCoords` / `LODCoords` rules, which AnvilLOD does not have.

use std::collections::{BTreeMap, HashSet};

use crate::export::ExportFile;
use crate::grid::{LEVELS, LodGrid, Quad};

/// The quads of an export with the indices (into `export.statics`) of the references in each.
#[derive(Debug, Default)]
pub struct Buckets {
    pub quads: BTreeMap<Quad, Vec<usize>>,
    /// References whose cell is outside the grid.
    pub outside: usize,
    /// References without a model at any built level.
    pub without_model: usize,
    /// Quads left out by `LimitCoords`.
    pub limited: usize,
}

/// Assigns every reference line to its quads. `levels` are the levels to build (a LOD32 column
/// is used only when `levels` contains 32).
pub fn bucket(export: &ExportFile, grid: &LodGrid, levels: &[i32]) -> Buckets {
    let mut b = Buckets::default();
    let limits: [Option<HashSet<(i32, i32)>>; 4] = std::array::from_fn(|k| export.limit_coords[k].as_ref().map(|v| v.iter().copied().collect()));
    for (i, r) in export.statics.iter().enumerate() {
        let cell = r.cell();
        if !grid.is_inside(cell) {
            b.outside += 1;
            continue;
        }
        let mut any = false;
        for (k, &level) in LEVELS.iter().enumerate() {
            if !levels.contains(&level) || r.model(k).is_none() {
                continue;
            }
            any = true;
            let q = grid.quad_for(cell, level);
            if let Some(l) = &limits[k]
                && !l.contains(&(q.x, q.y))
            {
                continue;
            }
            b.quads.entry(q).or_default().push(i);
        }
        if !any {
            b.without_model += 1;
        }
    }
    for k in 0..4 {
        if let Some(l) = &limits[k] {
            b.limited += l.len().saturating_sub(b.quads.keys().filter(|q| q.level == LEVELS[k]).count());
        }
    }
    b
}

/// Whether a quad of a DynDOLOD season export is a copy of the default season's file (spec 1.4):
/// the export names a season, carries `LODCoords`, and the quad is not among them.
pub fn is_season_copy(export: &ExportFile, q: &Quad) -> bool {
    if export.season.is_empty() {
        return false;
    }
    match &export.lod_coords {
        Some(lc) => !lc[crate::grid::level_index(q.level)].contains(&(q.x, q.y)),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::StaticRef;

    fn r(x: f32, y: f32, models: [&str; 4]) -> StaticRef {
        let mut s = StaticRef { x, y, models: models.map(str::to_string), ..Default::default() };
        s.update_cell();
        s
    }

    #[test]
    fn buckets_and_limits() {
        let mut e = ExportFile { south_west: (-96, -96), ..Default::default() };
        e.statics = vec![
            r(100.0, 100.0, ["a.nif", "a.nif", "", ""]),
            r(5000.0, 100.0, ["a.nif", "", "", "b.nif"]),
            r(-500_000.0, 0.0, ["a.nif", "", "", ""]),
            r(100.0, 100.0, ["", "", "", ""]),
        ];
        for (i, s) in e.statics.iter_mut().enumerate() {
            s.line = i;
        }
        let g = LodGrid::new((-96, -96), Some(256));
        let b = bucket(&e, &g, &[4, 8, 16]);
        assert_eq!(b.outside, 1);
        assert_eq!(b.without_model, 1);
        let q4 = Quad { level: 4, x: 0, y: 0 };
        assert_eq!(b.quads[&q4], vec![0, 1]);
        assert_eq!(b.quads[&Quad { level: 8, x: 0, y: 0 }], vec![0]);
        assert!(!b.quads.contains_key(&Quad { level: 32, x: 0, y: 0 }));
        let b = bucket(&e, &g, &[4, 8, 16, 32]);
        assert_eq!(b.quads[&Quad { level: 32, x: 0, y: 0 }], vec![1]);
        e.limit_coords[0] = Some(vec![(4, 0)]);
        let b = bucket(&e, &g, &[4, 8]);
        assert!(!b.quads.contains_key(&q4));
        assert_eq!(b.limited, 1);
        e.season = "WIN".into();
        e.lod_coords = Some([vec![(0, 0)], vec![], vec![], vec![]]);
        assert!(!is_season_copy(&e, &q4));
        assert!(is_season_copy(&e, &Quad { level: 8, x: 0, y: 0 }));
        e.season.clear();
        assert!(!is_season_copy(&e, &Quad { level: 8, x: 0, y: 0 }));
    }
}
