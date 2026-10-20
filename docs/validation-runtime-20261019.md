# Runtime gap qualification: 2026-10-19

Later result: [child resets and tree timing](validation-runtime-batch-20261019.md).
The measurements below remain the earlier full-build/replacement baseline.

**Verdict:** additional dynamic families and native LOD assignments are recovered,
with geometry preserved and representative in-game transitions verified.
The latest [replacement follow-up](#replacement-follow-up) also validates
windmill/water-wheel lifecycle and fixes ESM override registration.
**Not full parity:** generated patches, remaining Disabled/child-world selection,
Delay, destruction, base selection and grass/material residuals remain open.

This is a working-tree follow-up to [f1a31e6](validation-20261019.md), not a new
reference generation. The [gap plan](gap-closure-plan.md) owns the next steps.
Sections before the replacement follow-up describe the earlier full-build
qualification; their counts and limitations retain that historical scope.

## Changes and evidence

- **Full-model fallback:** `Unchanged` dynamic rules can use the original model
  when neither a dedicated dynamic model nor usable level-0 LOD exists. Golden
  base-copy MODL records confirm this for doors, rapids and enable-parent trees.
  `Replace` rules remain excluded without their replacement/disable machinery.
- **Empty rules:** a no-level/no-grid/non-VWD match blocks this fallback when no
  LOD model/billboard exists. Treating every such rule as a universal exclusion
  incorrectly removed 636 golden object placements; that approach was rejected.
  Existing resource-backed selection is preserved.
- **Persistent full LOD:** persistent `Is Full LOD` references without a matched
  rule receive full-model dynamic LOD before the writer clears the original flag.
  Explicit Disable rules still win; missing source assets fail explicitly.
- **Native MNAM aliases:** when filename/CRC discovery finds no levels,
  HasDistantLOD bases can use existing MNAM paths with different filename stems.
  This restores four CC Hall object placements without overriding discovered LOD.
- **Diagnostics:** `dyndolod-export --runtime-report <path>` writes source
  identities, rules, reference state, selections and NIF metadata as JSON.
  Diagnostic `--plugins` output is not the full integrated build's workaround
  package; use `build` output for runtime qualification.

## Reproduction and storage

The full command is unchanged from the [pinned run](validation-20261019.md#reproduction-and-scope):
`LODForge-E2E`, Medium, Tamriel/default season, TexGen, terrain, objects, trees,
underside, plugins, occlusion, cached BTO grass at 100%, eight Rayon threads.
All three prior E2E output mods are excluded.

The first attempt exhausted D: and failed objects/trees. Only that attempt's
incomplete Data/work directories were removed; its log is retained. A fresh
complete build used C: scratch storage and finished in **148.921 s**, zero failed
stages. This is a single run, not a controlled speed benchmark.

Persistent output: `skyrim-cs\mods\LODForge Runtime Gaps 20261019`.
Isolated profile: `LODForge-Runtime-Gaps-20261019`, with no copied saves and the
old validation/reference output mods disabled. All **8,524 installed file hashes**
match the completed build. C: scratch copies were removed after verification.
Evidence: `bench\runtime-qualification-20261019`; selection diagnostics:
`bench\gap-closure-20261019\runtime-fallback-report.json`.

## Offline qualification

| Check | Result |
|---|---|
| Output | 8,524 files; 11,743,535,499 bytes |
| Structural and asset validation | 1,711 NIF/BTO/BTR, 5,243 DDS, 2,443 unique assets; **zero errors/missing assets** |
| Dynamic originals | **3,042/3,078**, zero extras; **84 recovered**, no previous originals removed |
| Global Bases | 510/593, unchanged selection boundary |
| Disabled / Markers / Resets | 0/39; 865/877; **38/57** (resets previously 23) |
| Classic trees | **51,566/51,566**, identical block/scale; seven old extras now have dynamic LOD |
| Snow no-alpha linear RGB PSNR | **48.727 dB**; alpha-threshold agreement 100% |
| Preserved pinned files | 7,912 byte-identical; 612 changed; no removed paths |
| Reference preservation | All 1,499 reference hashes and six original-profile file hashes unchanged |
| Focused checks | 42 tests passed; one opt-in GPU fixture ignored; release build and focused Clippy passed |

Workspace `cargo fmt --all --check` still fails on existing formatting debt.
The new runtime module's formatting check passes. Existing Clippy advisories and
the missing input archive warning for `Skyrim - Patch.bsa` remain visible.

### Geometry and materials

The direct pinned-output comparison preserves **100% of previous object triangles
at LOD4/8/16 and all 125,162,456 grass triangles**, using the existing 0.25-unit
position grid. Added object triangles: **1,028 / 854 / 854**. These are geometry
checks, not byte equality or full-attribute equivalence after atlas repacking.

Against DynDOLOD, on common BTO filenames:

| Level | Golden object triangles | Current | Shared |
|---|---:|---:|---:|
| 4 | 5,596,496 | 5,591,184 | 5,444,367 |
| 8 | 2,877,472 | 2,891,856 | 2,728,472 |
| 16 | 828,532 | 876,262 | 730,470 |

There are 686 current BTOs versus 685 golden; comparisons with the pinned output
include the extra current/pinned file, so their LOD4 totals differ from this table.
Grass's poor golden overlap is unchanged, not fixed by runtime selection.

Only three DDS files differ from the pinned run: repacked main object
diffuse/normal atlases and the corrected snow linear copy. Alpha pages, tree
atlas, mini atlases and other textures remain byte-identical. Shape counts remain
3,280/973/324 non-grass and 863/108/30 named HD groups at LOD4/8/16.
Nine heightmap cells changed versus the pinned run. Occlusion remains 6,279/6,285
cells with common-cell bit Jaccard 0.996178; neither materials nor occlusion have
full parity.

### Runtime fields: keep the comparison population explicit

All previous originals retain their runtime fields except the intended initial
state correction. Across the expanded matching-original population:
InitiallyEnabled has **zero** mismatches, Delay **385**, LargeReference **6**,
Level **2**, destruction index/type **12 each**. The prior population had Delay
362 and LargeReference 2; the increases are newly selected rows exposing existing
metadata gaps, not changed fields on old originals. Golden duplicate rows remain
in these field counts; distinct original counts do not include duplicates.

## Live checks through Skyrim MCP

Runtime 1.7.104.0, SKSE 2.3.1.0, DLL NG 3.0.43.0; bridge handshake verified.
Native `GetSuccessful` and `GetMyActive` returned true; `GetResetsCount` returned
38. Papyrus reports successful initialization; CS TruePBR loads and hooks.

- Fresh initialization and save creation passed. Reload helper timed out waiting
  for a player `cell_loaded` event, but subsequent state showed one game load,
  no loading menu/pause, a loaded player and successful native status.
- Recovered rapid Skyrim `0x3919F` and tree `0x65605` both enabled and loaded 3D
  in their near-LOD regions. The tree correctly follows an inverted, disabled
  **interior** parent (`0x71C2C`, Sky Haven Temple).
- Recovered banner `0x97EDE` began disabled. Enabling parent `0x7265C` and crossing
  a cell boundary enabled/loaded its dynamic copy; disabling the parent and
  crossing back disabled/unloaded it. Parent state was restored. An in-cell
  84-unit walk alone did not trigger the observed update: no instant-update claim.
- The recovered door's generated reference resolves and remains disabled as
  expected; building the Hearthfire home/activating the door was **not** tested.
- Entering WhiterunWorld switched the tree copy's world and disabled/unloaded it;
  returning to its Tamriel near-LOD region restored enabled/loaded 3D.
- Two matched-vantage screenshots were inspected. SSIM to the earlier golden:
  tundra **0.9145**, Winterhold **0.9868** (pixel differences 6.52%/0.73% at tolerance
  16). These are smoke diagnostics, not passing visual-equivalence thresholds;
  weather, foliage motion and runtime timing are not perfectly locked.
- First shutdown required forced game/MO2 cleanup. A separate fresh launch
  initialized successfully and exited through console `qqq` without forced
  termination; MO2 then closed normally. The first shutdown's cause is unresolved.
  No crash logs were reported. Existing unrelated Papyrus errors remain.

The later harness investigation traced the recurring "Failed to run SKSE"
dialog to MO2's command-line handling of Root Builder's cancelled outer launch,
not a DynDOLOD initialization failure. SkyrimMCP now uses shortcut launches for
normal game runs and attempts console quit after a bridge timeout. Separate
live normal/forced-exit checks needed no window dismissal. This does not erase
the original qualification run's forced cleanup.

Final game/MO2 processes and machine lock are released. The first run created an
empty game-root `Mods` directory, inspected and removed; the retry's game-root
check is clean. Original profiles, reference output and older validation saves
are preserved. This is not rolling-update, replacement/destruction, all-world,
all-season or whole-modlist certification.

## Replacement follow-up

### Implemented and checked

- `Replace` uses the full model, preserves the generated copy's transform,
  retires the original as an underground XMarker without its enable parent,
  and writes `[Disabled]`. Delay, startup state, self-parent and replaced-parent
  links follow the golden records. Missing source/cell records fail explicitly.
- **Important live-found fix:** ESM temporary overrides need TES4 `ONAM`.
  Before the fix, a farm-parent toggle enabled both the original fan and its
  generated copy. Adding the 19 ONAM identities fixed this; every other ESM
  record/header field and all three other overlay files remained byte-identical.
- **45 focused tests pass** (27 DynDOLOD, 18 CLI; one opt-in GPU fixture ignored).
  Release build and focused Clippy pass, with existing advisories.
  Repeated final builds produce identical plugins/data and match the installed
  four-file overlay hashes.

| Check | Result |
|---|---|
| Selected source originals | 3,061; 3,060 shared with golden, 18 missing, one explained extra identity |
| Recovered replacements | 19 selected; 18 previously missing golden source originals recovered |
| Previous runtime rows | All 3,042 byte-for-byte unchanged |
| Added matching replacement fields | No mismatch after normalizing generated self-parent IDs |
| Disabled / Bases / Resets | 19/39, zero extras / 510/593 / 38/57 |
| Focused integrated validation | 743 files, 687 NIF/BTO, 51 DDS; 351 assets; zero missing/structural errors |
| Installed sparse overlay | Two plugins + two NG data files; 308 assets resolve against the pinned full mod |
| Pinned full-output preservation | All 8,524 file hashes unchanged, including geometry and materials |
| Corrected focused build | 16.886 s, zero failed stages; not a full-build performance comparison |

The source-original denominator excludes four generated DynDOLOD patch
originals. Counting those gives 22 missing identities, not 18. The extra source
identity is Katla's fan `Skyrim.esm:05C854`: the golden uses a generated original
at a corrected position/rotation. Patch application remains open; no hard-coded
exclusion was added to conceal it.

### Runtime boundary and results

Isolated profile `LODForge-Replacements-20261019`, no copied saves. Mod
`LODForge Replacements 20261019` overlays the preserved full
`LODForge Runtime Gaps 20261019` package. No grass-off BTOs or repacked atlases
were installed. Evidence: `bench\gap-closure-20261019\replacement-*`.

The first attempt traveled while the Survival Mode prompt was open. It is
**excluded from lifecycle qualification**. Dismissing it was followed by a
D3D11/Community Shaders-stack crash; logs are preserved, cause not established.
Subsequent runs chose **No before travel**, checked `paused=false` and advancing
game time, and used unpaused waits. The unpaused duplicate-fan regression above
was then found, fixed and retested.

Final corrected run (Skyrim 1.7.104.0, SKSE 2.3.1.0, DLL NG 3.0.43.0):

- Native success and active status true.
- Sarethi fan original `0007FFD2` is XMarker `0000003B` at Z=-30000.
  Copy `1A001BE6` is enabled/3D-loaded in cell `(27,-18)` and remains so after
  moving to `(30,-18)`, beyond the five-cell loaded grid's edge. Screenshots
  show the full fan; some viewpoints are foliage-occluded.
- Falkreath wheel original `000A1FF0` is the same hidden marker; copy
  `1A001BEA` is enabled/3D-loaded in cell `(-8,-21)`.
- Farm fan copy `1A001BF7` follows parent `FE038E29`: disabled initially,
  enabled/3D-loaded after enabling the parent and crossing to `(-17,0)`,
  disabled/unloaded after restoring the parent and returning to `(-18,0)`.
  Original `FE038C5A` remains an XMarker at Z=-30000 throughout, so enabling
  the parent no longer restores original fan geometry.
- Save `LODForge-replacements-20261019` was written after restoring the parent.
  Console reload completed: gameLoadedCount=1, no loading/blocking menus,
  `paused=false`, native success true; wheel copy loaded and original still
  an underground XMarker.
- Final run and the preceding unpaused run exited cleanly. MO2 closed itself,
  zero windows were dismissed, and game-root checks were clean. The initial
  crash remains recorded rather than relabeled as a passing run.

### Limits

This is a **plugin/runtime overlay qualification**, not a new full geometry or
material comparison. The focused `--keep-lod-mods --grass off --no-terrain
--no-trees --no-occlusion` build changed both main object atlases and produced
970/836 more LOD8/16 triangles than the full qualified build. The cause is not
isolated; those meshes/textures are not installed or claimed equivalent.
All other focused DDS match the pinned full mod. The overlay deliberately uses
that full mod's existing geometry/material assets.

Existing-save upgrades from a pre-replacement generation, all replacement
families/worlds, destruction and complete patch policy are not certified by
this fresh-save/reload test. The original below-ground XMarkers can report
enabled/loaded 3D; the required check is their marker base and hidden transform,
not a blanket expectation that every original's disabled bit is set.
