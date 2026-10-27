// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! What a worldspace's generated LOD consists of: the terrain (`.btr`), object (`.bto`) and tree
//! (`.btt`/`.lst`) files per level and season, found by listing the output, plus the LOD grid
//! settings when the load order is available.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::files::{Files, normalize};

/// One cell is 4096 game units.
pub const CELL_UNITS: f32 = 4096.0;

/// LOD levels: the block edge length in cells.
pub const LEVELS: [i32; 4] = [4, 8, 16, 32];

/// One LOD block: level (4, 8, 16, 32) and its south-west cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub struct QuadId {
    pub level: i32,
    pub x: i32,
    pub y: i32,
}

impl QuadId {
    /// World-space south-west corner.
    pub fn origin(&self) -> [f32; 2] {
        [self.x as f32 * CELL_UNITS, self.y as f32 * CELL_UNITS]
    }

    /// Edge length in game units.
    pub fn size(&self) -> f32 {
        self.level as f32 * CELL_UNITS
    }

    /// World-space rectangle (min x, min y, max x, max y).
    pub fn rect(&self) -> [f32; 4] {
        let [x, y] = self.origin();
        [x, y, x + self.size(), y + self.size()]
    }

    /// 2D distance from a point to the block (0 inside).
    pub fn distance_xy(&self, p: [f32; 2]) -> f32 {
        let [x0, y0, x1, y1] = self.rect();
        let dx = (x0 - p[0]).max(0.0).max(p[0] - x1);
        let dy = (y0 - p[1]).max(0.0).max(p[1] - y1);
        (dx * dx + dy * dy).sqrt()
    }

    /// The block one level up that contains this one (grid origin `sw`).
    pub fn parent(&self, sw: (i32, i32)) -> Option<QuadId> {
        let level = self.level * 2;
        if level > 32 {
            return None;
        }
        let f = |v: i32, s: i32| (v - s).div_euclid(level) * level + s;
        Some(QuadId {
            level,
            x: f(self.x, sw.0),
            y: f(self.y, sw.1),
        })
    }

    /// The four blocks one level down.
    pub fn children(&self) -> Option<[QuadId; 4]> {
        if self.level <= 4 {
            return None;
        }
        let l = self.level / 2;
        Some([
            QuadId {
                level: l,
                x: self.x,
                y: self.y,
            },
            QuadId {
                level: l,
                x: self.x + l,
                y: self.y,
            },
            QuadId {
                level: l,
                x: self.x,
                y: self.y + l,
            },
            QuadId {
                level: l,
                x: self.x + l,
                y: self.y + l,
            },
        ])
    }

    /// The block of `level` that contains cell `(cx, cy)` on a grid starting at `sw`.
    pub fn containing(level: i32, cell: (i32, i32), sw: (i32, i32)) -> QuadId {
        let f = |v: i32, s: i32| (v - s).div_euclid(level) * level + s;
        QuadId {
            level,
            x: f(cell.0, sw.0),
            y: f(cell.1, sw.1),
        }
    }
}

/// `lodsettings\<World>.lod` (16 bytes): south-west cell, stride, min and max level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct LodSettings {
    pub south_west: (i32, i32),
    pub stride: i32,
    pub min_level: i32,
    pub max_level: i32,
}

impl LodSettings {
    pub fn parse(bytes: &[u8]) -> Option<LodSettings> {
        if bytes.len() < 16 {
            return None;
        }
        let i16_at = |o: usize| i32::from(i16::from_le_bytes([bytes[o], bytes[o + 1]]));
        let i32_at =
            |o: usize| i32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
        Some(LodSettings {
            south_west: (i16_at(0), i16_at(2)),
            stride: i32_at(4),
            min_level: i32_at(8),
            max_level: i32_at(12),
        })
    }
}

/// `<World>.<L>.<x>.<y>[.<SEASON>].<ext>` -> (world, quad, season). The world name may contain
/// dots; the season code is three upper-case letters.
pub fn parse_quad_name(file: &str) -> Option<(String, QuadId, Option<String>, String)> {
    let name = file.rsplit('\\').next().unwrap_or(file);
    let (stem, ext) = name.rsplit_once('.')?;
    let parts: Vec<&str> = stem.split('.').collect();
    let n = parts.len();
    let tail_num = |i: usize| parts.get(i).and_then(|s| s.parse::<i32>().ok());
    let (world, level, x, y, season) = if n >= 4
        && tail_num(n - 1).is_some()
        && tail_num(n - 2).is_some()
        && tail_num(n - 3).is_some()
    {
        (
            parts[..n - 3].join("."),
            tail_num(n - 3)?,
            tail_num(n - 2)?,
            tail_num(n - 1)?,
            None,
        )
    } else if n >= 5
        && tail_num(n - 2).is_some()
        && tail_num(n - 3).is_some()
        && tail_num(n - 4).is_some()
    {
        (
            parts[..n - 4].join("."),
            tail_num(n - 4)?,
            tail_num(n - 3)?,
            tail_num(n - 2)?,
            Some(parts[n - 1].to_string()),
        )
    } else {
        return None;
    };
    if !LEVELS.contains(&level) {
        return None;
    }
    Some((world, QuadId { level, x, y }, season, ext.to_string()))
}

/// A file per quad, with the season's own file when it exists and the default one otherwise.
#[derive(Debug, Clone, Serialize)]
pub struct QuadFile {
    pub quad: QuadId,
    /// Data-relative path (normalized).
    pub path: String,
    /// The season has its own file (`false`: the default season's file stands in).
    pub seasonal: bool,
}

/// The generated LOD of one worldspace and season.
#[derive(Debug, Clone, Default, Serialize)]
pub struct WorldFiles {
    /// The worldspace as spelled in the file names.
    pub world: String,
    pub season: Option<String>,
    /// Season codes that have files of their own in this worldspace.
    pub seasons: Vec<String>,
    pub terrain: Vec<QuadFile>,
    pub objects: Vec<QuadFile>,
    /// Tree LOD blocks (`<World>.4.x.y.btt`), keyed by their LOD4 block.
    pub trees: Vec<QuadFile>,
    pub tree_list: Option<String>,
    pub tree_atlas: Option<String>,
    pub tree_atlas_normal: Option<String>,
    /// `meshes\lod\grass\*_lod1.nif` (Community Shaders Grass Optimizations meshes).
    pub grass_models: Vec<String>,
    pub lodsettings: Option<LodSettings>,
    /// LODForge Runtime files (trees, repeated statics); empty for a classic output.
    pub runtime: crate::runtime::RuntimeFiles,
}

impl WorldFiles {
    /// Terrain diffuse / normal texture of a quad for this season, with the default-season
    /// fallback: (diffuse, normal), both normalized, neither checked for existence.
    pub fn terrain_textures(&self, files: &dyn Files, q: &QuadId) -> (String, String) {
        let base = format!(
            "textures\\terrain\\{w}\\{w}.{}.{}.{}",
            q.level,
            q.x,
            q.y,
            w = self.world
        );
        let pick = |suffix: &str| {
            if let Some(s) = &self.season {
                let p = normalize(&format!("{base}.{s}{suffix}.dds"));
                if files.exists(&p) {
                    return p;
                }
            }
            normalize(&format!("{base}{suffix}.dds"))
        };
        (pick(""), pick("_n"))
    }

    /// Levels that have terrain or object files.
    pub fn levels(&self) -> Vec<i32> {
        let mut s: BTreeSet<i32> = self
            .terrain
            .iter()
            .chain(&self.objects)
            .map(|f| f.quad.level)
            .collect();
        if s.is_empty() {
            s.insert(4);
        }
        s.into_iter().collect()
    }

    /// Grid origin: lodsettings when known, else the smallest block coordinate at the top level
    /// (every block of a level is `level` cells apart, so any block's coordinates are on the grid).
    pub fn grid_origin(&self) -> (i32, i32) {
        if let Some(l) = self.lodsettings {
            return l.south_west;
        }
        let top = self.levels().into_iter().max().unwrap_or(4);
        let mut it = self
            .terrain
            .iter()
            .chain(&self.objects)
            .filter(|f| f.quad.level == top)
            .map(|f| (f.quad.x, f.quad.y));
        let Some(first) = it.next() else {
            return (0, 0);
        };
        let (mut x, mut y) = first;
        for (qx, qy) in it {
            // Reduce to the same residue class modulo the top level; both are on the grid.
            x = x.min(qx);
            y = y.min(qy);
        }
        (x.rem_euclid(top), y.rem_euclid(top))
    }

    /// Bounding rectangle in cells of every LOD4 (or finest available) block, inclusive of the
    /// block's cells.
    pub fn cell_bounds(&self) -> Option<((i32, i32), (i32, i32))> {
        let mut it = self.terrain.iter().chain(&self.objects).map(|f| f.quad);
        let f = it.next()?;
        let mut b = ((f.x, f.y), (f.x + f.level - 1, f.y + f.level - 1));
        for q in it {
            b.0.0 = b.0.0.min(q.x);
            b.0.1 = b.0.1.min(q.y);
            b.1.0 = b.1.0.max(q.x + q.level - 1);
            b.1.1 = b.1.1.max(q.y + q.level - 1);
        }
        Some(b)
    }
}

/// Worldspaces with generated LOD: the folders under `meshes\terrain\`, spelled as the files
/// spell them.
pub fn list_worlds(files: &dyn Files) -> Vec<String> {
    let mut worlds: BTreeMap<String, String> = BTreeMap::new();
    for p in files.list("meshes\\terrain\\") {
        let n = normalize(&p);
        let rest = &n["meshes\\terrain\\".len()..];
        let Some((folder, _)) = rest.split_once('\\') else {
            continue;
        };
        let file = p.rsplit(['\\', '/']).next().unwrap_or(&p);
        if let Some((world, _, _, ext)) = parse_quad_name(file) {
            if ext.eq_ignore_ascii_case("btr") || ext.eq_ignore_ascii_case("bto") {
                worlds.entry(folder.to_string()).or_insert(world);
            }
        }
    }
    worlds.into_values().collect()
}

/// Seasons that have files of their own under `meshes\terrain\<world>\`.
fn seasons_of(paths: &[String], world: &str) -> Vec<String> {
    let mut s = BTreeSet::new();
    for p in paths {
        if let Some((w, _, Some(season), _)) = parse_quad_name(p) {
            if w.eq_ignore_ascii_case(world) {
                s.insert(season.to_ascii_uppercase());
            }
        }
    }
    s.into_iter().collect()
}

/// Picks, per quad, the season's file or the default one.
fn quad_files(paths: &[String], world: &str, ext: &str, season: Option<&str>) -> Vec<QuadFile> {
    let mut by_quad: BTreeMap<QuadId, (Option<String>, Option<String>)> = BTreeMap::new();
    for p in paths {
        let Some((w, q, s, e)) = parse_quad_name(p) else {
            continue;
        };
        if !w.eq_ignore_ascii_case(world) || !e.eq_ignore_ascii_case(ext) {
            continue;
        }
        let slot = by_quad.entry(q).or_default();
        match (s, season) {
            (None, _) => slot.0 = Some(p.clone()),
            (Some(s), Some(want)) if s.eq_ignore_ascii_case(want) => slot.1 = Some(p.clone()),
            _ => {}
        }
    }
    by_quad
        .into_iter()
        .filter_map(|(quad, (default, seasonal))| match (seasonal, default) {
            (Some(p), _) => Some(QuadFile {
                quad,
                path: normalize(&p),
                seasonal: true,
            }),
            (None, Some(p)) => Some(QuadFile {
                quad,
                path: normalize(&p),
                seasonal: false,
            }),
            (None, None) => None,
        })
        .collect()
}

/// Finds the LOD files of `world` (as spelled in the file names) for a season (`None` = default).
/// The LOD files are those `lister` lists (the output alone, or the output merged with the load
/// order as the game sees it); `files` resolves everything else (textures, lodsettings).
pub fn discover(
    files: &dyn Files,
    lister: &dyn Files,
    world: &str,
    season: Option<&str>,
) -> WorldFiles {
    let wl = world.to_ascii_lowercase();
    let dir = format!("meshes\\terrain\\{wl}\\");
    let paths = lister.list(&dir);
    let seasons = seasons_of(&paths, world);
    let season = season.map(|s| s.to_ascii_uppercase());
    let terrain = quad_files(&paths, world, "btr", season.as_deref());
    let objects = quad_files(&paths, world, "bto", season.as_deref());
    let trees = quad_files(&paths, world, "btt", season.as_deref());
    let seasonal = |base: &str, ext: &str| {
        if let Some(s) = &season {
            let p = normalize(&format!("{base}.{s}.{ext}"));
            if lister.exists(&p) {
                return Some(p);
            }
        }
        let p = normalize(&format!("{base}.{ext}"));
        lister.exists(&p).then_some(p)
    };
    let tree_list = seasonal(&format!("meshes\\terrain\\{wl}\\trees\\{wl}"), "lst");
    let tree_atlas = seasonal(
        &format!("textures\\terrain\\{wl}\\trees\\{wl}treelod"),
        "dds",
    );
    let tree_atlas_normal = seasonal(
        &format!("textures\\terrain\\{wl}\\trees\\{wl}treelod_n"),
        "dds",
    )
    .or_else(|| {
        // Season atlases are named `<w>treelod.<S>.dds`; their normal map `<w>treelod_n.<S>.dds`.
        season
            .as_ref()
            .map(|s| {
                normalize(&format!(
                    "textures\\terrain\\{wl}\\trees\\{wl}treelod_n.{s}.dds"
                ))
            })
            .filter(|p| lister.exists(p))
    });
    let grass_models = lister
        .list("meshes\\lod\\grass\\")
        .into_iter()
        .map(|p| normalize(&p))
        .filter(|p| p.ends_with("_lod1.nif"))
        .collect();
    let lodsettings = files
        .read(&format!("lodsettings\\{wl}.lod"))
        .and_then(|b| LodSettings::parse(&b));
    let runtime = crate::runtime::discover(files, lister, world, season.as_deref());
    WorldFiles {
        world: world.to_string(),
        season,
        seasons,
        terrain,
        objects,
        trees,
        tree_list,
        tree_atlas,
        tree_atlas_normal,
        grass_models,
        lodsettings,
        runtime,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::MemFiles;

    #[test]
    fn quad_names() {
        let (w, q, s, e) = parse_quad_name("Tamriel.4.-8.12.btr").unwrap();
        assert_eq!(
            (w.as_str(), q, s, e.as_str()),
            (
                "Tamriel",
                QuadId {
                    level: 4,
                    x: -8,
                    y: 12
                },
                None,
                "btr"
            )
        );
        let (w, q, s, _) =
            parse_quad_name("meshes\\terrain\\x\\DLC2SolstheimWorld.16.0.-16.WIN.bto").unwrap();
        assert_eq!(
            (w.as_str(), q, s.as_deref()),
            (
                "DLC2SolstheimWorld",
                QuadId {
                    level: 16,
                    x: 0,
                    y: -16
                },
                Some("WIN")
            )
        );
        let (w, q, _, _) = parse_quad_name("My.World.8.0.0.btr").unwrap();
        assert_eq!((w.as_str(), q.level), ("My.World", 8));
        assert!(parse_quad_name("Tamriel.lst").is_none());
        assert!(parse_quad_name("Tamriel.5.0.0.btr").is_none());
    }

    #[test]
    fn quad_geometry() {
        let q = QuadId {
            level: 8,
            x: -8,
            y: 8,
        };
        assert_eq!(q.rect(), [-32768.0, 32768.0, 0.0, 65536.0]);
        assert_eq!(q.distance_xy([-16384.0, 40000.0]), 0.0);
        assert_eq!(q.distance_xy([4096.0, 40000.0]), 4096.0);
        assert_eq!(
            q.parent((-96, -96)),
            Some(QuadId {
                level: 16,
                x: -16,
                y: 0
            })
        );
        assert_eq!(
            q.children().unwrap()[3],
            QuadId {
                level: 4,
                x: -4,
                y: 12
            }
        );
        assert_eq!(
            QuadId::containing(32, (-1, -1), (-96, -96)),
            QuadId {
                level: 32,
                x: -32,
                y: -32
            }
        );
    }

    #[test]
    fn discovers_with_season_fallback() {
        let mut m = MemFiles::default();
        for f in [
            "Tamriel.4.0.0.btr",
            "Tamriel.4.0.0.WIN.btr",
            "Tamriel.4.4.0.btr",
            "Tamriel.8.0.0.btr",
            "Tamriel.4.0.0.bto",
            "Tamriel.4.0.0.WIN.bto",
        ] {
            m.insert(&format!("meshes\\terrain\\Tamriel\\{f}"), vec![0]);
        }
        m.insert(
            "meshes\\terrain\\Tamriel\\trees\\Tamriel.4.0.0.btt",
            vec![0],
        );
        m.insert("meshes\\terrain\\Tamriel\\trees\\Tamriel.lst", vec![0]);
        m.insert(
            "textures\\terrain\\Tamriel\\trees\\Tamrieltreelod.dds",
            vec![0],
        );
        m.insert("textures\\terrain\\Tamriel\\Tamriel.4.0.0.dds", vec![0]);
        m.insert("textures\\terrain\\Tamriel\\Tamriel.4.0.0.WIN.dds", vec![0]);
        m.insert("lodsettings\\Tamriel.lod", LodSettingsBytes::tamriel());
        assert_eq!(list_worlds(&m), vec!["Tamriel"]);
        let w = discover(&m, &m, "Tamriel", Some("win"));
        assert_eq!(w.seasons, vec!["WIN"]);
        assert_eq!(w.terrain.len(), 3);
        let t0 = w
            .terrain
            .iter()
            .find(|f| {
                f.quad
                    == QuadId {
                        level: 4,
                        x: 0,
                        y: 0,
                    }
            })
            .unwrap();
        assert!(t0.seasonal && t0.path.ends_with("tamriel.4.0.0.win.btr"));
        let t1 = w
            .terrain
            .iter()
            .find(|f| {
                f.quad
                    == QuadId {
                        level: 4,
                        x: 4,
                        y: 0,
                    }
            })
            .unwrap();
        assert!(!t1.seasonal && t1.path.ends_with("tamriel.4.4.0.btr"));
        assert_eq!(w.trees.len(), 1);
        assert_eq!(
            w.tree_list.as_deref(),
            Some("meshes\\terrain\\tamriel\\trees\\tamriel.lst")
        );
        assert_eq!(w.lodsettings.unwrap().south_west, (-96, -96));
        assert_eq!(w.levels(), vec![4, 8]);
        let (d, n) = w.terrain_textures(
            &m,
            &QuadId {
                level: 4,
                x: 0,
                y: 0,
            },
        );
        assert_eq!(d, "textures\\terrain\\tamriel\\tamriel.4.0.0.win.dds");
        assert_eq!(n, "textures\\terrain\\tamriel\\tamriel.4.0.0_n.dds");
        let d = discover(&m, &m, "Tamriel", None);
        assert!(d.terrain.iter().all(|f| !f.seasonal));
    }

    struct LodSettingsBytes;
    impl LodSettingsBytes {
        fn tamriel() -> Vec<u8> {
            let mut b = Vec::new();
            b.extend_from_slice(&(-96i16).to_le_bytes());
            b.extend_from_slice(&(-96i16).to_le_bytes());
            b.extend_from_slice(&256i32.to_le_bytes());
            b.extend_from_slice(&4i32.to_le_bytes());
            b.extend_from_slice(&32i32.to_le_bytes());
            b
        }
    }
}
