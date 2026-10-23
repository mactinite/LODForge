//! TexGen's rendered object-LOD textures (docs/re/dyndolod-spec.md §2.1 "Rendered object
//! textures", §3.3): every `Render\Skyrim\Objects\**\*.nif` under DynDOLOD's install is a
//! `TexGen Plane` quad plus model pieces laid out in front of it. TexGen draws the pieces with an
//! orthographic front view framed on the plane, unlit, and saves diffuse + `_n` to the paths the
//! plane's texture set names (`textures\lod\wrwallslod01.dds`), with PBR twins when the pieces
//! use PBR textures.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use lodforge_nif::Block;

/// One render NIF.
#[derive(Debug, Clone)]
pub struct RenderObject {
    pub nif: PathBuf,
    /// Output diffuse (Data-relative, lower case).
    pub out: String,
    /// The plane's world-space extent: x0, x1, z0, z1.
    pub plane: [f32; 4],
    /// The plane shape's own scale (multiplies the base size).
    pub scale: f32,
    /// A NiAlphaProperty on the plane: the output keeps alpha.
    pub keep_alpha: bool,
}

/// Lower case, backslashes, from `textures\` on.
fn tex_path(t: &str) -> String {
    let t = t.trim().replace('/', "\\").to_ascii_lowercase();
    match t.find("textures\\") {
        Some(p) => t[p..].to_string(),
        None => format!("textures\\{t}"),
    }
}

/// Reads the plane of a render NIF (the first shape named `TexGen Plane`).
pub fn read_object(path: &Path) -> Option<RenderObject> {
    let bytes = std::fs::read(path).ok()?;
    let nif = lodforge_nif::parse(&bytes).ok()?;
    let (idx, shape) = nif.blocks.iter().enumerate().find_map(|(i, b)| match b {
        Block::TriShape(t) if t.av.net.name.as_deref().is_some_and(|n| n.to_ascii_lowercase().contains("texgen plane")) => Some((i, t)),
        _ => None,
    })?;
    let out = tex_path(nif.shape_textures(shape).first()?);
    // World transform of the plane: walk parents from the root.
    let xf = world_transform(&nif, idx)?;
    let pts: Vec<[f32; 3]> = shape.positions.iter().map(|p| xf.apply(*p)).collect();
    let fold = |k: usize, f: fn(f32, f32) -> f32, init: f32| pts.iter().map(|p| p[k]).fold(init, f);
    let plane = [fold(0, f32::min, f32::MAX), fold(0, f32::max, f32::MIN), fold(2, f32::min, f32::MAX), fold(2, f32::max, f32::MIN)];
    let keep_alpha = matches!(nif.get(shape.alpha_property), Some(Block::Alpha { .. }));
    Some(RenderObject { nif: path.to_path_buf(), out, plane, scale: shape.av.transform.scale, keep_alpha })
}

#[derive(Clone, Copy)]
struct Xf {
    rot: [[f32; 3]; 3],
    trans: [f32; 3],
    scale: f32,
}

impl Xf {
    fn apply(&self, v: [f32; 3]) -> [f32; 3] {
        let r = self.rot;
        let s = self.scale;
        [0, 1, 2].map(|i| s * (r[i][0] * v[0] + r[i][1] * v[1] + r[i][2] * v[2]) + self.trans[i])
    }
    fn then(&self, c: &lodforge_nif::Transform) -> Xf {
        let r = self.rot;
        let mut rot = [[0.0; 3]; 3];
        for i in 0..3 {
            for j in 0..3 {
                rot[i][j] = r[i][0] * c.rotation[0][j] + r[i][1] * c.rotation[1][j] + r[i][2] * c.rotation[2][j];
            }
        }
        let t = c.translation;
        let tr = [0, 1, 2].map(|i| self.scale * (r[i][0] * t[0] + r[i][1] * t[1] + r[i][2] * t[2]) + self.trans[i]);
        Xf { rot, trans: tr, scale: self.scale * c.scale }
    }
}

/// Transform of block `idx` into the root's space (the root's own transform is ignored, as in
/// TexGen's scene loader).
fn world_transform(nif: &lodforge_nif::Nif, idx: usize) -> Option<Xf> {
    let mut parent: HashMap<usize, usize> = HashMap::new();
    for (i, b) in nif.blocks.iter().enumerate() {
        if let Block::Node(n) = b {
            for c in n.children.iter().flatten() {
                parent.insert(*c as usize, i);
            }
        }
    }
    let mut chain = vec![idx];
    let mut cur = idx;
    while let Some(&p) = parent.get(&cur) {
        chain.push(p);
        cur = p;
    }
    let mut xf = Xf { rot: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], trans: [0.0; 3], scale: 1.0 };
    for &b in chain.iter().rev().skip(1) {
        let av = match nif.blocks.get(b)? {
            Block::Node(n) => &n.av,
            Block::TriShape(t) => &t.av,
            _ => return None,
        };
        // The plane's own scale both scales it and multiplies the output size.
        xf = xf.then(&av.transform);
    }
    Some(xf)
}

/// Every render NIF under `<dyndolod>\Edit Scripts\DynDOLOD\Render\Skyrim\Objects`.
pub fn discover(dyndolod: &Path) -> Vec<RenderObject> {
    let root = dyndolod.join(r"Edit Scripts\DynDOLOD\Render\Skyrim\Objects");
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("nif")) {
                if let Some(o) = read_object(&p) {
                    out.push(o);
                }
            }
        }
    }
    out.sort_by(|a, b| a.out.cmp(&b.out));
    out
}

/// Render NIFs below `Objects\lod` and `Objects\dlcNN` are rendered; the mod folders (`enderal`,
/// `smim`, `hpp`) are not, even when their textures exist (golden: smim_fencewoven_lod and
/// enderal\rtbuildingsenderallod02 are missing from TexGen's output and its candidate list).
/// TexGen's actual condition is not known; LOD meshes in the load order name almost none of the
/// rendered textures, so it is not "used by a LOD model".
pub fn base_game_folder(path: &str) -> bool {
    let top = path.strip_prefix("textures\\").and_then(|r| r.split('\\').next()).unwrap_or("");
    top == "lod" || (top.len() > 3 && top.starts_with("dlc") && top[3..].chars().all(|c| c.is_ascii_digit()))
}

/// TexGen's rendered PBR twin needs a name the `lod` → `pbr_lod` rule applies to ("Could not
/// process PBR ... dlc01icesnow.nif" in the golden debug log).
pub fn pbr_name_ok(path: &str) -> bool {
    let stem = path.rsplit('\\').next().unwrap_or(path).trim_end_matches(".dds");
    stem.trim_end_matches(|c: char| c.is_ascii_digit()).ends_with("lod")
}

/// `RenderObjectsSize`: the base size the plane scale multiplies.
pub const BASE_SIZE: u32 = 256;

/// Output size: the plane's long side is `BASE_SIZE * scale`, the short side the next power of two
/// of the plane's aspect (golden: seruinswindowslod 100.9 x 403.4 → 64 x 256, sbridgelod
/// 3125 x 1088 at scale 2 → 512 x 256, whwallwulfharthlod 1014 x 2390 → 128 x 256).
pub fn output_size(o: &RenderObject) -> (u32, u32) {
    let long = ((BASE_SIZE as f32 * o.scale).round() as u32).max(4);
    let (w, h) = (o.plane[1] - o.plane[0], o.plane[3] - o.plane[2]);
    let short = |r: f32| ((long as f32 * r).round().max(1.0) as u32).next_power_of_two().min(long);
    if w >= h { (long, short(h / w.max(1e-3))) } else { (short(w / h.max(1e-3)), long) }
}

/// Output path from the NIF's place below `Render\Skyrim\Objects` (`dlc02\lod\x.nif` →
/// `textures\dlc02\lod\x.dds`); the plane's texture set is not always the output name
/// (dlc2dweruins01_lod's plane names textures\lod\dweruins01_lod.dds).
pub fn output_path(dyndolod: &Path, nif: &Path) -> Option<String> {
    let root = dyndolod.join(r"Edit Scripts\DynDOLOD\Render\Skyrim\Objects");
    let rel = nif.strip_prefix(&root).ok()?.with_extension("dds");
    Some(format!("textures\\{}", rel.to_string_lossy().replace('/', "\\").to_ascii_lowercase()))
}

/// One rendered texture (level 0), diffuse and `_n`.
pub struct Rendered {
    pub path: String,
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
    pub normal: Vec<u8>,
}

/// TexGen's `RenderObjects*` lighting: direct 0, ambient 1.0 (unlit albedo), `RenderPBRScale`.
pub const LIGHT: lodforge_texgen::Light = lodforge_texgen::Light { direct: 0.0, ambient: 1.0, dir: [0.0, -1.0, 0.0], pbr_scale: 0.65 };

/// Renders every render NIF whose pieces' textures are all in the load order (TexGen leaves out
/// the Enderal, SMIM and HPP ones here), plus a PBR twin when some piece has a `textures\pbr\`
/// diffuse and `materials_pbr` is on (`--materials pbr`; off, no `textures\pbr\` path is looked
/// up and no twin is rendered). `yaw` is the view (180: camera on -Y, image right = +X).
pub fn render_all(p: &crate::profile::Profile, gpu: &lodforge_gpu::Gpu, renderer: &lodforge_texgen::Renderer, dyndolod: &Path, filter: Option<&str>, yaw: f32, materials_pbr: bool) -> anyhow::Result<(Vec<Rendered>, Vec<String>)> {
    use lodforge_texgen::stitch::{pbr_dest, pbr_path};
    let mut out = Vec::new();
    let mut skipped = Vec::new();
    for o in discover(dyndolod) {
        let Some(path) = output_path(dyndolod, &o.nif) else { continue };
        if filter.is_some_and(|f| !path.contains(&f.to_ascii_lowercase())) {
            continue;
        }
        if !base_game_folder(&path) {
            skipped.push(format!("{path}: not a base-game folder"));
            continue;
        }
        let bytes = std::fs::read(&o.nif)?;
        let nif = match lodforge_nif::parse_all_geometry(&bytes) {
            Ok(n) => n,
            Err(e) => {
                skipped.push(format!("{path}: {e}"));
                continue;
            }
        };
        let mut scene = lodforge_texgen::Scene::from_nif(&nif);
        // The plane carries the output texture; it is not drawn.
        scene.meshes.retain(|m| m.diffuse != o.out);
        lodforge_texgen::resolve_textures(&mut scene, |t| p.vfs.exists(t));
        if let Some(m) = scene.meshes.iter().find(|m| !m.diffuse.is_empty() && !p.vfs.exists(&m.diffuse)) {
            skipped.push(format!("{path}: missing {}", m.diffuse));
            continue;
        }
        let (w, h) = output_size(&o);
        let view = lodforge_texgen::View { yaw_deg: yaw, x0: 0.0, x1: 0.0, z0: o.plane[2], z1: o.plane[3], width: w, height: h };
        let (a, b) = (lodforge_texgen::render::view_x([o.plane[0], 0.0, 0.0], yaw), lodforge_texgen::render::view_x([o.plane[1], 0.0, 0.0], yaw));
        let view = lodforge_texgen::View { x0: a.min(b), x1: a.max(b), ..view };
        let has_pbr = materials_pbr && pbr_name_ok(&path) && scene.meshes.iter().any(|m| !m.diffuse.is_empty() && p.vfs.exists(&pbr_path(&m.diffuse)));
        for pbr in [false, true] {
            if pbr && !has_pbr {
                continue;
            }
            let mut sc = scene.clone();
            if pbr {
                for m in &mut sc.meshes {
                    // `_p` is a parallax map in PBR naming: TexGen takes the plain `<stem>.dds`
                    // diffuse and the PBR `<stem>_n.dds` (golden dlc01icewallpbr_lod:
                    // IceWall01_p.dds -> IceWall01.dds + pbr\...\IceWall01_n.dds).
                    if let Some(stem) = m.diffuse.strip_suffix("_p.dds") {
                        let (d, n) = (format!("{stem}.dds"), pbr_path(&format!("{stem}_n.dds")));
                        if p.vfs.exists(&pbr_path(&m.diffuse)) && p.vfs.exists(&d) {
                            m.diffuse = d;
                            if p.vfs.exists(&n) {
                                m.normal = n;
                            }
                            continue;
                        }
                    }
                    let (d, n) = (pbr_path(&m.diffuse), pbr_path(&m.normal));
                    if p.vfs.exists(&d) {
                        m.diffuse = d;
                        if p.vfs.exists(&n) {
                            m.normal = n;
                        }
                    }
                }
            }
            let tex = lodforge_texgen::load_textures(&sc, |t| crate::texgen_billboards::load_texture(gpu, &p.vfs, t));
            let jobs = [
                lodforge_texgen::Job { scene: &sc, textures: &tex, view, light: LIGHT, pass: lodforge_texgen::Pass::Diffuse },
                lodforge_texgen::Job { scene: &sc, textures: &tex, view, light: LIGHT, pass: lodforge_texgen::Pass::Normal },
            ];
            let mut imgs = renderer.render_many(gpu, &jobs).into_iter();
            let (mut rgba, mut normal) = (imgs.next().unwrap_or_default(), imgs.next().unwrap_or_default());
            // Supersampled renders keep their soft coverage alpha (as tree billboards do).
            let finish = |img: &mut Vec<u8>| if lodforge_texgen::render::supersample() > 1 { lodforge_texgen::finish_alpha_soft(img, w) } else { lodforge_texgen::finish_alpha(img, w, 128) };
            finish(&mut rgba);
            finish(&mut normal);
            if !o.keep_alpha {
                rgba.chunks_exact_mut(4).for_each(|px| px[3] = 255);
            }
            out.push(Rendered { path: if pbr { pbr_dest(&path) } else { path.clone() }, w, h, rgba, normal });
        }
    }
    Ok((out, skipped))
}
