// SPDX-License-Identifier: GPL-3.0-only
// LODForge is free software under the GNU General Public License v3.0 only; see LICENSE.
// Third-party components and their notices: THIRD-PARTY-NOTICES.txt.
//! LODForge command line.

#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod btr_compare;
mod dyndolod_build;
mod e2e_compare;
mod large_refs;
mod plugin_dump;
mod child_copies;
mod dyndolod_export;
mod grass_types;
mod occlusion;
mod build;
mod info;
mod lod_models;
mod lod_sources;
mod tree_report;
mod tree_models;
mod inspect;
mod glow_atlas;
mod object_atlas;
mod object_atlas_plan;
mod pbr_linear;
mod runtime;
mod object_export;
mod object_lod;
mod profile;
mod scan;
mod season_delta;
mod terrain_bake;
mod terrain_lane;
mod terrain_meshes;
mod terrain_textures;
mod terrain_underside;
mod texgen_billboards;
mod texgen_rendered;
mod texgen_stitched;
mod tree_lod;
mod view;

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};

use profile::ProfileArgs;

#[derive(Parser)]
#[command(name = "lodforge", version, about = "Fast LOD generation for Skyrim SE")]
struct Cli {
    /// BC7 encoder for every texture written: `high` (default; all modes, CPU), `best` (slower,
    /// thorough search) or `fast` (GPU, mode 6 only: blocky on gradients and foliage).
    #[arg(long, global = true, value_enum, default_value = "high")]
    bc7: Bc7Arg,
    /// Where the `high` and `best` BC7 encoders run: `gpu` (default; a port of the CPU encoder
    /// that writes the same blocks, ~10x faster) or `cpu`.
    #[arg(long, global = true, value_enum, default_value = "gpu")]
    bc7_device: Bc7Device,
    /// Billboard resolution multiplier (tree, trunk and grass billboards): 0.5 = half the pixels
    /// per side, 2 = double. 1 is TexGen's default (11 units per pixel, at most 1024 px).
    #[arg(long, global = true, default_value_t = 1.0)]
    billboard_scale: f32,
    /// Billboard supersampling per axis (tree, shrub, trunk and grass billboards): each billboard
    /// is rendered this many times larger and averaged down, and keeps its soft edge coverage as
    /// alpha (smooth silhouettes, thin foliage that fades instead of breaking into pixel noise).
    /// 1 = TexGen's MSAA render with a hard alpha cut; 2..8 (default 4).
    #[arg(long, global = true, default_value_t = lodforge_texgen::render::DEFAULT_SUPERSAMPLE)]
    billboard_supersample: u32,
    /// Billboards that a mod installs ready-made (e.g. HD LODs Textures) for bases TexGen's rule
    /// would not render: `render` (default; render them too, so every tree and shrub gets the
    /// same supersampled quality) or `keep` (use the installed ones, as TexGen).
    #[arg(long, global = true, value_enum, default_value = "render")]
    installed_billboards: InstalledBillboards,
    /// Opaque share of tree billboards in object and tree LOD, relative to the rendered coverage
    /// (0.1..1, default 1). Crossed billboard planes and small mips make distant trees denser than
    /// the full trees; below 1 lets light through the crowns so the switch shows less.
    #[arg(long, global = true, default_value_t = 1.0)]
    billboard_density: f32,
    /// Keep tree billboards' rescaled soft alpha instead of dithering it into on/off alpha.
    #[arg(long, global = true)]
    billboard_soft: bool,
    #[command(subcommand)]
    command: Command,
}

/// See `Cli::installed_billboards`.
#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum InstalledBillboards {
    Render,
    Keep,
}

#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum Bc7Device {
    Gpu,
    Cpu,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum Bc7Arg {
    Fast,
    High,
    Best,
}

#[derive(Subcommand)]
enum Command {
    /// Load an MO2 profile (VFS, plugins, archives, worldspaces) and report what was found, with timings.
    Inspect {
        #[command(flatten)]
        profile: ProfileArgs,
        /// Write the full report as JSON.
        #[arg(long)]
        json: Option<PathBuf>,
    },
    /// Extract every archived .nif/.dds the profile sees (BSA decompression) and parse it.
    BsaScan {
        #[command(flatten)]
        profile: ProfileArgs,
    },
    /// Predict which terrain LOD tiles each season changes and, given a golden xLODGen seasons run,
    /// compare the prediction with the tiles that actually differ from the default season.
    SeasonDelta {
        #[command(flatten)]
        profile: ProfileArgs,
        #[arg(long, default_value = "Tamriel")]
        world: String,
        /// Golden run directory (contains output\Textures\Terrain\<World>\).
        #[arg(long)]
        golden: Option<PathBuf>,
        /// Widen each affected cell by this many cells.
        #[arg(long, default_value_t = 0)]
        margin: i32,
    },
    /// Bake one terrain LOD diffuse tile on the CPU and compare it with a golden xLODGen tile.
    TerrainBake(terrain_bake::BakeArgs),
    /// GPU-bake all terrain LOD diffuse textures of a worldspace (optionally a season delta).
    TerrainTextures(terrain_textures::Args),
    /// List the profile's virtual Data paths under a prefix, with where each comes from.
    VfsList {
        #[command(flatten)]
        profile: ProfileArgs,
        /// Data-relative prefix, e.g. `meshes\terrain\tamriel\trees\`
        prefix: String,
    },
    /// Copy one file out of the profile's virtual Data folder (loose or archived).
    VfsExtract {
        #[command(flatten)]
        profile: ProfileArgs,
        /// Data-relative path, e.g. lodsettings\tamriel.lod
        path: String,
        /// Destination file.
        out: PathBuf,
    },
    /// Re-encode DDS textures with the GPU BC7 encoder; report throughput and quality.
    Bc7Bench {
        dir: PathBuf,
        #[arg(long, default_value_t = 1024)]
        size: u32,
        #[arg(long, default_value_t = 400)]
        limit: usize,
        #[arg(long, default_value_t = 64)]
        batch: usize,
    },
    /// Terrain LOD meshes (.btr) for a worldspace, straight from the load order.
    TerrainMeshes(terrain_meshes::Args),
    /// DynDOLOD's terrain underside mesh (<World>_Underside.nif), optional golden comparison.
    TerrainUnderside(terrain_underside::Args),
    /// Object LOD meshes (.bto) from a LODGen export file (LODGen port), optional golden diff.
    ObjectLod(object_lod::Args),
    /// LODGen object export built from the load order (wbLOD.pas port), diffed against xLODGen's.
    ObjectExport(object_export::Args),
    /// DynDOLOD-compatible object LOD export from the load order and DynDOLOD's configuration.
    DyndolodExport(dyndolod_export::Args),
    /// DynDOLOD-compatible pre-computed occlusion (CELL TVDT), compared with a golden Occlusion.esp.
    Occlusion(occlusion::Args),
    /// TexGen-compatible tree billboards rendered on the GPU, compared with a golden TexGen run.
    TexgenBillboards(texgen_billboards::Args),
    /// Grass LOD billboards for every grass type in an NGIO grass cache, plus the grass map.
    GrassBillboards(texgen_billboards::GrassArgs),
    /// TexGen stitched object-LOD textures (mini atlases, _n, PBR twins) from the DynDOLOD rules.
    TexgenStitched(texgen_stitched::Args),
    /// Billboard tree LOD (.lst/.btt + atlas) from the load order.
    TreeLod(tree_lod::Args),
    /// Everything xLODGen does (terrain, objects, trees; all worldspaces and seasons) in one run.
    Build(build::Args),
    /// Inspect generated LOD as the game arranges it (window, or --screenshot / --check headless).
    View(view::Args),
    /// Compare a LODForge build with a DynDOLOD output folder (object LOD, plugins, occlusion).
    E2eCompare(e2e_compare::Args),
    /// Records and subrecords of a plugin (diagnostics).
    PluginDump(plugin_dump::Args),
    /// Where a DynDOLOD-mode build's LOD models come from (DynDOLOD Resources, vanilla, mods).
    LodSources(lod_sources::Args),
    /// What LOD every tree base gets in a DynDOLOD-mode export (3D model, billboard, none) and why
    TreeReport(tree_report::Args),
    /// Generated 3D tree LOD models (Ultra phase 3); `--inspect` prints a tree's bark/foliage split
    TreeModels(tree_models::Args),
    /// LOD models generated from full models where DynDOLOD's rules want LOD and none is installed.
    LodModels(lod_models::Args),
    /// Large references per cell and a DynDOLOD output's large-reference workarounds.
    LargeRefs(large_refs::Args),
    /// Child worldspace references DynDOLOD copies into a parent (diagnostics).
    ChildCopies(child_copies::Args),
    /// Profile summary for the UI: LOD worldspaces, seasons with swap files.
    Info(info::Args),
    /// Tolerant comparison of two folders of terrain LOD meshes (.btr), per level.
    BtrCompare(btr_compare::Args),
    /// Parse every .nif/.btr/.bto under a folder; report block types and failures.
    NifScan { dir: PathBuf },
    /// Compare the top mips of two DDS files block by block.
    DdsDiff {
        a: PathBuf,
        b: PathBuf,
        #[arg(long, default_value_t = 256)]
        block: u32,
    },
    /// Parse every .dds under a folder and decode the top mip; report formats and failures.
    DdsScan { dir: PathBuf },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    lodforge_gpu::bc7_cpu::set_default_quality(match cli.bc7 {
        Bc7Arg::Fast => lodforge_gpu::Bc7Quality::Fast,
        Bc7Arg::High => lodforge_gpu::Bc7Quality::High,
        Bc7Arg::Best => lodforge_gpu::Bc7Quality::Best,
    });
    lodforge_gpu::bc7_gpu::set_default_gpu(cli.bc7_device == Bc7Device::Gpu);
    lodforge_texgen::set_billboard_scale(cli.billboard_scale);
    lodforge_texgen::render::set_supersample(cli.billboard_supersample);
    texgen_billboards::set_render_installed(cli.installed_billboards == InstalledBillboards::Render);
    object_atlas::set_billboard_density(cli.billboard_density);
    object_atlas::set_billboard_dither(!cli.billboard_soft);
    match cli.command {
        Command::Inspect { profile, json } => inspect::run(&profile, json),
        Command::BsaScan { profile } => scan::bsa_scan(&profile.open()?.vfs),
        Command::SeasonDelta { profile, world, golden, margin } => season_delta::run(&profile, &world, golden.as_deref(), margin),
        Command::TerrainBake(a) => terrain_bake::run(&a),
        Command::TerrainTextures(a) => terrain_textures::run(&a),
        Command::VfsList { profile, prefix } => {
            let p = profile.open()?;
            for path in p.vfs.list_prefix(&prefix) {
                println!("{path}	{:?}", p.vfs.origin(&path));
            }
            Ok(())
        }
        Command::VfsExtract { profile, path, out } => {
            let p = profile.open()?;
            let bytes = p.vfs.read(&path)?.ok_or_else(|| anyhow::anyhow!("{path} not found in the VFS"))?;
            std::fs::write(&out, &bytes)?;
            println!("{path}: {} bytes from {:?} -> {}", bytes.len(), p.vfs.origin(&path), out.display());
            Ok(())
        }
        Command::Bc7Bench { dir, size, limit, batch } => scan::bc7_bench(&dir, size, limit, batch),
        Command::TerrainMeshes(a) => terrain_meshes::run(&a),
        Command::TerrainUnderside(a) => terrain_underside::run(&a),
        Command::ObjectLod(a) => object_lod::run(&a),
        Command::ObjectExport(a) => object_export::run(&a),
        Command::TreeLod(a) => tree_lod::run(&a),
        Command::Build(a) => build::run(&a),
        Command::View(a) => view::run(&a),
        Command::Info(a) => info::run(&a),
        Command::LodSources(a) => lod_sources::run(&a),
        Command::TreeReport(a) => tree_report::run(&a),
        Command::TreeModels(a) => tree_models::run(&a),
        Command::LodModels(a) => lod_models::run(&a),
        Command::PluginDump(a) => plugin_dump::run(&a),
        Command::LargeRefs(a) => large_refs::run(&a),
        Command::ChildCopies(a) => child_copies::run(&a),
        Command::E2eCompare(a) => e2e_compare::run(&a),
        Command::BtrCompare(a) => btr_compare::run(&a),
        Command::DyndolodExport(a) => dyndolod_export::run(&a),
        Command::Occlusion(a) => occlusion::run(&a),
        Command::TexgenBillboards(a) => texgen_billboards::run(&a),
        Command::GrassBillboards(a) => texgen_billboards::run_grass(&a),
        Command::TexgenStitched(a) => texgen_stitched::run(&a),
        Command::NifScan { dir } => scan::nif_scan(&dir),
        Command::DdsDiff { a, b, block } => scan::dds_diff(&a, &b, block),
        Command::DdsScan { dir } => scan::dds_scan(&dir),
    }
}
