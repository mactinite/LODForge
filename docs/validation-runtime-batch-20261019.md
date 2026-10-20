# Runtime batch: child resets and tree timing

Working-tree follow-up to `b4870a1`, using the existing golden and
[replacement qualification](validation-runtime-20261019.md#replacement-follow-up).
No new reference generation or full-parity claim.

## Results

| Check | Before | After |
|---|---:|---:|
| Golden reset identities matched | 38/57 | **57/57, zero extras** |
| Delay mismatches, preserving duplicate golden rows | 385 | **352; 33 fixed, zero regressions** |
| Dynamic source originals selected | 3,061 | 3,061, same IDs |
| Global Bases | 510/593 | Unchanged; remaining selection policy unresolved |
| Disabled / Markers | 19/39 / 865/877 | Unchanged |

**47 focused tests pass**, one opt-in GPU test ignored. Release build and focused
Clippy pass with existing advisories. Retained-ID regeneration is byte-identical
for both plugins and both NG data files.

### Child-world resets

The missing 19 originals are persistent Is Full LOD trees in SolitudeWorld.
Their known Tamriel counterparts already receive LOD, so they are excluded from
child copies but still need their full-LOD flag cleared and a runtime reset.
The writer now retains these originals in their **source world's persistent
cell**, merging with existing world groups rather than creating duplicate groups.
Large references use the ESM; other originals use the ESP.

Selection requires an emitted parent counterpart and a known child/parent match.
Explicit reference actions take precedence. The first comparison found two
extras with Delete rules; respecting those rules, rather than excluding IDs,
produced the exact golden set. Tests cover this boundary, unselected parents,
nonpersistent originals, large/non-large placement, missing source cells,
disabled workarounds and stable rebuilds.

All 19 added overrides match the golden's record flags, subrecord values and
world/persistent-cell grouping. This closes this fixture's reset-selection gap,
not all child-world export policies.

### Near-grid tree Delay

All 1,443 compared NearLOD TREE occurrences have golden Delay=false. Of these,
33 were delayed solely because their original was persistent. Persistence no
longer enables Delay for this cohort. Replacement behavior, other grids and
other base signatures are unchanged and covered by tests.

Only these 33 Delay fields change across existing runtime rows. The comparison
preserves all 3,066 shared golden occurrences, including duplicate originals.
This is a bounded empirical rule, not a recovered universal timing algorithm.
Broader far-grid and parent-persistence predicates were rejected because they
regressed currently matching rows.

## Asset and installation boundary

The focused build uses the same retained-ID inputs/options as the replacement
follow-up: Tamriel/Medium, `--keep-lod-mods`, grass off, no terrain/tree/occlusion
generation. The measured final build took 32.532 seconds; this is not a controlled
speed benchmark.

Of its **743 files**, only the ESP and Tamriel NG data change. All focused meshes,
DDS, the ESM and Worlds data are byte-identical to the prior focused output.
The installed package is a **four-file runtime overlay**, not that grass-off
geometry package:

- Profile: `LODForge-Runtime-Batch-20261019`.
- Mod: `LODForge Runtime Batch 20261019`.
- Required full asset mod: `LODForge Runtime Gaps 20261019`.
- The previous replacement overlay is disabled only in this new profile.

All **308 referenced assets resolve**, zero structural errors. All **8,524 pinned
full-output hashes** remain unchanged. The missing input archive warning for
`Skyrim - Patch.bsa` remains visible.

## Live upgrade and world-transition checks

Skyrim 1.7.104.0, SKSE 2.3.1.0, DLL NG 3.0.43.0, Community Shaders.
The disposable profile contains a copy of the previous replacement-qualified
save; the original ESS/SKSE pair remains hash-identical. Thus this checks an
upgrade from the preceding generation, not only fresh initialization.

- Loaded the copied save. Native successful/active status true; reset count
  **57**. `GetResetAt(38)` and `(56)` resolve the first/last new USSEP originals.
- Survival Mode was already declined in that save. Later low-stamina/health
  tutorial dialogs paused travel; their text was inspected and their normal
  CloseMenu callbacks used. Checks were repeated unpaused. Test-player health
  was restored and god mode used during travel, then switched off before exit.
  No paused samples are used below.
- In Tamriel, Solitude original `0F06312F` remains enabled but has no loaded 3D.
  Its parent-world counterpart `000688C3` and dynamic copy `1A001505` start
  disabled. Enabling the counterpart and crossing from `(-21,25)` to `(-22,25)`
  enables/loads the dynamic copy, exercising one of the corrected Delay rows.
- Entering `SolitudeWorld (-18,25)` loads the original tree. The dynamic copy
  switches to SolitudeWorld and disables/unloads: no duplicate loaded tree.
- Restoring the counterpart's disabled state and returning to Tamriel unloads
  the Solitude original; the dynamic copy returns to Tamriel disabled/unloaded.
- Saved as `LODForge-runtime-batch-20261019` and reloaded. Native success and
  reset count 57 remain correct; gameLoadedCount=2, no blocking/loading menu,
  `paused=false`, and the restored dynamic state remains disabled.
- Clean shutdown, MO2 exits itself, no windows dismissed, game root clean.

Evidence is under `bench\gap-closure-20261019`: `runtime-batch-*`,
`child-reset-final-records.json`, `delay-policy-evidence.json` and
`bases-followup-evidence.json`. The live folder contains logs and a screenshot;
it is a state/visual smoke check, not an image-equivalence threshold.

## Remaining boundary

Base-selection research did not justify broadening the existing selector:
54 Northern Roads omissions lack valid large-reference participation; 17
HearthFires omissions are persistent disabled large full-model dynamics, but
plausible broadening also admits known extras. No count-fitting fix was applied.

Still open: 83 Bases identities, 352 Delay mismatches, remaining Disabled,
18 missing source originals, four generated patch originals and Katla's patched
transform, destruction, and the grass/material/season residuals. One upgraded
save and one representative child-world route do not certify all saves/worlds.
