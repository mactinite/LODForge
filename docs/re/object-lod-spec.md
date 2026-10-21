# Object LOD (`.bto`): scrubbed specification

Written 2026-10-19 as stage B0.2 of [the reimplementation plan](../reimplementation-plan.md), so
that fresh sessions can rewrite `crates/objlod` and the LODGen-mirroring pieces of `apps/cli`
(stage B2) **without ever seeing the LODGen-derived code or the LODGen decompiles**. Everything
here comes from sources we may use: MPL-2.0 xEdit code we own, dyndolod.info documentation, the
shipped DynDOLOD configuration files, the xLODGen readmes, the NIF format, AnvilLOD (GPL-3.0-only)
and the golden output of real xLODGen / DynDOLOD runs. **It describes inputs, outputs, formats
and observable behaviour, never how LODGen computes anything.** Where a fact could not be
established from those sources it is an open question (section 10) that the implementer settles
empirically against the golden files with the comparison tooling of section 8.

Rule used while writing: if a sentence needed the decompile to be written, it is not here; it is
in the quarantine (section 9.3). Behaviour that is only knowable from the decompile but visible in
the output is stated as an observed output property with the golden file named.

**Evidence tags.**

| Tag | Source |
|---|---|
| `[xedit-src]` | MPL-2.0 xEdit sources, `D:\Modlists\ref\TES5Edit\Core\wbLOD.pas` (object LOD export writer, lines ~2960-3140 and ~3540-3660; atlas builder ~1020-1080; defaults ~240-300), and our ports of it: [crates/objlod/src/source.rs](../../crates/objlod/src/source.rs) (reference selection and export columns) and [crates/objlod/src/atlas_build.rs](../../crates/objlod/src/atlas_build.rs) (atlas layout and atlas-map writer). These two files stay. |
| `[doc]` | <https://dyndolod.info/Help/...> pages read 2026-10-19: Object-LOD, Mesh-Mask-Reference-Rules, Mod-Authors, Expert-Mode, LODGen, Snow-Ash-LOD-Shader, Glow-LOD, Large-References, Seasons, Ultra-Tree-LOD, 3D-Tree-LOD-Model, Tree-LOD, Tree-Grass-LOD-Billboards (the `Tree-LOD-Billboards` URL is a 404), Grass-LOD, Child-Parent-Worldspace-Copies, Object-Bounds, TexGen-Configuration. xLODGen readmes in `D:\Modlists\skyrim-cs\tools\xLODGen\`: `Readme.txt`, `LODSettings-File-Readme.txt`, `Seasons-Readme.txt`. UESP for record and `.lod` formats. |
| `[cfg]` | Shipped DynDOLOD files under `D:\Modlists\skyrim-cs\tools\DynDOLOD\Edit Scripts\DynDOLOD\` (read only, never bundled): `DynDOLOD_SSE.ini`, `Rules\DynDOLOD_SSE_*.ini`, `Configs\DynDOLOD_SSE_*.txt`, the billboard template NIFs `DynDOLOD_flat_*_lod.nif` and `LODGen_flat_lod.nif`, `Cache\DynDOLOD_SSE_re-uv-miniatlas.txt`, and `docs\trees.ultra\tools\re-uv\re-uv.txt`. |
| `[golden]` | Real runs, see the table in section 8.3: xLODGen 3.0.36 (`no-seasons-20261017-031139`, `seasons-20261017-032143`), DynDOLOD 3 with LODGen 3.1.8 (`dyndolod-golden2`, `dyndolod-grass-20261018`), TexGen (`texgen-20261017`, `texgen-grass-20261018`), plus the xLODGen export files in `D:\Modlists\skyrim-cs\tools\xLODGen\Edit Scripts\`. Facts were read with our own tools (`crates/nif/examples/peek_bto.rs`, `bto_stats.rs`, `lodforge vfs-extract`, shell text tools). |
| `[golden-log]` | The LODGen log files of those runs (`LODGen_log.txt`, `logs\LODGen_SSE_Tamriel[_SEASON]_log.txt`) and `logs\DynDOLOD_SSE_Debug_log.txt`. A log line is an observed output, not a description of the algorithm. |
| `[anvillod]` | AnvilLOD, GPL-3.0-only, Copyright (c) 2026 CageTV, `D:\Modlists\ref\AnvilLOD` at `606eece`. Files named per fact. Code derived from it must keep the attribution (docs/licensing.md "Obligations"). |
| `[nif-spec]` | The NIF format (nif.xml, local copy `D:\Modlists\ref\nifxml\nif.xml` from niftools 028cb35b placed by the terrain analyst; NifSkope; Skyrim SE: version 20.2.0.7, user 12, BS 100) as implemented by our reader [crates/nif/src/lib.rs](../../crates/nif/src/lib.rs) (reader side only; see 9.2 for what in that crate is off limits). Flag names are nif.xml's `SkyrimShaderPropertyFlags1/2` names. |
| `[ours]` | LODForge code that is not LODGen-derived: [crates/dyndolod](../../crates/dyndolod/src) except `occlusion.rs` (`rules.rs`, `lodmodels.rs`, `assign.rs`, `bases.rs`, `largerefs.rs`, `export.rs`, `sidefiles.rs`, `childworld.rs`, `ini.rs`), [crates/esp](../../crates/esp/src), [crates/treelod](../../crates/treelod/src), [crates/grass/src/cgid.rs](../../crates/grass/src/cgid.rs), the comparison tooling in [apps/cli/src/object_lod.rs](../../apps/cli/src/object_lod.rs) and [apps/cli/src/e2e_compare.rs](../../apps/cli/src/e2e_compare.rs), and [docs/re/dyndolod-spec.md](dyndolod-spec.md) sections that are not tagged `[lodgen-src]`. |
| `[inferred]` | Our deduction from the above; verify before relying on it. |

Units: 1 cell = 4096 game units; LOD levels 4, 8, 16, 32 are block edge lengths in cells. All file
formats are little-endian. Text files are CRLF, Windows-1252/ASCII.

## 1. Inputs

### 1.1 The LODGen export file (both dialects)

One text file per worldspace and season: header lines `Key=Value` (one per line, keys may
repeat), then one reference line per placed object, tab separated. A line is a reference line
when it starts with eight hex digits. `[golden]` `[xedit-src]` (wbLOD.pas writes exactly this).

**Where the files are** `[golden]`:

| | xLODGen (LODGen 3.0.36) | DynDOLOD (LODGen 3.1.8) |
|---|---|---|
| Export | `tools\xLODGen\Edit Scripts\LODGen[.<SEASON>].txt` | `tools\DynDOLOD\Edit Scripts\Export\LODGen_SSE_Export_<World>[_<SEASON>].txt` (golden copies in `dyndolod-golden2\export\`) |
| Atlas map | `Edit Scripts\SSELODGen-AtlasMap<World>[.<SEASON>].txt` | `Export\LODGen_SSE_ObjectAtlasMap_<World>.txt` (+ a binary `.lst` twin we do not use) |
| Side files | none (everything is in the export) | `LODGen_SSE_AltTextures_<World>[_<SEASON>].txt`, `LODGen_SSE_VC_<World>[_<SEASON>].txt`, `LODGen_SSE_FlatTextures.txt`, `LODGen_SSE_LODGen_Terrain_<World>.bin`, `LODGen_SSE_MiniAtlasExport_<World>.txt`, `LODGen_SSE_TexturesUsed_<World>.txt`, `LODGen_SSE_Grass_Map_<World>.txt` |
| Options | the export carries only a few keys; the rest were command-line switches of the xLODGen invocation (the log echoes them, 1.1.2) | every option is a header key |
| Reference count (Tamriel) | 13,617 (same in every season file) | 34,526 default season; 34,512 WIN; 34,425 SPR |

#### 1.1.1 Header keys written by xLODGen `[xedit-src]` `[golden]`

In order: `GameMode=SSE`, `Worldspace=<EditorID>`, `CellSW=<x> <y>` (the `.lod` south-west
cell, Tamriel `-96 -96`), `TextureDiffuseHD=` / `TextureNormalHD=` (the worldspace's `TNAM`
HD LOD diffuse/normal as `data\textures\...`, Tamriel `mountainslab02.dds`), `TextureAtlasMap=`
(absolute path), `AtlasTolerance=` (`UVRange - 1.0` formatted `%1.1f`; default `UVRange` 1.5
gives `0.5`; when no atlas was built `UVRange` is 10000), `PathData=<Data folder\>`,
`PathOutput=<...\meshes\terrain\<World>\Objects>`, `Season=<SPR|SUM|AUT|WIN>` (season files
only), one `Resource=<archive path>` per loaded archive except the last Data folder (100 lines in
the golden file), one `IgnoreTranslation=<lod mesh path>` per entry of the setting
`sMeshIgnoreTranslationTES5` (22 vanilla Solitude and Windhelm LOD meshes, wbLOD.pas lines
256-279; also listed in source.rs), and `FlatTextures=` lines for 3D trees (1.1.4). An "extra
options" text file chosen in the xLODGen dialog is appended verbatim.

#### 1.1.2 Header keys written by DynDOLOD `[golden]` `[cfg]`

Golden `LODGen_SSE_Export_Tamriel.txt`, in file order. "Meaning" gives what documentation or the
INI say; a dash means the key's semantics are not documented anywhere we may read and must be
treated as an open question (section 10).

| Key (golden value) | Meaning / source |
|---|---|
| `GameMode=SSE` | game |
| `Worldspace=Tamriel` | worldspace EditorID |
| `CellSW=-96 -96` | `.lod` south-west cell (1.3) |
| `LODCoords32=`, `LODCoords16=`, `LODCoords8=`, `LODCoords4=` (season files only) | the quads whose content differs from the default season, as `x,y;` south-west cell pairs (1.4) `[golden]` |
| `TerrainData=<...>\LODGen_SSE_LODGen_Terrain_Tamriel.bin` | binary terrain heights DynDOLOD exports for the hidden-face test (1.6); not needed by our pipeline |
| `LimitCoords4=` (884 entries), `LimitCoords8=` (316) | south-west cells of the LOD4/LOD8 quads to generate; all multiples of 4 / 8; x -72..68, y -56..60 (LOD4) `[golden]`. Absent for LOD16 (INI: "LOD level 16 used by default for the map is always generated completely") and produced by `OptimizeMode_Tamriel=1` ("0 = off, 1 = border, 2 = navmesh") with `OptimizeDistanceLOD4=69420`, `OptimizeDistanceLOD8=100000` ("compared to the SkyrimPrefs.INI [TerrainManager] fBlockLevel[0|1]Distance settings and whichever is greater is used") `[cfg]`. Our writer computes them in [crates/dyndolod/src/export.rs](../../crates/dyndolod/src/export.rs) `[ours]` |
| `RemoveUnseenFaces=True` | remove triangles below terrain/water (section 4); the log echoes "Remove Faces under Terrain: True", "Remove Faces under Water: True" `[golden-log]` |
| `DontMergeShapes=False` | shapes sharing a material are merged (3.3); the re-uv tool sets it `True` `[cfg]` |
| `TextureAtlasMap=`, `AltTextures=`, `FlatTextures=`, `VC=` | absolute paths of the atlas map and side files (1.2, 7.3, 1.1.4) |
| `CreateHeightMap=True` (`False` in season files) | write `<World>.objects.heightmap.dat` (7.1); INI `LODGenHeightMap=1` "LODGen calculates and exports object LOD heightmaps for occlusion generation" `[cfg]` |
| `Resource=<bsa>` (about 100 lines) | loaded archives, load order |
| `Threads=0`, `ThreadSplit=0` | performance only; INI `LODGenThreadSplit` "0 = automatically set number of main threads to number of physical cores" `[cfg]` |
| `UseDecalFlag=False` | INI: "use decal flag from source mesh for static object LOD generated by LODGen.exe, can help with z-fighting of trimmings" `[cfg]` |
| `Level32=False` | whether LOD32 BTOs are generated (none in the golden output); LOD32 is used only by the world map `[doc: Mesh-Mask-Reference-Rules]` |
| `UseOptimizer=false` | - |
| `UseHDFlag=true` | HD shapes get the HD shader name/flag (3.4); `[doc: Snow-Ash-LOD-Shader]` |
| `Experimental=True` | - |
| `NotHDMeshMask=nif`, `IsHDMeshMask=_lod_0`, `NotHDTextureMask=lod`, `=lowres`, `=architecture`, `IsHDTextureMask=Textures\Landscape\Mountains\MountainSlab02.dds`, `..._N.dds`, `mountainslab01.dds`, `mountainslab02.dds`, `dyndolod_atlas`, `flat_atlas`, `treelod*.dds` (one key per line) | substring masks on the LOD model path and on texture paths that decide whether a shape counts as "HD" (full-resolution texture) `[inferred from the names and the observed HD shapes, 3.4]` |
| `PassThruMeshMask=glow_lod`, `=passthru_lod` | LOD models whose path contains these keep their own shader values (`[doc: Glow-LOD]`, `[doc: 3D-Tree-LOD-Model]`; observed `objpassthru*` shapes, 3.4) |
| `AtlasTolerance=0.2` | UV range allowed for atlasing, as xLODGen's (1.1.1) |
| `RemoveUnseenZShift=10` | INI: "Adjust terrain height for remove unseen faces function of LODGen.exe" `[cfg]` |
| `MergeVertexColors=true` | log echoes "Merge Vertex Colors: True, True, True, True" (one per level) `[golden-log]`; semantics - |
| `FlatLODLevelLODFlag=0` | INI: "Set SLSF2_LOD_Objects flag for LOD levels equal or higher on billboards used in static object LOD ... 0 = none, 1 = LOD4, 2 = LOD8, 3 = LOD16, 4 = LOD32" `[cfg]` |
| `CrownBrightnessR/G/B=1`, `TrunkBrightness*=1`, `FlatTrunkBrightness*=1` | vertex-colour multipliers for 3D tree LOD shapes named crown/trunk/flattrunk, clamped to 0..1 `[doc: Ultra-Tree-LOD]`; season-suffixed INI keys `[cfg]` |
| `PathData=`, `PathOutput=` | as xLODGen; output is `...\meshes\Terrain\<World>\Objects\` |
| `Season=WIN` | season files only |
| `GrassMap=`, `GrassBrightnessTop/BottomR/G/B=`, `GrassLargeReference=False`, `GrassDensity=100`, `GrassGID=cgid` | grass LOD runs only (6.2) `[golden: dyndolod-grass-20261018]` |

The log also echoes options with no header key: "Fix Tangents: False ×4", "Generate Tangents:
True ×4", "Generate Vertex Colors: True ×4", "Merge Meshes: True", "Grouping: False", "Ignore
Materials: False", "Alpha DoubleSided: False", "Default Alpha Threshold: 128", "Use Source Alpha
Threshold: False", "Use Backlight Power: False", "Max Level: 32" `[golden-log]`. The FO4 branch of
wbLOD.pas writes the matching keys `RemoveUnseenFaces`, `IgnoreWater`, `DontGenerateVertexColors`,
`DontGenerateTangents`, `DefaultAlphaThreshold`, `UseAlphaThreshold`, `UseBacklightPower`
`[xedit-src]`, so a parser should accept them.

A parser must ignore unknown keys and tolerate keys repeating (masks) and trailing whitespace.

#### 1.1.3 Reference lines `[golden]` `[xedit-src]`

Tab-separated columns, 1-based:

| # | xLODGen | DynDOLOD |
|---|---|---|
| 1 | reference form ID, 8 upper-case hex digits (load-order form ID) | same |
| 2 | REFR record flags, 8 hex digits | same |
| 3-5 | x, y, z position; shortest round-trip decimal of the single-precision value (`-45866.6796875`) | `%.6f` (`-45866.679688`) |
| 6-8 | rotation x, y, z in **degrees**, `%.4f`, normalised to 0..360 | `%.6f` |
| 9 | scale: `1.0` when the REFR has no `XSCL`, else 6 decimals | `%.6f` |
| 10 | base record EditorID | same |
| 11 | base record flags, 8 hex digits (`00008000` Has Distant LOD) | also `00018800`, `00010800`, `02000040`, `00000040` (TREE) etc. |
| 12 | material: empty, `Snow`, `Ash`, `PassThru` or the lower-case MATO EditorID (`volcanicsulfurcrystalslite`), with `-LargeRef` appended for large references (`-LargeRef` alone when there is no material) | empty, `-LargeRef`, `Snow`, `Snow-LargeRef`, `Ash-LargeRef` |
| 13 | full model path as stored in the record (`Meshes\...`) | same |
| 14-16 | LOD4, LOD8, LOD16 model paths (`Meshes\LOD\...`, case as stored in `MNAM`), empty when absent | lower-case; a LOD model may also be a billboard texture (`textures\terrain\lodgen\<plugin>\<name>_<formid>.dds`) resolved through `FlatTextures` (1.1.4, 6.1) |
| 17 | (absent) | LOD32 model path, empty unless `Level32` |

The DynDOLOD dialect may append further columns; parsers must accept lines longer than listed and
treat missing trailing columns as empty `[inferred]`. Numbers are parsed as single precision.

Observed line-level facts `[golden]`:
- A reference whose rule gives it a glow LOD model gets **two** lines with the same form ID, the
  glow line first (`farmhouse02glow_lod_0.nif` in LOD4 only, LOD8/16 empty), then the normal line
  (`farmhouse02_lod.nif` ×3). Ref `00012DF6` in the Tamriel export.
- DynDOLOD appends a fixed line from `Configs\DynDOLOD_SSE_manual_static_Tamriel.txt` `[cfg]`:
  `00000000 00000000 98400.0000 -237948.0000 -7100.0000 0 0 0 1.0 ICLOD 00008000 <empty>
  meshes\lod\iclod_lod.nif ×3` (the Imperial City silhouette seen from the south).
- Child worldspace copies (walled cities) appear as ordinary lines in the parent's export, with
  their LOD model as the "full model" `[ours: childworld.rs]` `[doc: Child-Parent-Worldspace-Copies]`.

#### 1.1.4 `FlatTextures` `[xedit-src]` `[golden]` `[cfg]`

Billboard descriptions LODGen needs when a LOD model column names a `.dds` instead of a `.nif`.

- xLODGen writes them into the export as `FlatTextures=` lines, one per 3D tree billboard, 10
  tab-separated fields: `billboard path, width, height, shiftZ, scaleFactor, 1,
  LODGen_flat_lod.nif, <empty>, -1, -1` (wbLOD.pas).
- DynDOLOD writes the side file `LODGen_SSE_FlatTextures.txt` (golden: 540 lines, 15 fields each,
  the last empty):

| # | Field | Golden example / source |
|---|---|---|
| 1 | billboard diffuse (`textures\terrain\lodgen\<plugin>\<name>_<formid>.dds`) | TexGen output, `_n.dds` alongside |
| 2 | width | TexGen `.txt` sidecar `Width` (6.1) |
| 3 | height | sidecar `Height` |
| 4 | shiftZ | sidecar `ShiftZ` |
| 5 | scale | sidecar `Scale` (1) |
| 6 | lighting effect | INI `FlatLODLightingEffect=0.0` |
| 7 | normals files, four paths joined by `;` | INI `TreeBillboardNormals` → `Configs\DynDOLOD_SSE_flat_normals.txt` (normals `0,0,1`, tangents `0,0,-1`, bitangents `-1,0,0`, four entries) |
| 8 | glow map | INI `FlatLODGlowMap=textures\white.dds` |
| 9 | vertex colour | INI `FlatLODVertexColor=0.55` (`FlatLODWithNormalMapVertexColor=0.50` when the billboard has a normal map) |
| 10 | colour variance | INI `FlatLODColorVariance=0.1` |
| 11 | depth | sidecar `Depth` (width when absent) |
| 12-13 | shiftX, shiftY | sidecar |
| 14 | height variance | INI `FlatLODHeightVariance=0.0` |

Our writer is `flat_texture_line` in [crates/dyndolod/src/export.rs](../../crates/dyndolod/src/export.rs)
`[ours]`. Which template NIF the billboard uses is not in the line; it comes from the rule's
`Billboard1..6` choice and the INI `TreeBillboard1..6` table (6.1).

### 1.2 The atlas map `[xedit-src]` `[golden]` `[cfg]`

Tab-separated rows, no header. Two row shapes:

| # | 8-column row | 10-column row (DynDOLOD only) |
|---|---|---|
| 1 | source texture key | same |
| 2-3 | tile width, height (pixels) | same |
| 4-5 | tile x, y in the atlas | same |
| 6 | atlas texture path | target texture path |
| 7-8 | atlas width, height | target width, height |
| 9-10 | | scale U, scale V |

- xLODGen rows (`SSELODGen-AtlasMapTamriel.txt`, 76 rows): key is a single diffuse path, atlas
  `textures\terrain\Tamriel\Objects\TamrielObjectsLOD.dds` (4096×4096) and `...LOD01.dds`
  (4096×2048): pages are numbered `<base>.dds`, `<base>01.dds`, ... and cropped to a power of two;
  the normal atlas is the same name with `_n`. Layout algorithm: `wbBuildAtlas` / `TwbBinPacker`
  in wbLOD.pas, ported in atlas_build.rs (stable sort by height then width descending; first free
  node, depth first, right before down; blocks that do not fit go to the next page). Textures
  qualify when every UV of every lighting-shader shape using them lies within `±UVRange`
  (UVs beyond ±100 are ignored; two Whiterun terrain LOD meshes are forced untiled). Rows are
  sorted by lower-case key when mini rows are present `[xedit-src]`.
- DynDOLOD rows (`LODGen_SSE_ObjectAtlasMap_Tamriel.txt`, 700 rows: 355 eight-column, 345
  ten-column): key is `diffuse[,normal[,glow]]` (comma list, lower case, e.g.
  `textures\architecture\riften\riftenwindows02noalpha.dds,...\riftenwindows02_n.dds,...\riftenwindows02_g.dds`).
  Targets: `textures\DynDOLOD\LOD\DynDOLOD_Tamriel.dds` 8192×4096 (270 rows),
  `DynDOLOD_Tamriel_Alpha.dds` 4096×2048 (79 rows, alpha-tested textures), `DynDOLOD_Tamriel_Glow.dds`
  1024×512 (6 rows, keys with a `_g` glow map, 256×256 tiles), and TexGen mini atlases
  `textures\lod\<name>.dds` (ten-column rows with scale `1 1`, e.g. `farmhouselod01.dds` 256×512
  holding four 128×128 tiles). `ObjectLODMaxAtlasSize=8192` `[cfg]`. The `.lst` twin is binary and
  unused. The semantics of scale U/V are given by the re-uv tool `[cfg]` (section 5).
- A texture not in the map keeps its own texture in the BTO (`[doc: Object-LOD]`: "single texture
  when UVs don't allow atlas, e.g. mountain slabs"); observed `mountainslab02.dds`,
  `ridgedstone05.dds`, `dirt02.dds`, `snow01.dds` shapes in `Tamriel.4.0.-16.bto` `[golden]`.

### 1.3 `.lod` settings and the LOD grid `[doc]` `[anvillod]` `[golden]`

`Data\lodsettings\<World>.lod`, 16 bytes: `int16 swX, int16 swY, int32 stride, int32 minLevel,
int32 maxLevel` (UESP, AnvilLOD `LodSettings.cs`). Tamriel (vanilla, from `Skyrim - Meshes0.bsa`):
`-96, -96, 256, 4, 32`. xLODGen's readme: the south-west cell is the worldspace bounds minimum,
stride the next power of two covering the cells to the north-east, levels 4..32; an "Align"
option rounds the origin. The grid is the same for every tool.

- A cell `(cx, cy)` belongs to the level-`L` block with origin `sw + floor((c - sw) / L) * L`
  per axis (AnvilLOD `LodGrid.BlockOrigin`; the xEdit port tests `block >= sw` for the inside
  check and uses integer division on non-negative offsets). A reference is inside the grid when
  `sw <= c < sw + stride`.
- Output file: `meshes\terrain\<World>\Objects\<World>.<L>.<bx>.<by>[.<SEASON>].bto` (DynDOLOD
  writes `Terrain` with a capital T; the engine is case-insensitive). `[golden]`
- Observed counts, Tamriel: xLODGen 499 LOD4, 141 LOD8, 43 LOD16 (683); DynDOLOD 499 / 142 / 44
  (685) per season, 5 seasons. No LOD32 file in either.
- Inside a BTO every shape has translation `(bx·4096, by·4096, 0)` and scale `L`; vertex
  positions are block-local **divided by `L`** (so a LOD4 quad spans 0..4096 in local units, a
  LOD16 quad too) `[golden: Tamriel.4.0.-16.bto t=(0,-65536,0) s=4; Tamriel.16.0.0.bto s=16]`
  `[anvillod: BtoBuilder "Vertices are stored block-local and divided by the level"]`.

### 1.4 Seasons `[doc]` `[cfg]` `[golden]`

- Swap files `Data\Seasons\*.ini` (`[SPR|SUM|AUT|WIN]` suffix, `[Original]`/`[Swapped]`
  sections, `0x<hex>~<plugin>` or EditorID keys; STAT/TREE/LTEX swaps) `[doc: xLODGen
  Seasons-Readme]`; parsed by [crates/esp](../../crates/esp/src) `[ours]`. Output names get the
  season before the extension: `Tamriel.4.0.-16.WIN.bto`.
- xLODGen: one full export per season (`LODGen.WIN.txt`, `Season=WIN`, 13,617 lines like the
  default), the atlas map per season, and the "Snow" checkbox: in WIN every base without a
  material gets `Snow`, except (observed rule of the 4.1.5o beta, source.rs `snow_exempt`) bases
  whose EditorID contains `ice` or `frozen`.
- DynDOLOD: `SeasonsCopy=1` "generate default season first, then copy default object LOD *.BTO
  to *[SPR|SUM|AUT|WIN].BTO in case there are no changes"; `SeasonsCombineAtlas=1` "object LOD
  for all seasons uses the same object LOD atlas textures" `[cfg]`. The season export lists the
  quads that changed in `LODCoords<L>=` (`x,y;` pairs; WIN: 43 LOD4, 26 LOD8, 15 LOD16, 0 LOD32;
  SPR: 184 / 67 / 21 / 6) and the LODGen log prints `Copied LOD level 4 coord [x, y]` for every
  other quad (605 lines in the WIN log) `[golden-log]`. Season exports have `CreateHeightMap=False`
  and `AltTextures`/`VC` side files with the season suffix. Our writer computes `LODCoords` by
  diffing the season's reference lines against the default's `[ours: export.rs]`.

### 1.5 Grass cache `[ours]` `[doc]` `[cfg]` `[golden]`

- Grass LOD is generated into the **LOD4** BTOs only, from NGIO's grass cache
  `Data\grass\<World>x<cx>y<cy>.cgid` `[doc: Grass-LOD]`; INI `GrassGID=cgid`, `GrassBillboard=1`
  / `ComplexGrassBillboard=4` (template index into `GrassBillboard1..6`: `Internal`,
  `DynDOLOD_flat_2x2alt1_lod.nif`, `2x2alt2`, `4x2alt1`, `4x2alt2`, `4x2alt3`),
  `GrassBrightnessTop/Bottom R/G/B` (0.400/0.445/0.450, 0.200/0.223/0.225),
  `ComplexGrassBrightnessTop/Bottom` (0.70/0.75/0.75), `GrassGlowMap`, `GrassBacklightMask`,
  `CSGrassOptimizations=0` ("-1 force disable, 0 auto, 1 force enable") `[cfg]`. Density and
  "mode" settings are documented on the Grass-LOD page.
- `.cgid` layout: [crates/grass/src/cgid.rs](../../crates/grass/src/cgid.rs) (ours; the engine's
  `.gid` layout plus NGIO's instance data; see `docs/re/grass-cache-spec.md`). AnvilLOD
  `GrassCache.cs` documents the same layout including the ±49152 coordinate wrap.
- TexGen grass billboards: `textures\terrain\lodgen\<plugin>\<grass>_<formid>.dds` + `_n.dds` +
  `.txt` sidecar; a "complex" grass (one with a normal map) gets `Complex=True`, `Width_1,
  Height_1, Adjust_1, CRC32_1, Width_2, ...` and the files `_1.dds`, `_1_n.dds`, `_2.dds`,
  `_2_n.dds` `[golden: texgen-grass-20261018\textures\terrain\lodgen\seasonal landscapes - solstheim.esp\fern_ash_0000087a.txt]`.
  The side file `LODGen_SSE_Grass_Map_<World>.txt` maps the grass type name to the billboard it
  uses: `Fern_Brown_00000816.dds<TAB>textures\terrain\lodgen\seasonal landscapes.esp\fern_brown_00000816_1.dds`
  (30 lines) `[golden]`.

### 1.6 Terrain and water for clipping `[doc]` `[golden]` `[anvillod]`

- xLODGen readme: "Object LOD generation uses terrain LOD meshes to optimize the object LOD
  meshes by removing any triangles that are below the terrain or water planes." The terrain LOD
  meshes are the `.btr` files of the same worldspace (ours: `crates/btr` output, or the golden
  `seasons-20261017-032143\output\meshes\terrain\Tamriel\*.btr`, see 8.1 `--terrain`).
- DynDOLOD instead gives LODGen `TerrainData=...\LODGen_SSE_LODGen_Terrain_Tamriel.bin`, a binary
  height export whose layout we do not specify here (the only reader we had is quarantined); the
  rewrite takes heights from the LAND records directly. The LODGen log prints the covered cell
  range after reading it (`-60,-64 to 64,48`) `[golden-log]`.
- Full-detail heights: LAND `VHGT` (33×33 vertices per cell, 128 units apart; a float offset,
  then signed byte deltas, row starts relative to the previous row, `×8`), bilinear sampling;
  water: the cell's `XCLW` or the worldspace's default water height `[xedit-src: record
  definitions]` `[anvillod: TerrainHeights.cs, TerrainReader.cs]`.

## 2. Reference handling

### 2.1 Which references get object LOD

**xLODGen dialect** (what the export contains; wbLOD.pas `wbGenerateLODTES5`, object branch,
ported in source.rs) `[xedit-src]`:
- winning REFR records of the worldspace (and its persistent cell) whose base is a STAT or TREE;
- not Initially Disabled (0x800) or Deleted (0x20); with an enable parent (`XESP`) only when the
  REFR is Visible When Distant (0x8000); not Persistent (0x400) when the base is Never Fades
  (STAT 0x4) or the REFR Is Full LOD (0x10000);
- inside the LOD grid (1.3);
- base: STAT with Has Distant LOD (0x8000) or any TREE, with at least one `MNAM` LOD slot whose
  mesh exists (slot 0..2 = LOD4/8/16; names via `mesh_asset_name`, i.e. `Meshes\` prefix);
- material (1.1.3 column 12) from the base's `DNAM` material object EditorID;
- `-LargeRef` for references listed in a master plugin's `RNAM` (large reference data) for the
  cell they are in.

**DynDOLOD dialect**: the mesh-mask and reference rules decide, per base record, which LOD level
gets which model (`Level0..3` = `_lod_0.._3` variants, `Full model`, `Billboard1..6`, empty) and
what happens to the reference (`Unchanged`, `Replace`, `Disable`, `Delete`, `Ignore`, ...). The
rule format, matching order and file selection are documented in
[dyndolod-spec.md "Rules and mesh masks"](dyndolod-spec.md) and
[Mesh-Mask-Reference-Rules](https://dyndolod.info/Help/Mesh-Mask-Reference-Rules) `[doc]`, and
implemented in [crates/dyndolod/src/rules.rs](../../crates/dyndolod/src/rules.rs) `[ours]` and
AnvilLOD `LodRules.cs` / `RuleEntry.cs` `[anvillod]`: lines `LODGen<N>=mask,LOD4,LOD8,LOD16[,LOD32],Grid,Reference,Flags[,Description]`
(7, 8 or 9 columns; a 7-column line has no LOD32), first match wins, reference rules
(`plugin;formid`) beat mesh masks, `\` is the catch-all and stays last, flags bit mask `1 VWD, 2
NoGlow, 4 NoMATO, 8 Dynamic, 16 TREE`. `BaseSignatures=STAT,TREE,ACTI,MSTT,CONT,FURN,DOOR`
`[cfg]`. Our selection code is `assign.rs`, `bases.rs`, `largerefs.rs` `[ours]`.

### 2.2 Transform composition `[anvillod]` `[nif-spec]` `[golden]`

World position of a vertex `v` of a LOD model shape:
`world = T(ref position) · R(ref rotation) · S(ref scale) · (NIF node chain of the shape) · v`,
where the NIF local transform is `v' = R_node · (s_node · v) + t_node` for every node from the
shape up to the root (AnvilLOD `Transforms.Compose`, nif.xml `NiAVObject`).

Reference rotation: the export stores degrees (1.1.3); the REFR stores radians. AnvilLOD
`Transforms.Reference`: `R = Rx(-x) · Ry(-y) · Rz(-z)` with column-vector rotation matrices,
"verified against existing LODGen/DynDOLOD output to within 0.004 units on a reference rotated on
all three axes". Normals, tangents and bitangents are transformed by the rotation part only
(`RotationOnly`). Scale is uniform (`XSCL`).

`IgnoreTranslation` (xLODGen only, 1.1.1): for the listed LOD meshes the reference translation is
not applied `[inferred from the setting name; the 22 vanilla Solitude/Windhelm LOD meshes are
modelled in world coordinates]`; verify on the Solitude quads of the xLODGen golden run.

The stored result is block-local and level-scaled (1.3). Positions are single precision; the
comparison tooling tolerates 0.1-unit rounding (8.1).

### 2.3 Quad assignment and LimitCoords `[golden]` `[anvillod]` `[xedit-src]`

- A reference goes into the quad of its **position** (cell = `floor(pos / 4096)`, origin per
  1.3) at every level; its geometry is not clipped to the quad: shape bounding boxes in
  `Tamriel.4.0.-16.bto` extend past the block (`aabb centre 13654, extent 6143` on a block that
  spans x 0..16384) `[golden]`. AnvilLOD `QuadBucketer.cs` does the same.
- Quads not listed in `LimitCoords<L>` (when the key is present) are not generated; quads with
  no reference produce no file (884 LOD4 entries, 499 files) `[golden]`. In season exports
  `LODCoords<L>` lists the quads to generate and all others are copies of the default season
  (1.4).
- LOD4 quads are 4×4 cells; each shape records per-cell triangle ranges (segments, 3.3).

### 2.4 Large references `[doc]` `[golden]` `[ours]`

`[doc: Large-References]`: in SSE, references listed in a plugin's `RNAM` are "large references"
that the engine keeps loaded in the large-reference grid and whose object LOD must be switchable.
Export: `-LargeRef` in the material column (2.1). Output `[golden]`: the shape name gets the
suffix `-LargeRef` (`obj-LargeRef`, `objSnowHD-LargeRef`, `objpassthru-LargeRef`), and the shape
carries one `BSDistantObjectLargeRefExtraData` block named `DOLRED` with value `1` in its extra
data list (`[nif-spec]`; our reader decodes it as `Block::LargeRefExtra`).
Large-reference and normal geometry of the same material are separate shapes. Our large-ref
detection is [crates/dyndolod/src/largerefs.rs](../../crates/dyndolod/src/largerefs.rs); AnvilLOD
`LargeReferenceStage.cs` / `LargeReferenceRules.cs` (half-diagonal magnitude > `fLargeRefMinSize`
512 as the engine's criterion) `[anvillod]`.

### 2.5 LOD model selection `[xedit-src]` `[doc]` `[cfg]` `[ours]` `[anvillod]`

- xLODGen: the base's `MNAM` slots 0..2, each used only if the file exists (source.rs). No rules.
- DynDOLOD: the rule's per-level choice (2.1) is resolved to a file by LOD model discovery:
  `<model>_lod_<N>.nif` (`_lod_0` = Level0 ... `_lod_3`), `<model>_lod.nif`, passthru/glow
  variants `<model>_<CRC32>passthru_lod.nif`, `<model>glow_lod_<N>.nif` (glow: `_0` LOD4/near
  grid, `_1` LOD8/far grid, `_2` LOD16/never fade `[doc: Glow-LOD]`), DynDOLOD-generated
  `<model>_<CRC32>_dyndolod_lod.nif` `[ours: lodmodels.rs]`; `Configs\DynDOLOD_SSE_mesh_lookup.txt`
  (`full=other` aliases), `mesh_replace.txt`, `misnamed.txt`, `default_lod.txt` /
  `mod_default_lod.txt` (vanilla LOD paths), `IgnoreLODMesh`, `IgnoreFullMesh`, `IgnoreBaseEDID`,
  `ValidRootBlocks=BSFadeNode, BSLeafAnimNode, BSTreeNode`, folder precedence `Folder1..5` `[cfg]`;
  naming rules for mod authors `[doc: Mod-Authors]`. The per-level choice and discovery are
  specified in [dyndolod-spec.md "LOD model selection"](dyndolod-spec.md) and implemented in
  [crates/dyndolod/src/lodmodels.rs](../../crates/dyndolod/src/lodmodels.rs) `[ours]`, which the
  B2 rewrite keeps.
- Open conflict to settle empirically (section 10): AnvilLOD `LodMeshResolver.cs` lets `MNAM`
  win per level where its file exists and fills the rest from discovered names (`_lod_4` maps to
  level 3); our `lodmodels.rs` prefers discovery and uses `MNAM` as fallback, matching DynDOLOD's
  default ("Prefer base record LOD assignments over rules" is an option).
- Rules and masks for the Tamriel Medium golden run: `Rules\DynDOLOD_SSE_all.ini` (negatives such
  as `sky\,,,,,Unchanged,0`), `Rules\DynDOLOD_SSE_medium.ini` (`\rockcliff03,Static LOD4,,,,Unchanged,1`;
  `tree,Billboard,Billboard,Billboard,Near LOD,Unchanged,17`; catch-all
  `\,Static LOD4,Static LOD8,Static LOD16,Far LOD,Unchanged,1`) `[cfg]`.
## 3. Mesh processing, as observable in the output

### 3.1 Which shapes of a LOD model are taken `[nif-spec]` `[doc]` `[anvillod]` `[cfg]`

- Geometry blocks: `BSTriShape` family (`BSTriShape`, `BSSubIndexTriShape`,
  `BSMeshLODTriShape`, `BSDynamicTriShape`) and, in LE-format LOD models that mods and DynDOLOD
  Resources still ship, `NiTriShape` / `NiTriStrips` with their data blocks (our reader decodes
  both into one shape type; strips are converted to triangles) `[nif-spec]` `[ours: nif reader]`.
- Walk from the footer's root(s) through the `NiNode` family (`NiNode`, `BSFadeNode`,
  `BSLeafAnimNode`, `BSTreeNode`, `BSOrderedNode`, `BSMultiBoundNode`, `NiSwitchNode`,
  `NiBillboardNode`, `BSValueNode`), children in order; compose the node transforms (2.2).
  Roots DynDOLOD accepts: `ValidRootBlocks=BSFadeNode, BSLeafAnimNode, BSTreeNode` `[cfg]`.
- Skip: nodes and shapes with the `Hidden` flag (NiAVObject flags bit 0) `[anvillod:
  NifGeometryReader]` `[nif-spec]`; skinned shapes (skin instance set; `[doc: 3D-Tree-LOD-Model]`
  "skinned meshes are not supported") `[anvillod]`; collision objects, controllers, extra data
  other than what 2.4 needs; shapes without triangles. Editor-marker geometry (shapes or nodes
  named `EditorMarker`) must not appear in LOD `[inferred; verify]`.
- Shape-name terms with meaning for 3D tree LOD models, lower-case substring match `[doc:
  3D-Tree-LOD-Model]`: `crown`, `trunk`, `flattrunk` (brightness multipliers, 1.1.2),
  `passthru` (keep shader values), `spherenormals`, `fixednormals`, `noreuv` (skip ReUV),
  `usemipmaps`, `noatlas` (keep own texture), `donothing`, `scalexy`. The template NIFs use
  `PassThru SphereNormals` (6.1).
- Materials: the shape's `BSLightingShaderProperty` (or `BSEffectShaderProperty`) and its
  `BSShaderTextureSet` (9 slots: 0 diffuse, 1 normal, 2 glow/skin, 3 height/parallax, 4
  environment, 5 environment mask, 6 multilayer/inner, 7 specular/backlight, 8 unused). LE
  `NiMaterialProperty` is ignored `[inferred]`.

### 3.2 Vertex attributes `[golden]` `[nif-spec]` `[anvillod]`

Every output shape has positions, normals, tangents, bitangents and one UV set; vertex colours
are present on some shapes and absent on others.

- Observed vertex descriptors `[golden]`: `0x0003b00007650408` on shapes **with** colours
  (attribute flags `0x3B`: Vertex | UVs | Normals | Tangents | Vertex_Colors; 32-byte stride) and
  `0x0001b00000650407` on shapes **without** (`0x1B`; 28-byte stride). nif.xml `BSVertexDesc`:
  bits 0-3 vertex size in 4-byte units, then per-attribute offsets (4 bits each), the attribute
  flags from bit 44 `[nif-spec]`.
- Layout per vertex (AnvilLOD `BtoBuilder`, matches nif.xml `BSVertexData` for this flag set):
  `position f32×3, bitangent.x f32, uv half×2, normal i8×3 + bitangent.y i8, tangent i8×3 +
  bitangent.z i8, [colour u8×4]`; byte encoding `b = round((v + 1) · 127.5)`, decoding
  `v = b / 127.5 - 1` (`Transforms.EncodeByte/DecodeByte`).
- Vertex colours: the LODGen log says "Generate Vertex Colors: True ×4" and the export
  `MergeVertexColors=true`; the observable result is that shapes of the same material exist in
  a coloured and an uncoloured variant in the same file (`objSnow-LargeRef` on
  `dyndolod_tamriel.dds` with `SLSF2 0x25` and 3,928 coloured vertices next to one with `0x5` and
  1,069 uncoloured vertices in `Tamriel.4.0.-16.bto`) `[golden]`, so colour presence is part of
  the grouping key (3.3). What makes a source shape "coloured" is an open question (section 10);
  the VC side file (7.3) marks full-model shapes whose colours are all white as `False`, and
  LODForge's writer documents the intended consumer effect as "the LOD shape with that name is
  treated as white" `[ours: sidefiles.rs]` `[inferred]`.
- Vertex alpha: a lighting shader with `SLSF1 Vertex_Alpha (0x8)` needs its colour alpha; AnvilLOD
  keeps `Model_Space_Normals | Vertex_Alpha` of flags1 and `Double_Sided | Glow_Map` of flags2
  from the source (`ShaderGuard.KeepF1/KeepF2`) `[anvillod]`. `[doc: Snow-Ash-LOD-Shader]`: the
  snow/ash LOD shaders read the snow intensity from vertex alpha (0.4 light, 0.5 heavy on the
  shipped meshes).
- Tangents: "Generate Tangents: True ×4", "Fix Tangents: False ×4" `[golden-log]`: output
  shapes always have tangents and bitangents, even when the source has none. Whether source
  tangents are kept or recomputed is open (section 10); the comparison tolerates bitangent-x
  quantisation (8.1).
- Tree LOD brightness: `Crown/Trunk/FlatTrunkBrightness` multiply the vertex colours of shapes
  whose name contains `crown` / `trunk` / `flattrunk`, result clamped to 0..1 `[doc: Ultra-Tree-LOD]`.

### 3.3 Shape naming, grouping, limits, segments and block layout `[golden]` `[doc]` `[nif-spec]` `[anvillod]`

**Names.** Observed names in the golden BTOs (the NIF string table of a file lists only these
names, no numbering):

- `obj` + material (`Snow`, `Ash`, or the lower-case export material for anything else:
  `objvolcanicsulfurcrystalslite`, xLODGen) + `HD` when the shape counts as HD (3.4);
- `objpassthru` (+ `Snow`) for shapes of passthru LOD models (`PassThruMeshMask`);
- `treepassthru` for FlatTextures tree billboards, `grasspassthru` for grass LOD (section 6);
- suffix `-LargeRef` for large-reference geometry (2.4).

`[doc: Snow-Ash-LOD-Shader]`: the engine picks the LOD material from the name: `objSnow` →
`SnowLODMaterial` (00050372), `objSnowHD` → 0006AE38, `objAsh` → 04032720, `objAshHD` →
04024FE0; `obj` → the default LOD shader.

Observed counts (`bto_stats`; xLODGen = `no-seasons` run, DynDOLOD = all 5 season files of
`dyndolod-golden2` added up, so divide by about 5):

| Level | xLODGen 3.0.36 | DynDOLOD / LODGen 3.1.8 |
|---|---|---|
| LOD4 | `obj` 346, `obj-LargeRef` 676, `objHD-LargeRef` 256, `objSnow` 125, `objSnow-LargeRef` 440, `objSnowHD` 1, `objSnowHD-LargeRef` 307, `objvolcanicsulfurcrystalslite` 4 (+2 LargeRef) | `obj` 2,999, `obj-LargeRef` 3,517, `objHD-LargeRef` 1,948, `objSnow` 875, `objSnow-LargeRef` 2,997, `objSnowHD` 28, `objSnowHD-LargeRef` 1,992, `objAshHD-LargeRef` 25, `objpassthru` 87, `objpassthru-LargeRef` 885, `objpassthruSnow` 12 (+144 LargeRef), `treepassthru` 534 |
| LOD8 | `obj` 500, `objSnow` 290, `objSnowHD` 16, `objvolcanicsulfurcrystalslite` 5 | `obj` 2,336, `objSnow` 1,232, `objSnowHD` 432, `objAsh` 15, `objpassthru` 564, `objpassthruSnow` 104, `treepassthru` 266 |
| LOD16 | `obj` 98, `objSnow` 84, `objvolcanicsulfurcrystalslite` 5 | `obj` 841, `objSnow` 360, `objSnowHD` 120, `objAsh` 15, `objpassthru` 197, `objpassthruSnow` 4, `treepassthru` 106 |

No `-LargeRef` names above LOD4 (large references only matter in the LOD4 grid) `[golden]`.
`objHD` (non-snow HD) occurs at LOD4 only; `objSnowHD` occurs at every level but much less often
above LOD4 (`[doc: Snow-Ash-LOD-Shader]`: HD objects are used only at LOD level 4 by default;
`AllHDLOD32` `[cfg]`).

**Grouping.** All geometry of a quad that shares a material is written as one shape (one draw
call); `DontMergeShapes=False`, log "Merge Meshes: True" `[golden-log]`. The observable grouping
key is: shape name (material class, HD, passthru, LargeRef), the full texture set as written,
the shader block (type and both flag words at least; passthru shapes keep whole source shader
blocks, so those are compared field by field), and vertex-colour presence. Observed: two shapes
never differ only in texture clamp mode (clamp 0 goes with atlas pages, 3 with own textures).
The implementer defines its own key (AnvilLOD `LodMaterial` is a start); the comparison tooling
(8.1) matches shapes by name + textures + flags, so any finer key that produces the same
multiset of triangles per (name, textures, flags) passes.

**Limits.** `BSTriShape` stores `u16` vertex and triangle counts and `u16` indices `[nif-spec]`:
a group is split into several shapes of the same name before either count would exceed 65,535
(`MaxIndex = ushort.MaxValue` `[anvillod]`); observed grass shapes with 65,532 vertices / 32,766
triangles followed by another `grasspassthru` shape `[golden]`.

**Shape order** inside a file is not deterministic in LODGen (thread timing, ~65% of files differ
between two identical runs `[ours: object_lod.rs]`); the rewrite picks any stable order.

**Block layout** `[golden]` `[anvillod: BtoBuilder]` `[nif-spec]`:

```
NiNode "obj"                              flags 14, identity
└─ BSMultiBoundNode (one per shape)      flags 14, culling mode 1, name empty
   ├─ BSSubIndexTriShape "<name>"        flags 14, translation (bx·4096, by·4096, 0), scale L,
   │   │                                 bound centre/radius in local units, segments (below),
   │   │                                 extra data: [BSDistantObjectLargeRefExtraData "DOLRED" 1] for -LargeRef
   │   ├─ BSLightingShaderProperty       (or BSEffectShaderProperty for passthru shapes)
   │   │   └─ BSShaderTextureSet         9 slots, paths lower case with `Data\` prefix
   │   └─ (no NiAlphaProperty, see 3.4)
   └─ BSMultiBound → BSMultiBoundAABB    world-space centre and half extents of the shape
```

Header: `Gamebryo File Format, Version 20.2.0.7`, user version 12, BS version 100, strings =
the shape names and `DOLRED`; a texture set may be shared by several shaders (xLODGen golden:
block 31 used by two shapes) but need not be.

**Segments.** `BSSubIndexTriShape` segment records are `u8 flags (0), u32 start index (into the
index array), u32 triangle count` `[nif-spec]` `[anvillod]`. Observed `[golden]`:
- LOD4 shapes carry up to 16 segments = the 4×4 cells of the quad; empty cells are `(0, 0)` and
  trailing empty cells are omitted (shapes with 9, 13, 14, 15 and 16 entries occur in
  `Tamriel.4.0.-16.bto`); the triangles of a shape are stored grouped by cell so each segment is
  one contiguous range (`start 1590 count 291`, `start 2463 count 211`, ...). Grass shapes at
  LOD4 have a single segment `[0, count]`.
- LOD8 and LOD16 shapes have exactly one segment covering all triangles.
- Cell order inside the 16: AnvilLOD writes `x·4 + y` (x major, south-west cell first) `[anvillod]`;
  the golden order is an open question (section 10, trivially settled by one reference per cell).

### 3.4 Shader values `[golden]` `[nif-spec]` `[doc]`

Observed `BSLightingShaderProperty` fields of ordinary (non-passthru) shapes, identical in
xLODGen and DynDOLOD output:

| Field | Value |
|---|---|
| shader type | 0 (Default) |
| flags 1 | `0x80000300` = `ZBuffer_Test | Receive_Shadows | Cast_Shadows` |
| flags 2 | `0x5` = `ZBuffer_Write | LOD_Objects`; `| 0x20 Vertex_Colors` when the shape has colours; `| 0x10 Double_Sided` on some shapes (xLODGen LOD16 `0x15`); **HD** shapes: `0x80000000 HD_LOD_Objects` **instead of** `LOD_Objects` (`objSnowHD`: `0x80000021` with colours, `0x80000001` without) |
| texture clamp mode | 3 (wrap both) for a shape that keeps its own texture; 0 (clamp both) for a shape on an atlas page |
| glossiness | 1 |
| alpha | 1 |
| specular colour / strength | (1, 1, 1) / 1 |
| emissive colour / multiple | (0, 0, 0) / 1 |
| lighting effects 1 / 2 | 0 / 0 |
| UV offset / scale | 0, 0 / 1, 1 |
| textures | slot 0 diffuse, slot 1 normal (`<diffuse>_n.dds`; `textures\default_n.dds` when the source has none `[anvillod: BakedSlots]`), others empty (a PBR source may keep slot 5: `mountainslab02_rmaos.dds` on an `objSnow-LargeRef` shape) |

Passthru shapes (`objpassthru*`, LOD models matching `PassThruMeshMask`) keep the **source
shader block**: lighting shaders of other types with their own flags and values (water passthru:
type 1 Environment_Map, flags1 `0x82400381`, flags2 `0x8021`, glossiness 200, lighting effects
0.3 / 2, specular (1, 1, 0.91) × 1.2, emissive (0.216, 0.251, 0.251), cubemap in slot 4, mask in
slot 5) or a `BSEffectShaderProperty` copied through (62 in the default-season LOD4 files)
`[golden]`; `[doc: Glow-LOD, 3D-Tree-LOD-Model]`. Billboard and grass shapes have their own
fixed values (6.1, 6.2).

**No `NiAlphaProperty`** on any shape: 0 of the 683 xLODGen files and 0 of the 685 DynDOLOD
default-season files carry one `[golden]`. Alpha-tested textures are placed on the `_Alpha`
atlas page and the shape uses the plain shader; the game's LOD shaders alpha-test at 128
(`[doc: Object-LOD]`: "alpha threshold 128"; log "Default Alpha Threshold: 128, Use Source Alpha
Threshold: False, Alpha DoubleSided: False"). AnvilLOD emits an alpha property for sources with
alpha; do not port that. `Decal` / `Dynamic_Decal` flags are not set (`UseDecalFlag=False`).

**HD** `[golden]` `[doc]` `[inferred]`: HD shapes use full-resolution textures instead of LOD /
atlas textures (`objSnowHD-LargeRef` on `textures\pbr\landscape\mountains\mountainslab01linear.dds`,
`objHD-LargeRef` on `mountainslab01.dds`), get the `HD` name and the `HD_LOD_Objects` flag, and
are mostly LOD4 (3.3). The decision uses the mesh/texture masks of 1.1.2 and `UseHDFlag`; the
exact rule is an open question. In xLODGen the HD texture pair is `TextureDiffuseHD` /
`TextureNormalHD` (the worldspace's `TNAM`).

## 4. Hidden-face removal against terrain and water

Documentation `[doc]` `[cfg]` `[golden-log]`:
- xLODGen readme: "Object LOD generation uses terrain LOD meshes to optimize the object LOD
  meshes by removing any triangles that are below the terrain or water planes."
- DynDOLOD: `RemoveUnseenFaces=True`; `RemoveUnseenZShift=10` "Adjust terrain height for remove
  unseen faces function of LODGen.exe"; log "Remove Faces under Terrain: True", "Remove Faces
  under Water: True", "Remove Faces Z-Shift: 10". xLODGen passes the same options on its command
  line (the FO4 export keys of 1.1.2 name them: `RemoveUnseenFaces`, `IgnoreWater`).

Observed effect `[golden-log]`: per level, "LOD level N total triangles A reduced to B" (numbers
in 8.2); with the BTR terrain of the same level our previous implementation reproduced 99.73% of
the xLODGen triangles, so the rule is a deterministic function of (terrain LOD mesh of the same
level or the LAND heights, water height, z-shift).

Recommended starting point `[anvillod: BtoBuilder.TerrainTest, TerrainHeights.MarginFor]`: a
triangle is dropped only when **all seven** sample points (three corners, three edge midpoints,
centroid) are below `terrain(x, y) - margin`, or all seven below the water surface; a sample
over a cell without terrain data keeps the triangle; margin 64 / 128 / 256 / 512 units for LOD
4 / 8 / 16 / 32; water = the cell's `XCLW` else the worldspace default. Tune the sample set, the
margin (`RemoveUnseenZShift` is 10, so the margins above are AnvilLOD's own safety values, not
LODGen's) and the terrain source against the golden triangle sets (`object-lod --explain`);
quads with cliffs and shorelines (`Tamriel.4.0.-16`, the Solitude coast) show the differences
first. Triangles that straddle the terrain stay (every observed cliff base in the golden files
keeps its partially buried triangles) `[golden]` `[inferred]`.

## 5. UV atlas remap and ReUV

**Remap** (functional operation) `[xedit-src]` `[golden]`:
- For every shape whose texture key (1.2) has an atlas-map row `(w, h, x, y, W, H)` and whose
  UVs are within the tolerance: `u' = (x + u·w) / W`, `v' = (y + v·h) / H`; the texture set
  becomes the row's atlas page in slot 0 and the page's `_n` in slot 1 (lower case, `Data\`
  prefix); clamp mode 0 `[golden]`.
- Tolerance: xLODGen `UVRange` (1.5 default, export `AtlasTolerance = UVRange - 1`); DynDOLOD
  `AtlasTolerance=0.2`. A UV inside `[-tolerance, 1 + tolerance]` is atlasable; the xEdit atlas
  builder only puts a texture on the atlas when every shape using it (lighting shader, UVs
  beyond ±100 ignored) is within range, so after that test the remap applies to every user of
  the texture `[xedit-src]`. Shapes whose texture is not in the map keep it (clamp 3).
- Seasons: DynDOLOD uses one map for all seasons (`SeasonsCombineAtlas=1`, the season exports
  name the unsuffixed map) `[golden]`; xLODGen has one map per season.
- Mini-atlas rows (10 columns) point at TexGen's stitched LOD textures (`textures\lod\<name>.dds`,
  built by `lodforge texgen-stitched` / TexGen from the full textures); the remap is the same
  formula into the smaller page, and the extra `scaleU, scaleV` columns scale the UVs before
  the remap (`1 1` in every golden row) `[golden]` `[inferred]`.
- Glow rows (`diffuse,normal,glow` keys → `_Glow.dds` page) and alpha rows (`_Alpha.dds` page)
  are ordinary rows; a glow-mapped shape keeps its glow map by taking the page's `_g` texture in
  slot 2 `[inferred from the output textures `DynDOLOD_Tamriel_g.dds`; verify]`.

**ReUV** `[cfg]` `[doc]` `[golden]`: DynDOLOD ships a LODGen mode that rewrites a LOD model's
UVs so that a texture tiled beyond 0..1 can use a mini-atlas tile
(`docs\trees.ultra\tools\re-uv\re-uv.txt`: `GameMode=Convert5`, `UseOptimizer=True`,
`DontMergeShapes=True`, `DontFixTangents=False`, `DontGenerateTangents=False`,
`DontGenerateVertexColors=False`, `TextureAtlasMap=re-uv-miniatlas.txt`, `AtlasTolerance=0.2`,
comments `//Ignore all vertices with z <= ScaleXY`, `//RemoveUnseenFaces=True`,
`//ScaleXY=-120.0`; the shipped map is `Cache\DynDOLOD_SSE_re-uv-miniatlas.txt`, 344 rows).
`[doc: 3D-Tree-LOD-Model]`: the shape term `noreuv` exempts a shape; UVs of LOD models should
stay in 0..1 (2×2 billboards up to 2.0). The functional requirement for the rewrite: given a
model and a 10-column map, produce a model whose shapes use the mini-atlas tile with UVs inside
the tile, splitting or wrapping triangles that cross tile boundaries in a way that leaves the
rendered texture unchanged; `MiniAtlasExport` (7.4) lists the models and tiles the golden run
produced (`frozenmarshice02.dds` → `frozenmarshice02pbr_lod.dds`, four 0.5×0.5 tiles), which is
the test case.

## 6. FlatTextures billboards and grass LOD in the LOD4 BTOs

### 6.1 Tree billboards as object LOD `[cfg]` `[doc]` `[golden]`

- When a rule gives a tree `Billboard1..6`, the LOD model column holds the TexGen billboard
  texture and LODGen builds geometry from a template `[doc: Ultra-Tree-LOD, Tree-Grass-LOD-Billboards]`.
  INI `TreeBillboard1..6` = `Internal`, `DynDOLOD_flat_2x2_lod.nif`, `DynDOLOD_flat_3x2_lod.nif`,
  `DynDOLOD_flat_4x2_lod.nif`, `Internal`, `LODGen_flat_lod.nif` `[cfg]`; `TreeLODMaxTilt=30`
  `[cfg]`. The golden Medium run uses `Billboard` (= `Billboard1`, `Internal`) for `tree` masks.
- Templates (shipped, `Edit Scripts\DynDOLOD\`, read with `peek_bto`) `[cfg]`:
  `DynDOLOD_flat_2x2_lod.nif`: `BSFadeNode "Template"` with two `NiTriShape "PassThru
  SphereNormals"` of 4 vertices / 2 triangles each, textures `billboard_1.dds`,
  `billboard_1_n.dds`, glow `billboard_1.dds` (second plane `billboard_2.*`), `NiAlphaProperty`,
  lighting flags1 `0x82400300`, flags2 `0x2000031`; `3x2`: three such planes; `4x2`: four planes
  (flags1 `0x82000300`, flags2 `0x2000021`); `LODGen_flat_lod.nif`: one `NiTriShape "PassThru"`
  with 15 vertices / 16 triangles and textures `billboard.dds` / `billboard_n.dds` / glow
  `billboard.dds`. Template coordinates run 0..1 and are scaled by the billboard's width, depth
  and height; `_1/_2/_3` textures pick the per-plane billboards TexGen renders for 3×2 / 4×2
  `[doc: Ultra-Tree-LOD]`. `Internal` = two planes crossing at 90°, 4 triangles
  `[doc: Tree-Grass-LOD-Billboards]`.
- TexGen sidecar `<billboard>.txt`: `[LOD] Width, Depth, Height, ShiftX, ShiftY, ShiftZ, Scale,
  CRC32, Model, ModelCRC32, Texture_n, TextureCRC32_n` `[golden: texgen-20261017]`; the
  FlatTextures line (1.1.4) carries these plus the INI lighting/colour values.
- Observed output `[golden: Tamriel.4.0.-16.bto]`: one `treepassthru` shape per quad and
  material (534 across the five LOD4 season sets, also present at LOD8/16: 266 / 106) with 8
  vertices / 4 triangles **per tree** (two crossed quads), coloured vertices
  (desc `0x0003b00007650408`), lighting shader type 0, flags1 `0x80000000` (`ZBuffer_Test`
  only), flags2 `0x31` (`ZBuffer_Write | Double_Sided | Vertex_Colors`), glossiness 80, specular
  (0, 0, 0) × 1, emissive (0, 0, 0) × 0, clamp 0, textures slot 0
  `Data\textures\dyndolod\lod\dyndolod_tamriel_alpha.dds`, slot 1 `..._alpha_n.dds`, slot 2
  `..._alpha.dds` again; segments like object shapes. So the billboard textures end up on the
  `_Alpha` atlas page although neither the text atlas map nor its binary `.lst` twin has a
  `terrain\lodgen` row (0 in both; the grass run's `.lst` has 91 `_Alpha` rows against 79, none
  for billboards). Where LODGen learns the billboard tile positions is an open question
  (section 10); for our own pipeline it does not matter: we build the `_Alpha` page ourselves
  and write the billboard and grass rows into our map.
  Vertex colour = `FlatLODVertexColor` (0.55) with `FlatLODColorVariance` (0.1) per tree, normals
  from `flat_normals.txt`, `FlatLODLevelLODFlag=0` so `LOD_Objects` is not set `[cfg]` `[golden]`.

### 6.2 Grass LOD `[doc]` `[cfg]` `[golden]` `[ours]` `[anvillod]`

- Inputs: 1.5 (cache, billboards, Grass_Map, brightness keys). The export of a grass run adds
  the header keys `GrassMap=<...\LODGen_SSE_Grass_Map_Tamriel.txt>`, `GrassBrightnessTopR=0.4`,
  `...TopG=0.445`, `...TopB=0.45`, `GrassBrightnessBottomR=0.2`, `...BottomG=0.223`,
  `...BottomB=0.225`, `GrassLargeReference=False`, `GrassDensity=100`, `GrassGID=cgid`
  `[golden: dyndolod-grass-20261018\export\LODGen_SSE_Export_Tamriel.txt]`, and its
  `LODGen_SSE_FlatTextures.txt` grows to 202 lines with one line per grass billboard
  (`fern_ash_0000087a_1.dds  48.58  43.18  -1.28  1  0 ...`, same 15 columns as 1.1.4). Our
  CLI reads `GrassDensity` and falls back to 50 when the key is absent (`object-lod
  --grass-density`; pass 100 for a DynDOLOD export) `[ours]`.
- `[doc: Grass-LOD]`: grass LOD exists only in LOD4; every grass instance of the cache becomes a
  billboard (two crossed planes for `GrassBillboard=1 Internal`, template files for the others;
  complex grass with a normal map uses `ComplexGrassBillboard=4`, the `4x2alt1` template and
  the `_1/_2` billboard pair), scaled by the cache instance's scale and the sidecar's size,
  coloured by the instance's colour bytes times `GrassBrightnessTop/Bottom` (top/bottom
  vertices), with the mode/density settings subsampling instances. The cache's `GrassControl.ini`
  `DynDOLOD-Grass-Mode` ties the real grass fade to the LOD start `[anvillod: README]`.
- Observed output `[golden: dyndolod-grass-20261018\...\Tamriel.4.0.-16.bto]`: 19
  `grasspassthru` shapes (the quad's grass split at the 65,535-vertex limit; 65,532 / 32,766,
  65,452 / 32,726, ..., 26,444 / 13,222), coloured vertices, one segment `[0, count]` each,
  lighting shader type 0, flags1 `0x80000000`, flags2 `0x21` (`ZBuffer_Write | Vertex_Colors`),
  glossiness 80, specular (0, 0, 0), emissive × 0, clamp 0, slot 0
  `dyndolod_tamriel_alpha.dds`, slot 1 `..._alpha_n.dds`, nothing in slot 2; shape bound radius
  ~767 for a 1-cell group (`centre 511, 511`), so grass is grouped per cell before splitting
  `[inferred]`.
- Our previous implementation reproduced 99.95% of the grass triangles from the golden export
  (8.2), so the placement is deterministic from the cache and the FlatTextures values; the
  comparison treats grass separately (`e2e-compare` "grass" bucket, `object-lod --tolerant`
  ignores `grass*` vertex colours). AnvilLOD's `GrassPatchBuilder.cs` / `GrassLodSource.cs` use a
  different model (bins with one tuft per bin) and are a reference for the cache reading only.
- `CSGrassOptimizations` (Community Shaders) changes the billboard template/mipmaps; keep the
  INI keys (1.5) but leave the behaviour for later (`[cfg]`; our target is CS, see
  `docs/re/own-lod-resources.md` and the LODForge target memory).
## 7. Outputs beyond the BTOs

### 7.1 Object height map: our own format `[golden]` `[ours]`

Purpose: the occlusion stage (`crates/dyndolod/src/occlusion.rs`, being rewritten from
[occlusion-spec.md](occlusion-spec.md)) needs, per exterior cell, the highest object LOD
surface over the cell's terrain grid, so that LOD hidden behind buildings and rocks can be
culled like LOD hidden behind terrain. DynDOLOD's LODGen writes `<World>.objects.heightmap.dat`
next to the BTOs when `CreateHeightMap=True` (INI `LODGenHeightMap=1`, 1.1.2).

We define **our** file as the layout the golden file has, so that the existing comparison keeps
working and the occlusion consumer needs no change:

```
i32 cellCount
cellCount × { i32 cellX; i32 cellY; f32 height[33 * 33] }
```

- Golden `dyndolod-golden2\output\meshes\Terrain\Tamriel\Objects\Tamriel.objects.heightmap.dat`:
  61,641,504 bytes = 4 + 14,125 × (8 + 33·33·4); first cell `(0, 1)`, all its 1,089 values
  `-3.4028235e+38` (`f32::MIN`) `[golden]`. `f32::MIN` means "no object here".
- Sample positions: the LAND vertex grid of the cell, `x = cellX·4096 + i·128`,
  `y = cellY·4096 + j·128`, `i, j = 0..32`. We store row-major with `j` (y) outer and `i` (x)
  inner, i.e. `height[j * 33 + i]` (the same order as `VHGT`). Whether the golden file uses the
  same order is unverified; the e2e reader (`e2e_compare.rs`, `[ours]`) reads 33×33 floats per
  cell without interpreting the order, so a transposed choice would still "compare".
- Value: the highest Z (world units) of any default-season LOD4 object geometry whose triangle
  covers the sample point, after hidden-face removal; cells without any such geometry are
  omitted (14,125 of the 256×256 grid cells are present in the golden file). Which shapes
  contribute (passthru? billboards? grass?) is our choice; grass is excluded (it never occludes).
  Season runs do not write the file (`CreateHeightMap=False`).
- Only the default season has it; the name is `<World>.objects.heightmap.dat` in the `Objects`
  folder `[golden]`.

Agreement with the golden file is a diagnostic, not a gate: the previous implementation reached
94.28% exact and 99.02% within 1 unit (8.2) and the occlusion output was still bit-exact, because
occlusion quantises heights coarsely.

### 7.2 Glow atlas inputs (data, not traversal) `[golden]` `[doc]` `[cfg]`

- Which textures: the golden atlas map has 6 rows targeting `textures\DynDOLOD\LOD\DynDOLOD_Tamriel_Glow.dds`
  (1024×512, 256×256 tiles at x = 0, 256, 512, ...), keyed `diffuse,normal,glow` where the glow
  map is the source shape's slot-2 texture (`riftenwindows02_g.dds`, `swindow01_g.dds`,
  `wrroundwindow_g.dds`); every such source shape belongs to a glow / passthru LOD model (window
  glow, `[doc: Glow-LOD]`: glow LOD meshes keep their own emissive settings; "own emit only in
  LODGen"). A glow-mapped texture whose shape is not atlasable goes to the `_Alpha` page as a
  plain row (`wrroundwindow_g.dds` 64×64 at 2464,1280) `[golden]`.
- What the atlas page needs per row: the diffuse, normal and glow textures at tile size (tile =
  texture size capped to `ObjectLODMaxAtlasSize` and the per-format rules
  `ObjectLODDiffuseFormat=200` / `ObjectLODAlphaDiffuseFormat=225` / `ObjectLODNormalFormat=200`
  `[cfg]`), written as `DynDOLOD_<World>_Glow.dds` plus its `_n` and `_g` companions (observed
  output textures: `DynDOLOD_Tamriel.dds`, `_n.dds`, `_Alpha.dds`, `_Glow.dds`, `_g.dds` in
  `dyndolod-golden2\output\textures\DynDOLOD\LOD\`) `[golden]`.
- Criteria for "this shape has a glow map": lighting shader with `SLSF2 Glow_Map (0x40)` and a
  non-empty slot 2 `[nif-spec]` `[inferred]`. How LODGen walks the models to collect them is not
  specified; the rewrite collects the triples from the LOD models the export names (2.5) in any
  deterministic order.
- `LODGen_SSE_TexturesUsed_<World>.txt` (355 lines, `diffuse,<n>,normal[,glow]` with `<n>` = 8
  for 353 rows, 4 and 2 once each) lists the textures that went on the atlases; the second
  column's meaning is an open question `[golden]`.

### 7.3 Side-file shape ordering: observed format facts `[golden]` `[ours: sidefiles.rs]`

Both files are written by DynDOLOD and read by LODGen; our DynDOLOD-mode writer is
[crates/dyndolod/src/sidefiles.rs](../../crates/dyndolod/src/sidefiles.rs). Re-verified on
2026-10-19 from `dyndolod-golden2\export\` and NIFs extracted from the `LODForge-DynDOLOD`
profile with `lodforge vfs-extract`.

**`LODGen_SSE_VC_<World>[_SEASON].txt`** (5,892 lines):
`<full model path><TAB><shape name><TAB>True|False`, one line per shape of every full model named
in column 13 of the export (PGPatcher `_pgpatcher_dups\N\...` copies are separate models). `True`
= at least one vertex colour is not white. Observed order for `farmhouse02.nif`: `L2_Rope:18`,
`L2_Rope:22` (the two shapes under the `L2_Rope` child node, which is the root's first child),
then the root's own shapes in file order: the depth-first order of the NIF. For
`dirtcliffs01.nif` the lines are `L2_Roots:4, L2_Roots:9, Skirt, DirtCliffs01:0, DirtCliffs01:1`
while the vanilla file lists `Skirt` before the `L2_Roots` node; the run's winning file was a
PGPatcher copy whose block order may differ, so **treat the shape name as the key and the line
order as unspecified**.

**`LODGen_SSE_AltTextures_<World>[_SEASON].txt`** (17,494 lines):
`<ref form id><TAB><entry>[<TAB><entry>...]`, 1-5 entries per line (7,249 lines with one entry,
1,739 with two, 3,228 with three, 3,383 with four, 937 with five), entry =
`<index>=<shape name>=<t0>,<t1>,<t2>,<t3>,<t4>,<t5>,<t6>,<t7>,<t8>,<t9>,` (ten texture fields,
each followed by a comma, lower case, `textures\` prefix; a PBR diffuse with an `_rmaos` map is
written as `<stem>linear.dds`). No negative index occurs. Verified index semantics:

- Entries that name a shape of the reference's **LOD model** (column 14) carry the 0-based
  position of that shape among the LOD model's shapes in file (depth-first) order. Checked on 41
  LOD models that are loose files in `DynDOLOD Resources SE 3`: 71 such entries match exactly,
  e.g. `00013304` → `0=DirtCliffs01:0=...`, `1=DirtCliffs01:1=...` and
  `meshes\lod\dirtcliffs\dirtcliffs01_lod_0.nif` has exactly those two shapes in that order.
- The other 16 entries name a shape of the **full model** and carry the full model's shape
  position (the base's `MODS` alternate-texture `3D index` / `3D name`): `2=MountainTrim01:4` and
  `1=RockSkirt` for `mountaintrim01.nif`, whose shapes are `MountainTrim01_Rocks, RockSkirt,
  MountainTrim01:4, ...`.
- An index may therefore appear with a name that is not the LOD shape at that index:
  `00013AA4` (LOD model `mountaintrim01dirt_lod_2.nif`, shapes `MountainTrim01Dirt_LOD_2:2`,
  `...:3`) has `2=MountainTrim01Wet:2=<pbr riverbededge, with _rmaos in slot 5>`,
  `1=MountainTrim01Dirt_LOD_2:3=<textures\lod\dirt02pbr_lod.dds,...>` and
  `0=MountainTrim01Wet:2=<pbr riverbededge, no slot 5>`. The same name can appear twice with
  different indices and textures.
- The consumer rule (apply by index, by name, or by both; which entry wins) is **not** stated
  here; see section 10. The test case above (`00013AA4`, cell 26,-21, LOD4 quad `(24,-24)`) and
  the texture sets of the shapes in `Tamriel.4.24.-24.bto` settle it.

### 7.4 Other DynDOLOD side files `[golden]`

- `LODGen_SSE_MiniAtlasExport_<World>.txt` (16 data rows in blocks separated by blank lines):
  `0<TAB>1<TAB>1<TAB>1<TAB><full texture><TAB>0.500<TAB>0.500<TAB><u><TAB><v><TAB><absolute target path>\textures\lod\<stem>pbr_lod.dds<TAB>1.000<TAB>1.000`
  (12 columns); the stitched mini-atlas layout handed back to TexGen (section 5). Semantics of
  the first four columns are open.
- `LODGen_SSE_Grass_Map_<World>.txt`: see 1.5.
- `logs\DynDOLOD_SSE_BillboardsUsed_<World>[_SEASON].txt`: billboard list per season (diagnostic).

## 8. Acceptance

### 8.1 Comparison tooling (ours) `[ours]`

Both tools were written by us and are not LODGen-derived; keep them.

- `lodforge object-lod` ([apps/cli/src/object_lod.rs](../../apps/cli/src/object_lod.rs)): builds
  the BTOs of one worldspace from an export file and compares them with a golden folder.
  "LODGen's shape order inside a .bto depends on thread timing (two xLODGen runs on the same input
  differ byte-wise in ~65% of files), so the comparison is order-independent: shapes are matched
  by name, textures and shader flags, and each shape's triangles are compared as a multiset of
  fully attributed vertices." Reported per file: equal up to order / byte-identical / differing,
  and the share of triangles identical over the worldspace. `--tolerant` ("positions on a
  0.1-unit grid and bitangent x quantized like the normal bytes"; in the code it also strips
  `HD` from shape names and ignores the vertex colours of `tree*`/`grass*` shapes, for DynDOLOD
  output where brightness and HD classification differ). `--compare-only <folder>` skips the
  build; `--explain <file>` prints the per-shape comparison of one BTO; `--level`, `--seasons`,
  `--atlas`, `--golden`, `--export`, `--flavor xlodgen|dyndolod`, `--grass`, `--grass-map`,
  `--grass-flat`, `--grass-density`. For xLODGen golden files the terrain must be given
  explicitly: `--terrain D:\Modlists\LODForge-golden\seasons-20261017-032143\output\meshes\terrain\Tamriel`
  (the no-seasons run has the same BTRs). `lodforge object-lod --help` is authoritative; the
  `--dotnet8` switch belongs to the old port and goes away.
- `lodforge e2e-compare` ([apps/cli/src/e2e_compare.rs](../../apps/cli/src/e2e_compare.rs)): a
  whole LODForge build (`--ours`, a Data folder) against a DynDOLOD output (`--golden`) for one
  worldspace: file inventory by bucket, object LOD geometry on a 0.25-unit grid with grass and
  non-grass separated, "near common" centroid matching at 1/4 unit, the object height map (exact
  and within 1 unit), plugins, dynamic LOD, trees and occlusion (TVDT Jaccard). `--tolerant`,
  `--json`. Commands and the acceptance checklist: [docs/validation.md](../validation.md) §2-§3.
- `crates/nif/examples/peek_bto.rs` and `bto_stats.rs` dump one BTO's blocks and the shape-name
  / texture statistics of a folder; `validate_output` (apps/cli/examples) checks assets and
  geometry invariants.

### 8.2 "Before" numbers (the LODGen-derived implementation, to be matched or beaten)

Measured 2026-10-17..19 and recorded in `docs/baseline.md`, `docs/validation-20261019.md` and
`docs/validation-full-20261019.md` (the method sections of baseline.md that describe LODGen's
internals are quarantined; the numbers are measurements of our output and stay citable).

| Comparison | Result |
|---|---|
| xLODGen golden (default season, 683 BTOs, `--terrain` BTRs) | 528/683 files equal up to shape order (174 byte-identical); **99.732%** of triangles identical |
| xLODGen seasons (per season) | 527-544 of 683 equal; 99.75-99.88% |
| xLODGen vs itself (two LODGen runs, same input) | 525/683 equal; 99.796% (the ceiling an exact port can reach) |
| DynDOLOD golden2 from DynDOLOD's own export, `--tolerant` | **98.441%** triangles; 104 files equal; 0 files with differing shape sets |
| E2E from our own export (`build --mode dyndolod`), tolerant | 96.16%; full build geometry LOD4 97.28%, LOD8 94.82%, LOD16 88.16%; grass 1.09% (different grass placement) |
| Grass LOD from the golden export (`dyndolod-grass-20261018`) | 99.952% of triangles (132.72 M of 132.73 M); 2 files differ |
| Object height map | 94.28% of samples exact, 99.02% within 1 unit |
| Per-level triangle totals (from the LODGen logs, "total triangles N reduced to M") | xLODGen: LOD4 3,743,090 → 1,924,335; LOD8 1,541,975 → 898,451; LOD16 456,994 → 321,678. DynDOLOD default: LOD4 9,303,946 → 5,618,521; LOD8 4,720,471 → 2,909,119; LOD16 1,292,544 → 879,755; WIN: 1,392,097 → 894,403 / 1,540,281 → 994,969 / 843,656 → 594,244 |

Timings (release build, 20 threads, Tamriel): 0.44 s build + 0.02 s serialise per season (683
BTOs), 2.8 s for all five seasons; LODGen 3.0.36 needs about 19 s for the same, LODGen 3.1.8
1:07 for the DynDOLOD default season and 0:28 for WIN (`[golden-log]`); grass LOD4 15.2 s +
7.1 s write. The rewrite should stay within 2× of these.

### 8.3 Golden roots `[golden]`

| Root | Content |
|---|---|
| `D:\Modlists\LODForge-golden\no-seasons-20261017-031139\` | xLODGen 3.0.36, Tamriel + 8 small worldspaces, no seasons: `output\meshes\terrain\Tamriel\Objects\*.bto` (683), `LODGen_log.txt`, terrain BTRs |
| `D:\Modlists\LODForge-golden\seasons-20261017-032143\` | xLODGen with seasons: 3,415 BTOs, BTRs; `seasons-delta.json` from `scripts/golden/Compare-Seasons.ps1` |
| `D:\Modlists\skyrim-cs\tools\xLODGen\Edit Scripts\` | the xLODGen exports `LODGen[.SEASON].txt` and atlas maps of those runs (overwritten by later runs; copy before re-running xLODGen) |
| `D:\Modlists\LODForge-golden\dyndolod-golden2\` | DynDOLOD 3 (Medium, seasons Default/WIN/SPR/SUM/AUT, `Level32=0`, `UltraTreeLOD=0`, `DoGrass=0`, large-reference workarounds on; 939 s, 7 GB peak): `export\` (exports, atlas maps, side files), `logs\` (LODGen logs, debug log), `output\meshes\Terrain\Tamriel\Objects\` (3,425 BTOs + heightmap), `output\textures\DynDOLOD\LOD\`, `Tamriel_Underside.nif`, plugins |
| `D:\Modlists\LODForge-golden\dyndolod-grass-20261018\` | same with grass LOD: 685 BTOs + heightmap, grass side files |
| `D:\Modlists\LODForge-golden\texgen-20261017\`, `texgen-grass-20261018\` | TexGen billboards and sidecars (`textures\terrain\lodgen\<plugin>\`) |
| MO2 profiles (`D:\Modlists\skyrim-cs`) | `LODForge-DynDOLOD` (the golden runs' load order), `LODForge-E2E` (end-to-end comparison), `LODForge-Full-Validation-20261019`, `LODForge-MVP-Test`. The brief's `LODForge-Ref-Test` name does not appear in the docs. |

### 8.4 Procedure for the rewrite

1. Build from the **golden exports** first (xLODGen `LODGen.txt` with `--terrain`; DynDOLOD
   `LODGen_SSE_Export_Tamriel.txt` with its side files), compare with `object-lod --golden`.
   This isolates LODGen behaviour from our export writer.
2. Then the full pipeline from the load order (`build`, our export) against `dyndolod-golden2`
   with `e2e-compare --tolerant`.
3. In-game: [docs/validation.md](../validation.md) §4 (matrix: mountains, snow/ash, city
   outskirts, water/glow, trees, grass at near/mid/far; parent/child transitions; large
   references; seasons; save/reload) with the skyrim-mcp harness and a disposable profile
   (`LODForge-E2E`). Dismiss the Survival Mode prompt through `game_menu`, keep the player under
   the camera, compare matched weather.
4. Acceptance for B2 is "no regression against 8.2 on the same inputs", with each remaining
   difference explained with `--explain`, and the in-game matrix passed. Byte-identity is not a
   goal (LODGen itself is not byte-stable).

## 9. Process record

### 9.1 What was read (2026-10-19) and how it was used

- Allowed sources, used for the text: wbLOD.pas (export writer, atlas builder, defaults);
  `crates/objlod/src/source.rs`, `atlas_build.rs`; `crates/dyndolod/src/{rules,lodmodels,assign,bases,largerefs,export,sidefiles,childworld,ini}.rs`;
  `crates/nif/src/lib.rs` (format doc and reader; the LODGen default constants in it were noted
  only to exclude them); `crates/grass/src/cgid.rs`; `apps/cli/src/{object_lod,e2e_compare}.rs`;
  `apps/cli/src/dyndolod_build.rs` and `glow_atlas.rs` only to name their LODGen-mirroring
  functions (9.2); `crates/nif/examples/{peek_bto,bto_stats}.rs`; AnvilLOD files listed in 9.5;
  the dyndolod.info pages and xLODGen readmes listed in the tag table; the shipped INI, rules,
  configs, template NIFs and re-uv files; the golden roots of 8.3 (exports, side files, atlas
  maps, BTOs, logs, heightmap, TexGen sidecars); `docs/licensing.md`,
  `docs/reimplementation-plan.md`, `docs/re/dyndolod-spec.md` (non-`[lodgen-src]` sections),
  `docs/re/occlusion-spec.md` (format conventions), `docs/validation*.md`.
- Read **only to decide what to exclude** (content deliberately not reproduced here):
  `crates/objlod/src/{lib,export,build,shape,reuv,atlas,flat,grass,geometry,terrain,objheight,math}.rs`;
  `docs/re/lodgen-318-delta.md` (now quarantined); `docs/re/gap-audit-20261019.md`;
  `docs/baseline.md` sections listed in 9.4; `docs/parity.md`; the `[lodgen-src]` parts of
  `docs/re/dyndolod-spec.md`. The LODGen decompiles under `D:\Modlists\ref\lodgen-*` were not
  opened in this pass.
- Not used: `docs/re/gap-audit-20261019-ini-keys.md` (two decompile references; to be checked by
  the session that quarantines the gap audit).

### 9.2 Delete-unread list for B2 implementers

Sessions doing stage B2 must not open these (delete or move them out of the checkout before
starting; `git rm` them in the rewrite branch):

- `crates/objlod/src/`: `lib.rs`, `export.rs`, `build.rs`, `shape.rs`, `reuv.rs`, `atlas.rs`,
  `flat.rs`, `grass.rs`, `geometry.rs`, `terrain.rs`, `objheight.rs`, `math.rs` (everything
  except `delaunay.rs`, `atlas_build.rs`, `source.rs`).
- `apps/cli/src/dyndolod_build.rs`, the LODGen-mirroring functions: `atlas` (atlas planning in
  "LODGen's walk order"), `flat_textures`, `iclod_line`, `grass_header`, `complex_grass_flat`,
  `terrain_bin` (writes the `TerrainData` `.bin`, a LODGen-private format), `grass_lod1_nif`.
  The rest of the file (profile, export, plugins, occlusion driver) is ours.
- `apps/cli/src/glow_atlas.rs`: `model_shapes` (model walk and shape indexing), `diffuse_key`,
  `is_glow_key`, `glow_layout`, `drop_miniatlas`, `merge`, `tile_size` and the constants
  `GLOW_TILE`, `SHADER_GLOW`, `SLSF2_GLOW_MAP`. (`is_passthru_model`, `load_sized` are generic.)
- `crates/dyndolod/src/occlusion.rs` (`read_terrain_bin` and the TVDT writer; owned by the
  occlusion rewrite).
- `crates/btr`, `crates/nif/src/writer.rs`, `crates/nif/src/btr.rs`, and the LODGen-default
  constants in `crates/nif/src/lib.rs` (vertex descriptors and shader defaults named `lodgen_*`
  / "LODGen default"): per `docs/licensing.md`. B2.2 therefore needs its own BTO serialiser
  (port AnvilLOD `NifWriter.cs` + `BtoBuilder.cs`, or a cleared `writer.rs` from the terrain
  stage B1, whichever lands first).
- `crates/texgen/src/billboard.wgsl` (TexGen stage, listed for completeness).
- Documents: `docs/re/lodgen-318-delta.md` (pointer only now), `docs/re/gap-audit-20261019.md`,
  `docs/parity.md`, the `[lodgen-src]` sections of `docs/re/dyndolod-spec.md`, the baseline.md
  sections in 9.4, and everything under `D:\Modlists\ref\lodgen-*`, `D:\Modlists\ref\net10-numerics`,
  `D:\Modlists\ref\skyrimse-*`, REA snapshots and `D:\Modlists\ref\quarantine\`.

### 9.3 Quarantine files written in this pass (outside the repository, not for implementers)

- `D:\Modlists\ref\quarantine\object-lod-lodgen-delta.md`: verbatim copy of the former
  `docs/re/lodgen-318-delta.md` (638 lines, working copy of 2026-10-19) under a header; the repo
  file now holds a dated pointer.
- `D:\Modlists\ref\quarantine\object-lod-lodgen-notes.md`: verbatim copies of
  `docs/baseline.md` lines 446-552 ("LODForge P3: object LOD meshes", "Object LOD export + atlas
  from the load order"), 615-1032 ("P6: DynDOLOD + TexGen golden runs", "P7: closing the DynDOLOD
  gaps" steps 1-7), 1854-2193 ("AltTextures and VC side files", "Mini-atlas rows in the object
  atlas map", "Small output fixes", "LODGen port fixes P4, G3, G9, G10, G11", "MiniAtlasExport (P3)
  and the `noalpha` replacement"), `docs/parity.md` (whole) and `docs/re/gap-audit-20261019.md`
  (whole).

### 9.4 Repository sections that must later be replaced by pointers (not edited in this pass)

| File | Section (line numbers of the 2026-10-19 working copy) | Replacement |
|---|---|---|
| `docs/baseline.md` | 446-552 "LODForge P3: object LOD meshes (.bto)" + "Object LOD export + atlas from the load order" | keep the measurement tables, point the method text at the quarantine |
| `docs/baseline.md` | 615-1032 "P6" + "P7" (steps 1-7) | same |
| `docs/baseline.md` | 1854-2193 "AltTextures and VC side files", "Mini-atlas rows", "Small output fixes", "LODGen port fixes", "MiniAtlasExport" | same |
| `docs/parity.md` | whole file | pointer |
| `docs/re/gap-audit-20261019.md` | sections 1, 2, 4 (decompile addresses); the rest is status | pointer + status-only summary |
| `docs/re/gap-audit-20261019-ini-keys.md` | to be checked (2 decompile references) | |
| `docs/re/dyndolod-spec.md` | "Side-file formats [golden] [lodgen-src]" (644-745), "How LODGen consumes a billboard [lodgen-src ...]" (1254-1273) and the `[lodgen-src]` bullets near 1388-1413 and 1630; "What LODGen does differently for DynDOLOD" (798-815) | replace with pointers to this spec (7.3, 6.1) and the quarantine; another session is editing this file |
| `crates/dyndolod/src/sidefiles.rs` doc comments | the sentences that describe how LODGen walks a model and indexes its shapes (they name LODGen internals) | reword to the observed format fact of 7.3 (code stays) |

### 9.5 AnvilLOD files recommended for porting, per sub-stage `[anvillod]`

Port with attribution (GPL-3.0-only, CageTV); adapt, do not transcribe where the golden output
says otherwise (noted).

| Sub-stage | Scope | AnvilLOD files | Notes |
|---|---|---|---|
| B2.1 | export + atlas-map parsing, `.lod`, quads, transforms, LOD mesh resolution, rules, seasons, child copies, large refs | `World\LodSettings.cs`, `World\LodGrid.cs`, `World\CellCoord.cs`, `Lod\LodLevel.cs`, `Lod\LodReference.cs`, `Lod\QuadBucketer.cs`, `Meshes\Transforms.cs`, `Lod\LodMeshResolver.cs`, `Lod\LodMeshIndex.cs`, `Lod\LodRules.cs`, `Lod\RuleEntry.cs`, `Lod\ChildWorldCopies.cs`, `Lod\SeasonSwaps.cs`, `Lod\SeasonalLod.cs`, `Lod\LargeReferenceStage.cs`, `Lod\LargeReferenceRules.cs`, `ScanPipeline.cs` | our `crates/dyndolod` already covers rules/resolution/large refs/child copies; port only what is missing. AnvilLOD has no export-file parser (it reads plugins directly): write the parser from 1.1 |
| B2.2 | NIF geometry import, material key, merging, segments, shape naming, BTO serialiser | `Meshes\Nif\NifGeometryReader.cs`, `Meshes\Nif\NifWriter.cs`, `Meshes\BtoBuilder.cs`, `Meshes\LodMesh.cs`, `Meshes\MeshCleaner.cs`, `Meshes\ShaderGuard.cs`, `Meshes\LodInset.cs`, `Lod\LodLayers.cs` | do **not** port the `NiAlphaProperty` emission (3.4: golden BTOs have none) nor the "HD at LOD4 only" rule (3.4); use our `crates/nif` reader. `ShaderGuard` reduces flags to `0x80000300 | keep`, `0x5 | keep` which matches 3.4 |
| B2.3 | hidden-face removal | `Meshes\BtoBuilder.cs` (`TerrainTest`), `World\TerrainHeights.cs`, `World\TerrainReader.cs` | starting point; tune per section 4 |
| B2.4 | atlas remap, ReUV | none (AnvilLOD has no object atlas yet, README "Object texture atlasing ... still to come") | from 1.2, 5 and atlas_build.rs |
| B2.5 | FlatTextures billboards, grass LOD, VC rules, HD/passthru/effect | `Lod\GrassLodSource.cs`, `Lod\GrassPatchBuilder.cs`, `Lod\GrassCache.cs`, `Lod\TreeLodGenerator.cs` (billboard geometry), `Lod\TreeAtlasBuilder.cs` | AnvilLOD's grass model differs (bins, one tuft per bin); DynDOLOD places every cache instance (6.2). FlatTextures-as-DynDOLOD, HD/passthru/effect are not in AnvilLOD |
| B2.6 | height map, glow atlas | none | from 7.1, 7.2 |

### 9.6 Sub-stage checklist (done criteria)

- B2.1: parses both golden exports (every header key of 1.1.1/1.1.2, 13,617 and 34,526 lines),
  both atlas maps (8/10 columns), FlatTextures (export lines and side file), `.lod`; assigns every
  reference to the right quad (file set equals the golden set: 683 / 685 names per season);
  composes transforms so that a LOD model placed by a reference rotated on all three axes lands
  within 0.1 units of the golden geometry (`--explain`).
- B2.2: BTOs load in NifSkope and in our reader; block layout, names, flags, segments and vertex
  layouts of 3.3-3.4; xLODGen golden comparison ≥ 95% without hidden-face removal disabled on
  both sides is not possible, so compare with removal off against the "total triangles" counts
  of the logs (8.2) and shape sets.
- B2.3: xLODGen ≥ 99.7% triangles identical, 525+ files equal up to order; DynDOLOD ≥ 98.4%
  tolerant.
- B2.4: atlas-textured shapes carry atlas page + `_n`, clamp 0, UVs inside the tile; mini-atlas
  rows remap into `textures\lod\*.dds`; ReUV reproduces the `MiniAtlasExport` test models.
- B2.5: `treepassthru` shapes (8 vertices / 4 triangles per tree, flags of 3.4) and
  `grasspassthru` shapes (≤ 65,535 vertices each, single segment) match the grass golden to ≥
  99.9%; snow/ash/HD/passthru names and effect-shader copies as 3.3-3.4; VC `False` shapes
  white.
- B2.6: `<World>.objects.heightmap.dat` in the 7.1 layout, ≥ 99% within 1 unit of the golden;
  glow atlas rows and page as 7.2; occlusion stage consumes it unchanged.

## 10. Open questions to settle empirically (and where to look)

1. **Hidden-face rule** (B2.3): sample points per triangle, margin / `RemoveUnseenZShift`
   semantics, terrain source (level-matched BTR vs LAND), water test, whether partially buried
   triangles are kept. Look: `object-lod --golden --explain` on quads with cliffs and shores,
   e.g. `Tamriel.4.0.-16.bto`; log totals in 8.2.
2. **Segment cell order** at LOD4 (`x*4+y` as AnvilLOD, or `y*4+x`): put one reference per cell
   and read the segment table of the golden quad.
3. **Merge key**: whether clamp mode, specular/glossiness, double-sidedness or the source shader
   type split shapes beyond name + textures + flags1/2 + colour presence; whether shapes of the
   same key are split by source model (observed: `objSnow-LargeRef` with and without colours
   coexist, `obj` shapes with the same atlas texture are merged across models).
4. **Vertex colour rules**: `MergeVertexColors`, "Generate Vertex Colors", VC `False`
   treatment, `Vertex_Alpha` retention, tree/grass brightness multipliers (3.2, 6.1, 6.2).
5. **HD classification**: how `IsHD*`/`NotHD*` masks and `UseHDFlag` decide `HD` names and the
   `HD_LOD_Objects` flag at each level (3.4; counts in 3.3).
6. **AltTextures consumer rule** (7.3): index vs name; the `00013AA4` case.
7. **IgnoreTranslation** (xLODGen): verify the Solitude/Windhelm quads (2.2).
8. **MNAM vs discovery precedence** (2.5), including `Prefer base record LOD assignments`.
9. **ReUV**: how tiled UVs beyond the tolerance are handled (clamped, wrapped, triangles split at
   tile edges) and the meaning of scale U/V and of the MiniAtlasExport columns (section 5, 7.4).
10. **Billboard details** (6.1): glow map slot (`textures\white.dds` in the line vs the alpha
    atlas in the golden shape), vertex colour variance distribution, `Internal` template
    geometry (plane orientation, UV layout, 8 vertices), `TreeLODMaxTilt`, and where the
    billboard tiles on the `_Alpha` page come from (no atlas-map row names them).
11. **Grass** (6.2): instance → billboard geometry (size from the sidecar × cache scale, colour
    from the cache's colour bytes × brightness top/bottom, density/mode subsampling), tangent
    frames, why the previous port reached 99.95% (its notes are quarantined).
12. **Export keys with unknown semantics**: `UseOptimizer`, `Experimental`, `MergeVertexColors`,
    `Threads`/`ThreadSplit` beyond performance, `TexturesUsed` column 2.
13. **Height map** (7.1): which shapes contribute and the sample order; only diagnostic.
14. **Normals/tangents**: whether LODGen recomputes them ("Generate Tangents: True") or keeps
    the model's; compare normal bytes of a flat-shaded model.

## 11. Process record of the implementation, stages B2.1 and B2.2 (2026-10-20)

Implementer session started from the orchestrating session in its own worktree (branch
`objlod-rewrite`, cut from `main` at 7cac5ed, which already held the B1 terrain rewrite and the
occlusion re-implementation). Nothing merged, nothing pushed. Commits, oldest first: 6c5c9b6
(delete unread + CLI stubs), 557027c (crate B2.1 + B2.2), 282cc5a (CLI wiring, diagnostics,
licensing note), then the fixes settled on the golden files (IgnoreTranslation, vertex-colour
merging, tolerant comparison) and this record.

### 11.1 First action: the delete-unread list (9.2)

`git rm` of `crates/objlod/src/{lib,export,build,shape,reuv,atlas,flat,grass,geometry,terrain,
objheight,math}.rs`, `crates/objlod/tests/dotnet_numerics.rs`, `crates/objlod/tests/data/`
and `crates/objlod/examples/numerics_probe.rs` (the .NET-numerics test, data and probe of the
deleted `math.rs`) without opening any of them. The LODGen-mirroring functions of
`apps/cli/src/dyndolod_build.rs` (`atlas`, `flat_textures`, `iclod_line`, `grass_header`,
`complex_grass_flat`, `terrain_bin`, `grass_lod1_nif`) and of `apps/cli/src/glow_atlas.rs`
(`model_shapes`, `diffuse_key`, `is_glow_key`, `glow_layout`, `drop_miniatlas`, `merge`,
`tile_size`, `GLOW_TILE`, `SHADER_GLOW`, `SLSF2_GLOW_MAP`) had their bodies replaced by
`todo!()` stubs with a brace-matching script that printed the file with those bodies elided;
only the signatures and the surrounding (our own) code were seen. Four of the stubs were then
rewritten from public facts: `iclod_line` from the golden export's own line (1.1.3),
`terrain_bin` over `lodforge_btr::export` + `terrain_bin::write_cells` (the dump format of
terrain-lod-spec 2.5), `is_glow_key` from the atlas-map key format (1.2 / 7.2, three
comma-separated paths); `flat_textures` returns an empty list with a warning and `atlas`
returns an error (both B2.4-B2.6), so `lodforge build --mode dyndolod` stops at its atlas step
with a clear message instead of panicking; `grass_header`, `complex_grass_flat`,
`grass_lod1_nif` and the glow-atlas helpers stay `todo!()` (B2.5 / B2.6).

### 11.2 Inputs read

This spec (sections 1-3, 8, 9.2, 9.5, 10 in full; 4-7 for the stubs' scope);
`docs/licensing.md`, `docs/reimplementation-plan.md`, `docs/re/terrain-lod-spec.md` section 8;
the sibling sessions' hand-over notes. Our code: `crates/objlod/src/{source,atlas_build,
delaunay}.rs` (kept), `crates/nif/src/lib.rs` (data model, reader entry points) and
`writer.rs` (the B1 writer's API and doc header), `crates/btr/src/{lib,bounds,terrain_bin,
export,input,settings}.rs` (API; `decode_cell` verified to store `heights[col][row]` =
`[x][y]`), `crates/esp` (API grep), `crates/dyndolod/src/{lib,export,lodmodels,assign,bases,
largerefs,sidefiles}.rs` (API; `export.rs` header / line writer in full), `crates/grass/src/
cgid.rs` (types), `crates/seasons` and `crates/archive` APIs, the CLI files `main.rs`,
`object_lod.rs`, `object_export.rs`, `build.rs` (object stage), `profile.rs` (args),
`texgen_billboards.rs` (grass loop), `dyndolod_build.rs` and `glow_atlas.rs` with the listed
bodies elided, `crates/nif/examples/{peek_bto,bto_stats}.rs`. AnvilLOD (`D:\Modlists\ref\
AnvilLOD`, 606eece): `Meshes/Transforms.cs`, `Core/World/{QuadBucketer,LodGrid,LodSettings,
CellCoord,LodLevel,LodReference}.cs`, `Core/Lod/{LodLayers,LodMeshResolver,LodMeshIndex}.cs`,
`Meshes/{ShaderGuard,MeshCleaner,LodMesh,BtoBuilder}.cs`, `Meshes/Nif/{NifGeometryReader,
NifWriter}.cs`, `Meshes/LodGenerator.cs` (skimmed), `LICENSE`. Golden material: the xLODGen
export `LODGen.txt` and `SSELODGen-AtlasMapTamriel.txt`, the DynDOLOD golden2 export, atlas
map and side-file names, `no-seasons-20261017-031139\LODGen_log.txt` (option block and
triangle totals), the golden BTOs of both tools through `peek_bto`, `bto_stats` and the new
diagnostics, six LOD models extracted from the `LODForge-DynDOLOD` profile for their colour
statistics.

### 11.3 Never opened

The deleted modules in any revision, `D:\Modlists\ref\lodgen-*`, `ref\net10-numerics`,
`ref\skyrimse-*`, `ref\quarantine\`, `tools\rea-snapshots`, `docs/re/lodgen-318-delta.md`,
`docs/re/gap-audit-20261019*.md`, `docs/parity.md`, `docs/re/dyndolod-spec.md`,
`docs/baseline.md`, `docs/re/grass-cache-spec.md`, `crates/dyndolod/src/occlusion*`, the
history of `crates/texgen/src/billboard.wgsl`. No web search for LODGen sources or decompiles.
The project memory's LODGen-internal sentences were not used.

### 11.4 AnvilLOD code ported (GPL-3.0-only, Copyright (c) 2026 CageTV, commit 606eece)

Each file names its source and marks the modifications:

| Our file | AnvilLOD source | What was taken |
|---|---|---|
| `transform.rs` | `Meshes/Transforms.cs` | reference rotation `Rx(-x) Ry(-y) Rz(-z)`, NIF `v' = R(s v) + t`, rotation-only for normals, byte encoding; rewritten for column vectors |
| `grid.rs` | `Core/World/LodSettings.cs`, `LodGrid.cs` | `.lod` layout, block origin, inside test (stride optional) |
| `refs.rs` | `Core/World/QuadBucketer.cs` | one bucket per (level, block) for every level with a mesh; plus `LimitCoords` / `LODCoords` |
| `mesh.rs` | `Meshes/Nif/NifGeometryReader.cs` | node walk, skips, bake by full transform / rotation part, face normals, tangents from UVs; decoded by our reader, effect shaders kept, no fake water, no alpha |
| `material.rs` | `Meshes/ShaderGuard.cs` | flag reduction `0x80000300 | keep`, `0x5 | keep`, narrowed to `Double_Sided` |
| `build.rs` | `Meshes/BtoBuilder.cs` | chunk accumulation, 65,535 splits, per-cell index lists, trailing-empty segment trim; not its HD rule, alpha property, terrain test, inset |

Read but not ported: `MeshCleaner.cs` (no vertex dedupe yet), `LodMeshResolver.cs` /
`LodMeshIndex.cs` (our `lodmodels.rs` stays; the export already resolved the models),
`NifWriter.cs` (the B1 writer is used), `LodGenerator.cs`, the small record types. The
`THIRD-PARTY-NOTICES` entry is for the license-mechanics stage.

### 11.5 Modules written

`crates/objlod/src/{lib,export,atlas_map,grid,transform,refs,flat,sidefiles,terrain,stats,
mesh,material,build,bto}.rs` (all new text; `source.rs` adapted to the new `StaticRef`,
`Cargo.toml` license `GPL-3.0-only` with the MPL-2.0 files noted); `apps/cli/src/object_lod.rs`
rewritten on the crate (`--flavor auto|xlodgen|dyndolod`, `--geometry-only`, `--dotnet8` gone),
`build.rs` / `e2e_compare.rs` / `texgen_billboards.rs` adapted (the grass cache is read with
`lodforge_grass::cgid`); diagnostics `crates/nif/examples/{bto_flags,bto_segments,bto_vertex,
bto_attr_diff,nif_colors}.rs`; `docs/licensing.md` carries a status note on the rewritten
no-read items.

### 11.6 Decisions on the open questions (section 10)

- **2. Segment order**: `x * 4 + y` (x major), read off `Tamriel.4.0.-16.bto` with
  `bto_segments` (segment 12 holds cell (3,0), 9 holds (2,1)); offsets clamped to 0..3 for the
  geometry that straddles the block.
- **3. Merge key**: name + the nine written texture slots + every lighting / effect shader value
  (passthru shapes keep whole blocks) with the colour decision made per merged shape (below).
  No golden xLODGen file has two shapes with the same name + flags + clamp + textures + colour
  presence (`bto_flags`), so nothing finer splits shapes; DynDOLOD's repeats are passthru
  shapes whose other shader fields differ.
- **4. Vertex colours (partial)**: a colour array that is white in RGB on every vertex counts
  as no colours (`mountaintrimslab_lod_2.nif`, all 255, is uncoloured in the golden LOD8
  file; the same test as the DynDOLOD VC side file, LE data always counts); a merged shape is
  coloured when any part has non-white colours and the other vertices get opaque white (the
  golden `objSnowHD-LargeRef` of `Tamriel.4.0.-16.bto` holds white vertices of colourless
  parts). Not reproduced: LODGen 3.0.36 writes atlased shapes above LOD4 without colours
  (the Nordic-ruin stair trims are coloured at LOD4 and uncoloured at LOD8/16 in the xLODGen
  golden, 110 of our shapes), while DynDOLOD's LODGen 3.1.8 keeps them (76 coloured `obj`
  atlas shapes at LOD8 in golden2); we keep colours at every level. `MergeVertexColors`,
  VC `False` whitening and the tree / grass brightness are B2.5.
- **5. HD**: HD texture and (snow / ash material, or a large reference at LOD4), where the HD
  texture is the model's own slot 0 (before AltTextures) equal to `TextureDiffuseHD` (xLODGen)
  or containing an `IsHDTextureMask` entry (DynDOLOD); only snow / ash HD shapes carry
  `HD_LOD_Objects` (plain `objHD` keeps `LOD_Objects`); `-LargeRef` names and `DOLRED` only
  at LOD4. Fits the golden statistics except 2 `obj-LargeRef` + 2 `objHD-LargeRef` (no
  colours) shapes of ~700 at LOD4 and 2 `objHD-LargeRef` on `snow01noalphalinear.dds` in
  golden2. `NotHDTextureMask` and the mesh masks are parsed but not used.
- **7. IgnoreTranslation**: the listed mesh's own node and shape transforms are ignored
  (vertices as stored) and the reference transform applies; dropping the reference
  translation or transform puts the Solitude meshes at the world origin. Verified on
  `Tamriel.4.-20.24.bto` (uncoloured `obj-LargeRef`: 755 of 785 golden triangles).
- **14. Normals / tangents**: taken from the model and rotated, not regenerated: on own-texture
  shapes 99.76% of the normals and 96.98% of the tangents are within one byte of the golden
  (the rest is the dropped .NET-exact arithmetic); tangents generated for sources without
  them (from the UV layout, AnvilLOD's method) agree less (86.6% when atlas shapes are
  included).
- **6. AltTextures**: implemented as "entry whose index and name match the LOD shape, else the
  entry whose name matches"; not verified (no DynDOLOD run in this stage; the `00013AA4`
  case is still the test).
- **1, 9, 10, 11, 12, 13**: untouched (B2.3-B2.6). `Level32`, `UseHDFlag`, `PassThruMeshMask`,
  `RemoveUnseenFaces` are read from the DynDOLOD header; the xLODGen dialect takes the
  documented defaults (HD on, no passthru masks, no LOD32).
- Other layout facts fixed on the golden files: shape order sorted by key; bound radius = half
  diagonal of the local box; AABB = the plain world box; texture sets shared between shapes
  with the same nine slots; shader block before its texture set; string table in first-use
  order.

### 11.7 Numbers (Tamriel, the golden xLODGen export, default season, release build)

Build: 13,617 references, 35,080 placements, 683 BTOs (the golden set); models + buckets 12 ms,
build 55 ms, serialize + write 314 ms, 589 ms total (the LODGen-derived port: 0.44 s + 0.02 s).

Triangle totals before removal versus the golden log ("total triangles"): LOD4 3,737,782 vs
3,743,082 (-0.14%), LOD8 1,538,907 vs 1,541,975 (-0.20%), LOD16 453,873 vs 456,911 (-0.66%);
no source shape was skipped, so the residue is not explained (strip conversion is a guess).

Golden comparison (`object-lod --golden`, 683 files, golden 3,144,458 triangles, ours
5,730,562 = 1.82x because nothing is removed yet):

| Mode | Triangles identical | Files equal | Shape sets differ | Directions within 1 byte |
|---|---|---|---|---|
| exact (as 8.2) | 3 (0.000%) | 0 | 592 | - |
| `--tolerant` (0.1 grid, UVs, colours, no direction bytes) | 1,477,154 (46.98% of golden) | 0 | 592 | normals 99.76%, tangents 96.98% (925,590 paired vertices) |
| `--geometry-only` (exact positions, no UVs / textures) | 443 (0.014%) | 0 | 157 | - |
| `--geometry-only --tolerant` | 2,880,274 (**91.60%** of golden, 50.26% of ours) | 0 | 155 | normals 97.18%, tangents 86.57% (1,841,765 paired) |

Per quad (tolerant, geometry only): `Tamriel.4.0.-16.bto` coloured `obj-LargeRef` 4,979 of
5,061, `objSnow-LargeRef` 1,024 of 1,032, `obj` 340 of 345; LOD8 `obj` 6,067 of 6,332.

Where the gap is: the exact metric of 8.2 (99.73%) cannot be reached without the .NET-exact
arithmetic the rewrite deliberately dropped (one-byte normal / tangent differences on about half
the vertices), so the B2 baseline is the tolerant geometry-only figure. Of the rest: (a) the
missing UV atlas remap (B2.4) costs 44.6 points (46.98% with UVs and textures against 91.60%
without), and it also merges shapes that differ only by source texture into one atlas-page shape
in the golden; (b) the missing hidden-face removal (B2.3) is why we write 1.82x the golden
triangles (50% "of ours"); (c) the 8.4% of golden triangles not reproduced are the 155 files with
differing shape sets: the LOD8/16 colour quirk of 3.0.36 (110 shapes), colour / merge
differences at atlas seams, and positions beyond the 0.1 grid.

Tests: `cargo test --workspace` green (objlod 16 unit tests; the glow-layout test is ignored
until B2.6). In-game check not done: the output is incomplete without B2.3 (buried faces) and
B2.4 (atlas UVs). `lodforge build` was not run end to end in this session; its xLODGen-mode
object stage calls the same `run_with`, its DynDOLOD-mode object stage stops at the atlas stub.

### 11.8 Not done (for the next sessions, in order)

- **B2.3** hidden-face removal: `terrain.rs` has the LAND / dump heights with bilinear sampling
  and water per cell (`Sampler::height_at`, `water_at`); the triangle test and the margin /
  z-shift / terrain-source choices of section 4 are open; `--terrain` is accepted and ignored.
- **B2.4** atlas remap + ReUV: `AtlasMap::lookup` / `AtlasRow::remap` exist; the tolerance test,
  clamp 0 and the page textures in the set are not applied; `source.rs` / `atlas_build.rs`
  give the xLODGen-side rules. The DynDOLOD atlas planning (`dyndolod_build::atlas`, the
  glow-atlas helpers) is stubbed.
- **B2.5** billboards (`flat.rs` parses both layouts; `.dds` model columns are counted and
  skipped), grass, VC `False` whitening, brightness, the LOD8/16 colour choice, passthru checks
  against golden2 (`dyndolod_build::flat_textures` returns nothing, `grass_*` are `todo!()`).
- **B2.6** height map and glow atlas (`glow_atlas.rs` stubs).
- DynDOLOD golden run (AltTextures rule, passthru shaders, HD masks), `e2e-compare`, in-game.

### 11.9 `grass_lod1_nif`: the Grass Optimizations far-LOD mesh (2026-10-20, branch `grass-lod1-20261020`)

The last `todo!()` of 11.1 that `lodforge build --grass go` reached. Rewritten in a fresh
session from the stub's own doc comment (ours), not from any earlier version of the function.

**Inputs read.** `docs/licensing.md`; `docs/reimplementation-plan.md` "Stage B2"; this spec
sections 3.3, 3.4, 6.1, 6.2, 11.1-11.3, 12.4 (the paragraph on the grass stubs);
`docs/re/dyndolod-spec.md` 3.5 (and the `CSGrassOptimizations` key rows); `docs/re/cacheless-grass.md`
(nothing on `_lod1`). Our code at HEAD: the stub's signature and doc comment, `BillboardDims`,
`write_grass_lod1`, the `build.rs` grass-go call site, `crates/nif/src/lib.rs` (data model) and
`writer.rs` (`encode`: the header's type table is used only when it covers every block),
`crates/lodmodel/src/lib.rs` `generate_nif` and `crates/btr/src/underside.rs` (how a BSFadeNode
root is named), `crates/nif/tests/build.rs` (alpha round trip), `crates/texgen/src/lib.rs`
(sidecar writer: `ShiftX/Y` are always 0, `Scale` is the override scale). Community Shaders
(`github.com/community-shaders/skyrim-community-shaders`, MIT, default branch on 2026-10-20):
`src/Features/GrassOptimizations/GrassMeshLibrary.{h,cpp}`, `src/Features/GrassOptimizations.cpp`
(settings text and `DrawBucket`), `package/Shaders/RunGrass.hlsl` (`VS_INPUT`, colour use).
Observed output: the 58 `meshes\lod\grass\*_lod1.nif` in each of the skyrim-cs mods `LODForge
E2E - LODForge Output` and `LODForge MVP Output` (written 2026-10-18 by the earlier,
LODGen-derived LODForge, so not authored files; no other `_lod1.nif` exists in the load order,
loose or in a BSA name table), one parsed with `peek_bto`: BSFadeNode → one BSTriShape (8 / 4),
lighting shader + texture set + NiAlphaProperty `0x12EC` / 128. Used only as a sanity check of
the alpha values the spec already gives (3.4: threshold 128); layout, names and shader values
below were chosen from the CS requirements and sections 3.4 / 6.1.

**What Grass Optimizations requires** `[cs-src]`: `LoadLODMesh` demands
`meshes\LOD\Grass\<source model stem>_LOD1.nif` (far band; `_LOD0.nif` = middle band, which the
far band falls back to), walks the tree and rejects the file unless it holds exactly one
BSTriShape and every object on the way has the identity transform ("requires one TriShape with
baked node and geometry transforms"); it takes the shape's vertex / index buffers, vertex desc
(OR'd with the instancing bits), triangle count and bound, and draws them with the **source
grass shape's material**: the LOD file's own shader and textures are not used by the feature.
The grass vertex shader reads `POSITION`, `TEXCOORD0`, `NORMAL` and `COLOR`; colour alpha both
multiplies the texture alpha and weights the wind (`0.5 * a²`).

**Written** (`apps/cli/src/dyndolod_build.rs`): `BSFadeNode "<texture stem>"` → `BSTriShape
"GrassLOD1"` (identity transforms, flags 14) with `BSLightingShaderProperty` (type 0, flags1
`0x80000000` ZBuffer_Test, flags2 `0x31` ZBuffer_Write | Double_Sided | Vertex_Colors,
glossiness 80, specular (0, 0, 0) × 1, emissive × 0, lighting effects 0 / 0, clamp 0: the
billboard values of 6.1) → `BSShaderTextureSet` (slot 0 the billboard, slot 1 its `_n` when
TexGen wrote one, 7 empty), and `NiAlphaProperty` `0x12EC` (test enabled, GREATER, no blend) /
128. Vertex desc position + UV + normal + colour (`0x0002b00006050407`, from
`VertexDesc::with_attributes`), colours opaque white, normals (0, 0, 1). Two quads: quad 0 in
XZ at y = ShiftY over x ∈ ±Width/2 (u = 0 at +X), quad 1 = quad 0 turned -90° about Z at
x = ShiftX over y ∈ ±Depth/2 (u = 0 at -Y), z from ShiftZ to ShiftZ + Height, v = 0 at the
top; every size and shift × `Scale`. Bound = centre of the box, radius to the farthest vertex.

**Checked.** Unit test `dyndolod_build::tests::grass_lod1_mesh` (parse back: BSFadeNode root,
one BSTriShape, identity transforms, 8 / 4, both texture slots, up normals, UVs in 0..1 with u = 0
at +X / -Y and v = 0 on top, bottom at ShiftZ × Scale); `cargo test -p lodforge` 23 passed.
Real run, release build: `build --instance D:\Modlists\skyrim-cs --profile LODForge-E2E --mode
dyndolod --worlds MarkarthWorld --no-terrain --no-objects --no-trees --grass go --no-plugins
--no-occlusion --exclude-mod "LODForge E2E - LODForge Output,LODForge E2E - DynDOLOD Output"`:
78 / 111 grass billboards, **58 `_lod1.nif` written**, 0 already present, 3.5 s; all 58 parse,
each references a billboard that exists in the output (no `_n` billboards in this load order, so
slot 1 is empty in all of them), bound radii 19.5-73.9. Centre and radius of `fern_ash_lod1.nif`
equal those of the earlier output for the same billboard.

**Open** (not settled here): because CS draws the LOD mesh with the source grass texture, UVs
spanning the whole billboard (0..1) sample the whole grass diffuse, not the billboard; a mesh
that should look like the billboard needs UVs into the source texture (or a CS change). The
in-game look was not checked. `lodmodel::generate_nif` sets a one-entry type table, which the
writer ignores, so its roots are written as `NiNode`, not `BSFadeNode` (noticed, not changed).

## 12. Process record of stages B2.3 to B2.6 (2026-10-20, continued)

Same session, branch and provenance rules as section 11 (the deleted modules, the decompiles
and quarantine, the listed documents stay unopened; AnvilLOD read and ported with attribution;
everything else in the repository is ours). `main` was merged into the branch first (737f2e3,
a docs merge only). One commit per sub-stage.

### 12.1 B2.3 hidden-face removal (commit 97d1585)

**Written**: `crates/objlod/src/hidden.rs` (two terrain sources, the rule as data, a per-quad
view), `terrain.rs` (cell-indexed LAND heights, bilinear, water per cell), the builder hook
(`Chunk::append` drops hidden triangles before copying vertices, counts per level, writes a
bare-root file for a quad whose references lost every triangle, as the golden
`Tamriel.4.-20.40.bto`), `object-lod --terrain / --hidden-rule / --no-hidden / --hidden-study`,
and `--grid` for the tolerant comparison. The `.btr` files are read with `lodforge_btr::Btr::
from_nif` (stage B1), one bin index of 256 units per file, parsed on demand and shared; the
terrain under a sample point is the land triangle containing it (skirts have no footprint and
are ignored); water is the cell's `XCLW` or worldspace default from the LAND data, else the
`.btr` water shapes. AnvilLOD `BtoBuilder.TerrainTest` gave the shape of the test (every
sample below the surface minus a margin, a sample without data keeps the triangle); the sample
sets, margins and sources became parameters.

**How the rule was settled**: `object-lod --hidden-study` builds without removal, labels every
triangle "removed" when the golden file of the same quad has no triangle at its positions
(0.1-unit grid), and scores a matrix of rules (terrain source: level-matched `.btr`, own quad
only, own quad clamped at its border, LAND; samples: three corners, corners + centroid, seven
points, centroid, any corner, two of three; margins -10, 0, 10, 32; water on / off; outside
kept / hidden). Result on Tamriel (683 files, 5,730,562 triangles built, 2,690,685 absent from
the golden):

| Rule | removed in both | rule only | golden only | precision | recall |
|---|---|---|---|---|---|
| `.btr` of the level, 3 corners, margin 0, water | 2,592,675 | 0 | 98,010 | 100% | 96.36% |
| same, corners + centroid | 2,591,094 | 0 | 99,591 | 100% | 96.30% |
| same, seven samples (AnvilLOD) | 2,586,220 | 0 | 104,465 | 100% | 96.12% |
| LAND heights, 3 corners, margin 0, water | 2,574,010 | 16,845 | 116,675 | 99.35% | 95.66% |
| `.btr`, 3 corners, margin -10 | 2,593,797 | 39,214 | 96,888 | 98.51% | 96.40% |
| `.btr`, 3 corners, margin 10 (`RemoveUnseenZShift`) | 2,553,885 | 0 | 136,800 | 100% | 94.92% |

Own-quad-only, clamped, outside-hidden, centroid, any-corner and majority variants all score
below these (they either drop geometry the golden keeps past the block or remove too little).
The 98,010 residue triangles of the best rule have all three corners within one unit of golden
vertices in 93,146 cases and are a median 564 units above the terrain: they are triangles the
golden *has*, at positions that differ by a float ulp and round differently on the 0.1 grid;
with a 2-unit grid the same rule reaches 99.38% recall at 100% precision. A terrain height
read from the golden `.btr` under one residue corner confirmed it (-52 to -148 under a corner
at z 897). Decisions on the spec's open question 1: terrain source = the terrain LOD mesh of
the same level (LAND heights are a fallback where no `.btr` covers a sample); samples = the
three corners; margin 0 (`RemoveUnseenZShift=10` is not a margin on this test: 10 costs 1.4
points of recall for nothing); water = the cell's water, all corners below; a corner without
data keeps the triangle; partially buried triangles stay. The LODGen 3.0.36 log totals agree:
after removal LOD4 1,922,031 (log 1,924,335), LOD8 896,415 (898,451), LOD16 319,441 (321,678).

**Numbers** (Tamriel, golden xLODGen export, golden `.btr` terrain, 683 files, 506 ms build,
1.3 s total; ours 3,137,887 triangles vs golden 3,144,458):

| Comparison | before B2.3 | after |
|---|---|---|
| tolerant, geometry only (0.1 grid) | 91.60% of golden, 50.26% of ours, 155 files differ in shape sets | 91.60% / **91.79%**, 99 files, 24 equal up to order |
| same, 1-unit grid | - | 93.94% / 94.13%, 158 equal up to order |
| same, 4-unit grid | - | 94.12% / 94.32%, 271 equal |
| tolerant with UVs and textures (no atlas remap yet) | 46.98% | 46.98% / 47.08% |
| directions within 1 byte (own-texture shapes) | normals 99.76%, tangents 96.98% | 99.77% / 99.80% |

The remaining 6% of golden triangles not reproduced geometry-only are the 99 files with
different shape sets (the LOD8/16 colour quirk of section 11.6, atlas-seam merges that B2.4
reproduces, the 2 HD exceptions) and shapes whose vertices differ by more than the grid.

### 12.2 B2.4 atlas remap, ReUV, atlas planning (this commit)

**Written**: `crates/objlod/src/atlas_map.rs` (`resolve`: the row chain of a shape, the
scale-adjusted tolerance test, `Remap::apply`), `reuv.rs` (the re-cut of a tiled shape against
the integer UV lines, off by default), `mesh.rs` (UV offset / scale of the source shader baked
into the UVs at import), `material.rs` (page textures and clamp 0 for an atlased shape, `=N`
alpha keys, PBR `linear` names, `is_hd` shared with the builder), `build.rs` (the remap per
part, the VC whitening, the colour rule below, the study hooks), `sidefiles.rs` (VC verdict per
listed shape), `stats.rs`; `apps/cli/src/object_atlas_plan.rs` (the DynDOLOD atlas planner that
`dyndolod_build::atlas` was a stub for: keys from the LOD models of every season's export plus
the AltTextures, the tolerance test, the TexGen mini-atlas rows written through with a chained
row of the mini page itself, tiles capped by `ObjectLODMaxTileSize` / `FullTextureMaxTileSize`,
pages main / `_Alpha` / `_Glow` through `atlas_build` and `glow_atlas`), `glow_atlas.rs`
(rewritten from the spec: 256-unit tiles, row packing into a power-of-two page); `object-lod
--atlas / --reuv`, `--tolerant` now reports the UV agreement (1/1024) of the paired vertices;
`crates/nif/examples/trace_golden_colors.rs`, colour statistics in `peek_bto`. AnvilLOD: no new
files ported (its `AtlasBuilder.cs` was read in B2.2 for the row format; the re-cut and the
chain are ours).

**Settled on the goldens** (the xLODGen golden of section 11 and the DynDOLOD golden2 of 8.3):

- The map is read as spec 1.2 / 7.2 describe it; a shape's remap is a chain: its texture's row
  (which may be a TexGen mini atlas with scale 1/n, so UVs up to n fit the tolerance without a
  re-cut), then the row of that mini page onto the big page. 81% of the golden2 LOD4 parts go
  through such a chain; the re-cut of `reuv.rs` is kept behind `--reuv` (LODGen asks TexGen for
  a tiled mini atlas instead, `MiniAtlasExport`), and with it off the atlased UVs agree with the
  golden to 1/1024 on 89.5% of the paired vertices (the rest differ by one half-float step of
  LODGen's remap arithmetic).
- HD shapes keep their full texture: no HD shape sits on an atlas page in golden2 (0 of 1,218).
- The VC side file whitens exactly the LOD shapes it lists `False` (RGB 255, alpha kept: the
  golden holds alpha 96 / 101 / 127 under white RGB); a LOD shape not listed keeps its colours
  (`rockcaveentrance01.nif` lists `:0`, `:3`, `:8`; the LOD shape `RockCaveEntrance01:1 -
  L2_Rocks:1` is coloured in `Tamriel.4.16.12.bto`). Section 11's "whiten every shape of a
  model without a `True` entry" was wrong.
- Open question 4 (which merged shapes carry colours) was settled with a study that scores every
  source part against the golden: `LODFORGE_TRACE_COLORS=1 LODFORGE_TRACE_GOLDEN=<objects dir>`
  prints, per placed part, its colour array, VC verdict, shader bits and the golden's colour
  kind at each of its vertices (none / opaque white / white with alpha / coloured). Golden2
  LOD 4, grouped by the golden shape the part landed in (parts whose vertices the golden lacks
  left out):

  | class | page | golden shape | parts with real colour | whitened arrays only | no arrays |
  |---|---|---|---|---|---|
  | plain | atlas | coloured | 378 | 16 | 4 |
  | plain | atlas | uncoloured | 1 | 128 | 318 |
  | plain | own texture | coloured | 111 | 29 + 27 opaque | 11 |
  | plain | own texture | uncoloured | 0 | 41 + 30 opaque | 27 |
  | plain HD | both | coloured / uncoloured | 2 / 0 | 21 / 438 | 0 / 55 |
  | snow | own texture | coloured / uncoloured | 35 / 0 | 505 / 0 | 2 / 9 |
  | snow HD | both | coloured / uncoloured | 1 / 0 | 424 / 0 | 3 / 67 |
  | snow | atlas | coloured / uncoloured | 177 / 0 | 68 / 92 | 1 / 169 |

  So at LOD 4 a plain shape is coloured when some part has non-white RGB after whitening (the
  parts without an array get opaque white vertices, the whitened arrays keep their alpha), a
  snow / ash shape on its own texture when some part has an array at all (the HD snow mountains
  are white with alpha 96..127), and an atlased snow shape follows the plain rule (the 68 / 92
  cell is the overlay parts of the rock cliffs, whose vertices coincide with the base parts, and
  the plain rule wins on triangles). At LOD 8 and 16 the golden does not merge: the parts with
  colour form a shape of their own next to an uncoloured one of the same name and textures
  (`Tamriel.8.-16.-16.bto`: `obj` 836 triangles coloured, 11,966 not, both on
  `DynDOLOD_Tamriel.dds`; 250 such pairs in golden2, 149 of them `obj` / `objSnow` at LOD 8),
  so colour presence is part of the grouping key there, per part: colour for plain, atlased,
  HD and every LOD 16 part, an array for snow on its own texture at LOD 8. Colour presence is
  not in the key at LOD 4. The LOD8/16 colour quirk of section 11.6 is this rule.

**Numbers** (release build; xLODGen golden 683 files, DynDOLOD golden2 684 files):

| Comparison | end of B2.3 | B2.4 |
|---|---|---|
| xLODGen, tolerant with UVs, textures and colours | 46.98% | **95.29%** (174 files differ in shape sets, 24 equal up to order) |
| xLODGen, tolerant geometry only | 91.79% | 96.70% (0 files differ) |
| xLODGen, UVs within 1/1024 (paired vertices) | - | 89.46%; normals 97.63%, tangents 89.66% |
| DynDOLOD golden2, tolerant | 11.7% | **83.78%** (467 files differ, 18 equal up to order) |
| DynDOLOD golden2, tolerant geometry only | - | 92.38% (327 files differ) |

Golden2 build: 34,526 references, 67,954 placements, 96,756 parts atlased, 25,547 whitened,
1,218 HD shapes, 684 files in 0.9 s. Per level the matched keys agree on 87.2% (LOD 4), 85.2%
(LOD 8) and 81.8% (LOD 16) of the golden triangles.

**Left for B2.5** (HD and colour checks): golden2 gives the `HD_LOD_Objects` flag without the
`HD` name to snow shapes on an HD texture whose model is not `_lod_0` at LOD 8 (`objSnow`,
flags2 `0x80000001`, uncoloured, 88,552 triangles; ours names them `objSnow`, no flag,
coloured), and at LOD 4 our `objSnow-LargeRef` HD flag on `mountainslab02linear.dds` (120,952
triangles) meets 38,889 without and 6,249 with the flag in the golden; the plain own-texture
shapes `obj` on `wood01.dds` (ours uncoloured 38,700, golden coloured) are the next colour cell.
The atlas planner is exercised by `lodforge build --mode dyndolod` in the end-to-end step.

### 12.3 B2.5 tree billboards, grass LOD, brightness, passthru and HD checks (this commit)

**Written**: `crates/objlod/src/billboard.rs` (tree billboards from the FlatTextures lines),
`grass.rs` (the grass cache decode, the complex-grass template planes, the brightness colours),
`build.rs` (the billboard and grass hooks, the `Crown / Trunk / FlatTrunk Brightness`
multipliers, passthru alpha 255 and single-row remap, `ColorRule`), `material.rs`
(`Material::fixed`, the passthru shader flag rule), `stats.rs`; `apps/cli/src/dyndolod_build.rs`
(`flat_textures` from the TexGen sidecars for `lodforge build --mode dyndolod`, tree billboards
on the planner's `_Alpha` page with `d,n` keys), `object_lod.rs` (`--grass`, `--grass-map`,
`--grass-density` wired, `--only <quads>`), `crates/grass/examples/{cgid_info,cgid_dump}.rs`,
`crates/nif/examples/{dump_shape,bto_nearest}.rs`. AnvilLOD: `World/GrassCache.cs` read for the
cache layout (position wrap, scale, "then the rotation matrix"; its yaw-only reading was
replaced by the full frame below), `Meshes/GrassPatchBuilder.cs` and
`Plugins/TreeLodGenerator.cs` read and not ported (bins and engine tree LOD, not LODGen's
geometry). Not opened: the provenance list of section 11.

**Tree billboards** (`treepassthru`, spec 6.1), settled on golden2 `Tamriel.4.0.-16.bto`
(reference `170A4ACF`, `alder_slim_side01_summer`) and `Tamriel.16.-32.-16.bto` (a conifer
rotated 35.6 degrees about x): two crossed quads per reference, the first along the reference's
local Y with the billboard **depth** (FlatTextures field 11), the second along local -X with its
**width**, bottom at `shiftZ`, height `height`, shift X / Y in the local frame, the whole quad
set under the reference's full rotation (x, y, z) and scale: 444 of the 546 billboard references
are tilted and the golden tilts their quads with them, `TreeLODMaxTilt=30` does not cap the
geometry. Vertex order `+axis bottom, -axis bottom, -axis top, +axis top`, triangles `[0 1 2]
[2 3 0]`, UVs the billboard's tile on the `_Alpha` page (golden2's map has the rows; the
reader takes `d,n` and one-part keys), normals `(0, 0, 1)` unrotated, tangent `rot(0, 0, -1)`,
bitangent along the quad, grey vertex colour `FlatLODVertexColor` varied by
`FlatLODColorVariance` per quad (golden 116 / 144 for 0.55 / 0.1 from a time-seeded random;
ours a hash of reference and quad), shader type 0, flags1 `ZBuffer_Test`, flags2 `ZBuffer_Write
| Double_Sided | Vertex_Colors` (`LOD_Objects` from `FlatLODLevelLODFlag`), glossiness 80,
specular black, emissive black times 0, lighting effect 1 = field 6, glow slot = the page when
field 8 is not black. The hidden-face test applies (the golden's LOD16 tree shapes lose single
triangles). Result: the sample LOD4 tree's eight vertices equal the golden's to three decimals,
the LOD16 file `Tamriel.16.-16.-16.bto` pairs all 79 tree vertices within 0.1 units, and 94% of
the matched `treepassthru` triangles agree over golden2 (1,638 billboards placed, 1,638 lines
found).

**Grass LOD** (`grasspassthru`, spec 6.2), settled on the grass golden of 2026-10-18
(`Tamriel.4.0.-16.bto`, 19 shapes, 553,776 triangles) against the cache
`Tamrielx0000y-016.cgid` (10,342 instances) and the TexGen sidecars:

- Cache instance (sixteen halves): x, y, z wrapped into +-49152 around the group centre, a
  scale (**not applied** by LODGen: the quad widths are the sidecar's for every instance
  scale), axes `A` (halves 4-6) and `B` (8-10), the third axis `C = A x B` packed into halves
  (12, 7, 11) (100% of the cell's instances), a signed height offset `t` (13), two zero
  halves. The quad frame is `Mx = (Ax, Bx, Cx)`, `My = -(Ay, By, Cy)`, `Mz = (Az, Bz, Cz)`.
- Complex grass (`ComplexGrassBillboard=4`, `DynDOLOD_flat_4x2alt1_lod.nif`): four planes per
  instance, plane-major in the shape, fitted per type with every type's medians on the
  formulas to 0.05 units (`fit_grass3.py`): the `_1` plane (template x in -1..1, raw y +-0.05
  under a node translation 0.05) at `X = x * Width_1 / 2 - Adjust_1`, `Y = y_raw * Width_1 / 2
  + 0.05 * Adjust_1`; the `_2` plane (raw x +-0.05, y in -1..1) at `X = -x * (Width_2 / 2 -
  Adjust_2)`, `Y = -y * Width_2 / 2 + Adjust_2`; the template's `alt` planes are both rotated
  180 degrees about the up axis with u mirrored; vertical `Z = (z * Height_N + ShiftZ) * (1 +
  t)` along `Mz`, so `1 + t < 0` hangs the quad below the ground, as the golden does. Vertex
  order: template vertices 0, 2, 1, 3; triangles `[0 1 2] [0 2 3]`; normals `+-My` (`_1`) and
  `+-Mx` (`_2`), tangent `-Mz` (sign with the height), bitangent along the width edge; colours
  `ComplexGrassBrightnessTop / Bottom` (0.7 / 0.75 / 0.75 -> `(178, 191, 191, 255)`), plain
  grass `GrassBrightnessTop / Bottom` per vertex row; shader type 0, flags1 `ZBuffer_Test`,
  flags2 `ZBuffer_Write | Vertex_Colors`, glossiness 80, clamp 0 on the `_Alpha` page (the
  grass golden's map has `d,n` rows for the `_1` / `_2` billboards). Shapes: one segment, filled
  cell by cell and quad by quad to 65,532 vertices.
- Result on `Tamriel.4.0.-16.bto` (16 cells, 118,702 cache instances, 69,222 placed in the
  quad's cells after the wrap: the same as the golden's quad count): 553,776 grass triangles
  (= golden); nearest-vertex distances golden -> ours: 77.3% within 0.1 units, 97.8% within
  0.5, 99.2% within 1, 100% within 2 (`bto_nearest`); normals / tangents / UVs of the paired
  vertices 99.9% within a byte / 1/1024; `--tolerant --grid 1` 59.0% of the file's triangles
  identical, 0.1-grid 8.9% (the half-precision axes spread the vertices by a few tenths).
  Build 0.1 s for the quad, file 40.0 MB (golden 39.9 MB).
- Not reproduced: the remaining tenths (the axes are halves; recomputing `C` in single
  precision changes nothing), the golden's shape boundaries (19 shapes; ours 21, cut at
  65,532 vertices without regard to the cache groups), `GrassDensity` below 100 (ours keeps a
  hashed share; LODGen's choice unknown), plain (`Internal`) grass (no golden: the Community
  Shaders target has HD grass only).

**Passthru shapes** (spec 3.4): vertex alpha written 255 (golden2 `fxwatertile01` /
`fxwhitewater01`: 255 where the source has 58 / 229); the atlas remap takes one row only
(`Tamriel.4.-28.16.bto`: the `farmwindowinterior01lod.dds` passthru shape stays on TexGen's LOD
texture, not chained onto the big page); a lighting shader other than the environment-map type
loses `Specular`, `Receive_Shadows`, `Cast_Shadows`, `External_Emittance` and
`EnvMap_Light_Fade` (golden2: 91 of 109 LOD4 passthru lighting shapes `0x8c400000 / 0x69` from
sources `0xae400301 / 0x8069`; the 18 others also drop `Own_Emit`, rule unknown). The glow-page
passthru shapes now agree in flags (`objpassthru col 0x69`, 8,159 vs 8,216 triangles).

**Brightness**: `Crown / Trunk / FlatTrunk Brightness R/G/B` (export keys, 1 in golden2)
multiply the colours of parts named so, clamped to bytes; no golden with values other than 1.

**FlatTextures writer** (`lodforge build --mode dyndolod`): every tree base with a TexGen
billboard, the sidecar's Width / Height / ShiftZ / Depth / ShiftX / ShiftY, the INI's
`FlatLODLightingEffect`, `FlatLODGlowMap`, `FlatLODVertexColor` (with-normal-map variant when
`_n.dds` exists), `FlatLODColorVariance`, `FlatLODHeightVariance`, the flat normals file four
times, Delphi `FloatToStr` numbers. Against the E2E golden's 634 lines (ours 540: its TexGen
run made more billboards) no line is byte-identical because the sidecar values differ in the
fourth decimal (our TexGen vs theirs), the layout is the golden's.

**Numbers** (DynDOLOD golden2, 684 files):

| Comparison | B2.4 | B2.5 |
|---|---|---|
| tolerant | 83.78% (467 files differ) | **85.14%** (432 files differ, 18 equal up to order) |
| tolerant geometry only | 92.38% (327 files differ) | 92.45% (234 files differ) |
| matched keys agreeing, LOD 4 / 8 / 16 | 87.2 / 85.2 / 81.8% | 90.2 / 86.7 / 86.6% |

xLODGen golden unchanged (95.29%; its export has no billboards). Build 0.9 s.

**Left open**: the HD items of 12.2 (golden2's `HD_LOD_Objects` flag on `objSnow` shapes
whose model is not `_lod_0` at LOD 8, 88,552 triangles uncoloured; our `objSnowHD-LargeRef`
on `mountainslab02linear.dds` at LOD 4 against the golden's `objSnow-LargeRef` for the
`mountaincliffslope`, `rockcliff08`, `rockcaveentrance` models), the plain own-texture colour
cell (`obj` on `wood01.dds`), the 18 passthru shapes dropping `Own_Emit`, and the grass tenths.

### 12.4 B2.6 object height map and the glow page (this commit)

**Object height map** (`crates/objlod/src/heightmap.rs`, written by `object-lod` and the
DynDOLOD-mode build when the export says `CreateHeightMap=True`, default season only). The
consumer is the occlusion module on main (`crates/dyndolod/src/occlusion/mod.rs`
`object_heightmap`, `grid.rs` `raise_top`), read for this stage as the brief allows: it takes
point `k` of a cell at `((k / 33) * 128, (k % 33) * 128)`, i.e. `height[i * 33 + j]` with `i`
along x **outer**. 7.1 guessed the `VHGT` order (y outer); the golden file has x outer: our
samples as stored agree with the golden's in 24.9% of the shared points, transposed in 1.4%,
so 7.1's bullet is superseded by this one and the consumer's order stands.

The sample rule was fitted on the golden (`Tamriel.objects.heightmap.dat`, 14,125 cells, 7,009
of them entirely `f32::MIN`), with our point samples `P(i, j)` = the highest plane z of any
LOD4 triangle (after hidden-face removal, grass excluded, billboards cover no point) whose XY
projection covers the grid point, edges inclusive:

| Golden sample `(i, j)` modelled as | shared points within 1 unit | golden-only points |
|---|---|---|
| `P(i, j)` | 24.9% | 32,272 |
| the exact maximum of the triangle planes over the square `[i-1, i] x [j-1, j]` | 49.9% | 6,413 |
| `max` of `P` over the square's four corners | 65.6% | 7,171 |
| **`max(P(i, j), P(i-1, j), P(i, j-1))`** | **89.7%** (golden higher 4.9%, lower 5.4%) | 7,514 |
| `max` of `P` over the five-point cross | 54.6% | 2,687 |

(600-1,200 sampled common cells; the one-sample shifts alone: `P(i-1, j)` 39.8%, `P(i, j-1)`
43.2%, `P(i-1, j-1)` 1.5%.) The three-point rule is adopted: the stored sample is the maximum
of the point and its west and south neighbours, looked up across the cell border (sample 32 of
the western cell is sample 0 of this one), cells without any sample dropped. The triangle max
z instead of the plane z (`trimax`) and raising the whole XY bounding box (`bbox`) were tried
on the point model and lowered the agreement (47.5% -> 45.4% / 42.5% identical including
empties); not adopted. Why the golden's footprint is this L shape is not derivable from the
data (a rasteriser filling the square north-east of each covered point would give four
corners, not three); it is recorded as the observed rule.

Result on golden2 (`run_dyn.sh`, whole Tamriel): 7,143 cells (golden 7,116 non-empty); over
the common cells' 7.78 M points 54.05% identical and 92.94% within 1 unit (B2.6 start, point
samples: 47.48% / 57.06%), 3.47 M empty in both, only ours 88,277, only golden 49,775; among
the 4.17 M points present in both 90.1% within 1 unit. The previous implementation had 94.28%
exact / 99.02% within 1 (8.2); the remainder here follows the LOD4 geometry differences of
12.2 / 12.3 (the shapes we place differently cover different points), and 7.1 says agreement
is a diagnostic, not a gate: the occlusion stage quantises heights coarsely. `object-lod
--golden <dir>` prints the comparison ("vs golden height map").

**Glow page**: already planned and rendered since B2.4 (`object_atlas_plan` collects the
`diffuse,normal,glow` keys of glow-mapped shapes into the `_Glow` layout of `glow_atlas.rs`,
`object_atlas::render` writes `DynDOLOD_<World>_Glow.dds` / `_n` / `_g`); nothing to add in
code, the `dyndolod_build` note that called it a stub was stale. Golden2's six glow rows are
window glows of `glow_lod` models placed by DynDOLOD's `Windows` option; the E2E reference
output (built without it) has no glow page, so the end-to-end step runs without `--windows`
and the page is checked for presence only when the plan reports glow tiles.

**DynDOLOD grass stubs**: `grass_header` (the grass golden's header keys, brightness pairs,
`GrassLargeReference`, `RemoveUnseenGrass`, `GrassGID`) and `complex_grass_flat` (the
`GrassBillboard<ComplexGrassBillboard>` template path, the flat normals file three times,
black glow / back, `FlatLODWithNormalMapVertexColor`, `FlatLODColorVariance`) are implemented
from the grass golden's export (6.2); `grass_lod1_nif` (`--grass go`, Grass Optimizations
meshes) stays a stub: it needs `BSFadeNode` in the NIF writer, which the object LOD rewrite
does not need, and the Community Shaders grass path is parked (offline-grass-cache note).

Files never opened in this sub-stage: as 11.3 / 12.1; the occlusion module on main was read
(`crates/dyndolod/src/occlusion/mod.rs`, `grid.rs`, `apps/cli/src/occlusion.rs`, ours). No
AnvilLOD file ported. Tests: `heightmap.rs` two (coverage / dilation).

### 12.5 End to end: `lodforge build --mode dyndolod` on LODForge-E2E, `e2e-compare`, in game (this commit)

**Build** (release, `D:\LODForge-output\objlod-rewrite-20261020`):

```
lodforge build --instance D:\Modlists\skyrim-cs --profile LODForge-E2E
  --exclude-mod "LODForge E2E - DynDOLOD Output,LODForge E2E - TexGen Output,LODForge E2E - LODForge Output"
  --worlds Tamriel --out D:\LODForge-output\objlod-rewrite-20261020 --mode dyndolod --texgen
  --grass bto --grass-density 100
```

The reference (`LODForge E2E - DynDOLOD Output`, 686 BTOs) has grass in its LOD4 BTOs and no
glow page, so grass is on (DynDOLOD's default density assumed; the run's setting is not
recorded in its output) and `--windows` off. Two defects stopped the first runs, both outside
the object LOD builder's golden runs (fixed in 3ea97b2):

- The atlas map parser took tile positions as unsigned; our TexGen stage's re-uv row for
  `textures\architecture\farmhouse\wovenfence01.dds` places a 758 x 512 source region at
  x = -220 on `wovenfence01lod.dds` (scale 0.25 / 0.5) where DynDOLOD's TexGen has 445 x 256
  at x = 36 (scale 0.5 / 1). Positions are `i32` now; the row itself (TexGen's crop of that
  fence texture, `crates/texgen/src/stitch.rs`) is left as a follow-up for the TexGen stage.
- The grass step read every cache file with the strict `CellCache::parse`, which rejects
  trailing bytes; NGIO rewrites cache files in place without truncating, and the E2E cache has
  files of exactly twice and four times their content (`Tamrielx0060y-016.cgid`: 75,730 bytes,
  content 37,865; the Default cache's copy 151,460). One such file failed the whole grass step
  (no grass, no grass billboards on the atlas). Both readers use `parse_prefix` now, as the
  game's loader does.

Stage results (third run): texgen 543 tree + 241 trunk billboards; grass cache 5,694 cells,
46 types, 29.7 M placements read, 32 types billboarded, 15.7 M placements kept by TexGen's
rule; atlas plan 1,497 models, 304 keys (184 main, 622 alpha, 0 glow, 348 mini rows);
objects: 67,532 placements, 1,527 tree billboards, 15,645,307 grass instances (0 unmapped),
252 passthru shapes, 1,265 HD shapes, 48,704 AltTextures applied, 96,690 parts atlased,
25,561 VC-whitened, height map 7,171 cells, 27.8 s for the objects stage.

**`e2e-compare --tolerant`** against `LODForge E2E - DynDOLOD Output` (DynDOLOD 3's own run on
the same profile; JSON at `D:\LODForge-output\objlod-rewrite-20261020-e2e-compare.json`):

| Comparison | before (8.2, LODGen-derived) | now |
|---|---|---|
| object LOD files (default season) | - | ours 685, reference 685, 684 in both (LOD4 500 / 499, LOD8 142 / 142, LOD16 43 / 44; `Tamriel.16.16.-64.bto` reference-only) |
| tolerant, all attributes | 96.16% | 5.02% (atlas UVs and texture paths cannot match: our atlas plan lays the pages out differently, 7.25% of paired vertices agree in UV; normals 99.47%, tangents 98.62% within a byte) |
| geometry only, objects LOD4 / 8 / 16 | 97.28 / 94.82 / 88.16% | **97.42 / 92.89 / 84.22%** of the reference's triangles (94.87 / 92.03 / 79.52% of ours) |
| geometry only, grass LOD4 | 1.09% | 0.22% by triangle; centroids within 1 unit 4.87%, within 4 units 33.70% (the reference's placement differs from the E2E cache's, as before; the grass golden of 12.3 is the measure for grass) |
| object height map | 94.28% exact / 99.02% within 1 | 53.63% exact / 92.18% within 1 unit (7,171 cells, all in the reference's 14,125) |
| occlusion | bit-exact with the golden height map | 6,279 cells vs 6,285, bit Jaccard 0.8834 on the common cells (the clean-room occlusion module's own measure is 0.875) |
| dynamic LOD `[Objects]` | - | 3,061 originals, 3,060 of the reference's 3,078 (99.4%) |
| tree LOD | - | 51,566 references in 744 files, all in the same block at the same scale |

Reading: the full-attribute number is not comparable any more (the old builder reproduced
DynDOLOD's atlas layout and keys exactly; the rewrite's planner is its own, so every atlased
UV differs by design, and `e2e-compare`'s geometry-only line is the measure of the same
inputs). LOD4 geometry is at the old level, LOD8 / LOD16 are 1.9 / 3.9 points lower (the
colour-split and HD items left open in 12.2 / 12.3). The height map agreement follows the
LOD4 geometry; occlusion is the clean-room module on main consuming our height map.

**In game** (skyrim-mcp, instance `skyrim-cs`, profile `LODForge-E2E` with the output
installed as the mod `LODForge E2E - LODForge Output (objlod-rewrite 20261020)` and the
three reference output mods disabled, so nothing of DynDOLOD's or TexGen's output was in the
VFS; `coc WhiterunExterior01` on a new game, the Survival prompt answered No through
`MessageBoxMenu buttonPress 1`, `tgm`, `killall`, `tg`, no `tll`): DynDOLOD NG 3.0.43 reads
our `DynDOLOD_NG_Worlds.txt` / `_Tamriel.txt` and installs its handlers, Papyrus reports
"DynDOLOD successfully initialized", no crash in 4 minutes across three regions. Eight engine
captures (clean preset, free camera above the player; `D:\workspace\SkyrimMCP\runs\screenshots\20261020T225111Z\rewrite-*.png`
with JSON sidecars): Whiterun tundra east / west from 600 and 2,000 units up (farms, the
tower, tree billboards, grass LOD beyond the loaded cells, the mountains with snow), Solitude
shore (the glacier LOD across the water, the arch) and the coast east (shipwreck, tree LOD,
snowy ranges), Winterhold north (the College, the ice floes) and the ranges west. No LOD
holes, no missing (black / magenta) textures, no floating or doubled geometry seen; the
captures are hazy because Community Shaders' fog does not follow the preset's forced clear
weather within the settle time, which limits what they can show about texture detail. Not
exercised here: seasons, child worldspaces, the parent / child transitions, save / reload
(validation.md section 4 items 4 to 6).

**Follow-ups found by this step** (outside the object LOD builder): the TexGen re-uv row for
`wovenfence01` (above); the E2E grass cache's in-place rewritten files (handled); the plugin
record counts of `DynDOLOD.esm` (ours 1,289 CELL / 1,479 REFR / 251 STAT / 44 TREE, the
reference 1,340 / 1,608 / 338 / 69) and `DynDOLOD.esp` (ours 3,113 REFR, the reference
3,530) are main's plugin writer, unchanged by this branch.

**Commits of the end-to-end step**: 3ea97b2 (the two fixes), this one (the record). Branch
state: `objlod-rewrite` = main (737f2e3 merge base, main unchanged) + B2.3-B2.6 + end to
end; nothing merged, nothing pushed.
