//! Terrain LOD settings (docs/re/terrain-lod-spec.md section 2.6) and the control-file syntax
//! xLODGen (`LODGen_Terrain_<World>.txt`) and DynDOLOD (`LODGen_SSE_Export_TerrainUnderside_<W>.txt`)
//! write: one `Key=Value` per line.

/// Settings of one LOD level.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LevelSettings {
    /// Error threshold of the greedy insertion in quad-local height units (= Quality x level game
    /// units); "higher settings equal less quality".
    pub quality: f32,
    /// Point budget per quad; 2,000 of it are reserved for skirt vertices.
    pub max_verts: u32,
    /// Optimize Unseen: -1 off, 0 on (under-water points are not candidates), v > 0 on with
    /// shallow terrain pushed at least v game units below the water surface.
    pub water_delta: f32,
    /// Z translation of every shape of the level (`ZShiftLOD<L>`), game units; 0 by default.
    pub z_shift: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    /// No water planes and no water-aware simplification.
    pub ignore_water: bool,
    /// LOD4 quads are triangulated cell by cell.
    pub protect_cell_borders: bool,
    /// Leave out the hidden land quadrants of CELL records (holes in the mesh).
    pub hide_quads: bool,
    /// Skirt size key as xLODGen writes it (256); <= 0 disables skirts. Depth per level: see
    /// [`skirt_depth`].
    pub skirts: i32,
    /// Index 0..3 = LOD 4, 8, 16, 32.
    pub levels: [LevelSettings; 4],
}

pub const LEVELS: [i32; 4] = [4, 8, 16, 32];

/// 0..3 for levels 4, 8, 16, 32.
pub fn level_index(level: i32) -> usize {
    match level {
        4 => 0,
        8 => 1,
        16 => 2,
        _ => 3,
    }
}

/// Skirt depth in quad-local units for a `Skirts` key value. Observed for 256: 319 / 256 / 193 /
/// 130 at LOD 4 / 8 / 16 / 32 (= 256 + 63 x (1 - k) for level index k); other values follow the
/// same line (open question 7.3 of the spec). 0 or negative: no skirts.
pub fn skirt_depth(skirts: i32, level: i32) -> f32 {
    if skirts <= 0 {
        return 0.0;
    }
    let k = level_index(level) as i32;
    (skirts + (skirts / 4 - 1) * (1 - k)).max(1) as f32
}

impl Default for Settings {
    /// xLODGen's defaults as written into every observed control file: quality 5 / 8 / 8 / 10,
    /// max vertices 32,767, Optimize Unseen off / on / on / 550, skirts 256, protected cell
    /// borders, no hidden quads, water on.
    fn default() -> Self {
        let lvl = |quality, water_delta| LevelSettings {
            quality,
            max_verts: 32767,
            water_delta,
            z_shift: 0.0,
        };
        Settings {
            ignore_water: false,
            protect_cell_borders: true,
            hide_quads: false,
            skirts: 256,
            levels: [
                lvl(5.0, -1.0),
                lvl(8.0, 0.0),
                lvl(8.0, 0.0),
                lvl(10.0, 550.0),
            ],
        }
    }
}

impl Settings {
    pub fn level(&self, level: i32) -> &LevelSettings {
        &self.levels[level_index(level)]
    }
    pub fn level_mut(&mut self, level: i32) -> &mut LevelSettings {
        &mut self.levels[level_index(level)]
    }
}

/// A parsed control file: the settings plus the input description it carries.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ControlFile {
    pub game_mode: Option<String>,
    pub world: Option<String>,
    pub cell_sw: Option<(i32, i32)>,
    pub max_level: Option<i32>,
    /// Underside export: the single level to build (`LODLevel`).
    pub lod_level: Option<i32>,
    pub terrain_data: Option<String>,
    pub land_height: Option<f32>,
    pub water_height: Option<f32>,
    pub path_output: Option<String>,
    /// Underside export clip rectangle: MinX, MinY, MaxX, MaxY.
    pub min_x: Option<i32>,
    pub min_y: Option<i32>,
    pub max_x: Option<i32>,
    pub max_y: Option<i32>,
    /// Underside export: cells whose hide flags are ignored (`IgnoreHide=x,y;x,y`).
    pub ignore_hide: Vec<(i32, i32)>,
    pub remove_unseen_faces: Option<bool>,
    pub settings: Settings,
    /// Keys that were present for the settings (so callers know what the file set).
    pub keys: Vec<String>,
}

fn parse_bool(v: &str) -> Option<bool> {
    match v.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => Some(true),
        "false" | "0" | "no" => Some(false),
        _ => None,
    }
}

fn parse_pair(v: &str) -> Option<(i32, i32)> {
    let mut it = v
        .split(|c: char| c == ' ' || c == ',')
        .filter(|s| !s.is_empty());
    Some((
        it.next()?.trim().parse().ok()?,
        it.next()?.trim().parse().ok()?,
    ))
}

impl ControlFile {
    /// Parses `Key=Value` lines; unknown keys are ignored, later duplicates win (as xLODGen's
    /// options file is appended to its own export).
    pub fn parse(text: &str) -> Self {
        let mut c = ControlFile::default();
        for line in text.lines() {
            let line = line.trim();
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let (key, value) = (key.trim(), value.trim());
            c.keys.push(key.to_string());
            let num = || value.parse::<f32>().ok();
            let int = || value.parse::<i32>().ok();
            match key {
                "GameMode" => c.game_mode = Some(value.to_string()),
                "Worldspace" => c.world = Some(value.to_string()),
                "CellSW" => c.cell_sw = parse_pair(value),
                "MaxLevel" => c.max_level = int(),
                "LODLevel" => c.lod_level = int(),
                "TerrainData" => c.terrain_data = Some(value.to_string()),
                "LandHeight" => c.land_height = num(),
                "WaterHeight" => c.water_height = num(),
                "PathOutput" => c.path_output = Some(value.to_string()),
                "MinX" => c.min_x = int(),
                "MinY" => c.min_y = int(),
                "MaxX" => c.max_x = int(),
                "MaxY" => c.max_y = int(),
                "IgnoreHide" => c.ignore_hide = value.split(';').filter_map(parse_pair).collect(),
                "RemoveUnseenFaces" => c.remove_unseen_faces = parse_bool(value),
                "IgnoreWater" => c.settings.ignore_water = parse_bool(value).unwrap_or(false),
                "ProtectCellBorders" => {
                    c.settings.protect_cell_borders = parse_bool(value).unwrap_or(true)
                }
                "HideQuads" => c.settings.hide_quads = parse_bool(value).unwrap_or(false),
                "Skirts" => c.settings.skirts = int().unwrap_or(0),
                _ => {
                    for (prefix, field) in [
                        ("QualityLOD", 0),
                        ("MaxVertsLOD", 1),
                        ("WaterDeltaLOD", 2),
                        ("ZShiftLOD", 3),
                    ] {
                        if let Some(l) =
                            key.strip_prefix(prefix).and_then(|l| l.parse::<i32>().ok())
                        {
                            if !LEVELS.contains(&l) {
                                continue;
                            }
                            let s = c.settings.level_mut(l);
                            match field {
                                0 => s.quality = num().unwrap_or(s.quality),
                                1 => {
                                    s.max_verts =
                                        int().map(|v| v.max(0) as u32).unwrap_or(s.max_verts)
                                }
                                2 => s.water_delta = num().unwrap_or(s.water_delta),
                                _ => s.z_shift = num().unwrap_or(s.z_shift),
                            }
                        }
                    }
                }
            }
        }
        c
    }

    /// The text of a control file with these settings (the xLODGen layout, for diagnostics).
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        let mut kv = |k: &str, v: String| {
            out.push_str(k);
            out.push('=');
            out.push_str(&v);
            out.push('\n');
        };
        if let Some(v) = &self.game_mode {
            kv("GameMode", v.clone());
        }
        if let Some(v) = &self.world {
            kv("Worldspace", v.clone());
        }
        if let Some((x, y)) = self.cell_sw {
            kv("CellSW", format!("{x} {y}"));
        }
        if let Some(v) = self.max_level {
            kv("MaxLevel", v.to_string());
        }
        if let Some(v) = self.lod_level {
            kv("LODLevel", v.to_string());
        }
        if let Some(v) = &self.terrain_data {
            kv("TerrainData", v.clone());
        }
        kv("IgnoreWater", self.settings.ignore_water.to_string());
        if let Some(v) = self.land_height {
            kv("LandHeight", v.to_string());
        }
        if let Some(v) = self.water_height {
            kv("WaterHeight", v.to_string());
        }
        kv(
            "ProtectCellBorders",
            self.settings.protect_cell_borders.to_string(),
        );
        kv("HideQuads", self.settings.hide_quads.to_string());
        for l in LEVELS {
            let s = self.settings.level(l);
            kv(&format!("QualityLOD{l}"), s.quality.to_string());
            kv(&format!("MaxVertsLOD{l}"), s.max_verts.to_string());
            kv(&format!("WaterDeltaLOD{l}"), s.water_delta.to_string());
        }
        kv("Skirts", self.settings.skirts.to_string());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_xlodgen_control_file() {
        let text = "GameMode=TERRAINSSE\nWorldspace=Tamriel\nCellSW=-96 -96\nMaxLevel=32\nIgnoreWater=False\nLandHeight=-27000\nWaterHeight=-14000\nProtectCellBorders=True\nHideQuads=False\nQualityLOD4=5\nMaxVertsLOD4=32767\nWaterDeltaLOD4=-1\nQualityLOD32=2.5\nWaterDeltaLOD32=550\nSkirts=256\n";
        let c = ControlFile::parse(text);
        assert_eq!(c.world.as_deref(), Some("Tamriel"));
        assert_eq!(c.cell_sw, Some((-96, -96)));
        assert_eq!(
            (c.land_height, c.water_height),
            (Some(-27000.0), Some(-14000.0))
        );
        assert_eq!(c.settings.level(4).quality, 5.0);
        assert_eq!(c.settings.level(32).quality, 2.5);
        assert_eq!(c.settings.level(32).water_delta, 550.0);
        assert_eq!(c.settings.level(8).water_delta, 0.0); // default kept
        assert!(
            c.settings.protect_cell_borders && !c.settings.hide_quads && !c.settings.ignore_water
        );
        assert_eq!(c.settings.skirts, 256);
    }

    #[test]
    fn parses_underside_export() {
        let text = "GameMode=UNDERTERRAINSSE\nWorldspace=DLC2SolstheimWorld\nCellSW=-64 -64\nLODLevel=32\nMaxX=63\nMinY=-2\nMaxY=26\nIgnoreHide=-44,-1;-44,0;-43,1\nRemoveUnseenFaces=True\nProtectCellBorders=False\nQualityLOD32=10\nWaterDeltaLOD32=-1\nSkirts=0\n";
        let c = ControlFile::parse(text);
        assert_eq!(c.lod_level, Some(32));
        assert_eq!(
            (c.min_x, c.min_y, c.max_x, c.max_y),
            (None, Some(-2), Some(63), Some(26))
        );
        assert_eq!(c.ignore_hide, vec![(-44, -1), (-44, 0), (-43, 1)]);
        assert_eq!(c.remove_unseen_faces, Some(true));
        assert!(!c.settings.protect_cell_borders);
        assert_eq!(c.settings.skirts, 0);
    }

    #[test]
    fn skirt_depths_match_observation() {
        assert_eq!(
            [4, 8, 16, 32].map(|l| skirt_depth(256, l)),
            [319.0, 256.0, 193.0, 130.0]
        );
        assert_eq!(skirt_depth(0, 4), 0.0);
    }
}
