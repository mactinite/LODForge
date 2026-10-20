> **2026-10-19: the generator was removed from `main`** (a product decision: the modlist uses NGIO's own in-game
> precache; the generator also followed disassembly of the game engine, which LODForge no longer carries). The file-format section (§3) remains the
> reference for the `.cgid` reader/writer in `crates/grass`; the rest of this document is historical research. The
> generator and the `lodforge grass-cache` command live on branch `g1-grass-cache`. `--grass bto` now reads the
> NGIO cache only; cells without a cache file get no grass.

# Offline grass cache (.cgid): RE spec

Status: research spike, 2026-10-17. Nothing is implemented yet. Target game build: SkyrimSE.exe 1.7.104.0
(`D:\Modlists\skyrim-cs\Stock Game`). Cache producer being replaced: No Grass In Objects NG v1.6.14
(GPL-3, source at `D:\Modlists\ref\ngio-ng`) plus Grass Cache Helper NG 1.0.3.

Goal: write the same `Data\Grass\<World>x%04dy%04d.cgid` files NGIO's in-game precache writes, from
plugins and assets only, so the game and DynDOLOD grass LOD load them unchanged.

## 1. Verdict

| Piece | Status | Source of truth |
|---|---|---|
| File format | **Recovered and verified**: 3,201/3,201 real files parse exactly (19.1 M instances), and group bounds match the formula in 75,420/75,420 groups | engine writer/loader + NGIO `SaveGrassGroup` |
| Instance encoding (16 halfwords) | Recovered | NGIO `GrassPatchCreation::Encode` |
| Patch iteration per cell/quad | Recovered | engine `AddCellGrass` |
| Grass types and 3x3 density per patch | Recovered | engine per-vertex param builder |
| LAND decode the engine actually uses (VHGT, VNML, VCLR, BTXT/ATXT/VTXT quirks) | Recovered | engine LAND loader |
| Height/normal/colour interpolation | Recovered | engine `GetLandCoordData` / `GetLandData` |
| Candidate placement, RNG, water, slope | Recovered (NGIO port; engine for the fallback) | NGIO `GrassPatchCreation::Generate`, engine `CreateGrassPatch` |
| Raycast filter ("no grass in objects") | Semantics recovered. The offline collision world is **the main build cost** | NGIO `RaycastHelper` / `PhysicsRaycast` |
| Grass on cliffs, per-shape ignore rules | Semantics only; later milestone | NGIO |

Everything except collision is a pure function of plugin data, INI values and GRAS model bounds, so it
can match NGIO NG bit for bit. Section 12 has the effort and risk estimate.

## 2. Addresses (1.7.104, AE Address Library IDs)

> Moved to the quarantine folder on 2026-10-20: this section described the game engine's / NGIO's grass placement internals (function addresses, field offsets, pseudo-code). The offline generator that used it was removed from `main`; the `.cgid` reader needs only sections 3 and 4.

## 3. File format (little-endian, verified)

File name: `Grass\<WorldEDID>x%04dy%04d.cgid` (printf, so `x0005y-010`, `x-056y0022`). Grass Cache
Helper NG adds a season infix for Seasons of Skyrim (`.WIN.cgid`, ...). See
`scripts/grass/Run-GrassPrecache.ps1`.

```
u32 typeCount
repeat typeCount:
  u32  nameLen                 // includes the NUL
  char model[nameLen]          // GRAS model path with the leading "meshes\" removed (writer skips 7 chars); case as authored
  f32  wavePeriod              // written from the shape's grass shader property ; the loader puts it in GrassParam.wavePeriod
  u32  grassFormID             // load-order-resolved runtime FormID
  u8   fitsToSlope             // SLSF2 bit 12 (flags bit 44)
  u8   uniformScaling          // SLSF2 bit 11 (bit 43)
  u8   vertexLighting          // SLSF2 bit 10 (bit 42)
  u32  groupCount              // may be 0 (2,100 such entries in the sample)
  repeat groupCount:
    f32 center[3]              // group AABB centre (world space)
    f32 halfExtent[3]
    u32 triCount               // ignored by the loader (it recomputes meshTriCount*instances); NGIO writes junk here
    u32 instanceCount
    u32 shortsPerInstance      // always 16
    u16 instances[instanceCount * 16]
```

The vanilla SaveGrassGroup writes the header but not the instance data. That is why PC `.gid` never
worked and NGIO hooks it. The reader takes `instanceCount*shortsPerInstance*2` bytes after each header.

**Group bounds**:
for each instance, `px = half(x) + shapeOrigin.x`, `py = half(y) + shapeOrigin.y`, `pz = half(z)`. Take
min/max, then `lo = min - 30`, `hi = max + 30`, `center = lo + (hi - lo) * 0.5`, `halfExtent = hi - center`.
`shapeOrigin` is the 12x12 block origin, `12 * trunc(cell/12) * 4096`. Matches all 75,420 sampled groups
exactly.

**Grouping**: one shape per (model, 12x12 block). Engine AddCellGrass calls DoneAddingInstances after
each of the 4 quads, which splits a quad's instances into groups of at most `maxInstancesPerGroup` (exact cap still open). NGIO also flushes when its
0x40000-byte staging buffer fills (8,191 instances). The loader accepts any split, so the offline
writer can emit one group per (type, quad) up to the cap.

**Type order**: the order handles are created in AddCellGrass, i.e. first appearance while walking
quads → patches → vertex grass params (§5). Order matters only for byte-identical diffs.

## 4. Instance encoding (16 × half, NGIO Encode)

```
[0] x - 12*trunc(cellX/12)*4096      [1] y - 12*trunc(cellY/12)*4096      [2] z
[3] shade
[4..12] rotation r in order {r0,r1,r2,r7,r3,r4,r5,r8,r6} (row-major 3x3)
[13] heightVariation (scaled by Global-grass-scale if != 1: (v+1)*scale-1)
[14] 0  [15] 0
```
`r = [c,-s,0; s,c,0; 0,0,1]` with `c = orientation draw`, `s = sqrt(1-c²)`. With fitsToSlope it is multiplied by the
basis `[t×n | t | n]`, where `t = SlopeTangent(n)` (NGIO, smallest-axis rule). `n` is the triangle face normal (§7).
Half conversion is DirectXMath `XMConvertFloatToHalf` (round to nearest even), which Rust `half::f16::from_f32` matches.

## 5. Patch iteration

> Moved to the quarantine folder on 2026-10-20: this section described the game engine's / NGIO's grass placement internals (function addresses, field offsets, pseudo-code). The offline generator that used it was removed from `main`; the `.cgid` reader needs only sections 3 and 4.

## 6. Grass types and density at a vertex

> Moved to the quarantine folder on 2026-10-20: this section described the game engine's / NGIO's grass placement internals (function addresses, field offsets, pseudo-code). The offline generator that used it was removed from `main`; the `.cgid` reader needs only sections 3 and 4.

## 7. LAND as the engine sees it

> Moved to the quarantine folder on 2026-10-20: this section described the game engine's / NGIO's grass placement internals (function addresses, field offsets, pseudo-code). The offline generator that used it was removed from `main`; the `.cgid` reader needs only sections 3 and 4.

## 8. Interpolation

> Moved to the quarantine folder on 2026-10-20: this section described the game engine's / NGIO's grass placement internals (function addresses, field offsets, pseudo-code). The offline generator that used it was removed from `main`; the `.cgid` reader needs only sections 3 and 4.

## 9. Candidate placement

> Moved to the quarantine folder on 2026-10-20: this section described the game engine's / NGIO's grass placement internals (function addresses, field offsets, pseudo-code). The offline generator that used it was removed from `main`; the `.cgid` reader needs only sections 3 and 4.

## 10. Water and slope

> Moved to the quarantine folder on 2026-10-20: this section described the game engine's / NGIO's grass placement internals (function addresses, field offsets, pseudo-code). The offline generator that used it was removed from `main`; the `.cgid` reader needs only sections 3 and 4.

## 11. Settings the generator must read

From the profile's Skyrim.ini/SkyrimPrefs.ini plus any INI overrides mods ship (and NGIO's
`GrassControl.ini` / `_NGIO.ini` overrides): `iMinGrassSize`, `iMaxGrassTypesPerTexure`,
`fTexturePctThreshold`, `fWaveOffsetRange`, the source of grassEvalSize/grassPatchSize (open, from the manager ctor),
NGIO `Global-grass-scale`, `Ray-cast-*`, `Skip-/Only-pregenerate-world-spaces`, `Grass-cliffs-*`. Grass Cache
Helper NG toggles worldspace NoGrass flags (log: removes it from WhiterunWorld, adds it to KatariahWorld).

## 12. Raycast filter

> Moved to the quarantine folder on 2026-10-20: this section described the game engine's / NGIO's grass placement internals (function addresses, field offsets, pseudo-code). The offline generator that used it was removed from `main`; the `.cgid` reader needs only sections 3 and 4.

## 13. Validation plan

1. Golden A: in-game precache with `Ray-cast-enabled=false`, `Grass-cliffs-enabled=false`, Tamriel only.
   Offline output should be **byte-identical** apart from type/group order and the junk triCount. Diff per
   instance with a tool in the shape of `scripts/grass-re/cgid_parse.py` / `cgid_stats.py`: positions, rotation, shade, variation.
2. Golden B: the normal precache (another session ran one on skyrim-cs today: `overwrite\grass`, 3,201 Tamriel
   files, NGIO-NG.log). Judge raycast parity as the % of instances kept/rejected identically.
3. Load test: drop the offline cache in, `bAllowCreateGrass=0` (NGIO `Only-load-from-cache`), and screenshot-compare
   a few cells via skyrim-mcp. Then feed DynDOLOD grass LOD.

## 14. Open items

- Source of grassEvalSize/grassPatchSize (BGSGrassManager ctor) and `maxInstancesPerGroup`.
- Which LAND override the runtime uses when several plugins have one.
- Exact SE 1.5.97 / 1.6.x deltas (CreateGrassParams layout without densityMultiplier; slope index formula).
- Landscape collision layer vs the raycast mask; enable-parent / disabled refs in the collision world.
- How often the native-placement fallback triggers on real load orders.
- Season variants: GRAS/LTEX swaps from Seasons of Skyrim INIs, one cache per season.

## 15. Effort and risk

| Milestone | Scope | Estimate (agent days) | Risk |
|---|---|---|---|
| G1 placement without raycast | LAND engine-view decode, §5–§10, writer, CLI `lodforge grass-cache` | 2–3 | Low. Everything is specified; bit-exact parity is likely |
| G2 texture block + ignore lists + settings/INI plumbing + seasons | §11, §12 texture part | 1–2 | Low |
| G3 collision world | Havok NIF shape parsing incl. compressed mesh, BVH, ray/capsule/box tests, REFR gather | 5–10 | **Medium-high**. Compressed-mesh decoding and edge parity drive it |
| G4 grass cliffs | NGIO cliff surfaces, needs G3 | 2–3 | Medium; optional (not in cache files during generation) |

Runtime estimate (Tamriel, ~3.5k land cells): about 0.5–1 G candidates at tens of ns each, plus ~40 M collision tests
at ~1 µs, on 20 threads. That is seconds to low minutes, against hours in game. G1 alone already gives a
loadable cache, the same as NGIO with raycasting off.
