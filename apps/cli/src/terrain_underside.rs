//! `lodforge terrain-underside`: DynDOLOD's terrain underside mesh (`<World>_Underside.nif`), built
//! with `lodforge_btr::underside` (the BTR mesher at LOD32, faces flipped).
//!
//! Input is the load order (the cells inside the worldspace's object bounds, clipped by the
//! `TerrainUndersideMin/Max*` keys of `DynDOLOD_SSE.ini`), or a DynDOLOD
//! `LODGen_SSE_Export_TerrainUnderside_<W>.txt` with `--from-export` (settings and `TerrainData`
//! taken from it). `--golden` compares the result with a DynDOLOD output folder, quad by quad and
//! independent of block order.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result, bail};

use lodforge_btr::underside::{self, QuadSummary, UndersideSettings};
use lodforge_btr::{ControlFile, export, terrain_bin};
use lodforge_esp::scan_worldspaces;

use crate::profile::{Profile, ProfileArgs};

#[derive(clap::Args)]
pub struct Args {
    #[command(flatten)]
    pub profile: ProfileArgs,
    #[arg(long, default_value = "Tamriel")]
    pub world: String,
    /// Output Data folder (meshes\Terrain\<world>\<world>_Underside.nif is written inside).
    #[arg(long, default_value = "bench/out")]
    pub out: PathBuf,
    /// DynDOLOD `TerrainUndersideQuality` (`QualityLOD32` of the export) when not reading an export.
    #[arg(long, default_value_t = 10.0)]
    pub quality: f32,
    /// DynDOLOD's export for the underside: take settings and TerrainData from it.
    #[arg(long)]
    pub from_export: Option<PathBuf>,
    /// DynDOLOD output Data folder to compare with (meshes\Terrain\<world>\<world>_Underside.nif).
    #[arg(long)]
    pub golden: Option<PathBuf>,
    /// DynDOLOD `Edit Scripts\DynDOLOD` folder: per-worldspace `TerrainUndersideMin/MaxX/Y_<W>`
    /// and `TerrainUndersideIgnoreHide_<W>` from `DynDOLOD_SSE.ini` (and `TerrainUndersideQuality`).
    #[arg(long)]
    pub dyndolod: Option<PathBuf>,
    /// Leave cells without LAND flat instead of filling them from the shipped terrain LOD meshes
    /// (as the terrain meshes stage does).
    #[arg(long)]
    pub no_lod_fill: bool,
}

/// The export lines DynDOLOD adds for a worldspace from `DynDOLOD_SSE.ini` (`MinX=` ...,
/// `IgnoreHide=`), plus the quality.
fn ini_lines(dir: &Path, world: &str) -> (String, Option<f32>) {
    let Ok(ini) = lodforge_dyndolod::ini::Ini::read(&lodforge_dyndolod::overlay::path(dir, "DynDOLOD_SSE.ini")) else {
        return (String::new(), None);
    };
    let get = |k: &str| {
        ini.get("DynDOLOD", k)
            .map(str::trim)
            .filter(|v| !v.is_empty())
    };
    let mut text = String::new();
    for k in ["MinX", "MinY", "MaxX", "MaxY", "IgnoreHide"] {
        if let Some(v) = get(&format!("TerrainUnderside{k}_{world}")) {
            text.push_str(&format!("{k}={v}\n"));
        }
    }
    (
        text,
        get("TerrainUndersideQuality").and_then(|v| v.parse().ok()),
    )
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
    let parent = world
        .parent
        .and_then(|pid| worlds.iter().find(|w| w.form_id == pid));

    let path = format!("lodsettings\\{}.lod", world.editor_id);
    let lod = p
        .vfs
        .read(&path)?
        .with_context(|| format!("{path} not found"))?;
    let (south_west, _) =
        export::parse_lodsettings(&lod).with_context(|| format!("{path}: short file"))?;

    let t = Instant::now();
    let (settings, input) = match &a.from_export {
        Some(path) => {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("reading {}", path.display()))?;
            let mut s = UndersideSettings::parse(&text);
            if s.world.is_empty() {
                s.world = world.editor_id.clone();
            }
            let td = s
                .terrain_data
                .clone()
                .context("export without TerrainData=")?;
            let bin = read_terrain_data(path, &td)?;
            let cells = terrain_bin::read_cells(&bin).map_err(anyhow::Error::msg)?;
            let input = terrain_bin::terrain_input(
                &cells,
                &s.world,
                s.south_west,
                s.level,
                s.land_height,
                s.water_height,
            );
            println!(
                "settings from {}; terrain data {} cells, rectangle {:?}",
                path.display(),
                input.cells.len(),
                input.cell_rect()
            );
            (s, input)
        }
        None => {
            let (land, water) = export::world_defaults(world, parent);
            let (extra, quality) = a
                .dyndolod
                .as_deref()
                .map(|d| ini_lines(d, &world.editor_id))
                .unwrap_or_default();
            let mut s = UndersideSettings::dyndolod(
                &world.editor_id,
                south_west,
                land,
                water,
                quality.unwrap_or(a.quality),
            );
            s.apply_bounds(&ControlFile::parse(&extra));
            let mut input = export::terrain_input(lo, world, parent, south_west, 32)?;
            if !a.no_lod_fill {
                let f = crate::terrain_meshes::fill_from_lod_meshes(p, world, parent, &mut input);
                if f.filled > 0 {
                    println!("  {}", f.line());
                }
            }
            // DynDOLOD's underside input is the worldspace's object bounds (NAM0/NAM9), spec 4.2:
            // the cells the rectangle touches, a max corner on a cell line excluded (Tamriel's
            // NAM9 (253952, 208896) = cells up to 61, 50, as its export says).
            if let Some((lo_b, hi_b)) = world.object_bounds {
                let lo_c = ((lo_b[0] / 4096.0).floor() as i32, (lo_b[1] / 4096.0).floor() as i32);
                let hi_c = ((hi_b[0] / 4096.0).ceil() as i32 - 1, (hi_b[1] / 4096.0).ceil() as i32 - 1);
                input.clip(lo_c, hi_c);
            }
            (s, input)
        }
    };
    let export_ms = t.elapsed().as_millis();
    let rect = input.cell_rect();

    let t = Instant::now();
    let quads = underside::build(&input, &settings).map_err(anyhow::Error::msg)?;
    let build_ms = t.elapsed().as_millis();

    let name = format!("{}_Underside.nif", settings.world);
    let out_dir = a.out.join("meshes").join("Terrain").join(&settings.world);
    std::fs::create_dir_all(&out_dir)?;
    let nif = underside::to_nif(&settings.world, settings.season.as_deref(), &quads);
    let bytes = nif.to_bytes()?;
    std::fs::write(out_dir.join(&name), &bytes)?;
    println!(
        "{}: {} cells (rectangle {:?}, in {export_ms} ms); {} LOD{} quads with triangles, built in {build_ms} ms; {} vertices, {} triangles; {} bytes written; total {} ms",
        settings.world,
        input.cells.len(),
        rect,
        quads.len(),
        settings.level,
        quads.iter().map(|q| q.positions.len()).sum::<usize>(),
        quads.iter().map(|q| q.triangles.len()).sum::<usize>(),
        bytes.len(),
        total.elapsed().as_millis()
    );

    if let Some(g) = &a.golden {
        let path = g
            .join("meshes")
            .join("Terrain")
            .join(&settings.world)
            .join(&name);
        let gold = std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?;
        let gnif = lodforge_nif::parse(&gold)?;
        println!(
            "vs golden {}: creator ours {:?}, golden {:?}",
            path.display(),
            nif.header.author,
            gnif.header.author
        );
        compare(&underside::summarize(&nif), &underside::summarize(&gnif));
    }
    Ok(())
}

/// `TerrainData=` path; falls back to the file of the same name next to the export file (golden
/// copies keep the export folder but not DynDOLOD's absolute path).
fn read_terrain_data(export: &Path, td: &str) -> Result<Vec<u8>> {
    let p = PathBuf::from(td);
    if let Ok(b) = std::fs::read(&p) {
        return Ok(b);
    }
    let file = td.rsplit(['\\', '/']).next().unwrap_or(td);
    let near = export.parent().unwrap_or(Path::new(".")).join(file);
    std::fs::read(&near)
        .with_context(|| format!("terrain data {td} (also tried {})", near.display()))
}

type Tri = [[u32; 5]; 3];

/// Triangle as its three vertices (position + UV bits), rotated so the smallest vertex comes
/// first (winding kept).
fn tri_key(s: &QuadSummary, t: &[u16; 3]) -> Tri {
    let v = t.map(|i| {
        let (p, uv) = (
            s.positions[i as usize],
            s.uvs.get(i as usize).copied().unwrap_or_default(),
        );
        [
            p[0].to_bits(),
            p[1].to_bits(),
            p[2].to_bits(),
            uv[0].to_bits(),
            uv[1].to_bits(),
        ]
    });
    let m = (0..3).min_by_key(|&k| v[k]).unwrap();
    [v[m], v[(m + 1) % 3], v[(m + 2) % 3]]
}

/// Per-quad comparison: vertex and triangle sets, bounds, textures, shader, shape header.
fn compare(ours: &[QuadSummary], gold: &[QuadSummary]) {
    let key = |s: &QuadSummary| (s.translation[0] as i32, s.translation[1] as i32);
    let gmap: HashMap<_, _> = gold.iter().map(|s| (key(s), s)).collect();
    let omap: HashMap<_, _> = ours.iter().map(|s| (key(s), s)).collect();
    println!(
        "vs golden: {} quads ours, {} golden",
        ours.len(),
        gold.len()
    );
    for k in gmap.keys().filter(|k| !omap.contains_key(k)) {
        println!("  missing quad at {k:?}");
    }
    let mut keys: Vec<_> = omap.keys().copied().collect();
    keys.sort_by_key(|k| (k.1, k.0));
    let (
        mut geom_same,
        mut v_total,
        mut v_match,
        mut t_total,
        mut t_match,
        mut gv_total,
        mut gt_total,
        mut fields_same,
    ) = (0, 0usize, 0usize, 0usize, 0usize, 0usize, 0usize, 0);
    for k in keys {
        let o = omap[&k];
        let Some(g) = gmap.get(&k) else {
            println!("  extra quad at {k:?}");
            continue;
        };
        let vs = |s: &QuadSummary| -> BTreeSet<[u32; 5]> {
            s.positions
                .iter()
                .zip(s.uvs.iter().chain(std::iter::repeat(&[0.0, 0.0])))
                .map(|(p, uv)| {
                    [
                        p[0].to_bits(),
                        p[1].to_bits(),
                        p[2].to_bits(),
                        uv[0].to_bits(),
                        uv[1].to_bits(),
                    ]
                })
                .collect()
        };
        let ts = |s: &QuadSummary| -> BTreeSet<Tri> {
            s.triangles.iter().map(|t| tri_key(s, t)).collect()
        };
        let (ov, gv, ot, gt) = (vs(o), vs(g), ts(o), ts(g));
        let vm = ov.intersection(&gv).count();
        let tm = ot.intersection(&gt).count();
        v_total += ov.len();
        gv_total += gv.len();
        t_total += ot.len();
        gt_total += gt.len();
        v_match += vm;
        t_match += tm;
        if ov == gv && ot == gt {
            geom_same += 1;
        }
        let mut diffs = Vec::new();
        if o.positions.len() != g.positions.len() || o.triangles.len() != g.triangles.len() {
            diffs.push(format!(
                "counts v {}/{} t {}/{}",
                o.positions.len(),
                g.positions.len(),
                o.triangles.len(),
                g.triangles.len()
            ));
        }
        if vm != ov.len().max(gv.len()) {
            diffs.push(format!(
                "vertex set {vm} common of {}/{}",
                ov.len(),
                gv.len()
            ));
        }
        if tm != ot.len().max(gt.len()) {
            diffs.push(format!(
                "triangle set {tm} common of {}/{}",
                ot.len(),
                gt.len()
            ));
        }
        let mut field_diffs = Vec::new();
        if o.aabb != g.aabb {
            field_diffs.push(format!("aabb {:?} vs {:?}", o.aabb, g.aabb));
        }
        if o.textures != g.textures {
            field_diffs.push(format!("textures {:?} vs {:?}", o.textures, g.textures));
        }
        let unref = |s: &Option<lodforge_nif::LightingShader>| {
            s.clone().map(|mut l| {
                l.texture_set = None;
                l
            })
        };
        if unref(&o.shader) != unref(&g.shader) {
            field_diffs.push("shader differs".into());
        }
        if (
            o.scale,
            o.flags,
            o.translation,
            o.vertex_desc,
            o.node_culling,
        ) != (
            g.scale,
            g.flags,
            g.translation,
            g.vertex_desc,
            g.node_culling,
        ) {
            field_diffs.push(format!(
                "shape header {:?} vs {:?}",
                (
                    o.scale,
                    o.flags,
                    o.translation,
                    o.vertex_desc,
                    o.node_culling
                ),
                (
                    g.scale,
                    g.flags,
                    g.translation,
                    g.vertex_desc,
                    g.node_culling
                )
            ));
        }
        if field_diffs.is_empty() {
            fields_same += 1;
        }
        diffs.extend(field_diffs);
        if !diffs.is_empty() {
            println!("  quad {k:?}: {}", diffs.join("; "));
        }
    }
    println!(
        "  {geom_same}/{} quads with identical vertex and triangle sets; {fields_same} with identical bounds, textures, shader and shape header",
        gold.len()
    );
    println!(
        "  vertices: {v_match} common of {v_total} ours / {gv_total} golden; triangles: {t_match} common of {t_total} ours / {gt_total} golden"
    );
}
