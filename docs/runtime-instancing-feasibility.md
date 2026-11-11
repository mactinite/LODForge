# GPU-instanced distant LOD: feasibility (2026-10-23)

Question: can Skyrim SE + Bottled Shaders draw distant trees and grass with GPU instancing, to cut tree and grass pop-in, and can octahedral impostors replace billboards? Also: what would it take to cover everything AnvilLOD does?

Answer: yes. Instancing already works in both the engine and the Community Shaders DLL. Nothing here was run in game yet; it is all from source code and LODForge output.

## 1. Evidence that instancing works

### Grass Optimizations (Bottled Shaders, `src/Features/GrassOptimizations*.cpp`, ~4,400 lines)

This is the same technique in production, in the DLL the cs-base list already runs:

- `NiNode::OnVisible` hook (vtable 0x34) on the grass root: hands the engine *virtual proxy* shapes with `NiCullingProcess::AppendVirtual`, so the engine still makes render passes for them. Those passes run in the right order and with the right state: fog, deferred render targets, upscaler jitter, motion vectors.
- `BSGrassShader::SetupGeometry` and `SetupMaterial` hooks (vtable 6 and 4) bind its own constant buffer windows, which hold per-bucket cull data, PBR material constants and per-geometry lighting.
- It patches the batch renderer call site (`RelocationID(100847, 107637)` +0x663/0x64B) to replace the engine's instanced draw with `DrawIndexedInstancedIndirect`. That is one indirect draw per grass type and mesh tier.
- Compute culling per instance: frustum, distance, pixel size, density thinning and a Hi-Z pyramid (FidelityFX SPD).
- Mesh LOD tiers: full, mid and far mesh per grass type, swapped by pixel size (`_LOD0.nif` / `_LOD1.nif`).
- `RenderDistanceOverride` goes past the vanilla INI cap, but only for grass the engine generated, i.e. loaded cells (or more with NGIO's extended grass).

### Engine billboard tree LOD (`package/Shaders/DistantTree.hlsl`)

Tree LOD (`.btt`) is already drawn with instancing:

- Each tree type is one quad whose UVs point into a single per-world atlas. That atlas is `textures\terrain\<World>\Trees\<World>TreeLod.dds`.
- Per-instance data comes in `TEXCOORD4..7`: position, scale (`InstanceData1.w`), z rotation as cos/sin, and depth/fade terms.
- The pixel shader has a single texture slot, `t0`. It writes the deferred G-buffer: albedo, normal, motion vectors and masks.
- Bottled ships this shader as a loose `.hlsl` file and recompiles it, so a replacement needs no DLL change.

## 2. Two tiers

### Tier 1: octahedral impostors through the engine's own tree LOD (no DLL code)

- **LODForge:** writes each tree type's atlas rect as an N×N hemi-octahedral frame grid instead of one billboard. Project Terra's `HemiOctMath` and `TreeImpostorAtlasBaker` (`D:\workspace\Unity\ProjectTerra`) are the reference: Y-up hemisphere, grid-vertex frame sampling, dilation. The normal and depth frames sit next to the albedo frames in the same rect, because the shader only has `t0`.
- **Replacement `DistantTree.hlsl`:** the vertex shader turns the quad toward the camera, moves the view direction into tree space with the instance's rotation, picks the frame, and remaps UVs inside the rect. The quad's corner UVs give the rect bounds. The pixel shader blends frames, applies the baked normal and a depth offset, and writes Bottled's G-buffer. The depth technique uses the same frame selection for shadows.
- **What it fixes:** wrong silhouettes from the side and above (the "cardboard tree" swap), and no lighting response.
- **What it doesn't fix:** the atlas size limit (below), block and level swaps (engine `.btt` blocks), and the hard hide when a cell loads. There is no per-instance culling either.
- **Effort:** about 2–3 weeks. LODForge renders the frames (it already renders billboards on the GPU), plus the shader. It's the fastest way to see impostors in game.

### Tier 2: a GPU-driven LOD layer (Grass Optimizations' pattern applied to LOD)

- **LODForge writes instance databases** per world, cell and season instead of baking trees and grass into `.btt`/`.bto`. A record is 16–32 bytes: type, position, yaw, scale, plus a stable hash for thinning and dither.
- **Runtime:** proxy shapes go in via `OnVisible`. A compute cull then runs per view (main, shadow cascades, reflections): frustum, loaded-cell mask, pixel-size tier (3D mesh / impostor / thinned impostor) and Hi-Z. After that, one `DrawIndexedInstancedIndirect` per mesh per tier. All impostors share one quad, so all trees can be one indirect draw.
- **Textures:** a `Texture2DArray`, one slice per tree type and season, bound by us, so there is no atlas.
- **Pop-in:**
  - Trees dither in and out per instance over a distance band instead of whole blocks swapping.
  - Density thinning is hash-ordered, so it changes gradually with distance.
  - Near the loaded-cell edge, the impostor's fade-out can be timed to the real reference's fade-in. This needs the cell attach events the engine already sends to tree LOD.
- **Far grass:**
  - The same layer, fed from the `.cgid` grass cache.
  - It starts where Grass Optimizations' loaded grass ends and runs N cells further, with mesh tiers and then grass cards.
  - It replaces the grass that is baked into the LOD4 `.bto` today. AnvilLOD measured that as 162M triangles / 11 GB at 100% density for Tamriel. As instances it is a few million records, tens of MB (estimate; not measured yet).
- **Effort:** about 8–14 weeks for the runtime, plus 2–3 weeks of LODForge export.

## 3. Impostor texture budget

Assumptions:
- Tamriel in cs-base has 379 tree types with LOD (349 distinct models) and ~96k tree references (`LODForge_TreeReport_Tamriel.csv`).
- BC7 = 1 byte per texel, mips add 1/3.
- Two maps per type: albedo+coverage and normal+depth, as Project Terra does.

| Frame grid × frame size | Per type | ×349 types |
| --- | --- | --- |
| 16×16 × 128 px (Terra default, 2048²) | 10.7 MiB | 3.7 GiB |
| 16×16 × 64 px (1024²) | 2.7 MiB | 930 MiB |
| 16×16 × 32 px (512²) | 0.67 MiB | 233 MiB |

How many pixels a tree covers at 1080p with Skyrim's default FOV is about 1030 / distance px per unit. For a 1,500-unit (21 m) pine:
- ~130 px at 12k units (the edge of a 5×5 loaded grid);
- ~64 px at 24k;
- ~32 px at 48k.

1440p is 1.33× those numbers, 4K 2×. Only the first band past the loaded cells needs 128 px frames.

- **Tier 1 limit:** one atlas per world, at most 16384² (D3D11). As BC7 with mips that is 341 MiB. Sharing it between albedo and normal frames, and sizing each type by its height (shrubs are most types but need ~1/6 the texels), it fits about 120 tall trees at 12×12 × ~85 px and the rest at small sizes. That's sharp from ~1.5 cells past the loaded area at 1080p, and soft closer in at 1440p/4K. That's the atlas limit you expected; it's real for tier 1 only.
- **Tier 2:** `Texture2DArray` allows 2,048 slices of up to 16384². The limit becomes VRAM, which streaming manages:
  - every type kept at 512² (~233 MiB);
  - an LRU pool of ~32 nearby types at 1536² 12×12 × 128 px (~200 MiB).

  Hemi-octahedral is enough for trees. A full sphere doubles the frames and only matters for views from below.

## 4. Where the tier 2 runtime should live

- **(A) A feature inside a Bottled Shaders fork** (GPL-3.0, so it's compatible with LODForge).
  - Everything it needs is already there: `shadowState`, `SetDirtyStates`, the Hi-Z pyramid, G-buffer headers, reverse-Z handling, TruePBR.
  - It could be offered upstream later; Bottled's maintainer already takes ports.
  - Recommended for the prototype.
- **(B) A standalone SKSE DLL.**
  - It would have to share the batch-renderer patch site and shader vtables with CS, and copy its G-buffer and reverse-Z conventions.
  - It could break on every Bottled update.
  - Only worth it if the layer must also run on upstream CS without a fork.

## 5. AnvilLOD feature coverage

From AnvilLOD's README (v0.2.60, ref clone `D:\Modlists\ref\AnvilLOD`). "Runtime" means the SKSE plugin has to do it; "Generator" means LODForge. The LODForge column comes from memory and a quick grep; it needs a proper audit.

| AnvilLOD feature | Side | LODForge today | Higher-fidelity target |
| --- | --- | --- | --- |
| Object LOD `.bto`, rule files, Low/Med/High, child worlds | Generator | Yes | Same, plus instance export for repeated statics |
| Billboard tree LOD `.lst`/`.btt` + atlas | Generator | Yes | Octahedral impostors (tier 1, then tier 2) |
| 3D tree LOD (passthru, CRC/name match) | Generator | Yes | 3D mesh near, impostor far, per-instance dither |
| Grass LOD in LOD4 from `.cgid`, shared atlas, seasonal caches | Generator | Yes (`--grass bto/go`) | Instanced far grass (tier 2) |
| CS Grass Optimizations `_LOD0`/`_LOD1` grass meshes | Generator | `_lod1` only (renders the source material) | Both tiers |
| Seasons object + grass LOD | Generator | Yes | Same, plus seasonal impostor slices |
| PBR textures in object LOD (linear ×0.65) | Generator | Yes (`--materials pbr`, far-LOD match) | Real PBR flag on object LOD (earlier study) |
| Detailed ships from installed models, animated ships | Generator + runtime | Not confirmed | Same |
| Opaque copies for textures with gloss in alpha | Generator | Probably (`opaque` hits) | Same |
| Dynamic LOD without ESP records (`AnvilLOD.dyn`) | Runtime | No (uses DynDOLOD.esp + DynDOLOD DLL) | Native, part of tier 2 |
| Grid objects beyond loaded cells (water, waterfalls, windmills, fires, Near/Far grid), water stand-ins | Runtime | Through DynDOLOD's DLL | Native |
| Candles / FXGlow far LOD | Generator + runtime | Not found | Native |
| LOD distance INI settings, DynDOLOD MCM files read and hot-reloaded | Runtime | DynDOLOD DLL does it | Native |
| Per-layer, per-level show/hide in game (grass, 3D trees, cards) | Runtime | No | Native, and cheaper (cull instead of hide) |
| Adaptive quality from FPS | Runtime | No | Native, driving pixel-size thresholds |
| SKSE Menu Framework pages | Runtime | No | Native |
| Incremental rebuilds (per-block fingerprints) | Generator | No | Wanted |
| Author tools: missing-LOD finder, LOD mesh maker, billboards for all trees, tree stand-ins | Generator | Partial (billboard render) | Wanted |
| Rule editor UI + custom rules file | Generator UI | Not confirmed | Wanted |

AnvilLOD's SKSE plugin is GPL-3.0-only, and ports from it with attribution are allowed (decision of 2026-10-19). Its `Controller.cpp` (dynamic LOD and grid objects) and `LodDistances.cpp` are direct starting points for the runtime.

## 6. Suggested order

1. **Measure:** RenderDoc draw counts and GPU time for tree LOD, grass and object LOD at three fixed spots, with LOD layers on and off.
2. **Zero-code grass check:** NGIO extended grass + Grass Optimizations `RenderDistanceOverride` + mesh tiers. It shows how far instanced grass goes before memory runs out.
3. **Tier 1 impostor prototype:** LODForge frame-grid atlas + replacement `DistantTree.hlsl`.
4. **Tier 2 runtime** as a Bottled Shaders feature: trees first, then far grass, then dynamic LOD and grid objects (porting AnvilLOD's controller), then the menu and adaptive quality.
