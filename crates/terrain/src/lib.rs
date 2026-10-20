// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! Terrain LOD textures: the reverse-engineered xLODGen bake (CPU reference in [`spec`], GPU
//! packing in [`gpu_spec`]) and comparison tooling.

pub mod bake;
pub mod gpu_spec;
pub mod image;
pub mod spec;

pub use bake::TextureCache;
pub use image::{compare, Image, Metrics};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Esp(#[from] lodforge_esp::Error),
    #[error(transparent)]
    Archive(#[from] lodforge_archive::Error),
    #[error(transparent)]
    Dds(#[from] lodforge_dds::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
