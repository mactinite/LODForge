//! `lodforge terrain-meshes`: terrain LOD meshes (.btr) for a worldspace, built from the load order
//! with `lodforge-btr`. Season variants share the geometry; only the texture paths differ, so they
//! are serialized from the same encoding.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use rayon::prelude::*;

use lodforge_btr::mesh::{QuadOptions, build_quad, full_coverage};
use lodforge_btr::settings::LEVELS;
use lodforge_btr::{Btr, ControlFile, Settings, export};
use lodforge_esp::scan_worldspaces;

use crate::profile::{Profile, ProfileArgs};

#[derive(clap::Args)]
pub struct Args {
    #[command(flatten)]
    pub profile: ProfileArgs,
    #[arg(long, default_value = "Tamriel")]
    pub world: String,
    /// Output Data folder (meshes\terrain\<world>\ is created inside).
    #[arg(long, default_value = "bench/out")]
    pub out: PathBuf,
    /// Also write these season variants (e.g. SPR,SUM,AUT,WIN).
    #[arg(long, value_delimiter = ',')]
    pub seasons: Vec<String>,
    /// Terrain control file (`LODGen_Terrain_<World>.txt` layout) to take settings from
    /// (defaults: xLODGen's).
    #[arg(long)]
    pub settings: Option<PathBuf>,
    /// Golden folder of .btr files: byte comparison plus the tolerant comparison (0.5 local units).
    #[arg(long)]
    pub golden: Option<PathBuf>,
    /// Leave cells without LAND flat at the default land height instead of filling them from the
    /// terrain LOD meshes the game ships (docs/re/terrain-lod-spec.md 2.3).
    #[arg(long)]
    pub no_lod_fill: bool,
    /// LOD32 mesh error threshold (`Quality`; 0 = every vertex the budget allows, STEP's map
    /// setting). Default: the control file's or xLODGen's 10.
    #[arg(long)]
    pub lod32_quality: Option<f32>,
}

/// Fills the cells without LAND of `input` from the shipped terrain LOD meshes (`lodforge_btr::fill`),
/// reading each mesh's archived copy first so that generated loose files are not mistaken for the
/// game's data.
pub fn fill_from_lod_meshes(
    p: &Profile,
    world: &lodforge_esp::Worldspace,
    parent: Option<&lodforge_esp::Worldspace>,
    input: &mut lodforge_btr::TerrainInput,
) -> lodforge_btr::fill::FillReport {
    let landed = export::land_cells(world, parent);
    let read = |path: &str| {
        p.vfs
            .read_archived(path)
            .ok()
            .flatten()
            .or_else(|| p.vfs.read(path).ok().flatten())
    };
    lodforge_btr::fill::from_lod_meshes(input, &|c| landed.contains(&c), &read)
}

pub fn run(a: &Args) -> Result<()> {
    let p = a.profile.open()?;
    run_with(&p, a)
}

pub fn run_with(p: &Profile, a: &Args) -> Result<()> {
    let total = Instant::now();
    let lo = &p.load_order;
    let worlds = scan_worldspaces(lo)?;
    let Some(world) = worlds
        .iter()
        .find(|w| w.editor_id.eq_ignore_ascii_case(&a.world))
    else {
        bail!("worldspace {} not found", a.world)
    };

    // South-west cell and top level come from lodsettings\<world>.lod.
    let path = format!("lodsettings\\{}.lod", world.editor_id);
    let lod = p
        .vfs
        .read(&path)?
        .with_context(|| format!("{path} not found"))?;
    let (south_west, max_level) =
        export::parse_lodsettings(&lod).with_context(|| format!("{path}: short file"))?;
    let (settings, control) = match &a.settings {
        Some(path) => {
            let c = ControlFile::parse(
                &std::fs::read_to_string(path)
                    .with_context(|| format!("reading {}", path.display()))?,
            );
            (c.settings.clone(), Some(c))
        }
        None => (Settings::default(), None),
    };
    let mut settings = settings;
    if let Some(q) = a.lod32_quality {
        settings.level_mut(32).quality = q;
    }

    let t = Instant::now();
    let mut input = export::terrain_input(lo, world, None, south_west, max_level)?;
    if let Some(c) = &control {
        input.south_west = c.cell_sw.unwrap_or(input.south_west);
        input.max_level = c.max_level.unwrap_or(input.max_level);
        input.default_land = c.land_height.unwrap_or(input.default_land);
        if let Some(w) = c.water_height {
            input.default_water = Some(w);
        }
    }
    let fill = (!a.no_lod_fill).then(|| fill_from_lod_meshes(p, world, None, &mut input));
    let cell_count = input.cells.len();
    let export_ms = t.elapsed().as_millis();

    let field = input.sampler();
    let hide = settings.hide_quads;
    let field_ref = &field;
    let coverage = move |cx: i32, cy: i32, bit: u8| -> bool {
        !hide
            || field_ref
                .cell(cx, cy)
                .is_none_or(|c| c.hide_quadrants & bit == 0)
    };
    let quads: Vec<(i32, i32, i32)> = LEVELS
        .iter()
        .filter(|&&l| l <= input.max_level)
        .flat_map(|&l| input.quads(l).into_iter().map(move |(x, y)| (l, x, y)))
        .collect();

    let t = Instant::now();
    let built: Vec<(Btr, f64, bool)> = quads
        .par_iter()
        .map(|&(level, qx, qy)| {
            let opts = QuadOptions::for_level(&settings, level);
            let cov: &(dyn Fn(i32, i32, u8) -> bool + Sync) =
                if hide { &coverage } else { &full_coverage };
            let m = build_quad(&field, &opts, qx, qy, cov).map_err(anyhow::Error::msg)?;
            Ok((
                Btr::from_mesh(&m, settings.level(level).z_shift),
                m.max_error,
                m.budget_hit,
            ))
        })
        .collect::<Result<_>>()?;
    let build_ms = t.elapsed().as_millis();

    let out_dir = a.out.join("meshes").join("terrain").join(&world.editor_id);
    std::fs::create_dir_all(&out_dir)?;
    let t = Instant::now();
    let variants: Vec<Option<&str>> = std::iter::once(None)
        .chain(a.seasons.iter().map(|s| Some(s.as_str())))
        .collect();
    let written: usize = built
        .par_iter()
        .map(|(b, _, _)| -> Result<usize> {
            let files = b.write_variants(&world.editor_id, &variants)?;
            for (name, bytes) in &files {
                std::fs::write(out_dir.join(name), bytes)?;
            }
            Ok(files.len())
        })
        .sum::<Result<usize>>()?;
    let write_ms = t.elapsed().as_millis();
    println!(
        "{}: {cell_count} cells exported in {export_ms} ms; {} quads built in {build_ms} ms; {written} files written in {write_ms} ms; total {} ms",
        world.editor_id,
        quads.len(),
        total.elapsed().as_millis()
    );
    if let Some(f) = &fill {
        println!("  {}", f.line());
    }
    for &level in LEVELS.iter().filter(|&&l| l <= input.max_level) {
        let of_level: Vec<&(Btr, f64, bool)> =
            built.iter().filter(|(b, _, _)| b.level == level).collect();
        let verts: Vec<usize> = of_level
            .iter()
            .map(|(b, _, _)| b.land.positions.len())
            .collect();
        let tris: Vec<usize> = of_level
            .iter()
            .map(|(b, _, _)| b.land.triangles.len())
            .collect();
        let water_tris: usize = of_level
            .iter()
            .map(|(b, _, _)| b.water.iter().map(|w| w.triangles.len()).sum::<usize>())
            .sum();
        let with_water = of_level
            .iter()
            .filter(|(b, _, _)| !b.water.is_empty())
            .count();
        let worst = of_level.iter().map(|(_, e, _)| *e).fold(0.0, f64::max);
        let budget = of_level.iter().filter(|(_, _, h)| *h).count();
        let q = |v: &[usize]| {
            let mut s = v.to_vec();
            s.sort_unstable();
            if s.is_empty() {
                "-".to_string()
            } else {
                format!(
                    "{} / {} / {} / {}",
                    s[0],
                    s[s.len() / 2],
                    s[s.len() - 1],
                    s.iter().sum::<usize>()
                )
            }
        };
        println!(
            "  LOD{level:<2}: {} files; land vertices min/p50/max/sum {}; triangles {}; files with water {with_water}, water triangles {water_tris}; worst remaining error {worst:.2}; budget hit {budget}",
            of_level.len(),
            q(&verts),
            q(&tris)
        );
    }

    if let Some(g) = &a.golden {
        let (mut same, mut differ, mut missing) = (0, 0, 0);
        for (b, _, _) in &built {
            let name = b.file_name(&world.editor_id, None);
            match std::fs::read(g.join(&name)) {
                Ok(gold) => {
                    if std::fs::read(out_dir.join(&name))? == gold {
                        same += 1;
                    } else {
                        differ += 1;
                    }
                }
                Err(_) => missing += 1,
            }
        }
        println!("vs golden: {same} byte-identical, {differ} differ, {missing} missing in golden");
        for r in
            lodforge_btr::compare::compare_dirs(&out_dir, g, 0.5).map_err(anyhow::Error::msg)?
        {
            println!("  {}", r.line());
            for e in &r.examples {
                println!("    {e}");
            }
        }
        for r in lodforge_btr::compare::compare_heights(&out_dir, g).map_err(anyhow::Error::msg)? {
            println!("  {}", r.line());
        }
    }
    Ok(())
}
