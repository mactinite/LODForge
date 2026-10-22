// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! LOD inspection viewer: renders a worldspace's generated terrain, object, tree and grass LOD
//! as the game arranges it (one level everywhere, or the game's distance-based quadtree), with
//! a free camera, a top-down map overview, picking, and checks that flag missing files and
//! textures, broken geometry, culling bounds, atlas UVs, terrain seams and floating trees.
//!
//! Entry points: [`app::run`] (window), [`screenshot`] (headless PNG), [`check`] (report).

pub mod app;
pub mod atmosphere;
pub mod camera;
pub mod checks;
pub mod files;
pub mod headless;
pub mod lodselect;
pub mod mesh;
pub mod pick;
pub mod render;
pub mod scene;
pub mod snow;
pub mod trees;
pub mod viewstate;
pub mod world;

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use anyhow::{Context, Result, anyhow};

use camera::{FreeCam, Overview, ViewParams};
use checks::{Report, Tolerances};
use files::Files;
use render::textures::TexCache;
use render::{Layouts, Renderer};
use scene::{LoadOptions, Loader, Scene};
use viewstate::ViewSettings;
use world::WorldFiles;

/// Loads a world completely on the given device (blocks until the loader is done).
pub fn load_blocking(
    files: Arc<dyn Files>,
    world: Arc<WorldFiles>,
    gpu: &lodforge_gpu::Gpu,
    layouts: &Layouts,
    options: LoadOptions,
) -> (Scene, Arc<TexCache>) {
    let cache = Arc::new(TexCache::new(
        gpu.device.clone(),
        gpu.queue.clone(),
        gpu.device
            .features()
            .contains(wgpu::Features::TEXTURE_COMPRESSION_BC),
    ));
    let total = scene::total_items(&world, &options);
    let rx = scene::start(Loader {
        files,
        world,
        device: gpu.device.clone(),
        queue: gpu.queue.clone(),
        layouts: layouts.clone(),
        cache: cache.clone(),
        options,
        cancel: Arc::new(AtomicBool::new(false)),
    });
    let mut scene = Scene::new(total);
    for ev in rx {
        scene.apply(ev);
    }
    scene.done = true;
    (scene, cache)
}

/// A headless render.
pub struct ScreenshotConfig {
    pub files: Arc<dyn Files>,
    pub world: WorldFiles,
    pub load: LoadOptions,
    pub settings: ViewSettings,
    /// `None`: a top-down overview of the whole world.
    pub camera: Option<FreeCam>,
    /// Centre of the game-mode LOD arrangement (x, y); `None`: the camera position.
    pub lod_center: Option<[f32; 2]>,
    pub width: u32,
    pub height: u32,
    pub samples: u32,
}

/// Renders one PNG of the world; returns the PNG bytes and the loader's messages.
pub fn screenshot(cfg: ScreenshotConfig) -> Result<(Vec<u8>, Vec<String>)> {
    let gpu = lodforge_gpu::Gpu::new().context("opening the GPU")?;
    let layouts = Layouts::new(&gpu.device, &gpu.queue);
    let samples = cfg.samples.clamp(1, gpu.max_msaa);
    let mut renderer = Renderer::new(
        &gpu.device,
        layouts.clone(),
        headless::HEADLESS_FORMAT,
        samples,
    );
    let world = Arc::new(cfg.world);
    let mut load = cfg.load;
    if let Some(c) = &cfg.camera {
        load.start = [c.pos[0], c.pos[1]];
    }
    let files = cfg.files.clone();
    let (scene, cache) = load_blocking(cfg.files, world.clone(), &gpu, &layouts, load);
    renderer.set_projected(&gpu.device, &cache, files.as_ref());
    if scene.quads.is_empty() {
        return Err(anyhow!("nothing loaded for {}", world.world));
    }
    let aspect = cfg.width as f32 / cfg.height.max(1) as f32;
    let (view, cam_xy) = match cfg.camera {
        Some(c) => (ViewParams::free(&c, aspect), [c.pos[0], c.pos[1]]),
        None => {
            let o = fit_overview(&scene, aspect);
            (ViewParams::overview(&o, aspect), o.center)
        }
    };
    let cam_xy = cfg.lod_center.unwrap_or(cam_xy);
    let visible = viewstate::visible_quads(&scene, &world, cfg.settings.mode, &cfg.settings.lod_distances, cam_xy);
    let (frame, globals, _) = viewstate::build_frame(&scene, &visible, &cfg.settings, &view, None);
    let rgba = headless::render_rgba(
        &gpu.device,
        &gpu.queue,
        &mut renderer,
        cfg.width,
        cfg.height,
        &globals,
        &frame,
    )
    .map_err(|e| anyhow!(e))?;
    let png = headless::encode_png(&rgba, cfg.width, cfg.height).map_err(|e| anyhow!(e))?;
    Ok((png, scene.messages))
}

/// An overview framing the loaded world.
pub fn fit_overview(scene: &Scene, aspect: f32) -> Overview {
    let b = scene.bounds;
    if b.is_empty() {
        return Overview::default();
    }
    let c = b.center();
    let half_w = (b.max[0] - b.min[0]) * 0.5 / aspect.max(0.1);
    let half_h = (b.max[1] - b.min[1]) * 0.5;
    Overview {
        center: [c[0], c[1]],
        half_height: half_w.max(half_h) * 1.05 + 1000.0,
        top_z: b.max[2] + 5000.0,
        bottom_z: b.min[2] - 5000.0,
    }
}

/// Runs the checks over a world without a GPU.
pub fn check(
    files: &dyn Files,
    world: &WorldFiles,
    load: &LoadOptions,
    tol: &Tolerances,
    load_order: bool,
) -> (Report, Vec<String>) {
    let (quads, trees, messages) = scene::load_cpu(files, world, load);
    let refs: Vec<&checks::LoadedQuad> = quads.iter().collect();
    (
        checks::run(files, world, &refs, &trees, tol, load_order),
        messages,
    )
}
