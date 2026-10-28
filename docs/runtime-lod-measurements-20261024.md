# Distant LOD measurements in game (2026-10-24)

What distant LOD and grass cost today on skyrim-cs-base. This is the baseline the GPU-instanced LOD layer has to beat (see `runtime-instancing-feasibility.md`).

## Setup

- **Game and settings:** skyrim-cs-base, profile `LOD-Measure` (a copy of Default), LODForge Output as installed, Bottled Shaders 1.0.18, 1920×1080 windowed, uGridsToLoad 5, fBlockLevel0Distance 57344.
- **For the captures:** a temporary Community Shaders settings file turned frame generation off, set the upscaler to TAA, and turned Frame Annotations on. This lets RenderDoc capture and labels every draw with its shader type and shape name.
- **Capture:** RenderDoc 1.46 through AgentBridge (`game_renderdoc`, one bracketed frame). Each frame was then replayed with `qrenderdoc --python`, recording every draw's marker path, indices, instances and `EventGPUDuration`. Scripts: `scripts/lod-measure/rdc_dump.py` (run with `RDC_IN` / `RDC_OUT` set: `qrenderdoc.exe --python rdc_dump.py`) and `scripts/lod-measure/classify.js` (`node classify.js <dump.json>`).
- **GPU time is replay time:** each event is timed alone, so the totals (26–140 ms) are not real frame times. Use the percentages to compare.
- **CPU cost was not measured.** The draw counts stand in for it.
- **Classification by shape name:**
  - `obj`, `objHD`, `objsnow`, `objsnowHD` and their `-largeref` versions → object LOD, atlas shapes;
  - `objpassthru` → object LOD, passthru shapes;
  - `treepassthru` → 3D tree LOD;
  - `land` (Lighting technique 18) → terrain LOD;
  - Grass shader → grass;
  - technique 12 → full trees.
- **Passes:**
  - main = `World`;
  - depth = `Depth` prepass;
  - sun shadows = `Directional Light Shadowmaps`;
  - water cubemap = `Water Effects > Cubemap`.

## Spots

| Spot | Where | Player position, heading | What is in view |
| --- | --- | --- | --- |
| A | Solitude shore (cell -13, 28) | -52200, 115500, -13450; heading 40 | Coast, near pines, little distant LOD |
| B | Tundra east of Whiterun (3, -5) | 14700, -19369, -3970; heading 90 | Farm, grass, trees, mountains |
| C | Winterhold (27, 23) | 113230, 94803, -7799; heading 0 | College, snow; region weather overrides clear |
| D | Bleak Falls path over Riverwood (BleakfallsBarrowPath) | 12000, -38000, 3932; heading 165, pitch 8 | Forested valley, distant mountains |

Captures, thumbnails, dumps and summaries are in `D:\Modlists\LODForge-out\lod-measure` (≈20 GB of `.rdc`; delete when done).

## Results (draws / share of replay GPU time)

| | A | B | C | D |
| --- | ---: | ---: | ---: | ---: |
| All draws | 3,279 | 8,791 | 10,943 | 8,549 |
| Loaded-cell objects (non-LOD) | 2,600 / 70.5% | 5,775 / 56.7% | 8,613 / 70.4% | 5,496 / 56.3% |
| **Object LOD, all** | **276 / 5.3%** | **1,995 / 25.5%** | **1,256 / 10.3%** | **1,986 / 18.9%** |
| – of which in the water cubemap | 32 / 0.3% | 845 / 15.5% | 788 / 6.0% | 1,012 / 7.1% |
| – atlas shapes | 185 / 3.6% | 1,309 / 15.2% | 1,061 / 8.3% | 1,271 / 10.5% |
| – passthru shapes | 55 / 1.2% | 532 / 8.0% | 74 / 0.9% | 571 / 7.0% |
| – 3D tree LOD (`treepassthru`) | 36 / 0.5% | 154 / 2.3% | 121 / 1.1% | 144 / 1.4% |
| Terrain LOD | 42 / 1.3% | 59 / 1.0% | 71 / 1.0% | 58 / 0.9% |
| Billboard tree LOD (`.btt`) | 0 | 0 | 0 | 0 |
| Full trees (loaded cells, main pass only) | 75 / 4.9% | 198 / 3.4% | 367 / 4.2% | 470 / 10.4% |
| Grass (Grass Optimizations) | 22 / 1.4% | 34 / 2.3% | 22 / 0.4% | 60 / 1.2% |

## Findings

1. **LOD is not the main GPU cost.** Object LOD takes 5–26% of GPU time. Terrain LOD takes about 1%. Full-detail objects in the loaded cells take 56–70%. Object LOD's real cost is mostly its draw count: 1,250–2,000 draws per frame in open views, which is CPU submission time.
2. **The water reflection cubemap redraws object LOD.** At B that's 845 extra draws and 15.5% of GPU time; elsewhere 6–7%. It's the cheapest win available: leave LOD out of the cubemap, use a coarser LOD level there, or update the cubemap less often. That's a Bottled setting or a small change in its Water Effects code, and needs checking.
3. **Each LOD shape is drawn twice in the main view**: once in the depth prepass and once in the main pass. LOD is not drawn into the sun shadow maps.
4. **This build has no billboard tree LOD (`.btt`).** The `.lst` files are empty: tree LOD is part of object LOD, as 3D trees (`treepassthru`, 1–2%) and as billboard cards inside the atlas shapes. So an impostor layer would replace parts of object LOD, not the engine's tree LOD path.
5. **Grass is already GPU-instanced and cheap**: 22–60 draws and 0.4–2.3%, through Grass Optimizations. Grass pop-in is about how far grass reaches, not about rendering cost.

## Extended grass check (NGIO `Extend-grass-distance`, no cache)

Settings: `Extend-grass-distance = true`, `Overwrite-grass-distance = 16384`, `Overwrite-grass-fade-range = 8192`, so grass reaches about 24,500 units, more than twice the loaded 5×5 area. `GrassControl.ini` was restored afterwards.

| Spot B | Baseline | Extended |
| --- | ---: | ---: |
| Grass draws | 34 | 176 |
| Grass share of replay GPU | 2.3% | 3.7% |
| Compute/post share (includes grass culling) | 3.3% (64 dispatches) | 9.6% (135 dispatches) |
| Process private memory* | 25.0 GB | 28.0 GB |
| GPU dedicated memory* | 9.9 GB | 11.5 GB |

\* Measured with RenderDoc attached right after a capture, which inflates both numbers; only the difference counts.

- **It works without a cache.** Grass beyond the loaded cells appeared as more Grass Optimizations buckets.
- **Cost:** about 3 GB more RAM, 1.6 GB more VRAM, and roughly 7 more points of GPU share.
- **Not checked:** how far the grass visibly reaches. Spot B's view is mostly inside the loaded cells. The capture can't count instances, because indirect draw arguments live in a GPU buffer.

## CPU and GPU frame time, object LOD on vs off (PresentMon)

Setup:
- **Run conditions:** same spots and profile, no RenderDoc, your own Community Shaders settings (DLSS, no frame generation).
- **Vsync:** off for these runs, through a temporary `SSEDisplayTweaks.ini` in overwrite with `EnableVSync=false` and `EnableTearing=true`, moved out afterwards. The 240 FPS limit was never reached.
- **Tool:** Intel PresentMon 2.6.0 (`D:\Modlists\tools\PresentMon`, run elevated), one 20-minute recording of `SkyrimSE.exe` (`pm-session.csv`).
- **Windows:** 30 seconds per state, standing still, logged in `pm-phases.txt` and sliced with `scripts/lod-measure/pm_windows.js pm-session.csv pm-phases.txt 7`. PresentMon's clock read 7 hours behind local time.
- **Object LOD off:** console `slod 0 0 0 0`, which sets the three TerrainManager block distances to 0. Object LOD disappears and terrain LOD stays, checked in screenshots.
- **Object LOD back on:** `slod 57344 147456 327680 1` (the INI values) plus a teleport (`cow`). LOD only comes back after the teleport.
- **There is no console command for object LOD alone**, and `tll` (terrain LOD) has crashed the game before.

Medians per 30-second window (milliseconds):

| Spot | Frame time on → off | CPU busy on → off | GPU busy on → off | FPS on → off |
| --- | --- | --- | --- | --- |
| D forest | 12.46 → 9.43 (−3.03, −24%) | 12.39 → 9.37 (−3.02) | 6.71 → 5.91 (−0.79) | 53 → 112 |
| B Whiterun tundra | 8.82 → 7.33 (−1.50, −17%) | 8.76 → 7.26 (−1.50) | 6.65 → 6.04 (−0.61) | 113 → 133 |
| A Solitude shore | 4.98 → 4.31 (−0.66, −13%) | 4.92 → 4.25 (−0.67) | 4.85 → 4.37 (−0.48) | 190 → 204 |
| C Winterhold | 9.65 → 9.01 (−0.64, −7%) | 9.59 → 8.95 (−0.64) | 6.17 → 5.95 (−0.22) | 102 → 109 |

**D-on had stutter.** Its mean frame time (18.9 ms) and p95 (79 ms) are far above its median: it was the first window after arriving, while things were still streaming in. Its FPS (53) reflects those spikes; the median is the fair comparison.

Findings:

1. **The game is CPU-limited at every spot.** CPU busy time is about equal to the frame time, while the GPU is busy only 4.4–6.7 ms.
2. **Object LOD costs mostly CPU time.** It costs 0.6–3.0 ms of CPU per frame (7–24%), against 0.2–0.8 ms of GPU. The CPU cost follows the draw count from the captures: D, about 2,000 draws, costs 3.0 ms; A, about 280 draws, costs 0.7 ms. That's roughly 1.5 µs of CPU per object LOD draw, counting both passes plus the water cubemap.
3. **Batching is the right lever.** A GPU-instanced LOD layer that turns roughly 2,000 draws into tens would save most of that 0.6–3 ms. Keeping object LOD out of the water cubemap would save about 40% of these draws with no new renderer.

Caveats: one 30-second window per state, run one after the other rather than interleaved; the game clock kept moving; and `slod 0` may also change which terrain LOD level is shown (about 60 draws, minor).

## Keeping object LOD out of the water reflection (`bReflectLODObjects`)

- **Where the pass comes from:** the "Water Effects > Cubemap" pass is the game's own water reflection (`Main::RenderWaterEffects` → `BSCubeMapCamera::RenderCubemap`); Bottled only annotates it, see `FrameAnnotations.cpp`. Bottled's Hi-Z "Cubemap LOD" entry is just a debug label.
- **The switch already exists:** the game's `[Water]` settings in `skyrim.ini` choose what gets reflected: `bReflectLODObjects`, `bReflectLODLand`, `bReflectLODTrees` and `bReflectSky`, all 1 on cs-base.
- **No patch needed:** `setini "bReflectLODObjects:Water" 0` takes effect at runtime, without a restart or teleport.
- **Draws:** RenderDoc at spot B shows the reflection pass then holds only terrain LOD and sky. Its object LOD draws go from 845 to 0, and total draws from 8,791 to 7,934 (`B-reflectLODObjects0` capture).
- **Visual check (Solitude shore):** screenshots with the setting on and off, comparing the open-water area, showed no visible difference. Most of the reflection there is sky and the nearby shore. Not checked: a large lake reflecting distant mountain objects at a low angle.

CPU timing, PresentMon, windows alternating on/off/on/off at each spot (`pm-reflect2.csv`, `pm-reflect2-phases.txt`). The machine was idle; the first attempt (`pm-reflect.csv`) was discarded because another session's Rust build and `lodforge-qos` run kept the CPU at 80–100%.

| Spot | Frame time median, on (two windows) | Off (two windows) | Saving |
| --- | --- | --- | --- |
| D forest | 7.15 / 6.86 ms | 6.57 / 6.25 ms | ≈0.6 ms (8–9%); GPU busy −0.3 ms |
| B Whiterun tundra | 8.87 / 8.92 ms | 8.69 / 8.15 ms | ≈0.5 ms (5%); GPU busy −0.1 ms |

- **The saving is smaller than estimated:** about 0.6 µs of CPU per removed reflection draw, not the 1.5 µs predicted. Reflection draws seem cheaper per draw than main-view draws.
- **It costs nothing to try:** for most views it's a free 5–9%.
- **Not applied anywhere yet:** the cs-base profiles still have `bReflectLODObjects=1`.

**A doubt about the earlier object-LOD run:** in this idle run spot D ran at about 7 ms with LOD reflections on. The object-LOD-on recording above had 12.5 ms there. That run may have been slowed by background load too, at least in the D-on window, which had stutter. Treat its D numbers (−3.0 ms) as an upper bound, and repeat the object-LOD on/off windows on an idle machine before relying on them.

## What this changes in the plan

- **First, cheap fixes to existing LOD:** keep object LOD out of the water cubemap (up to 15% at B), and drop LOD from the depth prepass where it isn't needed. Both are small Bottled-side changes.
- **The instanced LOD layer** would mainly save CPU: 1,250–2,000 object LOD draws become tens. To show that, the next measurement must be CPU frame time, for example PresentMon or Bottled's performance overlay, with object LOD on and off, without RenderDoc attached.
- **Far grass:** NGIO can already push grass past the loaded cells. The limit is memory (+3 GB / +1.6 GB at 2.4× the default reach). GPU-instanced far grass from compact per-cell data (tier 2) is the way to reach further without that memory cost.
- **Impostors (tier 1 and 2)** would replace the billboard cards and 3D trees inside object LOD, so LODForge has to export trees as instances instead of baking them into the `.bto` files.
