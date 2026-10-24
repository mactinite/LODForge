//! `lodforge texgen-billboards`: tree billboards rendered on the GPU (`lodforge-texgen`) for the
//! same bases a golden TexGen run made billboards for, compared by size, sidecar values and PSNR.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};

use lodforge_texgen::{Renderer, Scene, Settings, TexData};

use crate::profile::ProfileArgs;

#[derive(clap::Args)]
pub struct Args {
    #[command(flatten)]
    pub profile: ProfileArgs,
    /// Golden TexGen output folder (contains textures\terrain\lodgen\) to compare with. Required by
    /// --hd-grass (it names the grass types).
    #[arg(long)]
    pub golden: Option<PathBuf>,
    /// Write our billboards (DDS + txt) under this Data folder.
    #[arg(long)]
    pub out: Option<PathBuf>,
    /// Only billboards whose name contains this.
    #[arg(long)]
    pub filter: Option<String>,
    /// DynDOLOD folder (contains `Edit Scripts\DynDOLOD\Render`); read only, for TexGen's `_static`
    /// replacement models. Defaults to `<instance>\tools\DynDOLOD`.
    #[arg(long)]
    pub dyndolod: Option<PathBuf>,
    /// Render the rendered (trunk) billboards of `Render\Skyrim\Billboards` instead of the tree
    /// billboards, compared with the golden `textures\DynDOLOD\LOD\trees`.
    #[arg(long)]
    pub trunks: bool,
    /// Render HD grass billboards for the grass types of the golden `--golden` run instead.
    #[arg(long)]
    pub hd_grass: bool,
    /// Also write `<stem>_n.dds` for tree billboards (TexGen `TreeNormalMaps=1`).
    #[arg(long)]
    pub normals: bool,
}

/// A texture as TexGen uploads it: block-compressed DDS files as they are (`LoadGLCompressed`),
/// decoded on the CPU only when the GPU lacks BC support.
pub(crate) fn load_texture(gpu: &lodforge_gpu::Gpu, vfs: &lodforge_archive::Vfs, path: &str) -> Option<TexData> {
    let bytes = vfs.read(path).ok().flatten()?;
    TexData::for_gpu(gpu, &bytes)
}

fn parse_txt(t: &str) -> HashMap<String, String> {
    t.lines().filter_map(|l| l.split_once('=')).map(|(k, v)| (k.trim().to_ascii_uppercase(), v.trim().to_string())).collect()
}

/// A tree billboard candidate as TexGen finds it (spec §1.1).
#[derive(Debug, Clone)]
pub struct Candidate {
    pub base: u32,
    /// Lower-cased file name of the plugin that defines the base record.
    pub plugin: String,
    /// Form id without the load-order byte(s) (`FE` + 3 digits for light plugins).
    pub local: u32,
    pub editor_id: String,
    /// MODL as stored.
    pub model: String,
    /// By TexGen's rule or an override.
    pub accepted: bool,
    /// `,True` / `,False` from a `TexGen_Billboards` override line.
    pub forced: Option<bool>,
    /// `Scale=` from an override file.
    pub scale: Option<f32>,
    /// Pixel height of a mod-installed billboard this render replaces: the render is at least
    /// this tall, so re-rendering (`--installed-billboards render`) never loses resolution.
    pub min_px: Option<u32>,
}

impl Candidate {
    /// `<model basename>_<formid8>` in lower case.
    pub fn stem(&self) -> String {
        let m = self.model.replace('/', "\\").to_ascii_lowercase();
        let base = m.rsplit('\\').next().unwrap_or("").trim_end_matches(".nif").to_string();
        format!("{base}_{:08x}", self.local)
    }
}

static RENDER_INSTALLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Whether bases with a mod-installed billboard are rendered too (`--installed-billboards`).
pub fn set_render_installed(on: bool) {
    RENDER_INSTALLED.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn render_installed() -> bool {
    RENDER_INSTALLED.load(std::sync::atomic::Ordering::Relaxed)
}

/// `_ResourcePack.esl` -> `_resourcepackesl`: the plugin part of DynDOLOD's config file names.
fn config_key(plugin: &str) -> String {
    plugin.to_ascii_lowercase().chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_').collect()
}

/// Lines of a DynDOLOD config file for `plugin`, from `Data\DynDOLOD\` (VFS) or else `Configs\`.
pub(crate) fn config_lines(p: &crate::profile::Profile, dyndolod: &std::path::Path, prefix: &str, plugin: &str) -> Vec<String> {
    let name = format!("{prefix}{}.txt", config_key(plugin));
    // The LODForge Config overlay's copy first, then Data\DynDOLOD\ (VFS), then DynDOLOD's Configs.
    let overlay = lodforge_dyndolod::overlay::current().map(|o| o.join("Configs").join(&name)).filter(|f| f.is_file()).and_then(|f| std::fs::read(f).ok());
    let text = overlay.or_else(|| p.vfs.read(&format!("dyndolod\\{}", name.to_ascii_lowercase())).ok().flatten()).or_else(|| std::fs::read(dyndolod.join(r"Edit Scripts\DynDOLOD\Configs").join(&name)).ok());
    text.map(|b| String::from_utf8_lossy(&b).lines().map(str::to_string).collect()).unwrap_or_default()
}

/// TexGen's tree billboard candidates: TREE records and STAT records with Has Tree LOD, accepted
/// when OBND magnitude >= 512, OBND z2 >= 256 and the model path contains `tree`, unless a
/// `DynDOLOD_SSE_TexGen_Billboards_<plugin>.txt` line says `,True` / `,False`; the
/// `DynDOLOD_SSE_Required_Billboards_<plugin>.txt` entries are forced on.
pub fn discover_trees(p: &crate::profile::Profile, bases: &HashMap<u32, lodforge_dyndolod::bases::Base>, dyndolod: &std::path::Path) -> Vec<Candidate> {
    let lo = &p.load_order;
    // (plugin lower case, local id) -> (forced, scale)
    let mut overrides: HashMap<(String, u32), (Option<bool>, Option<f32>)> = HashMap::new();
    // The `,True` / `,False` of `TexGen_Billboards` lines alone (for the report).
    let mut override_lines: HashMap<(String, u32), bool> = HashMap::new();
    for plugin in &p.plugin_names {
        for (prefix, required) in [("DynDOLOD_SSE_TexGen_Billboards_", false), ("DynDOLOD_SSE_Required_Billboards_", true)] {
            for line in config_lines(p, dyndolod, prefix, plugin) {
                let line = line.trim();
                if line.is_empty() || line.starts_with("//") || line.starts_with(';') {
                    continue;
                }
                let mut parts = line.split(';');
                let (Some(pl), Some(id)) = (parts.next(), parts.next()) else { continue };
                let Ok(id) = u32::from_str_radix(id.trim(), 16) else { continue };
                let rest = parts.next().unwrap_or("");
                let e = overrides.entry((pl.trim().to_ascii_lowercase(), id & 0xFF_FFFF)).or_default();
                if required {
                    e.0 = Some(true);
                }
                for opt in rest.split(',').skip(1) {
                    let opt = opt.trim();
                    if opt.eq_ignore_ascii_case("true") {
                        e.0 = Some(true);
                        if !required {
                            override_lines.insert((pl.trim().to_ascii_lowercase(), id & 0xFF_FFFF), true);
                        }
                    } else if opt.eq_ignore_ascii_case("false") {
                        e.0 = Some(false);
                        if !required {
                            override_lines.insert((pl.trim().to_ascii_lowercase(), id & 0xFF_FFFF), false);
                        }
                    } else if let Some(v) = opt.strip_prefix("Scale=").or_else(|| opt.strip_prefix("scale=")) {
                        e.1 = v.parse().ok();
                    }
                }
            }
        }
    }
    let owners = lodforge_dyndolod::bases::owners(lo);
    let mut out = Vec::new();
    for b in bases.values() {
        let has_tree_lod = &b.signature == b"STAT" && b.flags & 0x40 != 0;
        let Some(plugin) = owners.get(&lodforge_dyndolod::bases::owner_key(b.form_id)).and_then(|&i| p.plugin_names.get(i)) else { continue };
        let local = if b.form_id >> 24 == 0xFE { b.form_id & 0xFFF } else { b.form_id & 0xFF_FFFF };
        // Bases named in an override or required list are candidates whatever their type.
        let listed = overrides.contains_key(&(plugin.to_ascii_lowercase(), local));
        if &b.signature != b"TREE" && !has_tree_lod && !listed {
            continue;
        }
        let o = b.bounds.map(f32::from);
        let magnitude = ((o[3] - o[0]).powi(2) + (o[4] - o[1]).powi(2) + (o[5] - o[2]).powi(2)).sqrt();
        let rule = magnitude >= 512.0 && o[5] >= 256.0 && b.model.to_ascii_lowercase().contains("tree");
        let (forced, scale) = overrides.get(&(plugin.to_ascii_lowercase(), local)).copied().unwrap_or_default();
        let listed_forced = override_lines.get(&(plugin.to_ascii_lowercase(), local)).copied();
        if b.model.is_empty() {
            continue;
        }
        let mut c = Candidate { base: b.form_id, plugin: plugin.to_ascii_lowercase(), local, editor_id: b.editor_id.clone(), model: b.model.clone(), accepted: false, forced: listed_forced, scale, min_px: None };
        // Bases with a ready-made billboard from a mod (HD LODs Textures and similar) are
        // rendered too unless `--installed-billboards keep`: same supersampled quality and soft
        // alpha for every tree and shrub, and the same set of bases in tree LOD.
        let installed_px = if render_installed() && !rule {
            p.vfs.read(&format!("textures\\terrain\\lodgen\\{}\\{}.dds", c.plugin, c.stem())).ok().flatten().and_then(|d| lodforge_dds::parse(&d).ok()).map(|i| i.height)
        } else {
            None
        };
        c.min_px = installed_px;
        c.accepted = forced.unwrap_or(rule || installed_px.is_some());
        out.push(c);
    }
    out.sort_by(|a, b| (&a.plugin, a.local).cmp(&(&b.plugin, b.local)));
    out
}

/// Largest reference scale per base over every worldspace.
pub fn max_ref_scales(lo: &lodforge_esp::LoadOrder) -> Result<HashMap<u32, f32>> {
    let mut max_scale: HashMap<u32, f32> = HashMap::new();
    for w in lodforge_esp::scan_worldspaces(lo)? {
        for r in w.refs.values() {
            let s = if r.has_scale { r.scale } else { 1.0 };
            let e = max_scale.entry(r.base).or_insert(0.0);
            *e = e.max(s);
        }
    }
    Ok(max_scale)
}

/// One finished tree billboard.
pub struct TreeBillboard<'a> {
    pub cand: &'a Candidate,
    pub stem: String,
    pub billboard: lodforge_texgen::Billboard,
    pub source: lodforge_texgen::Source,
}

/// Billboards per GPU batch (two round trips each).
pub const BATCH: usize = 32;

/// Renders the accepted candidates in batches of [`BATCH`] and hands each billboard to `sink`.
/// Returns the number made.
#[allow(clippy::too_many_arguments)]
pub fn tree_billboards(p: &crate::profile::Profile, gpu: &lodforge_gpu::Gpu, renderer: &Renderer, dyndolod: &std::path::Path, cands: &[&Candidate], max_scale: &HashMap<u32, f32>, normals: bool, mut sink: impl FnMut(TreeBillboard) -> Result<()>) -> Result<usize> {
    let settings = Settings::default();
    let mut crc_cache: HashMap<String, u32> = HashMap::new();
    let mut made = 0;
    for chunk in cands.chunks(BATCH) {
        // CPU side: model, scene, textures, sidecar source.
        let mut prepared = Vec::new();
        for c in chunk {
            let record_model = lodforge_dyndolod::bases::model_path(&c.model);
            let raw = c.model.replace('/', "\\");
            let (bytes, model_value) = match lodforge_texgen::replacement_model(dyndolod, &raw) {
                Some((file, value)) => (std::fs::read(&file)?, value),
                None => {
                    let Some(bytes) = p.vfs.read(&record_model).ok().flatten() else { println!("  skip {record_model}: not in the VFS"); continue };
                    // TexGen writes `Meshes\` and the lower-cased path below it (replacement
                    // models keep the record's case, above).
                    (bytes, format!("Meshes\\{}", &record_model["meshes\\".len()..]))
                }
            };
            let nif = match lodforge_nif::parse_all_geometry(&bytes) {
                Ok(n) => n,
                Err(e) => {
                    println!("  skip {record_model}: {e}");
                    continue;
                }
            };
            let mut scene = Scene::from_nif(&nif);
            lodforge_texgen::resolve_textures(&mut scene, |t| p.vfs.exists(t));
            let tex = lodforge_texgen::load_textures(&scene, |path| load_texture(gpu, &p.vfs, path));
            let source = source_cached(&p.vfs, &scene, &model_value, &bytes, &mut crc_cache);
            let mut scale = c.scale.or_else(|| max_scale.get(&c.base).copied()).filter(|s| *s > 0.0).unwrap_or(1.7);
            // Re-rendering a mod-installed billboard: at least its pixel height.
            if let Some(px) = c.min_px {
                let (lo, hi) = scene.meshes.iter().flat_map(|m| m.positions.iter()).fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p[2]), hi.max(p[2])));
                if hi > lo {
                    scale = scale.max(px as f32 * settings.units_per_pixel / (hi - lo));
                }
            }
            prepared.push((c, scene, tex, source, scale));
        }
        let items: Vec<lodforge_texgen::Item> = prepared.iter().map(|(_, scene, tex, _, scale)| lodforge_texgen::Item { scene, textures: tex, scale: *scale }).collect();
        let bbs = lodforge_texgen::make_billboards(gpu, renderer, &items, &settings, normals);
        drop(items);
        for ((c, scene, _tex, source, _), bb) in prepared.into_iter().zip(bbs) {
            let Some(billboard) = bb else {
                println!("  skip {}: no billboard ({} meshes)", c.stem(), scene.meshes.len());
                continue;
            };
            made += 1;
            sink(TreeBillboard { cand: c, stem: c.stem(), billboard, source })?;
        }
    }
    Ok(made)
}

/// Writes a tree billboard (`<stem>.dds`, `_n.dds` when baked, `.txt`) under `out`.
pub fn write_tree_billboard(gpu: &lodforge_gpu::Gpu, out: &std::path::Path, t: &TreeBillboard) -> Result<()> {
    let dir = out.join(r"textures\terrain\lodgen").join(&t.cand.plugin);
    std::fs::create_dir_all(&dir)?;
    let bb = &t.billboard;
    let dds = bc7_dds(gpu, bb.width_px, bb.height_px, &bb.rgba)?;
    std::fs::write(dir.join(format!("{}.dds", t.stem)), &dds)?;
    if let Some(n) = &bb.normal {
        std::fs::write(dir.join(format!("{}_n.dds", t.stem)), bc7_dds(gpu, bb.width_px, bb.height_px, n)?)?;
    }
    std::fs::write(dir.join(format!("{}.txt", t.stem)), lodforge_texgen::sidecar(bb, 1.0, lodforge_dyndolod::lodmodels::crc32(&dds), &t.source))?;
    Ok(())
}

pub fn run(a: &Args) -> Result<()> {
    let p = a.profile.open()?;
    let lo = &p.load_order;
    let gpu = lodforge_gpu::Gpu::new()?;
    let dyndolod = a.dyndolod.clone().unwrap_or_else(|| a.profile.instance.join(r"tools\DynDOLOD"));
    let renderer = Renderer::new(&gpu);
    if a.trunks {
        return run_trunks(a, &p, &gpu, &renderer, &dyndolod);
    }
    if a.hd_grass {
        return run_hd_grass(a, &p, &gpu, &renderer);
    }
    let t = Instant::now();
    let bases = lodforge_dyndolod::bases::read_bases(lo)?;
    let max_scale = max_ref_scales(lo)?;
    let cands = discover_trees(&p, &bases, &dyndolod);
    let accepted: Vec<&Candidate> = cands.iter().filter(|c| c.accepted && a.filter.as_ref().is_none_or(|f| c.stem().contains(f.as_str()))).collect();
    println!("{} candidates, {} accepted, in {} ms; MSAA {}x", cands.len(), accepted.len(), t.elapsed().as_millis(), renderer.samples);
    let gold_root = a.golden.as_ref().map(|g| g.join(r"textures\terrain\lodgen"));
    if let Some(root) = &gold_root
        && let Ok(report) = std::fs::read_to_string(root.join("TexGen_SSE_Tree_Billboards.txt"))
    {
        // `Plugin;FormID;EditorID[,True|,False]`; accepted unless `,False`.
        let gold: std::collections::BTreeSet<(String, u32)> = report
            .lines()
            .filter(|l| !l.ends_with(",False"))
            .filter_map(|l| {
                let mut it = l.split(';');
                Some((it.next()?.to_ascii_lowercase(), u32::from_str_radix(it.next()?, 16).ok()?))
            })
            .collect();
        let ours: std::collections::BTreeSet<(String, u32)> = cands.iter().filter(|c| c.accepted).map(|c| (c.plugin.clone(), c.local)).collect();
        println!("discovery: {} accepted, golden {}, both {}; ours only {:?}; golden only {:?}", ours.len(), gold.len(), ours.intersection(&gold).count(), ours.difference(&gold).take(5).collect::<Vec<_>>(), gold.difference(&ours).take(5).collect::<Vec<_>>());
        let mut a_lines: Vec<String> = report.lines().map(str::to_ascii_lowercase).collect();
        let mut b_lines: Vec<String> = tree_report(&cands).lines().map(str::to_ascii_lowercase).collect();
        a_lines.sort();
        b_lines.sort();
        println!("report: {} of {} golden lines identical (sorted, case-insensitive)", b_lines.iter().filter(|l| a_lines.binary_search(l).is_ok()).count(), a_lines.len());
    }
    let mut stats = TreeStats::default();
    let t = Instant::now();
    let made = tree_billboards(&p, &gpu, &renderer, &dyndolod, &accepted, &max_scale, a.normals, |tb| {
        if let Some(root) = &gold_root {
            stats.add(&gpu, root, &tb)?;
        }
        if let Some(out) = &a.out {
            write_tree_billboard(&gpu, out, &tb)?;
        }
        Ok(())
    })?;
    println!("billboards: {made} rendered in {} ms", t.elapsed().as_millis());
    if gold_root.is_some() {
        stats.print();
    }
    Ok(())
}

/// Comparison of tree billboards with a golden TexGen run.
#[derive(Default)]
struct TreeStats {
    n: usize,
    same_size: usize,
    near_size: usize,
    shift_ok: usize,
    depth_ok: usize,
    cmp_n: usize,
    /// Uncompressed PSNR (golden-opaque, both-opaque), alpha agreement, BC7 PSNR (golden-opaque,
    /// both-opaque), 4x4 block PSNR.
    sums: [f64; 6],
    /// Width, height ratio and ShiftZ difference.
    dims: [f64; 3],
    sidecars: SidecarStats,
}

impl TreeStats {
    fn add(&mut self, gpu: &lodforge_gpu::Gpu, root: &std::path::Path, tb: &TreeBillboard) -> Result<()> {
        let txt_path = root.join(&tb.cand.plugin).join(format!("{}.txt", tb.stem));
        let Ok(text) = std::fs::read_to_string(&txt_path) else { return Ok(()) };
        let gold_txt = parse_txt(&text);
        let bb = &tb.billboard;
        let stem = &tb.stem;
        self.n += 1;
        self.sidecars.add(&gold_txt, &tb.source);
        let g = |k: &str| gold_txt.get(k).and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0);
        for (s, v) in self.dims.iter_mut().zip([bb.width / g("WIDTH"), bb.height / g("HEIGHT"), bb.shift_z - g("SHIFTZ")]) {
            *s += f64::from(v);
        }
        let gold_dds = std::fs::read(txt_path.with_extension("dds"))?;
        let gimg = lodforge_dds::decode(&gold_dds, &lodforge_dds::parse(&gold_dds)?, 0)?;
        self.near_size += usize::from(gimg.height == bb.height_px && gimg.width.abs_diff(bb.width_px) <= 4);
        self.shift_ok += usize::from((bb.shift_z - g("SHIFTZ")).abs() <= 1.0);
        self.depth_ok += usize::from((bb.depth - g("DEPTH")).abs() <= 0.5);
        if std::env::var_os("TG_FIT").is_some() {
            println!("SIZE\t{}\\{stem}\t{}x{}\t{}x{}\tH {:.2}/{:.2}\tSz {:.2}/{:.2}\tW {:.2}/{:.2}\tD {:.2}/{:.2}", tb.cand.plugin, bb.width_px, bb.height_px, gimg.width, gimg.height, bb.height, g("HEIGHT"), bb.shift_z, g("SHIFTZ"), bb.width, g("WIDTH"), bb.depth, g("DEPTH"));
        }
        let same = gimg.width == bb.width_px && gimg.height == bb.height_px;
        if std::env::var_os("TG_PNG").is_some() {
            let save = |path: String, w: u32, h: u32, d: &[u8]| -> Result<()> {
                let f = std::fs::File::create(path)?;
                let mut e = png::Encoder::new(std::io::BufWriter::new(f), w, h);
                e.set_color(png::ColorType::Rgba);
                e.write_header()?.write_image_data(d)?;
                Ok(())
            };
            save(format!("bench/tg_{stem}_ours.png"), bb.width_px, bb.height_px, &bb.rgba)?;
            save(format!("bench/tg_{stem}_gold.png"), gimg.width, gimg.height, &gimg.rgba)?;
            if same {
                // Golden | ours | 4x colour difference, each 3x nearest-upscaled, black background.
                let (gw, gh, z) = (gimg.width as usize, gimg.height as usize, 3usize);
                let sw = gw * 3 * z;
                let mut strip = vec![0u8; sw * gh * z * 4];
                for y in 0..gh * z {
                    for x in 0..sw {
                        let (panel, px, py) = (x / (gw * z), (x % (gw * z)) / z, y / z);
                        let i = (py * gw + px) * 4;
                        let (g, o) = (&gimg.rgba[i..i + 4], &bb.rgba[i..i + 4]);
                        let c = match panel {
                            0 if g[3] >= 128 => [g[0], g[1], g[2]],
                            1 if o[3] >= 128 => [o[0], o[1], o[2]],
                            2 if g[3] >= 128 && o[3] >= 128 => [0, 1, 2].map(|k| (u32::from(g[k].abs_diff(o[k])) * 4).min(255) as u8),
                            _ => [0, 0, 0],
                        };
                        strip[(y * sw + x) * 4..(y * sw + x) * 4 + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
                    }
                }
                save(format!("bench/tg_{stem}_strip.png"), sw as u32, (gh * z) as u32, &strip)?;
            }
        }
        if same {
            self.same_size += 1;
            let cmp = lodforge_texgen::compare_sized(&gimg.rgba, &bb.rgba, bb.width_px);
            // The same after our BC7 encode (the golden is BC7 too).
            let ours_dds = bc7_dds(gpu, bb.width_px, bb.height_px, &bb.rgba)?;
            let ours_bc7 = lodforge_dds::decode(&ours_dds, &lodforge_dds::parse(&ours_dds)?, 0)?;
            let cmp7 = lodforge_texgen::compare(&gimg.rgba, &ours_bc7.rgba);
            if std::env::var_os("TG_FIT").is_some() {
                println!("MEAN\t{}\\{stem}\t{:?}\t{:?}\t{:.2}\t{:.2}", tb.cand.plugin, mean_opaque(&gimg.rgba).map(|v| v.round()), mean_opaque(&bb.rgba).map(|v| v.round()), cmp.psnr_ref_opaque, cmp.psnr_both_opaque);
            }
            for (s, v) in self.sums.iter_mut().zip([cmp.psnr_ref_opaque, cmp.psnr_both_opaque, cmp.alpha_agreement, cmp7.psnr_ref_opaque, cmp7.psnr_both_opaque, cmp.psnr_blocks]) {
                *s += v;
            }
            self.cmp_n += 1;
        }
        Ok(())
    }

    fn print(&self) {
        let c = self.cmp_n.max(1) as f64;
        let n = self.n.max(1) as f64;
        println!(
            "golden: {} compared; {} the golden size ({} within 4 px), ShiftZ within 1 unit {}, Depth equal {}; PSNR uncompressed {:.2} dB on golden-opaque texels, {:.2} dB on texels opaque in both, BC7 {:.2} / {:.2} dB, 4x4 block means {:.2} dB; alpha agreement {:.1}%; width ratio {:.3}, height ratio {:.3}, shiftZ diff {:.1}",
            self.n, self.same_size, self.near_size, self.shift_ok, self.depth_ok,
            self.sums[0] / c, self.sums[1] / c, self.sums[3] / c, self.sums[4] / c, self.sums[5] / c, 100.0 * self.sums[2] / c,
            self.dims[0] / n, self.dims[1] / n, self.dims[2] / n
        );
        self.sidecars.print();
    }
}

#[derive(clap::Args)]
pub struct GrassArgs {
    #[command(flatten)]
    pub profile: ProfileArgs,
    /// Grass cache folder (`<World>x<XXXX>y<YYYY>.cgid`).
    #[arg(long)]
    pub grass: PathBuf,
    #[arg(long, default_value = "Tamriel")]
    pub world: String,
    /// Output Data folder for the billboards; the grass map and FlatTextures go next to it.
    #[arg(long)]
    pub out: PathBuf,
    /// Render every grass type in the cache, not only those TexGen makes billboards for
    /// ([`texgen_grass_billboard`]).
    #[arg(long)]
    pub all_types: bool,
    /// TexGen "HD Grass": two-view rendered billboards (`_1`, `_2`, `_n`, `Complex=`, `Width_n`)
    /// referenced as `<stem>_1.dds`, as TexGen makes them when Community Shaders is installed.
    #[arg(long)]
    pub hd: bool,
}

/// Renders a billboard for each grass type in the cache that TexGen would make one for and writes
/// `<out>\..\grass_map.txt` (LODGen `GrassMap`) and `<out>\..\grass_flat.txt` (FlatTextures lines).
pub fn run_grass(a: &GrassArgs) -> Result<()> {
    let p = a.profile.open()?;
    let lo = &p.load_order;
    let owners = lodforge_dyndolod::bases::owners(lo);
    let t = Instant::now();
    let prefix = format!("{}x", a.world.to_ascii_lowercase());
    // (model, load-order form id) -> (form id without the load-order byte, placements)
    let mut types: std::collections::BTreeMap<(String, u32), (u32, usize)> = std::collections::BTreeMap::new();
    let (mut files, mut placements) = (0usize, 0usize);
    for e in std::fs::read_dir(&a.grass)?.flatten() {
        let name = e.file_name().to_string_lossy().to_ascii_lowercase();
        if !(name.starts_with(&prefix) && name.ends_with(".cgid")) {
            continue;
        }
        let stem = name.trim_end_matches(".cgid");
        let (Some(xi), Some(yi)) = (stem.rfind('x'), stem.rfind('y')) else { continue };
        let (Ok(x), Ok(y)) = (stem[xi + 1..yi].parse::<i32>(), stem[yi + 1..].parse::<i32>()) else { continue };
        let bytes = std::fs::read(e.path())?;
        let _ = (x, y);
        // Trailing bytes are ignored as the game does (NGIO 1.6 rewrites cache files in place
        // without truncating them: the E2E cache has files of 2x and 4x their content).
        let parsed = lodforge_grass::cgid::CellCache::parse_prefix(&bytes).map(|(c, _)| c).with_context(|| format!("parsing {name}"))?;
        files += 1;
        for g in parsed.types {
            let n = g.groups.iter().map(|gr| gr.instances.len()).sum::<usize>();
            placements += n;
            // The cache stores load-order form ids; the billboard names use the object id.
            let local = if g.form_id >> 24 == 0xFE { g.form_id & 0xFFF } else { g.form_id & 0x00FF_FFFF };
            types.entry((g.model.clone(), g.form_id)).or_insert((local, 0)).1 += n;
        }
    }
    println!("grass cache: {files} cells, {} grass types, {placements} placements read in {} ms", types.len(), t.elapsed().as_millis());
    let grasses = read_grasses(lo)?;
    let gpu = lodforge_gpu::Gpu::new()?;
    let renderer = Renderer::new(&gpu);
    let settings = Settings { light: lodforge_texgen::Light { direct: 1.5, ambient: 0.25, dir: [0.0, 0.0, 1.0], pbr_scale: 0.7, normal_pull: 1.0 }, coverage_threshold: 112, ..Settings::default() };
    // TexGen's `GrassHDBillboards*` lighting (as `--hd-grass`).
    let hd_settings = Settings { light: lodforge_texgen::Light { direct: 0.0, ambient: 1.0, dir: [0.0, 0.0, 1.0], pbr_scale: 0.7, normal_pull: 1.0 }, coverage_threshold: 112, ..Settings::default() };
    let (mut map, mut flat) = (String::new(), String::new());
    let (mut made, mut skipped, mut kept_placements) = (0, 0, 0usize);
    for ((model, raw), (local, count)) in &types {
        let Some(plugin) = owners.get(&lodforge_dyndolod::bases::owner_key(*raw)).and_then(|&i| p.plugin_names.get(i)) else {
            eprintln!("  no plugin for {model} {raw:08X}");
            continue;
        };
        let model_path = lodforge_dyndolod::bases::model_path(model);
        let Some(bytes) = p.vfs.read(&model_path).ok().flatten() else { continue };
        let Ok(nif) = lodforge_nif::parse_all_geometry(&bytes) else { continue };
        let mut scene = Scene::from_nif(&nif);
        lodforge_texgen::resolve_textures(&mut scene, |t| p.vfs.exists(t));
        if !a.all_types
            && let Some(g) = grasses.get(raw)
            && !texgen_grass_billboard(g, scene.bounds())
        {
            println!("  no TexGen billboard: {} {local:08X}", g.editor_id);
            skipped += 1;
            continue;
        }
        kept_placements += count;
        // Grass vertex alpha drives wind sway, not opacity.
        for m in &mut scene.meshes {
            for c in &mut m.colors {
                c[3] = 1.0;
            }
        }
        let tex = lodforge_texgen::load_textures(&scene, |path| load_texture(&gpu, &p.vfs, path));
        let stem = model_path.rsplit('\\').next().unwrap_or("").trim_end_matches(".nif").to_string();
        if a.hd {
            // As run_hd_grass: complex textures (diffuse above, normal below), two views.
            let mut complex = false;
            for m in &mut scene.meshes {
                m.complex = tex.get(&m.diffuse).is_some_and(|t| t.height == 2 * t.width);
                if m.complex {
                    let n = m.normal.clone();
                    m.texture_names.retain(|t| !t.eq_ignore_ascii_case(&n));
                }
                complex |= m.complex;
            }
            let Some(rb) = lodforge_texgen::make_rendered_billboard(&gpu, &renderer, &scene, &tex, 1.0, &hd_settings, [0.0, 90.0]) else {
                eprintln!("  no HD billboard for {model}");
                continue;
            };
            let dir = a.out.join("textures\\terrain\\lodgen").join(plugin.to_ascii_lowercase());
            let name = format!("{stem}_{local:08x}").to_ascii_lowercase();
            let src = source(&p.vfs, &scene, &format!("Meshes\\{}", &model_path["meshes\\".len()..]), &bytes);
            write_rendered(&gpu, &dir, &name, &rb, Some(complex), &src)?;
            let txt = std::fs::read_to_string(dir.join(format!("{name}.txt")))?;
            let rel = format!("textures\\terrain\\lodgen\\{}\\{name}_1.dds", plugin.to_ascii_lowercase());
            map.push_str(&format!("{}_{local:08X}.DDS\t{rel}\r\n", stem.to_uppercase()));
            flat.push_str(&lodforge_dyndolod::export::flat_texture_line(&rel, &txt, "", 0.0, 1.0, 0.0, 0.0));
            flat.push_str("\r\n");
            made += 1;
            continue;
        }
        let Some(bb) = lodforge_texgen::make_billboard(&gpu, &renderer, &scene, &tex, 1.0, &settings, false) else {
            eprintln!("  no billboard for {model}: {} meshes, {} textures", scene.meshes.len(), tex.len());
            continue;
        };
        let rel = format!("textures\\terrain\\lodgen\\{}\\{stem}_{local:08x}.dds", plugin.to_ascii_lowercase());
        let path = a.out.join(&rel);
        std::fs::create_dir_all(path.parent().unwrap())?;
        let dds = bc7_dds(&gpu, bb.width_px, bb.height_px, &bb.rgba)?;
        std::fs::write(&path, &dds)?;
        let txt = lodforge_texgen::sidecar(&bb, 1.0, lodforge_dyndolod::lodmodels::crc32(&dds), &source(&p.vfs, &scene, &format!("Meshes\\{}", model.replace('/', "\\")), &bytes));
        std::fs::write(path.with_extension("txt"), &txt)?;
        map.push_str(&format!("{}_{local:08X}.DDS\t{rel}\r\n", stem.to_uppercase()));
        flat.push_str(&lodforge_dyndolod::export::flat_texture_line(&rel, &txt, "", 0.0, 1.0, 0.0, 0.0));
        flat.push_str("\r\n");
        made += 1;
    }
    let parent = a.out.parent().unwrap_or(&a.out).to_path_buf();
    std::fs::write(parent.join("grass_map.txt"), map)?;
    std::fs::write(parent.join("grass_flat.txt"), flat)?;
    println!("grass types left out by TexGen's rule: {skipped}; placements kept: {kept_placements} of {placements}");
    println!("grass billboards: {made}/{} written to {} in {} ms", types.len(), a.out.display(), t.elapsed().as_millis());
    Ok(())
}

/// Winning GRAS record: what TexGen looks at to decide whether a grass type gets a billboard.
#[derive(Debug, Clone, Default)]
pub struct Grass {
    pub editor_id: String,
    /// OBND (x1, y1, z1, x2, y2, z2).
    pub bounds: [i16; 6],
    /// DATA "units from water" type (0 above at least, 1 above at most, 2 below at least,
    /// 3 below at most, 4 either at least, 5 either at most, 6 either at most above, 7 either at
    /// most below).
    pub water_type: u32,
}

pub fn read_grasses(lo: &lodforge_esp::LoadOrder) -> Result<HashMap<u32, Grass>> {
    let mut out = HashMap::new();
    for (id, (_, r)) in lodforge_esp::winning_records(lo, lodforge_esp::Sig(*b"GRAS"))? {
        let data = r.data()?;
        let mut g = Grass::default();
        for f in lodforge_esp::fields(&data) {
            match &f.sig.0 {
                b"EDID" => g.editor_id = f.zstring(),
                b"OBND" if f.data.len() >= 12 => {
                    for (k, v) in g.bounds.iter_mut().enumerate() {
                        *v = i16::from_le_bytes([f.data[k * 2], f.data[k * 2 + 1]]);
                    }
                }
                b"DATA" if f.data.len() >= 12 => {
                    g.water_type = u32::from_le_bytes([f.data[8], f.data[9], f.data[10], f.data[11]]);
                }
                _ => {}
            }
        }
        out.insert(id, g);
    }
    Ok(out)
}

/// TexGen's automatic grass discovery (`TexGen_SSE.ini`): a used GRAS record gets a billboard
/// when its object bounds are at least `MinGrassModelMagnitude` (64) long and
/// `MinGrassModelHeight` (32) tall (z2), and it does not grow below water. An all-zero OBND falls
/// back to the model's bounds (`model_bounds`). Everything else is listed with `,False` in
/// `TexGen_SSE_Grass_Billboards.txt` and left out of DynDOLOD's grass map.
pub fn texgen_grass_billboard(g: &Grass, model_bounds: Option<([f32; 3], [f32; 3])>) -> bool {
    let mut b = g.bounds.map(f32::from);
    if g.bounds == [0; 6]
        && let Some((lo, hi)) = model_bounds
    {
        b = [lo[0], lo[1], lo[2], hi[0], hi[1], hi[2]];
    }
    let magnitude = ((b[3] - b[0]).powi(2) + (b[4] - b[1]).powi(2) + (b[5] - b[2]).powi(2)).sqrt();
    let underwater = matches!(g.water_type, 2 | 3);
    magnitude >= 64.0 && b[5] >= 32.0 && !underwater
}

/// A one-mip BC7 DDS of an RGBA8 image (TexGen: Texconv `-m 1 -f BC7_UNORM`).
pub(crate) fn bc7_dds(gpu: &lodforge_gpu::Gpu, w: u32, h: u32, rgba: &[u8]) -> Result<Vec<u8>> {
    let blocks = gpu.encode_bc7(&[lodforge_gpu::RgbaImage { width: w, height: h, rgba }])?;
    Ok(lodforge_dds::write_bc7(w, h, &blocks))
}

/// The sidecar's source fields: `Model=` value, CRC32 of the NIF bytes, and every texture the
/// scene names with the CRC32 of its file in the VFS.
fn source(vfs: &lodforge_archive::Vfs, scene: &Scene, model: &str, nif_bytes: &[u8]) -> lodforge_texgen::Source {
    let textures = lodforge_texgen::texture_names(scene)
        .into_iter()
        .map(|t| {
            let crc = vfs.read(&t.to_ascii_lowercase()).ok().flatten().map(|b| lodforge_dyndolod::lodmodels::crc32(&b)).unwrap_or(0);
            (t, crc)
        })
        .collect();
    lodforge_texgen::Source { model: model.to_string(), model_crc: lodforge_dyndolod::lodmodels::crc32(nif_bytes), textures }
}

/// How many sidecars agree with the golden ones, field by field.
#[derive(Default)]
struct SidecarStats {
    n: usize,
    model: usize,
    model_crc: usize,
    texture_set: usize,
    texture_order: usize,
    texture_crcs: usize,
}

impl SidecarStats {
    fn add(&mut self, gold: &HashMap<String, String>, src: &lodforge_texgen::Source) {
        self.n += 1;
        let g = |k: &str| gold.get(k).cloned().unwrap_or_default();
        self.model += usize::from(g("MODEL") == src.model);
        if g("MODEL") != src.model && !src.model.starts_with("DynDOLOD") && self.n - self.model <= 16 {
            println!("  Model= golden {} ours {}", g("MODEL"), src.model);
        }
        self.model_crc += usize::from(g("MODELCRC32").eq_ignore_ascii_case(&format!("{:08X}", src.model_crc)));
        let gold_tex: Vec<(String, String)> = (1..).map_while(|k| gold.get(&format!("TEXTURE_{k}")).map(|t| (t.to_ascii_lowercase(), g(&format!("TEXTURECRC32_{k}"))))).collect();
        let ours: Vec<(String, String)> = src.textures.iter().map(|(t, c)| (t.to_ascii_lowercase(), format!("{c:08X}"))).collect();
        let (mut a, mut b) = (gold_tex.clone(), ours.clone());
        a.sort();
        b.sort();
        let names = |v: &[(String, String)]| v.iter().map(|x| x.0.clone()).collect::<Vec<_>>();
        self.texture_set += usize::from(names(&a) == names(&b));
        if names(&a) != names(&b) {
            println!("  textures: golden {:?} ours {:?}", names(&a), names(&b));
        }
        self.texture_crcs += usize::from(a == b);
        self.texture_order += usize::from(names(&gold_tex) == names(&ours));
    }

    fn print(&self) {
        println!(
            "sidecars: {} compared; Model= {}, ModelCRC32 {}, texture set {} (same order {}), texture CRC32s {}",
            self.n, self.model, self.model_crc, self.texture_set, self.texture_order, self.texture_crcs
        );
    }
}

/// Every `.nif` below `dir` (recursive), as paths relative to it.
fn nifs_below(dir: &std::path::Path, rel: &std::path::Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for e in std::fs::read_dir(dir.join(rel))?.flatten() {
        let r = rel.join(e.file_name());
        if e.path().is_dir() {
            nifs_below(dir, &r, out)?;
        } else if r.extension().is_some_and(|x| x.eq_ignore_ascii_case("nif")) {
            out.push(r);
        }
    }
    Ok(())
}

fn mean_opaque(rgba: &[u8]) -> [f64; 3] {
    let (mut s, mut k) = ([0f64; 3], 0f64);
    for px in rgba.chunks_exact(4).filter(|p| p[3] >= 128) {
        for c in 0..3 {
            s[c] += f64::from(px[c]);
        }
        k += 1.0;
    }
    s.map(|v| v / k.max(1.0))
}

/// Comparison of rendered billboards (both views, `_n`, sidecar values) with golden ones.
#[derive(Default)]
struct RenderedStats {
    n: usize,
    cmp_n: usize,
    size_ok: [usize; 2],
    /// Width, Depth, Height, ShiftZ, Adjust_1, Adjust_2 within tolerance.
    fields_ok: [usize; 6],
    /// Diffuse PSNR (golden-opaque, both-opaque), alpha agreement, normal PSNR (both-opaque).
    sums: [f64; 4],
    nmean: [[f64; 3]; 2],
    /// Least-squares sums for golden normal (R, G) against ours.
    ncov: [f64; 7],
    sidecars: SidecarStats,
}

impl RenderedStats {
    fn add(&mut self, rb: &lodforge_texgen::RenderedBillboard, gold_dir: &std::path::Path, stem: &str, src: &lodforge_texgen::Source) -> Result<()> {
        let Ok(gt) = std::fs::read_to_string(gold_dir.join(format!("{stem}.txt"))) else { return Ok(()) };
        let gold = parse_txt(&gt);
        self.n += 1;
        self.sidecars.add(&gold, src);
        let g = |k: &str| gold.get(k).and_then(|v| v.parse::<f32>().ok()).unwrap_or(f32::NAN);
        for (i, (k, ours, tol)) in [("WIDTH", rb.width, 0.5), ("DEPTH", rb.depth, 0.5), ("HEIGHT", rb.height, 1.0), ("SHIFTZ", rb.shift_z, 1.0), ("ADJUST_1", rb.views[0].adjust, 1.0), ("ADJUST_2", rb.views[1].adjust, 1.0)].into_iter().enumerate() {
            self.fields_ok[i] += usize::from((ours - g(k)).abs() <= tol);
        }
        if std::env::var_os("TG_FIT").is_some() {
            println!(
                "RENDERED\t{stem}\t{}x{} {}x{}\tW {:.2}/{:.2} D {:.2}/{:.2} H {:.2}/{:.2} Sz {:.2}/{:.2}\tW1 {:.2}/{:.2} A1 {:.2}/{:.2}\tW2 {:.2}/{:.2} H2 {:.2}/{:.2} A2 {:.2}/{:.2}",
                rb.views[0].width_px, rb.views[0].height_px, rb.views[1].width_px, rb.views[1].height_px,
                rb.width, g("WIDTH"), rb.depth, g("DEPTH"), rb.height, g("HEIGHT"), rb.shift_z, g("SHIFTZ"),
                rb.views[0].width, g("WIDTH_1"), rb.views[0].adjust, g("ADJUST_1"),
                rb.views[1].width, g("WIDTH_2"), rb.views[1].height, g("HEIGHT_2"), rb.views[1].adjust, g("ADJUST_2")
            );
        }
        for (k, v) in rb.views.iter().enumerate() {
            let (Ok(gd), Ok(gn)) = (std::fs::read(gold_dir.join(format!("{stem}_{}.dds", k + 1))), std::fs::read(gold_dir.join(format!("{stem}_{}_n.dds", k + 1)))) else { continue };
            let gi = lodforge_dds::decode(&gd, &lodforge_dds::parse(&gd)?, 0)?;
            let gni = lodforge_dds::decode(&gn, &lodforge_dds::parse(&gn)?, 0)?;
            if std::env::var_os("TG_PNG").is_some() {
                for (name, w, h, d) in [("gold", gi.width, gi.height, &gi.rgba), ("ours", v.width_px, v.height_px, &v.rgba), ("goldn", gni.width, gni.height, &gni.rgba), ("oursn", v.width_px, v.height_px, &v.normal)] {
                    let opaque: Vec<u8> = d.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2], 255]).collect();
                    let f = std::fs::File::create(format!("bench/tr_{stem}_{}_{name}.png", k + 1))?;
                    let mut e = png::Encoder::new(std::io::BufWriter::new(f), w, h);
                    e.set_color(png::ColorType::Rgba);
                    e.write_header()?.write_image_data(&opaque)?;
                }
            }
            for (side, img) in [(0, &gni.rgba), (1, &v.normal)] {
                let m = mean_opaque(img);
                for c in 0..3 {
                    self.nmean[side][c] += m[c];
                }
            }
            if gi.width == v.width_px && gi.height == v.height_px {
                self.size_ok[k] += 1;
                let c = lodforge_texgen::compare(&gi.rgba, &v.rgba);
                let cn = lodforge_texgen::compare(&gni.rgba, &v.normal);
                for (s, x) in self.sums.iter_mut().zip([c.psnr_ref_opaque, c.psnr_both_opaque, c.alpha_agreement, cn.psnr_both_opaque]) {
                    *s += x;
                }
                self.cmp_n += 1;
                for (g, o) in gni.rgba.chunks_exact(4).zip(v.normal.chunks_exact(4)).filter(|(g, o)| g[3] >= 128 && o[3] >= 128) {
                    let (gx, gy, ox, oy) = (f64::from(g[0]) - 128.0, f64::from(g[1]) - 128.0, f64::from(o[0]) - 128.0, f64::from(o[1]) - 128.0);
                    for (i, x) in [ox * ox, ox * oy, oy * oy, gx * ox, gx * oy, gy * ox, gy * oy].into_iter().enumerate() {
                        self.ncov[i] += x;
                    }
                }
            }
        }
        Ok(())
    }

    fn print(&self, what: &str, ms: u128) {
        let c = self.cmp_n.max(1) as f64;
        let views = (2 * self.n).max(1) as f64;
        let f = &self.fields_ok;
        let (g, o) = (self.nmean[0].map(|v| v / views), self.nmean[1].map(|v| v / views));
        println!(
            "{what}: {} in {ms} ms; golden size {}/{} front, {}/{} side; Width {}, Depth {}, Height {}, ShiftZ {}, Adjust_1 {}, Adjust_2 {} within tolerance; diffuse PSNR {:.2} dB golden-opaque, {:.2} dB both-opaque, alpha agreement {:.1}%; normal PSNR {:.2} dB; normal means golden ({:.0}, {:.0}, {:.0}) ours ({:.0}, {:.0}, {:.0})",
            self.n, self.size_ok[0], self.n, self.size_ok[1], self.n, f[0], f[1], f[2], f[3], f[4], f[5],
            self.sums[0] / c, self.sums[1] / c, 100.0 * self.sums[2] / c, self.sums[3] / c, g[0], g[1], g[2], o[0], o[1], o[2]
        );
        // Least-squares 2x2 map golden (R, G) = M * ours (R, G).
        let v = &self.ncov;
        let det = v[0] * v[2] - v[1] * v[1];
        let m = [(v[3] * v[2] - v[4] * v[1]) / det, (v[4] * v[0] - v[3] * v[1]) / det, (v[5] * v[2] - v[6] * v[1]) / det, (v[6] * v[0] - v[5] * v[1]) / det];
        println!("normal fit: golden R = {:.2} R + {:.2} G, golden G = {:.2} R + {:.2} G", m[0], m[1], m[2], m[3]);
        self.sidecars.print();
    }
}

/// Writes a rendered billboard (`_1`, `_1_n`, `_2`, `_2_n` and the `.txt`) into `dir`.
pub fn write_rendered(gpu: &lodforge_gpu::Gpu, dir: &std::path::Path, stem: &str, rb: &lodforge_texgen::RenderedBillboard, complex: Option<bool>, src: &lodforge_texgen::Source) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut crcs = [0u32; 2];
    for (k, v) in rb.views.iter().enumerate() {
        let dds = bc7_dds(gpu, v.width_px, v.height_px, &v.rgba)?;
        crcs[k] = lodforge_dyndolod::lodmodels::crc32(&dds);
        std::fs::write(dir.join(format!("{stem}_{}.dds", k + 1)), &dds)?;
        std::fs::write(dir.join(format!("{stem}_{}_n.dds", k + 1)), bc7_dds(gpu, v.width_px, v.height_px, &v.normal)?)?;
    }
    std::fs::write(dir.join(format!("{stem}.txt")), lodforge_texgen::rendered_sidecar(rb, 1.0, complex, crcs, src))?;
    Ok(())
}

/// One finished rendered (trunk) billboard.
pub struct TrunkBillboard {
    pub stem: String,
    /// Output folder relative to Data (`textures\<path below Billboards>`).
    pub out_rel: PathBuf,
    pub billboard: lodforge_texgen::RenderedBillboard,
    pub source: lodforge_texgen::Source,
}

/// Where TexGen finds the render models of rendered billboards: `<dyndolod>\Edit Scripts\DynDOLOD\
/// Render\Skyrim\Billboards` (DynDOLOD's own) and `Data\DynDOLOD\Render\Billboards` (shipped by
/// mods, e.g. Nature of the Wild Lands; loose or in a BSA).
const DATA_BILLBOARDS: &str = "dyndolod\\render\\billboards\\";

/// One render model of a rendered billboard.
struct TrunkNif {
    /// Path below the `Billboards` folder (the output path below `textures\`).
    rel: PathBuf,
    /// `Model=` of the sidecar.
    model: String,
    /// Tools-folder file, or `None` for a data (VFS) file.
    file: Option<PathBuf>,
}

/// Every render model of both roots (filtered by `filter`), sorted by `rel`. A data file
/// replaces a tools file with the same `rel` (none overlap in the E2E profile).
fn trunk_nifs(vfs: &lodforge_archive::Vfs, dyndolod: &std::path::Path, filter: Option<&str>) -> Result<Vec<TrunkNif>> {
    let mut by_rel: std::collections::BTreeMap<String, TrunkNif> = Default::default();
    let root = dyndolod.join(r"Edit Scripts\DynDOLOD\Render\Skyrim\Billboards");
    if root.is_dir() {
        let mut nifs = Vec::new();
        nifs_below(&root, std::path::Path::new(""), &mut nifs)?;
        for rel in nifs {
            let key = rel.to_string_lossy().replace('/', "\\").to_ascii_lowercase();
            let model = format!("dyndolod\\render\\skyrim\\billboards\\{key}");
            by_rel.insert(key, TrunkNif { file: Some(root.join(&rel)), rel, model });
        }
    }
    for path in vfs.list_prefix(DATA_BILLBOARDS) {
        if !path.ends_with(".nif") {
            continue;
        }
        let key = path[DATA_BILLBOARDS.len()..].to_string();
        by_rel.insert(key.clone(), TrunkNif { rel: PathBuf::from(&key), model: path, file: None });
    }
    Ok(by_rel.into_iter().filter(|(k, _)| filter.is_none_or(|f| k.contains(&f.to_ascii_lowercase()))).map(|(_, n)| n).collect())
}

/// Rendered (trunk) billboards for every render model of [`trunk_nifs`] (filtered by `filter`), in
/// batches of [`BATCH`]. Returns the number made.
pub fn trunk_billboards(p: &crate::profile::Profile, gpu: &lodforge_gpu::Gpu, renderer: &Renderer, dyndolod: &std::path::Path, filter: Option<&str>, mut sink: impl FnMut(TrunkBillboard) -> Result<()>) -> Result<usize> {
    let nifs = trunk_nifs(&p.vfs, dyndolod, filter)?;
    // TexGen's `RenderBillboards*` lighting and `RenderModelHeightMultiplier`.
    let settings = Settings { light: lodforge_texgen::Light { direct: 0.5, ambient: 1.5, dir: [0.0, 0.0, 1.0], pbr_scale: 0.7, normal_pull: 1.0 }, ..Settings::default() };
    let mut crc_cache = HashMap::new();
    let mut made = 0;
    for chunk in nifs.chunks(BATCH) {
        let mut prepared = Vec::new();
        for n in chunk {
            let rel = &n.rel;
            let bytes = match &n.file {
                Some(f) => std::fs::read(f)?,
                None => p.vfs.read(&n.model)?.with_context(|| format!("{} vanished from the VFS", n.model))?,
            };
            let nif = match lodforge_nif::parse_all_geometry(&bytes) {
                Ok(n) => n,
                Err(e) => {
                    println!("  skip {}: {e}", rel.display());
                    continue;
                }
            };
            let mut scene = Scene::from_nif(&nif);
            lodforge_texgen::resolve_textures(&mut scene, |t| p.vfs.exists(t));
            let tex = lodforge_texgen::load_textures(&scene, |path| load_texture(gpu, &p.vfs, path));
            let source = source_cached(&p.vfs, &scene, &n.model, &bytes, &mut crc_cache);
            prepared.push((rel, scene, tex, source));
        }
        let items: Vec<lodforge_texgen::Item> = prepared.iter().map(|(_, scene, tex, _)| lodforge_texgen::Item { scene, textures: tex, scale: 1.85 }).collect();
        let rbs = lodforge_texgen::make_rendered_billboards(gpu, renderer, &items, &settings, lodforge_texgen::TRUNK_YAWS);
        drop(items);
        for ((rel, _, _, source), rb) in prepared.into_iter().zip(rbs) {
            let Some(billboard) = rb else {
                println!("  skip {}: no billboard", rel.display());
                continue;
            };
            made += 1;
            let stem = rel.file_stem().unwrap_or_default().to_string_lossy().into_owned();
            let out_rel = std::path::Path::new("textures").join(rel.parent().unwrap_or(std::path::Path::new("")));
            sink(TrunkBillboard { stem, out_rel, billboard, source })?;
        }
    }
    Ok(made)
}

/// `--trunks`: [`trunk_billboards`] compared with the golden `textures\<path>\<stem>_1.dds` ...
fn run_trunks(a: &Args, p: &crate::profile::Profile, gpu: &lodforge_gpu::Gpu, renderer: &Renderer, dyndolod: &std::path::Path) -> Result<()> {
    let t = Instant::now();
    let mut stats = RenderedStats::default();
    let made = trunk_billboards(p, gpu, renderer, dyndolod, a.filter.as_deref(), |tb| {
        if let Some(g) = &a.golden {
            stats.add(&tb.billboard, &g.join(&tb.out_rel), &tb.stem, &tb.source)?;
        }
        if let Some(out) = &a.out {
            write_rendered(gpu, &out.join(&tb.out_rel), &tb.stem, &tb.billboard, None, &tb.source)?;
        }
        Ok(())
    })?;
    println!("rendered billboards: {made} made");
    if a.golden.is_some() {
        stats.print("rendered billboards", t.elapsed().as_millis());
    }
    Ok(())
}

/// HD grass billboards (TexGen "HD Grass": two views with `_n`, `Complex=`), for the grass types
/// of a golden run's `textures\terrain\lodgen`.
fn run_hd_grass(a: &Args, p: &crate::profile::Profile, gpu: &lodforge_gpu::Gpu, renderer: &Renderer) -> Result<()> {
    let lo = &p.load_order;
    let grasses = read_grass_models(lo)?;
    // TexGen's `GrassHDBillboards*` lighting, `GrassMSAlphaThreshold`, `GrassModelHeightMultiplier`.
    let settings = Settings { light: lodforge_texgen::Light { direct: 0.0, ambient: 1.0, dir: [0.0, 0.0, 1.0], pbr_scale: 0.7, normal_pull: 1.0 }, coverage_threshold: 112, ..Settings::default() };
    let yaws = [0.0, 90.0];
    let root = a.golden.as_ref().context("--hd-grass needs --golden (it names the grass types)")?.join(r"textures\terrain\lodgen");
    let t = Instant::now();
    let mut stats = RenderedStats::default();
    let mut items = Vec::new();
    for pd in std::fs::read_dir(&root)?.flatten().filter(|d| d.path().is_dir()) {
        for f in std::fs::read_dir(pd.path())?.flatten() {
            let name = f.file_name().to_string_lossy().into_owned();
            if name.ends_with(".txt") && a.filter.as_ref().is_none_or(|flt| name.contains(flt.as_str())) {
                items.push((pd.file_name().to_string_lossy().into_owned(), f.path()));
            }
        }
    }
    items.sort();
    for (plugin, txt_path) in &items {
        let stem = txt_path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
        let Some((_, fid)) = stem.rsplit_once('_') else { continue };
        let Ok(local) = u32::from_str_radix(fid, 16) else { continue };
        let Some(model) = lodforge_dyndolod::bases::global_id(lo, plugin, local).and_then(|id| grasses.get(&id)) else { println!("  skip {stem}: no GRAS"); continue };
        let model_path = lodforge_dyndolod::bases::model_path(model);
        let Some(bytes) = p.vfs.read(&model_path).ok().flatten() else { println!("  skip {model_path}: not in the VFS"); continue };
        let Ok(nif) = lodforge_nif::parse_all_geometry(&bytes) else { continue };
        let mut scene = Scene::from_nif(&nif);
        lodforge_texgen::resolve_textures(&mut scene, |t| p.vfs.exists(t));
        let tex = lodforge_texgen::load_textures(&scene, |path| load_texture(gpu, &p.vfs, path));
        // A complex grass texture is twice as tall as it is wide (diffuse above, normal below).
        let mut complex = false;
        for m in &mut scene.meshes {
            m.complex = tex.get(&m.diffuse).is_some_and(|t| t.height == 2 * t.width);
            if m.complex {
                // The normal map is the texture's bottom half: slot 1 is not used (nor listed).
                let n = m.normal.clone();
                m.texture_names.retain(|t| !t.eq_ignore_ascii_case(&n));
            }
            complex |= m.complex;
        }
        let Some(rb) = lodforge_texgen::make_rendered_billboard(gpu, renderer, &scene, &tex, 1.0, &settings, yaws) else { println!("  skip {stem}: no billboard"); continue };
        let src = source(&p.vfs, &scene, &format!("Meshes\\{}", &model_path["meshes\\".len()..]), &bytes);
        if std::env::var_os("TG_FIT").is_some() {
            println!("COMPLEX\t{stem}\t{complex}");
        }
        stats.add(&rb, txt_path.parent().unwrap_or(&root), &stem, &src)?;
        if let Some(out) = &a.out {
            write_rendered(gpu, &out.join(r"textures\terrain\lodgen").join(plugin), &stem, &rb, Some(complex), &src)?;
        }
    }
    stats.print("HD grass billboards", t.elapsed().as_millis());
    Ok(())
}

/// Winning GRAS records' models.
fn read_grass_models(lo: &lodforge_esp::LoadOrder) -> Result<HashMap<u32, String>> {
    let mut out = HashMap::new();
    for (id, (_, r)) in lodforge_esp::winning_records(lo, lodforge_esp::Sig(*b"GRAS"))? {
        let data = r.data()?;
        if let Some(f) = lodforge_esp::fields(&data).find(|f| &f.sig.0 == b"MODL") {
            out.insert(id, f.zstring());
        }
    }
    Ok(out)
}

/// [`source`] with the texture CRC32s memoised by path (trees share textures).
fn source_cached(vfs: &lodforge_archive::Vfs, scene: &Scene, model: &str, nif_bytes: &[u8], cache: &mut HashMap<String, u32>) -> lodforge_texgen::Source {
    let textures = lodforge_texgen::texture_names(scene)
        .into_iter()
        .map(|t| {
            let key = t.to_ascii_lowercase();
            let crc = *cache.entry(key.clone()).or_insert_with(|| vfs.read(&key).ok().flatten().map(|b| lodforge_dyndolod::lodmodels::crc32(&b)).unwrap_or(0));
            (t, crc)
        })
        .collect();
    lodforge_texgen::Source { model: model.to_string(), model_crc: lodforge_dyndolod::lodmodels::crc32(nif_bytes), textures }
}

/// TexGen's `TexGen_SSE_Tree_Billboards.txt`: every candidate as `Plugin;FormID;EditorID`, with
/// `,False` when rejected and `,True` when an override line forces it on (CRLF lines).
pub fn tree_report(cands: &[Candidate]) -> String {
    let mut s = String::new();
    for c in cands {
        let suffix = if !c.accepted { ",False" } else if c.forced == Some(true) { ",True" } else { "" };
        s.push_str(&format!("{};{:08X};{}{suffix}\r\n", c.plugin, c.local, c.editor_id));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trunk_nifs_from_tools_and_data() {
        let tmp = std::env::temp_dir().join(format!("lodforge-trunk-nifs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let tools = tmp.join("DynDOLOD");
        let data = tmp.join("mod");
        let t = tools.join(r"Edit Scripts\DynDOLOD\Render\Skyrim\Billboards\dyndolod\lod\trees");
        let d = data.join(r"DynDOLOD\Render\billboards\dyndolod\lod\trees\Nature of the Wild Lands");
        std::fs::create_dir_all(&t).unwrap();
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(t.join("treeaspen01_trunk.nif"), b"").unwrap();
        std::fs::write(d.join("alder_cluster_medium03_summer_B7DCBA97_trunk.nif"), b"").unwrap();
        std::fs::write(d.join("readme.txt"), b"").unwrap();
        let vfs = lodforge_archive::Vfs::from_roots(vec![data]).unwrap();
        let nifs = trunk_nifs(&vfs, &tools, None).unwrap();
        let got: Vec<(String, String, bool)> = nifs.iter().map(|n| (n.rel.to_string_lossy().into_owned(), n.model.clone(), n.file.is_some())).collect();
        assert_eq!(
            got,
            vec![
                (
                    r"dyndolod\lod\trees\nature of the wild lands\alder_cluster_medium03_summer_b7dcba97_trunk.nif".to_string(),
                    r"dyndolod\render\billboards\dyndolod\lod\trees\nature of the wild lands\alder_cluster_medium03_summer_b7dcba97_trunk.nif".to_string(),
                    false
                ),
                (r"dyndolod\lod\trees\treeaspen01_trunk.nif".to_string(), r"dyndolod\render\skyrim\billboards\dyndolod\lod\trees\treeaspen01_trunk.nif".to_string(), true),
            ]
        );
        assert_eq!(trunk_nifs(&vfs, &tools, Some("Nature")).unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
