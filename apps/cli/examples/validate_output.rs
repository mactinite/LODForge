//! Offline output audit: bounded-memory NIF/DDS checks, asset resolution and material inventories.
//! This does not validate the game's shader interpretation or every plugin form reference.

#[path = "../src/profile.rs"]
mod profile;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use clap::Parser;
use lodforge_nif::Block;
use serde::Serialize;
use serde_json::{Value, json};

#[derive(Parser)]
struct Args {
    #[command(flatten)]
    profile: profile::ProfileArgs,
    /// Output Data directory to validate.
    #[arg(long)]
    root: PathBuf,
    /// Additional runtime assets, e.g. the reference's separate TexGen output.
    #[arg(long)]
    asset_root: Vec<PathBuf>,
    /// Optional reference Data directory for same-path, same-size non-atlas DDS comparisons.
    #[arg(long)]
    reference: Option<PathBuf>,
    #[arg(long)]
    json: PathBuf,
}

#[derive(Default, Serialize)]
struct Report {
    inputs: Value,
    files: usize,
    nif_files: usize,
    dds_files: usize,
    shapes: BTreeMap<String, usize>,
    materials: BTreeMap<String, usize>,
    diffuse: BTreeMap<String, BTreeMap<String, usize>>,
    textures: BTreeMap<String, Value>,
    plugins: BTreeMap<String, Value>,
    asset_sources: BTreeMap<String, BTreeSet<String>>,
    missing_assets: BTreeMap<String, BTreeSet<String>>,
    errors: Vec<String>,
}

fn asset_path(path: &str, prefix: &str) -> Result<String> {
    let path = path.replace('/', "\\").to_ascii_lowercase();
    let path = path.strip_prefix("data\\").unwrap_or(&path);
    ensure!(
        !path.contains(':') && !path.starts_with('\\') && !path.split('\\').any(|p| p == ".."),
        "non-relative asset path: {path}"
    );
    Ok(if path.starts_with(&format!("{prefix}\\")) {
        path.to_owned()
    } else {
        format!("{prefix}\\{path}")
    })
}

fn asset(report: &mut Report, path: &str, prefix: &str, source: &str) -> Result<()> {
    if !path.is_empty() {
        report
            .asset_sources
            .entry(asset_path(path, prefix)?)
            .or_default()
            .insert(source.to_owned());
    }
    Ok(())
}

fn files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut result = Vec::new();
    let mut pending = vec![root.to_owned()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).with_context(|| dir.display().to_string())? {
            let entry = entry?;
            let kind = entry.file_type()?;
            ensure!(
                !kind.is_symlink(),
                "symlink in output: {}",
                entry.path().display()
            );
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                result.push(entry.path());
            }
        }
    }
    result.sort();
    Ok(result)
}

fn nif(report: &mut Report, rel: &str, bytes: &[u8]) -> Result<()> {
    let nif = lodforge_nif::parse(bytes)?;
    report.nif_files += 1;
    for block in &nif.blocks {
        match block {
            Block::TextureSet(paths) => {
                for path in paths {
                    asset(report, path, "textures", rel)?;
                }
            }
            Block::EffectShader(shader) => {
                asset(report, &shader.source_texture, "textures", rel)?;
                asset(report, &shader.greyscale_texture, "textures", rel)?;
            }
            Block::TriShape(shape) => {
                let count = shape.positions.len();
                ensure!(
                    shape
                        .triangles
                        .iter()
                        .flatten()
                        .all(|&i| usize::from(i) < count),
                    "out-of-range triangle index"
                );
                ensure!(
                    shape.positions.iter().flatten().all(|v| v.is_finite())
                        && shape.uvs.iter().flatten().all(|v| v.is_finite())
                        && shape.normals.iter().flatten().all(|v| v.is_finite()),
                    "non-finite vertex, UV or normal"
                );
                ensure!(
                    (shape.uvs.is_empty() || shape.uvs.len() == count)
                        && (shape.normals.is_empty() || shape.normals.len() == count),
                    "vertex attribute count mismatch"
                );
                let name = shape.av.net.name.as_deref().unwrap_or("");
                let kind = name.trim_end_matches(|c: char| c.is_ascii_digit() || c == ':');
                let filename = rel.rsplit('\\').next().context("missing filename")?;
                let level = if rel.ends_with(".bto") {
                    filename.split('.').nth(1).context("missing BTO level")?
                } else {
                    "other"
                };
                let group = format!(
                    "{level}|{}",
                    if kind.starts_with("grass") {
                        "grass"
                    } else {
                        "objects"
                    }
                );
                *report.shapes.entry(format!("{group}|{kind}")).or_default() += 1;
                let (shader_key, diffuse) = match nif.get(shape.shader_property) {
                    Some(Block::LightingShader(shader)) => {
                        let paths = nif.shape_textures(shape);
                        let diffuse = paths.first().map(String::as_str).unwrap_or("");
                        (
                            format!(
                                "lighting:{}:{:08x}:{:08x}:clamp{}:alpha{}:vc{}",
                                shader.shader_type,
                                shader.flags1,
                                shader.flags2,
                                shader.texture_clamp_mode,
                                shape.alpha_property.is_some(),
                                !shape.colors.is_empty()
                            ),
                            diffuse.to_owned(),
                        )
                    }
                    Some(Block::EffectShader(shader)) => (
                        format!("effect:{:08x}:{:08x}", shader.flags1, shader.flags2),
                        shader.source_texture.clone(),
                    ),
                    None => ("none".into(), String::new()),
                    Some(_) => ("other".into(), String::new()),
                };
                *report
                    .materials
                    .entry(format!("{group}|{shader_key}"))
                    .or_default() += 1;
                if !diffuse.is_empty() {
                    *report
                        .diffuse
                        .entry(group)
                        .or_default()
                        .entry(asset_path(&diffuse, "textures")?)
                        .or_default() += 1;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn is_atlas(rel: &str) -> bool {
    rel.starts_with("textures\\dyndolod\\lod\\dyndolod_")
        || (rel.starts_with("textures\\terrain\\") && rel.contains("\\trees\\"))
}

fn mip_metrics(rgba: &[u8]) -> Value {
    let count = rgba.len() / 4;
    let opaque = rgba.chunks_exact(4).filter(|p| p[3] >= 128).count();
    let transparent = count - opaque;
    let mut rgb = [0u64; 3];
    for pixel in rgba.chunks_exact(4).filter(|p| p[3] < 128) {
        for k in 0..3 {
            rgb[k] += u64::from(pixel[k]);
        }
    }
    json!({
        "pixels": count, "alpha_ge_128": opaque,
        "coverage": opaque as f64 / count.max(1) as f64,
        "transparent_mean_rgb": rgb.map(|v| v as f64 / transparent.max(1) as f64)
    })
}

fn dds(report: &mut Report, rel: &str, bytes: &[u8], reference: Option<&Path>) -> Result<()> {
    let info = lodforge_dds::parse(bytes)?;
    ensure!(
        info.width > 0 && info.height > 0 && info.mip_count <= 32,
        "invalid DDS dimensions/mips"
    );
    let mips = info.mips();
    let end = mips.last().map(|m| m.0 + m.1).context("no mip data")?;
    let expected = info.data_offset + (end - info.data_offset) * info.layers as usize;
    ensure!(
        bytes.len() == expected,
        "DDS payload length {} != {expected}",
        bytes.len()
    );
    let mut data = json!({
        "width": info.width, "height": info.height, "format": format!("{:?}", info.format),
        "srgb": info.srgb, "mips": info.mip_count, "bytes": bytes.len()
    });
    if is_atlas(rel) {
        let mut metrics = Vec::new();
        for level in 0..info.mip_count {
            let image = lodforge_dds::decode(bytes, &info, level)?;
            metrics.push(mip_metrics(&image.rgba));
        }
        data["mip_metrics"] = json!(metrics);
    } else if let Some(root) = reference {
        let path = root.join(rel);
        if path.is_file() {
            let golden = std::fs::read(&path)?;
            let gi = lodforge_dds::parse(&golden)?;
            if (gi.width, gi.height) == (info.width, info.height) {
                let ours = lodforge_dds::decode(bytes, &info, 0)?;
                let theirs = lodforge_dds::decode(&golden, &gi, 0)?;
                let mut squared = 0u64;
                let mut alpha_equal = 0usize;
                for (a, b) in ours.rgba.chunks_exact(4).zip(theirs.rgba.chunks_exact(4)) {
                    for k in 0..3 {
                        let d = i32::from(a[k]) - i32::from(b[k]);
                        squared += (d * d) as u64;
                    }
                    alpha_equal += usize::from((a[3] >= 128) == (b[3] >= 128));
                }
                let mse = squared as f64 / (ours.rgba.len() / 4 * 3) as f64;
                data["top_rgb_psnr_db"] = if mse == 0.0 {
                    Value::Null
                } else {
                    json!(10.0 * (65025.0 / mse).log10())
                };
                data["top_rgb_exact"] = json!(mse == 0.0);
                data["top_alpha_agreement"] =
                    json!(alpha_equal as f64 / (ours.rgba.len() / 4) as f64);
            } else {
                data["reference_dimensions"] = json!([gi.width, gi.height]);
            }
        }
    }
    report.textures.insert(rel.to_owned(), data);
    report.dds_files += 1;
    Ok(())
}

fn plugin(report: &mut Report, path: &Path, rel: &str, vfs: &lodforge_archive::Vfs) -> Result<()> {
    let plugin = lodforge_esp::Plugin::open(path)?;
    for master in &plugin.header.masters {
        ensure!(vfs.exists(master), "missing plugin master {master}");
    }
    let mut counts = BTreeMap::<String, usize>::new();
    let mut stat_dnam = BTreeMap::<String, Vec<u8>>::new();
    fn walk(
        entries: lodforge_esp::Entries<'_>,
        report: &mut Report,
        rel: &str,
        counts: &mut BTreeMap<String, usize>,
        stat_dnam: &mut BTreeMap<String, Vec<u8>>,
    ) -> Result<()> {
        use lodforge_esp::Entry;
        for entry in entries {
            match entry? {
                Entry::Group(group) => walk(group.entries(), report, rel, counts, stat_dnam)?,
                Entry::Record(record) => {
                    *counts.entry(record.sig.to_string()).or_default() += 1;
                    let data = record.data()?;
                    let fields: Vec<_> = lodforge_esp::fields(&data).collect();
                    let edid = fields
                        .iter()
                        .find(|f| &f.sig.0 == b"EDID")
                        .map(|f| f.zstring())
                        .unwrap_or_default();
                    for field in fields {
                        if &field.sig.0 == b"MODL" {
                            asset(report, &field.zstring(), "meshes", rel)?;
                        } else if &record.sig.0 == b"TXST" && field.sig.0.starts_with(b"TX0") {
                            asset(report, &field.zstring(), "textures", rel)?;
                        } else if &record.sig.0 == b"STAT" && &field.sig.0 == b"DNAM" {
                            stat_dnam.insert(edid.clone(), field.data.to_vec());
                        }
                    }
                }
            }
        }
        Ok(())
    }
    walk(
        plugin.top_entries(),
        report,
        rel,
        &mut counts,
        &mut stat_dnam,
    )?;
    report.plugins.insert(
        rel.to_owned(),
        json!({"masters": plugin.header.masters, "records": counts, "stat_dnam": stat_dnam}),
    );
    Ok(())
}

fn main() -> Result<()> {
    let mut args = Args::parse();
    ensure!(args.root.is_dir(), "output directory does not exist");
    args.profile.exclude_mod.extend(profile::generated_lod_mods(
        &args.profile.instance,
        &args.profile.profile,
    ));
    let mut profile = args.profile.open()?;
    if !profile.missing_archives.is_empty() {
        eprintln!(
            "warning: configured input archives not found: {}",
            profile.missing_archives.join(", ")
        );
    }
    for root in &args.asset_root {
        ensure!(
            root.is_dir(),
            "asset root does not exist: {}",
            root.display()
        );
        profile.vfs.add_loose_root(root)?;
    }
    profile.vfs.add_loose_root(&args.root)?;
    let files = files(&args.root)?;
    let mut report = Report {
        inputs: json!({
            "root": args.root,
            "asset_roots": args.asset_root,
            "instance": args.profile.instance,
            "profile": args.profile.profile,
            "excluded_mods": args.profile.exclude_mod,
            "plugin_count": profile.plugin_names.len(),
            "loaded_plugins": profile.load_order.plugins.len(),
            "missing_archives": profile.missing_archives,
            "game_dir": profile.game_dir,
            "profile_timings_ms": profile.timings_ms
        }),
        files: files.len(),
        ..Report::default()
    };
    for path in files {
        let rel = path
            .strip_prefix(&args.root)?
            .to_string_lossy()
            .replace('/', "\\")
            .to_ascii_lowercase();
        let result = match path
            .extension()
            .and_then(|s| s.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref()
        {
            Some("nif" | "bto" | "btr") => std::fs::read(&path)
                .context("reading NIF")
                .and_then(|b| nif(&mut report, &rel, &b)),
            Some("dds") => std::fs::read(&path)
                .context("reading DDS")
                .and_then(|b| dds(&mut report, &rel, &b, args.reference.as_deref())),
            Some("esm" | "esp") => plugin(&mut report, &path, &rel, &profile.vfs),
            _ => Ok(()),
        };
        if let Err(error) = result {
            eprintln!("{rel}: {error:#}");
            report.errors.push(format!("{rel}: {error:#}"));
        }
    }
    for (path, sources) in &report.asset_sources {
        if !profile.vfs.exists(path) {
            report.missing_assets.insert(path.clone(), sources.clone());
        }
    }
    std::fs::write(&args.json, serde_json::to_vec_pretty(&report)?)?;
    println!(
        "{} files; {} NIFs, {} DDS; {} unique assets, {} missing; {} structural errors",
        report.files,
        report.nif_files,
        report.dds_files,
        report.asset_sources.len(),
        report.missing_assets.len(),
        report.errors.len()
    );
    if !report.errors.is_empty() || !report.missing_assets.is_empty() {
        bail!(
            "output validation failed; details in {}",
            args.json.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn asset_paths_are_relative_and_normalized() {
        assert_eq!(
            asset_path(r"Data\Textures\PBR\Rock.dds", "textures").unwrap(),
            r"textures\pbr\rock.dds"
        );
        assert_eq!(
            asset_path(r"LOD\Tree.nif", "meshes").unwrap(),
            r"meshes\lod\tree.nif"
        );
        assert!(asset_path(r"..\outside.dds", "textures").is_err());
        assert!(asset_path(r"D:\outside.dds", "textures").is_err());
    }

    #[test]
    fn alpha_metrics_use_the_runtime_threshold() {
        let metrics = mip_metrics(&[10, 20, 30, 127, 100, 110, 120, 128]);
        assert_eq!(metrics["coverage"], 0.5);
        assert_eq!(metrics["transparent_mean_rgb"], json!([10.0, 20.0, 30.0]));
    }

    #[test]
    fn truncated_dds_payload_fails_validation() {
        let mut bytes = lodforge_dds::write_dxt1(4, 4, &[vec![0; 8]]);
        let mut report = Report::default();
        dds(&mut report, "textures\\test.dds", &bytes, None).unwrap();
        bytes.pop();
        assert!(dds(&mut report, "textures\\test.dds", &bytes, None).is_err());
    }
}
