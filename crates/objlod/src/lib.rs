// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! Object LOD meshes (`.bto`) for Skyrim SE, written from `docs/re/object-lod-spec.md` (stage B2
//! of the reimplementation plan): the LODGen export file and atlas map that the xEdit ports in
//! [`source`] and [`atlas_build`] define, the `.lod` grid, the reference transforms and quad
//! assignment, LOD model import through `lodforge-nif`, material grouping, the 65,535 splits and
//! per-cell segments, and the BTO block layout observed in xLODGen / DynDOLOD output, serialized
//! with the `lodforge-nif` writer.
//!
//! Parts of [`transform`], [`refs`], [`mesh`] and [`build`] are derived from AnvilLOD
//! (Copyright (c) 2026 CageTV, GPL-3.0-only, commit 606eece); each file names what it took.
//!
//! Pipeline: [`export::ExportFile`] + [`atlas_map::AtlasMap`] + [`grid::LodGrid`] ->
//! [`build::ObjectLod`] (one [`grid::Quad`] per file) -> [`bto::assemble`] -> NIF bytes.
//!
//! Stage status: B2.1-B2.6 are done (hidden-face removal in [`hidden`], the atlas remap in
//! [`build`] with [`reuv`] for tiled shapes, the vertex-colour rule of spec 12.2 in [`build`]
//! and [`material`], tree billboards in [`billboard`], grass LOD in [`grass`], the tree
//! brightness and passthru rules of spec 12.3, the object height map in [`heightmap`] for
//! the occlusion stage, spec 12.4; the glow atlas page is planned and rendered by the CLI's
//! `object_atlas_plan` / `glow_atlas`).

pub mod atlas_build;
pub mod billboard;
pub mod grass;
pub mod heightmap;
pub mod atlas_map;
pub mod bto;
pub mod build;
pub mod delaunay;
pub mod export;
pub mod flat;
pub mod grid;
pub mod hidden;
pub mod material;
pub mod mesh;
pub mod refs;
pub mod reuv;
pub mod sidefiles;
pub mod source;
pub mod stats;
pub mod terrain;
pub mod tiling;
pub mod transform;

pub use atlas_map::AtlasMap;
pub use build::{BuiltQuad, ObjectLod, Options};
pub use export::{Dialect, ExportFile, StaticRef};
pub use grid::{LodGrid, LodSettings, Quad, file_name};
