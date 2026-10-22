// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! Which LOD blocks to draw: one level everywhere, or the game's arrangement, where finer levels
//! surround the camera and coarser ones take over with distance.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::world::QuadId;

/// Skyrim's defaults for the LOD level switch distances (game units): LOD4 up to
/// `fBlockLevel0Distance`, LOD8 up to `fBlockLevel1Distance`, LOD16 up to `fBlockMaximumDistance`,
/// LOD32 beyond.
pub const BLOCK_LEVEL0_DISTANCE: f32 = 35000.0;
pub const BLOCK_LEVEL1_DISTANCE: f32 = 70000.0;
pub const BLOCK_MAXIMUM_DISTANCE: f32 = 250000.0;

/// The LOD level switch distances in use: the game's defaults, or a profile's
/// `[TerrainManager]` values from SkyrimPrefs.ini (DynDOLOD-style lists raise them).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct LodDistances {
    pub level0: f32,
    pub level1: f32,
    pub maximum: f32,
}

impl Default for LodDistances {
    fn default() -> Self {
        LodDistances {
            level0: BLOCK_LEVEL0_DISTANCE,
            level1: BLOCK_LEVEL1_DISTANCE,
            maximum: BLOCK_MAXIMUM_DISTANCE,
        }
    }
}

impl LodDistances {
    /// Reads `fBlockLevel0Distance`, `fBlockLevel1Distance` and `fBlockMaximumDistance` from
    /// SkyrimPrefs.ini text; keys that are missing or unreadable keep the game's defaults.
    pub fn from_prefs_ini(text: &str) -> LodDistances {
        let mut d = LodDistances::default();
        let mut in_section = false;
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                in_section = line.eq_ignore_ascii_case("[TerrainManager]");
                continue;
            }
            if !in_section {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            let Ok(v) = v.trim().parse::<f32>() else { continue };
            if !(v.is_finite() && v > 0.0) {
                continue;
            }
            match k.trim().to_ascii_lowercase().as_str() {
                "fblocklevel0distance" => d.level0 = v,
                "fblocklevel1distance" => d.level1 = v,
                "fblockmaximumdistance" => d.maximum = v,
                _ => {}
            }
        }
        d
    }

    /// Distance up to which `level` is shown instead of the next coarser one.
    pub fn switch_distance(&self, level: i32) -> f32 {
        match level {
            4 => self.level0,
            8 => self.level1,
            16 => self.maximum,
            _ => f32::INFINITY,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum LodMode {
    /// Every block of one level.
    Single(i32),
    /// The game's quadtree: a block is split into its children while the camera is within the
    /// children's switch distance (times `distance_mult`).
    Game { distance_mult: f32 },
}

impl Default for LodMode {
    fn default() -> Self {
        LodMode::Game { distance_mult: 1.0 }
    }
}

/// Distance up to which `level` is shown with the game's default distances.
pub fn switch_distance(level: i32) -> f32 {
    LodDistances::default().switch_distance(level)
}

/// Blocks to draw for a camera at `cam` (x, y). `present` holds every block that has a file;
/// `top` is the coarsest level to start from. In game mode a block is drawn when it is present
/// and not split; a split block whose child is absent leaves that child's area empty (the
/// checks report the missing file).
pub fn visible(
    mode: LodMode,
    distances: &LodDistances,
    cam: [f32; 2],
    present: &HashSet<QuadId>,
    grid_sw: (i32, i32),
    top: i32,
) -> Vec<QuadId> {
    match mode {
        LodMode::Single(level) => present
            .iter()
            .filter(|q| q.level == level)
            .copied()
            .collect(),
        LodMode::Game { distance_mult } => {
            // Start from every top-level block that any present block belongs to.
            let mut roots: Vec<QuadId> = present
                .iter()
                .map(|q| QuadId::containing(top, (q.x, q.y), grid_sw))
                .collect();
            roots.sort();
            roots.dedup();
            let mut out = Vec::new();
            for r in roots {
                descend(r, cam, distances, distance_mult, present, &mut out);
            }
            out
        }
    }
}

fn descend(
    q: QuadId,
    cam: [f32; 2],
    d: &LodDistances,
    mult: f32,
    present: &HashSet<QuadId>,
    out: &mut Vec<QuadId>,
) {
    let here = present.contains(&q);
    if let Some(children) = q.children() {
        let child_level = q.level / 2;
        let split = q.distance_xy(cam) < d.switch_distance(child_level) * mult;
        // Split when the camera is close enough, or when this block is missing and finer ones
        // exist (a finer block beats a hole); never split into nothing.
        let finer = children
            .iter()
            .any(|c| present.contains(c) || has_descendant(*c, present));
        if finer && (split || !here) {
            for c in children {
                descend(c, cam, d, mult, present, out);
            }
            return;
        }
    }
    if here {
        out.push(q);
    }
}

fn has_descendant(q: QuadId, present: &HashSet<QuadId>) -> bool {
    match q.children() {
        Some(c) => c
            .iter()
            .any(|c| present.contains(c) || has_descendant(*c, present)),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(levels: &[i32], cells: i32) -> HashSet<QuadId> {
        let mut s = HashSet::new();
        for &l in levels {
            let mut x = 0;
            while x < cells {
                let mut y = 0;
                while y < cells {
                    s.insert(QuadId { level: l, x, y });
                    y += l;
                }
                x += l;
            }
        }
        s
    }

    #[test]
    fn prefs_ini_distances() {
        let d = LodDistances::from_prefs_ini(
            "[Display]\nfBlockLevel0Distance=1\n[TerrainManager]\nfBlockLevel0Distance=57344.0000\nfBlockLevel1Distance=147456.0000\nfBlockMaximumDistance=327680.0000\n",
        );
        assert_eq!(d.level0, 57344.0);
        assert_eq!(d.level1, 147456.0);
        assert_eq!(d.maximum, 327680.0);
        let d = LodDistances::from_prefs_ini("[TerrainManager]\nfBlockLevel1Distance=x\n");
        assert_eq!(d, LodDistances::default());
    }

    #[test]
    fn single_level() {
        let p = grid(&[4, 8, 16, 32], 64);
        let v = visible(LodMode::Single(16), &LodDistances::default(), [0.0, 0.0], &p, (0, 0), 32);
        assert_eq!(v.len(), 16);
        assert!(v.iter().all(|q| q.level == 16));
    }

    #[test]
    fn game_mode_covers_everything_once() {
        let p = grid(&[4, 8, 16, 32], 128);
        let v = visible(LodMode::Game { distance_mult: 1.0 }, &LodDistances::default(),
            [10000.0, 10000.0],
            &p,
            (0, 0),
            32,
        );
        // Union of the drawn blocks is the full 128x128 cells, with no overlap.
        let mut cells = HashSet::new();
        for q in &v {
            for x in q.x..q.x + q.level {
                for y in q.y..q.y + q.level {
                    assert!(cells.insert((x, y)), "overlap at {x},{y}");
                }
            }
        }
        assert_eq!(cells.len(), 128 * 128);
        assert!(v.contains(&QuadId {
            level: 4,
            x: 0,
            y: 0
        }));
        // The far corner (~500k units away, past fBlockMaximumDistance) is LOD32.
        assert!(v.iter().any(|q| q.level == 32));
        let far = visible(LodMode::Game { distance_mult: 1.0 }, &LodDistances::default(),
            [1.0e7, 1.0e7],
            &p,
            (0, 0),
            32,
        );
        assert!(far.iter().all(|q| q.level == 32));
    }

    #[test]
    fn missing_fine_level_falls_back() {
        // Only LOD8 and LOD32 exist near the camera: LOD8 is drawn under the split LOD32 block.
        let mut p = grid(&[8], 32);
        p.insert(QuadId {
            level: 32,
            x: 0,
            y: 0,
        });
        let v = visible(LodMode::Game { distance_mult: 1.0 }, &LodDistances::default(),
            [1000.0, 1000.0],
            &p,
            (0, 0),
            32,
        );
        assert!(v.iter().all(|q| q.level == 8), "{v:?}");
        assert_eq!(v.len(), 16);
    }
}
