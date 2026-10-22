// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! Weather and water looks for the viewer: sky gradient, distance fog, sun and ambient light
//! from a weather record (WTHR) and the distant water colours from the worldspace's LOD water
//! type (WATR), so a view reads like the game's under that weather. Daytime values only.

use serde::{Deserialize, Serialize};

/// Times of day in WTHR `NAM0` (each colour type holds sunrise, day, sunset, night).
pub const TIME_DAY: usize = 1;

/// Colour types of WTHR `NAM0` (Skyrim SE, 17 types x 4 times x RGBA8).
pub mod color {
    pub const SKY_UPPER: usize = 0;
    pub const FOG_NEAR: usize = 1;
    pub const AMBIENT: usize = 3;
    pub const SUNLIGHT: usize = 4;
    pub const SKY_LOWER: usize = 7;
    pub const HORIZON: usize = 8;
    pub const FOG_FAR: usize = 12;
    pub const WATER_MULTIPLIER: usize = 14;
}

/// Everything the viewer takes from a weather and a water type. Colours are sRGB 0..1 as the
/// records store them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Atmosphere {
    pub weather: String,
    pub water: String,
    pub fog_near: f32,
    pub fog_far: f32,
    pub fog_power: f32,
    pub fog_max: f32,
    pub fog_near_color: [f32; 3],
    pub fog_far_color: [f32; 3],
    pub sky_upper: [f32; 3],
    pub sky_lower: [f32; 3],
    pub horizon: [f32; 3],
    pub sunlight: [f32; 3],
    pub ambient: [f32; 3],
    pub water_multiplier: [f32; 3],
    pub water_deep: [f32; 3],
    pub water_shallow: [f32; 3],
    pub water_reflection: [f32; 3],
    pub water_fresnel: f32,
    pub water_reflectivity: f32,
}

fn rgb8(r: u8, g: u8, b: u8) -> [f32; 3] {
    [f32::from(r) / 255.0, f32::from(g) / 255.0, f32::from(b) / 255.0]
}

impl Default for Atmosphere {
    /// SkyrimClear_A (Skyrim.esm 0010E1F2) at day and DefaultWater (00000018).
    fn default() -> Self {
        Atmosphere {
            weather: "SkyrimClear_A".into(),
            water: "DefaultWater".into(),
            fog_near: 1200.0,
            fog_far: 80000.0,
            fog_power: 0.4,
            fog_max: 0.85,
            fog_near_color: rgb8(40, 111, 140),
            fog_far_color: rgb8(142, 201, 230),
            sky_upper: rgb8(57, 119, 155),
            sky_lower: rgb8(100, 159, 188),
            horizon: rgb8(125, 163, 183),
            sunlight: rgb8(163, 150, 135),
            ambient: rgb8(170, 192, 197),
            water_multiplier: rgb8(175, 237, 214),
            water_deep: rgb8(5, 16, 5),
            water_shallow: rgb8(37, 52, 37),
            water_reflection: rgb8(103, 122, 117),
            water_fresnel: 0.05,
            water_reflectivity: 1.0,
        }
    }
}

fn f32_at(d: &[u8], off: usize) -> Option<f32> {
    let b = d.get(off..off + 4)?;
    Some(f32::from_le_bytes([b[0], b[1], b[2], b[3]])).filter(|v| v.is_finite())
}

fn rgb_at(d: &[u8], off: usize) -> Option<[f32; 3]> {
    let b = d.get(off..off + 3)?;
    Some(rgb8(b[0], b[1], b[2]))
}

impl Atmosphere {
    /// Takes the weather's day values from WTHR `NAM0` (colours) and `FNAM` (fog distances:
    /// day near, day far, night near, night far, day power, night power, day max, night max).
    /// Missing or short fields keep the current values.
    pub fn apply_weather(&mut self, editor_id: &str, nam0: Option<&[u8]>, fnam: Option<&[u8]>) {
        self.weather = editor_id.to_string();
        if let Some(d) = nam0 {
            let at = |ty: usize| rgb_at(d, (ty * 4 + TIME_DAY) * 4);
            let set = |dst: &mut [f32; 3], ty: usize| {
                if let Some(c) = at(ty) {
                    *dst = c;
                }
            };
            set(&mut self.sky_upper, color::SKY_UPPER);
            set(&mut self.fog_near_color, color::FOG_NEAR);
            set(&mut self.ambient, color::AMBIENT);
            set(&mut self.sunlight, color::SUNLIGHT);
            set(&mut self.sky_lower, color::SKY_LOWER);
            set(&mut self.horizon, color::HORIZON);
            set(&mut self.fog_far_color, color::FOG_FAR);
            set(&mut self.water_multiplier, color::WATER_MULTIPLIER);
        }
        if let Some(d) = fnam {
            if let (Some(near), Some(far)) = (f32_at(d, 0), f32_at(d, 4)) {
                if far > near {
                    self.fog_near = near;
                    self.fog_far = far;
                }
            }
            if let Some(p) = f32_at(d, 16).filter(|p| *p > 0.0) {
                self.fog_power = p;
            }
            if let Some(m) = f32_at(d, 24).filter(|m| *m > 0.0) {
                self.fog_max = m.min(1.0);
            }
        }
    }

    /// Takes the water colours from WATR `DNAM`: reflectivity at byte 20, fresnel at 24,
    /// shallow / deep / reflection colours (RGBA8) at 40 / 44 / 48.
    pub fn apply_water(&mut self, editor_id: &str, dnam: &[u8]) {
        if dnam.len() < 52 {
            return;
        }
        self.water = editor_id.to_string();
        if let Some(v) = f32_at(dnam, 20) {
            self.water_reflectivity = v.clamp(0.0, 1.0);
        }
        if let Some(v) = f32_at(dnam, 24) {
            self.water_fresnel = v.clamp(0.0, 1.0);
        }
        self.water_shallow = rgb_at(dnam, 40).unwrap_or(self.water_shallow);
        self.water_deep = rgb_at(dnam, 44).unwrap_or(self.water_deep);
        self.water_reflection = rgb_at(dnam, 48).unwrap_or(self.water_reflection);
    }

    /// Shader constants (sRGB colours; the shader works in gamma space like the game without
    /// linear lighting): fog (near, far, power, max), fog near, fog far, sky upper, sky lower,
    /// horizon, sunlight, ambient, water deep, water reflected sky (the far fog colour), water
    /// (fresnel, reflectivity, -, -).
    pub fn gpu(&self) -> [[f32; 4]; 11] {
        let c = |c: [f32; 3]| [c[0], c[1], c[2], 0.0];
        [
            [self.fog_near, self.fog_far, self.fog_power, self.fog_max],
            c(self.fog_near_color),
            c(self.fog_far_color),
            c(self.sky_upper),
            c(self.sky_lower),
            c(self.horizon),
            c(self.sunlight),
            c(self.ambient),
            c(self.water_deep),
            // Reflected sky: the far fog colour, what the sky dome shows near the horizon (the
            // water multiplier tints the game's refraction, not its reflections).
            c(self.fog_far_color),
            [self.water_fresnel, self.water_reflectivity, 0.0, 0.0],
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weather_fields() {
        // NAM0 with only the day sky-upper and fog-far colours set; FNAM of SkyrimClear_A.
        let mut nam0 = vec![0u8; 272];
        nam0[(color::SKY_UPPER * 4 + TIME_DAY) * 4..][..3].copy_from_slice(&[0x39, 0x77, 0x9B]);
        nam0[(color::FOG_FAR * 4 + TIME_DAY) * 4..][..3].copy_from_slice(&[0x8E, 0xC9, 0xE6]);
        let fnam: [u8; 32] = [
            0x00, 0x00, 0x96, 0x44, 0x00, 0x40, 0x9C, 0x47, 0x00, 0x00, 0x96, 0x44, 0x00, 0x40, 0x1C, 0x47, //
            0xCD, 0xCC, 0xCC, 0x3E, 0xCD, 0xCC, 0xCC, 0x3E, 0x9A, 0x99, 0x59, 0x3F, 0x9A, 0x99, 0x59, 0x3F,
        ];
        let mut a = Atmosphere::default();
        a.apply_weather("Test", Some(&nam0), Some(&fnam));
        assert_eq!(a.sky_upper, rgb8(57, 119, 155));
        assert_eq!(a.fog_far_color, rgb8(142, 201, 230));
        assert_eq!(a.ambient, [0.0; 3]);
        assert_eq!((a.fog_near, a.fog_far), (1200.0, 80000.0));
        assert!((a.fog_power - 0.4).abs() < 1e-6 && (a.fog_max - 0.85).abs() < 1e-6);
        // A short FNAM keeps the distances.
        let mut b = Atmosphere::default();
        b.apply_weather("Short", None, Some(&fnam[..4]));
        assert_eq!(b.fog_far, 80000.0);
    }

    #[test]
    fn water_fields() {
        let mut d = vec![0u8; 228];
        d[20..24].copy_from_slice(&1.0f32.to_le_bytes());
        d[24..28].copy_from_slice(&0.05f32.to_le_bytes());
        d[40..43].copy_from_slice(&[37, 52, 37]);
        d[44..47].copy_from_slice(&[5, 16, 5]);
        d[48..51].copy_from_slice(&[103, 122, 117]);
        let mut a = Atmosphere::default();
        a.water_deep = [1.0; 3];
        a.apply_water("DefaultWater", &d);
        assert_eq!(a.water_deep, rgb8(5, 16, 5));
        assert_eq!(a.water_reflection, rgb8(103, 122, 117));
        assert!((a.water_fresnel - 0.05).abs() < 1e-6);
        assert_eq!(a.gpu()[0], [1200.0, 80000.0, 0.4, 0.85]);
    }
}
