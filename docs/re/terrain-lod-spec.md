# Terrain LOD specification (scrubbed, stage B0.1)

Written 2026-10-19 by the analyst session of [reimplementation-plan.md](../reimplementation-plan.md)
("Stage B0.1"). It is the only description of terrain LOD meshes the stage B1 implementer may use,
next to the inputs listed in the Process record (section 6). Every statement carries its source:

- `[golden]` observed in xLODGen / DynDOLOD output under `D:\Modlists\LODForge-golden\` with
  [scripts/golden/Dump-GoldenBtr.py](../../scripts/golden/Dump-GoldenBtr.py) (a reader written from the
  NIF format specification; it contains no generation logic). Unless stated otherwise the run is
  `no-seasons-20261017-031139` (xLODGen 4.1.5o beta, LODGen 3.0.36 as shipped with it, Tamriel, the
  skyrim-cs Default profile of 2026-10-17).
- `[nif]` the niftools NIF format specification `nif.xml` (niftoolsxml 0.10.0.0, github
  niftools/nifxml commit `028cb35b73f59f07acc39e7df7a89ecfda6439a1`, 2023-06-17; a copy is in the
  session scratchpad and should be added to `D:\Modlists\ref\nifxml\` for the implementer).
- `[terra]` the public-domain Terra 0.7 sources, `D:\Modlists\ref\terra-0.7\terra\`.
- `[doc]` xLODGen's shipped readmes (`D:\Modlists\skyrim-cs\tools\xLODGen\*.txt`), dyndolod.info,
  DynDOLOD's shipped `Edit Scripts\DynDOLOD\DynDOLOD_SSE.ini`, UESP's LAND record page.
- `[cfg]` option keys as xLODGen stores them (`[SSE LOD Options]` in
  `%LOCALAPPDATA%\Skyrim Special Edition\Plugins.sseviewsettings`) and as it writes them into the
  per-worldspace control file `Edit Scripts\LODGen_Terrain_<World>.txt` (an output of xLODGen,
  observed like any other).
- `[ours]` our own untainted code: `crates/btr/src/export.rs`, `crates/esp`, the `crates/nif`
  reader, `crates/dyndolod/src/dynplugins.rs`, the CLI wrappers.
- `[anvil]` AnvilLOD (GPL-3.0-only, `D:\Modlists\ref\AnvilLOD` @ 606eece), underside files only.

Parity policy: tolerance-level, not bit-exact (licensing.md, "Decision record"). Where the golden
files show an artefact of the reference tool that has no functional value, the spec says so and
the implementer may do better.

## 0. Terms and coordinates

- **Cell**: 4,096 x 4,096 game units; LAND holds a 33 x 33 height grid, 128 units apart, row 0
  south, column 0 west `[doc: UESP LAND]`. Heights are stored in units of 8.
- **Quad** at **level** L (4, 8, 16, 32): L x L cells, one `.btr` file. The quad lattice starts at
  the worldspace's south-west cell from `lodsettings\<World>.lod` and steps by L cells
  `[doc: LODSettings-File-Readme.txt]` `[golden]`: Tamriel's SW cell is (-96, -96), so its quads sit
  at multiples of L; MarkarthWorld's is (-48, -3) and its LOD32 quad starts at cell (-48, -3)
  (`MarkarthWorld_Underside.nif` shape translation (-196608, -12288)).
- **Quad-local units**: every shape in a BTR is written with `scale = L` and vertices in
  0..4096 on x and y whatever the level `[golden]`, so one local unit is L game units and the
  LAND grid step is 128 / L local units (32 at LOD4, 4 at LOD32). Heights are world height / L.
  The game places the file at the quad's south-west corner (x * 4096, y * 4096); the file holds no
  world offset (translation is (0, 0, z shift), see 1.10).
- **Grid point**: a LAND height sample inside a quad: (32 L + 1)^2 points, point (i, j) at local
  (i * 128 / L, j * 128 / L). Cell (cx, cy) of the quad covers grid i in 32 (cx - qx) .. +32.

## 1. BTR file format as observed

### 1.1 Files

`meshes\terrain\<World>\<World>.<L>.<x>.<y>[.<SEASON>].btr`, x and y the quad's south-west cell
`[golden]`. Tamriel (cells -64..63 x -64..96 exported, see 2.3): LOD4 1,312 files, LOD8 336,
LOD16 88, LOD32 24 = 1,760. Every lattice quad that intersects the exported cell rectangle is
written, including quads that are entirely flat or under water (the 640-triangle LOD32 quads of the
northern sea) `[golden]`. Season files (`.SPR`, `.SUM`, `.AUT`, `.WIN`) hold the same geometry as
the default file and differ only in the two texture paths (checked on 6 quads across all levels;
the one exception, `8.-40.0`, is the reference tool's own run-to-run variance, see 5.2) `[golden
seasons-20261017-032143]`.

### 1.2 Container `[nif]` `[golden]`

Gamebryo 20.2.0.7, user version 12, BS version 100 (Skyrim SE), little endian. Header per
`nif.xml` `Header`: header line `Gamebryo File Format, Version 20.2.0.7` + `\n`, version
`0x14020007`, endian 1, user version 12, block count, BS version 100, three export strings (byte
length including the NUL, then the bytes; author / process script / export script), block type
count (u16) + types (u32-sized strings), block type index (u16 per block), block sizes (u32 per
block), string count + max string length + strings (u32-sized), group count 0. Footer: root count 1,
root 0. Author string in the golden files: `LODGen 3.0.36.0 by Ehamloptiran, Zilav and Sheson`
(underside: `LODGen 3.1.8.0 by ...`); the other two are empty. LODForge writes its own name there.

Block type table order = order of first use; string table `chunk`, `land`, then `WATER` when the
file has water (max string length 5) `[golden]`. Our reader (`crates/nif`) parses all of this; the
round-trip test `crates/nif/tests/golden.rs` checks that reader + writer reproduce the golden files
byte for byte. That test may stay; a new writer only has to produce the same encoding of the same
decoded content, which the test checks.

### 1.3 Block list and order `[golden]`

Observed in every one of the 1,760 files (block indices for a file with one water shape):

```
0  BSMultiBoundNode  "chunk"  flags 0xE, identity, culling mode 1, children [1, 4], multi bound 8
1  BSTriShape        "land"   flags 0xE, translation (0, 0, zshift), scale L, shader 2
2  BSLightingShaderProperty   (section 1.6), texture set 3
3  BSShaderTextureSet         [diffuse, normal]
4  BSMultiBoundNode  "WATER"  flags 0xE, identity, culling mode 0, children [5..], multi bound 6   (only with water)
5  BSSubIndexTriShape (LOD4) / BSTriShape (LOD8+)  unnamed, flags 0xE, translation (0, 0, zshift), scale L, no shader
   ... one per water shape
6  BSMultiBound -> 7 BSMultiBoundAABB   water bounds
8  BSMultiBound -> 9 BSMultiBoundAABB   land bounds
```

Without water: blocks 0-3 then BSMultiBound/AABB at 4-5. Always exactly one land shape (even the
largest quad, 22,897 vertices / 45,159 triangles, is far below the u16 limits). Flags 0xE on every
AV object are the `nif.xml` defaults for `BSMultiBoundNode` / `BSTriShape` (SSE). Rotation is the
identity, no collision, no extra data, no controller.

### 1.4 Land shape `[golden]` `[nif]`

`BSTriShape` per `nif.xml`: NiAVObject fields, bounding sphere (centre, radius), skin ref -1, shader
property ref, alpha property ref -1, vertex desc (u64), num triangles (u16 for BS 100), num
vertices (u16), data size (= vertices * stride + triangles * 6), vertex data, triangles (u16 x 3),
particle data size 0.

- Vertex desc `0x0000300000000405`: stride 5 dwords (20 bytes), UV offset nibble 4, attributes
  `0x003` = Vertex | UVs (`nif.xml` bitfield `BSVertexDesc`, bitflags `VertexAttribute`). Vertex
  data per `BSVertexDataSSE`: position 3 x f32, "Unused W" u32 (written 0), UV 2 x half. No
  normals, tangents or colours in any terrain file.
- Positions in quad-local units, x and y in 0..4096 in all 1,760 files; z = world height / L.
- UV = (x / 4096, 1 - y / 4096) exactly (max deviation 0 over all files; the half-float rounding of
  the stored UV is the writer's, so compute in f32 and convert to half with round-to-nearest).
- Bounding sphere: centre = bbox centre of the shape's vertices (component-wise (min + max) / 2),
  radius = distance from that centre to the bbox max corner; holds in 1,760 / 1,760 files within
  0.05.
- Triangles counter-clockwise seen from above (the first triangle of `Tamriel.4.0.0.btr` is
  (928,480) (928,512) (896,480)).

### 1.5 Texture set `[golden]`

Two slots: `Data\Textures\Terrain\<World>\<World>.<L>.<x>.<y>[.<SEASON>].dds` and the same with
`_n` before `.dds`. Note the `Data\` prefix on terrain LOD textures; the underside (section 4)
uses `Textures\...` without it.

### 1.6 Shader property `[golden]` `[nif]`

`BSLightingShaderProperty`, identical in all 1,760 files:

| Field | Value |
|---|---|
| Shader type | 18 = `LOD Landscape Noise` (`nif.xml` enum `BSLightingShaderType`) |
| Name / extra data / controller | none |
| Shader flags 1 | `0x80401000` = ZBuffer_Test (bit 31) \| Own_Emit (bit 22) \| Model_Space_Normals (bit 12) |
| Shader flags 2 | `0x00000003` = ZBuffer_Write (bit 0) \| LOD_Landscape (bit 1) |
| UV offset / scale | (0, 0) / (1, 1) |
| Texture set | the block above |
| Emissive colour / multiple | (0, 0, 0) / 1.0 |
| Texture clamp mode | 0 (CLAMP_S_CLAMP_T) |
| Alpha | 1.0 |
| Refraction strength | 0 |
| Glossiness | 1.0 |
| Specular colour / strength | (1, 1, 1) / 1.0 |
| Lighting effect 1 / 2 | 0 / 0 |

Type 18 has no type-specific tail. Water shapes have no shader property and no alpha property.

### 1.7 Water node and shapes `[golden]`

The `WATER` BSMultiBoundNode (culling mode 0) exists when at least one cell of the quad gets a
water plane (rule in 3.9). Its children:

- **LOD4**: one `BSSubIndexTriShape` per distinct water height in the quad, children ordered by
  ascending height (42 / 42 files with more than one shape). Each shape holds one flat quad per
  cell at that height: 4 vertices (x0, y0), (x0 + 1024, y0), (x0, y0 + 1024), (x0 + 1024,
  y0 + 1024) at z = water / 4, triangles (0, 1, 2) and (1, 3, 2) relative to the cell's first
  vertex; vertices shared between neighbouring cells of the same shape are merged (a shape with
  all 16 cells has 25 vertices and 32 triangles). Vertex desc `0x0000100000000404`: 16-byte
  vertices, attributes `0x001` (position only), no UVs.
  Segments (`nif.xml` `BSSubIndexTriShape`, BS 100: `Num Segments` u32, then
  `BSGeometrySegmentData` = flags u8, start index u32, num primitives u32): one entry per cell slot
  in the order `dx * 4 + dy` (dx, dy = cell offset inside the quad, 0..3), so slot k = cell
  (k div 4, k mod 4). A slot whose cell has no water at this height is (0, 0, 0); trailing empty
  slots are dropped, so a shape has 1..16 segments (observed counts: 16 x 692, 1 x 9, every
  value between). A filled slot has flags 0, start index = 3 x (index of the cell's first
  triangle), num primitives 2. Example `Tamriel.4.36.96.btr`: 16 segments with start 0, 6, 12,
  ..., 90.
- **LOD8/16/32**: one unnamed `BSTriShape` for all water cells of the quad at whatever height
  (one shape per file, 238 / 70 / 22 files), same per-cell quads of side 4096 / L local units at
  z = water / L, shared vertices merged (a full LOD32 water quad has 1,089 vertices, 2,048
  triangles). No segments. The vertex and triangle order of these shapes in the golden files is
  not the per-cell order; any order is acceptable (no consumer indexes them).
- Water shapes' bounding sphere: centre = bbox centre, radius = largest distance from that centre
  to any vertex (for a single flat quad of side s that is s / sqrt 2, e.g. 724.077 for 1024).

### 1.8 Multibound AABBs `[golden]`

`BSMultiBound -> BSMultiBoundAABB` (position = centre, extent = half size, `nif.xml`). Both boxes
are in game units relative to the quad's origin, i.e. local bbox x L, with the top of the box
clamped to z >= 0: position.z = (min_z + max(max_z, 0)) / 2 x L, extent.z = (max(max_z, 0) - min_z)
/ 2 x L. Holds in 1,760 / 1,760 files for the land box (bbox of the land shape's vertices, skirts
included) and in every file for the water box (bbox of all water vertices of the quad). Examples:
`Tamriel.4.0.0.btr` land AABB (8192, 8192, -3838) / (8192, 8192, 3838) for a land bbox z
-1919..-990 local; `Tamriel.32.0.-64.btr` (65536, 65536, 7234.72) / (65536, 65536, 27178.72).

### 1.9 Root node `[golden]`

`chunk` BSMultiBoundNode, culling mode 1, children = [land shape, WATER node when present],
multi bound = the land AABB. Per `nif.xml` the field after the multi bound is `Culling Mode`
(`BSCPCullingType`); the observed values are 1 on the root and 0 on the water node.

### 1.10 Z shift `[golden]` `[cfg]`

Shapes are translated by (0, 0, zshift). The control file has no `ZShiftLOD<L>` key in any of the
observed xLODGen runs and every golden file has translation z = 0. Support a per-level z shift as
a setting that defaults to 0.

### 1.11 Byte-level encoding notes `[nif]` `[golden]`

Strings are Latin-1. Refs are i32 with -1 for none. Half floats are IEEE binary16; for the UV
values that occur (k / 4096 and 1 - k / 4096 with k = 0..4096 in steps of 128 / L) every value is
exactly representable at LOD4..LOD16 and rounds at LOD32 (step 1/1024 is exact too; all observed
UVs round-trip with error 0 through the dumper's half decoder, so round-to-nearest is adequate).

## 2. Terrain input

### 2.1 Source records `[doc]` `[ours]`

| Data | Record | Use |
|---|---|---|
| Heights | LAND `VHGT`: f32 offset, 33 x 33 signed-byte gradients, 3 pad bytes. Height(row r, col c) in units of 8: row 0 starts at offset, each next row's first value adds its gradient to the previous row's first value, each next column adds its gradient to the previous column; multiply by 8 for game units `[doc: UESP Skyrim_Mod:Mod_File_Format/LAND]`. Our reader: `lodforge_esp::Plugin::land_vhgt` `[ours]`. | heights |
| Water | CELL `XCLW` (f32). `FLT_MAX` (0x7F7FFFFF) means "use the worldspace default"; vanilla Tamriel also holds garbage values near +-2^31 and 2^32 that the reference tool treats the same way; a CELL whose `DATA` field is one byte (14 vanilla Blackreach cells) means "no water" (-FLT_MAX) `[golden .bin, baseline P2]` `[ours: export.rs]`. | per-cell water height |
| Defaults | WRLD `DNAM` = (default land height, default water height); Tamriel (-27000, -14000) `[golden: control file LandHeight/WaterHeight, Skyrim.esm WRLD 0x3C DNAM]`. | fill and water fallback |
| Lattice | `lodsettings\<World>.lod`, 16 bytes `[golden: extracted from Skyrim - Meshes0.bsa]`: i16 SW cell x, i16 SW cell y, i32 stride (256 Tamriel, 32 Markarth), i32 min level (4), i32 max level (32; JaphetsFollyWorld 16) `[doc: LODSettings-File-Readme.txt]`. | `CellSW`, `MaxLevel` |
| Hide flags | CELL `XCLC` quadrant hide flags (`HideQuads` / `IgnoreHide`, section 4) `[cfg]` `[doc: Terrain-LOD-Readme.txt]`. | underside only |

Winning records are the load order's last override, except duplicate LAND records of one cell in
several plugins where the earliest plugin's LAND is the real one (Dawnguard ships overrides;
`crates/esp` already handles this) `[ours]`.

### 2.2 In-memory structure (ours)

The rewrite defines its own input type; a suggestion that matches `export.rs`:

```rust
pub struct TerrainCell {
    pub x: i32, pub y: i32,
    pub water: Option<f32>,            // None = no water (the -FLT_MAX case)
    pub heights: [[f32; 33]; 33],      // [col][row] game units, row 0 = south
    pub hide_quadrants: u8,            // bits 1 SW, 2 SE, 4 NW, 8 NE; 0 for xLODGen-style builds
}
pub struct TerrainInput {
    pub world: String,
    pub south_west: (i32, i32), pub max_level: i32,          // lodsettings
    pub default_land: f32, pub default_water: f32,            // WRLD DNAM
    pub cells: Vec<TerrainCell>,                              // sorted by (y, x), unique
}
```

### 2.3 Cell set and fill rule `[golden]` `[ours]`

xLODGen exports every CELL inside the bounding rectangle of the worldspace's LAND cells, not only
cells with LAND; a cell without LAND, or with a LAND without `VHGT`, is flat at `DNAM` land height
(Soul Cairn: 913 LANDs, 7,236 exported cells) `[golden .bin of 11 worldspaces, baseline P2 "Other
worldspaces"]`. Tamriel: LAND cells -64..63 x -64..96 = 128 x 161 = 20,608 cells (log: "Data from
-64,-64 to 63,96"). `export.rs` implements exactly this and its output is byte-identical to
xLODGen's `.bin` for all 11 worldspaces; keep it.

Quads that extend beyond the rectangle (the lattice is aligned to `CellSW`, not to the data)
treat the missing cells as flat at the default land height with the default water `[golden]`: the
LOD4 quads of Tamriel's y = 96 row contain water planes at -14000 over their three non-existent
northern cell rows, and in LOD8 `8.16.96` the land row at local y = 512 sits at -27000 / 8.
Artefact to note: along the north and east sides of the exported rectangle the golden mesh gives
the shared grid row/column the flat fill height instead of the real cell's edge (LOD8 example above:
source -3584, mesh -3375). Reproducing this is optional; a rewrite should prefer the real data.

### 2.4 Water height per cell `[golden]` `[ours]`

`water = if DATA is one byte { none } else if |XCLW| < 2^24 { XCLW } else { DNAM water }`
(`export.rs`). The reference tool ignores water entirely with `IgnoreWater=True` `[cfg]`; a
cell whose water is none gets no water plane and its land is treated as dry.

### 2.5 Optional import format: `LODGen_Terrain_<World>.bin` `[golden]`

xLODGen writes `Edit Scripts\LODGen_Terrain_<World>.bin` (DynDOLOD: `Export\LODGen_SSE_LODGen_
Terrain_<World>.bin`). Observed layout, 1,112 bytes per cell, little endian, no header: x i32, y
i32, water f32, flags i32 (0 in xLODGen's files; DynDOLOD's carry the cell's hide-quadrant bits),
then the 1,096 `VHGT` bytes (offset f32, 1,089 gradients, 3 pad). Tamriel: 22,916,096 bytes =
20,608 records, cells ordered by (y, x). Keep a reader only as an optional input for comparisons
(`Dump-GoldenBtr.py fit` reads it); the pipeline itself works from `TerrainInput`.

### 2.6 Settings keys `[cfg]` `[doc]`

The control file xLODGen writes (observed, Tamriel, `LODGen_Terrain_Tamriel.txt` of the golden
run; DynDOLOD's underside export in 4.3):

```
GameMode=TERRAINSSE
Worldspace=Tamriel
CellSW=-96 -96                 (lodsettings SW cell)
MaxLevel=32                    (lodsettings max level)
TerrainData=<path>.bin
IgnoreWater=False
LandHeight=-27000              (DNAM)
WaterHeight=-14000             (DNAM)
ProtectCellBorders=True
HideQuads=False
QualityLOD4=5     MaxVertsLOD4=32767    WaterDeltaLOD4=-1
QualityLOD8=8     MaxVertsLOD8=32767    WaterDeltaLOD8=0
QualityLOD16=8    MaxVertsLOD16=32767   WaterDeltaLOD16=0
QualityLOD32=10   MaxVertsLOD32=32767   WaterDeltaLOD32=550
Skirts=256
PathOutput=<folder>
```

Mapping from the UI keys in `Plugins.sseviewsettings` `[cfg]`: `TerrainQualityLOD<L>` ->
`QualityLOD<L>` (float allowed, readme: "It is also possible to set floating point numbers like
2.5"); `TerrainMaxVertsLOD<L>` -> `MaxVertsLOD<L>`; `TerrainProtectCellBorders` / `TerrainHideQuads`
(0/1) -> True/False; `TerrainWaterDeltaLOD<L>` is a drop-down index: 0 -> -1 ("off"), 1 -> 0
("on"), k >= 2 -> 25 x (k - 1) (observed pairs 0/-1, 1/0, 23/550). `Skirts=256` has no UI key; it is
what xLODGen always writes. Meaning of the options `[doc: Terrain-LOD-Readme.txt]`:

- Quality: "higher settings equal less quality, less terrain detail".
- Max Vertices: "can be used to limit max files size regardless of terrain roughness. Also used to
  enforce NIF limits." The LODGen log prints `Max Vertices: 30767 (2000 reserved for skirts)` for
  32767 `[golden log]`.
- Optimize Unseen (WaterDelta): "on = merge large chunks of triangles under water resulting in less
  vertices. x = merge triangles and move shallow vertices under water to at least this distance
  below the water surface. The higher the value the more detail of the coast line is retained and
  less z-fighting between water and shallow terrain."
- Protect Cell Borders: "do not merge triangles across cell borders to avoid ugly terrain drops
  with stretched textures at the edge of the loaded cells."
- Hide Quads: "do not generate triangles for cell quads (a cell has four quadrants) that are set
  to hidden on the cell record. Punches a hole in the terrain LOD mesh."

Other worldspaces' control files (same folder) show the same quality/delta/skirt values with their
own `CellSW`, `MaxLevel`, `LandHeight`, `WaterHeight` (e.g. DeepwoodRedoubtWorld LandHeight
-4940.197265625, WaterHeight -500000; JaphetsFollyWorld MaxLevel=16).

## 3. The mesher

### 3.1 Greedy insertion as Terra 0.7 does it `[terra]`

Terra approximates a height field by inserting the worst-fitting grid point until a goal is met
(Garland & Heckbert, CMU-CS-95-181; `README.html`). The pieces, by file:

- `Map.h/.cc`: the height field, `eval(i, j)` on an integer grid of `width` x `height` samples.
- `Quadedge.h/.cc`: Guibas-Stolfi quad-edge structure (`Onext`, `Sym`, `Rot`, `Lnext`, ...),
  `splice`. `Subdivision.h/.cc`: a Delaunay triangulation over it. `initMesh(a, b, c, d)` builds
  the rectangle a-b-c-d with the diagonal a-c as two triangles; `locate(x, hint)` walks edges to the
  triangle containing x (the walk picks a random direction when both neighbours qualify; any
  deterministic tie-break works, the located triangle is the same except on exact ties);
  `spoke(x, e)` connects x to the corners of the triangle (or, if x lies on an edge, of both
  triangles sharing it, deleting that edge); `optimize(x, s)` restores Delaunay by swapping every
  interior edge opposite x that fails the `inCircle` test, then calls `update` on each triangle
  around x. `isInterior` is a topological test that assumes a rectangular boundary.
- `GreedyInsert.h/.cc`: `GreedySubdivision` keeps `is_used(x, y)` per grid point (UNUSED, USED,
  IGNORED) and a max-heap of triangles keyed by their candidate importance. The constructor calls
  `initMesh` on the four grid corners and marks them used (`count = 4`). `scanTriangle` computes the
  plane through the triangle's three corner heights, scans every grid point inside the triangle
  (two scan-line passes over the points sorted by y, `ceil(min x)..floor(max x)` per row), and for
  every unused point records the largest |height - plane| as the triangle's candidate
  (`Candidate::consider`; an optional importance mask multiplies it). A triangle whose best
  importance is below 1e-4 is removed from the heap; otherwise the heap entry is inserted or
  updated. `select(x, y, t)` marks the point used, bumps the count and inserts it (`insert` ->
  `locate` from the triangle's anchor, `spoke`, `optimize`; `TrackedTriangle::update` rescans the
  changed triangles). `greedyInsert` pops the heap's top triangle and selects its candidate.
- `Heap.h/.cc`: array heap with `insert`, `update`, `extract`, `kill`; triangles carry their heap
  token (`NOT_IN_HEAP = -47`).
- `greedy.cc`: the loop `while (maxError() > error_threshold && pointCount() < point_limit)
  greedyInsert()`, `-e` threshold and `-p` point limit from `cmdline.cc`; `scripted_preinsertion`
  (`s x y` selects a point, `i x y` marks it ignored so it is never a candidate).
- `output.cc`: walks the face list and writes each triangle's three corners (grid x, y, height).

Properties the rewrite inherits: every output vertex is a grid point with its own grid height; the
first triangle pair is the quad's two corner triangles; the result is a Delaunay triangulation of the
selected points; the largest remaining grid-point error is below the threshold unless the point
limit stopped the loop. Complexity is fine for the sizes here (a LOD32 quad is a 1,025 x 1,025
grid; the reference tool builds all 1,760 Tamriel quads in 24 s single-process, our port in 1.8 s
on 20 threads).

### 3.2 How a quad maps onto Terra `[golden]`

- Grid: (32 L + 1) x (32 L + 1) points, heights in local units (world / L); grid point (i, j) ->
  local (i * 128 / L, j * 128 / L). Output vertices: every non-skirt vertex of every golden land
  shape lies exactly on this grid (0 off-grid of 397,738 checked) and carries the source height
  except where section 3.7 modifies it.
- Each LOD4 quad is triangulated **cell by cell** when `ProtectCellBorders=True`: in all 1,312
  golden LOD4 files no triangle straddles an interior cell line (x or y = 1024, 2048, 3072 local;
  0 of 5,756,569 triangles), i.e. each cell is a 33 x 33 Terra problem of its own and the quad is the
  union. Levels 8/16/32 are single Terra problems: 168,012 / 127,970 / 96,980 triangles straddle
  interior cell lines.
- Within a LOD4 quad the cells are stitched: of 34,319 vertices on interior cell lines (219 quads
  sampled) 34,166 are used by triangles on both sides and 153 (0.45 %) only on one side
  (T-junctions). Requirement: no visible cracks inside a quad; the simplest way is to triangulate
  the shared border profile once (a 1-D greedy simplification of the 33 border heights with the same
  threshold) and pre-insert those points into both cells, which also removes the 0.45 %.
- Border points at levels 8/16/32: every quad-border grid point at a multiple of 256 local units
  (17 per edge, i.e. every 2 L-th grid step) is present in every sampled golden quad (2,688 / 2,688,
  2,816 / 2,816, 1,536 / 1,536); finer border points appear only where the terrain needs them. At
  LOD4 only the four corners of each cell are guaranteed (656 / 656 at stride 1024). The 17 x 17
  grid also appears in the interior of fully under-water quads (the 417-vertex / 640-triangle
  minimum of LOD8/16/32, section 3.7).

### 3.3 Stop criteria per level `[cfg]` `[golden]`

Measured with `Dump-GoldenBtr.py fit` on 40 quads per level (all 24 at LOD32) against the
golden run's own `.bin`: the largest |source height - mesh height| over the grid points that lie
above water, per quad, in local units:

| Level | Quality | per-quad max error, above water: min / p50 / p90 / max (local) | = world units at p90 |
|---|---|---|---|
| 4 | 5 | 0 / 4.96 / 5 / 7.56 | 20 |
| 8 | 8 | 0 / 7.96 / 8 / 8 | 64 |
| 16 | 8 | 0 / 8 / 8 / 8 | 128 |
| 32 | 10 | 0 / 10.1 / 10.2 / 10.2 | 326 (10.2 = 10 + the 0.17 raise of 3.7) |

So **Quality is the Terra error threshold in quad-local height units** (Quality x L game units), at
every level including the per-cell LOD4 build (the per-quad maximum sits at the threshold in the
median and 90th percentile; the LOD4 outlier 7.56 comes from cell-border rows where neighbouring
LAND records disagree, see 7). `MaxVerts` is the point limit (the log says 2,000 of it are reserved
for skirts; Tamriel never reaches it: max 22,897 vertices at LOD32 including skirts). Under water the
golden mesh is coarse: with Optimize Unseen on, the per-quad maximum over all grid points is up to
306 / 395 / 286 local units at LOD8/16/32 (section 3.7); at LOD4 (off) the two columns agree.

### 3.4 Shared edges between neighbouring quads `[golden]`

Measured with `Dump-GoldenBtr.py seams` over every pair of horizontally or vertically adjacent
golden quads of the same level, comparing the border vertices (skirts removed, world coordinates):

| Level | adjacent pairs | border positions | z identical on both sides | present on one side only | both sides, z differs |
|---|---|---|---|---|---|
| 4 | 2,551 | 68,152 | 67,304 | 729 (1.1 %) | 119 |
| 8 | 635 | 17,585 | 17,212 | 204 (1.2 %) | 169 |
| 16 | 157 | 4,998 | 4,858 | 71 (1.4 %) | 69 |
| 32 | 38 | 1,330 | 1,181 | 127 (9.5 %) | 22 |

The reference tool does not stitch neighbouring quads: where both sides hold a vertex the heights
agree (same source grid), but each side keeps its own subset of border points, so there are
T-junctions and sub-pixel cracks which the skirts (3.8) hide. The "z differs" positions are the
flat-fill artefact of 2.3 and the under-water modifications of 3.7 on one side of a seam.
Requirement for the rewrite: coincident border vertices of neighbouring quads must be identical in
world space (same grid, same source height) and skirts must be present; matching border point sets
across quads (a shared 1-D border simplification per quad edge, computed once per edge and level)
is allowed and removes the cracks entirely.

### 3.5 Quad lattice and which quads to build `[golden]` `[doc]`

For level L in 4, 8, 16, 32 up to `MaxLevel`: quads at (sw.x + i L, sw.y + j L) for every i, j such
that the quad intersects the exported cell rectangle; cells of the quad outside the rectangle are
flat (2.3). Build order is free (the reference tool logs LOD32 first, then 16, 8, 4).

### 3.6 Flat and missing cells `[golden]`

A quad whose cells are all flat at one height yields the minimal mesh: at LOD4 (cell by cell, no
water pre-insertion) 25 vertices of the 5 x 5 cell corners plus skirts (`Tamriel.4.36.96.btr`: 98
vertices, 109 triangles, all 16 cells under water); at LOD8/16/32 with Optimize Unseen on, the
17 x 17 border/under-water lattice: 289 + 128 skirt vertices = 417, 512 + 128 = 640 triangles (the
`[640/2048]` LOD32 sea quads).

### 3.7 Water-aware simplification ("Optimize Unseen") `[doc]` `[golden]`

Golden settings: off at LOD4 (-1), on at LOD8/16 (0), 550 at LOD32. Observed effects:

- **On (0)**: under-water grid points are not candidates; the mesh under water is as coarse as the
  pre-inserted lattice allows (every 2 L-th grid point, 3.2), with per-quad maximum errors of
  hundreds of local units under water and the normal threshold above water (3.3). Vertex heights are
  the source heights (0 of 54,131 above-water and 4,820 of 4,837 below-water LOD8 vertices differ;
  the 17 are the fill artefact of 2.3).
- **Value v (550 at LOD32)**: every above-water vertex is raised by exactly 0.17 local units
  (103,738 of 103,738 sampled); every under-water vertex is lowered, the typical lowering being 16
to 17 local units with a mode at -16.75. With v / L = 550 / 32 = 17.19 the observed 0.17 (not 0.172)
  and the ~17 lowering fit `d = trunc(v / L)` cells-of-local-units: dry vertices + d / 100, wet
  vertices at most `water - d`. This is an empirical fit of one setting; verify it with a second
  xLODGen run at another value before relying on it (open question 7.2).
- The readme's intent (coast-line detail kept, no z-fighting between shallow terrain and the water
  plane) is the requirement; the exact numbers are the reference tool's.

### 3.8 Skirts `[golden]` `[cfg]`

With `Skirts=256` every border edge of the land mesh (an edge whose two vertices both lie on the
same side x = 0, x = 4096, y = 0 or y = 4096) gets two extra vertices straight below its ends and
two triangles (a vertical quad); the drop in local units is constant per level:

| Level | drop (local units) | drop (game units) | border pairs measured |
|---|---|---|---|
| 4 | 319 | 1,276 | 132,490 (a few dozen pairs at 321..737 where a border vertex was itself moved) |
| 8 | 256 | 2,048 | 35,226 |
| 16 | 193 | 3,088 | 10,519 |
| 32 | 130 | 4,160 | 2,983 |

The four values are 256 + 63, 256, 256 - 63, 256 - 126 (a straight line in the level index); how
other `Skirts` values map is an open question (7.3). Skirt vertices copy the UV of the vertex above
them and are included in the land bbox / AABB (1.8). The log's `[land/water]` triangle counts
include the skirt triangles (1,760 / 1,760 files match the log with skirts included).

### 3.9 Water planes `[golden]`

A cell gets a water quad in its level's water shape when `water > min(height over the cell's 33 x
33 grid)` and water is a real value (below 2^24); a cell missing from the export counts with the
default land and water heights. Checked on all 1,312 LOD4 files: 1,280 agree exactly and the 32
remaining are the y = 96 row whose northern cells are missing (water at -14000 over flat -27000
land, i.e. the same rule). The height is the cell's water (section 2.4) / L. Shapes and segments per
1.7.

### 3.10 Acceptance numbers for the golden settings `[golden]`

Tamriel, default season, `no-seasons-20261017-031139`:

| Level | files | land vertices min / p50 / max / sum | land triangles min / p50 / max / sum | files with water | water tris sum | bytes (all files) |
|---|---|---|---|---|---|---|
| 4 | 1,312 | 75 / 1,797 / 8,796 / 3,080,121 | 81 / 3,339 / 16,816 / 5,756,569 | 787 (858 shapes) | 21,444 | 97,909,210 |
| 8 | 336 | 417 / 1,285 / 10,160 / 683,702 | 640 / 2,272 / 19,682 / 1,261,054 | 238 | 22,468 | 21,937,052 |
| 16 | 88 | 417 / 2,281 / 13,680 / 280,044 | 640 / 4,236 / 26,653 / 528,355 | 70 | 24,516 | 9,239,738 |
| 32 | 24 | 417 / 4,285 / 22,897 / 140,547 | 640 / 8,109 / 45,159 / 272,097 | 22 | 28,612 | 4,897,100 |

Per-file examples: `4.0.0` 2,995 vertices / 5,640 triangles / 3 water shapes; `8.0.0` 2,997 / 5,626
/ 8 water triangles; `16.0.0` 5,744 / 10,889 / 18; `32.0.-64` 5,811 / 11,155 / 280. The water sums
follow directly from the input (two triangles per water cell), the land sums depend on the mesher;
a rewrite that meets the error threshold of 3.3 should land within roughly +-10 % of the land
vertex and triangle sums per level (a greedy insertion with the same threshold on the same grid
selects nearly the same point count), see 5.3.

## 4. Terrain underside (DynDOLOD)

### 4.1 What it is `[doc: dyndolod.info/Help/Terrain-Underside]`

"A low-resolution terrain mesh, called the underside terrain," generated "for large worldspaces
that have LOD", placed "slightly under the full terrain" with its faces pointing the other way, so
that volumetric lighting / shadow mods (Enhanced Volumetric Lighting and Shadows, DVLaSS, Community
Shaders' sky sync) stop rays and shadows leaking through the landscape. Settings on that page:
Quality ("higher numbers improve performance but raise the chance the mesh pokes through the
terrain") and Height ("the negative distance below the actual terrain"). Troubleshooting there:
poke-through near the ground -> lower Quality or increase Height; holes -> check the cell record's
Force Hide Land flags. INI keys `[doc: DynDOLOD_SSE.ini]` `[golden DynDOLOD_SSE_Default.ini]`:
`DoTerrainUnderside=1`, `TerrainUndersideQuality=10`, `TerrainUndersideHeight=500`,
`TerrainUndersideIgnoreWorlds=Blackreach, DLC01SoulCairn, DLC2ApocryphaWorld,
ccBGSSSE067DeadlandsWorld, ccKRTSSE001QNWorld, JaphetsFollyWorld`,
`TerrainUndersideMinX/MinY/MaxX/MaxY_<EDID>` ("exclude terrain north and south of the playable
area"; Solstheim MaxX=63, MinY=-2, MaxY=26), `TerrainUndersideIgnoreHide_<EDID>` ("ignore hide quad
flag for specific cells to prevent holes"; MarkarthWorld lists 8 cells). xLODGen's
`Terrain-Underside-Readme.txt` `[doc]`: "a single NIF containing the terrain LOD meshes for LOD
level 32 (or LOD level 16) and their faces flipped", generated with `RemoveUnseenFaces=True`,
`ProtectCellBorders=False`, `Skirts=0` and "Optimize Unseen" off.

### 4.2 File as observed `[golden dyndolod-20261017-medium-tamriel, dyndolod-golden3]`

`meshes\Terrain\<World>\<World>_Underside.nif` (note the capital T, as DynDOLOD writes it):

```
0  BSFadeNode  unnamed, flags 0xE, identity; children = one BSMultiBoundNode per LOD32 quad
   per quad (6 blocks, in this order):
   BSMultiBoundNode  unnamed, flags 0xE, culling mode 0, children [shape], multi bound
   BSLightingShaderProperty  type 0, flags1 0x80001200 (ZBuffer_Test | Model_Space_Normals | Cast_Shadows),
                             flags2 0x00000001 (ZBuffer_Write), clamp 0, glossiness 1, specular (1,1,1) x 1,
                             lighting effects 0 / 0, alpha 1, emissive (0,0,0) x 1
   BSShaderTextureSet  [Textures\Terrain\<W>\<W>.32.<x>.<y>.dds, Textures\black.dds]
   BSTriShape  unnamed, flags 0xE, translation (x * 4096, y * 4096, 0), scale 32, vertex desc 0x0000300000000405
               (position + UV, 20 bytes), bound centre = local bbox centre, radius = distance to the max corner
   BSMultiBound -> BSMultiBoundAABB  world-space box of the shape: position = translation + local centre x 32,
               extent = local half size x 32, max z clamped to >= 0 as in 1.8
```

No string table entries, no skirts, no water. Tamriel: 16 quads (x in -64, -32, 0, 32; y in -64,
-32, 0, 32), 1,327..13,113 vertices and 2,329..26,026 triangles each; every triangle winds
clockwise seen from above (faces down). Vertices are quad-local as in the BTR (grid points of the
32-cell quad, UV = (x / 4096, 1 - y / 4096), error 0) and the mesh covers only cells present in the
input (quad (-64, 32): local x from 896 = 7 cells of fill skipped). The block order of the quads in
the golden file is the finishing order of the reference tool's threads and differs between runs;
write a deterministic order. Height field used: DynDOLOD's own terrain export, whose cell
rectangle is the worldspace's object bounds `[golden]`: the underside log says "Data from -57,-43 to
61,50", the export `.bin` has 119 x 94 = 11,186 records, and Skyrim.esm's Tamriel WRLD has
`NAM0 = (-233472, -176128)`, `NAM9 = (253952, 208896)` = cells -57..61 x -43..50 (MarkarthWorld:
146 records = all its CELLs). Artefact: the mesh ends one grid step short of the data on the north
and east sides (local x max 3836 instead of 3840 in quad (32, *), y max 2428 instead of 2432 in
row 32), the fill effect of 2.3; not required.

### 4.3 Export file DynDOLOD writes `[golden export\LODGen_SSE_Export_TerrainUnderside_<W>.txt]`

```
GameMode=UNDERTERRAINSSE
Worldspace=Tamriel
CellSW=-96 -96
LODLevel=32
[MinX= MinY= MaxX= MaxY=]          (only when the INI sets them; Solstheim: MaxX=63 MinY=-2 MaxY=26)
IgnoreHide=                        (MarkarthWorld: -44,-1;-44,0;-44,1;-43,-1;-43,0;-43,1;-42,-1;-42,0)
TerrainData=<path>\LODGen_SSE_LODGen_Terrain_<W>.bin
LandHeight=-27000  WaterHeight=-14000      (Solstheim: -4590 / 256)
RemoveUnseenFaces=True
ProtectCellBorders=False
QualityLOD32=10     (= TerrainUndersideQuality)
MaxVertsLOD32=32767
WaterDeltaLOD32=-1
Skirts=0
PathOutput=<folder>
```

So the underside is "the LOD32 terrain mesh with quality 10, no skirts, no water optimisation, no
protected borders, hidden quadrants removed (hide flags honoured except for `IgnoreHide` cells),
triangles over missing cells removed, faces flipped". Quality is in the same units as 3.3 (10
local units = 320 game units at LOD32), which is why the mesh can "poke through" and is placed
`TerrainUndersideHeight` below the world.

### 4.4 Plugin records that place it `[golden DynDOLOD.esm / DynDOLOD.esp]` `[ours: dynplugins.rs]`

Observed with `lodforge plugin-dump` on the golden plugins and already written by our own
`crates/dyndolod/src/dynplugins.rs` (keep):

- DynDOLOD.esm: `MSTT` `<W>_UNDERSIDE_DynDOLOD_BASERECORD`, record flags 0x4, `OBND`
  (-4096, -4096, -4096, 4096, 4096, 4096), `MODL Terrain\<W>\<W>_Underside.nif`, `DATA 00`.
- DynDOLOD.esp: `FLST` `<W>_UNDERSIDE_DynDOLOD_LIST` with `LNAM` = the worldspace and the child
  worldspaces that use its land (Tamriel: 0x3C, WhiterunWorld, WindhelmWorld, RiftenWorld,
  WhiterunDragonsreachWorld, SolitudeWorld, WindhelmPitWorldspace, KatariahWorld; MarkarthWorld,
  which has its own LOD, is not in it); a persistent `REFR` `<W>_UNDERSIDE_DynDOLOD_NOLOD` in the
  worldspace's persistent cell, flags 0x00010C00 (persistent 0x400 | initially disabled 0x800 |
  Is Full LOD 0x10000), `NAME` = the MSTT, `DATA` position (0, 0, -TerrainUndersideHeight) =
  (0, 0, -500), rotation 0, and a `VMAD` script `SHESON_DynDOLOD_ObjectEnabler` with the object
  property `MyWorldspaces` = the FLST (the script enables the reference while the player is in one
  of the listed worldspaces).

### 4.5 AnvilLOD as a GPL reference `[anvil]`

`src/AnvilLOD.Core/World/UndersideMesher.cs`: builds one underside shape per LOD32 block on a
regular lattice (default step 16 LAND vertices = 2,048 units, any divisor of 1,024 that is a
power of two from 4 to 128) where every lattice vertex takes the **lowest** LAND height within one
step around it, so the mesh never rises above the terrain between samples; squares without
terrain are skipped; neighbouring blocks share LAND vertices, so seams close without stitching;
triangles are wound to face down. `src/AnvilLOD.Meshes/UndersideBuilder.cs`: writes exactly the
layout of 4.2 (its comments document the same flags, 20-byte vertices, per-block AABB). Its plugin
writer places the mesh with its own ESL and a plain persistent full-LOD reference at z = -500
without the enabler script; for DynDOLOD compatibility keep our records of 4.4.

The rewrite may use either approach: (a) run the terrain mesher of section 3 at LOD32 with the
underside settings of 4.3 and flip the winding (closest to the golden file, tolerant comparison
possible), or (b) port AnvilLOD's conservative min-lattice (simpler, cannot poke through, no
golden parity). Record the choice; (a) is suggested so the BTR mesher is exercised once more, (b)
is acceptable if (a) shows poke-through in game. If AnvilLOD code is ported, keep CageTV's copyright
and GPL notice and list the file and commit in THIRD-PARTY-NOTICES.

## 5. Acceptance

### 5.1 Tolerant BTR comparison

Definition (reference implementation `Dump-GoldenBtr.py compare <dir A> <dir B> [--tol T]`, T in
local units, default 0 = exact):

1. Same set of files per level (every lattice quad of the exported rectangle).
2. Per file, land shape: the sets of vertex positions (rounded to T) and of triangles (as vertex
   position triples, rotated so the smallest vertex comes first, winding kept) are compared; report
   `common / max(|A|, |B|)` per level. Skirts are part of the sets.
3. Water: same number of shapes, same triangle count per shape and identical segment lists (LOD4).
4. Root AABB within T x L per component; shader fields and texture paths identical; vertex desc,
   flags, scale and translation identical.
5. Header strings may differ (author).

`apps/cli/src/btr_check.rs` is ours (first commit 5452c21, no LODGen logic: it counts triangles and
parses the reference log) but it is written against the old crate's API and against the `.bin` /
`.txt` inputs; replace it by a `lodforge btr-compare` that implements the definition above
(or extend `e2e_compare.rs`, ours, which only buckets terrain files by name today).

### 5.2 "Before" numbers (the port being replaced)

- Tamriel, 1,755 / 1,760 BTRs byte-identical to the golden run (2026-10-17; re-measured 2026-10-19
  with the current `target/release/lodforge.exe`: 1,755 identical, 5 differ: `32.-32.0`,
  `8.8.-64`, `8.-16.-16`, `8.-40.0`, `8.32.8`). The 5 are the reference tool's own run-to-run
  variance (a Terra-style `locate` steps randomly on ties, `Subdivision.cc` `[terra]`, and three
  xLODGen runs disagree on the same files), so the byte-identical ceiling is about 1,755.
- Other worldspaces (xLODGen golden of 2026-10-17): 1,271 / 1,274 byte-identical across 10 worlds.
- Timing, i7-12700K, 20 threads, release build, 2026-10-19: Tamriel 20,608 cells exported in 67
  ms, 1,760 quads built in 1,772 ms, files written in 941 ms, total 2.9 s (2.2 s recorded on
  2026-10-17; the reference tool: 24 s for the meshes alone). All five seasons 2.9 s (geometry built
  once, texture paths rewritten).
- Underside (DynDOLOD golden, 2026-10-18): Tamriel 16 / 16 quads with identical vertex and
  triangle sets, byte-identical in the golden's quad order.

### 5.3 "After" targets

Byte identity is not expected. A rewrite is accepted when, for Tamriel and one small worldspace
(MarkarthWorld or DLC01FalmerValley):

- every file parses with `crates/nif` and with `Dump-GoldenBtr.py dump`, with the block structure
  of section 1 (type order, names, flags, vertex descs, shader values, AABB rule, bound rule);
- the file set equals the golden set; water shapes, segments and water triangle counts are
  identical (they follow from the input);
- `fit`: per-quad max error above water <= Quality in local units at every level, no off-grid
  vertex, vertex heights equal to the source heights (except the documented Optimize Unseen offsets
  if reproduced);
- LOD4: no triangle straddles an interior cell line; no T-junction on interior cell lines (better
  than the golden 0.45 %);
- `seams`: coincident border vertices identical on both sides (golden: 0 within-1-unit mismatches);
  "present on one side only" at or below the golden percentages, or 0 with shared border
  simplification;
- land vertex and triangle sums per level within +-10 % of 3.10, or a documented reason;
- `compare --tol 0.5` against the golden: report the percentages (no threshold; the old port's
  99.7 % is not a target);
- timing: not worse than 2 x the port (about 4 s for Tamriel on this machine); seasons still built
  once;
- underside: Tamriel 16 quads, faces down, covers the exported cells, placed by the records of 4.4;
- in-game check (5.4) passes.

### 5.4 In-game check list

Profiles: skyrim-cs `LODForge-MVP-Test` (or a copy of Default with the new output as a mod);
launch and capture procedure as recorded in the project memory (dismiss the Survival Mode prompt,
hide the tutorial menu, `tgm`, `tg`, free camera). Vantage points: Solitude shore (player
-52200, 115500, -13450; camera -50000, 117500, -13300), the tundra east of Whiterun (14197,
-19369, -3942), Winterhold north, plus one spot on a quad corner (e.g. cell (0, 0)) and one coast.
Check, with the LOD textures of the same build:

1. No holes, flipped triangles or missing quads at any distance; LOD4 -> LOD8 -> LOD16 -> LOD32
   transitions without jumps larger than the texture blur (compare the same view with the golden
   output).
2. Seams: no visible cracks or light leaks along quad borders and LOD4 cell borders at the edge of
   the loaded cells; skirts not visible from above.
3. Water: distant water planes at the right height, no z-fighting of shallow terrain with water
   (coast lines around Solitude and the Sea of Ghosts), no terrain LOD floating above the water.
4. Underside: with EVLaS or Community Shaders volumetric lighting active, no sun rays or shadows
   through mountains from below; nothing poking through the terrain or the terrain LOD in the
   distance; the Tamriel underside present in Whiterun / Solitude city worldspaces.
5. `tll` toggles the terrain LOD off: what remains is objects only (sanity check that the files
   are the ones loaded).

### 5.5 "After" numbers (rewrite, 2026-10-20, branch `terrain-lod-rewrite`)

Measured with the commands of 5.1 / 5.3 on the same golden runs (`lodforge terrain-meshes
--golden`, `lodforge btr-compare`, `Dump-GoldenBtr.py stats / seams / fit / compare`); release
build, i7-12700K, 20 threads.

Tamriel, default season, settings of 2.6:

| Level | files | land vertices min / p50 / max / sum (golden sum, delta) | land triangles sum (golden, delta) | files with water / water tris | `fit` max error above water: p50 / p90 / max (local) | `fit` all points max (golden) |
|---|---|---|---|---|---|---|
| 4 | 1,312 | 79 / 1,684 / 8,239 / 2,871,420 (3,080,121, -6.8 %) | 5,352,241 (5,756,569, -7.0 %) | 787 / 21,444 (= golden) | 5 / 5 / 5 | 10.1 (7.56, see note) |
| 8 | 336 | 417 / 1,274 / 9,896 / 665,249 (683,702, -2.7 %) | 1,225,435 (1,261,054, -2.8 %) | 238 / 22,468 (= golden) | 8 / 8 / 8 | 306 (306) |
| 16 | 88 | 417 / 2,161 / 13,421 / 275,702 (280,044, -1.6 %) | 519,920 (528,355, -1.6 %) | 70 / 24,516 (= golden) | 8 / 8 / 8 | 395 (395) |
| 32 | 24 | 417 / 4,326 / 23,326 / 143,003 (140,547, +1.7 %) | 276,655 (272,097, +1.7 %) | 22 / 28,612 (= golden) | 10.2 / 10.2 / 10.2 | 286 (286) |

- `stats`: every vertex of every land shape on the source grid (`fit`: 0 off-grid of 391,196
  checked, heights = source except the Optimize Unseen offsets at LOD32); skirt depths 319 /
  256 / 193 / 130; 0 of 5,352,241 LOD4 triangles straddle an interior cell line; UV deviation
  0; root AABB rule and bound-sphere rule hold in 1,760 / 1,760 files; block types, strings,
  shader, texture paths, vertex descs, flags, scale and translation identical to the golden in
  every file (`btr-compare`: shader / textures / shape header 1,760 / 1,760).
- `seams`: 0 positions present on one side only at every level (golden: 729 / 204 / 71 / 127),
  z identical at 66,117 / 17,274 / 4,922 / 1,323 of as many positions. LOD4 interior cell
  lines: no T-junction (unit test; golden 0.45 %).
- `fit` note: the LOD4 "all points" maximum of 10.1 sits on the north row of a quad where the
  LAND records of the two cells sharing the row disagree; the dump's per-quad grid takes the
  southern cell's value, the mesh (7.5) the northern cell's. Above-water maxima equal the
  thresholds exactly, as the golden's.
- Water: shapes, segments and triangle counts identical to the golden in 1,760 / 1,760 files
  (they follow from the input).
- `compare --tol 0.5` against the golden (set agreement, no target): land vertices 61.8 / 48.5 /
  38.7 / 39.4 %, land triangles 28.7 / 20.4 / 13.6 / 11.3 % at LOD 4 / 8 / 16 / 32; land sets
  identical in 0 / 94 / 18 / 8 files; root AABB within 0.5 x L in 1,281 / 310 / 72 / 22 files (the
  rest differ by the lowest skirt vertex of a differently chosen border set, up to 2.5 local).
- Timing (output on the D: drive; the C: temp folder doubles the write time): Tamriel 20,608
  cells exported in 46 ms, 1,760 quads built in 1.3-1.5 s, files written in 0.3-0.9 s, total
  2.1-2.5 s (before: 2.2-2.9 s); with four extra seasons 8,800 files, total 2.8-3.3 s (before
  2.9 s). All 14 LOD worldspaces (6,173 quads) in one loop: about 12 s. `lodforge build
  --no-objects --no-trees --seasons SPR,SUM,AUT,WIN` for Tamriel: terrain-meshes stage 6.3 s on
  the C: temp drive, textures 18 + 4 x 14 s, 82 s in all, 0 failed stages.
- DLC01FalmerValley (xLODGen golden of `skyrim-cs\lod-output`): 70 / 70 files; land vertex
  sums 154,400 / 47,178 / 20,533 / 9,468 (compare totals 158,762 / 47,646 / 20,586 / 9,404);
  water equal 70 / 70; `fit` above water 5 / 8 / 8 / 10.2; `seams` 0 one-sided; `compare`
  vertices 72.5 / 53.7 / 40.7 / 27.8 %.
- Underside (approach (a)), `lodforge terrain-underside` from the load order vs the DynDOLOD
  golden: Tamriel 16 / 16 quads, input 11,186 cells (-57..61 x -43..50 = the export dump),
  91,450 vertices / 167,496 triangles (golden 81,062 / 157,213), faces down, built in 0.9 s;
  textures, shader and shape headers identical; AABB x/y equal except the north / east extent,
  where the golden stops one grid step short of its data (4.2); vertex sets 21,266 in common.
  Solstheim 8 / 8 quads (clip keys applied), DLC01FalmerValley, DeepwoodRedoubtWorld and
  MarkarthWorld 1 / 1, every input rectangle equal to DynDOLOD's dump. The plugin records are
  unchanged (`dynplugins.rs`).
- Tests: `cargo test --workspace` green (nif: 12 unit + 6 build + 4 golden round-trip tests;
  btr: 27 unit + 3 golden tests; the golden assembly round trip reproduces all 1,760 BTRs byte
  for byte from the `Btr` description).
- In-game check (5.4), 2026-10-20, skyrim-cs profile `LODForge-TerrainRewrite-Test` (a copy of
  `LODForge-MVP-Test` with the mod `LODForge Terrain Rewrite Test` on top: the 1,760 Tamriel
  BTRs of this build and its `Tamriel_Underside.nif`, the MVP build's textures and plugins):
  five free-camera captures through skyrim-mcp, the tundra east of Whiterun looking west and
  east, the LOD beyond the loaded cells around the cell (0, 0) quad corner, the Solitude shore
  (spec camera) and the Sea of Ghosts coast from above. No holes, missing quads, cracks at quad
  or cell borders, floating terrain or terrain poking through the water planes were visible;
  the distant islands sit in the water LOD at the right height. Not done: the `tll` toggle
  (the game stopped answering and exited after the toggle), the
  Winterhold vantage, a volumetric-lighting check of the underside, and a side-by-side with the
  golden output. Captures: the shared notes folder of the session group,
  `terrain-rewrite-shots\`.

## 6. Process record

- Date: 2026-10-19. Analyst session started from the licensing/provenance session; nothing
  committed; no Rust touched; no other document edited.
- Read for this spec (allowed for the analyst, forbidden for the implementer): `crates/btr/src/*`,
  `crates/nif/src/{btr,writer,lib}.rs`, `apps/cli/src/{terrain_meshes,terrain_underside,btr_check,
  dyndolod_build}.rs`, `docs/baseline.md`, `docs/parity.md`, `docs/re/lodgen-318-delta.md`, the
  terrain lines of `docs/re/dyndolod-spec.md` and the gap audits, the project memory. The
  decompiles under `D:\Modlists\ref\lodgen-*` were **not** opened in this session; the port's
  comments were enough to know what to exclude.
- Read and used (allowed inputs): the golden runs `no-seasons-20261017-031139`,
  `seasons-20261017-032143`, `lodgen-terrain-rerun`, `dyndolod-20261017-medium-tamriel`,
  `dyndolod-golden3`; xLODGen's `Terrain-LOD-Readme.txt`, `Terrain-Underside-Readme.txt`,
  `LODSettings-File-Readme.txt`, `Terrain-Heightmap-Readme.txt`, `Readme.txt`; the `[SSE LOD
  Options]` keys and their 2026-10-17 backups; the per-worldspace `LODGen_Terrain_*.txt` control
  files; `DynDOLOD_SSE.ini`; dyndolod.info/Help/Terrain-Underside; Terra 0.7 (`README.html`,
  `GreedyInsert.*`, `Heap.*`, `Map.*`, `Subdivision.*`, `Quadedge.*`, `greedy.cc`, `cmdline.cc`);
  `nif.xml` 028cb35b; AnvilLOD `UndersideMesher.cs`, `UndersideBuilder.cs`, `UndersideStage.cs`,
  `UndersidePluginWriter.cs`; our `export.rs`, `crates/esp` (`world.rs`, `terrain.rs`), the nif
  reader and `tests/golden.rs`, `dynplugins.rs`, `plugin-dump`, `vfs-extract`, `inspect`.
- Excluded, copied verbatim to `D:\Modlists\ref\quarantine\terrain-lod-lodgen-notes.md` (outside
  the repository): every comment line of `crates/btr/src/{lib,terra,quad,geometry,underside,output,
  bin}.rs`, `crates/nif/src/{btr,writer}.rs`, the LODGen lines of `crates/nif/src/lib.rs` and of the
  three CLI files, `docs/baseline.md` lines 400-437 ("LODForge P2: terrain LOD meshes"),
  `docs/re/lodgen-318-delta.md` lines 581-593 ("4. Terrain"), plus the analyst's list of the
  LODGen-internal rules deliberately left out of this spec.
- Helper added: `scripts/golden/Dump-GoldenBtr.py` (reader + measurements, written from `nif.xml`
  and the observed `.bin` layout; the implementer may use it).
- **The implementer must delete unread, as its first action:** `crates/btr/src/terra.rs`,
  `crates/btr/src/quad.rs`, `crates/btr/src/geometry.rs`, `crates/btr/src/underside.rs`,
  `crates/btr/src/output.rs`, `crates/btr/src/bin.rs`, `crates/nif/src/writer.rs`,
  `crates/nif/src/btr.rs`, and in `crates/nif/src/lib.rs` the LODGen-default items
  (`LODGEN_CREATOR`, `VertexDesc::lodgen_terrain`, `VertexDesc::lodgen_sse`,
  `TriShape::set_lodgen_vertex_desc`, `LightingShader::lodgen_terrain`, the `Default` impls that
  cite LODGen, `f32_to_half` / `float_to_byte` / `byte_to_float` as written, and the module header
  lines that name LODGen routines); `crates/btr/src/lib.rs` is to be rewritten (its two
  header lines are LODGen-naming). `crates/btr/src/export.rs` stays (ours). `apps/cli/src/
  btr_check.rs` and the two CLI wrappers are ours but call the deleted API; rewrite them against the
  new one.
- **Allowed inputs for the implementer:** this spec, `nif.xml` (add a copy under
  `D:\Modlists\ref\nifxml\`), Terra 0.7 sources, the golden folders named above, xLODGen's readmes
  and control files, DynDOLOD's INI and dyndolod.info, AnvilLOD's underside files (GPL,
  attribution), `crates/nif` reader + `tests/golden.rs`, `crates/esp`, `crates/btr/src/export.rs`,
  `crates/dyndolod/src/dynplugins.rs`, `scripts/golden/Dump-GoldenBtr.py`. Not allowed: everything
  in licensing.md "Research material that implementers must not read", including `D:\Modlists\ref\
  quarantine\` and the port sections of `docs/baseline.md`; the project memory's LODGen-internal
  sentences (quarantine file, "Not copied, pointers only") should be scrubbed or ignored.
- **Sections of other documents to replace by pointers to this spec later** (not edited in this
  pass because other sessions were editing them): `docs/baseline.md` "LODForge P2: terrain LOD
  meshes (.btr)" (lines 400-437; keep the timing table and the export fidelity bullets, drop the
  port description and the nondeterminism bullet) and the "Terrain underside" row of the gap pass
  (fine as is); `docs/re/lodgen-318-delta.md` section 4 "Terrain" (whole file goes to the B0.2
  quarantine); `docs/re/dyndolod-spec.md` "Building the height map [decomp]" item 1 (the terrain
  export; the occlusion scrub owns it) and the `TerrainUnderside*` INI rows (fine, documented keys);
  `docs/parity.md` has nothing to replace.

## 7. Open questions for the implementer (to settle empirically)

1. **Threshold at LOD4 inside a cell**: 3.3 shows the per-cell build meets Quality in local units;
   whether the cell border profile is simplified with the same threshold or kept dense is not
   observable from the golden files beyond the border-point statistics of 3.2. Choose, measure
   with `seams` / `fit`, and check the seam in game.
   - *Decision (2026-10-20):* the cell border profile is simplified as a 1-D greedy problem with
     the same threshold (Quality in local units) and pre-inserted on both sides; the remaining
     border points are never candidates. `seams`: 0 one-sided positions at every level (golden:
     1.1-9.5 %); `fit`: per-quad maximum above water = threshold at p50 and p90.
2. **Optimize Unseen values other than 550 / on / off**: the raise/lower fit of 3.7 comes from one
   setting. If the option is exposed with other values, run xLODGen once with e.g. 100 and 1000 at
   LOD32 (`scripts/golden/Run-XLodGenGolden.ps1`, chunk option, LOD32 only) and re-measure.
   - *Decision (2026-10-20):* implemented from the readme's intent with d = value / level local
     units: grid points deeper than d below their water are never candidates (value 0: every wet
     point), wet points within d of the surface are pushed to `water - d` and stay candidates
     (the clamped shelf is flat and the shoreline step keeps its detail), dry vertices are raised
     by d / 100 (the observed 0.17 at LOD32, which keeps a vertex sitting exactly at the water
     height above the plane). No xLODGen run with another value was made; `fit` at LOD32 gives
     the same above-water maximum (10.2) and the same under-water maxima as the golden
     (306 / 395 / 286 local at LOD8 / 16 / 32).
3. **Skirt depth for other `Skirts` values**: only 256 is observed (319 / 256 / 193 / 130). A
   negative or different value needs one xLODGen run to see the mapping; or expose only the
   observed default.
   - *Decision (2026-10-20):* `skirt_depth(Skirts, L) = Skirts + (Skirts / 4 - 1) x (1 - k)`,
     k = 0..3 for LOD 4..32 (= 319 / 256 / 193 / 130 for 256); 0 or negative = no skirts. Other
     values are an extrapolation of the observed line, unverified. Every border edge gets its
     own two lowered vertices (the observed 128 skirt vertices of a minimal LOD8+ quad), facing
     away from the quad.
4. **`HideQuads=True` for BTRs**: not in any golden xLODGen run (always False). The underside shows
   what "hidden quadrant removed" looks like at LOD32 (MarkarthWorld golden3 with `IgnoreHide`);
   a BTR with holes has no golden. Implement from the readme sentence and verify in game only if
   the option is exposed.
   - *Decision (2026-10-20):* implemented through the same coverage mechanism as the underside
     (`mesh.rs`): grid points inside an uncovered quadrant are never candidates, every grid
     point on the boundary of an uncovered region is pre-inserted (so no triangle straddles it)
     and triangles whose centroid lies in an uncovered quadrant are dropped together with the
     vertices only they used. Exposed through the `HideQuads` key; unit-tested, not checked in
     game.
5. **Cell-border rows where adjacent LAND records disagree** (the 119 / 169 / 69 / 22 "z differs"
   seam positions and the LOD4 7.56 outlier): decide which cell's value a shared row takes (the
   golden takes the later cell in (y, x) order for the LOD8+ fill artefact, and shows +2..+6 unit
   mismatches at LOD4 cell borders). Any consistent rule is fine; document it.
   - *Decision (2026-10-20):* one world-wide height sampler (`input.rs`): a grid point on a
     shared cell line belongs to the cell to the north / east (the later cell in (y, x) order);
     when that cell is missing the other side is used, so the exported rectangle's north and
     east edges carry the real cell's data, not the fill (the artefact of 2.3 is not
     reproduced). Every quad, cell problem and border profile reads the same sampler, which is
     why the seams agree.
6. **DynDOLOD's underside input rectangle**: observed equal to the WRLD object bounds (NAM0/NAM9)
   for Tamriel and to the full CELL set for MarkarthWorld; `TerrainUndersideMin/Max` keys clip it
   further. Confirm on Solstheim (golden3: MaxX=63, MinY=-2, MaxY=26) before relying on NAM0/NAM9.
   - *Decision (2026-10-20):* confirmed. The cells inside the WRLD object bounds, a max corner
     on a cell line excluded (Tamriel NAM9 = (253952, 208896) -> cells up to 61, 50), give
     exactly the records of DynDOLOD's export dumps: Tamriel 11,186, Solstheim 34,557
     (-64..127 x -64..127, before the MinY / MaxX / MaxY clip), DLC01FalmerValley 607
     (-16..10 x -13..13), DeepwoodRedoubtWorld 99, MarkarthWorld 146. The Min/Max keys clip the
     input afterwards (Solstheim: 8 quads, as the golden).
7. **Underside approach** (4.5): mesher-at-LOD32 vs AnvilLOD's min-lattice; decide after the first
   in-game check with volumetric lighting on.
   - *Decision (2026-10-20):* (a), the BTR mesher at LOD32 with the settings of 4.3 and the
     winding flipped (`underside.rs`); no AnvilLOD code was ported. In-game check: see 5.5.
8. **Water shape vertex order at LOD8+** and **LOD4 water shape merging**: the golden merges
   shared corners within a shape; whether the game needs the per-cell segment order at LOD4 beyond
   what 1.7 states is untested (segments are observed, their consumer in the engine is not
   documented). Keep the observed layout.
   - *Decision (2026-10-20):* LOD4 shapes are built in slot order (dx x 4 + dy) with the four
     corners of each cell merged against the shape's earlier cells, segments exactly as 1.7
     (trailing empties dropped); LOD8+ holds one shape with the cells in (dx, dy) order merged
     by (x, y, z). Water shapes, segments and triangle counts agree with the golden in
     1,760 / 1,760 Tamriel files.
9. **Point budget behaviour**: Tamriel never reaches `MaxVerts`; a worldspace with extreme terrain
   (or a low MaxVerts setting) would. Terra's loop stops at the point limit; whether skirts are
   then still added (the log's "2000 reserved") must be decided so that the shape stays below
   65,535 vertices.
   - *Decision (2026-10-20):* the point limit of a quad is `MaxVerts - 2000` (split equally over
     the 16 cell problems at LOD4); skirts are always added; a quad whose vertices exceed
     65,535 with the skirts is an error (never reached with the defaults: Tamriel's largest
     quad has 23,326 vertices). The build reports quads that hit their budget (none).
10. **Seasons**: geometry is season-independent (1.1); a season build only rewrites the two
    texture strings. Confirm the writer keeps block sizes correct when the path length changes.
    - *Decision (2026-10-20):* the writer encodes the blocks once (`writer::encode`), re-encodes
      the texture set block per season (`Encoded::replace_block`) and assembles each file with
      its own block sizes; `golden_btr_texture_block_replacement` checks that this equals a
      full write on golden files. Tamriel with four extra seasons: 8,800 files in 2.8-3.3 s.

## 8. Process record of the implementation (stage B1, 2026-10-20)

- Date: 2026-10-20, implementer session started from the orchestrating session in its own
  worktree (branch `terrain-lod-rewrite`, cut from `main` at 0f7f1e5). First action: `git rm`
  of `crates/btr/src/{terra,quad,geometry,underside,output,bin}.rs` and
  `crates/nif/src/{writer,btr}.rs` without opening them (commit 4066870); the LODGen-named
  items of `crates/nif/src/lib.rs` were replaced by nif.xml / golden-derived ones
  (`VertexDesc::sse` / `with_attributes` compute the offset nibbles from the bitfield,
  `f32_to_half` is IEEE round-to-nearest-even through the `half` crate, `CREATOR` is
  LODForge's; `LightingShader::lodgen_terrain` moved to `crates/btr/src/btr.rs` as the
  observed terrain shader of 1.6).
- Inputs read: this spec; `nif.xml` 028cb35b (`D:\Modlists\ref\nifxml\`); Terra 0.7
  (`GreedyInsert.*`, `Heap.*`, `Map.*`, `Subdivision.*`, `Quadedge.*`, `greedy.cc`, `Geom.h`,
  `Vec2.h`, `Array.h`, `output.cc`, `terra.h`, README); xLODGen's `Terrain-LOD-Readme.txt`,
  `Terrain-Underside-Readme.txt`, `LODSettings-File-Readme.txt`, the control files
  `Edit Scripts\LODGen_Terrain_*.txt`, the golden run logs' option lines; the golden runs
  `no-seasons-20261017-031139`, `dyndolod-20261017-medium-tamriel`, `dyndolod-golden3` (exports,
  logs, output); the xLODGen output of the other worldspaces under
  `D:\Modlists\skyrim-cs\lod-output\xLODGen`; `scripts/golden/Dump-GoldenBtr.py`; our
  `crates/nif` reader and its tests, `crates/esp` (`lib.rs`, `terrain.rs`, the `Worldspace` API),
  `crates/btr/src/export.rs`, `crates/btr/src/lib.rs` (two lines), the CLI files
  `main.rs`, `build.rs`, `profile.rs` (arguments), `terrain_meshes.rs`, `terrain_underside.rs`,
  `btr_check.rs`, the `terrain_bin` function and the underside call sites of
  `dyndolod_build.rs` (grep), the `WorldDynamic` API of `dynplugins.rs` (grep);
  `docs/reimplementation-plan.md`, `docs/licensing.md`, the sibling sessions' hand-over notes.
- Never opened: the deleted modules (in any worktree, any revision), `D:\Modlists\ref\lodgen-*`,
  `ref\net10-numerics`, `ref\skyrimse-*`, `ref\quarantine\`, `tools\rea-snapshots`,
  `docs/re/lodgen-318-delta.md`, `docs/re/gap-audit-*`, `docs/parity.md`,
  `docs/re/dyndolod-spec.md`, `docs/baseline.md`, `crates/objlod/src` (only identifier names
  and compiler messages without source lines were seen; three blind `sed` renames were applied
  to it), `crates/dyndolod/src/occlusion.rs`, `crates/texgen/src/billboard.wgsl`, the
  LODGen-internal sentences of the project memory. No web search for LODGen sources.
- AnvilLOD: none of its files were opened or ported (approach (a) of 4.5), so no CageTV
  attribution is needed for this stage.
- Third-party algorithm: `crates/btr/src/vcache.rs` is Tom Forsyth's "Linear-Speed Vertex
  Cache Optimisation" (2006) written from the published description, because
  `crates/objlod` (not to be read, rewritten in stage B2) imports a triangle-ordering function
  from this crate; the other compatibility surface it needs (`bounds::BBox` with `grow` /
  `grow_box` / `center` / `radius`, `terrain_bin::decode_cell` with a `[col][row]` 33 x 33
  height array, `VertexDesc::with_attributes`) is written here from the spec; the `[col][row]`
  order of the compatibility record follows the layout suggested in 2.2 and should be verified
  by the stage B2 session before it relies on it.
- Modules written (all new text): `crates/nif/src/writer.rs` (serializer, two-phase encoding),
  `crates/btr/src/{input,settings,quadedge,greedy,profile,mesh,bounds,btr,terrain_bin,
  underside,compare,vcache}.rs`, `crates/btr/src/export.rs` (reworked onto `TerrainInput`),
  `apps/cli/src/{terrain_meshes,terrain_underside,btr_compare}.rs` (`btr-check` replaced by
  `btr-compare`), tests `crates/nif/tests/{golden,build}.rs`, `crates/btr/tests/golden.rs`.
- NIF writer API for the object LOD rewrite: `lodforge_nif::writer::write(&Nif)` or
  `Nif::to_bytes`; `writer::encode(&Nif) -> Encoded` + `Encoded::replace_block` +
  `Encoded::to_bytes` for file families; blocks are the reader's `Block` enum (`Node` with
  `culling_mode: Some` for BSMultiBoundNode, else NiNode unless the header's block type table
  names the type, e.g. BSFadeNode; `TriShape` with `segments: Some` for BSSubIndexTriShape and
  `lod_sizes` for BSMeshLODTriShape; `LightingShader` with the type-specific `extra`;
  `EffectShader`; `TextureSet`; `Alpha`; `MultiBound` / `MultiBoundAabb`; `LargeRefExtra`;
  `Unknown` + `raw_blocks` for verbatim copies). `VertexDesc::sse(attributes)` builds the vertex
  desc; `TriShape::update_vertex_desc` from the filled vectors. Strings and block types are kept
  from the header when present, else named in first-use order.
