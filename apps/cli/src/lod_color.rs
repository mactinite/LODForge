//! `lodforge lod-color`: how bright each near-tier reference's LOD is next to its full model,
//! measured from a build's output without the game.
//!
//! The runtime draws a near-tier reference (statics `NEAR`) as its full model inside the
//! large-reference grid and as its LOD instances beyond, so the two are seen side by side as the
//! player moves. For every such reference this compares the surface colour of both:
//!
//! - full model: its shapes' diffuse textures through their UVs, times the vertex colours where
//!   the shader's `Vertex_Colors` flag is set (the engine ignores a colour array without it),
//!   times [`lodforge_detailbake::PBR_SCALE`] for `\pbr\` diffuses (the factor the build's PBR
//!   copies and bakes use for what a PBR albedo looks like through the LOD shader);
//! - LOD: its mesh in `StaticMeshes.bin` through the atlas page it is drawn from, times the
//!   pack's vertex colours (the runtime turns `Vertex_Colors` on for every page).
//!
//! Both are means over the surface area, taken in linear light. The ratio LOD / full is of those
//! means encoded back to stored (sRGB) values, the terms the build's own factors are in (a PBR
//! diffuse put on a page without `PBR_SCALE` reads x1.54); it says how much brighter (> 1) or
//! darker the LOD is; `tex` and `vc` split it
//! into the texture's share and the vertex colours' share. Lighting, snow and the reference's
//! alternate textures are not part of it (rows with alternate textures are marked).

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use anyhow::{Context, Result};

use lodforge_dds::mips::{linear_to_srgb, srgb_to_linear};
use lodforge_instances::meshpack::MeshPack;
use lodforge_instances::statics::StaticsFile;
use lodforge_objlod::mesh::ShaderSource;

use crate::profile::ProfileArgs;

const SLSF1_DECAL: u32 = 1 << 26;
const SLSF1_DYNAMIC_DECAL: u32 = 1 << 27;
const SLSF2_VERTEX_COLORS: u32 = 1 << 5;

#[derive(clap::Args)]
pub struct Args {
    #[command(flatten)]
    pub profile: ProfileArgs,
    /// Build output folder (holds `SKSE\Plugins\LODForgeRuntime` and the atlas pages).
    #[arg(long)]
    pub out: PathBuf,
    #[arg(long, default_value = "Tamriel")]
    pub world: String,
    /// Season file to read (`WIN`, `SUM`, ...); default: the world's default statics file.
    #[arg(long)]
    pub season: Option<String>,
    /// Rows listed at each end of the ranking.
    #[arg(long, default_value_t = 15)]
    pub top: usize,
    /// Rows outside 1 / tolerance .. tolerance count as off.
    #[arg(long, default_value_t = 1.15)]
    pub tolerance: f32,
    /// Every row as CSV.
    #[arg(long)]
    pub csv: Option<PathBuf>,
}

/// A decoded texture level (sRGB values as stored).
struct Tex {
    w: u32,
    h: u32,
    px: Vec<u8>,
}

impl Tex {
    /// The level of `bytes` nearest to `want` texels wide.
    fn load(bytes: &[u8], want: u32) -> Option<Tex> {
        let info = lodforge_dds::parse(bytes).ok()?;
        let mut level = 0;
        while level + 1 < info.mip_count && (info.width >> (level + 1)) >= want {
            level += 1;
        }
        let img = lodforge_dds::decode(bytes, &info, level).ok()?;
        Some(Tex { w: img.width, h: img.height, px: img.rgba })
    }

    fn at(&self, u: f32, v: f32, wrap: bool) -> [u8; 4] {
        let f = |t: f32, n: u32| -> u32 {
            let t = if wrap { t - t.floor() } else { t.clamp(0.0, 1.0) };
            ((t * n as f32) as u32).min(n - 1)
        };
        let i = ((f(v, self.h) * self.w + f(u, self.w)) * 4) as usize;
        [self.px[i], self.px[i + 1], self.px[i + 2], self.px[i + 3]]
    }
}

/// Area-weighted sums in linear light: the shown colour (texture times vertex colour), the
/// texture alone and the vertex colour alone.
#[derive(Default, Clone, Copy)]
struct Mean {
    area: f64,
    shown: f64,
    tex: f64,
    vc: f64,
    pbr_area: f64,
}

fn lum(c: [f32; 3]) -> f64 {
    f64::from(0.2126 * srgb_to_linear(c[0]) + 0.7152 * srgb_to_linear(c[1]) + 0.0722 * srgb_to_linear(c[2]))
}

impl Mean {
    /// One triangle: `sample(barycentric weights)` gives (texture rgb, vertex colour rgb), both
    /// 0..1 as stored, or `None` for a texel the alpha test drops.
    fn triangle(&mut self, p: [[f32; 3]; 3], pbr: bool, sample: impl Fn([f32; 3]) -> Option<([f32; 3], [f32; 3])>) {
        let e = |a: [f32; 3], b: [f32; 3]| [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let (a, b) = (e(p[0], p[1]), e(p[0], p[2]));
        let c = [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
        let area = f64::from((c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt()) * 0.5;
        if !(area > 0.0) {
            return;
        }
        const W: [[f32; 3]; 7] =
            [[1.0 / 3.0; 3], [0.6, 0.2, 0.2], [0.2, 0.6, 0.2], [0.2, 0.2, 0.6], [0.45, 0.45, 0.1], [0.45, 0.1, 0.45], [0.1, 0.45, 0.45]];
        let hits: Vec<([f32; 3], [f32; 3])> = W.iter().filter_map(|w| sample(*w)).collect();
        if hits.is_empty() {
            return;
        }
        let share = area * hits.len() as f64 / W.len() as f64 / hits.len() as f64;
        let scale = if pbr { lodforge_detailbake::PBR_SCALE } else { 1.0 };
        for (t, v) in hits {
            self.shown += share * lum([t[0] * v[0] * scale, t[1] * v[1] * scale, t[2] * v[2] * scale]);
            self.tex += share * lum(t.map(|x| x * scale));
            self.vc += share * lum(v);
            self.area += share;
            if pbr {
                self.pbr_area += share;
            }
        }
    }

    fn shown(&self) -> f64 {
        self.shown / self.area.max(1e-12)
    }
    fn tex(&self) -> f64 {
        self.tex / self.area.max(1e-12)
    }
    fn vc(&self) -> f64 {
        self.vc / self.area.max(1e-12)
    }
}

/// A linear-light mean as a stored (sRGB-encoded) value.
fn stored(linear: f64) -> f64 {
    f64::from(linear_to_srgb(linear as f32))
}

fn mix3(w: [f32; 3], v: [[f32; 3]; 3]) -> [f32; 3] {
    [0, 1, 2].map(|k| w[0] * v[0][k] + w[1] * v[1][k] + w[2] * v[2][k])
}

struct Row {
    /// The LOD entry has snow / ash class parts (drawn with LOD snow), its `MSNO` colour if it has one,
    /// and the full model's own directional material colour (`NEAR`).
    lod_snow_class: bool,
    lod_snow: Option<[f32; 3]>,
    full_snow: Option<[f32; 3]>,
    pos: [f32; 3],
    form: String,
    full: String,
    lod: String,
    level: u8,
    page: String,
    full_lum: f64,
    lod_lum: f64,
    ratio: f64,
    tex_ratio: f64,
    vc_full: f64,
    vc_lod: f64,
    pbr: f64,
    unflagged: bool,
    alt: bool,
}

pub fn run(a: &Args) -> Result<()> {
    let p = a.profile.open()?;
    let dir = crate::instances::out_dir(&a.out);
    let name = match &a.season {
        Some(s) => format!("{}.{}.statics.bin", a.world, s),
        None => format!("{}.statics.bin", a.world),
    };
    let statics = StaticsFile::from_bytes(&std::fs::read(dir.join(&name)).with_context(|| format!("reading {name}"))?)?;
    let pack = MeshPack::from_bytes(&std::fs::read(dir.join("StaticMeshes.bin")).context("reading StaticMeshes.bin")?)?;
    println!("{name}: {} near-tier references, {} MESH entries; StaticMeshes.bin: {} meshes on {} pages", statics.near.len(), statics.meshes.len(), pack.meshes.len(), pack.pages.len());

    // (PLUG index, form) -> the (level, MESH entry) of its records.
    let mut records: HashMap<(u16, u32), BTreeMap<u8, u16>> = HashMap::new();
    let mut positions: HashMap<(u16, u32), [f32; 3]> = HashMap::new();
    for ((level, _, _), recs) in &statics.blocks {
        for r in recs {
            records.entry((r.plugin, r.form)).or_default().insert(*level, r.mesh);
            positions.entry((r.plugin, r.form)).or_insert(r.pos);
        }
    }

    // Atlas pages: a level about 2048 wide (the page's own mips are linear-light averages).
    let mut pages: Vec<Option<Tex>> = Vec::new();
    for pg in &pack.pages {
        let path = a.out.join(&pg.albedo);
        pages.push(std::fs::read(&path).ok().and_then(|b| Tex::load(&b, 2048)));
    }
    let mut lod_cache: HashMap<u32, Option<(Mean, String)>> = HashMap::new();
    let mut lod_mean = |pm: u32| -> Option<(Mean, String)> {
        lod_cache
            .entry(pm)
            .or_insert_with(|| {
                let m = pack.meshes.get(pm as usize)?;
                let mut mean = Mean::default();
                let mut by_page: BTreeMap<u16, u32> = BTreeMap::new();
                for c in &pack.clusters[m.first_cluster as usize..(m.first_cluster + m.cluster_count) as usize] {
                    let Some(tex) = pages.get(usize::from(c.page)).and_then(|t| t.as_ref()) else { continue };
                    let thr = pack.pages[usize::from(c.page)].alpha_threshold;
                    *by_page.entry(c.page).or_default() += u32::from(c.triangles);
                    for t in 0..usize::from(c.triangles) {
                        let v = [0, 1, 2].map(|k| &pack.vertices[pack.indices[c.first_index as usize + 3 * t + k] as usize]);
                        let uv = v.map(|x| [f32::from(x.uv[0]) / 65535.0, f32::from(x.uv[1]) / 65535.0, 0.0]);
                        let col = v.map(|x| [f32::from(x.color[0]) / 255.0, f32::from(x.color[1]) / 255.0, f32::from(x.color[2]) / 255.0]);
                        mean.triangle(v.map(|x| x.pos), false, |w| {
                            let q = mix3(w, uv);
                            let t = tex.at(q[0], q[1], false);
                            (t[3] >= thr).then(|| ([f32::from(t[0]) / 255.0, f32::from(t[1]) / 255.0, f32::from(t[2]) / 255.0], mix3(w, col)))
                        });
                    }
                }
                let page = by_page.iter().max_by_key(|(_, n)| **n).map(|(pg, _)| pack.pages[usize::from(*pg)].albedo.clone()).unwrap_or_default();
                (mean.area > 0.0).then_some((mean, page))
            })
            .clone()
    };

    // Full models: (mean, a shape has a colour array the shader does not use).
    let mut tex_cache: HashMap<String, Option<Tex>> = HashMap::new();
    let mut full_cache: HashMap<String, Option<(Mean, bool)>> = HashMap::new();
    let mut full_mean = |model: &str| -> Option<(Mean, bool)> {
        if let Some(m) = full_cache.get(model) {
            return *m;
        }
        let r = (|| {
            let bytes = p.vfs.read(model).ok().flatten()?;
            let mesh = lodforge_objlod::mesh::import(model, &bytes).ok()?;
            let mut mean = Mean::default();
            let mut unflagged = false;
            for part in &mesh.parts {
                let ShaderSource::Lighting(l) = &part.shader else { continue };
                if l.flags1 & (SLSF1_DECAL | SLSF1_DYNAMIC_DECAL) != 0 || part.uvs.len() != part.positions.len() {
                    continue;
                }
                let Some(diffuse) = lodforge_detailbake::texture_path(&part.textures[0]) else { continue };
                let tex = tex_cache.entry(diffuse.clone()).or_insert_with(|| p.vfs.read(&diffuse).ok().flatten().and_then(|b| Tex::load(&b, 512)));
                let Some(tex) = tex.as_ref() else { continue };
                let colors = part.colors.as_ref().filter(|c| c.len() == part.positions.len());
                let flagged = l.flags2 & SLSF2_VERTEX_COLORS != 0;
                unflagged |= !flagged && colors.is_some_and(|c| c.iter().any(|x| x[0] < 250 || x[1] < 250 || x[2] < 250));
                let colors = colors.filter(|_| flagged);
                // The alpha test's threshold (NiAlphaProperty flags bit 9: test enabled).
                let cutoff = part.alpha.filter(|(f, _)| f & 0x200 != 0).map_or(0, |(_, t)| t);
                let pbr = diffuse.contains("\\pbr\\");
                for t in &part.triangles {
                    let i = t.map(usize::from);
                    let uv = i.map(|k| [part.uvs[k][0] * l.uv_scale[0] + l.uv_offset[0], part.uvs[k][1] * l.uv_scale[1] + l.uv_offset[1], 0.0]);
                    let col = i.map(|k| colors.map_or([1.0; 3], |c| [f32::from(c[k][0]) / 255.0, f32::from(c[k][1]) / 255.0, f32::from(c[k][2]) / 255.0]));
                    mean.triangle(i.map(|k| part.positions[k]), pbr, |w| {
                        let q = mix3(w, uv);
                        let t = tex.at(q[0], q[1], true);
                        (t[3] >= cutoff).then(|| ([f32::from(t[0]) / 255.0, f32::from(t[1]) / 255.0, f32::from(t[2]) / 255.0], mix3(w, col)))
                    });
                }
            }
            (mean.area > 0.0).then_some((mean, unflagged))
        })();
        full_cache.insert(model.to_string(), r);
        r
    };

    let mut rows: Vec<Row> = Vec::new();
    let (mut no_full, mut no_lod) = (0usize, 0usize);
    for n in &statics.near {
        let Some((full, unflagged)) = full_mean(&n.model.to_ascii_lowercase()) else {
            no_full += 1;
            continue;
        };
        let Some(levels) = records.get(&(n.plugin, n.form)) else {
            no_lod += 1;
            continue;
        };
        for (&level, &mesh) in levels {
            let Some(&pm) = statics.pack_meshes.get(usize::from(mesh)) else { continue };
            let Some((lod, page)) = lod_mean(pm) else {
                no_lod += 1;
                continue;
            };
            let entry = &statics.meshes[usize::from(mesh)];
            rows.push(Row {
                lod_snow_class: entry.classes.iter().any(|c| (1..=4).contains(c)),
                lod_snow: entry.snow.map(|s| s.color),
                full_snow: n.snow.map(|s| s.color),
                pos: positions.get(&(n.plugin, n.form)).copied().unwrap_or_default(),
                form: format!("{:04X}:{:06X}", n.plugin, n.form),
                full: n.model.clone(),
                lod: pack.meshes[pm as usize].model.clone(),
                level,
                page,
                full_lum: full.shown(),
                lod_lum: lod.shown(),
                ratio: stored(lod.shown()) / stored(full.shown()).max(1e-4),
                tex_ratio: stored(lod.tex()) / stored(full.tex()).max(1e-4),
                vc_full: full.vc(),
                vc_lod: lod.vc(),
                pbr: full.pbr_area / full.area,
                unflagged,
                alt: !n.alt.is_empty(),
            });
        }
    }
    rows.sort_by(|x, y| y.ratio.total_cmp(&x.ratio));
    println!("{} rows (reference x level); {no_full} references whose full model or textures could not be read, {no_lod} without a pack mesh", rows.len());
    if rows.is_empty() {
        return Ok(());
    }

    let pct = |v: &[f64], q: f64| v[((v.len() - 1) as f64 * q).round() as usize];
    let summary = |label: &str, pick: &dyn Fn(&Row) -> bool| {
        let mut v: Vec<f64> = rows.iter().filter(|r| pick(r)).map(|r| r.ratio).collect();
        if v.is_empty() {
            return;
        }
        v.sort_by(f64::total_cmp);
        let tol = f64::from(a.tolerance);
        let (hi, lo) = (v.iter().filter(|r| **r > tol).count(), v.iter().filter(|r| **r < 1.0 / tol).count());
        println!("  {label:<34} {:>5} rows: LOD / full p10 {:.2}, median {:.2}, p90 {:.2}; {hi} brighter than x{tol:.2}, {lo} darker than x{:.2}", v.len(), pct(&v, 0.1), pct(&v, 0.5), pct(&v, 0.9), 1.0 / tol);
    };
    println!("brightness of the LOD over the full model's, in stored values (1 = equal):");
    summary("all", &|_| true);
    for level in [4u8, 8, 16, 32] {
        summary(&format!("LOD{level}"), &|r| r.level == level);
    }
    summary("full model PBR", &|r| r.pbr > 0.5);
    summary("full model not PBR", &|r| r.pbr <= 0.5);
    summary("detail bake page", &|r| r.page.contains("bake"));
    summary("atlas page", &|r| !r.page.contains("bake"));
    summary("colour array without the VC flag", &|r| r.unflagged);
    summary("reference has alternate textures", &|r| r.alt);
    // Snow: the full model shows snow only through its base's directional material; a LOD entry with
    // snow-class parts draws LOD snow whether or not that material exists.
    let count = |f: &dyn Fn(&Row) -> bool| rows.iter().filter(|r| f(r)).count();
    println!(
        "snow: {} rows whose LOD has snow parts; {} of them where the full model has no snow material ({} of those with a snow material on the LOD entry, {} with the engine's LOD snow); {} rows where the full model has one and the LOD has no snow parts",
        count(&|r| r.lod_snow_class),
        count(&|r| r.lod_snow_class && r.full_snow.is_none()),
        count(&|r| r.lod_snow_class && r.full_snow.is_none() && r.lod_snow.is_some()),
        count(&|r| r.lod_snow_class && r.full_snow.is_none() && r.lod_snow.is_none()),
        count(&|r| !r.lod_snow_class && r.full_snow.is_some())
    );
    let mut bare: BTreeMap<String, usize> = BTreeMap::new();
    for r in rows.iter().filter(|r| r.lod_snow_class && r.full_snow.is_none()) {
        *bare.entry(format!("LOD{} {} <- {}", r.level, r.lod, r.full)).or_default() += 1;
    }
    let mut bare: Vec<_> = bare.into_iter().collect();
    bare.sort_by(|a, b| b.1.cmp(&a.1));
    for (k, n) in bare.iter().take(a.top) {
        println!("  {n:>4} {k}");
    }

    let line = |r: &Row| {
        println!(
            "  x{:.2} (tex x{:.2}, vc {:.2} -> {:.2}) LOD{:<2} full {:.3} lod {:.3} pbr {:.0}%{}{}  {} <- {}  [{}]",
            r.ratio,
            r.tex_ratio,
            r.vc_full,
            r.vc_lod,
            r.level,
            r.full_lum,
            r.lod_lum,
            r.pbr * 100.0,
            if r.unflagged { " noVCflag" } else { "" },
            if r.alt { " alt" } else { "" },
            r.lod,
            r.full,
            r.page
        )
    };
    println!("brightest LODs against their full model:");
    rows.iter().take(a.top).for_each(line);
    println!("darkest:");
    rows.iter().rev().take(a.top).for_each(line);

    if let Some(csv) = &a.csv {
        let mut s = String::from("form,x,y,z,lod_snow_parts,lod_snow_material,full_snow_material,level,ratio,tex_ratio,vc_full,vc_lod,full_lum,lod_lum,pbr_share,vc_array_without_flag,alt_textures,lod_model,full_model,page\n");
        for r in &rows {
            s += &format!("{},{:.0},{:.0},{:.0},{},{},{},{},{:.4},{:.4},{:.4},{:.4},{:.5},{:.5},{:.3},{},{},{},{},{}\n", r.form, r.pos[0], r.pos[1], r.pos[2], r.lod_snow_class, r.lod_snow.is_some(), r.full_snow.is_some(), r.level, r.ratio, r.tex_ratio, r.vc_full, r.vc_lod, r.full_lum, r.lod_lum, r.pbr, r.unflagged, r.alt, r.lod, r.full, r.page);
        }
        std::fs::write(csv, s)?;
        println!("wrote {}", csv.display());
    }
    Ok(())
}
