//! `lodforge runtime-heightfield`: a conservative terrain heightfield per worldspace for the LODForge
//! Runtime's occlusion culling (`SKSE\Plugins\LODForgeRuntime\<World>.hf.bin`, docs/runtime/
//! instance-formats.md section 8). The plugin draws it as one depth-only grid into its terrain depth
//! target instead of replaying every landscape draw of the engine (1200+ a frame at long terrain
//! LOD block distances: 4 ms of frame time at Whiterun, 2026-10-27).
//!
//! Every sample is the lowest LAND height within `spacing` grid steps of it, minus `margin`, so the
//! grid never rises above the real terrain nor above the LOD terrain meshes made from the same
//! heights (their simplification error is what the margin covers). Cells without LAND are holes
//! (nothing occludes there).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result, bail};
use rayon::prelude::*;

use lodforge_btr::export;
use lodforge_btr::input::{CELL_STEPS, STEP_UNITS, TerrainInput};
use lodforge_esp::world::scan_worldspaces;

use crate::profile::{Profile, ProfileArgs};

pub const MAGIC: &[u8; 4] = b"LFHF";
pub const VERSION: u16 = 1;
/// Stored value of a sample without terrain (a hole).
pub const NO_HEIGHT: i16 = i16::MIN;
/// Game units per stored unit (i16 range +-65534 units).
pub const UNIT: f32 = 2.0;
pub const FILE_SUFFIX: &str = ".hf.bin";

#[derive(clap::Args)]
pub struct Args {
    #[command(flatten)]
    pub profile: ProfileArgs,
    /// The output (Data folder or MO2 mod folder): writes `SKSE\Plugins\LODForgeRuntime\<World>.hf.bin`.
    #[arg(long)]
    pub out: PathBuf,
    /// Worldspaces; default: the worlds listed in the output's Build.txt.
    #[arg(long, value_delimiter = ',')]
    pub worlds: Vec<String>,
    /// Sample spacing in LAND grid steps of 128 units (2 = 256 units; the erosion radius is the same).
    #[arg(long, default_value_t = 2)]
    pub spacing: u32,
    /// Lowered below the eroded minimum by this many game units (terrain LOD mesh error headroom).
    #[arg(long, default_value_t = 48.0)]
    pub margin: f32,
}

pub struct Report {
    pub path: PathBuf,
    pub nx: usize,
    pub ny: usize,
    pub holes: usize,
    pub bytes: usize,
}

pub fn run(a: &Args) -> Result<()> {
    let p = a.profile.open()?;
    let token = crate::instances::init_build_token(&a.out);
    let worlds = if a.worlds.is_empty() { build_worlds(&a.out)? } else { a.worlds.clone() };
    if worlds.is_empty() {
        bail!("no worlds: pass --worlds or point --out at a runtime output with a Build.txt");
    }
    for w in &worlds {
        let t = Instant::now();
        match write_world(&p, &a.out, w, a.spacing, a.margin, &token)? {
            Some(r) => println!("{}: {} x {} samples every {} units, {} holes, {:.1} MB in {:.1} s", r.path.display(), r.nx, r.ny, a.spacing as f32 * STEP_UNITS, r.holes, r.bytes as f64 / 1048576.0, t.elapsed().as_secs_f64()),
            None => println!("{w}: no lodsettings\\{w}.lod (no terrain LOD): skipped"),
        }
    }
    Ok(())
}

/// The worlds of an output's Build.txt (`worlds=a,b,c`).
pub fn build_worlds(out: &Path) -> Result<Vec<String>> {
    let path = crate::instances::out_dir(out).join(crate::instances::BUILD_TXT);
    let txt = std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    Ok(txt.lines().find_map(|l| l.strip_prefix("worlds=")).map(|w| w.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()).unwrap_or_default())
}

/// Writes `<World>.hf.bin` into `out`'s runtime folder from the load order's LAND records. `None` for a
/// worldspace without `lodsettings` (no terrain LOD: nothing to occlude with beyond the loaded cells).
pub fn write_world(p: &Profile, out: &Path, world_name: &str, spacing: u32, margin: f32, token: &str) -> Result<Option<Report>> {
    let spacing = spacing.clamp(1, 32) as i32;
    let lo = &p.load_order;
    let worlds = scan_worldspaces(lo)?;
    let world = worlds.iter().find(|w| w.editor_id.eq_ignore_ascii_case(world_name)).with_context(|| format!("worldspace {world_name} not found"))?;
    let path = format!("lodsettings\\{}.lod", world.editor_id);
    let Some(lod) = p.vfs.read(&path)? else { return Ok(None) };
    let (south_west, max_level) = export::parse_lodsettings(&lod).with_context(|| format!("{path}: short file"))?;
    let input = export::terrain_input(lo, world, None, south_west, max_level)?;
    let landed: HashSet<(i32, i32)> = world.land_by_cell().keys().copied().collect();
    let (grid, x0, y0, nx, ny, holes) = sample(&input, &landed, spacing, margin);

    let dir = crate::instances::out_dir(out);
    std::fs::create_dir_all(&dir)?;
    let file = dir.join(format!("{}{}", world.editor_id, FILE_SUFFIX));
    let mut b: Vec<u8> = Vec::with_capacity(64 + grid.len() * 2);
    b.extend_from_slice(MAGIC);
    b.extend_from_slice(&VERSION.to_le_bytes());
    b.extend_from_slice(&(spacing as u16).to_le_bytes());
    b.extend_from_slice(&x0.to_le_bytes());
    b.extend_from_slice(&y0.to_le_bytes());
    b.extend_from_slice(&(nx as u32).to_le_bytes());
    b.extend_from_slice(&(ny as u32).to_le_bytes());
    b.extend_from_slice(&UNIT.to_le_bytes());
    b.extend_from_slice(&margin.to_le_bytes());
    b.extend_from_slice(&0u32.to_le_bytes()); // flags
    b.extend_from_slice(&0u32.to_le_bytes()); // reserved
    let s = |b: &mut Vec<u8>, t: &str| {
        let t = &t.as_bytes()[..t.len().min(255)];
        b.push(t.len() as u8);
        b.extend_from_slice(t);
    };
    s(&mut b, &world.editor_id);
    s(&mut b, token);
    while b.len() % 4 != 0 {
        b.push(0);
    }
    let data_offset = b.len() as u32;
    b[36..40].copy_from_slice(&data_offset.to_le_bytes()); // the reserved field: byte offset of the samples
    for v in &grid {
        b.extend_from_slice(&v.to_le_bytes());
    }
    let bytes = b.len();
    std::fs::write(&file, b).with_context(|| format!("writing {}", file.display()))?;
    Ok(Some(Report { path: file, nx, ny, holes, bytes }))
}

/// The eroded sample grid: (samples row-major from the south-west, cell x0, cell y0, nx, ny, holes).
/// Sample (i, j) sits at world (x0 * 4096 + i * spacing * 128, y0 * 4096 + j * spacing * 128).
fn sample(input: &TerrainInput, landed: &HashSet<(i32, i32)>, spacing: i32, margin: f32) -> (Vec<i16>, i32, i32, usize, usize, usize) {
    let field = input.sampler();
    let (mut cx0, mut cy0, mut cx1, mut cy1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for c in &input.cells {
        cx0 = cx0.min(c.x);
        cy0 = cy0.min(c.y);
        cx1 = cx1.max(c.x);
        cy1 = cy1.max(c.y);
    }
    if cx0 > cx1 {
        return (Vec::new(), 0, 0, 0, 0, 0);
    }
    // Dense 128-unit heights of the rectangle (NaN where the owner cell has no LAND), then a separable
    // min filter of +-spacing steps, then every spacing-th point.
    let gw = ((cx1 - cx0 + 1) * CELL_STEPS + 1) as usize;
    let gh = ((cy1 - cy0 + 1) * CELL_STEPS + 1) as usize;
    let gx0 = cx0 * CELL_STEPS;
    let gy0 = cy0 * CELL_STEPS;
    let dense: Vec<f32> = (0..gh)
        .into_par_iter()
        .flat_map_iter(|row| {
            let gy = gy0 + row as i32;
            let field = &field;
            (0..gw).map(move |col| {
                let gx = gx0 + col as i32;
                match field.owner(gx, gy) {
                    (cx, cy, c, r, Some(cell)) if landed.contains(&(cx, cy)) => cell.height(c, r),
                    _ => f32::NAN,
                }
            })
        })
        .collect();
    let min_run = |a: f32, b: f32| if a.is_nan() || b.is_nan() { f32::NAN } else { a.min(b) };
    let r = spacing as usize;
    // Rows.
    let rows: Vec<f32> = (0..gh)
        .into_par_iter()
        .flat_map_iter(|row| {
            let line = &dense[row * gw..(row + 1) * gw];
            (0..gw).map(move |col| {
                let lo = col.saturating_sub(r);
                let hi = (col + r).min(gw - 1);
                line[lo..=hi].iter().copied().fold(f32::INFINITY, min_run)
            })
        })
        .collect();
    // Columns, on the sampled points only.
    let nx = (gw - 1) / r + 1;
    let ny = (gh - 1) / r + 1;
    let out: Vec<i16> = (0..ny)
        .into_par_iter()
        .flat_map_iter(|j| {
            let rows = &rows;
            (0..nx).map(move |i| {
                let col = i * r;
                let row = j * r;
                let lo = row.saturating_sub(r);
                let hi = (row + r).min(gh - 1);
                let m = (lo..=hi).map(|rr| rows[rr * gw + col]).fold(f32::INFINITY, min_run);
                if m.is_nan() || !m.is_finite() {
                    NO_HEIGHT
                } else {
                    ((m - margin) / UNIT).floor().clamp(-32767.0, 32767.0) as i16
                }
            })
        })
        .collect();
    let holes = out.iter().filter(|&&v| v == NO_HEIGHT).count();
    (out, cx0, cy0, nx, ny, holes)
}
