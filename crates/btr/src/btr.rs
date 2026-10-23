//! Terrain LOD file (.btr) assembly: the observed block layout of docs/re/terrain-lod-spec.md
//! section 1 on top of the `lodforge-nif` writer, and the reverse (a parsed BTR back into the
//! same description) for comparisons.

use lodforge_nif::writer::{Encoded, encode};
use lodforge_nif::{
    AvObject, Block, LightingShader, Nif, Node, ObjectNet, Ref, Segment, Transform, TriShape,
    VertexDesc, va,
};

use crate::bounds::{Aabb, BBox};
use crate::input::CELL_UNITS;
use crate::mesh::{QuadMesh, WaterCell};

/// Geometry of one shape as stored (quad-local units).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ShapeData {
    pub positions: Vec<[f32; 3]>,
    /// Empty for water shapes.
    pub uvs: Vec<[f32; 2]>,
    pub triangles: Vec<[u16; 3]>,
    pub bound_center: [f32; 3],
    pub bound_radius: f32,
    /// LOD4 water shapes: per-cell segments.
    pub segments: Option<Vec<Segment>>,
}

/// One terrain LOD file.
#[derive(Debug, Clone, PartialEq)]
pub struct Btr {
    pub level: i32,
    /// South-west cell (from the file name; 0 when read from bytes alone).
    pub x: i32,
    pub y: i32,
    pub z_shift: f32,
    pub land: ShapeData,
    /// Water shapes in the observed order (LOD4: ascending height; LOD8+: at most one).
    pub water: Vec<ShapeData>,
    pub land_aabb: Aabb,
    pub water_aabb: Option<Aabb>,
}

/// The land shader of every golden BTR (spec 1.6): LOD Landscape Noise, ZBuffer_Test | Own_Emit
/// | Model_Space_Normals, ZBuffer_Write | LOD_Landscape, clamp, glossiness 1, no lighting effects.
pub fn terrain_shader(texture_set: Ref) -> LightingShader {
    LightingShader {
        shader_type: 18,
        net: ObjectNet::default(),
        flags1: 0x8040_1000,
        flags2: 0x0000_0003,
        uv_offset: [0.0, 0.0],
        uv_scale: [1.0, 1.0],
        texture_set,
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
        extra: lodforge_nif::LightingExtra::None,
    }
}

/// `<World>.<L>.<x>.<y>[.<SEASON>].btr`
pub fn file_name(world: &str, level: i32, x: i32, y: i32, season: Option<&str>) -> String {
    match season {
        Some(s) => format!("{world}.{level}.{x}.{y}.{s}.btr"),
        None => format!("{world}.{level}.{x}.{y}.btr"),
    }
}

/// (world, level, x, y, season) from a file name.
pub fn parse_file_name(name: &str) -> Option<(String, i32, i32, i32, Option<String>)> {
    let stem = name
        .strip_suffix(".btr")
        .or_else(|| name.strip_suffix(".BTR"))?;
    let parts: Vec<&str> = stem.split('.').collect();
    let (world_parts, rest) = if parts.len() >= 4 && parts[parts.len() - 1].parse::<i32>().is_ok() {
        (&parts[..parts.len() - 3], &parts[parts.len() - 3..])
    } else if parts.len() >= 5 {
        (&parts[..parts.len() - 4], &parts[parts.len() - 4..])
    } else {
        return None;
    };
    let level = rest[0].parse().ok()?;
    let x = rest[1].parse().ok()?;
    let y = rest[2].parse().ok()?;
    let season = rest.get(3).map(|s| s.to_string());
    Some((world_parts.join("."), level, x, y, season))
}

/// The two texture slots (spec 1.5).
pub fn texture_paths(world: &str, level: i32, x: i32, y: i32, season: Option<&str>) -> Vec<String> {
    let suffix = season.map(|s| format!(".{s}")).unwrap_or_default();
    vec![
        format!("Data\\Textures\\Terrain\\{world}\\{world}.{level}.{x}.{y}{suffix}.dds"),
        format!("Data\\Textures\\Terrain\\{world}\\{world}.{level}.{x}.{y}{suffix}_n.dds"),
    ]
}

fn water_shapes(level: i32, cells: &[WaterCell]) -> Vec<ShapeData> {
    let side = CELL_UNITS / level as f32;
    let mut shapes = Vec::new();
    if cells.is_empty() {
        return shapes;
    }
    // LOD4: one shape per height with per-cell segments; LOD8+: one shape for everything.
    let groups: Vec<Vec<&WaterCell>> = if level == 4 {
        let mut heights: Vec<f32> = cells.iter().map(|c| c.height).collect();
        heights.sort_by(|a, b| a.partial_cmp(b).unwrap());
        heights.dedup();
        heights
            .iter()
            .map(|&h| cells.iter().filter(|c| c.height == h).collect())
            .collect()
    } else {
        vec![cells.iter().collect()]
    };
    for group in groups {
        let mut s = ShapeData::default();
        let mut index: std::collections::HashMap<(u32, u32, u32), u16> =
            std::collections::HashMap::new();
        let mut cells: Vec<&WaterCell> = group;
        cells.sort_by_key(|c| (c.dx, c.dy));
        let mut slot_tris: Vec<Option<u32>> = vec![None; 16];
        for c in cells {
            let (x0, y0) = (f32::from(c.dx) * side, f32::from(c.dy) * side);
            let corners = [
                [x0, y0, c.height],
                [x0 + side, y0, c.height],
                [x0, y0 + side, c.height],
                [x0 + side, y0 + side, c.height],
            ];
            let idx = corners.map(|p| {
                *index
                    .entry((p[0].to_bits(), p[1].to_bits(), p[2].to_bits()))
                    .or_insert_with(|| {
                        s.positions.push(p);
                        (s.positions.len() - 1) as u16
                    })
            });
            if level == 4 {
                slot_tris[usize::from(c.dx) * 4 + usize::from(c.dy)] =
                    Some(s.triangles.len() as u32);
            }
            s.triangles.push([idx[0], idx[1], idx[2]]);
            s.triangles.push([idx[1], idx[3], idx[2]]);
        }
        if level == 4 {
            let last = slot_tris.iter().rposition(Option::is_some).unwrap_or(0);
            s.segments = Some(
                slot_tris[..=last]
                    .iter()
                    .map(|t| {
                        t.map_or(Segment::default(), |first| Segment {
                            flags: 0,
                            start: first * 3,
                            count: 2,
                        })
                    })
                    .collect(),
            );
        }
        let b = BBox::from_points(&s.positions);
        s.bound_center = b.center(false);
        s.bound_radius = b.max_radius(&s.positions);
        shapes.push(s);
    }
    shapes
}

impl Btr {
    /// The file description of a built quad.
    pub fn from_mesh(m: &QuadMesh, z_shift: f32) -> Btr {
        let lb = BBox::from_points(&m.land.positions);
        let land = ShapeData {
            positions: m.land.positions.clone(),
            uvs: m.land.uvs.clone(),
            triangles: m.land.triangles.clone(),
            bound_center: lb.center(false),
            bound_radius: lb.radius(),
            segments: None,
        };
        let water = water_shapes(m.level, &m.water);
        let water_aabb = (!water.is_empty()).then(|| {
            let mut b = BBox::default();
            for s in &water {
                for p in &s.positions {
                    b.add(*p);
                }
            }
            b.lod_aabb(m.level as f32)
        });
        Btr {
            level: m.level,
            x: m.x,
            y: m.y,
            z_shift,
            land,
            water,
            land_aabb: lb.lod_aabb(m.level as f32),
            water_aabb,
        }
    }

    pub fn file_name(&self, world: &str, season: Option<&str>) -> String {
        file_name(world, self.level, self.x, self.y, season)
    }

    fn shape(&self, s: &ShapeData, name: Option<&str>, shader: Ref) -> TriShape {
        let desc = if s.uvs.is_empty() {
            VertexDesc::sse(va::VERTEX)
        } else {
            VertexDesc::sse(va::VERTEX | va::UVS)
        };
        TriShape {
            av: AvObject {
                net: ObjectNet {
                    name: name.map(str::to_string),
                    ..Default::default()
                },
                flags: 0xE,
                transform: Transform {
                    translation: [0.0, 0.0, self.z_shift],
                    scale: self.level as f32,
                    ..Default::default()
                },
                collision: None,
            },
            bound_center: s.bound_center,
            bound_radius: s.bound_radius,
            skin: None,
            shader_property: shader,
            alpha_property: None,
            vertex_desc: desc.0,
            positions: s.positions.clone(),
            uvs: s.uvs.clone(),
            triangles: s.triangles.clone(),
            segments: s.segments.clone(),
            ..Default::default()
        }
    }

    /// The NIF in the observed block order (spec 1.3): root, land, shader, texture set, [water
    /// node, water shapes, water bound], land bound.
    pub fn to_nif(&self, world: &str, season: Option<&str>) -> Nif {
        let mut nif = Nif::new();
        let has_water = !self.water.is_empty();
        let water_node_index = 4u32;
        let first_water_shape = 5u32;
        let water_bound_index = first_water_shape + self.water.len() as u32;
        let land_bound_index = if has_water { water_bound_index + 2 } else { 4 };
        let mut root_children = vec![Some(1)];
        if has_water {
            root_children.push(Some(water_node_index));
        }
        nif.add_block(Block::Node(Node {
            av: AvObject {
                net: ObjectNet::named("chunk"),
                ..Default::default()
            },
            children: root_children,
            effects: Vec::new(),
            multi_bound: Some(land_bound_index),
            culling_mode: Some(1),
        }));
        nif.add_block(Block::TriShape(self.shape(
            &self.land,
            Some("land"),
            Some(2),
        )));
        nif.add_block(Block::LightingShader(terrain_shader(Some(3))));
        nif.add_block(Block::TextureSet(texture_paths(
            world, self.level, self.x, self.y, season,
        )));
        if has_water {
            let children: Vec<Ref> = (0..self.water.len() as u32)
                .map(|i| Some(first_water_shape + i))
                .collect();
            nif.add_block(Block::Node(Node {
                av: AvObject {
                    net: ObjectNet::named("WATER"),
                    ..Default::default()
                },
                children,
                effects: Vec::new(),
                multi_bound: Some(water_bound_index),
                culling_mode: Some(0),
            }));
            for w in &self.water {
                nif.add_block(Block::TriShape(self.shape(w, None, None)));
            }
            let wa = self.water_aabb.unwrap_or_default();
            nif.add_block(Block::MultiBound {
                data: Some(water_bound_index + 1),
            });
            nif.add_block(Block::MultiBoundAabb {
                position: wa.position,
                extent: wa.extent,
            });
        }
        nif.add_block(Block::MultiBound {
            data: Some(land_bound_index + 1),
        });
        nif.add_block(Block::MultiBoundAabb {
            position: self.land_aabb.position,
            extent: self.land_aabb.extent,
        });
        nif
    }

    /// Serialized files for the default season and every extra season: the geometry is encoded
    /// once, only the texture set block differs. Returns (file name, bytes) pairs.
    pub fn write_variants(
        &self,
        world: &str,
        seasons: &[Option<&str>],
    ) -> lodforge_nif::Result<Vec<(String, Vec<u8>)>> {
        let mut nif = self.to_nif(world, None);
        let mut enc: Encoded = encode(&nif)?;
        let mut out = Vec::with_capacity(seasons.len());
        for &season in seasons {
            nif.blocks[3] =
                Block::TextureSet(texture_paths(world, self.level, self.x, self.y, season));
            enc.replace_block(&nif, 3)?;
            out.push((
                self.file_name(world, season),
                enc.to_bytes(&nif.header, &nif.roots)?,
            ));
        }
        Ok(out)
    }

    /// Reads a BTR back into its description (block layout of spec 1.3); `None` when the file is
    /// laid out differently. `x` and `y` are left at 0.
    pub fn from_nif(nif: &Nif) -> Option<Btr> {
        let Block::Node(root) = nif.get(*nif.roots.first()?)? else {
            return None;
        };
        let aabb = |mb: Ref| -> Option<Aabb> {
            let Block::MultiBound { data } = nif.get(mb)? else {
                return None;
            };
            let Block::MultiBoundAabb { position, extent } = nif.get(*data)? else {
                return None;
            };
            Some(Aabb {
                position: *position,
                extent: *extent,
            })
        };
        let shape_data = |s: &TriShape| ShapeData {
            positions: s.positions.clone(),
            uvs: s.uvs.clone(),
            triangles: s.triangles.clone(),
            bound_center: s.bound_center,
            bound_radius: s.bound_radius,
            segments: s.segments.clone(),
        };
        let mut land = None;
        let mut water = Vec::new();
        let mut water_aabb = None;
        for &c in &root.children {
            match nif.get(c)? {
                Block::TriShape(s) if land.is_none() => {
                    land = Some((
                        s.av.transform.scale,
                        s.av.transform.translation[2],
                        shape_data(s),
                    ))
                }
                Block::Node(n) => {
                    water_aabb = aabb(n.multi_bound);
                    for &wc in &n.children {
                        let Block::TriShape(s) = nif.get(wc)? else {
                            return None;
                        };
                        water.push(shape_data(s));
                    }
                }
                _ => return None,
            }
        }
        let (scale, z_shift, land) = land?;
        Some(Btr {
            level: scale as i32,
            x: 0,
            y: 0,
            z_shift,
            land,
            water,
            land_aabb: aabb(root.multi_bound)?,
            water_aabb,
        })
    }

    /// Texture paths of a parsed BTR.
    pub fn textures(nif: &Nif) -> Vec<String> {
        match nif.blocks.get(1) {
            Some(Block::TriShape(s)) => nif.shape_textures(s),
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::LandMesh;
    use lodforge_nif::parse;

    fn mesh() -> QuadMesh {
        let positions = vec![
            [0.0, 0.0, -1919.0],
            [4096.0, 0.0, -1500.0],
            [0.0, 4096.0, -1200.0],
            [4096.0, 4096.0, -990.0],
        ];
        let uvs = positions
            .iter()
            .map(|p| [p[0] / 4096.0, 1.0 - p[1] / 4096.0])
            .collect();
        let land = LandMesh {
            positions,
            uvs,
            triangles: vec![[0, 1, 2], [2, 1, 3]],
            grid_vertices: 4,
            grid_triangles: 2,
        };
        let water = vec![
            WaterCell {
                dx: 0,
                dy: 0,
                height: -3500.0,
            },
            WaterCell {
                dx: 0,
                dy: 1,
                height: -3500.0,
            },
            WaterCell {
                dx: 3,
                dy: 3,
                height: -3400.0,
            },
        ];
        QuadMesh {
            level: 4,
            x: 0,
            y: 0,
            land,
            water,
            max_error: 0.0,
            budget_hit: false,
            shore: Default::default(),
        }
    }

    #[test]
    fn file_names() {
        assert_eq!(file_name("Tamriel", 4, -8, 96, None), "Tamriel.4.-8.96.btr");
        assert_eq!(
            file_name("Tamriel", 32, 0, -64, Some("WIN")),
            "Tamriel.32.0.-64.WIN.btr"
        );
        assert_eq!(
            parse_file_name("Tamriel.4.-8.96.btr"),
            Some(("Tamriel".into(), 4, -8, 96, None))
        );
        assert_eq!(
            parse_file_name("DLC2SolstheimWorld.32.0.-64.WIN.btr"),
            Some(("DLC2SolstheimWorld".into(), 32, 0, -64, Some("WIN".into())))
        );
        assert_eq!(parse_file_name("foo.dds"), None);
    }

    #[test]
    fn layout_matches_the_spec() {
        let b = Btr::from_mesh(&mesh(), 0.0);
        assert_eq!(b.land_aabb.position, [8192.0, 8192.0, -3838.0]);
        assert_eq!(b.land_aabb.extent, [8192.0, 8192.0, 3838.0]);
        assert_eq!(b.water.len(), 2);
        let w0 = &b.water[0];
        assert_eq!(w0.positions.len(), 6); // two cells sharing an edge
        assert_eq!(w0.triangles.len(), 4);
        assert_eq!(
            w0.segments.as_deref().unwrap(),
            &[
                Segment {
                    flags: 0,
                    start: 0,
                    count: 2
                },
                Segment {
                    flags: 0,
                    start: 6,
                    count: 2
                }
            ]
        );
        let w1 = &b.water[1];
        assert_eq!(w1.segments.as_ref().unwrap().len(), 16);
        assert_eq!(
            w1.segments.as_ref().unwrap()[15],
            Segment {
                flags: 0,
                start: 0,
                count: 2
            }
        );
        assert!((w1.bound_radius - 724.077).abs() < 0.01);
        let nif = b.to_nif("Tamriel", None);
        let bytes = nif.to_bytes().unwrap();
        let back = parse(&bytes).unwrap();
        let types: Vec<&str> = (0..back.blocks.len()).map(|i| back.block_type(i)).collect();
        assert_eq!(
            types,
            [
                "BSMultiBoundNode",
                "BSTriShape",
                "BSLightingShaderProperty",
                "BSShaderTextureSet",
                "BSMultiBoundNode",
                "BSSubIndexTriShape",
                "BSSubIndexTriShape",
                "BSMultiBound",
                "BSMultiBoundAABB",
                "BSMultiBound",
                "BSMultiBoundAABB"
            ]
        );
        assert_eq!(back.header.strings, ["chunk", "land", "WATER"]);
        let Block::Node(root) = &back.blocks[0] else {
            panic!()
        };
        assert_eq!(
            (
                root.culling_mode,
                root.children.as_slice(),
                root.multi_bound
            ),
            (Some(1), &[Some(1), Some(4)][..], Some(9))
        );
        let Block::Node(wn) = &back.blocks[4] else {
            panic!()
        };
        assert_eq!(
            (wn.culling_mode, wn.children.as_slice(), wn.multi_bound),
            (Some(0), &[Some(5), Some(6)][..], Some(7))
        );
        let Block::TriShape(land) = &back.blocks[1] else {
            panic!()
        };
        assert_eq!(
            (land.vertex_desc, land.av.transform.scale, land.av.flags),
            (0x0000_3000_0000_0405, 4.0, 0xE)
        );
        let Block::TriShape(w) = &back.blocks[5] else {
            panic!()
        };
        assert_eq!(w.vertex_desc, 0x0000_1000_0000_0404);
        assert_eq!(
            Btr::textures(&back),
            texture_paths("Tamriel", 4, 0, 0, None)
        );
        let mut round = Btr::from_nif(&back).unwrap();
        round.x = 0;
        assert_eq!(round, b);
        // Season variants share everything but the texture paths.
        let files = b.write_variants("Tamriel", &[None, Some("WIN")]).unwrap();
        assert_eq!(files[0].0, "Tamriel.4.0.0.btr");
        assert_eq!(files[0].1, bytes);
        let win = parse(&files[1].1).unwrap();
        assert_eq!(
            Btr::textures(&win),
            texture_paths("Tamriel", 4, 0, 0, Some("WIN"))
        );
        assert_eq!(Btr::from_nif(&win).unwrap(), Btr::from_nif(&back).unwrap());
    }

    #[test]
    fn lod8_water_is_one_shape() {
        let mut m = mesh();
        m.level = 8;
        let b = Btr::from_mesh(&m, 1.5);
        assert_eq!(b.water.len(), 1);
        assert!(b.water[0].segments.is_none());
        assert_eq!(b.water[0].triangles.len(), 6);
        let nif = b.to_nif("W", None);
        assert_eq!(nif.block_type(5), "BSTriShape");
        let Block::TriShape(s) = &nif.blocks[1] else {
            panic!()
        };
        assert_eq!(s.av.transform.translation, [0.0, 0.0, 1.5]);
    }
}
