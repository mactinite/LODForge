// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! The interactive window: winit event loop, wgpu surface, egui panels (world and season,
//! levels, layers, shading, camera, checks, selection, issues) over the rendered LOD.
//!
//! Controls: hold the right mouse button to look around, `W A S D` to move, `Q`/`E` down/up,
//! Shift fast, Ctrl slow, mouse wheel changes the speed; left click picks a shape; `M` toggles
//! the top-down overview (drag to pan, wheel to zoom, double-click to drop the camera there);
//! `1`-`4` show one level, `G` the game's arrangement; `F` flies to the selection; `C` runs the
//! checks; `F12` saves a screenshot next to the working directory; `Esc` clears the selection.

use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Receiver;
use std::time::Instant;

use anyhow::{Context, Result};
use glam::Vec3;
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition};
use winit::event::{
    DeviceEvent, DeviceId, ElementState, MouseButton, MouseScrollDelta, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

use crate::camera::{FreeCam, Overview, ViewParams};
use crate::checks::{self, Report, Severity, Tolerances};
use crate::files::Files;
use crate::lodselect::LodMode;
use crate::pick::{self, Hit};
use crate::render::textures::TexCache;
use crate::render::{Layouts, Renderer, Shading};
use crate::scene::{self, LoadEvent, LoadOptions, Loader, Scene};
use crate::viewstate::{self, FrameStats, ViewSettings};
use crate::world::{self, WorldFiles};

/// What the window shows.
pub struct ViewerConfig {
    pub files: Arc<dyn Files>,
    /// Where the LOD files are listed from (the output, or the output merged with the load order).
    pub lister: Arc<dyn Files>,
    pub world: String,
    pub season: Option<String>,
    /// Worlds the user can switch to.
    pub worlds: Vec<String>,
    pub load: LoadOptions,
    pub settings: ViewSettings,
    pub camera: Option<FreeCam>,
    pub title: String,
    /// Where `F12` screenshots go.
    pub screenshot_dir: std::path::PathBuf,
    /// The load order is attached behind the output (missing textures are then real).
    pub load_order: bool,
}

struct Gfx {
    window: Arc<Window>,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    gpu: Arc<lodforge_gpu::Gpu>,
    renderer: Renderer,
    egui_ctx: egui::Context,
    egui_state: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,
    can_copy: bool,
}

const SELECT_TINT: [f32; 4] = [1.0, 0.55, 0.1, 0.45];

struct App {
    cfg: ViewerConfig,
    gfx: Option<Gfx>,
    world: Arc<WorldFiles>,
    scene: Scene,
    cache: Option<Arc<TexCache>>,
    rx: Option<Receiver<LoadEvent>>,
    cancel: Arc<AtomicBool>,
    cam: FreeCam,
    cam_set: bool,
    overview: Overview,
    in_overview: bool,
    settings: ViewSettings,
    selection: Option<Hit>,
    report: Option<Report>,
    report_secs: f32,
    issue_min: Severity,
    issue_kind: String,
    keys: HashSet<KeyCode>,
    mouse_look: bool,
    dragging: bool,
    cursor: PhysicalPosition<f64>,
    last_click: Option<Instant>,
    pending_pick: Option<PhysicalPosition<f64>>,
    last_frame: Instant,
    speed: f32,
    fps: f32,
    stats: FrameStats,
    status: String,
    goto: String,
    world_pick: usize,
    season_pick: usize,
    screenshot: bool,
    show_help: bool,
    /// Requests made inside the UI pass (where the graphics state is borrowed out), applied
    /// right after it: a world/season switch and a selection change.
    pending_switch: Option<(String, Option<String>)>,
    pending_select: Option<Option<Hit>>,
    fatal: Option<anyhow::Error>,
}

/// Opens the viewer window and runs until it is closed.
pub fn run(cfg: ViewerConfig) -> Result<()> {
    let event_loop = EventLoop::new().context("creating the event loop")?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App::new(cfg);
    event_loop.run_app(&mut app).context("running the window")?;
    match app.fatal.take() {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

impl App {
    fn new(cfg: ViewerConfig) -> App {
        let world = Arc::new(world::discover(
            cfg.files.as_ref(),
            cfg.lister.as_ref(),
            &cfg.world,
            cfg.season.as_deref(),
        ));
        let world_pick = cfg
            .worlds
            .iter()
            .position(|w| w.eq_ignore_ascii_case(&cfg.world))
            .unwrap_or(0);
        let season_pick = cfg
            .season
            .as_ref()
            .and_then(|s| {
                world
                    .seasons
                    .iter()
                    .position(|x| x.eq_ignore_ascii_case(s))
                    .map(|i| i + 1)
            })
            .unwrap_or(0);
        let cam = cfg.camera.unwrap_or_default();
        let settings = cfg.settings.clone();
        App {
            cam,
            cam_set: cfg.camera.is_some(),
            settings,
            world,
            scene: Scene::new(0),
            cache: None,
            rx: None,
            cancel: Arc::new(AtomicBool::new(false)),
            gfx: None,
            overview: Overview::default(),
            in_overview: false,
            selection: None,
            report: None,
            report_secs: 0.0,
            issue_min: Severity::Info,
            issue_kind: String::new(),
            keys: HashSet::new(),
            mouse_look: false,
            dragging: false,
            cursor: PhysicalPosition::new(0.0, 0.0),
            last_click: None,
            pending_pick: None,
            last_frame: Instant::now(),
            speed: 3000.0,
            fps: 0.0,
            stats: FrameStats::default(),
            status: String::new(),
            goto: String::new(),
            world_pick,
            season_pick,
            screenshot: false,
            show_help: true,
            pending_switch: None,
            pending_select: None,
            fatal: None,
            cfg,
        }
    }

    fn init_gfx(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        let attrs = Window::default_attributes()
            .with_title(&self.cfg.title)
            .with_inner_size(LogicalSize::new(1600.0, 960.0));
        let window = Arc::new(
            event_loop
                .create_window(attrs)
                .context("creating the window")?,
        );
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
        let surface = instance
            .create_surface(window.clone())
            .context("creating the surface")?;
        let gpu = Arc::new(
            lodforge_gpu::Gpu::with_instance(
                &instance,
                Some(&surface),
                wgpu::Features::POLYGON_MODE_LINE,
            )
            .context("opening the GPU")?,
        );
        let caps = surface.get_capabilities(&gpu.adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);
        let can_copy = caps.usages.contains(wgpu::TextureUsages::COPY_SRC);
        let size = window.inner_size();
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | if can_copy {
                    wgpu::TextureUsages::COPY_SRC
                } else {
                    wgpu::TextureUsages::empty()
                },
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&gpu.device, &config);
        let layouts = Layouts::new(&gpu.device, &gpu.queue);
        let samples = 4.min(gpu.max_msaa);
        let renderer = Renderer::new(&gpu.device, layouts, format, samples);
        let egui_ctx = egui::Context::default();
        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            window.theme(),
            Some(2048),
        );
        let egui_renderer = egui_wgpu::Renderer::new(&gpu.device, format, None, 1, false);
        self.status = format!("{} ({:?})", gpu.adapter_info.name, gpu.adapter_info.backend);
        self.gfx = Some(Gfx {
            window,
            surface,
            config,
            gpu,
            renderer,
            egui_ctx,
            egui_state,
            egui_renderer,
            can_copy,
        });
        self.start_load();
        Ok(())
    }

    fn start_load(&mut self) {
        let Some(g) = &self.gfx else { return };
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::new(AtomicBool::new(false));
        let cache = Arc::new(TexCache::new(
            g.gpu.device.clone(),
            g.gpu.queue.clone(),
            g.gpu
                .device
                .features()
                .contains(wgpu::Features::TEXTURE_COMPRESSION_BC),
        ));
        let mut options = self.cfg.load.clone();
        options.start = if self.cam_set {
            [self.cam.pos[0], self.cam.pos[1]]
        } else {
            [0.0, 0.0]
        };
        let total = scene::total_items(&self.world, &options);
        self.scene = Scene::new(total);
        self.selection = None;
        self.report = None;
        self.rx = Some(scene::start(Loader {
            files: self.cfg.files.clone(),
            world: self.world.clone(),
            device: g.gpu.device.clone(),
            queue: g.gpu.queue.clone(),
            layouts: g.renderer.layouts.clone(),
            cache: cache.clone(),
            options,
            cancel: self.cancel.clone(),
        }));
        if let Some(g) = self.gfx.as_mut() {
            g.renderer.set_projected(&g.gpu.device, &cache, self.cfg.files.as_ref());
        }
        self.cache = Some(cache);
        self.status = format!("loading {} ({} files)", self.world.world, total);
    }

    fn switch_world(&mut self, world: String, season: Option<String>) {
        self.world = Arc::new(world::discover(
            self.cfg.files.as_ref(),
            self.cfg.lister.as_ref(),
            &world,
            season.as_deref(),
        ));
        self.cfg.world = world;
        self.cfg.season = season;
        self.cam_set = false;
        self.start_load();
    }

    fn drain_loader(&mut self) {
        let Some(rx) = &self.rx else { return };
        let mut n = 0;
        while let Ok(ev) = rx.try_recv() {
            if let LoadEvent::Message(m) = &ev {
                self.status = m.clone();
            }
            self.scene.apply(ev);
            n += 1;
            if n > 256 {
                break;
            }
        }
        if self.scene.done && self.rx.is_some() {
            self.rx = None;
            self.status = format!(
                "{}: {} files, {} triangles, {} MB of textures",
                self.world.world,
                self.scene.quads.len(),
                self.scene.triangles(),
                self.cache.as_ref().map_or(0, |c| c.bytes() >> 20)
            );
        }
        if !self.cam_set
            && !self.scene.bounds.is_empty()
            && (self.scene.done || self.scene.quads.len() * 4 >= self.scene.total.max(1))
        {
            self.fit_camera();
            self.cam_set = true;
        }
    }

    fn fit_camera(&mut self) {
        let b = self.scene.bounds;
        if b.is_empty() {
            return;
        }
        let c = b.center();
        let extent = (b.max[0] - b.min[0]).max(b.max[1] - b.min[1]);
        self.cam.pos = [c[0], c[1] - extent * 0.35, b.max[2] + extent * 0.25];
        self.cam.look_at([c[0], c[1], (b.min[2] + b.max[2]) * 0.5]);
        self.overview = crate::fit_overview(&self.scene, self.aspect());
    }

    fn aspect(&self) -> f32 {
        self.gfx.as_ref().map_or(1.6, |g| {
            g.config.width as f32 / g.config.height.max(1) as f32
        })
    }

    fn fly_to(&mut self, p: [f32; 3]) {
        let (sy, cy) = self.cam.yaw.to_radians().sin_cos();
        let d = 2500.0;
        self.cam.pos = [p[0] - sy * d, p[1] - cy * d, p[2] + 1200.0];
        self.cam.look_at(p);
        self.in_overview = false;
    }

    fn select(&mut self, hit: Option<Hit>) {
        let Some(g) = &self.gfx else { return };
        if let Some(q) = self
            .selection
            .and_then(|old| self.scene.quads.get_mut(old.quad).map(|q| (q, old.shape)))
        {
            q.0.set_tint(&g.gpu.queue, q.1, [0.0; 4]);
        }
        if let Some(q) = hit.and_then(|h| self.scene.quads.get_mut(h.quad).map(|q| (q, h.shape))) {
            q.0.set_tint(&g.gpu.queue, q.1, SELECT_TINT);
        }
        self.selection = hit;
    }

    fn run_checks(&mut self) {
        let t = Instant::now();
        let quads = self.scene.loaded_quads();
        let trees = self.scene.tree_data();
        let report = checks::run(
            self.cfg.files.as_ref(),
            &self.world,
            &quads,
            &trees,
            &Tolerances::default(),
            self.cfg.load_order,
        );
        self.report_secs = t.elapsed().as_secs_f32();
        self.status = format!(
            "checks: {} errors, {} warnings, {} notes in {:.1} s",
            report.summary.errors, report.summary.warnings, report.summary.infos, self.report_secs
        );
        self.report = Some(report);
    }

    fn view_params(&self) -> ViewParams {
        if self.in_overview {
            ViewParams::overview(&self.overview, self.aspect())
        } else {
            ViewParams::free(&self.cam, self.aspect())
        }
    }

    fn cam_xy(&self) -> [f32; 2] {
        if self.in_overview {
            self.overview.center
        } else {
            [self.cam.pos[0], self.cam.pos[1]]
        }
    }

    fn move_camera(&mut self, dt: f32) {
        if self.in_overview {
            return;
        }
        let mut v = Vec3::ZERO;
        let f = self.cam.forward();
        let r = self.cam.right();
        let k = |k: KeyCode| self.keys.contains(&k);
        if k(KeyCode::KeyW) || k(KeyCode::ArrowUp) {
            v += f;
        }
        if k(KeyCode::KeyS) || k(KeyCode::ArrowDown) {
            v -= f;
        }
        if k(KeyCode::KeyD) || k(KeyCode::ArrowRight) {
            v += r;
        }
        if k(KeyCode::KeyA) || k(KeyCode::ArrowLeft) {
            v -= r;
        }
        if k(KeyCode::KeyE) || k(KeyCode::Space) {
            v += Vec3::Z;
        }
        if k(KeyCode::KeyQ) {
            v -= Vec3::Z;
        }
        if v.length_squared() > 0.0 {
            let mut s = self.speed;
            if k(KeyCode::ShiftLeft) || k(KeyCode::ShiftRight) {
                s *= 5.0;
            }
            if k(KeyCode::ControlLeft) || k(KeyCode::ControlRight) {
                s *= 0.2;
            }
            let p = Vec3::from(self.cam.pos) + v.normalize() * s * dt;
            self.cam.pos = p.to_array();
        }
    }

    fn set_mouse_look(&mut self, on: bool) {
        let Some(g) = &self.gfx else { return };
        if on == self.mouse_look {
            return;
        }
        self.mouse_look = on;
        if on {
            let _ = g
                .window
                .set_cursor_grab(CursorGrabMode::Confined)
                .or_else(|_| g.window.set_cursor_grab(CursorGrabMode::Locked));
            g.window.set_cursor_visible(false);
        } else {
            let _ = g.window.set_cursor_grab(CursorGrabMode::None);
            g.window.set_cursor_visible(true);
        }
    }

    fn shortcut(&mut self, key: KeyCode) {
        match key {
            KeyCode::Digit1 => self.settings.mode = LodMode::Single(4),
            KeyCode::Digit2 => self.settings.mode = LodMode::Single(8),
            KeyCode::Digit3 => self.settings.mode = LodMode::Single(16),
            KeyCode::Digit4 => self.settings.mode = LodMode::Single(32),
            KeyCode::KeyG => self.settings.mode = LodMode::Game { distance_mult: 1.0 },
            KeyCode::KeyM => {
                if !self.in_overview && self.overview.half_height == Overview::default().half_height
                {
                    self.overview = crate::fit_overview(&self.scene, self.aspect());
                }
                self.in_overview = !self.in_overview;
                self.set_mouse_look(false);
            }
            KeyCode::KeyF => {
                if let Some(h) = self.selection {
                    self.fly_to(h.point);
                }
            }
            KeyCode::KeyC => self.run_checks(),
            KeyCode::F12 => self.screenshot = true,
            KeyCode::F1 => self.show_help = !self.show_help,
            KeyCode::Escape => {
                self.select(None);
                self.set_mouse_look(false);
            }
            _ => {}
        }
    }

    /// Drops the free camera where the overview was double-clicked.
    fn drop_camera(&mut self, px: f64, py: f64) {
        let Some(g) = &self.gfx else { return };
        let (w, h) = (g.config.width as f32, g.config.height as f32);
        let [x, y] = self.overview.world_at(px as f32, py as f32, w, h);
        let view = self.view_params();
        let (o, d) = view.ray(px as f32, py as f32, w, h);
        let visible =
            viewstate::visible_quads(&self.scene, &self.world, self.settings.mode, &self.settings.lod_distances, [x, y]);
        let layers = self.settings.layers;
        let ground = pick::pick(&self.scene, &visible, &|k, l| layers.shows(k, l), o, d)
            .map(|h| h.point[2])
            .unwrap_or(self.scene.bounds.center()[2]);
        self.cam.pos = [x, y - 3000.0, ground + 1500.0];
        self.cam.look_at([x, y, ground]);
        self.in_overview = false;
    }

    fn ui(&mut self, ctx: &egui::Context) {
        let mut switch: Option<(String, Option<String>)> = None;
        let mut fly: Option<[f32; 3]> = None;
        let mut copy: Option<String> = None;
        egui::TopBottomPanel::top("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(format!("{:.0} fps", self.fps));
                ui.separator();
                ui.label(format!(
                    "{} blocks, {} shapes, {} tris, {} trees",
                    self.stats.quads, self.stats.shapes, self.stats.triangles, self.stats.trees
                ));
                ui.separator();
                if !self.scene.done {
                    ui.add(
                        egui::ProgressBar::new(
                            self.scene.loaded() as f32 / self.scene.total.max(1) as f32,
                        )
                        .desired_width(160.0)
                        .text(format!(
                            "{}/{}",
                            self.scene.loaded(),
                            self.scene.total
                        )),
                    );
                }
                ui.label(&self.status);
            });
        });
        egui::SidePanel::left("controls").resizable(true).default_width(300.0).show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.heading("World");
                let worlds = self.cfg.worlds.clone();
                let mut wp = self.world_pick;
                egui::ComboBox::from_id_salt("world").selected_text(worlds.get(wp).cloned().unwrap_or_default()).show_ui(ui, |ui| {
                    for (i, w) in worlds.iter().enumerate() {
                        ui.selectable_value(&mut wp, i, w);
                    }
                });
                let mut sp = self.season_pick;
                let seasons = self.world.seasons.clone();
                egui::ComboBox::from_id_salt("season").selected_text(if sp == 0 { "default season".to_string() } else { seasons.get(sp - 1).cloned().unwrap_or_default() }).show_ui(ui, |ui| {
                    ui.selectable_value(&mut sp, 0, "default season");
                    for (i, s) in seasons.iter().enumerate() {
                        ui.selectable_value(&mut sp, i + 1, s);
                    }
                });
                if wp != self.world_pick || sp != self.season_pick {
                    self.world_pick = wp;
                    self.season_pick = sp;
                    let season = (sp > 0).then(|| seasons.get(sp - 1).cloned()).flatten();
                    switch = worlds.get(wp).map(|w| (w.clone(), season));
                }
                ui.label(format!("{} terrain, {} object, {} tree blocks; levels {:?}", self.world.terrain.len(), self.world.objects.len(), self.world.trees.len(), self.world.levels()));
                if let Some(l) = self.world.lodsettings {
                    ui.label(format!("lodsettings: SW {},{}  levels {}..{}", l.south_west.0, l.south_west.1, l.min_level, l.max_level));
                } else {
                    ui.label("no lodsettings file (grid origin taken from the files)");
                }
                ui.separator();
                ui.heading("Levels");
                let mut single = match self.settings.mode {
                    LodMode::Single(l) => Some(l),
                    _ => None,
                };
                ui.horizontal(|ui| {
                    if ui.selectable_label(single.is_none(), "game").clicked() {
                        self.settings.mode = LodMode::Game { distance_mult: 1.0 };
                        single = None;
                    }
                    for l in [4, 8, 16, 32] {
                        if ui.selectable_label(single == Some(l), format!("LOD{l}")).clicked() {
                            self.settings.mode = LodMode::Single(l);
                        }
                    }
                });
                if let LodMode::Game { distance_mult } = &mut self.settings.mode {
                    ui.add(egui::Slider::new(distance_mult, 0.1..=6.0).logarithmic(true).text("switch distance x"));
                }
                ui.separator();
                ui.heading("Layers");
                let l = &mut self.settings.layers;
                ui.horizontal_wrapped(|ui| {
                    ui.checkbox(&mut l.terrain, "terrain");
                    ui.checkbox(&mut l.water, "water");
                    ui.checkbox(&mut l.objects, "objects");
                    ui.checkbox(&mut l.tree_billboards, "tree billboards");
                    ui.checkbox(&mut l.grass, "grass");
                    ui.checkbox(&mut l.trees, "classic trees");
                    ui.checkbox(&mut l.large_refs_only, "large refs only");
                });
                ui.separator();
                ui.heading("Shading");
                egui::ComboBox::from_id_salt("shading").selected_text(self.settings.shading.label()).show_ui(ui, |ui| {
                    for s in Shading::ALL {
                        ui.selectable_value(&mut self.settings.shading, s, s.label());
                    }
                });
                ui.checkbox(&mut self.settings.normal_maps, "normal maps");
                ui.checkbox(&mut self.settings.atmosphere, "weather sky / fog / water")
                    .on_hover_text(format!("{} with {} (free camera)", self.settings.atmosphere_data.weather, self.settings.atmosphere_data.water));
                ui.checkbox(&mut self.settings.snow, "LOD snow / ash")
                    .on_hover_text("The directional snow / ash material the game projects onto objSnow / objAsh shapes");
                let mut alpha_on = self.settings.alpha_override.is_some();
                ui.horizontal(|ui| {
                    ui.checkbox(&mut alpha_on, "force alpha test");
                    let mut v = self.settings.alpha_override.unwrap_or(0.5);
                    if alpha_on {
                        ui.add(egui::Slider::new(&mut v, 0.0..=1.0));
                        self.settings.alpha_override = Some(v);
                    } else {
                        self.settings.alpha_override = None;
                    }
                });
                ui.add(egui::Slider::new(&mut self.settings.sun_azimuth, 0.0..=360.0).text("sun heading"));
                ui.add(egui::Slider::new(&mut self.settings.sun_elevation, 2.0..=90.0).text("sun elevation"));
                ui.add(egui::Slider::new(&mut self.settings.ambient, 0.0..=1.0).text("ambient"));
                ui.checkbox(&mut self.settings.grid, "block outlines");
                ui.checkbox(&mut self.settings.bounds, "selection bounds");
                ui.separator();
                ui.heading("Camera");
                ui.horizontal(|ui| {
                    if ui.selectable_label(!self.in_overview, "free").clicked() {
                        self.in_overview = false;
                    }
                    if ui.selectable_label(self.in_overview, "overview (M)").clicked() {
                        if !self.in_overview {
                            self.overview = crate::fit_overview(&self.scene, self.aspect());
                        }
                        self.in_overview = true;
                    }
                    if ui.button("fit world").clicked() {
                        self.fit_camera();
                    }
                });
                ui.monospace(self.cam.format());
                ui.add(egui::Slider::new(&mut self.cam.fov, 20.0..=120.0).text("fov"));
                ui.add(egui::Slider::new(&mut self.speed, 100.0..=100000.0).logarithmic(true).text("speed"));
                ui.horizontal(|ui| {
                    if ui.button("copy camera").clicked() {
                        copy = Some(self.cam.format());
                    }
                    ui.add(egui::TextEdit::singleline(&mut self.goto).hint_text("x,y,z[,yaw,pitch]").desired_width(150.0));
                    if ui.button("go").clicked()
                        && let Some(c) = FreeCam::parse(&self.goto)
                    {
                        self.cam.pos = c.pos;
                        if self.goto.split(',').count() >= 5 {
                            self.cam.yaw = c.yaw;
                            self.cam.pitch = c.pitch;
                        }
                        self.in_overview = false;
                    }
                });
                ui.separator();
                ui.heading("Checks");
                ui.horizontal(|ui| {
                    if ui.add_enabled(self.scene.done, egui::Button::new("run checks (C)")).clicked() {
                        self.run_checks();
                    }
                    if ui.button("screenshot (F12)").clicked() {
                        self.screenshot = true;
                    }
                });
                if let Some(r) = &self.report {
                    ui.label(format!("{} errors, {} warnings, {} notes ({:.1} s)", r.summary.errors, r.summary.warnings, r.summary.infos, self.report_secs));
                }
                if let Some(c) = &self.cache {
                    let missing = c.missing();
                    if !missing.is_empty() {
                        ui.colored_label(egui::Color32::from_rgb(255, 120, 120), format!("{} textures missing", missing.len()));
                    }
                }
                if !self.scene.messages.is_empty() {
                    ui.collapsing(format!("loader messages ({})", self.scene.messages.len()), |ui| {
                        for m in &self.scene.messages {
                            ui.label(m);
                        }
                    });
                }
                ui.separator();
                ui.collapsing("Help (F1)", |ui| {
                    ui.label("Right mouse: look. W A S D: move, Q/E: down/up, Shift: fast, Ctrl: slow, wheel: speed.");
                    ui.label("Left click: pick a shape. F: fly to the selection. Esc: clear.");
                    ui.label("M: top-down overview (drag to pan, wheel to zoom, double-click to drop the camera).");
                    ui.label("1-4: one LOD level. G: the game's arrangement. C: run checks. F12: screenshot.");
                });
            });
        });
        if let Some(h) = self.selection {
            egui::SidePanel::right("selection")
                .resizable(true)
                .default_width(360.0)
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        ui.heading("Selection");
                        let Some(q) = self.scene.quads.get(h.quad) else {
                            return;
                        };
                        let Some(sh) = q.shapes.get(h.shape) else {
                            return;
                        };
                        let cpu = &q.data.shapes[sh.cpu];
                        let m = &cpu.material;
                        ui.monospace(&q.data.path);
                        egui::Grid::new("sel")
                            .num_columns(2)
                            .striped(true)
                            .show(ui, |ui| {
                                let row = |ui: &mut egui::Ui, k: &str, v: String| {
                                    ui.label(k);
                                    ui.label(v);
                                    ui.end_row();
                                };
                                row(
                                    ui,
                                    "block",
                                    format!(
                                        "LOD{} {},{}",
                                        q.data.quad.level, q.data.quad.x, q.data.quad.y
                                    ),
                                );
                                row(
                                    ui,
                                    "shape",
                                    format!(
                                        "[{}] {}",
                                        cpu.block,
                                        if m.name.is_empty() {
                                            "(unnamed)"
                                        } else {
                                            &m.name
                                        }
                                    ),
                                );
                                row(
                                    ui,
                                    "kind",
                                    format!(
                                        "{}{}",
                                        cpu.kind.label(),
                                        if m.large_ref { ", large reference" } else { "" }
                                    ),
                                );
                                row(
                                    ui,
                                    "geometry",
                                    format!(
                                        "{} vertices, {} triangles{}",
                                        cpu.vertex_count(),
                                        cpu.triangle_count(),
                                        if cpu.segments.is_empty() {
                                            String::new()
                                        } else {
                                            format!(", {} segments", cpu.segments.len())
                                        }
                                    ),
                                );
                                row(
                                    ui,
                                    "hit",
                                    format!(
                                        "triangle {} at {:.0},{:.0},{:.0}",
                                        h.triangle, h.point[0], h.point[1], h.point[2]
                                    ),
                                );
                                row(
                                    ui,
                                    "shader",
                                    format!(
                                        "{} type {}, flags {:#010x} / {:#010x}",
                                        if m.effect { "effect" } else { "lighting" },
                                        m.shader_type,
                                        m.flags1,
                                        m.flags2
                                    ),
                                );
                                row(
                                    ui,
                                    "alpha",
                                    format!(
                                        "{}{}{}",
                                        if m.alpha_test {
                                            format!("test > {}", m.threshold)
                                        } else {
                                            "no test".into()
                                        },
                                        if m.alpha_blend { ", blend" } else { "" },
                                        if m.double_sided { ", double-sided" } else { "" }
                                    ),
                                );
                                row(
                                    ui,
                                    "vertex colours",
                                    format!(
                                        "{}{}",
                                        if m.vertex_colors {
                                            "shader on"
                                        } else {
                                            "shader off"
                                        },
                                        if cpu.colors.is_empty() {
                                            ""
                                        } else {
                                            ", present"
                                        }
                                    ),
                                );
                                if m.own_emit || m.glow_map {
                                    row(
                                        ui,
                                        "emissive",
                                        format!(
                                            "{:?} x {}{}",
                                            m.emissive,
                                            m.emissive_multiple,
                                            if m.glow_map { ", glow map" } else { "" }
                                        ),
                                    );
                                }
                                if m.uv_scale != [1.0, 1.0] || m.uv_offset != [0.0, 0.0] {
                                    row(
                                        ui,
                                        "uv transform",
                                        format!("scale {:?} offset {:?}", m.uv_scale, m.uv_offset),
                                    );
                                }
                                row(
                                    ui,
                                    "bound sphere",
                                    format!(
                                        "{:.0},{:.0},{:.0} r {:.0}",
                                        cpu.bound_center[0],
                                        cpu.bound_center[1],
                                        cpu.bound_center[2],
                                        cpu.bound_radius
                                    ),
                                );
                                row(
                                    ui,
                                    "vertex box",
                                    format!(
                                        "{:.0},{:.0},{:.0} .. {:.0},{:.0},{:.0}",
                                        cpu.aabb.min[0],
                                        cpu.aabb.min[1],
                                        cpu.aabb.min[2],
                                        cpu.aabb.max[0],
                                        cpu.aabb.max[1],
                                        cpu.aabb.max[2]
                                    ),
                                );
                                if let Some(b) = cpu.multibound {
                                    row(
                                        ui,
                                        "multibound",
                                        format!(
                                            "{:.0},{:.0},{:.0} .. {:.0},{:.0},{:.0}",
                                            b.min[0],
                                            b.min[1],
                                            b.min[2],
                                            b.max[0],
                                            b.max[1],
                                            b.max[2]
                                        ),
                                    );
                                }
                            });
                        ui.separator();
                        ui.label("Textures");
                        let files = self.cfg.files.clone();
                        let names = [
                            "diffuse",
                            "normal",
                            "glow",
                            "parallax/height",
                            "env",
                            "env mask",
                            "multilayer/inner",
                            "specular/subsurface",
                            "backlight",
                        ];
                        let mut shown = false;
                        for (i, t) in m.textures.iter().enumerate() {
                            if t.trim().is_empty() {
                                continue;
                            }
                            shown = true;
                            let n = crate::files::normalize(t);
                            match files.origin(&n) {
                                Some(o) => {
                                    ui.label(format!(
                                        "{}: {t}",
                                        names.get(i).copied().unwrap_or("slot")
                                    ));
                                    ui.small(format!("   from {o}"));
                                }
                                None => {
                                    ui.colored_label(
                                        egui::Color32::from_rgb(255, 120, 120),
                                        format!(
                                            "{}: {t}  MISSING",
                                            names.get(i).copied().unwrap_or("slot")
                                        ),
                                    );
                                }
                            }
                        }
                        if cpu.kind == crate::mesh::ShapeKind::Terrain {
                            let (d, n) = self.world.terrain_textures(files.as_ref(), &q.data.quad);
                            for (label, p) in [("terrain diffuse", d), ("terrain normal", n)] {
                                if files.exists(&p) {
                                    ui.label(format!("{label}: {p}"));
                                } else {
                                    ui.colored_label(
                                        egui::Color32::from_rgb(255, 120, 120),
                                        format!("{label}: {p}  MISSING"),
                                    );
                                }
                            }
                            shown = true;
                        }
                        if !shown {
                            ui.label("(none)");
                        }
                        if let (Some(d), true) = (&sh.textures.diffuse, true) {
                            ui.small(format!(
                                "GPU diffuse {}x{}, {} mips{}, {:?}",
                                d.width,
                                d.height,
                                d.mips,
                                if d.dropped > 0 {
                                    format!(" ({} dropped)", d.dropped)
                                } else {
                                    String::new()
                                },
                                d.format
                            ));
                        }
                        ui.separator();
                        ui.horizontal(|ui| {
                            if ui.button("fly to (F)").clicked() {
                                fly = Some(h.point);
                            }
                            if ui.button("copy path").clicked() {
                                copy = Some(q.data.path.clone());
                            }
                            if ui.button("clear (Esc)").clicked() {
                                fly = Some([f32::NAN; 3]);
                            }
                        });
                    });
                });
        }
        if let Some(r) = &self.report {
            egui::TopBottomPanel::bottom("issues")
                .resizable(true)
                .default_height(220.0)
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.heading(format!(
                            "Issues: {} errors, {} warnings, {} notes",
                            r.summary.errors, r.summary.warnings, r.summary.infos
                        ));
                        ui.separator();
                        for s in [Severity::Error, Severity::Warning, Severity::Info] {
                            if ui
                                .selectable_label(self.issue_min == s, s.label())
                                .clicked()
                            {
                                self.issue_min = s;
                            }
                        }
                        ui.separator();
                        ui.label("kind filter");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.issue_kind).desired_width(140.0),
                        );
                        if ui.button("save JSON").clicked() {
                            let p = self.cfg.screenshot_dir.join(format!(
                                "lodforge-check-{}{}.json",
                                r.world,
                                r.season
                                    .as_ref()
                                    .map(|s| format!("-{s}"))
                                    .unwrap_or_default()
                            ));
                            match serde_json::to_vec_pretty(r)
                                .map_err(|e| e.to_string())
                                .and_then(|b| std::fs::write(&p, b).map_err(|e| e.to_string()))
                            {
                                Ok(()) => self.status = format!("wrote {}", p.display()),
                                Err(e) => self.status = format!("saving {}: {e}", p.display()),
                            }
                        }
                    });
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        let kind = self.issue_kind.to_ascii_lowercase();
                        for (i, issue) in r
                            .issues
                            .iter()
                            .enumerate()
                            .filter(|(_, i)| {
                                i.severity >= self.issue_min
                                    && (kind.is_empty() || i.kind.contains(&kind))
                            })
                            .take(2000)
                        {
                            let color = match issue.severity {
                                Severity::Error => egui::Color32::from_rgb(255, 110, 110),
                                Severity::Warning => egui::Color32::from_rgb(255, 200, 90),
                                Severity::Info => egui::Color32::GRAY,
                            };
                            ui.horizontal(|ui| {
                                ui.colored_label(color, issue.kind);
                                if let Some(p) = issue.position
                                    && ui.small_button("go").clicked()
                                {
                                    fly = Some(p);
                                }
                                ui.label(&issue.message);
                            });
                            let _ = i;
                        }
                    });
                });
        }
        if let Some(text) = copy {
            ctx.copy_text(text);
            self.status = "copied".into();
        }
        if let Some(p) = fly {
            if p[0].is_nan() {
                self.pending_select = Some(None);
            } else {
                self.fly_to(p);
            }
        }
        if switch.is_some() {
            self.pending_switch = switch;
        }
    }

    /// Applies what the UI pass asked for, once the graphics state is back in place.
    fn apply_pending(&mut self) {
        if let Some(sel) = self.pending_select.take() {
            self.select(sel);
        }
        if let Some((w, s)) = self.pending_switch.take() {
            self.switch_world(w, s);
        }
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f32().min(0.25);
        self.last_frame = now;
        self.fps = self.fps * 0.9 + (1.0 / dt.max(1e-4)) * 0.1;
        self.drain_loader();
        self.move_camera(dt);
        let Some(mut g) = self.gfx.take() else { return };
        let (w, h) = (g.config.width, g.config.height);
        // UI first: it may change the settings this frame draws with.
        let raw = g.egui_state.take_egui_input(&g.window);
        let full = g.egui_ctx.run(raw, |ctx| self.ui(ctx));
        g.egui_state
            .handle_platform_output(&g.window, full.platform_output);
        if self.pending_select.is_some() || self.pending_switch.is_some() {
            self.gfx = Some(g);
            self.apply_pending();
            g = self.gfx.take().unwrap();
        }
        let jobs = g.egui_ctx.tessellate(full.shapes, full.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [w, h],
            pixels_per_point: full.pixels_per_point,
        };
        // Scene.
        let view = self.view_params();
        let visible =
            viewstate::visible_quads(&self.scene, &self.world, self.settings.mode, &self.settings.lod_distances, self.cam_xy());
        if let Some(p) = self.pending_pick.take() {
            let (o, d) = view.ray(p.x as f32, p.y as f32, w as f32, h as f32);
            let layers = self.settings.layers;
            let hit = pick::pick(&self.scene, &visible, &|k, l| layers.shows(k, l), o, d);
            self.gfx = Some(g);
            self.select(hit);
            g = self.gfx.take().unwrap();
            if let Some(hit) = hit {
                let q = &self.scene.quads[hit.quad];
                let sh = &q.shapes[hit.shape];
                self.status = format!(
                    "{} [{}] {}",
                    q.data.path, q.data.shapes[sh.cpu].block, q.data.shapes[sh.cpu].material.name
                );
            }
        }
        let surface_frame = match g.surface.get_current_texture() {
            Ok(f) => f,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                g.surface.configure(&g.gpu.device, &g.config);
                self.gfx = Some(g);
                return;
            }
            Err(wgpu::SurfaceError::OutOfMemory) => {
                self.fatal = Some(anyhow::anyhow!("the GPU ran out of memory"));
                event_loop.exit();
                self.gfx = Some(g);
                return;
            }
            Err(_) => {
                self.gfx = Some(g);
                return;
            }
        };
        let target = surface_frame.texture.create_view(&Default::default());
        let mut encoder = g
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        {
            let (frame, globals, stats) = viewstate::build_frame(
                &self.scene,
                &visible,
                &self.settings,
                &view,
                self.selection,
            );
            self.stats = stats;
            g.renderer.render(
                &g.gpu.device,
                &g.gpu.queue,
                &mut encoder,
                &target,
                w,
                h,
                &globals,
                &frame,
            );
        }
        for (id, delta) in &full.textures_delta.set {
            g.egui_renderer
                .update_texture(&g.gpu.device, &g.gpu.queue, *id, delta);
        }
        let cmds = g.egui_renderer.update_buffers(
            &g.gpu.device,
            &g.gpu.queue,
            &mut encoder,
            &jobs,
            &screen,
        );
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("egui"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            let mut pass = pass.forget_lifetime();
            g.egui_renderer.render(&mut pass, &jobs, &screen);
        }
        for id in &full.textures_delta.free {
            g.egui_renderer.free_texture(id);
        }
        let shot = if self.screenshot && g.can_copy {
            self.screenshot = false;
            let bpr = (w * 4).div_ceil(256) * 256;
            let buf = g.gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("screenshot"),
                size: u64::from(bpr) * u64::from(h),
                usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: &surface_frame.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &buf,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(bpr),
                        rows_per_image: Some(h),
                    },
                },
                wgpu::Extent3d {
                    width: w,
                    height: h,
                    depth_or_array_layers: 1,
                },
            );
            Some((buf, bpr))
        } else {
            if self.screenshot {
                self.status = "screenshots are not supported on this surface".into();
                self.screenshot = false;
            }
            None
        };
        g.gpu
            .queue
            .submit(cmds.into_iter().chain([encoder.finish()]));
        g.window.pre_present_notify();
        surface_frame.present();
        if let Some((buf, bpr)) = shot {
            let (tx, rx) = std::sync::mpsc::channel();
            buf.slice(..).map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
            let _ = g.gpu.device.poll(wgpu::PollType::Wait);
            if rx.recv().ok().and_then(Result::ok).is_some() {
                let data = buf.slice(..).get_mapped_range();
                let mut rgba = Vec::with_capacity((w * h * 4) as usize);
                for row in 0..h as usize {
                    rgba.extend_from_slice(
                        &data[row * bpr as usize..row * bpr as usize + w as usize * 4],
                    );
                }
                drop(data);
                buf.unmap();
                if matches!(
                    g.config.format,
                    wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb
                ) {
                    crate::headless::bgra_to_rgba(&mut rgba);
                }
                let name = format!(
                    "lodforge-view-{}-{}.png",
                    self.world.world,
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0)
                );
                let p = self.cfg.screenshot_dir.join(name);
                match crate::headless::encode_png(&rgba, w, h)
                    .and_then(|png| std::fs::write(&p, png).map_err(|e| e.to_string()))
                {
                    Ok(()) => self.status = format!("saved {}", p.display()),
                    Err(e) => self.status = format!("screenshot failed: {e}"),
                }
            }
        }
        self.gfx = Some(g);
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gfx.is_none()
            && let Err(e) = self.init_gfx(event_loop)
        {
            self.fatal = Some(e);
            event_loop.exit();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let (consumed, wants_pointer, wants_keys) = match &mut self.gfx {
            Some(g) => {
                let r = g.egui_state.on_window_event(&g.window, &event);
                (
                    r.consumed,
                    g.egui_ctx.wants_pointer_input() || g.egui_ctx.is_pointer_over_area(),
                    g.egui_ctx.wants_keyboard_input(),
                )
            }
            None => (false, false, false),
        };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(g) = &mut self.gfx {
                    g.config.width = size.width.max(1);
                    g.config.height = size.height.max(1);
                    g.surface.configure(&g.gpu.device, &g.config);
                }
            }
            WindowEvent::RedrawRequested => self.redraw(event_loop),
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    match event.state {
                        ElementState::Pressed => {
                            if !wants_keys {
                                self.keys.insert(code);
                                if !event.repeat {
                                    self.shortcut(code);
                                }
                            }
                        }
                        ElementState::Released => {
                            self.keys.remove(&code);
                        }
                    }
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                if self.in_overview
                    && self.dragging
                    && let Some(g) = &self.gfx
                {
                    let h = g.config.height.max(1) as f64;
                    let units_per_px = (2.0 * f64::from(self.overview.half_height)) / h;
                    self.overview.center[0] -= ((position.x - self.cursor.x) * units_per_px) as f32;
                    self.overview.center[1] += ((position.y - self.cursor.y) * units_per_px) as f32;
                }
                self.cursor = position;
            }
            WindowEvent::MouseInput { state, button, .. } => match (button, state) {
                (MouseButton::Right, ElementState::Pressed)
                    if !wants_pointer && !self.in_overview =>
                {
                    self.set_mouse_look(true)
                }
                (MouseButton::Right, ElementState::Released) => self.set_mouse_look(false),
                (MouseButton::Left, ElementState::Pressed) if !consumed && !wants_pointer => {
                    let now = Instant::now();
                    let double = self
                        .last_click
                        .is_some_and(|t| now.duration_since(t).as_millis() < 350);
                    self.last_click = Some(now);
                    if self.in_overview {
                        if double {
                            self.dragging = false;
                            self.drop_camera(self.cursor.x, self.cursor.y);
                        } else {
                            self.dragging = true;
                        }
                    } else {
                        self.pending_pick = Some(self.cursor);
                    }
                }
                (MouseButton::Left, ElementState::Released) => self.dragging = false,
                _ => {}
            },
            WindowEvent::MouseWheel { delta, .. } if !wants_pointer => {
                let steps = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => (p.y / 40.0) as f32,
                };
                if self.in_overview {
                    let factor = 1.15f32.powf(-steps);
                    if let Some(g) = &self.gfx {
                        // Zoom about the cursor.
                        let (w, h) = (g.config.width as f32, g.config.height as f32);
                        let before = self.overview.world_at(
                            self.cursor.x as f32,
                            self.cursor.y as f32,
                            w,
                            h,
                        );
                        self.overview.half_height =
                            (self.overview.half_height * factor).clamp(500.0, 5.0e6);
                        let after = self.overview.world_at(
                            self.cursor.x as f32,
                            self.cursor.y as f32,
                            w,
                            h,
                        );
                        self.overview.center[0] += before[0] - after[0];
                        self.overview.center[1] += before[1] - after[1];
                    }
                } else {
                    self.speed = (self.speed * 1.25f32.powf(steps)).clamp(100.0, 100000.0);
                }
            }
            WindowEvent::Focused(false) => {
                self.keys.clear();
                self.set_mouse_look(false);
            }
            _ => {}
        }
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: DeviceId,
        event: DeviceEvent,
    ) {
        if let DeviceEvent::MouseMotion { delta } = event
            && self.mouse_look
        {
            self.cam.turn(delta.0 as f32, delta.1 as f32, 0.12);
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(g) = &self.gfx {
            g.window.request_redraw();
        }
    }
}
