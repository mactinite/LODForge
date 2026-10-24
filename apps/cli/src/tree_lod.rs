//! `lodforge tree-lod`: billboard tree LOD (.lst, .btt, tree atlas) for a worldspace from the
//! load order (`lodforge-treelod`, port of xEdit's tree LOD).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result, bail};

use lodforge_esp::scan_worldspaces;
use lodforge_gpu::{Gpu, RgbaImage};
use lodforge_treelod::{Files, Rules, TreeLod, build_with, read_tree_bases};

use crate::profile::{Profile, ProfileArgs};

#[derive(clap::Args)]
pub struct Args {
    #[command(flatten)]
    pub profile: ProfileArgs,
    #[arg(long, default_value = "Tamriel")]
    pub world: String,
    /// Also build these seasons (SPR, SUM, AUT, WIN): base swaps from Seasons\*.ini.
    #[arg(long, value_delimiter = ',')]
    pub seasons: Vec<String>,
    /// Output Data folder.
    #[arg(long, default_value = "bench/out")]
    pub out: PathBuf,
    /// Test helper: write placeholder billboards (uncompressed DDS, varied sizes, some with a
    /// `.txt`) for every tree whose billboard is missing into this Data folder, then exit.
    #[arg(long)]
    pub placeholder_billboards: Option<PathBuf>,
    /// Compare the default season with a golden xLODGen output Data folder: LST exactly, BTT
    /// references ignoring rotation (xEdit draws it at random).
    #[arg(long)]
    pub golden: Option<PathBuf>,
    /// Print every tree base whose billboard is missing (form id, model, expected texture path).
    #[arg(long)]
    pub list_missing: bool,
    /// DynDOLOD tree LOD rules from this `Edit Scripts\DynDOLOD` folder (tilt limit, child world
    /// bounds), with a block for every terrain LOD4 quad.
    #[arg(long)]
    pub dyndolod: Option<PathBuf>,
    /// References to leave out of tree LOD (the build passes DynDOLOD's dynamic LOD references
    /// and those disabled by reference rules).
    #[arg(skip)]
    pub exclude: std::collections::HashSet<u32>,
}

/// LOD4 blocks of every quad with a CELL record (with or without LAND), which DynDOLOD writes
/// even without trees: exactly its block set in all 7 worldspaces checked (Tamriel 744, Soul
/// Cairn 629, ...).
pub fn cell_blocks(world: &lodforge_esp::Worldspace, sw: (i32, i32)) -> Vec<(i32, i32)> {
    let mut blocks: Vec<(i32, i32)> = world.cells.keys().map(|&(x, y)| (sw.0 + (x - sw.0).div_euclid(4) * 4, sw.1 + (y - sw.1).div_euclid(4) * 4)).collect();
    blocks.sort_unstable();
    blocks.dedup();
    blocks
}

/// Uncompressed A8R8G8B8 DDS (no mips), readable by xEdit's image library.
fn dds_argb(w: u32, h: u32, bgra: &[u8]) -> Vec<u8> {
    let mut o = Vec::with_capacity(128 + bgra.len());
    o.extend_from_slice(b"DDS ");
    let u = |o: &mut Vec<u8>, v: u32| o.extend_from_slice(&v.to_le_bytes());
    u(&mut o, 124);
    u(&mut o, 0x100F);
    u(&mut o, h);
    u(&mut o, w);
    u(&mut o, w * 4);
    u(&mut o, 0);
    u(&mut o, 0);
    for _ in 0..11 {
        u(&mut o, 0);
    }
    u(&mut o, 32);
    u(&mut o, 0x41);
    u(&mut o, 0);
    u(&mut o, 32);
    u(&mut o, 0x00FF_0000);
    u(&mut o, 0x0000_FF00);
    u(&mut o, 0x0000_00FF);
    u(&mut o, 0xFF00_0000);
    u(&mut o, 0x1000);
    for _ in 0..4 {
        u(&mut o, 0);
    }
    o.extend_from_slice(bgra);
    o
}

fn write_placeholders(dir: &Path, t: &TreeLod) -> Result<usize> {
    let mut n = 0;
    for (k, path) in t.missing_billboards.iter().enumerate() {
        // Sizes cycle so packing sees mixed shapes; every fifth tree gets the same image as the
        // previous one (CRC de-duplication).
        let (w, h) = [(256, 512), (128, 256), (256, 256), (512, 512), (128, 128)][k % 5];
        let seed = if k % 5 == 4 && k > 0 { k - 1 } else { k };
        let (w, h) = if seed != k { [(256, 512), (128, 256), (256, 256), (512, 512), (128, 128)][seed % 5] } else { (w, h) };
        let mut bgra = vec![0u8; (w * h * 4) as usize];
        for y in 0..h {
            for x in 0..w {
                let i = ((y * w + x) * 4) as usize;
                let inside = (x as i32 - w as i32 / 2).abs() < (w as i32 / 2) * (h - y) as i32 / h as i32 + 2;
                bgra[i] = (seed * 37 % 256) as u8;
                bgra[i + 1] = (120 + seed * 11 % 120) as u8;
                bgra[i + 2] = (seed * 73 % 200) as u8;
                bgra[i + 3] = if inside { 255 } else { 0 };
            }
        }
        let p = dir.join(path);
        std::fs::create_dir_all(p.parent().unwrap())?;
        std::fs::write(&p, dds_argb(w, h, &bgra))?;
        if k % 3 == 0 {
            let txt = p.with_extension("txt");
            std::fs::write(txt, format!("[LOD]\r\nWidth={}\r\nHeight={}\r\nShiftZ={}\r\nScale={}\r\n", 300 + k * 7, 600 + k * 5, k as f32 * 0.5, 1.0 + (k % 4) as f32 * 0.25))?;
        }
        n += 1;
    }
    Ok(n)
}

struct Vfs<'a>(&'a lodforge_archive::Vfs);

impl Files for Vfs<'_> {
    fn read(&self, path: &str) -> Option<Vec<u8>> {
        self.0.read(path).ok().flatten()
    }
    fn dds_size(&self, bytes: &[u8]) -> Option<(u32, u32)> {
        lodforge_dds::parse(bytes).ok().map(|i| (i.width, i.height))
    }
}

/// Mip levels of the tree LOD atlas (DynDOLOD's `TreeLODMinMipMaps`; `TamrielTreeLOD.dds` has 8).
pub const TREE_ATLAS_MIPS: u32 = 8;

/// Largest tree LOD atlas side the game loads. A bigger `<World>TreeLOD.dds` is silently replaced
/// by the vanilla atlas from the BSA, sampled with this list's UVs: in game every tree LOD shows
/// a random vanilla billboard (summer oaks as snowy pines; skyrim-cs-base 2026-10-22, an
/// 8192x4096 atlas, fixed by dropping its top mip). DynDOLOD's atlases stay at or below 4096 too.
pub const TREE_ATLAS_MAX_DIM: u32 = 4096;

/// Top mips to drop so the atlas fits [TREE_ATLAS_MAX_DIM]. The list's UVs are relative, so the
/// smaller atlas needs no other change.
pub fn atlas_drop_levels(w: u32, h: u32) -> u32 {
    let mut n = 0;
    while (w.max(h) >> n) > TREE_ATLAS_MAX_DIM {
        n += 1;
    }
    n
}

/// Composes the billboard atlas: billboards with their see-through background set to their
/// average colour (`TreeLODCreateAverageColor`), Lanczos mips that keep each billboard's alpha
/// coverage, [TREE_ATLAS_MIPS] levels, BC7. A packing larger than [TREE_ATLAS_MAX_DIM] is
/// composed at full size and written from the first mip that fits.
pub fn atlas_dds(p: &Profile, t: &TreeLod, gpu: &Gpu) -> Result<Option<Vec<u8>>> {
    let (w, h) = t.atlas;
    if w == 0 {
        return Ok(None);
    }
    let drop = atlas_drop_levels(w, h);
    let mips = TREE_ATLAS_MIPS + drop;
    let mut tiles = Vec::new();
    let mut density = Vec::new();
    let mut chains = Vec::new();
    for s in &t.slots {
        let Some(tree) = t.trees.iter().find(|tr| tr.crc == s.crc && tr.index != -1) else { continue };
        let Some(bytes) = p.vfs.read(&tree.billboard.to_lowercase())? else { continue };
        let info = lodforge_dds::parse(&bytes)?;
        let mut img = lodforge_dds::decode(&bytes, &info, 0)?;
        crate::object_atlas::fill_average_color(&mut img.rgba);
        let (tw, th) = (s.w.min(img.width), s.h.min(img.height));
        let mut px = vec![0u8; (tw * th * 4) as usize];
        for row in 0..th {
            let src = (row * img.width * 4) as usize;
            px[(row * tw * 4) as usize..((row + 1) * tw * 4) as usize].copy_from_slice(&img.rgba[src..src + (tw * 4) as usize]);
        }
        // Each billboard is filtered on its own, so lower mips never pull in the transparent
        // padding or a neighbouring billboard.
        chains.push(crate::object_atlas::TileChain::new(s.x, s.y, tw, th, &px, mips, lodforge_dds::mips::MipKind::Color, lodforge_dds::mips::Edge::Clamp));
        tiles.push((s.x, s.y, tw, th));
        density.push(crate::object_atlas::billboard_density());
    }
    let mut levels = crate::object_atlas::assemble(w, h, mips, [0, 0, 0, 0], &chains);
    let dither = vec![crate::object_atlas::billboard_dither(); tiles.len()];
    crate::object_atlas::keep_alpha_coverage_with(&mut levels, &tiles, &density, &dither);
    levels.drain(..drop as usize);
    let (w, h) = (levels[0].0, levels[0].1);
    let images: Vec<RgbaImage<'_>> = levels.iter().map(|(w, h, p)| RgbaImage { width: *w, height: *h, rgba: p }).collect();
    let blocks = gpu.encode_bc7(&images)?;
    Ok(Some(lodforge_dds::write_bc7(w, h, &blocks)))
}

/// `<World>.lst` -> `<World>.<SEASON>.lst` (same for the atlas).
fn season_path(p: String, season: Option<&str>) -> String {
    match season {
        Some(s) => match p.rsplit_once('.') {
            Some((a, b)) => format!("{a}.{s}.{b}"),
            None => p,
        },
        None => p,
    }
}

/// Writes LST, BTTs and atlas; returns the number of files.
pub fn write(out: &Path, t: &TreeLod, season: Option<&str>, atlas: Option<&[u8]>) -> Result<usize> {
    let mut n = 0;
    let put = |rel: &str, bytes: &[u8]| -> Result<()> {
        let path = out.join(rel);
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))
    };
    let suffix = |p: String| season_path(p, season);
    if !t.types.is_empty() {
        put(&suffix(t.list_path()), &t.list_bytes())?;
        n += 1;
        if let Some(a) = atlas {
            put(&suffix(t.atlas_path()), a)?;
            n += 1;
        }
        for b in &t.blocks {
            put(&t.block_path(b, season), &TreeLod::block_bytes(b))?;
            n += 1;
        }
    }
    Ok(n)
}

/// DynDOLOD's side files: `<World>[_SEASON].txt` (`plugin;object id` of every tree LOD reference,
/// CRLF; the DLL does not read it) and `<World>TreeLOD[.SEASON].ini` next to the atlas.
fn write_dyndolod_extras(out: &Path, lo: &lodforge_esp::LoadOrder, t: &TreeLod, season: Option<&str>) -> Result<usize> {
    let w = &t.world;
    let mut txt = String::new();
    for r in t.blocks.iter().flat_map(|b| b.types.iter()).flat_map(|(_, r)| r) {
        let plugin = lodforge_treelod::defining_plugin(lo, r.form_id).unwrap_or("").to_ascii_lowercase();
        let object = r.form_id & if r.form_id >> 24 == 0xFE { 0xFFF } else { 0x00FF_FFFF };
        txt.push_str(&format!("{plugin};{object:08X}\r\n"));
    }
    let (txt_name, ini_name) = match season {
        Some(s) => (format!("{w}_{s}.txt"), format!("{w}TreeLOD.{s}.ini")),
        None => (format!("{w}.txt"), format!("{w}TreeLOD.ini")),
    };
    let files = [(out.join("meshes").join("terrain").join(w).join("trees").join(txt_name), txt), (out.join("textures").join("terrain").join(w).join("trees").join(ini_name), "[Information]\r\nComplexAtlasTexture=0\r\n\r\n".to_string())];
    for (path, text) in &files {
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(files.len())
}

pub fn run(a: &Args) -> Result<()> {
    let p = a.profile.open()?;
    run_with(&p, a)
}

/// Ultra tree LOD: an empty `<World>[.SEASON].lst` (type count 0) for the default season and
/// each built season, so the game draws no tree LOD while the trees are in object LOD (DynDOLOD's
/// default, `CreateEmptyBTT=0`). Worldspaces the load order has no tree LOD for are left alone.
pub fn write_empty_lists(p: &Profile, a: &Args) -> Result<()> {
    let worlds = scan_worldspaces(&p.load_order)?;
    let Some(world) = worlds.iter().find(|w| w.editor_id.eq_ignore_ascii_case(&a.world)) else { bail!("worldspace {} not found", a.world) };
    let empty = TreeLod { world: world.editor_id.clone(), ..Default::default() };
    let mut seasons: Vec<Option<&str>> = vec![None];
    seasons.extend(a.seasons.iter().map(|s| Some(s.as_str())));
    let mut n = 0;
    for season in seasons {
        let rel = season_path(empty.list_path(), season.map(str::to_ascii_uppercase).as_deref());
        let path = a.out.join(&rel);
        std::fs::create_dir_all(path.parent().unwrap())?;
        std::fs::write(&path, empty.list_bytes()).with_context(|| format!("writing {}", path.display()))?;
        n += 1;
    }
    println!("{}: ultra tree LOD, {n} empty tree LOD lists (trees are in object LOD)", world.editor_id);
    Ok(())
}

pub fn run_with(p: &Profile, a: &Args) -> Result<()> {
    let t0 = Instant::now();
    let lo = &p.load_order;
    let worlds = scan_worldspaces(lo)?;
    let Some(world) = worlds.iter().find(|w| w.editor_id.eq_ignore_ascii_case(&a.world)) else { bail!("worldspace {} not found", a.world) };
    let bases = read_tree_bases(lo)?;
    let lod = p.vfs.read(&format!("lodsettings\\{}.lod", world.editor_id))?.context("lodsettings missing")?;
    let sw = (i32::from(i16::from_le_bytes([lod[0], lod[1]])), i32::from(i16::from_le_bytes([lod[2], lod[3]])));
    let inputs = crate::object_export::load_inputs(&p, &a.world)?;
    let mut seasons = vec![None];
    for s in &a.seasons {
        seasons.push(Some(crate::object_export::season_from_code(s).with_context(|| format!("unknown season {s}"))?));
    }
    let mut rules = match &a.dyndolod {
        Some(dir) => Rules::dyndolod(crate::dyndolod_build::tree_max_tilt(dir), crate::dyndolod_build::tree_object_bounds_in(dir, &world.editor_id), cell_blocks(world, sw)),
        None => Rules::default(),
    };
    rules.exclude.clone_from(&a.exclude);
    let mut gpu = None;
    for season in seasons {
        let t = Instant::now();
        let swaps: HashMap<u32, u32> = season.map(|s| crate::object_export::base_swaps(&inputs, s)).unwrap_or_default();
        let tl = build_with(world, &bases, sw, &swaps, &Vfs(&p.vfs), 16384, &rules);
        if let Some(dir) = &a.placeholder_billboards {
            let n = write_placeholders(dir, &tl)?;
            println!("wrote {n} placeholder billboards under {}", dir.display());
            return Ok(());
        }
        let refs: usize = tl.blocks.iter().flat_map(|b| b.types.iter()).map(|(_, r)| r.len()).sum();
        let name = season.map(|s| s.code()).unwrap_or("default");
        if a.list_missing {
            for t in tl.trees.iter().filter(|t| t.index == -1) {
                let b = &bases[&t.form_id];
                println!("missing [{name}] {:08X} {} {} -> {}", t.form_id, String::from_utf8_lossy(&b.signature), b.model, t.billboard);
            }
        }
        if tl.types.is_empty() {
            println!("{} [{name}]: {} tree bases, {} missing billboards (e.g. {:?}): no tree LOD (xLODGen: \"no resource billboards\")", world.editor_id, tl.trees.len(), tl.missing_billboards.len(), tl.missing_billboards.first());
            // DynDOLOD blanks the vanilla tree LOD of a worldspace without trees with an empty list
            // (4 bytes; golden3: ccKRTSSE001QNWorld, DLC2ApocryphaWorld). Vanilla .btt tiles stay.
            let lst = season_path(tl.list_path(), season.map(|s| s.code()));
            if a.dyndolod.is_some() && p.vfs.exists(&lst) {
                let path = a.out.join(&lst);
                std::fs::create_dir_all(path.parent().unwrap())?;
                std::fs::write(&path, tl.list_bytes()).with_context(|| format!("writing {}", path.display()))?;
                println!("  {lst}: empty list over the vanilla one");
            }
            continue;
        }
        if gpu.is_none() {
            gpu = Some(Gpu::new()?);
        }
        let atlas = atlas_dds(&p, &tl, gpu.as_ref().unwrap())?;
        let mut n = write(&a.out, &tl, season.map(|s| s.code()), atlas.as_deref())?;
        if a.dyndolod.is_some() {
            n += write_dyndolod_extras(&a.out, lo, &tl, season.map(|s| s.code()))?;
        }
        if let (None, Some(g)) = (season, &a.golden) {
            compare_golden(g, &tl, atlas.as_deref())?;
        }
        println!("{} [{name}]: {} tree types, {refs} references in {} blocks, atlas {}x{}, {} missing billboards, {} duplicate ids, {} never-enabled references left out, {} types ({} references) over the {}-type limit left out; {n} files in {} ms", world.editor_id, tl.types.len(), tl.blocks.len(), tl.atlas.0 >> atlas_drop_levels(tl.atlas.0, tl.atlas.1), tl.atlas.1 >> atlas_drop_levels(tl.atlas.0, tl.atlas.1), tl.missing_billboards.len(), tl.duplicates, tl.never_enabled, tl.over_limit.0, tl.over_limit.1, lodforge_treelod::MAX_TREE_TYPES, t.elapsed().as_millis());
    }
    println!("total {} ms", t0.elapsed().as_millis());
    Ok(())
}

fn find_ci(dir: &Path, rel: &str) -> Option<PathBuf> {
    let mut cur = dir.to_path_buf();
    for part in rel.split('\\') {
        let next = std::fs::read_dir(&cur).ok()?.flatten().find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(part))?;
        cur = next.path();
    }
    Some(cur)
}

/// BTT references per type, sorted, with rotation dropped: (type, form id, x, y, z, scale) bits.
fn btt_refs(bytes: &[u8]) -> Vec<(i32, u32, [u32; 4])> {
    let u = |o: usize| u32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
    let mut out = Vec::new();
    let mut o = 4;
    for _ in 0..u(0) {
        let (index, count) = (u(o) as i32, u(o + 4));
        o += 8;
        for _ in 0..count {
            out.push((index, u(o + 20), [u(o), u(o + 4), u(o + 8), u(o + 16)]));
            o += 32;
        }
    }
    out.sort_unstable();
    out
}

fn compare_golden(golden: &Path, t: &TreeLod, atlas: Option<&[u8]>) -> Result<()> {
    if let (Some(ours), Some(g)) = (atlas, find_ci(golden, &t.atlas_path())) {
        let gb = std::fs::read(g)?;
        let (gi, oi) = (lodforge_dds::parse(&gb)?, lodforge_dds::parse(ours)?);
        let (gd, od) = (lodforge_dds::decode(&gb, &gi, 0)?, lodforge_dds::decode(ours, &oi, 0)?);
        if (gd.width, gd.height) == (od.width, od.height) {
            use lodforge_terrain::image::{Image, compare};
            let m = compare(&Image::from_rgba8(gd.width, gd.height, &gd.rgba), &Image::from_rgba8(od.width, od.height, &od.rgba));
            println!("  golden atlas: {:?} {}x{}, PSNR {:.2} dB, MAE {:.2}", gi.format, gd.width, gd.height, m.psnr, m.mae);
        } else {
            println!("  golden atlas: {}x{} vs ours {}x{}", gd.width, gd.height, od.width, od.height);
        }
    }
    let ours_list = t.list_bytes();
    match find_ci(golden, &t.list_path()).map(std::fs::read).transpose()? {
        Some(g) if g == ours_list => println!("  golden LST: identical ({} types)", t.types.len()),
        Some(g) => {
            println!("  golden LST: differs ({} vs {} bytes)", ours_list.len(), g.len());
            for (i, (a, b)) in ours_list[4..].chunks(32).zip(g.get(4..).unwrap_or(&[]).chunks(32)).enumerate() {
                if a != b {
                    let f = |c: &[u8]| -> Vec<f32> { c[4..28].chunks(4).map(|x| f32::from_le_bytes(x.try_into().unwrap())).collect() };
                    println!("    entry {i}: ours idx {} {:?} golden idx {} {:?}", i32::from_le_bytes(a[..4].try_into().unwrap()), f(a), i32::from_le_bytes(b[..4].try_into().unwrap()), f(b));
                }
            }
        }
        None => println!("  golden LST: missing"),
    }
    let (mut same, mut differ, mut missing) = (0, 0, 0);
    let mut ours_paths = std::collections::HashSet::new();
    for b in &t.blocks {
        let rel = t.block_path(b, None);
        ours_paths.insert(rel.to_lowercase());
        let Some(g) = find_ci(golden, &rel) else {
            missing += 1;
            continue;
        };
        let (ours, theirs) = (btt_refs(&TreeLod::block_bytes(b)), btt_refs(&std::fs::read(g)?));
        if ours == theirs {
            same += 1;
        } else {
            if differ < 3 {
                let only_ours = ours.iter().filter(|r| !theirs.contains(r)).count();
                let only_theirs = theirs.iter().filter(|r| !ours.contains(r)).count();
                println!("    {rel}: {} vs {} refs, {only_ours} only ours, {only_theirs} only golden", ours.len(), theirs.len());
                let show = |r: &(i32, u32, [u32; 4])| format!("type {} ref {:08X} pos {:?} scale {}", r.0, r.1, [f32::from_bits(r.2[0]), f32::from_bits(r.2[1]), f32::from_bits(r.2[2])], f32::from_bits(r.2[3]));
                for r in ours.iter().filter(|r| !theirs.contains(r)).take(4) {
                    let g = theirs.iter().find(|g| g.1 == r.1);
                    println!("      ours   {}\n      golden {}", show(r), g.map(show).unwrap_or_default());
                }
                for r in theirs.iter().filter(|r| !ours.contains(r)).take(4) {
                    println!("      only golden {}", show(r));
                }
            }
            differ += 1;
        }
    }
    let trees_dir = find_ci(golden, &format!("meshes\\terrain\\{}\\trees", t.world));
    let extra = trees_dir
        .and_then(|d| std::fs::read_dir(d).ok())
        .map(|it| it.flatten().filter(|e| e.file_name().to_string_lossy().to_lowercase().ends_with(".btt") && !ours_paths.contains(&format!("meshes\\terrain\\{w}\\trees\\{}", e.file_name().to_string_lossy().to_lowercase(), w = t.world.to_lowercase()))).count())
        .unwrap_or(0);
    println!("  golden BTT (rotation ignored): {same} identical, {differ} differ, {missing} missing in golden, {extra} only in golden");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atlas_is_halved_until_the_game_loads_it() {
        assert_eq!(atlas_drop_levels(4096, 4096), 0);
        assert_eq!(atlas_drop_levels(2048, 1024), 0);
        assert_eq!(atlas_drop_levels(8192, 4096), 1);
        assert_eq!(atlas_drop_levels(8192, 8192), 1);
        assert_eq!(atlas_drop_levels(16384, 8192), 2);
    }
}
