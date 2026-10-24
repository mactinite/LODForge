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
    /// How the crown is made: `cloud` (default) or `prune` (TreeLODGen's pruned cards).
    #[arg(long, value_enum, default_value = "cloud")]
    pub crown: CrownMode,
    /// Cloud crowns: largest tile side in pixels.
    #[arg(long, default_value_t = 256)]
    pub cloud_max_tile: u32,
    /// Cloud crowns: keep the rendered soft coverage instead of dithering it into on/off alpha
    /// (the game alpha-tests the crown, which cuts away soft thin foliage).
    #[arg(long)]
    pub cloud_soft: bool,
    /// With `--inspect`: render the full model and its generated levels side by side into
    /// `<dir>\<stem>.png` (TexGen's billboard camera, same scale).
    #[arg(long)]
    pub render: Option<PathBuf>,
    /// With `--out`: after generating, contact sheets of every generated model (full model |
    /// LOD4), most placed trees first, as `<dir>\trees_NN.png` with a `.txt` index per page.
    #[arg(long)]
    pub sheets: Option<PathBuf>,
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

/// How a generated model's crown is made.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, clap::ValueEnum)]
pub enum CrownMode {
    /// A billboard cloud: a few planes textured with the leaves rendered onto them, over the
    /// reduced 3D trunk (keeps the crown's outline and density at a few dozen triangles).
    #[default]
    Cloud,
    /// TreeLODGen's pruned leaf cards (fewer of the source cards, enlarged).
    Prune,
}

/// Folder of the crown textures below `textures\`.
pub const CROWN_TEXTURES: &str = "dyndolod\\lod\\trees";

/// A tree's cloud crown and trunk levels, ready to render (CPU part, runs in parallel).
pub struct CloudPrep {
    nif: lodforge_nif::Nif,
    shapes: Vec<lodforge_treemodel::SourceShape>,
    /// Per level: the trunk parts (reduced to that level's bark budget).
    trunks: Vec<Vec<lodforge_treemodel::levels::LevelPart>>,
    tiles: Vec<lodforge_treemodel::cloud::Tile>,
    /// The first crown part's shape (for the crown's material).
    crown_shape: Option<usize>,
    pub crown_source: usize,
}

pub fn cloud_prep(bytes: &[u8], textures: &dyn Fn(&str) -> Option<Arc<AlphaImage>>, params: &[LevelParams], cp: &lodforge_treemodel::cloud::CloudParams) -> Result<CloudPrep, String> {
    let nif = lodforge_nif::parse_all_geometry(bytes).map_err(|e| format!("parse: {e}"))?;
    let shapes = collect(&nif);
    let mut notes = Default::default();
    let mut parts = split(&nif, &shapes, textures, &mut notes);
    if parts.is_empty() {
        return Err("no geometry".into());
    }
    // Small crown shapes with a texture of their own (bark blends, moss cards) stay 3D with the
    // trunk: sliced, they were projected away from the trunk they cover (floating bits).
    {
        let diffuse = |si: usize| match nif.blocks.get(shapes[si].block) {
            Some(lodforge_nif::Block::TriShape(s)) => lodforge_treemodel::split::diffuse(&nif, s).unwrap_or_default(),
            _ => String::new(),
        };
        let crown_total: usize = parts.iter().filter(|p| p.kind == Kind::Crown).map(|p| p.triangles).sum();
        if let Some(main) = parts.iter().filter(|p| p.kind == Kind::Crown).max_by_key(|p| p.triangles).map(|p| diffuse(p.shape)) {
            for p in parts.iter_mut() {
                if p.kind == Kind::Crown && diffuse(p.shape) != main && p.triangles * 100 < crown_total * 15 {
                    p.kind = Kind::Trunk;
                }
            }
        }
    }
    let sum = |k: Kind| parts.iter().filter(|p| p.kind == k).map(|p| p.triangles).sum::<usize>();
    // Leaves untouched (keep 1), bark reduced per level.
    let full: Vec<LevelParams> = params.iter().map(|l| LevelParams { keep: 1.0, area: 1.0, crown_floor: 0, ..*l }).collect();
    let levels = lodforge_treemodel::levels::build(&nif, &shapes, &parts, &full);
    let mut crown: Vec<&lodforge_treemodel::levels::LevelPart> = levels.first().map(|l| l.parts.iter().filter(|p| p.kind == Kind::Crown).collect()).unwrap_or_default();
    // The full bark goes into the slices too: the 3D trunk keeps only its budget, which drops the
    // branches, roots and twigs that hold the leaves (bare winter trees became floating specks).
    // In the slices they stay attached; the 3D trunk covers the main stem.
    let full_bark_params = [LevelParams { keep: 1.0, area: 1.0, crown_floor: 0, bark_tris: usize::MAX / 4, ..params[0] }];
    let full_bark = lodforge_treemodel::levels::build(&nif, &shapes, &parts, &full_bark_params);
    // Only the bark the 3D trunk lost (farther than a few percent of the tree's height from it);
    // the main stems drawn into the slices as well ghosted beside the 3D trunk.
    let mut lost: Vec<lodforge_treemodel::levels::LevelPart> = Vec::new();
    if cp.bark_in_slices && !crown.is_empty() {
        let kept: Vec<[f32; 3]> = levels[0]
            .parts
            .iter()
            .filter(|p| p.kind == Kind::Trunk)
            .flat_map(|p| {
                let pos: Vec<[f32; 3]> = p.mesh.positions.iter().map(|q| p.xf.point(*q)).collect();
                let mut pts = pos.clone();
                for tri in &p.mesh.triangles {
                    let [a, b, c] = tri.map(|i| pos[i as usize]);
                    pts.push([(a[0] + b[0] + c[0]) / 3.0, (a[1] + b[1] + c[1]) / 3.0, (a[2] + b[2] + c[2]) / 3.0]);
                    for (x, y) in [(a, b), (b, c), (c, a)] {
                        pts.push([(x[0] + y[0]) / 2.0, (x[1] + y[1]) / 2.0, (x[2] + y[2]) / 2.0]);
                    }
                }
                pts
            })
            .collect();
        let zs = || full_bark[0].parts.iter().flat_map(|p| p.mesh.positions.iter().map(|q| p.xf.point(*q)[2]));
        let height = zs().fold(f32::NEG_INFINITY, f32::max) - zs().fold(f32::INFINITY, f32::min);
        let r = (height * 0.03).max(15.0);
        let cell = |q: [f32; 3]| q.map(|v| (v / r).floor() as i32);
        let mut grid: HashMap<[i32; 3], Vec<[f32; 3]>> = HashMap::new();
        for q in &kept {
            grid.entry(cell(*q)).or_default().push(*q);
        }
        let near = |q: [f32; 3]| {
            let c = cell(q);
            (-1..=1).any(|dx| (-1..=1).any(|dy| (-1..=1).any(|dz| grid.get(&[c[0] + dx, c[1] + dy, c[2] + dz]).is_some_and(|v| v.iter().any(|k| (k[0] - q[0]).powi(2) + (k[1] - q[1]).powi(2) + (k[2] - q[2]).powi(2) < r * r)))))
        };
        for p in full_bark[0].parts.iter().filter(|p| p.kind == Kind::Trunk) {
            let mut m = p.mesh.clone();
            m.triangles.retain(|tri| {
                let [a, b, c] = tri.map(|i| p.xf.point(p.mesh.positions[i as usize]));
                !near([(a[0] + b[0] + c[0]) / 3.0, (a[1] + b[1] + c[1]) / 3.0, (a[2] + b[2] + c[2]) / 3.0])
            });
            m.delete_unreferenced();
            if !m.triangles.is_empty() {
                lost.push(lodforge_treemodel::levels::LevelPart { mesh: m, ..p.clone() });
            }
        }
        crown.extend(lost.iter());
    }
    let tiles = lodforge_treemodel::cloud::plan(&nif, &shapes, &crown, cp);
    if std::env::var_os("LODFORGE_CLOUD_DEBUG").is_some() {
        let total: usize = crown.iter().map(|p| p.mesh.triangle_count()).sum();
        eprintln!("cloud: {} crown tris in {} parts -> {} tiles holding {} tris", total, crown.len(), tiles.len(), tiles.iter().map(|t| t.triangles).sum::<usize>());
        for t in &tiles {
            eprintln!("  n {:?} d {:.0} a {:.0}..{:.0} b {:.0}..{:.0} {}x{} tris {}", t.n.map(|v| (v * 100.0).round() / 100.0), t.d, t.a0, t.a1, t.b0, t.b1, t.width, t.height, t.triangles);
        }
    }
    let crown_shape = crown.first().map(|p| p.shape);
    let trunks = levels.iter().map(|l| l.parts.iter().filter(|p| p.kind == Kind::Trunk).cloned().collect()).collect();
    Ok(CloudPrep { nif, shapes, trunks, tiles, crown_shape, crown_source: sum(Kind::Crown) })
}

/// The crown's material: the first crown shape's shader made plain, double-sided, without vertex
/// colours or tree animation, with the baked crown texture and its normal map.
fn crown_material(prep: &CloudPrep, diffuse: &str, normal: &str) -> lodforge_treemodel::write::Material {
    let src = prep.crown_shape.and_then(|i| match prep.nif.blocks.get(prep.shapes[i].block) {
        Some(lodforge_nif::Block::TriShape(s)) => Some(s),
        _ => None,
    });
    let mut shader = src.and_then(|s| match s.shader_property.and_then(|r| prep.nif.blocks.get(r as usize)) {
        Some(lodforge_nif::Block::LightingShader(sh)) => Some(sh.clone()),
        _ => None,
    }).unwrap_or_default();
    let mut dummy = vec![String::new(); 8];
    lodforge_treemodel::write::plain_lod_shader(&mut shader, &mut dummy);
    shader.shader_type = 0;
    shader.extra = lodforge_nif::LightingExtra::None;
    // The flags of DynDOLOD's tree billboards and passthru shapes in object LOD (golden Ultra run):
    // ZBuffer_Test only; ZBuffer_Write, Double_Sided and Vertex_Colors (white vertex colours on the
    // quads). The source leaf shader's other flags (Vertex_Alpha, which takes alpha from vertex
    // colours the quads did not have, among them) left the crowns unclipped squares in game.
    let soft = shader.flags2 & 0x0200_0000 != 0;
    shader.flags1 = 0x8000_0000;
    shader.flags2 = 0x31;
    // Soft lighting (thin foliage lit from behind, through a slot-2 mask) when the leaves have it,
    // with the crown diffuse as the mask, as DynDOLOD's own crown LOD models do
    // (reachtree02passthru_lod.nif: slot 2 = the branch diffuse; flags2 0x02000031 in object LOD).
    // Without it, vanilla broadleaf crowns lit at about half the brightness of the full tree.
    if soft {
        shader.flags2 |= 0x0200_0000;
    }
    shader.net = lodforge_nif::ObjectNet { name: shader.net.name.clone(), ..Default::default() };
    let alpha = src
        .and_then(|s| match s.alpha_property.and_then(|r| prep.nif.blocks.get(r as usize)) {
            Some(lodforge_nif::Block::Alpha { flags, threshold, .. }) => Some((*flags | 0x200, *threshold)),
            _ => None,
        })
        .or(Some((4844, 128)));
    let mut textures = vec![diffuse.to_string(), normal.to_string()];
    if soft {
        textures.push(diffuse.to_string());
    }
    lodforge_treemodel::write::Material { name: "Crown Cloud".into(), shader, textures, alpha }
}

/// Renders the tiles, packs the crown textures and writes every level. Returns the files (Data-
/// relative path, bytes) and the (crown, trunk) triangles per level.
pub fn cloud_finish(gpu: &lodforge_gpu::Gpu, renderer: &lodforge_texgen::Renderer, read: &dyn Fn(&str) -> Option<Vec<u8>>, prep: &CloudPrep, stem: &str, crc: u32, cp: &lodforge_treemodel::cloud::CloudParams) -> Result<(Vec<(String, Vec<u8>)>, Vec<(usize, usize)>), String> {
    // Albedo only (ambient 1, no direct light): the game lights the crown through its normal map,
    // as it lights the real leaves; TexGen's lit billboard colour would be lit twice.
    let mut s = lodforge_texgen::Settings::default();
    s.light.direct = 0.0;
    s.light.ambient = 1.0;
    // The leaves' own normals in the plane's frame (no billboard pull toward the viewer).
    s.light.normal_pull = 0.0;
    let mut files = Vec::new();
    let tex_base = format!("textures\\{CROWN_TEXTURES}\\{}_{crc:08x}crown", stem.to_ascii_lowercase());
    let (diffuse, normal) = (format!("{tex_base}.dds"), format!("{tex_base}_n.dds"));
    let mut crown_part = None;
    if !prep.tiles.is_empty() {
        let scenes: Vec<lodforge_texgen::Scene> = prep.tiles.iter().map(|t| lodforge_texgen::Scene::from_nif(&t.nif)).collect();
        // One texture set for the tree (every slice uses the same few textures), kept block
        // compressed for the GPU.
        let mut tex: HashMap<String, lodforge_texgen::TexData> = HashMap::new();
        for sc in &scenes {
            let more = lodforge_texgen::load_textures(sc, |t| if tex.contains_key(t) { None } else { read(t).and_then(|b| lodforge_texgen::TexData::for_gpu(gpu, &b)) });
            tex.extend(more);
        }
        let views: Vec<(lodforge_texgen::Item, lodforge_texgen::View)> = prep
            .tiles
            .iter()
            .zip(&scenes)
            .map(|(t, sc)| (lodforge_texgen::Item { scene: sc, textures: &tex, scale: 1.0 }, lodforge_texgen::View { yaw_deg: 0.0, x0: t.a0, x1: t.a1, z0: t.b0, z1: t.b1, width: t.width, height: t.height }))
            .collect();
        let mut imgs = lodforge_texgen::final_images(gpu, renderer, &views, &s, true);
        if cp.dither {
            for ((img, _), t) in imgs.iter_mut().zip(&prep.tiles) {
                lodforge_treemodel::cloud::dither_alpha(img, t.width);
            }
        }
        let sizes: Vec<(u32, u32)> = prep.tiles.iter().map(|t| (t.width, t.height)).collect();
        let (place, (aw, ah)) = lodforge_treemodel::cloud::pack(&sizes);
        let mut dif = vec![0u8; (aw * ah * 4) as usize];
        let mut nrm: Vec<u8> = (0..aw * ah).flat_map(|_| [128u8, 128, 255, 0]).collect();
        for ((img, n), (&(x, y), &(w, h))) in imgs.iter().zip(place.iter().zip(&sizes)) {
            for row in 0..h {
                let s0 = (row * w * 4) as usize;
                let d0 = (((y + row) * aw + x) * 4) as usize;
                let len = (w * 4) as usize;
                if img.len() >= s0 + len {
                    dif[d0..d0 + len].copy_from_slice(&img[s0..s0 + len]);
                }
                if let Some(n) = n
                    && n.len() >= s0 + len
                {
                    nrm[d0..d0 + len].copy_from_slice(&n[s0..s0 + len]);
                }
            }
        }
        crate::object_atlas::fill_average_color(&mut dif);
        for (path, rgba) in [(&diffuse, &dif), (&normal, &nrm)] {
            let blocks = gpu.encode_bc7(&[lodforge_gpu::RgbaImage { width: aw, height: ah, rgba }]).map_err(|e| e.to_string())?;
            files.push((path.clone(), lodforge_dds::write_bc7(aw, ah, &blocks)));
        }
        let mesh = lodforge_treemodel::cloud::quads(&prep.tiles, &place, (aw, ah));
        crown_part = Some(lodforge_treemodel::levels::LevelPart {
            kind: Kind::Crown,
            shape: prep.crown_shape.unwrap_or(0),
            xf: lodforge_treemodel::xf::Xf::IDENTITY,
            source_triangles: prep.crown_source,
            mesh,
            material: Some(crown_material(prep, &diffuse, &normal)),
        });
    }
    let mut tris = Vec::new();
    for (level, trunk) in prep.trunks.iter().enumerate() {
        let mut parts: Vec<lodforge_treemodel::levels::LevelPart> = crown_part.iter().cloned().collect();
        parts.extend(trunk.iter().cloned());
        let lv = lodforge_treemodel::levels::Level { parts, keep: 1.0 };
        tris.push((lv.triangles(Kind::Crown), lv.triangles(Kind::Trunk)));
        let mut notes = Vec::new();
        let Some(out) = lodforge_treemodel::write::level_nif(&prep.nif, &prep.shapes, &lv, &mut notes) else { continue };
        let bytes = out.to_bytes().map_err(|e| e.to_string())?;
        files.push((format!("meshes\\{}\\{}", lodforge_treemodel::write::FOLDER, lodforge_treemodel::write::file_name(stem, crc, level)), bytes));
    }
    Ok((files, tris))
}

/// Generates the models of every candidate tree into `out` (`meshes\dyndolod\lod\trees\`).
#[allow(clippy::too_many_arguments)]
pub fn generate(p: &Profile, c: &crate::dyndolod_build::Config, r: &Resolver<'_>, out: &Path, params: &[LevelParams], all: bool, only: &[String], crown: CrownMode, cp: &lodforge_treemodel::cloud::CloudParams) -> Result<Report> {
    let folder = out.join("meshes").join(lodforge_treemodel::write::FOLDER);
    std::fs::create_dir_all(&folder)?;
    // An earlier run's files (Data-relative; bare names are models): removed, never "installed".
    let rel = |l: &str| if l.contains('\\') { l.to_string() } else { format!("meshes\\{}\\{l}", lodforge_treemodel::write::FOLDER) };
    let own: HashSet<String> = std::fs::read_to_string(folder.join(LIST)).unwrap_or_default().lines().map(|l| rel(&l.trim().to_ascii_lowercase())).filter(|l| !l.ends_with('\\')).collect();
    for f in &own {
        let _ = std::fs::remove_file(out.join(f));
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
        if !all && !own.contains(&rel(&name)) && c.index.get(&name).is_some() {
            rep.installed += 1;
            continue;
        }
        todo.push((model.clone(), crc));
    }
    let textures = Textures::new(p);
    if crown == CrownMode::Cloud {
        let preps: Vec<(String, u32, Result<CloudPrep, String>)> = todo
            .par_iter()
            .map(|(model, crc)| {
                let prep = p.vfs.read(model).ok().flatten().ok_or_else(|| "mesh not readable".to_string()).and_then(|b| cloud_prep(&b, &|t| textures.get(t), params, cp));
                (model.clone(), *crc, prep)
            })
            .collect();
        let gpu = lodforge_gpu::Gpu::new()?;
        let renderer = lodforge_texgen::Renderer::new(&gpu);
        let read = |t: &str| p.vfs.read(t).ok().flatten();
        for (model, crc, prep) in preps {
            let stem = model.rsplit('\\').next().unwrap_or(&model).trim_end_matches(".nif").to_string();
            match prep.and_then(|pr| cloud_finish(&gpu, &renderer, &read, &pr, &stem, crc, cp)) {
                Ok((files, tris)) => {
                    let mut made = Made { model: model.clone(), files: Vec::new(), triangles: tris, bytes: 0, notes: Vec::new() };
                    for (path, data) in files {
                        let dest = out.join(&path);
                        if let Some(d) = dest.parent() {
                            std::fs::create_dir_all(d)?;
                        }
                        std::fs::write(&dest, &data)?;
                        made.bytes += data.len();
                        made.files.push(path);
                    }
                    rep.made.push(made);
                }
                Err(e) => rep.failed.push((model, e)),
            }
        }
        rep.made.sort_by(|a, b| a.model.cmp(&b.model));
        let list: String = rep.made.iter().flat_map(|m| m.files.iter()).map(|f| format!("{f}\r\n")).collect();
        std::fs::write(folder.join(LIST), list)?;
        return Ok(rep);
    }
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
                made.files.push(rel(&name));
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

fn inspect(p: &Profile, models: &[String], render: Option<&Path>, params: &[LevelParams], crown: CrownMode, cp: &lodforge_treemodel::cloud::CloudParams) -> Result<()> {
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
            let stem = path.rsplit('\\').next().unwrap_or(&path).trim_end_matches(".nif");
            let mut nifs = vec![lodforge_nif::parse(&bytes)?];
            let mut extra: HashMap<String, Vec<u8>> = HashMap::new();
            if crown == CrownMode::Cloud {
                let prep = cloud_prep(&bytes, &|t| textures.get(t), params, cp).map_err(anyhow::Error::msg)?;
                let gpu = lodforge_gpu::Gpu::new()?;
                let renderer = lodforge_texgen::Renderer::new(&gpu);
                let (files, tris) = cloud_finish(&gpu, &renderer, &|t| p.vfs.read(t).ok().flatten(), &prep, stem, 0, cp).map_err(anyhow::Error::msg)?;
                println!("  cloud: {} planes; per level (crown, trunk) triangles {tris:?}", prep.tiles.len());
                for (path, data) in files {
                    if path.ends_with(".nif") {
                        nifs.push(lodforge_nif::parse(&data)?);
                    } else {
                        extra.insert(path, data);
                    }
                }
            } else {
                let g = lodforge_treemodel::generate(&bytes, &|t| textures.get(t), params).map_err(|e| anyhow::anyhow!("{e:?}"))?;
                for l in g.levels.iter().flatten() {
                    nifs.push(lodforge_nif::parse(l)?);
                }
            }
            std::fs::create_dir_all(dir)?;
            let png = dir.join(format!("{stem}.png"));
            render_sheet(p, &nifs, &png, &extra)?;
            println!("  rendered {}", png.display());
        }
    }
    Ok(())
}

/// The models side by side as TexGen's billboard camera sees them, on grey.
/// The models side by side as TexGen's billboard camera sees them, each `height` pixels high
/// (billboard heights round up to a power of two), on grey: (width, height, RGBA).
fn sheet_image(gpu: &lodforge_gpu::Gpu, renderer: &lodforge_texgen::Renderer, read: &dyn Fn(&str) -> Option<Vec<u8>>, nifs: &[lodforge_nif::Nif], height: f32) -> (u32, u32, Vec<u8>) {
    let s = lodforge_texgen::Settings::default();
    let mut imgs = Vec::new();
    for nif in nifs {
        let scene = lodforge_texgen::Scene::from_nif(nif);
        let h = scene.bounds().map(|(lo, hi)| hi[2] - lo[2]).unwrap_or(1.0).max(1.0);
        let scale = height * s.units_per_pixel / h;
        let tex = lodforge_texgen::load_textures(&scene, |t| read(t).and_then(|b| lodforge_texgen::TexData::from_dds(&b, true)));
        if let Some(b) = lodforge_texgen::make_billboard(gpu, renderer, &scene, &tex, scale, &s, false) {
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
    (w, h, out)
}

fn save_png(path: &Path, w: u32, h: u32, rgba: &[u8]) -> Result<()> {
    let f = std::fs::File::create(path)?;
    let mut e = png::Encoder::new(std::io::BufWriter::new(f), w, h);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    e.write_header()?.write_image_data(rgba)?;
    Ok(())
}

/// Box-filtered copy of an image scaled to fit `cw` x `ch`, bottom-centred into a cell of that size.
fn fit(src: (u32, u32, &[u8]), cw: u32, ch: u32) -> Vec<u8> {
    let (sw, sh, s) = src;
    let k = (cw as f32 / sw as f32).min(ch as f32 / sh as f32).min(1.0);
    let (dw, dh) = (((sw as f32 * k) as u32).max(1), ((sh as f32 * k) as u32).max(1));
    let mut out: Vec<u8> = (0..cw * ch).flat_map(|_| [96u8, 104, 112, 255]).collect();
    let (ox, oy) = ((cw - dw) / 2, ch - dh);
    for y in 0..dh {
        for x in 0..dw {
            let (x0, x1) = ((x as f32 / k) as u32, (((x + 1) as f32 / k) as u32).max((x as f32 / k) as u32 + 1).min(sw));
            let (y0, y1) = ((y as f32 / k) as u32, (((y + 1) as f32 / k) as u32).max((y as f32 / k) as u32 + 1).min(sh));
            let mut acc = [0u32; 4];
            let mut n = 0;
            for yy in y0..y1 {
                for xx in x0..x1 {
                    let i = ((yy * sw + xx) * 4) as usize;
                    for c in 0..4 {
                        acc[c] += u32::from(s[i + c]);
                    }
                    n += 1;
                }
            }
            let d = (((oy + y) * cw + ox + x) * 4) as usize;
            for c in 0..4 {
                out[d + c] = (acc[c] / n.max(1)) as u8;
            }
        }
    }
    out
}

/// Contact sheets of generated models: per page `cols` x `rows` cells of (full model | LOD4).
fn contact_sheets(p: &Profile, out: &Path, made: &[(String, usize, String)], dir: &Path) -> Result<usize> {
    let gpu = lodforge_gpu::Gpu::new()?;
    let renderer = lodforge_texgen::Renderer::new(&gpu);
    let read = |t: &str| -> Option<Vec<u8>> { std::fs::read(out.join(t)).ok().or_else(|| p.vfs.read(t).ok().flatten()) };
    let (cols, rows, cw, ch) = (6u32, 4u32, 420u32, 300u32);
    std::fs::create_dir_all(dir)?;
    let per = (cols * rows) as usize;
    let mut pages = 0;
    for (pi, chunk) in made.chunks(per).enumerate() {
        let (pw, ph) = (cols * cw, rows * ch);
        let mut page: Vec<u8> = (0..pw * ph).flat_map(|_| [40u8, 40, 40, 255]).collect();
        let mut index = String::new();
        for (ci, (model, refs, lod0)) in chunk.iter().enumerate() {
            let nifs: Vec<lodforge_nif::Nif> = [read(model), read(lod0)].into_iter().flatten().filter_map(|b| lodforge_nif::parse(&b).ok()).collect();
            let (w, h, img) = sheet_image(&gpu, &renderer, &read, &nifs, 300.0);
            let cell = fit((w, h, &img), cw - 6, ch - 6);
            let (cx, cy) = ((ci as u32 % cols) * cw + 3, (ci as u32 / cols) * ch + 3);
            for y in 0..ch - 6 {
                let s0 = (y * (cw - 6) * 4) as usize;
                let d0 = (((cy + y) * pw + cx) * 4) as usize;
                page[d0..d0 + ((cw - 6) * 4) as usize].copy_from_slice(&cell[s0..s0 + ((cw - 6) * 4) as usize]);
            }
            index.push_str(&format!("{}\t{}\t{refs} refs\t{model}\r\n", ci / cols as usize + 1, ci % cols as usize + 1));
        }
        save_png(&dir.join(format!("trees_{:02}.png", pi + 1)), pw, ph, &page)?;
        std::fs::write(dir.join(format!("trees_{:02}.txt", pi + 1)), format!("row\tcol\treferences\tmodel (cell: full model | LOD4)\r\n{index}"))?;
        pages += 1;
    }
    Ok(pages)
}

fn render_sheet(p: &Profile, nifs: &[lodforge_nif::Nif], png: &Path, extra: &HashMap<String, Vec<u8>>) -> Result<()> {
    let gpu = lodforge_gpu::Gpu::new()?;
    let renderer = lodforge_texgen::Renderer::new(&gpu);
    let read = |t: &str| extra.get(t).cloned().or_else(|| p.vfs.read(t).ok().flatten());
    let (w, h, out) = sheet_image(&gpu, &renderer, &read, nifs, 500.0);
    save_png(png, w, h, &out)
}

pub fn run(a: &Args) -> Result<()> {
    let p = a.profile.open()?;
    let params = levels_arg(a.levels.as_deref())?;
    let cp = lodforge_treemodel::cloud::CloudParams { max_tile: a.cloud_max_tile, dither: !a.cloud_soft, ..Default::default() };
    if !a.inspect.is_empty() {
        return inspect(&p, &a.inspect, a.render.as_deref(), &params, a.crown, &cp);
    }
    let out = a.out.clone().context("--out or --inspect is needed")?;
    let dir = a.dyndolod_config.clone().unwrap_or_else(|| a.profile.instance.join(r"tools\DynDOLOD\Edit Scripts\DynDOLOD"));
    let mut c = crate::dyndolod_build::Config::load(&p, &dir, &a.preset, false)?;
    c.ultra = true;
    let t = std::time::Instant::now();
    let rep = crate::dyndolod_build::with_resolver(&p, &c, |r| generate(&p, &c, r, &out, &params, a.all, &a.only, a.crown, &cp))?;
    println!("{} in {:.1} s", rep.summary(), t.elapsed().as_secs_f64());
    if let Some(dir) = &a.sheets {
        // Most placed first (Tamriel's tree report), then by name.
        let refs: HashMap<String, usize> = crate::dyndolod_build::with_resolver(&p, &c, |r| -> Result<HashMap<String, usize>> {
            let worlds = lodforge_esp::scan_worldspaces(&p.load_order)?;
            let Some(ws) = worlds.iter().find(|w| w.editor_id.eq_ignore_ascii_case("Tamriel")) else { return Ok(HashMap::new()) };
            let h = crate::dyndolod_build::header(&p, &c, ws, "", &a.profile.game_dir().join("Data"), "");
            let we = crate::dyndolod_build::export_world(&p, &c, r, ws, &[], &h, &[])?;
            let s = &we.seasons[0];
            let rows = crate::tree_report::rows(&p, &c, r, ws, &s.lines, &s.skipped, &a.profile.instance.join("mods"), "");
            let mut m = HashMap::new();
            for row in rows {
                *m.entry(lodforge_dyndolod::bases::model_path(&row.model).to_ascii_lowercase()).or_default() += row.refs;
            }
            Ok(m)
        })?;
        let mut list: Vec<(String, usize, String)> = rep
            .made
            .iter()
            .filter_map(|m| {
                let lod0 = m.files.iter().find(|f| f.ends_with("passthru_lod.nif"))?.clone();
                Some((m.model.clone(), refs.get(&m.model.to_ascii_lowercase()).copied().unwrap_or(0), lod0))
            })
            .collect();
        list.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let t = std::time::Instant::now();
        let pages = contact_sheets(&p, &out, &list, dir)?;
        println!("contact sheets: {pages} pages of {} models in {:.1} s ({})", list.len(), t.elapsed().as_secs_f64(), dir.display());
    }
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
