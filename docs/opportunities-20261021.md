# LOD quality and build-speed opportunities (2026-10-21)

Read-only audit of main at 6a31af8 (v0.3.0). Nothing here was run or measured
anew; impact figures are estimates from the code paths plus the timings already
in `baseline.md`. Items marked **verified** were re-read line by line after the
audit; the rest are the auditors' readings and should be checked before acting.

## Where the time goes today

Full headless build, 15 worlds x 5 seasons, `--texgen --mode dyndolod --grass go`
(baseline.md, 417 s wall, 155 stages):

| Stage (summed over worlds and seasons) | s |
|---|---|
| terrain-textures | **316** (Tamriel 58.7, Solstheim 53.7) |
| objects | 21.7 |
| occlusion | 15.7 (quality 2 Tamriel alone is 11 min; the 15.7 s run used a lower setting) |
| dyndolod-export | 15.4 |
| texgen billboards | 13.7 |
| trees | 11.4 |
| objects-atlas | 10.8 |
| terrain-meshes | 9.2 |

Tamriel single season, grass bto (bench/full-validation-20261019, 159 s):
objects 48, grass 24, texgen 20, occlusion 16, terrain-textures 14, stitched 12.

Two facts drive the speed section:

1. Solstheim costs almost as much as Tamriel. Terrain-texture time is dominated
   by fixed per-call setup, not by baking pixels. Tamriel's default season is
   ~3 s setup + 5.6 s diffuse loop + 8.7 s normal loop; small worlds are mostly setup.
2. Everything is strictly serial: one world after another, one season after
   another, one stage after another, and inside each stage the CPU waits on the
   GPU and the GPU waits on the CPU.

## Speed: ranked

| # | Change | Where | Est. gain on the 417 s build | Effort |
|---|---|---|---|---|
| S1 | **Done 2026-10-21** (terrain lane commit; `TerrainContext`). **Build terrain-texture inputs once per build.** `SpecTextureSet::build` decodes the *top mip* of *every LTEX in the load order* (not just the world's) into f32, Lanczos-shrinks it to 16 px, and does so per world x season x kind = 150 times. **verified** `crates/terrain/src/gpu_spec.rs:26-51`. Fix: pick the smallest source mip >= 8x `tex_px`, restrict to LTEX ids the world's LAND records reference, cache by (path, tex_px, kind) across worlds and seasons, remap only swapped ids per season. | gpu_spec.rs:26-63, terrain_textures.rs:184-269, bake.rs:41-49 | 150-250 s | M |
| S2 | **Done 2026-10-21.** **Skip setup when a season changes nothing for a world.** If every `only[kind]` set is empty, hard-link and return before texture-set build or `Gpu::new`. | terrain_textures.rs:229-266 | tens of s | S |
| S3 | Terrain stages share one `Gpu` since 2026-10-21; the object-side stages still create their own. **One `Gpu` per build, passed by reference.** 15 call sites create a device and recompile WGSL (**verified** count). No wgpu pipeline cache (`cache: None`). | grep `Gpu::new` in apps/cli/src; crates/gpu/src/lib.rs:85,95 | 10-40 s (measure one `Gpu::new` first) | S-M |
| S4 | **Done 2026-10-21** (`apps/cli/src/terrain_lane.rs`): full build 516 s serial-equivalent -> 347 s. **Run terrain textures on their own thread.** They read only the VFS. Run all worlds x seasons on a dedicated thread while the main thread does export, objects, trees, occlusion. Wall time becomes max(GPU chain, CPU chain) instead of the sum. Needs the `&mut Profile` / `own_output` VFS mutation split out. | build.rs:332-362 | 70-100 s off the critical path | M |
| S5 | **Overlap CPU and GPU inside terrain textures.** Per top tile: pack N+1 on rayon while the GPU bakes N; mips on the GPU from the level buffers; all levels' BC7 in one submit; readback BC7 blocks (1 B/px) instead of RGBA (4 B/px) plus the extra `.to_vec()`; pool the per-batch buffers (~200 MB of wgpu zero-fill per batch today) and store `tmp` as f16. | terrain_textures.rs:297-380, crates/gpu/src/terrain_spec.rs:141-272, lib.rs:104-169 | 1.5-2x on the bake loops | M |
| S6 | **Occlusion: max-height pyramid + quick accept.** Hot loop marches every 128-unit step of every ray (**verified** `visibility.rs:200-225`), ~256 rays per (cell, block) pair, radius 100 cells. A max-pyramid over `ground` lets a ray skip 2^k tiles whose max is below the line; if both endpoints clear the bounding-box max, accept without marching. Then pad the map and bake `default` in to drop the per-step branches. GPU port only if still needed. | crates/dyndolod/src/occlusion/{visibility,grid}.rs | 11 min -> ~1 min on Tamriel at quality 2 | M |
| S7 | **Cache derived load-order data on `Profile`.** `scan_worldspaces` runs ~10x per world; `read_bases`, `read_materials`, `load_swaps`, `land_textures`, `decode_lands` re-run per world or season. | terrain_textures.rs:184-209, object_export.rs:51-59, tree_lod.rs:231-236, build.rs:125/293/568 | 15-30 s | S |
| S8 | **Share models, BTR terrain, LAND across seasons in object LOD.** `ObjectLod::new` re-imports all of it per season job; the tree atlas is recomposed and encoded per season even with identical slots. | object_lod.rs:248-328, tree_lod.rs:247-281 | several s per world | S |
| S9 | **TexGen billboards.** CRC-keyed skip of unchanged billboards on reruns (sidecars already carry ModelCRC32 and texture CRCs); one `encode_bc7` per batch instead of per image; parallel CPU prep; cache `TexData` across batches. | texgen_billboards.rs:186-250, 785-794; crates/texgen/src/render.rs:335-361 | reruns ~free; first run ~2x | S |
| S10 | **Atlas: stop decoding 4K textures to test transparency**, cache decoded tiles by (path,w,h), `vfs.exists` instead of reading the normal file. | object_atlas_plan.rs:187-195, glow_atlas.rs:101, object_atlas.rs:246 | s to tens of s on big lists | S |
| S11 | **Compile flags.** `-C target-cpu=x86-64-v3` (AVX2 for Lanczos, DDS decode, mesh code), `lto = "fat"`, `panic = "abort"` in a `release-native` profile so shipped binaries stay portable. | Cargo.toml:43-46 | 5-15% on CPU stages | XS |
| S12 | **Identical-tile reuse for normals.** No default-tile shortcut for normals (`default_normal_size: None`), so Soul Cairn / Sovngarde render and encode every empty normal tile. Hash tile RGBA and hard-link, or add the flat-normal shortcut. | terrain_textures.rs:271-282, spec.rs:69 | large on empty worlds | S-M |
| S13 | **Incremental builds across runs.** Hash per-world inputs (plugin mtimes, LTEX set, swap INIs, settings) into a manifest and skip unchanged worlds or seasons. Biggest win for iterative rebuilds. | build.rs | most of a rebuild | L |

Small items: `hidden.rs:244-253` samples all 7 points when the rule needs 3;
`heightmap.rs:81` allocates per point; `BtrTerrain::mesh` and the texture caches
can parse the same file twice (`hidden.rs:385-396`, `bake.rs:33-38`, `spec.rs:104-117`);
`gpu_spec.rs:122-180` packs 1,024 cells serially with a String allocation per layer;
`atlas_build::fit` bubble-sorts and refits per popped block.

Suggested order: S1 + S2 + S3 (quick, safe, biggest), then S6 (occlusion), then
S5/S4 (pipelining), then S7-S10, S11 anytime.

## Quality: ranked

### Likely bugs (fix regardless of parity policy)

| # | Finding | Where | Status |
|---|---|---|---|
| B1 | **Normal page is zero-filled** except on glow pages. A tile whose `_n` fails to load `continue`s, then alpha is forced to 255, so the tile decodes as normal (-1,-1,-1): wrong lighting on every such atlased shape. Fix: fill with (128,128,255,255) first. | apps/cli/src/object_atlas.rs:216-252 | **verified** |
| B2 | **Zero bitangents** when a shape has tangents but no bitangents in the LOD-model generator: broken tangent frame, wrong normal-mapped lighting on generated LOD. Fix: cross(n, t). | crates/lodmodel/src/lib.rs:213-217 | **verified** |
| B3 | **Glow lost on non-passthru glow rows.** Slot 2 gets `_g` but the shader is type 0 with emissive 0 and no `Glow_Map` flag, so window glow never shows except on `glow_lod` passthru models. | crates/objlod/src/material.rs:190-257 | needs a check |
| B4 | **Tree billboards built without normal maps** (`normals=false`): flat-lit tree LOD in object LOD. | apps/cli/src/build.rs:667 | verified call site; confirm the flag's meaning |

### Terrain LOD

| # | Finding | Where | Impact / effort |
|---|---|---|---|
| T1 | **Per-cell downscale with clamped edges.** Each cell is Lanczos-resized alone, so the kernel never sees neighbours. Leaves a visible cell grid that worsens at LOD16/32. Copies xLODGen. Fix: downscale the stitched tile with a 1-cell apron. | terrain_spec.wgsl:249,274; spec.rs:382-422 | High / M |
| T2 | **Mips are 2x2 box on 8-bit gamma data; normals not renormalised.** Shortened normals flatten distant lighting; diffuse mips are not gamma-correct. Build float mips on the GPU (comes with S5), renormalise normals, Kaiser/Lanczos filter. | terrain_textures.rs:125-147 | Medium / S-M |
| T3 | **PBR landscape textures get no treatment.** `LandTexture` holds only diffuse/normal/specular; raw PBR albedo is baked, while object LOD scales PBR albedo by 0.65 (`pbr_linear.rs:20`). Terrain LOD ends up brighter than object LOD on a PBR list. Fix: detect `textures\pbr\` or an `_rmaos` companion, apply the same scale, optionally multiply AO from rmaos. This is the top item for the CS + PBR target. | crates/esp/src/terrain.rs:151-157, crates/terrain | Med-High / S-M |
| T4 | **8-bit truncation at every stage, tone curve after quantise** (deliberate for golden fidelity). An opt-in `hq` path keeping f32 through the GPU and rounding once would remove the darkening bias and banding. | terrain_spec.wgsl:169-187,279-287; image.rs:151-163 | Medium / S |
| T5 | **BC7 encoder is mode 6 only**, <=2 refinement passes, alpha counted in the error. 1-3 dB available at texture boundaries from modes 1/3 (or a bc7e-style port); channel-weighted error for normals. | crates/gpu/src/bc7_mode6.wgsl | Medium / M-L |
| T6 | Gamma step LOD16 -> LOD32 (1.25 vs 1.0, copied from golden). | terrain_textures.rs:99 | Low-Med / S |
| T7 | Moiré from 16 px textures tiled 24x then 1.5:1 downscale. `--tex-px 32` gives an exact 3:1 at 4x render cost. | spec.rs:76-78 | Low-Med / S |
| T8 | VNML upsampled twice through 8-bit; alternative is normals from float VHGT. Trade-off: full terrain lights with VNML. | spec.rs:357 | Low-Med / M |
| T9 | Mesh: L-inf Delaunay greedy insertion (faithful). Data-dependent edge flips or a slope-aware metric would improve ridge silhouettes at LOD16/32. `forsyth_order` exists but `Btr::from_mesh` doesn't use it (runtime vertex-cache cost). | crates/btr/src/greedy.rs, btr.rs:178 | Low-Med / M-L |

### Object, tree and billboard LOD

| # | Finding | Where | Impact / effort |
|---|---|---|---|
| O1 | **Atlas bleed.** No gutter in the packer, Lanczos mips over the whole page (at mip 7 a 256 px tile is 2x2 and the kernel reaches ~3 px into neighbours), UV remap allows +-0.2 outside [0,1] into neighbouring tiles, clamp only at the page edge. Fix: per-tile mips assembled per level (also parallelises the serial page encode), 4-8 px gutter, half-texel UV inset. Atlas parity is already gone by design, so layout changes are free. | atlas_build.rs:135-166, object_atlas.rs:47-58, atlas_map.rs:42-45, dyndolod_build.rs:635 | High / M |
| O2 | **Tree atlas rims.** 16 px padding is transparent black and the average fill covers only the tile, so lower mips pull black into edges: dark outlines on distant trees. | tree_lod.rs:143-160 | Medium / S |
| O3 | **Billboard generational loss.** Rendered -> BC7 -> decoded -> BC7 again into the atlas, with a mode-6-only encoder that is weak on hard alpha edges. Hand RGBA through in-process; add modes 1/3/7 or a CPU bc7e path for alpha pages. | texgen_billboards.rs:244, glow_atlas.rs:101-121 | Medium / S + M |
| O4 | **Foliage edges aliased.** MSAA on, but `discard` per pixel and `alpha_to_coverage_enabled = false`, so only triangle edges get coverage. Try alpha-to-coverage or 2x supersample; check against golden (97.5% alpha agreement today). | crates/texgen/src/render.rs:296, billboard.wgsl:130 | Medium / S |
| O5 | Normal pages not renormalised after resize/mip. | object_atlas.rs:263 | Low-Med / XS |
| O6 | **PBR object LOD is flat.** `pbr_linear` scales RGB by 0.65 and sharpens alpha; RMAOS never used; billboard shader only multiplies by `pbr_scale`. Optional quality mode: bake AO (and darken metals) into the linear copy. | pbr_linear.rs:21-39, billboard.wgsl:197 | Medium on PBR lists / S |
| O7 | LOD-model generator: `permissive: true` at every level lets collapses cross UV seams on LOD4; no vertex-cache/overdraw ordering; root written as `NiNode` not `BSFadeNode`. | crates/lodmodel/src/lib.rs:49-53, 407 | Low-Med / S |
| O8 | Known parity residuals (object-lod-spec §11.8, §12): HD flag on LOD8 objSnow, own-texture colour cell, 18/109 passthru Own_Emit, grass shape boundaries and half-precision vertex drift, LOD8/16 colour grouping costing 1.9/3.9 pts. | spec | tracked |

## Recommended first batch

1. B1, B2, O5, S11 (hours; visible lighting fixes plus a free CPU boost).
2. S1 + S2 + S3 (terrain-texture setup once per build): the single biggest build-time win.
3. S6 occlusion pyramid (11 min -> ~1 min at quality 2 makes quality 2 the default).
4. T3 + O6 PBR brightness consistency between terrain and object LOD (the CS + PBR target).
5. O1 + O2 atlas gutters and per-tile mips, T1 tile-wide terrain downscale, T2 float mips: the three largest distant-LOD visual wins, all behind an opt-in `hq` profile so golden-comparison runs stay possible.
6. S4/S5 pipelining once the setup costs are gone, then S13 incremental builds.

## Occlusion stage in depth (2026-10-21 follow-up)

Numbers: Tamriel 5,697 cells, grid stride 256 = 64 x 64 leaf blocks, `OcclusionRadius` 100
cells, quality 2 (2 x 2 sample points per cell). The clean-room module
(`crates/dyndolod/src/occlusion/`, merged 2026-10-20) takes 11 min on Tamriel at 16 threads;
the full-validation build of 2026-10-19 still used the old module and finished the whole
occlusion stage in 15.7 s. The rewrite made the stage ~40x slower.

Work per cell (`visibility.rs:233-272`): ~1,960 blocks inside the radius; per block up to
16 cells x 4 samples = 64 targets x 4 eyes = 256 rays; a ray marches every 128-unit sample
along the line (`obstruction`, up to 3,200 steps) with a float `round`, two bounds checks and
a NO_DATA substitution per step (`grid.rs:142-148`). A visible block exits on its first clear
ray; a hidden block pays all 256 rays, each until it meets its blocker. Total ~11 M
(cell, block) pairs, on the order of 10^11 samples.

Ranked options, cheapest first. The first three keep the result bit-identical.
**OC1-OC3 implemented 2026-10-21** (working tree; see baseline.md "Occlusion: hierarchical line of
sight"): 300 cells 27.9 s -> 6.4 s, all of Tamriel 494 s -> 91 s, Occlusion.esp byte-identical.

| # | Change | Expected | Effort |
|---|---|---|---|
| OC1 | **Bake the map for the inner loop.** Pre-substitute `default` into `ground`, add a border of `outside` around the map so `ground_at` is a plain load with no branches, and keep `contains_sample` out of the loop by marking border samples. Replace the per-step float `round` with an exact integer DDA (the long axis advances by 1 per step; the short axis is `floor((2*d*s + steps) / (2*steps))`, identical to the current rounding). | 1.5-2x | S |
| OC2 | **Max-height pyramid over `ground`.** Levels of 2, 4, ..., 512 samples holding the max height. While marching, if `max(span) - clearance <= min(line over span)` the span cannot block and is skipped; otherwise descend. Falls back to level 0 only near real obstructions, so results are unchanged. Sea, tundra and plains rays collapse to a handful of lookups. | 5-20x on open terrain, less in mountains | M |
| OC3 | **Quick accept per (eye, target).** Before marching, take the pyramid max over the bounding box of the line; if it minus the clearance is below `min(e.h, t.h)` the pair is clear with no march. Most visible blocks on flat ground resolve in O(1). | multiplies with OC2 | S (needs OC2) |
| OC4 | **Per-eye horizon sweep (approximate).** Blocked iff `max_s (ground(s) - clearance - e.h) / s > (t.h - e.h) / S`, so for one eye and one direction a single march computing the prefix max of the obstruction slope answers every target along that direction. Per cell: 4 eyes x D directions x 3,200 steps instead of ~125k exact rays. Direction quantisation changes which samples a line visits, so it moves the parity numbers (harness gate <=8% / <=12%) and needs a run through `Test-OcclusionParity.ps1`. The spec lists it as the not-done "per-eye horizon". | 20-100x | M |
| OC5 | **wgpu compute port.** One thread per (cell, block) or per ray; the map (~100 MB) plus pyramid as a storage buffer or a texture with max-mips. Everything above (OC1-OC3) ports as-is. The GPU is idle during this stage today. | whole stage well under 30 s | L |
| OC6 | **Housekeeping.** Child worldspaces run serially after the main one (`dyndolod_build.rs:827-860`); `eyes()` allocates per cell; `block_order` stores 64 x 64 `Vec<u16>` (fine). Together under 5%. | small | XS |

OC1 + OC2 + OC3 are done (exact, 494 s -> 91 s on Tamriel). Next, if the stage still matters: a
lower-bound test per (eye, block) with a min pyramid over the cone of rays, so a hidden block
is proven hidden without its 256 rays, then OC4 or OC5. A quick profile before coding: time `compute` for a plains cell and a
mountain cell and count steps per ray; that tells how much OC2 will buy versus OC4.
