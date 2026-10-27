//! The `textures\pbr\...\<name>linear.dds` copies of PBR diffuse textures that DynDOLOD's
//! AltTextures point LOD shapes at (`lodforge_dyndolod::sidefiles::linear_texture_name`). LOD
//! uses the vanilla lighting shader, which shows a PBR albedo too bright.
//!
//! DynDOLOD makes them in `CreateNonAlphaImage` (docs/re/gap-audit-20261019.md P2): half size
//! (`FullTextureSizeDivider`), BC7 UNORM with a full mip chain, its PBR scale 0.65 on RGB. The
//! alpha handling is fitted to its output (E2E run 2026-10-18; docs/baseline.md "AltTextures and
//! VC side files"): RGB darkened towards black where the diffuse alpha is low, alpha sharpened
//! around 88. An opaque source is just scaled; a source with alpha ~0 (`snowrocks01`) comes out
//! black and transparent, as DynDOLOD's does. The PBR scale requires the source's RMAOS map:
//! TexGen no-alpha copies have no such companion and must not be darkened.

use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Context, Result};
use lodforge_archive::Vfs;
use lodforge_gpu::Gpu;

/// RGB factor of an opaque texel: DynDOLOD's hardcoded PBR scale (TexGen's `StitchPBRScale`).
pub const SCALE: f32 = 0.65;

fn logistic(x: f32, center: f32, width: f32) -> f32 {
    1.0 / (1.0 + (-(x - center) / width).exp())
}

/// One texel of the copy.
pub fn texel(p: [u8; 4], has_pbr: bool) -> [u8; 4] {
    let a = f32::from(p[3]);
    let weight = (1.1 * a / 255.0).max(logistic(a, 83.0, 7.0)).min(1.0);
    let scale = if has_pbr { SCALE } else { 1.0 };
    let c = |v: u8| (f32::from(v) * scale * weight).round().clamp(0.0, 255.0) as u8;
    [
        c(p[0]),
        c(p[1]),
        c(p[2]),
        (255.0 * logistic(a, 88.0, 5.0)).round() as u8,
    ]
}

/// One texel of the copy of a PBR landscape texture: its diffuse alpha is a height / blend mask
/// for terrain height blending, not opacity, so the colour is only scaled and the copy is
/// opaque. Darkening by that alpha (as [`texel`] does) printed the height map onto the LOD as
/// black blotches (Vanaheimr's coastbeach01 on the Solitude coast rocks).
pub fn landscape_texel(p: [u8; 4], has_pbr: bool) -> [u8; 4] {
    let scale = if has_pbr { SCALE } else { 1.0 };
    let c = |v: u8| (f32::from(v) * scale).round().clamp(0.0, 255.0) as u8;
    [c(p[0]), c(p[1]), c(p[2]), 255]
}

/// Whether a PBR texture path is a landscape texture (alpha = height, not opacity).
pub fn is_landscape(stem: &str) -> bool {
    stem.to_ascii_lowercase().starts_with(r"textures\pbr\landscape\")
}

/// Writes every requested `...linear.dds` from its `....dds` source into `out`.
/// Missing or invalid requested sources are errors, not successful partial output.
pub fn write(vfs: &Vfs, gpu: &Gpu, names: &BTreeSet<String>, out: &Path) -> Result<usize> {
    let mut n = 0;
    for name in names {
        let stem = name
            .strip_suffix("linear.dds")
            .with_context(|| format!("invalid linear texture name: {name}"))?;
        let src = format!("{stem}.dds");
        let has_pbr =
            stem.starts_with("textures\\pbr\\") && vfs.exists(&format!("{stem}_rmaos.dds"));
        let bytes = vfs
            .read(&src)?
            .with_context(|| format!("missing source {src} for {name}"))?;
        let info =
            lodforge_dds::parse(&bytes).with_context(|| format!("parsing {src} for {name}"))?;
        // Half size: the source's second mip (the first for a texture without mips).
        let level = u32::from(info.mips().len() > 1);
        let img = lodforge_dds::decode(&bytes, &info, level)
            .with_context(|| format!("decoding {src} for {name}"))?;
        let landscape = is_landscape(stem);
        let rgba: Vec<u8> = img
            .rgba
            .chunks_exact(4)
            .flat_map(|p| {
                let p = [p[0], p[1], p[2], p[3]];
                if landscape { landscape_texel(p, has_pbr) } else { texel(p, has_pbr) }
            })
            .collect();
        let levels = crate::object_atlas::mip_chain(img.width, img.height, rgba);
        let dds = crate::object_atlas::encode(gpu, &levels)?;
        let path = out.join(name.replace('\\', "/"));
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&path, dds)?;
        n += 1;
    }
    Ok(n)
}

/// The `...linear.dds` textures named in AltTextures text.
pub fn names_in(alt_textures: &str) -> BTreeSet<String> {
    alt_textures
        .split(['\t', ',', '=', '\r', '\n'])
        .filter(|f| f.ends_with("linear.dds"))
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texels() {
        // Opaque: scaled.
        assert_eq!(texel([200, 100, 0, 255], true), [130, 65, 0, 255]);
        // Transparent: black.
        assert_eq!(texel([200, 200, 200, 0], true), [0, 0, 0, 0]);
        let names = names_in(
            "00013304\t0=A:0=textures\\pbr\\a\\blinear.dds,textures\\pbr\\a\\b_n.dds,,\t1=C=textures\\lod\\clod.dds,\r\n",
        );
        assert_eq!(
            names.into_iter().collect::<Vec<_>>(),
            vec!["textures\\pbr\\a\\blinear.dds".to_string()]
        );
    }

    #[test]
    fn landscape_alpha_is_height_not_opacity() {
        assert!(is_landscape(r"textures\pbr\landscape\coastbeach01"));
        assert!(is_landscape(r"Textures\PBR\Landscape\mountains\mountainslab02"));
        assert!(!is_landscape(r"textures\pbr\architecture\whiterun\wrroof01"));
        // Low height alpha keeps the colour (scaled) and becomes opaque.
        assert_eq!(landscape_texel([200, 100, 0, 20], true), [130, 65, 0, 255]);
        assert_eq!(landscape_texel([200, 200, 200, 0], false), [200, 200, 200, 255]);
    }

    #[test]
    fn non_pbr_copies_preserve_opaque_color() {
        for v in 0..=255 {
            assert_eq!(texel([v, v, v, 255], false), [v, v, v, 255]);
        }
        assert_eq!(texel([213, 224, 238, 255], false), [213, 224, 238, 255]);
    }

    #[test]
    fn material_detection_does_not_change_alpha() {
        for a in 0..=255 {
            assert_eq!(
                texel([200, 100, 0, a], false)[3],
                texel([200, 100, 0, a], true)[3]
            );
        }
        assert_eq!(texel([200, 100, 0, 0], false), [0, 0, 0, 0]);
    }

    #[test]
    #[ignore = "requires GPU and LODFORGE_LINEAR_INPUTS, LODFORGE_LINEAR_REFERENCE, LODFORGE_LINEAR_OUT"]
    fn reference_textures() -> Result<()> {
        let roots = std::env::split_paths(
            &std::env::var_os("LODFORGE_LINEAR_INPUTS").context("LODFORGE_LINEAR_INPUTS")?,
        )
        .collect::<Vec<_>>();
        let reference = std::path::PathBuf::from(
            std::env::var_os("LODFORGE_LINEAR_REFERENCE").context("LODFORGE_LINEAR_REFERENCE")?,
        );
        let out = std::path::PathBuf::from(
            std::env::var_os("LODFORGE_LINEAR_OUT").context("LODFORGE_LINEAR_OUT")?,
        );
        anyhow::ensure!(!out.exists(), "fixture output must be a new directory");
        let mut dirs = vec![reference.clone()];
        let mut names = BTreeSet::new();
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(dir)? {
                let entry = entry?;
                if entry.file_type()?.is_dir() {
                    dirs.push(entry.path());
                } else if entry.file_name().to_string_lossy().ends_with("linear.dds") {
                    names.insert(
                        entry
                            .path()
                            .strip_prefix(&reference)?
                            .to_string_lossy()
                            .replace('/', "\\")
                            .to_ascii_lowercase(),
                    );
                }
            }
        }
        anyhow::ensure!(!names.is_empty(), "no reference linear textures");
        let vfs = Vfs::from_roots(roots)?;
        let gpu = Gpu::shared()?;
        assert_eq!(write(&vfs, &gpu, &names, &out)?, names.len());
        for name in names {
            let bytes = std::fs::read(out.join(&name))?;
            let golden = std::fs::read(reference.join(&name))?;
            let info = lodforge_dds::parse(&bytes)?;
            let gi = lodforge_dds::parse(&golden)?;
            assert_eq!(
                (
                    info.width,
                    info.height,
                    info.mip_count,
                    info.format,
                    info.srgb
                ),
                (gi.width, gi.height, gi.mip_count, gi.format, gi.srgb),
                "{name}"
            );
            let a = lodforge_dds::decode(&bytes, &info, 0)?;
            let b = lodforge_dds::decode(&golden, &gi, 0)?;
            let squared: f64 = a
                .rgba
                .chunks_exact(4)
                .zip(b.rgba.chunks_exact(4))
                .map(|(a, b)| {
                    (0..3)
                        .map(|k| (f64::from(a[k]) - f64::from(b[k])).powi(2))
                        .sum::<f64>()
                })
                .sum();
            let mse = squared / f64::from(info.width * info.height * 3);
            let psnr = 10.0 * (255.0f64.powi(2) / mse).log10();
            println!("{name}: RGB PSNR {psnr:.3} dB");
            assert!(psnr >= 30.0, "{name}: {psnr}");
            if name.ends_with("snow01noalphalinear.dds") {
                assert!(psnr >= 45.0, "snow no-alpha regression: {psnr}");
            }
        }
        Ok(())
    }
}
