# Gap-closure plan

Based on the [post-merge validation](validation-20261019.md), not the old
pre-atlas inventory. Goal: reproducible DynDOLOD-compatible CS/PBR output.
This is the implementation sequence. The original validation patch changed no
generation behavior; subsequent fixes are tracked below.

## Current progress

The latest [full validation](validation-full-20261019.md) confirms the measured
runtime gains and records rebuild variation plus distant terrain/glacier flicker.
The acceptance target is **renderable, visually consistent, save-compatible
output**, not exact DynDOLOD grass triangles or byte-identical rebuilt assets.
Prioritize the flicker and an actual pre-rebuild-save/post-rebuild-output test.
Retain geometry/hash differences as diagnostics; they block release only when
they expose a format, visual, coverage, runtime-identity or save regression.

- **1a closed in the working tree:** PBR darkening now requires the source RMAOS
  companion, rather than just a `linear.dds` request. Snow no-alpha RGB PSNR rose
  from 10.197 to **48.727 dB**; the other 46 linear textures are byte-identical.
- **1b partially closed:** load-order-wide enable-parent lookup and whole-chain
  initial-state conditions remove all **32** matched-row `InitiallyEnabled`
  mismatches. Original selection and all other compared runtime fields are
  unchanged. Missing parents and cycles are reported as export errors.
- **1b base-section plumbing implemented:** global `[Bases]` now contains **510/593**
  reference identities, with **zero extras**, for emitted large-reference object
  LOD. Runtime/configuration gating and stable full/light-plugin IDs are tested.
  The remaining **83** identities have no safe generalized selector yet.
  Independent follow-up rejected broadening to all emitted or large dynamic
  bases because it introduces extras.
- **1b selection advanced:** full-model fallback and persistent-full-LOD
  migration recover **84 originals** (3,042/3,078, zero extras); native MNAM aliases
  restore four object placements. Seven extra classic
  trees now use dynamic LOD; classic placements exactly match all 51,566 golden.
- **Integrated qualification:** [full build and live checks](validation-runtime-20261019.md)
  preserve all prior object/grass geometry and validate representative parent and
  child-world transitions. Shutdown needed a separate successful retry; this is
  not all-family runtime certification.
- **1b replacements qualified:** 19 full-model replacements recover 18 golden
  source originals; `[Disabled]` is now **19/39**, zero extras. ESM `ONAM`
  registration fixes a live duplicate-fan bug that record-only checks missed.
  Unpaused parent toggles, loaded-grid/distant fan state and wheel save/reload
  pass. [Scope and remaining patch mismatch](validation-runtime-20261019.md#replacement-follow-up).
- **1b reset selection closed on the fixture:** **57/57, zero extras**.
  Matched child originals now retain source-world overrides; explicit
  Delete/Disable/Replace rules take precedence.
- **1b timing advanced:** NearLOD TREE persistence no longer causes Delay:
  **33 mismatches fixed, zero regressions; 352 remain**. The combined
  [batch qualification](validation-runtime-batch-20261019.md) preserves all
  geometry/material assets and tests an upgraded save plus Solitude transitions.
- **Flicker candidates built (branch `terrain-mips-grass-atlas`):** the two
  output-side causes the 2026-10-19 texture audit found are fixed and measured in
  the [baseline](baseline.md#terrain-lod-mips-and-grass-billboards-on-the-alpha-atlas-2026-10-19-branch-terrain-mips-grass-atlas):
  terrain LOD textures now carry full mip chains at every level and kind (before:
  LOD4 diffuse only, the golden xLODGen settings; `--terrain-mips golden` keeps
  them for parity runs), and BTO grass billboards are placed on the `_Alpha` atlas
  like the tree billboards instead of being referenced raw with one mip. These are
  the first two candidates to re-test in game; the flicker is **not** claimed fixed.
- **Next:** re-test the flicker in game with the mipped terrain and atlased grass,
  then qualify rebuilt-output save compatibility; continue runtime fixes and
  functional grass checks independently.

Commands, scope and limitations are in the
[gap-closure baseline](baseline.md#gap-closure-2026-10-19).

## First: preserve a useful test boundary

Keep the pinned run and exact commands. Use the
[output validator](../apps/cli/examples/validate_output.rs) for missing assets,
shader inventories and DDS checks, and the existing E2E command for geometry.
Add small synthetic fixtures alongside each fix. Require unchanged non-PBR and
xLODGen controls. Do not fit a fix solely to Tamriel counts or page-wide PSNR.

## Work packages

| Order | Package | Main code owners | Dependency / exit criterion |
|---|---|---|---|
| 0 | Rebuild save compatibility and temporal visual triage | [build](../apps/cli/src/build.rs), [validation](validation.md) | Load a pre-rebuild save with the rebuilt package; preserve IDs/token and validate enable parents, replacements, resets, world transitions and repeat save/reload. Asset variation is acceptable without functional/visual regressions. Reproduce reported flicker with consecutive frames before assigning a cause. |
| 1a | Snow no-alpha linear conversion | [pbr_linear](../apps/cli/src/pbr_linear.rs), [TexGen stitching](../apps/cli/src/texgen_stitched.rs) | Independent. Explain 58.27 dB upstream vs 10.20 dB downstream; match reference transform on opaque/alpha/no-alpha synthetic inputs and the snow golden. Preserve other linear copies. |
| 1b | Runtime DLL sections and dynamic selection | [dynplugins](../crates/dyndolod/src/dynplugins.rs), [export](../crates/dyndolod/src/export.rs), [build wiring](../apps/cli/src/dyndolod_build.rs) | Independent of texture fixes. Preserve the 57/57 reset fixture; finish Bases/Disabled selection, 18 missing source originals, four generated patch originals and measured field mismatches. Validate referenced forms and in-game state changes. |
| 1c | Grass rendering, coverage and consistency | [TexGen CLI](../apps/cli/src/texgen_billboards.rs), [texgen](../crates/texgen/src), [grass preparation](../apps/cli/src/dyndolod_build.rs) | Independent bounded branch, coordinating build-wiring edits with 1b. Valid assets/shader data, intended type coverage/density, consistent rebuilds and acceptable near/far transitions. Exact golden triangle overlap is diagnostic, not an acceptance threshold. Trace framing only where it explains a visible or functional defect. |
| 2a | Alpha mip semantics and tree packing | [object_atlas](../apps/cli/src/object_atlas.rs), [tree stage](../apps/cli/src/tree_lod.rs), [treelod](../crates/treelod/src) | Keep merged page selection/formats intact. Compare corresponding tiles after re-UV, not packed pages. Reproduce reference alpha evolution, edges and rectangular tree atlas; test all eight mips and tiny/empty tiles. |
| 2b | Material fidelity and shader overrides | [AltTextures](../crates/objlod/src/export.rs), [shape/build](../crates/objlod/src), [base copies](../crates/dyndolod/src/dynplugins.rs) | Coordinate with 1b for plugin edits. Add 32-field overrides including shader-specific data/hash; trace shared STAT DNAM angle/snow/ash remaps; classify remaining HD/VC/soft-light differences by shape identity. |
| 3 | Effective settings and world policies | [config](../apps/cli/src/dyndolod_build.rs), [rules](../crates/dyndolod/src/rules.rs), [build](../apps/cli/src/build.rs), [UI](../apps/ui/src/main.rs) | Establish precedence for shipped INIs, presets, CLI and mod overrides; report effective values and unsupported requested features. Tests must change output for non-default settings without changing default controls. |
| 4 | Complex/Ultra tree and remaining CS paths | [assign](../crates/dyndolod/src/assign.rs), [tree stage](../apps/cli/src/tree_lod.rs), [TexGen scene](../crates/texgen/src/scene.rs) | Depends on settings contract and alpha fixtures. Complex normal atlas + metadata, Ultra templates/rules/blanking, PBR slot redirect for unpatched inputs. Explicit opt-in until validated. |
| 5 | Release qualification | [validation procedure](validation.md), test fixtures and runtime automation | Depends on supported package completion. Default + four seasons, representative DLC/child worlds, both grass modes, fresh initialization, save/reload and rolling updates; publish any exceptions. |

Packages 1a/1b/1c can be split between developers only with explicit file ownership.
Avoid a large simultaneous rewrite of the CLI pipeline.

## How to close each investigation

### Runtime compatibility (1b)

1. Follow the DLL parser/spec for section location and field meaning. `[Bases]`
   belongs in **Worlds.txt**, not each world file. Gate BaseLODDLL by the supported
   runtime/configuration instead of assuming all installs need the same section.
2. Match by original plugin + local ID, preserving duplicate golden rows for
   diagnosis. Doors, rapids and enable-parent full-model fallbacks are recovered.
   Windmill/water-wheel replacement relationships and their Disabled rows now
   have representative live coverage. Next investigate originals absent from
   the Tamriel source map. Implement patch application separately: Katla's fan
   uses a generated original identity and corrected transform in the golden.
   Do not hide that mismatch by excluding its unpatched source or fitting counts.
3. Add focused cases for Delay, InitiallyEnabled, destruction index/type,
   Level/LargeReference, Disabled and Reset transitions. Current fixed fields
   are not all wrong: change only behavior supported by evidence.
4. Test fresh and retained-ID builds; verify no missing forms after reload,
   disabled/changed objects don't leave stale LOD, and child-world transitions
   actually switch the intended references. A successful DLL startup is necessary
   but insufficient.

### Grass and mip quality (1c / 2a)

Grass comparison reads stored BTO vertices, not animated shader output. Runtime
sway cannot explain its offline triangle mismatch. Cached placements at 100%
density bypass LODForge's random thinning; the measured grass triangles are
identical across its own rebuilds. Different billboard geometry can still make
golden overlap low without implying missing or unrenderable grass.

1. Rebuild a small pinned grass tile with golden sidecars, then own sidecars,
   keeping placements fixed. Compare dimensions, ShiftZ/Depth, view yaws,
   normal maps and complex flags before changing density or RNG.
2. Resolve the missing/excluded type inventory, including vanilla and complex
   grass. Keep NGIO cache correctness separate from cacheless collision work;
   do not use generated unfiltered placements as a parity substitute.
3. For mip tests, extract the same source tile through each atlas map. Check
   alpha >=128 coverage, transparent RGB and normal directions at each mip.
   Establish tolerances from repeated reference runs and compression noise.
4. Preserve page size/format improvements. Coverage preservation is an intentional
   approximation today; reproduce the reference AlphaFactor behavior rather
   than treating the current implementation as finished.

### Materials/settings (2b / 3 / 4)

- Shader override fixtures: ten texture fields, full 32 fields, missing/invalid
  fields, type changes and passthrough merge hash. Also cover the lower-priority
  non-passthrough grass slot-clearing/LOD-flag path.
- Resolve directional material forms through plugin masters. Byte comparisons
  of DNAM without identity remapping can be misleading.
- PBR slot redirect fixtures: patched mesh, unpatched mesh with installed PBR
  twins, absent twins and non-PBR control. Trace emissive/shape-filter behavior
  separately before implementing PS2/PS4 from hypotheses.
- Settings fixtures: world exclusions/partial records, child policies, rule edits,
  BOS, season eligibility and non-default TexGen scales. Surface unsupported
  features rather than silently accepting keys.
- Keep optional fake lights/CS light export and standalone resource replacement
  outside the default-parity milestone unless explicitly enabled in its matrix.

## Completion policy

For every package, attach the commit, input hashes, fixture results and before/after
measurements to the [baseline log](baseline.md). Update [parity](parity.md) only
after verification. New generated missing assets or structural errors block a
merge. Material/runtime regressions block release even if geometry counts improve.

The two current screenshots are smoke evidence, not visual thresholds. Broaden
camera/lighting coverage and fix the reload helper's missing-cell-event assumption
in runtime test tooling before making it a fully unattended gate.
