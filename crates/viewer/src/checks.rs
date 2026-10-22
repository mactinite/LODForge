// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! Error detection over loaded LOD: missing files and textures, broken geometry, bounds that
//! would cull wrongly, atlas UVs out of range, terrain seams between neighbouring blocks, and
//! tree billboards hanging above or sunk into the terrain LOD.

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::files::Files;
use crate::mesh::{FileKind, ShapeKind, ShapeMesh};
use crate::trees::{TreeRef, TreeType};
use crate::world::{CELL_UNITS, QuadId, WorldFiles};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warning => "warning",
            Severity::Error => "error",
        }
    }
}

/// One finding. `position` lets the viewer fly to it.
#[derive(Debug, Clone, Serialize)]
pub struct Issue {
    pub severity: Severity,
    /// Short machine-readable kind (`texture_missing`, `terrain_seam`, ...).
    pub kind: &'static str,
    pub message: String,
    pub file: Option<String>,
    pub shape: Option<String>,
    pub level: Option<i32>,
    pub position: Option<[f32; 3]>,
}

impl Issue {
    fn new(severity: Severity, kind: &'static str, message: String) -> Issue {
        Issue {
            severity,
            kind,
            message,
            file: None,
            shape: None,
            level: None,
            position: None,
        }
    }
    fn file(mut self, f: &str) -> Issue {
        self.file = Some(f.to_string());
        self
    }
    fn shape(mut self, s: &ShapeMesh) -> Issue {
        self.shape = Some(format!("[{}] {}", s.block, s.material.name));
        self.position = Some(if s.aabb.is_empty() {
            s.bound_center
        } else {
            s.aabb.center()
        });
        self
    }
    fn level(mut self, l: i32) -> Issue {
        self.level = Some(l);
        self
    }
    fn at(mut self, p: [f32; 3]) -> Issue {
        self.position = Some(p);
        self
    }
}

/// A loaded file: its shapes or why it did not load.
#[derive(Debug, Clone)]
pub struct LoadedQuad {
    pub quad: QuadId,
    pub path: String,
    pub kind: FileKind,
    pub shapes: Vec<ShapeMesh>,
    pub error: Option<String>,
}

/// Tree LOD input to the checks.
#[derive(Debug, Clone, Default)]
pub struct TreeData {
    pub types: Vec<TreeType>,
    /// (block file, references).
    pub blocks: Vec<(String, Vec<TreeRef>)>,
    pub list_error: Option<String>,
}

/// Summary counts.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Summary {
    pub errors: usize,
    pub warnings: usize,
    pub infos: usize,
    pub quads: usize,
    pub shapes: usize,
    pub triangles: usize,
}

/// The report of one world and season.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Report {
    pub world: String,
    pub season: Option<String>,
    pub summary: Summary,
    pub issues: Vec<Issue>,
}

/// Tolerances.
#[derive(Debug, Clone, Copy)]
pub struct Tolerances {
    /// Terrain seam gap per level unit (gap limit = `seam_per_level` * level).
    pub seam_per_level: f32,
    /// Tree billboard base above the LOD4 terrain beyond this is "floating".
    pub tree_float: f32,
    /// Tree billboard base below the LOD4 terrain beyond this is "sunken".
    pub tree_sink: f32,
    /// Bound sphere / box slack (game units) before a vertex counts as outside.
    pub bounds_slack: f32,
    /// UV slack outside [0, 1] on atlas pages.
    pub uv_slack: f32,
}

impl Default for Tolerances {
    /// Trees stand on the full terrain, which LOD4 approximates to within a few hundred units
    /// on slopes, so the tree thresholds are loose; seams between same-level blocks should be
    /// close to exact.
    fn default() -> Self {
        Tolerances {
            seam_per_level: 2.0,
            tree_float: 400.0,
            tree_sink: 800.0,
            bounds_slack: 2.0,
            uv_slack: 0.02,
        }
    }
}

/// Textures a build writes itself (as opposed to full textures the load order provides;
/// `textures\lod\` holds both vanilla LOD textures and TexGen's, so it is not listed).
fn is_generated_texture(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    p.starts_with("textures\\dyndolod\\")
        || p.starts_with("textures\\terrain\\")
        || p.ends_with("linear.dds")
}

/// Runs every check. `load_order` says the load order is attached behind the output, so a
/// texture that resolves nowhere is really missing; without it, full textures that the output
/// does not carry are only noted.
pub fn run(
    files: &dyn Files,
    world: &WorldFiles,
    quads: &[&LoadedQuad],
    trees: &TreeData,
    tol: &Tolerances,
    load_order: bool,
) -> Report {
    let mut issues = Vec::new();
    missing_blocks(world, &mut issues);
    terrain_textures(files, world, &mut issues);
    let mut shapes = 0usize;
    let mut triangles = 0usize;
    let mut seen_textures: HashSet<(String, String)> = HashSet::new();
    for q in quads {
        if let Some(e) = &q.error {
            issues.push(
                Issue::new(Severity::Error, "parse_error", format!("{}: {e}", q.path))
                    .file(&q.path)
                    .level(q.quad.level)
                    .at([
                        q.quad.origin()[0] + q.quad.size() * 0.5,
                        q.quad.origin()[1] + q.quad.size() * 0.5,
                        0.0,
                    ]),
            );
            continue;
        }
        if q.shapes.is_empty() {
            issues.push(
                Issue::new(
                    Severity::Info,
                    "empty_file",
                    format!("{}: no shapes", q.path),
                )
                .file(&q.path)
                .level(q.quad.level),
            );
        }
        for s in &q.shapes {
            shapes += 1;
            triangles += s.triangle_count();
            shape_checks(
                files,
                q,
                s,
                tol,
                load_order,
                &mut seen_textures,
                &mut issues,
            );
        }
    }
    terrain_seams(quads, tol, &mut issues);
    tree_checks(quads, trees, tol, &mut issues);
    let large: usize = quads
        .iter()
        .flat_map(|q| &q.shapes)
        .filter(|s| s.material.large_ref)
        .count();
    if large > 0 {
        issues.push(Issue::new(
            Severity::Info,
            "large_refs",
            format!("{large} large-reference shapes"),
        ));
    }
    issues.sort_by(|a, b| {
        b.severity
            .cmp(&a.severity)
            .then_with(|| a.kind.cmp(b.kind))
            .then_with(|| a.file.cmp(&b.file))
    });
    let summary = Summary {
        errors: issues
            .iter()
            .filter(|i| i.severity == Severity::Error)
            .count(),
        warnings: issues
            .iter()
            .filter(|i| i.severity == Severity::Warning)
            .count(),
        infos: issues
            .iter()
            .filter(|i| i.severity == Severity::Info)
            .count(),
        quads: quads.len(),
        shapes,
        triangles,
    };
    Report {
        world: world.world.clone(),
        season: world.season.clone(),
        summary,
        issues,
    }
}

/// Coarser blocks that should cover every finer block present.
fn missing_blocks(world: &WorldFiles, issues: &mut Vec<Issue>) {
    let sw = world.grid_origin();
    for (label, files, severity) in [
        ("terrain", &world.terrain, Severity::Warning),
        ("object", &world.objects, Severity::Info),
    ] {
        let present: HashSet<QuadId> = files.iter().map(|f| f.quad).collect();
        let levels: HashSet<i32> = present.iter().map(|q| q.level).collect();
        let mut reported = HashSet::new();
        for q in &present {
            let mut cur = *q;
            while let Some(p) = cur.parent(sw) {
                if levels.contains(&p.level) && !present.contains(&p) && reported.insert(p) {
                    let o = p.origin();
                    issues.push(Issue::new(severity, "block_missing", format!("{label} LOD{} block {},{} is missing although LOD{} {},{} under it exists", p.level, p.x, p.y, cur.level, cur.x, cur.y)).level(p.level).at([o[0] + p.size() * 0.5, o[1] + p.size() * 0.5, 0.0]));
                }
                cur = p;
            }
        }
    }
}

fn terrain_textures(files: &dyn Files, world: &WorldFiles, issues: &mut Vec<Issue>) {
    for f in &world.terrain {
        let (d, n) = world.terrain_textures(files, &f.quad);
        let o = f.quad.origin();
        let at = [o[0] + f.quad.size() * 0.5, o[1] + f.quad.size() * 0.5, 0.0];
        if !files.exists(&d) {
            issues.push(
                Issue::new(
                    Severity::Error,
                    "terrain_texture_missing",
                    format!("{d} (diffuse of {})", f.path),
                )
                .file(&f.path)
                .level(f.quad.level)
                .at(at),
            );
        }
        if !files.exists(&n) {
            issues.push(
                Issue::new(
                    Severity::Warning,
                    "terrain_normal_missing",
                    format!("{n} (normal map of {})", f.path),
                )
                .file(&f.path)
                .level(f.quad.level)
                .at(at),
            );
        }
    }
}

fn is_atlas_page(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    p.contains("\\dyndolod\\lod\\dyndolod_") || p.contains("_atlas") || p.contains("treelod")
}

#[allow(clippy::too_many_arguments)]
fn shape_checks(
    files: &dyn Files,
    q: &LoadedQuad,
    s: &ShapeMesh,
    tol: &Tolerances,
    load_order: bool,
    seen: &mut HashSet<(String, String)>,
    issues: &mut Vec<Issue>,
) {
    let level = q.quad.level;
    let m = &s.material;
    // Textures: terrain quads reference their own tiles (checked per quad); everything else
    // resolves through the output and the load order. One report per texture and file.
    if q.kind != FileKind::Terrain {
        for (path, what, severity) in [
            (&m.diffuse, "diffuse", Severity::Error),
            (&m.normal, "normal map", Severity::Warning),
            (&m.glow, "glow map", Severity::Warning),
        ] {
            let Some(p) = path else { continue };
            if files.exists(p) || !seen.insert((q.path.clone(), p.clone())) {
                continue;
            }
            if load_order || is_generated_texture(p) {
                issues.push(
                    Issue::new(
                        severity,
                        "texture_missing",
                        format!("{p} ({what} of {} in {})", m.name, q.path),
                    )
                    .file(&q.path)
                    .shape(s)
                    .level(level),
                );
            } else {
                issues.push(Issue::new(Severity::Info, "texture_not_in_output", format!("{p} ({what} of {} in {}) is not in the output; attach the load order (--instance) to check it", m.name, q.path)).file(&q.path).shape(s).level(level));
            }
        }
        if m.diffuse.is_none() && !m.effect && s.kind != ShapeKind::Water {
            issues.push(
                Issue::new(
                    Severity::Warning,
                    "texture_empty",
                    format!("{} in {}: no diffuse texture", m.name, q.path),
                )
                .file(&q.path)
                .shape(s)
                .level(level),
            );
        }
    }
    if s.non_finite > 0 {
        issues.push(
            Issue::new(
                Severity::Error,
                "vertex_non_finite",
                format!(
                    "{} in {}: {} vertices with NaN or infinite coordinates",
                    m.name, q.path, s.non_finite
                ),
            )
            .file(&q.path)
            .shape(s)
            .level(level),
        );
    }
    if s.bad_indices > 0 {
        issues.push(
            Issue::new(
                Severity::Error,
                "index_out_of_range",
                format!(
                    "{} in {}: {} triangles index past the vertex count",
                    m.name, q.path, s.bad_indices
                ),
            )
            .file(&q.path)
            .shape(s)
            .level(level),
        );
    }
    if s.positions.is_empty() || s.indices.is_empty() {
        issues.push(
            Issue::new(
                Severity::Warning,
                "shape_empty",
                format!("{} in {}: no geometry", m.name, q.path),
            )
            .file(&q.path)
            .shape(s)
            .level(level),
        );
        return;
    }
    let degenerate = s
        .indices
        .chunks_exact(3)
        .filter(|t| {
            triangle_area2(
                s.positions[t[0] as usize],
                s.positions[t[1] as usize],
                s.positions[t[2] as usize],
            ) < 1e-4
        })
        .count();
    if degenerate > 0 {
        let sev = if degenerate * 20 > s.triangle_count() {
            Severity::Warning
        } else {
            Severity::Info
        };
        issues.push(
            Issue::new(
                sev,
                "degenerate_triangles",
                format!(
                    "{} in {}: {degenerate} of {} triangles have no area",
                    m.name,
                    q.path,
                    s.triangle_count()
                ),
            )
            .file(&q.path)
            .shape(s)
            .level(level),
        );
    }
    // Bounds: the game culls by the stored sphere and the BSMultiBoundAABB.
    let c = s.bound_center;
    let limit = s.bound_radius + tol.bounds_slack;
    let outside_sphere = s
        .positions
        .iter()
        .filter(|p| p.iter().all(|v| v.is_finite()))
        .filter(|p| {
            ((p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2) + (p[2] - c[2]).powi(2)).sqrt() > limit
        })
        .count();
    if outside_sphere > 0 {
        issues.push(Issue::new(Severity::Warning, "bound_sphere", format!("{} in {}: {outside_sphere} vertices outside the stored bounding sphere (radius {:.0}); the game may cull it while visible", m.name, q.path, s.bound_radius)).file(&q.path).shape(s).level(level));
    }
    if let Some(b) = s.multibound {
        let outside = s
            .positions
            .iter()
            .filter(|p| p.iter().all(|v| v.is_finite()))
            .filter(|p| !b.contains(**p, tol.bounds_slack))
            .count();
        if outside > 0 {
            issues.push(Issue::new(Severity::Warning, "multibound_aabb", format!("{} in {}: {outside} vertices outside the BSMultiBoundAABB; the game may cull it while visible", m.name, q.path)).file(&q.path).shape(s).level(level));
        }
    }
    // Atlas UVs.
    if let Some(d) = &m.diffuse {
        if is_atlas_page(d) && !s.uvs.is_empty() {
            let lo = -tol.uv_slack;
            let hi = 1.0 + tol.uv_slack;
            let out = s
                .uvs
                .iter()
                .filter(|uv| uv[0] < lo || uv[0] > hi || uv[1] < lo || uv[1] > hi)
                .count();
            if out > 0 {
                issues.push(
                    Issue::new(
                        Severity::Warning,
                        "uv_outside_atlas",
                        format!(
                            "{} in {}: {out} UVs outside the atlas page {d}",
                            m.name, q.path
                        ),
                    )
                    .file(&q.path)
                    .shape(s)
                    .level(level),
                );
            }
        }
    }
}

fn triangle_area2(a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> f32 {
    let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    let x = u[1] * v[2] - u[2] * v[1];
    let y = u[2] * v[0] - u[0] * v[2];
    let z = u[0] * v[1] - u[1] * v[0];
    (x * x + y * y + z * z).sqrt()
}

/// Height gaps along shared edges of neighbouring terrain blocks of the same level.
fn terrain_seams(quads: &[&LoadedQuad], tol: &Tolerances, issues: &mut Vec<Issue>) {
    let by_quad: HashMap<QuadId, &LoadedQuad> = quads
        .iter()
        .filter(|q| q.kind == FileKind::Terrain && q.error.is_none())
        .map(|q| (q.quad, *q))
        .collect();
    for (id, q) in &by_quad {
        let Some(land) = q.shapes.iter().find(|s| s.kind == ShapeKind::Terrain) else {
            continue;
        };
        let size = id.size();
        let [x0, y0] = id.origin();
        // East neighbour (shared edge x = x0 + size) and north neighbour (y = y0 + size).
        for (neighbour, axis) in [
            (
                QuadId {
                    level: id.level,
                    x: id.x + id.level,
                    y: id.y,
                },
                0usize,
            ),
            (
                QuadId {
                    level: id.level,
                    x: id.x,
                    y: id.y + id.level,
                },
                1usize,
            ),
        ] {
            let Some(n) = by_quad.get(&neighbour) else {
                continue;
            };
            let Some(nland) = n.shapes.iter().find(|s| s.kind == ShapeKind::Terrain) else {
                continue;
            };
            let edge = if axis == 0 { x0 + size } else { y0 + size };
            let along = 1 - axis;
            let a = edge_profile(&land.positions, axis, edge, along, id.level as f32);
            let b = edge_profile(&nland.positions, axis, edge, along, id.level as f32);
            if a.len() < 2 || b.len() < 2 {
                continue;
            }
            let limit = tol.seam_per_level * id.level as f32;
            let (mut worst, mut worst_at) = (0.0f32, 0.0f32);
            let mut count = 0usize;
            for &(t, z) in &a {
                let Some(zb) = interpolate(&b, t) else {
                    continue;
                };
                let gap = (z - zb).abs();
                if gap > limit {
                    count += 1;
                    if gap > worst {
                        worst = gap;
                        worst_at = t;
                    }
                }
            }
            if count > 0 {
                let pos = if axis == 0 {
                    [edge, worst_at, 0.0]
                } else {
                    [worst_at, edge, 0.0]
                };
                let zpos = interpolate(&a, worst_at).unwrap_or(0.0);
                issues.push(
                    Issue::new(Severity::Warning, "terrain_seam", format!("{} / {}: {count} edge vertices differ by more than {limit:.0} units (worst {worst:.0})", q.path, n.path)).file(&q.path).level(id.level).at([pos[0], pos[1], zpos]),
                );
            }
        }
    }
}

/// The top surface along a block edge: the vertices on the edge, one per position along it
/// (the highest, so skirt vertices hanging below the border do not count), sorted.
fn edge_profile(
    positions: &[[f32; 3]],
    axis: usize,
    edge: f32,
    along: usize,
    level: f32,
) -> Vec<(f32, f32)> {
    let mut v: Vec<(f32, f32)> = positions
        .iter()
        .filter(|p| (p[axis] - edge).abs() < 0.5 * level)
        .map(|p| (p[along], p[2]))
        .collect();
    v.sort_by(|p, r| p.0.total_cmp(&r.0).then(r.1.total_cmp(&p.1)));
    v.dedup_by(|p, r| (p.0 - r.0).abs() < 0.25 * level);
    v
}

/// Linear interpolation of a sorted (t, z) polyline; `None` outside its range.
fn interpolate(line: &[(f32, f32)], t: f32) -> Option<f32> {
    if line.is_empty() || t < line[0].0 - 1e-3 || t > line[line.len() - 1].0 + 1e-3 {
        return None;
    }
    let i = line.partition_point(|p| p.0 < t);
    if i == 0 {
        return Some(line[0].1);
    }
    if i >= line.len() {
        return Some(line[line.len() - 1].1);
    }
    let (t0, z0) = line[i - 1];
    let (t1, z1) = line[i];
    if (t1 - t0).abs() < 1e-6 {
        return Some(z0);
    }
    Some(z0 + (z1 - z0) * ((t - t0) / (t1 - t0)))
}

/// Terrain height lookup over the finest terrain level loaded.
pub struct HeightField {
    cells: HashMap<(i32, i32), Vec<(usize, u32)>>,
    shapes: Vec<(Vec<[f32; 3]>, Vec<u32>)>,
}

const HF_CELL: f32 = 512.0;

impl HeightField {
    pub fn new(quads: &[&LoadedQuad]) -> HeightField {
        let Some(level) = quads
            .iter()
            .filter(|q| q.kind == FileKind::Terrain)
            .map(|q| q.quad.level)
            .min()
        else {
            return HeightField {
                cells: HashMap::new(),
                shapes: Vec::new(),
            };
        };
        let mut hf = HeightField {
            cells: HashMap::new(),
            shapes: Vec::new(),
        };
        for q in quads
            .iter()
            .filter(|q| q.kind == FileKind::Terrain && q.quad.level == level)
        {
            for s in q.shapes.iter().filter(|s| s.kind == ShapeKind::Terrain) {
                let si = hf.shapes.len();
                for (ti, t) in s.indices.chunks_exact(3).enumerate() {
                    let ps = [
                        s.positions[t[0] as usize],
                        s.positions[t[1] as usize],
                        s.positions[t[2] as usize],
                    ];
                    let (x0, x1) = (
                        ps.iter().map(|p| p[0]).fold(f32::INFINITY, f32::min),
                        ps.iter().map(|p| p[0]).fold(f32::NEG_INFINITY, f32::max),
                    );
                    let (y0, y1) = (
                        ps.iter().map(|p| p[1]).fold(f32::INFINITY, f32::min),
                        ps.iter().map(|p| p[1]).fold(f32::NEG_INFINITY, f32::max),
                    );
                    if !(x0.is_finite() && x1.is_finite() && y0.is_finite() && y1.is_finite()) {
                        continue;
                    }
                    for cx in (x0 / HF_CELL).floor() as i32..=(x1 / HF_CELL).floor() as i32 {
                        for cy in (y0 / HF_CELL).floor() as i32..=(y1 / HF_CELL).floor() as i32 {
                            hf.cells.entry((cx, cy)).or_default().push((si, ti as u32));
                        }
                    }
                }
                hf.shapes.push((s.positions.clone(), s.indices.clone()));
            }
        }
        hf
    }

    pub fn is_empty(&self) -> bool {
        self.shapes.is_empty()
    }

    /// Highest terrain surface under (x, y).
    pub fn height(&self, x: f32, y: f32) -> Option<f32> {
        let key = ((x / HF_CELL).floor() as i32, (y / HF_CELL).floor() as i32);
        let mut best: Option<f32> = None;
        for &(si, ti) in self.cells.get(&key)? {
            let (pos, idx) = &self.shapes[si];
            let t = &idx[ti as usize * 3..ti as usize * 3 + 3];
            let (a, b, c) = (pos[t[0] as usize], pos[t[1] as usize], pos[t[2] as usize]);
            if let Some(z) = barycentric_z(a, b, c, x, y) {
                best = Some(best.map_or(z, |b| b.max(z)));
            }
        }
        best
    }
}

fn barycentric_z(a: [f32; 3], b: [f32; 3], c: [f32; 3], x: f32, y: f32) -> Option<f32> {
    let d = (b[1] - c[1]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[1] - c[1]);
    if d.abs() < 1e-9 {
        return None;
    }
    let l0 = ((b[1] - c[1]) * (x - c[0]) + (c[0] - b[0]) * (y - c[1])) / d;
    let l1 = ((c[1] - a[1]) * (x - c[0]) + (a[0] - c[0]) * (y - c[1])) / d;
    let l2 = 1.0 - l0 - l1;
    let eps = -1e-4;
    (l0 >= eps && l1 >= eps && l2 >= eps).then(|| l0 * a[2] + l1 * b[2] + l2 * c[2])
}

fn tree_checks(quads: &[&LoadedQuad], trees: &TreeData, tol: &Tolerances, issues: &mut Vec<Issue>) {
    if let Some(e) = &trees.list_error {
        issues.push(Issue::new(Severity::Error, "tree_list", e.clone()));
    }
    let known: HashSet<i32> = trees.types.iter().map(|t| t.index).collect();
    for t in &trees.types {
        if t.uv_min
            .iter()
            .chain(&t.uv_max)
            .any(|v| !(-tol.uv_slack..=1.0 + tol.uv_slack).contains(v))
            || t.uv_min[0] >= t.uv_max[0]
            || t.uv_min[1] >= t.uv_max[1]
        {
            issues.push(Issue::new(
                Severity::Error,
                "tree_type_uv",
                format!(
                    "tree type {}: atlas rectangle {:?}..{:?} is not inside the atlas",
                    t.index, t.uv_min, t.uv_max
                ),
            ));
        }
        if t.width <= 0.0 || t.height <= 0.0 {
            issues.push(Issue::new(
                Severity::Error,
                "tree_type_size",
                format!(
                    "tree type {}: billboard size {}x{}",
                    t.index, t.width, t.height
                ),
            ));
        }
    }
    if trees.blocks.is_empty() {
        return;
    }
    let hf = HeightField::new(quads);
    for (file, refs) in &trees.blocks {
        let mut unknown = 0usize;
        let (mut floating, mut sunken) = (0usize, 0usize);
        let (mut worst_float, mut worst_sink): (Option<(f32, [f32; 3])>, Option<(f32, [f32; 3])>) =
            (None, None);
        for r in refs {
            if !known.contains(&r.type_index) {
                unknown += 1;
            }
            if let Some(g) = hf.height(r.pos[0], r.pos[1]) {
                let d = r.pos[2] - g;
                if d > tol.tree_float {
                    floating += 1;
                    if worst_float.is_none_or(|w| d > w.0) {
                        worst_float = Some((d, r.pos));
                    }
                } else if -d > tol.tree_sink {
                    sunken += 1;
                    if worst_sink.is_none_or(|w| -d > w.0) {
                        worst_sink = Some((-d, r.pos));
                    }
                }
            }
        }
        if unknown > 0 {
            issues.push(
                Issue::new(
                    Severity::Error,
                    "tree_type_unknown",
                    format!(
                        "{file}: {unknown} references use a type index that is not in the .lst"
                    ),
                )
                .file(file)
                .level(4),
            );
        }
        if let Some((d, p)) = worst_float {
            issues.push(Issue::new(Severity::Warning, "tree_floating", format!("{file}: {floating} tree billboards start more than {:.0} units above the terrain LOD (worst {d:.0})", tol.tree_float)).file(file).level(4).at(p));
        }
        if let Some((d, p)) = worst_sink {
            issues.push(Issue::new(Severity::Warning, "tree_sunken", format!("{file}: {sunken} tree billboards start more than {:.0} units below the terrain LOD (worst {d:.0})", tol.tree_sink)).file(file).level(4).at(p));
        }
    }
    let _ = CELL_UNITS;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::files::MemFiles;
    use crate::mesh::{Aabb, Material};
    use crate::world::QuadFile;

    fn flat_quad(q: QuadId, z: f32, edge_bump: f32) -> LoadedQuad {
        // A 2x2 grid of vertices over the block at height z; the east edge raised by edge_bump;
        // plus skirt vertices 500 units below the east edge, which the seam check must ignore.
        let [x0, y0] = q.origin();
        let s = q.size();
        let positions = vec![
            [x0, y0, z],
            [x0 + s, y0, z + edge_bump],
            [x0, y0 + s, z],
            [x0 + s, y0 + s, z + edge_bump],
            [x0 + s, y0, z - 500.0],
            [x0 + s, y0 + s, z - 500.0],
        ];
        let aabb = Aabb::from_points(&positions);
        let shape = ShapeMesh {
            block: 1,
            kind: ShapeKind::Terrain,
            material: Material {
                name: String::new(),
                ..Default::default()
            },
            positions,
            normals: vec![],
            tangents: vec![],
            bitangents: vec![],
            uvs: vec![],
            colors: vec![],
            indices: vec![0, 1, 2, 2, 1, 3],
            aabb,
            bound_center: aabb.center(),
            bound_radius: s,
            multibound: None,
            scale: q.level as f32,
            bad_indices: 0,
            non_finite: 0,
            segments: vec![],
        };
        LoadedQuad {
            quad: q,
            path: format!("meshes\\terrain\\w\\w.{}.{}.{}.btr", q.level, q.x, q.y),
            kind: FileKind::Terrain,
            shapes: vec![shape],
            error: None,
        }
    }

    #[test]
    fn finds_seams_and_floating_trees() {
        let a = flat_quad(
            QuadId {
                level: 4,
                x: 0,
                y: 0,
            },
            100.0,
            50.0,
        );
        let b = flat_quad(
            QuadId {
                level: 4,
                x: 4,
                y: 0,
            },
            100.0,
            0.0,
        );
        let world = WorldFiles {
            world: "w".into(),
            terrain: vec![
                QuadFile {
                    quad: a.quad,
                    path: a.path.clone(),
                    seasonal: false,
                },
                QuadFile {
                    quad: b.quad,
                    path: b.path.clone(),
                    seasonal: false,
                },
            ],
            ..Default::default()
        };
        let mut files = MemFiles::default();
        for q in [&a, &b] {
            let (d, n) = world.terrain_textures(&files, &q.quad);
            files.insert(&d, vec![]);
            files.insert(&n, vec![]);
        }
        let trees = TreeData {
            types: vec![TreeType {
                index: 1,
                width: 100.0,
                height: 200.0,
                uv_min: [0.0, 0.0],
                uv_max: [0.5, 0.5],
            }],
            blocks: vec![(
                "w.4.0.0.btt".into(),
                vec![
                    TreeRef {
                        pos: [8000.0, 8000.0, 100.0],
                        rotation: 0.0,
                        scale: 1.0,
                        form_id: 1,
                        type_index: 1,
                    },
                    TreeRef {
                        pos: [8000.0, 9000.0, 1900.0],
                        rotation: 0.0,
                        scale: 1.0,
                        form_id: 2,
                        type_index: 1,
                    },
                    TreeRef {
                        pos: [8000.0, 9000.0, 100.0],
                        rotation: 0.0,
                        scale: 1.0,
                        form_id: 3,
                        type_index: 9,
                    },
                ],
            )],
            list_error: None,
        };
        let r = run(
            &files,
            &world,
            &[&a, &b],
            &trees,
            &Tolerances::default(),
            true,
        );
        let kinds: Vec<&str> = r.issues.iter().map(|i| i.kind).collect();
        assert!(kinds.contains(&"terrain_seam"), "{kinds:?}");
        assert!(kinds.contains(&"tree_floating"), "{kinds:?}");
        assert!(kinds.contains(&"tree_type_unknown"), "{kinds:?}");
        assert!(!kinds.contains(&"terrain_texture_missing"), "{kinds:?}");
        let seam = r.issues.iter().find(|i| i.kind == "terrain_seam").unwrap();
        assert_eq!(seam.position.unwrap()[0], 16384.0);
        assert_eq!(r.summary.errors, 1);
        // Without the east edge bump there is no seam.
        let a2 = flat_quad(
            QuadId {
                level: 4,
                x: 0,
                y: 0,
            },
            100.0,
            0.0,
        );
        let r2 = run(
            &files,
            &world,
            &[&a2, &b],
            &TreeData::default(),
            &Tolerances::default(),
            true,
        );
        assert!(!r2.issues.iter().any(|i| i.kind == "terrain_seam"));
    }

    #[test]
    fn reports_missing_parent_blocks_and_bounds() {
        let a = flat_quad(
            QuadId {
                level: 4,
                x: 0,
                y: 0,
            },
            0.0,
            0.0,
        );
        let top = flat_quad(
            QuadId {
                level: 8,
                x: 8,
                y: 8,
            },
            0.0,
            0.0,
        );
        let world = WorldFiles {
            world: "w".into(),
            terrain: vec![
                QuadFile {
                    quad: a.quad,
                    path: a.path.clone(),
                    seasonal: false,
                },
                QuadFile {
                    quad: top.quad,
                    path: top.path.clone(),
                    seasonal: false,
                },
            ],
            ..Default::default()
        };
        let files = MemFiles::default();
        let mut bad = a.clone();
        bad.shapes[0].bound_radius = 1.0;
        let r = run(
            &files,
            &world,
            &[&bad, &top],
            &TreeData::default(),
            &Tolerances::default(),
            true,
        );
        let kinds: Vec<&str> = r.issues.iter().map(|i| i.kind).collect();
        assert!(kinds.contains(&"block_missing"), "{kinds:?}");
        assert!(kinds.contains(&"bound_sphere"), "{kinds:?}");
        assert!(kinds.contains(&"terrain_texture_missing"), "{kinds:?}");
        let hf = HeightField::new(&[&a]);
        assert_eq!(hf.height(100.0, 100.0), Some(0.0));
        assert_eq!(hf.height(-100.0, 100.0), None);
    }
}
