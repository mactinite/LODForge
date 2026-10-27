// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! Occlusion during the build: a worldspace's TVDT needs only its terrain LOD meshes and the
//! object LOD height map, both done once its objects stage is. This thread computes each
//! worldspace as the build hands it over, while the later stages go on. Only main worldspaces:
//! a child worldspace's cells and LAND come from CELL records that DynDOLOD.esm adds, so the
//! children are computed at the end (a few seconds each). Occlusion.esp is still
//! written at the end, against the final load order ([`crate::occlusion::add_worlds`] selects
//! the cells again with DynDOLOD.esm/.esp in it, so the plugin matches computing it all there).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Instant;

use crate::dyndolod_build::OcclusionInputs;
use crate::occlusion::ComputedWorld;
use crate::profile::ProfileArgs;

pub struct OcclusionLane {
    tx: Option<mpsc::Sender<(String, OcclusionInputs)>>,
    handle: Option<std::thread::JoinHandle<HashMap<String, Vec<ComputedWorld>>>>,
}

impl OcclusionLane {
    /// `profile`: the build's input profile (without earlier LOD output); `worlds`: the
    /// worldspaces the build makes (child worldspaces of a built parent are done with it).
    pub fn start(profile: ProfileArgs, out: PathBuf, worlds: Vec<String>) -> OcclusionLane {
        let (tx, rx) = mpsc::channel::<(String, OcclusionInputs)>();
        let handle = std::thread::Builder::new()
            .name("occlusion-lane".into())
            .spawn(move || {
                let mut done = HashMap::new();
                let p = match profile.open() {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("occlusion lane: {e:#} (occlusion is computed at the end)");
                        return done;
                    }
                };
                let all = lodforge_esp::scan_worldspaces(&p.load_order).unwrap_or_default();
                // The build's own terrain LOD comes from the output folder (the main thread's VFS
                // sees it the same way once the build adds its output); everything else from the
                // load order.
                let read = |path: &str| -> Option<Vec<u8>> {
                    let lower = path.to_ascii_lowercase();
                    if lower.starts_with("meshes\\terrain\\")
                        && let Ok(b) = std::fs::read(out.join(path))
                    {
                        return Some(b);
                    }
                    p.vfs.read(path).ok().flatten()
                };
                for (world, inputs) in rx {
                    let Some(w) = all.iter().find(|w| w.editor_id.eq_ignore_ascii_case(&world)) else { continue };
                    if crate::dyndolod_build::occlusion_with_parent(w, &all, &worlds) {
                        continue;
                    }
                    let t = Instant::now();
                    let heightmap = crate::dyndolod_build::occlusion_heightmap(&out, &w.editor_id);
                    match crate::occlusion::compute_worlds(&p.load_order, &read, &w.editor_id, heightmap.as_deref(), inputs.params, &|child, is_main| if is_main { inputs.mode(child, is_main) } else { None }) {
                        Ok(c) => {
                            crate::build::timing_log(&format!("{{\"event\":\"lane\",\"world\":\"{}\",\"stage\":\"occlusion-compute\",\"ms\":{}}}", w.editor_id, t.elapsed().as_millis()));
                            done.insert(w.editor_id.to_ascii_lowercase(), c);
                        }
                        Err(e) => eprintln!("occlusion lane: {}: {e:#} (computed again at the end)", w.editor_id),
                    }
                }
                done
            })
            .ok();
        OcclusionLane { tx: Some(tx), handle }
    }

    /// Hands a worldspace over once its terrain meshes and objects stage are done.
    pub fn send(&self, world: &str, inputs: OcclusionInputs) {
        if let Some(tx) = &self.tx {
            let _ = tx.send((world.to_string(), inputs));
        }
    }

    /// Waits for the worldspaces handed over and returns them by main worldspace (lower case).
    pub fn finish(mut self) -> HashMap<String, Vec<ComputedWorld>> {
        self.tx.take();
        self.handle.take().and_then(|h| h.join().ok()).unwrap_or_default()
    }
}
