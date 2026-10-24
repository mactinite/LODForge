// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! Hemi-octahedral tree impostors for the LODForge runtime (docs/runtime/impostors.md): the
//! direction <-> frame mapping ([hemioct]), the GPU bake of one model ([bake]), edge dilation
//! ([dilation]), atlas pages ([atlas]) and the manifests ([manifest]).
//!
//! The hemi-octahedral math, the dilation and the capture conventions are ported from Project
//! Terra (the maintainer's own Unity project), moved to Skyrim's Z-up model space.

pub mod atlas;
pub mod bake;
pub mod dilation;
pub mod hemioct;
pub mod manifest;

pub use bake::{Baked, Settings, bake};
