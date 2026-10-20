//! A NIF flattened into renderable meshes: node transforms applied, textures and alpha test
//! resolved (TexGen's `LoadScene`).

use lodforge_nif::{Block, Nif, TriShape};

#[derive(Debug, Clone, Default)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub normals: Vec<[f32; 3]>,
    /// Per-vertex tangent frame as stored in the NIF (model space; empty when the shape has none).
    /// Skyrim's "tangent" runs along texture v and its "bitangent" along u.
    pub tangents: Vec<[f32; 3]>,
    pub bitangents: Vec<[f32; 3]>,
    /// RGBA 0..1 (white when the shape has none).
    pub colors: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
    /// Diffuse and normal map paths (Data-relative, lower-case, `textures\...`).
    pub diffuse: String,
    pub normal: String,
    /// Glow / soft-light mask (texture set slot 2), when the shader uses one.
    pub glow: String,
    /// Texture-set slots 0..2 as stored in the NIF (original case, empty slots and repeats left out),
    /// for the `.txt` `Texture_k` list.
    pub texture_names: Vec<String>,
    /// NiAlphaProperty alpha-test threshold (0..255) when alpha testing is on.
    pub alpha_test: Option<u8>,
    pub double_sided: bool,
    /// The shape's BSLightingShaderProperty (material values for shading).
    pub shader: Option<lodforge_nif::LightingShader>,
    /// ENB complex grass texture: diffuse in the top half, normal map in the bottom half.
    pub complex: bool,
}

/// Which parts of a NIF are drawn.
#[derive(Debug, Clone, Copy, Default)]
pub struct LoadOptions {
    /// Draw skinned shapes (vertex data from the NiSkinPartition, bind pose).
    pub skinned: bool,
    /// Draw only the active child of NiSwitchNode / NiLODNode (else every child).
    pub active_switch_child: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Scene {
    pub meshes: Vec<Mesh>,
    pub opts: LoadOptions,
}

type M3 = [[f32; 3]; 3];

fn mul3(a: &M3, b: &M3) -> M3 {
    let mut m = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            m[i][j] = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
        }
    }
    m
}

fn apply3(m: &M3, v: [f32; 3]) -> [f32; 3] {
    [m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2], m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2], m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2]]
}

#[derive(Clone, Copy)]
struct Xf {
    rot: M3,
    trans: [f32; 3],
    scale: f32,
}

impl Xf {
    const ID: Xf = Xf { rot: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]], trans: [0.0; 3], scale: 1.0 };

    /// parent ∘ child (NIF: v_parent = R * (s * v) + t).
    fn then(&self, child: &lodforge_nif::Transform) -> Xf {
        let t = apply3(&self.rot, child.translation);
        Xf {
            rot: mul3(&self.rot, &child.rotation),
            trans: [self.trans[0] + t[0] * self.scale, self.trans[1] + t[1] * self.scale, self.trans[2] + t[2] * self.scale],
            scale: self.scale * child.scale,
        }
    }

    fn point(&self, v: [f32; 3]) -> [f32; 3] {
        let r = apply3(&self.rot, v);
        [r[0] * self.scale + self.trans[0], r[1] * self.scale + self.trans[1], r[2] * self.scale + self.trans[2]]
    }
}

fn tex_path(t: &str) -> String {
    let t = t.replace('/', "\\").to_ascii_lowercase();
    let t = t.trim_start_matches('\\');
    let t = match t.find("textures\\") {
        Some(p) => &t[p..],
        None => t,
    };
    if t.is_empty() || t.starts_with("textures\\") { t.to_string() } else { format!("textures\\{t}") }
}

/// A texture-set entry as TexGen names it in the `.txt`: original case, from the `textures\`
/// folder on (a leading `SkyrimHD\build\PC\data\` and the like is dropped).
fn tex_name(t: &str) -> Option<String> {
    let t = t.replace('/', "\\");
    let t = t.trim_start_matches('\\');
    if t.is_empty() {
        return None;
    }
    Some(match t.to_ascii_lowercase().find("textures\\") {
        Some(p) => t[p..].to_string(),
        None => format!("textures\\{t}"),
    })
}

impl Scene {
    pub fn from_nif(nif: &Nif) -> Scene {
        Scene::load(nif, LoadOptions::default())
    }

    pub fn load(nif: &Nif, opts: LoadOptions) -> Scene {
        let mut s = Scene { opts, ..Default::default() };
        if matches!(nif.blocks.first(), Some(Block::Node(_))) {
            s.walk(nif, 0, Xf::ID);
        }
        s
    }

    fn walk(&mut self, nif: &Nif, idx: usize, parent: Xf) {
        match nif.blocks.get(idx) {
            Some(Block::Node(n)) => {
                if n.av.flags & 1 != 0 {
                    return;
                }
                let x = if idx == 0 { parent } else { parent.then(&n.av.transform) };
                // NiSwitchNode / NiLODNode: only the active child.
                let active = if self.opts.active_switch_child { nif.switch_index(idx) } else { None };
                for (k, c) in n.children.iter().enumerate() {
                    let Some(c) = c else { continue };
                    if active.is_none_or(|a| a as usize == k) {
                        self.walk(nif, *c as usize, x);
                    }
                }
            }
            Some(Block::TriShape(t)) => {
                if t.av.flags & 1 != 0 {
                    return;
                }
                // Skinned SSE shapes keep their vertices in the NiSkinPartition.
                let skinned;
                let t = if t.positions.is_empty() && t.skin.is_some() {
                    if !self.opts.skinned {
                        return;
                    }
                    let Some(g) = nif.skin_geometry(t) else { return };
                    skinned = TriShape { positions: g.positions, uvs: g.uvs, normals: g.normals, colors: g.colors, triangles: g.triangles, ..t.clone() };
                    &skinned
                } else {
                    t
                };
                if t.positions.is_empty() || t.uvs.is_empty() {
                    return;
                }
                if t.av.net.name.as_deref().is_some_and(|n| n.to_ascii_lowercase().contains("editormarker")) {
                    return;
                }
                let x = parent.then(&t.av.transform);
                let mut m = Mesh {
                    positions: t.positions.iter().map(|p| x.point(*p)).collect(),
                    uvs: t.uvs.clone(),
                    normals: if t.normals.len() == t.positions.len() { t.normals.iter().map(|n| apply3(&x.rot, *n)).collect() } else { vec![[0.0, 0.0, 1.0]; t.positions.len()] },
                    tangents: if t.tangents.len() == t.positions.len() { t.tangents.iter().map(|v| apply3(&x.rot, *v)).collect() } else { Vec::new() },
                    bitangents: if t.bitangents.len() == t.positions.len() { t.bitangents.iter().map(|v| apply3(&x.rot, *v)).collect() } else { Vec::new() },
                    colors: Vec::new(),
                    indices: t.triangles.iter().flat_map(|tr| tr.map(u32::from)).collect(),
                    ..Default::default()
                };
                let mut use_vc = false;
                match nif.get(t.shader_property) {
                    Some(Block::LightingShader(l)) => {
                        use_vc = l.flags2 & 0x20 != 0;
                        m.shader = Some(l.clone());
                        m.double_sided = l.flags2 & 0x10 != 0;
                        if let Some(Block::TextureSet(ts)) = nif.get(l.texture_set) {
                            m.diffuse = ts.first().map(|v| tex_path(v)).unwrap_or_default();
                            m.normal = ts.get(1).map(|v| tex_path(v)).unwrap_or_default();
                            m.glow = ts.get(2).map(|v| tex_path(v)).unwrap_or_default();
                            // Slots TexGen loads: diffuse, normal, glow, back light.
                            for v in [0, 1, 2, 7].iter().filter_map(|&k| ts.get(k)) {
                                let Some(name) = tex_name(v) else { continue };
                                if !m.texture_names.iter().any(|t| t.eq_ignore_ascii_case(&name)) {
                                    m.texture_names.push(name);
                                }
                            }
                        }
                        if l.uv_scale != [1.0, 1.0] || l.uv_offset != [0.0, 0.0] {
                            for uv in &mut m.uvs {
                                uv[0] = uv[0] * l.uv_scale[0] + l.uv_offset[0];
                                uv[1] = uv[1] * l.uv_scale[1] + l.uv_offset[1];
                            }
                        }
                    }
                    Some(Block::EffectShader(_)) => return,
                    _ => {}
                }
                if let Some(Block::Alpha { flags, threshold, .. }) = nif.get(t.alpha_property)
                    && flags & 0x200 != 0
                {
                    m.alpha_test = Some(*threshold);
                }
                let n = m.positions.len();
                m.colors = if use_vc && t.legacy && t.colors_f32.len() == n {
                    t.colors_f32.clone()
                } else if use_vc && t.colors.len() == n {
                    t.colors.iter().map(|c| c.map(|b| f32::from(b) / 255.0)).collect()
                } else {
                    vec![[1.0; 4]; n]
                };
                // TexGen ignores vertex alpha even with the Vertex_Alpha flag: trees keep wind weights
                // there (0 on whole trunks), and the golden billboards are drawn without it.
                for c in &mut m.colors {
                    c[3] = 1.0;
                }
                if !m.diffuse.is_empty() && !m.indices.is_empty() {
                    self.meshes.push(m);
                }
            }
            _ => {}
        }
    }

    /// (min, max) of all vertices.
    pub fn bounds(&self) -> Option<([f32; 3], [f32; 3])> {
        let mut it = self.meshes.iter().flat_map(|m| m.positions.iter());
        let first = *it.next()?;
        Some(it.fold((first, first), |(mut lo, mut hi), p| {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
            (lo, hi)
        }))
    }
}
