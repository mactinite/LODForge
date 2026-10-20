# DynDOLOD 3 / TexGen configuration keys vs LODForge (main 073361e, branch gap-audit-20261019)

Historical per-key snapshot, not a current support matrix. Some rows were fixed
after this commit. See the [current parity assessment](../parity.md) before using
an "absent" or "hardcoded" label to plan work.

Sources: `Edit Scripts\DynDOLOD\DynDOLOD_SSE.ini`, `TexGen_SSE.INI`, `Presets\DynDOLOD_SSE_Default.ini`,
`Presets\DynDOLOD_SSE_TexGen.ini`, `Configs\DynDOLOD_SSE_childworld_*.ini`, `Rules\*.ini` / `*.patch`, `Configs\*.txt`,
and UTF-16 strings of `DynDOLODx64.exe` / `TexGenx64.exe` (same code base; string tables are identical). The INI
loaders are the string runs after `LoadDynDOLODINI` (UTF-16 string dump lines 52187-52489, file offsets 0x146daf4-0x1471f6c) and
`LoadTexGenINI` (lines 53492-53572), `LoadTexGenPreset`/`SaveTexGenPreset`, the preset save/load runs
(`DoObjectLOD ... LODY`), `LoadChildWorldSettings` and `LoadPluginSettings` (lines 58525-58800).

LODForge reads `DynDOLOD_SSE.ini` `[DynDOLOD]` through `Config::cfg/num` (apps/cli/src/dyndolod_build.rs:145-171) and
`Configs\DynDOLOD_SSE_childworld_*.ini`. It does **not** read `TexGen_SSE.INI`, `Presets\DynDOLOD_SSE_Default.ini`
(only `dyndolod_export.rs:57`, a golden-diff tool) or `Presets\DynDOLOD_SSE_TexGen.ini`. Several preset (UI) keys are
looked up in `DynDOLOD_SSE.ini`, where they never exist, so they are effectively hardcoded: marked **W**.

Status legend: **R** read (file:line) - **CLI** a lodforge flag replaces it - **H** hardcoded (value) - **W** looked up
in the wrong file, default always used - **A** absent. Effect: does changing it change in-game output (Y / likely / no).

## 1. `DynDOLOD_SSE.ini` `[DynDOLOD]` (shipped keys)

| key | default | controls | LODForge | effect | imp |
|---|---|---|---|---|---|
| CrownBrightness, TrunkBrightness, FlatTrunkBrightness (+`R/G/B`, +`_SPR/_SUM/_AUT/_WIN`) | 1, 1, 1 | vertex-colour multipliers of 3D tree LOD in object LOD | R dyndolod_build.rs:282-283, written export.rs:420-424. **Bug**: per-channel lookup is `CrownBrightness_R`; exe key is `CrownBrightnessR` (no underscore). Season suffixes absent | Y | M |
| TreeFullFallBack | 1 | what a tree gets in object LOD when no 3D LOD model exists (0 full, n BillboardN) | A (no fallback logic; tree columns only from rules, assign.rs:108-131) | likely | L |
| TreeBillboard1..6 | Internal, flat_2x2, 3x2, 4x2, Internal, LODGen_flat | billboard template NIF for rule value BillboardN | H Internal for every N: assign.rs:114/130 ignore N, FlatTextures writes flat_normals for all levels export.rs:456 | Y (rules using Billboard2+) | M |
| TreeLODMaxTilt | 30 | tilted trees go to object LOD | R dyndolod_build.rs:359 (also :246) | Y | - |
| AllHDLOD32 (+hidden AllHDLOD4/8/16) | 0 | force/clear HD flag on snow shapes per LOD level | A | Y if set | L |
| IgnoreLargeReferences | 0 | skip large-ref handling | R dyndolod_build.rs:613 | Y | - |
| TreeLargeRefSize | 400 | min tree size for "Large" ultra trees | A (ultra absent) | Y with ultra | L |
| CreateEmptyBTT | 0 | ultra: empty .btt instead of empty .lst | A (ultra absent) | Y with ultra | L |
| FlatLODLightingEffect | 0.0 | soft-light Lighting Effect 1 on billboards | R dyndolod_build.rs:201 | Y | - |
| FlatLODGlowMap | textures\white.dds | 3rd texture of soft-lit billboards | H white.dds export.rs:457 | Y if changed | L |
| FlatLODVertexColor | 0.55 | billboard brightness | R dyndolod_build.rs:199 | Y | - |
| FlatLODWithNormalMapVertexColor | 0.50 | brightness of billboards that have a `_n` normal map | A (0.55 used for all billboards, dyndolod_build.rs:199) | Y | M |
| FlatLODColorVariance | 0.1 | random tint of billboards | R dyndolod_build.rs:200 | Y | - |
| FlatLODHeightVariance | 0.0 | random height of billboards | R dyndolod_build.rs:202 | Y | - |
| FlatLODLevelLODFlag | 0 | SLSF2_LOD_Objects on billboards from level n | R dyndolod_build.rs:297, export.rs:419 | Y | - |
| TreeLODMaxAtlasSize | 16384 | classic tree LOD atlas cap (one atlas/world) | H 8192 tree_lod.rs:244 | likely (big tree sets overflow) | M |
| TreeLODComplexAtlas | 0 | diffuse+normal "complex" tree atlas (CS) | A | Y if set | L |
| TreeLODCreateAverageColor / TreeLODForceAverageColor | 1 / 0 | fill transparent atlas texels with average colour | A (tree atlas tree_lod.rs:132-162 has no fill; TexGen-side bleed only, texgen lib.rs:298) | likely (fringes in far mips) | M |
| GrassLODCreateAverageColor / GrassLODForceAverageColor | 1 / 0 | same for grass LOD textures | A | likely | L |
| ObjectLODMaxAtlasSize | 8192 | object LOD atlas page cap | H 4096 page, 1024 max texture (larger textures skipped) dyndolod_build.rs:456-457, :528 | likely | M |
| ObjectLODCreateAverageColor / ObjectLODForceAverageColor | 1 / 0 | average-colour fill for object LOD atlas | A (object_atlas.rs:45-61 plain Lanczos) | likely | M |
| ObjectLODDiffuseFormat / ObjectLODAlphaDiffuseFormat / ObjectLODNormalFormat | 200 / 225 / 200 | atlas formats; differing formats -> 2 atlas files | H BC7 single file object_atlas.rs:60 | no (quality/size) | L |
| TreeLODDiffuseFormat | 225 | tree atlas format | H BC7 tree_lod.rs:161 (same) | no | L |
| ObjectLODGenerateMipMaps / TreeLODGenerateMipMaps | 2 / 2 | mip generation (0 none, 1 texconv, 2 internal) | H internal Lanczos object_atlas.rs:45, tree_lod.rs:132 | no | L |
| MaxTextureSize | 8192 | largest texture handled | A (atlas uses 1024 cap, dyndolod_build.rs:457) | likely | L |
| AlphaCoverage | 0.65 | texconv mip alpha coverage | A (only with mipmaps=1) | no | L |
| AlphaFactor | 0.25 | internal mip alpha scaling (keeps alpha-tested LOD from thinning) | A (object_atlas.rs:45, tree_lod.rs:132) | Y (foliage thins at distance) | M |
| IgnoreTexture | list | textures kept off the atlas | R dyndolod_build.rs:295, :517 | Y | - |
| GlowLODLocalMap | 0 | ShowLocalMap flag on glow refs | A | Y if set | L |
| LIGH2JSON | 0 | export LIGH to JSON for CS light mods | A | Y with CS LLF | L |
| FakeLightsMinScale / FakeLightsChildWorldMinScale / FakeLightsIntensity | 1 / 1 / 6 | fake-light glow orbs | A (fake lights absent) | Y with FakeLights | L |
| BaseSignatures | STAT,TREE,ACTI,MSTT,CONT,FURN,DOOR | base types scanned | H same list bases.rs:9 | Y if changed | L |
| uGridsToLoad | 5 | grid arithmetic ("leave at 5") | A: LODForge reads the game's skyrim.ini uGridsToLoad instead (dyndolod_build.rs:113) | likely (large-ref margin if game uses 7+) | M |
| fMinSecondsForLoadFadeIn / fNearMultiple / fFarMultiple | 2.7 / 1.25 / 1.5 | MCM defaults (fade-in after fast travel, glow) | A (MCM quest has no such props, dynplugins.rs:150) | likely (runtime defaults) | L |
| fNeverMultiple | 1.75 | `glowmultiple` in DynDOLOD_NG.ini | H 1.75 dynplugins.rs:647 | Y if changed | L |
| RemoveUnseenZShift | 10.0 | terrain offset for remove-unseen-faces | R dyndolod_build.rs:296 | Y | - |
| ConvertEnabledReferences | 1 | refs with enable parent -> base copy without Has Distant LOD | A (such refs are just skipped, export.rs:199) | likely | L |
| PartialWorldspace / PartialPersistentCell | 0 / 1 | Partial Form flag on WRLD / persistent CELL overrides | A (no 0x4000 flag written) | likely (override reverts other mods' cell data) | M |
| UseDecalFlag | 0 | keep decal flag in object LOD | H False export.rs:405 | Y if set | L |
| IgnoreModFileName | SDO, JK's, TAVE, ... | refs added by these plugins get no LOD | A | Y (doubled/floating LOD with those mods) | M |
| Folder1..5 | meshes, dlc01\lod, dlc02\lod, lod, dyndolod | LOD model scan order | R dyndolod_build.rs:178 | Y | - |
| IgnoreLODMesh | list | paths excluded from LOD discovery | R dyndolod_build.rs:179 | Y | - |
| IgnoreFullMesh | list | full-model paths excluded | A | likely | L |
| IgnoreBaseEDID | loadscreen, _invis_, dbm_arrivalship | base EDIDs skipped | R dyndolod_build.rs:156, used export.rs:203 | Y | - |
| ValidRootBlocks | BSFadeNode, BSLeafAnimNode, BSTreeNode | NIF roots accepted | A | likely (bad NIFs in LOD) | L |
| Temporary | 0 | activators as temporary refs | A (default behaviour assumed) | likely | L |
| OcclusionHeight / OcclusionRadius | 0 / 100 | observer height offset, max distance | R dyndolod_build.rs:641 | Y | - |
| OcclusionMode | -TVDT -Flat +Border | which cells get TVDT | R dyndolod_build.rs:642 (fallback "-Flat +Border" differs from shipped default if key removed) | Y | - |
| Occlusion_\<W\> | ArnimaDagonRealm=0 | force occlusion off/on per world | R partial dyndolod_build.rs:657: only `=0`; `=1` (force small world on) absent | Y | L |
| OcclusionMode_\<W\> | Tamriel, Sovngarde, SoulCairn | per-world mode | R dyndolod_build.rs:655 | Y | - |
| OptimizeMode_\<W\> + OptimizeDistanceLOD4/8 (+hidden OptimizeMult4/8/16, OptimizeDistanceLOD16) | Tamriel 1, Solstheim 1; 69420/100000 | skip LOD4/8 chunks never loaded (`LimitCoords4/8`) | A (Header.limit_coords4/8 never filled; emitted only if non-empty export.rs:389-394) | no (only file count/size) | L |
| GrassGID | cgid | grass cache extension | H `.cgid` crates/grass/src/lib.rs:178 | Y if renamed | L |
| GrassBillboard1..6 | Internal, 2x2alt1, 2x2alt2, 4x2alt1, 4x2alt2, 4x2alt3 | grass billboard templates | R via index dyndolod_build.rs:697 | Y | - |
| GrassBillboard | 1 | template for plain grass | H Internal (normal grass lines unchanged, dyndolod_build.rs:694) | Y if changed | L |
| ComplexGrassBillboard | 4 | template for complex grass | R dyndolod_build.rs:696 | Y | - |
| CSGrassOptimizations | 0 (auto) | CS Grass Optimizations mode | CLI `--grass go` dyndolod_build.rs:70; no auto-detect | Y | L |
| CSGrassOptimizationsMipMaps | 2 | mips for GO textures | A | no | L |
| CSGrassOptimizationsBillboard | DynDOLOD_flat_2x2plain_lod.nif | GO template | A (procedural crossed quads, dyndolod_build.rs:918-923) | likely | L |
| GrassBrightness{Top,Bottom}{R,G,B} | .400/.445/.450, .200/.223/.225 | grass LOD brightness | R dyndolod_build.rs:680-684 (season suffixes absent) | Y | - |
| ComplexGrassBrightness{Top,Bottom}{R,G,B} | .70/.75/.75 | complex grass brightness | R dyndolod_build.rs:680-684 (season suffixes absent) | Y | - |
| GrassGlowMap / GrassBacklightMask / ComplexGrassGlowMap / ComplexGrassBacklightMask | 0 | replace template glow/backlight textures (0 = keep template's) | H `textures\black.dds` dyndolod_build.rs:713 | likely (complex grass backlight) | L |
| LODGenHeightMap | 1 | LODGen exports object height map for occlusion | H `CreateHeightMap=True` export.rs:401 | Y | - |
| DoubleSidedTextureMask / DoubleSidedMeshMask | (commented) | force double_sided (EVLaS/volumetric) | A | Y if set | L |
| TerrainUndersideIgnoreWorlds | list | worlds without underside | R dyndolod_build.rs:119 | Y | - |
| TerrainUnderside{Min,Max}{X,Y}_\<W\> | Solstheim | underside cell bounds | R terrain_underside.rs:53 | Y | - |
| TerrainUndersideIgnoreHide_\<W\> | MarkarthWorld cells | ignore hide-quad flag | R terrain_underside.rs:53 | Y | - |
| DoTerrainHeightMap, DoObjectHeightMap, TerrainHeightMapIgnoreWorlds, TerrainHeightMap{Min,Max}{X,Y}_\<W\> | 0, 0, list, Solstheim | terrain/object height-map textures (map/tool aid) | A | no | L |
| SeasonsCopy | 1 | copy default .bto when season unchanged | H (LODCoords of changed quads only) dyndolod_build.rs:385 | no | L |
| SeasonsCombineAtlas | 1 | one atlas for all seasons | H dyndolod_build.rs:488 | no | L |
| SwapsChance | 0 | random-chance Base Object Swapper swaps | A (BOS `*_SWAP.ini` not read at all; seasons swaps only, crates/seasons/src/lib.rs:18) | Y with BOS mods | M |

## 2. Preset / UI state (`Presets\DynDOLOD_SSE_Default.ini` `[DynDOLOD]`, and user presets)

| key | default (this preset) | controls | LODForge | effect | imp |
|---|---|---|---|---|---|
| OutputPath | path | output folder | CLI `--out` build.rs:33 | no | - |
| Preset | Medium | rule preset Low/Medium/High | CLI `--preset` dyndolod_build.rs:38 | Y | - |
| PresetCopyChild | Low | child->parent copy preset (IgnoreChildEDIDLow/Medium/High) | A: conflated with `--preset` (build.rs:291 -> childworld.rs:82) | Y (city contents seen from outside) | M |
| DoObjectLOD | 1 | object LOD | CLI `--no-objects` build.rs:45 | Y | - |
| Level32 | 0 | generate LOD32 | H off export.rs:405, dyndolod_build.rs:191 | Y (map / far LOD32) | M |
| DoTreeLOD | 1 | tree LOD | CLI `--no-trees` build.rs:47 | Y | - |
| UltraTreeLOD | 0 | trees in object LOD (3D/billboard "ultra") | A | Y | H |
| TreeLargeRef | 0 | full-model trees as large refs (DLL NG) | A | Y | L |
| DoDynamicLOD | 1 | dynamic LOD plugins | CLI `--static-lod` dyndolod_build.rs:45 | Y | - |
| NearGrid / FarGrid | 11 / 21 | dynamic LOD grid sizes | W dyndolod_build.rs:614, dyndolod_export.rs:121 (default 11/21) | Y if changed | M |
| DoOcclusion | 1 | Occlusion.esp | CLI `--no-occlusion` dyndolod_build.rs:62 | Y | - |
| OcclusionQuality | 2 | TVDT quadtree depth | W dyndolod_build.rs:641 (default 2) | Y if changed | M |
| OcclusionPlugin | 1 | TVDT into separate Occlusion.esp | H 1 dyndolod_build.rs:665 | no | L |
| DoGrass | 1 | grass LOD | CLI `--grass bto` dyndolod_build.rs:70 (default off) | Y | - |
| GrassLODDensity | 100 | grass LOD density % | CLI `--grass-density`, **default 50** object_lod.rs:25 (DynDOLOD 100) | Y | H |
| GrassLODMode | 1 | grass LOD mode 1/2 (must match GrassControl) | A | Y | M |
| DoTerrainUnderside | 1 | terrain underside | CLI `--no-underside` dyndolod_build.rs:94 | Y | - |
| TerrainUndersideQuality | 10 | underside mesh quality | W terrain_underside.rs:57, else H 10 build.rs:384 | Y if changed | L |
| TerrainUndersideHeight | 500 | underside depth | W dyndolod_build.rs:591 (default 500) | Y if changed | L |
| DoSeasons / SeasonsToGenerate (/SeasonsType) | 1 / Default | seasons | CLI `--seasons` build.rs:30 | Y | - |
| SeasonsSnow | 0 | snow variants for seasons | A | Y if set | L |
| LoadCandles / LoadFXGlow | 0 / 0 | add candles_* / fxglow_* rule files | A (rules.rs:233-266 never loads them) | Y | M |
| AutoMap | 0 | world-map LOD support | A | Y if set | L |
| FakeLights / FakeLightsChildWorld | 0 / 0 | fake-light glow orbs | A | Y if set | L |
| Windows | 0 | glow LOD windows | CLI `--windows` dyndolod_build.rs:91 | Y | - |
| WindowsHigh | 0 | windows in dynamic LOD (react to daylight) | A | Y if set | L |
| UseOrgLOD | 0 | prefer base-record MNAM LOD over rules | A | Y if set | M |
| UpgradeLargeReferences | 0 | large refs on NearGrid -> FarGrid | H 0 export.rs:540 | Y if set | L |
| WorkaroundLargeReferencesBugs | 1 | large-ref workarounds | CLI `--no-large-ref-workarounds` dyndolod_build.rs:88 (+ W :613) | Y | - |
| DowngradeNormalReferences | 1 | non-large FarGrid refs -> NearGrid | H 1 dynplugins.rs:521 | Y if unset | L |
| CopyParent | 0 | parent->child reference copies | A (ScanParent only used for tree bounds, dyndolod_build.rs:259) | Y | M |
| ObjectLODMaxTileSize | 1024 | atlas tile cap | H 1024 dyndolod_build.rs:457 | Y if changed | M |
| TreeLODMaxTileSize / GrassLODMaxTileSize / FullTextureMaxTileSize (+`*Default` from resolution) | 1024 / 128 / 256 | tile caps per atlas kind | A | Y | M |
| ObjectLODBrightness / TreeLODBrightness / GrassLODBrightness / FullTextureBrightness (+FullTextureSizeDivider) | 0 | atlas brightness tweaks | A | Y if changed | M |
| LODLevel / LODX / LODY | 0, unset | expert single-chunk run | A (UI) | no | - |
| `<WorldEDID>=0/1` | Tamriel=1 | worlds to generate | CLI `--worlds` build.rs:27 | Y | - |
| `[Skyrim LODGen]` saved rule table | 87+ rules | the user's edited rule list | A: rebuilt from Rules\ every run (rules.rs:233); preset table only used by dyndolod_export.rs:57-58 | Y (user rule edits lost) | H |

## 3. Hidden `[DynDOLOD]` keys (exe loader only, not shipped)

| key | controls | LODForge | effect | imp |
|---|---|---|---|---|
| TreeLODMinTilt | lower tilt bound | A | likely | L |
| NoAutoSnow (list) | models excluded from automatic snow LOD | A | likely | L |
| SeasonsIDs / SeasonsNames / SeasonSections / SeasonsSnowAllowSignatures / SeasonsSnowDisallowModels / SeasonsSnowDisallowEDID | season codes, swap sections, snow rules | H seasons codes in crates/seasons; snow rules A | no (defaults) | L |
| ObjectLODRMAOSFormat / ObjectLODSSSFormat | PBR LOD atlas formats | A | Y with PBR | L |
| ObjectLODMinMipMaps / TreeLODMinMipMaps / CPUMipMaps / ObjectMipMaps | mip details | A | no | L |
| SplitTextureMaxSize, AtlasSpacing, SeparateAlphaAtlas, sRGBAtlas, LinearInternal, AlphaAdjustAfter, CreateMipMaps, PrepareImageColorFixCoarse/Fine | atlas building details | A | likely (subtle) | L |
| GrassLODFullAtlas / GrassLODMiniAtlas / IgnoreMiniAtlas | grass/mini-atlas handling | A (glow_atlas.rs drops mini-atlas sources only) | likely | L |
| FlatLODBacklightMask | backlight texture for billboards | A (field empty in FlatTextures, export.rs:448-463) | likely | L |
| RemoveUnseen / DontMergeShapes / Experimental / IgnoreUDRZValue | LODGen switches | H `RemoveUnseenFaces=True`, `DontMergeShapes=False` export.rs:395-396, `Experimental=True` :405; IgnoreUDRZValue A | Y if changed | L |
| IgnoreLODModels / IgnoreXESP / FlagTrees / PortalBoxFix / UndersideWorkaround / DisableParentChildMatches | processing switches | A (defaults) | likely | L |
| LargeReferencesSupport / LargeReferencesDLL / BaseLODDLL / WorldMapSupport / WorldMapDLL / WorshipperACTI / WorshipperLargeRef | DLL detection, worshipper markers | H (NG DLL assumed) | likely | L |
| uLockedObjectMapLOD, CellGrid, MaxBoundsWarning / MaxBoundsError, ExportLANDBinary, AddLoadScreens, AddEvilNote | misc | A | no/likely | L |
| ComplexGrassUseMipmaps, CSGrassOptimizationsMinMipMap(s), IgnoreGrassPlacementBrightness, IgnoreComplexGrassPlacementBrightness | grass details | H `IgnoreGrassPlacementBrightness=False` etc. dyndolod_build.rs:679; rest A | likely | L |
| Swaps | Base Object Swapper support on/off | A (no BOS) | Y with BOS | M |

## 4. `TexGen_SSE.INI` `[TexGen]` (LODForge never reads it)

| key | default | controls | LODForge | effect | imp |
|---|---|---|---|---|---|
| MaxTextureSize | 8192 | largest texture | A | no | L |
| DoAllGrasses | 0 | billboards for unused GRAS too | H used grasses only texgen_billboards.rs:585-599 | no | L |
| ForceComplexGrass | 1 | normal maps for all grass billboards when any complex grass | H "CS dll present" dyndolod_build.rs:672-674; TexGen rule A | Y | M |
| CSGrassOptimizations / CSGrassOptimizationsSides | 0 / 0 | GO assets; 2 or 4 sides | CLI `--grass go`; sides A | Y | L |
| TreeNormalMaps | 0 | `_n` for tree billboards | CLI `--normals` texgen_billboards.rs:41 | Y | - |
| BillboardDiscardRatio | 0.01 | skip near-empty billboards | A | likely | L |
| BillboardMinWidthMulitplier / BillboardMinHeightMulitplier | 4 / 4 | skip tiny billboards | A | likely | L |
| MinTreeModelMagnitude / MinTreeModelHeight | 512 / 256 | auto tree discovery | H texgen_billboards.rs:151-152 | Y if changed | L |
| MinGrassModelMagnitude / MinGrassModelHeight | 64 / 32 | auto grass discovery | H texgen_billboards.rs:597-599 | Y if changed | L |
| TreeModelHeightMultiplier | 1.7 | default max ref scale for tree px size | H texgen_billboards.rs:221 | Y if changed | L |
| GrassModelHeightMultiplier | 1.0 | grass px size | H (texgen_billboards.rs:877) | Y if changed | L |
| RenderModelHeightMultiplier | 1.85 | rendered billboards px size | H texgen_billboards.rs:835 | Y if changed | L |
| UseMipmaps / UseMipmapsComplexGrass | 1 / 1 | use source mips when rendering | H texgen render.rs:77-89 | no | L |
| BleedToTransparent | 1 | bleed opaque colour into transparent | H texgen lib.rs:298 (BleedRadius 2) | Y if changed | L |
| MaxMultiSamples | 16 | MSAA | H texgen render.rs:143 | no | L |
| TreeMSAlphaThreshold / GrassMSAlphaThreshold / ObjectMSAlphaThreshold | 128 / 112 / 128 | alpha cut-off | H lib.rs:29 (128), texgen_billboards.rs:466/468/878 (112); object A (default) | Y if changed | L |
| GLAlphaCoverage / ACAlphaThreshold | 0 / 255 | GL sample coverage | A (off = default) | no | L |
| TreeMaxSuperSamples / GrassMaxSuperSamples / ObjectMaxSuperSamples, SSAlphaThreshold, SSRenderAlpha, SSBlendAlpha | 0 / 0 / 0, 128, 0, 0 | supersampling | A (0 = default) | Y if raised | L |
| AlphaCoverage | 0.65 | texconv alpha coverage | A | no | L |

## 5. TexGen UI state (`Presets\DynDOLOD_SSE_TexGen.ini` `[TexGen]`, never read)

| key | value | controls | LODForge | effect | imp |
|---|---|---|---|---|---|
| OutputPath | path | output | CLI | no | - |
| StitchPBRScale / RenderPBRScale / BillboardPBRScale | 0.65 / 0.65 / 0.7 | PBR colour scale | H texgen_rendered.rs:182 (0.65), texgen lib.rs:29 (0.7) | Y if changed | L |
| DiffuseFormat / DiffuseAlphaFormat / NormalFormat / NormalSpecularFormat | 225 | TexGen output formats | H BC7 texgen_billboards.rs:244/247 | no | L |
| StitchObjects / StitchObjectsSize | 1 / 256 | stitched object LOD textures and size | CLI subcommand (texgen_stitched.rs); size H | Y if changed | L |
| RenderObjects / RenderObjectsSize | 1 / 256 | rendered object LOD textures | H 256 texgen_rendered.rs:151 | Y if changed | M |
| RenderObjectsDirect / DirectLR / DirectFB / DirectTB / Ambient (+Smoothness, GlowLR/FB/TB) | 0/0/-100/0/100 | rendered object lighting | H direct 0, ambient 1.0 texgen_rendered.rs:182 | Y if changed | L |
| Billboards / BillboardsUnitsPerPixel / BillboardsMinSize / BillboardsMaxSize (+`*Default`) | 1 / 110 / 32 / 1024 | tree billboard resolution | H UPP 11, 32..1024 texgen lib.rs:29 | Y if changed (common tweak) | M |
| TreeBillboards Direct/LR/FB/TB/Ambient | 135/0/0/-100/55 | tree billboard lighting | H lib.rs:29 | Y if changed | M |
| TreeHDBillboards (+Direct/Ambient/Smoothness/Glow/Back) | 0 | HD tree billboards | A | Y if enabled | L |
| GrassBillboards Direct/LR/FB/TB/Ambient/Smoothness | 0, 150/0/0/-100/25/0 | grass billboard lighting | H texgen_billboards.rs:466 | Y if changed | L |
| GrassHDBillboards Direct/LR/FB/TB/Ambient/Smoothness/Glow/Back | 1, 0/0/0/-100/100/0/100/100 | HD grass billboard lighting | H texgen_billboards.rs:468, :878 (Glow/Back A) | Y if changed | L |
| RenderBillboards Direct/LR/FB/TB/Ambient/Smoothness/Glow/Back | 1, 50/0/0/-100/150/0/100/100 | rendered (hybrid trunk) billboards | H texgen_billboards.rs:814 | Y if changed | L |

Hidden TexGen keys (loader run): GrassTrim, HDTreeTrim, RenderTrim, TreeMipMaps, GrassMipMaps, HDGrassFaceNormals,
BleedRadius (H 2, lib.rs:298), RenderMaxSuperSamples, NormalMapSteepness, BakeGlow, BakePBR, OldBuildAtlas,
LoadGLCompressed (H, texgen_billboards.rs:44): all A or H at the TexGen default, L.

## 6. `Configs\DynDOLOD_SSE_childworld_<Child>.ini` `[<Parent>]`

| key | controls | LODForge | effect | imp |
|---|---|---|---|---|
| ScanChild | copy child refs into parent | R childworld.rs:74/82 | Y | - |
| ScanParent | copy parent refs into child | R only for tree-object bounds dyndolod_build.rs:259; copy absent | Y | M |
| NoCellsWithNAVM | no copies into navmeshed cells | R dyndolod_build.rs:102 | Y | - |
| NoScanIfPluginExists / NoScanChildIfPluginExists | disable copies with city mods | R childworld.rs:76 | Y | - |
| NoScanParentIfPluginExists | same for parent copies | A (CopyParent absent) | Y | L |
| IgnoreChildEDIDLow/Medium/High | EDIDs not copied | R childworld.rs:82 (by `--preset`) | Y | - |
| IgnoreParentEDID | EDIDs not copied to child | A | Y with CopyParent | L |
| IgnoreChildMesh / IgnoreParentMesh / ParentModel | mesh filters | A (unused in shipped files) | likely | L |
| Bounds | child area (tree LOD -> object LOD) | R dyndolod_build.rs:101, :263 | Y | - |
| BoundsIgnore | exclusion bounds (WhiterunDragonsreach) | A | likely | L |
| NoDynamicLOD | no dynamic LOD in that child (WindhelmPit, Arnimatown =1) | A | Y | M |

## 7. Rule files (`Rules\*.ini`) sections and companions

| item | controls | LODForge | effect | imp |
|---|---|---|---|---|
| `[Skyrim LODGen]`, `[Skyrim <plugin>]` | rules | R rules.rs:239-241 | Y | - |
| `[Skyrim Settings]` IgnoreWorlds (17 files) / IgnoreParentWorlds (11) / IgnoreChildWorlds / IgnoreSignatureParentWorld / IgnoreSignatureChildWorld / DefaultLOD / ForceParentCells | per-plugin world exclusions | A (rules.rs:237-246 skips non-`Skyrim ` sections; `Settings` never applied) | Y with city overhauls | M |
| `[DynDOLOD]` override section in a rule file | per-plugin INI overrides | A | likely | L |
| `*_candles_*`, `*_fxglow_*`, `*_ultra.ini` variants | optional rule sets | A (see LoadCandles / UltraTreeLOD) | Y | M |

## 8. Config text files (not keys; listed for completeness)

| file | LODForge | effect | imp |
|---|---|---|---|
| misnamed.txt | R dyndolod_build.rs:180 | Y | - |
| xesplod.txt | R dyndolod_build.rs:153 | Y | - |
| flat_normals.txt | R (path written) dyndolod_build.rs:198 | Y | - |
| childworldfull.txt, ChildworldMatches.txt | R childworld.rs:48-60, :85-90 | Y | - |
| TexGen_{noalpha,alpha,copy}_\<plugin\>.txt, TexGen_Billboards_*, Required_Billboards_* | R texgen_stitched.rs:105-107, texgen_billboards.rs:106 | Y | - |
| `*.patch` (14 SSE, incl. skyrimesm_tamriel, skyrimesm_tamriel_largerefbugs, updateesm_tamriel, dawnguardesm_all) | A | Y (vanilla record fixes) | M |
| mod_world_ignore.txt (55 lines) | A | Y (refs of listed plugins get LOD) | M |
| mesh_replace.txt / mesh_lookup.txt | A | Y (windmill fans etc.) | M |
| childworldlod_Tamriel.txt (162) | A | Y (city refs given LOD in Tamriel) | M |
| mod_default_lod.txt (UseOrgLOD forced) | A | Y | M |
| xespignore.txt | A | Y | L |
| manual_static/base/refr_*.txt | A (1 Tamriel row) | Y | L |
| mesh_crc32.txt | A (CRC-named LOD lookup exists, export.rs:470-477, without the vanilla CRC table) | likely | L |
| texture_replace / textures_names / textures_simplify | A | likely | L |
| glow_{low,medium,high}.txt | A (shipped files are comments only) | no | L |
| default_lod.txt, Required_Resources.txt, missing_*.txt, wildedit_ignore.txt, worldspace_ignore.txt, parentcopy_ignorepatches.txt | A (validation, warnings, UI list) | no | L |

## 9. External INI

| key | LODForge | effect | imp |
|---|---|---|---|
| SkyrimPrefs `uLargeRefLODGridSize` | R dyndolod_build.rs:112 | Y | - |
| Skyrim.ini `uGridsToLoad` | R dyndolod_build.rs:113 (DynDOLOD uses its own INI value 5) | likely | M |
| SkyrimPrefs `fBlockLevel0Distance` / `fBlockLevel1Distance` / `fBlockMaximumDistance` | A (only for OptimizeMode) | no | L |
| SSEDisplayTweaks / Prefs resolution -> `*MaxTileSizeDefault`, `*SizeDefault` | A | likely | L |

## 10. UI-only / log-only / performance keys (no effect on generated files)

DynDOLOD: Wizard, Expert, CreateMod (alternate mode), Verbose, Notice, Debug, ReplaceModFileName(+Help), WarnModFileName(+Help),
RemoveModFileName(+Help), MaxRenderResolution, OcclusionMaxThreadsTerrainLOD, OcclusionMaxThreadsObjectLOD, LODGenThreadSplit
(H `ThreadSplit=0` export.rs:405), LODGenThreads, MaxLODGen, MaxLODGenThreads, MaxLODGenPerWorldspace,
MaxLODGenThreadsPerWorldspace, BaseThreads, RenderThreads, StitchThreads, AtlasThreads, RenderContexts, RenderSingle,
TextureCache, MaxTextureIndex, TexconvAdapterIndex, LockTexconv, LockRecords, LogLock, GLDebug, GLListDebug, KeepRenderer,
Summary, SummaryOnly, TreeTests, MipMapTests, BuildReverseList, AddMissingThread, CreateArchive, ArchiveType, ArchiveLevel,
ArchiveThreads, DeleteFiles, FormID, DebugFormID, PresetFilename, DotGrassGID, Theme, LODLevel/LODX/LODY, OutputPath,
the `[frmMain]`/`[Init]`/`[Options]`/`[SSE LOD Options]` sections of DynDOLOD.ini / TexGen.ini (xEdit window state).
TexGen: Notice, RenderThreads, CreateMod, MaxRenderResolution, RenderTexturesSingleThread, RenderBillboardsSingleThread,
BillboardsPreviewModel, RenderPreviewModel, Wizard, LockTexconv, GLDebug, GLListDebug, KeepRenderer, Summary(Only), LogLock,
CreateArchive/ArchiveType/Level/Threads, DeleteFiles, Swaps (TexGen copy), TexconvAdapterIndex, TextureCache.

## Counts

171 rows in sections 1-7 and 9 (grouped keys count once; config files of section 8 and the UI/log list excluded):
R 33, CLI 18, H 45, W 4 (looked up in DynDOLOD_SSE.ini but live in the preset), A 71.
Absent (A) or W and affecting output: 66 (H 2, M 21, L 43); A alone: 62. Output-affecting config files absent: 9 (section 8).
