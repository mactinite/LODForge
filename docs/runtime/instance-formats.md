# LODForge Runtime instance formats (v1.0)

Status: **v1.0, 2026-10-24.** Written by LODForge (`crates/instances`), read by the LODForge
Runtime SKSE plugin. This document is the contract between the two; when the code and this
document disagree, the document wins and the code is a bug. Changes follow the versioning rules in
section 2.

Three files, one container:

| File | Magic | Record | Sections | What it replaces |
|---|---|---|---|---|
| `<World>[.<SEASON>].trees.bin` | `LFTR` | 24 bytes | one per cell | trees in `.btt` / `.bto` (impostor tier, M2) |
| `<World>[.<SEASON>].statics.bin` | `LFST` | 32 bytes | one per LOD block and level | repeated meshes in `.bto` (3D tree LOD first, M3) |
| `<World>[.<SEASON>].lfg` | `LFGR` | 10 bytes | one per cell | far grass (M4), LOD4 grass in `.bto` |

All live in `Data\SKSE\Plugins\LODForgeRuntime\`. `<World>` is the worldspace EditorID as the
plugins spell it (`Tamriel`, `DLC2SolstheimWorld`). `<SEASON>` is absent for the default season,
else one of `SPR`, `SUM`, `AUT`, `WIN` (upper case, as Seasons of Skyrim names its files). A season
file is complete on its own: the runtime loads the season's file instead of the default one, never
both. If a season file is missing the runtime uses the default file.

Reference implementation: `crates/instances/src/{container,trees,statics,grass,hash}.rs`. Its
round-trip tests and the test vectors in section 8 pin the byte layout.

## 1. Conventions

- **Little-endian** everywhere. No implicit padding: every offset below is explicit.
- `iN`/`uN`: two's-complement / unsigned integers of N bits. `f32`: IEEE 754 binary32. `f16`:
  IEEE 754 binary16 (round to nearest even when written).
- **Offsets** are absolute byte offsets from the start of the file.
- **str**: `u16 byte_length` followed by that many UTF-8 bytes; no terminator.
- **Paths** are Data-relative, lower case, with backslashes: `meshes\landscape\trees\treepineforest01.nif`.
- **Cells**: Skyrim exterior cells are 4096 units. Cell `(x, y)` covers world
  `[x*4096, x*4096+4096) x [y*4096, y*4096+4096)`; a position belongs to cell
  `(floor(px/4096), floor(py/4096))`.
- **Cell-relative position** (`qx`, `qy`, u16): `q = clamp(round((p - cell*4096) * 16), 0, 65535)`,
  so `p = cell*4096 + q/16` with 1/16-unit resolution. (A value that rounds to 65536 is stored as
  65535; the error is at most 1/16 unit.)
- **Angles** (u16): `a = round(radians / (2*pi) * 65536) mod 65536`; back: `radians = a * 2*pi/65536`.
  Angles are the reference's `DATA` rotation exactly as the plugin stores it (what
  `TESObjectREFR::GetAngleX/Y/Z` return), not negated or normalised by the writer.
- **Reference rotation**: the world transform of a reference is `v' = T(pos) * R * S(scale) * v`
  with `R = Rx(-ax) * Ry(-ay) * Rz(-az)`, column vectors, `Rz(t) = [c -s 0; s c 0; 0 0 1]` (the
  convention object LOD is built with, `crates/objlod/src/transform.rs`). For a tree, `R = Rz(-yaw)`.
- **Form identity**: a record names its reference as `(plugin, form)`: `plugin` is an index into the
  file's `PLUG` table (section 3.2), `form` is the *local* form id inside that plugin (the low 24
  bits for a full plugin, the low 12 bits for a light plugin; the load-order byte is never stored).
  The runtime resolves it with `TESDataHandler::LookupFormID(form, pluginName)` (or
  `LookupForm`). This survives load-order changes that keep the plugins. Synthetic references
  (none in v1.0) would use `plugin = 0xFFFF`.

## 2. Versioning

- `major` changes when an existing field changes meaning or position. Readers **reject** a file
  whose major differs from theirs.
- `minor` grows when something is added that old readers can ignore: a new meta chunk, a new flag
  bit that is only an optimisation hint, a field in a reserved slot that was zero before. Readers
  accept any minor of their major; writers write the highest minor they implement.
- `header_size` and `record_size` are stored so a later minor can grow the header or append fields
  to a record. A reader uses the stored `record_size` as the stride and reads only the fields it
  knows. Readers reject a `record_size` smaller than the v1.0 size.
- Unknown meta chunk tags are skipped (their size is stored).

## 3. Container

### 3.1 File header (64 bytes)

| Offset | Type | Field | Notes |
|---|---|---|---|
| 0 | u8[4] | magic | `LFTR`, `LFST` or `LFGR` (ASCII) |
| 4 | u16 | major | 1 |
| 6 | u16 | minor | 0 |
| 8 | u32 | header_size | 64 |
| 12 | u32 | flags | bit 0: at least one section is zstd-compressed. Other bits 0. |
| 16 | u32 | record_size | 24 (trees), 32 (statics), 10 (grass) |
| 20 | u32 | section_count | |
| 24 | u64 | index_offset | section index, `section_count` entries of 32 bytes |
| 32 | u64 | meta_offset | meta block (section 3.2) |
| 40 | u32 | meta_size | bytes |
| 44 | i16 | min_x | smallest section x (0 when there are no sections) |
| 46 | i16 | min_y | |
| 48 | i16 | max_x | largest section x, inclusive |
| 50 | i16 | max_y | |
| 52 | u32 | data_crc32 | CRC-32 (IEEE, as zlib) of bytes `[64, file_size)`; 0 = not computed |
| 56 | u64 | total_records | sum of `record_count` over all sections |

The writer lays the file out as: header, meta block, section payloads, section index. Readers must
use the offsets, not this order.

### 3.2 Meta block

A sequence of chunks, `meta_size` bytes in total, starting at `meta_offset`. Each chunk:

| Offset | Type | Field |
|---|---|---|
| 0 | u8[4] | tag (ASCII) |
| 4 | u32 | size of the payload in bytes (excluding these 8 bytes and the padding) |
| 8 | u8[size] | payload |
| | | zero padding to the next multiple of 4 |

Chunks in v1.0 (each at most once; order as listed):

**`WRLD`** (all files): `str world` (EditorID), `str season` (`""`, `SPR`, `SUM`, `AUT`, `WIN`),
`str generator` (free text, e.g. `LODForge 0.2.0 runtime-exporters`).

**`PLUG`** (all files): `u32 count`, then `count` x `str file_name` (as in `plugins.txt`, e.g.
`Skyrim.esm`, `ccBGSSSE001-Fish.esm`). Record and type `plugin` fields index this table. The table
lists only the plugins some record or type uses, in load order at build time.

**`PARM`** (all files): `str text`, `key=value` lines separated by `\n` (build parameters for
diagnostics, e.g. `min_copies=8`). Never needed to read the data.

**`TTYP`** (trees): the tree types. `u32 count`, then per type:

| Offset | Type | Field |
|---|---|---|
| 0 | u16 | base_plugin (`PLUG` index of the base record's defining plugin) |
| 2 | u16 | flags: bit 0 the base is `TREE` (else a `STAT` with the has-tree-LOD flag) |
| 4 | u32 | base_form (local form id of the base) |
| 8 | u32 | model_crc32 (CRC-32 of the full model file bytes as the load order provides it; 0 if unreadable) |
| 12 | i16[6] | bounds: the base's `OBND` (x1, y1, z1, x2, y2, z2), unscaled |
| 24 | u32 | references (number of records of this type in the file) |
| 28 | str | model (full model, Data-relative, lower case: `meshes\landscape\trees\treepineforest01.nif`) |
| | str | lod4 (the LOD4 column the object LOD would have used: a 3D LOD model path, a billboard `.dds`, or `""`) |

Type indices are file-local (a season file has its own table). **Join key with the impostor
manifest:** `(model, model_crc32)`. Two bases with the same model are separate types (they can
differ in `OBND` and season swaps) but share an impostor.

**`MESH`** (statics): `u32 count`, then per mesh:

| Offset | Type | Field |
|---|---|---|
| 0 | f32[6] | bounds: AABB of the mesh's vertices in model space (min xyz, max xyz) after its own node transforms |
| 24 | u32 | flags: bit 0 a tree LOD model (used by tree references), bit 1 passthru (`PassThruMeshMask` / `passthru` in the name), bit 2 the mesh has vertex alpha < 255 somewhere |
| 28 | u32 | triangles (all shapes of the mesh) |
| 32 | str | model (`meshes\...`, the LOD model path from the export's LOD column) |

**`GTYP`** (grass): `u32 count`, then per grass type:

| Offset | Type | Field |
|---|---|---|
| 0 | u16 | plugin (`PLUG` index of the GRAS record's defining plugin; 0xFFFF if it could not be resolved in the build's load order) |
| 2 | u16 | flags: bit 0 fitsToSlope, bit 1 uniformScaling, bit 2 vertexLighting (as the `.cgid` stores them) |
| 4 | u32 | form (local form id of the GRAS record; the cache's load-order form id when `plugin` is 0xFFFF) |
| 8 | f32 | wave_period |
| 12 | str | model (`meshes\...`, lower case) |

### 3.3 Section index

`section_count` entries of 32 bytes at `index_offset`, **sorted by (level, y, x)** ascending (signed
compare), so a reader can binary-search or fill a dense grid over `[min_x..max_x] x [min_y..max_y]`.
No two entries share (level, x, y).

| Offset | Type | Field |
|---|---|---|
| 0 | i16 | x (cell; for statics the block's south-west cell, as in `.bto` names) |
| 2 | i16 | y |
| 4 | u8 | level: 0 for trees and grass; 4, 8, 16 or 32 for statics |
| 5 | u8 | flags: bit 0 the payload is one zstd frame |
| 6 | u16 | reserved, 0 |
| 8 | u64 | offset of the payload |
| 16 | u32 | stored_size (bytes in the file) |
| 20 | u32 | raw_size (bytes after decompression; = stored_size when not compressed) |
| 24 | u32 | record_count |
| 28 | u32 | raw_crc32 of the uncompressed payload (0 = not computed) |

A compressed payload is a single standard zstd frame (any level; content size in the frame header
is optional). Payloads start at 4-byte-aligned offsets. Readers can map the whole file and stream
sections on a worker thread: one seek and one read per section.

## 4. Hashes

The runtime thins instances by hash: an instance is kept at density level `k` (fraction `2^-k`)
iff `hash < 2^(32-k)` (level 0 keeps everything). Every section stores its records **sorted by hash
ascending** (statics: by mesh, then hash), so the kept set of level `k` is a prefix (per mesh run
for statics) and the sets of higher levels nest inside those of lower levels.

```
fmix32(h):            // MurmurHash3 finaliser
  h ^= h >> 16; h *= 0x85EBCA6B; h ^= h >> 13; h *= 0xC2B2AE35; h ^= h >> 16; return h

fnv1a32(bytes):       // offset 0x811C9DC5, prime 0x01000193
  h = 0x811C9DC5; for b in bytes: h ^= b; h *= 0x01000193; return h

ref_hash(plugin_name, form) =                      // trees, statics
  fmix32(fnv1a32(ascii_lowercase(plugin_name)) ^ fmix32(form ^ 0x9E3779B9))

grass_hash(cell_x, cell_y, qx, qy) =               // grass
  fmix32(fmix32((u16(cell_x)) | (u16(cell_y) << 16)) ^ (qx | (qy << 16)) ^ 0x5BD1E995)
```

All arithmetic is u32 with wrap-around; `u16(v)` is the low 16 bits of the two's-complement cell
coordinate. `ref_hash` depends only on the plugin *name* and local form id, so it does not change
when the load order does; `grass_hash` depends only on the stored position, so the runtime can
recompute it from a record.

## 5. Trees (`LFTR`)

Every tree reference the runtime draws as an impostor. LODForge leaves these references out of the
`.bto` and `.btt` it writes in runtime mode; the runtime draws them until the reference's 3D loads.

**Which trees** (the same filters as the DynDOLOD-mode export and tree LOD): references of `TREE`
bases and of `STAT` bases with the has-tree-LOD flag, in the worldspace (after the season's base
swaps), that the export would give object or tree LOD, i.e. not deleted, not initially disabled,
not behind an enable parent (unless DynDOLOD's `xesplod` list allows the parent), not ignored by
EditorID, `mod_world_ignore` or a reference rule, with a rule that gives them some LOD; **and not**
tilted more than `TreeLODMaxTilt` on x or y, not a large reference, not a child-worldspace copy and
not inside a child worldspace's tree object-LOD bounds. Those stay in object LOD (their `.bto`
shapes are unchanged). Trees that DynDOLOD handles as dynamic LOD (enable-parent trees) are not in
the file.

Section payload: `record_count` records of 24 bytes, sorted by `hash` ascending.

| Offset | Type | Field |
|---|---|---|
| 0 | u16 | qx (cell-relative x, 1/16 unit) |
| 2 | u16 | qy |
| 4 | f32 | z (world) |
| 8 | u16 | yaw (angle encoding of the reference's z rotation) |
| 10 | f16 | scale (the reference's `XSCL`, 1.0 without one) |
| 12 | u16 | type (`TTYP` index) |
| 14 | u16 | plugin (`PLUG` index of the plugin that defines the reference form) |
| 16 | u32 | form (local form id of the reference) |
| 20 | u32 | hash (`ref_hash(PLUG[plugin], form)`) |

## 6. Repeated statics (`LFST`)

Meshes that LODForge exports as instances instead of merging them into the `.bto` of a block.
One section per object LOD block and level that has instances: `level` 4, 8, 16 or 32 and `(x, y)`
the block's south-west cell (the numbers in `<World>.<level>.<x>.<y>.bto`). The runtime shows a
section when the engine shows that block at that level (and hides it when the block is hidden or
split), so the instances swap together with the merged shapes of the same block.

**Which meshes** (`PARM` records the values used):
- In runtime mode, every reference in `trees.bin` whose LOD column for the level is a 3D model
  (not a billboard `.dds`): its model is an instance, whatever the count (`tree_refs=1`). Billboard
  columns of those references are dropped (the impostor replaces them).
- Any other model listed in a block at least `min_copies` times at that level, when its path
  contains one of `model_masks` (default `passthru`, i.e. the 3D tree LOD models such as
  `...treepineforest01passthru_lod.nif`; empty = every model). `min_copies=0` disables this rule.
- Never instanced: billboards (`.dds` columns), large references (`-LargeRef`), and shapes that use
  side-file alternate textures or vertex-colour overrides for that reference (they cannot share a
  mesh).

An instanced reference keeps its LOD exactly as the merged shapes would: the runtime draws `mesh`
with the record's transform (section 1). Textures, atlas remaps and vertex colours come from the
mesh file itself (the merged `.bto` would have remapped them into the object LOD atlas; the runtime
binds the mesh's own textures).

Section payload: `record_count` records of 32 bytes, **sorted by mesh, then hash**.

| Offset | Type | Field |
|---|---|---|
| 0 | f32 | x (world) |
| 4 | f32 | y |
| 8 | f32 | z |
| 12 | u16 | ax (angle encoding of the x rotation) |
| 14 | u16 | ay |
| 16 | u16 | az |
| 18 | f16 | scale |
| 20 | u16 | mesh (`MESH` index) |
| 22 | u16 | plugin (`PLUG` index of the reference; 0xFFFF for synthetic references such as DynDOLOD's `ICLOD`) |
| 24 | u32 | form (local form id; 0 for synthetic) |
| 28 | u32 | hash (`ref_hash`; for synthetic references `fmix32` of the record's position bits) |

## 7. Far grass (`LFGR`, `.lfg`)

The grass instances of the cells, from the NGIO grass cache (`Grass\<world>x<X>y<Y>[.<season>].cgid`).
One section per cell with grass.

Section payload (uncompressed form):

| Offset | Type | Field |
|---|---|---|
| 0 | i16 | x (cell) |
| 2 | i16 | y |
| 4 | f32 | z_min |
| 8 | f32 | z_step: world `z = z_min + qz * z_step` |
| 12 | u16 | type_count (1..255) |
| 14 | u8 | level_count (8 in v1.0) |
| 15 | u8 | reserved, 0 |
| 16 | u32[level_count] | prefix: `prefix[k]` = number of records with `hash < 2^(32-k)`; `prefix[0] = record_count` |
| 16 + 4*level_count | u16[type_count] | types: cell type index -> `GTYP` index |
| ... | record[record_count] | 10 bytes each, at offset `16 + 4*level_count + 2*type_count` (2-byte aligned) |

Density level `k` reads the first `prefix[k]` records (`k = 0` all, 1 half, 2 a quarter, ... 7 =
1/128). Records are sorted by `grass_hash(x, y, qx, qy)` ascending; equal hashes keep cache order.

Record (10 bytes):

| Offset | Type | Field |
|---|---|---|
| 0 | u16 | qx (cell-relative, 1/16 unit) |
| 2 | u16 | qy |
| 4 | u16 | qz (`z = z_min + qz * z_step`) |
| 6 | u8 | type (index into the cell's `types`) |
| 7 | u8 | yaw: `yaw = v * 2*pi/256` |
| 8 | u8 | scale: `scale = v / 64` (0 .. 3.98) |
| 9 | u8 | tilt: high nibble `nx`, low nibble `ny`, each a signed 4-bit value `n = v/7` (-1..1) |

Reconstruction: the surface normal is `n = (nx, ny, sqrt(max(0, 1 - nx^2 - ny^2)))` (`tilt = 0` is
upright). The instance's rotation is `B(n) * Rz(yaw)` where `B` is an orthonormal basis with `n` as
its z axis (for `n = +z`, `B = I`). Scale applies uniformly (the `.cgid` height variation `h` gives
`scale = 1 + h`). The `.cgid` shade value is not stored; the runtime can derive per-instance tint
variation from the hash.

How the writer derives them from a `.cgid` instance (16 halves, `docs/re/grass-cache-spec.md` 4):
world `x = half[0] + 12*trunc(cell_x/12)*4096` and likewise `y`; `z = half[2]`; the rotation matrix
`r` (row-major, stored in the order `r0 r1 r2 r7 r3 r4 r5 r8 r6`) gives `n = (r2, r5, r8)` and
`yaw = atan2(r3, r0)`; `h = half[13]`.

## 8. Test vectors

```
fmix32(0)                          = 0x00000000
fmix32(1)                          = 0x514E28B7
fnv1a32("")                        = 0x811C9DC5
fnv1a32("skyrim.esm")              = 0xE330F2F3
ref_hash("Skyrim.esm", 0x012345)   = 0x997DE556   (plugin name case does not matter)
grass_hash(-3, 7, 100, 200)        = 0x6D7E02AF
angle(pi) = 0x8000, angle(-pi/2) = 0xC000, angle(2*pi) = 0
q(4096*3 + 100.5) in cell 3        = 1608
q(-1.0) in cell -1                 = 65520
```

`crates/instances/src/hash.rs` pins these; `crates/instances/src/tests.rs` pins a complete small
trees file byte for byte (`golden_trees_bytes`) and the `write -> read` round trip of all three
kinds, raw and zstd.

## 9. Sizes and limits

- Tamriel on skyrim-cs-base (measured, zstd 3): 93,544 trees -> 2.31 MB; 46,006 3D tree LOD
  instances -> 1.21 MB; 29.7 M grass instances -> 236 MB (297 MB raw). Details in
  [exporters-results-20261024.md](exporters-results-20261024.md) and
  [grass-lfg-sizes.md](grass-lfg-sizes.md).
- Limits: 65,535 tree types / meshes / grass types per file, 255 grass types per cell, 2^32 - 1
  records per section, cell coordinates in i16.
