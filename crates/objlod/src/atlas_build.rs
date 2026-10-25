//! Object LOD texture atlas layout, ported from xEdit's `wbGetUVRangeTexturesList`,
//! `TwbBinPacker` and `wbBuildAtlas` (`Core\wbLOD.pas`, MPL): which LOD textures go into the
//! atlas, where each lands, and the atlas map LODGen reads. Pixels are composed elsewhere.

use std::collections::BTreeMap;

use lodforge_nif::{Block, LightingShader, Nif, TriShape};

/// Meshes xEdit treats as untiled despite large UVs (matched case-sensitively, as xEdit's `Pos`).
const FORCE_UNTILED: &str = "meshes\\lod\\whiterun\\wrplainsdistrictterrainlod_lod.nif,meshes\\lod\\whiterun\\wrplainsdistrictterrainlod_lod_2.nif";

/// `TwbAsset.GetAssetName(name, '', atTexture)`.
pub fn texture_asset_name(name: &str) -> String {
    if name.trim().len() < 2 {
        return String::new();
    }
    let path = format!("\\{}", name.to_lowercase().replace('/', "\\"));
    if let Some(i) = path.find("\\data\\") {
        return name[i + 5..].to_string();
    }
    if let Some(i) = path.find("\\textures\\") {
        return name[i..].to_string();
    }
    format!("textures\\{}", name.trim_start_matches(['\\', '/']))
}

/// `wbGetUVRangeTexturesList` for one mesh: the first texture of every BSTriShape with a lighting
/// shader whose UVs all stay within `uv_range` (UVs beyond +-100 are ignored as mesh errors).
pub fn untiled_textures(mesh_name: &str, nif: &Nif, uv_range: f32, out: &mut Vec<String>) {
    untiled_texture_sets(mesh_name, nif, uv_range, |_, t| {
        if let Some(first) = t.first() {
            let name = texture_asset_name(first);
            if !name.is_empty() {
                out.push(name);
            }
        }
    });
}

/// The shapes `untiled_textures` looks at: `f(shader, texture set)` for every BSTriShape with a
/// lighting shader and a texture set whose UVs all stay within `uv_range`.
pub fn untiled_texture_sets(mesh_name: &str, nif: &Nif, uv_range: f32, mut f: impl FnMut(&LightingShader, &[String])) {
    for b in &nif.blocks {
        let Block::TriShape(s) = b else { continue };
        let Some(Block::LightingShader(l)) = nif.get(s.shader_property) else { continue };
        if !shape_untiled(mesh_name, s, uv_range) {
            continue;
        }
        if let Some(Block::TextureSet(t)) = nif.get(l.texture_set) {
            f(l, t);
        }
    }
}

/// `wbGetUVRangeTexturesList`'s test for one shape: it has UVs and all of them stay within
/// `uv_range` (UVs beyond +-100 are ignored as mesh errors; a few meshes always count as untiled).
pub fn shape_untiled(mesh_name: &str, s: &TriShape, uv_range: f32) -> bool {
    if s.uvs.is_empty() {
        return false;
    }
    let mut tiled = s.positions.is_empty();
    if !FORCE_UNTILED.contains(mesh_name) {
        for uv in &s.uvs {
            let (u, v) = (uv[0], uv[1]);
            tiled = !(u.is_nan() || v.is_nan()) && u > -100.0 && u < 100.0 && v > -100.0 && v < 100.0 && (u < -uv_range || u > uv_range || v < -uv_range || v > uv_range);
            if tiled {
                break;
            }
        }
    }
    !tiled
}

/// Sorted, de-duplicated texture list as xEdit's sorted `TStringList` (case-insensitive).
pub fn sorted_unique(mut list: Vec<String>) -> Vec<String> {
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    for t in list.drain(..) {
        seen.entry(collation_key(&t)).or_insert(t);
    }
    seen.into_values().collect()
}

/// Sort key for xEdit's case-insensitive `TStringList` order. Plain lower-case ordinal order; it
/// reproduces the golden atlas maps' line order (Delphi uses `AnsiCompareText`, which may order
/// some symbols differently).
pub fn collation_key(s: &str) -> String {
    s.to_lowercase()
}

/// One block for the packer.
#[derive(Debug, Clone, Copy)]
struct BinBlock {
    index: usize,
    w: u32,
    h: u32,
    x: u32,
    y: u32,
}

#[derive(Default)]
struct Node {
    used: bool,
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    right: Option<Box<Node>>,
    down: Option<Box<Node>>,
}

/// `TwbBinPacker.FindNode`: depth-first, right before down; returns the path to the free node.
fn find_path(n: &Node, w: u32, h: u32, path: &mut Vec<bool>) -> bool {
    if n.used {
        for (down, child) in [(false, &n.right), (true, &n.down)] {
            if let Some(c) = child.as_deref() {
                path.push(down);
                if find_path(c, w, h, path) {
                    return true;
                }
                path.pop();
            }
        }
        false
    } else {
        w <= n.w && h <= n.h
    }
}

fn node_at<'a>(mut n: &'a mut Node, path: &[bool]) -> &'a mut Node {
    for &down in path {
        n = if down { n.down.as_deref_mut().unwrap() } else { n.right.as_deref_mut().unwrap() };
    }
    n
}
/// `TwbBinPacker.Fit` (no padding): stable bubble sort by height, then width, descending (xLODGen
/// 4.1.5o; the public xEdit source sorts by the larger side), then each block goes into the first
/// free node (depth-first, right before down), which is split into a right and a down node.
fn fit(blocks: &mut [BinBlock], width: u32, height: u32) -> bool {
    // Tallest first, then widest; stable (the same order the former bubble sort gave).
    blocks.sort_by(|a, b| (b.h, b.w).cmp(&(a.h, a.w)));
    let mut root = Node { w: width, h: height, ..Default::default() };
    let mut path = Vec::new();
    for b in blocks.iter_mut() {
        path.clear();
        if find_path(&root, b.w, b.h, &mut path) {
            let n = node_at(&mut root, &path);
            n.used = true;
            n.down = Some(Box::new(Node { x: n.x, y: n.y + b.h, w: n.w, h: n.h - b.h, ..Default::default() }));
            n.right = Some(Box::new(Node { x: n.x + b.w, y: n.y, w: n.w - b.w, h: b.h, ..Default::default() }));
            b.x = n.x;
            b.y = n.y;
        } else {
            // The caller drops blocks and retries; the positions of a failed fit are not used.
            return false;
        }
    }
    true
}

/// One atlas texture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtlasPage {
    /// Game path, e.g. `textures\terrain\Tamriel\Objects\TamrielObjectsLOD.dds`.
    pub name: String,
    pub width: u32,
    pub height: u32,
}

/// Where one source texture lands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtlasEntry {
    pub texture: String,
    pub width: u32,
    pub height: u32,
    pub x: u32,
    pub y: u32,
    pub page: usize,
}

#[derive(Debug, Clone, Default)]
pub struct AtlasLayout {
    pub pages: Vec<AtlasPage>,
    /// In texture-list order (the map's line order).
    pub entries: Vec<AtlasEntry>,
    /// Ready-made lines that point a source into a mini atlas (DynDOLOD's 10-column re-UV rows,
    /// `source  w  h  x  y  miniatlas  W  H  scaleU  scaleV`), with their source.
    pub mini_rows: Vec<(String, String)>,
    /// Per diffuse (lower-case key): RGB factors applied to its tile when the page is composed
    /// (far-LOD bake matching, see the CLI's `object_atlas_plan`).
    pub tones: std::collections::BTreeMap<String, [f32; 3]>,
}

impl AtlasLayout {
    /// The atlas map text LODGen reads (`source  w  h  x  y  atlas  atlasW  atlasH`); with mini
    /// rows, all lines sorted by source (as DynDOLOD's map).
    pub fn map_text(&self) -> String {
        let mut lines: Vec<(String, String)> = self
            .entries
            .iter()
            .map(|e| {
                let p = &self.pages[e.page];
                (collation_key(&e.texture), format!("{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}", e.texture, e.width, e.height, e.x, e.y, p.name, p.width, p.height))
            })
            .collect();
        if !self.mini_rows.is_empty() {
            lines.extend(self.mini_rows.iter().map(|(k, l)| (collation_key(k), l.clone())));
            lines.sort_by(|a, b| a.0.cmp(&b.0));
        }
        lines.into_iter().map(|(_, l)| l + "\r\n").collect()
    }
}

/// `wbBuildAtlas` layout: textures (in list order, with their sizes) packed onto `width` x
/// `height` pages. Blocks that do not fit are taken off the end (at least two per page) and go to
/// the next page. Pages are cropped to the power of two covering their tiles. The first page is
/// `<base>.dds`, further ones `<base>01.dds`, `<base>02.dds`, ... (xLODGen 4.1.5o naming).
pub fn layout(textures: &[(String, u32, u32)], width: u32, height: u32, base: &str) -> AtlasLayout {
    let mut out = AtlasLayout { pages: Vec::new(), mini_rows: Vec::new(), tones: Default::default(), entries: textures.iter().map(|(t, w, h)| AtlasEntry { texture: t.clone(), width: *w, height: *h, x: 0, y: 0, page: usize::MAX }).collect() };
    let mut blocks: Vec<BinBlock> = textures.iter().enumerate().map(|(i, (_, w, h))| BinBlock { index: i, w: *w, h: *h, x: 0, y: 0 }).collect();
    let mut num = 0usize;
    while !blocks.is_empty() {
        let mut rest: Vec<BinBlock> = Vec::new();
        // Blocks whose total area exceeds the page never fit: drop them (smallest first, the
        // order the retry loop below drops them in) without trying, so a page costs a few fits
        // instead of one per dropped block.
        blocks.sort_by(|a, b| (b.h, b.w).cmp(&(a.h, a.w)));
        let page_area = u64::from(width) * u64::from(height);
        let mut area: u64 = blocks.iter().map(|b| u64::from(b.w) * u64::from(b.h)).sum();
        while blocks.len() > 2 && area > page_area {
            let b = blocks.pop().unwrap();
            area -= u64::from(b.w) * u64::from(b.h);
            rest.push(b);
        }
        loop {
            if fit(&mut blocks, width, height) {
                break;
            }
            if blocks.len() > 2 {
                rest.push(blocks.pop().unwrap());
            } else {
                return out;
            }
        }
        let name = if num == 0 { format!("{base}.dds") } else { format!("{base}{num:02}.dds") };
        let (mut maxw, mut maxh) = (0, 0);
        for b in &blocks {
            maxw = maxw.max(b.x + b.w);
            maxh = maxh.max(b.y + b.h);
        }
        let pow2 = |v: u32| {
            let mut i = 1;
            while i < v {
                i *= 2;
            }
            i
        };
        let page = out.pages.len();
        out.pages.push(AtlasPage { name, width: pow2(maxw), height: pow2(maxh) });
        for b in &blocks {
            let e = &mut out.entries[b.index];
            e.x = b.x;
            e.y = b.y;
            e.page = page;
        }
        blocks = rest;
        num += 1;
    }
    out.entries.retain(|e| e.page != usize::MAX);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packs_and_spills() {
        let t: Vec<(String, u32, u32)> = vec![("a".into(), 1024, 1024), ("b".into(), 1024, 1024), ("c".into(), 512, 512), ("d".into(), 512, 512), ("e".into(), 1024, 1024), ("f".into(), 1024, 1024)];
        let l = layout(&t, 2048, 2048, "x");
        assert_eq!(l.pages[0], AtlasPage { name: "x.dds".into(), width: 2048, height: 2048 });
        assert_eq!(l.pages.len(), 2);
        assert_eq!(l.pages[1].name, "x01.dds");
        let a = l.entries.iter().find(|e| e.texture == "a").unwrap();
        assert_eq!((a.x, a.y, a.page), (0, 0, 0));
        assert_eq!(texture_asset_name("Data\\Textures\\LOD\\x.dds"), "Textures\\LOD\\x.dds");
        assert_eq!(texture_asset_name("lod\\x.dds"), "textures\\lod\\x.dds");
    }
}

#[cfg(test)]
mod layout_equivalence {
    use super::*;

    /// The layout before the area pre-check, sort and early exit (2026-10-24), for comparison.
    fn old_fit(blocks: &mut [BinBlock], width: u32, height: u32) -> bool {
        let mut changed = true;
        while changed {
            changed = false;
            for i in 0..blocks.len().saturating_sub(1) {
                if (blocks[i].h, blocks[i].w) < (blocks[i + 1].h, blocks[i + 1].w) {
                    blocks.swap(i, i + 1);
                    changed = true;
                }
            }
        }
        let mut root = Node { w: width, h: height, ..Default::default() };
        let mut ok = true;
        let mut path = Vec::new();
        for b in blocks.iter_mut() {
            path.clear();
            if find_path(&root, b.w, b.h, &mut path) {
                let n = node_at(&mut root, &path);
                n.used = true;
                n.down = Some(Box::new(Node { x: n.x, y: n.y + b.h, w: n.w, h: n.h - b.h, ..Default::default() }));
                n.right = Some(Box::new(Node { x: n.x + b.w, y: n.y, w: n.w - b.w, h: b.h, ..Default::default() }));
                b.x = n.x;
                b.y = n.y;
            } else {
                ok = false;
            }
        }
        ok
    }

    fn old_layout(textures: &[(String, u32, u32)], width: u32, height: u32) -> Vec<(usize, u32, u32)> {
        let mut out = vec![(usize::MAX, 0, 0); textures.len()];
        let mut blocks: Vec<BinBlock> = textures.iter().enumerate().map(|(i, (_, w, h))| BinBlock { index: i, w: *w, h: *h, x: 0, y: 0 }).collect();
        let mut page = 0;
        while !blocks.is_empty() {
            let mut rest = Vec::new();
            loop {
                if old_fit(&mut blocks, width, height) {
                    break;
                }
                if blocks.len() > 2 {
                    rest.push(blocks.pop().unwrap());
                } else {
                    return out;
                }
            }
            for b in &blocks {
                out[b.index] = (page, b.x, b.y);
            }
            blocks = rest;
            page += 1;
        }
        out
    }

    #[test]
    fn same_layout_as_before() {
        let mut s = 0x1234_5678u64;
        let mut rnd = |n: u32| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            (s % u64::from(n)) as u32
        };
        for case in 0..40 {
            let n = 5 + rnd(300) as usize;
            let t: Vec<(String, u32, u32)> = (0..n).map(|i| (format!("t{i}"), 16 << rnd(6), 16 << rnd(6))).collect();
            let page = if case % 2 == 0 { 1024 } else { 2048 };
            let new = layout(&t, page, page, "x");
            let old = old_layout(&t, page, page);
            for (i, e) in new.entries.iter().enumerate() {
                let k = t.iter().position(|x| x.0 == e.texture).unwrap();
                assert_eq!((e.page, e.x, e.y), old[k], "case {case} entry {i}");
            }
            assert_eq!(new.entries.len(), old.iter().filter(|o| o.0 != usize::MAX).count());
        }
    }
}
