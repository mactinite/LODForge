# NGIO grass cache (`.cgid`): file format

Reference for the `.cgid` reader/writer in `crates/grass`. LODForge does not generate grass
caches: `--grass bto` and the runtime's far grass read the cache that No Grass In Objects NG
writes in game; cells without a cache file get no grass. Section numbers are kept from the
original research document, whose other sections live in the development notes.

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
