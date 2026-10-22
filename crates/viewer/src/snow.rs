// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! Snow and ash on object LOD. The game picks a directional material (MATO) by the shape's name
//! (`objSnow` -> SnowLODMaterial, `objSnowHD` -> SnowLODMaterialHD, `objAsh`, `objAshHD`) and
//! projects it onto faces that point along the material's projection vector, scaled by the
//! shape's vertex alpha. The parameters live in the material records; the defaults here are the
//! vanilla ones, replaced from the load order when it is known.

use serde::{Deserialize, Serialize};

/// Which directional material a shape gets (by name).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SnowClass {
    #[default]
    None,
    Snow,
    SnowHd,
    Ash,
    AshHd,
}

impl SnowClass {
    /// Index passed to the shader (0 = none).
    pub fn index(self) -> u32 {
        match self {
            SnowClass::None => 0,
            SnowClass::Snow => 1,
            SnowClass::SnowHd => 2,
            SnowClass::Ash => 3,
            SnowClass::AshHd => 4,
        }
    }

    /// The HD variants ignore vertex alpha as a multiplier and add it as a bias instead.
    pub fn is_hd(self) -> bool {
        matches!(self, SnowClass::SnowHd | SnowClass::AshHd)
    }
}

/// The class of an object LOD shape name: `obj` or `objpassthru`, then `Snow` / `Ash`, then
/// `HD`, then an optional `-LargeRef`. Anything else (`obj`, `obj-LargeRef`, `treepassthru`,
/// `objvolcanicsulfurcrystalslite`) has none.
pub fn class_of(name: &str) -> SnowClass {
    let n = name.to_ascii_lowercase();
    let Some(rest) = n.strip_prefix("objpassthru").or_else(|| n.strip_prefix("obj")) else {
        return SnowClass::None;
    };
    let (ash, rest) = if let Some(r) = rest.strip_prefix("snow") {
        (false, r)
    } else if let Some(r) = rest.strip_prefix("ash") {
        (true, r)
    } else {
        return SnowClass::None;
    };
    let hd = rest.starts_with("hd");
    match (ash, hd) {
        (false, false) => SnowClass::Snow,
        (false, true) => SnowClass::SnowHd,
        (true, false) => SnowClass::Ash,
        (true, true) => SnowClass::AshHd,
    }
}

/// The fields of a directional material (MATO `DATA`) the projection uses.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DirMaterial {
    pub falloff_scale: f32,
    pub falloff_bias: f32,
    pub noise_uv_scale: f32,
    pub material_uv_scale: f32,
    pub projection: [f32; 3],
    pub normal_dampener: f32,
    /// Colour the projected texture is multiplied with (sRGB, 0..1).
    pub color: [f32; 3],
}

impl DirMaterial {
    /// Parses the 48-byte MATO `DATA` field: falloff scale, falloff bias, noise UV scale,
    /// material UV scale, projection vector, normal dampener, single pass colour, flags.
    pub fn from_mato_data(d: &[u8]) -> Option<DirMaterial> {
        if d.len() < 44 {
            return None;
        }
        let f = |i: usize| f32::from_le_bytes([d[i * 4], d[i * 4 + 1], d[i * 4 + 2], d[i * 4 + 3]]);
        let m = DirMaterial {
            falloff_scale: f(0),
            falloff_bias: f(1),
            noise_uv_scale: f(2),
            material_uv_scale: f(3),
            projection: [f(4), f(5), f(6)],
            normal_dampener: f(7),
            color: [f(8), f(9), f(10)],
        };
        let all = [m.falloff_scale, m.falloff_bias, m.noise_uv_scale, m.normal_dampener];
        all.iter().all(|v| v.is_finite()).then_some(m)
    }

    const fn vanilla(fs: f32, fb: f32, noise: f32, color: [f32; 3], damp: f32) -> DirMaterial {
        DirMaterial {
            falloff_scale: fs,
            falloff_bias: fb,
            noise_uv_scale: noise,
            material_uv_scale: 512.0,
            projection: [0.0, 0.0, 1.0],
            normal_dampener: damp,
            color,
        }
    }
}

/// The four LOD materials.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SnowMaterials {
    pub snow: DirMaterial,
    pub snow_hd: DirMaterial,
    pub ash: DirMaterial,
    pub ash_hd: DirMaterial,
}

/// Where the game takes each LOD material from: (plugin, form id as stored in that plugin).
pub const MATERIAL_FORMS: [(SnowClass, &str, u32); 4] = [
    (SnowClass::Snow, "Skyrim.esm", 0x0005_0372),
    (SnowClass::SnowHd, "Skyrim.esm", 0x0006_AE38),
    (SnowClass::Ash, "Dragonborn.esm", 0x0203_2720),
    (SnowClass::AshHd, "Dragonborn.esm", 0x0202_4FE0),
];

impl Default for SnowMaterials {
    /// The vanilla records: SnowLODMaterial, SnowLODMaterialHD (Skyrim.esm),
    /// AshLODMaterialMtns1P, AshMaterialSolstheimMtns1P (Dragonborn.esm).
    fn default() -> Self {
        SnowMaterials {
            snow: DirMaterial::vanilla(0.25, 0.5, 1500.0, [0.3686, 0.4118, 0.4588], 0.4),
            snow_hd: DirMaterial::vanilla(0.45, 0.25, 50.0, [0.4745, 0.5176, 0.5686], 1.0),
            ash: DirMaterial::vanilla(0.35, 0.6, 1300.0, [0.2784, 0.2784, 0.2706], 0.4),
            ash_hd: DirMaterial::vanilla(0.4, 0.5, 20.0, [0.3216, 0.3216, 0.3216], 0.4),
        }
    }
}

impl SnowMaterials {
    pub fn get(&self, c: SnowClass) -> Option<&DirMaterial> {
        match c {
            SnowClass::None => None,
            SnowClass::Snow => Some(&self.snow),
            SnowClass::SnowHd => Some(&self.snow_hd),
            SnowClass::Ash => Some(&self.ash),
            SnowClass::AshHd => Some(&self.ash_hd),
        }
    }

    pub fn set(&mut self, c: SnowClass, m: DirMaterial) {
        match c {
            SnowClass::None => {}
            SnowClass::Snow => self.snow = m,
            SnowClass::SnowHd => self.snow_hd = m,
            SnowClass::Ash => self.ash = m,
            SnowClass::AshHd => self.ash_hd = m,
        }
    }

    /// Shader constants: three vec4 per class 1..=4: (falloff scale, falloff bias, 1 / noise UV
    /// scale, HD), (colour, unused), (projection vector, unused).
    pub fn gpu(&self) -> [[f32; 4]; 12] {
        let mut out = [[0.0; 4]; 12];
        for (i, c) in [SnowClass::Snow, SnowClass::SnowHd, SnowClass::Ash, SnowClass::AshHd].into_iter().enumerate() {
            let m = self.get(c).expect("a material class");
            let noise = if m.noise_uv_scale.abs() > 1e-3 { 1.0 / m.noise_uv_scale } else { 0.0 };
            out[i * 3] = [m.falloff_scale, m.falloff_bias, noise, if c.is_hd() { 1.0 } else { 0.0 }];
            out[i * 3 + 1] = [m.color[0], m.color[1], m.color[2], 0.0];
            out[i * 3 + 2] = [m.projection[0], m.projection[1], m.projection[2], 0.0];
        }
        out
    }
}

/// The game's projected-material textures (hard-coded in the executable).
pub const PROJECTED_DIFFUSE: &str = r"textures\effects\projecteddiffuse.dds";
pub const PROJECTED_NOISE: &str = r"textures\effects\projectednoise.dds";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(class_of("obj"), SnowClass::None);
        assert_eq!(class_of("obj-LargeRef"), SnowClass::None);
        assert_eq!(class_of("objSnow"), SnowClass::Snow);
        assert_eq!(class_of("objSnow-LargeRef"), SnowClass::Snow);
        assert_eq!(class_of("objSnowHD-LargeRef"), SnowClass::SnowHd);
        assert_eq!(class_of("objpassthruSnow"), SnowClass::Snow);
        assert_eq!(class_of("objAshHD"), SnowClass::AshHd);
        assert_eq!(class_of("objAsh"), SnowClass::Ash);
        assert_eq!(class_of("treepassthru"), SnowClass::None);
        assert_eq!(class_of("objvolcanicsulfurcrystalslite"), SnowClass::None);
    }

    #[test]
    fn mato_data() {
        // SnowLODMaterial (Skyrim.esm 00050372).
        let d: [u8; 48] = [
            0x00, 0x00, 0x80, 0x3E, 0x00, 0x00, 0x00, 0x3F, 0x00, 0x80, 0xBB, 0x44, 0x00, 0x00, 0x00, 0x44, //
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x3F, 0xCD, 0xCC, 0xCC, 0x3E, //
            0xBD, 0xBC, 0xBC, 0x3E, 0xD3, 0xD2, 0xD2, 0x3E, 0xEB, 0xEA, 0xEA, 0x3E, 0x01, 0x00, 0x00, 0x00,
        ];
        let m = DirMaterial::from_mato_data(&d).unwrap();
        assert_eq!(m.falloff_scale, 0.25);
        assert_eq!(m.falloff_bias, 0.5);
        assert_eq!(m.noise_uv_scale, 1500.0);
        assert_eq!(m.projection, [0.0, 0.0, 1.0]);
        assert!((m.color[1] - 0.4118).abs() < 1e-3);
        assert!(DirMaterial::from_mato_data(&d[..20]).is_none());
        let g = SnowMaterials::default().gpu();
        assert_eq!(g[0][0], 0.25);
        assert!((g[0][2] - 1.0 / 1500.0).abs() < 1e-9);
        assert_eq!(g[3][3], 1.0);
    }
}
