//! The atlas map (docs/re/object-lod-spec.md 1.2): tab-separated rows, no header, written by
//! xEdit's `wbBuildAtlas` (our port [`crate::atlas_build::AtlasLayout::map_text`]) or DynDOLOD.
//!
//! Eight columns: `key, tile w, tile h, tile x, tile y, atlas path, atlas w, atlas h`; DynDOLOD
//! adds `scale U, scale V` (ten columns) for TexGen mini-atlas rows. The key is one diffuse path
//! (xLODGen) or `diffuse[,normal[,glow]]` (DynDOLOD), lower case.
//!
//! The remap of a shape is a chain of rows ([`AtlasMap::resolve`]): its texture's row, which
//! may be a TexGen mini atlas (ten columns, scale 1/n for a texture repeated n x n in the mini
//! page, so UVs up to n fit), then the row of that mini page itself onto the big page, as the
//! golden DynDOLOD map has it (`textures\pbr\landscape\dirtcliffs\dirtcliffs01.dds ->
//! textures\lod\dirtcliffs01pbr_lod.dds` at scale 0.5, then `textures\lod\dirtcliffs01pbr_lod.dds,
//! ..._n.dds -> DynDOLOD_Tamriel.dds`).

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub struct AtlasRow {
    /// Lower-case key as written (`diffuse` or `diffuse,normal[,glow]`).
    pub key: String,
    pub width: u32,
    pub height: u32,
    /// Tile position on the page; a TexGen re-uv row may place its source region partly off
    /// the page (negative offsets), so these are signed.
    pub x: i32,
    pub y: i32,
    /// Atlas / target texture path as written.
    pub page: String,
    pub page_width: u32,
    pub page_height: u32,
    /// Ten-column rows: UV scale before the remap.
    pub scale: Option<(f32, f32)>,
}

impl AtlasRow {
    /// The diffuse part of the key.
    pub fn diffuse(&self) -> &str {
        self.key.split(',').next().unwrap_or("")
    }

    /// Remapped UV (spec 5): `u' = (x + u * w) / W`, `v' = (y + v * h) / H`, after the row's scale.
    pub fn remap(&self, uv: [f32; 2]) -> [f32; 2] {
        let (su, sv) = self.scale.unwrap_or((1.0, 1.0));
        [(self.x as f32 + uv[0] * su * self.width as f32) / self.page_width as f32, (self.y as f32 + uv[1] * sv * self.height as f32) / self.page_height as f32]
    }
}

/// A chain of rows a shape's UVs go through (see [`AtlasMap::resolve`]).
#[derive(Debug, Clone)]
pub struct Remap<'a> {
    pub rows: Vec<&'a AtlasRow>,
}

impl<'a> Remap<'a> {
    /// The final page's row (its page and key decide the written textures).
    pub fn last(&self) -> &'a AtlasRow {
        self.rows.last().unwrap()
    }

    pub fn apply(&self, mut uv: [f32; 2]) -> [f32; 2] {
        for r in &self.rows {
            uv = r.remap(uv);
        }
        uv
    }
}

#[derive(Debug, Clone, Default)]
pub struct AtlasMap {
    pub rows: Vec<AtlasRow>,
    by_key: HashMap<String, usize>,
    /// Repeat tiles (crate::tiling) by their base key: (repeats, row).
    tiled: HashMap<String, Vec<((u32, u32), usize)>>,
}

impl AtlasMap {
    /// Parses the map text; blank lines are skipped, a row with fewer than eight columns or a
    /// non-numeric size is an error.
    pub fn parse(text: &str) -> Result<AtlasMap, String> {
        let mut m = AtlasMap::default();
        for (n, raw) in text.lines().enumerate() {
            let line = raw.trim_end_matches('\r');
            if line.trim().is_empty() {
                continue;
            }
            let c: Vec<&str> = line.split('\t').map(str::trim).collect();
            if c.len() < 8 {
                return Err(format!("atlas map line {}: {} columns, expected 8 or 10", n + 1, c.len()));
            }
            let u = |i: usize| c[i].parse::<u32>().map_err(|_| format!("atlas map line {}: column {} is not a number: {:?}", n + 1, i + 1, c[i]));
            let s = |i: usize| c[i].parse::<i32>().map_err(|_| format!("atlas map line {}: column {} is not a number: {:?}", n + 1, i + 1, c[i]));
            let scale = if c.len() >= 10 {
                Some((c[8].parse().unwrap_or(1.0), c[9].parse().unwrap_or(1.0)))
            } else {
                // A repeat tile (`key*<ru>x<rv>`, crate::tiling) holds the texture ru x rv times.
                crate::tiling::split_key(c[0]).1.map(|(ru, rv)| (1.0 / ru as f32, 1.0 / rv as f32))
            };
            let row = AtlasRow { key: c[0].to_ascii_lowercase(), width: u(1)?, height: u(2)?, x: s(3)?, y: s(4)?, page: c[5].to_string(), page_width: u(6)?, page_height: u(7)?, scale };
            if let (base, Some(r)) = crate::tiling::split_key(&row.key) {
                m.tiled.entry(base.to_string()).or_default().push((r, m.rows.len()));
            }
            m.by_key.entry(row.key.clone()).or_insert(m.rows.len());
            m.rows.push(row);
        }
        Ok(m)
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// The row of an exact key (lower-cased).
    pub fn get(&self, key: &str) -> Option<&AtlasRow> {
        self.by_key.get(&key.to_ascii_lowercase()).map(|&i| &self.rows[i])
    }

    /// The row for a texture set: `diffuse,normal,glow`, then `diffuse,normal`, then `diffuse`
    /// (the DynDOLOD keys), the paths in lower case as `textures\...`; with `alpha` (an
    /// alpha-tested shape's threshold / 16 when it is not 8) each key is tried with the `=N`
    /// suffix first.
    pub fn lookup(&self, diffuse: &str, normal: &str, glow: &str, alpha: Option<u8>) -> Option<&AtlasRow> {
        let k = crate::export::texture_key;
        // A PBR diffuse named `<stem>linear.dds` (AltTextures side file) is keyed as the
        // `<stem>.dds` it was made from.
        let strip_linear = |t: String| -> String {
            match t.strip_suffix("linear.dds") {
                Some(stem) => format!("{stem}.dds"),
                None => t,
            }
        };
        let (d, n, g) = (strip_linear(k(diffuse)), k(normal), k(glow));
        if d.is_empty() {
            return None;
        }
        // An empty normal slot stands for the default `<diffuse>_n.dds`.
        let n = if n.is_empty() { format!("{}_n.dds", d.trim_end_matches(".dds")) } else { n };
        let mut keys = Vec::with_capacity(3);
        if !g.is_empty() {
            keys.push(format!("{d},{n},{g}"));
        }
        keys.push(format!("{d},{n}"));
        keys.push(d);
        for key in &keys {
            if let Some(a) = alpha
                && let Some(r) = self.get(&format!("{key}={a}"))
            {
                return Some(r);
            }
        }
        keys.iter().find_map(|key| self.get(key))
    }

    /// The remap chain of a shape with the given texture set and UVs: `None` when the texture has
    /// no row or the UVs (times the first row's scale) leave `[-tolerance, 1 + tolerance]`.
    /// The repeat tile (`crate::tiling`) of a diffuse / normal / alpha that holds at least
    /// `ru x rv` repeats (the smallest such tile when there are several).
    pub fn lookup_tiled(&self, diffuse: &str, normal: &str, alpha: Option<u8>, repeats: (u32, u32)) -> Option<&AtlasRow> {
        let k = crate::export::texture_key;
        let d = k(diffuse);
        let d = match d.strip_suffix("linear.dds") {
            Some(stem) => format!("{stem}.dds"),
            None => d,
        };
        if d.is_empty() {
            return None;
        }
        let n = match k(normal) {
            n if n.is_empty() => format!("{}_n.dds", d.trim_end_matches(".dds")),
            n => n,
        };
        let base = match alpha {
            Some(a) => format!("{d},{n}={a}"),
            None => format!("{d},{n}"),
        };
        let (ru, rv) = repeats;
        self.tiled.get(&base.to_ascii_lowercase())?.iter().filter(|((u, v), _)| *u >= ru && *v >= rv).min_by_key(|((u, v), _)| u * v).map(|(_, i)| &self.rows[*i])
    }

    pub fn resolve(&self, diffuse: &str, normal: &str, glow: &str, alpha: Option<u8>, uvs: &[[f32; 2]], tolerance: f32) -> Option<Remap<'_>> {
        let first = self.lookup(diffuse, normal, glow, alpha)?;
        let (su, sv) = first.scale.unwrap_or((1.0, 1.0));
        let fits = |v: f32| (-tolerance..=1.0 + tolerance).contains(&v);
        if !uvs.iter().all(|uv| fits(uv[0] * su) && fits(uv[1] * sv)) {
            return None;
        }
        let mut rows = vec![first];
        let mut page = first.page.clone();
        for _ in 0..3 {
            let k = crate::export::texture_key(&page);
            let stem = k.trim_end_matches(".dds").to_string();
            let next = self.get(&format!("{k},{stem}_n.dds")).or_else(|| self.get(&k));
            match next {
                Some(n) if !std::ptr::eq(n, *rows.last().unwrap()) && n.page.to_ascii_lowercase() != page.to_ascii_lowercase() => {
                    page = n.page.clone();
                    rows.push(n);
                }
                _ => break,
            }
        }
        Some(Remap { rows })
    }

    /// The `=N` suffix of an alpha-tested shape's key: threshold / 16 when the threshold is not
    /// 128, else none (spec 1.2 / 7.2; golden rows `...=4`, `...=2`).
    pub fn alpha_suffix(alpha: Option<(u16, u8)>) -> Option<u8> {
        match alpha {
            Some((flags, threshold)) if flags & 0x200 != 0 && threshold != 128 => Some(threshold / 16),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn both_row_shapes() {
        let t = "textures\\dungeons\\imperial\\impextfloor01.dds\t1024\t1024\t0\t0\ttextures\\terrain\\Tamriel\\Objects\\TamrielObjectsLOD.dds\t4096\t4096\r\n\
                 textures\\architecture\\farmhouse\\farmhouse01.dds\t128\t128\t128\t128\ttextures\\lod\\farmhouselod01.dds\t256\t512\t1\t1\r\n\
                 textures\\a.dds,textures\\a_n.dds,textures\\a_g.dds\t256\t256\t512\t0\ttextures\\DynDOLOD\\LOD\\DynDOLOD_Tamriel_Glow.dds\t1024\t512\r\n\r\n";
        let m = AtlasMap::parse(t).unwrap();
        assert_eq!(m.len(), 3);
        let r = m.get("textures\\dungeons\\imperial\\IMPEXTFLOOR01.dds").unwrap();
        assert_eq!((r.width, r.page_width, r.scale), (1024, 4096, None));
        assert_eq!(r.remap([0.5, 1.0]), [0.125, 0.25]);
        let f = m.lookup("Data\\textures\\architecture\\farmhouse\\farmhouse01.dds", "", "", None).unwrap();
        assert_eq!((f.x, f.y, f.scale), (128, 128, Some((1.0, 1.0))));
        assert_eq!(f.remap([1.0, 0.0]), [1.0, 0.25]);
        let g = m.lookup("textures\\a.dds", "textures\\a_n.dds", "textures\\a_g.dds", None).unwrap();
        assert_eq!(g.diffuse(), "textures\\a.dds");
        assert!(m.lookup("textures\\a.dds", "textures\\a_n.dds", "", None).is_none());
        assert!(AtlasMap::parse("a\tb\n").is_err());
        let m = AtlasMap::parse("textures\\t.dds,textures\\t_n.dds=4\t512\t512\t512\t0\ttextures\\x_Alpha.dds\t4096\t2048\r\ntextures\\t.dds\t512\t512\t0\t0\ttextures\\x.dds\t4096\t2048\r\n").unwrap();
        assert_eq!(m.lookup("textures\\t.dds", "textures\\t_n.dds", "", Some(4)).unwrap().page, "textures\\x_Alpha.dds");
        assert_eq!(m.lookup("textures\\t.dds", "textures\\t_n.dds", "", None).unwrap().page, "textures\\x.dds");
        assert_eq!(AtlasMap::alpha_suffix(Some((0x1200 | 0x200, 64))), Some(4));
        assert_eq!(AtlasMap::alpha_suffix(Some((0x200, 128))), None);
        assert_eq!(AtlasMap::alpha_suffix(Some((0x1, 64))), None);
        // A scaled mini row (2 x 2 repeat) chained onto the big page.
        let m = AtlasMap::parse("textures\\pbr\\landscape\\d.dds\t256\t256\t0\t0\ttextures\\lod\\dpbr_lod.dds\t256\t256\t0.5\t0.5\r\ntextures\\lod\\dpbr_lod.dds,textures\\lod\\dpbr_lod_n.dds\t256\t256\t3328\t2048\ttextures\\DynDOLOD\\LOD\\DynDOLOD_Tamriel.dds\t8192\t4096\r\n").unwrap();
        let r = m.resolve("textures\\pbr\\landscape\\dlinear.dds", "textures\\pbr\\landscape\\d_n.dds", "", None, &[[0.0, 0.0], [1.9, 1.9]], 0.2).unwrap();
        assert_eq!(r.rows.len(), 2);
        assert_eq!(r.last().page, "textures\\DynDOLOD\\LOD\\DynDOLOD_Tamriel.dds");
        let uv = r.apply([1.9, 1.9]);
        assert!((uv[0] - (3328.0 + 0.95 * 256.0) / 8192.0).abs() < 1e-6 && (uv[1] - (2048.0 + 0.95 * 256.0) / 4096.0).abs() < 1e-6);
        assert!(m.resolve("textures\\pbr\\landscape\\d.dds", "", "", None, &[[0.0, 0.0], [2.5, 0.0]], 0.2).is_none());
    }

    #[test]
    fn repeat_tile_rows() {
        // A tile holding the texture 4 x 2 times: UV (4, 2) is the tile's far corner.
        let m = AtlasMap::parse("textures\\w.dds,textures\\w_n.dds*4x2\t1024\t512\t1024\t0\ttextures\\x.dds\t4096\t2048\r\ntextures\\w.dds,textures\\w_n.dds\t256\t256\t0\t0\ttextures\\x.dds\t4096\t2048\r\n").unwrap();
        let t = m.lookup_tiled("textures\\W.dds", "textures\\w_n.dds", None, (4, 2)).unwrap();
        assert_eq!(t.scale, Some((0.25, 0.5)));
        assert_eq!(t.remap([4.0, 2.0]), [2048.0 / 4096.0, 512.0 / 2048.0]);
        assert_eq!(t.remap([1.0, 1.0]), [(1024.0 + 256.0) / 4096.0, 256.0 / 2048.0]);
        // Fewer repeats use the tile that holds more; more than it holds, none.
        assert_eq!(m.lookup_tiled("textures\\w.dds", "", None, (2, 1)).unwrap().key, t.key);
        assert!(m.lookup_tiled("textures\\w.dds", "", None, (2, 3)).is_none());
        // The ordinary row is untouched.
        assert_eq!(m.lookup("textures\\w.dds", "textures\\w_n.dds", "", None).unwrap().scale, None);
    }
}
