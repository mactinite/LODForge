//! TexGen's stitched object-LOD textures (docs/re/dyndolod-spec.md §3.1/§3.2): mini atlases
//! composited from full textures by the `DynDOLOD_SSE_TexGen_{alpha,noalpha}_<plugin>.txt` rules,
//! with `_n` twins, PBR variants and the re-uv-miniatlas rows; `_copy_` rules are name aliases.
//!
//! TexGen draws one quad per rule into an FBO with the `stitched` shader: a 6x6 Lanczos-3 filter
//! around the texel centre whose taps are sampled with the hardware's mip selection (`texture2D`
//! with a `bias`). At the reduction TexGen uses (full texture -> 64..512 px tile) the taps land a
//! source texel apart on a mip several levels down, so the result is the trilinear sample of the
//! source's own mip chain at the tile's LOD; that is what we compute, on the CPU. The bias that fits the
//! golden best is -1.25 (34.6 dB mean over 284 diffuse atlases; 0 gives 32.8, -2 gives 32.2; a
//! full emulation of the 36-tap filter and a Lanczos-3 resample of mip 0 both score lower).

use std::collections::HashMap;

/// One `_alpha_` / `_noalpha_` line.
#[derive(Debug, Clone, PartialEq)]
pub enum Rule {
    /// A source drawn into a tile of a mini atlas (all sizes are multipliers of the base size).
    Tile { brightness: i32, rgb: [f32; 3], src: String, tile: [f32; 2], pos: [f32; 2], dest: String, atlas: [f32; 2], alpha: bool },
    /// `x` rule: a full texture rewritten at its own size (alpha stripped / mips rebuilt).
    Full { brightness: i32, rgb: [f32; 3], src: String, mode: i32, param: i32, dest: String, level: i32, alpha: bool },
}

impl Rule {
    pub fn dest(&self) -> &str {
        match self {
            Rule::Tile { dest, .. } | Rule::Full { dest, .. } => dest,
        }
    }
    pub fn src(&self) -> &str {
        match self {
            Rule::Tile { src, .. } | Rule::Full { src, .. } => src,
        }
    }
    pub fn keeps_alpha(&self) -> bool {
        match self {
            Rule::Tile { alpha, .. } | Rule::Full { alpha, .. } => *alpha,
        }
    }
}

/// Lower-case, backslashes, no leading `data\`.
pub fn norm(p: &str) -> String {
    let p = p.trim().replace('/', "\\").to_ascii_lowercase();
    p.strip_prefix("data\\").map(str::to_string).unwrap_or(p)
}

/// Parses an `_alpha_` (`alpha` = true) or `_noalpha_` rule file. Malformed lines are skipped.
pub fn parse_rules(text: &str, alpha: bool) -> Vec<Rule> {
    let mut out = Vec::new();
    for l in text.lines() {
        let l = l.trim();
        if l.is_empty() || l.starts_with("//") {
            continue;
        }
        let c: Vec<&str> = l.split('\t').map(str::trim).collect();
        if c.len() < 12 {
            continue;
        }
        let f = |i: usize| c[i].parse::<f32>().ok();
        let (Some(b), Some(r), Some(g), Some(bl)) = (c[0].parse::<f32>().ok(), f(1), f(2), f(3)) else { continue };
        let brightness = b as i32;
        if c[5].eq_ignore_ascii_case("x") {
            let (Ok(mode), Ok(param), Ok(level)) = (c[6].parse(), c[7].parse(), c[10].parse()) else { continue };
            out.push(Rule::Full { brightness, rgb: [r, g, bl], src: norm(c[4]), mode, param, dest: norm(c[9]), level, alpha });
        } else {
            let (Some(tw), Some(th), Some(x), Some(y), Some(aw), Some(ah)) = (f(5), f(6), f(7), f(8), f(10), f(11)) else { continue };
            out.push(Rule::Tile { brightness, rgb: [r, g, bl], src: norm(c[4]), tile: [tw, th], pos: [x, y], dest: norm(c[9]), atlas: [aw, ah], alpha });
        }
    }
    out
}

/// Parses a `_copy_` file: `source,destination` pairs (TexGen 3 aliases `destination` to `source`).
pub fn parse_copy(text: &str) -> Vec<(String, String)> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("//"))
        .filter_map(|l| l.split_once(',').map(|(a, b)| (norm(a), norm(b))))
        .filter(|(a, b)| !a.is_empty() && !b.is_empty())
        .collect()
}

/// `textures\x\y.dds` -> `textures\pbr\x\y.dds` (other prefixes kept, `DynDOLOD-temp\` too).
pub fn pbr_path(p: &str) -> String {
    let p = norm(p);
    if let Some(i) = lodforge_nif::data_folder_start(&p, "textures") {
        let (head, rest) = p.split_at(i + "textures\\".len());
        if rest.starts_with("pbr\\") {
            return p;
        }
        return format!("{head}pbr\\{rest}");
    }
    p
}

/// TexGen's name of the PBR twin of a stitched destination: a file name ending in `lod` or
/// `lod<digits>` gets that `lod` replaced by `pbr_lod` (`slod04` -> `spbr_lod04`); any other
/// destination moves under `textures\pbr\`.
pub fn pbr_dest(dest: &str) -> String {
    let d = norm(dest);
    let (dir, file) = d.rsplit_once('\\').unwrap_or(("", &d));
    let stem = file.strip_suffix(".dds").unwrap_or(file);
    let digits = stem.len() - stem.trim_end_matches(|c: char| c.is_ascii_digit()).len();
    let head = &stem[..stem.len() - digits];
    if head.ends_with("lod") && !head.ends_with("pbr_lod") {
        let name = format!("{}pbr_lod{}.dds", &head[..head.len() - 3], &stem[stem.len() - digits..]);
        return if dir.is_empty() { name } else { format!("{dir}\\{name}") };
    }
    pbr_path(&d)
}

/// `x.dds` -> `x_n.dds`.
pub fn normal_path(p: &str) -> String {
    let p = norm(p);
    match p.strip_suffix(".dds") {
        Some(s) => format!("{s}_n.dds"),
        None => format!("{p}_n"),
    }
}

/// One mip level, RGBA in 0..1.
#[derive(Clone)]
pub struct Level {
    pub w: u32,
    pub h: u32,
    pub px: Vec<[f32; 4]>,
}

impl Level {
    pub fn from_rgba8(w: u32, h: u32, rgba: &[u8]) -> Level {
        Level { w, h, px: rgba.chunks_exact(4).map(|p| [p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0, p[3] as f32 / 255.0]).collect() }
    }
    pub fn to_rgba8(&self) -> Vec<u8> {
        self.px.iter().flat_map(|p| p.map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8)).collect()
    }
    /// 2x2 box reduction (odd sizes clamp).
    pub fn half(&self) -> Level {
        let (w, h) = ((self.w / 2).max(1), (self.h / 2).max(1));
        let mut px = Vec::with_capacity((w * h) as usize);
        for y in 0..h {
            for x in 0..w {
                let mut s = [0.0f32; 4];
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let sx = (x * 2 + dx).min(self.w - 1);
                    let sy = (y * 2 + dy).min(self.h - 1);
                    let p = self.px[(sy * self.w + sx) as usize];
                    for k in 0..4 {
                        s[k] += p[k] * 0.25;
                    }
                }
                px.push(s);
            }
        }
        Level { w, h, px }
    }
    fn texel(&self, x: i64, y: i64) -> [f32; 4] {
        let xi = x.rem_euclid(self.w as i64) as u32;
        let yi = y.rem_euclid(self.h as i64) as u32;
        self.px[(yi * self.w + xi) as usize]
    }
    /// Bilinear sample with GL_REPEAT wrapping.
    pub fn bilinear(&self, u: f32, v: f32) -> [f32; 4] {
        let x = u * self.w as f32 - 0.5;
        let y = v * self.h as f32 - 0.5;
        let (x0, y0) = (x.floor(), y.floor());
        let (fx, fy) = (x - x0, y - y0);
        let (x0, y0) = (x0 as i64, y0 as i64);
        let a = self.texel(x0, y0);
        let b = self.texel(x0 + 1, y0);
        let c = self.texel(x0, y0 + 1);
        let d = self.texel(x0 + 1, y0 + 1);
        let mut o = [0.0; 4];
        for k in 0..4 {
            o[k] = (a[k] * (1.0 - fx) + b[k] * fx) * (1.0 - fy) + (c[k] * (1.0 - fx) + d[k] * fx) * fy;
        }
        o
    }
}

/// A source texture's mip chain, from `first` (its full size is `w0` x `h0`).
pub struct MipChain {
    pub w0: u32,
    pub h0: u32,
    pub first: u32,
    pub levels: Vec<Level>,
}

impl MipChain {
    /// Decodes the DDS mips from `first` down (box-filtered levels are added when the file has
    /// fewer mips than a full chain).
    pub fn from_dds(bytes: &[u8], first: u32) -> Option<MipChain> {
        let info = lodforge_dds::parse(bytes).ok()?;
        let full = 32 - info.width.max(info.height).max(1).leading_zeros();
        let first = first.min(full - 1);
        let mut levels = Vec::new();
        for l in first..info.mip_count.min(full) {
            let img = lodforge_dds::decode(bytes, &info, l).ok()?;
            levels.push(Level::from_rgba8(img.width, img.height, &img.rgba));
        }
        if levels.is_empty() {
            // Fewer mips in the file than `first`: reduce the smallest one we have.
            let last = info.mip_count - 1;
            let img = lodforge_dds::decode(bytes, &info, last).ok()?;
            let mut l = Level::from_rgba8(img.width, img.height, &img.rgba);
            for _ in last..first {
                l = l.half();
            }
            levels.push(l);
        }
        while let Some(l) = levels.last() {
            if l.w <= 1 && l.h <= 1 {
                break;
            }
            let h = l.half();
            levels.push(h);
        }
        Some(MipChain { w0: info.width, h0: info.height, first, levels })
    }
    pub fn from_level(l: Level) -> MipChain {
        let (w0, h0) = (l.w, l.h);
        let mut levels = vec![l];
        while let Some(l) = levels.last() {
            if l.w <= 1 && l.h <= 1 {
                break;
            }
            let h = l.half();
            levels.push(h);
        }
        MipChain { w0, h0, first: 0, levels }
    }
    /// Trilinear sample at absolute mip `lod` (relative to the full size).
    pub fn sample(&self, u: f32, v: f32, lod: f32) -> [f32; 4] {
        let rel = (lod - self.first as f32).clamp(0.0, (self.levels.len() - 1) as f32);
        let l0 = rel.floor() as usize;
        let f = rel - l0 as f32;
        let a = self.levels[l0].bilinear(u, v);
        if f <= 1e-4 || l0 + 1 >= self.levels.len() {
            return a;
        }
        let b = self.levels[l0 + 1].bilinear(u, v);
        let mut o = [0.0; 4];
        for k in 0..4 {
            o[k] = a[k] * (1.0 - f) + b[k] * f;
        }
        o
    }
}

/// The mip TexGen's sampler picks for a `src_w` x `src_h` texture drawn into a `tw` x `th` px
/// tile (GL: log2 of the larger texel-per-pixel ratio), plus the shader's `bias`.
pub fn tile_lod(src_w: u32, src_h: u32, tw: f32, th: f32, bias: f32) -> f32 {
    let rx = src_w as f32 / tw.max(1e-3);
    let ry = src_h as f32 / th.max(1e-3);
    (rx.max(ry).max(1e-6).log2() + bias).max(0.0)
}

/// Knobs of the stitched shader we cannot read from TexGen's output directly.
#[derive(Debug, Clone, Copy)]
pub struct StitchParams {
    /// Base tile size in px (`StitchObjectsSize`).
    pub base: u32,
    /// Mip LOD bias of the sampler (fitted to the golden; see the module docs).
    pub bias: f32,
    /// `StitchPBRScale`.
    pub pbr_scale: f32,
    /// Apply each rule's brightness and gamma columns (TexGen's behaviour). Off, every tile keeps
    /// its source's tone (the PBR scale still applies), so LOD8/16 textures match the full
    /// textures and `linear` copies LOD4 shows: the rules darken e.g. mountainslab01 by
    /// brightness -4 and gamma 0.95/0.93, 14% below the LOD4 copy.
    pub rule_tone: bool,
}

impl Default for StitchParams {
    fn default() -> Self {
        StitchParams { base: 256, bias: -1.25, pbr_scale: 0.65, rule_tone: true }
    }
}

/// A rule's colour adjustment (columns 0-3): each channel `s^(1/gamma) + brightness/100`, then
/// times `scale`. Golden fits: glacierslabnoalpha (-3, 1 1 1) is the source minus ~7.7 levels;
/// mountainslab01lod (-4, 0.95 0.93 1) matches `s^(1/0.95) - 0.04` etc. per channel within one
/// level; the snow temporaries (10, 2 2 2.1) brighten farmhouselod01snow to the golden.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tone {
    pub gamma: [f32; 3],
    pub offset: f32,
    pub scale: f32,
}

impl Tone {
    pub const IDENTITY: Tone = Tone { gamma: [1.0; 3], offset: 0.0, scale: 1.0 };
    pub fn new(brightness: i32, rgb: [f32; 3]) -> Tone {
        Tone { gamma: rgb.map(|g| if g > 0.0 { g } else { 1.0 }), offset: brightness as f32 / 100.0, scale: 1.0 }
    }
    pub fn apply(&self, c: f32, k: usize) -> f32 {
        let g = if self.gamma[k] == 1.0 { c } else { c.max(0.0).powf(1.0 / self.gamma[k]) };
        ((g + self.offset).max(0.0) * self.scale).min(1.0)
    }
}

/// One tile draw: a source chain into `rect` (px, may be fractional / outside the atlas).
pub struct TileDraw<'a> {
    pub chain: &'a MipChain,
    pub rect: [f32; 4],
    pub tone: Tone,
    /// Use the source's alpha; else alpha 1.
    pub keep_alpha: bool,
    /// Divide the colour by alpha (TexGen's `unmulta`). Off for sources whose alpha is not
    /// coverage: PBR landscape textures keep a height / blend mask there, and dividing by it
    /// brightened the raised parts up to 2x and printed the mask onto the LOD texture as dark
    /// blotches (Vanaheimr's coastbeach01).
    pub unmultiply: bool,
}

/// Whether a source's alpha is a height / blend mask rather than coverage (PBR landscape).
pub fn alpha_is_height(path: &str) -> bool {
    path.to_ascii_lowercase().starts_with(r"textures\pbr\landscape\")
}

impl TileDraw<'_> {
    fn shade(&self, s: [f32; 4]) -> [f32; 4] {
        if !self.unmultiply {
            return [self.tone.apply(s[0], 0), self.tone.apply(s[1], 1), self.tone.apply(s[2], 2), if self.keep_alpha { s[3] } else { 1.0 }];
        }
        // TexGen's `unmulta`: texels at or above the alpha-test threshold (128) have their colour
        // divided by alpha, those below keep it (golden: the golden/ours ratio by source alpha is
        // 1.00 below 128, then 1.76, 1.45, 1.23, 1.07 = 255/a for snow01lod, impextrubble01lod,
        // fxdirtmound01lod). It comes before the tone curve (mountainslab01lod) and is not clamped
        // before the PBR scale (snow01pbr_lod: `s / a * 0.65` exceeds what a clamp would allow).
        let k = if s[3] * 255.0 >= 127.5 { 1.0 / s[3] } else { 1.0 };
        [self.tone.apply(s[0] * k, 0), self.tone.apply(s[1] * k, 1), self.tone.apply(s[2] * k, 2), if self.keep_alpha { s[3] } else { 1.0 }]
    }
}

/// What texels no tile covers hold in a stitched `_n`: a flat normal with alpha 0 (golden:
/// sovbasestone01lod_n, whose tiles cover a quarter of the atlas, is flat elsewhere).
pub const NORMAL_CLEAR: [f32; 4] = [0.5, 0.5, 1.0, 0.0];

/// Composites `draws` in order into a `w` x `h` image (later draws overwrite; texels no draw
/// covers stay transparent black). `normal` = the draws are normal maps (no vertex colour).
pub fn composite(w: u32, h: u32, draws: &[TileDraw], bias: f32, normal: bool) -> Level {
    let mut px = vec![if normal { NORMAL_CLEAR } else { [0.0; 4] }; (w * h) as usize];
    for d in draws {
        let [x0, y0, tw, th] = d.rect;
        let lod = tile_lod(d.chain.w0, d.chain.h0, tw, th, bias);
        // Pixel centres inside the rect (GL rasterisation).
        let i0 = (x0 - 0.5).ceil().max(0.0) as u32;
        let i1 = ((x0 + tw - 0.5).ceil().max(0.0) as u32).min(w);
        let j0 = (y0 - 0.5).ceil().max(0.0) as u32;
        let j1 = ((y0 + th - 0.5).ceil().max(0.0) as u32).min(h);
        for j in j0..j1 {
            let v = (j as f32 + 0.5 - y0) / th;
            for i in i0..i1 {
                let u = (i as f32 + 0.5 - x0) / tw;
                let s = d.chain.sample(u, v, lod);
                let o = &mut px[(j * w + i) as usize];
                if normal {
                    *o = s;
                } else {
                    *o = d.shade(s);
                }
            }
        }
    }
    Level { w, h, px }
}

/// The rules of one destination, in file order.
#[derive(Debug, Clone)]
pub struct Atlas {
    pub dest: String,
    pub rules: Vec<Rule>,
}

/// Groups rules by destination, keeping the order destinations first appear in (TexGen builds
/// `DynDOLOD-temp\` intermediates before the atlases that use them: those are listed first).
pub fn group(rules: &[Rule]) -> Vec<Atlas> {
    let mut idx: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<Atlas> = Vec::new();
    for r in rules {
        let i = *idx.entry(r.dest().to_string()).or_insert_with(|| {
            out.push(Atlas { dest: r.dest().to_string(), rules: Vec::new() });
            out.len() - 1
        });
        out[i].rules.push(r.clone());
    }
    out
}

/// Atlas size in px of a tile atlas (`atlas multiplier x base`, at least 1).
pub fn atlas_size(a: &Atlas, base: u32) -> Option<(u32, u32)> {
    a.rules.iter().find_map(|r| match r {
        Rule::Tile { atlas, .. } => Some((((atlas[0] * base as f32).round() as u32).max(1), ((atlas[1] * base as f32).round() as u32).max(1))),
        _ => None,
    })
}

/// Delphi `FloatToStr` of a Single widened to Extended: 15 significant digits, trailing zeros
/// trimmed (`0.335` -> `0.33500000834465`).
fn float_to_str(v: f32) -> String {
    let v = f64::from(v);
    if v == 0.0 {
        return "0".into();
    }
    let e = v.abs().log10().floor() as i32;
    let decimals = (14 - e).max(0) as usize;
    let s = format!("{v:.decimals$}");
    if s.contains('.') { s.trim_end_matches('0').trim_end_matches('.').to_string() } else { s }
}

/// `DynDOLOD_SSE_re-uv-miniatlas.txt` rows, one per source texture (the last tile rule using
/// it wins; `DynDOLOD-temp\` sources and the atlases `skip` names are left out). A row is
/// `src  atlasW atlasH  x y  dest  atlasW atlasH  tileW/atlasW tileH/atlasH` in px (x, y of
/// that last tile), which DynDOLOD/LODGen use to remap full-texture UVs onto the mini atlas.
/// `pbr_src` gives the PBR twin of a rule (its PBR source and destination) when there is one;
/// those get rows of their own.
pub fn reuv_rows(rules: &[Rule], base: u32, skip: impl Fn(&str) -> bool, pbr_src: impl Fn(&Rule) -> Option<(String, String)>) -> Vec<String> {
    let mut order: Vec<String> = Vec::new();
    // source -> (dest, scale text, row)
    let mut rows: HashMap<String, (String, String, String)> = HashMap::new();
    let b = base as f32;
    for r in rules {
        let Rule::Tile { src, tile, pos, dest, atlas, .. } = r else { continue };
        if src.starts_with("dyndolod-temp\\") || skip(dest) {
            continue;
        }
        let (aw, ah) = (((atlas[0] * b).round() as i32).max(1), ((atlas[1] * b).round() as i32).max(1));
        let mut put = |s: String, d: &str| {
            // A later tile of the same atlas moves the position but keeps the first tile's
            // scale (golden: wovenfence01lod 225 128 0.74 1).
            let scale = match rows.get(&s) {
                Some((od, sc, _)) if od == d => sc.clone(),
                _ => format!("{}\t{}", float_to_str(tile[0] / atlas[0]), float_to_str(tile[1] / atlas[1])),
            };
            if !rows.contains_key(&s) {
                order.push(s.clone());
            }
            let line = format!("{s}\t{aw}\t{ah}\t{}\t{}\t{d}\t{aw}\t{ah}\t{scale}", (pos[0] * b).round() as i32, (pos[1] * b).round() as i32);
            rows.insert(s, (d.to_string(), scale, line));
        };
        put(src.clone(), dest);
        if let Some((ps, pd)) = pbr_src(r) {
            put(ps, &pd);
        }
    }
    order.into_iter().map(|s| rows.remove(&s).unwrap().2).collect()
}

/// One mini-atlas row of DynDOLOD's object atlas map (`LODGen_SSE_ObjectAtlasMap_<W>.txt`):
/// `source  w h  x y  dest  atlasW atlasH  scaleU scaleV`, the region `w x h` at `x, y` (px) of
/// the mini atlas `dest` holding whole repetitions of `source`, one repetition being `scale` of
/// the region. LODGen re-UVs full-model shapes with that diffuse onto the region.
#[derive(Debug, Clone, PartialEq)]
pub struct MiniAtlasRow {
    pub source: String,
    pub w: i32,
    pub h: i32,
    pub x: i32,
    pub y: i32,
    pub dest: String,
    pub atlas_w: i32,
    pub atlas_h: i32,
    pub scale: [f32; 2],
}

impl MiniAtlasRow {
    pub fn line(&self) -> String {
        format!("{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}", self.source, self.w, self.h, self.x, self.y, self.dest, self.atlas_w, self.atlas_h, float_to_str(self.scale[0]), float_to_str(self.scale[1]))
    }
}

/// DynDOLOD's mini-atlas rows of the object atlas map, made from the same rules as
/// [`reuv_rows`] (sources and destinations alike; `DynDOLOD-temp\` ones and the atlases `skip`
/// names are left out), one per source: the atlas of the source's last tile rule wins, as in the
/// re-uv map, but the row covers all of the source's tiles in that atlas. Fitted to the E2E
/// golden (279 of 280 plain rows equal; `wovenfence01lod`, whose three tiles overlap and leave
/// the atlas, is the exception):
/// - tile rects in px, each rounded on its own (`round(pos x base)`, `round(size x base)`);
/// - x, y = the smallest corner; repetitions per axis = `round(extent / first tile size)`
///   (at least 1), w, h = `round(repetitions x first tile size)` and scale = 1 / repetitions
///   (golden `sovstairs01`: three 64 px rows 43 px apart = 2 repetitions, 128 px; Majestic
///   Mountains' 3 x 0.335 tiles = 257 px at 1/3).
///
/// `pbr_src` gives a rule's PBR twin (its PBR source, or the plain one, and the twin
/// destination); twin rows use the same tiles and, being later, replace a plain row of the same
/// source (golden: `wrslatefloor01` → `castlekitpbr_lod`).
pub fn atlas_map_rows(rules: &[Rule], base: u32, skip: impl Fn(&str) -> bool, pbr_src: impl Fn(&Rule) -> Option<(String, String)>) -> Vec<MiniAtlasRow> {
    let b = base as f32;
    // (source, dest) -> (atlas px, first tile size px, tile rects px); source -> dest of its last rule.
    let mut tiles: HashMap<(String, String), ((i32, i32), [f32; 2], Vec<[i32; 4]>)> = HashMap::new();
    let mut last: HashMap<String, String> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for r in rules {
        let Rule::Tile { src, tile, pos, dest, atlas, .. } = r else { continue };
        if src.starts_with("dyndolod-temp\\") || dest.starts_with("dyndolod-temp\\") || skip(dest) {
            continue;
        }
        let size = (((atlas[0] * b).round() as i32).max(1), ((atlas[1] * b).round() as i32).max(1));
        let first = [tile[0] * b, tile[1] * b];
        let rect = [(pos[0] * b).round() as i32, (pos[1] * b).round() as i32, first[0].round() as i32, first[1].round() as i32];
        let mut put = |s: String, d: String| {
            if !last.contains_key(&s) {
                order.push(s.clone());
            }
            last.insert(s.clone(), d.clone());
            tiles.entry((s, d)).or_insert_with(|| (size, first, Vec::new())).2.push(rect);
        };
        put(src.clone(), dest.clone());
        if let Some((ps, pd)) = pbr_src(r) {
            put(ps, pd);
        }
    }
    order
        .into_iter()
        .filter_map(|s| {
            let d = last.remove(&s)?;
            let ((aw, ah), first, rects) = tiles.remove(&(s.clone(), d.clone()))?;
            let (x0, y0) = (rects.iter().map(|t| t[0]).min()?, rects.iter().map(|t| t[1]).min()?);
            let (x1, y1) = (rects.iter().map(|t| t[0] + t[2]).max()?, rects.iter().map(|t| t[1] + t[3]).max()?);
            let reps = |extent: i32, t: f32| ((extent as f32 / t).round() as i32).max(1);
            let (ru, rv) = (reps(x1 - x0, first[0]), reps(y1 - y0, first[1]));
            Some(MiniAtlasRow { source: s, w: (ru as f32 * first[0]).round() as i32, h: (rv as f32 * first[1]).round() as i32, x: x0, y: y0, dest: d, atlas_w: aw, atlas_h: ah, scale: [1.0 / ru as f32, 1.0 / rv as f32] })
        })
        .collect()
}

/// A stitched diffuse as RGBA8: alpha kept for `_alpha_` atlases, else 255 (the colour was
/// un-premultiplied in [`TileDraw::shade`]).
pub fn finish_diffuse(l: &Level, keep_alpha: bool) -> Vec<u8> {
    l.px.iter()
        .flat_map(|p| {
            let c = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            [c(p[0]), c(p[1]), c(p[2]), if keep_alpha { c(p[3]) } else { 255 }]
        })
        .collect()
}
/// Box-filtered mip chain of an RGBA8 image, largest first, down to 1x1.
pub fn mips_rgba8(w: u32, h: u32, rgba: &[u8]) -> Vec<(u32, u32, Vec<u8>)> {
    let mut l = Level::from_rgba8(w, h, rgba);
    let mut out = vec![(w, h, rgba.to_vec())];
    while l.w > 1 || l.h > 1 {
        l = l.half();
        out.push((l.w, l.h, l.to_rgba8()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tile_and_full_rules() {
        let t = "// c\n0\t1\t1\t1\ttextures\\landscape\\dirt02.dds\t0.5\t0.5\t0.5\t0\ttextures\\lod\\dirt02lod.dds\t1\t1\n\
                 0\t1\t1\t1\ttextures\\landscape\\snow01.dds\tx\t7\t0\t0\ttextures\\landscape\\snow01noalpha.dds\t0\tx\n";
        let r = parse_rules(t, false);
        assert_eq!(r.len(), 2);
        assert!(matches!(&r[0], Rule::Tile { tile, pos, atlas, .. } if *tile == [0.5, 0.5] && *pos == [0.5, 0.0] && *atlas == [1.0, 1.0]));
        assert!(matches!(&r[1], Rule::Full { mode: 7, param: 0, level: 0, .. }));
        assert_eq!(parse_copy("a\\b_n.dds,c\\d_n.dds\n"), vec![("a\\b_n.dds".to_string(), "c\\d_n.dds".to_string())]);
    }

    #[test]
    fn pbr_names() {
        assert_eq!(pbr_dest("textures\\lod\\dirt02lod.dds"), "textures\\lod\\dirt02pbr_lod.dds");
        assert_eq!(pbr_dest("textures\\lod\\slod04.dds"), "textures\\lod\\spbr_lod04.dds");
        assert_eq!(pbr_dest("textures\\lod\\rtbuildingslod01.dds"), "textures\\lod\\rtbuildingspbr_lod01.dds");
        assert_eq!(pbr_dest("textures\\lod\\roaddetails01.dds"), "textures\\pbr\\lod\\roaddetails01.dds");
        assert_eq!(pbr_dest("textures\\dlc02\\lod\\dlc2lodbasaltside.dds"), "textures\\pbr\\dlc02\\lod\\dlc2lodbasaltside.dds");
        assert_eq!(pbr_dest("textures\\landscape\\glacierslabnoalpha.dds"), "textures\\pbr\\landscape\\glacierslabnoalpha.dds");
        assert_eq!(pbr_path("dyndolod-temp\\textures\\a\\b.dds"), "dyndolod-temp\\textures\\pbr\\a\\b.dds");
        // The twin goes under the textures folder, never inside a mod folder ending in "textures".
        assert_eq!(pbr_path("textures\\xjkcowtextures\\a.dds"), "textures\\pbr\\xjkcowtextures\\a.dds");
        assert_eq!(pbr_path("xjkcowtextures\\a.dds"), "xjkcowtextures\\a.dds");
    }

    #[test]
    fn reuv_rows_last_rule_wins() {
        let mk = |s: &str, x: f32, y: f32, t: f32, d: &str| Rule::Tile { brightness: 0, rgb: [1.0; 3], src: s.into(), tile: [t, t], pos: [x, y], dest: d.into(), atlas: [1.0, 1.0], alpha: false };
        let rules = vec![mk("s.dds", 0.0, 0.0, 0.5, "d.dds"), mk("s.dds", 0.5, 0.5, 0.5, "d.dds"), mk("m.dds", 0.6666, 0.0, 0.335, "e.dds"), mk("dyndolod-temp\\t.dds", 0.0, 0.0, 1.0, "d.dds")];
        let rows = super::reuv_rows(&rules, 256, |_| false, |_| None);
        assert_eq!(rows, vec!["s.dds\t256\t256\t128\t128\td.dds\t256\t256\t0.5\t0.5", "m.dds\t256\t256\t171\t0\te.dds\t256\t256\t0.33500000834465\t0.33500000834465"]);
    }

    #[test]
    fn atlas_map_rows_cover_all_tiles() {
        let mk = |s: &str, x: f32, y: f32, t: f32, d: &str, a: [f32; 2]| Rule::Tile { brightness: 0, rgb: [1.0; 3], src: s.into(), tile: [t, t], pos: [x, y], dest: d.into(), atlas: a, alpha: false };
        let one = [1.0, 1.0];
        let rules = vec![
            // golden roof01lod: four 0.5 tiles -> the whole atlas at 0.5
            mk("roof01.dds", 0.0, 0.0, 0.5, "roof01lod.dds", one),
            mk("roof01.dds", 0.5, 0.0, 0.5, "roof01lod.dds", one),
            mk("roof01.dds", 0.0, 0.5, 0.5, "roof01lod.dds", one),
            mk("roof01.dds", 0.5, 0.5, 0.5, "roof01lod.dds", one),
            // golden farmhouselod01: one tile of a 1x2 atlas
            mk("farmhouse01.dds", 0.5, 0.5, 0.5, "farmhouselod01.dds", [1.0, 2.0]),
            // golden sovstairs01: overlapping rows
            mk("sovstairs01.dds", 0.0, 0.5, 0.25, "sov.dds", one),
            mk("sovstairs01.dds", 0.0, 0.6666, 0.25, "sov.dds", one),
            mk("sovstairs01.dds", 0.0, 0.8333, 0.25, "sov.dds", one),
            // golden mountainslab01lod: 3 x 0.335
            mk("slab.dds", 0.0, 0.0, 0.335, "slablod.dds", one),
            mk("slab.dds", 0.6666, 0.6666, 0.335, "slablod.dds", one),
            // a later atlas wins
            mk("road01.dds", 0.0, 0.5, 0.5, "castlekitlod.dds", one),
            mk("road01.dds", 0.0, 0.0, 0.5, "road01lod.dds", one),
            mk("road01.dds", 0.5, 0.5, 0.5, "road01lod.dds", one),
        ];
        let rows: Vec<String> = super::atlas_map_rows(&rules, 256, |_| false, |_| None).iter().map(|r| r.line()).collect();
        assert_eq!(
            rows,
            vec![
                "roof01.dds\t256\t256\t0\t0\troof01lod.dds\t256\t256\t0.5\t0.5",
                "farmhouse01.dds\t128\t128\t128\t128\tfarmhouselod01.dds\t256\t512\t1\t1",
                "sovstairs01.dds\t64\t128\t0\t128\tsov.dds\t256\t256\t1\t0.5",
                "slab.dds\t257\t257\t0\t0\tslablod.dds\t256\t256\t0.333333343267441\t0.333333343267441",
                "road01.dds\t256\t256\t0\t0\troad01lod.dds\t256\t256\t0.5\t0.5",
            ]
        );
        // A PBR twin row replaces the plain row of a source without a PBR version.
        let twin = |r: &Rule| Some((r.src().to_string(), format!("pbr_{}", r.dest())));
        let rows = super::atlas_map_rows(&rules[..4], 256, |_| false, twin);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].dest, "pbr_roof01lod.dds");
    }

    #[test]
    fn no_pbr_rows_without_a_twin() {
        // `--materials complex`: the twin closure is always None, so neither map names a
        // `textures\pbr\` source or a `pbr_lod` destination; with a twin both do.
        let mk = |x: f32| Rule::Tile { brightness: 0, rgb: [1.0; 3], src: "textures\\landscape\\dirt02.dds".into(), tile: [0.5, 0.5], pos: [x, 0.0], dest: "textures\\lod\\dirt02lod.dds".into(), atlas: [1.0, 1.0], alpha: false };
        let rules = vec![mk(0.0), mk(0.5)];
        let twin = |r: &Rule| Some((pbr_path(r.src()), pbr_dest(r.dest())));
        let none = |_: &Rule| None;
        let has_pbr = |s: &str| s.contains("\\pbr\\") || s.contains("pbr_lod");
        let reuv = super::reuv_rows(&rules, 256, |_| false, none);
        let map: Vec<String> = super::atlas_map_rows(&rules, 256, |_| false, none).iter().map(|r| r.line()).collect();
        assert!(!reuv.is_empty() && !map.is_empty());
        assert!(reuv.iter().chain(&map).all(|l| !has_pbr(l)), "{reuv:?} {map:?}");
        let reuv = super::reuv_rows(&rules, 256, |_| false, twin);
        let map: Vec<String> = super::atlas_map_rows(&rules, 256, |_| false, twin).iter().map(|r| r.line()).collect();
        assert!(reuv.iter().any(|l| has_pbr(l)) && map.iter().any(|l| has_pbr(l)), "{reuv:?} {map:?}");
    }

    #[test]
    fn tone_and_unmultiply() {
        let t = Tone::new(-4, [0.95, 0.93, 1.0]);
        assert!((t.apply(0.5, 2) - 0.46).abs() < 1e-6);
        assert!((t.apply(0.5, 0) - (0.5f32.powf(1.0 / 0.95) - 0.04)).abs() < 1e-6);
        // Alpha >= 128: colour / alpha before the tone; below: unchanged.
        let chain = MipChain::from_level(Level { w: 1, h: 1, px: vec![[0.3, 0.3, 0.3, 0.6]] });
        let d = TileDraw { chain: &chain, rect: [0.0, 0.0, 1.0, 1.0], tone: Tone { scale: 0.65, ..Tone::IDENTITY }, keep_alpha: true, unmultiply: true };
        assert!((d.shade([0.3, 0.3, 0.3, 0.6])[0] - 0.5 * 0.65).abs() < 1e-6);
        assert!((d.shade([0.3, 0.3, 0.3, 0.4])[0] - 0.3 * 0.65).abs() < 1e-6);
        // A height-mask alpha (PBR landscape) is not divided out.
        let h = TileDraw { unmultiply: false, ..d };
        assert!((h.shade([0.3, 0.3, 0.3, 0.6])[0] - 0.3 * 0.65).abs() < 1e-6);
        assert!(alpha_is_height(r"textures\PBR\landscape\coastbeach01.dds"));
        assert!(!alpha_is_height(r"textures\landscape\coastbeach01.dds"));
        assert!(!alpha_is_height(r"textures\pbr\architecture\whiterun\wrroof01.dds"));
    }

    #[test]
    fn composite_fills_rect() {
        let chain = MipChain::from_level(Level { w: 2, h: 2, px: vec![[1.0, 0.0, 0.0, 1.0]; 4] });
        let img = composite(4, 4, &[TileDraw { chain: &chain, rect: [0.0, 0.0, 2.0, 2.0], tone: Tone::IDENTITY, keep_alpha: true, unmultiply: true }], 0.0, false);
        assert_eq!(img.px[0], [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(img.px[3], [0.0; 4]);
    }
}
