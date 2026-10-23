//! DynDOLOD's terrain underside (docs/re/terrain-lod-spec.md section 4): the LOD32 terrain mesh
//! of a worldspace, quality `TerrainUndersideQuality`, without skirts, water or water-aware
//! simplification, cell borders unprotected, hidden quadrants and cells outside the input
//! removed, faces flipped, in one `<World>_Underside.nif` with the observed layout of 4.2.
//! Approach (a) of 4.5: the BTR mesher is run once more.

use rayon::prelude::*;

use lodforge_nif::{
    AvObject, Block, LightingExtra, LightingShader, Nif, Node, ObjectNet, Transform, TriShape,
    VertexDesc, va,
};

use crate::bounds::{Aabb, BBox};
use crate::input::{CELL_UNITS, TerrainCell, TerrainInput};
use crate::mesh::{QuadOptions, build_quad};
use crate::settings::ControlFile;

#[derive(Debug, Clone, PartialEq)]
pub struct UndersideSettings {
    pub world: String,
    /// Quad lattice origin (`CellSW`).
    pub south_west: (i32, i32),
    /// LOD level of the quads (`LODLevel`, 32).
    pub level: i32,
    /// `TerrainUndersideQuality` (= `QualityLOD32` of the export), local units.
    pub quality: f32,
    pub max_verts: u32,
    /// Clip rectangle of the input cells (`TerrainUndersideMin/Max*` keys), inclusive.
    pub min_x: Option<i32>,
    pub min_y: Option<i32>,
    pub max_x: Option<i32>,
    pub max_y: Option<i32>,
    /// Cells whose hide flags are ignored (`TerrainUndersideIgnoreHide_<W>`).
    pub ignore_hide: Vec<(i32, i32)>,
    pub land_height: f32,
    pub water_height: Option<f32>,
    /// Season suffix of the LOD32 texture named by the shapes.
    pub season: Option<String>,
    /// `TerrainData=` of an export file.
    pub terrain_data: Option<String>,
}

impl UndersideSettings {
    /// DynDOLOD's defaults for a worldspace (export file of 4.3 without the optional keys).
    pub fn dyndolod(
        world: &str,
        south_west: (i32, i32),
        land_height: f32,
        water_height: Option<f32>,
        quality: f32,
    ) -> Self {
        UndersideSettings {
            world: world.to_string(),
            south_west,
            level: 32,
            quality,
            max_verts: 32767,
            min_x: None,
            min_y: None,
            max_x: None,
            max_y: None,
            ignore_hide: Vec::new(),
            land_height,
            water_height,
            season: None,
            terrain_data: None,
        }
    }

    /// From a parsed `LODGen_SSE_Export_TerrainUnderside_<W>.txt`.
    pub fn from_control(c: &ControlFile) -> Self {
        let level = c.lod_level.unwrap_or(32);
        let l = c.settings.level(level);
        let mut s = UndersideSettings::dyndolod(
            c.world.as_deref().unwrap_or(""),
            c.cell_sw.unwrap_or((0, 0)),
            c.land_height.unwrap_or(0.0),
            c.water_height,
            l.quality,
        );
        s.level = level;
        s.max_verts = l.max_verts;
        s.min_x = c.min_x;
        s.min_y = c.min_y;
        s.max_x = c.max_x;
        s.max_y = c.max_y;
        s.ignore_hide = c.ignore_hide.clone();
        s.terrain_data = c.terrain_data.clone();
        s
    }

    /// Parses the export text.
    pub fn parse(text: &str) -> Self {
        Self::from_control(&ControlFile::parse(text))
    }

    /// Takes the clip rectangle and the ignore list from another control file (the INI lines
    /// DynDOLOD adds per worldspace).
    pub fn apply_bounds(&mut self, c: &ControlFile) {
        self.min_x = c.min_x.or(self.min_x);
        self.min_y = c.min_y.or(self.min_y);
        self.max_x = c.max_x.or(self.max_x);
        self.max_y = c.max_y.or(self.max_y);
        if !c.ignore_hide.is_empty() {
            self.ignore_hide = c.ignore_hide.clone();
        }
    }
}

/// One LOD32 quad of the underside.
#[derive(Debug, Clone, PartialEq)]
pub struct UndersideQuad {
    pub x: i32,
    pub y: i32,
    pub level: i32,
    pub positions: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub triangles: Vec<[u16; 3]>,
    pub bound_center: [f32; 3],
    pub bound_radius: f32,
    /// World-space box (translation applied).
    pub aabb: Aabb,
}

/// Builds every quad of the lattice that holds terrain, sorted by (y, x). Cells outside the
/// settings' clip rectangle are left out of the input first.
pub fn build(input: &TerrainInput, s: &UndersideSettings) -> Result<Vec<UndersideQuad>, String> {
    let mut input = input.clone();
    input.south_west = s.south_west;
    input.default_land = s.land_height;
    input.default_water = s.water_height;
    let lo = (s.min_x.unwrap_or(i32::MIN), s.min_y.unwrap_or(i32::MIN));
    let hi = (s.max_x.unwrap_or(i32::MAX), s.max_y.unwrap_or(i32::MAX));
    input.clip(lo, hi);
    let field = input.sampler();
    let opts = QuadOptions {
        level: s.level,
        quality: s.quality,
        max_verts: s.max_verts,
        water_delta: -1.0,
        water_floor: -1.0,
        skirts: 0,
        protect_cell_borders: false,
        ignore_water: true,
        flip: true,
    };
    let ignore_hide = &s.ignore_hide;
    let field_ref = &field;
    let covered = move |cx: i32, cy: i32, bit: u8| -> bool {
        match field_ref.cell(cx, cy) {
            None => false,
            Some(c) => c.hide_quadrants & bit == 0 || ignore_hide.contains(&(cx, cy)),
        }
    };
    let quads = input.quads(s.level);
    let built: Vec<Result<Option<UndersideQuad>, String>> = quads
        .par_iter()
        .map(|&(qx, qy)| {
            let m = build_quad(&field, &opts, qx, qy, &covered)?;
            if m.land.triangles.is_empty() {
                return Ok(None);
            }
            let b = BBox::from_points(&m.land.positions);
            let lvl = s.level as f32;
            let local = b.lod_aabb(lvl);
            let aabb = Aabb {
                position: [
                    local.position[0] + qx as f32 * CELL_UNITS,
                    local.position[1] + qy as f32 * CELL_UNITS,
                    local.position[2],
                ],
                extent: local.extent,
            };
            Ok(Some(UndersideQuad {
                x: qx,
                y: qy,
                level: s.level,
                positions: m.land.positions,
                uvs: m.land.uvs,
                triangles: m.land.triangles,
                bound_center: b.center(false),
                bound_radius: b.radius(),
                aabb,
            }))
        })
        .collect();
    let mut out: Vec<UndersideQuad> = built
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .flatten()
        .collect();
    out.sort_by_key(|q| (q.y, q.x));
    Ok(out)
}

/// The underside shader (spec 4.2): default type, ZBuffer_Test | Model_Space_Normals |
/// Cast_Shadows, ZBuffer_Write, clamp, glossiness 1.
pub fn underside_shader(texture_set: u32) -> LightingShader {
    LightingShader {
        shader_type: 0,
        net: ObjectNet::default(),
        flags1: 0x8000_1200,
        flags2: 0x0000_0001,
        uv_offset: [0.0, 0.0],
        uv_scale: [1.0, 1.0],
        texture_set: Some(texture_set),
        emissive_color: [0.0; 3],
        emissive_multiple: 1.0,
        texture_clamp_mode: 0,
        alpha: 1.0,
        refraction_strength: 0.0,
        glossiness: 1.0,
        specular_color: [1.0; 3],
        specular_strength: 1.0,
        lighting_effect_1: 0.0,
        lighting_effect_2: 0.0,
        extra: LightingExtra::None,
    }
}

/// `meshes\Terrain\<W>\<W>_Underside.nif`: BSFadeNode root with one BSMultiBoundNode per quad,
/// each followed by its shader, texture set, shape and bound (spec 4.2).
pub fn to_nif(world: &str, season: Option<&str>, quads: &[UndersideQuad]) -> Nif {
    let mut nif = Nif::new();
    let suffix = season.map(|s| format!(".{s}")).unwrap_or_default();
    let children: Vec<Option<u32>> = (0..quads.len() as u32).map(|i| Some(1 + i * 6)).collect();
    nif.add_block(Block::Node(Node {
        av: AvObject::default(),
        children,
        effects: Vec::new(),
        multi_bound: None,
        culling_mode: None,
    }));
    for q in quads {
        let base = nif.blocks.len() as u32;
        nif.add_block(Block::Node(Node {
            av: AvObject::default(),
            children: vec![Some(base + 3)],
            effects: Vec::new(),
            multi_bound: Some(base + 4),
            culling_mode: Some(0),
        }));
        nif.add_block(Block::LightingShader(underside_shader(base + 2)));
        nif.add_block(Block::TextureSet(vec![
            format!(
                "Textures\\Terrain\\{world}\\{world}.{}.{}.{}{suffix}.dds",
                q.level, q.x, q.y
            ),
            "Textures\\black.dds".into(),
        ]));
        nif.add_block(Block::TriShape(TriShape {
            av: AvObject {
                net: ObjectNet::default(),
                flags: 0xE,
                transform: Transform {
                    translation: [q.x as f32 * CELL_UNITS, q.y as f32 * CELL_UNITS, 0.0],
                    scale: q.level as f32,
                    ..Default::default()
                },
                collision: None,
            },
            bound_center: q.bound_center,
            bound_radius: q.bound_radius,
            shader_property: Some(base + 1),
            vertex_desc: VertexDesc::sse(va::VERTEX | va::UVS).0,
            positions: q.positions.clone(),
            uvs: q.uvs.clone(),
            triangles: q.triangles.clone(),
            ..Default::default()
        }));
        nif.add_block(Block::MultiBound {
            data: Some(base + 5),
        });
        nif.add_block(Block::MultiBoundAabb {
            position: q.aabb.position,
            extent: q.aabb.extent,
        });
    }
    // The root is a BSFadeNode (the reader's Node carries no type of its own): name the block
    // types explicitly, in first-use order.
    let types = [
        "BSFadeNode",
        "BSMultiBoundNode",
        "BSLightingShaderProperty",
        "BSShaderTextureSet",
        "BSTriShape",
        "BSMultiBound",
        "BSMultiBoundAABB",
    ];
    nif.header.block_types = types.iter().map(|t| t.to_string()).collect();
    nif.header.block_type_index = (0..nif.blocks.len())
        .map(|i| if i == 0 { 0 } else { ((i - 1) % 6 + 1) as u16 })
        .collect();
    nif
}

/// Per-quad summary of an underside NIF (ours or a reference one) for comparisons.
#[derive(Debug, Clone, PartialEq)]
pub struct QuadSummary {
    pub translation: [f32; 3],
    pub scale: f32,
    pub flags: u32,
    pub vertex_desc: u64,
    pub node_culling: Option<u32>,
    pub positions: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub triangles: Vec<[u16; 3]>,
    pub bound_center: [f32; 3],
    pub bound_radius: f32,
    pub aabb: Option<Aabb>,
    pub textures: Vec<String>,
    pub shader: Option<LightingShader>,
}

pub fn summarize(nif: &Nif) -> Vec<QuadSummary> {
    let mut out = Vec::new();
    let Some(Block::Node(root)) = nif.roots.first().and_then(|&r| nif.get(r)) else {
        return out;
    };
    for &c in &root.children {
        let Some(Block::Node(n)) = nif.get(c) else {
            continue;
        };
        let aabb = match nif.get(n.multi_bound) {
            Some(Block::MultiBound { data }) => match nif.get(*data) {
                Some(Block::MultiBoundAabb { position, extent }) => Some(Aabb {
                    position: *position,
                    extent: *extent,
                }),
                _ => None,
            },
            _ => None,
        };
        for &sc in &n.children {
            let Some(Block::TriShape(s)) = nif.get(sc) else {
                continue;
            };
            let shader = match nif.get(s.shader_property) {
                Some(Block::LightingShader(l)) => Some(l.clone()),
                _ => None,
            };
            out.push(QuadSummary {
                translation: s.av.transform.translation,
                scale: s.av.transform.scale,
                flags: s.av.flags,
                vertex_desc: s.vertex_desc,
                node_culling: n.culling_mode,
                positions: s.positions.clone(),
                uvs: s.uvs.clone(),
                triangles: s.triangles.clone(),
                bound_center: s.bound_center,
                bound_radius: s.bound_radius,
                aabb,
                textures: nif.shape_textures(s),
                shader,
            });
        }
    }
    out
}

/// Whether a cell belongs to the underside input (helper for callers building the input).
pub fn cell_in_bounds(s: &UndersideSettings, c: &TerrainCell) -> bool {
    s.min_x.is_none_or(|v| c.x >= v)
        && s.max_x.is_none_or(|v| c.x <= v)
        && s.min_y.is_none_or(|v| c.y >= v)
        && s.max_y.is_none_or(|v| c.y <= v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::GRID;
    use lodforge_nif::parse;

    #[test]
    fn builds_flipped_quads_over_present_cells_only() {
        let mut cells = Vec::new();
        for y in 0..40 {
            for x in 0..40 {
                let mut c = TerrainCell::flat(x, y, 0.0, None);
                for k in 0..GRID * GRID {
                    c.heights[k] = (((x * 33 + (k % 33) as i32) as f32 * 0.21).sin() * 500.0
                        + ((y * 33 + (k / 33) as i32) as f32 * 0.17).cos() * 300.0)
                        .round();
                }
                cells.push(c);
            }
        }
        let mut input = TerrainInput {
            world: "W".into(),
            south_west: (-96, -96),
            max_level: 32,
            default_land: -27000.0,
            default_water: Some(-14000.0),
            cells,
        };
        input.normalize();
        let mut s = UndersideSettings::dyndolod("W", (-96, -96), -27000.0, Some(-14000.0), 10.0);
        s.max_y = Some(35);
        let quads = build(&input, &s).unwrap();
        assert_eq!(
            quads.iter().map(|q| (q.x, q.y)).collect::<Vec<_>>(),
            [(0, 0), (32, 0), (0, 32), (32, 32)]
        );
        for q in &quads {
            for t in &q.triangles {
                let p = t.map(|i| q.positions[i as usize]);
                let area = (p[1][0] - p[0][0]) * (p[2][1] - p[0][1])
                    - (p[1][1] - p[0][1]) * (p[2][0] - p[0][0]);
                assert!(area < 0.0);
                // Only over present cells (x < 40 cells, y <= 35 cells).
                let (cx, cy) = (
                    p.iter().map(|v| v[0]).sum::<f32>() / 3.0 / 128.0 + q.x as f32,
                    p.iter().map(|v| v[1]).sum::<f32>() / 3.0 / 128.0 + q.y as f32,
                );
                assert!(
                    cx < 40.0 && cy < 36.0,
                    "triangle over a missing cell at {cx},{cy}"
                );
            }
            assert_eq!(
                q.aabb.position[0],
                q.aabb.extent[0]
                    + q.x as f32 * 4096.0
                    + (q.positions.iter().map(|p| p[0]).fold(f32::MAX, f32::min)) * 32.0
            );
        }
        let nif = to_nif("W", None, &quads);
        let bytes = nif.to_bytes().unwrap();
        let back = parse(&bytes).unwrap();
        assert_eq!(back.block_type(0), "BSFadeNode");
        assert_eq!(back.blocks.len(), 1 + 6 * quads.len());
        let summary = summarize(&back);
        assert_eq!(summary.len(), quads.len());
        assert_eq!(summary[1].translation, [32.0 * 4096.0, 0.0, 0.0]);
        assert_eq!(
            summary[1].textures[0],
            "Textures\\Terrain\\W\\W.32.32.0.dds"
        );
        assert_eq!(summary[0].shader.as_ref().unwrap().flags1, 0x8000_1200);
        assert!(back.header.strings.is_empty());
    }
}
