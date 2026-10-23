//! Derived from TreeLODGen, Copyright (C) 2026 Clemus (GPL-3.0, commit dc4cc8a); ported to Rust and
//! modified for LODForge 2026-10-23. See THIRD-PARTY-NOTICES.txt part 1.7.
//!
//! The LOD levels of a tree (TreeLODGen `Split.Process` / `WriteStaticNif` budgets; spec 4.1,
//! 5, 7.2, 9). Per level, from the split parts:
//!
//! - crown parts first, then trunk parts, each in source order;
//! - crown share `max(keep, min(1, floor / crown triangles))` (a sparse crown is not pruned below
//!   the floor), pruning each crown part with that share when it is below 1;
//! - bark budget split over the bark parts in proportion to their triangles, at least 12 each,
//!   doubled for a tree without foliage.
//!
//! Geometry stays in shape-local space; the writer bakes [`LevelPart::xf`].

use lodforge_nif::{Block, Nif};

use crate::collect::SourceShape;
use crate::mesh::PartMesh;
use crate::prune;
use crate::simplify::{MIN_SHAPE_TRIS, simplify};
use crate::split::{Kind, Part};
use crate::xf::Xf;

/// One level's tunables (TreeLODGen `--levels keep:area:bark:floor`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LevelParams {
    /// Share of leaf pieces kept.
    pub keep: f32,
    /// Share of the crown's leaf area restored by growing the kept pieces.
    pub area: f32,
    /// Bark triangles for the whole tree.
    pub bark_tris: usize,
    /// Leaf triangles a crown is never pruned below.
    pub crown_floor: usize,
}

/// TreeLODGen's "Balanced" defaults for LOD 4, 8 and 16.
pub const DEFAULT_LEVELS: [LevelParams; 3] = [
    LevelParams { keep: 0.4, area: 0.8, bark_tris: 400, crown_floor: 2500 },
    LevelParams { keep: 0.15, area: 0.7, bark_tris: 150, crown_floor: 1000 },
    LevelParams { keep: 0.06, area: 0.6, bark_tris: 60, crown_floor: 400 },
];

/// Bark budget factor for trees without foliage.
pub const BARE_TREE_BUDGET: usize = 2;

/// One part of one level.
#[derive(Debug, Clone)]
pub struct LevelPart {
    pub kind: Kind,
    /// Index into the [`crate::collect`] list.
    pub shape: usize,
    pub xf: Xf,
    pub mesh: PartMesh,
    pub source_triangles: usize,
}

#[derive(Debug, Clone, Default)]
pub struct Level {
    pub parts: Vec<LevelPart>,
    /// Crown share used (1 = untouched).
    pub keep: f32,
}

impl Level {
    pub fn triangles(&self, kind: Kind) -> usize {
        self.parts.iter().filter(|p| p.kind == kind).map(|p| p.mesh.triangle_count()).sum()
    }
}

/// The levels of a tree from its split parts.
pub fn build(nif: &Nif, shapes: &[SourceShape], parts: &[Part], levels: &[LevelParams]) -> Vec<Level> {
    let crown_full: usize = parts.iter().filter(|p| p.kind == Kind::Crown).map(|p| p.triangles).sum();
    let bare = crown_full == 0;
    let ordered: Vec<&Part> = parts.iter().filter(|p| p.kind == Kind::Crown).chain(parts.iter().filter(|p| p.kind == Kind::Trunk)).collect();
    levels
        .iter()
        .map(|lp| {
            let keep = if crown_full > 0 { lp.keep.max((lp.crown_floor as f32 / crown_full as f32).min(1.0)) } else { 1.0 };
            let mut out: Vec<LevelPart> = ordered
                .iter()
                .filter_map(|p| {
                    let Some(Block::TriShape(s)) = nif.blocks.get(shapes[p.shape].block) else { return None };
                    let mesh = PartMesh::from_shape(s, p.vertices.as_deref());
                    let source_triangles = mesh.triangle_count();
                    Some(LevelPart { kind: p.kind, shape: p.shape, xf: shapes[p.shape].xf, mesh, source_triangles })
                })
                .collect();
            let bark_total: usize = out.iter().filter(|p| p.kind == Kind::Trunk).map(|p| p.mesh.triangle_count()).sum();
            let bark_budget = lp.bark_tris * if bare { BARE_TREE_BUDGET } else { 1 };
            for p in &mut out {
                match p.kind {
                    Kind::Crown if keep < 1.0 => {
                        prune::apply(&mut p.mesh, keep, lp.area);
                        prune::clamp_small_uv_overshoot(&mut p.mesh);
                    }
                    Kind::Crown => {}
                    Kind::Trunk => {
                        let target = MIN_SHAPE_TRIS.max(bark_budget * p.mesh.triangle_count() / bark_total.max(1));
                        simplify(&mut p.mesh, target);
                    }
                }
            }
            Level { parts: out, keep }
        })
        .collect()
}
