//! `lodforge texgen-stitched`: TexGen's stitched object-LOD textures (mini atlases under
//! `textures\lod\...`, their `_n` twins, PBR variants, `x`-rule full textures) from the
//! `DynDOLOD_SSE_TexGen_{noalpha,alpha,copy}_<plugin>.txt` rules of the loaded plugins, plus
//! `DynDOLOD_SSE_re-uv-miniatlas.txt`. Compared with a golden TexGen output by PSNR.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use anyhow::Result;
use rayon::prelude::*;

use lodforge_texgen::stitch::{self, Atlas, Level, MipChain, Rule, StitchParams, TileDraw};

use crate::profile::{Profile, ProfileArgs};

#[derive(clap::Args)]
pub struct Args {
    #[command(flatten)]
    pub profile: ProfileArgs,
    /// Write the textures under this Data folder.
    #[arg(long)]
    pub out: Option<PathBuf>,
    /// Golden TexGen output (Data folder with textures\) to compare with.
    #[arg(long)]
    pub golden: Option<PathBuf>,
    /// Golden `DynDOLOD_SSE_re-uv-miniatlas.txt` to compare our rows with.
    #[arg(long)]
    pub golden_reuv: Option<PathBuf>,
    /// Only destinations whose path contains this.
    #[arg(long)]
    pub filter: Option<String>,
    /// DynDOLOD folder (rule files in `Edit Scripts\DynDOLOD\Configs`); defaults to
    /// `<instance>\tools\DynDOLOD`. Read only.
    #[arg(long)]
    pub dyndolod: Option<PathBuf>,
    /// Mip LOD bias of the stitched shader.
    #[arg(long, default_value_t = -1.25, allow_hyphen_values = true)]
    pub bias: f32,
    /// Print every compared texture.
    #[arg(long)]
    pub verbose: bool,
    /// Print where these textures come from and their mean RGBA per mip, then exit.
    #[arg(long, value_delimiter = ',')]
    pub probe: Vec<String>,
    /// List the rendered object-LOD texture NIFs (plane, scale, output) with the golden sizes, then exit.
    #[arg(long)]
    pub render_probe: bool,
    /// Also render the rendered object-LOD textures (§3.3) from `RenderSkyrimObjects`.
    #[arg(long)]
    pub rendered: bool,
    /// View yaw for the rendered textures (degrees).
    #[arg(long, default_value_t = RENDER_YAW, allow_hyphen_values = true)]
    pub yaw: f32,
}

/// What one run made.
#[derive(Default)]
pub struct Made {
    pub atlases: usize,
    pub pbr: usize,
    pub full: usize,
    pub temps: usize,
    pub normals: usize,
    pub skipped: Vec<String>,
    pub reuv: Vec<String>,
    /// Rendered object-LOD textures (§3.3, diffuse count incl. PBR twins) and the NIFs left out.
    pub rendered: usize,
    pub rendered_skipped: Vec<String>,
}

/// View of the rendered object-LOD textures: from -Y, image right = +X (yaw 0 sees only the
/// pieces' back faces; golden wrwallslod01 33 dB at 180).
pub const RENDER_YAW: f32 = 180.0;

/// One finished texture (RGBA8, level 0) and its output path.
struct Out {
    path: String,
    w: u32,
    h: u32,
    rgba: Vec<u8>,
}

/// The stitching rules of the loaded plugins.
pub struct RuleSet {
    /// In TexGen's order (per plugin: noalpha, then alpha).
    pub rules: Vec<Rule>,
    /// Rules of destinations a later file defines again. TexGen keys its atlas list by output
    /// path, so the later definition replaces the plain atlas, while the PBR twin of the earlier
    /// one survives unless the later one makes a PBR twin too (golden: Majestic Mountains'
    /// `mountainslablod` 2-tile 512 px, `mountainslabpbr_lod` from vanilla's 9 tiles).
    pub shadowed: Vec<Rule>,
    /// Every rule of every file in order (for the re-uv map).
    pub all: Vec<Rule>,
    pub copies: Vec<(String, String)>,
}

pub fn load_rules(p: &Profile, dyndolod: &Path) -> RuleSet {
    let mut rules: Vec<Rule> = Vec::new();
    let mut shadowed = Vec::new();
    let mut all = Vec::new();
    let mut copies = Vec::new();
    for plugin in &p.plugin_names {
        let text = |kind: &str| crate::texgen_billboards::config_lines(p, dyndolod, &format!("DynDOLOD_SSE_TexGen_{kind}_"), plugin).join("\n");
        copies.extend(stitch::parse_copy(&text("copy")));
        for (kind, alpha) in [("noalpha", false), ("alpha", true)] {
            let new = stitch::parse_rules(&text(kind), alpha);
            all.extend(new.iter().cloned());
            let dests: HashSet<&str> = new.iter().map(Rule::dest).collect();
            let (old, keep): (Vec<Rule>, Vec<Rule>) = rules.into_iter().partition(|r| dests.contains(r.dest()));
            shadowed.retain(|r: &Rule| !dests.contains(r.dest()));
            shadowed.extend(old);
            rules = keep;
            rules.extend(new);
        }
    }
    RuleSet { rules, shadowed, all, copies }
}

/// Source lookup: copy aliases (only for destinations the load order lacks), `DynDOLOD-temp\`
/// intermediates made earlier in the run, then the VFS.
struct Sources<'a> {
    p: &'a Profile,
    /// `--materials pbr`: `textures\pbr\` versions are looked up (CheckPBR, PBR twins, the PBR
    /// copy aliases). Off (`--materials complex`), no `textures\pbr\` source is consulted.
    pbr: bool,
    alias: HashMap<String, String>,
    temps: Mutex<HashMap<String, Level>>,
    cache: Mutex<HashMap<(String, u32), std::sync::Arc<MipChain>>>,
}

impl<'a> Sources<'a> {
    fn new(p: &'a Profile, copies: &[(String, String)], pbr: bool) -> Sources<'a> {
        let mut alias = HashMap::new();
        for (src, dst) in copies {
            let pairs = [(src.clone(), dst.clone()), (stitch::pbr_path(src), stitch::pbr_path(dst))];
            for (s, d) in pairs.into_iter().take(if pbr { 2 } else { 1 }) {
                if !p.vfs.exists(&d) && p.vfs.exists(&s) {
                    alias.entry(d).or_insert(s);
                }
            }
        }
        Sources { p, pbr, alias, temps: Mutex::new(HashMap::new()), cache: Mutex::new(HashMap::new()) }
    }
    fn resolve(&self, path: &str) -> String {
        let n = stitch::norm(path);
        self.alias.get(&n).cloned().unwrap_or(n)
    }
    fn exists(&self, path: &str) -> bool {
        let n = self.resolve(path);
        if n.starts_with("dyndolod-temp\\") {
            return self.temps.lock().unwrap().contains_key(&n);
        }
        self.p.vfs.exists(&n)
    }
    /// TexGen's CheckPBR: some source (by its own name, before copy aliases) has a
    /// `textures\pbr\` version in the load order or among the PBR temporaries. Never with
    /// `--materials complex`.
    fn has_pbr(&self, a: &Atlas) -> bool {
        self.pbr && a.rules.iter().any(|r| {
            let raw = stitch::norm(r.src());
            let p = stitch::pbr_path(&raw);
            p != raw && if p.starts_with("dyndolod-temp\\") { self.temps.lock().unwrap().contains_key(&p) } else { self.p.vfs.exists(&p) }
        })
    }
    /// The full-size of a source without decoding it.
    fn size(&self, path: &str) -> Option<(u32, u32)> {
        let n = self.resolve(path);
        if n.starts_with("dyndolod-temp\\") {
            return self.temps.lock().unwrap().get(&n).map(|l| (l.w, l.h));
        }
        let b = self.p.vfs.read(&n).ok().flatten()?;
        let i = lodforge_dds::parse(&b).ok()?;
        Some((i.width, i.height))
    }
    /// The mip chain from the mip `first` down.
    fn chain(&self, path: &str, first: u32) -> Option<std::sync::Arc<MipChain>> {
        let n = self.resolve(path);
        if let Some(c) = self.cache.lock().unwrap().get(&(n.clone(), first)) {
            return Some(c.clone());
        }
        let c = if n.starts_with("dyndolod-temp\\") {
            let l = self.temps.lock().unwrap().get(&n)?.clone();
            let mut c = MipChain::from_level(l);
            let f = first.min(c.levels.len() as u32 - 1);
            c.levels.drain(..f as usize);
            c.first = f;
            c
        } else {
            let b = self.p.vfs.read(&n).ok().flatten()?;
            MipChain::from_dds(&b, first)?
        };
        let c = std::sync::Arc::new(c);
        self.cache.lock().unwrap().insert((n, first), c.clone());
        Some(c)
    }
}

/// Flat tangent-space normal for sources without a `_n`.
const FLAT_NORMAL: [u8; 4] = [128, 128, 255, 255];

/// Builds one tile atlas (diffuse and `_n`); `pbr` = the PBR twin (sources' `textures\pbr\`
/// versions where they exist, colour scaled by `StitchPBRScale`). None when a source is missing.
fn build_atlas(src: &Sources, a: &Atlas, prm: &StitchParams, pbr: bool) -> std::result::Result<(Level, Level, bool), String> {
    let (w, h) = stitch::atlas_size(a, prm.base).ok_or("no tile rules")?;
    let base = prm.base as f32;
    let mut chains = Vec::new();
    let mut nchains = Vec::new();
    let mut alpha = false;
    for r in &a.rules {
        let Rule::Tile { brightness, rgb, src: s, tile, pos, alpha: keep, .. } = r else { continue };
        alpha |= *keep;
        let path = if pbr && src.exists(&stitch::pbr_path(s)) { stitch::pbr_path(s) } else { s.clone() };
        let rect = [pos[0] * base, pos[1] * base, tile[0] * base, tile[1] * base];
        let (sw, sh) = src.size(&path).ok_or_else(|| format!("missing {path}"))?;
        let first = stitch::tile_lod(sw, sh, rect[2], rect[3], prm.bias).floor() as u32;
        let chain = src.chain(&path, first).ok_or_else(|| format!("unreadable {path}"))?;
        let mut tone = if prm.rule_tone { stitch::Tone::new(*brightness, *rgb) } else { stitch::Tone::IDENTITY };
        // StitchPBRScale scales the PBR sources only (golden castlekitpbr_lod mixes both).
        if path.contains(r"\pbr\") {
            tone.scale = prm.pbr_scale;
        }
        chains.push((chain, rect, tone));
        // The `_n` of the diffuse TexGen really loads (after a copy alias of the diffuse itself:
        // Majestic Mountains' mountainslabtemp.dds -> mountainslab01.dds uses mountainslab01_n).
        let npath = stitch::normal_path(&src.resolve(&path));
        let n = src.size(&npath).and_then(|(nw, nh)| src.chain(&npath, stitch::tile_lod(nw, nh, rect[2], rect[3], prm.bias).floor() as u32));
        nchains.push((n, rect));
    }
    let draws: Vec<TileDraw> = chains.iter().map(|(c, rect, tone)| TileDraw { chain: c, rect: *rect, tone: *tone, keep_alpha: true }).collect();
    let diffuse = stitch::composite(w, h, &draws, prm.bias, false);
    let flat = MipChain::from_level(Level::from_rgba8(1, 1, &FLAT_NORMAL));
    let ndraws: Vec<TileDraw> = nchains.iter().map(|(c, rect)| TileDraw { chain: c.as_deref().unwrap_or(&flat), rect: *rect, tone: stitch::Tone::IDENTITY, keep_alpha: true }).collect();
    let normal = stitch::composite(w, h, &ndraws, prm.bias, true);
    Ok((diffuse, normal, alpha))
}

/// An `x` rule: the source at its own size (mode/level semantics are open; we copy mip `level`
/// of the source, alpha stripped for `noalpha` destinations).
fn build_full(src: &Sources, r: &Rule, pbr: bool, rule_tone: bool) -> std::result::Result<Out, String> {
    let Rule::Full { src: s, dest, brightness, rgb, alpha, .. } = r else { return Err("not a full rule".into()) };
    let path = if pbr { stitch::pbr_path(s) } else { s.clone() };
    let chain = src.chain(&path, 0).ok_or_else(|| format!("missing {path}"))?;
    let l = &chain.levels[0];
    let t = if rule_tone { stitch::Tone::new(*brightness, *rgb) } else { stitch::Tone::IDENTITY };
    let px: Vec<[f32; 4]> = l.px.iter().map(|p| [t.apply(p[0], 0), t.apply(p[1], 1), t.apply(p[2], 2), if *alpha && !dest.contains("noalpha") { p[3] } else { 1.0 }]).collect();
    let lv = Level { w: l.w, h: l.h, px };
    Ok(Out { path: if pbr { stitch::pbr_path(dest) } else { dest.clone() }, w: lv.w, h: lv.h, rgba: lv.to_rgba8() })
}

/// Destinations whose rules write only the `_n` (a negative red value).
fn normal_only(rs: &RuleSet) -> HashSet<&str> {
    rs.rules.iter().filter(|r| matches!(r, Rule::Tile { rgb, .. } if rgb[0] < 0.0)).map(Rule::dest).collect()
}

/// A rule's PBR twin for the re-uv and atlas map rows (its PBR source, or the plain one, and the
/// twin destination). Every source of an atlas with a PBR twin also maps onto the twin, by its
/// PBR version when it has one (golden: castlekitpbr_lod rows for wrslatefloor01, which has no
/// PBR version). (Per definition: a shadowed one has its own PBR check.)
fn pbr_twin<'a>(src: &'a Sources<'_>, rs: &'a RuleSet) -> impl Fn(&Rule) -> Option<(String, String)> + 'a {
    let pbr_set = |rules: &[Rule]| -> HashSet<String> { stitch::group(rules).iter().filter(|a| src.has_pbr(a)).map(|a| a.dest.clone()).collect() };
    let (pbr_new, pbr_old) = (pbr_set(&rs.rules), pbr_set(&rs.shadowed));
    move |r: &Rule| {
        if !src.pbr || !(if rs.shadowed.contains(r) { &pbr_old } else { &pbr_new }).contains(r.dest()) {
            return None;
        }
        let p = stitch::pbr_path(r.src());
        Some((if src.p.vfs.exists(&p) { p } else { stitch::norm(r.src()) }, stitch::pbr_dest(r.dest())))
    }
}

/// DynDOLOD's mini-atlas rows of the object atlas map for the TexGen rules of the load order,
/// limited to mini atlases that exist (in the load order or in a build output added to the VFS).
/// No stitching: the PBR temporaries are only marked.
pub fn miniatlas_rows(p: &Profile, dyndolod: &Path, pbr: bool) -> Vec<stitch::MiniAtlasRow> {
    let rs = load_rules(p, dyndolod);
    let src = Sources::new(p, &rs.copies, pbr);
    for a in stitch::group(&rs.rules).iter().filter(|a| a.dest.starts_with("dyndolod-temp\\")) {
        if src.has_pbr(a) {
            src.temps.lock().unwrap().insert(stitch::pbr_path(&a.dest), Level { w: 0, h: 0, px: Vec::new() });
        }
    }
    let normal_only = normal_only(&rs);
    let mut rows = stitch::atlas_map_rows(&rs.all, StitchParams::default().base, |d| normal_only.contains(d), pbr_twin(&src, &rs));
    rows.retain(|r| p.vfs.exists(&r.dest));
    rows
}

/// `s` with the first case-insensitive match of `from` replaced by `to` (Delphi `StringReplace`
/// with `[rfIgnoreCase]`).
fn replace_first_ci(s: &str, from: &str, to: &str) -> String {
    if from.is_empty() {
        return s.to_string();
    }
    match s.to_ascii_lowercase().find(&from.to_ascii_lowercase()) {
        Some(i) => format!("{}{to}{}", &s[..i], &s[i + from.len()..]),
        None => s.to_string(),
    }
}

fn file_stem(p: &str) -> &str {
    let f = p.rsplit('\\').next().unwrap_or(p);
    f.rsplit_once('.').map_or(f, |(s, _)| s)
}

/// DynDOLOD's name for the mini atlas of `texture` made from `base`'s mini atlas `atlas`
/// (`AddMiniAtlas`): the atlas name with `base`'s file name replaced by `texture`'s
/// (`dirtcliffs01lod` -> `volcanictundradirtcliffs01lod`), `textures\lod\<texture>lod.dds` when
/// the atlas name does not contain it, `pbr_lod.dds` for a texture under `\pbr\`.
pub fn mini_atlas_name(atlas: &str, base: &str, texture: &str) -> String {
    let mut d = replace_first_ci(atlas, file_stem(base), file_stem(texture));
    if d == atlas {
        d = format!("textures\\lod\\{}lod.dds", file_stem(texture));
    }
    if texture.contains("\\pbr\\") {
        d = replace_first_ci(&d, "lod.dds", "pbr_lod.dds");
    }
    stitch::norm(&d)
}

/// The mini atlases DynDOLOD builds itself for `requests` (`MiniAtlasExport`, see
/// `lodforge_dyndolod::sidefiles::MiniAtlasRequest`): each request whose base is the source of a
/// TexGen tile rule gets the tiles of that source in the atlas of its last rule (TexGen's
/// layout, brightness and colour) with the requested texture drawn instead. Left out: textures
/// with a mini-atlas row of their own (`has_row`), destinations the load order (or TexGen's
/// output) already has, a destination asked for twice (the first wins).
pub fn mini_atlas_export(p: &Profile, dyndolod: &Path, requests: &[lodforge_dyndolod::sidefiles::MiniAtlasRequest], has_row: impl Fn(&str) -> bool) -> Vec<Atlas> {
    let rs = load_rules(p, dyndolod);
    let mut last: HashMap<&str, &str> = HashMap::new();
    for r in &rs.all {
        if let Rule::Tile { src, dest, .. } = r {
            last.insert(src, dest);
        }
    }
    let mut out: Vec<Atlas> = Vec::new();
    for q in requests {
        let (base, texture) = (stitch::norm(&q.base), stitch::norm(&q.texture));
        if has_row(&texture) {
            continue;
        }
        let Some(&atlas) = last.get(base.as_str()).filter(|a| !a.starts_with("dyndolod-temp\\")) else { continue };
        let dest = mini_atlas_name(atlas, &base, &texture);
        if p.vfs.exists(&dest) || out.iter().any(|a| a.dest == dest) {
            continue;
        }
        let mut rules: Vec<Rule> = Vec::new();
        for r in &rs.all {
            if let Rule::Tile { src, dest: d, .. } = r
                && *src == base
                && d == atlas
            {
                let mut r = r.clone();
                if let Rule::Tile { src, dest: d, .. } = &mut r {
                    *src = texture.clone();
                    *d = dest.clone();
                }
                if !rules.contains(&r) {
                    rules.push(r);
                }
            }
        }
        out.push(Atlas { dest, rules });
    }
    out
}

/// `LODGen_SSE_MiniAtlasExport_<W>.txt`: the rules of `atlases` in TexGen's rule format with the
/// destination under `out` (`0 1 1 1 src 0.500 0.500 0.000 0.000 <out>\textures\lod\x.dds 1.000
/// 1.000`), a blank line after each atlas.
pub fn mini_atlas_export_text(atlases: &[Atlas], out: &Path) -> String {
    let root = out.display().to_string().replace('/', "\\");
    let mut s = String::new();
    for a in atlases {
        for r in &a.rules {
            if let Rule::Tile { brightness, rgb, src, tile, pos, dest, atlas, .. } = r {
                s.push_str(&format!(
                    "{brightness}\t{}\t{}\t{}\t{src}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{}\\{dest}\t{:.3}\t{:.3}\r\n",
                    rgb[0], rgb[1], rgb[2], tile[0], tile[1], pos[0], pos[1], root.trim_end_matches('\\'), atlas[0], atlas[1]
                ));
            }
        }
        s.push_str("\r\n");
    }
    s
}

/// Stitches `atlases` (diffuse and `_n`, TexGen's stitching, PBR sources at full brightness: the
/// E2E golden `frozenmarshice02pbr_lod` has the mean colour of its source) into `out`; returns the textures
/// written and the atlases left out (missing source).
pub fn stitch_atlases(p: &Profile, gpu: &lodforge_gpu::Gpu, atlases: &[Atlas], out: &Path, pbr: bool) -> Result<(Vec<String>, Vec<String>)> {
    let src = Sources::new(p, &[], pbr);
    let prm = StitchParams { pbr_scale: 1.0, ..StitchParams::default() };
    let results: Vec<std::result::Result<Vec<Out>, String>> = atlases
        .par_iter()
        .map(|a| {
            let (d, n, alpha) = build_atlas(&src, a, &prm, false).map_err(|e| format!("{}: {e}", a.dest))?;
            Ok(vec![Out { path: stitch::normal_path(&a.dest), w: n.w, h: n.h, rgba: n.to_rgba8() }, Out { path: a.dest.clone(), w: d.w, h: d.h, rgba: stitch::finish_diffuse(&d, alpha) }])
        })
        .collect();
    let (mut written, mut skipped) = (Vec::new(), Vec::new());
    for r in results {
        match r {
            Ok(outs) => {
                for o in outs {
                    let path = out.join(&o.path);
                    std::fs::create_dir_all(path.parent().unwrap())?;
                    std::fs::write(&path, encode(gpu, &o)?)?;
                    written.push(o.path);
                }
            }
            Err(e) => skipped.push(e),
        }
    }
    Ok((written, skipped))
}

/// Everything the rules make, in TexGen's order; temporaries stay in memory.
fn make_all(src: &Sources, rs: &RuleSet, prm: &StitchParams, filter: Option<&str>, mut sink: impl FnMut(Out) -> Result<()>) -> Result<Made> {
    let mut made = Made::default();
    let atlases = stitch::group(&rs.rules);
    // Temporaries first (sequentially: later temps may use earlier ones), then the rest in parallel.
    let (temps, finals): (Vec<&Atlas>, Vec<&Atlas>) = atlases.iter().partition(|a| a.dest.starts_with("dyndolod-temp\\"));
    for a in &temps {
        for pbr in [false, true] {
            if pbr && !src.has_pbr(a) {
                continue;
            }
            match build_atlas(src, a, prm, pbr) {
                Ok((mut d, _, alpha)) => {
                    // Saved like any output (TexGen writes the temporary and reloads it): alpha 255
                    // for `_noalpha_` rules, so the atlas using it does not un-premultiply again.
                    if !alpha {
                        d.px.iter_mut().for_each(|p| p[3] = 1.0);
                    }
                    let key = if pbr { stitch::pbr_path(&a.dest) } else { a.dest.clone() };
                    src.temps.lock().unwrap().insert(key, d);
                    made.temps += 1;
                }
                Err(e) => made.skipped.push(format!("{}: {e}", a.dest)),
            }
        }
    }
    let normal_only = normal_only(rs);
    made.reuv = stitch::reuv_rows(&rs.all, prm.base, |d| normal_only.contains(d), pbr_twin(src, rs));
    // (atlas, plain, PBR twin): a shadowed definition only keeps a PBR twin the newer one lacks.
    let shadowed = stitch::group(&rs.shadowed);
    let mut jobs: Vec<(&Atlas, bool, bool)> = finals.iter().map(|a| (*a, true, src.has_pbr(a))).collect();
    for s in &shadowed {
        if src.has_pbr(s) && !jobs.iter().any(|(a, _, pbr)| a.dest == s.dest && *pbr) {
            jobs.push((s, false, true));
        }
    }
    jobs.retain(|(a, _, _)| filter.is_none_or(|f| a.dest.contains(&f.to_ascii_lowercase())));
    let results: Vec<(Vec<Out>, Vec<String>, [usize; 4])> = jobs
        .par_iter()
        .map(|&(a, plain, has_pbr)| {
            let mut outs = Vec::new();
            let mut skipped = Vec::new();
            let mut counts = [0usize; 4];
            let full = a.rules.iter().all(|r| matches!(r, Rule::Full { .. }));
            // A negative red multiplier: only the `_n` is written (Majestic Mountains'
            // `mountainslabHQlod`, golden: `_n` saved, no diffuse).
            let normal_only = a.rules.iter().any(|r| matches!(r, Rule::Tile { rgb, .. } if rgb[0] < 0.0));
            for pbr in [false, true] {
                if (pbr && !has_pbr) || (!pbr && !plain) {
                    continue;
                }
                if full {
                    match build_full(src, &a.rules[0], pbr, prm.rule_tone) {
                        Ok(o) => {
                            outs.push(o);
                            counts[2] += 1;
                        }
                        Err(e) => skipped.push(format!("{}: {e}", a.dest)),
                    }
                    continue;
                }
                match build_atlas(src, a, prm, pbr) {
                    Ok((d, n, alpha)) => {
                        let path = if pbr { stitch::pbr_dest(&a.dest) } else { a.dest.clone() };
                        outs.push(Out { path: stitch::normal_path(&path), w: n.w, h: n.h, rgba: n.to_rgba8() });
                        let rgba = stitch::finish_diffuse(&d, alpha);
                        if !normal_only {
                            outs.push(Out { path, w: d.w, h: d.h, rgba });
                        }
                        counts[if pbr { 1 } else { 0 }] += 1;
                        counts[3] += 1;
                    }
                    Err(e) => skipped.push(format!("{}{}: {e}", a.dest, if pbr { " (pbr)" } else { "" })),
                }
            }
            (outs, skipped, counts)
        })
        .collect();
    for (outs, skipped, c) in results {
        made.atlases += c[0];
        made.pbr += c[1];
        made.full += c[2];
        made.normals += c[3];
        made.skipped.extend(skipped);
        for o in outs {
            sink(o)?;
        }
    }
    Ok(made)
}

/// BC7 DDS with a full box-filtered mip chain (TexGen: Texconv BC7, full mips).
fn encode(gpu: &lodforge_gpu::Gpu, o: &Out) -> Result<Vec<u8>> {
    let mips = stitch::mips_rgba8(o.w, o.h, &o.rgba);
    let imgs: Vec<lodforge_gpu::RgbaImage> = mips.iter().map(|(w, h, d)| lodforge_gpu::RgbaImage { width: *w, height: *h, rgba: d }).collect();
    let blocks = gpu.encode_bc7(&imgs)?;
    Ok(lodforge_dds::write_bc7(o.w, o.h, &blocks))
}

/// PSNR (dB) of RGB, and of alpha, between two same-size RGBA8 images.
pub fn psnr(a: &[u8], b: &[u8]) -> (f64, f64) {
    let (mut se, mut sa, mut n) = (0f64, 0f64, 0u64);
    for (pa, pb) in a.chunks_exact(4).zip(b.chunks_exact(4)) {
        se += (0..3).map(|k| (f64::from(pa[k]) - f64::from(pb[k])).powi(2)).sum::<f64>();
        sa += (f64::from(pa[3]) - f64::from(pb[3])).powi(2);
        n += 1;
    }
    let p = |se: f64, n: f64| if se == 0.0 { 99.0 } else { 10.0 * (255.0f64 * 255.0 / (se / n)).log10() };
    (p(se, n as f64 * 3.0), p(sa, n as f64))
}

/// Comparison totals.
#[derive(Default)]
struct Stats {
    rows: Vec<(String, u32, u32, bool, f64, f64)>,
    size_mismatch: Vec<String>,
    no_golden: Vec<String>,
}

impl Stats {
    fn add(&mut self, golden: &Path, o: &Out, verbose: bool) {
        let gp = golden.join(&o.path);
        let Some(gb) = std::fs::read(&gp).ok() else {
            self.no_golden.push(o.path.clone());
            return;
        };
        let Some(gi) = lodforge_dds::parse(&gb).ok().and_then(|i| lodforge_dds::decode(&gb, &i, 0).ok()) else {
            self.no_golden.push(format!("{} (unreadable golden)", o.path));
            return;
        };
        if (gi.width, gi.height) != (o.w, o.h) {
            self.size_mismatch.push(format!("{} ours {}x{} golden {}x{}", o.path, o.w, o.h, gi.width, gi.height));
            return;
        }
        let (prgb, pa) = psnr(&gi.rgba, &o.rgba);
        if std::env::var_os("TG_PNG").is_some() {
            // Golden | ours, opaque, for eyeballing.
            let (w, h) = (o.w as usize, o.h as usize);
            let mut img = vec![0u8; w * 2 * h * 4];
            for y in 0..h {
                for x in 0..w {
                    let s = (y * w + x) * 4;
                    for (k, src) in [&gi.rgba, &o.rgba].into_iter().enumerate() {
                        let d = (y * w * 2 + k * w + x) * 4;
                        img[d..d + 3].copy_from_slice(&src[s..s + 3]);
                        img[d + 3] = 255;
                    }
                }
            }
            let name = o.path.rsplit('\\').next().unwrap_or("x").replace(".dds", ".png");
            let _ = std::fs::create_dir_all("bench/png");
            if let Ok(f) = std::fs::File::create(format!("bench/png/{name}")) {
                let mut e = png::Encoder::new(std::io::BufWriter::new(f), (w * 2) as u32, h as u32);
                e.set_color(png::ColorType::Rgba);
                if let Ok(mut wr) = e.write_header() {
                    let _ = wr.write_image_data(&img);
                }
            }
        }
        if verbose {
            let mean = |v: &[u8]| { let n = (v.len() / 4) as f64; let mut m = [0f64; 4]; for p in v.chunks_exact(4) { for k in 0..4 { m[k] += f64::from(p[k]) / n; } } m };
            let (mg, mo) = (mean(&gi.rgba), mean(&o.rgba));
            println!("  {:<64} {:>4}x{:<4} rgb {:6.2} dB  alpha {:6.2} dB  mean golden {:.0} {:.0} {:.0} {:.0} ours {:.0} {:.0} {:.0} {:.0}", o.path, o.w, o.h, prgb, pa, mg[0], mg[1], mg[2], mg[3], mo[0], mo[1], mo[2], mo[3]);
        }
        self.rows.push((o.path.clone(), o.w, o.h, o.path.ends_with("_n.dds"), prgb, pa));
    }
    fn print(&self) {
        let summary = |name: &str, f: &dyn Fn(&&(String, u32, u32, bool, f64, f64)) -> bool| {
            let r: Vec<_> = self.rows.iter().filter(f).collect();
            if r.is_empty() {
                return;
            }
            let mut v: Vec<f64> = r.iter().map(|x| x.4).collect();
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let mean = v.iter().sum::<f64>() / v.len() as f64;
            let ge30 = v.iter().filter(|x| **x >= 30.0).count();
            println!("  {name:<10} {:>4} compared: mean {:.2} dB, median {:.2}, min {:.2}, >=30 dB {}", v.len(), mean, v[v.len() / 2], v[0], ge30);
        };
        summary("diffuse", &|x| !x.3 && !x.0.contains("\\pbr\\") && !x.0.contains("pbr_lod"));
        summary("pbr", &|x| !x.3 && (x.0.contains("\\pbr\\") || x.0.contains("pbr_lod")));
        summary("normal", &|x| x.3);
        let mut worst: Vec<_> = self.rows.iter().collect();
        worst.sort_by(|a, b| a.4.partial_cmp(&b.4).unwrap());
        println!("  worst:");
        for w in worst.iter().take(12) {
            println!("    {:<64} {:6.2} dB", w.0, w.4);
        }
        if !self.size_mismatch.is_empty() {
            println!("  size mismatches: {}", self.size_mismatch.len());
            for s in self.size_mismatch.iter().take(20) {
                println!("    {s}");
            }
        }
        if !self.no_golden.is_empty() {
            println!("  not in golden: {}", self.no_golden.len());
            for s in self.no_golden.iter().take(20) {
                println!("    {s}");
            }
        }
    }
}

/// The paths a golden TexGen output has for stitched textures: everything under `textures\`
/// except billboards (`terrain\lodgen`, `DynDOLOD\LOD`).
fn golden_files(golden: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let root = golden.join("textures");
    let mut stack = vec![root.clone()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dds")) {
                let rel = p.strip_prefix(golden).unwrap().to_string_lossy().to_ascii_lowercase();
                if !rel.starts_with("textures\\terrain\\lodgen") && !rel.starts_with("textures\\dyndolod\\") {
                    out.push(rel);
                }
            }
        }
    }
    out.sort();
    out
}

/// Stitches everything into `out` (the build stage). Returns what was made. `pbr` off
/// (`--materials complex`): plain textures only, no `textures\pbr\` source is read.
pub fn stitch_into(p: &Profile, gpu: &lodforge_gpu::Gpu, dyndolod: &Path, out: &Path, reuv_out: Option<&Path>, pbr: bool, rule_tone: bool) -> Result<Made> {
    let rules = load_rules(p, dyndolod);
    let src = Sources::new(p, &rules.copies, pbr);
    let prm = StitchParams { rule_tone, ..StitchParams::default() };
    let write = |o: Out| -> Result<()> {
        let path = out.join(&o.path);
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(&path, encode(gpu, &o)?)?;
        Ok(())
    };
    let mut made = make_all(&src, &rules, &prm, None, write)?;
    // Rendered object-LOD textures (§3.3), after the stitched ones (TexGen's order).
    let renderer = lodforge_texgen::Renderer::new(gpu);
    let (rs, skipped) = crate::texgen_rendered::render_all(p, gpu, &renderer, dyndolod, None, RENDER_YAW, pbr)?;
    made.rendered = rs.len();
    made.rendered_skipped = skipped;
    for r in rs {
        write(Out { path: stitch::normal_path(&r.path), w: r.w, h: r.h, rgba: r.normal })?;
        write(Out { path: r.path, w: r.w, h: r.h, rgba: r.rgba })?;
    }
    if let Some(r) = reuv_out {
        std::fs::create_dir_all(r.parent().unwrap())?;
        std::fs::write(r, made.reuv.iter().map(|l| format!("{l}\r\n")).collect::<String>())?;
    }
    Ok(made)
}

pub fn run(a: &Args) -> Result<()> {
    let t = Instant::now();
    let p = a.profile.open()?;
    if a.render_probe {
        let dyndolod = a.dyndolod.clone().unwrap_or_else(|| a.profile.instance.join(r"tools\DynDOLOD"));
        for o in crate::texgen_rendered::discover(&dyndolod) {
            let g = a.golden.as_ref().and_then(|g| std::fs::read(g.join(&o.out)).ok()).and_then(|b| lodforge_dds::parse(&b).ok()).map(|i| format!("{}x{}", i.width, i.height)).unwrap_or_else(|| "-".into());
            println!("{:<52} scale {:>5} alpha {} plane x {:.1}..{:.1} z {:.1}..{:.1} ({:.1} x {:.1})  golden {g}", o.out, o.scale, o.keep_alpha, o.plane[0], o.plane[1], o.plane[2], o.plane[3], o.plane[1] - o.plane[0], o.plane[3] - o.plane[2]);
        }
        return Ok(());
    }
    if !a.probe.is_empty() {
        for t in &a.probe {
            let n = stitch::norm(t);
            println!("{n}: loose {:?}", p.vfs.loose_path(&n));
            for (i, bsa) in p.vfs.archives().iter().enumerate() {
                if bsa.files.iter().any(|x| x.path == n) {
                    println!("  in archive #{i} {}", bsa.path.display());
                }
            }
            println!("  winner: {:?}", p.vfs.origin(&n).map(|o| format!("{o:?}")));
            if let Some(b) = p.vfs.read(&n)? {
                let info = lodforge_dds::parse(&b)?;
                println!("  {}x{} {:?} mips {}", info.width, info.height, info.format, info.mip_count);
                for l in 0..info.mip_count.min(12) {
                    let img = lodforge_dds::decode(&b, &info, l)?;
                    let n = (img.rgba.len() / 4) as f64;
                    let mut m = [0f64; 4];
                    for px in img.rgba.chunks_exact(4) {
                        for k in 0..4 {
                            m[k] += f64::from(px[k]) / n;
                        }
                    }
                    println!("  mip {l} {}x{} mean {:.1} {:.1} {:.1} {:.1}", img.width, img.height, m[0], m[1], m[2], m[3]);
                }
            }
        }
        return Ok(());
    }
    let dyndolod = a.dyndolod.clone().unwrap_or_else(|| a.profile.instance.join(r"tools\DynDOLOD"));
    let rules = load_rules(&p, &dyndolod);
    println!("rules: {} ({} destinations, {} shadowed), copy rules {}", rules.rules.len(), stitch::group(&rules.rules).len(), rules.shadowed.len(), rules.copies.len());
    let src = Sources::new(&p, &rules.copies, true);
    println!("copy aliases active: {}", src.alias.len());
    let prm = StitchParams { bias: a.bias, ..StitchParams::default() };
    let gpu = if a.out.is_some() || a.rendered { Some(lodforge_gpu::Gpu::new()?) } else { None };
    let mut stats = Stats::default();
    let mut ours = HashSet::new();
    let mut handle = |o: Out| -> Result<()> {
        ours.insert(o.path.clone());
        if let (Some(out), Some(gpu)) = (&a.out, &gpu) {
            let path = out.join(&o.path);
            std::fs::create_dir_all(path.parent().unwrap())?;
            std::fs::write(&path, encode(gpu, &o)?)?;
        }
        if let Some(g) = &a.golden {
            stats.add(g, &o, a.verbose);
        }
        Ok(())
    };
    let mut made = make_all(&src, &rules, &prm, a.filter.as_deref(), &mut handle)?;
    if let (true, Some(gpu)) = (a.rendered, &gpu) {
        let t = Instant::now();
        let renderer = lodforge_texgen::Renderer::new(gpu);
        let (rs, skipped) = crate::texgen_rendered::render_all(&p, gpu, &renderer, &dyndolod, a.filter.as_deref(), a.yaw, true)?;
        made.rendered = rs.len();
        made.rendered_skipped = skipped;
        for r in rs {
            handle(Out { path: stitch::normal_path(&r.path), w: r.w, h: r.h, rgba: r.normal })?;
            handle(Out { path: r.path, w: r.w, h: r.h, rgba: r.rgba })?;
        }
        println!("rendered: {} textures, {} NIFs left out, {:.1} s", made.rendered, made.rendered_skipped.len(), t.elapsed().as_secs_f64());
        for s in &made.rendered_skipped {
            println!("  left out {s}");
        }
    }
    drop(handle);
    println!(
        "stitched: {} atlases, {} PBR twins, {} full (x) textures, {} temporaries, {} _n; {} skipped; {:.1} s",
        made.atlases,
        made.pbr,
        made.full,
        made.temps,
        made.normals,
        made.skipped.len(),
        t.elapsed().as_secs_f64()
    );
    for s in made.skipped.iter().take(30) {
        println!("  skipped {s}");
    }
    if let Some(out) = &a.out {
        let r = out.join(r"DynDOLOD\Cache\DynDOLOD_SSE_re-uv-miniatlas.txt");
        std::fs::create_dir_all(r.parent().unwrap())?;
        std::fs::write(&r, made.reuv.iter().map(|l| format!("{l}\r\n")).collect::<String>())?;
    }
    if let Some(g) = &a.golden {
        let gf = golden_files(g);
        let covered = gf.iter().filter(|f| ours.contains(*f)).count();
        println!("golden: {} stitched-area files, {} made by us ({:.1}%)", gf.len(), covered, 100.0 * covered as f64 / gf.len().max(1) as f64);
        if a.filter.is_none() {
            let missing: Vec<_> = gf.iter().filter(|f| !ours.contains(*f)).collect();
            println!("  golden files we do not make: {}", missing.len());
            for m in missing.iter().take(200) {
                println!("    {m}");
            }
        }
        stats.print();
    }
    if let Some(gr) = &a.golden_reuv {
        let gold: HashSet<String> = std::fs::read_to_string(gr)?.lines().map(|l| l.trim().to_ascii_lowercase()).filter(|l| !l.is_empty()).collect();
        let ourr: HashSet<String> = made.reuv.iter().map(|l| l.to_ascii_lowercase()).collect();
        let both = gold.intersection(&ourr).count();
        println!("re-uv-miniatlas: golden {} rows, ours {}, identical {}", gold.len(), ourr.len(), both);
        for l in gold.difference(&ourr).take(8) {
            println!("  golden only: {l}");
        }
        for l in ourr.difference(&gold).take(8) {
            println!("  ours only:   {l}");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mini_atlas_export_names_and_text() {
        // Golden E2E (AddMiniAtlas log, MiniAtlasExport_Tamriel).
        assert_eq!(mini_atlas_name(r"textures\lod\dirtcliffs01lod.dds", r"textures\landscape\dirtcliffs\dirtcliffs01.dds", r"textures\pbr\landscape\volcanictundradirtcliffs01.dds"), r"textures\lod\volcanictundradirtcliffs01pbr_lod.dds");
        assert_eq!(mini_atlas_name(r"textures\lod\sfloorhouse01lod.dds", r"textures\architecture\solitude\sfloorhouse01.dds", r"textures\cotn\architecture\bricks01.dds"), r"textures\lod\bricks01lod.dds");
        // The atlas name lacks the base's: textures\lod\<texture>lod.dds.
        assert_eq!(mini_atlas_name(r"textures\lod\roadslod.dds", r"textures\landscape\roads\road01.dds", r"textures\_resourcepack\landscape\roads\road01desertlumpy01_d.dds"), r"textures\lod\road01desertlumpy01_dlod.dds");
        let rules = stitch::parse_rules(&[r"0", "1", "1", "1", r"textures\cotn\landscape\groundmud01.dds", "0.5", "0.5", "0.5", "0", r"textures\lod\groundmud01lod.dds", "1", "1"].join("\t"), false);
        let text = mini_atlas_export_text(&[Atlas { dest: r"textures\lod\groundmud01lod.dds".into(), rules }], Path::new(r"D:\Out\Data"));
        let want = [r"0", "1", "1", "1", r"textures\cotn\landscape\groundmud01.dds", "0.500", "0.500", "0.500", "0.000", r"D:\Out\Data\textures\lod\groundmud01lod.dds", "1.000", "1.000"].join("\t");
        assert_eq!(text, format!("{want}\r\n\r\n"));
    }
}
