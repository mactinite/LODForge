# Community Shaders / PBR parity

Latest: [full-output qualification](validation-full-20261019.md).
Fresh full generation preserves the previous geometry/material assets and
confirms the runtime counts below. **In-place full rebuilds are byte-identical**
since the `rebuild-determinism` fix (the earlier 585-file drift came from the
build reading its own previous output; see the
[baseline entry](baseline.md#in-place-rebuild-determinism-2026-10-19-branch-rebuild-determinism)).
User-observed distant glacier/terrain flicker remains unresolved. Acceptance is
functional compatibility: renderable, visually consistent output and safe
supported save upgrades. Exact grass triangles and rebuilt asset hashes are
diagnostics, not independent release gates. A rebuilt output has not yet been
tested with a pre-rebuild save; byte-identical rebuilds are strong supporting
evidence, not a blanket save-safety certification.

Original code review baseline: `2737410`. P3/G2 are merged.
The [post-merge validation at f1a31e6](validation-20261019.md) now includes a fresh
full Tamriel BTO-grass build, asset/material/mip checks and an isolated in-game
A/B smoke test. See the [gap-closure plan](gap-closure-plan.md) for next steps.
Historical measurements below remain attributed to their original scope.
The [runtime gap qualification](validation-runtime-20261019.md) covers working-tree
snow/state fixes, 84 recovered dynamic originals and four native-MNAM placements,
with preserved geometry and representative live parent/world transitions.
The [replacement follow-up](validation-runtime-20261019.md#replacement-follow-up)
adds 19 full-model replacements and ESM ONAM registration, with unpaused parent
transitions and save/reload checks. The [runtime batch](validation-runtime-batch-20261019.md)
closes reset selection on the fixture (57/57) and fixes 33 NearLOD tree Delay
fields, with an upgraded save and Solitude transition checks. The remaining
runtime and grass gaps are not closed.

## Target and evidence

Target: DynDOLOD-compatible files and in-game behavior for a Community Shaders /
PBR load order, including dynamic LOD. This is distinct from making every LOD
shape use the True PBR material model.

Directly checked local references:

- LODGen decompile references (moved out of the repository on 2026-10-20; the LODGen-derived code they informed has been replaced).
- `D:\Modlists\ref\TES5Edit\Core\wbLOD.pas` (classic tree LOD).
- Installed SSE DynDOLOD INI and TexGen preset for option locations/defaults.

The managed LODGen decompilation is not the closed Delphi DynDOLOD/TexGen
front end. Front-end behavior and DLL contracts below also rely on the existing
[behavioral specification](re/dyndolod-spec.md) and
[earlier audit](re/gap-audit-20261019.md); suspected behavior is labeled as such.
The original audit did not freshly decompile those executables or the DLL. The
gap-closure follow-up inspected the installed DLL's `[Bases]` parser: it resolves
the two-field form identity and sets HasDistantLOD (`flags |= 0x8000`). Which bases
the generator selects remains a separate, unfinished investigation. The writer
now emits the verified large-reference object-LOD subset (510 matching identities,
no extras); full selection still has 83 omissions.

**Confirmed material boundary:** LODGen's non-passthrough SSE output creates a
conventional lighting shader; passthrough shapes keep their shader path.
Its source normalization preserves specular settings for the True PBR bit and
freezes an HD-mask texture key with `linear.dds` normalized back to `.dds`.
LODForge implements these distinctions in
[shape normalization](../crates/objlod/src/shape.rs) and
[BTO writing](../crates/objlod/src/build.rs).
Blindly enabling True PBR on all BTOs would diverge from the reference.

## Implemented, not equivalent to full certification

The current source has AltTextures/VC side files, converted PBR albedo,
PGPatcher CRC lookup, TexGen PBR twins, dynamic TXST/MODS, frozen HD-mask keys,
passthrough effect-shader merge hashes, tree brightness handling, missing-normal
fallbacks, vanilla-LOD blanking and the fill of LAND-less edge cells from the shipped
terrain LOD meshes (spec 2.3; the Terrain Tamriel resource plugins are not needed). Do not reopen these as wholly absent based
on the original audit tables.

The [latest port measurements](baseline.md#lodgen-port-fixes-p4-g3-g9-g10-g11-2026-10-19-branch-claudeobjlod-port-fixes-1d734a)
report 98.441% tolerant triangles for the pinned P7 export, but that is a
**port test**, not full PBR pipeline parity. The same log reports E2E LOD4
HD shapes at 863 vs 867, with missing ash groups attributed to export rather
than the port. Counts are not enough to certify material identity.
Earlier game checks cover marker/reset acceptance and a rolling-update reload,
not all Community Shaders features, worlds or saves.

## Remaining work

Priority indicates compatibility risk, not an effort estimate. Each item names
the next measurable result rather than prescribing a speculative implementation.

| Priority | Gap / ownership | Current evidence | Completion test |
|---|---|---|---|
| Medium; qualification | Rebuilt-output save compatibility | Rebuilds are byte-identical in place since 2026-10-19 (branch `rebuild-determinism`, [baseline](baseline.md#in-place-rebuild-determinism-2026-10-19-branch-rebuild-determinism)): the same full command run into the existing output and work folders reproduces all 8,524 files and the whole work folder, keeping DynDOLOD.esm/.esp form ids and the run token. The earlier 585-file drift was the build reading its own previous output through the VFS (four MiniAtlasExport mini atlases taken as already installed); only DynDOLOD.esm/.esp are read from the output now. | Save compatibility is not certified by this: still load pre-rebuild saves on rebuilt output, exercise runtime state/lifecycle, and save/reload. With identical assets that check is about the retained ids and token, not about asset drift. |
| Triaged; not output-specific (tabled) | Distant glacier/terrain flicker | [Consecutive-frame A/B + live session (2026-10-19/20)](validation-flicker-20261019.md): the distant LOD alternates between a hazy and a crisp rendering every 2-4 s, re-armed on demand 3 of 3 at the tundra spot by a teleport-and-weather sequence, identically with LODForge and DynDOLOD outputs; persists at native with no upscaler, with DynDOLOD inactive, with SSGI off, at 5% game time and after a weather snap; full models never change; never seen in normal play. | Not a LODForge generation defect: meshes and textures are not implicated; the renderer lights or fogs the distant LOD two ways in turn. Open and tabled by the user: which Community Shaders feature (Volumetric Lighting, Cloud Shadows, Skylighting, Terrain Shadows) and what arms it. Keep uLargeRefLODGridSize=11; still ship terrain mips for quality. |
| Implemented; residuals measured | PBR mini atlases; object page size/alpha split/mip cap; billboards on the alpha page (P3, G2, G29, G30) | Fresh full build confirms four matching object atlas files. Mini diffuse PSNR 33.78-40.91 dB. Follow-up fixes `snow01noalphalinear` from 10.20 to 48.73 dB, with 46 unchanged linear textures. Named HD groups including snow/ash: LOD4 863/867, LOD8 108/108; old narrower HD counts are not comparable. G29 now covers grass as well as tree billboards: the `_Alpha` map has 98 rows (golden 113), 64 of them the `_1`/`_2` grass views, page still 2048² BC7 with 8 mips. | Trace remaining HD membership, tree packing and mip coverage; do not reopen merged atlas production or the fixed snow scale. |
| High | DLL runtime sections / dynamic semantics (G35) | `[Bases]` 510/593; `[Disabled]` 19/39; resets **57/57**, all zero extras. Source originals 3,060/3,078 matched, plus one explained Katla identity/transform difference; four generated patch originals remain absent. Delay mismatches **352** after 33 isolated NearLOD TREE fixes, no regressions. New reset overrides match golden fields and source-world grouping. | Preserve reset parity; finish missing cross-world originals, base-selection, patch and destruction fixtures. Extend the verified single-save upgrade and Solitude transition coverage to remaining families/worlds. |
| High | Complex tree atlas and Ultra trees (G6, G26, G27) | [tree stage](../apps/cli/src/tree_lod.rs) emits `ComplexAtlasTexture=0`, composes diffuse only and passes an 8192 cap. Resolver has no Ultra mode switch. | Complex diffuse/normal output and metadata agree with the consumer; Ultra rules/templates and vanilla suppression work; test classic mode unchanged. |
| Medium; functional validation | Complex/vanilla BTO grass coverage (G32) | Cached/100% density run: 1.09% golden triangle overlap, but all current grass triangles reproduce across rebuilds and the sampled visuals were acceptable to the user. Runtime sway is not part of the offline metric. Offline generated placement has no raycasts. Grass billboards are now atlased like the golden's: LOD4 `grasspassthru` shapes 6,078 -> **4,084** (golden 4,016), all on `DynDOLOD_Tamriel_Alpha.dds`, grass triangle positions unchanged. Type coverage is unchanged (32 billboard types written, 14 left out by the TexGen rule; golden map 30). | Validate intended type coverage/density, formats/materials and near/far transitions. Investigate framing differences that cause visible defects; do not require exact golden geometry. Keep GO results separate. |
| Medium | Optional 22-field shader override (G13) | [AltTextures parser](../crates/objlod/src/export.rs) takes only the first 10 texture fields. Reference `AltTextureDesc` also accepts shader fields when there are 32 total. | Synthetic 10/32-field cases; shader values, type-specific data and merge hash match reference without changing legacy inputs. |
| Medium | Directional material handling (PS3) | [base copies](../crates/dyndolod/src/dynplugins.rs) write zeroed STAT DNAM, not the source angle/material. Earlier RE describes snow/ash remapping. | Compare golden STAT DNAM and material form resolution for snow/ash PBR dynamic bases; validate visuals before choosing preserve vs remap rules. |
| Medium | Settings and rule semantics (G14, G16, G17, G22) | Build config reads `DynDOLOD_SSE.ini` and reconstructs rules; [rule loader](../crates/dyndolod/src/rules.rs) skips `[Skyrim Settings]`. TexGen settings remain defaults rather than loaded presets. | Non-default PBR scales, world exclusions and edited rules alter exactly the expected output; publish effective settings and test precedence. |
| Medium | PBR texture-slot lookup (PS1) | [scene loading](../crates/texgen/src/scene.rs) reads model slots; [renderer](../crates/texgen/src/render.rs) marks PBR by diffuse path. Earlier RE describes PBR-pass redirects/pickup for unpatched meshes. | A model with vanilla slots plus installed PBR twins matches TexGen; patched models and non-PBR controls remain unchanged. |
| Medium | Alpha mip algorithm / tree packing (G23, G24) | Merged tree/object paths now fill average color and preserve tile coverage. Whole-page curves differ substantially from reference growth across mips; tree page is 4096 squared vs 4096x2048. | Compare matched tiles at every retained mip, infer reference alpha behavior and test edge color. Do not mistake different page occupancy for per-tile loss. |
| Medium | World/season/export policy (G18-G21, G33) | Earlier audit lists partial records, per-mod exclusions, BOS and child-world policy gaps. Current code has child copies, but that does not establish all policy switches. | Re-test non-default policies on small child/DLC/BOS fixtures; compare record fields, selected refs and season eligibility. |

## Important qualifications

### Runtime compatibility is more than plugin names

The plugin writer now includes framework quests/VMAD, TXST/MODS and large-reference
workarounds. The historical plan's blanket claim that these are missing is stale.
However, the writers still leave runtime sections incomplete. `[Bases]` is in
global Worlds.txt and is feature/runtime-dependent (the spec associates
`BaseLODDLL` with newer Skyrim runtimes). The tested golden has 593 entries;
absence is not a universal startup failure, as the successful smoke test shows.
The follow-up emits only bases used by selected large-reference object LOD,
excluding generated child copies and already-flagged bases. Simply including all
large dynamic references produced three unsupported updates and was rejected.
The current 83 omissions are 54 Northern Roads, 17 HearthFires and 12 Skyrim bases.
See the [base-section baseline](baseline.md#global-base-flag-updates) for controls
and the limits of automatic runtime detection.

The current writer hardcodes parts of world switching, destruction-stage fields
and names in `[Objects]`. Some are valid defaults; treat them as unsupported
non-default semantics until reference fixtures prove otherwise. `base_copy`
zeroing DNAM is a more precise finding than the old claim that it merely copies
unremapped material data.

### Shader overrides deserve a focused test

The reference tool's AltTextures side file carries ten texture fields plus 22 shader fields,
then updates the shader and its hash. LODForge explicitly does not implement
that extension. Recent baseline notes suggest this may explain passthrough merge
residuals; that causal link remains a hypothesis, even though the parser gap is
confirmed. Do not label the gap irrelevant just because one older export lacked
32-field entries.

Another confirmed, lower-priority port gap is **non-passthrough grass** (G12):
`CreateLODNodesSSE` adds the LOD flag when normals exist and clears slots 2..8;
the corresponding LODForge branch does not fully do so. The observed golden uses
`grasspassthru`, so this is not an explanation for its missing grass types.

### Findings that still need a targeted reference trace

- True PBR emissive/glow suppression and shape filters (PS2/PS4).
- The remaining ash/HD export selection and LOD8 HD deficit after current merges.
- Fake lights / CS light-data export, optional unsupported rule actions and
  non-default runtime paths in the [key inventory](re/gap-audit-20261019-ini-keys.md).
- Billboard normal-map quality, framing and small-size residuals. Existing PSNR
  numbers are profile-specific; they are not universal acceptance thresholds.

These are not all proven visible regressions. Use the original audit as the long
tail inventory, and record new evidence here before promoting an item to a blocker.

### Child world copies vs the Ultra golden (skyrim-cs-base, 2026-10-23)

Golden: `LODForge-golden\dyndolod-ultra-csbase-20261022` (DynDOLOD 3 Alpha-215, High rules,
Ultra, "Child > parent" Low, Tamriel only). Rules here come from DynDOLOD's output, logs and
shipped configs only (no reverse engineering of its executables). `lodforge child-copies
--preset High --child-parent Low` now gives the golden's 363 copies exactly (was 365 with 4 extra
and 2 missing). What closed the gap:

- **Reference key.** The debug log keys references as
  `<model>_<round(x/100)>_<round(y/100)>_<round(z/100)>_<scale%>`
  (`childworld::ref_key`; rounding half to even is assumed, as Delphi's `Round`).
- **Duplicates inside a child.** A later reference with the key and rotation of an earlier one
  is dropped ("Duplicate reference ignored for LOD ... adds same model at same position,
  rotation and scale"): Haelga's Bunkhouse `FE063817` duplicates JKs Skyrim `13000EAA`.
- **Parent twins, one to one.** A child reference whose key equals a parent reference's is not
  copied (Pawned Prawn `FE07E800`/`FE07E81C`, JKs Skyrim `130147D2`, whose Tamriel twins are up
  to 9 units away), but a parent reference takes at most one twin, and one already paired
  through `ChildworldMatches.txt` takes none: JK's Temple of the Divines `FE0798F8` has the key
  of Tamriel `000F4FEE`, which the match list pairs with Solitude's `000E73A4`, so it is copied.
- **Grid-only rules fall through.** `SovngardeStatue01` (copy of JKs Skyrim `130024C1`) matched
  `sovngardestatue0,,,,Far LOD,Unchanged` and got no LOD. DynDOLOD's object report lists that
  rule and then the catch-all `\,Static LOD4,...` for the base: a grid-only `Unchanged` rule
  passes a base that has LOD models on to the next matching rule with levels
  (`rules::select_mask_rule`). Bases without LOD models (`ShipCaptainsBroken01`) stay dynamic.
  Tamriel export: the three JKs Skyrim statues are now static object LOD as in the golden, and
  nothing else changed (52,835 -> 52,838 identical lines, dynamic overlap unchanged).

Still open: `skyrimesm&0391FE` is a Tamriel pine copied into Solitude's own LOD (parent ->
child, `ScanParent`); child worldspace LOD is not built yet.

## Exit criteria

Use the [validation matrix](validation.md). Certification requires equivalent
inputs, resolved assets, material/texture checks and runtime tests, not a single
geometry percentage. Preserve xLODGen and ordinary non-PBR behavior when adding
CS-specific paths. Unsupported settings must become explicit, not silent defaults.
