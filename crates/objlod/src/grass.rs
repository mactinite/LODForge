//! Grass LOD in the LOD4 BTOs (docs/re/object-lod-spec.md 6.2): every instance of NGIO's grass
//! cache becomes billboard quads in a `grasspassthru` shape, settled on the DynDOLOD grass golden
//! of 2026-10-18 (`Tamriel.4.0.-16.bto`, cell `Tamrielx0000y-016.cgid`, the complex grass
//! `shrub_berry_green_00000806` and `field_green_00000801` fitted vertex by vertex):
//!
//! - a cache instance is sixteen halves: position x, y, z (wrapped into +-49152 around the
//!   group centre), a scale (not applied by LODGen: the quad widths are the sidecar's whatever the
//!   instance scale), the axes `A` (halves 4-6) and `B` (8-10) of the instance frame and the
//!   third `C = A x B` packed as (12, 7, 11), a signed height offset `t` (13; the quad height
//!   is `Height * (1 + t)`, negative values hang the quad below the ground, as the golden does),
//!   and two zero halves. The frame used for the quads is `Mx = (Ax, Bx, Cx)`, `My = -(Ay, By,
//!   Cy)`, `Mz = (Az, Bz, Cz)` (the stored matrix transposed with its second column negated);
//! - complex grass (`ComplexGrassBillboard=4`, `DynDOLOD_flat_4x2alt1_lod.nif`) gets four planes
//!   from the template's two, fitted per type on the golden quads (every type's medians match
//!   the formulas to 0.05 units): the `_1` plane (template x in -1..1, raw y +-0.05 under a node
//!   translation of 0.05) at `X = x * Width_1 / 2 - Adjust_1`, `Y = y_raw * Width_1 / 2 + 0.05 *
//!   Adjust_1` (so it leans by a tenth of its half width and sits a twentieth of its adjust off
//!   the instance); the `_2` plane (raw x +-0.05, y in -1..1) at `X = -x * (Width_2 / 2 -
//!   Adjust_2)`, `Y = -y * Width_2 / 2 + Adjust_2`; and each again rotated 180 degrees about the
//!   up axis with the texture mirrored in u (the template's `alt` planes). Vertical: `Z = (z *
//!   Height_N + ShiftZ) * (1 + t)` along `Mz`;
//! - vertex order per quad: the template's vertices 0, 2, 1, 3 (bottom -x, bottom +x, top +x,
//!   top -x; for the unmirrored planes that is u max, u min, u min, u max), triangles `[0 1 2]
//!   [0 2 3]`; normals `+-My` for the `_1` planes and `+-Mx` for the `_2`
//!   planes (the alt planes negative), tangent `-Mz` (flipped with the height sign), bitangent
//!   along the width edge; colours `ComplexGrassBrightnessTop` on the top vertices and
//!   `...Bottom` on the bottom ones (golden: `(178, 191, 191, 255)` from 0.7 / 0.75 / 0.75),
//!   `GrassBrightnessTop/Bottom` for plain grass;
//! - plain grass (`GrassBillboard=1`, `Internal`) is two crossed planes of the billboard's width
//!   and height, not seen in a golden (the Community Shaders target has HD grass only);
//! - shader: lighting type 0, flags1 `ZBuffer_Test`, flags2 `ZBuffer_Write | Vertex_Colors`,
//!   glossiness 80, specular black, emissive black times 0, clamp 0 on the `_Alpha` page,
//!   textures the page and its `_n`; one segment per shape, shapes filled cell by cell and split
//!   at 65,535 vertices like the golden (19 shapes for the 16 cells of `Tamriel.4.0.-16.bto`).
//!
//! `GrassDensity` below 100 keeps that share of the instances (a hash of the instance, so a
//! rebuild is reproducible; LODGen's choice is not known). The golden also holds a few cells
//! beyond the quad's own sixteen (shape bounds reaching x < 0 in `Tamriel.4.0.-16.bto`); not
//! reproduced.

use std::collections::HashMap;
use std::path::PathBuf;

use half::f16;
use lodforge_grass::cgid::CellCache;
use lodforge_nif::{LightingExtra, LightingShader, ObjectNet};

use crate::atlas_map::AtlasMap;
use crate::flat::FlatList;
use crate::material::{self, Material, SLSF2_VERTEX_COLORS, SLSF2_ZBUFFER_WRITE};
use crate::mesh::{MeshPart, ShaderSource};

/// The shape name of grass LOD.
pub const NAME: &str = "grasspassthru";

/// One grass type's billboard(s) and sizes (TexGen's `<billboard>.txt` sidecar, 6.2).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GrassType {
    /// `textures\terrain\lodgen\<plugin>\<stem>_1.dds` (or `<stem>.dds`), lower case.
    pub billboard_1: String,
    /// The `_2` billboard of complex grass.
    pub billboard_2: Option<String>,
    pub complex: bool,
    pub width: f32,
    pub height: f32,
    pub shift_z: f32,
    pub width_1: f32,
    pub height_1: f32,
    pub adjust_1: f32,
    pub width_2: f32,
    pub height_2: f32,
    pub adjust_2: f32,
}

impl GrassType {
    /// From the billboard path, its sidecar text (`[LOD]` keys; `None` without one) and the
    /// FlatTextures line's width / height / shift (used when the sidecar lacks a value).
    pub fn new(billboard: &str, sidecar: Option<&str>, flat: Option<(f32, f32, f32)>) -> GrassType {
        let mut t = GrassType { billboard_1: billboard.to_ascii_lowercase(), ..Default::default() };
        if let Some((w, h, z)) = flat {
            t.width = w;
            t.height = h;
            t.shift_z = z;
        }
        if let Some(txt) = sidecar {
            for line in txt.lines() {
                let Some((k, v)) = line.split_once('=') else { continue };
                let k = k.trim().to_ascii_uppercase();
                let v = v.trim();
                if k == "COMPLEX" {
                    t.complex = v.eq_ignore_ascii_case("true");
                    continue;
                }
                let Ok(v) = v.parse::<f32>() else { continue };
                match k.as_str() {
                    "WIDTH" => t.width = v,
                    "HEIGHT" => t.height = v,
                    "SHIFTZ" => t.shift_z = v,
                    "WIDTH_1" => t.width_1 = v,
                    "HEIGHT_1" => t.height_1 = v,
                    "ADJUST_1" => t.adjust_1 = v,
                    "WIDTH_2" => t.width_2 = v,
                    "HEIGHT_2" => t.height_2 = v,
                    "ADJUST_2" => t.adjust_2 = v,
                    _ => {}
                }
            }
        }
        if t.width_1 == 0.0 {
            t.width_1 = t.width;
        }
        if t.height_1 == 0.0 {
            t.height_1 = t.height;
        }
        if t.complex {
            let b = t.billboard_1.clone();
            t.billboard_2 = Some(match b.strip_suffix("_1.dds") {
                Some(stem) => format!("{stem}_2.dds"),
                None => format!("{}_2.dds", b.trim_end_matches(".dds")),
            });
            if t.width_2 == 0.0 {
                t.width_2 = t.width_1;
            }
            if t.height_2 == 0.0 {
                t.height_2 = t.height_1;
            }
        }
        t
    }

    /// The sidecar path of a billboard: `<stem>.txt` without the `_1` suffix.
    pub fn sidecar_path(billboard: &str) -> String {
        let b = billboard.to_ascii_lowercase();
        match b.strip_suffix("_1.dds") {
            Some(stem) => format!("{stem}.txt"),
            None => format!("{}.txt", b.trim_end_matches(".dds")),
        }
    }
}

/// Everything grass LOD needs besides the atlas map.
#[derive(Debug, Clone)]
pub struct GrassInputs {
    /// Folder with `<World>x<XXXX>y<YYYY>.cgid`.
    pub cache_dir: PathBuf,
    pub world: String,
    /// Grass map key (`<MODEL STEM>_<OBJECT ID 8 hex>.DDS`, upper case) -> type.
    pub types: HashMap<String, GrassType>,
    /// `GrassBrightnessTop/Bottom R, G, B` and the complex pair.
    pub top: [f32; 3],
    pub bottom: [f32; 3],
    pub complex_top: [f32; 3],
    pub complex_bottom: [f32; 3],
    /// Percent of the instances kept.
    pub density: f32,
}

impl GrassInputs {
    /// Parses `LODGen_SSE_Grass_Map_<World>.txt` (`key<TAB>billboard`) into types, reading each
    /// billboard's sidecar through `read` (Data-relative path) and the sizes from `flat`.
    pub fn from_map(cache_dir: PathBuf, world: &str, map: &str, flat: &FlatList, read: &dyn Fn(&str) -> Option<Vec<u8>>) -> GrassInputs {
        let mut types = HashMap::new();
        for line in map.lines() {
            let c: Vec<&str> = line.trim_end_matches('\r').split('\t').collect();
            if c.len() < 2 || c[0].trim().is_empty() {
                continue;
            }
            let billboard = c[1].trim();
            let sidecar = read(&GrassType::sidecar_path(billboard)).map(|b| String::from_utf8_lossy(&b).into_owned());
            let f = flat.get(billboard).map(|f| (f.width, f.height, f.shift_z));
            types.insert(c[0].trim().to_ascii_uppercase(), GrassType::new(billboard, sidecar.as_deref(), f));
        }
        GrassInputs { cache_dir, world: world.to_string(), types, top: [0.4, 0.445, 0.45], bottom: [0.2, 0.223, 0.225], complex_top: [0.7, 0.75, 0.75], complex_bottom: [0.7, 0.75, 0.75], density: 100.0 }
    }

    /// The cache file of a cell.
    pub fn cache_path(&self, cell: (i32, i32)) -> PathBuf {
        self.cache_dir.join(format!("{}x{:04}y{:04}.cgid", self.world, cell.0, cell.1))
    }

    /// The type of a cache entry: `<stem of the model>_<object id>.DDS`.
    pub fn type_of(&self, model: &str, form_id: u32) -> Option<&GrassType> {
        let stem = model.rsplit(['\\', '/']).next().unwrap_or(model);
        let stem = stem.rsplit_once('.').map(|(s, _)| s).unwrap_or(stem);
        let object = if form_id >> 24 == 0xFE { form_id & 0xFFF } else { form_id & 0x00FF_FFFF };
        self.types.get(&format!("{}_{:08X}.DDS", stem.to_ascii_uppercase(), object)).or_else(|| self.types.get(&format!("{}_{:08X}.DDS", stem.to_ascii_uppercase(), form_id)))
    }
}

/// A decoded cache instance.
#[derive(Debug, Clone, Copy)]
pub struct Instance {
    pub position: [f32; 3],
    pub scale: f32,
    /// The quad frame (see the module doc).
    pub mx: [f32; 3],
    pub my: [f32; 3],
    pub mz: [f32; 3],
    /// Signed height offset: the quad height is `Height * (1 + height_offset)`.
    pub height_offset: f32,
}

fn unwrap(v: f32, center: f32) -> f32 {
    v + ((center - v) / 49152.0).round() * 49152.0
}

impl Instance {
    pub fn decode(h: &[u16; 16], center: [f32; 3]) -> Instance {
        let f: Vec<f32> = h.iter().map(|&v| f16::from_bits(v).to_f32()).collect();
        let a = [f[4], f[5], f[6]];
        let b = [f[8], f[9], f[10]];
        // The stored third axis (halves 12, 7, 11) equals A x B on every instance of the studied
        // cell; recomputing the product in single precision changes nothing measurable.
        let c = [f[12], f[7], f[11]];
        Instance {
            position: [unwrap(f[0], center[0]), unwrap(f[1], center[1]), unwrap(f[2], center[2])],
            scale: f[3],
            mx: [a[0], b[0], c[0]],
            my: [-a[1], -b[1], -c[1]],
            mz: [a[2], b[2], c[2]],
            height_offset: f[13],
        }
    }
}

/// A plane of a billboard template: four template vertices (x, y, z, u, v) with `z` 0 at the
/// bottom and 1 at the top.
struct Plane {
    verts: [[f32; 5]; 4],
    /// Which billboard (`_1` = 0, `_2` = 1): its width / height / adjust and the lateral formula.
    texture: usize,
    /// The 180-degree copy (X and Y negated, u mirrored).
    rot180: bool,
    /// Normal: `+-My` (`_1` planes) or `+-Mx` (`_2` planes).
    normal_axis: usize,
    normal_sign: f32,
    /// Plain grass: no adjust terms.
    plain: bool,
}

/// `DynDOLOD_flat_4x2alt1_lod.nif`'s planes as stored: `_1` along x with raw y +-0.05 (the
/// shape's node translation of 0.05 is in the lateral formula), `_2` along y with x +-0.05.
const PLANE_1: [[f32; 5]; 4] = [[-1.0, 0.05, 0.0, 1.0, 1.0], [1.0, -0.05, 1.0, 0.0, 0.0], [1.0, 0.05, 0.0, 0.0, 1.0], [-1.0, -0.05, 1.0, 1.0, 0.0]];
const PLANE_2: [[f32; 5]; 4] = [[-0.05, -1.0, 0.0, 1.0, 1.0], [0.05, 1.0, 1.0, 0.0, 0.0], [-0.05, 1.0, 0.0, 0.0, 1.0], [0.05, -1.0, 1.0, 1.0, 0.0]];

fn complex_planes() -> Vec<Plane> {
    vec![
        Plane { verts: PLANE_1, texture: 0, rot180: false, normal_axis: 1, normal_sign: 1.0, plain: false },
        Plane { verts: PLANE_2, texture: 1, rot180: false, normal_axis: 0, normal_sign: 1.0, plain: false },
        Plane { verts: PLANE_1, texture: 0, rot180: true, normal_axis: 1, normal_sign: -1.0, plain: false },
        Plane { verts: PLANE_2, texture: 1, rot180: true, normal_axis: 0, normal_sign: -1.0, plain: false },
    ]
}

/// The internal two-plane billboard of plain grass (not seen in a golden: both planes with the
/// billboard's width, crossing at the instance).
fn plain_planes() -> Vec<Plane> {
    vec![
        Plane { verts: PLANE_1, texture: 0, rot180: false, normal_axis: 1, normal_sign: 1.0, plain: true },
        Plane { verts: PLANE_2, texture: 0, rot180: false, normal_axis: 0, normal_sign: 1.0, plain: true },
    ]
}

impl Plane {
    /// The lateral (X along Mx, Y along My) position of a template vertex (see the module doc).
    fn lateral(&self, t: &GrassType, w: f32, x: f32, y: f32) -> (f32, f32) {
        let (a1, a2) = if self.plain { (0.0, 0.0) } else { (t.adjust_1, t.adjust_2) };
        let (mut px, mut py) = if self.texture == 0 { (x * w * 0.5 - a1, y * w * 0.5 + 0.05 * a1) } else { (-x * (w * 0.5 - a2), -y * w * 0.5 + a2) };
        if self.rot180 {
            px = -px;
            py = -py;
        }
        (px, py)
    }
}

/// The material of grass shapes: the `_Alpha` page when the map has a row for the billboard.
pub fn material(atlas: &AtlasMap, billboard: &str) -> Material {
    let row = atlas.lookup(billboard, "", "", None);
    let mut written = vec![String::new(); 9];
    match row {
        Some(row) => {
            let page = crate::export::texture_key(&row.page);
            let stem = page.trim_end_matches(".dds").to_string();
            written[0] = format!("Data\\{page}");
            written[1] = format!("Data\\{stem}_n.dds");
        }
        None => {
            written[0] = material::written_texture(billboard);
            written[1] = material::written_texture(&format!("{}_n.dds", billboard.trim_end_matches(".dds")));
        }
    }
    let lighting = LightingShader {
        shader_type: 0,
        net: ObjectNet::default(),
        flags1: 0x8000_0000,
        flags2: SLSF2_ZBUFFER_WRITE | SLSF2_VERTEX_COLORS,
        uv_offset: [0.0; 2],
        uv_scale: [1.0; 2],
        texture_set: None,
        emissive_color: [0.0; 3],
        emissive_multiple: 0.0,
        texture_clamp_mode: if row.is_some() { 0 } else { 3 },
        alpha: 1.0,
        refraction_strength: 0.0,
        glossiness: 80.0,
        specular_color: [0.0; 3],
        specular_strength: 1.0,
        lighting_effect_1: 0.0,
        lighting_effect_2: 0.0,
        extra: LightingExtra::None,
    };
    Material::fixed(NAME.to_string(), written, lighting)
}

/// The quads of every instance of one grass type in a cell, one part per quad (so the shapes
/// fill up to the vertex limit quad by quad, as the golden's do), in world space.
pub fn build_type(inputs: &GrassInputs, atlas: &AtlasMap, t: &GrassType, instances: &[Instance]) -> Vec<MeshPart> {
    let tile = |b: &str| -> [f32; 4] {
        match atlas.lookup(b, "", "", None) {
            Some(row) => {
                let a = row.remap([0.0, 0.0]);
                let z = row.remap([1.0, 1.0]);
                [a[0], a[1], z[0], z[1]]
            }
            None => [0.0, 0.0, 1.0, 1.0],
        }
    };
    let tiles = [tile(&t.billboard_1), t.billboard_2.as_deref().map(tile).unwrap_or([0.0, 0.0, 1.0, 1.0])];
    let planes = if t.complex { complex_planes() } else { plain_planes() };
    let (top, bottom) = if t.complex { (inputs.complex_top, inputs.complex_bottom) } else { (inputs.top, inputs.bottom) };
    let colour = |c: [f32; 3]| [(c[0].clamp(0.0, 1.0) * 255.0).round() as u8, (c[1].clamp(0.0, 1.0) * 255.0).round() as u8, (c[2].clamp(0.0, 1.0) * 255.0).round() as u8, 255];
    let (top, bottom) = (colour(top), colour(bottom));
    let dims = [(t.width_1, t.height_1), (t.width_2, t.height_2)];
    let shader = ShaderSource::Lighting(LightingShader::default());
    let mut textures: [String; 9] = Default::default();
    textures[0] = t.billboard_1.clone();
    let new_part = || MeshPart { name: NAME.to_string(), index: 0, textures: textures.clone(), shader: shader.clone(), positions: Vec::new(), uvs: Vec::new(), normals: Vec::new(), tangents: Vec::new(), bitangents: Vec::new(), colors: Some(Vec::new()), triangles: Vec::new(), source_normals: true, source_tangents: true, legacy: false, alpha: None };
    let mut parts = Vec::with_capacity(instances.len() * planes.len());
    for inst in instances {
        let hf = 1.0 + inst.height_offset;
        for p in &planes {
            let (w, h) = dims[p.texture];
            let uv = tiles[p.texture];
            // Template vertices -> world, in the golden's order: the template's vertices 0, 2,
            // 1, 3 (bottom -x, bottom +x, top +x, top -x), whatever the (mirrored) UVs.
            let vs: Vec<([f32; 3], [f32; 2], bool)> = [0usize, 2, 1, 3]
                .iter()
                .map(|&i| &p.verts[i])
                .map(|v| {
                    let (x, y) = p.lateral(t, w, v[0], v[1]);
                    let z = (v[2] * h + t.shift_z) * hf;
                    let world = [
                        inst.position[0] + x * inst.mx[0] + y * inst.my[0] + z * inst.mz[0],
                        inst.position[1] + x * inst.mx[1] + y * inst.my[1] + z * inst.mz[1],
                        inst.position[2] + x * inst.mx[2] + y * inst.my[2] + z * inst.mz[2],
                    ];
                    let u = if p.rot180 { 1.0 - v[3] } else { v[3] };
                    (world, [uv[0] + u * (uv[2] - uv[0]), uv[1] + v[4] * (uv[3] - uv[1])], v[2] > 0.5)
                })
                .collect();
            let mut part = new_part();
            let base = 0u16;
            let axis = if p.normal_axis == 0 { inst.mx } else { inst.my };
            let normal = [axis[0] * p.normal_sign, axis[1] * p.normal_sign, axis[2] * p.normal_sign];
            let ts = if hf < 0.0 { 1.0 } else { -1.0 };
            let tangent = [inst.mz[0] * ts, inst.mz[1] * ts, inst.mz[2] * ts];
            let e = [vs[1].0[0] - vs[0].0[0], vs[1].0[1] - vs[0].0[1], vs[1].0[2] - vs[0].0[2]];
            let el = (e[0] * e[0] + e[1] * e[1] + e[2] * e[2]).sqrt().max(1e-6);
            let bitangent = [e[0] / el, e[1] / el, e[2] / el];
            for (world, uv, is_top) in &vs {
                part.positions.push(*world);
                part.uvs.push(*uv);
                part.normals.push(normal);
                part.tangents.push(tangent);
                part.bitangents.push(bitangent);
                part.colors.as_mut().unwrap().push(if *is_top { top } else { bottom });
            }
            part.triangles.extend([[base, base + 1, base + 2], [base, base + 2, base + 3]]);
            parts.push(part);
        }
    }
    parts
}

/// The instances of a cell's cache, per type (`None` when the cell has no cache file).
pub fn read_cell(inputs: &GrassInputs, cell: (i32, i32)) -> Option<Vec<(GrassType, Vec<Instance>)>> {
    let bytes = std::fs::read(inputs.cache_path(cell)).ok()?;
    // Trailing bytes ignored as the game does (NGIO rewrites cache files in place).
    let (cache, _) = CellCache::parse_prefix(&bytes)?;
    let keep = |i: usize| -> bool {
        if inputs.density >= 100.0 {
            return true;
        }
        let mut h: u32 = 0x811c_9dc5 ^ (cell.0 as u32).wrapping_mul(0x9e37_79b9) ^ (cell.1 as u32).wrapping_mul(0x85eb_ca6b);
        for b in (i as u32).to_le_bytes() {
            h ^= u32::from(b);
            h = h.wrapping_mul(0x0100_0193);
        }
        (h % 10_000) as f32 / 100.0 < inputs.density
    };
    let mut out = Vec::new();
    let mut n = 0usize;
    for t in &cache.types {
        let Some(gt) = inputs.type_of(&t.model, t.form_id) else {
            n += t.groups.iter().map(|g| g.instances.len()).sum::<usize>();
            continue;
        };
        let mut instances = Vec::new();
        for g in &t.groups {
            for raw in &g.instances {
                if keep(n) {
                    instances.push(Instance::decode(raw, g.center));
                }
                n += 1;
            }
        }
        out.push((gt.clone(), instances));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn golden_shrub_berry() {
        // Grass golden 2026-10-18, Tamrielx0000y-016.cgid: a Shrub_Berry_Green instance (frame
        // from the dump) and its plane 1 quad in Tamriel.4.0.-16.bto (vertices v28..v31).
        let t = GrassType::new(
            "textures\\terrain\\lodgen\\seasonal landscapes.esp\\shrub_berry_green_00000806_1.dds",
            Some("[LOD]\nWidth=52.313988\nDepth=63.141434\nHeight=46.501324\nShiftZ=0.984830\nComplex=True\nWidth_1=51.843735\nHeight_1=46.501324\nAdjust_1=5.222918\nWidth_2=48.622505\nHeight_2=46.501324\nAdjust_2=7.038453\n"),
            None,
        );
        assert!(t.complex);
        assert_eq!(t.billboard_2.as_deref(), Some("textures\\terrain\\lodgen\\seasonal landscapes.esp\\shrub_berry_green_00000806_2.dds"));
        let inst = Instance {
            position: [46.22, -64992.0, 2812.0],
            scale: 0.99,
            mx: [-0.215, -0.975, -0.054],
            my: [-0.977, 0.215, 0.012],
            mz: [0.0, -0.056, 0.998],
            height_offset: -0.021,
        };
        let mut inputs = GrassInputs::from_map(PathBuf::new(), "Tamriel", "", &FlatList::default(), &|_| None);
        inputs.types.insert("SHRUB_BERRY_GREEN_00000806.DDS".into(), t.clone());
        let parts = build_type(&inputs, &AtlasMap::default(), &t, &[inst]);
        assert_eq!(parts.len(), 4);
        let mut p = parts[0].clone();
        for q in &parts[1..] {
            let base = p.positions.len() as u16;
            p.positions.extend_from_slice(&q.positions);
            p.uvs.extend_from_slice(&q.uvs);
            p.colors.as_mut().unwrap().extend_from_slice(q.colors.as_ref().unwrap());
            p.triangles.extend(q.triangles.iter().map(|t| [t[0] + base, t[1] + base, t[2] + base]));
        }
        let p = &p;
        assert_eq!(p.positions.len(), 16);
        assert_eq!(p.triangles.len(), 8);
        // Plane 1, bottom edge: v0 at +Mx, v1 at -Mx, both shifted -Adjust_1 along Mx.
        let close = |a: [f32; 3], b: [f32; 3], tol: f32| (0..3).all(|i| (a[i] - b[i]).abs() < tol);
        assert!(close(p.positions[0], [51.445, -64961.359, 2814.667], 0.2), "{:?}", p.positions[0]);
        assert!(close(p.positions[1], [40.295, -65011.914, 2811.859], 0.2), "{:?}", p.positions[1]);
        assert!(close(p.positions[2], [42.826, -65014.996, 2857.261], 0.2), "{:?}", p.positions[2]);
        assert!(close(p.positions[3], [53.977, -64964.441, 2860.069], 0.2), "{:?}", p.positions[3]);
        // Plane 2 (quad 11 of the golden): along My, Adjust_2 along My, thickness W2/2 - A2.
        assert!(close(p.positions[4], [15.418, -64986.164, 2813.289], 0.2), "{:?}", p.positions[4]);
        assert!(close(p.positions[5], [62.901, -64996.605, 2812.709], 0.2), "{:?}", p.positions[5]);
        assert_eq!(p.uvs[0], [1.0, 1.0]);
        assert_eq!(p.uvs[2], [0.0, 0.0]);
        assert_eq!(p.colors.as_ref().unwrap()[0], [179, 191, 191, 255]);
        assert_eq!(p.triangles[0], [0, 1, 2]);
        // Plane 3 mirrors u.
        assert_eq!(p.uvs[8], [0.0, 1.0]);
        assert_eq!(inputs.type_of("Landscape\\Grass\\Shrub_Berry_Green.nif", 0x1600_0806).map(|g| g.width_1), Some(51.843735));
    }
}
