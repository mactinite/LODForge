# Build performance (2026-10-26)

What a full Quality build costs and where the time goes. Profile: skyrim-cs-base, 15
worldspaces, default + 4 seasons, the UI's Quality level (`--preset High --bc7 high
--atlas-max-tile 2048 --billboard-scale 1.5 --tree-lod ultra --tree-models generate
--lod-models --texgen`) plus `--runtime lodforge`. RTX 4080, 20 threads.

## Why the user's UI build took ~50 min

The installed copy in `tools\LODForge` was built on 2026-10-24 01:27, before the two fixes
that took Ultra from ~38 to ~10 min: the GPU port of the BC7 encoder (0a9f68c) and the opt-out
of Windows power throttling (9d201f4; a CLI launched from a background window ran on ~3 of 20
threads). That build also predates the runtime mode, so it wrote no runtime files. Install a
current build first.

## Every build keeps its timings

`%LOCALAPPDATA%\LODForge\logs\build-<unix time>.jsonl`: the command line, then one line per
stage start/end (`t` = ms since start, `ms` = stage time, BC7 totals), and `lane` lines for
background occlusion. The UI keeps no log, so this is the record of a slow build.

## Measurements

| Run | Code | Total |
| --- | --- | --- |
| baseline | runtime-ui-viewer ed0ccf9 (main + runtime-integration), fresh folder | 881 s |
| 2 | + the changes below, fresh folder | 613 s |
| 3 | final code, rebuild into the run-2 folder | 638 s |

Impostors: 197 s, 75 s, 14 s (run 3 kept every page). Occlusion at the end: 161 s, 5 s, 23 s
(run 3 computes the child worldspaces there; see below). The baseline overlapped a parallel
`cargo build`, so part of the texgen/tree-models gain (78/79 s to 57/66 s) is contention.

Output check: run 2 against the baseline, file by file: all 138,452 files identical except
those holding the random run token (`.bin`, `Build.txt`, `DynDOLOD_NG_*.txt`, DynDOLOD.esm/.esp:
31-34 bytes each), the impostor reports (timing column), the new `pages.keys`, and
Occlusion.esp. Occlusion.esp from run 2 was short of 232 cells: DynDOLOD.esm gives child
worldspaces CELL records that change the LAND they borrow from the parent, so the lane now
computes main worldspaces only and the children are done at the end. Run 3's Occlusion.esp is
byte-identical to the baseline's.

Baseline, stage totals over all worldspaces (terrain runs on its own thread beside the rest):
terrain 363 s, impostors 197 s, occlusion 161 s (serial, at the end), objects-atlas 106 s,
dyndolod-config 91 s (incl. lod-models 7 s, tree-models 79 s), texgen 78 s, dyndolod-export
73 s, texgen-stitched 58 s, tree-pack 30 s, pbr-linear 27 s, dyndolod-atlas 25 s, objects 21 s.

## Changes

- **Impostors**: cached types load in parallel (Tamriel: 543 types, all from the bake cache,
  took 35 s one by one); pages are composed one at a time on three worker threads while the GPU
  encodes the previous one (before: all 95 Tamriel pages, ~17 GB, composed first on one thread);
  pages whose inputs match the previous build in the same folder (`pages.keys`) are kept.
- **Occlusion lane**: each worldspace's TVDT is computed on a background thread as soon as its
  objects stage is done; Occlusion.esp is still written at the end against the final load order
  (cells are selected again there, so the plugin is the same).
- **Shared GPU device** (`Gpu::shared`): per-worldspace stages no longer create a device and
  compile every pipeline again (0.3-1 s each, 9 s cold).
