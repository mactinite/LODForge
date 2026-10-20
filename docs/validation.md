# Validation

There are three different claims: the LODGen port consumes exports correctly,
LODForge generates equivalent inputs/outputs itself, and the result behaves
correctly in game. Passing one does not establish the others.

## 1. Freeze inputs

Record commit, Rust/tool versions, GPU/driver, profile/load order, PGPatcher output,
DynDOLOD/TexGen configuration and preset, worldspaces, seasons and grass mode/density.
Keep reference output separate from LODForge output.

Copy reference exports **with all side files** and make their absolute paths
resolve to the pinned copies. `AltTextures`, `VC`, `FlatTextures`, terrain data and
atlas maps in the installed tool's export directory are overwritten by later runs.
The [baseline log](baseline.md) documents a misleading 33.59% vs 98.44% result
caused by reading side files from a different run.

For a clean E2E comparison, exclude old LOD and TexGen output mods explicitly.
Build into a fresh directory. For rolling-update tests, deliberately reuse a
backed-up output and record that distinction.

## 2. Run code checks and structural comparison

```powershell
.\scripts\Check.ps1 -Check Test -Package lodforge-objlod,lodforge-dyndolod,lodforge-texgen,lodforge-treelod
cargo build --locked --release -p lodforge
.\target\release\lodforge.exe e2e-compare `
  --instance 'D:\Modlists\skyrim-cs' --profile 'LODForge-E2E' `
  --exclude-mod 'LODForge E2E - DynDOLOD Output,LODForge E2E - TexGen Output,LODForge E2E - LODForge Output' `
  --ours 'D:\LODForge-output\trial-01' --golden 'D:\LODForge-golden\pinned-output' `
  --world Tamriel --tolerant --json 'D:\LODForge-output\comparison.json'
```

Substitute your actual paths and output mod names. `e2e-compare` is a diagnostic
report, not a full certification gate. Inspect the result; its exit status does
not enforce all acceptance criteria below. Lower-level `object-lod`, `tree-lod`,
`dds-diff`, `nif-scan` and `plugin-dump` commands help isolate differences
(`--help` documents their arguments).

### Offline asset/material validator

The [validate_output example](../apps/cli/examples/validate_output.rs) adds
NIF geometry-invariant checks, referenced texture/model resolution, plugin-master
presence, DDS header/payload checks, shader inventories and atlas mip coverage.
It exits nonzero for structural errors or missing referenced assets, after writing
its JSON report. Input/archive details are included in the report.

```powershell
cargo build --locked --release -p lodforge --example validate_output
.\target\release\examples\validate_output.exe `
  --instance 'D:\Modlists\skyrim-cs' --profile LODForge-E2E `
  --exclude-mod 'LODForge E2E - DynDOLOD Output,LODForge E2E - TexGen Output,LODForge E2E - LODForge Output' `
  --root 'D:\LODForge-output\trial-01' --reference 'D:\LODForge-golden\pinned-output' `
  --json 'D:\LODForge-output\validation.json'
```

For reference validation, use `--asset-root` to supply that reference's
separate TexGen output. Do not add old output roots to validate a standalone
LODForge build: they could conceal missing generated assets. A deliberately
sparse runtime overlay must instead declare its pinned full-output dependency
with `--asset-root`; report it as an overlay, not a complete generation.
The example is intentionally narrower
than full plugin/form validation; see the [measured scope](validation-20261019.md).
Whole-page alpha coverage is diagnostic when atlas layouts differ; compare
corresponding tiles before drawing per-material conclusions.

## 3. Acceptance checklist

These are required checks to implement/run, not a claim that the existing
comparison command automates all of them.

- **Assets:** resolve every generated NIF/TXST texture and model reference in the
  final runtime VFS. Required generated assets must exist; missing input assets
  must be reported and accounted for. No dependence on disabled old output.
- **Geometry:** compare shape groups and full attributes as well as positions,
  at LOD4/8/16 (and LOD32 when supported). Separate grass from non-grass.
  Record tolerance and both numerator and denominator.
- **Materials:** compare shader type/flags, passthrough behavior, snow/ash/HD masks,
  texture slots, UV/clamp state, vertex color and alpha properties.
- **Textures:** verify DDS format, color-space interpretation, dimensions and mip
  count; compare alpha coverage across mips and normal maps separately from albedo.
  Geometry equality is not a substitute for this.
- **Runtime data:** resolve all plugin/form references; compare relevant record
  fields and per-world DLL sections, not only record counts. Check TVDT with the
  same terrain/object height maps and plugin inputs.
- **Seasons/worlds:** compare the same selected worlds and season eligibility,
  including empty outputs that suppress vanilla LOD.
- **Grass:** compare BTO to BTO using the same cache and density. Test Grass
  Optimizations separately. `auto` can generate unfiltered placements where
  caches are absent.
- **Performance:** time equivalent inputs/features on the same machine; record
  warm/cold cache, wall time, peak memory and output size. Historical speedups
  are profile-specific, not general guarantees.

## 4. In-game matrix

Use a disposable profile/save and the intended Skyrim, SKSE, DynDOLOD DLL NG,
Community Shaders, PBR and seasonal runtime versions.

1. Test clean initialization, plugin masters and DLL logs. On a fresh `coc` game,
   wait for the Survival Mode prompt, inspect its text/buttons, and choose **No**
   through `game_menu` (`MessageBoxMenu`, callback `buttonPress`, argument `1`
   when the labels are `Yes`, `No`). Do not merely hide the menu. Require
   `paused=false`, no blocking menus, and advancing game time before traveling,
   measuring runtime transitions or taking comparison screenshots. Recheck
   after loads; do not dismiss unrelated prompts blindly.
2. Inspect mountains, snow/ash, city outskirts, water/glow, trees and grass at
   near/mid/far LOD distances in matched lighting/weather. Prevent combat during
   travel as well as captures in disposable profiles; confirm the player is
   alive, out of combat and at the intended position. God mode alone does not
   prevent attacks/stagger. Keep the harness's tracked AI state synchronized:
   ad-hoc console toggles are not reflected in its cached scene state.
   With `game_screenshot`, `preset=clean, restore=false` keeps its AI controls
   applied between shots. Recheck blocking menus and combat after loads/travel.
3. Move the **player** under the comparison camera. A distant free camera alone
   does not load the same LOD grid and can create false "missing terrain" findings.
4. Exercise parent/child transitions, fast travel, enable-parent/destruction
   changes and large-reference transitions. For replacements, check both the
   original and generated reference: a loaded copy alone does not rule out
   duplicate geometry. ESM temporary overrides need TES4 `ONAM` registration;
   successfully parsing their REFR records offline does not prove Skyrim uses them.
5. Repeat default/SPR/SUM/AUT/WIN and representative DLC/child worldspaces.
6. Save/reload; rebuild with retained IDs/token and test reload again.
   Test fresh identity only with a fresh save. Never promise save safety from
   byte-identical plugin output alone. Compare the **full** rebuilt package,
   not only plugins/data: the [full validation](validation-full-20261019.md)
   found geometry variation hidden by byte-identical runtime files. Variation
   alone is not a failure: load a pre-rebuild save against the complete rebuilt
   package, check runtime identities/state and visual transitions, then save and
   reload again. Define the supported upgrade boundary; unchanged-input testing
   does not certify arbitrary modlist or load-order changes.

Temporal flicker needs consecutive frames/video with stable player/camera state.
Still-image similarity and a few separated screenshots cannot certify its
absence. Keep engine captures separate from presented-frame/HDR comparisons;
record frame-generation state, capture source and any fallback.

Attach screenshots, logs and exact settings to the result. A single successful
DLL initialization is useful evidence, not full runtime certification.

### Driving the game from an agent (skyrim-mcp)

The repository's [`.mcp.json`](../.mcp.json) registers the `skyrim-mcp` server
(`D:\workspace\SkyrimMCP\server\dist\index.js`, run with Node 22). Claude Code asks
once to approve project servers. The server provides:

- **MO2:** `mo2_list_profiles`, `mo2_set_mod_enabled`, `mo2_create_test_profile`;
- **game control:** `game_launch`, `game_load`, `game_console` (`coc` / `cow` to put the player
  under the camera), `game_move`, `game_quit`;
- **capture and comparison:** `game_screenshot` and `image_compare` (SSIM).

Use `game_screenshot source=frame preset=clean` for LOD comparisons. Community Shaders HDR
clips window grabs (see docs/baseline.md "Clean in-game comparison").
