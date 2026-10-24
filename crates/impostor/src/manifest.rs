// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! Impostor manifests (JSON): one per type (`types\<name>.json`) and the set index
//! (`impostors.json`) the runtime and the viewer read. Field meanings: docs/runtime/impostors.md.

use serde::{Deserialize, Serialize};

/// Manifest format version.
pub const MANIFEST_VERSION: u32 = 1;

/// A base record (TREE, or STAT with Has Tree LOD) drawn with this impostor type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BaseRef {
    /// Lower-case file name of the plugin defining the record.
    pub plugin: String,
    /// Form id without the load-order byte(s), hex (`0006F0F2`; light plugins `00000FFF`).
    pub form_id: String,
    pub editor_id: String,
}

/// One impostor type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TypeManifest {
    pub version: u32,
    /// `<model stem>_<crc32 of the model path>`, lower case: the type's identity.
    pub name: String,
    /// Data-relative model path as rendered (lower case, `meshes\...`, or the DynDOLOD
    /// `_static` replacement).
    pub model: String,
    /// The record's MODL (lower case, `meshes\...`).
    pub record_model: String,
    pub model_crc32: String,
    pub bases: Vec<BaseRef>,
    /// Frames per side.
    pub grid: u32,
    pub frame_px: u32,
    /// `grid * frame_px`.
    pub atlas_px: u32,
    /// Quad edge in model units at reference scale 1 (the bounding sphere's diameter).
    pub quad_size: f32,
    /// Model origin (trunk base) -> impostor centre, model units (Z up).
    pub bounds_center_offset: [f32; 3],
    pub bounds_min: [f32; 3],
    pub bounds_max: [f32; 3],
    /// Index into [`Index::pages`].
    pub page: u32,
    /// Data-relative page textures.
    pub albedo: String,
    pub normal_depth: String,
    /// Pixel rectangle on the page `[x, y, w, h]` (top-left origin).
    pub rect_px: [u32; 4],
    /// The same, normalised by the page size `[u0, v0, du, dv]`.
    pub rect: [f32; 4],
    /// Mean coverage of the frames (0..1).
    pub coverage: f32,
    pub cache_key: String,
}

/// One atlas page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PageInfo {
    pub index: u32,
    pub albedo: String,
    pub normal_depth: String,
    pub width: u32,
    pub height: u32,
    pub mips: u32,
}

/// The set index: every page and type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Index {
    pub version: u32,
    pub baker_version: u32,
    /// Hemisphere convention: `"hemi-oct-z-up"` (docs/runtime/impostors.md).
    pub layout: String,
    pub worlds: Vec<String>,
    pub pages: Vec<PageInfo>,
    pub types: Vec<TypeManifest>,
}

/// `meshes\landscape\trees\treepineforest01.nif` -> `treepineforest01_<crc32 of the path>`.
pub fn type_name(model: &str) -> String {
    let m = model.replace('/', "\\").to_ascii_lowercase();
    let stem = m.rsplit('\\').next().unwrap_or(&m);
    let stem = stem.strip_suffix(".nif").unwrap_or(stem);
    let clean: String = stem.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' { c } else { '_' }).collect();
    format!("{clean}_{:08x}", crc32(m.as_bytes()))
}

/// CRC-32 (IEEE).
pub fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
        }
    }
    !c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_names_are_stable_and_path_case_insensitive() {
        let a = type_name(r"Meshes\Landscape\Trees\TreePineForest01.nif");
        assert_eq!(a, type_name("meshes/landscape/trees/treepineforest01.nif"));
        assert!(a.starts_with("treepineforest01_") && a.len() == "treepineforest01_".len() + 8);
        assert_ne!(a, type_name(r"meshes\other\treepineforest01.nif"));
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }
}
