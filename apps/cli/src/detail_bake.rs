//! Detail bake step of `build --runtime lodforge` (`lodforge-detailbake`): every instanced LOD
//! model whose references use a full model under `meshes\landscape\` (mountains, cliffs, rocks)
//! gets its own texture baked from the full model, and the statics entries drawing it are
//! pointed at the baked model with its own textures. Runs after the statics files are written
//! and before the static mesh pack, which then places the baked textures on its pages
//! (`tree_pack::BAKED_TILE_CAP`).
//!
//! Entries with AltTextures keep their model unless every AltTextures diffuse is the full
//! model's own material (its PBR `linear` LOD copy, for instance): a recoloured variant of a
//! rock would otherwise lose its colours.
//!
//! Writes `SKSE\Plugins\LODForgeRuntime\DetailBake.txt` (one line per baked model: LOD model,
//! full model, baked model, size, density, GPU bytes).

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::sync::Mutex;
use std::time::Instant;

use anyhow::{Context, Result};
use lodforge_detailbake::{Baked, Params, Source, Textures};
use lodforge_instances::WriteOptions;
use lodforge_instances::statics::{MESH_PASSTHRU, MESH_TREE, StaticsFile};

use crate::profile::Profile;

/// Data-relative folders of the baked models and textures (the static pack gives textures
/// under `\lodforge\bake\` their full size).
pub const MESH_DIR: &str = r"meshes\lod\lodforge\baked";
pub const TEXTURE_DIR: &str = r"textures\lodforge\bake";
pub const REPORT: &str = "DetailBake.txt";

#[derive(Default, Debug)]
pub struct Report {
    /// Distinct LOD models eligible / baked.
    pub models: usize,
    pub baked: usize,
    /// Statics entries pointed at a baked model, and those kept for their AltTextures.
    pub entries: usize,
    pub kept_alt: usize,
    /// Snow entries of PBR full models (snow colour scaled, vanilla snow flag off).
    pub pbr_snow: usize,
    /// Snow entries of LOD models left unbaked whose full models are PBR (snow colour scaled).
    pub pbr_snow_unbaked: usize,
    /// Full model variants baked with a reference's own full-model AltTextures.
    pub variants: usize,
    /// Landscape LOD models left alone because several full models share them.
    pub shared: usize,
    /// Landscape LOD models left alone for their alpha-tested / effect parts.
    pub cutout: usize,
    pub shared_examples: Vec<String>,
    pub failed: Vec<(String, String)>,
    /// Per LOD level (4, 8, 16): models, GPU bytes of their textures at full resolution.
    pub levels: [(usize, u64); 3],
    pub ms: u128,
}

impl Report {
    pub fn summary(&self) -> String {
        let mb = |b: u64| b as f64 / (1024.0 * 1024.0);
        let total: u64 = self.levels.iter().map(|l| l.1).sum();
        format!(
            "detail bake: {} of {} landscape LOD models baked ({} full models with AltTextures baked as their own variant; {} failed, {} shared by several full models and {} with cut-out parts left alone), {} statics entries ({} kept for AltTextures, {} snow entries of PBR models dimmed, {} more on unbaked LOD models); textures {:.0} MB BC7 at full size (LOD4 {} / {:.0} MB, LOD8 {} / {:.0} MB, LOD16 {} / {:.0} MB; the runtime's iSkipMips=1 shows a quarter); {:.1} s",
            self.baked,
            self.models,
            self.variants,
            self.failed.len(),
            self.shared,
            self.cutout,
            self.entries,
            self.kept_alt,
            self.pbr_snow,
            self.pbr_snow_unbaked,
            mb(total),
            self.levels[0].0,
            mb(self.levels[0].1),
            self.levels[1].0,
            mb(self.levels[1].1),
            self.levels[2].0,
            mb(self.levels[2].1),
            self.ms as f64 / 1000.0
        )
    }
}

fn level_index(level: u8) -> usize {
    match level {
        0..=4 => 0,
        5..=8 => 1,
        _ => 2,
    }
}

fn normalize_model(m: &str) -> String {
    let m = m.trim().replace('/', "\\").to_ascii_lowercase();
    let m = m.strip_prefix("data\\").map(str::to_string).unwrap_or(m);
    if m.starts_with("meshes\\") { m } else { format!("meshes\\{m}") }
}

/// A texture's material name: file stem without a `linear` suffix (the PBR LOD copy) and
/// without the `\pbr\` folder.
fn material_stem(path: &str) -> String {
    let p = path.to_ascii_lowercase().replace('/', "\\");
    let stem = p.rsplit('\\').next().unwrap_or(&p).trim_end_matches(".dds");
    stem.strip_suffix("linear").unwrap_or(stem).to_string()
}

/// A landscape full model: under `meshes\landscape\`, also as a PGPatcher duplicate
/// (`meshes\_pgpatcher_dups\<n>\landscape\...`, a copy PGPatcher made to give some references
/// other textures).
fn is_landscape_model(m: &str) -> bool {
    original_model(m).starts_with("meshes\\landscape\\")
}

/// Applies a reference's full-model AltTextures to the full model before baking: each entry's
/// non-empty slots replace the named shape's textures. The PBR `linear` LOD copies the file
/// names are swapped for their source textures when those exist (the bake applies the PBR
/// albedo scale itself).
fn apply_alt(m: &mut lodforge_objlod::mesh::LodMesh, alt: &[(String, Vec<String>)], exists: &dyn Fn(&str) -> bool) {
    for part in &mut m.parts {
        let Some((_, slots)) = alt.iter().find(|(s, _)| s.eq_ignore_ascii_case(&part.name)) else { continue };
        for (k, t) in slots.iter().enumerate().take(part.textures.len()) {
            if t.is_empty() {
                continue;
            }
            let src = t.strip_suffix("linear.dds").map(|s| format!("{s}.dds")).filter(|s| exists(s));
            part.textures[k] = src.unwrap_or_else(|| t.clone());
        }
    }
}

/// Whether most of a full model's surface (by triangles) uses PBR textures (`\pbr\` diffuse).
fn full_model_is_pbr(p: &Profile, full: &str) -> bool {
    let Some(m) = p.vfs.read(full).ok().flatten().and_then(|b| lodforge_objlod::mesh::import(full, &b).ok()) else { return false };
    let (mut pbr, mut all) = (0usize, 0usize);
    for part in &m.parts {
        all += part.triangles.len();
        if part.textures[0].to_ascii_lowercase().replace('/', "\\").contains("\\pbr\\") {
            pbr += part.triangles.len();
        }
    }
    pbr * 2 > all
}

/// The model a PGPatcher duplicate copies (`meshes\_pgpatcher_dups\<n>\x` -> `meshes\x`);
/// other models unchanged.
fn original_model(m: &str) -> String {
    match m.strip_prefix("meshes\\_pgpatcher_dups\\").and_then(|r| r.split_once('\\')) {
        Some((_, rest)) => format!("meshes\\{rest}"),
        None => m.to_string(),
    }
}

/// Statics entry (worldspace, season as the file names them, `MESH` index) -> the (full model,
/// [`alt_key`] of the full model's own AltTextures) of the references drawing it, lower case.
pub type FullModels = HashMap<(String, String, u16), BTreeSet<(String, String)>>;

/// A reference's full-model AltTextures (shape, nine slots) as a key; [`parse_alt_key`] reads it.
pub fn alt_key(alt: &[(String, Vec<String>)]) -> String {
    alt.iter().map(|(s, t)| format!("{}={}", s.to_ascii_lowercase(), t.join(","))).collect::<Vec<_>>().join("|")
}

fn parse_alt_key(key: &str) -> Vec<(String, Vec<String>)> {
    key.split('|').filter_map(|e| e.split_once('=')).map(|(s, t)| (s.to_string(), t.split(',').map(str::to_string).collect())).collect()
}

/// What a full model variant looks like: (full model, full-model AltTextures key).
type Variant = (String, String);

/// The full model variant a statics entry is baked from: one model (PGPatcher's duplicates of
/// it count as the same mesh, baked from the original when it is there) with one set of
/// full-model AltTextures. `Err(fulls)` when several differ (a bake from one would put its
/// surface on the others), `Ok(None)` when not eligible.
fn variant_of(full_of: &FullModels, s: &StaticsFile, index: usize, model: &str) -> std::result::Result<Option<Variant>, String> {
    let Some(fulls) = full_of.get(&(s.common.world.clone(), s.common.season.clone(), index as u16)) else { return Ok(None) };
    // References whose own model is a LOD mesh (DynDOLOD's generated copies name the LOD
    // model as their model) do not make a second full model.
    let fulls: BTreeSet<(String, String)> = fulls.iter().map(|(f, a)| (normalize_model(f), a.clone())).filter(|(f, _)| !f.starts_with("meshes\\lod\\") && !f.contains("\\lod\\") && f != model).collect();
    let originals: BTreeSet<(String, String)> = fulls.iter().map(|(f, a)| (original_model(f), a.clone())).collect();
    let mut it = originals.iter();
    let Some((orig, alt)) = it.next() else { return Ok(None) };
    if it.next().is_some() {
        return Err(fulls.iter().map(|(f, a)| if a.is_empty() { f.clone() } else { format!("{f} (alt {a})") }).collect::<Vec<_>>().join(", "));
    }
    let full = if fulls.contains(&(orig.clone(), alt.clone())) { orig.clone() } else { fulls.iter().next().unwrap().0.clone() };
    Ok(is_landscape_model(&full).then(|| (full, alt.clone())))
}

/// Bakes the eligible LOD models of the statics files in `out` and rewrites those files.
/// `full_of`: which full models (and their AltTextures) each LOD entry stands for.
pub fn run(p: &Profile, out: &Path, full_of: &FullModels, write: WriteOptions) -> Result<Report> {
    let t0 = Instant::now();
    let mut rep = Report::default();
    let dir = crate::instances::out_dir(out);
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)?.flatten().map(|e| e.path()).filter(|p| p.to_string_lossy().to_ascii_lowercase().ends_with(".statics.bin")).collect();
    files.sort();
    // Eligible LOD models: full model variant -> (LOD model, finest level it is drawn at).
    let mut work: BTreeMap<Variant, BTreeMap<String, u8>> = BTreeMap::new();
    let mut shared: BTreeSet<String> = BTreeSet::new();
    for f in &files {
        let s = StaticsFile::from_bytes(&std::fs::read(f)?).with_context(|| format!("reading {}", f.display()))?;
        let mut level_of: HashMap<u16, u8> = HashMap::new();
        for ((level, _, _), recs) in &s.blocks {
            for r in recs {
                let e = level_of.entry(r.mesh).or_insert(*level);
                *e = (*e).min(*level);
            }
        }
        for (i, m) in s.meshes.iter().enumerate() {
            if m.flags & (MESH_TREE | MESH_PASSTHRU) != 0 {
                continue;
            }
            let model = m.model.to_ascii_lowercase();
            let v = match variant_of(full_of, &s, i, &model) {
                Ok(Some(v)) => v,
                Ok(None) => continue,
                Err(fulls) => {
                    if shared.insert(model.clone()) && rep.shared_examples.len() < 8 {
                        rep.shared_examples.push(format!("{model} <- {fulls}"));
                    }
                    continue;
                }
            };
            let Some(&level) = level_of.get(&(i as u16)) else { continue };
            let e = work.entry(v).or_default().entry(model).or_insert(level);
            *e = (*e).min(level);
        }
    }
    rep.models = work.values().map(BTreeMap::len).sum();
    rep.shared = shared.len();
    let read = |path: &str| p.vfs.read(path).ok().flatten();
    let exists = |path: &str| p.vfs.exists(path);
    let tex = Textures::new(&read, &exists);
    // Full model -> its diffuse material stems; LOD model -> (baked model path, level, baked).
    let full_materials: Mutex<HashMap<Variant, Vec<String>>> = Mutex::new(HashMap::new());
    let full_pbr: Mutex<BTreeSet<Variant>> = Mutex::new(BTreeSet::new());
    let results: Mutex<Vec<(String, Variant, String, u8, Baked)>> = Mutex::new(Vec::new());
    let failed: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());
    let cutout: Mutex<Vec<String>> = Mutex::new(Vec::new());
    rep.variants = work.keys().filter(|(_, a)| !a.is_empty()).count();
    let queue: Mutex<Vec<(Variant, BTreeMap<String, u8>)>> = Mutex::new(work.into_iter().collect());
    // A few full models at a time: each bake is parallel inside, and a 2048 px bake holds
    // about 400 MB of working buffers.
    let workers = 3.min(rayon::current_num_threads().max(1));
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let Some((v, lods)) = queue.lock().unwrap().pop() else { break };
                    let full = &v.0;
                    let src = p.vfs.read(full).ok().flatten().and_then(|b| lodforge_objlod::mesh::import(full, &b).ok()).and_then(|mut m| {
                        apply_alt(&mut m, &parse_alt_key(&v.1), &exists);
                        Source::new(&m)
                    });
                    let Some(src) = src else {
                        for lod in lods.keys() {
                            failed.lock().unwrap().push((lod.clone(), format!("full model {full} not loadable")));
                        }
                        continue;
                    };
                    full_materials.lock().unwrap().insert(v.clone(), src.diffuses().iter().map(|d| material_stem(d)).collect());
                    if src.is_pbr() {
                        full_pbr.lock().unwrap().insert(v.clone());
                    }
                    for (lod, level) in lods {
                        let Some(mesh) = p.vfs.read(&lod).ok().flatten().and_then(|b| lodforge_objlod::mesh::import(&lod, &b).ok()) else {
                            failed.lock().unwrap().push((lod.clone(), "LOD model not loadable".into()));
                            continue;
                        };
                        // Alpha-tested skirts and effect parts cannot be merged into one opaque
                        // baked shape: such LOD models keep their own textures.
                        if lodforge_detailbake::has_cutout_parts(&mesh) {
                            cutout.lock().unwrap().push(lod.clone());
                            continue;
                        }
                        let stem = lod.rsplit('\\').next().unwrap_or(&lod).trim_end_matches(".nif").to_string();
                        // Name by LOD model, plus the variant's textures when it has its own.
                        let id = if v.1.is_empty() { lod.clone() } else { format!("{lod}|{}", v.1) };
                        let name = format!("{stem}_{:08x}", crc32fast::hash(id.as_bytes()));
                        let (d, n) = (format!("{TEXTURE_DIR}\\{name}.dds"), format!("{TEXTURE_DIR}\\{name}_n.dds"));
                        match lodforge_detailbake::bake(&src, &mesh, &tex, &Params::for_level(level), &d, &n) {
                            Ok(b) => results.lock().unwrap().push((lod.clone(), v.clone(), format!("{MESH_DIR}\\{name}.nif"), level, b)),
                            Err(e) => failed.lock().unwrap().push((lod.clone(), format!("{e:#}"))),
                        }
                    }
                }
            });
        }
    });
    tex.clear();
    let results = results.into_inner().unwrap();
    rep.failed = failed.into_inner().unwrap();
    let cutout = cutout.into_inner().unwrap();
    rep.cutout = cutout.len();
    let full_materials = full_materials.into_inner().unwrap();
    let full_pbr = full_pbr.into_inner().unwrap();
    // Files and the report.
    let mut lines = vec!["# lod_model\tfull_model\tbaked_model\twidth\theight\ttexels_per_unit\tgpu_bytes\tmissed_samples".to_string()];
    let mut baked_of: HashMap<(String, Variant), String> = HashMap::new();
    for (lod, v, model, level, b) in &results {
        let full = if v.1.is_empty() { v.0.clone() } else { format!("{} (alt {})", v.0, v.1) };
        let write_file = |rel: &str, bytes: &[u8]| -> Result<()> {
            let path = out.join(rel);
            std::fs::create_dir_all(path.parent().unwrap())?;
            std::fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))
        };
        let name = model.rsplit('\\').next().unwrap().trim_end_matches(".nif");
        write_file(model, &b.nif)?;
        write_file(&format!("{TEXTURE_DIR}\\{name}.dds"), &b.diffuse)?;
        write_file(&format!("{TEXTURE_DIR}\\{name}_n.dds"), &b.normal)?;
        let li = level_index(*level);
        rep.levels[li].0 += 1;
        rep.levels[li].1 += b.gpu_bytes();
        lines.push(format!("{lod}\t{full}\t{model}\t{}\t{}\t{:.3}\t{}\t{}", b.width, b.height, b.density, b.gpu_bytes(), b.misses));
        baked_of.insert((lod.clone(), v.clone()), model.clone());
    }
    rep.baked = results.len();
    for (lod, e) in &rep.failed {
        lines.push(format!("# failed {lod}: {e}"));
    }
    for lod in &cutout {
        lines.push(format!("# kept (cut-out parts) {lod}"));
    }
    // PBR full models: Community Shaders draws their snow through its PBR path, darker and
    // without the vanilla snow sparkle / rim light. The LOD draw page draws it the vanilla way,
    // so it gets the PBR albedo scale and no snow flag, baked or not.
    let dim = |sn: &mut lodforge_instances::statics::Snow| {
        sn.color = sn.color.map(|c| c * lodforge_detailbake::PBR_SCALE);
        sn.flags &= !1;
    };
    let mut pbr_cache: HashMap<String, bool> = HashMap::new();
    // Statics entries -> baked models; every snow entry -> its PBR snow scale.
    for f in &files {
        let mut s = StaticsFile::from_bytes(&std::fs::read(f)?)?;
        let mut changed = false;
        let variants: Vec<Option<Variant>> = (0..s.meshes.len()).map(|i| variant_of(full_of, &s, i, &s.meshes[i].model.to_ascii_lowercase()).ok().flatten()).collect();
        let key = (s.common.world.clone(), s.common.season.clone());
        for (i, (m, v)) in s.meshes.iter_mut().zip(variants).enumerate() {
            let model = m.model.to_ascii_lowercase();
            let baked = v.as_ref().and_then(|v| baked_of.get(&(model.clone(), v.clone())).map(|b| (b.clone(), v.clone())));
            if let Some((baked, v)) = baked {
                // A variant baked with its own full-model AltTextures already shows them; without
                // any, entry AltTextures must be the full model's own materials (else a recoloured
                // variant would lose its colours).
                let keep = !m.textures.is_empty() && v.1.is_empty() && {
                    let own = full_materials.get(&v).map(Vec::as_slice).unwrap_or(&[]);
                    !m.textures.iter().all(|(d, _)| own.contains(&material_stem(d)))
                };
                if keep {
                    rep.kept_alt += 1;
                } else {
                    m.model = baked;
                    m.textures.clear();
                    if full_pbr.contains(&v) {
                        if let Some(sn) = &mut m.snow {
                            dim(sn);
                            rep.pbr_snow += 1;
                        }
                    }
                    rep.entries += 1;
                    changed = true;
                    continue;
                }
            }
            // Not baked: the full models' textures decide.
            if m.snow.is_some() {
                let fulls = full_of.get(&(key.0.clone(), key.1.clone(), i as u16)).map(|s| s.iter().map(|(f, _)| normalize_model(f)).filter(|f| !f.contains("\\lod\\")).collect::<Vec<_>>()).unwrap_or_default();
                let pbr = fulls.iter().any(|f| *pbr_cache.entry(f.clone()).or_insert_with(|| full_model_is_pbr(p, f)));
                if pbr {
                    dim(m.snow.as_mut().unwrap());
                    rep.pbr_snow_unbaked += 1;
                    changed = true;
                }
            }
        }
        if changed {
            std::fs::write(f, s.to_bytes(write)?).with_context(|| format!("writing {}", f.display()))?;
        }
    }
    std::fs::write(dir.join(REPORT), lines.join("\r\n") + "\r\n")?;
    rep.ms = t0.elapsed().as_millis();
    Ok(rep)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alt_keys_round_trip() {
        let alt = vec![("mountaincliffslope:2".to_string(), vec![r"textures\pbr\landscape\snowrocks01linear.dds".to_string(), r"textures\pbr\landscape\snowrocks01_n.dds".to_string(), String::new()])];
        assert_eq!(parse_alt_key(&alt_key(&alt)), alt);
        assert!(parse_alt_key("").is_empty());
    }

    #[test]
    fn landscape_models_include_pgpatcher_duplicates() {
        assert!(is_landscape_model(r"meshes\landscape\rocks\rockl01.nif"));
        assert!(is_landscape_model(r"meshes\_pgpatcher_dups\3\landscape\rocks\s3drockpilem02.nif"));
        assert!(!is_landscape_model(r"meshes\_pgpatcher_dups\3\clutter\barrel.nif"));
        assert!(!is_landscape_model(r"meshes\clutter\landscape\x.nif"));
        assert_eq!(original_model(r"meshes\_pgpatcher_dups\5\landscape\mountains\mountaincliffslope.nif"), r"meshes\landscape\mountains\mountaincliffslope.nif");
        assert_eq!(original_model(r"meshes\landscape\rocks\rockl01.nif"), r"meshes\landscape\rocks\rockl01.nif");
    }

    #[test]
    fn material_stems_ignore_pbr_folder_and_linear_copies() {
        assert_eq!(material_stem(r"textures\pbr\landscape\mountains\mountainslab02linear.dds"), "mountainslab02");
        assert_eq!(material_stem(r"textures\pbr\landscape\mountains\mountainslab02.dds"), "mountainslab02");
        assert_eq!(material_stem(r"textures\landscape\mountains\MountainSlab02.dds"), "mountainslab02");
        assert_ne!(material_stem(r"textures\landscape\mountains\mountainslab02snow.dds"), "mountainslab02");
    }

    #[test]
    fn models_are_data_relative() {
        assert_eq!(normalize_model(r"Meshes\Landscape\Mountains\MountainCliff02.nif"), r"meshes\landscape\mountains\mountaincliff02.nif");
        assert_eq!(normalize_model(r"landscape\rocks\rock01.nif"), r"meshes\landscape\rocks\rock01.nif");
        assert_eq!(normalize_model(r"Data\Meshes\x.nif"), r"meshes\x.nif");
    }
}
