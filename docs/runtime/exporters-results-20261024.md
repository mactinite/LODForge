# Runtime exporters: Tamriel build results (2026-10-24)

`build --runtime lodforge` on skyrim-cs-base (profile Default), Tamriel with the WIN season, compared
with the same build without it. Branch `runtime-exporters`; outputs in
`D:\Modlists\LODForge-out\runtime-20261024\{before,after}`, logs next to them.

```
lodforge build --instance D:\Modlists\skyrim-cs-base --worlds Tamriel --seasons WIN --mode dyndolod ^
  --texgen --preset High --atlas-max-tile 2048 --billboard-scale 1.5 --tree-lod ultra ^
  --tree-models generate --no-occlusion --no-plugins --out <dir>
  [--runtime lodforge --runtime-min-copies 4 --runtime-zstd 3 --runtime-grass-cache <cgid dir>]
```

(Occlusion and the DynDOLOD plugins were left out to keep the runs short; they do not depend on the
runtime option. Other sessions were compiling on the machine at the same time, so stage times are
noisy by a few seconds.)

## Output files (`SKSE\Plugins\LODForgeRuntime`)

| File | Contents | Size |
|---|---|---|
| `Tamriel.trees.bin` | 93,544 trees, 379 types, 4,665 cells | 2.31 MB |
| `Tamriel.WIN.trees.bin` | 63,382 trees, 242 types, 4,438 cells | 1.64 MB |
| `Tamriel.statics.bin` | 46,006 instances (3D tree LOD), 275 meshes, 348 blocks | 1.21 MB |
| `Tamriel.WIN.statics.bin` | 40,939 instances, 214 meshes, 348 blocks | 1.08 MB |
| `Tamriel.lfg` | 29.7 M grass instances, 5,617 cells, 46 types | 235.8 MB |
| `Tamriel.WIN.lfg` | 10.5 M grass instances, 5,575 cells, 35 types | 83.8 MB |

zstd level 3 on all sections; every file is read back and CRC-checked after writing.

Trees left in object LOD (not drawable as yaw-only impostors or not owned by the runtime): 1,720
tilted more than `TreeLODMaxTilt` (30 degrees), 308 inside child-worldspace tree bounds; no large
references or child copies among Tamriel's trees. Tree report: 122,802 tree references in the
export, 93,544 (76%) now drawn by the runtime (`runtime_refs` column of
`LODForge_TreeReport_Tamriel.csv`); the rest are trees with no LOD in any level or the kept ones.

The 46,006 instances are, all but at most a few, the runtime trees' 3D LOD4 models: the tree report
has 46,167 LOD4 `3d` references, some of them kept in object LOD as tilted or in bounds.
`--runtime-min-copies 4` (mask `passthru`) adds non-runtime passthru models with 4+ copies per block;
the split between the two is not counted separately (the `MESH` flags mark tree meshes).

Grass sizes per density level: [grass-lfg-sizes.md](grass-lfg-sizes.md).

## Object LOD before vs after (`lodforge bto-stats`)

Default season:

| Level | Files | Size before | after | Shapes (draws) before | after | Triangles before | after |
|---|---|---|---|---|---|---|---|
| LOD4 | 506 | 1,123.5 MB | 401.2 MB | 11,134 | 7,369 (-34%) | 28,210,579 | 9,010,063 (-68%) |
| LOD8 | 142 | 207.9 MB | 181.7 MB | 2,385 | 2,233 (-6%) | 4,341,399 | 3,968,679 (-9%) |
| LOD16 | 43 | 96.3 MB | 70.7 MB | 894 | 860 (-4%) | 1,883,284 | 1,518,005 (-19%) |

WIN:

| Level | Shapes before | after | Triangles before | after |
|---|---|---|---|---|
| LOD4 | 10,605 | 7,254 | 25,997,637 | 8,990,592 |
| LOD8 | 2,356 | 2,204 | 4,218,385 | 3,965,174 |
| LOD16 | 889 | 856 | 1,765,920 | 1,514,653 |

The runtime adds back one indirect draw per mesh per pass for the instanced models and one for all
impostors (plan, M1/M2), instead of the 3,765 LOD4 shapes the trees took. Triangles drawn by the
runtime depend on its distance tiers; the `.bto` side alone drops 19.2 M LOD4 triangles.

## Timings

| Stage | Before | After |
|---|---|---|
| dyndolod-export (after: + tree selection and the two trees files) | 18.7 s | 18.0 s |
| objects (after: + instancing and the two statics files) | 7.5 s | 5.6 s |
| runtime-grass (two `.lfg` from 1.3 GB of `.cgid`) | - | 5.4 s |
| whole build | 246.0 s | 242.6 s |

The tree selection and statics writing cost no measurable time; objects got faster because the
trees are no longer merged. Far grass is the only new stage.

## Not done / open

- Not checked in game (the runtime plugin does not read these files yet).
- The object height map (occlusion input) no longer contains the instanced trees' 3D models; the
  occlusion stage was not run in these builds. Whether occlusion should keep counting runtime trees
  is open.
- The atlas still holds the billboards of runtime trees (unused in `.bto` now); harmless, could be
  dropped later.

## Update 10:35: static mesh pack with snow/ash and VC variants (format v1.5)

Full cs-base build (15 worlds x 5 seasons, no terrain, plugins + occlusion) in 722 s, swapped
into `mods\LODForge Output (runtime)` with the far-grass `.lfg` (G1 test data).

| Tamriel `.bto` (default season) | Before static instancing | After |
|---|---|---|
| LOD4 shapes / triangles / MB | 7,379 / 9.03 M / 402 | 1,248 / 0.55 M / 31 |
| LOD8 | 2,233 / 3.97 M | 715 / 0.48 M |
| LOD16 | 860 / 1.52 M | 417 / 0.44 M |

Packs: `StaticMeshes.bin` 2,264 meshes, 1.43 M triangles in 23,698 clusters, 8 pages (plain, snow,
snowHD, ash) 275 MB + 110 MB buffers; `TreeMeshes.bin` 489 meshes, 7 pages 291 MB + 59 MB.
Static instance records (default season, all levels): Tamriel 94,054, all worlds about 125 k.
