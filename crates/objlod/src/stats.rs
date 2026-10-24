//! Build counters, shared by the parallel quad builders.

use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Debug, Default)]
pub struct Stats {
    /// Per level index: triangles placed before and after hidden-face removal (equal until
    /// stage B2.3).
    pub total_tris: [AtomicUsize; 4],
    pub reduced_tris: [AtomicUsize; 4],
    /// Per level index: triangles dropped by the hidden-face test.
    pub culled_tris: [AtomicUsize; 4],
    /// Reference placements that found their LOD model / did not.
    pub placed: AtomicUsize,
    pub missing_meshes: AtomicUsize,
    /// Tree billboards placed (LOD model columns naming a billboard texture with a FlatTextures
    /// line) and the ones without a line, skipped.
    pub billboards: AtomicUsize,
    pub billboards_missing: AtomicUsize,
    /// Grass instances placed (LOD4) and the ones whose type has no billboard.
    pub grass_instances: AtomicUsize,
    pub grass_unmapped: AtomicUsize,
    /// Shapes written with their source shader (passthru models).
    pub passthru: AtomicUsize,
    /// Shapes split at the 65,535 vertex / triangle limit.
    pub splits: AtomicUsize,
    /// Shapes whose textures came from the AltTextures side file.
    pub alt_textures: AtomicUsize,
    /// Source shapes skipped while importing models (hidden, skinned, no shader, ...).
    pub skipped_parts: AtomicUsize,
    /// Shapes named HD.
    pub hd_shapes: AtomicUsize,
    /// Source parts remapped into an atlas tile, and those re-cut along UV tile edges first.
    pub atlased_parts: AtomicUsize,
    pub reuv_parts: AtomicUsize,
    /// Parts re-cut at their atlas tile edges because their UVs overshoot the tile.
    pub overshoot_parts: AtomicUsize,
    /// Parts whose colours the VC side file marked white (dropped).
    pub vc_whitened: AtomicUsize,
    /// Placements written as runtime instances, and runtime references dropped (billboards).
    pub instanced: AtomicUsize,
    pub runtime_dropped: AtomicUsize,
}

impl Stats {
    pub fn add(c: &AtomicUsize, n: usize) {
        c.fetch_add(n, Ordering::Relaxed);
    }

    pub fn get(c: &AtomicUsize) -> usize {
        c.load(Ordering::Relaxed)
    }
}
