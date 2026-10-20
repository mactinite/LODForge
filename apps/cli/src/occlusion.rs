//! `lodforge occlusion`: pre-computed occlusion (CELL TVDT, Occlusion.esp) for a worldspace and
//! the child worldspaces that use its LOD, optionally compared with a golden plugin. The
//! computation is `lodforge_dyndolod::occlusion` (docs/re/occlusion-spec.md; section 8 is this
//! command's contract).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};

use lodforge_dyndolod::occlusion::{self, Calculator, Grid, LandCell, Mode, Model, Occluder, Params, Tvdt};
use lodforge_dyndolod::plugin::PluginBuilder;
use lodforge_esp::{Entries, Entry, LoadOrder, fields};

#[derive(clap::Args)]
pub struct Args {
    /// MO2 instance: load order, terrain LOD meshes (.btr), lodsettings and strings.
    #[arg(long)]
    pub instance: PathBuf,
    #[arg(long, default_value = "LODForge-DynDOLOD")]
    pub profile: String,
    /// The worldspace DynDOLOD runs for; its child worldspaces follow unless --no-child-worlds.
    #[arg(long, default_value = "Tamriel")]
    pub world: String,
    /// Plugins DynDOLOD has loaded when it computes occlusion but the profile does not have: the
    /// DynDOLOD.esm and DynDOLOD.esp of the same run (DynDOLOD.esm's child-world cell copies get
    /// TVDT too, DynDOLOD.esp's CELL overrides win). Master-flagged files go after the last master,
    /// others at the end.
    #[arg(long)]
    pub extra_plugin: Vec<PathBuf>,
    /// Only the main worldspace, not its child worldspaces.
    #[arg(long)]
    pub no_child_worlds: bool,
    /// Only these worldspaces (EditorIDs, comma separated).
    #[arg(long)]
    pub only_worlds: Option<String>,
    /// LODGen's `<World>.objects.heightmap.dat` of the main worldspace.
    #[arg(long)]
    pub heightmap: Option<PathBuf>,
    /// DynDOLOD's `LODGen_SSE_LODGen_Terrain_<World>.bin` of the main worldspace, compared with the
    /// terrain read from the load order.
    #[arg(long)]
    pub terrain_bin: Option<PathBuf>,
    /// Golden Occlusion.esp to compare with.
    #[arg(long)]
    pub golden: Option<PathBuf>,
    /// Compute the golden plugin's cells instead of selecting cells by `--mode`.
    #[arg(long)]
    pub golden_cells: bool,
    /// DynDOLOD `OcclusionMode` (child worldspaces drop `+Border`).
    #[arg(long, default_value = "-Flat +Border")]
    pub mode: String,
    /// Write Occlusion.esp here.
    #[arg(long)]
    pub out: Option<PathBuf>,
    /// Only N cells per worldspace, spread over the list (quick checks).
    #[arg(long)]
    pub limit: Option<usize>,
    /// Only these cells, `x,y;x,y`.
    #[arg(long)]
    pub cells: Option<String>,
    #[arg(long, default_value_t = 2)]
    pub quality: i32,
    #[arg(long, default_value_t = 100)]
    pub radius: i32,
    #[arg(long, default_value_t = 0.0)]
    pub height: f32,
    /// Grid south-west cell and stride (default: the worldspace's lodsettings, else the main
    /// worldspace's, else -96,-96 and 256).
    #[arg(long)]
    pub origin: Option<String>,
    #[arg(long)]
    pub stride: Option<i32>,
    /// Worldspace default land height (default: the worldspace's DNAM, else the main worldspace's).
    #[arg(long, allow_hyphen_values = true)]
    pub land_height: Option<f32>,
    /// Print block maps (ours vs golden) of the most different cells, up to this many per world.
    #[arg(long, default_value_t = 0)]
    pub show: usize,
    /// Write one line per processed cell (`<World> x,y water: W, min: A, max: B, top: C`) here.
    #[arg(long)]
    pub dump_heights: Option<PathBuf>,
    /// With --golden: for the blocks only the golden sees and the blocks only we see, print how
    /// far the best line of sight is obstructed (model diagnostics).
    #[arg(long)]
    pub analyze: bool,
    /// With --golden: for this observer cell (`x,y`), print every block within `--probe-cells`
    /// cells that only one side sees, with the best line of sight and what blocks it.
    #[arg(long)]
    pub probe: Option<String>,
    #[arg(long, default_value_t = 24)]
    pub probe_cells: i32,
}

/// Histogram of needed clearances (game units) by distance band, for model diagnostics.
#[derive(Default)]
struct Obstructions {
    /// (distance band of 20 cells, clearance bin) -> blocks.
    bins: HashMap<(usize, usize), usize>,
}

const CLEARANCE_BINS: [f32; 8] = [0.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, f32::INFINITY];

impl Obstructions {
    fn add(&mut self, dist: f64, needed: f32) {
        let bin = CLEARANCE_BINS.iter().position(|&b| needed <= b).unwrap_or(CLEARANCE_BINS.len() - 1);
        *self.bins.entry(((dist / 20.0) as usize, bin)).or_default() += 1;
    }

    fn print(&self, what: &str) {
        println!("  {what}: blocks by distance band (cells) and clearance the best line of sight needs (<= bound, game units)");
        let labels = ["<=0", "250", "500", "1000", "2000", "4000", "8000", "more"];
        println!("    {:>8} {}", "band", labels.iter().map(|l| format!("{l:>7}")).collect::<String>());
        for band in 0..6 {
            let row: Vec<usize> = (0..CLEARANCE_BINS.len()).map(|b| self.bins.get(&(band, b)).copied().unwrap_or(0)).collect();
            if row.iter().sum::<usize>() > 0 {
                println!("    {:>3}..{:<3} {}", band * 20, band * 20 + 19, row.iter().map(|n| format!("{n:>7}")).collect::<String>());
            }
        }
    }
}

fn pair(s: &str) -> Option<(i32, i32)> {
    let (a, b) = s.split_once(',')?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
}

/// The cells of a LODGen terrain input file: `i32 x, i32 y, f32 water, u32 flags`, then the
/// VHGT payload (offset, 33 x 33 deltas, 3 pad bytes).
pub fn read_terrain_bin(data: &[u8]) -> Vec<LandCell> {
    const REC: usize = 16 + 4 + 33 * 33 + 3;
    data.chunks_exact(REC)
        .map(|r| LandCell {
            x: i32::from_le_bytes(r[0..4].try_into().unwrap()),
            y: i32::from_le_bytes(r[4..8].try_into().unwrap()),
            water: f32::from_le_bytes(r[8..12].try_into().unwrap()),
            vhgt: r[16..].to_vec(),
        })
        .collect()
}

/// TVDT and MHDT (when present) of a cell, by grid.
pub type CellFields = HashMap<(i32, i32), (Vec<u8>, Option<Vec<u8>>)>;

/// TVDT and MHDT of every exterior cell of a plugin's worldspace (by EditorID), by grid.
pub fn plugin_tvdt(path: &Path, world: &str) -> Result<CellFields> {
    let pl = lodforge_esp::Plugin::open(path)?;
    let mut out = HashMap::new();
    fn walk(es: Entries<'_>, world: &str, inside: bool, last_edid: &mut String, out: &mut CellFields) -> Result<()> {
        for e in es {
            match e? {
                Entry::Record(r) if &r.sig.0 == b"WRLD" => {
                    let data = r.data()?;
                    *last_edid = fields(&data).find(|f| &f.sig.0 == b"EDID").map(|f| f.zstring()).unwrap_or_default();
                }
                Entry::Record(r) if &r.sig.0 == b"CELL" && inside => {
                    let data = r.data()?;
                    let (mut grid, mut tvdt, mut mhdt) = (None, None, None);
                    for f in fields(&data) {
                        match &f.sig.0 {
                            b"XCLC" if f.data.len() >= 8 => grid = Some((i32::from_le_bytes(f.data[0..4].try_into().unwrap()), i32::from_le_bytes(f.data[4..8].try_into().unwrap()))),
                            b"TVDT" => tvdt = Some(f.data.to_vec()),
                            b"MHDT" => mhdt = Some(f.data.to_vec()),
                            _ => {}
                        }
                    }
                    if let (Some(g), Some(t)) = (grid, tvdt) {
                        out.insert(g, (t, mhdt));
                    }
                }
                Entry::Record(_) => {}
                Entry::Group(g) => {
                    let inside = if g.group_type == 1 { last_edid.eq_ignore_ascii_case(world) } else { inside };
                    walk(g.entries(), world, inside, last_edid, out)?;
                }
            }
        }
        Ok(())
    }
    walk(pl.top_entries(), world, false, &mut String::new(), &mut out)?;
    Ok(out)
}

/// The profile's load order with extra plugins added the way DynDOLOD has them loaded while it
/// computes occlusion: master-flagged files after the last master, others at the end.
pub fn extend_load_order(p: &crate::profile::Profile, extra_plugins: &[PathBuf]) -> Result<LoadOrder> {
    let mut names = p.plugin_names.clone();
    let extra: HashMap<String, PathBuf> = extra_plugins.iter().filter_map(|path| Some((path.file_name()?.to_string_lossy().into_owned(), path.clone()))).collect();
    let mut at = p.load_order.plugins.iter().rposition(|lp| lp.plugin.is_master()).map_or(0, |i| i + 1);
    for path in extra_plugins {
        let Some(n) = path.file_name() else { continue };
        if lodforge_esp::Plugin::open(path)?.is_master() {
            names.insert(at, n.to_string_lossy().into_owned());
            at += 1;
        } else {
            names.push(n.to_string_lossy().into_owned());
        }
    }
    Ok(LoadOrder::open(&names, |n| extra.get(n).cloned().or_else(|| p.vfs.loose_path(n).map(|q| q.to_owned())))?)
}

/// Localized strings of the load order's plugins (`strings\<plugin>_english.strings`), read
/// through the VFS once per plugin.
pub struct Strings<'a> {
    read: &'a dyn Fn(&str) -> Option<Vec<u8>>,
    tables: RefCell<HashMap<String, Option<Vec<u8>>>>,
}

impl<'a> Strings<'a> {
    pub fn new(read: &'a dyn Fn(&str) -> Option<Vec<u8>>) -> Strings<'a> {
        Strings { read, tables: RefCell::new(HashMap::new()) }
    }

    pub fn lookup(&self, plugin: &str, id: u32) -> Option<String> {
        let stem = plugin.rsplit_once('.').map_or(plugin, |s| s.0).to_ascii_lowercase();
        let mut tables = self.tables.borrow_mut();
        let table = tables.entry(stem.clone()).or_insert_with(|| (self.read)(&format!("strings\\{stem}_english.strings")));
        occlusion::strings_lookup(table.as_deref()?, id)
    }
}

/// The visibility model for the settings, with the fit overrides of `LODFORGE_OCC_MODEL`
/// (`clearance=<units>`, `occluder=ground|top`, comma separated; model experiments only).
fn model_for(params: Params) -> Model {
    let mut m = params.model();
    if let Ok(spec) = std::env::var("LODFORGE_OCC_MODEL") {
        for term in spec.split(',') {
            let Some((k, v)) = term.trim().split_once('=') else { continue };
            let v = v.trim().to_ascii_lowercase();
            match (k.trim(), v.as_str()) {
                ("clearance", v) => m.clearance = v.parse().unwrap_or(m.clearance),
                ("occluder", "ground") => m.occluder = Occluder::Ground,
                ("occluder", "top") => m.occluder = Occluder::Top,
                _ => {}
            }
        }
        eprintln!("occlusion model: {m:?}");
    }
    m
}

/// Computes the TVDT of a main worldspace and its child worldspaces and adds them to `pb` (the
/// content of Occlusion.esp). `mode(world, is_main)` gives the `OcclusionMode` of a worldspace,
/// `None` to skip it; `+Border` is dropped when no cell of a worldspace lies in a border region.
/// Returns (worldspace, cells) per worldspace done.
pub fn build_worlds(lo: &LoadOrder, read: &dyn Fn(&str) -> Option<Vec<u8>>, main_world: &str, heightmap: Option<&[u8]>, params: Params, mode: &dyn Fn(&str, bool) -> Option<String>, pb: &mut PluginBuilder) -> Result<Vec<(String, usize)>> {
    let worlds = lodforge_esp::scan_worldspaces(lo)?;
    let main = worlds.iter().find(|w| w.editor_id.eq_ignore_ascii_case(main_world)).with_context(|| format!("worldspace {main_world} not found"))?;
    let border = occlusion::border_regions(lo)?;
    let objects = heightmap.map(occlusion::object_heightmap).unwrap_or_default();
    let main_grid = occlusion::lod_settings(read, &main.editor_id).unwrap_or(Grid::TAMRIEL);
    let strings = Strings::new(read);
    let text = |p: &str, id: u32| strings.lookup(p, id);
    let model = model_for(params);
    let main_lands: Vec<LandCell> = occlusion::land_cells(lo, main, occlusion::world_heights(main, None).1)?;
    let mut done = Vec::new();
    for world in occlusion::occlusion_worlds(&worlds, main, true) {
        let is_main = world.form_id == main.form_id;
        let Some(mode_text) = mode(&world.editor_id, is_main) else { continue };
        let grid = occlusion::lod_settings(read, &world.editor_id).unwrap_or(main_grid);
        let setup = occlusion::prepare_world(lo, read, main, world, grid, None, (!is_main).then_some(main_lands.as_slice()), &objects)?;
        let mode = Mode::parse(&mode_text);
        let mut cells = occlusion::select_cells(lo, world, &setup.lands, &grid, &mode, &border)?;
        if cells.is_empty() && mode.border {
            eprintln!("occlusion: {}: no cell lies in a Border Region, +Border dropped", world.editor_id);
            cells = occlusion::select_cells(lo, world, &setup.lands, &grid, &mode.without_border(), &border)?;
        }
        let calc = Calculator::new(&setup.map, grid.origin, grid.stride, model).with_context(|| format!("{}: lodsettings stride {} is not a power of two times four", world.editor_id, grid.stride))?;
        let tv = occlusion::compute_cells(&calc, &cells);
        done.push((world.editor_id.clone(), tv.len()));
        if !tv.is_empty() {
            pb.worlds.extend(occlusion::world_content(lo, world, &tv, &text, &|c| setup.map.mhdt(c.0, c.1)));
        }
    }
    Ok(done)
}

/// Comparison with the golden plugin, over the set bits of the whole TVDT field.
#[derive(Default)]
struct Score {
    cells: usize,
    same: usize,
    both: usize,
    either: usize,
    only_ours: usize,
    only_gold: usize,
}

impl Score {
    fn add(&mut self, o: &Score) {
        self.cells += o.cells;
        self.same += o.same;
        self.both += o.both;
        self.either += o.either;
        self.only_ours += o.only_ours;
        self.only_gold += o.only_gold;
    }

    fn jaccard(&self) -> f64 {
        self.both as f64 / self.either.max(1) as f64
    }
}

fn bits(t: &[u8]) -> impl Iterator<Item = bool> + '_ {
    (0..t.len() * 8).map(move |p| t[p >> 3] >> (p & 7) & 1 == 1)
}

/// Block map of one cell: ours against the golden, LOD4 blocks labelled by their south-west cell.
fn show_map(c: (i32, i32), diff: usize, ours: &Tvdt, gold: &Tvdt, grid: &Grid) {
    println!("cell {c:?}: {diff} bits differ; '#' both, 'o' ours only, 'g' golden only, '*' the observer's block; rows and columns are LOD4 blocks labelled by their south-west cell");
    let nb = ours.layout.nb;
    let (vo, vg) = (ours.leaves(), gold.leaves());
    let me = (((c.0 - grid.origin.0) / 4) as usize, ((c.1 - grid.origin.1) / 4) as usize);
    let any = |bx: usize, by: usize| vo[bx * nb + by] || vg[bx * nb + by];
    let rows: Vec<usize> = (0..nb).rev().filter(|&by| (0..nb).any(|bx| any(bx, by))).collect();
    let cols: Vec<usize> = (0..nb).filter(|&bx| (0..nb).any(|by| any(bx, by))).collect();
    let (c0, c1) = (cols.first().copied().unwrap_or(0), cols.last().copied().unwrap_or(0));
    println!("       x from {} to {}", c0 as i32 * 4 + grid.origin.0, c1 as i32 * 4 + grid.origin.0);
    for by in rows {
        let line: String = (c0..=c1)
            .map(|bx| match (vo[bx * nb + by], vg[bx * nb + by]) {
                _ if (bx, by) == me => '*',
                (true, true) => '#',
                (true, false) => 'o',
                (false, true) => 'g',
                (false, false) => '.',
            })
            .collect();
        println!("  {:4} {line}", by as i32 * 4 + grid.origin.1);
    }
}

pub fn run(a: &Args) -> Result<()> {
    let p = crate::profile::open(&a.instance, &a.profile, &a.instance.join("Stock Game"))?;
    let extended;
    let lo = if a.extra_plugin.is_empty() {
        &p.load_order
    } else {
        extended = extend_load_order(&p, &a.extra_plugin)?;
        &extended
    };
    let worlds = lodforge_esp::scan_worldspaces(lo)?;
    let main = worlds.iter().find(|w| w.editor_id.eq_ignore_ascii_case(&a.world)).with_context(|| format!("worldspace {} not found", a.world))?;
    let mut todo = occlusion::occlusion_worlds(&worlds, main, !a.no_child_worlds);
    if let Some(only) = &a.only_worlds {
        let names: Vec<String> = only.split(',').map(|s| s.trim().to_ascii_lowercase()).collect();
        todo.retain(|w| names.contains(&w.editor_id.to_ascii_lowercase()));
    }
    let border = occlusion::border_regions(lo)?;
    let read = |path: &str| p.vfs.read(path).ok().flatten();
    let objects = match &a.heightmap {
        Some(h) => occlusion::object_heightmap(&std::fs::read(h).context("reading the heightmap")?),
        None => Vec::new(),
    };
    let main_grid = occlusion::lod_settings(&read, &main.editor_id).unwrap_or(Grid::TAMRIEL);
    let strings = Strings::new(&read);
    let text = |pl: &str, id: u32| strings.lookup(pl, id);
    let params = Params { quality: a.quality, radius: a.radius, height: a.height };
    let model = model_for(params);
    let mut pb = occlusion::plugin_builder();
    let mut total = Score::default();
    let mut total_leaves = Score::default();
    let (mut selection_extra, mut selection_missing) = (0usize, 0usize);
    let t_all = Instant::now();
    // Child worldspaces take the main worldspace's landscape where they have none, also when
    // --only-worlds leaves the main worldspace out.
    let main_lands: Vec<LandCell> = occlusion::land_cells(lo, main, occlusion::world_heights(main, None).1)?;
    let mut dump = a.dump_heights.as_ref().map(std::fs::File::create).transpose().context("creating the heights dump")?;
    for world in todo {
        let t = Instant::now();
        let is_main = world.form_id == main.form_id;
        let mut grid = occlusion::lod_settings(&read, &world.editor_id).unwrap_or(main_grid);
        if let Some(o) = &a.origin {
            grid.origin = pair(o).context("--origin x,y")?;
        }
        if let Some(s) = a.stride {
            grid.stride = s;
        }
        let mode = if is_main { Mode::parse(&a.mode) } else { Mode::parse(&a.mode).without_border() };
        let setup = occlusion::prepare_world(lo, &read, main, world, grid, a.land_height, (!is_main).then_some(main_lands.as_slice()), &objects)?;
        if is_main && let Some(b) = &a.terrain_bin {
            let g = read_terrain_bin(&std::fs::read(b).context("reading the terrain .bin")?);
            let gm: HashMap<(i32, i32), &LandCell> = g.iter().map(|c| ((c.x, c.y), c)).collect();
            let same = setup.lands.iter().filter(|c| gm.get(&(c.x, c.y)).is_some_and(|g| g.water.to_bits() == c.water.to_bits() && g.vhgt[..4 + 33 * 33] == c.vhgt[..4 + 33 * 33])).count();
            println!("terrain from the load order: {} cells, {same} identical to the golden .bin's {}", setup.lands.len(), g.len());
        }
        let golden = match &a.golden {
            Some(g) => plugin_tvdt(g, &world.editor_id)?,
            None => HashMap::new(),
        };
        let mut cells: Vec<(i32, i32)> = match &a.cells {
            Some(s) => s.split(';').filter_map(pair).collect(),
            None if a.golden_cells => {
                let mut c: Vec<(i32, i32)> = golden.keys().copied().collect();
                c.sort_by_key(|&(x, y)| (y, x));
                c
            }
            None => occlusion::select_cells(lo, world, &setup.lands, &grid, &mode, &border)?,
        };
        if let Some(n) = a.limit {
            let step = (cells.len() / n.max(1)).max(1);
            cells = cells.into_iter().step_by(step).take(n).collect();
        }
        let calc = Calculator::new(&setup.map, grid.origin, grid.stride, model).with_context(|| format!("{}: lodsettings stride {} is not a power of two times four", world.editor_id, grid.stride))?;
        let tv = occlusion::compute_cells(&calc, &cells);
        println!("{}: {} land cells, mode \"{}\", {} cells, land {} water {}, {:.1} s", world.editor_id, setup.lands.len(), mode.text, cells.len(), setup.land_height, setup.water_height, t.elapsed().as_secs_f64());
        if setup.lod_meshes > 0 {
            println!("  {} terrain LOD meshes filled the cells without landscape", setup.lod_meshes);
        }
        if let Ok(spec) = std::env::var("LODFORGE_OCC_DUMP_CELLS") {
            // "x0,x1,y[,step]": ground min/max and top of a row of cells (map diagnostics).
            let v: Vec<i32> = spec.split(',').filter_map(|s| s.trim().parse().ok()).collect();
            if v.len() >= 3 {
                let step = v.get(3).copied().unwrap_or(4).max(1);
                let mut x = v[0];
                while x <= v[1] {
                    match setup.map.cell_stats(x, v[2]) {
                        Some((lo_, hi, top)) => println!("  cell {x},{}: land {} ground {lo_:.0}..{hi:.0} top {top:.0}", v[2], setup.map.has_land(x, v[2])),
                        None => println!("  cell {x},{}: outside the map (default {})", v[2], setup.default),
                    }
                    x += step;
                }
            }
        }
        if let Some(f) = &mut dump {
            let water: HashMap<(i32, i32), f32> = setup.lands.iter().map(|c| ((c.x, c.y), c.water)).collect();
            for c in &cells {
                if let Some((lo_, hi, top)) = setup.map.cell_stats(c.0, c.1) {
                    writeln!(f, "{} {},{} water: {}, min: {}, max: {}, top: {}", world.editor_id, c.0, c.1, water.get(c).copied().unwrap_or(setup.water_height), lo_, hi, top)?;
                }
            }
        }
        if a.out.is_some() && !tv.is_empty() {
            pb.worlds.extend(occlusion::world_content(lo, world, &tv, &text, &|c| setup.map.mhdt(c.0, c.1)));
        }
        if golden.is_empty() {
            continue;
        }
        let mine: HashSet<(i32, i32)> = cells.iter().copied().collect();
        let mut extra: Vec<_> = mine.iter().filter(|c| !golden.contains_key(c)).collect();
        let mut missing: Vec<_> = golden.keys().filter(|c| !mine.contains(c)).collect();
        extra.sort();
        missing.sort();
        if a.cells.is_none() && a.limit.is_none() && (!extra.is_empty() || !missing.is_empty()) {
            println!("  cells only ours ({}) {:?}{}", extra.len(), &extra[..extra.len().min(12)], if extra.len() > 12 { " ..." } else { "" });
            println!("  cells only golden ({}) {:?}{}", missing.len(), &missing[..missing.len().min(12)], if missing.len() > 12 { " ..." } else { "" });
            selection_extra += extra.len();
            selection_missing += missing.len();
        }
        let mut sc = Score::default();
        let mut leaves = Score::default();
        let mut worst: Vec<(usize, (i32, i32), &Vec<u8>, &Vec<u8>)> = Vec::new();
        for (c, o) in &tv {
            let Some((g, _)) = golden.get(c) else { continue };
            sc.cells += 1;
            if o == g {
                sc.same += 1;
            }
            let mut diff = 0;
            for (x, y) in bits(o).zip(bits(g)) {
                sc.both += usize::from(x && y);
                sc.either += usize::from(x || y);
                sc.only_ours += usize::from(x && !y);
                sc.only_gold += usize::from(!x && y);
                diff += usize::from(x != y);
            }
            if diff > 0 {
                worst.push((diff, *c, o, g));
            }
            let (lo, lg) = (Tvdt::from_bytes(calc.layout(), o.clone()).leaves(), Tvdt::from_bytes(calc.layout(), g.clone()).leaves());
            for (x, y) in lo.iter().zip(&lg) {
                leaves.both += usize::from(*x && *y);
                leaves.either += usize::from(*x || *y);
                leaves.only_ours += usize::from(*x && !*y);
                leaves.only_gold += usize::from(!*x && *y);
            }
        }
        worst.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        for &(diff, c, o, g) in worst.iter().take(a.show) {
            show_map(c, diff, &Tvdt::from_bytes(calc.layout(), o.clone()), &Tvdt::from_bytes(calc.layout(), g.clone()), &grid);
        }
        if a.analyze {
            let (mut only_gold, mut only_ours, mut both) = (Obstructions::default(), Obstructions::default(), Obstructions::default());
            let nb = calc.layout().nb;
            for (c, o) in &tv {
                let Some((g, _)) = golden.get(c) else { continue };
                let (vo, vg) = (Tvdt::from_bytes(calc.layout(), o.clone()).leaves(), Tvdt::from_bytes(calc.layout(), g.clone()).leaves());
                for bx in 0..nb {
                    for by in 0..nb {
                        let (x, y) = (vo[bx * nb + by], vg[bx * nb + by]);
                        if x == y && !x {
                            continue;
                        }
                        let Some(need) = calc.needed_clearance(c.0, c.1, bx, by) else { continue };
                        let dist = ((bx as f64 * 4.0 + 1.5 - (c.0 - grid.origin.0) as f64).powi(2) + (by as f64 * 4.0 + 1.5 - (c.1 - grid.origin.1) as f64).powi(2)).sqrt();
                        match (x, y) {
                            (false, true) => only_gold.add(dist, need),
                            (true, false) => only_ours.add(dist, need),
                            _ => both.add(dist, need),
                        }
                    }
                }
            }
            only_gold.print("only golden");
            only_ours.print("only ours");
            both.print("both");
        }
        if let Some(pc) = a.probe.as_deref().and_then(pair)
            && let (Some(o), Some((g, _))) = (tv.iter().find(|(c, _)| *c == pc).map(|(_, t)| t), golden.get(&pc))
        {
            let nb = calc.layout().nb;
            let (vo, vg) = (Tvdt::from_bytes(calc.layout(), o.clone()).leaves(), Tvdt::from_bytes(calc.layout(), g.clone()).leaves());
            let (gx, gy) = (pc.0 - grid.origin.0, pc.1 - grid.origin.1);
            println!("probe {pc:?}: blocks within {} cells that only one side sees (block = south-west cell)", a.probe_cells);
            for by in 0..nb {
                for bx in 0..nb {
                    let (x, y) = (vo[bx * nb + by], vg[bx * nb + by]);
                    if x == y {
                        continue;
                    }
                    let (x0, y0) = (bx as i32 * 4, by as i32 * 4);
                    if (x0 - gx).max(gx - x0 - 3).max(0) > a.probe_cells || (y0 - gy).max(gy - y0 - 3).max(0) > a.probe_cells {
                        continue;
                    }
                    let Some(line) = calc.best_line(pc.0, pc.1, bx, by) else { continue };
                    println!("  {} block ({}, {}): {line}", if y { "only golden" } else { "only ours  " }, x0 + grid.origin.0, y0 + grid.origin.1);
                }
            }
        }
        println!("  vs golden: {}/{} cells identical (golden has {}), bit Jaccard {:.4} (set bits: both {}, only ours {}, only golden {})", sc.same, sc.cells, golden.len(), sc.jaccard(), sc.both, sc.only_ours, sc.only_gold);
        let golden_leaves = leaves.both + leaves.only_gold;
        println!(
            "  visible LOD4 blocks: golden {golden_leaves}, ours {}, hidden by us but visible in golden {} ({:.2}% of golden's), visible to us only {} ({:.2}%), block Jaccard {:.4}",
            leaves.both + leaves.only_ours,
            leaves.only_gold,
            100.0 * leaves.only_gold as f64 / golden_leaves.max(1) as f64,
            leaves.only_ours,
            100.0 * leaves.only_ours as f64 / golden_leaves.max(1) as f64,
            leaves.jaccard()
        );
        total.add(&sc);
        total_leaves.add(&leaves);
        // MHDT of the cells whose winning record has none: ours against the golden's.
        let (mut n, mut same, mut offset, mut eq, mut hi, mut lo_, mut off_by_one) = (0usize, 0usize, 0usize, 0usize, 0usize, 0usize, 0usize);
        let mut offsets_differ: Vec<String> = Vec::new();
        for (c, _) in &tv {
            let (Some((_, Some(g))), Some(&rr)) = (golden.get(c), world.cell_records.get(c)) else { continue };
            if occlusion::cell_record_info(lo, rr)?.has_mhdt {
                continue;
            }
            let Some(m) = setup.map.mhdt(c.0, c.1) else { continue };
            n += 1;
            same += usize::from(&m == g);
            if m[..4] == g[..4] {
                offset += 1;
            } else if offsets_differ.len() < 6 {
                let f = |b: &[u8]| f32::from_le_bytes(b[..4].try_into().unwrap());
                offsets_differ.push(format!("{c:?} ours {} golden {} (ground min {:.0})", f(&m), f(g), setup.map.cell_stats(c.0, c.1).map_or(0.0, |s| s.0)));
            }
            for (x, y) in m[4..].iter().zip(&g[4..]) {
                eq += usize::from(x == y);
                hi += usize::from(x > y);
                lo_ += usize::from(x < y);
                off_by_one += usize::from(x.abs_diff(*y) == 1);
            }
        }
        if n > 0 {
            println!("  MHDT computed for {n} cells without one: {same} identical, {offset} same offset; bytes equal {eq}, ours higher {hi}, golden higher {lo_} (off by one {off_by_one})");
            for s in offsets_differ {
                println!("    offset differs: {s}");
            }
        }
    }
    println!("total {:.1} s", t_all.elapsed().as_secs_f64());
    if total.cells > 0 {
        println!("all worldspaces vs golden: {}/{} cells identical, bit Jaccard {:.4}", total.same, total.cells, total.jaccard());
        let golden_leaves = total_leaves.both + total_leaves.only_gold;
        println!(
            "all worldspaces visible LOD4 blocks: golden {golden_leaves}, ours {}, hidden by us but visible in golden {} ({:.2}%), visible to us only {} ({:.2}%), block Jaccard {:.4}, cells only ours {selection_extra}, cells only golden {selection_missing}",
            total_leaves.both + total_leaves.only_ours,
            total_leaves.only_gold,
            100.0 * total_leaves.only_gold as f64 / golden_leaves.max(1) as f64,
            total_leaves.only_ours,
            100.0 * total_leaves.only_ours as f64 / golden_leaves.max(1) as f64,
            total_leaves.jaccard()
        );
    }
    if let Some(out) = &a.out {
        let bytes = pb.write(lo);
        std::fs::write(out, &bytes)?;
        let pl = lodforge_esp::Plugin::open(out)?;
        println!("wrote {} ({} bytes, {} worldspaces), masters {:?}", out.display(), bytes.len(), pb.worlds.len(), pl.header.masters);
    }
    Ok(())
}
