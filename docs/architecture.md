# Architecture

What exists in the workspace and how a build flows through it.

## Execution model

```text
Tauri UI (apps/ui; React frontend in apps/ui/web, bundled into apps/ui/dist)
  -> lodforge info --json / build --json / view   (child processes)
CLI (apps/cli)
  -> MO2 profile -> loose-file VFS + archives + winning plugin load order
  -> optional TexGen stage: billboards, stitched and rendered LOD textures
  -> DynDOLOD configuration; optional generated LOD models and tree models
  -> for each worldspace:
       terrain lane (own thread): BTRs -> diffuse/normal textures (default + seasonal deltas)
       object export -> side files / atlas plan -> BTOs + object height map
       tree LOD: classic LST/BTT + atlas, or trees in object LOD (ultra)
       optional terrain underside
       runtime mode: impostors, trees/statics instance files, mesh packs, far grass, heightfield
  -> optional Grass Optimizations meshes
  -> DynDOLOD plugins + DLL NG data + seasonal swaps (runtime mode: .dyn.bin as well)
  -> Occlusion.esp (background lane)
  -> completion event and process exit code
```

Object LOD follows DynDOLOD's rules, with its dynamic LOD plugins and occlusion; `--mode
dyndolod` is still accepted and is the only mode. BTO grass is prepared inside the object path;
Grass Optimizations meshes are a separate later stage. Runtime mode (`--runtime lodforge`) moves
trees, repeated statics, far grass and dynamic LOD into files for the LODForge Runtime plugin.
Terrain LOD follows xLODGen's behavior, and `object-lod` can still read an xLODGen export file.

The pipeline lives in [build.rs](../apps/cli/src/build.rs) and
[dyndolod_build.rs](../apps/cli/src/dyndolod_build.rs). Stages are sequenced there; crates
parallelize chunks and models with Rayon and batch GPU work. There is no persistent task graph
and no shared orchestration library: the UI runs the CLI.

## Crate map

All library package names use the `lodforge-` prefix.

| Directory | Responsibility |
|---|---|
| [esp](../crates/esp/src) | Plugin parsing, form-ID/load-order resolution and world/record data |
| [archive](../crates/archive/src) | BSA decoding, MO2 layout and loose/archive VFS precedence |
| [nif](../crates/nif/src) | NIF parsing/writing and the geometry/shader subset used by LOD |
| [dds](../crates/dds/src) | DDS formats, decoding and encoded texture containers |
| [gpu](../crates/gpu/src) | wgpu device, BC7 compression (GPU and CPU) and terrain compute kernels |
| [btr](../crates/btr/src) | Terrain mesh triangulation, chunks, water, skirts and underside |
| [terrain](../crates/terrain/src) | LAND/LTEX-based terrain texture baking and image operations |
| [objlod](../crates/objlod/src) | Export consumption, transforms, clipping, atlas UVs, merging, BTO output, instancing split |
| [treelod](../crates/treelod/src) | Billboard discovery/layout and classic LST/BTT generation |
| [texgen](../crates/texgen/src) | Scene flattening, GPU billboard rendering, stitching and sidecars |
| [dyndolod](../crates/dyndolod/src) | Rules, model selection, export/side files, plugins, large references, child-world copies and occlusion |
| [seasons](../crates/seasons/src) | Swap parsing and dependency-based terrain texture delta planning |
| [grass](../crates/grass/src) | Reader/writer for NGIO's `.cgid` grass cache |
| [lodmodel](../crates/lodmodel/src) | LOD models generated from full models (`--lod-models`) |
| [treemodel](../crates/treemodel/src) | Generated 3D tree LOD models (`--tree-models`), ported from TreeLODGen |
| [detailbake](../crates/detailbake/src) | Per-model texture bake (albedo, normals, ambient occlusion) for instanced rock and mountain LOD |
| [impostor](../crates/impostor/src) | Hemi-octahedral tree impostors: bake, dilation, atlas pages, manifests |
| [instances](../crates/instances/src) | Reference writer/reader of the LODForge Runtime files (`.trees.bin`, `.statics.bin`, `.lfg`, mesh packs) |
| [viewer](../crates/viewer/src) | LOD inspection: file discovery, mesh decoding, wgpu rendering, winit/egui window, headless screenshots and checks |
| [CLI](../apps/cli/src) (`lodforge`) | Profile loading, stage wiring, command options, diagnostics and comparisons |
| [UI](../apps/ui/src) (`lodforge-ui`) | Tauri commands and child-process lifecycle |

The libraries use each other directly. The CLI depends on the functional crates. The UI depends
on the CLI executable's contract (`info --json`, `build --json`, `view`), not on the libraries.

## CLI commands

`lodforge --help` lists everything. By role:

- **Build and use:** `build`, `info` (profile summary for the UI), `view`.
- **Single stages** (the same code `build` runs, for one stage): `terrain-meshes`,
  `terrain-textures`, `terrain-underside`, `dyndolod-export`, `object-lod`,
  `tree-lod`, `texgen-billboards`, `texgen-stitched`, `grass-billboards`, `occlusion`,
  `lod-models`, `tree-models`, `impostors`.
- **Runtime output maintenance** (rewrite part of a `--runtime lodforge` output without a full
  build): the `runtime-*` commands, `impostor-regrade`.
- **Reports and diagnostics:** `inspect`, `lod-sources`, `tree-report`, `large-refs`,
  `child-copies`, `bto-stats`, `lod-color`, `runtime-check`, `runtime-dyn-info`, `impostor-stats`,
  `plugin-dump`, `vfs-list`, `vfs-extract`, `nif-scan`, `dds-scan`, `bsa-scan`.
- **Comparison with reference output:** `e2e-compare`, `btr-compare`, `dds-diff` and the
  `--golden` options of the single-stage commands.

## Inputs and generated state

[Profile loading](../apps/cli/src/profile.rs) constructs the winning Data view from game files,
enabled mods, overwrite and archives. It excludes generated plugins. The build additionally
filters recognized generated mods and overlays fresh output into the VFS for downstream stages.
Diagnostic commands do not all apply the build's mod filtering.

- Output is a Data-layout directory or an MO2 mod, not an archive.
- Intermediate exports and atlas maps live in a work directory (`.lodforge-work-<pid>`). An
  explicit `--work-dir` is kept; an automatic one is removed on normal completion.
- Generated LOD models and impostor bakes have their own caches
  (`%LOCALAPPDATA%\LODForge\cache`). There is no general content-addressed cache for the build.
- A rebuild reads the previous output's `DynDOLOD.esm` / `DynDOLOD.esp` to keep form IDs and the
  run token, and nothing else from it.
- Every build logs to `%LOCALAPPDATA%\LODForge\logs\build-<time>.jsonl`.

## Output contracts

| Files, relative to output | Consumer |
|---|---|
| `meshes\terrain\<world>\*.btr` | Skyrim terrain LOD |
| `textures\terrain\<world>\...` | Terrain diffuse/normal and tree atlas textures |
| `meshes\terrain\<world>\objects\*.bto` | Skyrim object LOD, optionally BTO grass |
| `meshes\terrain\<world>\trees\*.lst`, `*.btt` | Classic tree LOD |
| `textures\lod\...`, `textures\DynDOLOD\LOD\...`, `textures\pbr\...linear.dds` | Generated LOD textures, atlas pages and converted PBR albedo |
| `textures\terrain\lodgen\...` | Billboards and metadata |
| `meshes\dyndolod\lod\...` | Generated LOD models and tree models |
| `meshes\lod\grass\..._lod1.nif` | Community Shaders Grass Optimizations |
| `DynDOLOD.esm`, `DynDOLOD.esp`, `SKSE\Plugins\DynDOLOD_Data\...` | Dynamic LOD scripts / DLL NG |
| `Seasons\...` | Runtime seasonal swaps |
| `Occlusion.esp` | CELL TVDT precomputed occlusion |
| `SKSE\Plugins\LODForgeRuntime\...`, `textures\LODForge\Impostors\...` | LODForge Runtime ([formats](runtime/instance-formats.md), [impostors](runtime/impostors.md)) |
| `LODForge_TreeReport_<World>.csv`, `LODForge_TreeModels.txt` | Reports ([build options](build-options.md)) |

Naming and binary contracts are in the specifications under [re/](re) and [runtime/](runtime).

## Errors and automation

The build records stage failures, can continue with other stages, and exits nonzero if its
failure list is nonempty. Output is written incrementally, not atomically. There are also
lower-level best-effort skips; a successful exit alone does not prove that every input asset
produced output.

`--json` emits `begin` and `stage` objects and a final `done` object with `ms` and `failed`.
Some stages also print ordinary text. Consumers must pick out the lines that start with `{`,
capture stderr and check the exit code. The UI forwards stdout/stderr as events and polls the
exit status.
