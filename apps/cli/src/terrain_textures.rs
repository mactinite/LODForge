//! `lodforge terrain-textures`: GPU bake of all terrain LOD textures (diffuse and normal maps) of
//! a worldspace, following the reverse-engineered xLODGen pipeline (lodforge_terrain::spec on the
//! CPU, terrain_spec.wgsl on the GPU). Each cell is rendered once and downscaled into every level.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use rayon::prelude::*;

use lodforge_esp::{land_textures, scan_worldspaces};
use lodforge_gpu::terrain_spec::{LevelSpec, TextureKind as GpuKind, TOP_LEVEL};
use lodforge_gpu::{Gpu, RgbaImage};
use lodforge_seasons::{affected_tiles, effective_swaps, load_swaps, tile_of, Season, TextureKind};
use lodforge_terrain::bake::{decode_lands, texture_path, TextureCache};
use lodforge_terrain::gpu_spec::{top_tile, SpecTextureSet};
use lodforge_terrain::spec::{Kind, ShrunkTextures, SpecBaker, SpecParams};
use lodforge_terrain::{compare, Image};

use crate::profile::{Profile, ProfileArgs};

#[derive(clap::Args)]
pub struct Args {
    #[command(flatten)]
    pub profile: ProfileArgs,
    #[arg(long, default_value = "Tamriel")]
    pub world: String,
    /// LOD levels to build.
    #[arg(long, value_delimiter = ',', default_value = "4,8,16,32")]
    pub levels: Vec<i32>,
    /// Texture kinds to build: diffuse, normal.
    #[arg(long, value_delimiter = ',', default_value = "diffuse,normal")]
    pub kinds: Vec<String>,
    /// Output Data folder (textures\terrain\<world>\ is created inside).
    #[arg(long, default_value = "bench/out")]
    pub out: PathBuf,
    /// Build a season variant (SPR, SUM, AUT, WIN) with that season's LTEX swaps applied.
    #[arg(long)]
    pub season: Option<String>,
    /// With --season: bake only tiles the season changes; reuse the rest from this default build.
    #[arg(long)]
    pub delta_from: Option<PathBuf>,
    /// Golden run directory: score every written tile against the golden texture.
    #[arg(long)]
    pub golden: Option<PathBuf>,
    /// Only build the LOD32 tile whose south-west cell is x,y (debugging).
    #[arg(long, value_parser = crate::terrain_bake::parse_xy, allow_hyphen_values = true)]
    pub top: Option<(i32, i32)>,
    /// Rendered cells per GPU batch.
    #[arg(long, default_value_t = 96)]
    pub batch: usize,
    /// Which textures get mip chains: `full` (every level and kind, the default), `golden` (the
    /// xLODGen golden run's saved settings: LOD4 diffuse only) or `none`.
    #[arg(long, value_enum, default_value = "full", alias = "terrain-mips")]
    pub mips: TerrainMips,
}

/// Which terrain LOD textures get a mip chain. xLODGen decides this per level and kind
/// (`TerrainDiffuseMipMapLODn`, `TerrainNormalMipMapLODn` in `Plugins.sseviewsettings`); its
/// defaults, used by the golden run, mip only the LOD4 diffuse. Un-mipped terrain LOD textures
/// alias at distance (sparkle), so LODForge mips everything by default. "Raise steepness" (an
/// xLODGen option that only applies with normal mips) stays off: its formula is not analysed.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, clap::ValueEnum)]
pub enum TerrainMips {
    /// Full 2x2 box chains down to 1x1 on diffuse and normal maps of every level.
    #[default]
    Full,
    /// The golden xLODGen settings: `TerrainDiffuseMipMapLOD4=1`, every other
    /// `TerrainDiffuseMipMapLODn` / `TerrainNormalMipMapLODn` 0.
    Golden,
    /// Top mip only everywhere.
    None,
}

impl TerrainMips {
    /// Whether a `level` texture of `kind` gets a mip chain.
    pub fn enabled(self, level: i32, kind: Kind) -> bool {
        match self {
            TerrainMips::Full => true,
            TerrainMips::Golden => kind == Kind::Diffuse && level == 4,
            TerrainMips::None => false,
        }
    }
}

/// Per-level, per-kind output settings: sizes and gamma as the golden xLODGen run, mips per
/// [TerrainMips].
struct LevelSettings {
    size: u32,
    gamma: f32,
    mips: bool,
}

fn level_settings(level: i32, kind: Kind, mips: TerrainMips) -> LevelSettings {
    let size = if level == 4 { 1024 } else { 512 };
    let mips = mips.enabled(level, kind);
    match kind {
        Kind::Diffuse => LevelSettings { size, gamma: if level == 32 { 1.0 } else { 1.25 }, mips },
        Kind::Normal => LevelSettings { size, gamma: 1.0, mips },
    }
}

/// The default landscape texture's LTEX (Skyrim.esm 0x00000C16); seasons may swap it.
const DEFAULT_LTEX: u32 = 0x0000_0C16;

#[derive(Default)]
struct Stages {
    reuse: u128,
    pack: u128,
    gpu: u128,
    encode: u128,
    write: u128,
}

fn tile_name(world: &str, level: i32, t: (i32, i32), season: Option<&str>, kind: Kind) -> String {
    let n = if kind == Kind::Normal { "_n" } else { "" };
    match season {
        Some(s) => format!("{world}.{level}.{}.{}.{s}{n}.dds", t.0, t.1),
        None => format!("{world}.{level}.{}.{}{n}.dds", t.0, t.1),
    }
}

/// 2x2 box-filtered mip chain (excluding the top level) down to 1x1, on 8-bit RGBA with rounding.
fn mip_chain(size: u32, rgba: &[u8]) -> Vec<(u32, Vec<u8>)> {
    let mut out: Vec<(u32, Vec<u8>)> = Vec::new();
    let mut n = size as usize;
    let mut src = rgba.to_vec();
    while n > 1 {
        let m = n / 2;
        let mut dst = vec![0u8; m * m * 4];
        for y in 0..m {
            let (r0, r1) = (2 * y * n * 4, (2 * y + 1) * n * 4);
            for x in 0..m {
                for c in 0..4 {
                    let i = 2 * x * 4 + c;
                    let s = u16::from(src[r0 + i]) + u16::from(src[r0 + i + 4]) + u16::from(src[r1 + i]) + u16::from(src[r1 + i + 4]);
                    dst[(y * m + x) * 4 + c] = ((s + 2) / 4) as u8;
                }
            }
        }
        out.push((m as u32, dst.clone()));
        src = dst;
        n = m;
    }
    out
}

/// BC7 DDS files (top mip + optional mips) for RGBA images, encoded on the GPU in one batch.
fn encode_dds(gpu: &Gpu, images: &[(u32, &[u8])], mips: bool) -> Result<Vec<Vec<u8>>> {
    let chains: Vec<Vec<(u32, Vec<u8>)>> = if mips { images.par_iter().map(|(s, d)| mip_chain(*s, d)).collect() } else { vec![Vec::new(); images.len()] };
    let mut flat: Vec<RgbaImage> = Vec::new();
    for ((s, d), chain) in images.iter().zip(&chains) {
        flat.push(RgbaImage { width: *s, height: *s, rgba: d });
        flat.extend(chain.iter().map(|(cs, cd)| RgbaImage { width: *cs, height: *cs, rgba: cd }));
    }
    let mut encoded = gpu.encode_bc7(&flat)?.into_iter();
    Ok(images
        .iter()
        .zip(&chains)
        .map(|((s, _), chain)| {
            let levels: Vec<Vec<u8>> = (0..=chain.len()).map(|_| encoded.next().unwrap_or_default()).collect();
            lodforge_dds::write_bc7(*s, *s, &levels)
        })
        .collect())
}

fn parse_kind(s: &str) -> Result<Kind> {
    match s.trim().to_ascii_lowercase().as_str() {
        "diffuse" | "d" => Ok(Kind::Diffuse),
        "normal" | "n" => Ok(Kind::Normal),
        other => bail!("unknown texture kind {other} (diffuse, normal)"),
    }
}

pub fn run(a: &Args) -> Result<()> {
    let p = a.profile.open()?;
    run_with(&p, a)
}

pub fn run_with(p: &Profile, a: &Args) -> Result<()> {
    let total = Instant::now();
    let lo = &p.load_order;
    let worlds = scan_worldspaces(lo)?;
    let Some(world) = worlds.iter().find(|w| w.editor_id.eq_ignore_ascii_case(&a.world)) else { bail!("worldspace {} not found", a.world) };
    let ltex = land_textures(lo)?;
    for &l in &a.levels {
        if ![4, 8, 16, 32].contains(&l) {
            bail!("unsupported LOD level {l}");
        }
    }
    let kinds: Vec<Kind> = a.kinds.iter().map(|k| parse_kind(k)).collect::<Result<_>>()?;

    let t = Instant::now();
    let all_cells: Vec<(i32, i32)> = world.lands.values().map(|l| l.cell).collect();
    let mut lands = decode_lands(lo, world, all_cells.iter().copied())?;
    let decode_ms = t.elapsed().as_millis();

    let season = match &a.season {
        Some(s) => Some(Season::ALL.into_iter().find(|x| x.code().eq_ignore_ascii_case(s)).with_context(|| format!("unknown season {s}"))?),
        None => None,
    };
    let season_code = season.map(|s| s.code());
    let mut spec_params = SpecParams::default();
    // Per kind: tiles to bake when building a delta; None = everything.
    let mut only: HashMap<Kind, Option<HashSet<(i32, i32, i32)>>> = kinds.iter().map(|k| (*k, None)).collect();
    if let Some(season) = season {
        let edids: HashMap<String, u32> = ltex.iter().map(|(id, t)| (t.editor_id.to_ascii_lowercase(), *id)).collect();
        let swaps = load_swaps(&p.vfs, lo, &edids)?;
        let swapped = swaps.land_textures(season).cloned().unwrap_or_default();
        if let Some(d) = swapped.get(&DEFAULT_LTEX).and_then(|id| ltex.get(id)) {
            if !d.diffuse.is_empty() {
                spec_params.default_diffuse = texture_path(&d.diffuse);
            }
            if !d.normal.is_empty() {
                spec_params.default_normal = texture_path(&d.normal);
            }
        }
        for land in lands.values_mut() {
            for q in &mut land.quadrants {
                if let Some(b) = q.base.as_mut() {
                    *b = *swapped.get(b).unwrap_or(b);
                }
                for l in &mut q.layers {
                    l.ltex = *swapped.get(&l.ltex).unwrap_or(&l.ltex);
                }
            }
        }
        if a.delta_from.is_some() {
            // Plan on the unswapped textures: which cells use an LTEX whose texture of this kind changes.
            let cells = lodforge_seasons::cell_textures(lo, world)?;
            for &kind in &kinds {
                let tk = if kind == Kind::Normal { TextureKind::Normal } else { TextureKind::Diffuse };
                let eff = effective_swaps(&swapped, &ltex, tk);
                if eff.contains_key(&DEFAULT_LTEX) {
                    continue; // the default texture changes: everything is rebuilt
                }
                let mut set = HashSet::new();
                for &level in &a.levels {
                    for t in affected_tiles(&cells, &eff, level, 0) {
                        set.insert((level, t.0, t.1));
                    }
                }
                only.insert(kind, Some(set));
            }
        }
    }

    let gpu = Gpu::new()?;
    let out_dir = a.out.join("textures").join("terrain").join(&world.editor_id);
    std::fs::create_dir_all(&out_dir)?;
    let mut tops: BTreeSet<(i32, i32)> = lands.keys().map(|&c| tile_of(c, TOP_LEVEL as i32)).collect();
    if let Some(t) = a.top {
        tops.retain(|x| *x == t);
    }
    println!("{}: {} cells decoded in {decode_ms} ms; GPU {}", world.editor_id, lands.len(), gpu.adapter_info.name);

    let cache = TextureCache::new(&p.vfs);
    let mut scores: Vec<(Kind, i32, (i32, i32), f32)> = Vec::new();
    for &kind in &kinds {
        let kt = Instant::now();
        // Shrunk landscape textures (24 repeats per cell; texPx from the largest pixels-per-cell).
        let max_ppc = a.levels.iter().map(|&l| level_settings(l, kind, a.mips).size / l as u32).max().unwrap_or(64);
        let tex_px = SpecParams::tex_px_for(max_ppc);
        let params = SpecParams { tex_px, ..spec_params.clone() };
        let set = SpecTextureSet::build(&p.vfs, &ltex, &params.default_path(kind), tex_px, kind);
        let gtex = gpu.upload_spec_textures(tex_px, &set.images);
        let shrunk = ShrunkTextures::new(&cache, tex_px, kind);
        let baker = SpecBaker { textures: &shrunk, ltex: &ltex, params: params.clone() };

        // Diffuse default tiles are all equivalent: bake one per level on the CPU. Normals have
        // no default size (golden), so their default tiles are rendered like any other.
        let mut default_dds: HashMap<i32, Vec<u8>> = HashMap::new();
        if kind == Kind::Diffuse {
            for &level in &a.levels {
                let ls = level_settings(level, kind, a.mips);
                let b = SpecBaker { textures: &shrunk, ltex: &ltex, params: SpecParams { gamma: ls.gamma, ..params.clone() } };
                let (img, _) = b.bake_tile(&HashMap::new(), (0, 0), level, ls.size / level as u32);
                let rgba = img.to_rgba8();
                default_dds.insert(level, encode_dds(&gpu, &[(img.width, &rgba)], ls.mips)?.remove(0));
            }
        }
        let levels: Vec<LevelSpec> = a
            .levels
            .iter()
            .map(|&l| {
                let ls = level_settings(l, kind, a.mips);
                LevelSpec { level: l as u32, ppc: ls.size / l as u32, gamma: ls.gamma, brightness: 0.0, contrast: 0.0 }
            })
            .collect();
        let gkind = if kind == Kind::Normal { GpuKind::Normal } else { GpuKind::Diffuse };
        let prep_ms = kt.elapsed().as_millis();

        let mut st = Stages::default();
        let mut counts: HashMap<i32, (usize, usize, usize)> = HashMap::new(); // level -> (baked, default, reused)
        let bake_t = Instant::now();
        for &top in &tops {
            let mut wanted: Vec<(i32, (i32, i32))> = Vec::new();
            let mut reuse: Vec<(i32, (i32, i32))> = Vec::new();
            for &level in &a.levels {
                let per = TOP_LEVEL as i32 / level;
                for ty in 0..per {
                    for tx in 0..per {
                        let t = (top.0 + tx * level, top.1 + ty * level);
                        match &only[&kind] {
                            Some(s) if !s.contains(&(level, t.0, t.1)) => reuse.push((level, t)),
                            _ => wanted.push((level, t)),
                        }
                    }
                }
            }
            let t0 = Instant::now();
            if let (Some(src), Some(code)) = (&a.delta_from, season_code) {
                let src_dir = src.join("textures").join("terrain").join(&world.editor_id);
                reuse.par_iter().try_for_each(|&(level, t)| -> Result<()> {
                    let (from, to) = (src_dir.join(tile_name(&world.editor_id, level, t, None, kind)), out_dir.join(tile_name(&world.editor_id, level, t, Some(code), kind)));
                    // Hard link when possible (same volume): no data is copied.
                    let _ = std::fs::remove_file(&to);
                    if std::fs::hard_link(&from, &to).is_err() {
                        std::fs::copy(&from, &to)?;
                    }
                    Ok(())
                })?;
                for &(level, _) in &reuse {
                    counts.entry(level).or_default().2 += 1;
                }
            }
            st.reuse += t0.elapsed().as_millis();
            if wanted.is_empty() {
                continue;
            }

            let t0 = Instant::now();
            let packed = top_tile(&baker, &set, &lands, top, a.batch, kind);
            st.pack += t0.elapsed().as_millis();
            let constant_default = |level: i32, t: (i32, i32)| kind == Kind::Diffuse && packed.tile_is_default(level, (t.0 - top.0) / level, (t.1 - top.1) / level);
            let needs_gpu = wanted.iter().any(|&(level, t)| !constant_default(level, t));
            let t0 = Instant::now();
            let images = if needs_gpu { gpu.bake_top_tile(&gtex, gkind, 1.0, &levels, &packed.batches)? } else { Vec::new() };
            st.gpu += t0.elapsed().as_millis();

            for &level in &a.levels {
                let ls = level_settings(level, kind, a.mips);
                let mine: Vec<(i32, i32)> = wanted.iter().filter(|(l, _)| *l == level).map(|(_, t)| *t).collect();
                let (defaults, full): (Vec<_>, Vec<_>) = mine.iter().partition(|t| constant_default(level, **t));
                let t0 = Instant::now();
                let mut files: Vec<((i32, i32), Vec<u8>)> = defaults.iter().map(|&&t| (t, default_dds[&level].clone())).collect();
                if !full.is_empty() {
                    let img = images.iter().find(|i| i.level == level as u32).context("level image missing")?;
                    let tiles: Vec<(u32, &[u8])> = full.iter().map(|t| (img.tile_size, img.tile(((t.0 - top.0) / level) as u32, ((t.1 - top.1) / level) as u32))).collect();
                    for (t, dds) in full.iter().zip(encode_dds(&gpu, &tiles, ls.mips)?) {
                        files.push((**t, dds));
                    }
                }
                st.encode += t0.elapsed().as_millis();
                let c = counts.entry(level).or_default();
                c.0 += full.len();
                c.1 += defaults.len();

                let t0 = Instant::now();
                let level_scores: Vec<Option<(Kind, i32, (i32, i32), f32)>> = files
                    .par_iter()
                    .map(|(t, dds)| -> Result<Option<(Kind, i32, (i32, i32), f32)>> {
                        std::fs::write(out_dir.join(tile_name(&world.editor_id, level, *t, season_code, kind)), dds)?;
                        let Some(g) = &a.golden else { return Ok(None) };
                        let gp = g.join("output").join("Textures").join("Terrain").join(&world.editor_id).join(tile_name(&world.editor_id, level, *t, season_code, kind));
                        let Ok(gb) = std::fs::read(&gp) else { return Ok(None) };
                        let (gi, oi) = (lodforge_dds::parse(&gb)?, lodforge_dds::parse(dds)?);
                        if gi.width != oi.width {
                            return Ok(Some((kind, level, *t, -1.0)));
                        }
                        let (gd, od) = (lodforge_dds::decode(&gb, &gi, 0)?, lodforge_dds::decode(dds, &oi, 0)?);
                        let m = compare(&Image::from_rgba8(od.width, od.height, &od.rgba), &Image::from_rgba8(gd.width, gd.height, &gd.rgba));
                        Ok(Some((kind, level, *t, m.psnr)))
                    })
                    .collect::<Result<_>>()?;
                scores.extend(level_scores.into_iter().flatten());
                st.write += t0.elapsed().as_millis();
            }
        }
        let bake_ms = bake_t.elapsed().as_millis();
        println!("{kind:?}: textures {} shrunk to {tex_px}px + setup in {prep_ms} ms", set.images.len());
        for &level in &a.levels {
            let (b, d, r) = counts.get(&level).copied().unwrap_or_default();
            println!("  LOD{level:<2}: {} tiles: {b} baked, {d} default, {r} reused", b + d + r);
        }
        println!(
            "  stages (ms): reuse {} | pack {} | gpu render+downscale {} | bc7+mips {} | write{} {}  — {} top tiles in {bake_ms} ms",
            st.reuse,
            st.pack,
            st.gpu,
            st.encode,
            if a.golden.is_some() { "+score" } else { "" },
            st.write,
            tops.len()
        );
    }

    if a.golden.is_some() {
        for &kind in &kinds {
            for &level in &a.levels {
                let s: Vec<&(Kind, i32, (i32, i32), f32)> = scores.iter().filter(|x| x.0 == kind && x.1 == level).collect();
                let mism = s.iter().filter(|x| x.3 < 0.0).count();
                let mut ok: Vec<&&(Kind, i32, (i32, i32), f32)> = s.iter().filter(|x| x.3 >= 0.0).collect();
                if ok.is_empty() {
                    continue;
                }
                let mean = ok.iter().map(|x| x.3).sum::<f32>() / ok.len() as f32;
                ok.sort_by(|x, y| x.3.total_cmp(&y.3));
                println!(
                    "{kind:?} LOD{level:<2} vs golden (BC7 vs BC7): {} compared, mean PSNR {mean:.2} dB, min {:.2} dB, {mism} size mismatches; worst {}",
                    ok.len(),
                    ok[0].3,
                    ok.iter().take(4).map(|x| format!("{},{}", x.2 .0, x.2 .1)).collect::<Vec<_>>().join(" ")
                );
            }
        }
    }
    println!("total {} ms (output: {})", total.elapsed().as_millis(), out_dir.display());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mip_settings_per_mode() {
        for level in [4, 8, 16, 32] {
            for kind in [Kind::Diffuse, Kind::Normal] {
                assert!(level_settings(level, kind, TerrainMips::Full).mips, "full {level} {kind:?}");
                assert!(!level_settings(level, kind, TerrainMips::None).mips, "none {level} {kind:?}");
                // The golden xLODGen run: TerrainDiffuseMipMapLOD4=1, everything else 0.
                assert_eq!(level_settings(level, kind, TerrainMips::Golden).mips, kind == Kind::Diffuse && level == 4, "golden {level} {kind:?}");
            }
        }
        // Sizes and gamma do not depend on the mip mode.
        assert_eq!(level_settings(4, Kind::Diffuse, TerrainMips::Full).size, 1024);
        assert_eq!(level_settings(8, Kind::Normal, TerrainMips::Golden).size, 512);
        assert_eq!(level_settings(32, Kind::Diffuse, TerrainMips::None).gamma, 1.0);
        assert_eq!(level_settings(16, Kind::Diffuse, TerrainMips::Full).gamma, 1.25);
    }

    #[test]
    fn box_mip_chain_reaches_one_texel() {
        // 8x8 with a 2x2 checker of (255, 0) in R: each 2x2 box averages to 128 (rounded).
        let size = 8u32;
        let mut top = vec![0u8; (size * size * 4) as usize];
        for y in 0..size as usize {
            for x in 0..size as usize {
                let v = if (x + y) % 2 == 0 { 255 } else { 0 };
                top[(y * size as usize + x) * 4] = v;
                top[(y * size as usize + x) * 4 + 3] = 255;
            }
        }
        let chain = mip_chain(size, &top);
        assert_eq!(chain.iter().map(|(s, _)| *s).collect::<Vec<_>>(), [4, 2, 1]);
        assert!(chain.iter().all(|(s, d)| d.len() == (*s * *s * 4) as usize));
        assert_eq!(chain[0].1[0], 128);
        assert_eq!(chain[2].1[3], 255);
        // A 1024 top mip gets the 10 further levels xLODGen's texconv writes (11 in the header).
        assert_eq!(mip_chain(1024, &vec![0u8; 1024 * 1024 * 4]).len(), 10);
        assert_eq!(mip_chain(1, &[0, 0, 0, 0]).len(), 0);
    }

    #[test]
    fn dds_header_mips_match_payload() {
        // The DDS writer's header mip count and payload are what validate_output checks: one
        // 16-byte BC7 block per 4x4 (or smaller) mip.
        let size = 32u32;
        let chain = mip_chain(size, &vec![0u8; (size * size * 4) as usize]);
        let levels: Vec<Vec<u8>> = std::iter::once(size).chain(chain.iter().map(|(s, _)| *s)).map(|s| vec![0u8; (s.div_ceil(4) * s.div_ceil(4) * 16) as usize]).collect();
        let dds = lodforge_dds::write_bc7(size, size, &levels);
        let info = lodforge_dds::parse(&dds).unwrap();
        assert_eq!(info.mip_count, 6);
        let mips = info.mips();
        let end = mips.last().map(|m| m.0 + m.1).unwrap();
        assert_eq!(dds.len(), end);
        assert_eq!(mips.iter().map(|m| m.1).collect::<Vec<_>>(), [1024, 256, 64, 16, 16, 16]);
        // One mip: no mip flags, header count 1.
        let one = lodforge_dds::write_bc7(size, size, &levels[..1]);
        assert_eq!(lodforge_dds::parse(&one).unwrap().mip_count, 1);
    }
}
