// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! A compatibility implementation of the parts of DynDOLOD 3 that produce the object LOD export (rule
//! tables, LOD model discovery and the export files the object LOD builder reads) and its plugins
//! (docs/re/dyndolod-spec.md), and its pre-computed occlusion, re-implemented in a fresh context
//! from the scrubbed docs/re/occlusion-spec.md. DynDOLOD's configuration and Resources are read
//! from the user's install at run time and never bundled.
//! Provenance and distribution clearance remain under review; see docs/licensing.md.

pub mod ini;
pub mod rules;
pub mod bases;
pub mod lodmodels;
pub mod assign;
pub mod export;
pub mod plugin;
pub mod largerefs;
pub mod dynplugins;
pub mod occlusion;
pub mod childworld;
pub mod overlay;
pub mod sidefiles;
