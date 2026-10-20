# Post-merge validation: 2026-10-19

**Verdict:** the merged output builds, resolves its checked assets, initializes
DynDOLOD in game and passes a limited runtime smoke test. It is **not full parity**.
The next work is in the [gap-closure plan](gap-closure-plan.md).

## Reproduction and scope

- Generation commit: `f1a31e6` (P3 and G2 merged). No generation code changed in this pass.
- Profile: `skyrim-cs / LODForge-E2E`; Medium, Tamriel, default season.
- Full terrain, TexGen, objects, classic trees, underside, plugins and occlusion;
  BTO grass from the existing E2E cache at **100%** density. Eight Rayon threads.
- All three prior E2E LODForge/DynDOLOD/TexGen output mods excluded from generation.
- Golden: `mods\LODForge E2E - DynDOLOD Output`, with its separate TexGen output
  available only for reference asset resolution/runtime. The saved golden preset
  confirms Medium, default season, tile caps 1024/256, NearGrid 11 and FarGrid 21.
- Rust 1.98.1; RTX 4080 present, NVIDIA driver 32.0.16.1714.
- Local evidence: `bench\validation-f1a31e6-20261019\`: manifests, input/reference
  SHA256 hashes, build log, comparison/validation JSON, DLL-field comparison,
  screenshots and runtime logs. Large outputs are ignored, not committed.

```powershell
$run = 'D:\Modlists\LODForge\bench\validation-f1a31e6-20261019'
$env:RAYON_NUM_THREADS = '8'
.\target\release\lodforge.exe build `
  --instance 'D:\Modlists\skyrim-cs' --profile LODForge-E2E `
  --exclude-mod 'LODForge E2E - DynDOLOD Output,LODForge E2E - TexGen Output,LODForge E2E - LODForge Output' `
  --mode dyndolod --preset Medium --texgen `
  --grass bto --grass-source cache --grass-density 100 --worlds Tamriel `
  --out "$run\Data" --work-dir "$run\work" --json
```

Use a new run directory when reproducing. No new golden generation was performed.
The historical reference terrain input is not pinned here, so TVDT differences
cannot be attributed solely to the occlusion implementation.

## Build and offline validation

| Check | Result |
|---|---|
| Complete generation | 186.121 s; zero failed stages |
| Output | 8,524 files, 11,743,354,394 bytes |
| NIF/BTO/BTR parsing and checked geometry invariants | 1,711 files; zero errors |
| DDS headers/payload sizes | 5,243 files; zero errors |
| NIF texture sets/effect paths and plugin MODL/TXST paths | 2,423 unique assets resolved; zero missing in the filtered runtime VFS |
| Reference validation | 686 NIFs, 60 DDS, 440 unique assets; zero missing/errors |
| Changed-path tests | 76 passed, including two GPU tests |
| New validator tests | Four passed (includes reused profile test) |
| Reference preservation | All 1,499 output SHA256 hashes unchanged after testing |
| Original profile preservation | Captured mod/plugin/load-order and game INI hashes unchanged |

The [new validator](../apps/cli/examples/validate_output.rs) processes one file at a
time. It decodes all retained mips of recognized atlases and compares top mips of
same-path, same-size non-atlas DDS files. It does **not** validate every plugin
form/VMAD field, BTT record, or pixel/layer of every other texture. Clippy completed
with existing dependency warnings and advisory validator/reused-module warnings.

Both input VFS reports warn that the INI-listed `Skyrim - Patch.bsa` is absent.
It did not leave any of the checked output references unresolved, but the warning
is retained rather than calling the entire input installation error-free.
Current cache/PGPatcher contents are also hashed (34,988 files); no historical
cache hash was available to prove those bytes have never changed since the golden.

### Atlas and material results

The four object atlas files match golden names, dimensions, compression, byte sizes
and eight-mip count: main diffuse/normal 8192x4096 BC1, alpha diffuse 2048x2048 BC7,
alpha normal 2048x2048 BC1. Packing/content differs, so page-image PSNR is not a
meaningful equivalence test.

| Default Tamriel | LODForge | Golden |
|---|---:|---:|
| Non-grass shapes LOD4 / 8 / 16 | 3,280 / 973 / 324 | 3,271 / 971 / 324 |
| **Named HD groups**, including snow/ash, LOD4 / 8 / 16 | 863 / 108 / 30 | 867 / 108 / 30 |
| Classic tree atlas | 4096x4096 BC7, 8 mips | 4096x2048 BC7, 8 mips |

The old 43-vs-51 LOD8 count used a different HD classification; it must not be
compared to all named HD groups. Equal totals still do not imply equal group
membership/materials. In LOD4, shader group `type=0, flags2=0x80000021` has
436 shapes versus 441; passthrough soft-light/vertex-color groups also differ.

Four generated mini-atlas diffuse top mips score **33.78-40.91 dB** RGB PSNR.
Their normal maps score **24.82-36.06 dB**. This confirms production, not exact pixels.

**New outlier:** `textures\pbr\landscape\snow01noalphalinear.dds` is only
**10.20 dB** against the golden. Its upstream `snow01noalpha.dds` is **58.27 dB**
against reference TexGen output. This localizes the major residual after the
no-alpha source stage; trace linear conversion/source mip/color interpretation
before changing its fitted algorithm. Other compared linear albedos start at
30.40 dB. Alpha agreement for this snow copy is 100%.

### Alpha mip behavior is still different

Fraction of texels with alpha >= 128, first to eighth mip:

| Atlas | LODForge | Golden |
|---|---|---|
| Object alpha page | .352, .352, .352, .352, .351, .349, .348, .332 | .390, .414, .433, .461, .492, .519, .545, .547 |
| Classic tree atlas | .142, .142, .142, .140, .138, .137, .150, .203 | .257, .281, .297, .320, .340, .369, .395, .480 |

These are **whole-page diagnostics**, not matched-tile measurements: tile sets
and tree page sizes differ. Nevertheless the merged coverage-preserving method
does not establish equivalence with DynDOLOD's expanding mip opacity. Average-color
fill and alpha handling are implemented; the remaining task is algorithm parity,
not adding the missing features from scratch.

## Geometry and runtime data

Geometry uses the existing comparison's 0.25-unit position grid and **common BTO
filenames only**. There are 686 ours, 685 golden, 685 shared files.

| Kind | Golden triangles | Ours, common files | Common | Share of golden |
|---|---:|---:|---:|---:|
| Objects LOD4 | 5,596,496 | 5,590,156 | 5,443,341 | 97.26% |
| Objects LOD8 | 2,877,472 | 2,891,002 | 2,727,622 | 94.79% |
| Objects LOD16 | 828,532 | 875,408 | 729,697 | 88.07% |
| Grass LOD4 | 123,242,568 | 125,162,456 | 1,344,342 | 1.09% |

Grass centroid matches are 5.42% within one unit, 38.35% within four units.
This is a substantial remaining billboard/framing/selection investigation,
even with cache and density matched. Earlier no-terrain recount percentages use
different clipping inputs; these numbers alone do not prove an atlas regression.

- Trees: 51,573 vs 51,566 references, all 51,566 golden refs shared with matching
  block/scale; seven additional enable-parent trees. Both have 214 list types.
- Dynamic originals: **2,958 / 3,078**, no extras. Raw `[Objects]` rows are
  2,958 / 3,088 because the reference repeats some originals.
- DLL data: global **Worlds.txt `[Bases]` 0 / 593**; Tamriel `[Disabled]` 0 / 39,
  `[Markers]` 865 / 877, `[Resets]` 23 / 57.
- Matching-original raw rows: Delay differs in 362, InitiallyEnabled in 32,
  destruction index/type in 12 each, Level/LargeReference in two each.
  The field report counts golden rows, including duplicate original identities.
- STAT DNAM: nonzero in **1 / 246** ours, **337 / 338** golden. Record sets differ;
  compare shared identities/material semantics before prescribing exact bytes.
- Object height map: 14,125 cells each, 94.28% values exact, 99.013% within one unit.
- Occlusion: 6,279 / 6,285 cells, 2,361 identical TVDT; common-cell bit Jaccard
  0.996178. City cell sets differ, so aggregate similarity is not occlusion parity.

## Runtime smoke test (MCP verified)

Used an isolated `LODForge-Validation-f1a31e6` profile without copied saves.
Reference run enabled only reference DynDOLOD/TexGen outputs; the fresh run enabled
only `LODForge Validation f1a31e6`. Required runtime assets remained enabled.
The main E2E profile was not toggled.

- Live AgentBridge handshake: Skyrim **1.7.104.0**, SKSE **2.3.1.0**, DLL NG
  **3.0.43.0**. Community Shaders logged TruePBR and grass features loading.
- Both runs: Papyrus logged **"DynDOLOD successfully initialized"**; DLL opened
  Worlds/Tamriel data with no missing-form errors in the captured DLL log.
- Clear/noon, FOV 75, 1280x720, player beneath the camera, ten-second settling.
  Calendar season settings unchanged (month 7); this is not a five-season test.
- A Survival Mode prompt initially paused the reference. It was identified via
  Scaleform, declined in both runs, and shots were retaken. Only `*-settled` shots
  are compared.

| Matched viewpoint | SSIM | Pixels differing >16/channel |
|---|---:|---:|
| Tundra east | 0.949696 | 3.520% |
| Winterhold north | 0.991339 | 0.324% |

No obvious missing-texture surfaces appeared in the inspected frames. Near objects,
weather/animation and terrain contribute to these scores: neither is a pure LOD
metric or a pass threshold.

The fresh output saved and reloaded `LODForge-validation-f1a31e6`. The MCP load
helper timed out waiting for a player-cell event, but subsequent state showed
`gameLoadedCount=1`, loading closed, unpaused, and `inLoadedArea=true`; Papyrus
confirmed reinitialization. This is a verified reload with an event-wait limitation,
not a clean automation pass. WhiterunWorld entry and return to Tamriel also worked.

Both sessions quit normally; no game/MO2 process or machine lock remains.
The dedicated test mod/profile/save and evidence remain for inspection.
This does not test rolling-update rebuilds, destruction/enable-parent scenarios,
all worlds, Ultra/complex trees, GO-only grass, all seasons or every camera distance.
