//! The object LOD builder (docs/re/object-lod-spec.md 2, 3.3): references placed by their
//! transform into block-local, level-scaled coordinates, grouped by [`Material`] into one shape
//! per material, split before 65,535 vertices or triangles, with one triangle range per cell at
//! LOD4 (segments) and a single range above.
//!
//! The chunk accumulation (transform each part, copy only the vertices its surviving triangles
//! use, one index list per cell, split at the index limit) follows AnvilLOD
//! `AnvilLOD.Meshes/BtoBuilder.cs` (Copyright (c) 2026 CageTV, GPL-3.0-only, commit 606eece).
//! Modified: the material key, the shape names and the HD / passthru rules come from
//! [`crate::material`] (AnvilLOD's "HD at LOD4 only" rule and NiAlphaProperty are not used), the
//! export file is the input instead of plugin records, the terrain test and the LOD inset are
//! left out (hidden-face removal is stage B2.3), and the NIF is assembled by [`crate::bto`].

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use lodforge_nif::{Nif, Segment};
use rayon::prelude::*;

use crate::atlas_map::AtlasMap;
use crate::export::{Dialect, ExportFile, StaticRef};
use crate::flat::FlatList;
use crate::grid::{LEVELS, LodGrid, Quad};
use crate::hidden::{HiddenRule, Samples, Terrain, TerrainKind, TerrainView};
use crate::material::{Context, HdRules, Material, MaterialClass, is_passthru_model};
use crate::mesh::{LodMesh, MeshPart, ShaderSource};
use crate::refs::{self, Buckets};
use crate::sidefiles::{AltTextures, VcList, VcVerdict};
use crate::stats::Stats;
use crate::transform::{Mat3, Xform, normalize};

/// Largest vertex / triangle count of one `BSTriShape` (u16 counts and indices).
pub const MAX_PER_SHAPE: usize = u16::MAX as usize;

/// What `IgnoreTranslation` means for a listed LOD mesh (spec 2.2, open question 7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum IgnoreMode {
    /// The mesh's own node and shape transforms are ignored (vertices as stored); the reference
    /// transform applies as usual. Settled on the golden Solitude quad: the other readings put
    /// the listed meshes at the world origin.
    #[default]
    MeshTransforms,
    /// The reference's translation is not applied (rotation and scale are).
    ReferenceTranslation,
    /// The reference transform is not applied at all (the mesh is in world coordinates).
    ReferenceTransform,
    /// The setting is ignored.
    Off,
}

/// Build options; [`Options::from_export`] derives them from the header keys of a DynDOLOD
/// export and from xLODGen's documented defaults.
#[derive(Clone)]
pub struct Options<'a> {
    pub dialect: Dialect,
    pub hd: HdRules,
    /// `PassThruMeshMask` entries, lower case.
    pub passthru_masks: Vec<String>,
    /// `IgnoreTranslation` LOD meshes, lower case.
    pub ignore_translation: Vec<String>,
    pub ignore_mode: IgnoreMode,
    /// Build LOD32 files (`Level32`).
    pub level32: bool,
    /// `RemoveUnseenFaces`: drop the triangles [`Options::hidden_rule`] finds hidden under
    /// [`Options::terrain`]; without a terrain source nothing is removed.
    pub remove_unseen_faces: bool,
    pub hidden_rule: HiddenRule,
    pub terrain: Option<Arc<Terrain<'a>>>,
    pub alt_textures: AltTextures,
    pub vc: VcList,
    pub flat: FlatList,
    /// Grass LOD inputs (LOD4 only); `None` for no grass.
    pub grass: Option<Arc<crate::grass::GrassInputs>>,
    /// Rasterize the LOD4 shapes into the object height map (`CreateHeightMap`).
    pub height_map: bool,
    /// `AtlasTolerance`: a shape is remapped into its atlas tile when every UV lies within
    /// `[-tolerance, 1 + tolerance]` (spec 5).
    pub atlas_tolerance: f32,
    /// Re-cut tiled shapes along integer UV lines so they fit a mini-atlas tile (10-column
    /// rows); shapes named `noreuv` are exempt. Off by default: LODGen leaves such shapes on
    /// their own texture and asks DynDOLOD for a tiled mini atlas instead (`MiniAtlasExport`,
    /// spec 7.4), which `lodforge_dyndolod::sidefiles::mini_atlas_requests` reproduces.
    pub reuv: bool,
    /// A shape remapped into an atlas tile whose UVs leave 0..1 (by up to `atlas_tolerance`) is
    /// re-cut at the tile edges first, so every piece samples its own tile instead of reaching
    /// into the neighbouring tiles on the page. LODGen remaps such shapes as they are.
    pub recut_overshoot: bool,
}

/// UV slack a shape may have past its atlas tile without being re-cut (half a texel of a
/// 256-px tile).
pub const ATLAS_UV_SLACK: f32 = 0.002;

impl Default for Options<'_> {
    fn default() -> Self {
        Options {
            dialect: Dialect::default(),
            hd: HdRules::default(),
            passthru_masks: Vec::new(),
            ignore_translation: Vec::new(),
            ignore_mode: IgnoreMode::default(),
            level32: false,
            remove_unseen_faces: false,
            hidden_rule: DEFAULT_HIDDEN_RULE,
            terrain: None,
            alt_textures: AltTextures::default(),
            vc: VcList::default(),
            flat: FlatList::default(),
            atlas_tolerance: 0.5,
            reuv: false,
            recut_overshoot: true,
            grass: None,
            height_map: false,
        }
    }
}

/// The hidden-face rule settled on the golden files (spec section 12.1): the terrain LOD mesh
/// of the quad being built, a triangle is hidden when each of its three corners is below it or
/// outside it, no margin; triangles entirely under the cell water are dropped too.
pub const DEFAULT_HIDDEN_RULE: HiddenRule = HiddenRule { terrain: TerrainKind::Btr, samples: Samples::Vertices, shift: 0.0, water: true, outside_hidden: false };

impl<'a> Options<'a> {
    pub fn from_export(e: &ExportFile) -> Options<'a> {
        let lower = |v: Vec<&str>| v.into_iter().map(|s| s.trim().to_ascii_lowercase()).filter(|s| !s.is_empty()).collect::<Vec<_>>();
        match e.dialect {
            Dialect::XLodgen => Options {
                dialect: e.dialect,
                hd: HdRules { enabled: true, exact: e.hd_textures.iter().filter(|t| !t.ends_with("_n.dds")).cloned().collect(), is_hd: Vec::new(), not_hd: Vec::new(), mesh_masks: Vec::new() },
                passthru_masks: Vec::new(),
                ignore_translation: e.ignore_translation.clone(),
                ignore_mode: IgnoreMode::MeshTransforms,
                level32: false,
                remove_unseen_faces: true,
                flat: e.flat_textures.iter().fold(FlatList::default(), |mut l, line| {
                    l.push_line(line);
                    l
                }),
                atlas_tolerance: e.atlas_tolerance,
                reuv: false,
                grass: None,
                height_map: false,
                ..Default::default()
            },
            Dialect::Dyndolod => Options {
                dialect: e.dialect,
                hd: HdRules { enabled: e.flag("UseHDFlag", true), exact: Vec::new(), is_hd: lower(e.settings_all("IsHDTextureMask")), not_hd: lower(e.settings_all("NotHDTextureMask")), mesh_masks: lower(e.settings_all("IsHDMeshMask")) },
                passthru_masks: lower(e.settings_all("PassThruMeshMask")),
                ignore_translation: e.ignore_translation.clone(),
                ignore_mode: IgnoreMode::MeshTransforms,
                level32: e.flag("Level32", false),
                height_map: e.flag("CreateHeightMap", false),
                remove_unseen_faces: e.flag("RemoveUnseenFaces", true),
                // `RemoveUnseenZShift` is the margin of the test (10 in DynDOLOD exports; the
                // DynDOLOD golden keeps 83,000 triangles a zero margin would drop).
                hidden_rule: HiddenRule { shift: e.number("RemoveUnseenZShift").unwrap_or(0.0), ..DEFAULT_HIDDEN_RULE },
                atlas_tolerance: e.atlas_tolerance,
                reuv: false,
                grass: None,
                ..Default::default()
            },
        }
    }

    /// The levels to build: 4, 8, 16 and, with `level32`, 32.
    pub fn levels(&self) -> Vec<i32> {
        LEVELS.iter().copied().filter(|&l| l != 32 || self.level32).collect()
    }
}

/// One built file.
pub struct BuiltQuad {
    pub quad: Quad,
    pub nif: Nif,
    pub shapes: usize,
    pub triangles: usize,
    pub vertices: usize,
    /// The object height map cells this LOD4 quad's shapes raise (`Options::height_map`).
    pub heights: Option<crate::heightmap::Cells>,
}

/// The builder of one worldspace / season.
pub struct ObjectLod<'a> {
    pub export: &'a ExportFile,
    pub atlas: &'a AtlasMap,
    pub opts: Options<'a>,
    pub grid: LodGrid,
    pub stats: Stats,
    pub buckets: Buckets,
    /// Model path (lower case) -> the model, `None` when missing or unreadable.
    meshes: HashMap<String, Option<Arc<LodMesh>>>,
    /// Import warnings (one line each) and the paths that could not be read.
    pub warnings: Vec<String>,
    pub missing_models: Vec<String>,
    /// `FlatLODLevelLODFlag`: billboards get `LOD_Objects` from this level index on (1 = LOD4,
    /// 2 = LOD8, 3 = LOD16; 0 = never).
    flat_lod_flag: usize,
    /// `Crown / Trunk / FlatTrunk Brightness R/G/B`: multipliers of the vertex colours of 3D tree
    /// LOD shapes named so (spec 3.2).
    brightness: [[f32; 3]; 3],
}

/// What makes a merged shape coloured (see the vertex-colour note in `ObjectLod::build_quad`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ColorRule {
    /// Some part has a vertex whose RGB is not white.
    AnyColor,
    /// Some part has a colour array at all.
    AnyArray,
}

impl<'a> ObjectLod<'a> {
    /// Buckets the references and loads every LOD model the built levels name through `read`
    /// (a Data-relative path, lower case, `meshes\...`).
    pub fn new(export: &'a ExportFile, atlas: &'a AtlasMap, opts: Options<'a>, grid: LodGrid, read: &(dyn Fn(&str) -> Option<Vec<u8>> + Sync)) -> ObjectLod<'a> {
        let levels = opts.levels();
        let buckets = refs::bucket(export, &grid, &levels);
        let mut wanted: Vec<String> = Vec::new();
        let mut seen: HashMap<String, ()> = HashMap::new();
        for refs in buckets.quads.values() {
            for &i in refs {
                for (k, &level) in LEVELS.iter().enumerate() {
                    if !levels.contains(&level) {
                        continue;
                    }
                    if let Some(m) = export.statics[i].model(k) {
                        let l = m.to_ascii_lowercase().replace('/', "\\");
                        if !l.ends_with(".dds") && seen.insert(l.clone(), ()).is_none() {
                            wanted.push(l);
                        }
                    }
                }
            }
        }
        let no_node_transforms = opts.ignore_mode == IgnoreMode::MeshTransforms;
        let loaded: Vec<(String, Option<Arc<LodMesh>>, Vec<String>, bool)> = wanted
            .par_iter()
            .map(|p| match read(p) {
                Some(bytes) => match crate::mesh::import_with(p, &bytes, !(no_node_transforms && opts.ignore_translation.iter().any(|m| m == p))) {
                    Ok(m) => {
                        let w = m.warnings.clone();
                        (p.clone(), Some(Arc::new(m)), w, false)
                    }
                    Err(e) => (p.clone(), None, vec![format!("{p}: {e}")], true),
                },
                None => (p.clone(), None, Vec::new(), true),
            })
            .collect();
        let mut meshes = HashMap::with_capacity(loaded.len());
        let mut warnings = Vec::new();
        let mut missing_models = Vec::new();
        for (p, m, w, missing) in loaded {
            warnings.extend(w);
            if missing {
                missing_models.push(p.clone());
            }
            meshes.insert(p, m);
        }
        missing_models.sort();
        let flat_lod_flag = export.number("FlatLODLevelLODFlag").unwrap_or(0.0).max(0.0) as usize;
        let brightness = ["Crown", "Trunk", "FlatTrunk"].map(|n| ["R", "G", "B"].map(|c| export.number(&format!("{n}Brightness{c}")).or_else(|| export.number(&format!("{n}Brightness"))).unwrap_or(1.0).clamp(0.0, 1.0)));
        ObjectLod { export, atlas, opts, grid, stats: Stats::default(), buckets, meshes, warnings, missing_models, flat_lod_flag, brightness }
    }

    pub fn levels(&self) -> Vec<i32> {
        self.opts.levels()
    }

    /// Every quad with at least one reference, in (level, x, y) order.
    pub fn quads(&self) -> Vec<Quad> {
        self.buckets.quads.keys().copied().collect()
    }

    /// Whether a quad of a season export is a copy of the default season (spec 1.4).
    pub fn season_copy(&self, q: &Quad) -> bool {
        refs::is_season_copy(self.export, q)
    }

    pub fn mesh(&self, model: &str) -> Option<&Arc<LodMesh>> {
        self.meshes.get(&model.to_ascii_lowercase().replace('/', "\\")).and_then(|m| m.as_ref())
    }

    /// Builds the given quads in parallel; quads that end up without geometry are left out.
    pub fn build_level(&self, quads: &[Quad]) -> Vec<BuiltQuad> {
        quads.par_iter().filter_map(|q| self.build_quad(q)).collect()
    }

    /// Builds one quad.
    pub fn build_quad(&self, q: &Quad) -> Option<BuiltQuad> {
        let refs = self.buckets.quads.get(q)?;
        let k = crate::grid::level_index(q.level);
        let origin = q.origin();
        let inv = 1.0 / q.level as f32;
        let segments = if q.level == 4 { 16 } else { 1 };
        let mut chunks: BTreeMap<String, Vec<Chunk>> = BTreeMap::new();
        let (mut placed, mut missing, mut billboards, mut billboards_missing, mut alt_used, mut total_tris, mut culled) = (0usize, 0usize, 0usize, 0usize, 0usize, 0usize, 0usize);
        let (mut atlased, mut reuv_parts, mut vc_white, mut overshoot_parts) = (0usize, 0usize, 0usize, 0usize);
        let view = if self.opts.remove_unseen_faces { self.opts.terrain.as_deref().map(|t| t.view(q)) } else { None };
        let rule = self.opts.hidden_rule;
        for &ri in refs {
            let r: &StaticRef = &self.export.statics[ri];
            let Some(model) = r.model(k) else { continue };
            let model_l = model.to_ascii_lowercase().replace('/', "\\");
            if model_l.ends_with(".dds") {
                // A tree billboard (spec 6.1, `billboard`): one shape of two crossed quads.
                match self.opts.flat.get(&model_l) {
                    Some(ft) => {
                        billboards += 1;
                        let seg = if q.level == 4 { q.segment_of(r.cell()) } else { 0 };
                        let lod_flag = self.flat_lod_flag > 0 && crate::grid::level_index(q.level) + 1 >= self.flat_lod_flag;
                        let b = crate::billboard::build(r, ft, &model_l, self.atlas, lod_flag);
                        let list = chunks.entry(b.material.key.clone()).or_default();
                        if list.last().is_none_or(|c| !c.fits(8, 4)) {
                            if !list.is_empty() {
                                Stats::add(&self.stats.splits, 1);
                            }
                            list.push(Chunk::new(b.material, segments, ColorRule::AnyArray));
                        }
                        culled += list.last_mut().unwrap().append(&b.part, &Xform::IDENTITY, &Mat3::IDENTITY, origin, inv, seg, view.as_ref().map(|v| (v, &rule)), None);
                        total_tris += 4;
                    }
                    None => billboards_missing += 1,
                }
                continue;
            }
            let Some(mesh) = self.mesh(&model_l) else {
                missing += 1;
                continue;
            };
            if mesh.parts.is_empty() {
                continue;
            }
            placed += 1;
            let mut xform = Xform::reference([r.x, r.y, r.z], [r.rot_x, r.rot_y, r.rot_z], r.scale);
            if self.opts.ignore_translation.iter().any(|m| *m == model_l) {
                xform = match self.opts.ignore_mode {
                    IgnoreMode::ReferenceTranslation => xform.without_translation(),
                    IgnoreMode::ReferenceTransform => Xform::IDENTITY,
                    IgnoreMode::MeshTransforms | IgnoreMode::Off => xform,
                };
            }
            let rot = xform.rotation_only();
            let class = MaterialClass::parse(r.material());
            let passthru = class == MaterialClass::PassThru || is_passthru_model(&model_l, &self.opts.passthru_masks);
            let model_hd = self.opts.hd.mesh_masks.iter().any(|m| model_l.contains(m.as_str()));
            let ctx = Context { class: class.clone(), large_ref: r.is_large_ref(), level: q.level, passthru, hd: &self.opts.hd, model_hd };
            let seg = if q.level == 4 { q.segment_of(r.cell()) } else { 0 };
            for part in &mesh.parts {
                let alt = self.opts.alt_textures.textures_for(&r.ref_id, part.index, &part.name);
                if alt.is_some() {
                    alt_used += 1;
                }
                let textures = alt.unwrap_or(&part.textures[..]);
                // Atlas remap (spec 5): the shape's texture key has a row and its UVs fit the
                // tolerance; a tiled shape with a mini-atlas row is re-cut first.
                let glow_slot = match &part.shader {
                    ShaderSource::Lighting(l) if l.flags2 & crate::material::SLSF2_GLOW_MAP != 0 => textures.get(2).map(String::as_str).unwrap_or(""),
                    _ => "",
                };
                let tol = self.opts.atlas_tolerance;
                let normal_slot = textures.get(1).map(String::as_str).unwrap_or("");
                let alpha = AtlasMap::alpha_suffix(part.alpha);
                // HD shapes keep their full texture (no HD shape sits on an atlas page in the
                // DynDOLOD golden2; `objHD-LargeRef` on `mountainslab02linear.dds` in 4.-20.0).
                let hd_part = crate::material::is_hd(&ctx, textures);
                let mut remap = if hd_part { None } else { self.atlas.resolve(&textures[0], normal_slot, glow_slot, alpha, &part.uvs, tol) };
                // A passthru shape takes one row only (golden2 `Tamriel.4.-28.16.bto`: the
                // passthru `farmwindowinterior01lod.dds` shape stays on TexGen's LOD texture
                // although the map chains it onto the big page).
                if passthru && let Some(m) = &mut remap {
                    m.rows.truncate(1);
                }
                let recut: Option<MeshPart>;
                let unit_scale = remap.as_ref().is_some_and(|m| m.rows[0].scale.is_none_or(|s| s == (1.0, 1.0)));
                let part = if remap.is_none() && self.opts.reuv && self.atlas.lookup(&textures[0], normal_slot, glow_slot, alpha).is_some() && !part.name.to_ascii_lowercase().contains("noreuv") {
                    recut = Some(crate::reuv::recut(part));
                    reuv_parts += 1;
                    let p = recut.as_ref().unwrap();
                    remap = self.atlas.resolve(&textures[0], normal_slot, glow_slot, alpha, &p.uvs, tol);
                    p
                } else if remap.is_some() && self.opts.recut_overshoot && unit_scale && crate::reuv::needs_recut(&part.uvs, ATLAS_UV_SLACK) {
                    // Overshooting the tile: cut at the tile edges, every piece wraps into 0..1.
                    recut = Some(crate::reuv::recut(part));
                    let p = recut.as_ref().unwrap();
                    match self.atlas.resolve(&textures[0], normal_slot, glow_slot, alpha, &p.uvs, tol) {
                        Some(mut m) => {
                            if passthru {
                                m.rows.truncate(1);
                            }
                            remap = Some(m);
                            overshoot_parts += 1;
                            p
                        }
                        None => part,
                    }
                } else {
                    part
                };
                let row = remap.as_ref().map(|r| r.last());
                if row.is_some() {
                    atlased += 1;
                }
                // Vertex colours (spec 3.2 / 7.3, open question 4; settled on the DynDOLOD
                // golden2 by the LODFORGE_TRACE_GOLDEN study, spec 12.2): the VC side file
                // whitens the LOD shapes it lists `False` (RGB 255, alpha kept: the golden keeps
                // alpha 96 / 101 / 127 under white RGB); unlisted shapes keep their colours. A
                // part "has colour" when some RGB is not white after that; it "has an array"
                // when the source has one. At LOD 4 the merged shape decides (`Chunk::finish`):
                // a plain shape or an atlased snow shape is coloured when some part has colour,
                // a snow / ash shape on its own texture when some part has an array (the HD snow
                // mountains are white with alpha), and the parts without get opaque white. At
                // LOD 8 and 16 the golden does not merge: colour presence is part of the key,
                // per part (plain / atlased / HD / LOD 16: has colour; snow on its own texture at
                // LOD 8: has an array), and the parts without lose their array.
                let source: &MeshPart = part;
                let whitened: Option<MeshPart>;
                let part = match &part.colors {
                    Some(c) if self.opts.vc.verdict(&r.full_model, &part.name) == VcVerdict::White => {
                        vc_white += 1;
                        whitened = Some(MeshPart { colors: Some(c.iter().map(|v| [255, 255, 255, v[3]]).collect()), ..part.clone() });
                        whitened.as_ref().unwrap()
                    }
                    _ => part,
                };
                // Tree LOD brightness (spec 3.2): `Crown / Trunk / FlatTrunk Brightness` multiply
                // the colours of shapes named so; passthru shapes write alpha 255 (golden2
                // `fxwatertile01` / `fxwhitewater01` passthru shapes: alpha 255 where the source
                // has 58 / 229).
                let adjusted: Option<MeshPart>;
                let part = match &part.colors {
                    Some(c) => {
                        let lower = part.name.to_ascii_lowercase();
                        let mul = if lower.contains("flattrunk") {
                            Some(self.brightness[2])
                        } else if lower.contains("trunk") {
                            Some(self.brightness[1])
                        } else if lower.contains("crown") {
                            Some(self.brightness[0])
                        } else {
                            None
                        }
                        .filter(|m| *m != [1.0; 3]);
                        let opaque = passthru && c.iter().any(|v| v[3] != 255);
                        if mul.is_some() || opaque {
                            let m = mul.unwrap_or([1.0; 3]);
                            let scale = |v: u8, f: f32| ((f32::from(v) * f).round() as i32).clamp(0, 255) as u8;
                            adjusted = Some(MeshPart { colors: Some(c.iter().map(|v| [scale(v[0], m[0]), scale(v[1], m[1]), scale(v[2], m[2]), if opaque { 255 } else { v[3] }]).collect()), ..part.clone() });
                            adjusted.as_ref().unwrap()
                        } else {
                            part
                        }
                    }
                    None => part,
                };
                let mut material = Material::for_part(part, textures, &ctx, row);
                let stripped: Option<MeshPart>;
                let part = if q.level > 4 {
                    let has_color = part.colors.as_ref().is_some_and(|c| c.iter().any(|v| v[0] != 255 || v[1] != 255 || v[2] != 255));
                    let own_snow = class.is_snow_or_ash() && row.is_none() && !material.hd && q.level == 8;
                    let presence = if own_snow { part.colors.is_some() } else { has_color };
                    material.key.push_str(if presence { "|c1" } else { "|c0" });
                    if presence || part.colors.is_none() {
                        part
                    } else {
                        stripped = Some(MeshPart { colors: None, ..part.clone() });
                        stripped.as_ref().unwrap()
                    }
                } else {
                    part
                };
                if std::env::var_os("LODFORGE_TRACE_COLORS").is_some() {
                    // Per part: the source colour array (before whitening), the VC verdict, the
                    // source shader's vertex-colour / vertex-alpha bits, our decision.
                    let src = source.colors.as_ref();
                    let non_white = src.map(|c| c.iter().filter(|c| c[0] != 255 || c[1] != 255 || c[2] != 255).count()).unwrap_or(0);
                    let (amin, amax) = src.map(|c| c.iter().fold((255u8, 0u8), |(lo, hi), v| (lo.min(v[3]), hi.max(v[3])))).unwrap_or((0, 0));
                    let (f1, f2) = match &source.shader {
                        crate::mesh::ShaderSource::Lighting(l) => (l.flags1, l.flags2),
                        crate::mesh::ShaderSource::Effect(e) => (e.flags1, e.flags2),
                    };
                    let n = part.positions.len();
                    let sample = |i: usize| {
                        let i = i.min(n.saturating_sub(1));
                        let w = xform.apply(part.positions[i]);
                        let c = source.colors.as_ref().map_or("-".to_string(), |c| format!("{},{},{},{}", c[i][0], c[i][1], c[i][2], c[i][3]));
                        format!("{:.3},{:.3},{:.3}/{c}", w[0], w[1], w[2])
                    };
                    // With LODFORGE_TRACE_GOLDEN=<objects dir>: the golden's colour kind at every
                    // vertex of the part (none / white / walpha / rgb / miss), majority shape name.
                    let gold = std::env::var_os("LODFORGE_TRACE_GOLDEN").map(|dir| {
                        let map = golden_map(std::path::Path::new(&dir), &format!("{}.{}.{}.{}.bto", self.export.worldspace, q.level, q.x, q.y));
                        let mut counts = [0usize; 5];
                        let mut names: HashMap<&str, usize> = HashMap::new();
                        for p in &part.positions {
                            let w = xform.apply(*p);
                            match golden_lookup(&map, w) {
                                Some((name, c)) => {
                                    *names.entry(name).or_default() += 1;
                                    counts[match c {
                                        None => 0,
                                        Some([255, 255, 255, 255]) => 1,
                                        Some([255, 255, 255, _]) => 2,
                                        Some(_) => 3,
                                    }] += 1;
                                }
                                None => counts[4] += 1,
                            }
                        }
                        let top = names.iter().max_by_key(|(_, n)| **n).map(|(k, _)| k.to_string()).unwrap_or_default();
                        format!("\tgold=none:{},white:{},walpha:{},rgb:{},miss:{}\tgshape={}", counts[0], counts[1], counts[2], counts[3], counts[4], top)
                    });
                    eprintln!(
                        "trace-colors\t{}.{}.{}\t{}\t{}\t{:?}\tlegacy={}\tsrc={}\tnon_white={}/{}\talpha={}..{}\tvc={:?}\tvcol={}\tvalpha={}\tclass={:?}\t{}\tcol={}\ttex={}\tverts={}\ttris={}\tpage={}\tp0={}\tp1={}\tp2={}{}",
                        q.level, q.x, q.y, r.ref_id, model_l, source.name, source.legacy, src.is_some(), non_white, src.map_or(0, Vec::len), amin, amax,
                        self.opts.vc.verdict(&r.full_model, &source.name), f2 & 0x20 != 0, f1 & 0x8 != 0, class, material.name, part.colors.is_some(), textures[0],
                        n, part.triangles.len(), row.map_or("own", |_| "atlas"), sample(0), sample(n / 2), sample(n.saturating_sub(1)), format!("{}	key={}", gold.unwrap_or_default(), material.key)
                    );
                }
                let list = chunks.entry(material.key.clone()).or_default();
                let needs_new = match list.last() {
                    Some(c) => !c.fits(part.positions.len(), part.triangles.len()),
                    None => true,
                };
                if needs_new {
                    if !list.is_empty() {
                        Stats::add(&self.stats.splits, 1);
                    }
                    list.push(Chunk::new(material, segments, if class.is_snow_or_ash() && row.is_none() { ColorRule::AnyArray } else { ColorRule::AnyColor }));
                }
                let chunk = list.last_mut().unwrap();
                culled += chunk.append(part, &xform, &rot, origin, inv, seg, view.as_ref().map(|v| (v, &rule)), remap.as_ref());
                total_tris += part.triangles.len();
            }
        }
        // Grass LOD (spec 6.2, `grass`): the cache instances of the quad's cells, one shape list
        // filled cell by cell, one segment per shape, split at the vertex limit.
        if q.level == 4
            && let Some(g) = &self.opts.grass
        {
            let mut grass_chunks: Vec<Chunk> = Vec::new();
            let (mut instances, mut unmapped) = (0usize, 0usize);
            for cy in q.y..q.y + 4 {
                for cx in q.x..q.x + 4 {
                    let Some(cell) = crate::grass::read_cell(g, (cx, cy)) else { continue };
                    for (gt, insts) in &cell {
                        instances += insts.len();
                        if gt.billboard_1.is_empty() {
                            unmapped += insts.len();
                            continue;
                        }
                        for part in crate::grass::build_type(g, self.atlas, gt, insts) {
                            if grass_chunks.last().is_none_or(|c| !c.fits(part.positions.len(), part.triangles.len())) {
                                grass_chunks.push(Chunk::new(crate::grass::material(self.atlas, &gt.billboard_1), 1, ColorRule::AnyArray));
                            }
                            grass_chunks.last_mut().unwrap().append(&part, &Xform::IDENTITY, &Mat3::IDENTITY, origin, inv, 0, None, None);
                            total_tris += part.triangles.len();
                        }
                    }
                }
            }
            Stats::add(&self.stats.grass_instances, instances);
            Stats::add(&self.stats.grass_unmapped, unmapped);
            if !grass_chunks.is_empty() {
                chunks.entry(format!("~grass|{}", crate::grass::NAME)).or_default().extend(grass_chunks);
            }
        }
        Stats::add(&self.stats.placed, placed);
        Stats::add(&self.stats.missing_meshes, missing);
        Stats::add(&self.stats.billboards, billboards);
        Stats::add(&self.stats.billboards_missing, billboards_missing);
        Stats::add(&self.stats.alt_textures, alt_used);
        Stats::add(&self.stats.total_tris[k], total_tris);
        Stats::add(&self.stats.culled_tris[k], culled);
        Stats::add(&self.stats.atlased_parts, atlased);
        Stats::add(&self.stats.reuv_parts, reuv_parts);
        Stats::add(&self.stats.overshoot_parts, overshoot_parts);
        Stats::add(&self.stats.vc_whitened, vc_white);
        let mut shapes: Vec<BuiltShape> = Vec::new();
        for list in chunks.into_values() {
            for c in list {
                if c.tri_count == 0 {
                    continue;
                }
                if c.material.passthru {
                    Stats::add(&self.stats.passthru, 1);
                }
                if c.material.hd {
                    Stats::add(&self.stats.hd_shapes, 1);
                }
                shapes.push(c.finish());
            }
        }
        // A quad whose references all lost their triangles still gets a file with the bare root
        // node (golden: `Tamriel.4.-20.40.bto`, 236 bytes).
        if shapes.is_empty() && placed == 0 {
            return None;
        }
        let triangles: usize = shapes.iter().map(|s| s.triangles.len()).sum();
        let vertices: usize = shapes.iter().map(|s| s.positions.len()).sum();
        Stats::add(&self.stats.reduced_tris[k], triangles);
        let nif = crate::bto::assemble(q, &shapes);
        let heights = (q.level == 4 && self.opts.height_map).then(|| {
            let mut cells = crate::heightmap::Cells::new();
            crate::heightmap::rasterize(&shapes, q.level, origin, &mut cells);
            cells
        });
        Some(BuiltQuad { quad: *q, nif, shapes: shapes.len(), triangles, vertices, heights })
    }
}

/// One output shape's geometry in block-local, level-scaled units.
pub struct BuiltShape {
    pub material: Material,
    pub positions: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    pub normals: Vec<[f32; 3]>,
    pub tangents: Vec<[f32; 3]>,
    pub bitangents: Vec<[f32; 3]>,
    pub colors: Option<Vec<[u8; 4]>>,
    /// Grouped by segment.
    pub triangles: Vec<[u16; 3]>,
    pub segments: Vec<Segment>,
}

struct Chunk {
    material: Material,
    positions: Vec<[f32; 3]>,
    uvs: Vec<[f32; 2]>,
    normals: Vec<[f32; 3]>,
    tangents: Vec<[f32; 3]>,
    bitangents: Vec<[f32; 3]>,
    colors: Vec<[u8; 4]>,
    /// How the shape's colour presence is decided (see the vertex-colour note in `build_quad`).
    rule: ColorRule,
    /// Any appended part had a colour array (the others' vertices are opaque white).
    has_array: bool,
    /// Any appended part had a vertex whose RGB is not white (after VC whitening).
    has_color: bool,
    seg_tris: Vec<Vec<[u16; 3]>>,
    tri_count: usize,
}

impl Chunk {
    fn new(material: Material, segments: usize, rule: ColorRule) -> Chunk {
        Chunk { material, positions: Vec::new(), uvs: Vec::new(), normals: Vec::new(), tangents: Vec::new(), bitangents: Vec::new(), colors: Vec::new(), rule, has_array: false, has_color: false, seg_tris: (0..segments).map(|_| Vec::new()).collect(), tri_count: 0 }
    }

    fn fits(&self, vertices: usize, triangles: usize) -> bool {
        self.positions.len() + vertices <= MAX_PER_SHAPE && self.tri_count + triangles <= MAX_PER_SHAPE
    }

    /// Appends the part's surviving triangles; returns how many were hidden under the terrain.
    #[allow(clippy::too_many_arguments)]
    fn append(&mut self, part: &MeshPart, xform: &Xform, rot: &Mat3, origin: [f32; 3], inv: f32, seg: usize, hidden: Option<(&TerrainView<'_, '_>, &HiddenRule)>, atlas: Option<&crate::atlas_map::Remap<'_>>) -> usize {
        let mut remap = vec![u32::MAX; part.positions.len()];
        let world: Vec<[f32; 3]> = part.positions.iter().map(|&p| xform.apply(p)).collect();
        let mut culled = 0usize;
        let mut any_kept = false;
        for tri in &part.triangles {
            if let Some((v, rule)) = hidden
                && v.hidden(rule, &[world[tri[0] as usize], world[tri[1] as usize], world[tri[2] as usize]])
            {
                culled += 1;
                continue;
            }
            any_kept = true;
            let mut out = [0u16; 3];
            for (k, &vi) in tri.iter().enumerate() {
                let i = vi as usize;
                if remap[i] == u32::MAX {
                    remap[i] = self.positions.len() as u32;
                    let w = world[i];
                    self.positions.push([(w[0] - origin[0]) * inv, (w[1] - origin[1]) * inv, (w[2] - origin[2]) * inv]);
                    self.uvs.push(match atlas {
                        Some(chain) => chain.apply(part.uvs[i]),
                        None => part.uvs[i],
                    });
                    self.normals.push(normalize(rot.apply(part.normals[i]), [0.0, 0.0, 1.0]));
                    self.tangents.push(normalize(rot.apply(part.tangents[i]), [1.0, 0.0, 0.0]));
                    self.bitangents.push(normalize(rot.apply(part.bitangents[i]), [0.0, 1.0, 0.0]));
                    // Every vertex gets a colour (opaque white where the part has none); the
                    // array is written only when some part of the group kept one.
                    self.colors.push(part.colors.as_ref().map(|c| c[i]).unwrap_or([255; 4]));
                }
                out[k] = remap[i] as u16;
            }
            self.seg_tris[seg].push(out);
            self.tri_count += 1;
        }
        if any_kept && let Some(c) = &part.colors {
            self.has_array = true;
            self.has_color |= c.iter().any(|v| v[0] != 255 || v[1] != 255 || v[2] != 255);
        }
        culled
    }

    /// The group's colour decision (see the vertex-colour note in `build_quad`); above LOD 4
    /// the parts without presence arrive stripped, so `has_array` is the per-part decision.
    fn colored(&self) -> bool {
        match self.rule {
            ColorRule::AnyArray => self.has_array,
            ColorRule::AnyColor => self.has_color || (self.material.key.ends_with("|c1") && self.has_array),
        }
    }

    fn finish(mut self) -> BuiltShape {
        let colored = self.colored();
        self.material.set_colors(colored);
        let mut triangles = Vec::with_capacity(self.tri_count);
        let mut segments = Vec::with_capacity(self.seg_tris.len());
        for tris in &self.seg_tris {
            let start = (triangles.len() * 3) as u32;
            segments.push(Segment { flags: 0, start: if tris.is_empty() { 0 } else { start }, count: tris.len() as u32 });
            triangles.extend_from_slice(tris);
        }
        while segments.len() > 1 && segments.last().is_some_and(|s| s.count == 0) {
            segments.pop();
        }
        let colors = colored.then_some(self.colors);
        BuiltShape { material: self.material, positions: self.positions, uvs: self.uvs, normals: self.normals, tangents: self.tangents, bitangents: self.bitangents, colors, triangles, segments }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export::StaticRef;
    use lodforge_nif::{AvObject, Block, LightingShader, Node, ObjectNet, Transform, TriShape};

    /// A one-triangle model with its shape under a translated node.
    fn model(colors: bool) -> Vec<u8> {
        let mut nif = Nif::new();
        nif.add_block(Block::Node(Node { av: AvObject { net: ObjectNet::named("Root"), ..Default::default() }, children: vec![Some(1)], ..Default::default() }));
        let mut s = TriShape {
            av: AvObject { net: ObjectNet::named("Shape"), transform: Transform { translation: [0.0, 0.0, 50.0], ..Default::default() }, ..Default::default() },
            positions: vec![[0.0, 0.0, 0.0], [100.0, 0.0, 0.0], [0.0, 100.0, 0.0]],
            uvs: vec![[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]],
            triangles: vec![[0, 1, 2]],
            shader_property: Some(2),
            ..Default::default()
        };
        if colors {
            s.colors = vec![[255, 0, 0, 255]; 3];
        }
        s.update_vertex_desc();
        nif.add_block(Block::TriShape(s));
        nif.add_block(Block::LightingShader(LightingShader { texture_set: Some(3), ..Default::default() }));
        nif.add_block(Block::TextureSet(vec!["textures\\d.dds".into(), "textures\\d_n.dds".into()]));
        nif.to_bytes().unwrap()
    }

    fn reference(id: &str, x: f32, y: f32, material: &str, models: [&str; 4]) -> StaticRef {
        let mut r = StaticRef { ref_id: id.into(), x, y, z: 0.0, scale: 1.0, material_name: material.into(), models: models.map(str::to_string), ..Default::default() };
        r.update_cell();
        r
    }

    #[test]
    fn builds_a_quad_with_segments_and_groups() {
        let mut e = ExportFile { south_west: (-96, -96), ..Default::default() };
        e.statics = vec![
            reference("00000001", 100.0, 100.0, "", ["meshes\\a.nif", "meshes\\a.nif", "", ""]),
            reference("00000002", 4096.0 * 3.0 + 10.0, 4096.0 + 10.0, "", ["meshes\\a.nif", "", "", ""]),
            reference("00000003", 200.0, 100.0, "Snow-LargeRef", ["meshes\\b.nif", "", "", ""]),
            reference("00000004", 200.0, 100.0, "", ["meshes\\missing.nif", "", "", ""]),
            reference("00000005", 200.0, 100.0, "", ["textures\\bb.dds", "", "", ""]),
        ];
        for (i, s) in e.statics.iter_mut().enumerate() {
            s.line = i;
        }
        let atlas = AtlasMap::default();
        let (a, b) = (model(false), model(true));
        let read = |p: &str| match p {
            "meshes\\a.nif" => Some(a.clone()),
            "meshes\\b.nif" => Some(b.clone()),
            _ => None,
        };
        let lod = ObjectLod::new(&e, &atlas, Options::from_export(&e), LodGrid::new((-96, -96), Some(256)), &read);
        assert_eq!(lod.missing_models, ["meshes\\missing.nif"]);
        let quads = lod.quads();
        assert_eq!(quads, [Quad { level: 4, x: 0, y: 0 }, Quad { level: 8, x: 0, y: 0 }]);
        let built = lod.build_level(&quads);
        assert_eq!(built.len(), 2);
        let q4 = &built[0];
        assert_eq!(q4.shapes, 2);
        assert_eq!(q4.triangles, 3);
        let shapes: Vec<&TriShape> = q4.nif.blocks.iter().filter_map(|b| if let Block::TriShape(s) = b { Some(s) } else { None }).collect();
        let obj = shapes.iter().find(|s| s.av.net.name.as_deref() == Some("obj")).unwrap();
        assert_eq!(obj.positions.len(), 6);
        assert_eq!(obj.triangles.len(), 2);
        // Reference 1 in cell (0,0) = segment 0, reference 2 in cell (3,1) = segment 13.
        let segs = obj.segments.as_ref().unwrap();
        assert_eq!(segs.len(), 14);
        assert_eq!((segs[0].start, segs[0].count), (0, 1));
        assert_eq!((segs[13].start, segs[13].count), (3, 1));
        assert!(segs[1..13].iter().all(|s| s.count == 0 && s.start == 0));
        // Block-local, divided by 4: the shape's node translation of 50 in z -> 12.5.
        assert_eq!(obj.positions[0], [25.0, 25.0, 12.5]);
        assert_eq!(obj.av.transform.scale, 4.0);
        assert!(obj.colors.is_empty());
        let snow = shapes.iter().find(|s| s.av.net.name.as_deref() == Some("objSnow-LargeRef")).unwrap();
        assert_eq!(snow.colors.len(), 3);
        assert_eq!(snow.av.net.extra_data.len(), 1);
        let q8 = &built[1];
        assert_eq!(q8.shapes, 1);
        let s8 = q8.nif.blocks.iter().find_map(|b| if let Block::TriShape(s) = b { Some(s) } else { None }).unwrap();
        assert_eq!(s8.segments.as_ref().unwrap().len(), 1);
        assert_eq!(s8.av.transform.scale, 8.0);
        assert_eq!(Stats::get(&lod.stats.missing_meshes), 1);
        assert_eq!(Stats::get(&lod.stats.billboards_missing), 1);
        assert_eq!(Stats::get(&lod.stats.total_tris[0]), 3);
        // The file round-trips through the writer and the reader.
        let bytes = q4.nif.to_bytes().unwrap();
        let back = lodforge_nif::parse(&bytes).unwrap();
        assert_eq!(back.blocks.len(), q4.nif.blocks.len());
    }
}

/// Study support (`LODFORGE_TRACE_GOLDEN`): a golden BTO's vertices by world position on a
/// 0.5-unit grid -> (shape name, colour), loaded once per quad.
type GoldenMap = Arc<HashMap<[i64; 3], (String, Option<[u8; 4]>)>>;

fn golden_map(dir: &std::path::Path, file: &str) -> GoldenMap {
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<String, GoldenMap>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(m) = cache.lock().unwrap().get(file) {
        return m.clone();
    }
    let mut map = HashMap::new();
    if let Ok(bytes) = std::fs::read(dir.join(file))
        && let Ok(nif) = lodforge_nif::parse(&bytes)
    {
        for b in &nif.blocks {
            let lodforge_nif::Block::TriShape(s) = b else { continue };
            let name = s.av.net.name.clone().unwrap_or_default();
            let t = &s.av.transform;
            for (i, p) in s.positions.iter().enumerate() {
                let w = [p[0] * t.scale + t.translation[0], p[1] * t.scale + t.translation[1], p[2] * t.scale + t.translation[2]];
                map.entry(golden_key(w)).or_insert((name.clone(), s.colors.get(i).copied()));
            }
        }
    }
    let m = Arc::new(map);
    cache.lock().unwrap().insert(file.to_string(), m.clone());
    m
}

fn golden_key(p: [f32; 3]) -> [i64; 3] {
    [(p[0] * 2.0).round() as i64, (p[1] * 2.0).round() as i64, (p[2] * 2.0).round() as i64]
}

fn golden_lookup(map: &GoldenMap, p: [f32; 3]) -> Option<(&str, Option<[u8; 4]>)> {
    let k = golden_key(p);
    for dx in -1..=1 {
        for dy in -1..=1 {
            for dz in -1..=1 {
                if let Some((name, c)) = map.get(&[k[0] + dx, k[1] + dy, k[2] + dz]) {
                    return Some((name.as_str(), *c));
                }
            }
        }
    }
    None
}
