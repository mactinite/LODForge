# LODForge Runtime instance formats (v1.6)

Status: **v1.6, 2026-10-24** (1.0-1.5 earlier the same day; see "History" at the end). Written by LODForge (`crates/instances`), read by the LODForge
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
| 0 | u8[4] | magic | `LFTR`, `LFST`, `LFGR` or `LFMP` (ASCII) |
| 4 | u16 | major | 1 |
| 6 | u16 | minor | 6 |
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

**`TTYP`** (trees): the tree types. **They are the worldspace's impostor types, one entry per
`types[i]` of `textures\LODForge\Impostors\<World>\impostors.json`, in the same order**
(docs/runtime/impostors.md): a record's `type` is directly the impostor type index, and every
season file of the worldspace carries the same table (one impostor set covers all seasons).
`u32 count`, then per type:

| Offset | Type | Field |
|---|---|---|
| 0 | u32 | model_crc32 (CRC-32 of the rendered model file; = `impostors.json` `model_crc32`) |
| 4 | i16[6] | bounds: `OBND` of the type's first base used in this file (x1, y1, z1, x2, y2, z2), unscaled; zeros when no base of the type is used here |
| 16 | u32 | references (number of records of this type in the file) |
| 20 | u16 | base_count |
| 22 | u16 | flags: bit 0 some base is a `TREE` record (else `STAT`s with the has-tree-LOD flag) |
| 24 | str | name (= `impostors.json` `types[i].name`, e.g. `treepineforest01_1a2b3c4d`; check it to detect a mismatched impostor set) |
| | str | model (the rendered model, = `impostors.json` `model`) |
| | str | lod4 (the LOD4 column the object LOD would have used: the most used 3D LOD model among the type's references in this file, else the most used column (a billboard `.dds`), else `""`) |
| | (u16, u32)[base_count] | bases drawn with this type in this file: `PLUG` index, local form id |

A type with `references = 0` exists only to keep the indices aligned with the impostor set.
Trees whose base has no baked impostor type are not in the file; they stay in object LOD.

**`TMSH`** (trees, since 1.2): the **near tier**: each type's LOD4 3D tree model, which the
runtime draws instanced at the type's records (the same placements as the impostors; nothing is
duplicated in `.statics.bin`). Exactly one entry per `TTYP` entry, same order. `u32 count`, then
per type:

| Offset | Type | Field |
|---|---|---|
| 0 | u16 | flags: bit 0 the type has a mesh; bit 1 generated by this build (LODForge tree models, `--tree-models generate`); bit 2 passthru model (keeps its own full textures, no atlas) |
| 2 | u16 | shape_count |
| 4 | u32 | triangles (all shapes) |
| 8 | f32[6] | bounds: model-space AABB (min xyz, max xyz), node transforms applied |
| 32 | str | model: the LOD4 3D model (= `TTYP.lod4` when that is a `.nif`), `""` when the type has none (its LOD4 is a billboard or nothing: the runtime uses the impostor near too) |
| | shape[shape_count] | see below |

Shape:

| Offset | Type | Field |
|---|---|---|
| 0 | u16 | alpha_flags (`NiAlphaProperty` flags; 0 without one) |
| 2 | u8 | alpha_threshold |
| 3 | u8 | flags: bit 0 has an `NiAlphaProperty`; bit 1 has vertex colours |
| 4 | u32 | triangles |
| 8 | str | name (shape name in the model) |
| | str | diffuse: slot 0, Data-relative lower case, **resolved**: the path the model names if the load order has it, else the only texture in the load order with that file name, else `""` |
| | str | normal: slot 1, resolved the same way |

The model is transformed exactly like the impostor (section 1, `R = Rz(-yaw)`, uniform scale);
the tree models are authored at reference scale 1 around the trunk base, like the full tree.
`TMSH` is absent when no type has a mesh.

**`TMPK`** (trees, since 1.3): `u32 count` (= `TTYP` count), then per type a `u32`: the index of
the type's merged near-tier mesh in `TreeMeshes.bin` (section 7a), `0xFFFFFFFF` when it has none.
Present when the build wrote a mesh pack; the runtime then draws the near tier from the pack and
uses `TMSH` only for diagnostics.

**`MESH`** (statics): `u32 count`, then per mesh:

| Offset | Type | Field |
|---|---|---|
| 0 | f32[6] | bounds: AABB of the mesh's vertices in model space (min xyz, max xyz) after its own node transforms |
| 24 | u32 | flags: bit 0 a tree LOD model (used by tree references), bit 1 passthru (`PassThruMeshMask` / `passthru` in the name), bit 2 the mesh has vertex alpha < 255 somewhere, bit 3 (1.4) its instances are large references |
| 28 | u32 | triangles (all shapes of the mesh) |
| 32 | str | model (`meshes\...`, the LOD model path from the export's LOD column) |

Several `MESH` entries can name the same model (since 1.4): one per (model, large reference,
texture set). Records point at the entry, so a record's entry says whether it is a large reference
and which textures it uses.

**`MTEX`** (statics, since 1.4, present when some entry has textures): `u32 count` (= `MESH`
count), then per entry `u16 n` and `n` x (`str diffuse`, `str normal`): the textures of the model's
parts in order, from the reference's AltTextures entries (the model's own textures for parts
without one); `n = 0`: the model's own textures throughout.

**`MCLS`** (statics, since 1.5, present when some entry has snow/ash parts): `u32 count` (=
`MESH` count), then per entry `u16 n` and `n` x `u8`: per part, bits 0-6 the LOD material class (0
plain, 1 snow, 2 snowHD, 3 ash, 4 ashHD) and bit 7 set when the VC side file whitens the part's
vertex colours for this reference (RGB 255, alpha kept; already baked into the pack mesh, so
informational for the runtime); `n = 0`: all plain, colours as stored.

**`SMPK`** (statics, since 1.4): `u32 count` (= `MESH` count), then per entry a `u32`: its mesh in
`StaticMeshes.bin` (section 7a), `0xFFFFFFFF` for none. Entries that differ only by the large
reference flag share a pack mesh.

**`GTYP`** (grass): `u32 count`, then per grass type:

| Offset | Type | Field |
|---|---|---|
| 0 | u16 | plugin (`PLUG` index of the GRAS record's defining plugin; 0xFFFF if it could not be resolved in the build's load order) |
| 2 | u16 | flags: bit 0 fitsToSlope, bit 1 uniformScaling, bit 2 vertexLighting (as the `.cgid` stores them) |
| 4 | u32 | form (local form id of the GRAS record; the cache's load-order form id when `plugin` is 0xFFFF) |
| 8 | f32 | wave_period |
| 12 | str | model (`meshes\...`, lower case) |

**`GMPK`** (grass, since 1.6, present when the build wrote `GrassMeshes.bin`): `u32 count` (=
`GTYP` count), then per type 20 bytes:

| Offset | Type | Field |
|---|---|---|
| 0 | u32[3] | pack mesh per tier in `GrassMeshes.bin` (section 7a): 0 full grass model, 1 simplified, 2 card; `0xFFFFFFFF` for none |
| 12 | u8[4] | mean albedo of the type's covered diffuse texels (sRGB RGB; texels below alpha 128 skipped when the shape alpha-tests), A = covered share (0..255) |
| 16 | f32 | model height (bounds z extent), model units |

Tiers: **full** is the grass model as the game draws it (vertex colours and alpha test kept,
the threshold remapped onto the page's 128). **Simplified**: LODForge's mesh simplifier with
a 3-unit error limit, at least a third of the triangles kept. **Card**: two crossed, double-sided
quads over the model's bounds (one in the XZ plane, one in YZ, through the bounds centre) showing
the model's first textured shape's whole diffuse (UV 0..1) with its alpha test, upward normals
(lit like the ground) and the shape's mean vertex colour. For "complex grass" models whose
diffuse is a sheet of many blades the card shows the whole sheet.

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
not inside a child worldspace's tree object-LOD bounds, **and its base has a baked impostor type**. Those stay in object LOD (their `.bto`
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
| 12 | u16 | type (`TTYP` index = impostor type index in `impostors.json`) |
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
- References in `trees.bin` are never merged into the `.bto`. By default (since 1.2) they are not
  in `.statics.bin` either: their near tier is the trees file's `TMSH` mesh per type
  (`tree_refs=0`). With `build --runtime-tree-statics` their LOD models are written here per
  block and level as before (`tree_refs=1`). Billboard columns are always dropped (the impostor
  replaces them).
- Any other model placed at least `min_copies` times **in the worldspace** at that LOD level
  (since 1.4; default 8 in runtime mode; counted per worldspace and level, not per block), when its
  path contains one of `model_masks` (default: every model) and the reference is eligible:
  - material plain, passthru, or (since 1.5, default on) snow/ash; no material object. Each part
    gets a LOD material class (`MCLS`: plain, snow, snowHD, ash, ashHD; HD by the same rule as
    the `.bto` shape names) and lands on pages of that class (`MPPG.class`);
  - not a glow-window model, not a billboard;
  - vertex-colour overrides from the VC side file are allowed since 1.5: such references get
    their own entry with whitened parts (`MCLS` bit 7).
  Large references are eligible (their `MESH` entry has flag bit 3; the runtime hides such an
  instance while the reference's own 3D is loaded, as the engine does for object LOD). References
  with AltTextures entries are eligible: they get their own `MESH` entry carrying the textures
  (`MTEX`). `min_copies=0` disables this rule.

An instanced reference keeps its LOD model as the merged shapes would: the runtime draws the
entry's pack mesh (`SMPK`) with the record's transform (section 1, full 3-axis rotation). The
pack mesh carries the model's (or `MTEX`) textures on the static atlas pages and the model's
vertex colours; hidden-face removal does not apply to instances. The section's level is the LOD
level; a reference has one record per level it has a model at (same plugin, form and hash), so the
runtime can pick the level per instance by distance and dither between them.

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


## 7a. Mesh packs (`LFMP`: `TreeMeshes.bin` since 1.3, `StaticMeshes.bin` since 1.4)

`GrassMeshes.bin` (since 1.6) has the same layout too: the three tiers of every grass type of the
build's `.lfg` files (mesh names `<model>`, `<model>#decimated`, `<model>#card`), plain page class,
pages `textures\lodforge\grassatlas\grassatlas_alpha[NN].dds`; `.lfg` files link to it with
`GMPK`. Grass meshes are small (often under 64 triangles), so their clusters carry more
padding.

`StaticMeshes.bin` has exactly the layout described below for `TreeMeshes.bin`; it holds every
`MESH` model (and texture variant) of the build's statics files, its pages are
`textures\lodforge\staticatlas\staticatlas_alpha[NN].dds` (+ `_n`), and statics files link to it
with `SMPK`. The rest of this section describes the tree pack; read "statics file" and `SMPK` for
the static pack.

`SKSE\Plugins\LODForgeRuntime\TreeMeshes.bin`: one file per build, shared by every worldspace and
season. It holds every near-tier tree model of the build's trees files (`TMSH`) merged for
GPU-driven drawing: one vertex buffer, one index buffer and a table of fixed-size clusters, all
models' textures on a few shared atlas pages. The runtime uploads the three buffers once and
draws every visible (tree instance x cluster) pair of one page with one indirect draw (vertex
pulling: `vertex = vertices[indices[cluster.first_index + 3 * tri + corner]]`).

Trees files link to it with `TMPK` (section 3.2): record `type` -> `TMPK[type]` = mesh index.

### Container use

Same container as the other files (`magic = "LFMP"`, `record_size = 28` = the vertex size).
Meta chunks: `WRLD` (world and season empty), `PLUG` (empty), `PARM` (`tile_cap`, `page`,
`alpha_threshold`), `MPPG`, `MPMS`. Exactly three sections, all with `x = y = 0`; the index
entry's `level` names the stream:

| level | Stream | Element | `record_count` |
|---|---|---|---|
| 1 | vertices | 28 bytes | vertex count |
| 2 | indices | u32, absolute vertex index | `3 * 64 * cluster count` |
| 3 | clusters | 40 bytes | cluster count |

**`MPPG`** (pages): `u32 count`, then per page: `u32 width`, `u32 height`, `u16 mips`,
`u8 alpha_threshold` (128: draw where `albedo.a >= 128/255`), `u8 class` (since 1.5; the LOD
material of every cluster on the page: 0 plain `obj`, 1 snow `objSnow` (SnowLODMaterial
00050372), 2 snowHD `objSnowHD` (0006AE38), 3 ash `objAsh`, 4 ashHD `objAshHD`; the runtime names
the page's proxy shape accordingly or sets that material. Vertex alpha carries the snow amount,
as in `.bto`; 0 before 1.5), `str albedo`,
`str normal` (Data-relative DDS, BC7 with full mips; pages are `textures\lodforge\treeatlas\
treeatlas_alpha[NN].dds` and `..._n.dds`). Static pages: `staticatlas[_snow|_snowhd|_ash|_ashhd]_alpha[NN].dds`, one page family
per class. Shapes without an alpha test get tiles with alpha 255 (their alpha is not coverage). Albedo RGB is the source diffuse (sRGB), A its alpha
remapped so that the shape's own alpha-test threshold lands on 128 (shapes with another
threshold get their own tile), with coverage-preserving mips and the average colour under the
cut-out (the object LOD page renderer). Normal pages hold the source normal maps
(tangent space, as the source), flat where a shape has none.

**`MPMS`** (meshes): `u32 count`, then per mesh: `u32 first_cluster`, `u32 cluster_count`,
`u32 first_vertex`, `u32 vertex_count`, `u32 triangles` (real), `f32 bounds[6]` (model-space
AABB), `str model` (the `TMSH` model it was made from).

### Vertex (28 bytes)

| Offset | Type | Field |
|---|---|---|
| 0 | f32[3] | position, model space (the tree's own space; the instance transform of section 1 applies) |
| 12 | u32 | normal: 10:10:10 unorm of `n * 0.5 + 0.5` (bits 0-9 x, 10-19 y, 20-29 z), bits 30-31 zero |
| 16 | u32 | tangent: same packing; bits 30-31 = 1 when the bitangent is `cross(n, t)`, 0 when it is `-cross(n, t)` |
| 20 | u16[2] | uv on the page, unorm16 (`uv = v / 65535`), already inside the texture's tile with a half-texel inset |
| 24 | u8[4] | vertex colour RGBA (opaque white when the source shape has none) |

### Cluster (40 bytes)

| Offset | Type | Field |
|---|---|---|
| 0 | u32 | first_index: its `3 * 64` indices start here (always `3 * 64 * cluster number`) |
| 4 | u32 | first_vertex of its mesh |
| 8 | u32 | mesh |
| 12 | u16 | page (`MPPG` index) |
| 14 | u8 | triangles: real ones (1..64); the rest of the 64 are degenerate (all three corners the same vertex) |
| 15 | u8 | flags, 0 |
| 16 | f32[3] | AABB min, model space |
| 28 | f32[3] | AABB max |

A mesh's clusters are contiguous, grouped by page, and within a page ordered along a Morton curve
of the triangle centroids, so each cluster is spatially compact (good AABBs for culling).

### How models are merged

Each `TMSH` model is loaded with node transforms applied; every shape whose UVs leave one texture
repeat is re-cut along integer UV lines (the object LOD ReUV), its texture placed on a page tile
(largest source mip that fits `tile_cap`, default 512 px), its UVs remapped into the tile, and
all shapes of the model merged into one mesh (one alpha mode: alpha test at 128 on every page).
Shapes without a texture are dropped.

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

## History

- **1.6** (2026-10-24): `GrassMeshes.bin` (three tiers per grass type) and the `GMPK` chunk in
  `.lfg` (tier meshes, mean albedo, height). Additive.
- **1.5** (2026-10-24): snow/ash references in the static pack: `MCLS` per-part classes, page
  class in `MPPG` (former reserved byte); opaque shapes get alpha-255 tiles. Additive.
- **1.4** (2026-10-24): static mesh pack `StaticMeshes.bin`; statics files instance repeated object
  LOD models per worldspace (`min_copies`, default 8), large references and AltTextures variants
  included; new `MESH` flag bit 3, `MTEX` and `SMPK` chunks. Additive.
- **1.3** (2026-10-24): `TreeMeshes.bin` (`LFMP`, section 7a): the near-tier models merged onto
  shared atlas pages and cut into 64-triangle clusters; `TMPK` chunk in trees files. Additive.
- **1.2** (2026-10-24): new `TMSH` chunk in trees files (the near-tier LOD4 tree mesh per type);
  `TTYP.lod4` is the most used 3D model of the type; runtime trees are no longer written to
  `.statics.bin` by default. Purely additive for readers of 1.1.
- **1.1** (2026-10-24): `TTYP` is the worldspace's impostor type list (impostors.json order, name,
  model, model_crc32, bounds, references, base list; section 3.2) and a record's `type` is the
  impostor type index. This changed the layout of an existing chunk, which section 2 reserves for
  a major bump; it was made a minor bump because no 1.0 file was released and the one reader
  (LODForge Runtime) handles both. Readers tell them apart by `minor`.
- **1.0** (2026-10-24): first version; `TTYP` had one entry per base (base_plugin u16, flags u16,
  base_form u32, model_crc32 u32, bounds i16[6], references u32, model str, lod4 str).
