# xLODGen baselines

Historical measurement log, not current product status. Start with the
[documentation map](README.md), [parity assessment](parity.md), or
[validation procedure](validation.md). Read the dated section relevant to your
change; results from different profiles, exports and commits are not interchangeable.

**Latest integrated run:** [2026-10-19 runtime gap qualification](validation-runtime-20261019.md):
full Tamriel/default/BTO grass at 100%, 148.921 s; 84 recovered dynamic originals,
four restored object placements, geometry/material checks and live parent/world
transitions. The [pinned f1a31e6 run](validation-20261019.md) remains the control.

## Materials mode (2026-10-20, branch `materials-flag-20261020`)

`build --materials pbr|complex` (default `pbr`). `pbr` is the earlier automatic behaviour:
wherever a `textures\pbr\` source exists TexGen makes `pbr_lod` twins (stitched and rendered), the
AltTextures and DynDOLOD.esm base copies name PBR texture sets and `...linear.dds` copies, and the
`pbr-linear` step writes them. `complex` (ENB complex material or vanilla shading) consults no
`textures\pbr\` source: no twins, no PBR copy aliases or re-uv/atlas-map rows, PBR texture sets and
full-model textures under `textures\pbr\` are ignored by the side files, no `linear.dds`. The
DynDOLOD docs show no object-LOD behaviour specific to ENB complex material (complex *grass* is
detected per shader, `ForceComplexGrass`, docs/re/dyndolod-spec.md §3.5, and is unchanged), so
`complex` means "not PBR".

Smoke, MarkarthWorld, LODForge-E2E, `--mode dyndolod --texgen --no-plugins --no-occlusion`, RTX 4080:
- `pbr` with the new binary vs c0c65dc: all 3,862 output files byte-identical, work-folder files
  identical apart from the output paths in the export header (66 PBR twins, 5 linear textures).
- `complex`: 0 PBR twins, 58 rendered (70 with PBR), no file under `textures\pbr\`, no `*pbr_lod*`
  or `*linear.dds`; AltTextures, re-uv and atlas map name no `pbr`/`linear.dds` path; with plugins,
  DynDOLOD.esm has no `_DynDOLOD_TXST`.

## Terrain LOD mips and grass billboards on the alpha atlas (2026-10-19, branch `terrain-mips-grass-atlas`)

Two output-side candidates for the distant terrain shimmer the user sees with both LODForge and
DynDOLOD output (see the [gap-closure plan](gap-closure-plan.md)). Neither is claimed to fix it;
the in-game re-test is a separate session. Evidence: `D:\Modlists\LODForge-out\mips-20261019`
(`terrain-full`, `terrain-golden`, `dyndolod\Data`, logs, validation and comparison JSON).

### 1. Terrain LOD texture mip chains

`terrain_textures::level_settings` copied the golden xLODGen run's saved settings
(`TerrainDiffuseMipMapLOD4=1`, every other `TerrainDiffuseMipMapLODn` / `TerrainNormalMipMapLODn`
0 in `Plugins.sseviewsettings`): mips on the LOD4 diffuse only. The golden xLODGen output has the
same pattern (LOD4 diffuse 582 x 11 mips + 954 default 32² tiles x 6, LOD4 `_n` 1,536 x 1 mip, every
LOD8/16/32 tile 1 mip). Un-mipped terrain LOD textures alias at distance. New: `terrain-textures
--mips full|golden|none` (`full` default; `build --terrain-mips`), the same 2x2 box chain down to
1x1 for diffuse and normal maps at every level. xLODGen's "raise steepness" (only active with
normal mips) stays off: its formula is not analysed (`re/xlodgen-terrain-textures.md` §6).

`terrain-textures --world Tamriel` (default season, LODForge-E2E with the three E2E output mods
excluded, RTX 4080, release build, one run each; the two outputs were then scanned with a DDS
header grouper and checked with `validate_output`, which verifies the payload length against the
header mip count):

| Tamriel terrain, 2,720 files | `--mips golden` (= main before this branch) | **`--mips full` (new default)** |
|---|---|---|
| LOD4 diffuse 1024² (574) / default 32² (450) | 11 / 6 mips | 11 / 6 |
| LOD4 normal 1024² (1,024) | 1 mip | **11** |
| LOD8 diffuse 512² (159) / default 32² (97), normal 512² (256) | 1 / 1 / 1 | **10 / 6 / 10** |
| LOD16 diffuse 512² (44) / 32² (20), normal 512² (64) | 1 / 1 / 1 | **10 / 6 / 10** |
| LOD32 diffuse 512² (16), normal 512² (16) | 1 / 1 | **10 / 10** |
| Bytes | 2,022,905,984 | **2,429,401,728** (+406,495,744, +20.1%) |
| `total` time | 19,699 ms | 20,115 ms (+0.4 s) |
| Normal bake loop, `bc7+mips` stage | 3,224 ms | 5,423 ms |
| Top mips (2,720 files) | identical | identical (byte-for-byte) |
| `validate_output` DDS header/payload | 2,720 DDS, 0 errors | 2,720 DDS, 0 errors |

The per-level counts of the golden mode equal the previous full validation output
(`bench\full-validation-20261019`, scanned the same way), so `--mips golden` still reproduces the
old files for the xLODGen parity comparison (`--golden` scores top mips, which do not change). The
diffuse `bc7+mips` stage did not move outside GPU timing noise (2,523 vs 3,038 ms); the extra
2.2 s are the normal maps' box chains plus the BC7 encode of 10 more levels per tile.

### 2. Grass billboards on the `_Alpha` atlas

With `--grass bto` the LOD4 grass shapes referenced the raw per-type billboards the grass stage
renders (`textures\terrain\lodgen\<plugin>\<type>_1.dds`, BC7, 1 mip; 6,078 shapes over 32
textures in the previous full validation), because the grass stage ran after the atlas and its
billboards were never offered to it. DynDOLOD's atlas map puts the `_1`/`_2` views of every grass
type on `DynDOLOD_<World>_Alpha.dds` (keys `<view>.dds,<view>_n.dds`, 62 rows for its 30 types),
so its 4,016 `grasspassthru` shapes sample a 2048² page with 8 coverage-preserving alpha mips and
merge per quad. Now the grass stage runs before `dyndolod-atlas`, its billboard files go into the
VFS, and `dyndolod_build::atlas` adds the views ([`grass_billboard_views`]); LODGen's existing
FlatTextures/atlas remap (`flat_template`, `billboard_nif` with `uv_atlas(force)`) does the rest,
so `objlod` is unchanged. Page caps (2048 default page, 8192 max, 8 mips) and the alpha-coverage
mips are as before.

Full Tamriel build (`build --instance D:\Modlists\skyrim-cs --profile LODForge-E2E --exclude-mod
<the three E2E output mods> --mode dyndolod --preset Medium --texgen --grass bto --grass-source
cache --grass-density 100 --worlds Tamriel`, `RAYON_NUM_THREADS=8`, release, fresh `--out` /
`--work-dir`), `validate_output` and `e2e-compare` against `mods\LODForge E2E - DynDOLOD Output`,
compared with the previous full validation (`bench\full-validation-20261019`, b4870a1):

| Tamriel, default season | before (b4870a1) | **after** | DynDOLOD |
|---|---|---|---|
| LOD4 `grasspassthru` shapes | 6,078 on 32 raw billboards | **4,084**, all on `dyndolod_tamriel_alpha.dds` | 4,016 on `_Alpha` |
| BTO/NIF references to `textures\terrain\lodgen\` | 6,078 (grass only) | **0** | 0 |
| Atlas map rows / `_Alpha` rows / grass view rows | 646 / 34 / 0 | **714 / 98 / 64** | 728 / 113 / 62 |
| `_Alpha` page | 2048² BC7, 8 mips, 5,592,468 B | same | same |
| `DynDOLOD_Tamriel[_n]` | 8192x4096 BC1, 8 mips | same | same |
| `_Alpha` alpha>=128 coverage, mips 0..7 | .352 .352 .352 .352 .351 .349 .348 .332 | .360 .360 .360 .359 .358 .354 .365 .383 | .390 .414 .434 .461 .492 .519 .545 .547 |
| Other LOD4 shape groups (obj, -LargeRef, Snow, HD, passthru, treepassthru) | 561 / 666 / 358 / 213 / 749 / 7 / 498 / 2 / 110 / 116 | identical | |
| Grass material (`lighting:0:80000000:00000021:clamp0:alphafalse:vctrue`) | 6,078 | 4,084 | |
| e2e geometry, objects LOD4 / 8 / 16 common triangles (% of golden) | 5,444,367 / 2,728,472 / 730,470 (97.28 / 94.82 / 88.16) | **identical** | |
| e2e grass LOD4 positions common / within 1 / within 4 units | 1,344,342 / 6,679,761 / 47,264,118 | **identical** | |
| BTOs equal up to order / with different shape sets | 15 / 424 | 15 / **234** | |
| Files / bytes | 8,524 / 11,744,161,889 | 8,524 / 12,148,476,562 (+404 MB: the terrain mips) | |
| Build (`done`) | 159,115 ms | 174,324 ms | |
| `terrain-textures` / `grass` / `dyndolod-atlas` / `objects-atlas` / `objects` stages | 13,988 / 24,185 / 696 / 2,827 / 48,126 ms | 18,939 / 21,742 / 818 / 3,417 / 49,578 ms | |
| `validate_output` | 0 missing, 0 errors | 0 missing, 0 errors (1,711 NIF, 5,243 DDS, 2,383 assets) | |

The grass positions and every non-grass number are unchanged; the shape count moves because the
grass shapes of a quad now share one texture pair and merge. The 68 extra shapes over DynDOLOD's
are the two extra grass types in our map (32 vs 30). The +15 s build time is the terrain mips
(+5 s in that stage here, +0.4 s standalone: GPU timing noise within a full build is large) and
run-to-run variation in the other stages; nothing new runs besides the mip chains and 64 small
tiles. Atlas page coverage per mip is whole-page diagnostics (different occupancy from DynDOLOD),
not a per-tile comparison. Tests: `cargo test --workspace --exclude lodforge-ui` passes (new:
`terrain_textures::tests` mip modes, box chain and DDS header/payload; `dyndolod_build::tests::
grass_views_for_the_atlas`; `flat::tests::grass_billboard_on_the_alpha_page`).

Not done here: the in-game flicker re-test (separate session); grass type coverage (G32: 32 of 46
types, 14 left out by the TexGen rule); xLODGen's "raise steepness" for mipped normals.

## Gap closure (2026-10-19)

Working-tree changes on top of `f1a31e6`, not yet committed. Evidence:
`bench\gap-closure-20261019`. The focused measurements below precede the
[full runtime qualification](validation-runtime-20261019.md); keep their scopes
separate.

### Snow linear copy

The reference `stitched.frag` applies `pbrscale` only with `hasPBR`. The source
material companion matters: `snow01.dds` has RMAOS, but TexGen's rewritten
`snow01noalpha.dds` does not. Applying the same 0.65 scale to both was wrong.
The source no-alpha RGB mean is roughly 213/224/238; reference linear output
preserves it, while our old output was 139/146/155. This was not a source-mip
or upstream TexGen brightness defect.

[Conversion](../apps/cli/src/pbr_linear.rs) now tests the source RMAOS companion.
The existing alpha approximation and source mip choice are unchanged. Missing
or malformed requested source textures now fail the stage with the source path.

- Snow no-alpha top-mip RGB PSNR: **10.197 -> 48.727 dB**.
- Other **46** linear textures: byte-identical to the pinned pre-fix output.
- All 47 match reference dimensions, format and mip count; all RGB PSNR >=30 dB.
- Synthetic opaque/non-PBR and alpha controls pass. GPU fixture invocation:

```powershell
$env:LODFORGE_LINEAR_INPUTS = "D:\Modlists\skyrim-cs\mods\Faultier's PBR Landscapes 2k;D:\Modlists\LODForge\bench\validation-f1a31e6-20261019\Data"
$env:LODFORGE_LINEAR_REFERENCE = 'D:\Modlists\skyrim-cs\mods\LODForge E2E - DynDOLOD Output'
$env:LODFORGE_LINEAR_OUT = 'D:\Modlists\LODForge\bench\gap-closure-20261019\linear-fixed'
cargo test --release --locked -p lodforge --bin lodforge pbr_linear::tests::reference_textures -- --ignored --nocapture
```

Use a new output directory on repeat runs; the fixture refuses existing output.
The pinned Data root supplies generated no-alpha input, not reference linear
output. The writer regenerates every requested linear texture.

### Initial runtime enable state

[Dynamic export](../crates/dyndolod/src/export.rs) previously looked up parents only
in the current exterior world, silently treating absent parents as enabled.
`DefaultDragonEnableMarker` is a disabled interior reference; other chains have
disabled intermediate links. Initial enablement requires **every link's local
condition**, whereas the runtime parent identity is the ultimate ancestor and
OppositeParent combines link inversions with XOR. Testing only the root flag, or
only the immediate flag, caused new regressions and was rejected.

Winning REFR/ACHR states are now read across the load order, with master/light
ID resolution. Deleted winners are removed. Selected missing/cyclic chains
fail explicitly instead of using an enabled default or a 16-link cutoff.

The bounded `dyndolod-export --plugins` fixture uses `LODForge-E2E`, Medium,
Tamriel and the existing reference TexGen input; old DynDOLOD/LODForge outputs
are excluded. It does not claim a fresh terrain/grass build:

- Same **2,958** distinct dynamic originals.
- **26** changed original rows; **32 -> 0** `InitiallyEnabled` differences when
  duplicate golden rows are retained, as in the prior report.
- Other compared fields unchanged: Delay 362, destruction fields 12 each,
  Level/LargeReference 2 each remain unresolved.
- Object export bytes unchanged in the cross-world-lookup control.
- Four parent-chain tests cover interior markers, mixed inversions, disabled
  intermediate links, chains longer than 16, player parents, missing and cyclic
  parents. Existing rolling-ID tests pass.

DLL inspection also confirmed `[Bases]` resolves `(decimal ID, plugin)` and sets
HasDistantLOD. Neither "every emitted base" nor "every large-ref
base" matches the reference selection; that initial-state fix emitted no section.
The test fixtures do not install these diagnostic plugins in MO2. New in-game
toggle/destruction/rolling-update qualification remains required.

### Integrated control

Release CLI generation completed in **17.214 s**, zero failed stages, with:

```powershell
$env:RAYON_NUM_THREADS = '8'
.\target\release\lodforge.exe build `
  --instance 'D:\Modlists\skyrim-cs' --profile LODForge-E2E `
  --exclude-mod 'LODForge E2E - DynDOLOD Output,LODForge E2E - LODForge Output' `
  --mode dyndolod --preset Medium --worlds Tamriel --grass off `
  --no-terrain --no-occlusion --no-underside `
  --out '.\bench\gap-closure-20261019\integrated\Data' `
  --work-dir '.\bench\gap-closure-20261019\integrated\work' --json
```

This uses installed reference TexGen inputs and deliberately excludes terrain,
grass and occlusion; do not compare its timing/geometry to the full 186 s run.
Output validation checked **1,498 files, 686 NIFs, 60 DDS and 309 unique asset
references**, with **zero missing references or structural errors**. Snow
no-alpha RGB PSNR with this TexGen input is 48.506 dB; alpha-threshold agreement
is 100%. The unchanged input warning for `Skyrim - Patch.bsa` remains.

Validation: 15 CLI tests, 18 DynDOLOD tests (including rolling IDs), and the
explicit 47-texture GPU fixture passed. Release build and focused Clippy passed;
Clippy still reports existing advisories, including the repository's
`chunks_exact` usage. Conversion-module rustfmt and changed-document link checks
passed. No new runtime launch or MO2 installation was performed in this follow-up.

### Global base flag updates

[Export selection](../crates/dyndolod/src/export.rs) now collects original bases
used by emitted **large-reference object LOD**, across requested seasons/worlds.
Generated child copies are excluded: their generated bases are separate records.
Empty model columns do not qualify. Dynamic-only selection is deliberately not
included. The [writer](../crates/dyndolod/src/dynplugins.rs) skips already-flagged
bases and emits deduplicated, ordered decimal local IDs with their **owning**
plugin, not the winning override plugin. Full and light-plugin IDs are covered.
Both integrated builds and diagnostic `dyndolod-export --plugins` append this
section to the global worlds file. Collection precedes filtering dynamic worlds,
so static-only worlds' selected bases are not lost.

Configuration: `[DynDOLOD] BaseLODDLL` in the selected `DynDOLOD_SSE.ini` accepts:

| Value | Behavior |
|---|---|
| absent / `auto` | Emit when the profile has a loose DynDOLOD DLL and the actual `SkyrimSE.exe` file version is >=1.6.1130. Missing DLL / older runtime disables it with a diagnostic. |
| `0` | Omit the section; preserves the prior behavior, **not** a legacy plugin-override implementation. |
| `1` | Require those capabilities; fail explicitly if missing/unsupported. |

Invalid settings and unreadable executable metadata fail explicitly. The native
Windows version API is used through a Windows-only `windows-sys` dependency; no
subprocess or version guessed from plugin headers. Non-Windows generation can
use `0` to bypass native detection. The gate assumes the documented **DLL NG**
dependency; DLL presence is not DLL-version certification. The minimum runtime
follows the existing specification and was not exercised against every historic
runtime. This fixture detects **1.7.104.0**.

Measured against the same golden identities:

- **510/593 matching bases, zero extras.** Remaining: 54 Northern Roads,
  17 HearthFires and 12 Skyrim identities. Full selection parity remains open.
- Including all large dynamic references yielded 535 entries, 532 matching and
  **three extras** (`NorTentLarge`, `NorTentLargeSnow`,
  `StockadeBridgeExtraExtraLong01Light_SN`); rejected. A bounds-only test also
  produced extras. Neither heuristic is used.
- Final fresh integrated build: **18.571 s**, zero failed stages. Same command
  and exclusions as the integrated control above, with output/work under
  `bench\gap-closure-20261019\bases-final`.
- **1,497/1,498 output files unchanged** after normalizing the fresh-build token;
  only global Worlds.txt changes. This includes both generated plugins, all
  per-world runtime fields, meshes and textures.
- Output validator: **1,498 files, 686 NIFs, 60 DDS, 309 unique assets**;
  **zero missing assets or structural errors**. The existing missing
  `Skyrim - Patch.bsa` input warning remains.
- A retained-ID rebuild completed in **17.751 s**, zero failed stages;
  **all 1,498 files byte-identical** to the fresh build.
- **38 focused tests passed**, including selection exclusions, duplicate/owner/ESL
  serialization, runtime boundary, explicit-off/auto/required modes, invalid
  settings, missing dependencies and metadata errors. The opt-in GPU texture
  fixture was not rerun for this non-texture change. Release build and focused
  Clippy passed with existing advisory warnings.

Evidence: `bases-final-comparison.json`, `bases-final-hashes.json`,
`bases-final-validation.json`, build/rolling logs and `bases-clippy.log` under
`bench\gap-closure-20261019`. MCP responded successfully; Skyrim/MO2 were stopped
and no live bridge was active. **No new in-game test or installation** was
performed; the preserved validation profile still contains the earlier pinned
output.

## Earlier baseline environment

Host: i7-12700K (20 threads), RTX 4080 16 GB, NVMe. Instance: `D:\Modlists\skyrim-cs` (Cairn skyrim-cs, 86 mods;
Seasonal Landscapes + Northern Roads seasonal swaps, xLODGen Resource SSE Terrain Tamriel).
xLODGen: SSELODGen 4.1.5o EXTREMELY EXPERIMENTAL Terrain LOD Beta x64. Settings: see `scripts/golden/presets/`.
Golden outputs and raw timelines live in `D:\Modlists\LODForge-golden\<preset>-<stamp>\` (not committed).

## Golden runs: Tamriel, terrain + objects + trees (2026-10-17, `scripts/golden/Run-XLodGenGolden.ps1`)

| | no-seasons | seasons (Default+SPR+SUM+AUT+WIN, WIN snow) |
|---|---|---|
| Plugin load | 10.5 s | 10.5 s |
| **Generate wall time** | **9:24** (564 s) | **45:31** (2,731 s) — 4.84× |
| Terrain meshes | 0:24 | 5 × 0:24 = 2:00 |
| Terrain textures (bake + convert) | ~7:57 (≈85%) | ~39:53 (≈88%) |
| Trees + objects (incl. LODGen) | ~0:40 | ~5 × 0:40 |
| xLODGen CPU time | 7,840 s (≈14 of 20 threads busy) | 38,918 s (≈14 threads) |
| Peak working set | 3.6 GB | 4.1 GB |
| Concurrent LODGen.exe | 1 | 1 |
| Output | 6,527 files / 2.8 GB | 32,635 files / 13.9 GB |

Run dirs: `LODForge-golden\no-seasons-20261017-031139`, `LODForge-golden\seasons-20261017-032143`.
Caveat: no TexGen billboards are installed, so tree LOD is mostly empty (`LOD not found Textures\Terrain\LODGen\...`).
Tree-LOD parity needs a run with billboards later.

### How much of each season actually differs from Default (`scripts/golden/Compare-Seasons.ps1`)

| Season | Terrain .dds byte-identical to Default | .bto identical | .btr identical |
|---|---|---|---|
| SPR | 1,665 / 2,041 (81.6%) | 14.1% | 0% |
| SUM | 1,644 / 2,041 (80.5%) | 14.2% | 0% |
| AUT | 1,878 / 2,041 (92.0%) | 13.8% | 0% |
| WIN | 1,577 / 2,041 (77.3%) | 11.4% | 0% |

- **Terrain textures:** only 8–23% of the season textures differ from Default, yet xLODGen re-bakes all of them.
  Textures are ~88% of seasonal build time, so baking only the changed quads should cut seasonal texture work
  from ~4 × 8 min to roughly 4 × 1–2 min before any GPU work.
- **Terrain meshes (.btr):** never byte-identical, but the season file differs at least in the embedded texture
  paths (`Tamriel.4.0.0.SPR.dds` / `.SPR_n.dds`). Geometry looks unchanged (seasons only swap LTEX); still to be verified
  with a NIF-level diff. If it holds, season BTRs are a path rewrite of the Default mesh rather than a rebuild.
- **Object LOD (.bto):** mostly different; needs a NIF-level diff to tell real swaps from atlas/texture-path changes.

## LODForge P1 readers (2026-10-17, release build, warm file cache)

`lodforge inspect --instance D:\Modlists\skyrim-cs` loads 98 plugins (417 MB) and 100 BSAs, indexes 28,206 loose
and 184,468 archived files, and scans every worldspace's CELL/LAND/REFR records (load order resolved,
overrides and deletions applied) in **~210 ms** (5 runs: 206–325 ms). Breakdown: VFS 18 ms, plugin mmap 3 ms,
archive indexes 50 ms, worldspace scan ~140 ms. xLODGen's comparable stage (plugin load until the LOD dialog) is
10.5 s, but it loads every record type, not just worldspaces, so this is not a like-for-like number.
Cold-cache timing is not measured yet.

Cross-checks:
- The 15 worldspaces we flag as having `lodsettings\<EditorID>.lod` exactly match xLODGen's worldspace list, with the
  same load-order form IDs (incl. `0B0E1592` and light-plugin `FE03EA5F`).
- Tamriel: 271,135 winning REFRs vs xLODGen's last "Processed Records" sample of 271,238 (close, not reconciled).
- `lodforge bsa-scan`: 60,335 archived NIF/DDS (25 GB uncompressed) extracted and parsed in **5.7 s**; failures are
  only 28 Skyrim LE-format NIFs (BS 83), which P1 does not decode.
- `lodforge nif-scan`: all 2,443 golden BTR/BTO and 11,692 SSE NIFs from the modlist parse; 195 LE-format NIFs are
  rejected, including DynDOLOD Resources LOD meshes, so P3 needs `NiTriShape`/`NiTriShapeData` support.
- `lodforge dds-scan`: all 4,084 golden DDS (BC1/BC3/BC7, up to 4096²) and DynDOLOD Resources textures decode.

Format lessons:
- Some vanilla BSTriShapes carry stale per-attribute offset nibbles in the vertex desc (e.g. `dlc2horker01.nif`:
  a UV slot is counted though the UV flag is clear). Offsets must be derived from the attribute flags.

## LODForge P2: terrain LOD textures, diffuse + normal maps (2026-10-17, RTX 4080 via Vulkan, release build)

`lodforge terrain-textures --instance D:\Modlists\skyrim-cs [--season SPR --delta-from <default build>]`
bakes every Tamriel terrain LOD texture (diffuse and `_n` normal maps, LOD 4/8/16/32) on the GPU (wgpu
compute) and BC7-encodes it on the GPU (mode 6, the "BC7 Quick" tier xLODGen's texconv uses).

The bake follows the reverse-engineered xLODGen pipeline (`docs/re/xlodgen-terrain-textures.md`):
each cell rendered once at 384 px (landscape textures Lanczos-shrunk to 16 px, tiled 24x per cell),
opacity splatted with the square/diamond brushes, `sum(w·c) + base·(1 − sum w)` blending; diffuse adds
Lanczos vertex colours; normals bake the blended landscape `_n` maps onto the VNML terrain normal
(model space, R = x, G = z, B = y). Each cell is then Lanczos-downscaled into every LOD level; diffuse
gets the tone curve; BC7. CPU reference: `lodforge_terrain::spec` (`terrain-bake [--normal]`); GPU:
`terrain_spec.wgsl` (`terrain-textures`).

### Timing: terrain textures, Tamriel, all levels, diffuse + normals

Wall clock including process start, profile load, LAND decode, texture prep and GPU init; xLODGen
numbers are its "Building terrain LOD textures" stage from the golden runs (its whole runs, which also
build meshes, objects and trees, took 9:24 and 45:31).

| Build | LODForge | xLODGen texture stage | Speed-up |
|---|---|---|---|
| Default season (2,040 + 2,040 files) | **17.5 s** | 7:57 (477 s) | ~27x |
| Default + 4 seasons, season deltas | **73.8 s** | 39:53 (2,393 s) | ~32x |
| Default + 4 seasons, full rebuilds | ~88.7 s (17.5 + 4 x 17.8) | 39:53 | ~27x |

Per season delta: SPR 13.8 s, SUM 13.4 s, AUT 12.0 s, WIN 17.1 s. Stage split of the default build:
diffuse bake loop 5.6 s (GPU render + downscale 2.4, BC7 + mips 2.1, packing 0.4, writes 0.3);
normal bake loop 8.7 s (GPU 3.9, BC7 2.4, packing 1.7, writes 0.4); ~3 s outside the loops.
Deltas still spend ~1.5–2.7 s per kind hard-linking unchanged tiles and render whole LOD32 tiles that
contain a changed tile; both are the obvious next optimisations. One timing run during the session was
3–10x slower in the GPU stages only, with uneven per-season numbers; a re-run with an idle GPU gave the
numbers above (likely contention from another GPU client).

### Fidelity vs golden (`--golden`; our BC7 vs xLODGen's BC7, top-mip RGB PSNR per tile)

| Level | Diffuse mean / min | Normal mean / min |
|---|---|---|
| LOD4 (1,536) | 47.5 / 42.7 dB | 30.7 / 24.2 dB |
| LOD8 (384) | 47.6 / 43.9 dB | 38.9 / 32.1 dB |
| LOD16 (96) | 50.6 / 43.6 dB | 37.7 / 31.7 dB |
| LOD32 (24) | 46.2 / 43.6 dB | 35.0 / 29.5 dB |

The full SPR build vs golden SPR scores the same for both kinds. Tile coverage, sizes and the diffuse
full/default split (582/954, 160/224, 44/52, 16/8) match golden exactly; diffuse default tiles are
bit-identical to golden. Normal tiles are limited by BC7 itself: our own normal output survives a BC7
round trip at only ~25–34 dB (the maps carry dense texel-level detail), so per-pixel BC7-vs-BC7 PSNR
is near that floor; low-pass (4x box) agreement is 36–44 dB. GPU output equals the CPU reference pushed
through the same BC7 encoder, and builds are byte-identical run to run (diffuse checked over four full
builds, normals over two).

### Facts established against golden beyond the RE spec

- Tiles: every LOD32 tile containing LAND, fully subdivided for LOD16/8/4 (lodsettings\Tamriel.lod:
  origin -96,-96, stride 256, levels 4..32).
- Duplicate LAND: Dawnguard.esm adds second LAND records at Tamriel -44,37 and -43,36; xLODGen uses the
  earliest plugin's (Skyrim.esm).
- 8-bit stores: in the diffuse pipeline the Lanczos *downscales* round (texture shrink, cell downscale,
  default-size shrink); every other store truncates (blends, vertex colours, masks, upscales, tone).
  In the normal pipeline every store truncates, downscales included (inferred cause: format-dependent
  resampling — the normal path is R8G8B8). Measured: diffuse 41.7 -> 45.0 dB per tile, default tiles
  bit-exact; normals low-pass 36 -> 41 dB.
- Normals: the terrain normal is the interior 64x64 of the 66 px Lanczos stretch (crop offset 1; 0 and
  2 are ~5 dB worse); the bake runs at the 384 px cell size, then the cell is downscaled (baking at the
  output size is 2–8 dB worse); VHGT-derived normals score worse than VNML.
- Golden had `TerrainDefaultNormalSize=None`, so normal tiles are never shrunk.

Season delta planner (`lodforge season-delta`): LTEX swaps filtered to those that change the texture file;
**0 missed tiles** across all seasons/levels/kinds vs golden, 36 extra out of ~2,700 predicted.

## LODForge P2: terrain LOD meshes (.btr) (2026-10-17, release build, 20 threads)

> Method text of this section was moved out of the repository on 2026-10-20 (it described the LODGen-derived implementation that has since been replaced; see docs/licensing.md). The measurement tables below are kept as the historical record; the current implementation and its numbers are in docs/re/terrain-lod-spec.md, docs/re/object-lod-spec.md and docs/re/occlusion-spec.md.

| | LODForge | LODGen / xLODGen |
|---|---|---|
| Default season, all 1,760 BTRs | **2.2 s** (build 1.7 s, write 0.2 s) | 24.3 s (LODGenx64.exe standalone) |
| All 5 seasons (8,800 BTRs) | **2.9 s** | ~2:00 (5 x ~24 s, one full rebuild per season) |

### Terrain LOD overall (meshes + textures, diffuse + normal), Tamriel

| | LODForge | xLODGen |
|---|---|---|
| Default season | ~20 s (2.2 + 17.5) | 8:21 (0:24 meshes + 7:57 textures) |
| Default + 4 seasons | ~77 s (2.9 + 73.8) | ~41:53 (~2:00 + 39:53) |

## LODForge P3: object LOD meshes (.bto) (2026-10-17, release build, 20 threads)

> Method text of this section was moved out of the repository on 2026-10-20 (it described the LODGen-derived implementation that has since been replaced; see docs/licensing.md). The measurement tables below are kept as the historical record; the current implementation and its numbers are in docs/re/terrain-lod-spec.md, docs/re/object-lod-spec.md and docs/re/occlusion-spec.md.

| | LODForge | LODGen / xLODGen |
|---|---|---|
| Default season, 683 BTOs | **0.44 s** build, 0.02 s serialize | 19 s (LODGenx64.exe; per-season runs took 18-20 s) |
| All 5 seasons, 3,415 BTOs (incl. profile load, file writes) | **2.8 s** wall | ~97 s LODGen + ~20 s/season xLODGen record scan, atlas, tree blocks (object/tree stage 3:08) |

#### Parity drop to 71 / 683 (2026-10-17): terrain input, not code or master cleaning

| Profile / terrain | equal up to order | triangles identical |
|---|---|---|
| LODForge-DynDOLOD (cleaned masters), VFS terrain = vanilla BSA | 71 / 683 | 97.709 % |
| LODForge-DynDOLOD, `--terrain` = golden BTRs | **528 / 683** | 99.732 % |
| Default (cleaned masters, `LODForge Output` enabled since 18:52), VFS terrain | **528 / 683** | 99.732 % |
| Default, `--from-load-order` (export from the cleaned masters), golden BTRs | **528 / 683** | 99.732 % |

### Object LOD export + atlas from the load order (2026-10-17)

| | LODForge | xLODGen |
|---|---|---|
| Export + atlas layout, 5 seasons | **1.1 s** | ~12 s per season (record scan + atlas) |
| + atlas textures (20 pages, BC7) | **5.6 s** | (inside the 12 s) |
| Full object LOD, 5 seasons (export, atlas textures, 3,415 BTOs) | **8.8 s** | 3:08 object/tree stage |

## LODForge P3: tree LOD (.lst, .btt, tree atlas) (2026-10-17)

`lodforge tree-lod --instance <MO2> [--seasons ...] [--golden <xLODGen Data>]`: `lodforge-treelod`, a port
of the tree branch of xEdit's `wbGenerateLODTES5` (`TwbLodTES5TreeList` / `TwbLodTES5TreeBlock`, MPL).
The golden install has no tree billboards, so validation used a test mod: `tree-lod
--placeholder-billboards <dir>` writes an uncompressed DDS per tree (five sizes, every fifth image
a CRC duplicate, every third with a `.txt` setting Width/Height/ShiftZ/Scale), and xLODGen ran
trees-only on it (`Run-XLodGenGolden.ps1 -Preset trees-only`, 28 s including load).

| | LODForge | xLODGen |
|---|---|---|
| Tamriel tree LOD (131 trees, 114,027 refs, 388 blocks, 4096² atlas) | **1.2 s** | 28 s (incl. ~18 s plugin load) |

Fidelity: LST byte-identical (131 types: sizes, UVs); all 388 BTTs identical (types, form ids,
positions, scales; rotation ignored because xEdit draws it with `Random`); atlas 46 dB PSNR (ours
BC7, xLODGen BC3), same layout.

Beta rules found (not in the public xEdit source):
- Atlas: 16-px padding (public: 2); blocks sorted by height, then width; the right free node keeps
  the padding row (`h + pad`).
- The fallen-tree check (> 30 degrees around x or y) runs before the tree joins the list, so
  fallen-only references don't take a tree index. Degrees as xEdit reads them (4-decimal
  rounding): USSEP's 330.0 / 30.0 degree rotations are kept.
- Positions and XSCL go through `RoundToEx(v, -6)` with a single-precision factor (0.79999995 ->
  0.8, 0.97000003 -> 0.96999997), then single-precision maths.
- References defined in light plugins (`FExxxyyy`) are left out (all 14 from four CC .esl files).

## Full build: everything xLODGen does, one command (2026-10-17)

`lodforge build --instance D:\Modlists\skyrim-cs --seasons SPR,SUM,AUT,WIN --out <Data>` (or
`--mo2-mod <name> [--enable]`): terrain meshes + textures, object LOD and tree LOD for all 15
worldspaces with a lodsettings file, default + 4 seasons.

| | LODForge | xLODGen |
|---|---|---|
| 15 worldspaces × 5 seasons | **437 s** (0 failed stages) | Tamriel alone with seasons: 45:31; the other 11 terrain-only, default season: 7:02 |

Output: 30,870 BTR, 5,350 BTO, 95,360 DDS (46 GB; each season writes its own full texture set, as
xLODGen does). No BTT/LST: the profile has no tree billboards. Most of the time is terrain
textures for the big worlds and seasons.

## Data point A: terrain only, 11 non-Tamriel worldspaces (user run, 2026-10-17 02:51)

Total **7:02**. Meshes ≈ 12 s summed; terrain **textures ≈ 6:50 (~97%)**.
Texture time scales with worldspace area, not complexity: Sovngarde 1:39, DLC01SoulCairn 2:17, small worlds 7–35 s.
Plugin load ≈ 10 s for this list.

| Worldspace | Meshes | Textures |
|---|---|---|
| Blackreach | 0:00 | 0:19 |
| SkuldafnWorld | 0:01 | 0:19 |
| DeepwoodRedoubtWorld | 0:00 | 0:17 |
| Sovngarde | 0:07 | 1:39 |
| JaphetsFollyWorld | 0:00 | 0:07 |
| DLC01FalmerValley | 0:01 | 0:25 |
| DLC01SoulCairn | 0:03 | 2:17 |
| DLC2ApocryphaWorld | 0:00 | 0:17 |
| ccBGSSSE067DeadlandsWorld | 0:00 | 0:19 |
| ccKRTSSE001QNWorld | 0:00 | 0:35 |

Takeaway: terrain texture baking (render LTEX blend + BC7 encode + mips) is the first GPU target.

## P6: DynDOLOD + TexGen golden runs (2026-10-17)

> Method text of this section was moved out of the repository on 2026-10-20 (it described the LODGen-derived implementation that has since been replaced; see docs/licensing.md). The measurement tables below are kept as the historical record; the current implementation and its numbers are in docs/re/terrain-lod-spec.md, docs/re/object-lod-spec.md and docs/re/occlusion-spec.md.

| Run | Settings | Load | Generate | Wall | CPU | Peak RAM | Output |
|---|---|---|---|---|---|---|---|
| TexGen | defaults: stitched + rendered object LOD textures (base 256), tree billboards (UPP 11.0, 32–1024), BC7 Quick | ~33 s | 2:04 | 2:06 | 112 s | 3.2 GB | 1,522 files, 139 MB (364 billboards, ~600 `textures\DynDOLOD\LOD`, ~130 `textures\dlcXX\lod`) |
| DynDOLOD | Advanced, **Medium**, Tamriel only, seasons Default+WIN/SPR/SUM/AUT, object + tree + dynamic LOD, occlusion (quality 2, Occlusion.esp), terrain underside | ~30 s | 6:59 | 9:06 incl. save | 873 s | 6.9 GB | 7,183 files, 2.1 GB: 3,425 BTO, 3,720 BTT, 3 plugins |
| Stage | At |
|---|---|
| patches | 0:19 |
| 19,425 base records (3,467 object LOD + 812 dynamic LOD models) | 1:00–2:10 |
| Tamriel references (271,268) | 2:18–3:09 |
| object LOD export + mini atlases | 3:10–4:21 |
| LODGen × 5 seasons (concurrent, ~31 s each; default season 18:18:50–18:19:22) | 4:21–~4:55 |
| object atlas | 4:24 |
| tree LOD (4 season atlases) | 4:34–4:45 |
| dynamic LOD | 4:45–4:51 |
| terrain underside | 4:51 |
| large references | 4:55 |
| occlusion: height map, then 5,697 Tamriel + child-world cells | 4:56–6:56 |

### LODForge vs the golden run: object LOD from DynDOLOD's own exports

| | Value |
|---|---|
| DynDOLOD's LODGen | ~31 s per season process, 5 processes concurrently |
| Files | 685/685 per season, same names (with `LimitCoords` honoured) |
| Triangles identical in every attribute | **40.5%** default, 40.5–41.1% per season (net48-style numerics: 9.1%; .NET 10 numerics: 36.3%; + AltTextures: 39.7%; + Skyrim LE meshes: 40.5%) |
| Triangles identical within tolerance (positions 0.1 u, bitangent x as byte) | **71.0%** default, 70.5–71.7% per season |
| Per attribute (all vertices, 5 seasons) | position 90.3%, normal bytes 96.4%, tangent 96.1%, UV 96.4%, colour 93.1%, bitangent x (float) 62.7% |
| Files with different shape sets | 169 default (234–239 for SPR/SUM/AUT) |
| Triangles built vs golden (default season) | 9.48 M vs 9.67 M |

## P7: closing the DynDOLOD gaps (2026-10-17, branch `p7-dyndolod-gaps`)

> Method text of this section was moved out of the repository on 2026-10-20 (it described the LODGen-derived implementation that has since been replaced; see docs/licensing.md). The measurement tables below are kept as the historical record; the current implementation and its numbers are in docs/re/terrain-lod-spec.md, docs/re/object-lod-spec.md and docs/re/occlusion-spec.md.


### Step 1: LODGen 3.1.8 side (`object-lod --flavor dyndolod`, DynDOLOD's own exports)

| Change | Tolerant | Exact | Files with different shape sets |
|---|---|---|---|
| P6 end | 70.96% | 40.5% | 169 |
| FlatTextures internal billboards (393 `.dds` LOD models) | 70.98% | 40.5% | 148 |
| MergeVertexColors (ReUV + RemoveUnseenFaces tail) | 87.47% | 49.5% | 148 |
| RemoveUnseenFaces heightmap stage (`TerrainData=` .bin) + `RemoveUnseenZShift` | 90.79% | 51.3% | 134 |
| tolerant comparison ignores "HD" in shape names (see rule 3) | 97.08% | — | 75 |
| passthru BSEffectShaderProperty written, no shader for other passthru types | **98.44%** | **52.1%** | **2** |

### Step 2: the DynDOLOD object LOD export, written by LODForge (`lodforge dyndolod-export`)

| Part | Parity |
|---|---|
| Merged rule table vs the preset's `[Skyrim LODGen]` | 158/158 lines |
| Reference lines (default season) | 31,425/34,590 byte-identical (90.9%); seasons 90.6–91.0% |
| Header | 153/155 lines (missing: `LimitCoords4/8`) |
| `LODCoords` (season quad lists) | WIN exact; SPR 143/208 LOD4 quads, 57/73 LOD8, 21/22 LOD16 (sets; order not reproduced) |
| `LODGen_SSE_FlatTextures.txt` | 116/123 lines |
| AltTextures, VC | written since branch `dyndolod-side-files` (see "AltTextures and VC side files") |
| Atlas map + `.lst`, mini atlases, TexturesUsed | not written yet |

### Step 3: TexGen tree billboards on the GPU (`lodforge texgen-billboards`), first version

| | Value |
|---|---|
| Billboards rendered | 64 of the 127 golden ones (the rest have no static geometry we read yet) |
| Same pixel size as the golden one | 30/64; mean width ratio 0.98, height ratio 0.97, ShiftZ off by 8.4 units |
| Mean PSNR on opaque texels (same-size ones) | 20.95 dB; binary-alpha agreement 83.8% |
| Time | ~0.28 s per billboard (one GPU round trip per pass, not batched) |

#### Grass golden (2026-10-18)

| Stage | Rule | Tamriel |
|---|---|---|
| Grass types | Only GRAS records with a TexGen billboard go into `LODGen_SSE_Grass_Map_<World>.txt`. TexGen lists the others with `,False` in `TexGen_SSE_Grass_Billboards.txt`: ground covers (`Ground_*`), underwater (kelp, coral, `RockGrassWater01`), `RockGrass01`/`RockGrassSnowObj01`, `Flower_Daisy` | 32 of 46 cache types; 15.94 M of 30.03 M placements (53.1%) |
| Density | `GrassDensity=<percent>` in the export. The UI default is **100%**, which means no thinning. Below 100, each grass instance loses random placements until `count * density` remain (time-seeded `System.Random`, not reproducible) | 100% |
| Level | Grass goes into LOD4 quads only | 499 BTOs |
| Geometry | `ComplexGrassBillboard=4` uses the FlatTextures NIF template `DynDOLOD_flat_4x2alt1_lod.nif` (absolute path), i.e. 4 planes and 8 triangles per placement. Normal grass would use `GrassBillboard=1` = Internal, 2 planes | 127.0 M grass triangles |
| Change | Tolerant | Files with different shape sets | Triangles ours / golden |
|---|---|---|---|
| as of step 4 (absolute template path not read) | 4.25% | 346 | 5.70 M / 132.73 M |
| CLI mesh reader reads absolute paths | 4.25% | 346 | 132.72 M / 132.73 M |

### Step 5: DynDOLOD.esm/.esp and the DLL NG data files, first version

| | Ours | Golden |
|---|---|---|
| Dynamic LOD references (`[Objects]`) | 4,893 | 1,681 (1,671 originals; 1,646 of them are in ours) |
| DynDOLOD.esp records | KYWD 1, STAT 415, REFR 4,894, WRLD 1, CELL 1 | REFR 2,121, CELL 32, STAT 8, WRLD 8, QUST 2, FLST 1, ... |
| DynDOLOD.esm records | KYWD 1 | 5,934 (large-reference workarounds) |

### Step 6: occlusion (TVDT), exact

| Stage | Result vs golden |
|---|---|
| Terrain input from the load order (LAND cells, water) | 11,186/11,186 records identical to DynDOLOD's `LODGen_SSE_LODGen_Terrain_Tamriel.bin` |
| Height map, cell level (water/min/max/top) | 5,697/5,697 cells equal the values in DynDOLOD's log |
| Cell selection (`-Flat +Border`; children `-Flat`) | same cells in all 8 worldspaces (5,697 + 588) |
| TVDT | **6,285/6,285 identical** (bit Jaccard 1.0000), all 8 worldspaces |
| Computed MHDT (279 DynDOLOD.esm child-world cells) | 279/279 identical |
| Occlusion.esp records (decompressed, form ids by master name) | 6,285/6,293 identical; the 8 WRLD differ only in OFST values |
| Header, masters, group and record order | identical (12 masters incl. DynDOLOD.esm, HEDR 1.71/6500/0x800, ESL, INCC) |

### Step 7: object LOD height map (LODGen 3.1.8 `CreateHeightMap`)

| | Value |
|---|---|
| Cells bit-identical | 8,349/14,125 (59.1%) |
| Values bit-identical | 14,741,331/15,382,125 (**95.83%**); within 1 unit 15,381,340 (99.995%) |
| Values off by more than 1 unit | 785 in 136 cells (66 MinValue only in ours, 51 only in golden) |
| `lodforge occlusion` with our height map | **6,283/6,285 TVDT cells identical**, bit Jaccard 1.0000 (13 of 4.47 M set bits differ, all in Tamriel) |

## P8: TexGen billboard parity (2026-10-18, branch `p8-texgen-parity`)

Command for every step: `lodforge texgen-billboards --instance D:/Modlists/skyrim-cs --profile
LODForge-DynDOLOD --golden D:/Modlists/LODForge-golden/texgen-20261017/output`. The golden run
has 126 tree billboards (the P7 count of 127 was one too many).

### Step 1: geometry

What TexGen draws, read from the golden debug log (`TwbModel.LoadFromFile.IterateNodes` and the
`LoadScene` "Vertexdata" lines per shape):
- It does **not** draw skinned shapes and does **not** follow NiSwitchNode's active index. Vanilla
  SSE trees keep two versions under `NiSwitchNode`: child 0 is the skinned full tree
  (BSTriShapes with their vertices in NiSkinPartition), child 1 holds the low-detail `<Tree>_1:n`
  shapes. TexGen draws every non-skinned shape below every switch child.
- Those low-detail shapes are often **NiTriShape** or **BSLODTriShape** blocks with
  NiTriShapeData, inside SSE (BS 100) files. We skipped them (only LE files were read that way),
  which is why 50 golden trees had no geometry. Some trees have them next to `_1:n` BSTriShapes
  with the same vertices; TexGen draws both.
- Skinned trees without a usable static version (the `_ResourcePack` palms, willows, cypresses,
  snow gums) are rendered from `tools\DynDOLOD\Edit Scripts\DynDOLOD\Render\Skyrim\Replace\
  <path>_static.nif`, and the `.txt` `Model=` names that file (`DynDOLOD\Render\...`).

Changes:
- `crates/nif`: `parse_all_geometry` (also decodes NiTriShape / NiTriStrips / BSLODTriShape in SSE
  files; `parse` is unchanged for the LODGen ports), `Nif::skin_geometry` (NiSkinPartition
  vertex buffer + triangles over all partitions) and `Nif::switch_index`.
- `crates/texgen`: `LoadOptions` (skinned shapes, active switch child; both off = TexGen),
  `replacement_model`.
- CLI: the model comes from the base record (not the golden `.txt`), then the replacement check;
  `--dyndolod` (default `<instance>\tools\DynDOLOD`).

| Geometry | Rendered | Golden size | PSNR (opaque) | Alpha agreement | W / H ratio | ShiftZ diff |
|---|---|---|---|---|---|---|
| P7 (static BSTriShapes only) | 64/126 | 30 | 20.95 dB | 83.8% | 0.980 / 0.973 | 8.4 |
| + skinned shapes (bind pose) | 114 | 43 | 19.62 dB | 82.5% | 1.010 / 1.027 | -88.2 |
| active switch child only | 34 | 8 | 20.91 dB | 87.0% | 0.973 / 0.954 | 15.7 |
| + SSE NiTriShape / BSLODTriShape (TexGen's rule) | 114 | 55 | 19.72 dB | 82.7% | 0.994 / 0.982 | 4.7 |
| + `_static` replacements | **126/126** | 62 | 19.89 dB | 83.5% | 0.995 / 0.982 | 4.2 |

PSNR and alpha agreement only count billboards at the golden size, so the set changes between
rows; size, framing and shading are steps 2 and 3.

### Step 2: size and framing

Fitted against the 89 billboards whose size lines the debug log has (`Scale`, `Width/Height`
p0, pre-pass `GetImageFromRenderFB w x h`, final `w x h -> w x h`) and the 126 golden `.txt`:
- `p0 = round(modelHeight * scale / UPP)`, where `modelHeight` is the z extent of the rendered
  geometry (the shapes of step 1). A single additive constant c in `floor((H*s + c) / UPP)`
  fits all 89 rows only for c in [5.38, 5.59), i.e. UPP / 2: rounding. P7's `floor` was one size
  class off near powers of two.
- The pre-pass is framed on the geometry's x range (not symmetric about the origin) at
  `ceil(h * xExtent / H)` x `h` pixels (89/89 within rounding; the snow gum is 409 px wide where a
  symmetric frame would be 505).
- Final: `h` rows spanning the trimmed z range, `4 * ceil(h * 2 * max(|xl|, |xr|) / height / 4)`
  columns, centred on the origin (as P7).
- TexGen ignores vertex alpha. Trees keep wind weights in it (0 on whole Soul Cairn trunks even
  with the Vertex_Alpha shader flag); with it our Soul Cairn trees were fragments.
- Texture sampling: the twig density of the golden billboards lies between mip-0 alpha (P7) and
  plain trilinear mips. A mip LOD bias of about -1 matches it, and so does **anisotropic
  filtering without bias**, which gives mean width and height ratios of 0.999 / 1.000; we use
  16x anisotropy.
- `unmulta` (shader file, behaviour only): a texel is opaque when fully covered, or when its
  coverage is >= 128/255 and its colour is not black; the trim uses the same test.
- Back faces of shapes without Double_Sided are culled (silhouette unchanged, +0.3 dB).
- Textures are uploaded block-compressed (TexGen's `LoadGLCompressed`), so the GPU decodes BC
  like TexGen's GL upload: same quality, 4x faster than our CPU decode (2.4 s vs 11 s for 126).
- The adapter (wgpu on Vulkan and DX12) allows at most 8x MSAA. 16 samples emulated as 8x with
  2x horizontal supersampling (gradients scaled to keep the texture LOD) was worse (alpha
  agreement 92.6% vs 94.1%), so we stay at 8x.

Tried and rejected (golden-size count / alpha agreement): writing the material alpha to the
coverage target (32 / 90.3%), trim thresholds 1..160 (68..78 / ≤94.2%), square-pixel pre-pass
frames anchored left, centred or right (58..73), sub-pixel shifts of the final view (best at 0,0:
±0.5 px costs 1.4..2.0 points of alpha agreement), nearest-mip filtering (62).

| | Golden size | Within 4 px | ShiftZ within 1 unit | PSNR | Alpha agreement | W / H ratio | Time |
|---|---|---|---|---|---|---|---|
| Step 1 | 62 | | | 19.89 dB | 83.5% | 0.995 / 0.982 | 10.3 s |
| + TexGen framing (round, x-range pre-pass), 8x MSAA | 56 | | | 19.86 dB | 83.8% | 1.005 / 0.983 | |
| + vertex alpha ignored | 63 | | | 20.23 dB | 91.1% | 1.027 / 1.007 | |
| + trilinear mips (not mip-0 alpha) | 64 | | | 20.31 dB | 91.3% | 0.995 / 0.997 | |
| + 16x anisotropic | 75 | | | 20.81 dB | 94.1% | 0.999 / 1.000 | |
| + back-face culling, BC upload, unmulta rgb test | **74/126** | **111** | **111** | 21.07 dB | 94.1% | 0.999 / 1.000 | 2.4 s |

The size target (>= 90% at the golden pixel size) is not met. The residual is texel-level
coverage noise, not a framing rule:
- misses are symmetric (+4 px: 17, -4 px: 19, ±8: 12, more: 3);
- the alpha difference images show isolated leaf texels on both sides, with no one-sided edges;
- the size depends on the single most extreme silhouette pixel, so a one-pixel trim difference
  flips the 4-px quantisation about half the time.
TexGen renders with 16x MSAA on GL (its sample pattern and alpha-test path we cannot reproduce
with wgpu's 8x), so exact sizes need the same rasterisation. 111/126 (88%) are within one 4-px
step, and ShiftZ is within 1 unit for 111/126; the misses are bottom trims one pre-pass row
apart at thin roots.

`Depth` matches for 35/126 only. It is quantised to 4 px like `Width`, but TexGen gives
different `Depth` values for the same model at the same pixel height
(`dlc1treepineshortheavysnow` 172 px vs 176 px for its three copies), so it is not a pure
function of the model. Open.

### Step 3: shading

The `default` fragment shader's behaviour (read for behaviour only, spec §2.2), now in
`billboard.wgsl`:
- per-pixel normals from the normal map in the vertex tangent frame. The frame is x = the
  NIF's stored bitangent, y = its stored tangent, z = the normal (the other three assignments
  score 1.7..3.7 dB lower); vectors normalised per vertex, not per pixel; no flip on back faces;
- `rgb = albedo * vertexColour * (A + D * max(N·L, 0) + emissive)`, light from the top
  (A 0.55, D 1.35); other light directions score 3.7..8.8 dB lower;
- Blinn-Phong specular for Specular-flagged non-PBR shapes (normal-map alpha mask), Own_Emit
  emissive, soft and rim light with the texture-set slot 2 mask (removing soft light costs
  2.4 dB: vanilla pine branches have Soft_Lighting with the diffuse itself in slot 2),
  `BillboardPBRScale` 0.7 for `textures\pbr\`;
- colour bleed: each transparent texel takes the mean of the opaque texels within
  `BleedRadius` (2) in a 5x5 square, else the image's average opaque colour.

The comparison now reports PSNR over golden-opaque texels (P7's measure; alpha mismatches count
with our bleed colour), over texels opaque in both (shading only), the same two after our BC7
encode (the golden is Texconv BC7 Quick), and a diagnostic on 4x4 block means.

| | Golden-opaque | Opaque in both | BC7 (both measures) | 4x4 blocks | Alpha agreement |
|---|---|---|---|---|---|
| Step 2 (vertex normals) | 21.07 dB | | | | 94.1% |
| + normal maps, tangent frame, material terms | 24.86 dB | 25.53 dB | 24.22 / 24.79 dB | 30.38 dB | 94.1% |
| + radius-2 bleed | **25.25 dB** | **25.53 dB** | 24.57 / 24.76 dB | **30.38 dB** | 94.1% |

The targets (>= 30 dB opaque, >= 97% alpha agreement) are not met:
- Low-frequency shading matches: 4x4 block means agree at 30.4 dB and the mean opaque colour
  over all compared billboards is (66.2, 65.6, 50.6) golden vs (65.9, 65.7, 51.5) ours. The
  per-texel gap is foliage noise (which needle or leaf texel wins a pixel), the same
  rasterisation difference as the alpha speckle of step 2.
- Six snow pines (`treepineforestsnow02..04`, `deadsnow01/03/05`) score 12..14 dB, against 21..29
  for every other tree. Their texture detail does not line up even unlit (albedo only: 13.8 dB),
  although the texture file is the one TexGen logged (CRC 82865A59), its mips look normal, and
  mip bias, texture resolution, the alpha threshold and the normal map make no difference. The
  `SnowL` variants with the same geometry score 22..23 dB. Open. Without these six the mean
  is ~1 dB higher.
- Our BC7 encoder (mode 6 only) costs 0.7 dB against the golden's BC7.

Tried and rejected: vertex colours without the Vertex_Colors flag (no change), colour LOD bias
0..2 and separate alpha bias, alpha-test reference scaled 0.25..0.75 (worse: 21.7..24.4 dB),
dropping the top mips of 4096 textures (no change).

### Step 4: outputs (`_n`, rendered two-view billboards, HD grass, full `.txt`)

New: `make_rendered_billboard` (front and side view, each trimmed tightly, `_1/_2` + `_n`),
the six-light normal bake pass, `--normals` (TexGen `TreeNormalMaps`), `--trunks` (every NIF in
`tools\DynDOLOD\Edit Scripts\DynDOLOD\Render\Skyrim\Billboards`, against the golden
`textures\DynDOLOD\LOD\trees`), `--hd-grass` (the grass types of `texgen-grass-20261018`), and full
sidecars (`CRC32` of our final DDS, `ModelCRC32`, `Texture_k` / `TextureCRC32_k`; rendered ones
with `Width_n`, `Height_n`, `Adjust_n`, `CRC32_n` and HD grass with `Complex=`).

Rules found against the goldens:
- Rendered billboard sizes: `w_n = 4 * ceil(h * Width_n / Height_n / 4)`, the final image framed
  exactly on the silhouette (non-square texels), `Width = w_1 * Height_1 / h`; `Height`/`ShiftZ`
  from the front view; `Depth = 2 * max(|left|, |right|)` of the side silhouette (exact on 3 of 12
  `_ResourcePack` trunks, about one pre-pass texel off elsewhere); `RenderModelHeightMultiplier`
  1.85 as the scale.
- **Trunks are rendered from the back** (180° / 270°): from 0° / 90° the images are mirrored and
  single-sided cards are culled differently (diffuse 20.87 → 27.65 dB, alpha agreement
  84.5% → 97.5%). Tree billboards stay at 0° (180° costs 5.4 dB) and HD grass at 0° / 90°
  (180° / 270°: alpha agreement 74.9% vs 87.3%). `Adjust_n` is positive toward image left.
- The normal bake uses a fixed direct light of 0.5, not the billboard's: HD grass is lit with
  direct 0 yet has normals, and 0.5 gives the golden x scale (least-squares fit 1.00) and blue
  mean (248) on HD grass and the means on trunks. The G sign matches the golden (positive fit
  coefficients, no R/G cross term), mean `_n` colour (126, 123, 249) vs golden (127, 126, 249).
- `.txt` fields: `Model=` is `Meshes\` + the lower-cased path (replacement models keep the
  record's case, trunks `dyndolod\render\skyrim\billboards\...` lower case). Textures are slots 0,
  1, 2 (when the shader uses glow / soft / rim / back light) and 7 (back light), cut to start at
  `textures\`, only existing files; a missing or `textures\effects\` normal map falls back to
  `<diffuse>_n.dds`; complex grass does not list slot 1. TexGen's order looks hash-based, so we
  write first use (DynDOLOD only checks membership). CRLF lines and a trailing empty line.

| | Rendered | Golden size (front / side) | Width / Depth / Height / ShiftZ ok | Adjust_1 / _2 ok | Diffuse PSNR (golden-opaque / both) | Alpha agreement | `_n` PSNR | Sidecars (Model, ModelCRC, textures + CRCs) |
|---|---|---|---|---|---|---|---|---|
| Tree billboards | 126/126 | 74 (111 within 4 px) | — / 35 / — / 111 | | 25.25 / 25.53 dB | 94.1% | | 126 / 126 / 126 |
| Trunk billboards | 119/119 | 74 / 77 | 48 / 27 / 66 / 119 | 39 / 63 | 27.65 / 27.83 dB | 97.5% | 19.54 dB | 119 / 119 / 119 |
| HD grass | 79/79 | 28 / 24 | 15 / 8 / 18 / 49 | 47 / 64 | 29.66 / 30.52 dB | 87.3% | 22.54 dB | 79 / 79 / 79 |

Tolerances: 0.5 unit for Width/Depth, 1 unit for the rest. Texture order matches 8/126, 49/119
and 78/79.

Open:
- `_n` PSNR is low (19.5 / 22.5 dB). On trunks the golden x/y tilt is 1.54x / 1.29x ours with the
  same blue mean; no single bake light or z formula reproduces both, so TexGen's interior normals
  differ from ours in a way we have not found. On grass G is 10 levels low on average.
- TexGen's trunk twigs are thin dark lines where ours are short bark-textured cards; the bark
  texture is opaque BC1, so it is not alpha. Open.
- HD grass is rendered at the 32 px minimum, where one texel of trim is a large share of the
  size: 28/79 match the golden pixel size and heights differ by up to 40% on sparse grass.
- HD tree billboards (`TreeHDBillboards`) use the same two-view path but there is no golden.

### Step 5: batching and the `build` stage

- `Renderer::render_many` encodes any number of renders into one command buffer with one
  readback wait; `make_billboards` / `make_rendered_billboards` do all pre-passes of a batch,
  then all final renders (and normal bakes): two GPU round trips per batch of 32 instead of
  three or four per billboard. The renderer keeps uploaded source textures across renders
  (path-keyed, emptied past 1 GiB), and sidecar texture CRC32s are memoised.
- Tree billboards come from TexGen's own discovery now (`--golden` is optional): TREE records
  and STATs with Has Tree LOD, OBND magnitude >= 512, z2 >= 256, `tree` in the model, plus the
  `DynDOLOD_SSE_TexGen_Billboards_<plugin>.txt` overrides (`,True` / `,False` / `Scale=`) and the
  `DynDOLOD_SSE_Required_Billboards_<plugin>.txt` entries (candidates whatever the record type:
  the three Dawnguard winter aspens are STATs without the flag). Against the golden: 489/489
  candidates, the same 126 accepted, and `TexGen_SSE_Tree_Billboards.txt` identical line for line.
- `lodforge build --texgen` (opt-in) runs a TexGen stage before the worldspaces: the report, all
  accepted tree billboards and the 119 trunk billboards into the output, which is then added on
  top of the VFS (`Vfs::add_loose_root`) so the tree LOD stage uses them.

| | P7 | Step 4 | Step 5 |
|---|---|---|---|
| 126 tree billboards | 10.5 s (0.28 s per billboard from 64) | 8.4 s | **3.2 s** (25 ms each, discovery + CRCs included) |
| 119 trunk billboards (`_1`, `_2`, `_n`) | — | 3.9 s | **2.4 s** |
| 79 HD grass billboards | — | 3.0 s | 2.5 s (not batched yet) |
| `build --texgen` TexGen stage, Tamriel | — | — | 5.5 s (report + 126 trees + 119 trunks, BC7 + sidecars) |

`build --worlds Tamriel --no-terrain --no-objects --texgen`: TexGen 5.5 s, then tree LOD with 41
tree types and 43,745 references, the same coverage as from the golden billboards in this
profile (90 tree bases TexGen does not make billboards for remain without tree LOD, as with
TexGen itself).

## MVP: `lodforge build --mode dyndolod` (2026-10-18, branch `build-mvp`)

The build now chains the DynDOLOD pieces without any DynDOLOD output or golden file as input:
its own export → object atlas → object LOD (LODGen 3.1.8 port) + object height map → tree LOD →
DynDOLOD.esm/.esp + DLL NG data → Occlusion.esp. What it reads from DynDOLOD's install is only
its configuration (`Edit Scripts\DynDOLOD`: INI, rules, configs, billboard templates), as
DynDOLOD itself does. TexGen's stitched LOD textures (`textures\lod\*`) still come from the
profile (the `LODForge E2E - TexGen Output` mod here); LODForge's TexGen does billboards only.

```
lodforge build --instance <MO2> --profile <profile> --out <Data> | --mo2-mod <name>
    [--worlds ...] [--seasons SPR,SUM,AUT,WIN] [--texgen]
    --mode dyndolod [--preset Low|Medium|High] [--no-plugins] [--no-occlusion] [--static-lod]
    [--grass off|go|bto [--grass-source auto|cache] [--grass-density N]]
    [--work-dir <dir>] [--keep-lod-mods] [--materials pbr|complex]
```

(`--materials` was added on 2026-10-20, see "Materials mode" above.)

Stages and JSON events: once `texgen`, `dyndolod-config`; per worldspace `terrain-meshes`,
`terrain-textures` (per season), `dyndolod-export`, `objects-atlas`, `grass` (bto), `objects`,
`trees`; once `grass-lod1` (go), `plugins`, `occlusion`. The Tauri UI has the same options (LOD
style, preset, dynamic LOD, occlusion, grass) and counts these stages.

Input isolation: a build leaves out enabled mods that hold earlier LOD output (meta.ini
`comments=Generated by LODForge`, or DynDOLOD.esm / Occlusion.esp in the mod root) and the
`--mo2-mod` target; DynDOLOD.esm/.esp and Occlusion.esp are always dropped from the
load order. New `--exclude-mod` / `--exclude-plugin` work on every command that opens a profile.
LODGen's input files (export, atlas map, terrain `.bin`, FlatTextures, grass) go to a work folder
next to the output, removed afterwards unless `--work-dir` is given.

### Validation against the DynDOLOD E2E reference (Tamriel, Medium, grass LOD, default season)

Reference: `mods\LODForge E2E - DynDOLOD Output` (DynDOLOD 3, Medium, Tamriel, grass LOD at
100%, `CSGrassOptimizations=-1`), logs and export in `LODForge-golden\e2e-dyndolod-20261018`.
Ours: `build --worlds Tamriel --mode dyndolod --grass bto --grass-density 100` (56 s without grass,
80 s with, see below). Compared with `lodforge e2e-compare` (new; inventory, object LOD with
full attributes and by geometry, height map, plugins, dynamic LOD, TVDT) and `dyndolod-export
--golden`.

| Part | Ours vs reference |
|---|---|
| Rule table (`[Skyrim LODGen]`) | 159/159 lines |
| Export reference lines | 31,078/34,250 identical (90.7%); 37 trees ours to tree LOD, 200 child-world copies and 4 NoRule missing, 213 extra (tilted trees) |
| FlatTextures | 534/634 golden lines (grass lines come from the grass stage) |
| Object LOD files (default season) | 683 vs 685 (2 golden-only quads at y -64) |
| Object LOD geometry (positions only, 0.25-unit grid), non-grass | LOD4 **94.24%**, LOD8 **92.21%**, LOD16 **82.84%** of golden triangles (90.2 / 87.5 / 73.6% of ours) |
| Grass LOD4 geometry | **93.05%** of golden (98.35% of ours); 116.6 M vs 123.2 M triangles |
| Object LOD, all attributes | 0.21%: by design, our atlas is packed by xEdit's packer on 4096² pages (4 pages + `_n`), DynDOLOD's on one 8192×4096 page plus an alpha atlas, so atlas UVs and texture paths differ |
| LODGen port alone (DynDOLOD's own E2E export as input) | **96.16%** of 132.5 M triangles identical with all attributes (tolerant), 32/685 files equal up to order |
| Object height map | 12,125 vs 14,125 cells (golden reaches y -64); values on common cells 93.08% bit-identical, 98.61% within 1 unit |
| Dynamic LOD `[Objects]` originals | first MVP: ours 7,093, common 3,047; after the runtime iteration below: **ours 2,958, all in the golden 3,078 (96.1%)** |
| DynDOLOD.esp | after the runtime iteration: KYWD 2, ACTI 2, QUST 2, STAT 272, REFR 2,959 in 1 WRLD; golden 9 masters, QUST 2, FLST 1, LSCR 3, REFR 3,530, STAT 17, CELL 33 in 8 WRLD |
| DynDOLOD.esm | ours the two keywords; golden 1,340 CELL / 1,608 REFR (1,236 `_Disciple_` marker large references for the large-reference workarounds, 199 `_DynDOLOD_CHILD` copies, base record copies) |
| Occlusion, Tamriel | same 5,697 cells; 2,077 TVDT identical, bit Jaccard 0.9955 |
| Occlusion, child worlds | first MVP 0 (bug: child worlds used the INI's generic mode); now 350 vs 588 cells: the rest are cells DynDOLOD.esm adds to child worlds for its marker large references |
| Terrain underside, mini atlases | not produced (`Tamriel_Underside.nif`; 8 `textures\lod` + 47 `textures\pbr` DynDOLOD-made textures) |
| Tree LOD | xLODGen-style (389 files) vs DynDOLOD's 744 LOD4 `.btt` + lst |

Why occlusion is not exact here: with DynDOLOD's own height map and plugins, `lodforge occlusion`
gives 2,427/6,285 identical (Jaccard 0.9963), so the gap is not ours. The reference ran
04:08–04:46 against the terrain LOD in the profile at that time; `LODForge E2E - LODForge Output`
was rewritten 04:54–05:05, and DynDOLOD reads terrain LOD meshes from the VFS. The exactness gate
on the older golden still passes after the refactor: `Test-OcclusionParity.ps1` TVDT 6,283/6,285,
byte-identical with DynDOLOD's height map (24.9 s).

Regression check: `dyndolod-export` (now built on the same functions as the build) still gives
31,425/34,590 identical lines and 158/158 rules on the older golden.

### Grass

The user's direction is NGIO NG 2.0 plus Community Shaders' Grass Optimizations (GO). GO's
description asks LOD tools for object LOD **without** grass and for far-LOD grass meshes
`meshes\LOD\Grass\<model>_LOD1.nif` (simple crossed planes, normals matching the grass mesh); a
grass cache is optional for it. So there are two grass modes:

- `--grass go` (no cache, no placements): per grass model a `_lod1.nif`: two crossed quads from
  the grass billboard, laid out like LODGen's internal billboard (quad 0 along X, quad 1 at −90°,
  u = 0 at +X), normals up, alpha tested, two-sided, BSFadeNode root; models the load order
  already ships a `_lod1` for are skipped. 111 GRAS types used by LTEX → 78 billboards → 58
  meshes in 3.0 s. Not tried in game.
- `--grass bto` (DynDOLOD's classic grass LOD in LOD4 BTOs): placements per cell from the NGIO
  cache; cells without a cache file get no grass (one warning per worldspace; nothing is
  generated from the load order since 2026-10-19, see `docs/re/grass-cache-spec.md`).
  NGIO stores load-order form ids, and this profile's cache was made with another plugin set
  (25,360 of 32,125 files named the wrong GRAS), so cache files are rewritten for the current
  load order by model + object id; without that TexGen's type filter missed (40 instead of 32
  types). On a Community Shaders setup the grass lines use DynDOLOD's `ComplexGrassBillboard`
  template (`DynDOLOD_flat_4x2alt1_lod.nif`, 4 planes) and its grass brightness keys: grass
  geometry vs the reference went from 0% to 93.05%.

| Tamriel, default season | Cache (`auto`) | No cache (`generate`) |
|---|---|---|
| Placements | 32,125 cache files (all seasons), 0 cells generated | 5,686 cells generated |
| Grass types with billboards | 29 of 32 (3 weed meshes give no billboard) | 29 of 32 |
| Grass stage | 16.7 s (copy/remap 1.1 GB + billboards) | **4.0 s** |
| Object LOD size | 8.1 GB at 100% with the complex template (reference: 9.0 GB) | 2.4 GB at the default 50%, 2-plane template (before the complex-template change) |

An in-game NGIO precache of Tamriel takes tens of minutes (scripts/grass); `generate` needs none,
at the cost of grass inside rocks in the LOD (no Havok filtering; LOD4 only).

### Full build, headless (LODForge-E2E, all worldspaces, 5 seasons)

`lodforge build --json --instance D:\Modlists\skyrim-cs --profile LODForge-E2E --seasons
SPR,SUM,AUT,WIN --texgen --mode dyndolod --grass go --out <scratch>\Data`, release build,
i7-12700K / RTX 4080; nothing written to the profile's mods.

- **417 s wall, 155 stages, 0 failed**, 19 GB output. 15 worldspaces: terrain + DynDOLOD object
  LOD (all 15 got BTOs: Tamriel 3,416 files incl. 683 per season, Solstheim 381, Apocrypha 366,
  ...), tree LOD, one DynDOLOD.esm/.esp with DLL NG data for all 15, Occlusion.esp (10.5 MB),
  58 Grass Optimizations `_lod1` meshes.
- Stage sums: terrain textures 316.0 s (Tamriel 58.7, Solstheim 53.7), object LOD 21.7, occlusion
  15.7, DynDOLOD export 15.4, TexGen 13.7, trees 11.4, object atlases 10.8, terrain meshes 9.2,
  grass meshes 2.4, configuration 0.2, plugins < 0.1. Tamriel's stages 81 s, the other 14
  worldspaces 303 s, once-per-build stages 32 s.
- About the same as the xLODGen-mode build of this profile (6.9 min through the UI): DynDOLOD's
  object LOD replaces xLODGen's, and plugins + occlusion add about 16 s. DynDOLOD's final reference run (Tamriel only, default season) started 04:22 and
  its last outputs are from 04:46 (about 24 min).

### Runtime iteration (2026-10-18): plugins the DLL accepts, dynamic LOD as DynDOLOD selects it

Goal: an output that DynDOLOD DLL NG 3.07 can run, checked against the E2E reference.
New diagnostics: `lodforge plugin-dump <plugin> [--sig] [--groups]` and, in `e2e-compare`, a
per-base breakdown of dynamic LOD differences (`LODFORGE_DYNBASE`, `LODFORGE_DYNFIELDS`).

**DynDOLOD.esp framework** (golden dump; matches it byte for byte apart from the self load-order
index in VMAD and the run token):
- KYWD 0x910 (EditorID = run token) and **KYWD 0x912, EditorID `3.07`** in both plugins: the
  plugin format version the DLL compares (`GetESMVersion`/`GetESPVersion` = 307).
- ACTI 0x901 `DynDOLOD_MasterActivator` (VMAD `SHESON_DynDOLOD_Master`: MyQuest → 0x911,
  PlayerRef → 0x14), ACTI 0x905 `DynDOLOD_ObjectActivator` (`SHESON_DynDOLOD_ObjectActivator`).
- QUST 0x90F `DynDOLOD_MCM` (`SHESON_DynDOLOD_MCM`: ModName, MyQuest, PlayerRef; alias
  `PlayerAlias` → player with `SKI_PlayerLoadGameAlias`) and QUST 0x911 `DynDOLOD_Quest`
  (`SHESON_DynDOLOD_Quest`; alias script `SHESON_DynDOLOD_Alias`, Script → 0x911). DNAM
  `11 01 00 FF ...` (start game enabled), ALST/ALID/FNAM/ALFR/ALED.
- Master REFR per worldspace `<World>_ShesonObject` from ACTI 0x901, flags 0x40010400; dynamic
  REFRs flags 0x40010C00, EditorID `<plugin without punctuation>&<object id>_<World>_DynDOLOD_REFERENCE`.
- TES4 description `... SSE Generated Plugin\r<role>\rPlugin generated by DynDOLODVersion 3.07`.
- New ids start at 0x1000 (0x900–0x92F are DynDOLOD's fixed ids).
- `plugin::Rec::field_ids` marks form ids inside arbitrary subrecords (VMAD objects, ALFR) so the
  master list and load-order indices are written correctly.

**Dynamic LOD selection** (vs the golden `[Objects]` originals):

| Rule | Ours | Common with golden |
|---|---|---|
| first MVP | 7,093 | 3,047 |
| enable parent = player + opposite ⇒ never enabled ⇒ skipped (road/tree mods switching off vanilla refs) | 3,422 | 2,958 |
| `… LOD` grids: `<model>[_CRC]_dyndolod_lod.nif`, else the full model if the rule says Full model, else level 0, else none | (in the row above) | |
| large references on NearGrid skipped ("Skipping large reference in NearGrid") | **2,958** | **2,958 (100% precision, 96.1% recall)** |

Remaining golden-only: Hearthfire door/wall pieces with destruction or parent rules, water wheels,
windmill fans, a few FX rapids from Northern Roads (120 refs).

**`[Objects]` grammar from DynDOLOD.DLL** (REA/Ghidra on the DLL's data loader;
behaviour only): 17 comma-separated fields, each read as a string: 1–2 dynamic reference,
3 Level (int: 0 NearGrid, 1 FarGrid), 4 Delay, 5 InitiallyEnabled, 6 SwitchParentWorld,
7 OppositeParent, 8–9 enable parent, 10 destruction stage index (int, empty = 0), 11 stage type,
12 unused, 13 SwitchChildWorld, 14 Names3D (`;`-separated), 15–16 original, 17 LargeReference
(only read when present). Booleans are `== "1"`. Door (and no Fade) comes from the dynamic
reference's base type (DOOR), Map from a base flag, Persistent from the enable parent's record.
Errors it logs: "Object {} {} not found / not resolved / not a Reference / has no Base / Enable
Parent ... not found / Original ... not found".

Generator rules matched against the golden (2,958 common lines; all other fields equal):

| Field | Rule | Lines differing |
|---|---|---|
| Level | FarGrid only for large references (`DowngradeNormalReferences=1`) | 2 |
| Delay | rule grid was Far but the reference was downgraded, or the reference is persistent | 362 |
| InitiallyEnabled | not initially disabled and the top enable parent's state (XOR accumulated opposite) is enabled | 26 |
| enable parent / opposite | the top of the enable-parent chain, opposite flags XORed; the original itself without one | 0 |
| SwitchParentWorld / SwitchChildWorld | always 1 (golden: all but 18 large references) | 18 |
| destruction stage | -1,-1 | 6 |
| LargeReference | master-listed large reference | 2 |

**Occlusion**: child worldspaces now use their main worldspace's `OcclusionMode_<World>` minus
`+Border` (they used the INI's generic `-TVDT -Flat +Border`, which selected no cells): 6,047 of
6,285 cells (Tamriel 5,697 + 350 child cells; the other 238 are cells only DynDOLOD.esm's marker
copies create).

Not done (not needed for the DLL to run): DynDOLOD's large-reference workarounds (DynDOLOD.esm
`_Disciple_` markers, RNAM, `[Markers]`/`[Resets]`), `[Bases]` + base MNAM overrides, the
underside FLST, loading screens, child-world `_DynDOLOD_CHILD` copies.

### First in-game run (2026-10-18, skyrim-cs, profile `LODForge-MVP-Test`)

Output: `build --seasons SPR,SUM,AUT,WIN --texgen --mode dyndolod --grass go --mo2-mod "LODForge MVP
Output"` (460 s; 15 worldspaces, DynDOLOD.esm/.esp with 3,418 dynamic references in 11 worldspaces,
Occlusion.esp 11,202 cells in 22 worldspaces, 58 grass `_lod1` meshes). Test profile = copy of
LODForge-E2E with that mod at top priority and `LODForge E2E - DynDOLOD Output` / `LODForge E2E -
LODForge Output` disabled; `debug=true` set in its `DynDOLOD_NG_Worlds.txt` for the run. Launched
with skyrim-mcp (SKSE through MO2), `coc Riverwood` from the main menu.

- DynDOLOD DLL NG 3.0.43 log: reads `DynDOLOD_NG_Worlds.txt`, "27 full plugins of 254, 81 light
  plugins of 4096", **"Init Sucessful"**. On the first grid change it enables our dynamic LOD
  references (campfires, waterfalls, farmhouses, tents with their `_dyndolod_lod` / LOD models).
- Papyrus: `SHESON_DynDOLOD_MCM ... INITIALIZED`, **"DynDOLOD successfully initialized"**,
  "Registered DynDOLOD at MCM". No crash log; the game ran and quit cleanly.
- Visual comparison with the reference profile (`LODForge-Ref-Test`, same vantages) was not
  conclusive: a new game started with `coc` pauses on the Survival Mode prompt, and Community
  Shaders' exposure under the paused/free camera swings between dark and white. Screenshots are in
  `D:\workspace\SkyrimMCP\runs\screenshots\20261018T222942Z` (ours) and `...T223234Z` (reference).
  A fair visual check needs a saved game in both profiles.

Second pass with a save (`lod-compare-riverwood`: made in `LODForge-Ref-Test` past the Survival
prompt via MessageBoxMenu `buttonPress` 1, noon, SkyrimClear; copied to `LODForge-MVP-Test`), both
profiles loading it, `raw` preset, window capture, free camera above Riverwood, 15 s settle:

| Vantage | SSIM ours vs DynDOLOD (compass masked) | Pixels > 16 apart |
|---|---|---|
| A: eye z 2,500, north | 0.951 | 9.9% |
| B: eye z 8,000, north toward Whiterun | 0.937 | 13.1% |
| C: eye z 4,000, south | 0.930 | 13.5% |

The heatmaps put the differences in moving clouds, the animated mist on the peaks and swaying
trees; terrain, mountains and the distant valley match. At B's distance Whiterun is a faint shape
in both, slightly more defined in DynDOLOD's frame (not resolved at this haze). Loading the
DynDOLOD-made save in our output logs EDST "reference ... could not be found" (the cosave lists
the reference DynDOLOD.esp's dynamic references), as expected when DynDOLOD output is swapped
under a save. One run hung (main thread, no crash log) during a window capture right after a
Community Shaders render-target reinit; the same shot on a relaunch worked, so it is not
attributed to the output yet.
Screenshots: `D:\workspace\SkyrimMCP\runs\screenshots\20261018T223638Z` (reference),
`...T224129Z` and `...T224746Z` (ours), with `-diff.png` heatmaps next to ours.

## Rolling updates and large-reference workarounds (2026-10-18, branch `dyndolod-rolling`)

Goal: DynDOLOD.esm/.esp that can be rebuilt under an existing playthrough. Spec: docs/re/
dyndolod-spec.md "Large-reference workarounds" and "Rolling updates". DLL behaviour from REA on
DynDOLOD.DLL 3.0.43 (LoadData `[Markers]` parser, LargeREFRsFixAttachDetach, REFRsResets).

**Rolling updates.** When the output already holds a LODForge DynDOLOD.esm/.esp, every new record
keeps its object id, matched by (signature, EditorID, model). New ones continue from the old HEDR
next object id, and the run token is kept (`--new-token`, `--renumber` opt out).
- Unit test `rolling_updates_keep_form_ids`: rebuilding the same input gives byte-identical
  plugins and data files. Adding one dynamic reference with a new model keeps every old id; the
  new base copy and reference get old-next and old-next+1.
- Tamriel build on top of its own output (LODForge-E2E): DynDOLOD.esm, DynDOLOD.esp and both data
  files byte-identical.
- Full rebuild of `LODForge MVP Output` (all 15 worldspaces, 5 seasons, TexGen, GO grass; 821 s,
  0 failed stages) on top of the build-mvp plugins: "loading DynDOLOD.esp next object ID 1EB2 ...
  (3768 records)". All 3,768 records keep their form ids. The new records are the 21 REVERT
  overrides.

**Large-reference workarounds** vs the E2E DynDOLOD reference (`lodforge large-refs --world
Tamriel --output <ours> --golden <reference>`):

| Part | Ours | Golden | Common |
|---|---|---|---|
| Disciple markers Tamriel | 861 | 873 | 861 (no extra cells) |
| MarkarthWorld (own LOD, main rule, listed) | 4 | 4 | 4 |
| RiftenWorld / WindhelmWorld (Bounds ± 3, NoCellsWithNAVM) | 81 / 80 | 81 / 80 | 81 / 80 |
| WhiterunWorld / SolitudeWorld | 110 / 118 | 100 / 98 | 100 / 98 |
| markers total | 1,254 | 1,236 | 1,224 |
| `[Markers]` lines | 865 | 877 | 865; same count 862, same persistent list 851 |
| `[Markers]` persistent items | 434 | 449 | |
| `[Resets]` (= `_DynDOLOD_REVERT`) | 23 | 57 | 23 |

- The `[Markers]` count is the RNAM entries listed in the cell plus 1 (the marker): 863/873 golden
  lines. The DLL uses the engine's count whenever that is higher.
- `[Resets]`: all 23 of ours are in the golden list. The other 34 are references our export gives
  no LOD (Civil War banners, CC tower/hall pieces, 19 Solitude references DynDOLOD copies into
  Tamriel).
- Occlusion.esp (Tamriel build): 6,279 cells vs golden 6,285 (6,047 before). Tamriel, Riften,
  Windhelm and Katariah have exactly the golden cell counts. Whiterun/Solitude have 10/20 extra
  (the marker cells above). WhiterunDragonsreach/WindhelmPit lack 15/21 cells that only
  `_DynDOLOD_CHILD` copies create. Bit Jaccard on common cells 0.9954.
- Full build: Occlusion.esp 11,434 cells in 22 worldspaces (11,202 before).

### In game (skyrim-cs, profile `LODForge-MVP-Test`, `LODForge MVP Output` rebuilt with this branch)

- Save `lod-compare-riverwood`, made under DynDOLOD's own output: DynDOLOD DLL NG 3.0.43 reads
  our `DynDOLOD_NG_Tamriel.txt` (887 `[Markers]` lines, 23 `[Resets]`) without a "Worldspace /
  Marker ... not found" error, logs "Init Sucessful", and Papyrus says "DynDOLOD successfully
  initialized".
  - With `debug=true` the DLL:
    - toggles our markers (DynDOLOD.esm base 0x928) count+1 times when cells enter the
      large-reference grid (1,562 toggle lines over one load and two `cow` trips);
    - runs `recycleactor` on all 23 REVERT references at every TravelEnd;
    - enables and disables our dynamic references (DynDOLOD.esp) across Riverwood → Whiterun →
      Riverwood.
  - "Large References count -20,-8 11 > 10" and "-39,2 22 > 21": in 2 cells the engine counts
    one large reference more than our count. The DLL takes the higher value.
  - The 3,088 EDST "could not be found" lines all name DynDOLOD's own ids from that save
    (0x12xxx/0x13xxx). Ours start at 0x1000.
- Saved `lodforge-rolling-1`, quit, and rebuilt the mod in full (610 s). DynDOLOD.esm,
  DynDOLOD.esp, Occlusion.esp and the per-world data files came out byte-identical.
- Relaunched and loaded `lodforge-rolling-1`: 0 EDST/EDLR "could not be found", DLL initialised,
  game responsive.
- The first launch of the session hung: the main thread was in a sleep, about 4 s after "DynDOLOD
  successfully initialized", with no crash log. Relaunching with the same save and output (plus
  `debug=true`) worked, and so did the two runs after it. Not attributed yet. That launch ran
  while another session had Community Shaders HDR and frame generation switched off (5:52–6:22
  PM) and used its rebuilt AgentBridge.dll (SkyrimMCP `capture-hdr-fixes`). The build-mvp
  session saw a similar one-off hang without markers.
- Not done: a visual check that large-reference LOD does not vanish on long walks (no
  screenshots this run).

## Gap pass (2026-10-18): tree LOD, underside, glow, seasons, other worldspaces

Goldens: E2E DynDOLOD run (Tamriel), golden2 (Tamriel, Windows + FakeLights, all seasons),
golden3 (the 13 other worldspaces), all under `D:\Modlists\LODForge-golden`.

| Area | Result |
|---|---|
| Tree LOD (DynDOLOD rules) | Tamriel 51,566/51,566 golden refs, same blocks/scales (7 extra: refs DynDOLOD made dynamic); seasons within 2-19 refs; all golden3 worlds exact incl. Solstheim 1505/1505; .btt blocks = LOD4 quads with any CELL (exact in 7 worlds) |
| Tilt rule | xEdit `30 < deg < 330` on DynDOLOD's rounded signed degrees: 391/391 exclusions, 0 extra |
| Terrain underside | Tamriel 16/16 quads, every vertex/triangle; byte-identical in golden quad order; ESM MSTT + ESP FLST/REFR as golden |
| Glow windows | export glow lines 255/255; glow atlas map lines identical; 56/56 glow-page shapes match |
| Seasons (SoS swap ini) | written; SUM 59/108 pairs, SPR/AUT/WIN low (NOFLAG/GLOWREFR copies, vanilla-EDID naming) |
| LODGen terrain .bin | Tamriel + 10/13 worlds identical cell sets; object LOD Markarth 99%, HunterHQ 98%, Deepwood 89%, Skuldafn 83% of golden triangles |

Open: child-world copies into DynDOLOD.esm (`_DynDOLOD_CHILD`, 220 lines in Tamriel), fake lights
(need replacement fxglow meshes), NOFLAG/REVERT/GLOWREFR ESM records, mini-atlas re-UV, `_Alpha`
atlas page, dynamic LOD recall outside Tamriel (Blackreach 10%, Sovngarde 38%), Soul Cairn object
LOD (52%), Apocrypha extra LOD8, in-game seasonal check.

### Child worldspace copies (`_DynDOLOD_CHILD`, 2026-10-18)

`lodforge child-copies` (diagnostic) and `build --mode dyndolod` copy child worldspace references
into the parent (Tamriel) as DynDOLOD does. Against golden2 (Medium preset):

| Check | Result |
|---|---|
| Copy selection | 199/199 references, none extra (ScanChild, IgnoreChildEDIDMedium, ChildworldMatches pairs unless the parent side is deleted by a reference rule, duplicate of a parent ref, initially disabled through enable parents, trees with enable parents, has LOD) |
| DynDOLOD.esm REFRs | 199/199 EditorIDs; position, rotation, scale, base record type and base model identical; VWD flag 190/199; base copy flags 168/199; base EditorIDs differ for 86 trees (DynDOLOD's `_Default` season naming) |
| Export lines | 220/220 (with glow lines), 203 identical; the rest are the large-reference flag on Windhelm civil-war pairs and float text of one z |
| Object LOD (Tamriel vs golden2) | LOD4 94.21% -> 94.52%, LOD8 91.76% -> 92.05% of golden triangles in common |

## Own LOD resources: builds without DynDOLOD Resources (2026-10-18, branch `own-lod-resources`)

Design and inventory: docs/re/own-lod-resources.md. New: `lodforge lod-sources` (where a
DynDOLOD-mode build's LOD models come from), `lodforge lod-models` (generator, optional
comparison with a reference mod), crate `lodforge-lodmodel` (meshoptimizer edge collapse), and
`build --lod-models [--lod-model-cache DIR] [--lod-model-min-size a,b,c]`.

Inventory (Tamriel, Medium, LODForge-E2E): DynDOLOD Resources supplies the LOD model of 93.5% of
LOD4 / 64.9% of LOD8 / 58.2% of LOD16 references (8.68 M of 9.23 M placed LOD4 triangles) and 12
`_dyndolod_lod` models (488 dynamic references) plus 152 level-0 models used by 1,239 dynamic
references. Without it, the export drops from 34,222 to 14,838 reference lines and dynamic LOD
from 2,958 to 1,900 references.

Validation: `build --worlds Tamriel --texgen --mode dyndolod` on LODForge-E2E, compared with
`e2e-compare` against `mods\LODForge E2E - DynDOLOD Output` (DynDOLOD with Resources), outputs
in `D:\Modlists\LODForge-resources-runs`:

| | (a) with Resources | (b) `--exclude-mod "DynDOLOD Resources SE 3"` | (c) = (b) + `--lod-models` | DynDOLOD |
|---|---|---|---|---|
| Build time | 63 s | 65 s | 82 s (models cached: 2.2 s stage) | |
| Export reference lines | 34,222 | 14,838 | 47,018 | 34,250 |
| Golden LOD4 / LOD8 / LOD16 refs that have a model | (MVP: 90.7% lines identical) | | **92.0% / 96.7% / 93.5%** (+15.5 k / 11.0 k / 8.3 k refs golden has none for) | |
| Object LOD triangles LOD4 | 5.85 M | 2.37 M | 10.16 M | 5.60 M |
| LOD8 | 3.03 M | 1.23 M | 4.18 M | 2.87 M |
| LOD16 | 0.93 M | 0.59 M | 1.65 M | 0.83 M |
| Geometry identical to golden (0.25 grid), LOD4 / 8 / 16, % of golden | 94.2 / 92.2 / 82.7 | 35.8 / 37.7 / 51.6 | 36.4 / 38.0 / 51.6 (generated meshes differ from Resources' by construction) | |
| Dynamic LOD `[Objects]` | 2,958 (all in golden, 96.1%) | 1,900 (61.7%) | 3,702 (2,546 in golden = 82.7%) | 3,078 |
| Atlas | 122 textures, 4 pages | 61, 1 | 246, 11 | |

(c) generates 2,827 levels for 1,614 full models (124 have no usable geometry: markers, decals,
blended leaves), 44 MB under `meshes\lod\lodforge\` in the output; uncached, `lod-models` makes
~7,200 levels for 2,400 models in 2.5-6 s on 20 threads. 32,473 of the 47,018 export lines use a
generated model. The surplus over DynDOLOD is (1) objects Resources gives no LOD (driftwood,
branch piles, logs, stumps, Hearthfire house parts and Civil War rubble with enable parents,
which also become dynamic LOD), and (2) denser models (below).

Triangle budgets against Resources' models (`lod-models --min-size 0 --reference-mod`, same
models, weighted by references): level 0 **1.59x**, level 1 **0.63x**, level 2 **0.77x** with
the defaults (error 32 / 56 / 140 units, caps 30 / 15 / 8%). Per model the gap is largest for
city architecture, which Resources ships only as coarse `_lod_2` boxes (e.g. `whpalace1_lod_2`,
204 triangles; ours 4,632 at level 0), and for rocks at level 0 (2.1x at error 24). Sweep of the
level-0 error: 12 units 3.17x, 24 units 2.09x (permissive 1.97x), 40 units 1.50x.

Not yet checked in game; `_dyndolod_lod` effect models (waterfalls, fires, water) are not
generated (those dynamic references fall back to level-0 or full models).

### In-game check of (c) (2026-10-19, skyrim-cs)

Mod `LODForge OwnLOD Output` (hardlinked copy of build (c), Tamriel only) in a new profile
`LODForge-OwnLOD-Test` (copy of `LODForge-MVP-Test` with that mod instead of `LODForge MVP Output`
and **DynDOLOD Resources SE 3 disabled**). Save `lod-compare-riverwood`, same vantages and capture
settings as the MVP pass (raw preset, window capture, 15 s settle, compass masked), compared with
the DynDOLOD reference frames (`20261018T223638Z`). Ours: `D:\workspace\SkyrimMCP\runs\screenshots\20261019T002827Z`.

| Vantage | SSIM own models vs DynDOLOD | MVP (with Resources) vs DynDOLOD |
|---|---|---|
| A: eye z 2,500, north | 0.936 (15.8% of pixels > 16 apart) | 0.951 |
| B: eye z 8,000, north | 0.905 (25.8%) | 0.937 |
| C: eye z 4,000, south | 0.923 (17.8%) | 0.930 |

- Papyrus "DynDOLOD successfully initialized"; no crash log; clean quit. The DLL log lists EDST
  "could not be found" for the reference DynDOLOD.esp's references in the save, as in the MVP run.
- Differences are concentrated in the far mountain ranges (B, C), but they are capture noise,
  not LOD: a rebuild without any generated level-2 model (`--lod-model-min-size 256,512,100000`,
  LOD16 0.59 M triangles, vanilla models only) shows the same crisp far range, and two shots of
  vantage B taken 15 s apart in one session score SSIM 0.53 against each other (Community Shaders
  exposure / haze drift and small framing shifts). The SSIM gap to the MVP run (0.905 vs 0.937)
  is inside that noise. The user, comparing the frames by eye, sees no real difference.
- The window captures (PrintWindow, `source=window`) are noticeably brighter than the game on
  screen, which washes out distant detail; a fair LOD comparison needs a capture path that matches
  the displayed image and several shots per vantage to measure the noise floor first.
- Grass LOD meshes (`--grass go`) were not part of (c), so far grass differs as well.

### Clean in-game comparison (2026-10-19, HDR off, frame captures)

The earlier window captures were unusable: skyrim-cs runs Community Shaders with HDR (paper
white 203, peak 800 nits) and frame generation, and a window grab clips the HDR output at paper
white. With CS HDR and frame generation switched off for the run (restored afterwards) and
skyrim-mcp's frame capture fixed for a 10-bit SDR back buffer (SkyrimMCP branch
`capture-hdr-fixes`), `game_screenshot source=frame preset=clean` is stable. Shots in
`D:\workspace\SkyrimMCP\runs\screenshots\lodcmp-20261019\{own,mvp,ref}`.

Vantage B (eye z 8,000 above Riverwood, north), SSIM:

| Pair | SSIM |
|---|---|
| Same build, two shots: own / MVP / DynDOLOD | 0.974 / 0.962 / 0.949 (noise floor) |
| MVP (with Resources) vs DynDOLOD | 0.979 (caveat: another session was rebuilding `LODForge MVP Output` 18:05-18:19 local while these MVP frames were taken at 18:17, so they may mix old and new files) |
| Own models (no Resources) vs DynDOLOD / vs MVP | 0.879 / 0.884 |

Findings (crops `crop-left-B.png`, `crop-far-B.png` next to the shots):
1. **Plains void south of Whiterun, in all three builds (DynDOLOD's too):** seen from the free
   camera, whole level-4 terrain blocks (about cells x 0..8 north of y -4, and west of x 0 down to
   y -8) draw as a flat pale area with a dark edge. All three profiles use LODForge-built terrain
   LOD (Ref-Test takes it from `LODForge E2E - LODForge Output`). Ruled out: the BTR meshes
   (present, byte-identical to xLODGen's, heights continuous across block edges), the water
   quads (small, within terrain height), the terrain textures (same format and size as unaffected
   blocks), `Occlusion.esp` (void unchanged with it disabled) and the block distance INI settings
   (LOD4 to 57,344 units; the void is ~30,000 from the player).
   **Resolved 2026-10-19: not a LODForge terrain issue but an artefact of free-camera shots
   taken far from the player.** Evidence (shots in
   `D:\workspace\SkyrimMCP\runs\screenshots\terrain-void-20261019`, profile `LODForge-Void-Test`
   = Ref-Test copy):
   - `LODForge E2E - LODForge Output` disabled, so vanilla terrain LOD from the BSAs plus
     DynDOLOD objects: the same block-shaped void in the same place (`V1-noLF-top` vs Ref-Test
     `top-riverwood`, camera 16000,-16000,60000 looking down).
   - `tll` (all LOD off): the void area looks the same, only the loaded cells around Riverwood
     draw, so nothing (terrain or object LOD) is drawn there at all (`V1-tll-top`). `tws` (water
     off) leaves it too (`B-nowater`): the pale area is fog/sky, not water.
   - Same camera, player moved with `cow Tamriel -3 -3` (Fort Greymoor, inside the void): the
     whole area renders, no void (`V1-top-player-m3m3`).
   So the engine leaves out LOD blocks depending on where the player stands, not on the files.
   From Riverwood the plains lie behind the valley's hills, which fits culling from the player's
   point of view (not confirmed which engine path). LODForge's terrain LOD (btr byte-identical to
   xLODGen, base and `.SUM`; textures decode to normal plains, checked as a mosaic of the LOD4 diffuse maps) is not at fault. For
   LOD comparisons, put the player near the camera (`cow`/`coc` to a cell under the vantage)
   instead of using a free camera far from the player. Vantage B and the top-down shot should be
   retaken that way before reading anything into that area.
   **Whiterun itself** additionally is missing in both LODForge builds (with and without
   Resources): DynDOLOD shows the city in Tamriel through its own child-world copies
   (`DynDOLOD.esm`, 72 of the golden-only references near Whiterun), which LODForge does not
   write yet.
2. **Mountain moss/snow overlay (LOD4):** generated level-0 mountain models keep the full
   model's moss/snow overlay layer and render it as opaque pale sheets where DynDOLOD and the MVP
   show green moss.
3. **Far mountains (LOD8/16) bright white:** without Resources these levels use vanilla's
   mountain `_lod` meshes (baked snow textures); Resources' models use the full textures with
   DynDOLOD's snow handling (`_lod_Nh/l`), which reads grey and misty.
So the own-models build has a real visual gap on mountains; the earlier "capture noise"
conclusion was wrong, and the earlier SSIM numbers in this section are superseded.

## TexGen stitched object-LOD textures (2026-10-18, branch `texgen-stitched`; measured at 00c9e81 = main + build-mvp 7cbff2a)

`lodforge texgen-stitched` (and a `texgen-stitched` stage of `build --texgen --mode dyndolod`,
after the billboards, before the DynDOLOD export) makes TexGen's mini atlases from the
`DynDOLOD_SSE_TexGen_{noalpha,alpha,copy}_<plugin>.txt` rules of the loaded plugins: tile atlases
under `textures\lod\...` with `_n` twins, PBR twins, `DynDOLOD-temp\` intermediates, the `x`-rule
full textures (`*noalpha.dds`, glow maps) and `DynDOLOD_SSE_re-uv-miniatlas.txt` (written to the
work folder). CPU resampling (rayon) + GPU BC7 with full box mips. Rules in
docs/re/dyndolod-spec.md §3.1/§3.2 ("LODForge stitched").

Golden: `mods\LODForge E2E - TexGen Output` (TexGen run of `LODForge-golden\e2e-texgen-20261018`),
profile LODForge-E2E with that mod excluded (`--exclude-mod "LODForge E2E - TexGen Output"`).

| | Ours vs golden TexGen output |
|---|---|
| Rules | 1,149 lines, 290 destinations (9 shadowed by Majestic Mountains), 32 copy rules → 37 aliases |
| Made | 272 atlases, 66 PBR twins, 14 `x` textures, 5 temporaries, 338 `_n`; 0 skipped; 9.7 s (stage incl. BC7 + write) |
| File coverage | 688 of the golden's 828 non-billboard textures (83.1%) from the rules; the other 140 are the 70 rendered textures + `_n`, made since by the rendered stage below (828/828) |
| Diffuse (non-PBR, incl. `x`) | 284: mean **34.62 dB**, median 34.02, min 20.75, 229 ≥ 30 dB |
| PBR twins | 66: mean **35.71 dB**, median 35.77, min 22.54, 48 ≥ 30 dB |
| `x`-rule textures | 16 diffuse/glow: mean 53.2 dB (exact copies + tone, up to BC7 noise) |
| `_n` | 338: mean **27.73 dB**, median 27.30, min 17.25, 90 ≥ 30 dB |
| Re-UV rows | 342/344 identical (2 sources shared by two atlases where TexGen's pick follows its hash order) |

PSNR is RGB over all texels, ours uncompressed vs the golden BC7 (TexGen: Texconv BC7 quick), so
the ceiling is below the BC7 noise floor. Steps that moved the diffuse mean: plain trilinear at the
tile LOD 31.4 dB → `unmulta` (colour / alpha at alpha ≥ 128) 32.8 → bias −1.25 34.5 → gamma +
additive brightness, PBR scale on PBR sources only, temporaries saved with alpha 255 34.6 (the
outliers they fixed: farmhouselod01snow 14 → 31 dB, castlekitpbr_lod 19 → 27, shiplod 20 → 27,
snow01lod / impextrubble01lod / fxdirtmound01lod 14 → 21..34). Sub-pixel offsets (±0.25, ±0.5 px)
all lose 5+ dB, so registration is right. Open: the `_n` residual (golden X/Y/Z means differ from
the source `_n` by a few levels on some atlases, e.g. dirt02lod_n 130/130/221 vs source
127/127/229; slopes of a linear fit 0.7–0.9, i.e. texel-level, not a gain), alpha edges of the
`_alpha_` atlases (21–26 dB on alpha), and the real `bias` uniform.

### Rendered object-LOD textures (§3.3)

The same stage (and `texgen-stitched --rendered`) renders TexGen's 70 object textures from
`Render\Skyrim\Objects\**\*.nif` with the P8 GPU renderer: orthographic view of the pieces from
−Y framed on the `TexGen Plane`, unlit (direct 0, ambient 1.0), vertex colours, `unmulta` + bleed,
size `256 × plane scale` on the long side; PBR twins with PBR pieces. 7.8 s for 70 textures + `_n`.

| | Ours vs golden |
|---|---|
| Files | the golden's 70 exactly (58 + 12 PBR twins), nothing extra; 21 NIFs left out like TexGen (19 with missing textures in Enderal/HPP/SMIM folders, SMIM fence and Enderal `rtbuildingsenderallod02` by folder) |
| Sizes | 70/70 equal; atlas map of the excluded-mod build: all 38 TexGen-made entries (stitched + rendered) at TexGen's size |
| Diffuse | mean **32.48 dB**, 57 of 70 ≥ 30 dB (wrwallslod01 33.1, icefloessolid02pbr_lod 43.4, dlc01icewallpbr_lod 41.1) |
| `_n` | mean 26.13 dB, 3 ≥ 30 dB |
| All TexGen object textures | 828/828 golden files; diffuse 342 at 34.18 dB, PBR 78 at 35.56, `_n` 408 at 27.45 |

Fits: yaw 180 (from +Y only back faces render), the plane's scale applies to its geometry too
(without it the frame was 2x zoomed: 18.7 → 33.1 dB on wrwallslod01), output name from the NIF
path, `_p` diffuse → plain diffuse + PBR `_n` in the PBR twin (8.9 → 41.1 dB). Low ones:
seruinswindowslod 21.7 (alpha), ridgedmossstone08lod 22.7 (TexGen alpha-blends the moss layer over
the stone; we only alpha-test), whwallwulfharthlod 22.8, dweobservatoryexteriordomelod 23.3 (dark:
metal/envmap shading).

### E2E with the TexGen Output mod excluded (Tamriel, Medium, grass bto 100%)

`build --worlds Tamriel --texgen --mode dyndolod --grass bto --grass-density 100 --exclude-mod
"LODForge E2E - TexGen Output"`: **187 s, 0 failed stages** (TexGen billboards 20.5 s, stitched
textures 9.7 s). `e2e-compare --exclude-mod "LODForge E2E - TexGen Output"` against `mods\LODForge
E2E - DynDOLOD Output`:

| Object LOD geometry (% of golden) | LOD4 | LOD8 | LOD16 | Grass LOD4 |
|---|---|---|---|---|
| MVP control: TexGen Output enabled, no `--texgen` | 94.24 | 92.21 | 82.84 | 93.05 |
| `--texgen` (incl. stitched stage), TexGen Output enabled | 94.22 | 92.17 | 82.70 | 93.05 |
| **`--texgen`, TexGen Output excluded** | **94.22** | **92.17** | **82.70** | **0.00** |

- Object geometry with our textures is 100.00% identical (LOD4/8/16) to the run that still has
  TexGen's; the 0.02–0.14 point drop vs the MVP control comes from `--texgen`'s tree billboards
  (FlatTextures), not from the stitched textures. Object atlas: the same 122 entries; 3 differ in
  size because the 70 rendered textures (§3.3) fall back to the vanilla `textures\lod` file of the
  same name (e.g. Bridge01LOD 256² instead of TexGen's 512²).
- Grass: not a texture issue; see "Grass LOD without the TexGen output" below (the guess first
  written here, that the grass stage skips types the mod has billboards for, was wrong).
- With the rendered stage (build of 3285a7f+, 105 s, 0 failed): the object atlas takes all 38 of
  its TexGen-made inputs at TexGen's size (only the P8 trunk billboards differ, 64/80 vs 68/84 px
  wide); object geometry unchanged at 94.22 / 92.17 / 82.70%; grass still 0% (above).

### Grass LOD without the TexGen output (cause and fix)

Cause, traced: the object stage (LODGen port, `FlatList::parse`) sizes every grass quad from the
billboard's `.txt` read through the VFS. The grass stage writes its billboards to the work folder,
which is not in the VFS, so with TexGen's output installed LODGen used TexGen's HD grass sidecars
(`Complex=True`, `Width_n`), and without it there were none and the quads kept the flat-line sizes
of our standard billboards (fern_brown 69.3 units wide instead of 106.5). Grass inputs were
identical between the runs; the `all_types` rule was not involved.

Fix: with Community Shaders installed the grass stage renders TexGen's **HD grass** billboards
(two views, `_1`/`_2` + `_n`, full sidecar; `GrassArgs::hd`, `grass-billboards --hd`) and
references them as `<stem>_1.dds` like DynDOLOD's export; the object stage falls back to the grass
stage's own sidecars when the load order has none. 32 of 46 cached grass types get billboards
(the 3 weeds that gave no standard billboard render in HD).

| Tamriel, grass bto 100% | Grass LOD4 exact (0.25-unit grid) | Within 1 unit | Within 4 units | Objects LOD4/8/16 |
|---|---|---|---|---|
| TexGen Output excluded, before | 0.00% (116.6 M triangles) | — | — | 94.22 / 92.17 / 82.70 |
| **TexGen Output excluded, after** | 1.09% (125.2 M vs 123.2 M golden) | 5.42% | 38.35% | 94.22 / 92.17 / 82.70 |
| TexGen Output enabled, after | **100.00%** of golden (was 93.05%; 1.9 M extra triangles ours) | 100.00% | 99.61% | 94.22 / 92.17 / 82.70 |

The rest of the gap is HD grass framing (P8 open item: only 15/79 golden widths within 0.5 unit;
fern_brown `Width` matches to 1e-5 but `Width_1` is 104.0 vs 106.2, weed_green 33 vs 53 units
wide), which moves every grass vertex. `e2e-compare` now also prints grass LOD4 by triangle centroid
within 1 / 4 units.

With TexGen's sidecars (mod enabled) every golden grass triangle is now reproduced exactly: the
HD `_1.dds` references were the missing piece of the earlier 93.05%. So the excluded-mod gap is
only our HD billboard dimensions (P8).

## ParallaxGen CRC map (gap P7, 2026-10-18, branch `p7-parallaxgen-crc`)

LOD model discovery now reads PGPatcher's `ParallaxGen_Diff.json` through the VFS (last
winner; here in `mods\PGPatcher Output`, 1,713 entries). For a mesh whose installed CRC32
equals `crc32patched`, the `<stem>_<CRC32>_lod` lookup uses `crc32original`. This is used for
object LOD levels, `_dyndolod_lod` dynamic models, the golden Object_Report comparison and
`lod-models` naming. `lod-sources` and the build use the same resolver.
`LODFORGE_NO_PARALLAXGEN=1` turns it off for comparisons. Spec:
docs/re/dyndolod-spec.md "ParallaxGen CRC map".

`dyndolod-export --profile LODForge-E2E --exclude-mod "LODForge E2E - DynDOLOD Output,LODForge E2E
- LODForge Output"`, compared with the reference exports in `tools\DynDOLOD\Edit
Scripts\Export` (reference lines identical as a multiset):

| World | Lines changed by the map | Identical to reference, before → after (ours / reference lines) |
|---|---|---|
| Tamriel | 0 | 31,078 → 31,078 (34,009 / 34,526) |
| MarkarthWorld | 1 (`000B39F7` mrkmillrocks01 LOD4/LOD8 → `mrkmillrocks01_2ed49507_lod_1.nif`, as in the reference) | 209 → 210 (220 / 222) |
| DLC2SolstheimWorld | 0 | 2,283 → 2,283 (2,699 / 2,714) |

Why so few: only 4 of the 1,080 CRC-named LOD files in this profile carry the original CRC of a
patched mesh: `mountaincliffcrevasse_5b9d9efd_lod_2h` (snow variant; the reference does not use
it either), `mrkmillrocks01_2ed49507_lod_1` (Markarth only) and `wrskyforge01_866b650f_lod_0/2`
(Tamriel places `wrskyforge01lod`, not `wrskyforge01`). The other 163 CRC-named files for
patched stems are for other mesh variants. The reference exports use 3 CRC-named object LOD
models in total, and the map explains the one of them that is for a patched mesh.
TexGen side: billboards are found by form id, not CRC. The 11 billboard sidecars of patched
meshes carry the patched `ModelCRC32` in TexGen's output, as LODForge writes it, so nothing
changes there.

## AltTextures and VC side files (branch `dyndolod-side-files`)

> Method text of this section was moved out of the repository on 2026-10-20 (it described the LODGen-derived implementation that has since been replaced; see docs/licensing.md). The measurement tables below are kept as the historical record; the current implementation and its numbers are in docs/re/terrain-lod-spec.md, docs/re/object-lod-spec.md and docs/re/occlusion-spec.md.

| Season | VC lines identical | AltTextures lines identical | References with the same entry set | Entries |
|---|---|---|---|---|
| default | 5,805/5,892 (98.5%) | 11,017/17,494 (63.0%) | 17,206/17,494 (98.4%) | 44,730/45,120 (99.1%); ours 44,733 |
| SPR | 5,583/5,691 (98.1%) | 10,928/17,390 (62.8%) | 17,134/17,390 (98.5%) | 43,488/43,837 (99.2%) |
| SUM | 5,235/5,346 (97.9%) | 11,163/16,819 (66.4%) | 16,589/16,819 (98.6%) | 39,245/39,549 (99.2%) |
| AUT | 5,585/5,884 (94.9%) | 10,612/17,501 (60.6%) | 17,205/17,501 (98.3%) | 44,818/45,239 (99.1%) |
| WIN | 5,801/5,874 (98.8%) | 11,017/17,491 (63.0%) | 17,206/17,491 (98.4%) | 44,730/45,116 (99.1%) |
| Worldspace | VC | AltTextures lines | Same entry set | Entries |
|---|---|---|---|---|
| Blackreach | 443/444 | 191/231 | 226/231 | 406/411 |
| ccBGSSSE067DeadlandsWorld | 85/85 | 91/124 | 124/124 | 237/237 |
| ccKRTSSE001QNWorld | 53/59 | 33/50 | 49/50 | 150/151 |
| DeepwoodRedoubtWorld | 167/167 | 59/71 | 71/71 | 163/163 |
| DLC01FalmerValley | 243/243 | 805/1,010 | 1,009/1,010 | 1,579/1,580 (SUM 1,270/1,370) |
| DLC01SoulCairn | 154/154 | 388/652 | 652/652 | 1,641/1,641 |
| DLC1HunterHQWorld | 139/139 | 51/62 | 62/62 | 117/117 |
| DLC2ApocryphaWorld | 36/36 | 0/0 | — | — |
| DLC2SolstheimWorld | 1,566/1,572 | 708/1,095 | 1,044/1,095 | 2,265/2,326 |
| JaphetsFollyWorld | 146/146 | 56/118 | 74/118 | 104/164 |
| MarkarthWorld | 1,011/1,030 | 101/111 | 111/111 | 167/167 |
| SkuldafnWorld | 172/172 | 87/126 | 125/126 | 293/294 |
| Sovngarde | 213/213 | 141/193 | 193/193 | 407/407 |
| Output | Rock/snow/ice HD shapes | Using PBR textures | Mean vertex colour |
|---|---|---|---|
| DynDOLOD (E2E reference) | 1,468 | 1,416 (96.5%) | 254.2 |
| LODForge main (`LODForge E2E - LODForge Output`) | 1,905 | 1 (0.1%) | 184.5 |
| **This branch** | 2,212 | 2,021 (91.4%) | **254.2** |

## Mini-atlas rows in the object atlas map (gap G1, 2026-10-19, branch `claude/g1-miniatlas-rows-9c6ee3`)

> Method text of this section was moved out of the repository on 2026-10-20 (it described the LODGen-derived implementation that has since been replaced; see docs/licensing.md). The measurement tables below are kept as the historical record; the current implementation and its numbers are in docs/re/terrain-lod-spec.md, docs/re/object-lod-spec.md and docs/re/occlusion-spec.md.

| | DynDOLOD | LODForge |
|---|---|---|
| Mini rows (10 columns) | 345 | 344, **342 identical** (misses: `wovenfence01`, `landscape\cavebaseground01`, `sgrass01aut` from MiniAtlasExport / P3) |
| Atlas page rows (8 columns) | 355 (169 mini atlases) | 214 (141 mini atlases, 123 the same); was 124 on 4 pages, now 2 pages |
| Tamriel LOD4 | before (`LODFORGE_NO_MINIATLAS=1`) | **after** | DynDOLOD (grass bto) |
|---|---|---|---|
| Distinct textures | 156 (6,028 refs) | **60** (3,315 refs) | 14 (7,287 refs, 4,016 of them grass) |
| Shapes obj / obj-LargeRef | 1,513 / 2,273 | **825 / 868** | 560 / 655 |
| Shapes objSnow / objSnow-LargeRef | 458 / 1,067 | 301 / 604 | 213 / 749 |
| Geometry LOD4 / 8 / 16 (`e2e-compare`, % of golden) | 94.47 / 92.34 / 83.29 | **97.01 / 94.26 / 86.10** | |
| Object LOD triangles LOD4 | 5.86 M | 5.59 M | 5.60 M |
| Build time | 209 s (cold) | 90 s | |

## G4: Nature of the Wild Lands trunk billboards (2026-10-18, branch `claude/g4-notw-trunks-9ec205`)

Cause: TexGen takes the render models of rendered (trunk) billboards from two roots, and
LODForge only read the first:
- `<DynDOLOD>\Edit Scripts\DynDOLOD\Render\Skyrim\Billboards` (DynDOLOD's own; `Model=`
  `dyndolod\render\skyrim\billboards\...`);
- `Data\DynDOLOD\Render\Billboards` from the load order (loose or BSA; `Model=`
  `dyndolod\render\billboards\...`). Nature of the Wild Lands ships its 122 trunk NIFs there.

The E2E TexGen debug log lists 121 `GetModelsToRender.CheckModel` "Adding" lines for the first
root and 122 for the second. Space in the folder name, `_summer` variants, rules and mesh masks
play no part. Fix: `trunk_nifs` in `apps/cli/src/texgen_billboards.rs` merges both roots by the
path below `Billboards` (a data file replaces a tools file with the same path; none overlap in
this profile). The output path is `textures\<path below Billboards>`, and data paths come from
the VFS in lower case, so stems are lower case like TexGen's (`..._b7dcba97_trunk`, NIF on disk
`..._B7DCBA97_trunk.nif`). `lodforge build --texgen` gets them through the same function.

Command: `lodforge texgen-billboards --instance D:/Modlists/skyrim-cs --profile LODForge-E2E
--trunks --golden "D:/Modlists/skyrim-cs/mods/LODForge E2E - TexGen Output" [--filter "nature of
the wild lands"]`, against the E2E TexGen output (Community Shaders, PBR Landscapes, PGPatcher).

| Set | Made | Golden size front / side | Diffuse PSNR (golden-opaque) | Alpha agreement | Normal PSNR | Sidecars: Model / ModelCRC32 / texture set / texture CRC32s | Time |
|---|---|---|---|---|---|---|---|
| Before (main 073361e) | 119 | | | | | | |
| All trunks | **241** (119 + 122) | 159 / 162 | 25.27 dB | 97.1% | 19.83 dB | 241 / 241 / 240 / 240 | 8.0 s |
| Nature of the Wild Lands only | **122/122** | 85 / 85 | 23.17 dB | 96.7% | 20.09 dB | 122 / 122 / 122 / 122 | 2.5 s |

- All 610 NOTW file names (122 `.txt` + `_1`/`_2` and their `_n`) equal the reference folder's.
- Golden pixel size is 70% for NOTW (85/122), like P8's vanilla trunks (74/126); the residual is
  P8's 16x-vs-8x MSAA coverage noise. Width / Height / ShiftZ within tolerance: 60 / 84 / 116 of 122.
- The one texture-set mismatch over all 241 is vanilla `tundradriftwood01_trunk`: TexGen lists the
  PBR `textures\pbr\landscape\mountains\mountainslab02*` set, we list the vanilla one (PBR
  swap on trunks, not part of G4).

## Small output fixes (2026-10-19, branch `claude/small-output-fixes-944329`; gap audit G7, G8, G29, G31)

> Method text of this section was moved out of the repository on 2026-10-20 (it described the LODGen-derived implementation that has since been replaced; see docs/licensing.md). The measurement tables below are kept as the historical record; the current implementation and its numbers are in docs/re/terrain-lod-spec.md, docs/re/object-lod-spec.md and docs/re/occlusion-spec.md.


## LODGen port fixes P4, G3, G9, G10, G11 (2026-10-19, branch `claude/objlod-port-fixes-1d734a`)

> Method text of this section was moved out of the repository on 2026-10-20 (it described the LODGen-derived implementation that has since been replaced; see docs/licensing.md). The measurement tables below are kept as the historical record; the current implementation and its numbers are in docs/re/terrain-lod-spec.md, docs/re/object-lod-spec.md and docs/re/occlusion-spec.md.

| Check (default season, `--tolerant`) | Before (073361e; main 3fd7e68) | After |
|---|---|---|
| P7 golden (`dyndolod-20261017-medium-tamriel`, profile LODForge-DynDOLOD) | 98.435%, 104 files equal up to order, 2 with different shape sets | **98.441%**, 104, **0** |
| E2E port (DynDOLOD's E2E export, profile LODForge-E2E, vs `LODForge E2E - DynDOLOD Output`, no grass) | 4.21 M triangles identical, 32 files equal, 470 with different shape sets | **8.67 M** (of 9.34 M ours; the golden's 132.5 M include grass), **49**, **366** |
| E2E port, Tamriel LOD4 HD shapes | 418 (objSnowHD-LargeRef 307, objHD-LargeRef 105, objAshHD-LargeRef 5, objSnowHD 1) | **868** (498 / 358 / 5 / 7); DynDOLOD 867 (498 / 357 / 5 / 7) |
| P7 golden, LOD4 HD shapes | 459 | 461; golden 455 (the objHD-LargeRef spread is rule 3 of P7) |
| E2E full build (`build --worlds Tamriel --texgen --mode dyndolod --grass go`), LOD4 HD shapes, before AltTextures (P1) | 460 | 460 |
| Same build with AltTextures (main 6d860cc vs merge 6fbfee6) | 413 (objSnowHD-LargeRef 307, objHD-LargeRef 105, objSnowHD 1) | **863** (498 / 358 / 7); DynDOLOD 867 (498 / 357 / 7, + objAshHD-LargeRef 5) |
| xLODGen flavour (Default profile, 683 BTOs) | | byte-identical |

## MiniAtlasExport (P3) and the `noalpha` replacement (2026-10-19, branch `claude/p3-miniatlas-export-a1361c`)

> Method text of this section was moved out of the repository on 2026-10-20 (it described the LODGen-derived implementation that has since been replaced; see docs/licensing.md). The measurement tables below are kept as the historical record; the current implementation and its numbers are in docs/re/terrain-lod-spec.md, docs/re/object-lod-spec.md and docs/re/occlusion-spec.md.

| | DynDOLOD | before | **after** |
|---|---|---|---|
| MiniAtlasExport atlases / rows | 4 / 16 | none | **4 / 16** (same sources, layouts, names) |
| AltTextures lines with `snow01noalphalinear` | 3 | 0 | **3** (the same references) |
| LOD4 distinct textures (refs), `bto_stats` | 14 (7,287, 4,016 grass) | 66 (3,535) | **62** (3,500) |
| LOD4 shapes obj / obj-LargeRef | 560 / 655 | 797 / 626 | **777 / 611** |
| LOD4 refs `bricks01.dds` / `volcanictundradirtcliffs01linear.dds` | 0 / 0 | 20 / 14 | **0 / 0** |
| LOD4 BTOs with `snow01noalphalinear.dds` | 2 | 0 | **2** |
| Texture | diffuse | `_n` |
|---|---|---|
| `frozenmarshice02pbr_lod` | 33.78 dB | 36.06 dB |
| `volcanictundradirtcliffs01pbr_lod` | 34.81 dB | 26.75 dB |
| `bricks01lod` | 36.90 dB | 34.42 dB |
| `groundmud01lod` | 40.91 dB | 24.82 dB |

## Billboard shader re-derivation (2026-10-19, branch `billboard-rederive`)

`crates/texgen/src/billboard.wgsl` was replaced by a shader written from a functional description
of the output (lit colour per texel, normal-texture encoding, material terms) and the renderer's
uniform / vertex layout, for the open-source release: the previous file was a structural
transcription of TexGen's shipped GLSL. Where the description left a detail open, the plain
physical reading was tried first and the golden TexGen billboards arbitrated; the target is the
comparison tool's region, not bit-exactness. Size estimation, trimming, binary alpha, bleed, BC7
and the sidecars (lib.rs) are untouched.

Commands (release build, RTX 4080): `lodforge texgen-billboards --instance D:/Modlists/skyrim-cs
--profile LODForge-DynDOLOD --golden D:/Modlists/LODForge-golden/texgen-20261017/output` (126 tree
billboards; `--trunks` for the 119 trunk billboards) and the same with `--golden
D:/Modlists/LODForge-golden/texgen-grass-20261018 --hd-grass` (79 HD grass billboards). "Before"
is the main checkout's binary of the same day (the P8 shader) against the same goldens; the P7
figure of the brief (64/127 within tolerance, 20.95 dB) predates the P8 geometry and framing.

| | Before | Re-derived |
|---|---|---|
| Trees: golden size (within 4 px), alpha agreement | 74/126 (111), 94.1% | 74/126 (111), 94.1% |
| Trees: diffuse PSNR golden-opaque / both-opaque | 25.25 / 25.53 dB | 25.16 / 25.44 dB |
| Trees: BC7 PSNR, 4x4 block means | 24.57 / 24.76 dB, 30.38 dB | 24.51 / 24.71 dB, 29.98 dB |
| Trunks: diffuse PSNR, alpha agreement | 27.65 / 27.83 dB, 97.5% | 27.58 / 27.76 dB, 97.5% |
| Trunks: `_n` PSNR (both-opaque), golden = k × ours fit R / G | 19.54 dB, 1.54 / 1.29 | 20.06 dB, 1.28 / 0.96 |
| HD grass: diffuse PSNR, alpha agreement | 29.66 / 30.52 dB, 87.3% | 29.65 / 30.50 dB, 87.3% |
| HD grass: `_n` PSNR, fit R / G | 22.54 dB, 1.08 / 1.22 | 22.21 dB, 0.86 / 0.85 |
| HD grass: Depth / Height / ShiftZ within tolerance | 8 / 18 / 49 | 13 / 20 / 51 |

Sizes, framing and sidecars do not depend on the shader (the alpha test `a > threshold` instead
of `a >= threshold` moves a few HD grass silhouettes by a texel, the last row). Visual check of
the PNG dumps (`TG_PNG=1`): `treepineforest02`, `treeaspen01` (golden | ours | 4x difference:
foliage speckle only, no lighting or colour cast), the `dlc1treepineshortheavysnow` trunk `_n`
(same left-red / right-cyan cylinder shading as the golden, upright) and the `beachgrassobj01`
`_n` (blade detail in the same orientation as the golden).

Choices the golden arbitrated (each noted where it is made in the shader):
- Base: `vertexColour × albedo × (ambient + direct × max(N·L, 0))`, normal map in the stored
  frame (x = bitangent along u, y = tangent along v, z = vertex normal), green along +v. The
  base alone: 22.81 dB on trees. Green along −v: 21.05 dB. Without the vertex colour: 21.56 dB.
  Flipping the normal on back faces: 20.24 dB.
- Soft lighting (vanilla pine and aspen branches: Soft_Lighting with the diffuse itself as the
  slot-2 mask, lighting effect 1 between 1 and 10): a fill of 0.75 × direct for the directions
  the direct light does not cover, `base × mask × direct × 0.75 × (1 − max(N·L, 0))`: 25.27 dB
  (0.7..0.85 alike; 1.0: 24.96; 0.5: 24.83; squared falloff 25.24). The first reading, a wrapped
  diffuse scaled by lighting effect 1, blew the pines out (16.16 dB); every form using lighting
  effect 1 as a wrap exponent, divisor or gain scored lower than the constant fill (wrap^e1
  22.67, wrap/e1 23.32, constant 1/e1 23.95, back-lit max(−N·L, 0) 23.41, wrap alone 23.08).
- Specular (Blinn-Phong, normal-map alpha mask, non-PBR): −0.1 dB on trees (25.16 with it),
  no change on trunks; kept as described.
- Rim lighting: no vanilla tree has the flag; implemented as described, unconstrained.
- Normal pass: the description's view-space normal, `0.5 + 0.5 n`, scores 15.45 dB on trunks
  and 12.40 dB on grass. The golden textures encode green as down (the game's normal-map
  convention; +2.5 dB on trunks), keep the map's tilt on back faces and only turn the depth
  component toward the viewer (negating the whole vector mirrors the detail: −7 dB on grass),
  and are nearly frontal (blue mean 249 on a bare trunk cylinder, 248 on grass). The half-way
  direction between the viewer-facing surface normal and the view direction gives 20.06 /
  22.21 dB (trunks / grass); the surface normal itself with the two conventions 18.6 / 12.2; a
  tilt floor of 18° instead of the pull 21.4 / 20.8; a pull of 0.5 / 1.5 / 2.5: 20.9 / 19.2 /
  18.1 on trunks and 20.0 / 22.5 / 21.6 on grass.
- `_n` alpha: the golden `_n` files are opaque everywhere (alpha 255, bleed colour outside the
  silhouette), not the specular strength the description names; the pipeline takes coverage
  from the render alpha, so both passes write alpha 1 and `finish_alpha` binarises as before.

Also: `flag::GLOW_MAP` now needs the Glow_Map shader flag (the slot-2 texture of a Soft_Lighting
shape is its mask, not an emissive map); `LODFORGE_BILLBOARD_WGSL=<file>` loads a shader at run
time (about 70 variants were measured without a rebuild); `TG_MAT=1` prints each draw's material
values; `render::BAKE_DIRECT` is gone (the normal pass no longer lights anything).

Process record (provenance):
- 2026-10-19, worktree `billboard-rederive` cut from main 07c8e1a. First action: `git rm
  crates/texgen/src/billboard.wgsl` without opening it.
- Inputs read: crates/texgen/src/{render.rs, lib.rs, scene.rs}, apps/cli/src/texgen_billboards.rs
  and texgen_rendered.rs (the normal-pass consumers), crates/nif/src/lib.rs (`LightingShader`
  fields), crates/dds (decode API), docs/baseline.md (P8 "TexGen billboard parity", steps 2–4,
  behavioural notes only), the golden DDS / TXT files, the comparison tool's PNG dumps and the
  material / tangent-frame dumps of the meshes.
- Never opened: the previous billboard.wgsl in any revision, diff or log; `D:\Modlists\skyrim-cs\
  tools\DynDOLOD\Edit Scripts\DynDOLOD\Shaders`; docs/re/dyndolod-spec.md; anything under
  D:\Modlists\ref\ (AnvilLOD was later allowed as an optional GPL reference and was still not
  consulted, so no GPL attribution applies); no web search for TexGen's shaders.
- Every fitted constant above was chosen only by the comparison tool's numbers on the golden
  output images (PSNR, alpha agreement, the `_n` least-squares fit and mean colours).
