# Provenance re-implementation plan (GPL-3.0-only)

Replaces every module whose provenance is incompatible with LODForge's chosen
license, GPL-3.0-only (see [licensing](licensing.md), "Decision record"), with
code written in fresh contexts from scrubbed specifications, public
documentation, permissively licensed or GPL-compatible references (public-domain
Terra 0.7, MPL-2.0 xEdit, GPL-3.0-only AnvilLOD) and golden output. Revised
2026-10-19 (late) for Path B. Nothing here waits on anyone's permission.

## Scope inventory (measured 2026-10-19 evening)

| Module | Lines | Provenance | Action |
|---|---|---|---|
| [crates/btr](../crates/btr/src): terra.rs 795, quad.rs 566, geometry.rs 418, underside.rs 372, output.rs 88, bin.rs 91, export.rs 59 | ~2,400 | LODGen 3.1.8 (MPL 1.1): Terra port, quad/cell assembly, water, skirts, underside, Forsyth cache optimiser | **Stage B1: rewrite** (export.rs is ours and may stay) |
| [crates/nif](../crates/nif/src): writer.rs 414, btr.rs 255, LODGen-default constants in lib.rs | ~770 | "mirroring xLODGen's `NiFile.Write`", "`TerrainLOD.GenerateBTR`" block order, LODGen `BBox` maths | **Stage B1: rewrite** the writer and BTR assembly from the NIF format spec + golden files. The reader stays |
| [crates/objlod](../crates/objlod/src): build.rs 1,694, shape.rs 778, reuv.rs 604, export.rs 504, flat.rs 479, geometry.rs 450, terrain.rs 267, grass.rs 252, atlas.rs 246, objheight.rs 137, math.rs 489, lib.rs 42 | ~5,900 | LODGen 3.1.8 (MPL 1.1) | **Stage B2: rewrite.** Keep [delaunay.rs](../crates/objlod/src/delaunay.rs) (s-hull beerware), [atlas_build.rs](../crates/objlod/src/atlas_build.rs) and [source.rs](../crates/objlod/src/source.rs) (xEdit MPL-2.0). Drop the .NET-numerics emulation |
| CLI pieces: atlas planning + grass billboard layout in [dyndolod_build.rs](../apps/cli/src/dyndolod_build.rs), [glow_atlas.rs](../apps/cli/src/glow_atlas.rs) 204, LODGen-format plumbing in [terrain_underside.rs](../apps/cli/src/terrain_underside.rs), [occlusion.rs](../apps/cli/src/occlusion.rs), flavour plumbing in [object_lod.rs](../apps/cli/src/object_lod.rs) | ~500 | Mirror LODGen traversal/key rules and file formats | **Stage B2** (rewritten together with objlod; formats become ours where only our pipeline uses them) |
| [dyndolod/sidefiles.rs](../crates/dyndolod/src/sidefiles.rs) shape ordering | part of 489 | DynDOLOD side files index shapes in a traversal order | Interoperability: re-verify against golden side files, document, keep |
| [occlusion.rs](../crates/dyndolod/src/occlusion.rs) | 1,010 | Ghidra/REA on DynDOLODx64.exe | **Target 1: re-implement**, AnvilLOD derivation allowed (scrub session running) |
| [billboard.wgsl](../crates/texgen/src/billboard.wgsl) | 162 | Transcription of TexGen's GLSL | **Target 3: re-derive** (session running) |
| [crates/grass](../crates/grass/src) generator | 917 of 1,078 | NGIO (GPL-3) + engine disassembly | License-compatible; **removal optional** (session running on `remove-grass-generator`; merge is a product decision) |
| [treelod](../crates/treelod/src) 511, objlod `atlas_build.rs` 282, `source.rs` 390 | 1,183 | xEdit MPL-2.0 | Keep (MPL 2.0 §3.3); notices |
| [dyndolod](../crates/dyndolod/src) remainder ~4,500, [terrain](../crates/terrain), [texgen](../crates/texgen) remainder, nif reader, esp/archive/dds/core/gpu/lodmodel/seasons, UI | — | Own code, interoperability, behavioural specs | Keep; classify per spec section |

Rewrite total: roughly **10,000 lines of Rust** plus the specs. The original
ports took about four days with the decompiles in hand; a clean rewrite with
tolerance-level validation is a multi-week effort of many sessions.

## Method (applies to every stage)

1. **Spec scrub (analyst).** The analyst may read everything, including the
   tainted modules and decompiles, and writes a spec that conveys *function*,
   not expression: file formats (with the legitimate source of each format:
   golden files, MPL xEdit writers, public docs), observable rules and
   constants, configuration keys, acceptance metrics. Decompile-derived detail
   goes verbatim to `D:\Modlists\ref\quarantine\` (outside the repository) with
   a pointer left behind. Rule of thumb: if a sentence needs the decompile to
   be written, it does not belong in the spec.
2. **Fresh implementer context.** A separate session that has never had the
   tainted modules, decompiles, quarantined text or the research documents
   listed in [licensing.md](licensing.md) ("Research material that implementers
   must not read") in context. Its brief lists the allowed inputs and the files
   it must delete unread as its first action. It may read AnvilLOD and port
   from it with GPL attribution.
3. **Output-based validation only.** Golden files, the existing comparison
   tooling (`e2e-compare`, the tolerant BTO/BTR comparisons, the texgen image
   comparisons, `Test-OcclusionParity.ps1`) and in-game checks on the
   LODForge-E2E / Ref-Test profiles. Never iterate against the old code or the
   decompiles.
4. **Record the process.** Each stage lands as commits that delete the old
   module, add the new one, and append a "Process record" (date, inputs read,
   files never opened, AnvilLOD files ported) to the stage's spec.
5. **Parity policy.** Byte-identical parity with LODGen/DynDOLOD is no longer
   a goal (it cannot be reached without the decompile). Each stage records the
   tolerant-comparison numbers before and after; acceptance is "valid files,
   documented tolerance, no in-game visual defects at the agreed vantage
   points", not a fixed percentage.
6. **Architecture stays LODForge's.** Export + atlas map from the xEdit ports →
   mesh builders → NIF writer; seasons as deltas; GPU atlas/texture work.
   AnvilLOD is a reference for algorithms and formats, not a pipeline to adopt.

**Caveat:** a solo-maintainer clean room is weaker than a two-team one. This
process improves defensibility and documents good faith; it is not a guarantee,
and it is not legal advice.

## What AnvilLOD provides (GPL-3.0-only, `D:\Modlists\ref\AnvilLOD` @ 606eece)

Its own provenance statements are output-observation based (see licensing).
Useful files for the rewrite, by stage:

- **B1 terrain:** none for the mesher (AnvilLOD leaves terrain LOD to xLODGen).
  Underside: `AnvilLOD.Meshes\UndersideBuilder.cs`, `AnvilLOD.Core\World\UndersideMesher.cs`,
  `AnvilLOD.Plugins\UndersideStage.cs` / `UndersidePluginWriter.cs`.
- **B2 object LOD:** `AnvilLOD.Meshes\LodGenerator.cs` (block generation),
  `BtoBuilder.cs` (BTO layout "verified vertex-for-vertex against existing output",
  per-cell `BSSubIndexTriShape` segments, 65,535 splits), `MeshCleaner.cs`,
  `Transforms.cs` (REFR rotation convention), `LodInset.cs`, `ShaderGuard.cs`,
  `Nif\NifGeometryReader.cs` / `NifWriter.cs`; `AnvilLOD.Core\World\QuadBucketer.cs`,
  `LodGrid.cs`, `LodSettings.cs`, `TerrainHeights.cs` (hidden-face test against
  LAND heights, conservative); `AnvilLOD.Core\Lod\LodMeshResolver.cs` /
  `LodMeshIndex.cs` (MNAM + `_lod_N` naming + folder precedence), `LodRules.cs` /
  `RuleEntry.cs`, `ChildWorldCopies.cs`, `SeasonSwaps.cs`; `AnvilLOD.Plugins\ScanPipeline.cs`
  (stage order), `SeasonalLod.cs`, `LargeReferenceStage.cs`, `GrassLodSource.cs` +
  `AnvilLOD.Meshes\GrassPatchBuilder.cs` (a *different* grass-LOD model: one tuft
  per ground bin), `TreeLodGenerator.cs` + `AnvilLOD.Textures\TreeAtlasBuilder.cs`.
  Not provided: object texture atlas and UV remap ("planned"), FlatTextures
  billboards as DynDOLOD does them, DynDOLOD's HD/passthru/effect-shader special
  cases, any .NET-exact arithmetic.
- **Target 1 occlusion:** `AnvilLOD.Core\Occlusion\TvdtData.cs`, `OcclusionGrid.cs`,
  `OcclusionCalculator.cs` (429 lines).
- **Target 3 billboards:** the software rasterizer in `AnvilLOD.Meshes\Authoring`
  and `AnvilLOD.Plugins\Authoring` (optional reference).

Every port keeps "Copyright (c) 2026 CageTV", names the source file and commit,
marks modifications, and is listed in THIRD-PARTY-NOTICES.

## Stage B0: specs and quarantine (analyst sessions, may run now)

- **B0.1 Terrain LOD spec** → `docs/re/terrain-lod-spec.md`. Contents: BTR file
  format as observed in golden files and read by our own `nif` reader (block
  types, per-level vertex layout, segments, water shapes, multibound, z shift),
  the terrain input (what `crates/btr/src/export.rs` produces from LAND:
  heights, water, flags; define it as **our own in-memory structure**; the
  `LODGen_Terrain_*.bin` file format stays only as an optional import), the
  greedy-insertion mesher as the public-domain Terra 0.7 describes it
  (`D:\Modlists\ref\terra-0.7`: GreedyInsert, Heap, Map) with LODForge's
  requirements (per-level point budget / error threshold as **xLODGen UI/INI
  options** `[cfg]`, border and seam handling as observable in golden BTRs:
  shared edges between quads, skirts, flat cells), water planes and skirts as
  observed, the underside (DynDOLOD docs + golden `<World>_Underside.nif`), and
  the measurements an implementer needs (golden worldspaces, cell ranges,
  current timings). Quarantine: the LODGen-internal sentences of
  [baseline.md](baseline.md) "P2 terrain LOD meshes", the Terra/quad port notes.
- **B0.2 Object LOD spec** → `docs/re/object-lod-spec.md`. Contents: the export
  file and atlas map formats **from the MPL-2.0 writers we own**
  (`source.rs`, `atlas_build.rs`; plus DynDOLOD's extra keys as observed in the
  golden exports and documented at dyndolod.info), the `.lod` settings and LOD
  grid (public), reference transform composition (public convention; AnvilLOD
  `Transforms.cs`), LOD mesh selection (dyndolod.info: MNAM, `_lod_N`, folder
  precedence), rule semantics as documented at dyndolod.info and shipped in the
  rule files `[doc]/[cfg]`, BTO format as observed (golden + AnvilLOD
  `BtoBuilder.cs`), merging by material/shader key and the 65,535 limit, per-cell
  segments, hidden-face removal against terrain/water as documented
  (`RemoveUnseen*` INI keys) and observed, vertex colour handling as observed,
  UV atlas remap and ReUV as a functional operation (dyndolod.info "ReUV",
  `DynDOLOD_SSE_re-uv-miniatlas.txt` `[cfg]`), FlatTextures billboards as
  documented (Billboard rule levels, `.txt` sidecar fields), grass LOD in BTO as
  documented + golden (DynDOLOD grass golden, 2026-10-18), the objects height
  map as **our own format** shared with the occlusion stage, and the acceptance
  metrics (current tolerant parity numbers to compare against). Quarantine:
  [lodgen-318-delta.md](re/lodgen-318-delta.md) (whole file → pointer),
  [gap-audit-20261019.md](re/gap-audit-20261019.md) decompile sections, the
  `[lodgen-src]` "Side-file formats" paragraphs of the DynDOLOD spec (after
  re-verifying the side-file ordering from golden), the port sections of
  baseline.md. Coordinate with the occlusion scrub session before editing
  `docs/re/dyndolod-spec.md` (it edits the Occlusion section).
- **B0.3 Occlusion spec** → running (`docs/re/occlusion-spec.md`).
- **B0.4** Move the decompile folders under `D:\Modlists\ref\quarantine\` so the
  boundary is a path, not a list.

## Stage B1: terrain LOD meshes + NIF writer (fresh session)

Delete unread: `crates/btr/src/{terra,quad,geometry,underside,output,bin}.rs`,
`crates/nif/src/writer.rs`, `crates/nif/src/btr.rs`, the LODGen-default
constants in `crates/nif/src/lib.rs`. Inputs: `docs/re/terrain-lod-spec.md`,
Terra 0.7 sources, the NIF format specification, golden xLODGen BTRs
(`D:\Modlists\LODForge-golden\`), our nif reader (round-trip tests in
`crates/nif/tests/golden.rs` stay valid: they test reader+writer against real
files), AnvilLOD underside files. Build: Terra-based mesher with the per-level
budgets; quad/cell assembly, water, skirts; a NIF writer from the spec; BTR
assembly; underside. Validation: tolerant BTR comparison (triangle/vertex set
equality within tolerance, bounds), in-game terrain check (seams, water, LOD
transitions) on Tamriel and one small world. Record: before (1,755/1,760
byte-identical) and after.

## Stage B2: object LOD (fresh sessions, one crate, sequential sub-stages)

Delete unread: all of `crates/objlod/src` except `delaunay.rs`, `atlas_build.rs`,
`source.rs`; the LODGen-mirroring functions of `apps/cli/src/dyndolod_build.rs`
and `glow_atlas.rs`. Inputs: `docs/re/object-lod-spec.md`, the xEdit MPL-2.0
writers (format), AnvilLOD files listed above, golden xLODGen and DynDOLOD BTOs
and exports, DynDOLOD grass golden, the comparison tooling in `apps/cli`
(`e2e-compare`, the tolerant object-LOD comparison; check they are ours before
use — they are listed as such in the spec).

- B2.1 Export + atlas map parsing; reference → quad assignment; transforms;
  LOD mesh resolution; `Flavor` reduced to what the two export dialects need.
- B2.2 Mesh import (nif reader), shape/material keys, merging, segments, BTO
  writing (nif writer from B1).
- B2.3 Hidden-face removal against terrain (BTR of the same level) and water.
- B2.4 UV: atlas remap and ReUV (triangles re-cut along UV tile edges, UVs
  shifted into 0..1, duplicates merged, normals/tangents rebuilt).
- B2.5 FlatTextures billboards, grass LOD into LOD4 BTOs, vertex colour rules,
  HD/passthru/effect-shader handling as documented + observed.
- B2.6 Objects height map (own format) for the occlusion stage; glow atlas.

Validation per sub-stage: tolerant BTO comparison vs the xLODGen golden (ours
was 99.73% of triangles order-independent) and the DynDOLOD golden (98.4%
tolerant), grass golden (99.95%), then the in-game E2E comparison. Record the
new numbers; expect a drop and document it.

## Target 1: occlusion (scrub running; implementer next)

Inputs: `docs/re/occlusion-spec.md` (public docs, xEdit TVDT definition,
golden-verified layout, INI keys), AnvilLOD `Occlusion/*.cs` (port allowed),
golden `Occlusion.esp`, `Test-OcclusionParity.ps1`. Delete `occlusion.rs`
unread. Acceptance: TVDT equality count and visible-block delta per worldspace
recorded; in-game hole check in Tamriel and Solstheim. Expected: an
output-fitted model differs by a few percent of blocks (AnvilLOD reports 2–7%
depending on clearance).

## Target 3: billboard shader (running)

Functional spec and rules are in the running session's brief; AnvilLOD's
rasterizer is an optional reference. Acceptance: image tolerance comparison in
the region of the previous 64/127 at 20.95 dB; visual inspection.

## Grass generator (optional)

License-compatible under GPL-3.0-only. Keeping it means NGIO notices and a
recorded engine-behaviour re-expression; removing it (session running) means
`--grass bto` skips cells without an NGIO cache. Decide at merge time.

## Cross-cutting: license mechanics (after B1, B2, Targets 1 and 3 land)

- `LICENSE` = GPL-3.0 text; README "License" section in the style of AnvilLOD's
  LICENSE.txt ("why GPL-3.0-only", "not covered", third-party components).
- Headers: GPL notice on our files; CageTV copyright + modification marks on
  AnvilLOD-derived files; MPL-2.0 notices kept on xEdit-derived files; s-hull
  notice on `delaunay.rs`; NGIO notices if the generator stays.
- Cargo `license = "GPL-3.0-only"`; `THIRD-PARTY-NOTICES.txt` + `licenses/`
  including the Windows release dependency graph (Tauri/WebView2 included).
- Corresponding Source for binary releases (source ZIP of the exact commit +
  build instructions; dependency sources from crates.io, recorded).
- Repository published from a clean commit (history contains MPL-1.1-derived
  code and GPL combinations).
- Spec documents: quarantined material pointed to, remaining sections
  classified; `grep -rn "LODGen's"` reviewed for leftover internals.

## Sequencing

1. Now, in parallel: B0.1 and B0.2 analyst sessions; Target 1 scrub, Target 3
   and the grass removal are already running.
2. B1 (fresh) as soon as B0.1 lands; B2.1–B2.6 (fresh, one after another in one
   worktree) as soon as B0.2 lands; Target 1 implementer when its scrub lands.
3. Full build + in-game E2E comparison with everything merged; record numbers.
4. License mechanics, clearance checklist, release gate.
