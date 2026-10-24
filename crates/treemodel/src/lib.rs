//! Generated 3D tree LOD models ("passthru" LOD for DynDOLOD's Ultra tree LOD), ported from
//! TreeLODGen (docs/ultra-tree-models-plan.md, algorithm in docs/re/treelodgen-algorithm.md).
//!
//! Derived from TreeLODGen, Copyright (C) 2026 Clemus, https://github.com/Cl3mus33/TreeLODGen
//! (commit dc4cc8a, GPL-3.0): `Split.cs`, `WoodSplit.cs`, `Islands.cs`, `NifUtil.cs`, `Prune.cs`,
//! `Simplify.cs`, `Textures.cs`. Ported to Rust and modified for LODForge, 2026-10-23.
//!
//! Pipeline per tree model: [collect] the source shapes (static copy preferred, node transforms
//! without the root's), [split] them into bark ("trunk") and foliage ("crown") parts, then per LOD
//! level prune the crown and simplify the bark ([levels]) and write the LOD model ([write]); [generate]
//! does it all for one tree.
//! Nothing here reads files: textures come in through a lookup closure.

pub mod collect;
pub mod islands;
pub mod levels;
pub mod mesh;
pub mod prune;
pub mod simplify;
pub mod split;
pub mod texture;
pub mod write;
pub mod xf;

pub use collect::{SourceShape, collect};
pub use split::{Kind, Part, Reason, split};
pub use texture::AlphaImage;

/// The LOD models of one tree.
#[derive(Debug, Default)]
pub struct Generated {
    /// NIF bytes per level (LOD4, 8, 16); `None` when a level came out empty.
    pub levels: Vec<Option<Vec<u8>>>,
    /// Source leaf and bark triangles.
    pub crown_source: usize,
    pub trunk_source: usize,
    /// Leaf and bark triangles per level.
    pub triangles: Vec<(usize, usize)>,
    pub skinned_only: bool,
    pub notes: Vec<String>,
}

/// Why a tree produced nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
    Parse(String),
    /// No shape with triangles, or nothing classified.
    NoGeometry,
    Write(String),
}

/// Generates the levels of one tree from its full model.
pub fn generate(full: &[u8], texture: &dyn Fn(&str) -> Option<std::sync::Arc<AlphaImage>>, params: &[levels::LevelParams]) -> Result<Generated, Failure> {
    let nif = lodforge_nif::parse_all_geometry(full).map_err(|e| Failure::Parse(e.to_string()))?;
    let shapes = collect(&nif);
    let mut notes = split::Notes::default();
    let parts = split(&nif, &shapes, texture, &mut notes);
    if parts.is_empty() {
        return Err(Failure::NoGeometry);
    }
    let sum = |k: Kind| parts.iter().filter(|p| p.kind == k).map(|p| p.triangles).sum::<usize>();
    let mut g = Generated { crown_source: sum(Kind::Crown), trunk_source: sum(Kind::Trunk), skinned_only: shapes.iter().any(|s| s.skinned), notes: notes.0, ..Default::default() };
    if g.crown_source == 0 {
        g.notes.push("bare tree: bark only LOD".into());
    }
    for level in levels::build(&nif, &shapes, &parts, params) {
        g.triangles.push((level.triangles(Kind::Crown), level.triangles(Kind::Trunk)));
        let bytes = match write::level_nif(&nif, &shapes, &level, &mut g.notes) {
            Some(out) => Some(out.to_bytes().map_err(|e| Failure::Write(e.to_string()))?),
            None => None,
        };
        g.levels.push(bytes);
    }
    if g.levels.iter().all(Option::is_none) {
        return Err(Failure::NoGeometry);
    }
    Ok(g)
}
