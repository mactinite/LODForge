// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! Terrain LOD meshes (.btr) and DynDOLOD's terrain underside, written from
//! docs/re/terrain-lod-spec.md (the BTR format and the mesher requirements as observed in the
//! reference tools' output), the public-domain Terra 0.7 greedy insertion (Garland & Heckbert)
//! and the NIF format specification.
//!
//! Pipeline: [`input::TerrainInput`] (from the load order, [`export`], or from a terrain dump,
//! [`terrain_bin`]) -> [`mesh::build_quad`] per quad of the lattice -> [`btr::Btr`] file
//! description -> NIF bytes through the `lodforge-nif` writer. [`underside`] runs the same mesher
//! at LOD32 for DynDOLOD's `<World>_Underside.nif`; [`compare`] is the tolerant comparison of
//! two output folders.

pub mod bounds;
pub mod btr;
pub mod compare;
pub mod export;
pub mod greedy;
pub mod input;
pub mod mesh;
pub mod profile;
pub mod quadedge;
pub mod settings;
pub mod terrain_bin;
pub mod underside;
pub mod vcache;

pub use btr::Btr;
pub use input::{HeightField, TerrainCell, TerrainInput};
pub use mesh::{QuadMesh, QuadOptions, build_quad, full_coverage};
pub use settings::{ControlFile, Settings};
