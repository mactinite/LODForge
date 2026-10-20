# Cacheless grass: can an SKSE plugin make the grass precache unnecessary?

Status: design spike, 2026-10-18. Nothing is built. Inputs: [grass-cache-spec.md](grass-cache-spec.md) (engine
pipeline, SkyrimSE.exe 1.7.104), NGIO NG source at `D:\Modlists\ref\ngio-ng` (a v2.0.0 fork; skyrim-cs runs the
1.6.14 DLL), the skyrim-cs `Default` profile (97 plugins), and its finished cache `mods\Grass Cache - Default\Grass`
(5,700 Tamriel `.cgid`, 1.2 GB). The offline experiment tool (`gmask`, Rust, built on `lodforge-esp`/`-archive`/`-nif`)
is in the session notes folder, `cacheless-grass-gmask\`, not in the repo.

## 1. Verdict

Yes, mostly. The cache only does real work in two places:

1. **Collision exclusion for cells without loaded collision.** That covers distant cells (Extend-grass-distance) and
   attached cells before Havok has their bodies.
2. **The input to DynDOLOD grass LOD.**

Placement itself is cheap and deterministic. Collision exclusion can be replaced by a **top-down exclusion mask**
rasterized from object geometry:

- About **0.5–0.9 KB per cell** at 32-unit resolution (zlib), against roughly 210 KB per cell of `.cgid`. That is
  about 300× smaller: ~4 MB for all of Tamriel instead of 1.2 GB.
- It needs no Havok, so grass never waits on collision.
- It works for unloaded cells.
- On the measured cells it agrees with NGIO's Havok filter to within 0.3–2% of the grass kept (§3).

Recommended architecture (§6): **runtime placement (NGIO's deterministic generator) + a per-worldspace exclusion mask
built offline by LODForge (or once, in the background, on first launch) + no instance cache.** DynDOLOD grass LOD is
fed by the same generator run offline (the G1 work), not by an in-game precache.

## 2. Where the time goes today (question 1)

### In-game profile: not run yet

A precache was running on skyrim-cs during this spike, and the G1 session is still active, so the game was not
launched. The protocol below is ready. It needs the user's go-ahead, and skyrim-cs must be free.

1. Clone `Default` into a test profile (`mo2_create_test_profile`). Disable `Grass Cache - Default`. Add a mod with
   `Spike_NGIO.ini` (NGIO reads `*_NGIO.ini` overrides from Data) setting `Use-grass-cache=false`,
   `Only-load-from-cache=false`, `Profiler-report=true`. The shared `GrassControl.ini` stays untouched.
2. Run A: raycast on. Run B: `Ray-cast-enabled=false`. Run C: B plus `Threaded-grass-placement=false`. Each run loads
   a save, `coc`s through a fixed list of 10 exterior cells (Riverwood, Whiterun tundra, Falkreath forest, Reach,
   Rift), then reads NGIO's report line (`Grass generation: Samples/Average/Median/Max`) and its
   `Physics queries: ... unavailable ..., pending first placements ...` line.
3. Derive the numbers:
   - **placement** = B's per-cell time;
   - **raycast cost** = A − B;
   - **Havok wait** = the time from cell attach to grass appearing, minus A. This comes from `game_events` cell-attach
     timestamps against `game_screenshot`/`image_compare` polling at 100 ms;
   - **shape build and upload** = C's single-thread time minus the candidate loop, estimated from §5 counts.

### Evidence available now

- **Placement is cheap.** Kept grass is about 6,000 instances per cell (19.1 M over 3,201 files; 128 k over 22 cells
  near Riverwood). The spec's candidate cost is tens of ns. NGIO runs placement on TBB, capped at 6 threads during
  gameplay (`GameplayParallelism`). The candidate loop plus encoding is therefore on the order of 1 ms per cell. The
  in-game profile has to confirm this.
- **Collision gating is a wait, not a computation.** For an attached cell, NGIO's `PrepareCell` → `CanFilterCell`
  requires all of the following before any grass is placed:
  - the cell and all 8 neighbours have `refsFullyLoaded` and no queued refs;
  - the bhkWorld has no pending (body) operations.

  Otherwise the cell is deferred and retried every 100 ms (`RetryPendingCells`). Queries then go through
  `WorldReadGate`, a time-windowed read lock that yields to Havok writers (`EnterWriteLock` is hooked). Grass
  therefore appears only after the 3x3 neighbourhood's collision has settled, and its queries compete with physics
  for the world lock.
- **The precache is dominated by cell loading.** The 2026-10-18 03:31–03:35 NGIO log shows 223 precache steps (one
  grid load each) with median 0.12 s, p90 0.95 s, max 2.3 s per step. Steps that crossed populated land were the slow
  ones (5.4 s for Tamriel(-7,23) right after resume).
- **Unloaded cells get no collision filtering.** `GrassPatchCreation` takes raycast settings only when
  `cell->IsAttached()`, and `RaycastHelper::IsExempt` returns true for unattached cells. Extend-grass-distance cells
  that have no cache file are generated **with grass inside rocks**, or left as LOD (`GrassState::Lod`). This is the
  real reason the cache is "required" for distant grass.

## 3. Can a top-down mask replace the Havok query? (question 2)

### Method

Ground truth is NGIO's capsule test: from `z-5` to `z+150`, radius `max(GRAS OBND half-extent x, y) / 2 × Ray-cast-width-multiplier`, against
collision layers `1 2 13 20 31`. The **kept** instances in the existing `.cgid` serve as the check. A kept instance
that falls in a masked texel means the mask over-rejects (it would remove grass that Havok allowed).

Each run also reports the **grass density inside the mask relative to outside**. A value near 0 means Havok also
removed almost everything there, so the mask lies over real collision.

Under-rejection (collision outside the mask) cannot be measured from a filtered cache. It needs the unfiltered G1
candidates (follow-up, §7).

Texel test: the texel is masked if any object surface crosses `[terrain-5, terrain+150]` at the texel centre. Terrain
comes from bilinear VHGT. Disabled refs are skipped.

### Results

**(a) OBND boxes.** Riverwood block, 22 cells. Unusable: 76% of the kept grass falls inside a box column. Bounds
include FX activators (`FXBirdFlockFleePineACT` alone accounts for 17 k), cloud statics, and whole mountains whose
boxes cover the slopes grass grows on.

**(b) Visual triangles of bases whose NIF has a `bhkRigidBody` on a masked layer.** Layer histogram of the models in
range: 286 on layer 1, 12 on layer 2, plus non-masked 4/10/15/27.

| Region (cells) | Kept grass wrongly masked @16 / @32 | Density inside mask vs outside | Masked area |
|---|---|---|---|
| Riverwood (22) | 11.7% / 11.8% | 0.34 | 28% |
| Same, Grass-cliffs forms excluded | **1.6% / 1.8%** | **0.057** | 22% |
| Whiterun tundra (36), cliffs excluded | 0.70% / 0.78% | 0.025 | 22% |
| Reach (49), cliffs excluded | 0.40% / 0.45% | 0.015 | 21% |
| Rift (56), cliffs excluded | 0.28% / 0.36% | 0.016 | 15% |
| Solitude coast (47), cliffs excluded | 0.28% / 0.31% | 0.014 | 17% |

Findings:

- **Mesh geometry works; bounds don't.** On cells where Havok had collision, the visual-mesh mask is nearly exact.
  Per cell: (4,-8) 0.0%, (5,-12) 0.1%, (3,-11) 0.4% of kept grass masked.
- **Most of the residual is the 94 Grass-cliffs forms.** These are the DirtCliffs* family and others, listed in the
  NGIO log. The golden cache keeps grass under them in many cells, although their collision (`bhkCompressedMeshShape`,
  layer 1) has bounds equal to the visual mesh, and the blockers sit in the same cell (11.7 k vs 3.3 k neighbour-cell
  cases). The ref source's normal filter does not ignore cliffs, so the deployed 1.6.14 DLL probably behaves
  differently. It is an open item. The practical point is that the mask must reproduce whatever NGIO does for cliff
  forms. That is a per-form flag, cheap either way.
- **The remainder is objects whose collision is much smaller than the visual mesh** (`TreePineForestUprooted01`,
  `TreePineForestLog02`: the visual bbox spans x -203..356, the collision -0..140). Rasterizing the **collision
  shapes** instead of the visual mesh removes this. Offline, that means G3's compressed-mesh decoder. At runtime, the
  plugin can cast against each loaded reference's own `hkpShape` (see §6), which needs neither the world nor its lock.
- **Resolution barely matters.** Going from 8 → 16 → 32 → 64 units changes over-rejection only by 1.57 → 1.63 → 1.79
  → 2.13% (Riverwood, cliffs excluded). The cache's own positions are half-floats relative to the 12x12 block origin,
  so x/y already carry a 16–32 unit step far from the origin. **32 units is the sweet spot.**
- **Cost:** rasterizing about 15–37 k triangles per cell takes 1.5–2.4 ms per cell at 32 units, or 4–7 ms at 16, on
  one thread. All of Tamriel is under 20 s single-threaded. The capsule radius (a few units) can be applied by
  dilating the mask, or ignored given the 32-unit step.

## 4. Grass beyond the loaded grid and DynDOLOD grass LOD (question 3)

| Data for an unattached cell | Available at runtime? |
|---|---|
| LAND (heights, normals, colours, texture layers) | **Yes.** NGIO already does `CreateCellAt` + `LoadLand` (TESFile read) for distant cells (`DistantGrass::LoadTerrain`) |
| LTEX → GRAS lists, GRAS data, worldspace and cell water | Yes, in memory as forms |
| References in the cell | No. They exist only once the cell loads. Reading them from TESFile means reimplementing cell loading |
| Object 3D / collision | No |

Collision exclusion for distant grass therefore has to come from precomputed data. A mask is enough, because grass z
is always the terrain z, so the predicate is 2D: `blocked(x, y)`.

| Representation | Size per cell | Tamriel (5,700 cells) |
|---|---|---|
| `.cgid` cache (measured) | ~210 KB | 1.2 GB (one season); × seasons with Seasons of Skyrim |
| 1-bit mask @16, zlib | 1.2–2.3 KB | ~7–13 MB |
| **1-bit mask @32, zlib** | **0.5–0.9 KB** | **~3–5 MB** |
| 1-bit mask @64, zlib | 0.3–0.35 KB | ~2 MB |
| Mask + per-texel cliff/form-class bits (2 bits @32) | ~1–1.5 KB | ~6–8 MB |

The mask does not depend on season unless Seasons of Skyrim swaps statics. Its grass inputs (GRAS/LTEX swaps) only
affect placement, which runs at runtime anyway. One mask can therefore serve all seasons, whereas the cache needs one
copy per season.

**DynDOLOD grass LOD** reads `.cgid` density and positions. It needs a cache-shaped input, not a runtime one. That is
exactly what the offline G1 generator produces, once it has the same mask (or G3 collision) as its filter. Nothing in
the game is needed.

## 5. GPU compute placement (question 4)

**Not worth it.**

- Placement is about 1 ms per cell on the CPU (§2). The latency comes from collision gating and cell loading, not
  arithmetic.
- The engine needs CPU-side instance data. `BSMultiStreamInstanceTriShape::AddGroup` computes group bounds from the
  instances, and `SaveGrassGroup`/the DynDOLOD path reads them. GPU output would have to be read back or have its
  bounds duplicated.
- Writing directly into the D3D11 instance buffers bypasses `DoneAddingInstances` group splitting and culling bounds,
  which means hooking the renderer for no gain.
- Community Shaders on this list hooks `BSGrassShader`/`BSGrassShaderProperty` (features GrassLighting,
  GrassCollision, GrassOptimizations; see CommunityShaders.log). Its shaders consume the engine's per-instance stream
  (16 halfs: position, 3x3 rotation, shade, height variation), so any GPU writer would have to match that layout
  exactly and track CS changes to it. CS internals were not checked against source, because none is available
  locally.
- The only GPU-shaped win would be generating far grass entirely in a vertex shader (no instance buffer). That is a
  new renderer feature, not a cache replacement, and it conflicts with CS's grass pipeline. Out of scope.

## 6. Recommended architecture

1. **Placement at runtime, always.** Use NGIO's deterministic per-patch generator (seed from cell, form and patch
   start), unchanged. It already runs in about 1 ms per cell across TBB workers. Disable `Use-grass-cache`.
2. **Exclusion mask, `Data\Grass\<World>.gmask`.** 32-unit bits per land cell, zlib per cell, with an index. Two
   ways to build it, with the same format:
   - **LODForge, offline**: `lodforge grass-mask`, from collision shapes (G3), with the visual-mesh raster above as a
     G3-less fallback;
   - **plugin, first launch**: a background job that reads REFRs and NIFs through the game's own file system. It
     needs ESP and NIF readers in the plugin, so LODForge is the cheaper first target.
3. **Attached cells:** test candidates against the mask immediately, with no `CanFilterCell` wait. Optionally refine
   later: once a ref's 3D is loaded, cast against that ref's `bhkRigidBody` shape directly (shape-local
   `castRay`/linear cast, read-only, no `hkpWorld` lock), only for texels the mask marks as blocked. This gives Havok
   accuracy where it matters without the world gate.
4. **Distant cells (Extend-grass-distance):** the same generator plus the mask, with LAND loaded the way NGIO already
   does it. Grass inside rocks goes away, with no cache.
5. **DynDOLOD grass LOD:** the offline G1 generator plus the same mask writes `.cgid` for DynDOLOD only (or straight
   to DynDOLOD's grass input). The game never needs them.
6. **Load stutter:** handled by NGIO's existing async distant-cell queue. Shape build and upload per cell is the
   remaining cost and stays the same as with the cache (the cache only saves placement). Measure it in the §2
   protocol.

## 7. Options, effort, risk

| Option | Scope | Effort (agent days) | Risk |
|---|---|---|---|
| A. Offline mask from visual meshes (this spike's raster, productized in LODForge) + NGIO fork reading it | `grass-mask` CLI, `.gmask` writer; NGIO hook in `RaycastHelper::CanPlaceGrass` and the attached gate | 2–3 | Low–medium: residual mismatch is objects whose collision is much smaller than the visual mesh (logs) plus the cliff-form rule |
| B. Mask from collision shapes | needs G3's compressed-mesh / convex / box decode | +3–5 on top of G3 work shared with the offline cache | Medium (G3) |
| C. Runtime per-ref shape cast refinement | SKSE: walk loaded 3D for `bhkRigidBody`, shape-local casts | 2–4 | Medium: Havok internals on 1.5.97/1.6/1.7 layouts; but avoids the world lock |
| D. First-launch in-plugin mask build | ESP REFR reader + NIF/bhk reader in C++ | 5–8 | Medium-high; duplicates LODForge |
| E. GPU placement | — | 5+ | High; no benefit (§5) |
| F. Keep the cache, shrink it | not possible below ~210 KB per cell; the instance stream is the payload | — | — |

Order: **A first**, which already beats the status quo for distant grass (today it is unfiltered). Then B, sharing
G3. C only if the profile shows the attached-cell wait is visible to players.

## 8. Open items

- Run the §2 in-game profile once skyrim-cs is free and the user agrees.
- Under-rejection: compare the mask with G1's unfiltered candidates. A candidate removed by Havok (missing from the
  golden) but kept by the mask counts as a miss.
- Grass-cliffs forms: why the 1.6.14 golden keeps grass under DirtCliffs*. Diff 1.6.14 against the v2.0.0 ref, or
  run an in-game A/B with `Grass-cliffs-enabled=false`.
- Non-mask layers present in the models (4, 10, 15, 27) are correctly excluded. Check whether any common grass
  blockers use them.
- The cell-load cost of the precache loop (median 0.12 s per step) is what makes precaching take hours on big
  lists. Runtime placement does not pay it, but get the number for a full list from the profile.
