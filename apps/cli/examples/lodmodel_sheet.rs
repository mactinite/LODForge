//! Contact sheet of a full model and the LOD levels `lodforge_lodmodel` makes of it, for judging
//! silhouettes: rows are views (front, side, three-quarter from above, and the three-quarter view
//! again at a LOD4-like distance), columns the full model and levels 0, 1, 2. With `--open` the
//! same columns follow for the open state of an animated door (its `Open` sequence at the end).
//! Every cell of a row shares one orthographic frame, so sizes compare across columns.
//!
//! `cargo run --release --example lodmodel_sheet -- <full.nif> <out.png> [--mods <MO2 mods folder>]
//!  [--open] [--upp <units per pixel, default 1.5>] [--max-error e0,e1,e2] [--max-ratio r0,r1,r2]
//!  [--compare <level> [--shade-tol t]]`
//!
//! `--compare` replaces the level columns with one level made three ways: without the three checks below (the generator before version 9), with the
//! triangle cap giving way to the checks and glow shapes kept (`checked_cap`, `keep_glow`), and
//! with the shading check as well.
//!
//! Textures are looked up as loose files below the mods folder's sub-folders (first hit); without
//! one the models render untextured.
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use lodforge_texgen::{Basis, Item, Scene, TexData, View};

struct Opts {
    full: PathBuf,
    out: PathBuf,
    mods: Option<PathBuf>,
    open: bool,
    upp: f32,
    max_error: Option<Vec<f32>>,
    max_ratio: Option<Vec<f32>>,
    compare: Option<usize>,
    shade_tol: f32,
}

fn parse_args() -> Result<Opts> {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let mut pos = Vec::new();
    let mut o = Opts { full: PathBuf::new(), out: PathBuf::new(), mods: None, open: false, upp: 1.5, max_error: None, max_ratio: None, compare: None, shade_tol: 0.15 };
    let mut i = 0;
    let list = |s: &str| -> Result<Vec<f32>> { s.split(',').map(|v| v.trim().parse::<f32>().map_err(|e| anyhow!("{s}: {e}"))).collect() };
    while i < a.len() {
        match a[i].as_str() {
            "--mods" => {
                o.mods = Some(PathBuf::from(a.get(i + 1).context("--mods needs a folder")?));
                i += 1;
            }
            "--open" => o.open = true,
            "--compare" => {
                o.compare = Some(a.get(i + 1).context("--compare needs a level")?.parse()?);
                i += 1;
            }
            "--shade-tol" => {
                o.shade_tol = a.get(i + 1).context("--shade-tol needs a value")?.parse()?;
                i += 1;
            }
            "--upp" => {
                o.upp = a.get(i + 1).context("--upp needs a value")?.parse()?;
                i += 1;
            }
            "--max-error" => {
                o.max_error = Some(list(a.get(i + 1).context("--max-error needs values")?)?);
                i += 1;
            }
            "--max-ratio" => {
                o.max_ratio = Some(list(a.get(i + 1).context("--max-ratio needs values")?)?);
                i += 1;
            }
            s => pos.push(s.to_string()),
        }
        i += 1;
    }
    if pos.len() != 2 {
        return Err(anyhow!("usage: lodmodel_sheet <full.nif> <out.png> [--mods <dir>] [--open] [--upp n] [--max-error a,b,c] [--max-ratio a,b,c]"));
    }
    o.full = PathBuf::from(&pos[0]);
    o.out = PathBuf::from(&pos[1]);
    Ok(o)
}

/// Orthographic camera `yaw_deg` about Z, raised `elev_deg` above the horizon.
fn elevated(yaw_deg: f32, elev_deg: f32) -> Basis {
    let (s, c) = yaw_deg.to_radians().sin_cos();
    let (se, ce) = elev_deg.to_radians().sin_cos();
    let right = [-c, -s, 0.0];
    let toward = [-s * ce, c * ce, se];
    // right x up = toward for an orthonormal right-handed frame when up = toward x right.
    let up = [toward[1] * right[2] - toward[2] * right[1], toward[2] * right[0] - toward[0] * right[2], toward[0] * right[1] - toward[1] * right[0]];
    Basis { right, up, toward, slab: (0.0, 0.0), mip_bias: 0.0 }
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn save_png(path: &Path, w: u32, h: u32, rgba: &[u8]) -> Result<()> {
    let f = std::fs::File::create(path)?;
    let mut e = png::Encoder::new(std::io::BufWriter::new(f), w, h);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    e.write_header()?.write_image_data(rgba)?;
    Ok(())
}

fn main() -> Result<()> {
    let o = parse_args()?;
    let bytes = std::fs::read(&o.full).with_context(|| format!("reading {}", o.full.display()))?;
    let nif = lodforge_nif::parse(&bytes)?;
    let name = o.full.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "model".into());
    let mut params = lodforge_lodmodel::LEVELS;
    for (k, p) in params.iter_mut().enumerate() {
        if let Some(v) = o.max_error.as_ref().and_then(|v| v.get(k).or(v.last())) {
            p.max_error = *v;
        }
        if let Some(v) = o.max_ratio.as_ref().and_then(|v| v.get(k).or(v.last())) {
            p.max_ratio = *v;
        }
    }
    let mut columns: Vec<(String, lodforge_nif::Nif)> = vec![("full".into(), nif.clone())];
    let poses: Vec<(&str, Option<lodforge_lodmodel::Pose>)> = if o.open {
        match lodforge_lodmodel::open_pose(&nif) {
            Some(p) => vec![("", None), ("-open", Some(p))],
            None => {
                println!("no Open sequence in {}", o.full.display());
                vec![("", None)]
            }
        }
    } else {
        vec![("", None)]
    };
    for (suffix, pose) in &poses {
        if pose.is_some() {
            let whole = lodforge_lodmodel::LevelParams { max_error: 0.0, max_ratio: 1.0, min_triangles: usize::MAX / 8, ..params[0] };
            match lodforge_lodmodel::generate_nif_posed(&nif, &whole, &name, pose.as_ref()) {
                Ok((n, _)) => columns.push((format!("full{suffix}"), n)),
                Err(e) => println!("full{suffix}: {e:?}"),
            }
        }
        let variants: Vec<(String, lodforge_lodmodel::LevelParams)> = match o.compare {
            Some(k) => {
                let p = *params.get(k).context("--compare: level 0, 1 or 2")?;
                let p = lodforge_lodmodel::LevelParams { shade_tol: f32::INFINITY, checked_cap: false, keep_glow: false, ..p };
                vec![(format!("lod{k} before"), p), (format!("lod{k} checked cap"), lodforge_lodmodel::LevelParams { checked_cap: true, keep_glow: true, ..p }), (format!("lod{k} checked cap + shading"), lodforge_lodmodel::LevelParams { checked_cap: true, keep_glow: true, shade_tol: o.shade_tol, ..p })]
            }
            None => params.iter().enumerate().map(|(k, p)| (format!("lod{k}"), *p)).collect(),
        };
        for (k, p) in &variants {
            match lodforge_lodmodel::generate_nif_posed(&nif, p, &name, pose.as_ref()) {
                Ok((n, st)) => {
                    println!("{k}{suffix}: {} -> {} triangles in {} shapes (error {:.1}, fill {:.2}, loss {:.2})", st.source_triangles, st.triangles, st.shapes, st.error, st.fill, st.loss);
                    columns.push((format!("{k}{suffix}"), n));
                }
                Err(e) => println!("{k}{suffix}: {e:?}"),
            }
        }
    }

    // Textures: loose files below the mods folder's sub-folders.
    let mods: Vec<PathBuf> = o
        .mods
        .as_ref()
        .and_then(|m| std::fs::read_dir(m).ok())
        .map(|rd| {
            let mut v: Vec<PathBuf> = rd.flatten().filter(|e| e.file_type().is_ok_and(|t| t.is_dir())).map(|e| e.path()).collect();
            v.sort();
            v
        })
        .unwrap_or_default();
    let read = |t: &str| -> Option<Vec<u8>> {
        let rel = t.replace('\\', "/");
        mods.iter().map(|m| m.join(&rel)).find(|p| p.is_file()).and_then(|p| std::fs::read(p).ok())
    };
    let gpu = lodforge_gpu::Gpu::shared()?;
    let renderer = lodforge_texgen::Renderer::new(gpu);
    let settings = lodforge_texgen::Settings::default();
    let scenes: Vec<(String, Scene, HashMap<String, TexData>)> = columns
        .iter()
        .map(|(n, nif)| {
            let scene = Scene::from_nif(nif);
            let tex = lodforge_texgen::load_textures(&scene, |t| read(t).and_then(|b| TexData::from_dds(&b, true)));
            (n.clone(), scene, tex)
        })
        .collect();
    let positions: Vec<[f32; 3]> = scenes.iter().flat_map(|(_, s, _)| s.meshes.iter().flat_map(|m| m.positions.iter().copied())).collect();
    if positions.is_empty() {
        return Err(anyhow!("no geometry"));
    }

    // Views: name, basis, units per pixel.
    let views: Vec<(&str, Basis, f32)> = vec![("front", Basis::from_yaw(0.0), o.upp), ("side", Basis::from_yaw(90.0), o.upp), ("three-quarter", elevated(45.0, 30.0), o.upp), ("three-quarter at distance", elevated(45.0, 30.0), o.upp * 8.0)];
    let gap = 8u32;
    let mut rows: Vec<(u32, u32, Vec<Vec<u8>>)> = Vec::new();
    for (vname, basis, upp) in &views {
        let (mut x0, mut x1, mut z0, mut z1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for p in &positions {
            let x = dot(*p, basis.right);
            let z = dot(*p, basis.up);
            x0 = x0.min(x);
            x1 = x1.max(x);
            z0 = z0.min(z);
            z1 = z1.max(z);
        }
        let pad = 0.03 * (x1 - x0).max(z1 - z0).max(1.0);
        let (x0, x1, z0, z1) = (x0 - pad, x1 + pad, z0 - pad, z1 + pad);
        let width = (((x1 - x0) / upp).ceil() as u32).clamp(4, 4096);
        let height = (((z1 - z0) / upp).ceil() as u32).clamp(4, 4096);
        let view = View { yaw_deg: 0.0, x0, x1, z0, z1, width, height, basis: Some(*basis) };
        let jobs: Vec<(Item, View)> = scenes.iter().map(|(_, s, t)| (Item { scene: s, textures: t, scale: 1.0 }, view)).collect();
        let imgs: Vec<Vec<u8>> = lodforge_texgen::final_images(gpu, &renderer, &jobs, &settings, false).into_iter().map(|(img, _)| img).collect();
        println!("{vname}: {width} x {height} px at {upp} units per pixel");
        rows.push((width, height, imgs));
    }
    let ncol = columns.len() as u32;
    let sheet_w = rows.iter().map(|(w, _, _)| w).max().copied().unwrap_or(1) * ncol + gap * (ncol + 1);
    let sheet_h: u32 = rows.iter().map(|(_, h, _)| h + gap).sum::<u32>() + gap;
    let mut out = vec![0u8; (sheet_w * sheet_h * 4) as usize];
    for px in out.chunks_exact_mut(4) {
        px.copy_from_slice(&[96, 104, 112, 255]);
    }
    let cell_w = rows.iter().map(|(w, _, _)| w).max().copied().unwrap_or(1);
    let mut y0 = gap;
    for (w, h, imgs) in &rows {
        for (c, img) in imgs.iter().enumerate() {
            if img.len() < (w * h * 4) as usize {
                continue;
            }
            let x0 = gap + c as u32 * (cell_w + gap) + (cell_w - w) / 2;
            for y in 0..*h {
                for x in 0..*w {
                    let si = ((y * w + x) * 4) as usize;
                    let di = (((y0 + y) * sheet_w + x0 + x) * 4) as usize;
                    let a = u32::from(img[si + 3]);
                    for k in 0..3 {
                        out[di + k] = ((u32::from(img[si + k]) * a + u32::from(out[di + k]) * (255 - a)) / 255) as u8;
                    }
                }
            }
        }
        y0 += h + gap;
    }
    save_png(&o.out, sheet_w, sheet_h, &out)?;
    println!("columns: {}", columns.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(" | "));
    println!("rows: {}", views.iter().map(|(n, _, _)| *n).collect::<Vec<_>>().join(" | "));
    println!("wrote {}", o.out.display());
    Ok(())
}
