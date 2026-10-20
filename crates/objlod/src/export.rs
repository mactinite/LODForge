//! The LODGen export file (docs/re/object-lod-spec.md 1.1): `Key=Value` header lines, then one
//! tab-separated reference line per placed object. Two dialects write it: xLODGen (our port of
//! xEdit's writer is [`crate::source`]) and DynDOLOD (`lodforge_dyndolod::export`). The parser
//! accepts both; [`Dialect`] records which one a file came from, which decides only what the
//! header carries (options are header keys in the DynDOLOD dialect and command-line switches of
//! the xLODGen run otherwise) and whether a 17th LOD32 column exists.

use std::collections::HashMap;

/// Which tool wrote the export (spec 1.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Dialect {
    /// xLODGen: 16 columns, options not in the file, `TextureDiffuseHD` / `TextureNormalHD`.
    #[default]
    XLodgen,
    /// DynDOLOD: 17 columns, every option a header key, side files (`AltTextures`, `VC`,
    /// `FlatTextures`, `TerrainData`).
    Dyndolod,
}

/// One reference line (spec 1.1.3). Numbers are single precision as the file stores them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StaticRef {
    /// Column 1, eight upper-case hex digits (load-order form id).
    pub ref_id: String,
    /// Column 2.
    pub ref_flags: u32,
    pub x: f32,
    pub y: f32,
    pub z: f32,
    /// Columns 6-8, degrees.
    pub rot_x: f32,
    pub rot_y: f32,
    pub rot_z: f32,
    pub scale: f32,
    /// Column 10, the base record EditorID.
    pub static_name: String,
    /// Column 11.
    pub static_flags: u32,
    /// Column 12 as written: empty, `Snow`, `Ash`, `PassThru`, a lower-case MATO EditorID, with
    /// `-LargeRef` appended for large references.
    pub material_name: String,
    /// Column 13.
    pub full_model: String,
    /// Columns 14-17: LOD4, LOD8, LOD16, LOD32 model paths (empty when absent; the xLODGen
    /// dialect has no LOD32 column).
    pub models: [String; 4],
    /// Cell of the position (`floor(pos / 4096)`).
    pub cell_x: i32,
    pub cell_y: i32,
    /// Position among the reference lines (a stable order for the builder).
    pub line: usize,
}

impl StaticRef {
    pub fn cell(&self) -> (i32, i32) {
        (self.cell_x, self.cell_y)
    }

    /// `-LargeRef` in the material column (spec 2.4).
    pub fn is_large_ref(&self) -> bool {
        self.material_name.to_ascii_lowercase().ends_with("-largeref")
    }

    /// The material column without the `-LargeRef` suffix.
    pub fn material(&self) -> &str {
        let m = &self.material_name;
        if m.len() >= 9 && m[m.len() - 9..].eq_ignore_ascii_case("-largeref") { &m[..m.len() - 9] } else { m }
    }

    /// The LOD model of level index `k` (0 = LOD4 .. 3 = LOD32), `None` when the column is empty.
    pub fn model(&self, k: usize) -> Option<&str> {
        let m = self.models[k].as_str();
        (!m.is_empty()).then_some(m)
    }

    /// Fills `cell_x` / `cell_y` from the position.
    pub fn update_cell(&mut self) {
        self.cell_x = (self.x / 4096.0).floor() as i32;
        self.cell_y = (self.y / 4096.0).floor() as i32;
    }
}

/// A parsed export file.
#[derive(Debug, Clone, Default)]
pub struct ExportFile {
    pub dialect: Dialect,
    pub game_mode: String,
    pub worldspace: String,
    /// `CellSW`.
    pub south_west: (i32, i32),
    /// `Season=` (`SPR`, `SUM`, `AUT`, `WIN`), empty for the default season.
    pub season: String,
    /// `AtlasTolerance` (xLODGen: `UVRange - 1`, default 0.5).
    pub atlas_tolerance: f32,
    /// xLODGen `TextureDiffuseHD` / `TextureNormalHD` as lower-case `textures\...` paths.
    pub hd_textures: Vec<String>,
    /// `TextureAtlasMap=` (absolute path; may be empty).
    pub atlas_map: String,
    /// DynDOLOD side files (absolute paths; empty in the xLODGen dialect).
    pub alt_textures_file: String,
    pub vc_file: String,
    pub flat_textures_file: String,
    pub terrain_data: String,
    pub path_data: String,
    pub path_output: String,
    /// `Resource=` lines (loaded archives, load order).
    pub resources: Vec<String>,
    /// `IgnoreTranslation=` entries, lower case (xLODGen, spec 2.2).
    pub ignore_translation: Vec<String>,
    /// xLODGen `FlatTextures=` values (the ten-field billboard lines of spec 1.1.4), raw.
    pub flat_textures: Vec<String>,
    /// Season exports: `LODCoords4/8/16/32`, the quads whose content differs from the default
    /// season (spec 1.4); `None` when the header has no such key.
    pub lod_coords: Option<[Vec<(i32, i32)>; 4]>,
    /// `LimitCoords4/8/16/32`: the quads to generate at that level; `None` = all (spec 2.3).
    pub limit_coords: [Option<Vec<(i32, i32)>>; 4],
    /// Every header key with its value, in file order, keys upper-cased (keys may repeat).
    pub settings: Vec<(String, String)>,
    /// Lines that started with eight hex digits but had fewer than 16 columns.
    pub malformed_lines: usize,
    pub statics: Vec<StaticRef>,
}

/// Decodes the bytes of an export, atlas map or side file: a UTF-8 BOM is dropped, valid UTF-8
/// is taken as is, anything else is read as Windows-1252 / Latin-1 (the files are ASCII in
/// practice, with mod names in paths).
pub fn decode(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) => bytes.iter().map(|&b| b as char).collect(),
    }
}

fn is_ref_line(line: &str) -> bool {
    line.len() > 8 && line.as_bytes()[..8].iter().all(|c| c.is_ascii_hexdigit()) && matches!(line.as_bytes()[8], b'\t' | b' ')
}

fn parse_coords(v: &str) -> Vec<(i32, i32)> {
    v.split(';')
        .filter_map(|p| {
            let (x, y) = p.trim().split_once(',')?;
            Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
        })
        .collect()
}

fn hex(s: &str) -> u32 {
    u32::from_str_radix(s.trim(), 16).unwrap_or(0)
}

fn num(s: &str) -> f32 {
    s.trim().parse().unwrap_or(0.0)
}

/// Lower-case `textures\...` form of a header texture path (`data\textures\x.dds` -> `textures\x.dds`).
pub fn texture_key(path: &str) -> String {
    let l = path.trim().to_ascii_lowercase().replace('/', "\\");
    let l = l.trim_start_matches('\\');
    match l.find("textures\\") {
        Some(i) => l[i..].to_string(),
        None if l.is_empty() => String::new(),
        None => format!("textures\\{l}"),
    }
}

/// Keys only the DynDOLOD dialect writes (spec 1.1.2).
const DYNDOLOD_KEYS: &[&str] = &["TERRAINDATA", "ALTTEXTURES", "VC", "LIMITCOORDS4", "LIMITCOORDS8", "LODCOORDS4", "USEHDFLAG", "PASSTHRUMESHMASK", "REMOVEUNSEENFACES", "CREATEHEIGHTMAP"];

impl ExportFile {
    /// Parses an export, detecting the dialect from the header keys.
    pub fn parse(text: &str) -> ExportFile {
        let dyn_keys = text.lines().take_while(|l| !is_ref_line(l)).any(|l| l.split_once('=').is_some_and(|(k, _)| DYNDOLOD_KEYS.contains(&k.trim().to_ascii_uppercase().as_str())));
        Self::parse_with(text, if dyn_keys { Dialect::Dyndolod } else { Dialect::XLodgen })
    }

    /// Parses an export as the given dialect.
    pub fn parse_with(text: &str, dialect: Dialect) -> ExportFile {
        let mut e = ExportFile { dialect, atlas_tolerance: 0.5, ..Default::default() };
        let mut limit: [Option<Vec<(i32, i32)>>; 4] = Default::default();
        let mut lod_coords: Option<[Vec<(i32, i32)>; 4]> = None;
        for raw in text.lines() {
            let line = raw.trim_end_matches(['\r', '\n']);
            if line.trim().is_empty() {
                continue;
            }
            if is_ref_line(line) {
                let cols: Vec<&str> = line.split('\t').collect();
                if cols.len() < 16 {
                    e.malformed_lines += 1;
                    continue;
                }
                let col = |i: usize| cols.get(i).map(|s| s.trim()).unwrap_or("");
                let mut r = StaticRef {
                    ref_id: col(0).to_ascii_uppercase(),
                    ref_flags: hex(col(1)),
                    x: num(col(2)),
                    y: num(col(3)),
                    z: num(col(4)),
                    rot_x: num(col(5)),
                    rot_y: num(col(6)),
                    rot_z: num(col(7)),
                    scale: if col(8).is_empty() { 1.0 } else { num(col(8)) },
                    static_name: col(9).to_string(),
                    static_flags: hex(col(10)),
                    material_name: col(11).to_string(),
                    full_model: col(12).to_string(),
                    models: [col(13).to_string(), col(14).to_string(), col(15).to_string(), col(16).to_string()],
                    cell_x: 0,
                    cell_y: 0,
                    line: e.statics.len(),
                };
                r.update_cell();
                e.statics.push(r);
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            let key = k.trim().to_ascii_uppercase();
            let v = v.trim();
            match key.as_str() {
                "GAMEMODE" => e.game_mode = v.to_string(),
                "WORLDSPACE" => e.worldspace = v.to_string(),
                "CELLSW" => {
                    let mut it = v.split_whitespace().filter_map(|s| s.parse::<i32>().ok());
                    e.south_west = (it.next().unwrap_or(0), it.next().unwrap_or(0));
                }
                "SEASON" => e.season = v.to_ascii_uppercase(),
                "ATLASTOLERANCE" => e.atlas_tolerance = v.parse().unwrap_or(0.5),
                "TEXTUREDIFFUSEHD" | "TEXTURENORMALHD" => {
                    let t = texture_key(v);
                    if !t.is_empty() {
                        e.hd_textures.push(t);
                    }
                }
                "TEXTUREATLASMAP" => e.atlas_map = v.to_string(),
                "ALTTEXTURES" => e.alt_textures_file = v.to_string(),
                "VC" => e.vc_file = v.to_string(),
                "TERRAINDATA" => e.terrain_data = v.to_string(),
                "PATHDATA" => e.path_data = v.to_string(),
                "PATHOUTPUT" => e.path_output = v.to_string(),
                "RESOURCE" => e.resources.push(v.to_string()),
                "IGNORETRANSLATION" => e.ignore_translation.push(v.to_ascii_lowercase().replace('/', "\\")),
                "FLATTEXTURES" => {
                    // xLODGen: a ten-field billboard line; DynDOLOD: the side file's path.
                    if v.contains('\t') {
                        e.flat_textures.push(v.to_string());
                    } else {
                        e.flat_textures_file = v.to_string();
                    }
                }
                "LIMITCOORDS4" | "LIMITCOORDS8" | "LIMITCOORDS16" | "LIMITCOORDS32" => {
                    let i = crate::grid::level_index(key[11..].parse().unwrap_or(4));
                    limit[i].get_or_insert_with(Vec::new).extend(parse_coords(v));
                }
                "LODCOORDS4" | "LODCOORDS8" | "LODCOORDS16" | "LODCOORDS32" => {
                    let i = crate::grid::level_index(key[9..].parse().unwrap_or(4));
                    lod_coords.get_or_insert_with(Default::default)[i].extend(parse_coords(v));
                }
                _ => {}
            }
            e.settings.push((key, v.to_string()));
        }
        e.limit_coords = limit;
        e.lod_coords = lod_coords;
        e
    }

    /// First value of a header key (case-insensitive).
    pub fn setting(&self, key: &str) -> Option<&str> {
        let k = key.to_ascii_uppercase();
        self.settings.iter().find(|(a, _)| *a == k).map(|(_, v)| v.as_str())
    }

    /// Every value of a (repeating) header key.
    pub fn settings_all(&self, key: &str) -> Vec<&str> {
        let k = key.to_ascii_uppercase();
        self.settings.iter().filter(|(a, _)| *a == k).map(|(_, v)| v.as_str()).collect()
    }

    /// A boolean header key (`True` / `true` / `1`), `default` when absent.
    pub fn flag(&self, key: &str, default: bool) -> bool {
        match self.setting(key) {
            Some(v) => matches!(v.trim().to_ascii_lowercase().as_str(), "true" | "1" | "yes"),
            None => default,
        }
    }

    pub fn number(&self, key: &str) -> Option<f32> {
        self.setting(key)?.trim().parse().ok()
    }

    /// The distinct LOD model paths named by the reference lines, lower case.
    pub fn model_paths(&self) -> Vec<String> {
        let mut seen: HashMap<String, ()> = HashMap::new();
        let mut out = Vec::new();
        for s in &self.statics {
            for m in &s.models {
                if m.is_empty() {
                    continue;
                }
                let l = m.to_ascii_lowercase();
                if seen.insert(l.clone(), ()).is_none() {
                    out.push(l);
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const XLODGEN: &str = "GameMode=SSE\r\nWorldspace=Tamriel\r\nCellSW=-96 -96\r\nTextureDiffuseHD=data\\textures\\landscape\\mountains\\mountainslab02.dds\r\nTextureNormalHD=data\\textures\\landscape\\mountains\\mountainslab02_n.dds\r\nTextureAtlasMap=D:\\x\\map.txt\r\nAtlasTolerance=0.5\r\nPathData=D:\\Data\\\r\nResource=D:\\a.bsa\r\nIgnoreTranslation=meshes\\lod\\solitude\\cwtower01_lod.nif\r\nFlatTextures=textures\\x.dds\t10\t20\t0\t1\t1\tLODGen_flat_lod.nif\t\t-1\t-1\r\n00012DF6\t00000000\t22176.359375\t-45866.6796875\t-118.982612609863\t0.0000\t0.0000\t135.0000\t1.0\tFarmhouse02\t00008000\t-LargeRef\tMeshes\\Architecture\\Farmhouse\\Farmhouse02.NIF\tMeshes\\LOD\\Farmhouse\\Farmhouse02_LOD.nif\tMeshes\\LOD\\Farmhouse\\Farmhouse02_LOD.nif\t\r\n";

    #[test]
    fn xlodgen_dialect() {
        let e = ExportFile::parse(XLODGEN);
        assert_eq!(e.dialect, Dialect::XLodgen);
        assert_eq!(e.worldspace, "Tamriel");
        assert_eq!(e.south_west, (-96, -96));
        assert_eq!(e.hd_textures, ["textures\\landscape\\mountains\\mountainslab02.dds", "textures\\landscape\\mountains\\mountainslab02_n.dds"]);
        assert_eq!(e.atlas_tolerance, 0.5);
        assert_eq!(e.ignore_translation, ["meshes\\lod\\solitude\\cwtower01_lod.nif"]);
        assert_eq!(e.flat_textures.len(), 1);
        assert_eq!(e.resources.len(), 1);
        assert_eq!(e.statics.len(), 1);
        let s = &e.statics[0];
        assert_eq!(s.ref_id, "00012DF6");
        assert_eq!((s.x, s.y, s.z), (22176.36, -45866.68, -118.98261));
        assert_eq!(s.rot_z, 135.0);
        assert_eq!(s.scale, 1.0);
        assert_eq!(s.static_flags, 0x8000);
        assert!(s.is_large_ref());
        assert_eq!(s.material(), "");
        assert_eq!(s.cell(), (5, -12));
        assert_eq!(s.model(0), Some("Meshes\\LOD\\Farmhouse\\Farmhouse02_LOD.nif"));
        assert_eq!(s.model(2), None);
        assert_eq!(s.model(3), None);
        assert_eq!(e.model_paths().len(), 1);
    }

    #[test]
    fn dyndolod_dialect() {
        let t = "GameMode=SSE\r\nWorldspace=Tamriel\r\nCellSW=-96 -96\r\nLODCoords32=\r\nLODCoords16=0,0;16,16\r\nLODCoords8=\r\nLODCoords4=4,-16;8,-16\r\nTerrainData=D:\\t.bin\r\nLimitCoords4=-40,16;-36,16\r\nLimitCoords8=-48,0\r\nRemoveUnseenFaces=True\r\nAltTextures=D:\\alt.txt\r\nVC=D:\\vc.txt\r\nFlatTextures=D:\\flat.txt\r\nUseHDFlag=true\r\nIsHDTextureMask=a\r\nIsHDTextureMask=b\r\nPassThruMeshMask=glow_lod\r\nSeason=WIN\r\n00012DF6\t00000000\t22176.359375\t-45866.679688\t-118.982613\t0.000000\t0.000000\t135.000000\t1.000000\tFarmhouse02\t00008000\tSnow-LargeRef\tMeshes\\architecture\\farmhouse\\farmhouse02.nif\tmeshes\\lod\\farmhouse\\farmhouse02_lod.nif\tmeshes\\lod\\farmhouse\\farmhouse02_lod.nif\tmeshes\\lod\\farmhouse\\farmhouse02_lod.nif\t\r\nbad\t1\r\n";
        let e = ExportFile::parse(t);
        assert_eq!(e.dialect, Dialect::Dyndolod);
        assert_eq!(e.season, "WIN");
        assert_eq!(e.limit_coords[0].as_deref(), Some(&[(-40, 16), (-36, 16)][..]));
        assert_eq!(e.limit_coords[1].as_deref(), Some(&[(-48, 0)][..]));
        assert!(e.limit_coords[2].is_none());
        let lc = e.lod_coords.as_ref().unwrap();
        assert_eq!(lc[0], [(4, -16), (8, -16)]);
        assert_eq!(lc[2], [(0, 0), (16, 16)]);
        assert!(lc[3].is_empty());
        assert_eq!(e.settings_all("IsHDTextureMask"), ["a", "b"]);
        assert!(e.flag("UseHDFlag", false));
        assert!(e.flag("RemoveUnseenFaces", false));
        assert_eq!(e.flat_textures_file, "D:\\flat.txt");
        assert_eq!(e.statics[0].material(), "Snow");
        assert!(e.statics[0].is_large_ref());
        assert_eq!(e.statics[0].model(3), None);
        assert_eq!(e.malformed_lines, 0);
    }

    #[test]
    fn decoding() {
        assert_eq!(decode(&[0xEF, 0xBB, 0xBF, b'a']), "a");
        assert_eq!(decode(&[b'a', 0xE9]), "a\u{e9}");
        assert_eq!(texture_key("Data\\Textures\\A.dds"), "textures\\a.dds");
        assert_eq!(texture_key("lod\\x.dds"), "textures\\lod\\x.dds");
    }
}
