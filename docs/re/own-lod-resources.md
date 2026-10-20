# Own LOD resources: dropping DynDOLOD Resources (2026-10-18, branch `own-lod-resources`)

Goal: `lodforge build --mode dyndolod` must not need the **DynDOLOD Resources SE** mod, and its
output must stay DynDOLOD-compatible, so public modlists keep running the stock DynDOLOD DLL NG
and scripts. This document has the inventory of what a build reads from Resources and from the
DynDOLOD install, the licensing constraints, the design of the replacement, and the first
prototype's numbers. Numbers come from `lodforge lod-sources` and `lodforge lod-models` (new) on
profile `LODForge-E2E`, Tamriel, Medium rules, with `LODForge E2E - DynDOLOD Output` left out.

## 1. Inventory

### 1.1 What the build reads from DynDOLOD Resources SE 3

The mod holds 4,823 NIFs, 1,231 DDS, 9 scripts (+ sources), `Interface\DynDOLOD\DynDOLOD.dds`
and `meshes\DynDOLOD\lod\version.ini`.

Object LOD (the `LODGen_SSE_Export_Tamriel.txt` columns, `lod-sources`):

| Columns (references) | LOD4 | LOD8 | LOD16 |
|---|---|---|---|
| LOD model | 33,560 | 19,112 | 14,674 |
| Full model | 23 | 22 | 0 |
| Billboard (`.dds`) | 636 | 636 | 636 |
| None | 3 | 14,452 | 18,912 |

| Origin of the LOD model | Models | LOD4 refs | LOD8 refs | LOD16 refs | Placed LOD4 triangles |
|---|---|---|---|---|---|
| **Resources** | 1,071 | 31,384 (93.5%) | 12,411 (64.9%) | 8,535 (58.2%) | 8.68 M (94.0%) |
| Vanilla BSAs | 334 | 1,885 (5.6%) | 6,393 (33.5%) | 5,831 (39.7%) | 0.47 M |
| Other mods | 41 | 291 | 308 | 308 | 0.08 M |

So Resources supplies almost all of LOD4 and about 60% of LOD8/16. By category (LOD4 refs):
mountains 7,820, rocks 5,610, nordic exterior ruins 3,672, imperial ruins 2,869, clutter 2,186,
farmhouses 1,902, imperial ice 1,491, dirt cliffs 1,377, tundra 712, then shack kit, Dwemer,
Whiterun, ice and smaller sets. 1,655 of the 1,971 bases with object LOD use at least one
Resources level, 1,519 use nothing else; their full models are vanilla (731), other mods (911,
563 of them PGPatcher copies of vanilla meshes) or Resources itself (13).

How Resources' LOD models relate to the full models (median triangle share, per category):
level 0 about 8% of the full model (mountains 6%, rocks 6%, farmhouses 6%, imperial ruins 8%,
ice 32%), level 1 2-8%, level 2 0.6-4%. Many architecture models exist only as `_lod_2`, which
fills levels 0-2 (e.g. `whpalace1_lod_2.nif`, 204 triangles for an 18,009-triangle palace).
They mostly reference the **full models' textures** (`textures\landscape\mountains\*`,
`textures\dungeons\imperial\*`, ...); 297 of their 393 distinct textures are vanilla, 86 come
from other mods, 10 from Resources, plus TexGen-made `textures\lod\*lod.dds` mixes.
Other variants in use: 24 snow `_lod_Nh/l` models (4,396 LOD4 refs, all Resources), 29
`passthru` models (165 refs), 4 CRC-named models (38 refs).

Without Resources in the load order the same export has **14,838** reference lines instead of
34,222 (the catch-all `\` rule is VWD: a base without any LOD model level gets no object LOD),
97.7% of the remaining LOD4 models from vanilla BSAs.

Dynamic LOD (`DynDOLOD_NG_Tamriel.txt` `[Objects]`, 2,958 references):

| Model the copy shows | References | From Resources |
|---|---|---|
| `_dyndolod_lod.nif` | 488 | all 12 models: water1024, campfire01(land)burning, giantcampfire01burningnoland, slighthousefire, fxwaterfallbody{slope,tall,tall02}, 4 CRC-named fxwaterfallthin |
| LOD level 0 model | 2,195 | 1,239 refs / 152 models (Nature of the Wild Lands 1,428 refs, vanilla 16) |
| Full model | 275 | – |

Without Resources: 1,900 dynamic references (275 full, 1,625 level-0 models), so 1,058 lose
their dynamic LOD.

Scripts: DynDOLOD.esp's VMAD names `SHESON_DynDOLOD_{Master,ObjectActivator,MCM,Quest,Alias}`;
all five ship in the **DynDOLOD DLL NG** mod (which wins over Resources' older copies).
Resources' extra `Firstborn`, `LODObject` and `Worshipper` scripts are not used by our plugins.
`Interface\DynDOLOD\DynDOLOD.dds` (MCM logo) and `version.ini` are not read.

### 1.2 What the build reads from the DynDOLOD install (`tools\DynDOLOD`)

All under `Edit Scripts\DynDOLOD\` unless noted. Text is DynDOLOD's own (CC BY-NC-SA); the NIFs
are DynDOLOD assets.

| File(s) | Used by | For |
|---|---|---|
| `DynDOLOD_SSE.ini` `[DynDOLOD]` | export, atlas, plugins, occlusion | `Folder1..5`, `IgnoreLODMesh`, `IgnoreBaseEDID`, `IgnoreTexture`, brightness, `FlatLOD*`, `TreeLODMaxTilt`, `NearGrid`/`FarGrid`, `OcclusionMode*`, `RemoveUnseenZShift`, ... |
| `Rules\DynDOLOD_SSE_all.ini`, per-plugin `*.ini`, `_all_dynamic`/`_all_static`, `DynDOLOD_SSE_{Low,Medium,High}.ini` (+ `Data\DynDOLOD\*` overrides) | rules | the mesh-mask / reference rule table (159 rules for Medium) |
| `Configs\DynDOLOD_SSE_misnamed.txt` | discovery | LOD files to ignore |
| `Configs\DynDOLOD_SSE_xesplod.txt` | dynamic LOD | enable parents treated as none |
| `Configs\DynDOLOD_SSE_childworld_*.ini` | export | child-world `Bounds` (trees to object LOD) |
| `Configs\DynDOLOD_SSE_flat_normals.txt` | FlatTextures | billboard normals file (path written into the export, LODGen reads it) |
| `Configs\DynDOLOD_SSE_{TexGen,Required}_Billboards_<plugin>.txt` | TexGen | tree billboard candidates |
| `DynDOLOD_flat_4x2alt1_lod.nif` (and the other 8 `DynDOLOD_flat_*` / `LODGen_flat_lod.nif`) | grass `bto` | grass billboard template |
| `Render\Skyrim\Replace\*_static.nif` (12) | TexGen | static stand-ins for skinned trees |
| `Render\Skyrim\Billboards\**\*_trunk.nif` (119) | TexGen | trunk models for rendered billboards |

Not read: `mesh_lookup`, `mesh_replace`, `mesh_crc32`, `default_lod`, `mod_default_lod`,
`textures_*`, `TexGen_{alpha,noalpha,copy}` (the stitched-texture lists belong to the TexGen
session), `Shaders\`, `Presets\`.

### 1.3 Object LOD vs dynamic LOD needs

- **Object LOD** needs per base: model levels 0, 1, 2 (`_lod_0/1/2`, or `_lod` for all) for the
  rule columns; a LOD file only has to exist at build time (LODGen bakes it into the BTO).
- **Dynamic LOD** needs, at game runtime, the model the DynDOLOD.esp STAT copy points to:
  `_dyndolod_lod.nif` when one exists (keeps animation and effect shaders, e.g. waterfalls,
  fires, water planes), else the level-0 LOD model, else the full model. These files must ship
  in the output Data folder.

## 2. Licensing

- DynDOLOD Resources assets (and DynDOLOD's own NIF/text assets in the install) are **CC BY-NC-SA
  4.0** (`docs\DynDOLOD-LICENSE.txt`). LODForge never bundles, copies, converts or derives from
  them. The generator's only geometric input is the full model the load order uses; Resources'
  files are read solely by the measurement commands (`lod-sources`, `lod-models
  --reference-mod`) to count triangles and coverage, never as an input to generation.
- The selection policy and level budgets were tuned against Resources' aggregate numbers (share
  of triangles, which sizes get LOD); no per-model data from Resources is stored or shipped.
- meshoptimizer (used through the `meshopt` crate) is MIT licensed.
- DynDOLOD's rules and configs are its text. LODForge keeps reading them from the user's install
  as an optional source. To drop that dependency LODForge needs its own:
  - **rule preset** (mask → LOD4/8/16 columns, grid, reference action, flags) in a LODForge
    format (TOML), written from scratch from the documented semantics
    (docs/re/dyndolod-spec.md "Rules and mesh masks"), not by converting DynDOLOD's files;
  - defaults for the `[DynDOLOD]` keys the build uses (folders, ignore lists, brightness, grids,
    occlusion modes) as LODForge settings;
  - its own `flat_normals` data (a normals file for billboards; LODGen's built-in
    `billboard_nif` path already covers the case without a NIF template);
  - its own grass billboard template (procedural crossed planes; see 3.5);
  - TexGen stand-ins for skinned trees made from the tree's own skin bind pose
    (`crates/nif` already reads skin data) instead of `Replace\*_static.nif`, and trunk models
    cut from the full tree model instead of `Render\Skyrim\Billboards`.

## 3. Design

### 3.1 Which bases get generated models (`lod_models::plan`)

For every non-deleted reference in the selected worldspaces:
1. Skip trees (billboards / tree LOD), `IgnoreBaseEDID` bases, markers (`marker` in the file
   name or a `\markers\` folder: invisible in game) and `\effects\` models (animated or
   effect-shader meshes belong to dynamic LOD).
2. Take the mask rule that would apply if every model level existed
   (`select_mask_rule` with all `LevelN` available). Its LOD4/8/16 columns name the model levels
   the base would use.
3. A level is generated when discovery finds no LOD model for it (fill mode; `--all` generates
   every level for comparisons) and the base is large enough: largest OBND extent times the
   largest reference scale >= 256 units for level 0, 512 for level 1, 1024 for level 2
   (`--min-size` / `--lod-model-min-size`). Smaller objects only get the levels they can be
   seen at; LODGen's level fallback then leaves LOD8/16 empty for them, as Resources does.

Coverage against Resources' choices (planned models that Resources has a LOD model for,
weighted by references): Resources covers <10% of references of objects under 128 units, about
a third at 256-1024 and 90% at 2048-4096. No geometric rule (size, height, volume) separates its
hand-picked set cleanly; size >= 256 keeps 19.7 k of Resources' 22.5 k covered placements and
adds 13.8 k placements of debris Resources leaves out (driftwood, branch piles, stumps, logs,
raised platforms).

### 3.2 Generation (`crates/lodmodel`)

Per level (model level 0, 1, 2):
1. **Collect** visible, non-skinned shapes under the root with node transforms applied; skip
   hidden nodes, editor markers, all but the first child of switch nodes, effect-shader shapes,
   decals (`SLSF1` decal / dynamic decal), alpha-blended shapes that are not alpha-tested, and
   shapes without a lighting shader.
2. **Merge** shapes with identical shader, texture set and alpha property.
3. **Weld** exact duplicate vertices (same position and attributes) so they are not seams.
4. **Drop small parts**: connected parts (through shared or coincident vertices) with a bounding
   diagonal under twice the level's error limit; the largest part always stays.
5. **Simplify** all materials together with meshoptimizer's attribute-aware edge collapse
   (normals and UVs weighted, absolute error), permissive across UV / normal seams but with
   material borders protected; then a triangle cap as a share of the source. meshoptimizer only
   removes vertices, so UVs, normals, tangents and vertex colours are the source's values
   (atlas-friendly as far as the source is: LODGen atlases untiled textures and keeps tiled ones,
   exactly as for the full model).
6. **Write** one BSTriShape per material with the source's BSLightingShaderProperty, texture set
   and NiAlphaProperty under a BSFadeNode root, identity transforms.

Default limits (`lodforge_lodmodel::LEVELS`): absolute error 32 / 56 / 140 units, cap 30% / 15% /
8% of the source triangles. LOD4 starts about 2.5 cells out (uGridsToLoad 5), where a pixel at
1080p covers roughly 15 units, so 32 units is about two pixels at the nearest LOD4 distance.

Things meshoptimizer's own options did badly here: `Prune` removed whole buildings and roofs
under absolute error (component error is not the component's size), and without welding and
permissive seams many models stalled at 20-40% of the source because every UV seam and unwelded
duplicate blocks collapses.

### 3.3 Naming and discovery

Generated files go to the build's output Data folder as
`meshes\lod\lodforge\<folder of the full model below meshes>\<stem>_lod_<N>.nif`, or
`<stem>_<CRC32>_lod_<N>.nif` where several different full models share a file name (PGPatcher
copies, mod replacers). That is under DynDOLOD's `Folder4=meshes\lod` and keyed by bare file
name, so DynDOLOD's own discovery (and LODForge's port of it) picks them up unchanged: numbered
levels, level fallback, CRC preference. Fill mode adds a level only where discovery found
nothing, so a `_lod_0` next to a vanilla `_lod.nif` keeps the vanilla file for the other levels.
They are in the output (not only a cache) because dynamic LOD copies point at level-0 models at
runtime. A cache (`.lodforge-cache` next to the output, keyed by full-model CRC32, level,
parameters and `GENERATOR_VERSION`) makes rebuilds near free.

### 3.4 Dynamic LOD models (`_dyndolod_lod`), not built yet

The 12 Resources `_dyndolod_lod` models are effect meshes: waterfalls (animated UV effect
shaders), fires (particles, emissive), a water plane. Plan:
- copy the full model's node tree, drop collision (`bhk*`), sounds, lights and havok extra data,
  keep NiControllers on effect shaders; simplify lighting-shader geometry as for level 0;
- name `<stem>[_<CRC32>]_dyndolod_lod.nif` under `meshes\dyndolod\lod\lodforge\...` (Folder5);
- needs `crates/nif` to rewrite block references for blocks it keeps raw (controllers,
  particle systems); until then those 488 references fall back to the level-0 model or full model
  (`build_dynamic`), which the DLL handles the same way.

### 3.5 Billboard templates

`DynDOLOD_flat_4x2alt1_lod.nif` (grass, 4 planes) can be replaced by a procedurally built
template with the same plane layout, written by LODForge to its work folder; `objlod::flat`
already builds the 2-plane tree billboard NIF itself when no template is given.

## 4. Prototype status and numbers

Tamriel on LODForge-E2E without Resources, `build --mode dyndolod --texgen --lod-models` (82 s):
2,827 levels for 1,614 full models; DynDOLOD's (with Resources) LOD4 / LOD8 / LOD16 references
that have a model: 92.0% / 96.7% / 93.5% (without generation: 14,838 of 34,250 lines). Object LOD
triangles 10.2 M / 4.2 M / 1.7 M vs 5.6 M / 2.9 M / 0.8 M for DynDOLOD; per model, against
Resources' own models weighted by references, 1.59x / 0.63x / 0.77x. Full table in
docs/baseline.md "Own LOD resources".

## 5. Next steps

1. Mountains: drop or handle the moss/snow overlay layer of full mountain models in generated levels, and generate levels 1-2 for mountains instead of falling back to vanilla snow-baked `_lod` meshes (docs/baseline.md "Clean in-game comparison"). Whiterun in Tamriel needs child-world copies (MVP gap, not this branch).
2. Selection: a debris rule (low, flat models; `landscape\trees\` non-tree statics) and enable-
   parented Hearthfire / Civil War parts, to get closer to Resources' 34 k lines.
3. Coarser architecture: a box/hull proxy level (silhouette-preserving, e.g. voxel or convex
   decomposition) for city buildings that Resources ships as `_lod_2` only.
4. `_dyndolod_lod` effect models (3.4), then the DynDOLOD-install pieces (2): rule preset,
   billboard template, TexGen stand-ins.
