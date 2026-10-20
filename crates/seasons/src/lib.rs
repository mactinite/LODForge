// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! Seasons: swap files and season delta planning.
//!
//! Swap files live at `Data\Seasons\*_<SPR|SUM|AUT|WIN>.ini`. Each section (`[LandTextures]`,
//! `[Statics]`, `[Trees]`, ...) holds lines `original|replacement`, where each side is either
//! `0x<object id>~<plugin file>` or an EditorID. `;` starts a comment.
//!
//! The delta planner answers: for a season, which LOD tiles can differ from the default season?
//! Terrain textures only change where a cell uses a swapped LTEX, so every other tile can be
//! copied from the default build.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use rayon::prelude::*;

use lodforge_archive::Vfs;
use lodforge_esp::{LoadOrder, Worldspace};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Archive(#[from] lodforge_archive::Error),
    #[error(transparent)]
    Esp(#[from] lodforge_esp::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Season {
    Spring,
    Summer,
    Autumn,
    Winter,
}

impl Season {
    pub const ALL: [Season; 4] = [Season::Spring, Season::Summer, Season::Autumn, Season::Winter];

    /// File-name suffix: `SPR`, `SUM`, `AUT`, `WIN`.
    pub fn code(self) -> &'static str {
        match self {
            Season::Spring => "SPR",
            Season::Summer => "SUM",
            Season::Autumn => "AUT",
            Season::Winter => "WIN",
        }
    }
}

/// One side of a swap line before resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SwapRef {
    FormId { object_id: u32, plugin: String },
    EditorId(String),
}

impl SwapRef {
    fn parse(s: &str) -> Option<SwapRef> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }
        if let Some((id, plugin)) = s.split_once('~') {
            let hex = id.trim().trim_start_matches("0x").trim_start_matches("0X");
            return u32::from_str_radix(hex, 16).ok().map(|object_id| SwapRef::FormId { object_id, plugin: plugin.trim().to_string() });
        }
        Some(SwapRef::EditorId(s.to_string()))
    }
}

/// Raw swap lines of one season, per section (lowercase section name), in file order.
#[derive(Debug, Default, Clone)]
pub struct SwapFile {
    pub sections: BTreeMap<String, Vec<(SwapRef, SwapRef)>>,
}

pub fn parse_swap_ini(text: &str) -> SwapFile {
    let mut f = SwapFile::default();
    let mut section = String::new();
    for line in text.lines() {
        let line = line.split(';').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].trim().to_ascii_lowercase();
            continue;
        }
        if let Some((a, b)) = line.split_once('|') {
            if let (Some(a), Some(b)) = (SwapRef::parse(a), SwapRef::parse(b)) {
                f.sections.entry(section.clone()).or_default().push((a, b));
            }
        }
    }
    f
}

/// Resolved swaps for every season found in the data folder.
#[derive(Debug, Default)]
pub struct SeasonSwaps {
    /// Season -> section -> original global form id -> replacement global form id.
    pub swaps: BTreeMap<Season, BTreeMap<String, HashMap<u32, u32>>>,
    /// Swap files read, per season (Data-relative paths).
    pub files: BTreeMap<Season, Vec<String>>,
    /// References that could not be resolved (plugin not loaded, unknown EditorID).
    pub unresolved: Vec<String>,
}

impl SeasonSwaps {
    pub fn land_textures(&self, season: Season) -> Option<&HashMap<u32, u32>> {
        self.swaps.get(&season).and_then(|s| s.get("landtextures"))
    }
}

/// Reads every `seasons\*_<season>.ini` the VFS sees and resolves references against the load
/// order. `editor_ids` maps EditorID (lowercase) -> global form id for EditorID-style lines.
pub fn load_swaps(vfs: &Vfs, lo: &LoadOrder, editor_ids: &HashMap<String, u32>) -> Result<SeasonSwaps> {
    let mut out = SeasonSwaps::default();
    let resolve = |r: &SwapRef| -> Option<u32> {
        match r {
            SwapRef::FormId { object_id, plugin } => lo.find(plugin).map(|p| p.index.global(*object_id)),
            SwapRef::EditorId(e) => editor_ids.get(&e.to_ascii_lowercase()).copied(),
        }
    };
    for path in vfs.list_prefix("seasons\\") {
        let Some(stem) = path.strip_suffix(".ini") else { continue };
        let Some(season) = Season::ALL.into_iter().find(|s| stem.to_ascii_uppercase().ends_with(s.code())) else { continue };
        let Some(bytes) = vfs.read(&path)? else { continue };
        let file = parse_swap_ini(&String::from_utf8_lossy(&bytes));
        let sections = out.swaps.entry(season).or_default();
        for (section, lines) in file.sections {
            let map = sections.entry(section).or_default();
            for (a, b) in lines {
                match (resolve(&a), resolve(&b)) {
                    (Some(a), Some(b)) => {
                        map.insert(a, b);
                    }
                    _ => out.unresolved.push(format!("{path}: {a:?} | {b:?}")),
                }
            }
        }
        out.files.entry(season).or_default().push(path);
    }
    Ok(out)
}

/// LTEX ids used by each exterior cell of a worldspace (decoded in parallel).
pub fn cell_textures(lo: &LoadOrder, world: &Worldspace) -> Result<HashMap<(i32, i32), HashSet<u32>>> {
    let lands: Vec<_> = world.lands.values().copied().collect();
    let per_cell: Vec<((i32, i32), HashSet<u32>)> = lands
        .par_iter()
        .map(|l| -> Result<_> {
            let lp = &lo.plugins[l.plugin as usize];
            let rec = lp.record_at(l.offset)?;
            let land = lp.decode_land(&rec)?;
            Ok((l.cell, land.textures().collect()))
        })
        .collect::<Result<_>>()?;
    Ok(per_cell.into_iter().collect())
}

/// Which texture of a landscape texture a bake output depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextureKind {
    Diffuse,
    Normal,
}

/// The subset of LTEX swaps that change the texture file of `kind` (a swap to an LTEX using the
/// same file cannot change that output).
pub fn effective_swaps(swapped: &HashMap<u32, u32>, ltex: &HashMap<u32, lodforge_esp::LandTexture>, kind: TextureKind) -> HashMap<u32, u32> {
    let path = |id: &u32| {
        ltex.get(id).map(|t| match kind {
            TextureKind::Diffuse => t.diffuse.to_ascii_lowercase(),
            TextureKind::Normal => t.normal.to_ascii_lowercase(),
        })
    };
    swapped.iter().filter(|(a, b)| path(a) != path(b)).map(|(a, b)| (*a, *b)).collect()
}

/// LOD tile origin (south-west cell) containing `cell` at `level` cells per tile side.
pub fn tile_of(cell: (i32, i32), level: i32) -> (i32, i32) {
    (cell.0.div_euclid(level) * level, cell.1.div_euclid(level) * level)
}

/// Tiles of `level` that contain at least one cell using a swapped LTEX. `margin` widens each
/// affected cell by that many cells in every direction (for bakes that sample neighbours).
pub fn affected_tiles(cells: &HashMap<(i32, i32), HashSet<u32>>, swapped: &HashMap<u32, u32>, level: i32, margin: i32) -> BTreeSet<(i32, i32)> {
    let mut tiles = BTreeSet::new();
    for (&(x, y), tex) in cells {
        if tex.iter().any(|t| swapped.contains_key(t)) {
            for dy in -margin..=margin {
                for dx in -margin..=margin {
                    tiles.insert(tile_of((x + dx, y + dy), level));
                }
            }
        }
    }
    tiles
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_swap_lines() {
        let f = parse_swap_ini("[LandTextures]\n\n;Pine\n;LPine|PineSPR\n0x137C8~Skyrim.esm|0x80D~Seasonal Landscapes.esp\nLDirt01 | LDirtSPR ; trailing\n[Trees]\n0x1~A.esp|0x2~B.esp\n");
        let lt = &f.sections["landtextures"];
        assert_eq!(lt.len(), 2);
        assert_eq!(lt[0].0, SwapRef::FormId { object_id: 0x137C8, plugin: "Skyrim.esm".into() });
        assert_eq!(lt[0].1, SwapRef::FormId { object_id: 0x80D, plugin: "Seasonal Landscapes.esp".into() });
        assert_eq!(lt[1], (SwapRef::EditorId("LDirt01".into()), SwapRef::EditorId("LDirtSPR".into())));
        assert_eq!(f.sections["trees"].len(), 1);
    }

    #[test]
    fn tiles_floor_to_level() {
        assert_eq!(tile_of((-1, -1), 4), (-4, -4));
        assert_eq!(tile_of((5, -64), 4), (4, -64));
        assert_eq!(tile_of((31, 33), 32), (0, 32));
    }
}
