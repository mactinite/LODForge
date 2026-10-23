//! The terrain lane of `lodforge build`: terrain meshes and textures of every worldspace and
//! season on a thread of their own, beside the object, tree and DynDOLOD stages. Terrain reads
//! only the load order, so the lane opens its own view of the profile and shares one
//! [TerrainContext](crate::terrain_textures::TerrainContext) (GPU device, decoded LAND, shrunk
//! landscape textures) across all its runs. The main thread waits for a worldspace's meshes
//! before the stages that clip against them (object LOD, underside) and for the whole lane
//! before the stages that read the terrain LOD through the VFS (plugins, occlusion).

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;

use crate::profile::ProfileArgs;
use crate::terrain_textures::{TerrainContext, TerrainMips};

/// What the lane thread needs (owned: the main thread keeps mutating its own profile).
#[derive(Clone)]
pub struct LaneArgs {
    pub profile: ProfileArgs,
    pub out: PathBuf,
    pub worlds: Vec<String>,
    /// Extra seasons (codes as given on the command line).
    pub seasons: Vec<String>,
    pub mips: TerrainMips,
    pub filter: crate::terrain_textures::TerrainFilter,
    pub lod32: crate::terrain_textures::Lod32,
    pub json: bool,
    /// Colour scale for PBR landscape albedo (`terrain_textures::Args::pbr_scale`).
    pub pbr_scale: f32,
    /// Fill cells without LAND from the shipped terrain LOD meshes (`--no-lod-fill` clears it).
    pub lod_fill: bool,
}

#[derive(Default)]
struct State {
    /// Worldspaces whose terrain meshes stage has ended (written, or failed).
    meshes_done: HashSet<String>,
    finished: bool,
    failed: Vec<String>,
}

struct Shared {
    state: Mutex<State>,
    cv: Condvar,
}

/// A running lane; dropping it joins the thread.
pub struct TerrainLane {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl TerrainLane {
    pub fn start(args: LaneArgs) -> TerrainLane {
        let shared = Arc::new(Shared { state: Mutex::new(State::default()), cv: Condvar::new() });
        let s = shared.clone();
        let thread = std::thread::Builder::new().name("terrain-lane".into()).spawn(move || run(&args, &s)).expect("spawning the terrain lane");
        TerrainLane { shared, thread: Some(thread) }
    }

    /// Blocks until the worldspace's terrain meshes stage has ended (or the lane has).
    pub fn wait_meshes(&self, world: &str) {
        let mut st = self.shared.state.lock().unwrap();
        while !st.finished && !st.meshes_done.contains(world) {
            st = self.shared.cv.wait(st).unwrap();
        }
    }

    /// Joins the lane and appends its stage failures to `failed`.
    pub fn finish(mut self, failed: &mut Vec<String>) {
        self.join();
        failed.append(&mut self.shared.state.lock().unwrap().failed);
    }

    fn join(&mut self) {
        if let Some(t) = self.thread.take() {
            if t.join().is_err() {
                let mut st = self.shared.state.lock().unwrap();
                st.failed.push("terrain lane: thread panicked".into());
                st.finished = true;
                self.shared.cv.notify_all();
            }
        }
    }
}

impl Drop for TerrainLane {
    fn drop(&mut self) {
        self.join();
    }
}

fn run(a: &LaneArgs, shared: &Shared) {
    let fail = |msg: String| {
        eprintln!("{msg}");
        shared.state.lock().unwrap().failed.push(msg);
    };
    let meshes_done = |world: &str| {
        shared.state.lock().unwrap().meshes_done.insert(world.to_string());
        shared.cv.notify_all();
    };
    let step = |world: &str, stage: &str, season: Option<&str>, f: &mut dyn FnMut() -> anyhow::Result<()>| {
        let t = Instant::now();
        crate::build::begin(a.json, world, stage, season);
        match f() {
            Ok(()) => crate::build::event(a.json, world, stage, season, t.elapsed().as_millis()),
            Err(e) => fail(format!("{world} {stage}: {e:#}")),
        }
    };
    // Mesh variants keep the season codes as given; texture names use them upper case (as the
    // build did when it ran the stages in line).
    let seasons: Vec<Option<String>> = std::iter::once(None).chain(a.seasons.iter().map(|s| Some(s.to_uppercase()))).collect();
    let profile = a.profile.open();
    let setup = profile.as_ref().map_err(|e| anyhow::anyhow!("{e:#}")).and_then(TerrainContext::new);
    match setup {
        Ok(ctx) => {
            // Every worldspace's meshes first (seconds in all): the object stages wait on them.
            for world in &a.worlds {
                let args = crate::terrain_meshes::Args { profile: a.profile.clone(), world: world.clone(), out: a.out.clone(), seasons: a.seasons.clone(), settings: None, golden: None, no_lod_fill: !a.lod_fill, lod32_quality: a.lod32.mesh_quality() };
                step(world, "terrain-meshes", None, &mut || crate::terrain_meshes::run_with(ctx.p, &args));
                meshes_done(world);
            }
            for world in &a.worlds {
                for season in &seasons {
                    let args = crate::terrain_textures::Args {
                        profile: a.profile.clone(),
                        world: world.clone(),
                        levels: vec![4, 8, 16, 32],
                        kinds: vec!["diffuse".into(), "normal".into()],
                        out: a.out.clone(),
                        season: season.clone(),
                        delta_from: season.as_ref().map(|_| a.out.clone()),
                        golden: None,
                        top: None,
                        batch: 96,
                        mips: a.mips,
                        filter: a.filter,
                        lod32: a.lod32,
                        pbr_scale: a.pbr_scale,
                    };
                    step(world, "terrain-textures", season.as_deref(), &mut || crate::terrain_textures::run_in(&ctx, &args));
                }
            }
        }
        Err(e) => {
            fail(format!("terrain: {e:#}"));
            for world in &a.worlds {
                meshes_done(world);
            }
        }
    }
    shared.state.lock().unwrap().finished = true;
    shared.cv.notify_all();
}
