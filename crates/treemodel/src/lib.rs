//! Generated 3D tree LOD models ("passthru" LOD for DynDOLOD's Ultra tree LOD), ported from
//! TreeLODGen (docs/ultra-tree-models-plan.md, algorithm in docs/re/treelodgen-algorithm.md).
//!
//! Derived from TreeLODGen, Copyright (C) 2026 Clemus, https://github.com/Cl3mus33/TreeLODGen
//! (commit dc4cc8a, GPL-3.0): `Split.cs`, `WoodSplit.cs`, `Islands.cs`, `NifUtil.cs`, `Prune.cs`,
//! `Simplify.cs`, `Textures.cs`. Ported to Rust and modified for LODForge, 2026-10-23.
//!
//! Pipeline per tree model: [collect] the source shapes (static copy preferred, node transforms
//! without the root's), [split] them into bark ("trunk") and foliage ("crown") parts, then per LOD
//! level prune the crown and simplify the bark ([levels]); writing the LOD model comes next.
//! Nothing here reads files: textures come in through a lookup closure.

pub mod collect;
pub mod islands;
pub mod levels;
pub mod mesh;
pub mod prune;
pub mod simplify;
pub mod split;
pub mod texture;
pub mod xf;

pub use collect::{SourceShape, collect};
pub use split::{Kind, Part, Reason, split};
pub use texture::AlphaImage;
