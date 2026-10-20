//! `lodforge terrain-bake`: bake one terrain LOD diffuse tile with the CPU reference
//! (lodforge_terrain::spec) and compare it with a golden xLODGen tile.

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{bail, Context, Result};

use lodforge_esp::{land_textures, scan_worldspaces};
use lodforge_terrain::bake::decode_lands;
use lodforge_terrain::image::side_by_side;
use lodforge_terrain::spec::{Kind, ShrunkTextures, SpecBaker, SpecParams};
use lodforge_terrain::{compare, Image, TextureCache};

use crate::profile::ProfileArgs;

#[derive(clap::Args)]
pub struct BakeArgs {
    #[command(flatten)]
    pub profile: ProfileArgs,
    #[arg(long, default_value = "Tamriel")]
    pub world: String,
    #[arg(long, default_value_t = 4)]
    pub level: i32,
    /// South-west cell of the tile, "x,y".
    #[arg(long, value_parser = parse_xy, allow_hyphen_values = true)]
    pub tile: (i32, i32),
    /// Golden run directory to compare with.
    #[arg(long)]
    pub golden: Option<PathBuf>,
    /// Season suffix of the golden file to compare with (SPR, SUM, AUT, WIN).
    #[arg(long)]
    pub season: Option<String>,
    #[arg(long, default_value = "bench/terrain")]
    pub out: PathBuf,
    /// Output pixels per cell (golden: LOD4 256, LOD8 64, LOD16 32, LOD32 16).
    #[arg(long, default_value_t = 256)]
    pub pixels_per_cell: u32,
    #[arg(long, default_value_t = 1.25)]
    pub gamma: f32,
    /// Bake the normal map (_n) instead of the diffuse.
    #[arg(long)]
    pub normal: bool,
}

pub fn parse_xy(s: &str) -> Result<(i32, i32), String> {
    let (x, y) = s.split_once(',').ok_or("expected x,y")?;
    Ok((x.trim().parse().map_err(|e| format!("{e}"))?, y.trim().parse().map_err(|e| format!("{e}"))?))
}

fn load_golden(dir: &Path, world: &str, level: i32, tile: (i32, i32), season: Option<&str>, normal: bool) -> Result<Image> {
    let mut season = season.map(|s| format!(".{s}")).unwrap_or_default();
    if normal {
        season.push_str("_n");
    }
    let path = dir.join("output").join("Textures").join("Terrain").join(world).join(format!("{world}.{level}.{}.{}{season}.dds", tile.0, tile.1));
    let bytes = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
    let info = lodforge_dds::parse(&bytes)?;
    let img = lodforge_dds::decode(&bytes, &info, 0)?;
    Ok(Image::from_rgba8(img.width, img.height, &img.rgba))
}

pub fn run(a: &BakeArgs) -> Result<()> {
    let p = a.profile.open()?;
    let lo = &p.load_order;
    let worlds = scan_worldspaces(lo)?;
    let Some(world) = worlds.iter().find(|w| w.editor_id.eq_ignore_ascii_case(&a.world)) else { bail!("worldspace {} not found", a.world) };
    let ltex = land_textures(lo)?;
    let cells = (0..a.level).flat_map(|dy| (0..a.level).map(move |dx| (a.tile.0 + dx, a.tile.1 + dy)));
    let lands = decode_lands(lo, world, cells)?;
    let cache = TextureCache::new(&p.vfs);
    println!("tile {}.{}.{}.{}: {} cells with LAND", world.editor_id, a.level, a.tile.0, a.tile.1, lands.len());

    let t = Instant::now();
    let params = SpecParams { gamma: a.gamma, ..SpecParams::default() };
    let kind = if a.normal { Kind::Normal } else { Kind::Diffuse };
    let shrunk = ShrunkTextures::new(&cache, params.tex_px, kind);
    let (img, default) = SpecBaker { textures: &shrunk, ltex: &ltex, params }.bake_tile_kind(&lands, a.tile, a.level, a.pixels_per_cell, kind);
    println!("baked {}x{} in {} ms (default tile: {default}, {} source textures)", img.width, img.height, t.elapsed().as_millis(), cache.loaded());

    std::fs::create_dir_all(&a.out)?;
    let stem = format!("{}.{}.{}.{}{}", world.editor_id, a.level, a.tile.0, a.tile.1, if a.normal { "_n" } else { "" });
    img.write_png(&a.out.join(format!("{stem}.ours.png")))?;
    if let Some(g) = &a.golden {
        let g = load_golden(g, &world.editor_id, a.level, a.tile, a.season.as_deref(), a.normal)?;
        if (g.width, g.height) != (img.width, img.height) {
            bail!("size mismatch: ours {}x{}, golden {}x{}", img.width, img.height, g.width, g.height);
        }
        let m = compare(&img, &g);
        println!("vs golden {}x{}: PSNR {:.2} dB, MAE {:.1}, mean ours {:.0?} golden {:.0?}", g.width, g.height, m.psnr, m.mae, m.mean_a, m.mean_b);
        if g.width >= 64 {
            // Low-pass view: separates structural error from texel-level noise and BC7 error.
            let lp = compare(&img.downsample(4), &g.downsample(4));
            println!("  4x box-downsampled: PSNR {:.2} dB, MAE {:.1}", lp.psnr, lp.mae);
        }
        // Our output through the same BC7 encoding tier as the golden file.
        if let Ok(gpu) = lodforge_gpu::Gpu::new() {
            let rgba = img.to_rgba8();
            let blocks = gpu.encode_bc7(&[lodforge_gpu::RgbaImage { width: img.width, height: img.height, rgba: &rgba }])?.remove(0);
            let dds = lodforge_dds::write_bc7(img.width, img.height, &[blocks]);
            let info = lodforge_dds::parse(&dds)?;
            let dec = lodforge_dds::decode(&dds, &info, 0)?;
            let bm = compare(&Image::from_rgba8(dec.width, dec.height, &dec.rgba), &g);
            let self_err = compare(&Image::from_rgba8(dec.width, dec.height, &dec.rgba), &img);
            println!("  ours BC7-encoded vs golden: PSNR {:.2} dB (BC7 round trip of ours alone: {:.2} dB)", bm.psnr, self_err.psnr);
        }
        g.write_png(&a.out.join(format!("{stem}.golden.png")))?;
        side_by_side(&img, &g).write_png(&a.out.join(format!("{stem}.side.png")))?;
        println!("wrote {}", a.out.join(format!("{stem}.side.png")).display());
    }
    Ok(())
}
