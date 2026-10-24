// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
//! Instanced object LOD for the LODForge Runtime (docs/runtime/instance-formats.md section 6):
//! references whose LOD model the runtime draws instanced instead of the `.bto` merging it.
//!
//! Two rules decide, per quad and level:
//! - `runtime_refs`: references the runtime owns (trees drawn as impostors). They never go into
//!   merged shapes; their 3D LOD models become instances, their billboards are dropped.
//! - `min_copies`: any other model placed at least that many times in the quad (and matching
//!   `model_masks`) becomes instances, unless a reference needs per-reference texture or colour
//!   changes (AltTextures / VC side files) or is a large reference.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::export::StaticRef;

/// The instancing options ([`crate::Options::instancing`]).
#[derive(Debug, Clone, Default)]
pub struct Instancing {
    /// References (export column 1, upper case) the runtime draws.
    pub runtime_refs: Arc<HashSet<String>>,
    /// Write the 3D LOD models of `runtime_refs` as instances (false: drop them entirely).
    pub runtime_ref_instances: bool,
    /// Other models with at least this many placements in one quad become instances; 0 = off.
    pub min_copies: usize,
    /// Only models whose lower-case path contains one of these; empty = every model.
    pub model_masks: Vec<String>,
    /// Per (level index, model): eligible placements in the whole worldspace. When set,
    /// `min_copies` counts copies per worldspace instead of per quad.
    pub world_counts: Arc<HashMap<(usize, String), usize>>,
    /// Only references with a plain or passthru material (no snow/ash shader, no material
    /// object): the runtime draws instances with one lighting material.
    pub plain_only: bool,
    /// Large references may be instanced (flagged on the instance).
    pub large_refs: bool,
    /// References with AltTextures entries may be instanced (their textures go on the instance).
    pub alt_textures: bool,
    /// With `plain_only`: snow and ash references may be instanced too (their parts carry a
    /// snow/ash class, see [`Instance::part_classes`]).
    pub snow: bool,
}

/// One instanced placement.
#[derive(Debug, Clone, PartialEq)]
pub struct Instance {
    /// Export column 1 (load-order form id, upper-case hex).
    pub ref_id: String,
    /// LOD model path, lower case.
    pub model: String,
    pub position: [f32; 3],
    /// Degrees, as the export's columns 6-8.
    pub rotation_deg: [f32; 3],
    pub scale: f32,
    /// One of `runtime_refs`.
    pub runtime_ref: bool,
    /// A large reference (`-LargeRef`): the runtime hides it while the reference's own 3D is
    /// loaded, as the engine does for its object LOD.
    pub large_ref: bool,
    /// Per part of the model: the (diffuse, normal) the AltTextures side file gives this
    /// reference, `None` for none (the model's own textures).
    pub alt_textures: Option<Vec<(String, String)>>,
    /// Per part: the LOD material class the `.bto` would give it ([`PART_PLAIN`], [`PART_SNOW`],
    /// [`PART_SNOW_HD`], [`PART_ASH`], [`PART_ASH_HD`]); empty = all plain.
    pub part_classes: Vec<u8>,
}

pub const PART_PLAIN: u8 = 0;
pub const PART_SNOW: u8 = 1;
pub const PART_SNOW_HD: u8 = 2;
pub const PART_ASH: u8 = 3;
pub const PART_ASH_HD: u8 = 4;

/// What [`Instancing::plan`] decided for one reference of a quad.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Merge,
    Instance,
    Drop,
}

impl Instancing {
    pub fn is_runtime_ref(&self, r: &StaticRef) -> bool {
        self.runtime_refs.contains(&r.ref_id.to_ascii_uppercase())
    }

    /// Whether model `m` of `r` may be instanced at all (side-file overrides aside).
    pub fn eligible(&self, r: &StaticRef, m: &str) -> bool {
        let plain = || {
            let c = crate::material::MaterialClass::parse(r.material());
            matches!(c, crate::material::MaterialClass::Plain | crate::material::MaterialClass::PassThru) || (self.snow && c.is_snow_or_ash())
        };
        !m.ends_with(".dds") && (self.large_refs || !r.is_large_ref()) && !m.contains("glow") && self.model_matches(m) && (!self.plain_only || plain())
    }

    /// Eligible placements per (level index, model) over a whole export (runtime references and
    /// references with AltTextures entries left out).
    pub fn count_world(&self, statics: &[StaticRef], has_alt: impl Fn(&StaticRef) -> bool) -> HashMap<(usize, String), usize> {
        let mut out = HashMap::new();
        for r in statics {
            if self.is_runtime_ref(r) || (!self.alt_textures && has_alt(r)) {
                continue;
            }
            for k in 0..4 {
                if let Some(m) = r.model(k).map(|m| m.to_ascii_lowercase().replace('/', "\\"))
                    && self.eligible(r, &m)
                {
                    *out.entry((k, m)).or_default() += 1;
                }
            }
        }
        out
    }

    fn model_matches(&self, model_l: &str) -> bool {
        self.model_masks.is_empty() || self.model_masks.iter().any(|m| model_l.contains(m.as_str()))
    }

    /// Routes every reference of a quad at level index `k`. `per_ref_overrides` says whether a
    /// reference has side-file texture or colour changes for this model.
    pub fn plan(&self, refs: &[&StaticRef], k: usize, per_ref_overrides: impl Fn(&StaticRef, &str) -> bool) -> Vec<Route> {
        let model_of = |r: &StaticRef| r.model(k).map(|m| m.to_ascii_lowercase().replace('/', "\\"));
        let eligible = |r: &StaticRef, m: &str| self.eligible(r, m) && !per_ref_overrides(r, m);
        let mut counts: HashMap<String, usize> = HashMap::new();
        if self.min_copies > 0 {
            for r in refs {
                if self.is_runtime_ref(r) {
                    continue;
                }
                if let Some(m) = model_of(r)
                    && eligible(r, &m)
                {
                    *counts.entry(m).or_default() += 1;
                }
            }
        }
        refs.iter()
            .map(|r| {
                let Some(m) = model_of(r) else { return Route::Merge };
                if self.is_runtime_ref(r) {
                    return if !m.ends_with(".dds") && self.runtime_ref_instances { Route::Instance } else { Route::Drop };
                }
                let n = if self.world_counts.is_empty() { counts.get(&m).copied().unwrap_or(0) } else { self.world_counts.get(&(k, m.clone())).copied().unwrap_or(0) };
                if self.min_copies > 0 && eligible(r, &m) && n >= self.min_copies { Route::Instance } else { Route::Merge }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(id: &str, model: &str, material: &str) -> StaticRef {
        StaticRef { ref_id: id.into(), scale: 1.0, material_name: material.into(), models: [model.into(), String::new(), String::new(), String::new()], ..Default::default() }
    }

    #[test]
    fn routes() {
        let refs = [
            r("00000001", r"meshes\lod\trees\pinepassthru_lod.nif", ""),
            r("00000002", r"textures\terrain\lodgen\pine.dds", ""),
            r("00000003", r"meshes\lod\trees\pinepassthru_lod.nif", ""),
            r("00000004", r"meshes\lod\trees\pinepassthru_lod.nif", ""),
            r("00000005", r"meshes\lod\trees\pinepassthru_lod.nif", "-LargeRef"),
            r("00000006", r"meshes\lod\rock_lod.nif", ""),
            r("00000007", r"meshes\lod\rock_lod.nif", ""),
        ];
        let refs: Vec<&StaticRef> = refs.iter().collect();
        let i = Instancing { runtime_refs: Arc::new(["00000001".to_string(), "00000002".to_string()].into()), runtime_ref_instances: true, min_copies: 2, model_masks: vec!["passthru".into()], ..Default::default() };
        // 1: runtime 3D -> instance; 2: runtime billboard -> drop; 3, 4: two non-runtime copies
        // -> instances; 5: large ref -> merge; 6, 7: no mask match -> merge.
        assert_eq!(i.plan(&refs, 0, |_, _| false), vec![Route::Instance, Route::Drop, Route::Instance, Route::Instance, Route::Merge, Route::Merge, Route::Merge]);
        // A side-file override on 4 leaves 3 alone below min_copies.
        assert_eq!(i.plan(&refs, 0, |r, _| r.ref_id == "00000004")[2..4], [Route::Merge, Route::Merge]);
        let none = Instancing { runtime_ref_instances: false, min_copies: 0, ..i.clone() };
        assert_eq!(none.plan(&refs, 0, |_, _| false)[..4], [Route::Drop, Route::Drop, Route::Merge, Route::Merge]);
    }
}
