# Full-output qualification: 2026-10-19

**Acceptance clarification after this run:** exact grass-triangle overlap and
byte-identical rebuilt assets are not release requirements. The user accepts
renderable, visually consistent grass and rebuild variation that remains save
safe. The measurements below are unchanged; their mismatch alone is not a defect.
The full drifting rebuild still needs an actual pre-rebuild-save upgrade test,
and the reported flicker remains unresolved.

**Verdict: structurally valid and substantially closer on runtime selection,
but not DynDOLOD parity or release-ready.** Fresh full generation and live checks
pass their measured scope. Grass, materials and runtime fields still differ.
This pass also found **in-place rebuild variation requiring save/visual checks** and records user-observed
**distant glacier/terrain flicker as unresolved**.

## Scope and preservation

Working tree at `b4870a1`, including the child-reset/tree-Delay changes.
No generator changes were made during this validation.
Tamriel, default season, Medium, TexGen, terrain, objects, classic trees,
underside, plugins, occlusion, cached BTO grass at 100%; eight Rayon threads.
Same `LODForge-E2E` inputs and three old-output exclusions as the
[original full run](validation-20261019.md#reproduction-and-scope).

- Reference: installed `LODForge E2E - DynDOLOD Output`, with its separate
  TexGen dependency only for reference validation/runtime.
- First build/live-tested package: `skyrim-cs\mods\LODForge Full Validation 20261019`.
- Disposable profiles: `LODForge-Full-Validation-20261019` and
  `LODForge-Full-Golden-20261019`. No old output assets enabled in the first.
- Evidence: `bench\full-validation-20261019`, including commands/logs, manifests,
  SHA256 inventories, comparisons, screenshots/sidecars and runtime logs.
- All 1,499 reference hashes, 558 generation-profile/config hashes and
  34,988 previously pinned cache/PGPatcher hashes match. The prior qualified
  full package remains unchanged.

The scratch `Data` folder now holds the **drifting in-place rebuild**, not the
first live-tested build. `seeded-Data` holds its byte-identical clean rebuild.
Do not substitute the scratch rebuild for the installed first-build baseline.

## Build, code and structural checks

| Check | Result |
|---|---|
| Fresh full build | 159.115 s; zero failed stages |
| Output | 8,524 files; 11,744,161,889 bytes |
| NIF/BTO/BTR / DDS validation | 1,711 / 5,243 files; zero checked structural errors |
| Referenced models/textures | 2,447 unique assets; zero missing |
| Golden validation | 686 NIF, 60 DDS, 440 assets; zero errors/missing |
| Workspace tests, excluding UI | 138 pass; two opt-in fixtures ignored |
| Validator tests | Four pass |
| Release build / Clippy / Rustdoc | Pass, with existing warnings |
| Workspace formatting | **Fails** on existing formatting debt; no bulk formatting applied |
| Previous full asset package comparison | 8,519 byte-identical files; only three plugins and two NG data files differ |

All meshes/textures are byte-identical to the preceding full qualification.
The validator is not a complete plugin/form/VMAD or shader-rendering validator.
Warnings remain visible: missing configured `Skyrim - Patch.bsa`; tree generation
reports 156 missing billboard candidates and 29 duplicate IDs despite exact
final classic placements. Grass preparation writes 32/46 billboard types,
excluding 14 by its current rules. Their complete golden eligibility is not certified.

## Distance from the golden output

Geometry: common BTO filenames, positions quantized to 0.25 units, grass separated.
There are 686 current BTO files, 685 golden, 685 shared.

| Geometry | Golden triangles | Current, shared files | Shared triangles | Golden overlap |
|---|---:|---:|---:|---:|
| Objects LOD4 | 5,596,496 | 5,591,184 | 5,444,367 | **97.28%** |
| Objects LOD8 | 2,877,472 | 2,891,856 | 2,728,472 | **94.82%** |
| Objects LOD16 | 828,532 | 876,262 | 730,470 | **88.16%** |
| BTO grass LOD4 | 123,242,568 | 125,162,456 | 1,344,342 | **1.09%** |

Classic trees: **51,566/51,566**, no extras; same block/scale, 214 types,
744 blocks including 391 empty blocks. This does not certify billboard pixels.
All-attribute triangle overlap is only 4.256%, dominated by grass and atlas
UV/layout differences; it is not a standalone visual-quality score.
The heuristic grass-centroid metric is not an acceptance gate: its repeat
control is non-monotonic with tolerance even when grass triangles are identical.

| Runtime identity set | Matched / golden | Extra |
|---|---:|---:|
| Source dynamic originals | **3,060 / 3,078** | 1 |
| Resets | **57 / 57** | 0 |
| Bases | 510 / 593 | 0 |
| Disabled | 19 / 39 | 0 |
| Marker world/cell keys | 865 / 877 | 0 |

The extra source identity is the known Katla fan mismatch. Four golden-generated
patch originals are separate from the 3,078 source denominator: **22 identities
missing in total**, 18 source plus four generated. Golden has 3,088 raw rows /
3,082 distinct originals; current has 3,061. Six shared originals have two golden
occurrences but only one current row.

Across 3,066 shared golden occurrences: **352 Delay**, **6 LargeReference**,
**2 Level**, **12 destruction-index** and **12 destruction-type** mismatches.
InitiallyEnabled, OppositeParent, both world-switch flags, Names3D and normalized
enable-parent identities have zero mismatches. Duplicate-row multiplicity is
reported separately; matching a row is not implementing its destruction lifecycle.

Occlusion: 6,279/6,285 cells, 2,361 identical TVDT, common-cell bit Jaccard
**0.996178**. Object heights: 94.28% exact, 99.021% within one unit.
Different city cell sets and unpinned historical terrain inputs prevent a
terrain/occlusion parity claim.

## Materials and alpha

- Four object atlas names, dimensions, formats and eight-mip counts match.
  Contents/packing do not.
- Non-grass shapes LOD4/8/16: 3,280/973/324 versus 3,271/971/324.
  Named HD groups: 863/108/30 versus 867/108/30.
- Snow no-alpha linear RGB PSNR remains **48.727 dB**, alpha agreement 100%.
  The validator compares 55 same-path/same-size non-atlas top mips; this is not
  all-texture or all-mip pixel equivalence.
- Object alpha-page coverage, first to eighth mip: **.352 -> .332** versus
  **.390 -> .547**. Tree coverage: **.142 -> .203** versus **.257 -> .480**.
- Tree atlas remains 4096x4096 versus 4096x2048.

Coverage is whole-page, not corresponding-tile measurement. Occupancy and layout
differences must not be mistaken for a proven per-material alpha defect.

## In-place full rebuild variation

The identical command into existing output/work folders took 133.745 s and
changed **585 files**: 581 BTOs, two main atlases, the height map and Occlusion.esp.
Both DynDOLOD plugins and both NG data files remained byte-identical.
The repeated package still passes structural/asset validation.

This is not only ordering/repacking. Against the first full output, across all
686 BTOs:

| Objects | Triangles removed | Triangles added | Net change |
|---|---:|---:|---:|
| LOD4 | 8,406 | 22,335 | +13,929 |
| LOD8 | 737 | 1,707 | +970 |
| LOD16 | 675 | 1,511 | +836 |

Grass remains exact. 124 height-map cells and one TVDT cell change.
Atlas entries fall from 650 to 646 with the same 34,213 exported references.

**Control:** a fresh output/work folder seeded with only the first build's
DynDOLOD.esm/.esp takes 139.751 s and reproduces **all 8,524 files byte-for-byte**.
This isolates retained IDs and general warm-cache effects from the problematic
existing generated state. Output versus work-folder contributions are not yet
separated. Investigate the early whole-output VFS registration in
[build.rs](../apps/cli/src/build.rs) and generated-asset discovery before fixing.
The older runtime-overlay-only determinism result remains valid within its scope.

## In game

Skyrim 1.7.104.0, SKSE 2.3.1.0, DynDOLOD DLL NG 3.0.43.0; TruePBR loaded.
Both fresh profiles initialize successfully with 57 resets.
Survival prompts were inspected and declined normally. After the user reported
enemy attacks, earlier captures were excluded; AI/combat AI stayed disabled
between captures, with god mode during travel. Detection/existing combat was
also cleared for Solitude. Accepted shots were unpaused and out of combat.

Player and camera positions, FOV, weather, hour, calendar and loading cell match.
The player was moved into each target cell before the free-camera capture.

| Matched engine capture | SSIM | Pixels differing at tolerance 16 |
|---|---:|---:|
| Tundra east | .9806 | 0.827% |
| Winterhold north | .9731 | 6.545% |
| Winterhold night | .9934 | 0.028% |
| Solitude outskirts | .9714 | 3.048% |

These four still-image diagnostics are **not** temporal or full-material parity.
Engine capture differs from the final HDR presentation path.

Fresh full output additionally passes parent enable/disable plus Solitude
child-world switching for tree `000688C3` / copy `1A001518`, with the original
child tree loaded instead of a duplicate dynamic tree. The Falkreath wheel
original `000A1FF0` is an underground XMarker and copy `1A001005` loads correctly.
The new ESS/SKSE save reloads successfully both in-process and in a fresh launch.
All three game runs quit cleanly; MO2 self-exits, zero windows dismissed,
game root unchanged. FNIS/XPMSE errors occur in both profiles; logs are not clean.

### User-observed flicker remains open

The user reported distant glaciers near Solitude and distant terrain elsewhere.
Three raw **presented-frame** samples per output, at a fixed Solitude camera,
retain HDR conversion metadata; Community Shaders frame generation was active.
Reviewed distant-terrain crops show no obvious terrain disappearance, but contain
moving foliage and advancing lighting. Sparse frames cannot exclude intermittent
flicker or distinguish culling, overlapping geometry, temporal shading and
frame-generation effects. **No cause, fix, or flicker-free claim is established.**

Next: capture consecutive frames/video at the reported region, then controlled
occlusion and rendering-path A/B tests with identical player/camera positions.
Do not label this a LODForge regression merely from a static-image difference,
or dismiss it because a golden run also has temporal changes.

## Qualification still missing

Full runtime-form/VMAD and destruction lifecycle coverage; matched-tile material
and alpha checks; grass type/framing parity; all four seasons, DLC/other worlds,
complex/Ultra trees, Grass Optimizations, settings variants and broad save-upgrade
coverage. Historical terrain inputs and a matched-feature performance/memory
benchmark are also missing. There is **no defensible single overall parity %**.
