# Occlusion (CELL TVDT, Occlusion.esp): scrubbed specification

Written 2026-10-19 as the analyst step of [Target 1](../reimplementation-plan.md) so that a
fresh session can re-implement `crates/dyndolod/src/occlusion.rs` without ever seeing DynDOLOD's
internals. Everything here comes from public documentation, the MPL-2.0 xEdit record
definitions, the shipped configuration, LODForge's own files and the golden output of a real
DynDOLOD run. **It describes inputs, outputs and formats, never how DynDOLOD computes
anything.** Where a fact could not be established from those sources it is listed as an open
question for the implementer to settle empirically against the golden files (section 10).

**Evidence tags.**

| Tag | Source |
|---|---|
| `[doc: readme]` | `D:\Modlists\skyrim-cs\tools\xLODGen\Skyrim-Occlusion-Readme.txt` (xLODGen, the same generator DynDOLOD embeds) |
| `[doc: dyndolod.info]` | <https://dyndolod.info/Help/Occlusion-Data> (read 2026-10-19) |
| `[doc: uesp]` | <https://en.uesp.net/wiki/Skyrim_Mod:Mod_File_Format/CELL> and `/WRLD` (read 2026-10-19) |
| `[cfg]` | `D:\Modlists\skyrim-cs\tools\DynDOLOD\Edit Scripts\DynDOLOD\DynDOLOD_SSE.ini` as shipped (read, never bundled) |
| `[xedit-src]` | MPL-2.0 xEdit sources, `D:\Modlists\ref\TES5Edit\Core\` (`wbDefinitionsTES5.pas`, `wbDefinitionsCommon.pas`, `wbLOD.pas`, `wbDataFormatMisc.pas`) |
| `[golden]` | the golden run `D:\Modlists\LODForge-golden\dyndolod-20261017-medium-tamriel\` (DynDOLOD 3.0 Alpha-215, Tamriel, Medium preset, seasons; `output\Occlusion.esp`, `output\DynDOLOD.esm/.esp`, `logs\DynDOLOD_SSE_log.txt`, `logs\DynDOLOD_SSE_Debug_log.txt`, `export\`, `options.json`, the run's saved `DynDOLOD.ini` / `DynDOLOD_SSE_Default.ini`) |
| `[check]` | measured by [Check-TvdtLayout.py](../../scripts/golden/dyndolod/Check-TvdtLayout.py) on the golden plugin (run 2026-10-19; invocation in section 4) |
| `[ours]` | LODForge's own files: `apps/cli`, `scripts/golden/dyndolod/Test-OcclusionParity.ps1`, `docs/baseline.md` |
| `[anvillod]` | AnvilLOD `TvdtData.cs` header and `NodeBit` (GPL-3.0-only, `D:\Modlists\ref\AnvilLOD` at `606eece`), read only for the corroboration paragraph of section 4 after the licensing session allowed it (section 11) |
| `[inferred]` | our deduction from the above; verify before relying on it |

## 1. The feature `[doc: readme]` `[doc: dyndolod.info]`

- The `TVDT` subrecord of an exterior CELL record holds pre-computed occlusion data: for the
  cell it tells the engine which terrain and object LOD meshes around it are hidden behind
  terrain (a mountain in front of them), so they are not drawn. The data covers the LOD levels
  4, 8, 16 and 32 (blocks of 4x4, 8x8, 16x16 and 32x32 cells).
- "The TVDT - Occlusion Data is a z-order curve bitmap lookup table for each quad (LOD) level
  up to a maximum of 256x256 of cells. The area is fixed (each quad always uses the same bit
  regardless of the relative position to the current cell) and has its origin at the south west
  cell defined in the lodsettings. The actual maximum is defined by the stride value also
  defined in the lodsettings file." `[doc: readme]`
- The vanilla game ships TVDT only for Tamriel (not for DLC2SolstheimWorld), and it is wrong
  for several cells (rectangular LOD holes, notably in the Sea of Ghosts). The CK does not
  generate it.
- For TVDT to be applied the CELL also needs `MHDT` (max height data): from it the engine knows
  whether the player is at the "same" elevation as the terrain/objects the occlusion was computed
  for; when the player is higher (dragon flight) all TVDT occlusion is ignored. The WRLD's own
  MHDT is unrelated.
- Generation: the generator "calculates the TVDT - Occlusion Data based on the current terrain
  height data from the loaded plugins and complements it with the height data from the existing
  terrain (in case of cut content) and object (to determine the maximum height of things) LOD
  meshes." Object LOD should therefore be generated first; LOD files in the output folder take
  precedence over BSA/loose files. Occlusion should be generated last, after all patches and
  terrain and object LOD for the worldspace. `[doc: readme]` `[doc: dyndolod.info]`
- "If a cell record is missing its MHDT - Max Height Data, xLODGen also calculates it from the
  terrain height data and the object LOD as a best effort so that the occlusion works in the
  cell." The CK computes MHDT from terrain and full-model collision instead; a missing WRLD
  MHDT is not generated. `[doc: readme]`
- "If there are child worldspaces (e.g. walled cities) using the selected worldspace for LOD,
  their TVDT data will be updated as well." `[doc: readme]`
- Options `[doc: readme]` `[doc: dyndolod.info]`:
  - **Quality**: samples per cell. "Use 2 for a quick update. Use 3 if time permits - it
    typically results in a few more areas that are disabled." Quality 3 "may have to compare up
    to 256 (16 x 16) samples between two cells instead of 16 (4 x 4)" and can be orders of
    magnitude slower. Quality 2 is DynDOLOD's default.
  - **Height**: "the algorithm uses the highest point of the height map or object LOD between
    two samples points. Since the player itself could be standing on the highest point, this can
    add a bit of additional height if required." Change it only if holes remain visible from a
    vantage point.
  - **Radius**: "the engine renders about a 100 cells into the distance, so there is no need to
    show or calculate the visibility of cells beyond that limit." Lower values artificially
    limit LOD distance; higher values cost time for no benefit.
  - **Mode**: limits which CELL records are processed. "CELL records without terrain height data
    are always skipped. -Flat means to skip cells with terrain height data that is perfectly
    flat - it typically means the cell is way outside the playable area. +Border means to only
    process cells if they are linked to a region that has the Border Region flag set ... Do not
    use if no suitable cells are found in case the worldspace has no border region defined.
    +TVDT will update cells that already have TVDT data and -TVDT will update cells that do not
    already have TVDT data. +Border is ignored for child worldspaces that use the selected
    worldspace for LOD."
  - xLODGen also offers a "specific chunk" option (1, 4x4 ... 32x32 cells from given W/S
    coordinates) `[doc: readme]`; DynDOLOD's dialog exposes `Chunk=0` `[golden: DynDOLOD.ini]`.
    Not needed for LODForge.
- Occlusion.esp `[doc: readme]` `[doc: dyndolod.info]`: a new plugin that "will contain the
  WRLD/CELL records from the winning overwrite with the updated TVDT data"; when it already
  exists and holds the selected worldspace, that worldspace is deleted from it and copied again.
  "For Skyrim Special Edition the plugin is automatically flagged as ESL as it only contains
  overwrite records." It should load last so its CELL records are not overwritten. DynDOLOD can
  alternatively write the data into DynDOLOD.esp (`OcclusionPlugin=0`; Enderal always does).

## 2. Configuration `[cfg]`

Shipped `DynDOLOD_SSE.ini`, section `[DynDOLOD]`, occlusion keys and defaults (comments are the
file's own words):

| Key | Shipped default | Shipped comment / meaning |
|---|---|---|
| `OcclusionMaxThreadsTerrainLOD` | `-1` | "Limit the number of threads in case updating height map from terrain/object LOD meshes requires large amounts of memory"; −1 = no limit |
| `OcclusionMaxThreadsObjectLOD` | `4` | same; "0 = true single thread and will take forever" |
| `OcclusionHeight` | `0` | "additional height to add to oberserver height" |
| `OcclusionRadius` | `100` | "max render distance" |
| `OcclusionMode` | `-TVDT -Flat +Border` | terms `All`, `+Border`, `-Flat`, `+TVDT`, `-TVDT`, combined as listed in the file ("-Flat +Border = All terrain that is not flat and a border region", etc.); "In any case, only CELLs that are part of the dynamic LOD Near/Far grids are updated" |
| `Occlusion_<World>` | (example `;Occlusion_ExampleWorld=1`, `Occlusion_ArnimaDagonRealm=0`) | "world specific settings *_[Worldspace name] (EditorID)"; "by default worlds that have the small world flag set are skipped automatically"; 1 forces a small world on, 0 forces a large world off |
| `OcclusionMode_<World>` | `OcclusionMode_Tamriel=-Flat +Border`, `OcclusionMode_Sovngarde=-TVDT -Flat`, `OcclusionMode_DLC01SoulCairn=-TVDT -Flat` | per-world mode |
| `LODGenHeightMap` | `1` | "LODGen calculates and exports object LOD heightmaps for occlusion generation, requires occlusion generated to be checked as well" |

The dyndolod.info page adds: any occlusion setting takes a `_<Worldspace EditorID>` suffix;
`OcclusionQuality` (default 2); `OcclusionMaxThreadsObjectLOD=1` is the remedy for out-of-memory
during the object LOD mesh pass `[doc: dyndolod.info]`. The run-level switches `DoOcclusion=1`,
`OcclusionPlugin=1` (separate Occlusion.esp) and `OcclusionQuality=2` are stored in the
options file DynDOLOD saves with a run, and the dialog's values in `DynDOLOD.ini` section
`[SSE LOD Options]` (`OcclusionQuality=2`, `OcclusionHeight=0`, `OcclusionRadius=100`,
`OcclusionMode=-Flat +Border`, `OcclusionNewPlugin=1`, `Chunk=0`) `[golden]`. The debug log lists
the resolved per-world values (`Occlusion_<World>: True/False`, `OcclusionMode_<World>: ...`) for
every worldspace of the load order `[golden: Debug_log "ListVars"]`.

**Effective settings of the golden run** `[golden: DynDOLOD_SSE_log.txt]`: Tamriel selected in the
worldspace list; `Quality: 2 Adjust: 0` ("Adjust" is `OcclusionHeight`), radius 100, mode
`-Flat +Border` for Tamriel and `-Flat` for each child worldspace (the `+Border` term removed, as
the public text says).

## 3. Record and file formats `[xedit-src]` `[doc: uesp]`

CELL (exterior), the subrecords that matter, in xEdit's definition order
(`wbDefinitionsTES5.pas`, record `CELL`): `EDID, FULL, DATA, XCLC, XCLL, TVDT, MHDT, LTMP, LNAM,
XCLW, XNAM, XCLR, XLCN, XWCN/XWCS, XWCU, XCWT, XOWN, XRNK, XILL, XWEM, XCCM, XCAS, XEZN, XCMO,
XCIM` (`SetUnordered`: a file may hold them in any order; a copy made by xEdit writes them in this
order, see section 7).

| Subrecord | Definition |
|---|---|
| `XCLC` | `int32 X, int32 Y` grid coordinates of the cell, then `uint32` land-hide flags (`wbDefinitionsCommon.pas`, "Grid") |
| `TVDT` | `wbByteArray(TVDT, 'Occlusion Data')`: an opaque byte array to xEdit; 684 bytes in every observed record (section 4) |
| `MHDT` | `wbMHDTCELL`: `float Offset`, then `Max Heights` = 32 rows × 32 columns of `uint8` (1,028 bytes). UESP: "A 32×32 grid of per-quad height ceilings used by the dragon flight system"; the float is the vertical reference, each byte the height above it; UESP estimates "roughly 110 game units per int8 step" for vanilla cells. The unit measured in the golden output is 128 (section 6) |
| `XCLW` | `float` water height of the cell (non-ocean water); xEdit notes that some records hold `$FF7FFFFF` (an invalid float meaning "none") |
| `XCLR` | sorted array of `REGN` form ids: the regions containing the cell |
| `DATA` | cell flags (`uint16`, or a single byte in some vanilla records) |

LAND `VHGT` (`wbDefinitionsCommon.pas`, `wbLandHeights`): `float Offset`, 33 rows × 33 columns of
`int8` deltas, 3 unused bytes. Heights are in units of 8 game units; row 0 starts at the offset,
each row's first value is relative to the previous row's first value, and the other 32 values of
a row are relative to their left neighbour (`[doc: uesp]`, LAND page; the same decoding LODForge's
`crates/esp` already performs). A cell is 4,096 game units square, so the 33×33 vertices are 128
units apart.

WRLD (`wbDefinitionsTES5.pas`, record `WRLD`; `[doc: uesp]` for the meanings):

| Subrecord | Definition |
|---|---|
| `WNAM` | parent worldspace (form id) |
| `PNAM` | `uint16` "Use flags": what is inherited from the parent: `0x01` Use Land Data (DNAM), `0x02` Use LOD Data (NAM3, NAM4), `0x04` Use Map Data, `0x08` Use Water Data (NAM2), `0x10` (unknown; xEdit: Use Climate Data is bit 4 = `0x10`), `0x40` Use Sky Cell |
| `DNAM` | `float[2]`: default land height and default water height ("-27000 & -14000.0 for Tamriel") |
| `NAM4` | `float` LOD water height ("-14000.0 for Tamriel") |
| `DATA` | `uint8` flags: `0x01` Small World, `0x02` Can't Fast Travel, `0x08` No LOD Water, `0x10` No Landscape, `0x20` No Sky, `0x40` Fixed Dimensions, `0x80` No Grass |
| `RNAM` | large references (`wbWorldLargeRefs`): per cell grid, the references of base objects of size ≥ 512 |
| `MHDT` | the worldspace's own max height data (`wbWorldMaxHeight`): min/max cell grid then one byte per quad |
| `OFST` | `wbByteArray(OFST, 'Unknown', 0, cpIgnore)`: UESP: "an ordered array of Relative Absolute file offsets to each Cell record under the Worldspace", zero for cells not in the plugin; local to each plugin, rewritten by xEdit on save |
| `NAM0`, `NAM9` | `int32[2]` south-west and north-east corner of the worldspace |

REGN: record-header flag `0x40` = "Border Region" (`wbDefinitionsTES5.pas`, `wbRecord(REGN, ...)`
flags list, bit 6).

`lodsettings\<World>.lod` (`wbLOD.pas`, `TwbLodSettings.LoadFromData`, Skyrim branch): exactly
16 bytes: `int16 SWCell.x, int16 SWCell.y, int32 Stride, int32 LODLevelMin, int32 LODLevelMax`.
LOD blocks of level L start at `SWCell + ((cell − SWCell) div L) * L` (`BlockForCell`).
Tamriel's file (`lodsettings\tamriel.lod` in `Skyrim - Meshes0.bsa`, extracted with
`lodforge vfs-extract`) is `a0 ff a0 ff 00 01 00 00 04 00 00 00 20 00 00 00` =
south-west cell **(−96, −96), stride 256, levels 4..32** `[golden]`; the golden log prints the
same: `[Tamriel] LODsettings: Level 4 to 32 stride 256 from -96,-96 to 0,0`. None of the seven
child worldspaces in the golden plugin has a lodsettings file (`lodforge vfs-list lodsettings\`
lists 15 files, among Tamriel's children only `markarthworld.lod`), so they share Tamriel's grid.

Object LOD height map `<World>.objects.heightmap.dat` (written by LODGen when the export has
`CreateHeightMap=True`, and by LODForge's `object-lod --flavor dyndolod`; LODGen is MPL 1.1, its
writer is in `D:\Modlists\ref\lodgen-dyndolod`, and LODForge's writer is `crates/objlod`):
`int32 count`, then per cell `int32 x, int32 y` and 33 × 33 `float` heights, value `k` belonging to
the point `((k div 33) · 128, (k mod 33) · 128)` game units from the cell's south-west corner.
Verified on the golden file `output\meshes\Terrain\Tamriel\Objects\Tamriel.objects.heightmap.dat`:
count 14,125, size 61,641,504 = 4 + 14,125 × (8 + 4,356) bytes, cells x −60..64, y −64..48
`[golden]`. The heights are the tops of object LOD geometry per 128-unit sample; cells without
object LOD have no record.

Terrain LOD meshes: `meshes\terrain\<World>\<World>.4.<x>.<y>.btr` (NIF; LODForge reads and
writes them, `crates/nif`, `crates/btr`). The golden run read the vanilla BSAs' terrain LOD.

## 4. TVDT bit layout `[golden]` `[check]`

Derived from the golden `Occlusion.esp` alone, by testing candidate layouts that only assume the
public description (one bit per block at each LOD level over a fixed `stride × stride` grid in
z-order from the lodsettings south-west cell). Reproduce with

```bash
python3 -I scripts\golden\dyndolod\Check-TvdtLayout.py D:\Modlists\LODForge-golden\dyndolod-20261017-medium-tamriel\output\Occlusion.esp --origin -96,-96 --stride 256 --radius 100 --other-plugin D:\Modlists\LODForge-golden\dyndolod-20261017-medium-tamriel\output\DynDOLOD.esm --other-plugin D:\Modlists\LODForge-golden\dyndolod-20261017-medium-tamriel\output\DynDOLOD.esp --skyrim-esm "D:\Modlists\skyrim-cs\Stock Game\Data\Skyrim.esm"
```

(`python3` = `C:\Users\macti\AppData\Local\Python\bin\python3.exe`, about 15 s, prints `PASS`.)

**The layout.** For a grid of `stride` cells (Tamriel 256) with `nb = stride / 4` leaf blocks per
side (64):

- The field is a stream of bits over a **quadtree of blocks**: level 0 = 4×4-cell blocks
  (64 × 64 = 4,096 of them), level 1 = 8×8 cells (32 × 32), ... level 6 = the whole 256-cell grid
  (1). Total **5,461 bits** = 4,096 + 1,024 + 256 + 64 + 16 + 4 + 1.
- Bit position `p` (p = 0 ..) is bit `p mod 8` of byte `p div 8` (least-significant bit first).
  Stream index `k` is stored at bit position `k`.
- The stream is the **pre-order traversal** of the quadtree: a node's bit, then the four
  children's subtrees in **z-order from the south-west**: south-west, north-west, south-east,
  north-east (x is the higher Morton bit, y the lower). So bit 0 is the whole-grid node, bit 1
  the south-west 128-cell quarter, bit 2 its south-west 64-cell quarter, ... bit 6 the
  south-west-most 4×4 block (cells −96..−93), bit 7 the block north of it (−96, −92), bit 8 the
  block east of it (−92, −96), bit 9 (−92, −92), bit 10 the 8×8 node at (−96, −88), and so on.
  Equivalently: read backwards from bit 5,460 the stream is the post-order traversal with the
  siblings in the order north-east, south-east, north-west, south-west (the same assignment of
  bits to nodes described from the other end; the check script accepts both descriptions).
- **Every parent bit is the OR of its four children** (8,579,025 parent/child relations in the
  6,285 golden cells, none violated). A set leaf means the 4×4 block is **visible** (LOD drawn),
  a clear bit hidden `[inferred from: the cell's own block is set in every cell; nothing beyond
  the radius is set; the readme says the data disables occluded LOD]`.
- The 11 bits after the stream (byte 682 bits 5–7 and byte 683) are **0** in every record; the
  field is always **684 bytes**. 684 × 8 = 5,472 = 5,461 + 11, so the size is the stream rounded
  up to whole bytes plus one spare byte.
- Consequence: the first `uint32` of a TVDT is 3 (root + south-west quarter set) in 6,055 of the
  golden cells, 7 in 171, 15 in 32; it is not a header `[check]`.

**Measured facts** `[check]` (golden Occlusion.esp, 6,285 cells with TVDT):

| Measurement | Value |
|---|---|
| TVDT size | 684 bytes in 6,285/6,285 |
| Stream bits / spare bits | 5,461 / 11, spare bits set in 0 cells |
| Candidate structures tested (traversal × bit numbering × direction, with and without 6 lead bits) | 24; the pre-order/forward and post-order/backward pair is the only survivor (the other 22 fail in 40/40 sampled cells with 3,702–16,946 bad nodes each) |
| Parent = OR(children) | 8,579,025/8,579,025 relations, 0 bad cells |
| Square symmetries tested for leaf → block | 8; only "leaf 0 at the south-west corner, x high" sets the observer's own block in all 6,285 cells (the others: 1,371–3,160) |
| Observer's own 4×4 block set | 6,285/6,285 |
| Root bit set | 6,285/6,285 |
| Set blocks whose nearest cell is beyond 100 cells from the observer | 0 |
| Farthest set block: distance to its nearest cell / its farthest cell | 100.0 / 104.2 cells (Euclidean, in cells) |
| Set 4×4 blocks per cell | min 40, max 1,529, mean 519.3 of 4,096 |
| Set blocks span | blocks x 0..61, y 0..56 = cells x −96..151, y −96..131 (grid −96..159) |
| Origin shifted by ±4/±8 cells | own block set drops to 6,185–6,274 (one block) and 5,250–5,976 (two); max Chebyshev block distance rises from 25 to 26–27; only (−96, −96) gives 6,285 / 25 |

The last row ties the bitmap to the lodsettings origin: the layout is only consistent with the
grid starting at (−96, −96). The stride fixes the number of levels (`4 << levels <= stride`).

**Corroboration** `[anvillod]`: AnvilLOD's `TvdtData.cs` (GPL-3.0-only, commit `606eece`,
`D:\Modlists\ref\AnvilLOD\src\AnvilLOD.Core\Occlusion\`) states that it decoded the field from
xLODGen's output, verified against xLODGen's debug images, and its `NodeBit` puts the 4,096 leaf
bits at exactly the positions found here (identical for 4,096/4,096 leaves `[check]`: the same
z-order with the y bit lowest, from the south-west cell of the `.lod` file, set = visible). It
describes the other 1,365 bits differently: 6 "lead bits" of unknown meaning (bits 0 and 1 always
set), the internal nodes in post-order after their children, then 5 zero bits. In the golden data
that description does not hold: under it the internal-node bits are not the OR of their children
(5,805 violations in 40 sampled cells), while under the pre-order description every one of the
8,579,025 relations holds and the "lead bits" are simply the root and the ancestors of the
south-west-most block. The debug images only show leaves, which explains the difference. **A port
of `TvdtData.cs` must therefore take the non-leaf bit layout from this section**; the engine reads
those bits for the LOD 8, 16 and 32 levels.

**Decoding/encoding, in words.** Walk the quadtree from the root; at each node consume one bit;
if the node is a 4×4 block, that bit is the block's visibility; otherwise descend into the
south-west, north-west, south-east and north-east children in that order. Encoding is the same
walk with each internal node's bit = OR of its children (so compute leaves first). For a grid
whose stride is not a power of two times four, nothing is known: the only observed stride is 256
(open question 10.1).

## 5. Inputs, worldspaces and cells `[golden]` `[doc]`

**What the reference reads**, from the run's main log (`DynDOLOD_SSE_log.txt`, 04:55–06:56):

```
[Tamriel] Generating Pre-Computed Occlusion TVDT
[Tamriel] Gathering CELL and LAND records
[Tamriel] Land height: -27000, Water height: -14000
[Tamriel] LODsettings: Level 4 to 32 stride 256 from -96,-96 to 0,0
[Tamriel] Building height map from 11186 cell records
[Tamriel] Updating height map from terrain LOD meshes
[Tamriel] Updating height map from object LOD heightmap
[Tamriel] Calculating occlusion for 5697 CELL records. Quality: 2 Adjust: 0 Mode: -Flat +Border
   0%: [CELL:00001896] [-33,39] water: -14000, min: -14000, max: -14000, top: -14000
 ...
[WindhelmWorld] Gathering CELL and LAND records
[WindhelmWorld] Land height: -27000, Water height: -14000
[WindhelmWorld] LODsettings: Level 4 to 32 stride 256 from -96,-96 to 0,0
[WindhelmWorld] Building height map from 103 cell records
[Tamriel] Updating height map from terrain LOD meshes
[Tamriel] Updating height map from object LOD heightmap
[WindhelmWorld] Calculating occlusion for 103 CELL records. Quality: 2 Adjust: 0 Mode: -Flat
 ...
Occlusion.esp completed successfully
```

So the inputs are, in this order: the worldspace's exterior CELL and LAND records (11,186 Tamriel
cells with terrain, the same count LODForge's `land_cells` finds and DynDOLOD exports to LODGen
as `LODGen_SSE_LODGen_Terrain_Tamriel.bin`, see the "Side-file formats" section of
[dyndolod-spec.md](dyndolod-spec.md)), the terrain LOD meshes (`.btr`), and the object LOD height
map of section 3. For every child worldspace the terrain LOD and object height map lines carry
the **main** worldspace's name: the children use Tamriel's meshes and height map, and their own
"cell records" count equals the number of cells they get TVDT for (103, 83, 143, 134, 63, 12,
50). The per-cell progress lines print four heights per observer cell (`water`, `min`, `max`,
`top`); their meaning is not documented, `[inferred]` water = the cell's water height, min/max =
terrain range, top = highest terrain-or-object point (checked over all 6,285 lines of the 8
worldspaces: top ≥ max and min ≤ max in every line; 458 lines, sea cells, have all four equal,
e.g. −14000). They are a validation signal, not a structure requirement.

**Land and water height per worldspace** (the log's `Land height: ..., Water height: ...` lines
against the WRLD records of the golden plugin and Skyrim.esm):

| Worldspace | PNAM | DNAM / NAM4 | Log |
|---|---|---|---|
| Tamriel | — | DNAM (−27000, −14000), NAM4 −14000 | −27000, −14000 |
| WindhelmWorld, WhiterunWorld, SolitudeWorld, WhiterunDragonsreachWorld | 0x7F | none | −27000, −14000 |
| WindhelmPitWorldspace | 0x5F | none | −27000, −14000 |
| RiftenWorld | 0x77 (no 0x08) | none | −27000, **0** |
| KatariahWorld | 0x57 (no 0x08) | none | −27000, **0** |

`[golden]` + `[xedit-src]`: a worldspace without DNAM takes the parent's land height when PNAM has
Use Land Data (0x01) and the parent's water height when it has Use Water Data (0x08); without
the water flag the water height is 0. Tamriel's DNAM water and NAM4 are equal, so whether NAM4
takes precedence is open (10.2). `apps/cli/src/terrain_underside.rs` also calls this
`world_heights(world, parent)` function `[ours]`.

**Which worldspaces** `[doc: readme]` `[golden]`: the selected worldspace and its child
worldspaces that use it for LOD = WRLD records whose `WNAM` is the selected worldspace and whose
`PNAM` has `0x02` (Use LOD Data). Skyrim.esm has 34 worldspaces with `WNAM = Tamriel`; exactly the
seven with the 0x02 flag are in the golden plugin, in this file order:

| # | Worldspace | Form id | PNAM | DATA | Cells with TVDT | Grid range x / y |
|---|---|---|---|---|---|---|
| 1 | Tamriel | 0000003C | — | 0x00 | 5,697 | −47..52 / −33..41 |
| 2 | WindhelmWorld | 0001691D | 0x7F | 0x01 (small) | 103 | −1..37 / −1..26 |
| 3 | RiftenWorld | 00016BB4 | 0x77 | 0x01 | 83 | −1..46 / −28..0 |
| 4 | WhiterunWorld | 0001A26F | 0x7F | 0x89 | 143 | −3..16 / −12..15 |
| 5 | SolitudeWorld | 00037EDF | 0x7F | 0x01 | 134 | −20..3 / −3..30 |
| 6 | WhiterunDragonsreachWorld | 00046033 | 0x7F | 0x01 | 63 | −1..10 / −4..4 |
| 7 | KatariahWorld | 000C350D | 0x57 | 0x43 | 12 | −13..0 / 0..29 |
| 8 | WindhelmPitWorldspace | 000D45F0 | 0x5F | 0x03 | 50 | 0..36 / 0..13 |

(The children appear in ascending form id order; MarkarthWorld, PNAM 0x44, has its own
lodsettings and is absent.) All seven children carry the Small World flag, and the debug log's
resolved defaults show `Occlusion_KatariahWorld: False`, `Occlusion_WhiterunDragonsreachWorld:
False`, `Occlusion_WindhelmPitWorldspace: False`, yet they were processed: children that use the
parent's LOD are done with the parent regardless of their own small-world default. Whether an
explicit `Occlusion_<Child>=0` skips a child is open (10.3).

**Which cells** `[doc]` `[cfg]` `[golden]`: every cell that gets TVDT has terrain (LAND with VHGT)
and lies in the grid; `-Flat` drops cells whose terrain is perfectly flat (all VHGT deltas 0);
`+Border` keeps only cells whose `XCLR` lists a region with the Border Region flag (and is not
applied to child worldspaces); `±TVDT` filters on the presence of an existing TVDT in the winning
record. The INI comment restricts updates further to "CELLs that are part of the dynamic LOD
Near/Far grids". The golden selection (5,697 + 588 cells) is the reference the implementer's
selection must reproduce; `lodforge occlusion --golden` lists the cells that differ. Note that
the terms are case-insensitive and that the golden Tamriel run used `-Flat +Border` while the
shipped global default adds `-TVDT`.

**What the output shows about visibility** `[check]`: see the table in section 4. In addition, from
coastal observers the set blocks reach 99 cells out over open sea (set blocks up to x = 151 while
the easternmost observer is at x = 52 and LAND records end at x = 64): sea beyond the terrain is
marked visible, not hidden. The cell with the fewest visible blocks has 40, the most 1,529 (the
full radius-100 disc would be about 1,963 blocks).

## 6. MHDT in the output `[golden]` `[check]`

- Every one of the 6,285 cells in the golden plugin has an `MHDT` of 1,028 bytes.
- All offsets are multiples of 128; they range from −13,952 to 36,224; the largest byte is 183.
- 225 of the cells are new records of DynDOLOD.esm (child-world cell copies) that have no MHDT in
  any plugin, so the reference computed theirs. Only 4 of those 225 equal the parent Tamriel
  cell's MHDT at the same grid, so MHDT is computed, not copied from the parent cell. For the
  other cells the winning CELL already had MHDT (DynDOLOD.esm 1,034, DynDOLOD.esp 24, the rest
  from the masters) and the plugin carries that record's MHDT.
- Unit: against the Skyrim.esm LAND of the same 225 grids, the rule `byte ≥ ceil((max terrain
  height of the quad − offset) / unit)` holds for 226,260 of 230,400 quads with unit 128 and
  bytes in x-major order (byte `k` ↔ quad `x = k div 32`, `y = k mod 32`; 79,579 quads equal),
  but only for 168,854 quads with unit 110, 50,983 with unit 8, and 168,091 with unit 128 in
  y-major order. **One byte = 128 game units above the offset, x-major.** The 4,140 quads below
  the terrain-derived minimum and the exact rule for the offset (85 of 225 offsets equal the
  terrain minimum rounded up to 128; the other 140 are higher, water being the likely reason)
  are open (10.4).

## 7. The plugin `[golden]` `[check]` `[ours]`

Header of the golden `Occlusion.esp` (4,061,170 bytes): `TES4` flags ESL (0x200) set, master
flag clear, Localized clear; `HEDR` version 1.71, record count 6,500, next object id 0x800;
`CNAM` `xLODGen`; `SNAM` `Generated plugin for TVDT occlusion data`; 12 `MAST` entries
(Skyrim.esm, Update.esm, Dawnguard.esm, HearthFires.esm, Dragonborn.esm, cceejsse002-tower.esl,
ccvsvsse004-beafarmer.esl, ccrmssse001-necrohouse.esl, cceejsse004-hall.esl, cceejsse005-cave.esm,
ccafdsse001-dwesanctuary.esm, DynDOLOD.esm: every plugin a copied record's form id or content
refers to); `INCC` 0.

Records: the 8 `WRLD` overrides and 6,285 `CELL` overrides, nothing else (HEDR's 6,500 = 6,293
records + 207 group headers: 1 top group, 8 world-children groups, 37 exterior blocks, 161
sub-blocks). The WRLD copy keeps the
winning override's subrecords (Tamriel: `EDID FULL CNAM NAM2 NAM3 NAM4 DNAM MNAM ONAM NAMA DATA
NAM0 NAM9 ZNAM TNAM UNAM OFST`) but **not `RNAM` and not `MHDT`**; `OFST` is present (xEdit
rewrites it on save). Because the plugin is not flagged Localized, copied `FULL` strings are plain
text. Each CELL is a full copy of the winning override (the run's own DynDOLOD.esm and
DynDOLOD.esp are loaded last and win), with `TVDT` replaced and `MHDT` added when the copied
record had none; subrecords are in xEdit's definition order in 6,285/6,285 cells. Copied records
keep the source record's compressed flag (Tamriel 5,697/5,697 compressed; children partly), and
xEdit's compressed records are libdeflate level 12 (`docs/baseline.md`, P7 step 6). Groups are the
usual world → exterior block → sub-block layout.

LODForge's plugin writer (`crates/dyndolod/src/plugin.rs`, `PluginBuilder`, `WorldContent`,
`CellContent`, `Rec::copy`) is LODForge's own code and already reproduces this file byte for byte
when given the same content (`docs/baseline.md`, "Byte-identical Occlusion.esp (2026-10-18)").
The re-implementation should keep using it; the occlusion module's job is the content: which
worldspaces and cells, the TVDT bytes and the computed MHDT bytes.

## 8. LODForge's observable contract `[ours]`

**`lodforge occlusion`** (`apps/cli/src/occlusion.rs`; `Args` as of 2026-10-19, keep these flags):

| Flag | Meaning |
|---|---|
| `--instance <dir>` | MO2 instance: load order, terrain LOD meshes, lodsettings, strings |
| `--profile <name>` | default `LODForge-DynDOLOD` |
| `--world <EditorID>` | the main worldspace, default `Tamriel`; its child worldspaces follow |
| `--extra-plugin <path>` (repeatable) | plugins DynDOLOD has loaded but the profile has not: the run's `DynDOLOD.esm` / `DynDOLOD.esp`. Master-flagged files go after the last master of the load order, others at the end, so DynDOLOD.esp's CELL overrides win |
| `--no-child-worlds` | only the main worldspace |
| `--only-worlds a,b` | restrict to these EditorIDs |
| `--heightmap <file>` | the main worldspace's `<World>.objects.heightmap.dat` |
| `--terrain-bin <file>` | optional diagnostic: DynDOLOD's `LODGen_SSE_LODGen_Terrain_<World>.bin`; prints `terrain from the load order: N cells, S identical to the golden .bin's G` |
| `--golden <Occlusion.esp>` | compare with a golden plugin (see output below) |
| `--golden-cells` | compute the golden plugin's cells instead of selecting by `--mode` |
| `--mode "<terms>"` | `OcclusionMode`, default `-Flat +Border`; children get it without `+Border` |
| `--out <Occlusion.esp>` | write the plugin |
| `--limit N`, `--cells "x,y;x,y"` | subsets for quick checks |
| `--quality 2`, `--radius 100`, `--height 0.0` | the three numeric settings |
| `--origin x,y`, `--stride N` | override the grid (default: the worldspace's lodsettings, else the main worldspace's, else −96,−96 / 256) |
| `--land-height f` | override the worldspace's default land height |
| `--show N` | print block maps (ours vs golden) of the N most different cells per worldspace |

The current file also has `--init-water` and a `LODFORGE_OCC_NODES` debug hook; both expose the
old implementation's internals and are **dropped**.

Output lines (the parity script greps the `all worldspaces` line):

```
<World>: <N> land cells, mode "<mode>", <M> cells, land <L> water <W>, <t> s
  cells only ours (<n>) [...]          (with --golden, when the selections differ)
  cells only golden (<n>) [...]
  vs golden: <same>/<cells> cells identical (golden has <k>), bit Jaccard <j> (set bits: both <b>, only ours <o>, only golden <g>)
total <t> s
all worldspaces vs golden: <same>/<cells> cells identical, bit Jaccard <j>
wrote <path> (<bytes> bytes, <n> worldspaces), masters [...]
```

The Jaccard figure is over set bits of the whole 684-byte field; "only ours / only golden" are
the visible-block deltas the acceptance criteria ask for.

**`lodforge build --mode dyndolod`** (`apps/cli/src/dyndolod_build.rs`, `write_occlusion`): after
terrain LOD, object LOD and the plugins, for every selected main worldspace that is not itself a
LOD-child of another selected one, it reads `OcclusionQuality` (2), `OcclusionRadius` (100),
`OcclusionHeight` (0) and `OcclusionMode_<World>` (else `OcclusionMode`, else `-Flat +Border`)
from the DynDOLOD INI it was given, skips a child when `Occlusion_<Child>=0`, takes the height map
from `<out>\meshes\terrain\<World>\Objects\<World>.objects.heightmap.dat`, loads the build's own
`DynDOLOD.esm`/`DynDOLOD.esp` from `<out>` as extra plugins, and writes `<out>\Occlusion.esp`.
`--no-occlusion` skips the stage. The stage reports `(worldspace, cells)` per worldspace.

**Functions other modules call** (keep the behaviour; signatures may change together with the
callers): `occlusion::land_cells(load_order, worldspace, default_water)` = the winning LAND
`VHGT` per exterior cell of a worldspace with the cell's water height (XCLW, else the default),
used by `dyndolod_build.rs`; `occlusion::world_heights(worldspace, parent)` = (land height, water
height) as in section 5, used by `terrain_underside.rs`.

**Parity harness** `scripts/golden/dyndolod/Test-OcclusionParity.ps1`: (1) copies the golden
export file with `TerrainData=` pointed at the golden run's saved `export\` folder; (2) runs
`lodforge object-lod --flavor dyndolod --level 4` to produce `Tamriel.objects.heightmap.dat`;
(3) runs `lodforge occlusion` with that height map, the golden run's DynDOLOD.esm/.esp and
`--golden`, and parses `all worldspaces vs golden: N/M cells identical`; (4) compares the two
plugins byte for byte; (5) "writer gate": the same run with DynDOLOD's own height map must be
byte-identical. It writes `bench\occlusion-test\summary.json` and fails when TVDT identical cells
< `-MinTvdtIdentical` (default 6,283), when the writer gate fails, or with `-RequireIdentical` when
the files differ. Steps 4 and 5 encode the old bit-exact policy; under the plan's tolerance policy
the implementer records the new numbers and changes the gates to the documented tolerance.

## 9. Acceptance criteria (from the plan)

- Record the TVDT-cell equality count against the golden (was 6,285/6,285) and the visible-block
  delta per worldspace (bit Jaccard, set bits only ours / only golden, from `lodforge occlusion
  --golden`), before and after.
- Accept a documented tolerance: an output-fitted model is expected to differ by a few percent of
  blocks. Exactness is not the goal if it is unreachable without the decompile.
- In-game check: no LOD holes from ground vantage points in Tamriel and Solstheim (Solstheim has
  no golden; generate it and look).
- The CLI and `build` surface of section 8 stay, so nothing else moves; `Test-OcclusionParity.ps1`
  keeps running with its thresholds adjusted to the recorded tolerance.

## 10. Open questions for the implementer (settle against the golden files)

1. Grids whose stride is not 256: the number of levels is `4 << levels <= stride` by
   construction; whether a non-power-of-two stride ever occurs is unknown. Only Tamriel's 256 is
   observed; DLC2SolstheimWorld's lodsettings should be read before generating it.
2. Water height precedence when `NAM4` (LOD water height) and `DNAM` differ, and the water height
   of worldspaces flagged No LOD Water (0x08) or without DNAM and without a parent.
3. `OcclusionMode` details beyond the public terms: whether `+Border` falls back to all cells when
   no border region exists; whether `All` is a literal term; whether an explicit
   `Occlusion_<Child>=0` skips a LOD-child; whether cells outside the "dynamic LOD Near/Far
   grids" (INI comment) are excluded by something other than terrain presence.
4. MHDT computation for cells without one: the exact offset rule (water involvement), whether
   object LOD tops raise the bytes, and the ordering of samples within a quad; validate on the
   225 computed golden cells (`Check-TvdtLayout.py --skyrim-esm` reports the fit).
5. The visibility model itself: how "quality = samples per cell" is laid out, which height the
   observer and the target use (terrain vs. object top, plus `OcclusionHeight`), how a block
   farther than the radius is treated (observed: no set block has its nearest cell beyond 100.0
   cells; set blocks reach 104.2 cells to their far corner), and what height cells without
   terrain or object LOD have (observed: open sea is visible to the horizon). Fit to the golden
   with `--show` and the Jaccard figures; the tolerance policy applies.
6. Whether reading the terrain LOD meshes (`.btr`) changes the result for a load order whose LAND
   records cover the playable area; the reference reads them (section 5), the readme says they
   fill cut content.
7. Child worldspaces: whether a child's own LAND or the parent's LAND at the same grid is used
   when both exist (DynDOLOD.esm's child-world cell copies have no LAND of their own).
8. Quality 3 and `OcclusionHeight ≠ 0`: no golden exists; keep the flags, validate quality 2.

## 11. Process record

- **Date:** 2026-10-19, analyst session started from the LODForge licensing session; shared
  checkout `D:\Modlists\LODForge` on `main`, nothing committed.
- **Read for this document:** `docs/reimplementation-plan.md`, `docs/licensing.md`, the former
  "Occlusion" section of `docs/re/dyndolod-spec.md` (to decide what to exclude), the xLODGen
  readme, the dyndolod.info page, the UESP CELL/WRLD pages, the shipped `DynDOLOD_SSE.ini`, the
  xEdit sources listed in the tag table, the golden run's plugins, logs, export and option files,
  `lodsettings\tamriel.lod` (via `lodforge vfs-extract`), Skyrim.esm's WRLD, CELL and LAND
  records (scratch scripts), `apps/cli/src/occlusion.rs`, `apps/cli/src/dyndolod_build.rs`,
  `apps/cli/src/terrain_underside.rs` (callers, for section 8), the public item names and doc
  comments of `crates/dyndolod/src/occlusion.rs` (to list the callers' needs; nothing from its
  body is in this document), `scripts/golden/dyndolod/Test-OcclusionParity.ps1` and
  `docs/baseline.md` P7 step 6.
- **Excluded and quarantined:** every sentence of the old section that needed the decompile:
  the node data structure and its initial values, the height-map build rules, the ray-stepping
  rule, the order in which child worldspaces reuse the parent's map, the exact MHDT arithmetic,
  the exact cell-selection terms not in the public documentation, Ghidra addresses. Copied
  verbatim to `D:\Modlists\ref\quarantine\dyndolod-occlusion-decomp.md` (outside the repository)
  and replaced in `dyndolod-spec.md` by a pointer to this file.
- **Not used for the derivation:** any decompile under `D:\Modlists\ref\`; AnvilLOD. Section 4
  and the check script were finished from the golden plugin alone. Afterwards, when the licensing
  session lifted the AnvilLOD restriction (below), the header and `NodeBit` of its `TvdtData.cs`
  were read for the corroboration paragraph of section 4; nothing else of AnvilLOD was read.
- **Rule for the implementing session** (set by the licensing session on 2026-10-19, after the
  maintainer chose GPL-3.0-only for LODForge): never read `crates/dyndolod/src/occlusion.rs`
  (delete it unread as the first action), the quarantine file
  `D:\Modlists\ref\quarantine\dyndolod-occlusion-decomp.md`, or any decompile or Ghidra material
  under `D:\Modlists\ref\` (`lodgen-*`, `skyrimse-*`, REA snapshots). AnvilLOD
  (`D:\Modlists\ref\AnvilLOD`, commit `606eece`, GPL-3.0-only) is an allowed reference under the
  GPL-3.0-only project license: its `src\AnvilLOD.Core\Occlusion\` files may be read, and where
  their expression is carried over the file must carry the upstream copyright notice
  ("Copyright (c) 2026 CageTV"), the GPL-3.0-only terms and marked modifications. See the caveat
  on `TvdtData.cs`'s non-leaf bits in section 4. The implementer must also not read the **body**
  of `apps/cli/src/occlusion.rs` beyond its `Args` struct (its `world_map`, `occlusion_worlds`,
  `Params::init_water` and `LODFORGE_OCC_NODES` code carry the old module's internals; rewrite
  the glue from section 8), nor the "What made it exact" bullet of `docs/baseline.md` P7 step 6,
  which repeats three quarantined facts. Validation is against the golden files and the harness
  only.
- **Check script:** `scripts/golden/dyndolod/Check-TvdtLayout.py` (standard library, run with
  `python3 -I`), output recorded in sections 4 and 6.

## 12. Implementation (2026-10-20): `crates/dyndolod/src/occlusion/`

Written by the implementing session from sections 1–10, the golden files and AnvilLOD's three
occlusion files; nothing of the old module was read (section 13). The module is six files:
`mod.rs` (inputs, worldspaces, cell selection, the per-worldspace setup), `grid.rs` (the height
map), `lodmesh.rs` (terrain LOD meshes as heights), `visibility.rs` (the line-of-sight model),
`tvdt.rs` (the bit layout of section 4) and `content.rs` (the plugin content of section 7).
`apps/cli/src/occlusion.rs` is the rewritten glue of section 8 (same flags; `--init-water` and
the old debug hook dropped; `--dump-heights`, `--analyze`, `--probe` added as diagnostics; the
`LODFORGE_OCC_MODEL` and `LODFORGE_OCC_DUMP_CELLS` environment variables are model-fitting
hooks). Everything the module decides that the sections above left open is listed in 12.1.

**Height map** (`grid.rs`). 32 × 32 samples per cell at 128 game units: sample (i, j) of a
cell is landscape vertex (i, j) (the 33rd row and column belong to the neighbour), clamped to the
cell's water height (XCLW of the winning CELL when it holds a real height, none for the vanilla
cells whose DATA is one byte, else the worldspace default); the `top` layer is that ground raised
by the object LOD height map's point (i, j). Verified against the golden log's per-cell
`water/min/max/top` lines: identical in 100/100 sampled Tamriel cells and 588/588 child cells,
where the max-of-quad sampling tried first matched only 37/100 (its minimum was too high, its
maximum included the shared edge).

**Inputs beyond the load order.** The main worldspace's LOD4 terrain meshes fill every cell
without landscape (the sea floor), the LOD water planes in them included (the sea surface at
−14000): without the water planes the sea beyond the coast is hidden from every observer, with
them it is seen as the golden shows (10.6). The map covers the meshes' extent plus one 4-cell
block on each side, unknown cells inside it at the default water height, and everything outside
it reads as height 0 with no clearance (10.5). The object LOD height map is the main
worldspace's for every child.

**Visibility** (`visibility.rs`, fitted; 10.5). Quality q gives 2^(q−1) sample points per cell
side (q = 2: 2 × 2), each at the highest `top` sample of its sub-area. A block is visible when
some observer sample point (plus `OcclusionHeight`) has a straight line to some sample point of
some cell of the block that no ground sample on the way rises more than the clearance above;
objects are seen, not seen through (with the `top` layer as occluder the fit is far worse: block
Jaccard 0.51 against 0.72 at clearance 0 on the same 100 cells). Target cells farther than the
radius (Euclidean, integer cell grid) are skipped, the observer's own block is always set, the
levels above the blocks are the OR of their children. Clearance 1,250 units, fitted on 300
Tamriel cells (block Jaccard 0.867 at 750, 0.873 at 1,000, 0.870 at 1,250, 0.862 at 1,500; 1,250
hides fewer of the golden's blocks than 1,000 at the price of more extra draws). AnvilLOD fitted
the same kind of constant (3,072, with sample points at sub-area centres and no water planes).

**MHDT** (section 6, 10.4): see 12.1 item 4.

**Plugin** (`content.rs`): the winning WRLD without RNAM and MHDT, the winning CELL of every
selected cell with the TVDT replaced or added and an MHDT added when the record has none,
subrecords in xEdit's CELL order, localized FULL strings replaced by the text from the plugin's
`_english.strings` table. Author `LODForge` (the old module wrote the reference tool's name to be
byte-identical; the description `Generated plugin for TVDT occlusion data` is kept as the string
other tools recognise an occlusion plugin by). `Check-TvdtLayout.py` passes on the written
plugin (pre-order layout, parent = OR of children in 7,835,100/7,835,100 relations, own block
set 5,740/5,740, no block beyond the radius, spare bits 0).

### 12.1 Section 10 settled

1. **Stride.** `Layout::for_stride` accepts a stride that is a power of two times four and
   derives the levels from it; the field is the stream rounded up to whole bytes plus one spare
   byte (684 for 256). Any other stride is an error naming the worldspace. A worldspace's own
   lodsettings file is read through the VFS (DLC2SolstheimWorld was not generated in this
   session, 12.3).
2. **Water height.** `DNAM`'s water, the parent's when `PNAM` has Use Water Data (0x08) or the
   worldspace has no `DNAM`; 0 for a worldspace without `DNAM` and without the flag
   (RiftenWorld, KatariahWorld, as the golden log prints); `NAM4` is not consulted (Tamriel's
   equals `DNAM`, nothing else was observed); a No LOD Water worldspace (DATA 0x08) has no
   default water.
3. **Mode.** `All` clears the other terms; `+Border` with no cell in a Border Region is dropped
   with a warning in `build` (the readme's "do not use" advice applied for the user); an
   explicit `Occlusion_<Child>=0` skips a LOD child in `build` (kept from the old glue; the
   golden has no case either way). No "Near/Far grid" filter was needed: `-Flat +Border`
   (Tamriel) / `-Flat` (children) reproduces the golden's cell sets exactly (cells only ours 0,
   only golden 0 in all eight worldspaces).
4. **MHDT.** Offset = the lowest `top` sample of the cell (ground with its water, raised by
   the object LOD) rounded **up** to a multiple of 128; byte `x * 32 + y` = ceil((highest of
   the four `top` corners of quad (x, y) − offset) / 128), x-major. Fit on the 279 cells of the
   golden whose winning record in this load order has no MHDT (the 225 of section 6 plus 54
   DynDOLOD.esp overrides): offsets identical in 242, bytes identical in 156,261 of 285,696
   (55%), the golden one step higher in most of the rest (92,479 higher, 36,956 lower). The
   alternatives measured: ground minimum instead of `top` 214 offsets (the walled-city offsets
   lie above the terrain because the object LOD covers it), rounding to nearest 165, the
   minimum over quad maxima 238; rounding the bytes to nearest 143k equal. Whether the golden's
   quads reach one sample further is not resolved; the field is a best effort on both sides
   (section 1).
5. **Visibility model and the void.** See section 12. Cells without landscape inside the
   terrain LOD meshes' extent are the meshes (sea floor) with the LOD water planes over them;
   cells beyond the meshes plus one block read as height 0: measured on the golden over all
   6,285 cells, observers with `top` < 0 set 0 of 400,485 far-void blocks within the radius
   against 10,236 of 23,768 for tops in 0..2000, while the block row and column just beyond the
   vanilla meshes (cells 96..99) is set from sea-level observers like the sea next to it (75%).
   A radius on the block centre or on the sample points was tested and rejected: over 525k set
   blocks the nearest-cell distance peaks at exactly 100.000 while centre distances reach 102.1.
   Model variants tried and rejected on 100 cells (block Jaccard with the `top` occluder, void at
   −14000): box targets at the block's maximum 0.56, skipping the observer's own cell 0.55, both
   0.61, ray samples every 16 vertices 0.67, sub-area maxima as occluders 0.45, eye raised by
   512/2048: 0.67/0.77, clearance 500/1000/2000/3000/4000 with the `top` occluder:
   0.71/0.77/0.81/0.77/0.71; the ground occluder alone 0.72, with clearance 1000 0.83.
6. **Terrain LOD meshes.** Needed: they are the only height source for the sea beyond the last
   LAND record and for the sea surface over it (12); cells with landscape are never changed.
7. **Child worldspaces.** A child's cells are its exterior CELL records; each takes its own LAND
   when it has one, else the parent's LAND at that grid with the child's cell water (the seven
   golden children inherit their land data, `PNAM` 0x01, and have no LAND of their own: with own
   LAND only they got 2, 12, 7, 21, 0, 0 and 1 cells). The map is those cells, the parent's LAND
   elsewhere, the parent's meshes and object height map. Validation: cell sets identical to the
   golden's in all seven, heights identical 588/588.
8. **Quality 3, `OcclusionHeight`.** Implemented as 4 × 4 sample points and an eye offset; no
   golden, not validated.

### 12.2 Numbers (golden run of section 1)

Before: 6,285/6,285 TVDT cells identical (old module). After: 0/6,285 identical; the
tolerance figures below. "Hidden by us" = LOD4 blocks the golden shows that we hide (LOD holes
in game), "ours only" = blocks we show that the golden hides (extra draws), both as a share of
the golden's visible blocks. Computed with the golden run's own object height map (the parity
harness repeats the run with LODForge's, 12.2.1); quality 2, radius 100, height 0; 16 threads.

| Worldspace | Cells (ours = golden) | Block Jaccard | Hidden by us | Ours only | Bit Jaccard | Time |
|---|---|---|---|---|---|---|
| Tamriel | 5,697 | 0.877 | 150,588 (4.99%) | 251,771 (8.34%) | 0.886 | 668 s |
| WindhelmWorld | 103 | 0.855 | 3,279 (7.15%) | 3,975 (8.66%) | 0.866 | 10 s |
| RiftenWorld | 83 | 0.834 | 1,182 (7.95%) | 1,545 (10.39%) | 0.854 | 5 s |
| WhiterunWorld | 143 | 0.856 | 3,570 (6.58%) | 4,987 (9.19%) | 0.864 | 12 s |
| SolitudeWorld | 134 | 0.899 | 3,749 (4.58%) | 5,074 (6.20%) | 0.905 | 14 s |
| WhiterunDragonsreachWorld | 63 | 0.845 | 709 (3.90%) | 2,507 (13.78%) | 0.853 | 6 s |
| KatariahWorld | 12 | 0.630 | 305 (3.03%) | 5,426 (53.83%) | 0.658 | 3 s |
| WindhelmPitWorldspace | 50 | 0.852 | 1,233 (6.22%) | 2,005 (10.12%) | 0.863 | 6 s |
| children together | 588 | 0.854 | 14,027 (5.73%) | 25,519 (10.42%) | 0.864 | 56 s |

Set LOD4 blocks per Tamriel cell: ours min 41, max 1,503, mean 547.4 (golden 40 / 1,529 /
519.3). The Tamriel time is the per-cell visibility search (4 eyes × up to 64 targets per block,
every sample on the line); the reference took 25 min for the same cells on the same machine.
KatariahWorld (a ship at sea, water 0) is the one outlier: we show five times more sea than the
golden from its 12 cells.

**Tolerance adopted** (section 9): hidden by us ≤ 8% and ours only ≤ 12% of the golden's visible
blocks over all worldspaces, cell selection identical; `Test-OcclusionParity.ps1` gates on these
(`-MaxHiddenPercent`, `-MaxExtraPercent`) and records byte identity without requiring it.

#### 12.2.1 Parity harness (LODForge's own object height map)

`Test-OcclusionParity.ps1` (2026-10-20, `bench\occlusion-test\summary.json`): PASS. TVDT 0/6,285
identical; visible LOD4 blocks over all eight worldspaces: golden 3,263,954, ours 3,376,625,
hidden by us 164,616 (5.04%), ours only 277,287 (8.50%), block Jaccard 0.8752; cells only ours
0, only golden 0; the same run with DynDOLOD's own height map: hidden 5.04%, extra 8.50% (the
object height map port changes 4 blocks); Occlusion.esp 4,059,328 bytes against the golden's
4,061,170; 1,303 s for the object LOD pass and two occlusion passes.

### 12.3 Pending

- **In-game check** (section 9): done 2026-10-20, see the end of this section. Original note:
  not done in the implementation session. The game was not launched (the
  maintainer had just killed a locked-up game, and the check needs a profile that loads a LOD
  output together with `bench\Occlusion-full.esp` / `bench\Occlusion-Solstheim.esp` last). To
  do: install the golden run's output (or a `build --mode dyndolod` output) with this
  Occlusion.esp in a test profile, then `tll` / `tfc` sweeps from ground vantage points
  (Whiterun plains looking at the Throat of the World, the Solitude arch, Windhelm from the
  bridge; Raven Rock and the Earth Stone on Solstheim) for rectangular LOD holes, as the xLODGen
  readme describes; the 5% of golden-visible blocks we hide are the candidates for holes.
- **Solstheim** was generated as a smoke test only: 34,383 LAND cells, `-Flat +Border` selects
  599 cells (x 0..23, y −1..27), grid (−64, −64) stride 256 from its lodsettings, 48 s, no object
  height map (none in the golden), `Check-TvdtLayout.py --origin -64,-64` PASS.
- **Performance**: Tamriel's 5,697 cells take 11 min at quality 2 (the reference: 25 min).
  Bidirectional marching or a per-eye horizon would cut the hidden-block cost; not done.
- **`THIRD-PARTY-NOTICES`** entry for the AnvilLOD-derived files: with the license mechanics
  stage of the plan.

#### In-game check (2026-10-20): no LOD holes seen

Done in this session (one launch, 11:42-12:02 UTC). Result: **no rectangular patch of missing
terrain or object LOD at any vantage point**, Tamriel or Solstheim, including four cells where
this module hides a LOD4 block only 2-3 cells away that the golden shows. The first "Pending"
bullet above is closed; nothing was compared against the golden `Occlusion.esp` because no hole
appeared.

Setup. skyrim-cs profile `LODForge-Occlusion-Reimpl-Test` (`mo2_create_test_profile` from
`LODForge-Full-Golden-20261019`), three mods on top: `LODForge Golden DynDOLOD 20261017` (the
golden run's `output\` minus `Occlusion.esp`: DynDOLOD.esm/.esp, meshes, textures, SKSE
DynDOLOD_Data, Seasons INIs; 7,185 files, 2.1 GB), `LODForge Occlusion Reimpl - Tamriel`
(`bench\Occlusion-full.esp`) and `LODForge Occlusion Reimpl - Solstheim`
(`bench\Occlusion-Solstheim.esp`); `LODForge E2E - DynDOLOD Output` disabled, the golden
`Occlusion.esp` deactivated. Runtime load order (`game_query plugins`): DynDOLOD.esm `10`,
DynDOLOD.esp `1A`, then the light plugins with `Occlusion-full.esp` at `FE:050` and
`Occlusion-Solstheim.esp` at `FE:051`, both last. Game 1.7.104, SKSE 2.3.1, AgentBridge; new
game via `coc WhiterunBanneredMare`, Survival Mode prompt answered No, `tgm`, `killall`, then
`cow <world> x y` per vantage cell. Captures: `game_screenshot` preset clean (HUD off,
SkyrimClear, noon, AI frozen), engine PNG 1280x720 with a JSON sidecar each, free camera
("tfc") at `eye` = the player's position + 180 units with the player left in the vantage cell
so that cell's TVDT applies. `tll` was not used: the B1 session reported the game exiting on
it, and this check had one launch.

Files: `bench/occlusion-ingame-20261020/` (git-ignored; the same folder is copied to the main
checkout's `bench/`), `NN-*.png` + `NN-*.json`, plus `tvdt-diff-golden-vs-ours.txt` and
`tvdt_diff.py` (below). `diag-*.png` are orientation shots; two of them show the world map
because the Map Menu was open in the game window at the time, and they are not evidence.

| capture | world | player cell | eye (x, y, z) | pitch / yaw | fov | what is in the frame |
|---|---|---|---|---|---|---|
| `01-whiterun-plains-east-wide.png` | Tamriel | (-1,-4) Wilderness | (-1925, -14697, -5390) | -2 / 100 | 90 | first try at cell (-1,-4)/(-1,-3): a round tower and the ridge south of the tundra fill the frame; diagnostic only |
| `02-whiterun-plains-throat.png` | Tamriel | (-1,-4) Wilderness | (-1925, -14697, -5390) | -4 / 120 | 60 | same spot; diagnostic only |
| `03-whiterun-plains-east-wide.png` | Tamriel | (-4,-5) Wilderness | (-15178, -19748, -6160) | -3 / 92 | 100 | tundra to the horizon, Western Watchtower, mountains behind in haze; no gap |
| `04-whiterun-plains-city.png` | Tamriel | (-4,-5) Wilderness | (-15178, -19748, -6160) | -3 / 72 | 65 | Whiterun's direction; tundra continuous to the haze limit; no gap |
| `05-whiterun-plains-throat.png` | Tamriel | (-4,-5) Wilderness | (-15178, -19748, -6160) | -5 / 112 | 55 | foot of the southern ridge in front; no gap |
| `06-solitude-docks-arch.png` | Tamriel | (-16,24) SolitudeDocks02 | (-63359, 101040, -13000) | -12 / 336 | 80 | cliff under the arch fills the frame; no gap |
| `07-solitude-docks-bay-northeast.png` | Tamriel | (-16,24) SolitudeDocks02 | (-63359, 101040, -13000) | -2 / 25 | 90 | cliff, a sliver of the bay; no gap |
| `08-solitude-docks-coast-east.png` | Tamriel | (-16,24) SolitudeDocks02 | (-63359, 101040, -13000) | -2 / 85 | 80 | the arch frames the bay, the east shore and far mountains; no gap |
| `09-solitude-arch-from-bay.png` | Tamriel | (-16,24) SolitudeDocks02 | (-59800, 101600, -13000) | -8 / 268 | 80 | cliff and docks from the water side; no gap |
| `10-windhelm-bridge-north-city.png` | Tamriel | (33,7) WindhelmBridge03 | (135245, 32267, -12380) | -5 / 0 | 80 | inside the gatehouse; wall |
| `11-windhelm-bridge-northwest.png` | Tamriel | (33,7) WindhelmBridge03 | (135245, 32267, -12380) | -4 / 328 | 70 | inside the gatehouse; wall |
| `12-windhelm-bridge-east.png` | Tamriel | (33,7) WindhelmBridge03 | (135245, 32267, -12380) | -2 / 96 | 80 | inside the gatehouse; wall |
| `13-windhelm-bridge-south.png` | Tamriel | (33,7) WindhelmBridge03 | (135245, 32267, -12380) | -2 / 180 | 80 | along the bridge to the stables, mountains behind; no gap |
| `14-windhelm-bridge-span-north-city.png` | Tamriel | (33,7) WindhelmBridge03 | (135400, 29800, -12300) | -5 / 0 | 80 | bush in front, city walls behind; no gap |
| `15-windhelm-bridge-span-northwest.png` | Tamriel | (33,7) WindhelmBridge03 | (135400, 29800, -12300) | -3 / 300 | 80 | upriver, snowy mountains; no gap |
| `16-windhelm-bridge-span-east.png` | Tamriel | (33,7) WindhelmBridge03 | (135400, 29800, -12300) | -2 / 100 | 80 | mountains across the river; no gap |
| `17-windhelm-bridge-span-city-gate.png` | Tamriel | (33,7) WindhelmBridge03 | (135550, 31000, -12250) | -6 / 358 | 75 | gatehouse wall; diagnostic only |
| `18-rift-road-south-riften.png` | Tamriel | (42,-20) RiftenAttackStart01 | (175014, -78056, 8340) | -2 / 185 | 80 | forest and rocks close in; no gap, short range |
| `19-rift-road-southwest-lake.png` | Tamriel | (42,-20) RiftenAttackStart01 | (175014, -78056, 8340) | -3 / 204 | 80 | rocks close in; no gap, short range |
| `20-rift-road-southeast.png` | Tamriel | (42,-20) RiftenAttackStart01 | (175014, -78056, 8340) | -3 / 133 | 80 | forest; no gap, short range |
| `21-rift-road-northwest.png` | Tamriel | (42,-20) RiftenAttackStart01 | (175014, -78056, 8340) | -3 / 299 | 80 | forest, ruin; no gap, short range |
| `22-honrich-north-shore-south.png` | Tamriel | (38,-21) POIFallForest30 | (159520, -84770, 11620) | 0 / 180 | 85 | Lake Honrich, far shore buildings; no gap |
| `23-honrich-north-shore-southwest.png` | Tamriel | (38,-21) POIFallForest30 | (159520, -84770, 11620) | -2 / 220 | 85 | lake and far shore; no gap |
| `24-honrich-north-shore-riften.png` | Tamriel | (38,-21) POIFallForest30 | (159520, -84770, 11620) | 1 / 130 | 80 | lake toward Riften; no gap |
| `25-raven-rock-east.png` | DLC2SolstheimWorld | (6,8) DLC2RavenRock03 | (28520, 33090, 530) | -2 / 90 | 85 | docks, the Bulwark blocks the east; no gap |
| `26-raven-rock-northeast.png` | DLC2SolstheimWorld | (6,8) DLC2RavenRock03 | (28520, 33090, 530) | -3 / 40 | 85 | town and the hills behind; no gap |
| `27-raven-rock-southeast-coast.png` | DLC2SolstheimWorld | (6,8) DLC2RavenRock03 | (28520, 33090, 530) | -1 / 125 | 85 | Bulwark; no gap |
| `28-raven-rock-bulwark-east.png` | DLC2SolstheimWorld | (6,8) DLC2RavenRock03 | (33800, 35300, 1100) | -1 / 90 | 85 | eye inside the Bulwark; diagnostic only |
| `29-raven-rock-bulwark-northeast.png` | DLC2SolstheimWorld | (6,8) DLC2RavenRock03 | (33800, 35300, 1100) | -2 / 45 | 85 | eye beside the Bulwark; diagnostic only |
| `30-raven-rock-east-gate-east.png` | DLC2SolstheimWorld | (8,8) Wilderness | (32802, 35121, 1080) | -2 / 90 | 85 | under a plaza roof; diagnostic only |
| `31-raven-rock-east-gate-northeast.png` | DLC2SolstheimWorld | (8,8) Wilderness | (32802, 35121, 1080) | -3 / 40 | 85 | under a plaza roof; diagnostic only |
| `32-raven-rock-east-gate-north.png` | DLC2SolstheimWorld | (8,8) Wilderness | (32802, 35121, 1080) | -4 / 0 | 85 | under a plaza roof; diagnostic only |
| `33-raven-rock-east-gate-west-town.png` | DLC2SolstheimWorld | (8,8) Wilderness | (32802, 35121, 1080) | -2 / 270 | 85 | town from the plaza; no gap |
| `34-ash-wastes-east-of-raven-rock-east.png` | DLC2SolstheimWorld | (10,8) DLC2TestAshTrees | (42654, 35188, 3110) | -2 / 90 | 85 | hills east; no gap |
| `35-ash-wastes-east-of-raven-rock-northeast.png` | DLC2SolstheimWorld | (10,8) DLC2TestAshTrees | (42654, 35188, 3110) | -3 / 40 | 85 | ash hills in the distance; no gap |
| `36-ash-wastes-east-of-raven-rock-west-town.png` | DLC2SolstheimWorld | (10,8) DLC2TestAshTrees | (42654, 35188, 3110) | -1 / 270 | 85 | toward Raven Rock, far hills and sea; no gap |
| `37-ash-wastes-east-of-raven-rock-south.png` | DLC2SolstheimWorld | (10,8) DLC2TestAshTrees | (42654, 35188, 3110) | 0 / 180 | 85 | coast; no gap |
| `38-earth-stone-north.png` | DLC2SolstheimWorld | (5,7) DLC2StoneEarth | (23459, 31375, 786) | -3 / 0 | 85 | rock spire, sea to the left; no gap |
| `39-earth-stone-northeast.png` | DLC2SolstheimWorld | (5,7) DLC2StoneEarth | (23459, 31375, 786) | -3 / 45 | 85 | Raven Rock across the bay, hills; no gap |
| `40-earth-stone-east-raven-rock.png` | DLC2SolstheimWorld | (5,7) DLC2StoneEarth | (23459, 31375, 786) | -2 / 90 | 85 | the Bulwark and the mountain behind; no gap |
| `41-earth-stone-southwest-sea.png` | DLC2SolstheimWorld | (5,7) DLC2StoneEarth | (23459, 31375, 786) | 0 / 225 | 85 | the stone itself; no gap |
| `42-diff-cell-35-6-toward-block-32-0-wide.png` | Tamriel | (35,6) WindhelmAttackStart04 | (146635, 25585, -9296) | 0 / 199 | 100 | ridge directly in the look direction hides the block; no gap |
| `43-diff-cell-35-6-toward-block-32-0-narrow.png` | Tamriel | (35,6) WindhelmAttackStart04 | (146635, 25585, -9296) | -2 / 199 | 55 | same; no gap |
| `44-diff-cell-21-0-toward-block-24-0-wide.png` | Tamriel | (21,0) Wilderness | (87507, 1383, -9218) | 0 / 67 | 100 | White River gorge wall in the look direction; no gap |
| `45-diff-cell-21-0-toward-block-24-0-narrow.png` | Tamriel | (21,0) Wilderness | (87507, 1383, -9218) | -2 / 67 | 55 | same; no gap |
| `46-diff-cell-m2-8-toward-block-0-4-wide.png` | Tamriel | (-2,8) Wilderness | (-8017, 36306, -3560) | 0 / 118 | 100 | wooded slope rising in the look direction; no gap |
| `47-diff-cell-m2-8-toward-block-0-4-narrow.png` | Tamriel | (-2,8) Wilderness | (-8017, 36306, -3560) | -2 / 118 | 55 | same; no gap |
| `48-diff-cell-3-m11-toward-block-0-m8-wide.png` | Tamriel | (3,-11) Wilderness | (13771, -44863, 19) | 0 / 351 | 100 | mountain face in the look direction; no gap |
| `49-diff-cell-3-m11-toward-block-0-m8-narrow.png` | Tamriel | (3,-11) Wilderness | (13771, -44863, 19) | -2 / 351 | 55 | same; no gap |

Vantage points of section 9 / the Pending bullet, verdicts:

- **Whiterun plains toward the Throat of the World**, cell (-4,-5), captures 03-05: no hole. The
  probe for this cell (`lodforge occlusion ... --cells=-4,-5 --probe=-4,-5 --probe-cells 48`)
  lists 3 blocks the golden shows and we hide, all 28+ cells away ((-32,-32), (-44,16),
  (-40,16)); 11 blocks only we show.
- **Solitude arch from the docks**, SolitudeDocks02 cell (-16,24), captures 06-09: no hole.
  Probe: 48 golden-only blocks, the nearest (-16,32) 8 cells north; most are sea blocks 24-48
  cells north of the city, every best line of sight blocked at (-15,26)/(-15,27), the rock the
  city stands on (rise 1,300-3,000 units above the line).
- **Windhelm from its bridge**, WindhelmBridge03 cell (33,7), captures 13, 15, 16 (10-12, 14,
  17 are gatehouse walls): no hole. Probe: 13 golden-only blocks, nearest (24,16) 9 cells
  north-west behind the city rock (31,12), the rest 11-31 cells east; 35 blocks only we show.
- **The Rift from the road above Riften**, RiftenAttackStart01 cell (42,-20), captures 18-21,
  plus the north shore of Lake Honrich, cell (38,-21), captures 22-24: no hole; the Rift's
  forest keeps the visible range short, the lake view reaches the far shore. Probe for (42,-20):
  5 golden-only blocks, all 18+ cells away (mountains at (28,-40), (36,-40), (60,-40), (16,-8)).
- **Raven Rock looking east**: from the docks (cell (6,8), 25-27) the Bulwark blocks the east;
  from the ash wastes two cells east of it (DLC2TestAshTrees, cell (10,8), 34-37) all four
  directions are clean, 36 looks back west over the town to the far hills and the sea. No hole.
  (No golden exists for Solstheim; `Occlusion-Solstheim.esp` is the smoke-test plugin.)
- **The Earth Stone**, DLC2StoneEarth cell (5,7), captures 38-41: no hole.

Where a hole could be seen at all. The per-cell diff of the two plugins (`tvdt_diff.py`, decoder
written from section 4: pre-order quadtree, children SW/NW/SE/NE, set bit = visible, grid
origin (-96,-96) stride 256; invariants checked on 400 decodes: own block visible 400/400,
level-1 parent = OR(children) 400/400) gives, over the 5,740 cells both plugins carry:
golden 3,040,985 visible leaf bits, ours 3,142,156; 151,750 blocks visible only in the golden,
252,921 only in ours. Distance from the observer cell to the nearest cell of a golden-only
block, 4-cell bins: 0-3: 82, 4-7: 1,253, 8-11: 2,181, 12-15: 3,143, 16-19: 4,050, then 4,600-8,600
per bin out to 95 cells. So 99.9% of the blocks we hide and the golden shows are 4+ cells away
and 97.7% are 12+ cells away; with uGridsToLoad 5 the nearest LOD block starts 2 cells out,
and this list's haze (Community Shaders; the same in the B1 terrain captures) washes distant
LOD out beyond roughly 25-30k units (6-7 cells), so the 82 + 1,253 pairs within 7 cells are the
ones a ground observer could notice. Four of the nearest were visited (the `diff-cell-*`
captures): observer (35,6) and block (32..35, 0..3) south, (21,0) and (24..27, 0..3) east,
(-2,8) and (0..3, 4..7) east-south-east, (3,-11) and (0..3, -8..-5) north. In all four the look
direction ends on the local ground within the loaded cells (a ridge, the White River gorge
wall, a wooded slope, a mountain face), which is exactly the terrain this module treats as the
occluder; the hidden block is behind it and nothing is missing from the frame.

Caveats. (1) Haze: distant LOD beyond 6-7 cells has little contrast in this list, so the check
is strongest for the near LOD rings, where the diff puts 1,335 of the 151,750 disagreements.
(2) The free camera stayed within 180 units of the player's ground position except where the
table says otherwise (bridge span, Bulwark), so the TVDT in force was the vantage cell's.
(3) `cow` places the player at the cell's marker, not at a viewpoint: several first frames look
into a wall or a slope and are kept as diagnostics. (4) One launch, no golden A/B; the test
profile and the three mods stay in skyrim-cs for a re-run (the 2.1 GB mod can be deleted).

## 13. Process record (implementation)

- **Date:** 2026-10-20, implementing session (started from the licensing session) in the
  worktree `D:\Modlists\LODForge\.claude\worktrees\occlusion-reimpl`, branch `occlusion-reimpl`
  from `main` at 07c8e1a. First action: `git rm crates/dyndolod/src/occlusion.rs` unread, with
  this spec and the check script copied in from the main checkout (commit 67f50e6).
- **Read for the implementation:** this spec; `Check-TvdtLayout.py` and
  `Test-OcclusionParity.ps1`; AnvilLOD `606eece` `src/AnvilLOD.Core/Occlusion/TvdtData.cs`,
  `OcclusionGrid.cs`, `OcclusionCalculator.cs` and its `LICENSE`; the xLODGen readmes
  (`Skyrim-Occlusion-Readme.txt`, `Terrain-Heightmap-Readme.txt`, `LODSettings-File-Readme.txt`);
  xEdit's CELL, WRLD and REGN definitions (`wbDefinitionsTES5.pas`); the golden run's
  `DynDOLOD_SSE_log.txt` (occlusion lines), `Debug_log` `Occlusion_*` lines, `DynDOLOD.ini` /
  `DynDOLOD_SSE_Default.ini` occlusion keys, `Occlusion.esp`, `DynDOLOD.esm/.esp`, the object
  height map and the terrain `.bin`; Skyrim.esm's Tamriel WRLD record (`lodforge plugin-dump`);
  the vanilla Tamriel terrain LOD file list (`lodforge vfs-list`) and the bounds of one `.btr`;
  `docs/licensing.md`, `docs/reimplementation-plan.md`; LODForge's own `crates/esp` (public
  API, `terrain.rs`, `world.rs`, `record.rs`), `crates/dyndolod/src/{plugin,ini,childworld,
  lib}.rs`, the `crates/archive` VFS API, `crates/nif/src/lib.rs` public items and `parse`,
  `apps/cli/src/{build,dyndolod_build,profile}.rs` (the `write_occlusion` stage, the terrain
  `.bin` writer's water rule, the profile/VFS API). Statistics of the golden plugin were taken
  with scratch Python scripts outside the repository (radius rules, void visibility by observer
  height and by block row, leaf agreement under the grid symmetries).
- **Read beyond the spec's rule, disclosed:** (1) the whole of `apps/cli/src/occlusion.rs`
  (the brief named it an allowed input; section 11's restriction to its `Args` was seen
  afterwards). It exposed that the old module kept per-cell nodes with `water/min/max/top`
  heights, started the main worldspace's water at 0, and built a child's map from the parent's
  finished map with the child's cells reset to the child's water, then the parent's LAND at the
  child's grids. None of that was used as a design input: the heights here come from vertex
  sampling validated against the golden log, the child map is built from LAND cells, and there
  is no initial water. (2) `crates/nif/src/btr.rs` lines 1–110 (module comment, the `Btr`,
  `ShapeData`, `Aabb`, `Water` structs and two private helpers), opened for a `.btr` reader
  before `docs/licensing.md` was read; nothing of it is used: the meshes are read with the
  generic NIF reader and the transform observed on a vanilla file. (3) `terrain_bin` in
  `apps/cli/src/dyndolod_build.rs` (the LODGen input writer, named by section 8 as a caller);
  only its per-cell water rule (XCLW, one-byte DATA, default) was reused, in LODForge's own
  words.
- **Never opened:** `crates/dyndolod/src/occlusion.rs` (any version, any worktree),
  `D:\Modlists\ref\quarantine\`, `D:\Modlists\ref\lodgen-*`, `D:\Modlists\ref\skyrimse-*`,
  `D:\Modlists\tools\rea-snapshots`, any Ghidra/REA material, `docs/re/dyndolod-spec.md`,
  `docs/baseline.md`, `docs/parity.md`, `docs/re/gap-audit-20261019*.md`,
  `docs/re/lodgen-318-delta.md`, `crates/objlod/src`, `crates/btr/src` other than `lib.rs`,
  `crates/nif/src/writer.rs`; no web search for decompiles.
- **AnvilLOD files ported** (GPL-3.0-only, "Copyright (c) 2026 CageTV", commit 606eece, named
  in each file's header with the modifications): `TvdtData.cs` → `tvdt.rs` (accessor design;
  the internal-node layout is section 4's), `OcclusionGrid.cs` → `grid.rs` (two-layer grid;
  vertex sampling, water clamp, height-map points, statistics and MHDT are LODForge's),
  `OcclusionCalculator.cs` → `visibility.rs` (sample points per sub-area, eye/target search
  order, a fitted clearance constant; the fitted values, the void rules and the diagnostics are
  LODForge's). `THIRD-PARTY-NOTICES` entry pending with the license mechanics stage.
- **Validation:** output only (12.2); nothing was compared with the old module beyond the
  numbers already in this spec. `cargo test --workspace`: 154 passed, 0 failed, 2 ignored
  (35 test binaries).
