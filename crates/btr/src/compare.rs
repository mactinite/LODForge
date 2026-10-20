//! Tolerant BTR comparison (docs/re/terrain-lod-spec.md 5.1) between two output folders: per
//! level, the land vertex and triangle sets rounded to a tolerance, the water shapes and
//! segments, the root AABB, shader and texture paths, and the shape headers.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use rayon::prelude::*;

use lodforge_nif::{Block, parse};

use crate::btr::{Btr, parse_file_name};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct LevelReport {
    pub level: i32,
    /// Files present in both folders.
    pub files: usize,
    /// Files present in one folder only.
    pub only_one_side: usize,
    pub vertices_common: usize,
    pub vertices_total: usize,
    pub triangles_common: usize,
    pub triangles_total: usize,
    /// Files whose water shapes agree in count, triangle counts and segments.
    pub water_equal: usize,
    pub aabb_equal: usize,
    pub shader_equal: usize,
    pub textures_equal: usize,
    pub header_equal: usize,
    /// Files whose land shape is identical as a set (vertices and triangles).
    pub land_identical: usize,
    pub examples: Vec<String>,
}

impl LevelReport {
    pub fn line(&self) -> String {
        let pct = |a: usize, b: usize| {
            if b == 0 {
                100.0
            } else {
                100.0 * a as f64 / b as f64
            }
        };
        format!(
            "LOD{}: {} files compared ({} only on one side): land vertices {}/{} ({:.2}%), triangles {}/{} ({:.2}%), land sets identical {}, water shapes+segments equal {}, root AABB within tol {}, shader {}, textures {}, shape header {}",
            self.level,
            self.files,
            self.only_one_side,
            self.vertices_common,
            self.vertices_total,
            pct(self.vertices_common, self.vertices_total),
            self.triangles_common,
            self.triangles_total,
            pct(self.triangles_common, self.triangles_total),
            self.land_identical,
            self.water_equal,
            self.aabb_equal,
            self.shader_equal,
            self.textures_equal,
            self.header_equal
        )
    }
}

type Key = (i64, i64, i64);

fn round(v: f32, tol: f32) -> i64 {
    if tol > 0.0 {
        (v / tol).round() as i64
    } else {
        i64::from(v.to_bits() as i32)
    }
}

fn land_sets(b: &Btr, tol: f32) -> (BTreeSet<Key>, BTreeSet<[Key; 3]>) {
    let key = |p: &[f32; 3]| (round(p[0], tol), round(p[1], tol), round(p[2], tol));
    let verts: BTreeSet<Key> = b.land.positions.iter().map(key).collect();
    let tris: BTreeSet<[Key; 3]> = b
        .land
        .triangles
        .iter()
        .map(|t| {
            let v = t.map(|i| key(&b.land.positions[i as usize]));
            let m = (0..3).min_by_key(|&k| v[k]).unwrap();
            [v[m], v[(m + 1) % 3], v[(m + 2) % 3]]
        })
        .collect();
    (verts, tris)
}

fn btr_files(dir: &Path) -> std::io::Result<BTreeMap<(i32, i32, i32), std::path::PathBuf>> {
    let mut out = BTreeMap::new();
    for e in std::fs::read_dir(dir)? {
        let p = e?.path();
        let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if let Some((_, level, x, y, None)) = parse_file_name(name) {
            out.insert((level, x, y), p);
        }
    }
    Ok(out)
}

struct FileResult {
    level: i32,
    vc: usize,
    vt: usize,
    tc: usize,
    tt: usize,
    land_identical: bool,
    water: bool,
    aabb: bool,
    shader: bool,
    textures: bool,
    header: bool,
    note: Option<String>,
}

fn compare_pair(a: &Path, b: &Path, level: i32, tol: f32) -> Result<FileResult, String> {
    let na = parse(&std::fs::read(a).map_err(|e| e.to_string())?)
        .map_err(|e| format!("{}: {e}", a.display()))?;
    let nb = parse(&std::fs::read(b).map_err(|e| e.to_string())?)
        .map_err(|e| format!("{}: {e}", b.display()))?;
    let ba = Btr::from_nif(&na).ok_or_else(|| format!("{}: not a BTR layout", a.display()))?;
    let bb = Btr::from_nif(&nb).ok_or_else(|| format!("{}: not a BTR layout", b.display()))?;
    let (va, ta) = land_sets(&ba, tol);
    let (vb, tb) = land_sets(&bb, tol);
    let vc = va.intersection(&vb).count();
    let tc = ta.intersection(&tb).count();
    let water = ba.water.len() == bb.water.len()
        && ba
            .water
            .iter()
            .zip(&bb.water)
            .all(|(x, y)| x.triangles.len() == y.triangles.len() && x.segments == y.segments);
    let within = |p: f32, q: f32| (p - q).abs() <= tol * level as f32 + 1e-3;
    let aabb = (0..3).all(|i| {
        within(ba.land_aabb.position[i], bb.land_aabb.position[i])
            && within(ba.land_aabb.extent[i], bb.land_aabb.extent[i])
    });
    let shader_of = |n: &lodforge_nif::Nif| match n.blocks.get(2) {
        Some(Block::LightingShader(s)) => Some(s.clone()),
        _ => None,
    };
    let shader = shader_of(&na).is_some() && shader_of(&na) == shader_of(&nb);
    let textures = Btr::textures(&na) == Btr::textures(&nb);
    let header_of = |n: &lodforge_nif::Nif| match n.blocks.get(1) {
        Some(Block::TriShape(s)) => Some((
            s.vertex_desc,
            s.av.flags,
            s.av.transform.scale.to_bits(),
            s.av.transform.translation.map(f32::to_bits),
        )),
        _ => None,
    };
    let header = header_of(&na).is_some() && header_of(&na) == header_of(&nb);
    let mut notes = Vec::new();
    if vc < va.len().max(vb.len()) {
        notes.push(format!("vertices {vc} common of {}/{}", va.len(), vb.len()));
    }
    if !water {
        notes.push(format!(
            "water {} vs {} shapes",
            ba.water.len(),
            bb.water.len()
        ));
    }
    if !aabb {
        notes.push(format!("aabb {:?} vs {:?}", ba.land_aabb, bb.land_aabb));
    }
    if !shader {
        notes.push("shader differs".into());
    }
    if !textures {
        notes.push("textures differ".into());
    }
    if !header {
        notes.push("shape header differs".into());
    }
    Ok(FileResult {
        level,
        vc,
        vt: va.len().max(vb.len()),
        tc,
        tt: ta.len().max(tb.len()),
        land_identical: va == vb && ta == tb,
        water,
        aabb,
        shader,
        textures,
        header,
        note: (!notes.is_empty()).then(|| notes.join("; ")),
    })
}

/// Compares the default-season BTRs of two folders. `tol` in quad-local units (0 = exact).
pub fn compare_dirs(a: &Path, b: &Path, tol: f32) -> Result<Vec<LevelReport>, String> {
    let fa = btr_files(a).map_err(|e| format!("{}: {e}", a.display()))?;
    let fb = btr_files(b).map_err(|e| format!("{}: {e}", b.display()))?;
    let mut levels: BTreeMap<i32, LevelReport> = BTreeMap::new();
    for k in fa.keys().chain(fb.keys()) {
        levels.entry(k.0).or_insert_with(|| LevelReport {
            level: k.0,
            ..Default::default()
        });
    }
    for k in fa
        .keys()
        .filter(|k| !fb.contains_key(k))
        .chain(fb.keys().filter(|k| !fa.contains_key(k)))
    {
        levels.get_mut(&k.0).unwrap().only_one_side += 1;
    }
    let pairs: Vec<(&(i32, i32, i32), &std::path::PathBuf, &std::path::PathBuf)> = fa
        .iter()
        .filter_map(|(k, pa)| fb.get(k).map(|pb| (k, pa, pb)))
        .collect();
    let results: Vec<(String, Result<FileResult, String>)> = pairs
        .par_iter()
        .map(|(k, pa, pb)| {
            (
                format!("{}.{}.{}", k.0, k.1, k.2),
                compare_pair(pa, pb, k.0, tol),
            )
        })
        .collect();
    for (name, r) in results {
        let r = r?;
        let l = levels.get_mut(&r.level).unwrap();
        l.files += 1;
        l.vertices_common += r.vc;
        l.vertices_total += r.vt;
        l.triangles_common += r.tc;
        l.triangles_total += r.tt;
        l.land_identical += usize::from(r.land_identical);
        l.water_equal += usize::from(r.water);
        l.aabb_equal += usize::from(r.aabb);
        l.shader_equal += usize::from(r.shader);
        l.textures_equal += usize::from(r.textures);
        l.header_equal += usize::from(r.header);
        if let Some(n) = r.note
            && l.examples.len() < 6
        {
            l.examples.push(format!("{name}: {n}"));
        }
    }
    Ok(levels.into_values().collect())
}
