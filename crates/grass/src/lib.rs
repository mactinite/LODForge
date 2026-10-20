// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! Grass cache (`Grass\<World>x%04dy%04d.cgid`) files as No Grass In Objects NG writes them in
//! game: a reader/writer for the file format, verified against real caches. Spec:
//! `docs/re/grass-cache-spec.md` (file-format section).

pub mod cgid;

/// `Grass\<World>x%04dy%04d.cgid`.
pub fn file_name(world: &str, x: i32, y: i32) -> String {
    format!("{world}x{x:04}y{y:04}.cgid")
}
