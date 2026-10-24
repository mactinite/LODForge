//! `lodforge tree-models`: generated 3D tree LOD models (Ultra phase 3,
//! docs/ultra-tree-models-plan.md), written as
//! `meshes\dyndolod\lod\trees\<stem>_<crc>passthru_lod[_1|_2].nif` so DynDOLOD-mode LOD discovery
//! finds them like installed models. `--inspect` prints how trees split and their level counts.
//!
//! Selection follows TreeLODGen: one model per distinct full model of a tree base whose bounds
//! pass the size test (`tree_report::generator_candidate`), skipped when a
//! `<stem>_<crc>passthru_lod.nif` is already installed (unless `--all`); models listed in
//! `LODForge_TreeModels.txt` (an earlier run's) do not count as installed and are replaced.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use rayon::prelude::*;

use lodforge_dyndolod::assign::Resolver;
use lodforge_nif::Block;
use lodforge_treemodel::levels::{DEFAULT_LEVELS, LevelParams};
use lodforge_treemodel::{AlphaImage, Kind, collect, split};

use crate::profile::{Profile, ProfileArgs};

#[derive(clap::Args)]
pub struct Args {
    #[command(flatten)]
    pub profile: ProfileArgs,
    /// Print the bark/foliage split and level counts of these full models (paths below `meshes\`,
    /// comma separated) instead of generating.
    #[arg(long, value_delimiter = ',')]
    pub inspect: Vec<String>,
    /// Output mod folder for the generated models.
    #[arg(long)]
    pub out: Option<PathBuf>,
    /// Also generate for trees that already have an installed 3D LOD model.
    #[arg(long)]
    pub all: bool,
    /// Only models whose path contains one of these (comma separated, lower case).
    #[arg(long, value_delimiter = ',')]
    pub only: Vec<String>,
    /// DynDOLOD's `Edit Scripts\DynDOLOD` folder (default: `<instance>\tools\DynDOLOD\Edit Scripts\DynDOLOD`).
    #[arg(long)]
    pub dyndolod_config: Option<PathBuf>,
    #[arg(long, default_value = "High")]
    pub preset: String,
    /// Level budgets `keep:area:bark:floor[:max_scale]` for LOD4,LOD8,LOD16 (default: TreeLODGen's
    /// `0.4:0.8:400:2500,0.15:0.7:150:1000,0.06:0.6:60:400`).
    #[arg(long)]
    pub levels: Option<String>,
    /// With `--inspect`: render the full model and its generated levels side by side into
    /// `<dir>\<stem>.png` (TexGen's billboard camera, same scale).
    #[arg(long)]
    pub render: Option<PathBuf>,
}

/// Texture alpha through the VFS, decoded once per path.
pub struct Textures<'a> {
    p: &'a Profile,
    cache: Mutex<HashMap<String, Option<Arc<AlphaImage>>>>,
}

impl<'a> Textures<'a> {
    pub fn new(p: &'a Profile) -> Self {
        Textures { p, cache: Mutex::new(HashMap::new()) }
    }

    pub fn get(&self, path: &str) -> Option<Arc<AlphaImage>> {
        if let Some(v) = self.cache.lock().unwrap().get(path) {
            return v.clone();
        }
        let img = (|| {
            let bytes = self.p.vfs.read(path).ok()??;
            let info = lodforge_dds::parse(&bytes).ok()?;
            let img = lodforge_dds::decode(&bytes, &info, 0).ok()?;
            Some(Arc::new(AlphaImage::from_rgba(img.width, img.height, &img.rgba)))
        })();
        self.cache.lock().unwrap().insert(path.to_string(), img.clone());
        img
    }
}

/// `--levels` / `--tree-model-levels`, or TreeLODGen's defaults.
pub fn levels_arg(s: Option<&str>) -> Result<Vec<LevelParams>> {
    match s {
        Some(s) => LevelParams::parse_list(s).context("levels: keep:area:bark:floor[:max_scale] per level, comma separated"),
        None => Ok(DEFAULT_LEVELS.to_vec()),
    }
}

/// List of the models a run wrote, in the models folder.
pub const LIST: &str = "LODForge_TreeModels.txt";

/// One generated tree.
pub struct Made {
    pub model: String,
    pub files: Vec<String>,
    pub triangles: Vec<(usize, usize)>,
    pub bytes: usize,
    pub notes: Vec<String>,
}

#[derive(Default)]
pub struct Report {
    pub candidates: usize,
    pub installed: usize,
    pub made: Vec<Made>,
    pub failed: Vec<(String, String)>,
}

impl Report {
    pub fn summary(&self) -> String {
        let bytes: usize = self.made.iter().map(|m| m.bytes).sum();
        let lod4: usize = self.made.iter().filter_map(|m| m.triangles.first()).map(|(c, t)| c + t).sum();
        format!(
            "tree models: {} generated of {} candidate models ({} already have 3D LOD, {} failed); {:.1} MB, {} LOD4 triangles in all, {:.0} per tree",
            self.made.len(),
            self.candidates,
            self.installed,
            self.failed.len(),
            bytes as f64 / 1048576.0,
            lod4,
            lod4 as f64 / self.made.len().max(1) as f64
        )
    }
}

/// Generates the models of every candidate tree into `out` (`meshes\dyndolod\lod\trees\`).
pub fn generate(p: &Profile, c: &crate::dyndolod_build::Config, r: &Resolver<'_>, out: &Path, params: &[LevelParams], all: bool, only: &[String]) -> Result<Report> {
    let folder = out.join("meshes").join(lodforge_treemodel::write::FOLDER);
    std::fs::create_dir_all(&folder)?;
    // An earlier run's models: removed, and never "installed".
    let own: HashSet<String> = std::fs::read_to_string(folder.join(LIST)).unwrap_or_default().lines().map(|l| l.trim().to_ascii_lowercase()).filter(|l| !l.is_empty()).collect();
    for f in &own {
        let _ = std::fs::remove_file(folder.join(f));
    }
    // Distinct models of candidate tree bases.
    let mut models: BTreeMap<String, ()> = BTreeMap::new();
    for b in c.bases.values() {
        if !lodforge_dyndolod::assign::is_tree(b) {
            continue;
        }
        let model = lodforge_dyndolod::bases::model_path(&b.model);
        if crate::tree_report::generator_candidate(&model, b.bounds) && (only.is_empty() || only.iter().any(|o| model.contains(o.as_str()))) {
            models.insert(model, ());
        }
    }
    let mut rep = Report { candidates: models.len(), ..Default::default() };
    let mut todo: Vec<(String, u32)> = Vec::new();
    for model in models.keys() {
        let Some(crc) = r.model_crc(model) else {
            rep.failed.push((model.clone(), "mesh not found".into()));
            continue;
        };
        let stem = model.rsplit('\\').next().unwrap_or(model).trim_end_matches(".nif");
        let name = lodforge_treemodel::write::file_name(stem, crc, 0);
        if !all && !own.contains(&name) && c.index.get(&name).is_some() {
            rep.installed += 1;
            continue;
        }
        todo.push((model.clone(), crc));
    }
    let textures = Textures::new(p);
    let results: Vec<Result<Made, (String, String)>> = todo
        .par_iter()
        .map(|(model, crc)| {
            let bytes = p.vfs.read(model).ok().flatten().ok_or_else(|| (model.clone(), "mesh not readable".to_string()))?;
            let g = lodforge_treemodel::generate(&bytes, &|t| textures.get(t), params).map_err(|e| (model.clone(), format!("{e:?}")))?;
            let stem = model.rsplit('\\').next().unwrap_or(model).trim_end_matches(".nif");
            let mut made = Made { model: model.clone(), files: Vec::new(), triangles: g.triangles.clone(), bytes: 0, notes: g.notes.clone() };
            for (level, data) in g.levels.iter().enumerate() {
                let Some(data) = data else { continue };
                let name = lodforge_treemodel::write::file_name(stem, *crc, level);
                std::fs::write(folder.join(&name), data).map_err(|e| (model.clone(), e.to_string()))?;
                made.bytes += data.len();
                made.files.push(name);
            }
            Ok(made)
        })
        .collect();
    for res in results {
        match res {
            Ok(m) => rep.made.push(m),
            Err(e) => rep.failed.push(e),
        }
    }
    rep.made.sort_by(|a, b| a.model.cmp(&b.model));
    let list: String = rep.made.iter().flat_map(|m| m.files.iter()).map(|f| format!("{f}\r\n")).collect();
    std::fs::write(folder.join(LIST), list)?;
    Ok(rep)
}

fn inspect(p: &Profile, models: &[String], render: Option<&Path>, params: &[LevelParams]) -> Result<()> {
    let textures = Textures::new(p);
    for m in models {
        let path = lodforge_dyndolod::bases::model_path(m);
        let bytes = p.vfs.read(&path)?.with_context(|| format!("{path} not found"))?;
        let nif = lodforge_nif::parse_all_geometry(&bytes).with_context(|| format!("parsing {path}"))?;
        let shapes = collect(&nif);
        let mut notes = Default::default();
        let parts = split(&nif, &shapes, &|t| textures.get(t), &mut notes);
        println!("{path}: {} source shapes", shapes.len());
        for (i, s) in shapes.iter().enumerate() {
            let Some(Block::TriShape(ts)) = nif.blocks.get(s.block) else { continue };
            let diffuse = lodforge_treemodel::split::diffuse(&nif, ts).unwrap_or_default();
            let xf = if s.xf.is_identity() { String::new() } else { format!(" scale {:.3} offset {:?}", s.xf.s, s.xf.t) };
            println!("  #{i} {:?} [{}] {} tris{}{} {diffuse}", s.name, nif.block_type(s.block), s.triangles, if s.skinned { " skinned" } else { "" }, xf);
            for pt in parts.iter().filter(|pt| pt.shape == i) {
                println!("      -> {:?} {} tris ({:?}){}", pt.kind, pt.triangles, pt.reason, pt.vertices.as_ref().map(|v| format!(", {} of {} vertices", v.len(), ts.positions.len())).unwrap_or_default());
            }
        }
        let sum = |k: Kind| parts.iter().filter(|pt| pt.kind == k).map(|pt| pt.triangles).sum::<usize>();
        println!("  crown {} tris, trunk {} tris{}", sum(Kind::Crown), sum(Kind::Trunk), if notes.0.is_empty() { String::new() } else { format!("; notes: {}", notes.0.join(", ")) });
        let t = std::time::Instant::now();
        let levels = lodforge_treemodel::levels::build(&nif, &shapes, &parts, params);
        for (i, l) in levels.iter().enumerate() {
            println!("  LOD{}: crown {} tris (keep {:.2}), trunk {} tris", [4, 8, 16][i.min(2)], l.triangles(Kind::Crown), l.keep, l.triangles(Kind::Trunk));
        }
        println!("  levels built in {} ms", t.elapsed().as_millis());
        if let Some(dir) = render {
            let g = lodforge_treemodel::generate(&bytes, &|t| textures.get(t), params).map_err(|e| anyhow::anyhow!("{e:?}"))?;
            let mut nifs = vec![lodforge_nif::parse(&bytes)?];
            for l in g.levels.iter().flatten() {
                nifs.push(lodforge_nif::parse(l)?);
            }
            let stem = path.rsplit('\\').next().unwrap_or(&path).trim_end_matches(".nif");
            std::fs::create_dir_all(dir)?;
            let png = dir.join(format!("{stem}.png"));
            render_sheet(p, &nifs, &png)?;
            println!("  rendered {}", png.display());
        }
    }
    Ok(())
}

/// The models side by side as TexGen's billboard camera sees them, on grey.
fn render_sheet(p: &Profile, nifs: &[lodforge_nif::Nif], png: &Path) -> Result<()> {
    let gpu = lodforge_gpu::Gpu::new()?;
    let renderer = lodforge_texgen::Renderer::new(&gpu);
    let s = lodforge_texgen::Settings::default();
    let mut imgs = Vec::new();
    let scenes: Vec<lodforge_texgen::Scene> = nifs.iter().map(lodforge_texgen::Scene::from_nif).collect();
    // One scale for all, so the full model is about 512 pixels high.
    let height = scenes.first().and_then(|s| s.bounds()).map(|(lo, hi)| hi[2] - lo[2]).unwrap_or(1.0).max(1.0);
    let scale = 512.0 * s.units_per_pixel / height;
    for scene in &scenes {
        let tex = lodforge_texgen::load_textures(scene, |t| p.vfs.read(t).ok().flatten().and_then(|b| lodforge_texgen::TexData::from_dds(&b, true)));
        if let Some(b) = lodforge_texgen::make_billboard(&gpu, &renderer, scene, &tex, scale, &s, false) {
            imgs.push(b);
        }
    }
    let h = imgs.iter().map(|b| b.height_px).max().unwrap_or(1);
    let w: u32 = imgs.iter().map(|b| b.width_px + 8).sum::<u32>().max(1);
    let mut out = vec![0u8; (w * h * 4) as usize];
    for px in out.chunks_exact_mut(4) {
        px.copy_from_slice(&[96, 104, 112, 255]);
    }
    let mut x0 = 0u32;
    for b in &imgs {
        let top = h - b.height_px;
        for y in 0..b.height_px {
            for x in 0..b.width_px {
                let si = ((y * b.width_px + x) * 4) as usize;
                let di = (((top + y) * w + x0 + x) * 4) as usize;
                let a = u32::from(b.rgba[si + 3]);
                for k in 0..3 {
                    out[di + k] = ((u32::from(b.rgba[si + k]) * a + u32::from(out[di + k]) * (255 - a)) / 255) as u8;
                }
            }
        }
        x0 += b.width_px + 8;
    }
    let f = std::fs::File::create(png)?;
    let mut e = png::Encoder::new(std::io::BufWriter::new(f), w, h);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    e.write_header()?.write_image_data(&out)?;
    Ok(())
}

pub fn run(a: &Args) -> Result<()> {
    let p = a.profile.open()?;
    let params = levels_arg(a.levels.as_deref())?;
    if !a.inspect.is_empty() {
        return inspect(&p, &a.inspect, a.render.as_deref(), &params);
    }
    let out = a.out.clone().context("--out or --inspect is needed")?;
    let dir = a.dyndolod_config.clone().unwrap_or_else(|| a.profile.instance.join(r"tools\DynDOLOD\Edit Scripts\DynDOLOD"));
    let c = crate::dyndolod_build::Config::load(&p, &dir, &a.preset, false)?;
    let t = std::time::Instant::now();
    let rep = crate::dyndolod_build::with_resolver(&p, &c, |r| generate(&p, &c, r, &out, &params, a.all, &a.only))?;
    println!("{} in {:.1} s", rep.summary(), t.elapsed().as_secs_f64());
    let mut notes: BTreeMap<String, usize> = BTreeMap::new();
    for m in &rep.made {
        for n in &m.notes {
            *notes.entry(n.split(':').next().unwrap_or(n).to_string() + n.strip_prefix("shader").unwrap_or("")).or_default() += 1;
        }
    }
    if !notes.is_empty() {
        println!("  notes: {}", notes.iter().map(|(n, k)| format!("{n} x{k}")).collect::<Vec<_>>().join(", "));
    }
    for (m, why) in rep.failed.iter().take(10) {
        println!("  failed {m}: {why}");
    }
    Ok(())
}
