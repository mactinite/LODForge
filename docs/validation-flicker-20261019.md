# Distant glacier/terrain flicker: consecutive-frame A/B (2026-10-19)

**Verdict (2026-10-20, after a live session with the user; tabled): the flicker is
real, is not output-specific, is not caused by the temporal upscaler, and is a
render-side alternation between two shadings of the same distant LOD.** At the tundra
spot it can be re-armed on demand (Winterhold, forced overcast snow, Solitude, tundra,
Winterhold, tundra, clean preset: 3 of 3 launches), after which the distant mountains
alternate between a hazier, flatter rendering and a crisper, higher-contrast one every
2-4 s for minutes, with the player and camera fixed. It is the same with the LODForge
and the DynDOLOD outputs; it persists with DLSS and frame generation off (native, no
jitter), with DynDOLOD's dynamic LOD set inactive, with Screen Space GI disabled, and
at 5% game time (the rhythm follows real time); snapping the weather does not stop it;
full models in the same frames do not change at all. A fresh launch with a single
teleport never shows it, and the user has not seen it in normal play. Which renderer
feature alternates is open (Community Shaders' Volumetric Lighting and Cloud Shadows
first, then Skylighting and Terrain Shadows); the user tabled the question on
2026-10-20 because it is hard to reach, absent from normal play and identical with
the DynDOLOD output. Nothing in the LOD generation changes it; keep
`uLargeRefLODGridSize=11`; the one generator-side item still worth shipping is the
terrain mip chain (quality, not this flicker). Details in "Live session" below; the
sections before it are the earlier A/B and keep their numbers.

Evidence: `bench/flicker-20261019/` (frames + sidecars per run, `analysis/` with
per-burst JSON, std heat maps, event maps, crop strips, pair-diff strips,
`temporal.py`, `pairdiff.py`, `series.py`, Community Shaders logs per run,
`SettingsUser.backup.json`, `modlist.backup.txt`).

## Setup

- Skyrim SE 1.7.104, SKSE 2.3.1, DynDOLOD DLL NG 3.0.43, Community Shaders 1.9.1
  (HDR10 output, DLSS 4.5 at 0.667 render scale, FidelityFX frame generation through
  the D3D12 proxy in the user's settings). Driven through `skyrim-mcp`.
- Profiles: `LODForge-Full-Validation-20261019` (mod `LODForge Full Validation
  20261019`) and `LODForge-Full-Golden-20261019` (`LODForge E2E - DynDOLOD Output` +
  TexGen, vanilla BSA terrain LOD). Both ship `DynDOLOD.esm/.esp` and `Occlusion.esp`.
  The Default profile was not touched.
- Capture: `game_screenshot source=frame frame_generation=hudless preset=raw
  settle_ms=0`, 20-30 shots per burst, 165-210 ms apart (about 5 frames/s, bursts of
  4-6 s). `hudless` keeps the D3D11 frame before frame interpolation; a `source=window`
  burst (the composited output the user sees) was taken at every spot in the baseline.
  Frames are HDR10 converted to 8-bit sRGB by the bridge.
- Metrics (`temporal.py`, luma 0-255 on a region of interest): per-pixel std over the
  burst; share of pixels whose range exceeds 24; "events" = a jump > 24 between
  consecutive frames that reverts within 2-3 frames; share of pixels differing by > 16
  between consecutive frames (mean and max over the burst).

## Reproduction recipe

1. `game_launch profile=<profile> instance=skyrim-cs cell=WhiterunExterior01`; wait for
   `MessageBoxMenu`, decline Survival Mode with `game_menu callback MessageBoxMenu
   buttonPress [1]`.
2. Console: `tgm`, `tai`, `tcai`, `tdetect`, `tg`, `set timescale to 1`.
3. `cow Tamriel <x> <y>` to the spot's cell, then one `preset=clean restore=false
   view=free` shot with the vantage below (forces SkyrimClear at 12:00, hides the HUD,
   places the player and the free camera; settle 8 s). The player stands under the
   camera in the same cell; occlusion and the LOD grid come from the player.
4. 20-30 `preset=raw settle_ms=0` shots in a row (parallel tool calls are serialised by
   the server; each gets its own frame).
5. `temporal.py <dir> <prefix> --roi x,y,w,h`.

Spots (player `pos`, free-camera `eye`, `look` [pitch, yaw], fov):

| Spot | cow | pos | eye | look | fov | ROI (distant LOD) |
|---|---|---|---|---|---|---|
| A Solitude shore, glacier field over the Sea of Ghosts with the lighthouse at left | -13 28 | -52200, 115500, -13450 | -50000, 117500, -13300 | -1, 40 | 50 | 250,300,720,130 |
| B Tundra east of Whiterun, distant mountains at left | 3 -5 | 14197, -19369, -3942 | 14197, -19369, -3342 | 2, 90 | 75 | 0,170,600,200 |
| C Winterhold north, glaciers at right | 27 23 | 113230, 94803, -7799 | 113230, 94803, -7199 | 4, 0 | 75 | 900,330,380,200 |

Pitfalls met: `game_move` drives the free camera, not the player, while `tfc` is on
(use `player.setpos`); the forced clear weather lapses back to the region's weather
within seconds at the Solitude coast (re-apply before each burst, check `game_query
scene`); the first baseline Solitude burst has a one-off exposure shift between frames
2 and 3 because 40 s passed between them (not flicker).

## Baseline (LODForge output, user settings: frame generation on, DLSS)

| Burst | frames | ROI std | event share | range>24 share | consec. diff mean / max |
|---|---:|---:|---:|---:|---:|
| A glacier, hudless | 25 | 1.02 | 0.0002 | 0.0004 | 0.0001 / 0.0002 |
| A glacier, window (post frame generation) | 10 | 0.47 | 0.0000 | 0.0000 | 0.0000 / 0.0001 |
| B mountains, hudless | 25 | 1.54 | 0.0121 | 0.0216 | 0.0036 / 0.0205 |
| B mountains, window | 5 | 1.11 | 0.0026 | 0.0104 | 0.0067 / 0.0114 |
| C glaciers, hudless | 25 | 1.14 | 0.0024 | 0.0043 | 0.0011 / 0.0082 |
| C distant mountain, hudless | 25 | 0.88 | 0.0003 | 0.0014 | 0.0003 / 0.0004 |

- Spot A: the glacier field does not flicker. All temporal variance in the frame is
  water and cloud motion (`analysis/ours-solA-hudless-stdheat.png`). `tll` removes the
  whole glacier field (`ours/solA-lodoff.png`): it is pure object LOD here, no loaded
  full model, so there is no large-reference z-fighting at this spot. Teleporting the
  player across the cell boundary (x -52200 to -53600) changes 0.01% of the glacier
  ROI: no culling pop.
- Spot B: a periodic pulse. In the 25-frame burst, frames 2->3 and 21->22 change 8.9% and
  4.2% of the mountain crop by > 16 levels (`analysis/ours-tunB-frames2-5.png`,
  `-frames20-23.png`); the snow region's mean G steps 228.7 -> 231.1 and decays back
  over about 10 frames (`series.py`). The event map (`analysis/ours-tunB-hudless-events.png`)
  puts the events on the distant mountains and slopes, plus the turning windmill and a
  flag. `tll` removes the whole range (`ours/tunB-lodoff.png`): object LOD + terrain LOD.
- Spot C: the glacier ROI shows the same step-and-decay events as spot B, on the
  glaciers themselves (a pale shelf appears at the waterline for one captured frame,
  `analysis/ours-winC-glac-zoom-11-12-13-14-15.png`); the sea region next to them is
  flat at G 199.4 and the full models (College, rocks), sky and sea do not change
  (`regions.py`). An earlier draft of this line attributed the activity to the sea
  surface; that was wrong. The distant mountain ROI is flat.

## A/B, one variable at a time (same spots, same bursts)

| Run | Variable | B mountains event share | B consec. diff max | pulse present | A glacier event share |
|---|---|---:|---:|---|---:|
| ours (baseline) | FG on, DLSS, Occlusion on, LOD4-only mips | 0.0121 | 0.0205 | yes (frames 3, 22) | 0.0002 |
| ours-fgoff | (a) frame generation off | 0.0092 | 0.0158 | yes (3, 18) | 0.0000 |
| golden-fgoff | DynDOLOD output, frame generation off | 0.0129 | 0.0140 | yes (1, 7, 17) | 0.0000 |
| ours-noaa | (d) upscaler none (no DLSS/TAA), FG off | 0.0060 | **0.0016** | **no** (flat series) | - |
| ours-noocc | (b) Occlusion.esp deactivated, user settings | 0.0135 | 0.0148 | yes (1, 6, 11, 15) | 0.0001 |
| ours-mips | (c) full terrain mip chains (sibling build), user settings | 0.0116 (30 f) | 0.0151 | yes (0, 15, 26) | 0.0001 |

Notes per variable:

- (a) Frame generation: the pulse is in the pre-interpolation frame; turning FG off
  changes nothing. The composited window frames show the same stability at A and the
  same activity at B.
- (b) Occlusion.esp: no effect at rest (pulse unchanged) and no effect across the cell
  crossing at A (0.1% of ROI pixels differ before/after with it off, 0.01% with it on).
- (c) Terrain mips: the first 20-frame burst happened to fall between pulses
  (`mips-tunB`, event share 0.0027); the 30-frame repeat (`mips-tunB2`) shows the same
  pulses as every other DLSS run. Mip counts were verified on the loaded tiles
  (`Tamriel.8.0.-8.dds` 1 mip in the validation mod, 10 mips in the test mod).
- (d) Upscaler: with `Upscaling.upscaleMethod 0` the snow region's mean G is flat at
  223.7 +-0.1 over 25 frames and the consecutive-difference share never exceeds 0.16%
  (that residual is plain aliasing on the un-antialiased mountain). The pulse therefore
  needs the temporal accumulation of the upscaler (DLSS here; the FidelityFX/TAA paths
  were not run, so they are unverified).
- (e) Large references: at spot A the glaciers are LOD-only (`tll` empties the view), so
  the reported glacier flicker is not full-model/LOD z-fighting at this spot. A spot
  inside a glacier's large-reference grid was not tested.

Golden vs LODForge under identical settings (frame generation off): A 0.0000 vs 0.0000
event share, B 0.0129 vs 0.0092, C 0.0152 vs 0.0024 (the golden C burst contains more
sea). The pulse is shared; the DynDOLOD reference is not quieter.

## Live session with the user (2026-10-20, `ours-r1` .. `ours-r5-mcm`)

The user had seen the glaciers at spot C and the mountains at spot B switch between
"far" and an "intermediate" look while the player stood still. Re-reading the earlier
bursts at exactly those regions (`geom.py`, `regions.py`, zoom strips in `analysis/`)
confirmed the change is on the distant LOD only: glacier and mountain regions step,
full models, sky and sea do not. The user then watched the live window while the
following runs were made, LODForge profile, same vantages and clean preset as above.

Winterhold spot C did not reproduce in four bursts (settled clear weather, during a
forced clear-to-overcast transition, 15 s after three teleports): glacier regions
under 0.15% changed pixels per frame. The tundra spot B, in launch order:

| Run | Upscaler / FG | `uLargeRefLODGridSize` | DynDOLOD DLL NG | teleports before | frames | consec. diff max / mean | snow luma max step | user |
|---|---|---:|---|---:|---:|---:|---:|---|
| `ours-r1/tunB5` (long session, after Winterhold runs and a forced overcast) | DLSS + FG (user settings) | 11 | on | 6 | 25 | 0.0152 / 0.0044 | 1.80 | flicker, persists for minutes |
| `ours-r2-noaa` (fresh launch) | none, FG off | 11 | on | 1 | 40 | 0.0014 / 0.0011 | 0.03 | none |
| `ours-r3-largeref5` (fresh launch) | DLSS + FG | 5 | on | 1 | 25 | 0.0011 / 0.0009 | 0.53 | none |
| `ours-r4-nodll` (fresh launch) | DLSS + FG | 11 | off (mod disabled) | 1 | 25 | 0.0002 / 0.0001 | 0.49 | none |
| `ours-r5-mcm` (fresh launch, control) | DLSS + FG | 11 | on | 1, then 3 | - | - | - | none, none |

Snow luma is the mean G of region 150,200,250,70; "max step" is the largest change
between consecutive captured frames; 0.5-level steps are DLSS jitter noise without the
decay pattern. The last row is the control that was missing: every setting at the
user's values, and no pulse after the first teleport nor after a Solitude round trip,
confirmed on screen. So rows 2-4 do not isolate anything; they differ from row 1 in
being fresh single-teleport launches, exactly like the control. In `ours-r5-mcm` the
weather was then forced cloudy (`fw 12F89`), clouds arrived, and the clean preset
forced clear again to mimic the pulsing runs; the game's main thread hung within two
minutes (bridge requests timed out, no crash log) and the run ended with `qqq`. At the
moment of the hang CommunityShaders.log shows a fresh render-target setup block (the
HDR "Upgraded render target" lines and the SceneSettings discovery that otherwise
appear only at startup), so a swap-chain or device re-creation was in progress.

### Re-armed runs (`ours-r6-rearm` .. `ours-r8-noaa-rearm`)

The sequence that produced `ours-r1/tunB5` re-arms the pulse reliably: launch, cow
Winterhold (27 23), clean preset, `fw 4D7FB` (SkyrimOvercastSnow), wait 40 s, cow
Solitude (-13 28), cow tundra (3 -5), cow Winterhold, free camera seated 45 s, cow
tundra, clean preset. Three launches, three pulses, each confirmed on screen. The
Winterhold visit, the forced snow weather, the two extra teleports and the extra
minutes were not separated (the same launch without them, `ours-r5-mcm`, showed
nothing after one teleport and after three).

| Run | Change | Step | User | Numbers |
|---|---|---|---|---|
| `ours-r6-rearm` | none (DLSS + FG, grid 11, DLL on) | re-arm | flicker | snow G 227.7-230.5, steps every 2-4 s |
| `ours-r6-rearm` | `SHESON_DynDOLOD_Plugin.SetMyActive(false)` via `call_papyrus` (the MCM's "DynDOLOD is active") | in-session | still flickers | snow G 230.0-231.3 |
| `ours-r6-rearm` | `sgtm 0.05` (game time at 5%, renderer at full speed) | in-session | same rhythm; "shading, not LOD" | - |
| `ours-r7-nossgi` | Screen Space GI disabled | relaunch + re-arm | flicker | - |
| `ours-r8-noaa-rearm` | upscaler none, FG off (native) | relaunch + re-arm | flicker | two flat states: snow G 232.0 / 233.2, sharpness 5.34 / 5.49; 12 of 40 captures in the crisp state, singly and in runs of four; consecutive diff 0.012-0.029 at every switch, 0.002-0.005 otherwise |
| `ours-r8-noaa-rearm` | `sw 81A` (weather set, no blending) | in-session | still flickers | - |

So `ours-r2-noaa` ("upscaler none, no flicker") was the fresh-launch confound, not an
isolation: at native the pulse is a clean alternation between two renderings of the
distant LOD, not DLSS history. The magnified strip
`analysis/ours-r8-raw-zoom-8-9-10-11.png` shows the two states, one hazier and
flatter, one with darker rock faces and crisper snow; the whole-frame difference
`analysis/ours-r8-raw-fulldiff-8-9.png` lights up the distant range and the slopes
and nothing else except the windmill and swaying trees (tower and house 0.0000). The
user's description, "snow loses its pattern, hills lose the dark green, like
z-fighting", matches the hazy state.

Hypotheses and their state: shader compilation (ruled out, no compile lines in any
run's CommunityShaders.log); snowflake particles (ruled out); teleport count alone
(ruled out, `ours-r5-mcm` had three); DLSS or temporal upscaling as the cause (ruled
out, it only softened the alternation into a step-and-decay); DynDOLOD's dynamic LOD
(ruled out, inactive makes no difference; the DLL-off and grid-5 runs were fresh-launch
confounds and are void); Screen Space GI (ruled out); game-time driven causes such as
weather blending, sun and shadow updates and script timers (ruled out by the 5%
game-time run); an ongoing weather blend (ruled out by `sw`); a jitter-driven z-fight
(ruled out, it alternates coherently over the whole surface at native); draw-order
z-fighting between object LOD and terrain LOD (open; two flat states fit two surfaces
taking turns, but the states differ in haze and contrast rather than in which texture
shows); a Community Shaders feature with a real-time cycle on distant geometry (open:
Volumetric Lighting exterior, Cloud Shadows, Skylighting and Terrain Shadows are
enabled; SSGI is excluded).

DLL facts that stand: both outputs ship DLL NG 3.0.43 data in plugin format 3.07
(`DynDOLOD_NG_Worlds.txt`, `DynDOLOD_NG_Tamriel.txt`; `[Markers]`, `[Disabled]`,
`[Resets]`; `crates/dyndolod/src/dynplugins.rs`, `largerefs.rs`), LODForge's marker
cells match the golden rules (861 of 873 Tamriel cells), and the DLL's MCM in this
version has no large-reference workaround switch (only "DynDOLOD is active" and
"Large References Child Worlds").

Harness notes: the game hung twice when its window regained focus during a bridge
session (CommunityShaders.log shows a render-target rebuild at that moment; `qqq`
ends it, no crash log), so the in-menu Community Shaders toggles could not be used;
settings changes need a relaunch and the re-arm sequence, about five minutes each.
`call_papyrus` on `SHESON_DynDOLOD_Plugin` works for the DLL's native toggles.

If it is ever picked up again: re-arm with the sequence above, confirm on screen,
then one relaunch per candidate with the feature disabled in
`overwrite/SKSE/Plugins/CommunityShaders/SettingsUser.json` (Volumetric Lighting
`ExteriorEnabled`, Cloud Shadows and Skylighting through "Disable at Boot", Terrain
Shadows `EnableTerrainShadow`), re-arming each time; and separately the same sequence
without the forced weather and without the Winterhold visits to see which part arms
it. Leave `frameGenerationMode 0` (the user's default since 2026-10-20) afterwards.

## What LODForge should change

Nothing for this flicker. The terrain mip chain (merged to main at 8803bd2) should
still land for distant-terrain quality, but it does not touch the pulse. Both outputs
alternate the same way under the same conditions, nothing the generator writes is
implicated by the evidence, and the alternation is in how the distant LOD is lit or
fogged by the renderer, not in the meshes or textures. Keep `uLargeRefLODGridSize=11`.

## Not covered

Which Community Shaders feature alternates, which part of the re-arm sequence arms
it, the player camera instead of the free camera, moving-camera captures, the
DynDOLOD profile beyond `golden-fgoff`, other weathers and night, TAA and FSR, and
the window-focus hang. Tabled by the user on 2026-10-20.
