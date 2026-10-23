# TreeLODGen algorithm specification (3D "passthru" tree LOD)

Provenance: source repository https://github.com/Cl3mus33/TreeLODGen, commit dc4cc8a (version 1.0.6 per
`TreeLODGen.csproj`), licence GPL-3.0, read 2026-10-23. Copyright (C) 2026 Clemus. LODForge is
GPL-3.0-only, so porting is allowed; any Rust port must carry attribution to TreeLODGen.

All `file:line` references are into that checkout (`D:\Modlists\ref\TreeLODGen`). "nifly" means the
C++ NIF library by ousnius, used through the `niflysharp` 1.3.0 C# binding; behaviour marked
**(nifly)** is library behaviour, not TreeLODGen code, and must be reproduced in `crates/nif`.

## 0. Pipeline overview

The production path is `Generate.Run` (Generate.cs:71) -> `Split.Process(..., hybridInputs: false)`
(Generate.cs:217). For every winning TREE model it writes, per level `L` in `0..levels.len()`:

```
meshes\dyndolod\lod\trees\<stem>_<CRC>passthru_lod[ _L].nif      (L=0: no suffix; L>=1: "_1", "_2")
```

each a flat static NIF: a `BSFadeNode` root with the crown shapes (pruned leaf cards) and the bark
shapes (edge-collapse simplified), all transforms baked into vertices.

The `split`/`hybrid`/`batch` commands (Split.cs:20, Hybrid.cs, Batch.cs) are developer commands for an
older HYBRID workflow (trunk billboard + crown). They are not used by the generator; see §10.

Third-party libraries (Rust replacements needed):

| C# dependency | Used for | Where | Rust in LODForge |
|---|---|---|---|
| niflysharp 1.3.0 (nifly, GPL-3.0) | NIF load/save, CloneShape, DeleteVertsForShape, CalcTangentsForShape, CreateShapeFromData, transforms, DeleteUnreferencedNodes, DeleteBlockByType | everywhere | `crates/nif` (must replicate the listed operations) |
| BCnEncoder.Net 2.3.0 | DDS decode (any BCn) to RGBA | Textures.cs:59-63 | `crates/dds` |
| System.IO.Hashing 10.0.12 | CRC-32 (IEEE 802.3, reflected 0xEDB88320, init/xorout 0xFFFFFFFF), same as zlib `crc32` | NifUtil.cs:133-136 | `crc32fast` or equivalent |
| K4os.Compression.LZ4.Streams | SSE BSA (LZ4 frame) extraction | Bsa.cs | `crates/archive` |
| System.IO.Compression.ZLibStream | compressed plugin records | Plugins.cs:88-94 | `crates/esp` |
| .NET `PriorityQueue<T,double>` | QEM collapse queue | Simplify.cs:83 | `BinaryHeap` (tie order differs, see §5.6) |

**No mesh simplification library is used.** The bark simplifier is TreeLODGen's own ~130-line QEM
half-edge collapse (Simplify.cs). It is *not* meshoptimizer and differs from `meshopt_simplify`
(collapse onto existing vertices only, area-weighted plane quadrics, explicit boundary planes, a
specific fold test, UV-seam corner hand-over). To match output, port Simplify.cs literally rather than
calling meshoptimizer.

## 1. Which trees are processed

### 1.1 Load order and records

* Load order: MO2 profile `loadorder.txt` filtered by `plugins.txt` stars (Plugins.cs:35-44), or
  base masters + `Skyrim.ccc` + starred `plugins.txt` (Plugins.cs:48-55). Then plugins named
  `DynDOLOD.*` and `Occlusion.esp` are removed (Generate.cs:155).
* MO2 mods excluded from the file system view (so their files never count as existing LOD or as
  source meshes): name starts with `TreeLODGen`, or contains `DynDOLOD Output`, `TexGen Output`,
  `AnvilLOD Output` (case-insensitive) (Generate.cs:135-137).
* Only the top-level `TREE` GRUP of each plugin is read (Plugins.cs:57-112). Per record: `EDID`,
  first `MODL`, `OBND` (6 x i16). Records with empty MODL are dropped. Winner = last plugin in load
  order for key `(owner master lowercased, formID & 0x00FFFFFF)`; owner = `masters[formID>>24]`, or
  the plugin itself when the index is past its master list (Plugins.cs:105-108).
* Records are grouped by `MODL.ToLowerInvariant()` (Generate.cs:189), groups sorted by key with
  the invariant-culture string comparer (not ordinal; port: ordinal is close enough unless exact
  report row order matters). One group = one model = one set of LOD files. The model path is used
  as-is (no slash normalisation); the mesh is looked up as `"meshes\" + key`.

### 1.2 Skip tests, in order (Generate.cs:205-213)

| # | Test | Status string written to report |
|---|---|---|
| 1 | no record in the group is LOD-sized | `too small for tree LOD` |
| 2 | model key does not contain substring `tree` | `not a tree for TexGen (no 'tree' in the model path)` |
| 3 | mesh not resolvable in VFS | `mesh not found` |
| 4 | `!--all` and the level-0 LOD name exists in the load order | `has 3D LOD (<source>)` |
| — | otherwise processed | `generated`, or the error text (§1.5) |

LOD-sized (Plugins.cs:9-13, "same test TexGen uses"), with OBND = `(x1,y1,z1,x2,y2,z2)`:

```
dx=x2-x1; dy=y2-y1; dz=z2-z1
sized = sqrt(dx²+dy²+dz²) >= 512.0  &&  z2 >= 256
```

A group is a candidate if **any** of its records is sized (Generate.cs:191).

### 1.3 "Already has 3D LOD" (Generate.cs:162-168, 212-213)

* `existingLod` = file names (no folder) of every loose or archived file under
  `meshes\dyndolod\lod\trees\` (recursive), except paths containing `\treelodgen\` (old output
  subfolder). First source wins (`TryAdd`).
* Names listed in the installed `meshes\dyndolod\lod\trees\TreeLODGen_models.txt` (one per line,
  trimmed) are removed, as is the list file itself: the tool's own previous output is not "existing".
* Test name = `"<stem>_<CRC>passthru_lod.nif"`, `stem` = file name without extension of the
  lowercased model key, `CRC` = the DynDOLOD lookup CRC (§1.4). Only the level-0 name is checked.
  Comparison is case-insensitive.
* `--all` bypasses this test.

### 1.4 CRC32 of the full model (Generate.cs:210-211, ParallaxGenDiff.cs)

* `meshCrc = CRC32(all bytes of the resolved mesh file)`; for archived meshes the bytes are the
  extracted (decompressed) file. Formatted `X8` (8 upper-case hex digits).
* `ParallaxGen_Diff.json` is resolved at the VFS root. It is a JSON object; for each member
  `name -> { "crc32original": u32, "crc32patched": u32, ... }` an entry
  `name -> (original, patched)` is stored, keys case-insensitive (ParallaxGenDiff.cs:9-21). Parse
  failures are a note, map empty.
* `crc = (map[ "meshes\" + key ] exists && entry.patched == meshCrc) ? entry.original : meshCrc`
  (ParallaxGenDiff.cs:25-26). The JSON key format is assumed to be `meshes\...` with backslashes.
* Files are named with `crc`. If `meshCrc != crc` (PGPatcher patched this exact mesh), every staged
  file `"<stem>_<crc>passthru_lod*.nif"` is also copied to the same name with `_<meshCrc>passthru_lod`
  (Generate.cs:232-234): DynDOLOD looks up the original CRC, AnvilLOD the installed one.

### 1.5 Errors from processing

`failed to load <path>` (Split.cs:78-83), `no geometry found (nothing to make a LOD model from)`
(Split.cs:131-136), `failed: <exception message>` (Generate.cs:218). An errored tree gets no files
(already written levels stay in staging, though: an exception mid-way can leave partial levels —
port should write all-or-nothing).

## 2. Geometry collection from the source NIF

### 2.1 Source shapes (Split.SourceShapes, Split.cs:208-216)

```
shapes   = every shape in the file with triangle count > 0       (nifly GetShapes: all shape blocks)
skinned  = shapes where IsSkinned()
fixed    = shapes where !IsSkinned()
if skinned empty or fixed empty: use shapes
else: use fixed  if  sum(tris fixed)*2 >= sum(tris skinned)  else use skinned
```

Rationale: vanilla trees hold the geometry twice (skinned for wind, static for distance); wind-only
mods have an empty/partial static copy.

**Not filtered at all** (verified by search of the source): `NiSwitchNode` children (every child of
every switch is included), hidden flags (`NiAVObject` flag 1), effect-shader shapes
(`BSEffectShaderProperty`), shapes with the `SLSF2_Tree_Anim` flag (constant `0x20000000` is
declared at NifUtil.cs:11 but never used), controllers. Only skinned vs static selection happens.
A port that wants to be stricter should do so deliberately, as a known divergence.

"Skinned only" (Split.cs:192): `SkinnedOnly = any selected part shape IsSkinned()`. With the rule
above this means the static copy was missing/insufficient. Effects: §8.6 (stand-ins), and the report
`billboard` column.

### 2.2 Transforms

Static shape (NifUtil.GlobalTransform, NifUtil.cs:25-35):

```
T = shape.transform
p = parent(shape)
while p != null and parent(p) != null:        # stop before the root
    T = p.transform ∘ T                        # ComposeTransforms: parent applied after child
    p = parent(p)
```

**The root node's transform is excluded**: the game replaces the root transform with the
reference's placement, so root scale/offset (common in Ulvenwald) must not reach the LOD.

Skinned shape (Split.cs:301-312): `T = inverse(GetShapeTransformGlobalToSkin(shape))` **(nifly**:
the skin's global-to-skin transform, i.e. `NiSkinData` overall transform, identity if none). Skinning
is then deleted from the clone (`DeleteSkinning`, also clears the vertex-desc skinned flag
**(nifly)**) and `SLSF1_Skinned` (`0x2`) is cleared from shaderFlags1. Vertices stay in bind pose.
Note the root transform is *not* excluded on this path.

`T` is put on the clone, then baked later (§7.6). nifly `MatTransform` = rotation 3x3 + translation +
uniform scale.

### 2.3 Old geometry blocks (Split.ToBSTriShape, Split.cs:369-405)

Any clone whose block name is not exactly `BSTriShape` (NiTriShape, BSLODTriShape,
BSSubIndexTriShape, BSMeshLODTriShape, BSDynamicTriShape, ...) is rebuilt: positions, UVs, normals
(if count matches), triangles and vertex colours (if `HasVertexColors` and count matches) are read
from the *source* shape, a new `BSTriShape` is made with `CreateShapeFromData` **(nifly)**, its default
shader deleted, the original shader and alpha property re-attached, vertex colours set
(`SetVertexColors(true)`), the old transform copied, tangents computed (`CalcTangentsForShape`
**(nifly)**), bounds updated. Vertex indices are preserved, so mixed-shape vertex subsets (§3.3)
still apply. Note `rebuilt as BSTriShape`.

## 3. Bark vs foliage classification (Split.cs:93-120, WoodSplit.cs)

Per source shape, first rule that applies:

| Step | Test | Result | Reason string |
|---|---|---|---|
| 1 | lowercased name contains any of `leaf`, `leaves`, `crown`, `frond`, `needle`, `foliage` (checked first) | crown | `name contains '<hint>'` |
| 2 | lowercased name contains `trunk` or `bark` | trunk | idem |
| 3 | shape has **no** `NiAlphaProperty` (presence only; flags not inspected) | trunk | `opaque shape` |
| 4 | otherwise `WoodSplit.Analyze` | per island: trunk, crown, or both ("mixed") | `texture alpha` / `uv tiling (texture not found)` |

(Split.cs:10-11, 218-228, 98-99.) No shader flag is used for classification.

### 3.1 WoodSplit.Analyze (WoodSplit.cs:24-74)

Returns empty (shape contributes nothing!) when triangle count is 0 or `uvs.len != verts.len`.

1. Texture = slot 0 path of the shape, loaded via the VFS (§3.4). Alpha threshold = the shape's
   `NiAlphaProperty.threshold`, else 128 (always present here, since step 3 filtered).
2. Islands by **index connectivity only** (union-find over triangle corners, no position welding —
   welding would glue leaf cards to the branch they touch) (WoodSplit.cs:38-42).
3. Per triangle `t`: `area = |cross(b-a, c-a)|/2` (f32), and an "opaque" bool:
   * texture found: `IsOpaque` — 28 barycentric samples, all must have alpha `>= threshold`:
     ```
     steps = 6
     for i in 0..=6: for j in 0..=6-i:
         w0 = (i+0.25)/6.75 ; w1 = (j+0.25)/6.75 ; w2 = 1-w0-w1   (skip if w2<0; never happens)
         uv = w0*uvA + w1*uvB + w2*uvC
         if alpha(uv) < threshold: return false
     return true
     ```
     (WoodSplit.cs:79-93; the 0.25/0.75 offsets pull samples inward away from atlas cell borders.)
   * texture missing/unreadable: opaque if **any** of the three UVs is "tiling":
     `u < -0.01 || u > 1.01 || v < -0.01 || v > 1.01` (WoodSplit.cs:76).
4. Per island: `ratio = opaqueArea / area` (0 if area 0).
   * texture found: wood iff `ratio >= 0.9`.
   * no texture: wood iff `ratio > 0.5 || island_tris > 200`.
   * ambiguous note when texture found and `0.3 < ratio < 0.9` and `tris > 50` (treated as foliage;
     at most 5 notes `ambiguous-piece:<name>` per shape) (WoodSplit.cs:59-62, Split.cs:115-119).
5. Vertices of wood islands -> `WoodVertices`, others -> `LeafVertices`; tri counts summed.
   `IsMixed = WoodTris > 0 && LeafTris > 0`.

Split then adds: a trunk part if `WoodTris > 0`, a crown part if `LeafTris > 0`. Each part carries
the vertex subset only when mixed (else `null` = whole shape) (Split.cs:110-111). A mixed shape
therefore appears twice in the output (as a crown shape and a trunk shape cloned from the same
source).

| Constant | Value | Where |
|---|---|---|
| OpaqueIslandRatio | 0.9 | WoodSplit.cs:10 |
| FallbackWoodIslandTris | 200 | WoodSplit.cs:11 |
| fallback opaque ratio | > 0.5 | WoodSplit.cs:60 |
| tiling UV tolerance | 0.01 | WoodSplit.cs:76 |
| sample grid steps | 6 (28 samples) | WoodSplit.cs:81 |
| ambiguous band | 0.3 < r < 0.9, > 50 tris | WoodSplit.cs:61 |
| default alpha threshold | 128 | WoodSplit.cs:35 |

### 3.2 Leaf "cards" = islands (Islands.cs:13-49)

Used by pruning (§4), on the crown clone. **Different from WoodSplit**: here vertices are welded.

* Union-find over vertex indices. Weld: key = `(round(x/0.01), round(y/0.01), round(z/0.01))` as
  i32 (`MathF.Round` = round-half-to-even); every vertex with the same key is unioned with the first
  one seen. Then each triangle unions `p1-p2`, `p1-p3`. Positions are in shape-local space (before
  baking).
* Island = the triangles whose `p1` root matches. Island order = order of first appearance while
  scanning triangles (C# `Dictionary` insertion order, which matters for stable sort ties).
* Per island: triangle indices, set of vertex indices, `Area = Σ triangle area` (f32), `Centroid =
  arithmetic mean of its unique vertex positions` (not area-weighted).

No quad/card detection: an island is any connected welded piece (a card, a twig cluster, ...).

### 3.3 Mixed-shape subset (Split.cs:324-329)

After cloning, if the part has a vertex subset, all other vertices are deleted with
`DeleteVertsForShape` **(nifly**: removes the listed vertices, drops every triangle that references
one, renumbers the remaining vertices and triangles keeping relative order; same for all per-vertex
arrays**)**.

### 3.4 Textures (Textures.cs)

* Lookup: the raw slot-0 string from the NIF is passed to the VFS resolver as a Data-relative path
  (Textures.cs:39-41). There is **no** normalisation: a path without the `textures\` prefix or with
  `/` is not found, and classification silently falls back to UV tiling. A port should normalise.
* Decode full DDS top mip with BCnEncoder (all BCn formats), then point-downsample by power-of-two
  `step` until `max(w,h)/step <= 2048`: stored pixel `(x,y)` = source pixel `(x*step, y*step)`
  (top-left of each block, no filtering) (Textures.cs:54-64). `Width/Height` keep the file size.
* `Texel(u,v)`: `u -= floor(u); v -= floor(v)` (wrap), `x = min(W-1, (int)(u*W))`,
  `y = min(H-1, (int)(v*H))`, no V flip (row 0 = v 0) (Textures.cs:17-21). Nearest sample, alpha =
  bits 24-31.
* Cache per resolved path; decode errors = missing texture.

## 4. Crown: leaf card pruning (Prune.cs, Split.cs:138-142, 342-346)

### 4.1 Effective keep per level

```
CrownTrisFull = Σ tris over all crown parts of the tree (after classification)
keep_eff[i] = max(keep[i], min(1.0, floor[i] / CrownTrisFull))     if CrownTrisFull > 0
floor[i]    = CrownFloors[min(i, CrownFloors.len-1)]
```

(Split.cs:141-142.) Pruning is applied to a crown shape only if `keep_eff < 1.0` (Split.cs:342); at
`keep_eff >= 1` the crown is copied untouched (no enlargement, no UV clamp). The floor uses the
whole-tree crown count but pruning runs **per shape** with the same share.

### 4.2 Prune.Apply(shape, keep, area) — per crown shape

1. `islands = Islands.Find(verts, tris)`; return unchanged if `< 2` islands.
2. Bounding box over **all** vertices of the shape: `minX,minY,minZ`;
   `size = max(maxX-minX, maxY-minY, maxZ-minZ)`.
3. Order islands by Morton code of the centroid (stable sort, ties keep island order):
   ```
   q(v) = clamp((int)((v - min)/size * 1023.0), 0, 1023)       # 10 bits, truncation toward 0
   spread(q) = Σ_{bit=0..9} ((q>>bit)&1) << (3*bit)
   morton = spread(qx) | spread(qy)<<1 | spread(qz)<<2          # u64
   ```
   (Prune.cs:99-109.) If `size == 0` the division gives NaN/inf; C# `(int)NaN` on x64 is
   `int.MinValue` -> clamped to 0. Port: guard `size <= 0` -> code 0.
4. Error-diffusion selection, deterministic, no RNG (Prune.cs:22-30):
   ```
   acc = 0.5
   for island in ordered: acc += keep; if acc >= 1: acc -= 1; keep it  else drop it
   ```
   (f32 accumulation.) Return unchanged if nothing kept.
5. Enlargement factor (Prune.cs:33-37):
   ```
   totalArea = Σ area(all islands); keptArea = Σ area(kept)
   scale = min(2.0, sqrt(max(1.0, totalArea * area / keptArea)))
   ```
   so kept cards are scaled so that kept area ≈ `area` × original area, never shrunk, at most 2x
   linear (4x area).
6. Outline profile (Prune.Profile, Prune.cs:61-97), built from **all** vertices of the shape
   (including dropped islands), in shape-local space:
   * `cx, cy` = mean x, mean y of all vertices; `minZ, maxZ`.
   * 24 height bands: `band(z) = clamp((int)((z-minZ)/max(1e-3, maxZ-minZ)*24), 0, 23)`.
   * `radius[b] = max over vertices in band b of sqrt((x-cx)²+(y-cy)²)` (0 for empty bands).
   * smoothed: `r'[i] = max(r[i], r[i-1], r[i+1])` (indices clamped to 0..23).
   * `Clamp(v)`: `z' = clamp(v.z, minZ, maxZ)`; `limit = r'[band(z')]`; `rad = radial distance of v`;
     if `rad <= limit || rad < 1e-3` return `(v.x, v.y, z')`; else scale x,y toward `(cx,cy)` by
     `limit/rad`, z = `z'`.
7. For each kept island, each of its vertices: `v' = Clamp(c + (v - c)*scale)` with `c` = island
   centroid (Prune.cs:39-48). Normals/tangents are not touched.
8. Delete all vertices of dropped islands (sorted ascending) via `DeleteVertsForShape`; update
   bounds (Prune.cs:51-53).

Then `ClampSmallUvOvershoot` (Split.cs:409-418): if every UV is inside `[-0.05, 1.05]` and at least
one is outside `[0,1]`, clamp all UVs to `[0,1]` (otherwise LODGen splits those triangles). If any UV
exceeds 0.05 overshoot, nothing changes.

Note: the profile axis is the shape's local Z. Shapes with a rotated parent transform are pruned in
their local frame before baking.

## 5. Bark simplification (Simplify.cs)

Called per bark shape with budget (Split.cs:333-341):

```
barkTotal = Σ tris over the bark shapes of this output file (after subset deletion)
target    = max(12, trunkTris * shapeTris / max(1, barkTotal))       # integer (long) arithmetic
```

`trunkTris` = level's third `--levels` field; ×2 (`BareTreeBudget`, Split.cs:232) for trees with no
crown part (Split.cs:146-153, note `bare tree: bark only LOD`).

Bark shapes are identified **by name** (`barkNames` holds the LOD names of trunk parts,
Split.cs:289, 333-337); a crown shape that ended up with the same name as a bark shape would also
be simplified. Port: track by part kind instead.

### 5.1 Early out
Return unchanged if `tris <= target` or `uvs.len != verts.len` (Simplify.cs:20).

### 5.2 Welding
`WeldGrid = 0.01`; key `(round(x/0.01), round(y/0.01), round(z/0.01))`; one *point* per distinct key
(position = first vertex's). Each vertex maps to its point. Faces are point triples; corners keep the
original vertex triples (attributes). Faces degenerate after welding (two equal points) are **dropped
from the output entirely** (Simplify.cs:39-43).

### 5.3 Quadrics (all f64)
10-element symmetric quadric per point
`[nx², nx·ny, nx·nz, nx·d, ny², ny·nz, ny·d, nz², nz·d, d²]` with `d = -n·p0`.

* `Normal(a,b,c)`: `n = (b-a)×(c-a)`; `len = |n|`; if `len <= 1e-12`: area 0, else `n/len`,
  `area = len/2`.
* For each face with area > 0: add plane (n, through vertex 0) × `area` to its three points; count
  each undirected edge's face uses.
* Boundary edges (used by exactly one face): for each such directed edge `a->b` of a face with area >
  0: `wall = (pb - pa) × n`; if `|wall| > 0` add plane `wall/|wall|` through `pa`, weight
  `area × 10.0` (`BoundaryWeight`), to points `a` and `b` (Simplify.cs:66-80).

### 5.4 Candidates
Half-edge collapse `from -> to` onto the existing point `to` (no optimal position). For every point,
`Offer(from)`: for each distinct neighbour `to` (points of faces in `facesOf[from]`, excluding
`from`), enqueue `(from, to, version[from], version[to])` with priority
`cost = vᵀ(Q_from + Q_to)v` at `v = pos[to]`:

```
Q0x² + 2Q1xy + 2Q2xz + 2Q3x + Q4y² + 2Q5yz + 2Q6y + Q7z² + 2Q8z + Q9
```

Min-priority queue. Initial order of offers: points `0..n`, neighbours in LINQ `Distinct` order of
`facesOf[from]` (a `HashSet<int>`, iteration order ~insertion order in .NET).

### 5.5 Main loop (Simplify.cs:91-131)
```
remaining = number of faces
while remaining > target and queue not empty:
    pop (from,to,vF,vT)
    skip if version[from]!=vF or version[to]!=vT or facesOf[from] empty
    # fold test
    for f in facesOf[from] not containing `to`:
        before = Normal(f).n ; after = Normal(f with from->pos[to])
        if after.area <= 0 or dot(before, after.n) < 0.2: reject (candidate discarded, not re-queued)
    # attribute targets at `to`: distinct original vertices used at corner `to` in alive faces
    targets = { corners[f][index of to in f] : f in facesOf[to], alive }
    for f in facesOf[from]:
        if f contains to: kill f (remaining -= 1, remove f from facesOf of its points)
        else:
            k = index of from in f
            uv = uvs[corners[f][k]]
            corners[f][k] = argmin_{a in targets} |uvs[a] - uv|²     # same side of the UV seam
            faces[f][k] = to ; facesOf[to].add(f)
    facesOf[from].clear()
    Q_to += Q_from
    version[from]++ ; version[to]++
    Offer(to); for each neighbour of to: Offer(neighbour)
```

Positions never move; collapses land on an existing vertex, so UV, normal, tangent, colour of the
surviving corners are original data. Normals are not recomputed (except later tangent calc when the
transform is baked, §7.6).

### 5.6 Output
Alive faces become triangles of original corner vertices; unreferenced vertices deleted
(`DeleteVertsForShape`), bounds updated. Log `simplified '<name>': tris A -> B`.

Determinism: fully deterministic in C#, but ties in the .NET `PriorityQueue` (4-ary heap, unstable)
and `HashSet` iteration order are implementation details. A Rust port with `BinaryHeap` will produce
slightly different but equivalent results unless ties are broken explicitly (suggest secondary key
`(from, to)` and accept the divergence).

| Constant | Value |
|---|---|
| WeldGrid | 0.01 (local units) |
| BoundaryWeight | 10.0 |
| fold dot threshold | 0.2 |
| degenerate normal | len <= 1e-12 |
| min per-shape budget | 12 tris |
| BareTreeBudget multiplier | 2 |

## 6. Vertex colours, normals, tangents

* VColor.cs is **diagnostic only** (prints averages, VColor.cs:7-22). No vertex colour is computed or
  altered anywhere. Colours present on the source BSTriShape are kept as-is by `CloneShape`
  **(nifly)**; for rebuilt old shapes they are copied (§2.3). So vertex alpha (often used by tree
  wind/AO) passes through unchanged; the `SLSF2_Vertex_Colors` flag is not touched.
* Normals: kept from source; on bake rotated by the transform (`ApplyTransformToDir`, rotation
  only in nifly) and renormalised if length > 1e-6 (NifUtil.cs:66-74). Pruning moves vertices without
  updating normals. Simplification keeps original normals.
* Tangents/bitangents: recomputed with `CalcTangentsForShape` **(nifly**: per-triangle UV-derived
  tangent accumulation, normalised**)** only when the shape's transform was non-identity (bake) or
  the shape was rebuilt from old geometry. Otherwise the source tangents stay.

## 7. Output NIF structure (Split.WriteStaticNif, Split.cs:275-364)

### 7.1 Root
`CreateNamedBSFadeNode(src header version, "Scene Root")` — a new NIF with the **source file's
header version** and a `BSFadeNode` root named `Scene Root`. Every shape is a direct child.

### 7.2 Shape order
For the 3D LOD: crown parts first, then trunk parts (`crownParts.Concat(trunkParts)`,
Split.cs:171), each in source-shape order. `index` (used in names) = position in that combined list.

### 7.3 Shape names (Split.LodShapeName, Split.cs:245-274)

DynDOLOD pairs LOD shapes with full-model shapes by name containment (for texture swaps) and reads
`Crown`/`Trunk` in the name for crown/trunk brightness. Candidates in order:

1. `own` if it contains `Crown`/`Trunk` (case-insensitive, per kind), else `own + " Crown"` /
   `own + " Trunk"`
2. `"Crown <index>"` / `"Trunk <index>"`
3. `"Leaves <index>"` / `"Bark <index>"`

First candidate that is not *mistaken* wins, else `"LOD shape <index>"`. Mistaken(name) = some other
source shape (any shape in the file, including skipped ones) with non-empty name, name different
from `own` (case-insensitive), whose name is a case-insensitive substring of the candidate, **and**
whose slot-0 texture differs (case-insensitive).

Bark only: append `" noatlas"` if **any** UV of the whole *source* shape (not just the bark subset)
is outside `[-0.05, 1.05]` in u or v (`TilingTolerance`, Split.cs:233, 268-272). DynDOLOD then
gives that shape its own texture instead of an atlas tile, so repeating bark works. There is no
`treepassthru` shape name; the `passthru_lod` marker is only in the file name.

### 7.4 Geometry block
`CloneShape` **(nifly)** copies the shape with its shader property (and texture set), alpha property,
skin and extra data, attached to the new root. The BSTriShape **vertex descriptor is the source's**
(not re-chosen): whatever the source had (position, UV, normal, tangent, colours, full/half
precision) is written back. Rebuilt old geometry gets nifly's `CreateShapeFromData` layout
(position, UV, normal if given, tangent; colours when set). Skin data is removed for skinned
sources (§2.2).

### 7.5 Shader and alpha
* The original `BSLightingShaderProperty` is kept, including flags, shader type, colours, and all
  texture slots, **except** when `PlainLodShader` finds something to remove (NifUtil.cs:99-126):

  | Detected | Condition |
  |---|---|
  | PBR | SLSF2 bit 23 (`0x00800000`, "Unused01", PGPatcher/CS PBR) |
  | environment map | SLSF1 bit 7 (`0x80`) or bit 17 (`0x20000`), or shader type 1 or 16 |
  | parallax | SLSF1 bit 11 (`0x800`) or bit 28 (`0x10000000`), or SLSF2 bit 24 (`0x01000000`), or type 3, 7, 11 |
  | glow map | SLSF2 bit 6 (`0x40`) or type 2 |

  If nothing detected: shader untouched (slots 2-7 and all flags stay). If something detected:
  * SLSF1 &= ~(bits 7, 11, 17, 28); if PBR also clear SLSF1 bit 0 (Specular).
  * SLSF2 &= ~(bits 6, 15 EnvMap_Light_Fade, 23, 24).
  * shader type set to 0 (Default) if it was 1, 2, 3, 7, 11 or 16.
  * Texture slots 2..7 cleared to `""`, except (only when not PBR) slot 2 if SLSF2 bit 25 (Soft
    Lighting) or bit 26 (Rim Lighting) is set, and slot 7 if SLSF2 bit 27 (Back Lighting) is set.
  * Note `shader:<what>` (e.g. `shader:PBR+parallax`).
* The SLSF1 Skinned bit is cleared for skinned sources. **Tree_Anim (SLSF2 bit 29) is not
  cleared**, nor Vertex_Colors.
* `NiAlphaProperty`: the source's, unchanged (flags and threshold). Shapes without one stay without.
* Port note: LODForge targets CS + PBR. TreeLODGen deliberately strips PBR to a plain diffuse +
  normal shader (README.md:149-152: copied PBR made distant trees glow). Decide explicitly whether
  LODForge keeps PBR slots for its own LOD shader or follows this.

### 7.6 Per-shape post-processing order (Split.cs:333-353)
For each shape in output order: (bark) Simplify | (crown, keep<1) Prune then UV overshoot clamp;
then for every shape `BakeTransform` (NifUtil.cs:59-78): if transform not identity — apply `T` to
positions (`ApplyTransform`), rotate+renormalise normals, set transform identity,
`CalcTangentsForShape`, update bounds; then `PlainLodShader`.

### 7.7 Cleanup and save (Split.cs:355-363)
`DeleteUnreferencedNodes()`, then `DeleteBlockByType(type, orphanedOnly)` for
`BSMultiBoundNode` (orphanedOnly = **false**), and `BSMultiBound`, `BSMultiBoundOBB`, `NiSwitchNode`,
`BSTreeNode`, `BSLeafAnimNode`, `NiNode` (orphanedOnly = true). Result: root + shapes + their
shader/texture set/alpha (+ any extra data the clone carried). Save; failure throws.

Shapes are never merged; one output shape per crown part and per trunk part.

## 8. Output files

### 8.1 Names and folder
* Folder: `<output>\meshes\dyndolod\lod\trees\` (flat, `LodFolder`, Generate.cs:11). Older versions
  used a `treelodgen` subfolder; its `*passthru_lod*.nif` are deleted and the folder removed if empty
  (Generate.cs:260-265).
* `stem = lowercase(file name of the model without extension)`; files
  `<stem>_<CRC>passthru_lod.nif`, `<stem>_<CRC>passthru_lod_1.nif`, `<stem>_<CRC>passthru_lod_2.nif`
  (Split.cs:87, 170). Plus the `meshCrc` copies (§1.4).
* Everything is staged in `%TEMP%\TreeLODGen_stage_<guid>` and moved at the end; a cancelled run
  changes nothing (Generate.cs:48-69, 254-273).

### 8.2 TreeLODGen_models.txt
In the LOD folder. Before moving, files listed by the previous list are deleted. Then all staged
`*.nif` are moved in and their file names (one per line, no folder) written to the list
(Generate.cs:257-272). Used next run to exclude own files from "existing LOD" (§1.3).

### 8.3 TreeLODGen_report.csv
At `<output>\TreeLODGen_report.csv`. `;` separated, header (Generate.cs:182):

```
model;editor_ids;mesh_source;crc;status;bark_tris;leaf_tris;notes;vanilla;billboard;lod4_kb;records
```

One row per model group (Generate.cs:251):

| Column | Value |
|---|---|
| model | lowercased model key (relative to `meshes\`) |
| editor_ids | EDIDs of the group's records, `,`-joined |
| mesh_source | MO2 mod name / `Data` / `<bsa> (<source>)` of the resolved mesh, or empty |
| crc | lookup CRC `X8` (empty when skipped before step 4) |
| status | §1.2 / §1.5 |
| bark_tris | `TrunkTris` = source wood tris (before simplification) |
| leaf_tris | `CrownTrisFull` = source leaf tris (before pruning) |
| notes | `,`-joined notes (e.g. `skinned:X`, `transformed:X`, `rebuilt as BSTriShape`, `shader:PBR`, `bare tree: bark only LOD`, `ambiguous-piece:X`, `uv-out-of-range:X`, `static render stand-in written`) |
| vanilla | `yes` if the mesh is in a `Skyrim - *.bsa` or any record owner is a base master (Skyrim/Update/Dawnguard/HearthFires/Dragonborn.esm), else `no` |
| billboard | `yes` unless SkinnedOnly and no `--dyndolod` |
| lod4_kb | size of level-0 file / 1024 (integer), 0 if absent |
| records | number of records in the group |

Skipped rows leave bark/leaf/notes and the last four columns empty. Only processed trees fill
vanilla..records.

`uv-out-of-range:<shape>` (Split.cs:420-429): after writing, for each crown part, if any of its UVs
is outside `[-0.05, 1.05]` (warning: LODGen will split triangles).

### 8.4 DynDOLOD rule INI (TreeRules.Write, TreeRules.cs:42-70; Generate.cs:275-281)
Unless `--no-rules`. First all `<output>\DynDOLOD\DynDOLOD_SSE_*` files are deleted. Rows of the
report with status `generated` are read back. Per tree:

```
choice = (billboard column == "no") ? "all" : (choices[model] if non-empty else "default")
```

`default` trees get no line. If no tree has a non-default choice, no file is written. Otherwise:

```
[Skyrim LODGen]
LODGen1=<model>,<levels>,Far LOD,Unchanged,17
LODGen2=...
```

(numbering = current line count, so starts at 1), `<levels>`:

| choice | levels |
|---|---|
| `all` | `Static LOD4,Static LOD8,Static LOD16` |
| `billboard` | `Billboard,Billboard,Billboard` |
| `lod4` (anything else) | `Static LOD4,Billboard,Billboard` |

The same lines are written to `DynDOLOD_SSE_skyrimesm_high.ini`, `..._medium.ini`, `..._low.ini` in
`<output>\DynDOLOD\`. `--tree-choices <file>`: JSON object `model path -> "lod4"|"all"|"billboard"`.

### 8.5 Tile-size advice (Generate.cs:326-335, WoodSplit.cs:98-111)
Console advice only. For each crown part's slot-0 texture: `threshold` = alpha threshold or 128;
`full = Coverage(max stored side)`; `tile = 256; while tile < size && Coverage(tile) < 0.7*full:
tile *= 2`; result `min(tile, size)`. `Coverage(maxSide)`: box-downsample stored pixels by
`step = max(1, maxStoredSide / maxSide)` and count blocks with `Σalpha >= threshold*step²`, as a
fraction. If the worst tile > 256, prints "set DynDOLOD 'Max tile size full' to <tile>".

### 8.6 Animated-only trees and stand-ins (Split.cs:189-199, Generate.cs:237-245, 282-303)
Only with `--dyndolod <folder>` (must contain `Edit Scripts\DynDOLOD`). For SkinnedOnly trees a
static stand-in is written to
`<dyndolod>\Edit Scripts\DynDOLOD\Render\Skyrim\Replace\<model dir>\<model name>_static.nif`
(full crown+bark, keep 1, no simplify, original shape names, transforms baked, skin removed);
previous stand-ins listed in `TreeLODGen_standins.txt` there are deleted and the list rewritten. Also
`<output>\DynDOLOD\DynDOLOD_SSE_TexGen_Billboards_<owner without '.' lowercased>.txt` with lines
`<Owner>;<LocalId:X8>;<EditorId>,True` per record.

## 9. Tunables

`--levels` (Generate.cs:97-101): comma-separated levels, each `keep:area[:barkTris[:crownFloor]]`.

| Field | Meaning | Default L0 / L1 / L2 |
|---|---|---|
| keep | share of leaf islands kept (Morton error diffusion) | 0.4 / 0.15 / 0.06 |
| area | target share of original leaf area restored by enlarging kept cards (≤2x linear) | 0.8 / 0.7 / 0.6 |
| barkTris | bark triangle budget for the whole tree at that level (default 400 if field omitted) | 400 / 150 / 60 |
| crownFloor | leaf triangles a crown is never pruned below (applied only if **every** level has 4 fields; otherwise the default floors stay) | 2500 / 1000 / 400 |

Level count = number of comma groups (default 3 -> LOD 4, 8, 16 files `""`, `_1`, `_2`).

GUI presets (MainForm.cs:12-17):

| Preset | --levels |
|---|---|
| Light | `0.3:0.8:300:1800,0.1:0.7:100:700,0.04:0.6:40:300` |
| Balanced (default) | `0.4:0.8:400:2500,0.15:0.7:150:1000,0.06:0.6:60:400` |
| Detailed | `0.55:0.85:600:3500,0.25:0.75:250:1500,0.1:0.65:100:700` |

Other options: `--all`, `--no-rules`, `--tree-choices`, `--dyndolod`, `--output`, `--data`, `--mo2`
+ `--profile`, `--plugins`, `--far-3d` (ignored). Hard-coded constants are in the tables of §3-§5
plus: `MaxCardScale = 2.0`, profile `Bands = 24`, Morton 10 bits/axis, UV overshoot clamp 0.05,
noatlas tolerance 0.05, texture max stored side 2048, FoliageTile `KeptCoverage = 0.7`, start 256.

## 10. Hybrid.cs and the HYBRID path (dev only)

`Hybrid.Run` (Hybrid.cs:12-85) drives DynDOLOD's `Edit Scripts\LODGenx64.exe` in `GameMode=HYBRID`
for crowns written by the `split` command: per level dir `hybrid\input` (L0) / `hybrid\input_<L>`,
writes `hybrid\hybrid_<L>.txt` with `GameMode=HYBRID`, `FlatNif=<dyndolod>\docs\trees.ultra\tools\hybrids\template4.nif`,
`PathData=<TexGen_Output>\Textures\DynDOLOD\LOD\Trees\`, `PathInput=`, `PathOutput=`; warns when
`<name>_trunk.txt` billboard info is missing; renames level-L outputs to `..._L.nif`. In `split`
mode `WriteStaticNif` also writes the crown alone (original shape names, pruned) and a trunk NIF to
`DynDOLOD\Render\Billboards\DynDOLOD\lod\trees\<stem>_<crc>_trunk.nif` for TexGen. The generator
never calls any of this (`hybridInputs: false`), and LODForge does not need it for passthru 3D LOD.
`Batch.cs` compares `split` output against an author's hand-made LOD (dev metric only).

## 11. Port risks and divergences to decide

1. No filtering of NiSwitchNode alternates, hidden shapes, effect shaders or Tree_Anim shapes;
   all shape blocks with triangles go in. Reproduce or filter knowingly.
2. Shape-local spaces: island welding (0.01), Morton bbox, crown profile Z axis and simplifier
   weld all run **before** the transform bake, in shape-local units.
3. Simplifier is custom; meshoptimizer will not match it. Tie-breaking differs from .NET.
4. Bark identified by output name; name collisions could simplify a crown.
5. `noatlas` test uses the whole source shape's UVs even for a mixed shape's bark subset.
6. Texture lookup is not normalised (missing `textures\` prefix -> UV-tiling fallback).
7. Shader slots 2-7 and flags are kept untouched for plain (non-PBR/env/parallax/glow) trees.
8. Vertex format and vertex colours are inherited from the source via nifly CloneShape; LODForge's
   writer must reproduce clone semantics (shader, texture set, alpha, extra data).
9. Skinned path uses the inverse skin transform and does not exclude the root transform.
10. Partial-level output on exceptions (a port should be all-or-nothing per tree).
