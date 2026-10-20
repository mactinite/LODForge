//! `FlatTextures` billboard descriptions (docs/re/object-lod-spec.md 1.1.4): the ten-field
//! `FlatTextures=` values of an xLODGen export, or the fifteen-field lines of DynDOLOD's
//! `LODGen_SSE_FlatTextures.txt`. Parsed here as data; the billboard geometry is stage B2.5.

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct FlatTexture {
    /// Billboard diffuse (`textures\terrain\lodgen\<plugin>\<name>_<formid>.dds`), as written.
    pub texture: String,
    pub width: f32,
    pub height: f32,
    pub shift_z: f32,
    pub scale: f32,
    /// Field 6: xLODGen writes `1`; DynDOLOD `FlatLODLightingEffect`.
    pub lighting_effect: f32,
    /// Field 7: xLODGen the template NIF (`LODGen_flat_lod.nif`); DynDOLOD four normals files
    /// joined by `;`.
    pub template_or_normals: String,
    /// Field 8: glow map (DynDOLOD) or empty.
    pub glow: String,
    /// Fields 9-10: xLODGen `-1, -1`; DynDOLOD vertex colour and colour variance.
    pub vertex_color: f32,
    pub color_variance: f32,
    /// DynDOLOD fields 11-14: depth (width when absent), shiftX, shiftY, height variance.
    pub depth: f32,
    pub shift_x: f32,
    pub shift_y: f32,
    pub height_variance: f32,
    pub fields: usize,
}

#[derive(Debug, Clone, Default)]
pub struct FlatList {
    pub entries: Vec<FlatTexture>,
    by_texture: HashMap<String, usize>,
}

fn f(s: Option<&&str>) -> f32 {
    s.and_then(|v| v.trim().parse().ok()).unwrap_or(0.0)
}

impl FlatList {
    /// Parses billboard lines (one per line, tab separated; blank lines skipped).
    pub fn parse(text: &str) -> FlatList {
        let mut l = FlatList::default();
        for raw in text.lines() {
            let line = raw.trim_end_matches('\r');
            if line.trim().is_empty() {
                continue;
            }
            l.push_line(line);
        }
        l
    }

    /// Adds one line (the value of an xLODGen `FlatTextures=` key or a side-file line).
    pub fn push_line(&mut self, line: &str) {
        let c: Vec<&str> = line.split('\t').collect();
        if c.len() < 5 || c[0].trim().is_empty() {
            return;
        }
        let width = f(c.get(1));
        let mut e = FlatTexture {
            texture: c[0].trim().to_string(),
            width,
            height: f(c.get(2)),
            shift_z: f(c.get(3)),
            scale: if c.get(4).is_some_and(|s| !s.trim().is_empty()) { f(c.get(4)) } else { 1.0 },
            lighting_effect: f(c.get(5)),
            template_or_normals: c.get(6).map(|s| s.trim().to_string()).unwrap_or_default(),
            glow: c.get(7).map(|s| s.trim().to_string()).unwrap_or_default(),
            vertex_color: f(c.get(8)),
            color_variance: f(c.get(9)),
            depth: f(c.get(10)),
            shift_x: f(c.get(11)),
            shift_y: f(c.get(12)),
            height_variance: f(c.get(13)),
            fields: c.len(),
        };
        if e.depth == 0.0 {
            e.depth = width;
        }
        self.by_texture.insert(e.texture.to_ascii_lowercase().replace('/', "\\"), self.entries.len());
        self.entries.push(e);
    }

    pub fn get(&self, texture: &str) -> Option<&FlatTexture> {
        self.by_texture.get(&texture.to_ascii_lowercase().replace('/', "\\")).map(|&i| &self.entries[i])
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_layouts() {
        let mut l = FlatList::parse("textures\\terrain\\lodgen\\skyrim.esm\\treepineforest01_0004f08b.dds\t512\t1024\t-3.5\t1\t0.0\ta;b;c;d\ttextures\\white.dds\t0.55\t0.1\t300\t1\t2\t0.0\t\r\n");
        l.push_line("textures\\x.dds\t10\t20\t0\t1\t1\tLODGen_flat_lod.nif\t\t-1\t-1");
        assert_eq!(l.len(), 2);
        let d = l.get("Textures\\Terrain\\LODGen\\Skyrim.esm\\TreePineForest01_0004F08B.dds").unwrap();
        assert_eq!((d.width, d.height, d.shift_z, d.scale), (512.0, 1024.0, -3.5, 1.0));
        assert_eq!((d.vertex_color, d.color_variance, d.depth, d.shift_x, d.shift_y), (0.55, 0.1, 300.0, 1.0, 2.0));
        assert_eq!(d.template_or_normals, "a;b;c;d");
        assert_eq!(d.fields, 15);
        let x = l.get("textures\\x.dds").unwrap();
        assert_eq!((x.width, x.depth, x.template_or_normals.as_str(), x.vertex_color), (10.0, 10.0, "LODGen_flat_lod.nif", -1.0));
    }
}
