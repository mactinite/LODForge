# Architecture

This describes the implementation inspected at `2737410` on 2026-10-19.
Atlas/PBR work in parallel sessions may change details; the ownership boundaries
below are the useful starting point.

## Execution model

```text
Tauri UI (checked-in HTML/JS)
  -> lodforge info --json / build --json (child process)
CLI
  -> MO2 profile -> loose-file VFS + archives + winning plugin load order
  -> optional TexGen billboards / stitched and rendered textures
  -> DynDOLOD configuration + optional missing LOD models
  -> for each worldspace:
       terrain BTRs -> diffuse/normal textures (default + seasonal deltas)
       object export -> side files / atlas -> BTOs + object height map
       classic tree LST/BTT + atlas; optional terrain underside
  -> optional Grass Optimizations meshes
  -> DynDOLOD plugins + DLL NG data + seasonal swaps
  -> Occlusion.esp
  -> completion event and process exit code
```

The build has two object paths: xLODGen-style export/LOD, and the DynDOLOD path
with its rules and runtime outputs. BTO grass is prepared inside the DynDOLOD
object path; it is not the later Grass Optimizations stage.

The main pipeline lives in [CLI build](../apps/cli/src/build.rs) and
[DynDOLOD orchestration](../apps/cli/src/dyndolod_build.rs). Stage loops are
orchestrated there; crates parallelize expensive chunks/models using Rayon and
batch GPU work. This is **not** a persistent task graph.
[core](../crates/core/src/lib.rs) still contains only a scaffold function/test.
The UI does not call a shared core scheduler; it runs the CLI.

## Crate map

All library package names use the `lodforge-` prefix.

| Directory | Responsibility |
|---|---|
| [esp](../crates/esp/src) | Plugin parsing, form-ID/load-order resolution and world/record data |
| [archive](../crates/archive/src) | BSA decoding, MO2 layout and loose/archive VFS precedence |
| [nif](../crates/nif/src) | NIF parsing/writing and the geometry/shader subset used by LOD |
| [dds](../crates/dds/src) | DDS formats, decoding and encoded texture containers |
| [gpu](../crates/gpu/src) | wgpu device, BC7 compression and terrain compute kernels |
| [btr](../crates/btr/src) | Terrain mesh triangulation, chunks, water, skirts and underside |
| [terrain](../crates/terrain/src) | LAND/LTEX-based terrain texture baking and image operations |
| [objlod](../crates/objlod/src) | LODGen export consumption, transforms, clipping, atlas UVs, merging and BTO output |
| [treelod](../crates/treelod/src) | Billboard discovery/layout and classic LST/BTT generation |
| [texgen](../crates/texgen/src) | Scene flattening, GPU billboard rendering, stitching and sidecars |
| [dyndolod](../crates/dyndolod/src) | Rules, model selection, export/side files, plugins, large references and occlusion |
| [seasons](../crates/seasons/src) | Swap parsing and dependency-based terrain texture delta planning |
| [grass](../crates/grass/src) | Offline placements and NGIO-compatible cache serialization, without raycast filtering |
| [lodmodel](../crates/lodmodel/src) | Full-model simplification and generated replacement LOD meshes |
| [core](../crates/core/src) | Placeholder; not an application service layer |
| [CLI](../apps/cli/src) (`lodforge`) | Profile loading, stage wiring, command options, diagnostics and comparisons |
| [UI](../apps/ui/src) (`lodforge-ui`) | Tauri commands and child-process lifecycle |

The libraries use each other directly; `core` is not their dependency root.
The CLI depends on the functional crates. The UI depends on the CLI executable
contract rather than those libraries. The checked-in [frontend](../apps/ui/dist)
is source for this application, despite the directory name.

## Inputs and generated state

[Profile loading](../apps/cli/src/profile.rs) constructs the winning Data view
from game files, enabled mods, overwrite and archives. It excludes generated
plugins. The build additionally filters recognized generated mods and overlays
fresh output into the VFS for downstream stages. Diagnostic commands do not all
apply the build's mod filtering automatically.

- Output is a Data-layout directory or an MO2 mod, not an archive.
- Intermediate exports/atlas maps live in a work directory. Explicit `--work-dir`
  preserves it; automatic work directories are removed on normal completion.
- Missing-model generation has a separate cache. There is no general persistent
  content-addressed cache for the whole build.
- Plugin rolling updates read the prior output's LODForge plugins to retain IDs
  and run token. An arbitrary old output directory is not a clean baseline.

## Output contracts

| Files, relative to output | Consumer |
|---|---|
| `meshes\terrain\<world>\*.btr` | Skyrim terrain LOD |
| `textures\terrain\<world>\...` | Terrain diffuse/normal and tree atlas textures |
| `meshes\terrain\<world>\objects\*.bto` | Skyrim object LOD, optionally BTO grass |
| `meshes\terrain\<world>\trees\*.lst`, `*.btt` | Classic tree LOD |
| `textures\lod\...`, `textures\DynDOLOD\LOD\...`, `textures\pbr\...linear.dds` | Generated LOD textures, atlas pages and converted PBR albedo |
| `textures\terrain\lodgen\...` | Billboards and metadata |
| `meshes\lod\grass\..._lod1.nif` | Community Shaders Grass Optimizations |
| `DynDOLOD.esm`, `DynDOLOD.esp`, `SKSE\Plugins\DynDOLOD_Data\...` | Dynamic LOD scripts / DLL NG |
| `Seasons\...` | Runtime seasonal swaps |
| `Occlusion.esp` | CELL TVDT precomputed occlusion |

Exact naming and binary contracts belong in the
[behavioral specification](re/dyndolod-spec.md); output support limits belong in
[parity](parity.md).

## Errors and automation

The build records stage failures, can continue other stages, and exits nonzero
if its failure list is nonempty. Output is written incrementally, not atomically.
There are also lower-level best-effort skips; successful exit alone is not proof
that every input asset generated output.

`--json` emits stage objects and a final `done` object with `ms` and `failed`.
Some stages also print ordinary text. Consumers must distinguish event lines,
capture stderr and check the process exit code; stdout is not guaranteed to be
a pure JSONL stream. The UI forwards stdout/stderr as events and polls exit status.
