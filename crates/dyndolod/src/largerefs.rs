//! DynDOLOD's large-reference workarounds (`WorkaroundLargeReferencesBugs=1`): which cells get a
//! `_Disciple_` marker in DynDOLOD.esm, and what the DLL NG data file says about them
//! (docs/re/dyndolod-spec.md "Large-reference workarounds"; rules matched against the E2E golden
//! output, the DLL's handling from DynDOLOD.DLL's LoadData / LargeREFRsFixAttachDetach).
//!
//! The engine's large references stop unloading their object LOD when one of them changes state
//! at runtime. The DLL works around that per cell: when the cell enters the large-reference grid
//! it toggles the cell's marker (itself a large reference) so the engine re-evaluates the cell, and
//! it enables or disables the cell's persistent large references with the grid.

use std::collections::HashMap;

use lodforge_esp::{LoadOrder, Worldspace};

use crate::bases::Base;

/// REFR record flags.
const INITIALLY_DISABLED: u32 = 0x800;
const IS_FULL_LOD: u32 = 0x10000;
/// STAT record flag "Is Marker".
const BASE_IS_MARKER: u32 = 0x0080_0000;

/// One `[Markers]` cell.
#[derive(Debug, Clone, PartialEq)]
pub struct MarkerCell {
    pub grid: (i32, i32),
    /// Large references the engine lists in the cell, plus the marker itself (the DLL toggles
    /// the marker this many times plus 3).
    pub count: usize,
    /// Persistent, initially disabled large references of the cell without an enable parent,
    /// which the DLL enables and disables with the large-reference grid (global form ids).
    pub persistent: Vec<u32>,
}

/// Cells of a worldspace (one with its own object LOD) that get a marker: those where one of the
/// large references the RNAM lists for the cell can change state or is not what the master
/// placed: it has an enable parent, is initially disabled, its winning record is from a non-master
/// plugin, or it is persistent without Is Full LOD. Golden Tamriel: 861 of 873 cells, no extra
/// cells; MarkarthWorld 4 of 4.
pub fn marker_cells(lo: &LoadOrder, world: &Worldspace, bases: &HashMap<u32, Base>) -> Vec<MarkerCell> {
    let is_master = |pi: u16| lo.plugins.get(usize::from(pi)).is_some_and(|p| p.plugin.is_master());
    let mut cells: HashMap<(i32, i32), (usize, bool, Vec<u32>)> = HashMap::new();
    for (&id, &grid) in &world.large_ref_cells {
        let e = cells.entry(grid).or_default();
        e.0 += 1;
        let Some(r) = world.refs.get(&id) else { continue };
        let disabled = r.flags & INITIALLY_DISABLED != 0;
        if r.enable_parent || disabled || !is_master(r.plugin) || (r.is_persistent() && r.flags & IS_FULL_LOD == 0) {
            e.1 = true;
        }
        let marker_base = bases.get(&r.base).is_some_and(|b| b.flags & BASE_IS_MARKER != 0);
        if r.is_persistent() && disabled && !r.enable_parent && is_master(r.plugin) && !marker_base {
            e.2.push(id);
        }
    }
    let mut out: Vec<MarkerCell> = cells
        .into_iter()
        .filter(|(_, e)| e.1)
        .map(|(grid, (n, _, mut persistent))| {
            persistent.sort_unstable();
            MarkerCell { grid, count: n + 1, persistent }
        })
        .collect();
    out.sort_by_key(|m| m.grid);
    out
}

/// Cells of a child worldspace that uses its parent's LOD: the childworld configuration's
/// `Bounds` (SW x, SW y, NE x, NE y) widened by `margin` cells ((uLargeRefLODGridSize -
/// uGridsToLoad) / 2), leaving out cells with a navmesh when the configuration says
/// `NoCellsWithNAVM=1`. Golden: Riften and Windhelm exact; DynDOLOD leaves out one more row
/// (Whiterun) or two more columns (Solitude) at the north/east edge.
pub fn child_marker_cells(world: &Worldspace, bounds: (i32, i32, i32, i32), margin: i32, no_navmesh_cells: bool) -> Vec<MarkerCell> {
    let (x0, y0, x1, y1) = bounds;
    let mut out = Vec::new();
    for x in x0 - margin..=x1 + margin {
        for y in y0 - margin..=y1 + margin {
            if no_navmesh_cells && world.navmesh_cells.contains(&(x, y)) {
                continue;
            }
            let count = world.large_ref_cells.values().filter(|g| **g == (x, y)).count() + 1;
            out.push(MarkerCell { grid: (x, y), count, persistent: Vec::new() });
        }
    }
    out
}

/// Persistent Is Full LOD references among `lod_refs` (references that get object, tree or
/// dynamic LOD): DynDOLOD overrides them without Is Full LOD (`_DynDOLOD_REVERT`, in DynDOLOD.esm
/// for large references, else DynDOLOD.esp) and lists them in `[Resets]`, where the DLL runs
/// `recycleactor` on them after loading so saved games pick the change up.
pub fn revert_refs(world: &Worldspace, lod_refs: &[u32]) -> Vec<u32> {
    let mut v: Vec<u32> = lod_refs.iter().copied().filter(|id| world.refs.get(id).is_some_and(|r| r.is_persistent() && r.flags & IS_FULL_LOD != 0)).collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// Marker EditorID: `<World>_Disciple_<x>_<y>`, negative numbers written with `%` for the minus.
pub fn marker_editor_id(world: &str, (x, y): (i32, i32)) -> String {
    let n = |v: i32| if v < 0 { format!("%{}", -v) } else { v.to_string() };
    format!("{world}_Disciple_{}_{}", n(x), n(y))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marker_editor_ids() {
        assert_eq!(marker_editor_id("Tamriel", (19, -19)), "Tamriel_Disciple_19_%19");
        assert_eq!(marker_editor_id("WindhelmWorld", (28, 5)), "WindhelmWorld_Disciple_28_5");
    }

    #[test]
    fn child_rectangle() {
        let mut w = Worldspace::default();
        w.navmesh_cells.insert((41, -24));
        w.large_ref_cells.insert(7, (40, -24));
        let all = child_marker_cells(&w, (41, -25, 43, -23), 3, false);
        assert_eq!(all.len(), 81);
        assert_eq!(all.iter().find(|m| m.grid == (40, -24)).map(|m| m.count), Some(2));
        assert_eq!(child_marker_cells(&w, (41, -25, 43, -23), 3, true).len(), 80);
    }
}
