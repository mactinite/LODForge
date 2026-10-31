# Level coverage: keeping objects across LOD levels (2026-10-29)

In game, the cs-base build (`--preset High`, LODForge Runtime) showed rocks, buildings and trees
appearing as the player approached: objects dropped out of the farther LOD levels. Three causes:

1. **Generated LOD models only for large objects.** DynDOLOD's High catch-all rule
   (`\,Level0,Level1,Level2`) asks for LOD8 and LOD16 models, but a base without an installed
   `_lod_1` / `_lod_2` model gets nothing there. `--lod-models` generated LOD8 models only for
   objects of 512 units and more and LOD16 models from 1024 (`lod_models::DEFAULT_MIN_SIZE`).
2. **Logs, driftwood and branch piles.** Models under `landscape\trees\` match the `tree` mask
   (`tree,Level0,Billboard1,Billboard1`). Statics have no billboard, so they had nothing past
   LOD4 (even the 1,105-unit `TreePineForestLog01`).
3. **Runtime tree impostors** were thinned by hash rank to 35 % from 60,000 to 200,000 units and
   faded out from 200,000 to 240,000, while object LOD runs to `fBlockMaximumDistance`
   (327,680 on cs-base).

Tamriel export with `--preset High` (`dyndolod-export --runtime-report`, before):

| | references | LOD4 | LOD8 | LOD16 | LOD4 only |
|---|---|---|---|---|---|
| rocks | 50,289 | 50,289 | 42,352 | 33,629 | 7,937 |
| buildings | 25,206 | 25,203 | 15,426 | 8,644 | 9,777 |
| other | 11,847 | 11,847 | 5,684 | 3,172 | 6,163 |
| `tree`-mask statics | 8,461 | 6,731 | 1,937 | 1,937 | 6,524 |

## Changes

- `build --level-coverage auto|rules|keep` (`auto`: `keep` with `--preset High`). `keep`
  generates LOD8 models down to 128 units and LOD16 models down to 256 (`KEEP_MIN_SIZE`, about
  3 px where the level starts at 3440 px / 100 degrees); LOD4 stays at 256. A non-tree base whose
  rule asks for a billboard it does not have takes its model level instead
  (`assign::level_choice`, `Resolver::model_fallback`). `--lod-model-min-size` still wins.
- LODForge Runtime: `[Trees] fFadeOutStart` / `fFadeOutEnd` 0 = fade out over the last 32,768
  units before `fBlockMaximumDistance`; `bThinning` is off by default. `lodforge view
  --runtime-trees legacy` draws the old defaults.

## Cost (Tamriel only, cs-base Default, same arguments as the user's build plus `--no-detail-bake`)

| | rules | keep |
|---|---|---|
| generated LOD models | 1,988 | 4,670 |
| build time | 445 s | 564 s |
| `.statics.bin` records LOD4 / LOD8 / LOD16 | 92,178 / 60,409 / 42,176 | 98,965 / 97,719 / 87,789 |
| `StaticMeshes.bin` | 207 MB, 20 pages (321 Mpx) | 248 MB, 24 pages (367 Mpx) |
| merged `.bto` LOD8 shapes / triangles (whole world) | 1,125 / 514k | 1,788 / 673k |
| merged `.bto` LOD16 shapes / triangles (whole world) | 511 / 372k | 846 / 507k |

Statics drawn from one point by distance alone (`crates/instances/examples/lod_band_counts.rs`;
no frustum or occlusion, so an upper bound):

| vantage | rules instances / triangles | keep instances / triangles |
|---|---|---|
| tundra east of Whiterun | 59,582 / 7.57 M | 94,643 / 10.01 M |
| Solitude | 52,942 / 6.12 M | 91,470 / 8.57 M |
| Riften | 41,844 / 4.59 M | 77,087 / 6.58 M |
| Markarth | 43,299 / 4.88 M | 79,873 / 6.91 M |

Draw calls: the statics draw one call per atlas page and pass, so 4 more per pass (24 pages,
below the runtime's 32-page cluster limit). With the detail bake on top, the full cs-base build
(2026-10-30) reached 46 pages of 4096 px: clusters on pages 32+ drew page 31's texture (wrong
textures on mountains). `tree_pack.rs` now packs statics on 8192 px pages (13 pages) and warns
past 32 (`RUNTIME_CLUSTER_PAGES`); raising the runtime's `kClusterPages` is the longer-term fix. Trees: same draw count; at the tundra vantage
the impostors drawn go from 67,027 to 93,584 (all of them; +53k triangles).
VRAM: +46 Mpx of static atlas pages (about +60 MB with all mips, +15 MB at the default
`iSkipMips=1`), plus the larger vertex data (`StaticMeshes.bin` +41 MB on disk) and +3 MB of
instance records. Not measured in game.

LOD32 is not done: the runtime's LOD32 band is empty (it ends at `fBlockMaximumDistance`). From
the middle of Tamriel nothing lies past 327,680 units; from Riften or Markarth 5,700-7,100 LOD16
records do, about 2,000 of them 2,048 units or larger.

Viewer shots: `D:\Modlists\LODForge-out\coverage\shots` (`--camera
"-40000,60000,40000,200,-18" --fov 100 --size 3440x1440`; `*-crop.png` = before above, after
below, 2x; `nf-*` without fog).
