// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! Skyrim Special Edition plugin reader.
//!
//! Plugins are memory-mapped; records are walked without copying and decompressed on demand, so
//! independent groups (plugins, exterior sub-blocks) can be processed in parallel.

pub mod load_order;
pub mod plugin;
pub mod record;
pub mod terrain;
pub mod world;

use std::path::PathBuf;

pub use load_order::{mo2_active_plugins, LoadOrder, LoadedPlugin, ModIndex};
pub use plugin::{Plugin, PluginHeader};
pub use record::{fields, Entries, Entry, Field, Group, Record, Sig};
pub use terrain::{land_textures, winning_records, Land, LandTexture, Layer, Quadrant};
pub use world::{scan_worldspaces, LandRef, Placement, RecordRef, Worldspace, CELL_SHORT_DATA};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("malformed plugin data at {offset:#x}: {what}")]
    Malformed { offset: usize, what: &'static str },
    #[error("{path}: malformed plugin data at {offset:#x}: {what}")]
    MalformedIn { path: PathBuf, offset: usize, what: &'static str },
    #[error("zlib error in record at {offset:#x}: {source}")]
    Decompress { offset: usize, source: std::io::Error },
    #[error("plugin not found in the data folder: {0}")]
    MissingPlugin(String),
    #[error("load order exceeds 253 full or 4096 light plugins")]
    TooManyPlugins,
}

impl Error {
    pub(crate) fn in_file(self, path: &std::path::Path) -> Error {
        match self {
            Error::Malformed { offset, what } => Error::MalformedIn { path: path.to_owned(), offset, what },
            e => e,
        }
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
