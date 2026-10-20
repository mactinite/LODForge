# DynDOLOD 3 / TexGen behavioural spec

Behavioural specification of sheson's **DynDOLOD 3** (`DynDOLODx64.exe` 3.0 Alpha-215), **TexGen**
(`TexGenx64.exe`, same build) and the LODGen they drive, written so LODForge can reimplement them
(PLAN.md phase P6). Personal / Cairn use.

**Provenance boundary.** This is a research record, not evidence of a separated
clean-room implementation. Later sections explicitly use decompilation and
disassembly. See the [licensing review](../licensing.md) before redistribution.

DynDOLOD and TexGen are closed source (Delphi, on the MPL xEdit core). This
document describes behaviour, formats and semantics in our own words. Its sources are string
dumps, symbol names, the public docs at dyndolod.info, the shipped configuration (read, never
bundled), the DLL NG Papyrus sources, and the outputs of a real run. The original pass aimed
to describe behavior rather than reproduce implementation code; that intent does not
establish independent provenance or redistribution rights.
Earlier research identified Resources/configs as CC BY-NC-SA
and LODGen as MPL-1.1; those assertions still need verification against the exact
distributions. Reading installed resources is not permission to redistribute them,
and decompilation does not itself grant porting rights.
See [lodgen-318-delta.md](lodgen-318-delta.md) for the LODGen 3.1.8 changes ported in
`crates/objlod` (`Flavor::Dyndolod318`).

**Evidence tags.**

| Tag | Source |
|---|---|
| `[doc: Page]` | <https://dyndolod.info/...> help pages (read 2026-10-17) |
| `[cfg: file]` | shipped files under `tools\DynDOLOD\Edit Scripts\DynDOLOD\` (Configs, Rules, INI) |
| `[res]` | file names in the *DynDOLOD Resources SE 3* mod |
| `[psc: file]` | DynDOLOD DLL NG Papyrus sources (NG unless marked legacy) |
| `[pdb]`, `[dll-string]` | DynDOLOD.DLL symbol names and string literals |
| `[exe-string]` | string literals in DynDOLODx64.exe / TexGenx64.exe |
| `[shader: file]` | TexGen GLSL shaders, paraphrased |
| `[lodgen-src]` | retired 2026-10-20: LODGen decompile evidence; the sections that relied on it were moved to the quarantine folder outside the repository |
| `[golden]` | the golden TexGen and DynDOLOD runs in `D:\Modlists\LODForge-golden\` (docs/baseline.md, section "P6") |
| `[decomp]` | REA/Ghidra 12.1.4 decompilation and disassembly of `DynDOLODx64.exe` (2026-10-18, occlusion only), described here in our own words |
| `[inferred]` | our own deduction; verify before relying on it |

Ghidra / REA was not needed for the first pass: strings plus symbols, configs, docs and golden
outputs answered most questions. The occlusion section was later pinned down with a REA/Ghidra
pass over DynDOLOD's TVDT code `[decomp]` and checked bit for bit against the golden run. The deep
TexGen rendering questions (exact model-height source, light-vector signs) are the next
candidates.

## Pipeline overview

```
TexGen  ──►  textures\terrain\lodgen\<plugin>\*.dds/.txt (billboards), textures\lod\* (stitched /
             rendered LOD textures), Cache\..._re-uv-miniatlas.txt, textures\DynDOLOD\LOD\TexGen_SSE.ini
   │ (installed as a mod, then)
   ▼
DynDOLOD ──► loads the load order (xEdit core) → patches → rules → per-base LOD models →
             per-world references → object LOD export + atlas → LODGen (one process per season,
             concurrent) → tree LOD (.btt + atlas) → dynamic LOD (plugin records + DLL data) →
             terrain underside (LODGen) → large references → occlusion (TVDT) → save plugins
```

The golden run (Tamriel only, Medium preset, seasons on) took **6:59** of generation after a
~30 s load. Object LOD export took ~70 s, the five LODGen processes ~31 s each (in parallel),
and occlusion ~2 min [golden]. Full numbers are in docs/baseline.md.

## Golden-run facts (Tamriel, Medium, seasons) [golden]

These facts close several open questions from the static analysis.

- **Output tree.**
  - `meshes\Terrain\Tamriel\Objects\` holds 3,425 `.bto` (685 per season × 5) plus
    `Tamriel.objects.heightmap.dat`.
  - `meshes\Terrain\Tamriel\Trees\` holds 3,720 `.btt` (744 per season). It also has
    `Tamriel[_SEASON].txt`, a plain list of `plugin.esm;FORMID8` tree references, and
    `Tamriel[_SEASON].lst`.
  - `textures\Terrain\Tamriel\Trees\TamrielTreeLOD[.SEASON].dds` plus a matching `.ini`. The INI
    has `[Information] ComplexAtlasTexture=0`.
  - `textures\DynDOLOD\LOD\DynDOLOD_Tamriel[_Alpha][_n].dds`: the object atlas, split into
    alpha and non-alpha.
  - `textures\lod\*lod[_n].dds`: mini atlases DynDOLOD built itself.
  - `Seasons\DynDOLOD_{WIN,SPR,SUM,AUT}.ini`.
  - `SKSE\plugins\DynDOLOD_Data\DynDOLOD_NG_{Worlds,Tamriel}.txt`.
  - `DynDOLOD.esm`, `DynDOLOD.esp`, `Occlusion.esp`.
- **Seasons in LODGen.** Each season export is a full export with its own `LimitCoords`.
  - LODGen *copies* the default-season BTO for quads the season does not change. Its log prints
    "Copied LOD level N coord [x, y]".
  - In this run: WIN rebuilt 3 quads and copied 682; SPR rebuilt 303 and copied 382.
  - This is exactly LODForge's planned delta build (P4), so a compatible generator gets the
    speed-up for free.
- **Plugins** (record counts, header):
  - `DynDOLOD.esm`: ESM flag, HEDR 1.71, 5,934 records.
    - Records: 1,500 REFR (14 persistent, 1,486 temporary), 1,314 CELL, 296 STAT, 40 MSTT,
      24 TREE, 19 ACTI, 12 WRLD, 3 TXST, 3 LSCR, 2 KYWD, 2 CONT, 1 DOOR, 1 FURN, 1 LAND.
    - 13 masters (Skyrim, Update, DLCs, the CC files that own touched refs, USSEP).
    - Description: "…Master to work around game breaking bugs.\rPlugin generated by
      DynDOLODVersion 3.07".
  - `DynDOLOD.esp`: no flags, HEDR 1.71.
    - Records: 2,121 REFR (2,098 persistent; 1,684 with *Is Full LOD* 0x10000; 1,683
      initially disabled), 32 CELL, 8 STAT, 8 WRLD, 3 LSCR, 2 KYWD, 2 ACTI, 2 QUST, 1 FLST.
    - Masters include `DynDOLOD.esm` and `Seasonal Landscapes.esp`.
  - `Occlusion.esp`: **ESL flag (0x200)**, HEDR 1.71, author `xLODGen`. It holds 6,285 CELL
    overrides (Tamriel and 7 child worldspaces) under 8 WRLD records. Each CELL is a full copy
    with TVDT (and MHDT when missing) filled in; see "Occlusion".
  - The plugin version the DLL compares is most likely the `3.07` in the TES4
    description (SNAM), i.e. 307 = `GetScriptVersion()` [inferred].
- **DLL data files** use comma-separated fields, **decimal** local form IDs and plugin names.
  See "DLL NG and scripts" for the grammar.
- **Rule merge order** (resolved from the saved preset):
  - Files are concatenated in this order: `DynDOLOD_SSE_all.ini` → per-plugin files in load
    order (`<plugin>_<Preset>.ini` when it exists, else `<plugin>.ini`) → `_all_dynamic.ini` (or
    `_all_static`) → the base preset `DynDOLOD_SSE_<Preset>.ini`.
  - They are numbered `LODGen1..N` in that order. The first match wins, so negatives and specific
    rules naturally precede the base preset and its `\` catch-all.
- **Preset file layout** (resolved): `Presets\DynDOLOD_SSE_Default.ini` has two sections.
  - `[DynDOLOD]`: the UI keys listed in the INI section below, plus `LODX`/`LODY`.
  - `[Skyrim LODGen]`: rules saved as
    `LODGen<n>=mask,LOD4,LOD8,LOD16,LOD32,Grid,Reference,Flags,<source rule file path>`. The
    ninth field records where each rule came from.
- **`_lod_0h` / `_lod_0l`.** The export gives `MountainCliff02_HeavySN` the LOD model
  `mountaincliff02_lod_0h.nif`, which confirms that `h`/`l` select heavy or light snow variants by
  the base's directional material.
- **LODGen runtime.** DynDOLOD launched `Edit Scripts\LODGenx64Win10.exe`, a single-file
  `.NETCoreApp v8.0` build. It is not the net48 `LODGenx64.exe` that sits next to it.
  - It rolls forward to the newest installed runtime; its log says ".NET Version: 10.0.12".
  - .NET 9+ System.Numerics fuses multiply-adds. So the same DynDOLOD version produces
    last-bit-different BTOs on machines with different .NET runtimes. LODForge emulates both:
    `numerics::set_fma`, CLI `--dotnet8`.

## INI keys and presets

### Files and where settings live

| File | Role | Evidence |
|---|---|---|
| `Edit Scripts\DynDOLOD\DynDOLOD_SSE.ini`, section `[DynDOLOD]` | Global tool settings, edited by hand. Game modes TES5, ENDERAL, ENDERALSE, SSE (the TES5VR mode also uses the SSE file) each have their own file. | [cfg: DynDOLOD_SSE.ini] |
| `Edit Scripts\DynDOLOD\Presets\DynDOLOD_SSE_Default.ini` | Saved Advanced-mode choices plus the current rule table. Written when the user clicks OK (start generation) and loaded on the next start. | [doc: Help/Advanced-Mode], [exe-string `PresetFilename`, `Default`, `SavePreset`, `LoadPreset`] |
| `Presets\*.ini` (user-named) | Written by "Save preset" and read by "Load preset". The file dialog filter is "DynDOLOD preset (*.ini)". | [exe-string] |
| `Presets\DynDOLOD_SSE_TexGen.ini`, section `[TexGen]` | TexGen's saved choices (OutputPath, formats, Stitch/Render/Billboard sizes, lighting sliders). Observed on disk after a TexGen run. | [cfg: observed file] |
| `Data\DynDOLOD\<same file name>` | Any rule, patch or TexGen text file placed here takes precedence over the shipped copy with the same name. | [doc: Mod-Authors], [cfg: header comment of *.patch] |
| Per-plugin rule file `[DynDOLOD]` section | Overrides `DynDOLOD_SSE.ini` keys when that rule file is loaded. | [doc: Mod-Authors] |
| `SkyrimPrefs.ini` / `SkyrimCustom.ini` / `SSEDisplayTweaks.ini` | Read for `uGridsToLoad`, `uLargeRefLODGridSize`, `uLockedObjectMapLOD`, `fBlockLevel0Distance`, `fBlockLevel1Distance`, `fBlockMaximumDistance`, and display resolution. The resolution sets default max tile sizes on first run. | [exe-string `Prefs.ini`, `TerrainManager`, `SSEDisplayTweaks.ini`, `Resolution`, `*MaxTileSizeDefault`], [doc: Help/Advanced-Mode] |

Saved preset keys (the UI state). These names appear in the SavePreset and LoadPreset string runs [exe-string]. Meanings come from the Advanced-mode docs and the form hint texts [doc: Help/Advanced-Mode], [exe-string DFM hints]:

| Key | Meaning |
|---|---|
| `OutputPath` | Output folder. |
| `Preset` | Last rule preset: Low, Medium or High. Default is `Medium`. |
| `PresetCopyChild` | Child-to-parent copy preset: Low (no trees), Medium (with trees), High (with trees and mountains). Default is `Medium`. |
| `DoObjectLOD`, `Level32` | Generate object LOD (.bto files). Also generate LOD level 32. |
| `DoTreeLOD`, `UltraTreeLOD`, `TreeLargeRef` | Classic tree LOD (.btt/.lst). Ultra moves trees into object LOD. Large makes full-model trees large references; it needs DLL NG. |
| `DoDynamicLOD`, `NearGrid`, `FarGrid` | Dynamic LOD on/off, and grid diameters in cells. NearGrid must be smaller than FarGrid. |
| `Windows`, `WindowsHigh` | Glow LOD windows. High puts them in dynamic LOD so they react to daylight. |
| `LoadCandles`, `LoadFXGlow` | Add candle and FX-glow rule files when a preset button is clicked. |
| `FakeLights`, `FakeLightsChildWorld` | Add fake-light glow for LIGH references in the selected world or in its child worlds. |
| `UseOrgLOD` | "Prefer base record LOD assignments over rules". |
| `UpgradeLargeReferences` | Large refs whose rule says NearGrid go to FarGrid. Unchecked means they get no dynamic LOD. |
| `DowngradeNormalReferences` | Non-large refs whose rule says FarGrid go to NearGrid. Forced on when the large-reference workarounds are unavailable. |
| `WorkaroundLargeReferencesBugs`, `WorkaroundLargeReferencesBugsEnabled`, `IgnoreLargeReferences` | Large-reference bug handling. |
| `CopyParent` | Parent-to-child copies. |
| `ObjectLODMaxTileSize`, `FullTextureMaxTileSize`, `TreeLODMaxTileSize`, `GrassLODMaxTileSize` | Atlas tile caps. Each has a `*Default` key derived from the resolution. |
| `ObjectLODBrightness`, `TreeLODBrightness`, `GrassLODBrightness`, `FullTextureBrightness`, `FullTextureSizeDivider` | Brightness and size tuning. |
| `DoOcclusion`, `OcclusionQuality`, `OcclusionPlugin` | TVDT occlusion data. `OcclusionPlugin` writes it to a separate Occlusion.esp. |
| `DoGrass`, `GrassLODDensity`, `GrassLODMode`, `CSGrassOptimizations` | Grass LOD. Mode 1 or 2 must match GrassControl. |
| `DoTerrainUnderside`, `TerrainUndersideQuality`, `TerrainUndersideHeight` | Terrain underside mesh. |
| `DoSeasons`, `SeasonsSnow`, `SeasonsType`, `SeasonsToGenerate` | Seasons support. |
| `AutoMap`, `LIGH2JSON`, `Expert`, `LODLevel` | World-map helper, light JSON export, expert mode, and the single LOD level used in expert mode. |
| Rule table | Saved in the same file as the rule-file format. The section is probably `[Skyrim LODGen]`. The writer format string is `%s,%s,%s,%s,%s,%s,%s,%d%s`: mask, LOD4, LOD8, LOD16, LOD32, Grid, Reference, flags, then a suffix. | [exe-string `SaveRulesFile`, `LODGen`, `GameName`], section name [inferred] |

The worldspace list state is not in the preset. Double-clicking a worldspace toggles a `[Seasons]` tag on it. Hidden worlds come from `Configs\DynDOLOD_SSE_worldspace_ignore.txt` [doc: Help/Advanced-Mode].

### `DynDOLOD_SSE.ini` `[DynDOLOD]`: shipped keys

Defaults are the shipped values [cfg: DynDOLOD_SSE.ini]. Meanings are paraphrased from the in-file comments unless another tag is given.

**Mode and general**

| Key | Default | Meaning |
|---|---|---|
| `Wizard` | 1 | Start in wizard mode (1) or advanced mode (0). |
| `Expert` | 0 | Show expert LODGen controls (Execute LODGen, Rebuild Atlas, Create Mod, LOD level / W / S chunk pickers). Turns wizard off. |
| `CreateMod` | 0 | "Create Mod" mode: writes a plugin that only updates STAT LOD assignments and the Has Distant LOD flag, for CK-style LOD. |
| `Verbose` | 0 | Pass `--verbose` to LODGen. |
| `Notice`, `Debug` | 0, 0 | Extra log output. Debug also turns on notices and warnings. |
| `BaseSignatures` | `STAT,TREE,ACTI,MSTT,CONT,FURN,DOOR` | Base record types scanned for LOD. GRAS is handled separately by grass LOD. The exe has the same built-in default [exe-string]. |
| `uGridsToLoad` | 5 | Keep at 5 even if the game uses more. Used for grid arithmetic. |
| `IgnoreLargeReferences` | 0 | Set to 1 only if `uLargeRefLODGridSize == uGridsToLoad`. Then DynDOLOD.esm is not needed. Has no effect with DLL NG. DynDOLOD forces it back to 0 if the Prefs value is larger [exe-string "Forcing IgnoreLargeReferences=0"]. |
| `TreeLargeRefSize` | 400 | Minimum tree size for the "Large" ultra-tree option. |
| `Temporary` | 0 | Add activators as temporary refs (more masters and larger plugin). |
| `ConvertEnabledReferences` | 1 | Refs with an enable parent get a copy of their base record without Has Distant LOD, so object LOD does not flash when they enable. |
| `PartialWorldspace` | 0 | Set the Partial Form flag on WRLD overrides. |
| `PartialPersistentCell` | 1 | Set the Partial Form flag on persistent CELL overrides. |
| `SwapsChance` | 0 | Base-object swaps with a random chance: 0 means use the original base, 1 means no LOD. |
| `IgnoreModFileName` | list | Partial plugin names whose added references are ignored (their overrides are still processed). |
| `ReplaceModFileName` | list | Obsolete mods. An optional `;Help/Page` suffix gives the doc page for the warning. |
| `WarnModFileName` | empty | Mods to warn about. |
| `IgnoreBaseEDID` | `loadscreen, _invis_, dbm_arrivalship` | Partial base-record EditorIDs to skip. |
| `ValidRootBlocks` | `BSFadeNode, BSLeafAnimNode, BSTreeNode` | NIF root types accepted. Other NIFs get a warning and are avoided. |

**LOD model discovery**

| Key | Default | Meaning |
|---|---|---|
| `Folder1`..`Folder5` | `meshes`, `meshes\dlc01\lod`, `meshes\dlc02\lod`, `meshes\lod`, `meshes\dyndolod` | Scan order for `*_lod*.nif`. On a bare file-name collision the file found last wins. |
| `IgnoreLODMesh` | `.mohidden,\aom\,\enderal\,\smskyrim\,\anthelius\,\nsurlod\,\rigmorcyrodiil\trees\,\lod\midwoodisle\,\lod\revenant` | Partial paths excluded from LOD auto-discovery. |
| `IgnoreFullMesh` | `.mohidden,\aom\,\enderal\,\smskyrim\,\anthelius\` | Partial paths excluded as full models. |

**Tree LOD**

| Key | Default | Meaning |
|---|---|---|
| `TreeFullFallBack` | 1 | What a tree gets when no 3D LOD is found: 0 = full model (strongly discouraged), n = Billboard n. |
| `TreeBillboard1`..`6` | `Internal`, `DynDOLOD_flat_2x2_lod.nif`, `DynDOLOD_flat_3x2_lod.nif`, `DynDOLOD_flat_4x2_lod.nif`, `Internal`, `LODGen_flat_lod.nif` | Billboard templates for rule values Billboard1..6. Lookup is `Data\DynDOLOD\` first, then the Edit Scripts folder. `Internal` (two crossed quads) is the fallback. |
| `TreeLODMaxTilt` | 30 | Classic tree LOD ignores X/Y rotation. Trees tilted more than this (degrees) go to object LOD instead. A hidden `TreeLODMinTilt` also exists [exe-string]. |
| `CrownBrightness`, `TrunkBrightness`, `FlatTrunkBrightness` | 1 | Vertex-colour multipliers for 3D tree LOD shapes. `_R/_G/_B` per-channel variants exist [exe-string]. A season suffix such as `_WIN` gives a per-season value. |
| `CreateEmptyBTT` | 0 | With ultra, write empty .btt files instead of an empty .lst. |
| `TreeLODMaxAtlasSize` | 16384 | Classic tree LOD is limited to one atlas per world. |
| `TreeLODComplexAtlas` | 0 | Diffuse in the top half and normals in the bottom half (Community Shaders only). |
| `TreeLODCreateAverageColor`, `TreeLODForceAverageColor` | 1, 0 | Fill transparent texels with the average colour. |
| `TreeLODDiffuseFormat` | 225 | Atlas format for tree LOD. |
| `TreeLODGenerateMipMaps` | 2 | Mipmaps: 0 none, 1 texconv, 2 internal. |

**Billboards in object LOD ("flat" LOD)**

| Key | Default | Meaning |
|---|---|---|
| `FlatLODLightingEffect` | 0.0 | Lighting Effect 1 value for soft-lit billboards. |
| `FlatLODGlowMap` | `textures\white.dds` | Glow texture for soft-lit billboards. |
| `FlatLODVertexColor` | 0.55 | Vertex-colour brightness for billboards. |
| `FlatLODWithNormalMapVertexColor` | 0.50 | Vertex-colour brightness for billboards with a normal map. |
| `FlatLODColorVariance` | 0.1 | Random colour jitter. |
| `FlatLODHeightVariance` | 0.0 | Random height jitter. |
| `FlatLODLevelLODFlag` | 0 | Set the SLSF2_LOD_Objects flag (no lighting) on billboards at this LOD level and above: 1 = LOD4, 2 = LOD8, 3 = LOD16, 4 = LOD32. |
| `AllHDLOD32` | 0 | Force (1) or clear (−1) the HD flag on snow/ash shapes in LOD32. `AllHDLOD4/8/16` also exist [exe-string]. |

**Atlases, textures, rendering**

| Key | Default | Meaning |
|---|---|---|
| `ObjectLODMaxAtlasSize` | 8192 | Object LOD atlas cap. Multiple atlases are allowed. |
| `ObjectLODCreateAverageColor`, `ObjectLODForceAverageColor` | 1, 0 | Average-colour fill for object LOD textures. |
| `GrassLODCreateAverageColor`, `GrassLODForceAverageColor` | 1, 0 | Average-colour fill for grass LOD textures. |
| `ObjectLODDiffuseFormat` | 200 | Codes: 200 BC1, 201 BC2, 202 BC3, 225 BC7 quick, 226 BC7 max, 87 R8G8B8, 88 A8R8G8B8. If the alpha and non-alpha formats are the same, one atlas file is written; otherwise two. |
| `ObjectLODAlphaDiffuseFormat` | 225 | See above. |
| `ObjectLODNormalFormat` | 200 | See above. |
| `ObjectLODGenerateMipMaps` | 2 | Mipmaps: 0 none, 1 texconv, 2 internal. |
| `MaxTextureSize` | 8192 | Largest texture handled. |
| `MaxRenderResolution` | 8192 | Render buffer size cap. |
| `AlphaCoverage` | 0.65 | Texconv mip alpha coverage. |
| `AlphaFactor` | 0.25 | Internal mip alpha factor. |
| `IgnoreTexture` | `mountainslab01.dds, …, treelod*.dds` | File masks never put on the atlas; these textures are used directly. |
| `UseDecalFlag` | 0 | Keep the source decal flag in object LOD. |
| `DoubleSidedTextureMask`, `DoubleSidedMeshMask` | commented out | Force double_sided on matching shapes (for volumetric-lighting mods). |

**Glow and lights**

| Key | Default | Meaning |
|---|---|---|
| `GlowLODLocalMap` | 0 | Add the ShowLocalMap flag to glow refs. |
| `LIGH2JSON` | 0 | Export LIGH bases and refs to JSON for Community Shaders instead of fake lights. |
| `FakeLightsMinScale`, `FakeLightsChildWorldMinScale`, `FakeLightsIntensity` | 1, 1, 6 | Minimum glow-orb scale (1 = 256 units) and intensity (1..9). |
| `fMinSecondsForLoadFadeIn`, `fNearMultiple`, `fFarMultiple`, `fNeverMultiple` | 2.7, 1.25, 1.5, 1.75 | Defaults written for the in-game MCM. |

**Occlusion, optimisation, terrain**

| Key | Default | Meaning |
|---|---|---|
| `OcclusionMaxThreadsTerrainLOD`, `OcclusionMaxThreadsObjectLOD` | −1, 4 | Thread limits. |
| `OcclusionHeight`, `OcclusionRadius` | 0, 100 | Observer height offset and maximum distance. |
| `OcclusionMode` | `-TVDT -Flat +Border` | Which cells get TVDT updates. Terms combine: All, ±TVDT, −Flat, +Border. Per world: `OcclusionMode_<EDID>`, `Occlusion_<EDID>=0/1` (force off or on; small worlds are skipped by default), `OcclusionQuality_`, `OcclusionHeight_`, `OcclusionRadius_` [exe-string]. |
| `OptimizeMode_<EDID>` | Tamriel 1, Solstheim 1, BSHeartland 0 | Skip LOD4/LOD8 chunks the player can never get close to: 0 off, 1 border, 2 navmesh. LOD16 is always complete. |
| `OptimizeDistanceLOD4`, `OptimizeDistanceLOD8` | 69420, 100000 | Compared with fBlockLevel0/1Distance; the larger value is used. Hidden `OptimizeMult4/8/16` and `OptimizeDistanceLOD16` exist [exe-string]. |
| `RemoveUnseenZShift` | 10.0 | Terrain-height offset for LODGen's remove-unseen-faces. |
| `TerrainUndersideIgnoreWorlds` | list | Worlds skipped for terrain underside. |
| `TerrainUndersideMin/MaxX/Y_<EDID>` | — | Cell bounds for the underside. |
| `TerrainUndersideIgnoreHide_<EDID>` | — | `x,y;x,y;…` cells where the hide-quad flag is ignored. |
| `DoTerrainHeightMap`, `TerrainHeightMapIgnoreWorlds`, `TerrainHeightMapMin/MaxX/Y_<EDID>`, `DoObjectHeightMap`, `LODGenHeightMap` | 0, list, —, 0, 1 | Height-map export. |

**Grass**

| Key | Default | Meaning |
|---|---|---|
| `GrassGID` | `cgid` | Grass cache file extension. |
| `GrassBillboard1`..`6` | — | Grass billboard templates. |
| `GrassBillboard`, `ComplexGrassBillboard` | 1, 4 | Which template is used for plain and complex grass. |
| `CSGrassOptimizations`, `CSGrassOptimizationsMipMaps`, `CSGrassOptimizationsBillboard` | 0 (auto), 2, `DynDOLOD_flat_2x2plain_lod.nif` | Community Shaders grass optimisation; −1 forces off. |
| `GrassBrightnessTop/BottomR/G/B`, `ComplexGrassBrightness…` | — | Brightness multipliers; season suffixes allowed. |
| `GrassGlowMap`, `GrassBacklightMask`, `ComplexGrass…` | 0 | 0 keeps the template texture, 100 uses white.dds, other n writes a 4×4 grey texture with value 255·n/100. |

**LODGen process control and seasons**

| Key | Default | Meaning |
|---|---|---|
| `LODGenThreadSplit` | 0 | 0 means one main thread per physical core. |
| `MaxLODGen` | 0 | Limit on concurrent LODGen processes; 0 = unlimited. |
| `MaxLODGenPerWorldspace` | 2 | Concurrent LODGen processes per worldspace when generating seasons. |
| `SeasonsCopy` | 1 | Copy the default .bto to the season names when nothing changed. |
| `SeasonsCombineAtlas` | 1 | One atlas shared by all seasons. |

### Keys that exist in the exe but not in the shipped INI

These names appear next to the INI loader [exe-string]. Their defaults are not in the shipped file. Treat them as optional and keep the code defaults unless they are observed in use:

`IgnoreLODModels, IgnoreXESP, LargeReferencesSupport, LargeReferencesDLL, BaseLODDLL, WorldMapSupport, WorldMapDLL, WorshipperACTI, WorshipperLargeRef, GrassLODFullAtlas, GrassLODMiniAtlas, CreateObjectLODAtlas(Map), Experimental, uLockedObjectMapLOD, AutoMap, IgnoreUDRZValue, RemoveUnseen, DontMergeShapes, FlagTrees, Swaps, Seasons, SeasonsSnow, AlphaAdjustAfter, CreateMipMaps, ExportLANDBinary, SeparateAlphaAtlas, ObjectLODSpecular, PortalBoxFix, UndersideWorkaround, AddLoadScreens, CreateArchive, DeleteFiles, DisableParentChildMatches, LinearInternal, FlatLODBacklightMask, NoAutoSnow (default list: loadscreen, _invis_, dyndolod_lod, snow, ice, glacier, water, effect, wetrock), RemoveModFileName, SeasonsIDs (WIN,SPR,SUM,AUT), SeasonsSnowAllowSignatures (STAT,MSTT,CONT), SeasonsSnowDisallowModels, SeasonsSnowDisallowEDID, IgnoreMiniAtlas, StitchPBRScale/RenderPBRScale/BillboardPBRScale, ObjectLODRMAOSFormat, ObjectLODSSSFormat, ObjectLOD/TreeLODMinMipMaps, SplitTextureMaxSize, MaxBoundsWarning/Error, MaxLODGenThreads(PerWorldspace), LODGenThreads, AtlasSpacing, ArchiveType/Level/Threads, FormID/DebugFormID`.

### Presets: how Low, Medium and High are built

When the user clicks Low, Medium or High, the whole rule table is cleared and rebuilt from rule files [doc: Help/Advanced-Mode, Mod-Authors]. The candidate files below are named in the `LoadRules` string run (`candles`, `fxglow`, `dynamic`, `static`, `ultra`, `_map.txt`) [exe-string]; which ones are loaded is checked against the shipped files [cfg: Rules\]:

| Part | File(s) | When loaded |
|---|---|---|
| Base preset | `DynDOLOD_SSE_{low,medium,high}.ini` | Always. |
| Always rules | `DynDOLOD_SSE_all.ini` | Always: never-LOD masks such as sky, magic, 1stperson, swirl and mist volumes, plus vanilla FormID `Delete`/`Far LOD` fixes. |
| Dynamic/static variant | `DynDOLOD_SSE_all_dynamic.ini` or `_all_static.ini` | Depends on whether Dynamic LOD is checked. The same mask gets Grid + flag 9 (VWD + Dynamic) in the dynamic variant and flag 1 in the static one. [inferred from content] |
| Candles | `DynDOLOD_SSE_candles_all.ini`, then `DynDOLOD_SSE_candles_{low,medium,high}.ini` | If the Candles box is checked. `_all` holds negative rules for the "…off" (unlit) variants. Low and Medium put candles in Near LOD, High in Far LOD. |
| FX glow | `DynDOLOD_SSE_fxglow_{low,medium,high}.ini` | If the FXGlow box is checked. Low uses Near only, Medium mixes Near and Far by brightness, High uses Far only. |
| Per-plugin | `DynDOLOD_SSE_<plugin>.ini`, `…_<plugin>_{low,medium,high}.ini`, `…_<plugin>_ultra.ini` | Only when that plugin is in the load order. `<plugin>` is the plugin file name lowercased with non-alphanumerics (except `_`) removed, e.g. `legacyofthedragonbornesm`. The `_ultra` variant (e.g. `bentpinesesp_ultra.ini`) is presumably used when Ultra is checked. [doc: Mod-Authors], [cfg], ultra [inferred] |
| Map | `…_map.txt` | Used by world-map mod support ("A Clear Map of Skyrim"). [exe-string], details unknown |

Each rule file can contain these sections [doc: Mod-Authors], [cfg]:
- `[Skyrim LODGen]`: rules, always applied. The section name is `<GameName> LODGen`.
- `[Skyrim <otherpluginesp>]`: rules applied only if that second plugin is also loaded (compatibility).
- `[Skyrim Settings]`: `IgnoreWorlds=`, `IgnoreParentWorlds=`, `IgnoreChildWorlds=`, `IgnoreSignatureParentWorld=`, `IgnoreSignatureChildWorld=`, `DefaultLOD=`, `ForceParentCells=` [exe-string LoadPluginSettings]. IgnoreWorlds and IgnoreParentWorlds are used in shipped files [cfg].
- `[DynDOLOD]`: INI key overrides.

How the three base presets differ (our summary of the shipped files) [cfg: Rules\DynDOLOD_SSE_{low,medium,high}.ini]:

| Aspect | Low (63 rules) | Medium (48) | High (47) |
|---|---|---|---|
| Final catch-all `\` | Level0 / Level1 / Level2, Far LOD, VWD | same | same |
| Small objects with LOD models excluded (empty LOD columns + VWD) | Many: dirt cliffs, roads, snowdrifts, frozen marsh, small and medium rocks, rock piles and cairns, tundra streams and ponds, fences, floor chunks, stone walls, rubble, Dwemer clutter, and more | Fewer: hay, roadchunk, snowdrifts, small rocks, rock piles, ice piles, some block sets, braziers | None. Roadchunk is promoted to Level0/1/2. |
| Coarser model at near levels | Rocks Level1/1/2. Mountain trims and slopes start at Level1 or Level2. Windhelm bridge Level1. | Rock cliffs 03/04/06 LOD4 only. A few buildings Level0/1/1. | Similar to Medium, plus a Full-model rule for a Dwemer column. |
| Trees (`tree` mask, flags 17 = VWD + TREE) | Billboard ×3, no Grid | Billboard ×3, Near LOD | Level0 (3D model) at LOD4, Billboard at LOD8/LOD16, Far LOD |
| Water planes, streams, waterfalls in dynamic LOD | Mostly none; waterfall Near | Waterfall and creek Near | More: thin waterfalls Far, mineral pools and tundra streams Near |
| Fires and FX | Near / Far | Near / Far | Adds fire-with-embers Near; fires Far |
| FormID-specific rules | — | — | A few vanilla FormIDs |

Child-to-parent copy presets are separate. They select the `IgnoreChildEDIDLow`, `IgnoreChildEDIDMedium` or `IgnoreChildEDIDHigh` list in `Configs\DynDOLOD_SSE_childworld_<World>.ini` [doc: Help/Child-Parent-Worldspace-Copies].

## Rules and mesh masks

### Rule line format

```
[Skyrim LODGen]
LODGen1=<mask>,<LOD4>,<LOD8>,<LOD16>[,<LOD32>],<Grid>,<Reference>,<Flags>
```

- Keys are numbered from `LODGen1` with no gaps or repeats, separately within each section [doc: Mod-Authors].
- Two widths are in use [cfg: counts over all SSE rule files: 2209 seven-field rows, 35 eight-field rows]:
  - 7 fields: no LOD32 column. LOD32 then follows LOD16 ("by default LOD32 uses LOD16") [doc: Help/Mesh-Mask-Reference-Rules]. The exe has a `Level32Rule` string [exe-string].
  - 8 fields: explicit LOD32, as in the UI (LOD Level 4/8/16/32) and the saver format `%s,%s,%s,%s,%s,%s,%s,%d%s` [exe-string].
- Parse by field count: 7 means no LOD32, 8 means LOD32 present [inferred]. One shipped file has a malformed `0[Skyrim LODGen]` on a line, and a few rows have a backtick typo. A parser should be lenient: read the leading integer from the flags field, and treat an unknown level token as empty [inferred].

**Mask field**
- Mesh mask: a case-insensitive substring of the base record's full model path, relative to `meshes\`. There are no wildcards. Backslashes are literal path separators, so they can anchor a match at a folder boundary. For example `\rockl` matches a file name beginning with `rockl`, `\hay\` matches a folder, and `sky\` matches any folder ending in `sky` [doc: Help/Mesh-Mask-Reference-Rules], [cfg].
  - A bare `\` matches every model. It is the final catch-all.
  - A mask ending in `.nif` effectively pins an exact file name.
- Reference rule: `plugin.ext;00XXXXXX`. The load-order byte is written as `00`; the plugin name fixes the identity [doc: Mod-Authors].
  - Patch files (below) may attach rules to new references with `Rule=;;;Far LOD;Keep Both;0` (`;`-separated, same column order without the mask) [cfg: *.patch].

**LOD4 / LOD8 / LOD16 / LOD32 columns.** Accepted tokens, from the exe value list `Full Model, Static LOD4, Static LOD8, Static LOD16, Static LOD32, Billboard, Billboard1..6` and observed files [exe-string], [cfg]:

| Token | Meaning |
|---|---|
| empty or `None` | No object LOD at this level. |
| `Level0`..`Level3`, or legacy `Static LOD4`/`Static LOD8`/`Static LOD16`/`Static LOD32` | Use the discovered LOD model for model level 0, 1, 2 or 3. The legacy names map in order: Static LOD4 = Level0 … Static LOD32 = Level3 [inferred: catch-all `\,Static LOD4,Static LOD8,Static LOD16` matches the documented Level0/1/2 default; `Static LOD8` in the LOD4 column means a coarser model]. The column chooses which model *file level* goes into that LOD *tile level*, so `Level1` in the LOD4 column is valid. |
| `Full`, `Full model`, `Full Model` | Use the full model itself in this LOD level, subject to LODGen limits (no animation, alpha threshold 128). |
| `Billboard`, `Billboard1`..`Billboard6` | Tree billboard template n from `TreeBillboardn`. Bare `Billboard` means Billboard1 [inferred]. Valid only when the base is a tree. |

**Grid column (dynamic LOD).** Exe value list `Near LOD, Near Full, Far LOD, Far Full, Never Fade LOD, Never Fade Full` [exe-string]. Files also use `NeverFadeLOD`, `FarLOD`, `None` and empty, so the parser should ignore spaces when comparing [cfg], [inferred].

| Value | Meaning |
|---|---|
| empty or `None` | No dynamic LOD. |
| `Near …` / `Far …` | Enabled while the player is within the NearGrid or FarGrid diameter. |
| `Never Fade …` | Is Full LOD flag: visible at any distance. |
| `… LOD` | Use the dynamic LOD model (Level0 / `_dyndolod_lod`) if one exists, else the full model. |
| `… Full` | Always use the full model. |

**Reference column.** Exe value list `Unchanged, Replace, Disable, Keep Parent, Keep Child, Keep Both, Delete, Original, Enable, Ignore`. The form hints also list `Copy` [exe-string], [doc: Help/Mesh-Mask-Reference-Rules]. Files also use `None` and `FarLOD` in this position in 8-field rows; those are really the Grid in a shifted column, not a reference value [cfg].

| Value | Effect |
|---|---|
| `Unchanged` | Default. The LOD switches off and the original shows when its cell attaches. |
| `Replace` | Original disabled. A new full-model copy is placed and used for LOD regardless of Grid (windmill fans, water wheels). |
| `Disable` | Original disabled; LOD switches off as usual. |
| `Keep Parent` / `Keep Child` / `Keep Both` | Original loads normally and the LOD is not switched off, in the parent world, the child world, or both. |
| `Delete` | Original disabled with no LOD. Levels and Grid are ignored. |
| `Original` | Set Is Full LOD (never-fade) on the original reference. Levels and Grid are ignored. |
| `Copy` | Original disabled; a never-fade copy is created. |
| `Enable` | Set the XESP enable parent to the player, then behave like Unchanged. This turns an initially-disabled or parented ref into one that can have static LOD. |
| `Ignore` | Set the Ignore flag on the reference; no LOD. |

**Flags column.** An integer bitmask. The UI checkboxes are VWD, NoGlow, NoMATO, Dynamic, TREE, and "Show on worldmap" [exe-string DFM]. Bit values are inferred from usage [inferred, cfg]:

| Bit | Flag | Meaning | Usage evidence |
|---|---|---|---|
| 1 | VWD | Apply the rule only if a LOD model (or tree billboard) was found for this base. Otherwise skip the rule and keep matching further down. | Catch-all and most level rules use 1. |
| 2 | NoGlow | Do not add glow LOD windows. | A building rule uses 2. |
| 4 | NoMATO | Do not apply the snow/ash material-object shader to the LOD. | `windhelmbridge`=5. |
| 8 | Dynamic | Add both dynamic LOD and object LOD. | `fxwaterfallbody`: 9 in all_dynamic, 1 in all_static. |
| 16 | TREE | Also match every TREE base, and every STAT with the Has Tree LOD flag, that no earlier rule matched, regardless of path. | `tree`=17. |
| 32 | Map | Show the dynamic LOD reference on the world map. | Katariah ship 32, Gildergreen 33, Numidium 32; the hint text is "Show dynamic LOD reference on worldmap". The bit order follows the checkbox order; confirm by saving a rule. |

### Matching order and precedence

1. A reference FormID rule always beats any mesh-mask rule [doc: Help/Mesh-Mask-Reference-Rules].
2. Mask rules are tested from top to bottom. The first rule that applies wins, where "applies" includes the VWD gate: a VWD rule on a base with no LOD model falls through to the next rule [doc]. There is no longest-match logic. Specific masks must come before general ones; shipped files list `fxwaterfallmist` before `fxwaterfall`, `wrtempletreesapling` before `wrtempletree` before `tree`, and `\` last [cfg].
3. Rules from several files are merged by an add-or-update step: a rule with an identical mask probably replaces the existing one in place [exe-string `AddOrUpdateRule`; replace-in-place is inferred]. For the shipped files to work, the merged order must put per-plugin rules and the `_all` / `candles_all` negatives above the base preset's general masks and the `\` catch-all. For example, `candles_all`'s `candlehornfloor01off` must be tested before `candles_medium`'s `candlehornfloor01` [inferred]. The exact merge order is an open question; see the open questions at the end.
4. Negative rules: if all level columns and Grid are empty, matching models get neither object LOD nor dynamic LOD. With VWD set, this applies only to models that have a LOD model [doc], [cfg].
5. Grid-only rules (no levels, no VWD) mean dynamic LOD from the full model only. Typical for animated or FX meshes [doc].
6. If "Prefer base record LOD assignments over rules" (`UseOrgLOD`) is on, a STAT's own MNAM LOD assignments are used instead of rule-driven file-name matching. Plugins listed in `mod_default_lod.txt` always behave this way [doc: Help/Advanced-Mode, DynDOLOD-Reference], [cfg].

**2026-10-19 output qualification:** item 4 is not a safe unconditional
implementation rule. Making every no-level/no-grid/non-VWD match terminal removed
636 placements that exist in the golden. LODForge currently blocks full-model
fallback when such a match has no LOD resource, but preserves resource-backed
pass-through. Golden MODL records confirm full-model dynamic fallback for
`Unchanged` doors, rapids and parented trees. This does not establish `Replace`
semantics; see the [measured boundary](../validation-runtime-20261019.md).

### Patch files (`*.patch`)

These are applied at generation time to create or change records in the DynDOLOD plugins. They are loaded from `Rules\` and `Data\DynDOLOD\`; a same-named file in Data wins [doc: DynDOLOD-Reference], [cfg header].
- Naming: `DynDOLOD_SSE_<plugin>_{all|<WorldEDID>|largerefbugs|…}.patch`. `_all` always applies; `_<World>` applies when that world is generated. `_dynamic.patch`, `_static.patch` and an ultra variant are selected by options [exe-string `_all.patch`, `_dynamic.patch`, `_static.patch`, `LargeRefBugs`, `ultra`], [cfg file names].
- Line syntax: `//` starts a comment, otherwise `verb=plugin;FormID,Path\To\Element=value,…`.
- Verbs [exe-string], [cfg]:
  - `overwrite`: in place, into the DynDOLOD plugin.
  - `overwrite2esp`, `overwrite2esm`: force the target plugin.
  - `overwriteMaster`, `overwriteITM`.
  - `copy`: a new base record with an EDID suffix `_DynDOLOD_BASERECORD`.
  - `copy2esp`, `copy2esm`.
  - `delete`.
  - `addcell[...]`.
- Special pseudo-fields:
  - `REFR=new`: create a reference in that CELL.
  - `NAME=`: the base.
  - `CELL=`.
  - `Rule=` `;`-separated rule columns.
  - Element paths use xEdit names, e.g. `DATA\Position\X`, `Record Header\Record Flags`, `XCLC\Land Flags`.
  - The exe can also parse a `(pos|rot|scale|flags)([arc]?)(…)` mini-syntax [exe-string].
- `Configs\DynDOLOD_SSE_parentcopy_ignorepatches.txt` lists patch paths to skip when parent copies are made [cfg].

### Config text files (`Configs\DynDOLOD_SSE_*.txt`)

General rules: `//` starts a comment. Paths are lower-case with backslashes, relative to Data (with `meshes\` prefix) unless noted. Plugin+FormID is written `Plugin.ext;XXXXXXXX` with the load-order byte ignored.

| File | Line format | Meaning |
|---|---|---|
| `mesh_lookup.txt` | `meshes\actual\full.nif=meshes\pretend\full.nif` | For LOD discovery, treat the left full model as if it had the right name, so its LOD files are found under the "should-have-been" name. [cfg header] |
| `mesh_replace.txt` | `vanilla\model.nif=replacement\model.nif` (no `meshes\`) | If the replacement exists, swap it onto the base record. The replacement inherits the rules of the vanilla path. Also used by patch `copy` records. [cfg header], [exe-string] |
| `default_lod.txt` | One LOD mesh path per line (1017 lines, all `meshes\…_lod*.nif`). | The vanilla LOD model set: LOD files the base game ships and assigns in MNAM. Probably used to tell "vanilla default assignment" from mod-provided LOD, and to know these exist inside BSAs. [cfg] for content; purpose [inferred] |
| `mod_default_lod.txt` | One plugin file name per line. | For these plugins, use the LOD assignments already on their base records instead of file-name matching. [cfg header] |
| `misnamed.txt` | One mesh path per line. | Files whose names contain `_lod` but are really full models, or are otherwise mis-named. Excluded from `*_lod*` auto-discovery. [cfg header], [exe-string `Misnamed`] |
| `mesh_crc32.txt` | `meshes\path.nif;CRC32HEX`, grouped under `// <game edition version>` comments. | Known CRC32s of vanilla full models (SE 1.5.97 and AE 1.6.x). If the installed full model's CRC is not listed, a replacer has overwritten it; the plain-named LOD model may then not fit, so CRC-named LOD files decide. The report prints `[Original CRC32:]` / `[Patched CRC32:]`. [cfg header], [exe-string]; exact fallback [inferred] |
| `manual_static_<World>.txt`, `manual_static_<plugin>_<World>.txt` | A raw tab-separated LODGen export row: refFormID, baseFormID, x, y, z, rx, ry, rz, scale, EditorID, flags hex (e.g. `00008000`), then per-level LOD model paths (some empty). | Rows copied verbatim into the LODGen export for that world (static LOD with no reference). [cfg header], [exe-string "Adding %d manual object LOD entries"] |
| `manual_base_<World>.txt` | `TYPE,EDID,origMODL,LODMODL,DNAM angle,DNAM mat plugin,DNAM mat FormID` | Create a DynDOLOD base record. [cfg header] |
| `manual_refr_<World>.txt` | `edid,origPlugin,origFormID,initiallyEnabled,switch,…,basePlugin,baseEDID,px,py,pz,scale,rx,ry,rz,model=texture-override` | Create a DynDOLOD reference. [cfg header] |
| `childworld_<ChildEDID>.ini` | INI, section `[<ParentEDID>]` with keys `ScanChild`, `ScanParent`, `NoCellsWithNAVM`, `NoScanIfPluginExists`, `NoScanChildIfPluginExists`, `NoScanParentIfPluginExists`, `IgnoreChildEDIDLow/Medium/High`, `IgnoreParentEDID`, `IgnoreChildMesh`, `IgnoreParentMesh`, `Bounds=SWx,SWy,NEx,NEy`, `BoundsIgnore`, `NoDynamicLOD`. | Child↔parent reference copying. A child world is considered only if this file exists. Bounds also marks where trees go to object LOD, to work around the tree-LOD-in-child-world bug. [cfg], [exe-string], [doc: Help/Child-Parent-Worldspace-Copies] |
| `childworldfull.txt` | `Plugin;FormID;EDID` | Reference or base entries always copied as full models into the parent. [cfg]; semantics [inferred] |
| `childworldlod_<Parent>.txt` | `Plugin;FormID;EDID` | Child refs given LOD in the parent. [cfg]; semantics [inferred] |
| `ChildworldMatches.txt` | `childPlugin;FID;parentPlugin;FID;<xEdit description>` | Known child/parent duplicate pairs, so copies are not made twice. [doc], [cfg] |
| `glow_{low,medium,high}.txt` | Glow mesh masks to bake into static LOD (needs LODGen 0.9+). All shipped files contain only comments. | The preset-matched file supplies extra glow masks. [cfg header] |
| `flat_normals.txt` | Three blocks (normals, tangents, bitangents), each with 4 `x,y,z` lines, one per quad vertex. | Applied to billboard quads in static LOD. SSE needs normals and tangents for lighting and vertex colours. [cfg header] |
| `Required_Billboards_<plugin>.txt` | `Plugin;FormID;` (optional third field, empty in shipped files) | Bases that must have a TexGen billboard. Missing ones are reported or forced. [cfg], [exe-string `GetRequiredBillboards`]; exact action [inferred] |
| `Required_Resources.txt` | `path;CRC32` (6073 lines) | Checksums of DynDOLOD Resources files, used to verify the install. [cfg]; purpose [inferred] |
| `missing_meshes`, `missing_textures`, `missing_scripts` | `Plugin;FormID;path` | Suppress warnings for known-missing vanilla assets. [cfg header] |
| `worldspace_ignore.txt` | `Plugin;FormID;EDID` | Worlds hidden from the list. [cfg] |
| `mod_world_ignore.txt` | `plugin.esp=World,World` | Ignore every reference a plugin adds to those worlds. [cfg header] |
| `xesplod.txt` | `Plugin;FormID;EDID` | Enable parents to ignore. Their children get static LOD if initially enabled, or no LOD if initially disabled. [cfg header] |
| `xespignore.txt` | `Plugin;FormID;EDID` | References with these enable parents get no LOD at all. [cfg header] |
| `wildedit_ignore.txt` | `Plugin;FormID` | Exceptions for the wild-edit check. [cfg], [exe-string `WildEditCheck`] |
| TexGen and texture files (`TexGen_{copy,alpha,noalpha}_<plugin>.txt`, `texture_replace`, `texture_sets`, `textures_names`, `textures_simplify`) | — | Covered in the TexGen/atlas section. |

## LOD model selection

### Which base records and references are considered

- **Base types.** `BaseSignatures` defaults to `STAT,TREE,ACTI,MSTT,CONT,FURN,DOOR` [cfg], [exe-string]. GRAS is handled by grass LOD and LIGH by fake lights / LIGH2JSON [doc: DynDOLOD-Reference, Help/Glow-LOD]. Only STAT has MNAM LOD slots and the Has Distant LOD record flag (0x8000); the export rows carry `00008000`. Other types, ACTI for example, can only get LOD by file-name matching or rules [doc: DynDOLOD-Reference], [cfg: manual_static].
- **Excluded by configuration.**
  - Bases whose EditorID contains an `IgnoreBaseEDID` entry [cfg].
  - Models under `IgnoreFullMesh` paths [cfg].
  - NIFs whose root is not in `ValidRootBlocks` [cfg].
  - References added by `IgnoreModFileName` plugins [cfg].
  - References added to worlds listed for a plugin in `mod_world_ignore` [cfg].
  - References whose enable parent is in `xespignore` [cfg].
  - Worlds flagged as small, unless forced [cfg comment].
- **References scanned.** Exterior REFRs in the selected world (persistent cell plus temporary cells) whose base has a LOD model or matches a rule. Deleted references are not used. When deleted large references are present, the tool tells the user to clean the plugin first [exe-string]. Duplicate FormIDs among tree references exclude those references from LOD [exe-string].
- **Static or dynamic.** The rule's level columns give object LOD and the Grid gives dynamic LOD. The catch-all `\` (Level0/1/2, Far LOD, VWD) means:
  - a normal, initially enabled reference with a LOD model goes to object LOD;
  - a reference that has an XESP enable parent, or is Initially Disabled, goes to dynamic LOD on the FarGrid instead, so the LOD follows the reference's enable state;
  - an animated model needs a Grid-only or Full rule to get dynamic LOD [doc: Help/Mesh-Mask-Reference-Rules, Mod-Authors].
  
  Enable parents listed in `xesplod.txt` are treated as having no parent [cfg]. The `Dynamic` flag forces both [doc].
- **Never-fade (Is Full LOD) references.** At generation, the Is Full LOD flag on matching references is cleared, and the reference gets static or dynamic LOD instead [doc: DynDOLOD-Reference step 6]. The `Original`/`Copy` rule values and the expert "Generate Neverfades" option go the other way: they set Is Full LOD (Generate Neverfades skips large references) [exe-string].
- **Swaps.** Base Object Swapper and seasons swap files are read (`*_SWAP.ini`, `po3_SeasonsOfSkyrim.ini`, `mainformswap_*.ini`). `SwapsChance` decides what happens with random-chance swaps [exe-string], [cfg].

### Flags DynDOLOD writes

- **Has Distant LOD** (STAT record flag) is set on base records that get object LOD, but only where needed [doc: Mod-Authors, DynDOLOD-Reference step 13].
- With `ConvertEnabledReferences=1`, a reference with an enable parent is pointed at a copy of its base without Has Distant LOD (EditorID suffix `_DynDOLOD_BASERECORD`), so object LOD does not include it [cfg], [doc: step 16].
- **Visible When Distant** on REFRs is set or cleared to match whether the reference is in object LOD [doc: step 16], [exe-string "needs to have it's Visible when Distant flag set …"].
- **MSTT large-reference flag** (DATA flags 0x4) is restored when an old-form-version override lost it [doc: Help/Large-References], [exe-string].
- Zero-size object bounds are restored from the master. PortalBox primitives become Box on non-SE games [doc: step 19].
- Rule-driven reference edits: Ignore flag, Initially Disabled, XESP enable parent set to the player (`Enable`), Is Full LOD (`Original`/`Copy`) [doc], [exe-string].
- **Partial Form flag** on WRLD and persistent CELL overrides, per `PartialWorldspace` and `PartialPersistentCell` [cfg].

### Large references (SSE)

- **Large reference.** Listed in the world's RNAM by the ESM-flagged plugin that adds it, base is STAT or MSTT (MSTT needs flag 0x4), and base bounds magnitude × scale > the `fLargeRefMinSize` global [doc: Help/Large-References], [exe-string `fLargeRefMinSize`, `IsLarge`, `RNAM`]. DynDOLOD can add RNAM data itself (`AddRNAM`, "Adding %d large references") [exe-string], [doc: step 20].
- **Dynamic LOD.** The FarGrid starts beyond `uLargeRefLODGridSize` and the NearGrid beyond `uGridsToLoad`.
  - Large refs always go to FarGrid.
  - NearGrid large refs are skipped unless `UpgradeLargeReferences` is on.
  - Non-large refs keep their rule's grid unless `DowngradeNormalReferences` is on [doc: Help/Dynamic-LOD, Help/Advanced-Mode], [exe-string "Skipping large reference in NearGrid", "Upgrading large reference to FarGrid"].
- **Object LOD.** Large references are included like any other. The game unloads their LOD per cell. Initially-disabled or ESP-overridden large refs trigger the engine bug; DynDOLOD reports or works around it (`_DynDOLOD_FIX`, moving refs, `largerefbugs.patch`) [doc], [exe-string], [cfg file name].

### LOD model discovery and per-level choice

1. **Scan.** Walk `Folder1..5` in order, including BSA contents, collecting files whose name contains `_lod`. Skip `IgnoreLODMesh` partial paths and `misnamed.txt` entries [cfg], [doc: DynDOLOD-Reference step 1]. The key is the bare file name with no path; a file found later replaces an earlier one with the same name [cfg comment]. Exceptions to the "unique file name" convention are not supported [doc: Mod-Authors].
2. **Name a full model's LOD set.** Take the full model's file name without `.nif`, after applying `mesh_lookup` and `mesh_replace`, and look for these suffixes [doc: Mod-Authors, Help/3D-Tree-LOD-Model], [res], [exe-string]:

| Suffix | Use |
|---|---|
| `<name>_lod_0.nif` … `<name>_lod_3.nif` | Model levels 0..3 for object LOD. |
| `<name>_lod.nif` | Fills levels 0, 1, 2 when no numbered files exist; otherwise only the missing lower levels. |
| `<name>_lod_4.nif` | Fills level 3 only and never fills lower levels. |
| `<name>_<CRC32>_lod_N.nif`, `<name>_<CRC32>passthru_lod.nif`, `<name>_<CRC32>_dyndolod_lod.nif` | Preferred when the 8-hex CRC32 equals the CRC of the installed full model. CRC32 of the whole NIF file, uppercase hex; Resources mostly uses uppercase. For a mesh PGPatcher / ParallaxGen patched, the CRC before patching counts (see "ParallaxGen CRC map" below). |
| `<name>passthru_lod[_N].nif` | Same level semantics, but LODGen keeps the source lighting-shader settings ("passthru") instead of the default LOD shader. 3D tree LOD uses this form. |
| `<name>glow_lod_0/1/2.nif` | Glow-window overlay: `_0` NearGrid (LOD4), `_1` FarGrid (LOD8), `_2` neverfade (LOD16) when windows are dynamic; all in object LOD otherwise [doc: Help/Glow-LOD]. |
| `<name>_dyndolod_lod.nif` | Dedicated dynamic LOD model, used by `… LOD` grids. Can keep animation and effects. |
| `<name>_dyndolod_child.nif` | Model variant used for copies into child worlds [res]; exact use [inferred]. |
| `<name>_lod_Nh.nif` / `<name>_lod_Nl.nif` | Present for some mountains [res]. In the exe the suffixes `h.nif`/`l.nif` sit next to `_heavysn` and `DNAM\Material` [exe-string]. Probably variants chosen by the base's directional material (heavy vs light snow); unconfirmed. |
| `<name>_lod1.nif` etc. (no underscore before the digit; 92 files in Resources) | Exist in Resources. Whether they are discovered by the same pattern, or reached only through MNAM or rules, is unknown. |

   Level fallback: a missing lower level takes the next higher numbered file, so `_lod_1` alone fills levels 0 and 1 [doc: Mod-Authors].
3. **Fallback to the base record.** If no file name matches, or `UseOrgLOD` is set, or the plugin is in `mod_default_lod`, use the STAT's `MNAM Level 0..3` paths [doc: Help/Object-LOD], [exe-string `MNAM\Level 0`].
   The qualified LODForge fallback uses existing MNAM paths on HasDistantLOD
   bases only when all discovered levels are absent. Filename stems need not
   match: the CC Hall assigns Thirsk models. This is not full `UseOrgLOD` policy
   support, and discovered/CRC models still take precedence.
4. **Trees.** In priority order [doc: Help/3D-Tree-LOD-Model, Help/Ultra-Tree-LOD]:
   1. the CRC32-named `…passthru_lod.nif` 3D model;
   2. object LOD names `_lod_[0|1|2]`;
   3. the TexGen billboard `textures\terrain\lodgen\<plugin>\<name>_<FormID>_1.dds` (`_2`, `_3` for extra planes; `_n` for normals);
   4. `TreeFullFallBack`.
   
   A tree is a TREE base, or a STAT with Has Tree LOD [exe-string DFM hint for TREE flag]. Classic (non-ultra) tree LOD takes upright trees only. Trees tilted beyond `TreeLODMaxTilt`, trees inside child-world `Bounds`, and trees with duplicate FormIDs go to object LOD or are excluded [cfg], [doc], [exe-string]. Classic tree LOD supports at most 256 distinct tree types [exe-string].
5. **Per-level choice.** For each LOD tile level L in {4, 8, 16, 32}, read the matching rule column [inferred, consistent with docs]:
   - `LevelK` / `Static LOD…`: use model level K from step 2/3. If missing and VWD was set, the rule would not have matched. If missing without VWD, that level gets nothing.
   - `Full`: the full model.
   - `BillboardN`: billboard N.
   - empty / `None`: nothing.
   - LOD32 with no column: copy the LOD16 choice. LOD32 tiles are written only if `Level32` is on [doc: Help/Object-LOD].
   
   With Ultra, the `tree` rule's columns drive tree LOD inside object LOD. Without Ultra, trees go to classic .btt LOD unless forced to object LOD (tilt, child bounds).
   **ParallaxGen CRC map** [exe-string `LoadParallaxGen`, 0x1418c0a80] [golden debug log] [golden export].
   At start-up DynDOLOD reads `<Data>\ParallaxGen_Diff.json`, which PGPatcher / ParallaxGen
   writes next to its patched meshes: `{ "meshes\\...nif": { "crc32original": n,
   "crc32patched": n } }` (decimal CRC32 of the whole file before and after patching). The debug
   log prints `Processing <json> with <entries>` and then one `Processing <mesh> <original>
   <patched>` line per entry (1,713 for skyrim-cs). When the installed full model is the
   patched copy (its CRC equals `crc32patched`), CRC-named LOD discovery uses `crc32original`,
   so `<name>_<CRC32>_lod` models made for the unpatched mesh still match. Golden evidence:
   MarkarthWorld `000B39F7` uses `mrkmillrocks01_2ed49507_lod_1.nif`, and 2ED49507 is the
   original CRC of the patched `mrkmillrocks01.nif` (installed CRC 34DC7E45). The same log also
   prints the bare model name of 11 entries (`Reachtree02.nif`, `Floracabbage01.nif`, ...).
   These are exactly the patched meshes that have a TexGen billboard, and their sidecars carry
   the *patched* `ModelCRC32`. So the map probably also feeds the stale-billboard check; that
   is [inferred]. TexGen (which also contains `LoadParallaxGen`) writes the CRC of the installed,
   patched mesh into `ModelCRC32` [golden: 11/11 sidecars].
   Keyed by path in LODForge (`lodmodels::ParallaxGenDiff`, read as the last VFS winner); whether
   DynDOLOD keys by path or by CRC alone is not decompiled (the decompile timed out), but both
   give the same result unless two meshes share a patched CRC.

6. **Report.** Final assignments (Mask, File, LOD4/8/16/32, Grid, Reference, CRC32) go to `Logs\DynDOLOD_SSE_Object_Report.txt` and `…_Tree_Report.txt` [doc], [exe-string `Mask:`, `LOD4:` …]. The golden-output comparisons should diff against these reports.

### Open questions (configuration)

1. ~~Merge order of rule files~~ **Resolved by the golden run**: plain concatenation in the order `_all` → per-plugin (load order) → `_all_dynamic`/`_all_static` → base preset (see "Golden-run facts"). Still open: where candles / fxglow files go (not enabled in the golden run) and whether a duplicate mask replaces in place.
2. **Flag bit values.** Confirm 2 = NoGlow, 4 = NoMATO and 32 = Map by toggling the checkboxes and saving a preset.
3. **Model-level suffixes.** `_lod_Nh`/`_lod_Nl` = heavy / light snow variant (confirmed by the golden export). Still open: the underscore-less `_lod1.nif`.
4. **`default_lod.txt`.** Exact role; and what happens when a full model's CRC differs from `mesh_crc32.txt` and no CRC-named LOD exists (use the plain LOD anyway, warn, or skip?).
5. **Reference conditions.** Exact conditions for "dynamic instead of static", e.g. persistent refs and refs with XESP "Set Enable State to Opposite of Parent". Also how `Temporary=1` changes this.
6. ~~Preset file layout~~ **Resolved**: `[DynDOLOD]` UI keys plus `[Skyrim LODGen]` rules with a ninth source-file field (see "Golden-run facts").

## The LODGen export file: DynDOLOD vs xLODGen

Both tools write a LODGen input ("export") per worldspace and season and run LODGen on it.
`crates/objlod/src/export.rs` parses both: `ExportFile::parse` for xLODGen and
`parse_with(.., Flavor::Dyndolod318)` for DynDOLOD.

### Files and invocation [golden] [exe-string]

| | xLODGen (LODGen 3.0.36, net48) | DynDOLOD (LODGen 3.1.8, `LODGenx64Win10.exe`, .NET 8 → newest runtime) |
|---|---|---|
| Export | `Edit Scripts\LODGen[.<SEASON>].txt` | `Edit Scripts\Export\LODGen_SSE_Export_<W>[_<SEASON>].txt`; the previous file is kept as `.backup.<timestamp>` |
| Atlas map | `Edit Scripts\SSELODGen-AtlasMap<W>.txt` (one big atlas, `textures\terrain\<W>\Objects\<W>ObjectsLOD[NN].dds`) | `Export\LODGen_SSE_ObjectAtlasMap_<W>.txt` (atlas `textures\DynDOLOD\LOD\DynDOLOD_<W>[_Alpha].dds`, entries may point at TexGen mini atlases `textures\lod\*.dds`), plus a binary `.lst` twin |
| Side files | none | `AltTextures=`, `VC=`, `FlatTextures=`, `TerrainData=` (binary `.bin`), `MiniAtlasExport`, `TexturesUsed`, `TreeAtlasMap[_SEASON].txt/.lst`, `Export_TerrainUnderside_<W>.txt` |
| Command line | options as CLI switches (`--removeUnseenFaces`, …) | `--inputfile <export> --logfile <log>`; every option is in the export |
| Processes | one per season, sequential | one per season, concurrent (`MaxLODGenPerWorldspace`, default 2; here all 5 started within 1 s) |

### Header keys DynDOLOD writes (Tamriel, Medium) [golden]

`GameMode=SSE`, `Worldspace`, `CellSW=-96 -96`, `TerrainData=<bin>`, `LimitCoords4=` (884
`x,y` entries), `LimitCoords8=` (316), `RemoveUnseenFaces=True`, `DontMergeShapes=False`,
`TextureAtlasMap`, `AltTextures`, `FlatTextures`, `VC`, `CreateHeightMap=True`, 77 × `Resource=`
(every loaded BSA, full path), `Threads=0`, `ThreadSplit=0`, `UseDecalFlag=False`,
`Level32=False`, `UseOptimizer=false`, `UseHDFlag=true`, `Experimental=True`,
`NotHDMeshMask=nif`, `IsHDMeshMask=_lod_0`, `NotHDTextureMask=lod|lowres|architecture`,
`IsHDTextureMask=` mountain slabs, `dyndolod_atlas`, `flat_atlas` and `treelod*.dds`,
`PassThruMeshMask=glow_lod|passthru_lod`, `AtlasTolerance=0.2`, `RemoveUnseenZShift=10`,
`MergeVertexColors=true`, `FlatLODLevelLODFlag=0`, `{Crown,Trunk,FlatTrunk}Brightness{R,G,B}=1`,
`PathData`, `PathOutput`.

xLODGen writes only `GameMode`, `Worldspace`, `CellSW`, `TextureDiffuseHD`/`TextureNormalHD`,
`TextureAtlasMap`, `AtlasTolerance=0.5`, `PathData`, `PathOutput` and a list of
`IgnoreTranslation=`; its other switches go on the command line.

So `Options::from_export` starts from LODGen's built-in defaults (for example
`RemoveUnseenFaces` false) and applies the export's keys. The `Options::default()` used for
xLODGen encodes xLODGen's command line instead.

`LimitCoords` lists the south-west corners of the quads to generate.
- DynDOLOD's `OptimizeMode_<W>` (Tamriel 1 = "border": skip LOD4/LOD8 chunks the player can never
  reach) produces the default-season list.
- A season export's list names only the quads whose content differs from the default season. The
  other quads are copied from the default-season BTO, which the LODGen log prints as "Copied".
- An empty or absent key means all quads.

### Reference lines [golden]

Both use the same tab-separated column layout. Columns: refID, flags, x, y, z, rx, ry, rz, scale,
EDID, base flags, material, full model, LOD4, LOD8, LOD16, LOD32, enable parent, material swap
from/to, then 7 columns of extra transform.

| | xLODGen | DynDOLOD |
|---|---|---|
| Columns | 16: LOD4, LOD8, LOD16, then one empty field | 17: LOD4, LOD8, LOD16, LOD32 (empty unless `Level32`), then one empty field |
| Number formatting | position as the shortest round-trip double (`-45866.6796875`), rotation `%.4f`, scale `1.0` | everything `%.6f` (`-45866.679688`, `135.000000`, `1.000000`) |
| Model case | as stored (`Meshes\LOD\Farmhouse\Farmhouse02_LOD.nif`) | LOD paths lower-case, full model as stored |
| Base flags (col 10) | `00008000` (Has Distant LOD) | mostly Has Distant LOD 0x8000, plus 0x10000 (`00018800`), 0x20000 (the HD/large-ref bit LODGen tests for `UseHDFlag`, `00038800`) and 0x800 bits; `08000000`, `00000040` (tree) also occur |
| Material (col 11) | `-LargeRef` or empty | `-LargeRef`, `Snow-LargeRef`, `Snow`, empty. The snow material comes from the base's directional material |
| LOD models | MNAM / xEdit rules | rule columns (Level0..3, Full, Billboard), so `.dds` billboard "models" appear for trees (142 lines here) and are resolved through `FlatTextures` |
| Count (Tamriel) | 13,617 refs | 34,590 refs (Medium rules give many more objects LOD) |

### Side-file formats (moved; see object-lod-spec 7.3-7.4)

> Moved to the quarantine folder on 2026-10-20 (it described the side-file formats through the LODGen decompile). The observed format facts are in docs/re/object-lod-spec.md sections 7.3 and 7.4; the writer is `crates/dyndolod/src/sidefiles.rs`.

### How DynDOLOD derives the side files [golden] [debug-log]

Worked out from the E2E run of 2026-10-18 (PGPatcher-patched Faultier's PBR Landscapes; goldens and
`Logs\DynDOLOD_SSE_Debug_log.txt`) and implemented in `crates/dyndolod/src/sidefiles.rs`. Match rates
are in docs/baseline.md "AltTextures and VC side files".

**VC** (`[AddModel] VC Color in N <model> <shape>` is logged for every `True` shape):
- One line per shape of every full model in the export (each model once, each shape name once), in
  export order; a child worldspace copy's line names its LOD model as the full model, and so does
  this file. Shapes are walked from the NIF's root nodes (not block 0: some files start with
  `BSXFlags`), including those below hidden nodes.
- `True` when some vertex colour is not white. LE shapes (NiTriShapeData) are always `True`
  (DynDOLOD tests SSE vertex data only): `cotn_wr_bridge.nif WRHouseMeadery01:0.001` is all white
  and `True`, `farmhouse02.nif Farmhouse02:25` all white and `False`.
- Effect: a PBR-patched full model usually has white vertex colours (Faultier's meshes), so LODGen
  whitens the colours of the vanilla LOD model's shape with the same name.

**AltTextures** (`[BuildBaseRecords] Textures do not match for "<shape>": <full> in <model> ->
<lod> in <lod model>` per base, `[AddAltTextures] Processing <texture>` per reference):
- One line per reference with an export line (glow line and normal line merged); child worldspace
  copies get none. DynDOLOD also writes lines for references it processed without an export line
  (dynamic LOD, scanned child worldspace references); LODGen never reads those.
- **Full model**: every MODS alternate texture of the base, as stored: `index=shape` are the 3D
  index and name from MODS, which name the original mesh, not necessarily the PGPatcher copy the
  base now points at (`RockCaveEntrance01Rocks01Snow`: MODS `1=RockCaveEntrance01:1 - L2_Rocks:1`,
  copy shape 1 `RockCaveEntrance01:3`). Textures: the TXST set in NIF slot order (TX00 diffuse,
  TX01 normal, TX02 environment mask = slot 5, TX03 glow = 2, TX04 height = 3, TX05 environment =
  4, TX06, TX07), without the height map (slot 3). Written even when the diffuse equals the model's.
  MODS entries come first, last MODS entry first.
- **LOD models** (each distinct LOD column; `*glow_lod*` overlays skipped), per shape in LODGen's
  `numGeom` order:
  - full texture: the source is the base's MODS entry with the shape's name, else the full model's
    shape with the same name; an entry when its diffuse differs from the LOD shape's. Diffuse and
    normal only.
  - LOD texture (`textures\lod\`): TexGen's PBR variant (`dirt02lod.dds` → `dirt02pbr_lod.dds`,
    `mrkbuildingslod02.dds` → `mrkbuildingspbr_lod02.dds`) when it exists and some full-model shape
    uses a `textures\pbr\` diffuse. Diffuse and `_n`. The variant goes on the object atlas.
- **DynDOLOD.esm base copies** (dynamic LOD, `CreatePBRBaseRecord`): MODS on the
  `_DynDOLOD_BASERECORD` copy, one entry per shape of its model with the LOD-model swap above
  (MODS index = `numGeom`), plus the base's MODS texture on LOD-textured shapes of that name; each
  points at a shared TXST `<diffuse stem without linear>_DynDOLOD_TXST` (zero OBND, TX00/TX01
  without `textures\`, TX02-TX07 empty).
- A PBR diffuse with a `_rmaos` map (`textures\pbr\...\x.dds`) is written as
  `textures\pbr\...\xlinear.dds`, a copy DynDOLOD creates (`Creating ...linear.dds` in the log,
  `CreateNonAlphaImage`): half size, BC7 UNORM, RGB × 0.65 (its PBR scale) where
  opaque, darker and with a sharpened alpha where the source alpha is low (`snowrocks01linear.dds`
  with source alpha ~0 is black and transparent). LOD uses the vanilla lighting shader. Rare
  variant not reproduced: `snow01noalphalinear.dds`.
- Paths lower case; each of the ten texture fields followed by a comma; entries deduplicated by
  their whole text, so a key can repeat with different textures (full model with the whole set,
  LOD model with two). Entry order otherwise differs between references with identical inputs.

### What LODGen does differently for DynDOLOD

> Moved to the quarantine folder on 2026-10-20 (a summary of the LODGen 3.1.8 vs 3.0.36 decompile delta). The export dialect differences that matter to LODForge are in docs/re/object-lod-spec.md section 1.1.

## Plugin records (DynDOLOD.esm/.esp)

### Files and their roles

| Plugin | When created | Role |
|---|---|---|
| `DynDOLOD.esp` | always when dynamic LOD or any record change is made | main LOD patch: dynamic LOD references, new base records, flag changes, MCM quest |
| `DynDOLOD.esm` | when the large reference bugs workarounds are on (`WorkaroundLargeReferencesBugs`), i.e. DLL NG + NG scripts detected | "master" half. Holds whatever the engine only honours when it comes from an ESM-flagged file: large reference (WRLD `RNAM`) entries, large-reference related reference overrides and copies, and the persistent bookkeeping the DLL checks [doc: Large-References] [exe-string `CreateMasters`, `Master to work around game breaking bugs.`] [inferred] |
| `Occlusion.esp` | when "Occlusion data" + "Plugin" are checked | only CELL `TVDT` occlusion data (see Occlusion). ESL-flagged automatically when possible [doc: Occlusion-Data] |

Header details:
- Author (`CNAM`) is `Sheson` [exe-string]. Descriptions (`SNAM`) come from a fixed set of flavour strings ("Plugin generated by DynDOLOD", "Master to work around game breaking bugs.", "Plugin to work around game breaking bugs.", "Plugin waiting for game breaking bugs to be fixed.", and the legacy firstborns line) [exe-string]. Occlusion.esp uses author `xLODGen` and a description saying it is a generated plugin for TVDT occlusion data [exe-string `OcclusionPlugin`, `xLODGen`].
- Header version follows the game runtime. With Backported Extended ESL Support, or on runtimes ≥ 1.6.1130, header 1.71 is used and new form IDs may start below 0x800. Otherwise 1.70 is used and object IDs start at 0x800. If the load order mixes versions, DynDOLOD uses the highest version it found [exe-string: "Game runtime version %s does not support plugin %s version %g with form ID < 0x800...", `BackportedESLSupport`, `PluginVersion`].
- Form version: copied or overridden records are upgraded to the current form version (SSE 44) [exe-string "Upgrading form version %d to %d"].
- Load order: DynDOLOD.esm must be the highest-priority ESM, or sit just before the first non-ESM. DynDOLOD.esp goes as late as possible. Occlusion.esp loads last, right after DynDOLOD.esp [doc: Generation-Instructions, Updating]. The generator refuses to update an existing plugin that is not last of its type [exe-string "%s is not the last %s (%d < %d)"].
- DynDOLOD.esp must be a **full** plugin. The NG quest script checks that the load-order index of `DynDOLOD.esp:0x910` (form ID >> 24) is below `GetPluginLimit()` [psc: SHESON_DynDOLOD_Quest]. An ESL-flagged esp would sit at 0xFE and fail. The DLL also counts full and light plugins against their limits [dll-string "{} full plugins of {}, {} light plugins of 4096"].
- Master list: every plugin that contributes a record that is overridden or copied, plus whatever xEdit's copy-as-override adds. A master cannot be added if it loads higher than the target [exe-string "The required master ... can not be added ... higher load order"]. DynDOLOD and Occlusion can also hold data derived from non-master plugins [doc: Updating].
- Rolling updates: when an existing DynDOLOD.esp is updated rather than generated from scratch, the next free object ID is loaded from the old plugin, so existing form IDs stay stable [exe-string "Loading %s next object ID from %s for rolling updates", "Next object ID %x for %s", `SaveObjectID`/`LoadObjectID`].
- `PartialWorldspace=0` and `PartialPersistentCell=1` [ini] drive `SetPartialFormFlags` [exe-string]. This sets the Partial Form record flag (0x4000) on overridden WRLD and persistent CELL records, so the overrides carry only changed data [inferred].

### Fixed / well-known form IDs

| Local ID | Signature | Content | Evidence |
|---|---|---|---|
| `0x000910` (in both .esm and .esp) | KYWD | its **EditorID is the "bunch of numbers"**, a random token unique to one generation run. The NG MCM shows it, and the legacy scripts compare it with the data files | [psc: SHESON_DynDOLOD_Quest, SHESON_DynDOLOD_MCM, legacy SHESON_DynDOLOD_Firstborn] |
| `0x000903` | base used by glow-window dynamic refs; legacy scripts test `baseFormID & 0xFFFFFF == 0x903` to apply the distance-based emissive multiplier | [psc legacy: SHESON_DynDOLOD_LODObject] [inferred: glow base] |

A compatible generator must reserve these IDs and emit the KYWD in both plugins with the same EDID.

### Main ("framework") records: `AddDynDOLODMainRecords` [exe-string]

| Record | EditorID / content | Scripts and properties |
|---|---|---|
| QUST (MCM / init quest) | start-game-enabled quest | `VMAD` scripts: `SHESON_DynDOLOD_Quest` (props `PlayerRef`, `Active` int −1, `LastMessage` string "Initializing") and `SHESON_DynDOLOD_MCM` (props `MyQuest`→this quest, `PlayerRef`). One alias filled with the player (0x14) running `SHESON_DynDOLOD_Alias` (prop `Script`→quest). The alias re-runs `Initialize()` on every game load [psc: SHESON_DynDOLOD_Quest/MCM/Alias] [exe-string `VMAD\Aliases\Alias\...`, `MyQuest`] |
| ACTI or STAT "master" base | EDID `ThouShaltBowToObject`, `MODL markerx.nif`, `FULL`, `OBND` | — |
| REFR "master" (one per dynamic-LOD worldspace, persistent) | placed from the master base | `SHESON_DynDOLOD_Master` (props `PlayerRef`, `MyQuest`, `MyActiveFirstborn`, `MyCurrentFirstbornList`, `MyCurrentMinionList`). In NG it is only a compatibility shim. Its local form ID is written as `master=` in the per-world data file [psc: SHESON_DynDOLOD_Master] [exe-string `master=`] |
| ACTI `DynDOLOD_ObjectActivator` | marker-like activator | `SHESON_DynDOLOD_ObjectActivator` (prop `PlayerRef`). OnLoad/OnUnload asks the DLL to refresh (`SetMyActive(TRUE)`) [psc] |
| REFRs `*_DynDOLOD_ACTIVATOR` | object activators placed at specific objects, with `XESP` enable parent and position (`CreateObjectActivator`) | as above [exe-string] |
| STAT `DynDOLOD_DiscipleMarker` | marker with specific record flags | the DLL handles a "DiscipleAttach" event: an attach of these per-cell markers triggers processing [exe-string] [pdb `DiscipleAttach`, `DiscipleID`] [inferred] |
| REFRs `*_DynDOLOD_MARKER` | XMarker refs added per cell (`AddXMarkers`, `CellResetList`). Listed in the `[Markers]` section of the per-world data file | [exe-string] [dll-string "Marker {} {} not found"] |
| legacy only: ACTI `DynDOLOD_WorshipperActivator` (EDID `WorshipperACTI`), trigger-box REFRs `_Firstborn`/`_FirstbornActivator`/`_Minion`/`_Worshipper`/`_Disciple` (XPRM primitives) | PapyrusUtil mode | `SHESON_DynDOLOD_Firstborn`, `SHESON_DynDOLOD_Worshipper`, `SHESON_DynDOLOD_LODObject` [psc legacy] [exe-string `CreateFirstborn`, `CreateMinion`, `CreateWorshipper`, `CreateDisciple`]. **Not needed for NG**: the NG path is called "ScriptLess" in the generator (`DoDynamicLODScriptLess`, `SaveWorldsDataScriptLess`) [exe-string] |

### Dynamic LOD records (DynDOLOD.esp)

- **New base records** `<orig EDID>_DynDOLOD_BASERECORD` (also `_DynDOLOD_MAPBASERECORD` for world-map use, and `_Snow_DynDOLOD_BASERECORD`/`_Snow_DynDOLOD_MAPBASERECORD` for snow variants). Each is a copy of the original STAT/MSTT/ACTI/TREE/... base with `Model\MODL` pointing at the LOD model (or the full model for "Full" grids), optional `MODS` alternate textures, and `DNAM` material and snow fields. The copy is made with the AddNew, CopyNew or Overwrite action [exe-string `CreateBaseRecord`, "Overwrite"/"CopyNew"/"AddNew", `DNAM\Material`]. The base signature set comes from `BaseSignatures=STAT,TREE,ACTI,MSTT,CONT,FURN,DOOR` [ini].
- Helper suffixes for other new or changed records [exe-string]: `_DynDOLOD_TXST` (new texture sets for PBR or alt textures), `_DynDOLOD_STAGE` (destruction-stage aware copies), `_DynDOLOD_GLOW`/`_DynDOLOD_GLOWREFR` (glow-window bases/refs), `_DynDOLOD_CHILD` (copies into child worlds), `_DynDOLOD_PARENT`, `_DynDOLOD_FIX` (large-ref fixes), `_DynDOLOD_NOFLAG`, `_DynDOLOD_NOSCRIPT`, `_DynDOLOD_LAND`, `_DynDOLOD_REPLACED`, `_DynDOLOD_REVERT`, `_DynDOLOD_OBJECT`, `_DynDOLOD_BASE`, `_DynDOLOD_MAP`, `_DynDOLOD_LIST` (FLST). An `EditorIDFix` step uses a regex that matches `(esm|esp|esl)_XXXXXX`, which suggests EDIDs are formed from the source plugin name plus a 6-hex-digit form ID when the original has no EDID [exe-string] [inferred].
- **New references** `_DynDOLOD_REFERENCE`: one REFR per dynamic LOD object in each exterior cell of the LOD worldspace, placed with the original's position, rotation and scale [exe-string `CreateDynamicReference`, "Creating cell [%d,%d] with %d dynamic LOD references"]. Initially disabled is the expected state, because the DLL enables and disables them by NearGrid/FarGrid [doc: Dynamic-LOD] [inferred]. They are NeverFade/IsFullLOD so they render beyond uGrids [doc: Dynamic-LOD]. The link to the original reference goes in the per-world data file (`OriginalID`, `EnableParentID`, ...), not in record fields [pdb `DynamicREFR`]. Large references on FarGrid have `LargeReference`/`Persistent` set in that data [pdb] [exe-string "Upgrading large reference to FarGrid", "Skipping large reference in NearGrid"].
- **Child worldspace copies** (`_DynDOLOD_CHILD`): child worldspace references copied into the parent (golden2 Tamriel: 199 from Whiterun, Windhelm, Riften, Markarth, Solitude). Golden facts: a DynDOLOD.esm temporary REFR `<plugin>&<id>_DynDOLOD_CHILD` in the parent exterior cell at the same position/rotation/scale, no script, flag 0x8000 on static copies and tilted trees; its base is a `<EDID>_DynDOLOD_BASERECORD` copy (same record type) whose MODL is the LOD4 model (trees: the full model), or the original base for `childworldfull.txt` references; large copies are listed in DynDOLOD.esm's own Tamriel WRLD RNAM (which holds only DynDOLOD.esm references). Selection: see `crates/dyndolod/src/childworld.rs`. The ObjectEnabler / `MyWorldspaces` FLST mechanism is used by the terrain underside, not by these copies.

### Changes to existing records (overrides)

| Rule / feature | Record change | Evidence |
|---|---|---|
| Reference rule **Original** | set IsFullLOD (REFR flag 0x10000, "neverfade") on the original | [doc: Mesh-Mask-Reference-Rules] [exe-string "Creating Neverfade overwrite for"] |
| **Copy** | new REFR copy with IsFullLOD; the original is disabled | [doc] [exe-string "Creating Neverfade copy for"] |
| **Replace / Disable / Delete** | the original is disabled. The DLL keeps the list in `[Disabled]` and can move or disable at runtime ("Move {:08X}", "Disable {:08X}") | [doc] [dll-string] |
| **Enable** | `XESP` enable parent of the original set to the player (0x14) | [doc] [exe-string "Setting XESP Enable Parent to %s"] |
| **Ignore** | Ignore flag on the original | [doc] |
| object LOD bases | STAT `MNAM` Level 0..3 model paths plus the VWD/HasDistantLOD record flag (0x8000) set on the base (`UpdateBaseRecords`). With DLL feature `BaseLODDLL` (runtime ≥ 1.6.1130) these may instead be listed in `[Bases]` of the worlds data file for the DLL to apply at runtime | [exe-string `MNAM\Level 0`, `BaseLODDLL`, `UpdatedBaseRecords`, `[Bases]`] [inferred] |
| `ConvertEnabledReferences=1` | converts references whose enable state blocks LOD | [ini] [exe-string "Converting %d enabled references"] [inferred] |
| large references | moved to the correct cell ("Moved large reference from [x,y] to [x,y]"), permanently-initially-disabled ones converted, MSTT `DATA` flag 0x4 fixed, WRLD `RNAM` entries added (`AddRNAM`) in DynDOLOD.esm | [exe-string] [doc: Large-References] |
| portal boxes | `XPRM` primitive type Portal Box → Box (`PortalBoxFix`) | [exe-string] |
| trees (ultra) | `FlagTrees`: tree bases flagged for LOD. `TreeLargeRef` makes full trees large references when ≥ `TreeLargeRefSize` (400) and from ESM | [exe-string] [ini] [doc: Large-Reference-Bugs-Workarounds] |

ESM vs ESP: anything that must be large-reference-valid (RNAM, overrides of large refs, persistent large-ref copies) goes to the ESM. Everything else goes to the ESP [doc: Large-References] [inferred split].

Golden check: the ESM holds 1,500 REFR, mostly temporary large-reference copies and overrides, plus the new base records (296 STAT, 40 MSTT, 24 TREE …) and 12 WRLD with 1,314 CELL. The ESP holds 2,121 REFR, nearly all persistent, initially disabled and *Is Full LOD*: the dynamic LOD references. It also holds the 2 QUST and the FLST [golden].

### Large-reference workarounds (`WorkaroundLargeReferencesBugs=1`) [golden] [dll-decomp]

The engine stops unloading a cell's large-reference object LOD when one of the cell's large
references changes state at runtime. The workaround in the NG framework is per cell: when the
cell enters the large-reference grid, the DLL toggles a marker in that cell. The marker is itself
a large reference, so the toggle makes the engine re-evaluate the cell. The DLL also enables the
cell's persistent large references while the cell is inside the grid. DynDOLOD.DLL behaviour,
from LoadData 0x180030bf0 (`[Markers]` at 0x1800324fc–0x180032c6c) and LargeREFRsFixAttachDetach
0x180038270 (full notes in the side-session note `dll-markers-resets.md`):

- **`[Markers]` line**: `world,plugin,x,y,marker,plugin,count[,plugin;id]*`. The line is
  lowercased and split on `,`, and needs at least 7 fields (shorter lines are dropped without a
  log). Each trailing item is a whole `plugin;decimalId` pair. Unresolved items are dropped
  without a log.
- **Count**: for lines of the LOD world being loaded, the DLL compares the count with the
  engine's large-reference count for the cell (from RNAM). The higher of the two is kept ("Large
  References count {},{} {} > {}"), and the DLL stores it plus 3.
- **Marker toggle**: when the cell enters the `uLargeRefLODGridSize` grid, `LargeREFRFix` waits
  (about 5 s at most) for the marker to be enabled with its 3D loaded. It then disables and
  enables the marker count+1 times.
- **Persistent list**: the trailing references are enabled when the cell enters the grid and
  disabled when it leaves.
- **What identifies a marker**: nothing but its line. The DLL does not look up base 0x928 or the
  EditorID, and never checks that the marker really sits in cell x,y. The marker can be in any
  plugin. The "DiscipleAttach" event has a log label but no handler in 3.0.43.
- **Errors**: "Worldspace {} {} not found / not resolved", "Marker {} {} not found". A bad line
  never aborts loading.
- **Child worlds**: lines match the current world or its parent. In MarkarthWorld the Tamriel
  file is loaded, so Markarth's lines get the full fix and Tamriel's lines only the persistent
  enable/disable. Markers of child worlds that are not listed are never used.
- **`[Resets]`** (`decimalId,plugin`): each entry must be a REFR with a base. References flagged
  0x40000000 are skipped. When the Fader Menu closes after a load, `REFRsResets` (0x180034c30)
  runs the console command `recycleactor` on each one. Doors are only reset when their cell is
  outside the loaded grid.
- **`[Disabled]`**: once each, after loading and after grid changes. An enabled ACTI/STAT/MSTT/TREE
  is moved to z = -30000 and then disabled.
- **`[LargeReferences]`** in the worlds file is recognised, but its lines are ignored.
- A hard-coded table of 37 Skyrim.esm references (0x18010af40) turns large-reference fixing off
  for a world if any of them is deleted.

Generator rules (matched against the E2E golden; `lodforge large-refs` prints the analysis):

| Record / line | Rule | Golden match |
|---|---|---|
| STAT 0x928 in DynDOLOD.esm | `DynDOLOD_DiscipleMarker`, record flags 0x00800000 (Is Marker), `MarkerX.nif`, OBND −18,−20,0 / 18,18,16, DNAM max angle 90 | byte-identical fields |
| Marker cells, worldspace with its own LOD (Tamriel, MarkarthWorld) | each cell where one of the large references that RNAM lists *for that cell* (the entry's own grid, not the RNAM field's grid, which also lists neighbours) has an enable parent, is initially disabled, has its winning record in a non-master plugin, or is persistent without Is Full LOD | Tamriel 861/873 cells, no extra cells; Markarth 4/4 |
| Marker cells, child world on the parent's LOD with a childworld config (Whiterun, Riften, Solitude, Windhelm) | childworld INI `Bounds` widened by (uLargeRefLODGridSize − uGridsToLoad)/2 = 3 cells; with `NoCellsWithNAVM=1`, cells with a NAVM are left out; missing CELLs are created (`<World>&DynDOLOD&CELL&<x>&<y>`, DATA from the parent's cell, XCLC, LTMP 0, XCLW `FF FF 7F 7F`). Children without a childworld config get nothing | Riften 81/81, Windhelm 80/80 exact; Whiterun 100 of our 110, Solitude 98 of our 118 (DynDOLOD drops the north row / two east columns for a reason not found) |
| Marker REFR | in the cell's temporary group, flags 0x40000200, EditorID `<World>_Disciple_<x>_<y>` (a negative number is written `%n`), at the cell centre ((x+0.5)·4096, (y+0.5)·4096, 0) | as golden |
| WRLD RNAM (DynDOLOD.esm) | WRLD override copied from the winning record, its RNAM dropped. One RNAM right after EDID per marker: `y,x` (i16), count 1, (marker, y, x). The engine merges RNAM over all masters | as golden apart from 2-entry RNAMs holding `_DynDOLOD_CHILD` copies |
| `[Markers]` lines | one per marker of the world and of its own-LOD children (Markarth uses world `93553,Skyrim.esm`); count = RNAM entries listed in the cell + 1 (the marker); persistent list = the cell's persistent, initially disabled large references with no enable parent, winning record from a master and a base without Is Marker; lines sorted as text | count 862/865 lines, persistent lists 851/865 (434 items, golden 449) |
| `_DynDOLOD_REVERT` + `[Resets]` | persistent Is Full LOD references that get object, tree or dynamic LOD are overridden without Is Full LOD (copy of the winning record, EditorID `<plugin without punctuation>&<id6>_DynDOLOD_REVERT`). Large references go into DynDOLOD.esm's persistent cell, the rest into DynDOLOD.esp's. Each one is listed in `[Resets]` so saves reload it. Temporary Is Full LOD references are not touched | every golden REVERT is a persistent Is Full LOD reference and every `[Resets]` entry is a REVERT (57/57). Ours: 23, all in golden; the other 34 are references our export does not give LOD (12 Civil War banners, 5 CC tower/hall pieces, 19 Solitude refs copied by `_DynDOLOD_CHILD`) |

The counts above are historical. Child copies and replacement support have since
advanced; see the [current qualification](../validation-runtime-20261019.md).
Remaining policies include missing child copies, `_DynDOLOD_FIX`,
ConvertEnabledReferences and non-replacement Disabled selection.

#### Replacement follow-up [golden records] [ours] [live]

`Replace` selects the full source model, not a dedicated dynamic/LOD0 model,
because the generated copy must also render inside the loaded grid. The original
identity is retained as an XMarker (`Skyrim.esm:00003B`) at Z=-30000, with its
parent removed, XY/rotation/scale retained, and only its persistent flag kept.
The temporary/persistent cell placement is retained and the original is listed
in `[Disabled]`. Its EDID embeds the generated copy ID and ends `_DynDOLOD_REPLACED`.

The copy retains the source transform. Runtime Delay is true and both
world-switch fields are false. Its parent is the original explicit parent, or
itself when absent; replaced parents are remapped to their generated copies.
InitiallyEnabled reflects the original local flag, not the parent's startup
state. The DLL evaluates that parent at runtime. Replacements are not ordinary
Is Full LOD resets.

**ESM loading requirement:** temporary overrides must be registered in TES4
`ONAM`, sorted in the output master's form-ID space. New records, persistent
references, CELL and base-record overrides do not belong in this list. The
golden registers all 19 selected replacement originals. Without it, Skyrim
ignored the hidden originals and a parent toggle loaded duplicate fan geometry;
adding ONAM alone corrected the live result. A fresh original can still report
enabled/loaded 3D as an XMarker below the world; do not require its disabled bit
as the sole correctness check.

Patch-generated originals remain separate: Katla's Farm `Skyrim.esm:05C854`
is replaced by a generated patch reference at a corrected transform in the
golden. The current unpatched replacement must not be counted as patch parity.

### Matched child resets and tree timing [golden] [ours] [live]

The later [runtime batch](../validation-runtime-batch-20261019.md) also recovers
the 19 missing Solitude resets. A persistent full-LOD child with a known
parent counterpart receiving LOD still needs a REVERT even when it is excluded
from child copies. Write it to the child's persistent cell, not the parent's;
honor explicit child reference actions first. This gives 57/57 reset identities
and exact added-record fields/grouping on the pinned fixture.

NearLOD TREE copies do not acquire Delay solely from original persistence.
All 1,443 compared golden occurrences agree; this fixes 33 rows. Replacement
Delay and other grids/types retain their previous rules. Other Delay families
remain unresolved; this is not a universal timing predicate.

### Rolling updates (form-id stability) [exe-string] [ours]

DynDOLOD logs "Loading %s next object ID from %s for rolling updates" when it updates an existing
DynDOLOD.esp. LODForge does the same when the output already holds a LODForge DynDOLOD.esm/.esp
(TES4 CNAM `LODForge`). The new records of the old plugin are indexed by (signature, EditorID,
model). The model is a STAT's MODL; for a REFR it is the MODL of its base when the base is a new
STAT of the same plugin. Every record the build writes again takes its old object id. New
records get ids from the old HEDR next object id on. The run token (KYWD 0x910) is kept, so the
cosave's EDST/EDLR records (dynamic and large references by form id) stay valid. `--new-token`
draws a new token. `--renumber` numbers from scratch.

## DLL NG and scripts

### Components and version handshake

- `SKSE\Plugins\DynDOLOD.DLL` (CommonLibSSE-NG, spdlog, rapidcsv; source files Main, Events, CellEvent, PlayerEvent, MenuEvent, Papyrus, Patch, PatchHooks, Helpers) [pdb]. It logs to `Documents\My Games\<Skyrim Special Edition|Skyrim VR>\SKSE\DynDOLOD.log` [doc: Large-Reference-Bugs-Workarounds]. Address Library: `Data/SKSE/Plugins/versionlib-{ver}.bin` / `version-{ver}.bin|csv` [dll-string].
- Scripts and the native class: `SHESON_DynDOLOD_Plugin` (native globals), `_Quest`, `_MCM`, `_Alias`, `_Master`, `_ObjectActivator`, `_ObjectEnabler` [psc]. The generator checks that the NG scripts are present by scanning `scripts\sheson_dyndolod_*.pex` for specific native function names (`sheson_dyndolod_plugin`, `getsuccessful`, `getpluginlimit`, `getlargerefchildworlds`, `setmyactive`). Without them it falls back to PapyrusUtil mode [exe-string `LoadPex`, "DynDOLOD.DLL from DynDOLOD DLL NG and Scripts not found. Dynamic LOD will use papyrus scripts."].
- Version chain, checked at every load in `SHESON_DynDOLOD_Quest.Initialize()` [psc]:
  1. SKSE DLL and script release must match, and be ≥ `GetSKSEVersion()`.
  2. `GetVersion()` (DLL) must equal `GetScriptVersion()`, which returns **307** (data/plugin format 3.07) [psc: SHESON_DynDOLOD_Plugin].
  3. `DynDOLOD.esp:0x910` must exist as a KYWD with a non-empty EDID, and the plugin must be full (see above).
  4. After a 3 s wait, `GetSuccessful()` / `GetLastMessage()` report the DLL's own `Checks()`.
- DLL `Checks()` [dll-string]: TESDataHandler available. The worlds config parses ("problem with worlds config file"). `esmplugin`/`espplugin` names are present and loaded ("no ESM plugin filename", "{} not found"). The plugin versions match ("can not get version from {}", "{} is the wrong version {}"; MCM shows ESM/ESP versions as int/100) [psc: MCM]. The bunch of numbers from each plugin matches the data file ("can not get bunch of numbers from {}", "bunch of numbers {} in {} does not match {}"). The plugin counts are under the limits. It also reads `uGridsToLoad:General`, `uLargeRefLODGridSize:General` (`:VR` on VR), `fSkyCellRefProcessDistanceMult`, `fSkyCellRefFadeDistance`, `fTreeLoadDistance:TerrainManager`, `fBlockMaximumDistance:TerrainManager` and `fBlockLevel0Distance` [dll-string] [pdb]. The plugin version most likely comes from the TES4 description, which ends in "Plugin generated by DynDOLODVersion 3.07" in both golden plugins [golden] [inferred].

### Runtime data files (written by the generator, read by the DLL)

Location: `Data\SKSE\Plugins\DynDOLOD_Data\` [dll-string `Data/SKSE/Plugins/DynDOLOD_Data/DynDOLOD_`].
- `DynDOLOD_NG_Worlds.txt`: global config [dll-string `NG_Worlds`] [psc: MCM header "DynDOLOD_NG_Worlds.txt"] [doc: Large-Reference-Bugs-Workarounds].
- `DynDOLOD_NG_<LODWorldEditorID>.txt`: one per worldspace that has dynamic LOD, named after the worldspace whose LOD is used (`GetMyLODWorld`) [psc: MCM] [exe-string "DynDOLOD", "NG", ".txt"].

Format: INI-like text with `[Section]` headers. Section names are matched case-insensitively (the DLL compares lowercase, the generator writes Capitalised). Lines are `key=value` in `[Config]`, and data lines in other sections. On a parse error the DLL logs "Error {} parsing line {} in section {} in file {}" and on an unknown section "Unknown section {} in file {}" [dll-string].

`DynDOLOD_NG_Worlds.txt` [exe-string `SaveWorldsDataScriptLess`] [dll-string `LoadWorlds`]:

| Section / key | Meaning |
|---|---|
| `[Config]` `bunchofnumbers=` | run token. Must equal the EDID of KYWD 0x910 in both plugins |
| `esmplugin=` / `espplugin=` | plugin filenames (normally `DynDOLOD.esm` / `DynDOLOD.esp`) |
| `glowmultiple=` | glow window distance multiplier. The generator writes it from INI `fNeverMultiple` (default 1.75). MCM "Window Glow Multiplier" [ini] [exe-string] |
| `neargrid=` / `fargrid=` | NearGrid/FarGrid diameters in cells (odd; MCM offers values derived from `uGridsToLoad`) [psc: MCM] |
| `debug=false` | written by the generator. The user may set `true` for verbose logging [doc: Large-Reference-Bugs-Workarounds] |
| optional, user-set keys read by the DLL: `msdelay`, `dragondistance`, `debuglevel` | [dll-string] |
| `[Worlds]` | one line per worldspace → its LOD data. Errors "Worldspace {} {} not found / {:08X} not resolved" show each line names a form by (plugin, ID) [dll-string] |
| `[LargeReferences]` | large references per cell, used by the workarounds ("Large Reference Cell {},{} #{} = {:08X}", "Large References count {},{} {} > {}") [exe-string `LargeReferencesTXT`] [dll-string] |
| `[Bases]` | base records whose LOD the DLL updates at runtime (comma-separated fields) [exe-string `UpdatedBaseRecords`] [dll-string "Base {} {} not found"] |
| `[BOSRandom]` (read by the DLL) | plugins and their expected load-order IDs for Base Object Swapper fixed-chance swaps ("BOS fixed chance full/light plugin {} wrong load order ID") [dll-string] |

`DynDOLOD_NG_<World>.txt` [exe-string `DoDynamicLODScriptLess`] [dll-string `LoadData`]:

| Section | Content |
|---|---|
| `[Config]` | `bunchofnumbers=`, `master=` (form ID of the master REFR), `x=`, `y=` (LOD origin cell, `GetMyLODWorldX/Y`) |
| `[Markers]` | marker references per cell (`_DynDOLOD_MARKER`) |
| `[Disabled]` | original references DynDOLOD disables or moves |
| `[Resets]` | references to reset (`ReferenceResetList`). DLL struct `ResetREFR { coord, Door }`. Errors show (plugin, ID) addressing and checks "not a Reference" / "has no Base" |
| `[Objects]` | one line per dynamic LOD reference. Line content mirrors DLL struct `DynamicREFR` [pdb]: `EnableParentID`, `OriginalID`, `coord` (cell), `Level` (Near/Far/Never), `InitiallyEnabled`, `SwitchParentWorld`, `SwitchChildWorld`, `Delay`, `Fade`, `OppositeParent`, `LargeReference`, `Persistent`, `Door`, `Map`, `DestructionParentStageIndex`, `DestructionParentStageType`, `Names3D` (3D node names, used for emissive/glow) |

The legacy JSON object list gives the same field semantics, keyed by DynDOLOD.esp local form ID [psc legacy: SHESON_DynDOLOD_LODObject]:

| Index | Meaning |
|---|---|
| 0 | FarGrid |
| 1 | delay |
| 2 | initially enabled |
| 3 | switch with original in same world |
| 4 | invert parent state |
| 5, 6 | enable-parent form ID and plugin |
| 7 | destruction stage, or −2 = follow original transform |
| 8 | stage type −1/0/1 |
| 9 | (unclear) |
| 10 | switch in other world |
| 11 | glow |
| 12, 13 | original form ID and plugin |

The NG line grammar is not recoverable from strings; the golden run shows it:

#### Observed grammar (golden run, Tamriel) [golden]

All sections hold comma-separated fields. A form is written as **decimal local form ID** plus a
plugin file name (`69853,DynDOLOD.esp`, `441064,Skyrim.esm`); the load-order byte is not
written. Section names are capitalised.

- `DynDOLOD_NG_Worlds.txt`:
  - `[Config]` holds `bunchofnumbers=<32 alphanumerics>`, `esmplugin=dyndolod.esm`,
    `espplugin=dyndolod.esp`, `glowmultiple=1.75`, `neargrid=11`, `fargrid=21`, `debug=false`.
  - `[Worlds]` lists world EditorIDs, one per line, lower-case (`tamriel`).
  - `[Bases]` lists `formID,plugin` base records (no `[LargeReferences]` in this run).
- `DynDOLOD_NG_Tamriel.txt`:
  - `[Config]` holds `bunchofnumbers=` (same token), `master=69848,DynDOLOD.esp`, `x=-96`,
    `y=-96`.
  - `[Objects]`: 1,681 lines, always **17 fields**. Our positional reading (field meanings are
    hypotheses matched against the DLL's `DynamicREFR` members; confirm in game):

    | # | Values seen | Reading |
    |---|---|---|
    | 1–2 | `69849,DynDOLOD.esp` | the dynamic LOD reference |
    | 3 | 0 (1,317) / 1 (364) | grid level: Near / Far [inferred] |
    | 4–7 | `1` or empty | booleans, e.g. initially enabled, switch with parent / child world, opposite-parent [inferred] |
    | 8–9 | form + plugin | enable parent (differs from 15–16 on 869 lines) [inferred] |
    | 10–11 | `-1,-1` (rarely `4,1` …) | destruction stage index / type |
    | 12, 14 | always empty here | delay / names3D [inferred] |
    | 13 | `1` or empty | large reference / persistent [inferred] |
    | 15–16 | form + plugin | the original reference |
    | 17 | `1` or empty | door / map flag [inferred] |
  - `[Markers]` (E2E golden: 877 lines): `world,plugin,x,y,marker,plugin,count[,plugin;id]*`,
    e.g. `60,Skyrim.esm,-1,-14,82305,DynDOLOD.esm,5`, where 60 = 0x3C = Tamriel. Each trailing
    item is one `plugin;id` pair. See "Large-reference workarounds".
  - `[Disabled]`: 39 lines `form,plugin`.
  - `[Resets]`: 57 lines `form,plugin` (repeats allowed): the `_DynDOLOD_REVERT` references.


Legacy (PapyrusUtil) equivalents, not needed for NG: `SKSE\Plugins\StorageUtilData\DynDOLOD_Worlds.json` (string `plugin`, `masters`, `bunchofnumbers`, `debug`; int `largereffix`; floats `nearmultiple`, `farmultiple`, `nevermultiple`, `ugrid`, `ularge`, `neargrid`, `fargrid`, `fminsecondsforloadfadein`; world→LOD-world map) and `DynDOLOD_<World>.json` / `DynDOLOD_<World>_Objects.json` (`master`, `x`, `y`, `largereffix`, `stringList`/`intList` per firstborn/minion/cell, `reset`), each also as `.txt` with sections `[Worshippers]`, `[Brothers]`, `[Firstborns]`, `[Minions]`, `[Cells]`, `[Index]`, `[Objects]` [exe-string] [psc legacy].

### What the DLL does at runtime [pdb] [dll-string]

- **Events**: load start/end, fast-travel start/end, cell attach/detach, player cell enter/leave, player position, player death, menus (`Loading Menu`, `Fader Menu`, `MapMenu`, `HUD Menu`), world start/end. Each is queued into `DoProcess` per worldspace.
- **Grid tracking**: it keeps CellGrid (uGridsToLoad), LargeGrid (uLargeRefLODGridSize), NearGrid and FarGrid around the player. On change it runs `ObjectsUpdates`, which enables the `[Objects]` refs inside the Near/Far ring for their Level, disables them closer in (where the full model is loaded) unless Keep*, and honours enable parents, inversion, destruction stages, doors and child/parent world switching.
- **Large references** (workarounds): `LargeREFRsEnable/Disable/FixAttachDetach/PersistentEnableAll`, `LargeREFRFix`, and a patch on `IsValidLargeReference` that toggles large refs so their object LOD unloads correctly. `LargeRefChildWorlds` can turn large refs off in child worlds (MCM toggle).
- **LOD fixes**: `LODFix(bool,int)` and `FixLOD` re-apply tree/object LOD after fast travel. Hook `TerrainManagerForLODParent` handles child worlds that use parent LOD. `MapFix`/`MapEnd` handle world-map LOD (SkyCell). `SetfSkyCellRefProcessDistanceMult` changes dragon distance.
- **Glow**: `EmissiveUpdate(s)` scales emissive on named 3D nodes (struct `Emissive { aRefFID, names, multiplier, wait }`) by distance × `glowmultiple`.
- **Cosave** records: `CONF`, `CNF2`, `EDST`, `EDLR`, `GRID` (config values, enabled/disabled dynamic refs and large refs, grids).
- **Papyrus natives** (`SHESON_DynDOLOD_Plugin`): `GetVersion`, `GetSKSEVersion`, `GetESMVersion`, `GetESPVersion`, `GetBoNWorlds`, `GetBoNData`, `GetSuccessful`, `GetLastMessage`, `GetPluginLimit`, `IsPluginInstalled`, `GetMyMaster`, `GetMyLODWorld`, `GetMyLODWorldX/Y`, `GetWorldspaceLODNameStr`, `GetConfig(file,key,default)`, `Get/SetINIUnsigned` (also serves virtual names `NearGrid`, `FarGrid`), `Get/SetMyActive`, `Get/SetLargeRefChildWorlds`, `Get/SetGlowMultiple`, `Get/SetDragonDistance`, `GetResetsCount`, `GetResetAt` [psc] [dll-string].

## Output layout

DynDOLOD writes to the chosen output path (default folder name ends in `_Output`, i.e. `DynDOLOD_Output`), and on "Zip & Exit" archives it [exe-string `_Output`, `SetDefaultOutputPath`, `CreateArchive`] [doc: Generation-Instructions]. Tree names: `<W>` is the worldspace EditorID, `L` ∈ {4, 8, 16, 32}, `X`/`Y` are the SW cell of the block, and seasons are `.SPR/.SUM/.AUT/.WIN` before the extension.

| Path | Content | Evidence |
|---|---|---|
| `DynDOLOD.esm`, `DynDOLOD.esp`, `Occlusion.esp` | plugins | above |
| `meshes\terrain\<W>\objects\<W>.<L>.<X>.<Y>[.<SEASON>].bto` | object LOD (incl. ultra trees, glow, grass at L4) | [exe-string format `meshes\terrain\%s\objects\%s.4.%d.%d.bto`] [doc: Expert-Mode, Seasons] [psc: MCM] |
| `meshes\terrain\<W>\trees\<W>.4.<X>.<Y>.btt` and the trees `.lst` | billboard tree LOD (non-ultra). `CreateEmptyBTT` can write empties to suppress native tree LOD | [exe-string "Creating %d *.btt tree LOD files", `meshes\terrain\%s\trees\%s.%d.%d.%d.`] [ini] |
| `textures\DynDOLOD\LOD\...` object LOD atlas (diffuse, `_n`, plus separate Alpha / Glow / sRGB / sRGB_Glow atlases as needed) | atlas textures | [exe-string `textures\`,`DynDOLOD`,`LOD`,`Alpha`,`Glow`,`sRGB_Glow`] [inferred exact filenames] |
| tree LOD atlas under `textures\terrain\<W>\trees\` | tree billboards atlas | [exe-string `TreeLod.dds`] [inferred] |
| `meshes\terrain\<W>\objects\<W>.objects.heightmap.dat` | LODGen object heightmap for occlusion (`LODGenHeightMap=1`) | [exe-string] [ini] |
| `textures\terrain\<W>\occlusion\` | debug images when `OcclusionCreateImages` | [exe-string] |
| `SKSE\Plugins\DynDOLOD_Data\DynDOLOD_NG_Worlds.txt`, `DynDOLOD_NG_<W>.txt` | DLL data | above |
| `Seasons\DynDOLOD_<SPR\|SUM\|AUT\|WIN>.ini` (+ `_SNOW`/`_NOSNOW` variants) | Seasons of Skyrim swap files for DynDOLOD's own records | [doc: Seasons] [exe-string `SaveSeasons`, `_SNOW.ini`, `_NOSNOW.ini`] |
| `*_SWAP.ini` (Base Object Swapper) | swaps for dynamic LOD bases | [exe-string `DynDOLODSwaps`, `_SWAP.ini`] [inferred] |
| `SKSE\Plugins\FireHurtsRE\` JSON, `SKSE\XEMIUtil\` | optional 3rd-party integration files (FireHurts offsets/bounds; XEMI emittance copies) | [exe-string] |
| `lodsettings\<W>.lod` | read. Whether DynDOLOD rewrites it is open | [exe-string] |

Outside the output:
- `..\DynDOLOD\Logs\DynDOLOD_SSE_log.txt`, the summary of messages, `LODGen_SSE_<W>_log.txt` and `DynDOLOD_SSE_ChildworldMatches.txt` [doc: LODGen, Child-Parent-Worldspace-Copies].
- `..\Edit Scripts\Export\LODGen_SSE_Export_<W>.txt`, `LODGen_SSE_[Object|Tree]_Atlas_Map_<W>.lst` and the grass/complex-grass maps (`GrassMap=` / `ComplexGrassMap`) [doc: Expert-Mode] [exe-string].
- Temp texture work goes in a `DynDOLOD-temp` folder [exe-string].

LODGen is run as `"LODGenx64.exe" --inputfile "<export>" --logfile "<log>" [--LODLevel N] [--Verbose]` [exe-string]. Export header keys seen [exe-string]: `GameMode=`, `Worldspace=`, `CellSW=`, `TerrainData=`, `PathData=`, `PathOutput=`, `Resource=`, `TextureAtlasMap=`, `AltTextures=`, `FlatTextures=`, `GrassMap=`, `GrassDensity=`, `PathGrass=`, `GrassGID=`, `GrassLargeReference=`, `RemoveUnseenGrass=`, `CreateHeightMap=`, `RemoveUnseenFaces=`, `RemoveUnseenZShift=`, `DontMergeShapes=`, `IgnoreWater=`, `UseHDFlag`, `IsHDMeshMask`/`NotHDMeshMask`/`IsHDTextureMask`/`NotHDTextureMask`, `PassThruMeshMask`, `DoubleSidedMeshMask`, `UseAlphaThreshold`, `AtlasTolerance`, `MergeVertexColors`, `Level32=`, `AllHDLOD4..32`, `Season=`, `Threads=`, `ThreadSplit=`, plus brightness keys.

## Occlusion

Moved. The behavioural specification of TVDT occlusion (Occlusion.esp) is now
[occlusion-spec.md](occlusion-spec.md), written 2026-10-19 from public documentation, the MPL
xEdit record definitions, the shipped configuration and the golden output only (Target 1 of the
[re-implementation plan](../reimplementation-plan.md)). The `[decomp]`-derived description of
DynDOLOD's internals that stood here (node structure and initial values, height-map build rules,
ray stepping, the order in which child worldspaces reuse the parent's map, Ghidra findings) was
quarantined outside the repository on 2026-10-19 (`D:\Modlists\ref\quarantine\dyndolod-occlusion-decomp.md`)
and must not return; the `[golden]` facts (plugin header, worldspaces and cell counts, TVDT
layout as verified from the golden plugin, MHDT observations, settings of the golden run) moved
into the new document. The INI keys are also listed under "INI keys and presets" above.

## Grass LOD

- **Inputs**:
  - The NGIO / Faster NGIO grass precache: per cell `Data\Grass\<WorldEditorID>x<XXXX>y<YYYY>.cgid`, with 4-digit, sign-aware zero-padded coordinates (`Grass\%sx%.4dy%.4d` plus the extension from `GrassGID=cgid`). Seasonal caches add `.<SEASON>` before the extension (Grass Cache Helper NG) [exe-string] [ini] [doc: Grass-LOD, Seasons]. Vanilla `.gid` files do not work. Renamed or copied caches need `GrassGID=gid` [ini].
  - The cache format is not documented by DynDOLOD. The reader rejects unknown files ("Unknown grass data file format") and notes cells "out of range". The format comes from NGIO and needs separate RE. It is open.
  - GRAS records (`DATA` density, slope, water, position/height/color ranges, flags) and LTEX are read as well [exe-string].
- **Billboards**: TexGen renders one billboard per GRAS model into `textures\terrain\lodgen\<plugin>\<GrassEDID>_<FormID8>.dds`, with an `_n.dds` for complex grass and a `.txt` holding size/offset data (keys like `Width`, `Height`, `ShiftX/Y/Z`). Only grasses with bounds magnitude ≥ `MinGrassModelMagnitude` (64) and height ≥ `MinGrassModelHeight` (32) are rendered, unless `DoAllGrasses=1` [ini TexGen] [exe-string regex `textures\terrain\lodgen\([^\]*)\.*_([\dA-F]{8})`]. DynDOLOD packs them into a grass mini atlas or full atlas and exports a grass map for LODGen (`GrassMap=`) [exe-string `GrassLODMiniAtlas`, `GrassLODFullAtlas`].
- **Geometry**: grass instances from the cache become billboard quads, built from template NIFs. `GrassBillboard=1` (normal grass) and `ComplexGrassBillboard=4` (complex) index `GrassBillboard1..6`: `Internal` 2-plane, `DynDOLOD_flat_2x2alt1_lod.nif` (no backlight), `..._2x2alt2`, `..._4x2alt1/2/3`. `CSGrassOptimizations` uses `DynDOLOD_flat_2x2plain_lod.nif` [ini] [doc].
  - Vertex colors use `GrassBrightnessTop/BottomRGB` and `ComplexGrassBrightness*`, with per-season `_SPR`.. suffixes. Optional glow and backlight masks (`GrassGlowMap`, `GrassBacklightMask`) [ini].
  - Density is the `GrassDensity` percentage (UI "density").
  - The result is **merged into the LOD level 4 object LOD BTOs only** [doc: Grass-LOD].
  - `GrassLODMode` 1 starts grass LOD outside `uGridsToLoad`. Mode 2 starts it outside `uLargeRefLODGridSize` and sets `GrassLargeReference=True` in the export. This must match the NGIO setting `DynDOLOD-Grass-Mode` [doc] [exe-string].
  - `RemoveUnseenGrass` drops instances hidden under terrain [exe-string] [inferred].
  - Changing the mode needs a regenerated BTO.
- **Updating**: placement changes need Execute LODGen. Texture changes need TexGen plus Rebuild Atlas [doc: Grass-LOD].

## TexGen billboards and LOD textures

Clean-room behavioural spec of **TexGen** (`TexGenx64.exe` 3.0 Alpha-215, DynDOLOD 3 standalone) for LODForge.
Everything here is described in our own words. No DynDOLOD code, shader source, config bodies or long doc passages are reproduced.

**Evidence tags**
- `[doc: page]`: dyndolod.info help page (fetched 2026-10-17).
- `[cfg: file]`: file under `DynDOLOD\Edit Scripts\DynDOLOD\`.
- `[shader: file]`: GLSL under `...\DynDOLOD\Shaders\`, paraphrased.
- `[exe-string]`: UTF-16 string in `TexGenx64.exe`. The TexGen-specific region is roughly file offsets 0x146D000–0x1540000.
- `[golden]`: the golden run in `D:\Modlists\LODForge-golden\texgen-20261017\`. It covers the output tree, `TexGen_SSE_log.txt`, `TexGen_SSE_Debug_log.txt` and the preset, with Tree + Rendered billboards, Stitched and Rendered objects on, Grass/HD off, CS grass optimisations auto-enabled, BC7 Quick, base 256, UPP 11.0.
- `[lodgen-src]`: retired 2026-10-20 (LODGen decompile; material quarantined outside the repository).
- `[inferred]`: our own deduction. Verify it against the golden run.
- No Ghidra evidence was used. The headless import was started (`D:/Modlists/tools/rea-snapshots/TexGenx64.json`) but had not produced results when this was written.

### 0. Architecture in one paragraph

TexGen is an xEdit-based Delphi tool. It loads the load order and BSAs like xEdit, then runs four jobs:
- **(a) Stitched object-LOD textures**: resize and stitch full textures into small "mini atlases" (`textures\lod\...`).
- **(b) Rendered object-LOD textures**: render special NIFs front-on (`textures\lod\...`).
- **(c) Billboards**: render tree, grass and "rendered" (trunk) NIFs to sprite textures plus a `.txt` sidecar.
- **(d) Copy / alias rules.**

Rendering uses **OpenGL** with legacy GLSL 1.10 shaders on offscreen FBOs. Each worker thread gets one GL context (`RenderThreads`, `RenderContexts`), and MSAA is used up to `MaxMultiSamples` [exe-string: `TwbRender.*`, `GL_ARB_*`, `wglCreateContextAttribsARB`; golden: "OpenGL version 4.6.0", "Max samples: 16"].

All images are first saved uncompressed by TexGen. A batch of **Texconv** jobs then compresses them in place, and the work ends with "Waiting for Texconv to convert N textures" [golden].

### 1. Billboard outputs

#### 1.1 Which records get billboards

- **Trees**: every `TREE` base record, and every `STAT` with the *Has Tree LOD* flag, that passes all of these [doc: TexGen-Configuration; golden debug "Ignoring ... bounds magnitude ..., height ..."]:
  - OBND magnitude `sqrt(dx²+dy²+dz²) >= MinTreeModelMagnitude` (512).
  - `OBND.Z2 >= MinTreeModelHeight` (256).
  - The model path contains `tree`. Moss records under `landscape\plants` are rejected for this reason.
- **Grass**: `GRAS` records with magnitude ≥ 64 and Z2 ≥ 32 that have an above-water setting [doc]. Only used grasses are considered unless `DoAllGrasses=1` [cfg: TexGen_SSE.INI].
- **Overrides** come from `DynDOLOD_SSE_TexGen_Billboards_<plugin>.txt`. Lines have the form `Plugin.esm;FFFFFF;EditorID[,True|,False][,Scale=f]` [doc, cfg]:
  - `True` forces a billboard and `False` removes it.
  - `Scale=` overrides the maximum reference scale used for sizing.
  - `<plugin>` is the plugin filename with every non-alphanumeric character except `_` removed (`_ResourcePack.esl` → `_resourcepackesl`).
  - Files in `Data\DynDOLOD\` take precedence over files in `Configs\` [doc].
- **`DynDOLOD_SSE_Required_Billboards_<plugin>.txt`** lists `plugin;formid;`. These are billboards DynDOLOD always needs, for example the vanilla trees that hybrid/3D LOD falls back to. They are loaded together with the override lists. We assume they are forced on [cfg; golden debug `_LoadList`; inferred].
- **Report**: `textures\terrain\lodgen\TexGen_SSE_Tree_Billboards.txt` (and the matching `..._Grass_Billboards.txt`) lists every candidate as `Plugin;FormID;EditorID` [golden: 489 lines, 363 with `,False`]:
  - `,False` means filtered out.
  - `,True` means forced by an override.
  - No suffix means accepted automatically.
- **Replacement models**: if `Render\Skyrim\Replace\<model path without "meshes\">` contains `<name>_static.nif`, that NIF is rendered instead of the full model. The golden run used this for `_ResourcePack` palms and willows, which are skinned or animated [golden log "Creating billboard ... with ...\Replace\_ResourcePack\Landscape\Trees\TreeWhiteElephantPalm01_static.nif"].

#### 1.2 File naming

| Kind | Texture(s) | Sidecar |
|---|---|---|
| Standard tree/grass billboard | `textures\terrain\lodgen\<master plugin>\<model basename>_<formid8>.dds` | same stem `.txt` |
| Standard billboard + normal (`TreeNormalMaps=1`, or HD grass) | `+ <stem>_n.dds` | same |
| HD tree/grass billboard (front + side) | `<stem>_1.dds`, `<stem>_1_n.dds`, `<stem>_2.dds`, `<stem>_2_n.dds` | `<stem>.txt` (shared) |
| Rendered billboard (trunks for hybrids) | `textures\dyndolod\lod\trees\<subpath>\<nif stem>_1[_n].dds`, `_2[_n].dds` | `textures\dyndolod\lod\trees\<subpath>\<nif stem>.txt` |

Rules for the names:
- Everything is lower-cased. `<master plugin>` is the plugin that defines the base record, with its extension (`skyrim.esm`, `_resourcepack.esl`, `seasonal landscapes.esp`) [golden].
- `<formid8>` is the base record's FormID with the load-order byte(s) zeroed: `00` for a full plugin, `FE`+3 hex digits for a light plugin, so `_resourcepack.esl` gives `00000108` [doc: Tree-Grass-LOD-Billboards; golden].
- The model basename is the NIF filename without `.nif`, for example `treeaspen01_0006a9e6.dds` [golden].
- Rendered billboards keep the folder structure below `Render\Skyrim\Billboards\`. One example is `...\Billboards\DynDOLOD\lod\trees\sbt\treeaspen01_3C90228C_trunk.nif` → `textures\DynDOLOD\LOD\trees\sbt\treeaspen01_3c90228c_trunk_1.dds`. Trunk NIFs follow the pattern `^(.*)_([0-9A-F]{8})_trunk.nif`, where the 8 hex digits are the CRC32 of the full tree model [exe-string; doc: 3D-Tree-LOD-Model; golden].
- How LODGen uses the numbering [doc: Ultra-Tree-LOD; lodgen-src]:
  - An external billboard NIF can ask for `_1`, `_2`, `_3` textures per plane.
  - If `_1.dds` exists, `_2.dds`, `_3.dds` and so on are used for the other planes.
  - A file without a number is used for all planes.
  - LODGen strips `_1.` when it looks up the `.txt`.

#### 1.3 Billboard `.txt` format (as written)

The file is INI style: one `[LOD]` section with `Key=value` lines. Floats use 6 decimals and a `.` separator. All lengths are in game units **before** reference scale [golden].

| Key | Meaning |
|---|---|
| `Width` | Full width of the front billboard quad. For standard billboards it is centred on the model origin, so the left and right halves are equal. It equals `pixel_width * Height / pixel_height`, so it is quantised to the texture's 4-px width step [golden: 364/364 billboards satisfy `w_px = h_px*Width/Height` exactly] |
| `Depth` | Full width of the side (Y-axis) extent. Observed: the second, 90°-rotated billboard plane is this wide (object-lod-spec 6.1) |
| `Height` | Visible (trimmed) height from the lowest to the highest rendered texel |
| `ShiftX`, `ShiftY` | Plane offsets. Always `0` for trees in the golden run |
| `ShiftZ` | Z of the bottom edge of the quad relative to the model origin. It is the lowest visible geometry, often below 0 because trunks extend underground (`-16`, `-66.148`) |
| `Scale` | Always `1.000000` in the golden run. LODGen multiplies the plane transform by it |
| `CRC32` | CRC32 (zlib polynomial) of the **final compressed** `.dds` file [golden: verified for treeaspen01]. So the `.txt` is written after Texconv |
| `Width_n`, `Height_n`, `Adjust_n`, `CRC32_n` | Per-view values for HD and rendered billboards (n = 1 front, n = 2 side). `Adjust_n` is the horizontal offset of the quad centre from the origin, because HD/rendered views are trimmed tightly and asymmetrically. `CRC32_n` is the CRC32 of `<stem>_n.dds` [golden trunk txt; verified CRC] |
| `Model` | Source NIF path as resolved (`Meshes\landscape\trees\treeaspen01.nif`, or `dyndolod\render\skyrim\billboards\...\x_trunk.nif`; render models shipped by mods in the data folder are `dyndolod\render\billboards\...\x_trunk.nif`, e.g. Nature of the Wild Lands) [E2E TexGen debug log: `GetModelsToRender.CheckModel` adds 121 from the tools folder and 122 from data] |
| `ModelCRC32` | CRC32 of the NIF file bytes [golden: verified for a trunk NIF] |
| (P8) | `Model=` is `Meshes` plus the lower-cased path; replacement models keep the record's MODL case; trunks are `dyndolod
enderskyrimillboards...` lower case [golden: 324/324] |
| `Texture_k`, `TextureCRC32_k` | Every texture the render used and the CRC32 of each file [golden: 324/324 as sets]: slots 0, 1, 2 (only when the shader uses glow, soft, rim or back light) and 7, from `textures` on, existing files only; a missing or `texturesffects` normal map falls back to `<diffuse>_n.dds`; complex grass lists no slot 1. The order looks hash-based (first-use order matches 8/126) |
| `Complex` | `True`/`False` for grass that uses ENB complex-grass textures [exe-string; lodgen-src parses `COMPLEX`] |
| `BakeGlow` (and the grass/tree "Glow"/"Back" variants) | Written when glow/backlight was baked [exe-string list in `WriteBillboardTXT`; not present in the golden run] |

DynDOLOD uses the CRC fields to detect stale billboards. The messages "Billboard For Model * Has Different CRC32" and "... Does Not Contain Texture" come from comparing `ModelCRC32` and `Texture_k` against the current load order [doc: Messages].

#### 1.4 Billboard sidecar consumption (moved; see object-lod-spec 6.1)

> Moved to the quarantine folder on 2026-10-20 (it described LODGen's billboard parser). The billboard sidecar fields and the observed billboard geometry are in docs/re/object-lod-spec.md section 6.1.

### 2. Rendering

#### 2.1 Projection, view directions, framing

- **Projection**: orthographic for everything. Rendered objects are viewed from the **front** [doc: TexGen-Configuration "orthographic projection", "front view"].
- **View directions**:
  - Standard billboards: one view, the front.
  - HD and rendered billboards: two views, front (`_1`) and side (`_2`), each with diffuse and normal [doc: Tree-Grass-LOD-Billboards, 3D-Tree-LOD-Model; golden: `_1`, `_1_n`, `_2`, `_2_n`].
  - CS grass optimisations can render 2 or 4 sides (`CSGrassOptimizationsSides` 0 = two, 1 = four) [cfg].
  - Camera axes [inferred from LODGen UVs + golden]: the front view looks along the model's ±Y axis with world Z up and image row 0 at the top. The golden pine is narrow at row 0. Texture u runs toward world −X (u = 0 ↔ +X), as if the camera sat on +Y looking toward −Y. The side view is the same camera rotated 90° about Z. **Verify the mirror direction** against a golden billboard of an asymmetric tree.
- **Framing, in two passes** [golden debug, per billboard]:
  1. **Size estimate.** Debug prints `Scale: s` and `Width/Height: p0`:
     - `s` = maximum reference scale over all placed references of the base record, overridable with `Scale=`.
     - Without references, `s` falls back to `TreeModelHeightMultiplier` 1.7, `GrassModelHeightMultiplier` 1.0, or `RenderModelHeightMultiplier` 1.85 for rendered billboards. The golden trunks show `Scale: 1.85`.
     - `p0 = round(modelHeight * s / UPP)`, where `UPP = BillboardsUnitsPerPixel/10` (preset `110` → 11.0 u/px). Rounding, not truncation: fitted on all 89 logged billboards [golden; LODForge P8].
     - `modelHeight` is the z extent (max − min vertex z) of the geometry TexGen draws: every non-skinned shape, including NiTriShape/BSLODTriShape blocks in SSE files, below every NiSwitchNode child; not OBND Z2 [golden; LODForge P8].
  2. **Pixel height.** `h_px = clamp(nextPow2(p0), BillboardsMinSize, BillboardsMaxSize)`, also capped by `MaxTextureSize` and `MaxRenderResolution` [doc: "always rounded up to the next power of 2"; golden 235 → 256, 315 → 512, 91 → 128].
  3. **Pre-pass (trim measure).** Render at `w0 x h_px`, for example `109 x 256`, to find the opaque silhouette bounds. The frame is the geometry's own x range (asymmetric) and z range, `w0 = ceil(h_px * xExtent / modelHeight)` [golden: 89/89]. A texel counts as opaque by the `unmulta` test (below). The `onecolor` and `average` helper shaders support this pass [golden FB lines; shader].
  4. **Final pass.** Re-render at `w_px x h_px` with the camera framed on the trimmed bounds [golden "120 x 256 -> 120 x 256"]:
     - Vertically: from the lowest to the highest visible texel (→ `ShiftZ`, `Height`).
     - Horizontally, standard billboards: symmetric about the model origin, so half-width = max(|xmin|, |xmax|). The golden aspen's opaque columns are 2..109 of 120.
     - Horizontally, HD/rendered billboards: tight and asymmetric, with the offset written as `Adjust_n`. The golden trunk's opaque columns span 0..31 of 32.
     - Rendered billboards [LODForge P8, 119 golden trunks]: each view has its own pre-pass and final render; the final image spans exactly the trimmed silhouette (`Width_n`, non-square texels) at `4 * ceil(h * Width_n / Height_n / 4)` px; `Width = w_1 * Height_1 / h`, `Height`/`ShiftZ` from the front view, `Depth` ≈ `2 * max(|left|, |right|)` of the side silhouette; `Adjust_n` is the silhouette centre, positive toward image left. Trunk billboards are seen from the back (yaw 180° / 270°); tree and HD grass billboards from the front (0° / 90°).
     - `w_px = 4 * ceil(h_px * trimmedWidth / trimmedHeight / 4)`. The **width is not a power of two, only a multiple of 4** (BC block). Seen widths: 52, 64, 80, 120, 168, 396, 408 [golden].
     - Trim toggles: `RenderTrim`, `HDTreeTrim` and `GrassTrim` are INI keys (hidden, defaults unknown) [exe-string].
- **Rejection rules** [cfg: TexGen_SSE.INI comments; exe-string "can not trim %s"]:
  - Skip if opaque/transparent pixel ratio < `BillboardDiscardRatio` (0.01).
  - Skip if trimmed width or height in units < `BillboardMin{Width,Height}Mulitplier` (4) × UPP (sic, the key name is misspelled).
  - The skips are logged when `Notice=1`.
- **Rendered object textures** (`Render\Skyrim\Objects\**\*.nif`) [doc; golden NIF strings]:
  - The first shape must be a 4-vertex plane named with `TexGen Plane`.
  - Its X/Z extent defines the ortho viewport.
  - Its BSShaderTextureSet names the **output paths** (`textures\lod\wrwallslod01.dds` + `_n`). Usually; the output name actually follows the NIF path (§3.3).
  - Its shape scale (0.25, 0.5, 1, 2, 4 ...) multiplies the base size (`RenderObjectsSize`, e.g. 256). The golden `wrwallslod01` is 512² from base 256 with scale 2; debug prints "256 ? 8192" as size and max.
  - Width and height are snapped to the base size or integer multiples of it.
  - A NiAlphaProperty on the plane means the output keeps alpha.

#### 2.2 Lighting model and maps

The `default`/`envmap` fragment shader is shared by objects and billboards. `envmap` adds cubemap reflection [shader: default.frag, envmap.frag; paraphrased].

- **Inputs**:
  - Per-material uniforms come from `BSLightingShaderProperty`: UV offset/scale, emissive colour × multiple, specular colour/strength/glossiness, lighting effect 1 and 2, env map scale [exe-string `LoadScene` field list].
  - Texture slots: 0 diffuse, 1 normal, 2 glow, 4 cube, 5 env/RMAOS, 7 back/subsurface.
  - Shader-flag booleans: complex, specular, own-emit, PBR, soft-light, rim-light, back-light.
- **Light**: one directional light `L` plus ambient `A` and diffuse `D`. The lighting-only path works in **gamma space** (no sRGB decode; textures are loaded as UNORM) [inferred].
  - UI/preset values are percentages: `*Direct` → D, `*Ambient` → A, and `*DirectLR/FB/TB` → light direction components (−100..100).
  - Defaults [golden preset]:
    - Trees: D = 1.35, A = 0.55, TB = −100 (light from top).
    - Grass: D = 1.5, A = 0.25.
    - Rendered billboards: D = 0.5, A = 1.5.
    - **Rendered objects: D = 0, A = 1.0, FB = −100**, so object LOD textures are essentially unlit albedo.
  - The exact mapping (sign of TB, normalisation) is inferred from the key names and the doc ("direct (default from top)").
- **Tangent frame** [LODForge P8, golden]: the shading normal comes from the normal map with x along the NIF's stored bitangent, y along its stored tangent and z along the vertex normal; the vectors are normalised per vertex and not flipped on back faces. Light from the top (+Z) for trees.
- **Diffuse output**:
  - `rgb = albedo*vertexColour * (A + D*max(N·L,0) + emissive) + subsurface + specular`.
  - Specular is Blinn-Phong `specColour*specStrength*normal.a*pow(N·H, gloss)*D`, used only for non-PBR materials with the specular flag.
  - Rim light, soft light and back light are approximated from the glow/back mask.
  - `alpha = vertexColour.a * diffuse.a`.
  - For PBR materials (`textures\pbr\`, `_rmaos`) rgb is multiplied by `pbrscale`: `StitchPBRScale`/`RenderPBRScale` 0.65, `BillboardPBRScale` 0.7 [cfg preset].
  - A second render target gets the same rgb with alpha = 1.
- **Complex grass**: the diffuse is sampled from the top half of the texture (v*0.5) and the normal from the bottom half (v*0.5+0.5) [shader].
- **Normal map ("bakeNormal")**: the normal is **not** a transform of interpolated normals.
  - The pixel is lit six times by axis lights (±X, ±Y, ±Z in view space, converted to tangent space). Each result is `A.r + D.r*max(n·Lk, 0)`.
  - The encoded normal is `nx = I(+X) − I(−X)`, `ny = I(−Y) − I(+Y)`, `nz = sqrt(1 − nx² + ny²)` (sic: the source has this precedence quirk), then normalised and written as `(n+1)/2`.
  - The result is a **view-space ("billboard plane tangent-space") normal**: R = image right, B = toward the camera. The G sign convention is to be checked against the golden data; the golden `_n` means are ≈ (126,126,249).
  - [LODForge P8] The bake uses a fixed direct light of 0.5 whatever the billboard's lighting (HD grass is lit with direct 0 and still has normals). G sign: positive against the golden (fit coefficient > 0, no cross term); golden `_n` means (127, 126, 249) trunks, (127, 125, 248) HD grass.
  - `alpha = specStrength*normal.a` when specular is on, else 1.
- **Glow, env and back bake passes**: they output the emissive, RMAOS or subsurface terms alone. These feed the HD `Glow`/`Back` options and `BakeGlow`/`BakePBR`.

#### 2.3 Alpha handling

1. Each billboard renders into an MSAA FBO with `MaxMultiSamples` (16) samples [golden "MultiSamples: 16, SuperSamples: 0"].
   - Material alpha-test thresholds come from each shape's NiAlphaProperty and are logged as "Alpha threshold <nif> <shape> <tex> 80".
   - `GLAlphaCoverage=1` enables GL sample coverage (`ACAlphaThreshold/255`); it is off by default [cfg].
   - The alpha test is fixed-function GL (the `default` shader has no discard). It tests the diffuse alpha only: vertex alpha is ignored even with the Vertex_Alpha shader flag (trees keep wind weights there) [golden: Soul Cairn trees; LODForge P8].
   - Source textures are sampled with their mips **and anisotropic filtering** (matches the golden foliage density without any LOD bias), uploaded block-compressed (`LoadGLCompressed`).
   - Shapes without Double_Sided are drawn with back faces culled.
2. The resolved image is premultiplied by coverage against a transparent-black clear. The **`unmulta`** pass makes alpha binary [shader: unmulta.frag; golden: every billboard alpha is exactly {0,255}]:
   - If `a >= threshold` and rgb ≠ 0, set `rgb /= a` and `a = 1`.
   - Otherwise set `a = 0`.
   - The threshold is `TreeMSAlphaThreshold` 128, `GrassMSAlphaThreshold` 112 or `ObjectMSAlphaThreshold` 128, divided by 255.
3. **Supersampling** (`Tree/Grass/Object/RenderMaxSuperSamples`, default 0) renders at 2^n × resolution and downsamples [cfg]:
   - Alpha is binarised with `SSAlphaThreshold` (128).
   - `SSRenderAlpha=1` takes alpha from a native-resolution render instead.
   - `SSBlendAlpha=1` ORs the two alphas.
4. `AlphaFactor` (hidden key) multiplies alpha in the stitched shader [exe-string; shader].
5. **Colour bleed** (`BleedToTransparent=1`) [shader: bleed.frag, average.frag; golden: mean transparent rgb ≈ mean opaque rgb, no black texels]:
   - Each transparent texel gets the mean colour of the opaque texels within `BleedRadius` (hidden key). The golden run logs `BleedRadius: 2`; the neighbourhood is the (2r+1)² square without the centre, edges clamped.
   - If none is in range, it gets the **average opaque colour** of the whole image, computed into a 1×1 target by the `average` shader.
   - Alpha stays 0.
6. **Mips**:
   - Billboards ship with **one mip level** (`-m 1`) [golden: all 364 billboard textures have 1 level]. DynDOLOD builds atlas mips later.
   - Stitched and rendered object textures get full chains via Texconv with `-keepcoverage <AlphaCoverage>` (0.65). This preserves alpha-test coverage per mip and is a flag of DynDOLOD's custom Texconv [golden cmdline; doc: Texconv].
   - `UseMipmaps=1`: source textures are sampled with their own mips during rendering. With 0 they are not.
   - `UseMipmapsComplexGrass` and `ComplexGrassUseMipmaps` control whether complex-grass mips are used or regenerated [cfg].
   - `CPUMipMaps`, `TreeMipMaps`, `GrassMipMaps` and `ObjectMipMaps` are hidden keys [exe-string].
7. **Noalpha vs alpha lists** (stitched textures): `_noalpha_` rules strip the alpha channel and `_alpha_` rules keep it. The reason is that LOD only supports a binary alpha test, typically at 128 [doc: TexGen-Configuration].

#### 2.4 Gamma, formats, compression

- **Colour space**: Texconv runs **without** `-srgb`. The output is `BC7_UNORM` (DXGI 98) for every texture in the golden run (1273 BC7, plus one 4×4 DXT1 `defaultdiffuse.dds`) [golden].
  - sRGB format variants exist in the exe (`BC1/BC2/BC3/BC7_UNORM_SRGB ... -srgb`), probably for the `sRGBAtlas`/`LinearInternal` paths [exe-string].
  - The UI has a "Billboard gamma" combo (`ComboBoxBillboardGamma`), but its preset key and effect are unknown (open).
- **Format choice**: four preset slots, `DiffuseFormat`, `DiffuseAlphaFormat`, `NormalFormat` and `NormalSpecularFormat`. Each holds an internal enum code; **225 = "Default (BC7 Quick)"** [golden preset]. The selectable options map to these Texconv arguments [exe-string]:
  - `BC1_UNORM_SRGB -bc d -srgb`, `DXT1 -bc d`
  - `BC2_UNORM_SRGB …`, `DXT3`
  - `BC3_UNORM_SRGB …`, `DXT5`
  - `B5G6R5_UNORM`, `BC4_UNORM`, `BC5_UNORM`
  - `BC7_UNORM[_SRGB] -bc q` (Quick), `-bc x` (Max)
  - plus uncompressed A8R8G8B8 / R8G8B8.
  - The doc warns against BC1 for the alpha slots.
- **Texconv command line** [golden, 1304 calls]:
  - `Texconvx64.exe -nologo -gpu <TexconvAdapterIndex> -y -sepalpha -aw 256 [-keepcoverage 0.65 | -m 1] -f <fmt> -bc <q|x|d> -o <dir> <file>`
  - `-m 1` is used for billboards; `-keepcoverage` for stitched/rendered textures.
  - `-aw 256` is the alpha weight for BC7 and `-sepalpha` gives separate alpha mip filtering.
  - Another template, `-nologo -y -m 1 -sepalpha -aw 256 -srgb -f R8G8B8A8_UNORM`, exists in the exe. It is probably used to decode unsupported source formats [exe-string; inferred].
  - Texconv errors are reported with the HRESULT table in [doc: Texconv].
- **Resolution rules**:
  - The game needs power-of-two textures, or at least multiples of 4 for BC formats [doc: Texture-Resolution].
  - Billboards: height is a power of two and width a multiple of 4.
  - Stitched textures: `atlas multiplier × base size`, where base size `StitchObjectsSize` is 256 in the golden run. Examples: `dirt02lod` 256², `farmhouselod01snow` 256×512, `leatherhide01lod` 128² [golden].
  - Global cap `MaxTextureSize` (8192); render-buffer cap `MaxRenderResolution` (8192). The GPU allowed 32768 in the golden run [cfg; golden debug].

### 3. Other TexGen outputs

#### 3.1 Stitched object-LOD textures (`_alpha_` / `_noalpha_` lists)

- **Config files**: `DynDOLOD_SSE_TexGen_{alpha|noalpha}_<plugin>.txt`. Each is loaded only if the plugin is in the load order, and `Data\DynDOLOD\` copies override the ones in `Configs\` [doc].
- **Line format**: tab-separated with 12 columns. Lines starting with `//` are comments [cfg; doc: Mod-Authors table]:

| col | meaning |
|---|---|
| 0 | brightness, signed int (negative darker) |
| 1–3 | R, G, B multipliers. The doc gives `pow(mult, 1/Gamma)`; they become the vertex colour C multiplied in the stitched shader [shader] |
| 4 | source texture (data, BSA, or the current output, including `DynDOLOD-temp\` intermediates) |
| 5–6 | tile width/height multiplier × base size |
| 7–8 | tile x/y position multiplier × base size |
| 9 | destination texture (output path). Several lines with the same destination are composited into one mini atlas |
| 10–11 | destination atlas width/height multiplier × base size |

- **Variant: column 5 = `x`.** Columns 5–11 then read `x, mode, param, 0, dest, level, x`, for example `hhwin02.dds x 7 0 0 hhwin02noalpha.dds 0 x`. TexGen copies a full texture at (close to) its own resolution:
  - It strips alpha (`*noalpha.dds`) or regenerates the mips of glow maps (`_g`, `_em`, `_m`). When source equals destination, the doc says the rule exists "for updating mipmaps of full glow textures".
  - It is skipped when `CreateMod=1` [doc: Mod-Authors; cfg; golden: `hhwin02noalpha.dds` 2048², `swindow01_g.dds` 512²].
  - Our working guess: `mode` is a channel/operation bitmask and `level` is a mip offset or downscale exponent [inferred, open].
- **Processing**:
  - Each tile is resampled with a **6×6-tap Lanczos-3** filter (`stitched` shader, `sin(πx)·sin(πx/3)/x²` weights, normalised) with a mip `bias`.
  - It is then multiplied by C, with alpha × `alpha` (`AlphaFactor`).
  - Tiles are drawn into the destination rect [shader].
  - The normal map is built the same way from each source's `_n`. The destination is `<dest>_n.dds`, and every stitched output in the golden run has a `_n` twin. Normal alpha comes from the env/RMAOS map when one exists.
  - When the `alpha` uniform ≠ 1, the normal is re-normalised after its Z (blue) is multiplied by it, which controls steepness (`NormalMapSteepness`) [shader; exe-string].
  - PBR sources (`textures\pbr\...`) are detected by `CheckPBR`. Their colour is scaled by `StitchPBRScale`, and `_rmaos` maps are used [exe-string; golden debug].
- **Re-UV map**: TexGen also writes `Edit Scripts\DynDOLOD\Cache\DynDOLOD_SSE_re-uv-miniatlas.txt`. Each tab-separated line is `src.dds  tileW_px tileH_px  x_px y_px  dest.dds  atlasW_px atlasH_px  uScale vScale`, 281 lines in the golden run. DynDOLOD/LODGen use it to remap full-model UVs onto the mini atlas [golden].

- **Behaviour fitted to the E2E golden** [golden; LODForge stitched, `crates/texgen/src/stitch.rs`]:
  - Order: per loaded plugin `noalpha` then `alpha`. A destination that a later file defines again is rebuilt from that file's rules only, but the PBR twin of the earlier definition survives when the later one has none (Majestic Mountains' `mountainslablod` 2 tiles 512², `mountainslabpbr_lod` from vanilla's 9 tiles). `DynDOLOD-temp\` intermediates are built first and saved like outputs (alpha 255 for `noalpha` rules).
  - Geometry: tile rect = position/size × 256 px in an atlas of atlas multiplier × 256, top-left origin, GL pixel-centre coverage, clipped; the whole source maps onto the rect with GL_REPEAT. Later rules overwrite.
  - Sampling: a trilinear sample of the source's own DDS mip chain at `log2(source px / tile px) − 1.25` fits best (34.6 dB mean). The 36-tap `stitched.frag` emulation and a Lanczos-3 resample of mip 0 both score lower; TexGen's actual `bias` uniform is not known.
  - Colour columns: each channel `s^(1/gamma) + brightness/100`, gamma = columns 1–3 (`shipwoodside01green`: 0.9/1/0.9 greens; snow temps: 2/2/2.1 brighten). A negative red value means only the `_n` is written (MM `mountainslabHQlod`: 96, −3, 1, 1).
  - `unmulta` at save: texels with alpha ≥ 128 get colour / alpha, those below keep it (golden/ours ratio by alpha: 1.00, then 1.76, 1.45, 1.23, 1.07). It applies before the tone and is not clamped before `StitchPBRScale`.
  - PBR twin (CheckPBR): made when some source, by its own name before copy aliases, has `textures\pbr\<path>`. Name: a file name ending in `lod`/`lod<digits>` gets `pbr_lod` in place of `lod` (`slod04` → `spbr_lod04`), anything else moves under `textures\pbr\`. Sources use their PBR version where one exists; only those are scaled by `StitchPBRScale` 0.65 (castlekitpbr_lod mixes both).
  - `_n`: same tiles from the `_n` of the diffuse TexGen really loads (after a copy alias of the diffuse: `mountainslabtemp.dds` → `mountainslab01.dds` uses `mountainslab01_n`); texels no tile covers are a flat normal (128,128,255) with alpha 0. Golden `_n` keep a residual texel-level difference (27.7 dB mean) not explained by bias, offset or gain.
  - `x` rules: the source at its own size with the tone; alpha stripped for `*noalpha*` destinations; mip regeneration is Texconv's job. 16 outputs at 53 dB mean.
  - Re-UV map: one row per source texture (skipping `DynDOLOD-temp\` sources and `_n`-only atlases), `src aw ah x y dest aw ah tw/aw th/ah` in px, `x y` of the last tile using that source and the scale of the first tile in the same atlas, scale printed as Delphi `FloatToStr` of a Single (`0.33500000834465`). Every source of an atlas with a PBR twin also maps onto the twin, by its PBR path when it has one. 342/344 rows equal; the 2 others are sources shared by two atlases where TexGen's choice follows its hash order.

#### 3.2 Copy lists (`_copy_`)

- Each line is `source.dds,destination.dds`.
- In TexGen 3 this is a **virtual alias**, not a file copy. When a later step asks for `destination`, `source` is used, for example "Texture ...whruinstonedark_n.dds replaced with ...whruinstone_n.dds". No file named after the destination appears in the output [golden debug `TexGenGenerate`, `TAtlasMap.BuildAtlas`; doc: purpose is to give every diffuse a same-named `_n`].
- LODForge can therefore treat copy rules as a name-substitution table in its VFS.
- An alias is skipped when the destination already exists in the load order ("Notice: File already exists", 3 in the golden), and each rule also aliases the `textures\pbr\` versions of both names [golden debug; LODForge stitched: 32 rules → 37 aliases].

#### 3.3 Rendered object-LOD textures

The rules are in §2.1. Outputs go to `textures\lod\...`, `textures\dlc0x\lod\...` and similar (whatever the TexGen Plane texture set says), as diffuse + `_n` with full mips, using `RenderObjects*` lighting (unlit by default) [golden: 58 "Creating texture ... with ...nif"].

Fitted to the E2E golden (70 rendered textures) [golden; LODForge rendered, `apps/cli/src/texgen_rendered.rs`]:
- Output name: the NIF's place below `Render\Skyrim\Objects` (`dlc02\lod\dlc2dweruins01_lod.nif` → `textures\dlc02\lod\dlc2dweruins01_lod.dds`), not the plane's texture set (that plane names `textures\lod\dweruins01_lod.dds`).
- Frame: the plane's world extent (its own scale included) in X/Z, seen from −Y (image right = +X; from +Y only back faces show). The plane is not drawn.
- Size: long side `256 × plane scale`, short side the next power of two of `long × aspect` (seruinswindowslod 100.9 × 403.4 → 64 × 256; sbridgelod 3125 × 1088, scale 2 → 512 × 256).
- Shading: vertex colours on (without them 25.6 instead of 32.5 dB), alpha test, `unmulta` at 128 with bleed; alpha kept only when the plane has a NiAlphaProperty.
- Candidates: the base-game folders (`lod`, `dlcNN`) only. The `enderal`, `smim` and `hpp` NIFs are not rendered even when all their textures exist (SMIM fence, Enderal-folder buildings); LOD meshes in the load order name almost none of the rendered textures, so the rule is not "used by an LOD model". A NIF with a missing piece texture is skipped.
- PBR twin ("Adding ...\winterholdpbr_lod02.nif"): when a piece has a `textures\pbr\` diffuse and the output name takes `lod` → `pbr_lod` ("Could not process PBR ...dlc01icesnow.nif" otherwise). Pieces use their PBR diffuse/normal; a `_p.dds` diffuse (a parallax map in PBR naming) becomes the plain `<stem>.dds` with the PBR `<stem>_n.dds` (dlc01icewallpbr_lod).
- Open: alpha-blended layers (ridgedmossstone08lod's moss over stone: we only alpha-test), the Dwemer dome's metal/envmap brightness, `_n` 26 dB.

#### 3.4 Glow textures

- TexGen does not render "glow LOD" geometry.
- Its glow involvement is limited to:
  - (a) `x`-mode rules that regenerate full-resolution `_g`/`_em` glow maps and `*noalpha` window textures (§3.1);
  - (b) `BakeGlow`/`*Glow` options that bake emissive into HD/rendered billboards [cfg; exe-string].
- DynDOLOD's own `FlatLODGlowMap`, `GrassGlowMap` and similar keys point LODGen at `textures\white.dds` or `textures\gray.dds` defaults for billboards with no glow or backlight map [exe-string; lodgen-src defaults `Textures\White.dds`, `Textures\Gray.dds`].

#### 3.5 Grass billboards

- Same pipeline and naming as trees, under `textures\terrain\lodgen\<plugin>\<model>_<formid>.dds`. Grass uses `GrassModelHeightMultiplier`, `GrassMSAlphaThreshold` 112 and the `GrassBillboards*` lighting [cfg; exe-string]:
  - Plain grass: diffuse only.
  - HD grass (ENB complex grass, or Community Shaders present): adds `_n`.
- `ForceComplexGrass` [cfg; doc: TexGen]:
  - 0 = normals only for grass that really uses complex textures.
  - 1 = normals for all grass if any complex grass exists (the default).
  - 2 = always.
  - "Grass" and "HD Grass" are mutually exclusive in the UI depending on what was detected.
- `HDGrassFaceNormals` is a hidden key [exe-string].
- Grass billboards rarely need more than 32 px [doc].
- **Community Shaders grass optimisations** (`CSGrassOptimizations` −1/0/1, "auto-enabled" in the golden log) [exe-string]:
  - TexGen also creates `*_LOD1.nif`, mini-atlas textures and updated full grass models.
  - Billboards are rendered with 2 or 4 sides.
  - The default billboard model is `DynDOLOD_flat_2x2plain_lod.nif`.
  - These were not exercised in the golden run.

#### 3.6 Bookkeeping outputs [golden]

- `textures\DynDOLOD\LOD\TexGen_SSE.ini` is a copy of the active preset (the `[TexGen]` section). DynDOLOD reads it to learn the scales and formats used.
- `textures\DynDOLOD\LOD\version.ini` contains `[Main] Version=3.00` (`WriteTexGenVersion`).
- `textures\DynDOLOD\LOD\defaultdiffuse.dds` is a 4×4 DXT1 placeholder.
- `TexGen_SSE_{Tree,Grass}_Billboards.txt` (§1.1).
- `DynDOLOD-temp\` holds intermediates such as the "green" and "snow" tinted copies used by later stitch lines. It is removed at the end ("Removing temporary files").
- Run log `Logs\TexGen_SSE_log.txt`; debug log `Logs\TexGen_SSE_Debug_log.txt`.

### 4. Settings

#### 4.1 `TexGen_SSE.INI` `[TexGen]` (shipped keys)

| Key | Default | Meaning |
|---|---|---|
| `Notice` | 0 | log skipped-billboard notices |
| `RenderThreads` | 2 | parallel render contexts |
| `CreateMod` | 0 | mod-author mode: skip same-src/dst rules, no "disable old output" prompt |
| `MaxTextureSize` | 8192 | cap on any generated texture |
| `MaxRenderResolution` | 8192 | cap on the FBO size |
| `DoAllGrasses` | 0 | consider unused GRAS too |
| `ForceComplexGrass` | 1 | see §3.5 |
| `CSGrassOptimizations` / `...Sides` | 0 / 0 | −1 off, 0 auto, 1 on; 0 = 2 sides, 1 = 4 sides |
| `TreeNormalMaps` | 0 | also write `_n` for standard tree billboards |
| `BillboardDiscardRatio` | 0.01 | min opaque/transparent ratio |
| `BillboardMin{Width,Height}Mulitplier` | 4 | min trimmed size in units of UPP |
| `MinTreeModelMagnitude` / `MinTreeModelHeight` | 512 / 256 | auto-discovery thresholds (OBND) |
| `MinGrassModelMagnitude` / `MinGrassModelHeight` | 64 / 32 | " |
| `TreeModelHeightMultiplier` / `Grass…` / `Render…` | 1.7 / 1.0 / 1.85 | fallback max-scale for sizing |
| `UseMipmaps` / `UseMipmapsComplexGrass` | 1 / 1 | sample source mips when rendering |
| `BleedToTransparent` | 1 | colour bleed into α = 0 texels |
| `MaxMultiSamples` | 16 | MSAA samples (0 = off) |
| `Tree/Grass/ObjectMSAlphaThreshold` | 128 / 112 / 128 | binarisation threshold after MSAA resolve |
| `GLAlphaCoverage`, `ACAlphaThreshold` | 0, 255 | GL sample coverage value = ACAlphaThreshold/255 |
| `Tree/Grass/ObjectMaxSuperSamples` | 0 | supersampling exponent |
| `SSAlphaThreshold`, `SSRenderAlpha`, `SSBlendAlpha` | 128, 0, 0 | supersample alpha policy |
| `AlphaCoverage` | 0.65 | Texconv `-keepcoverage` value for mip alpha preservation |

**Hidden keys** are read by `LoadTexGenINI` but not shipped [exe-string]. Their defaults are unknown.
- Diagnostics: `Wizard`, `LockTexconv`, `GLListDebug`, `GLDebug`, `KeepRenderer`, `Summary`, `SummaryOnly`, `LogLock`, `FormID`, `DebugFormID`, `PresetFilename`.
- Threading and render paths: `RenderTexturesSingleThread`, `RenderBillboardsSingleThread`, `OldBuildAtlas`, `RenderSingle`, `StitchThreads`, `RenderContexts`, `LoadGLCompressed`.
- Archive output: `CreateArchive`, `DeleteFiles`, `ArchiveType`, `ArchiveLevel`, `ArchiveThreads`.
- Resources: `TexconvAdapterIndex`, `TextureCache`, `MaxTextureIndex`, `SplitTextureMaxSize`.
- Trimming and mips: `GrassTrim`, `HDTreeTrim`, `RenderTrim`, `CPUMipMaps`, `ComplexGrassUseMipmaps`, `ObjectMipMaps`, `TreeMipMaps`, `GrassMipMaps`.
- Rendering and baking: `HDGrassFaceNormals`, `BakeGlow`, `BakePBR`, `BleedRadius`, `RenderMaxSuperSamples`, `AlphaFactor`, `NormalMapSteepness`.
- PBR scales: `StitchPBRScale`, `RenderPBRScale`, `BillboardPBRScale`.

#### 4.2 UI options and where they are saved

- **Preset file**: the options window is saved on **Start** to `Edit Scripts\DynDOLOD\Presets\DynDOLOD_SSE_TexGen.ini`. The doc says `..._TexGen_Default.ini`, but the golden run used the file without the suffix. A copy is written to `textures\DynDOLOD\LOD\TexGen_SSE.ini` [golden; doc].
- **Window geometry** is saved in `Edit Scripts\DynDOLOD\TexGen.ini` (`[frmMain]` …) [golden].
- **First-start defaults**: base sizes, UPP and max size are derived from the game resolution read from the Prefs, Custom or Display Tweaks INI (`[Render] Resolution=`). These are the `*SizeDefault` and `BillboardsUnitsPerPixelDefault` values [doc; exe-string `SSEDisplayTweaks.ini`, `iSize H`].

| Preset key | UI | Notes |
|---|---|---|
| `OutputPath` | output folder | forbidden in Program Files / Documents / game / MO2 folders [doc] |
| `StitchObjects`, `StitchObjectsSize` | Stitched object LOD textures + base size | combo shows `%d (720p/1440p/2160p/4320p)` |
| `RenderObjects`, `RenderObjectsSize`, `RenderObjects{Direct,DirectLR,DirectFB,DirectTB,Ambient,Smoothness,GlowLR/FB/TB}` | Rendered object LOD textures | default unlit (Direct 0, Ambient 100) |
| `Billboards`, `BillboardsUnitsPerPixel` (×10), `BillboardsMinSize`, `BillboardsMaxSize`, `BillboardsPreviewModel` | Tree/Grass LOD billboards block | UPP combo `%1.1f (720p … 4320p)`. Doc numbers: 1080p ≈ 11.8, 1440p 8.85, 2160p 5.9 (from `2·8192·tan(37.5°)/screenH`) |
| `TreeBillboards*`, `TreeHDBillboards*`, `GrassBillboards*`, `GrassHDBillboards*`, `RenderBillboards*` | per-type checkbox + Direct/DirectLR/DirectFB/DirectTB/Ambient (+Smoothness, Glow, Back for HD/Rendered) | percentages; the Preview button is a live GL preview (`TfrmTexGenPreview`) |
| `DiffuseFormat`, `DiffuseAlphaFormat`, `NormalFormat`, `NormalSpecularFormat` | compression combos | enum code, 225 = Default (BC7 Quick) |
| `StitchPBRScale`, `RenderPBRScale`, `BillboardPBRScale` | (not in UI) | PBR albedo scale |
| `BakeGlow`, `BakePBR` | (preset) | bake toggles |

### 5. GPU reimplementation sketch (wgpu)

Per billboard or rendered object, on one queue, with batching across many small targets. A texture-array atlas of FBOs is fine.

1. **Load and size.** Resolve the model; prefer `Render\Skyrim\Replace\…_static.nif`. Compute `s` (max ref scale, override or fallback multiplier), `p0 = H*s/UPP`, `h = clamp(nextPow2(p0))`.
2. **Pass A: silhouette measure.** Ortho camera on the model bounds. Draw depth plus alpha-tested coverage into an R8 target at `w0×h` with MSAA 16 (or 4 plus our own resolve). A compute reduction finds the opaque bbox. Derive `ShiftZ`, `Height` and either the symmetric half-width or the asymmetric width plus `Adjust`. Then `w = 4*ceil(h*W/H/4)`.
3. **Pass B: final MSAA render** at `w×h` (×2^SS when supersampling). Use MRT outputs: lit diffuse RGBA (premultiplied by coverage), plus the 6-light normal bake (we can compute the view-space normal directly from TBN·normalMap and encode it the same way, including the `nz` quirk, for bit-parity), plus optional glow/back/RMAOS. One draw per shape, using the BSLightingShader parameters in §2.2.
4. **Resolve and unmultiply** (compute): average the samples, then apply `a ≥ thr ? (rgb/a, 1) : (rgb, 0)`. When supersampling, downsample and binarise with `SSAlphaThreshold`.
5. **Discard test**: opaque ratio and minimum size. Skip with a notice.
6. **Bleed** (compute): first a reduction for the average opaque colour, then fill α = 0 texels from neighbours within the radius, or with the average. Apply the same fill to the `_n` image using the diffuse alpha mask, so normals have no alpha.
7. **Encode**: BC7 in our own GPU encoder (or intel-tex/bc7e). Use UNORM, not sRGB, with alpha weighting. Billboards get 1 mip. Stitched and rendered textures get a full chain with coverage-preserving alpha per mip, scaling α so that the fraction above 0.5 matches the base level × 0.65 target.
8. **Write `.txt`** after the DDS is final, so the CRC32 values can be computed: file CRC, model CRC, per-texture CRC.
9. **Stitched textures** (separate path): for each destination, render quads into the atlas RT with the Lanczos-3 6×6 sampling shader and vertex-colour multipliers. Do the same for `_n`, with an optional Z-steepness renormalise. Emit `re-uv-miniatlas` rows. Copy rules become VFS aliases.

### Open questions for a golden TexGen run / diff

1. What exactly is `modelHeight` in `p0 = modelHeight*s/UPP`? It is 1–7 % above the txt `Height` and is not OBND Z2. Is it the NIF geometry bounds, the bounding sphere, or untrimmed pass-A bounds?
2. How is the pre-pass width `w0` chosen (109 for aspen, 190 for pine, 51 for the aspen trunk)? Is any padding added to the trimmed bbox? (The golden aspen has a 2-px gap on the tight side.)
3. Is the front view mirrored: does u = 0 map to +X? What is the sign of `DirectTB`/`LR`/`FB`, and how do the percentages map to D, A and the light vector? The Preview/Use path could confirm.
4. Does the normal map's G channel point up or down? What does `*Smoothness` change in the normal bake?
5. What is the "Billboard gamma" combo, which preset key does it write, and where is it applied (`Power(mult, 1/Gamma)`?)
6. In `x`-mode stitch rules, what do columns 6 (`7`/`0`), 7 (`12`/`10`/`5`) and 10 (`0`/`1`/`2`) mean?
7. Hidden-key defaults: `BleedRadius`, `RenderTrim`/`HDTreeTrim`/`GrassTrim`, `AlphaFactor`, `NormalMapSteepness`, `TreeMipMaps`.
8. HD tree and HD grass outputs, not produced in this golden run: the txt keys (`Width_1/2`, `Complex`, glow/back), `_n` content, and side-view orientation (+90° or −90°).
9. Grass billboards and CS grass optimisation outputs (`*_LOD1.nif`, mini atlases), which this golden run did not produce.
10. Is per-material alpha-test (NiAlphaProperty threshold) applied per fragment before MSAA, or only through the 128 post-resolve threshold?

## Open questions (consolidated)

Configuration: see the list at the end of "LOD model selection". Runtime and data:

1. `[Objects]` field meanings in the NG data file (positional reading above). Confirm by toggling
   one dynamic LOD option at a time and diffing the generated file.
2. TVDT byte layout. Occlusion.esp from the golden run is the sample to decode against vanilla
   Skyrim.esm TVDT.
3. `.cgid` (NGIO grass cache) format. Grass LOD was not exercised (no cache on the test instance).
4. Billboard sizing: the exact source of `modelHeight`, the light-vector signs, and the
   normal-map G direction. See the TexGen open questions; a TexGen decompile pass (REA/Ghidra)
   is the right tool.
5. Object LOD parity gaps left after the 3.1.8 port: FlatTextures internal billboards, and one
   vertex-alpha / HD naming case. Skyrim LE meshes are now read. See docs/baseline.md "P6".

## Implementation notes for LODForge

- The LODGen side is the cheapest part to reach parity on. Feed DynDOLOD's own export into
  `lodforge object-lod --flavor dyndolod` (validated per season against the golden run) before
  writing our own export.
- The export itself has to be built from DynDOLOD's knowledge: rules, LOD model discovery, the
  CRC32-named variants and the atlas / mini-atlas build. The configs and Resources that drive
  that knowledge are CC BY-NC-SA and stay in the user's install. LODForge reads them at run time
  and ships none of them.
- Plugins and DLL data are a separate, smaller target. The DLL NG validates
  `bunchofnumbers`, the plugin names and versions, the plugin limits and the presence of
  KYWD 0x910 in both plugins. It does not inspect how the BTOs were made.
