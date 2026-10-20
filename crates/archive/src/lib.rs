// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! Game resource access: BSA archives and a virtual `Data\` folder (loose files over archives),
//! including MO2 mod-folder layering.

pub mod bsa;
pub mod vfs;

use std::path::PathBuf;

pub use bsa::{Bsa, BsaFile};
pub use vfs::{archive_load_order, ini_archive_list, Mo2Layout, Origin, Vfs};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("{path}: {what}")]
    Malformed { path: PathBuf, what: &'static str },
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Normalizes a Data-relative path: lowercase ASCII, backslashes, no leading `data\` or slash.
pub fn normalize(p: &str) -> String {
    let mut s = p.replace('/', "\\").to_ascii_lowercase();
    while let Some(rest) = s.strip_prefix('\\') {
        s = rest.to_string();
    }
    if let Some(rest) = s.strip_prefix("data\\") {
        s = rest.to_string();
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes() {
        assert_eq!(normalize("Data/Meshes\\Terrain/X.BTO"), "meshes\\terrain\\x.bto");
        assert_eq!(normalize("\\textures\\a.dds"), "textures\\a.dds");
    }
}
