// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! Instance files for the LODForge Runtime SKSE plugin: trees drawn as impostors
//! (`.trees.bin`), repeated statics drawn instanced instead of merged into `.bto`
//! (`.statics.bin`) and far grass (`.lfg`). The byte layout is specified in
//! `docs/runtime/instance-formats.md`; this crate is its reference writer and reader.

pub mod container;
pub mod dynlod;
pub mod grass;
pub mod hash;
pub mod meshpack;
pub mod statics;
pub mod trees;

pub use container::{Common, WriteOptions};

/// Folder of the files, relative to `Data`.
pub const DATA_DIR: &str = r"SKSE\Plugins\LODForgeRuntime";

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("truncated file")]
    Truncated,
    #[error("invalid file: {0}")]
    Invalid(&'static str),
    #[error("unsupported version {0}.{1}")]
    Version(u16, u16),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests;
