# xLODGen terrain LOD textures: behavioural spec (clean-room)

Target: `D:\Modlists\skyrim-cs\tools\xLODGen\xLODGenx64.exe`, "SSELODGen 4.1.5o EXTREMELY EXPERIMENTAL Terrain LOD Beta x64".
SHA-256 `738b2ac42aff3c438ae5706cb33b7e1410e17a3f34da8af9a0c14cef3c270ce4`. I analysed it statically with REA 6.4.0 and Ghidra 12.1.4 on 2026-10-17. No code was executed.
Function names are the analyst's own labels (the binary is stripped). The Ghidra addresses and the evidence table were moved out of the repository on 2026-10-20; only the observed behaviour remains here.

This spec describes behaviour only. It contains no decompiled code. "Observed" means I read it directly in the decompilation. "Inferred" means it depends on knowing a third-party library, and the library is named. "Unknown" means I could not establish it.

The terrain code is built on **Vampyre Imaging Library**, which Delphi xEdit already links. Several facts below depend on how Vampyre defines its types:
- `TImageData` is a 0x29-byte record.
- Format ids: `ifDefault=1` (A8R8G8B8), `ifR8G8B8=87 (0x57)`, `ifR5G6B5=82`, `ifDXT1=200`.
- `ResizeImage` filter 3 is `rfLanczos`, and filter 0 is nearest.
- `TColorFPRec` stores channels in B, G, R, A order. Channel index 0 is B, 1 is G, 2 is R, 3 is A.
- `TImagingCanvas.DrawBlend` factor enum: 2=One, 3=SrcAlpha, 4=OneMinusSrcAlpha.

Compression is not done inside the exe. It is delegated to `Edit Scripts\Texconvx64.exe`, which is a DirectXTex texconv build with file version 666.666.666.666.

---

## 0. Pipeline overview (order of operations)

1. **Setup** (`TerrainLOD_Generate`):
   - Read the flags: build diffuse, build normal, `TerrainNormalBakeMaps` (requires normals), `TerrainNormalBakeSpecs` (requires bake).
   - Read LODSettings, align the area to the stride (32), and set the default textures.
   - Compute the internal resolutions (§1, §2).
   - Pre-resolve LTEX→TXST paths for every LTEX in the load order into a cache.
2. **Season loop.** "Default" runs first, then each enabled season (SPR/SUM/AUT/WIN). For each one:
   1. Set the filename suffix.
   2. Possibly swap the default texture (§9).
   3. Run `PrepareSeason`. This clears the image cache and pre-renders the cell images named "default" (diffuse), "defaultn" (landscape normal map) and "defaults" (spec), plus a flat terrain normal "defaultm" and the two 3×3 splat brushes "square" and "diamond".
   4. Run a root `TQuadThread` over the worldspace (§10).
3. **Per cell** (`RenderCell`), using the internal resolution:
   1. Diffuse = layer blend (§2, §3), then vertex colours (§4), then a vertical flip.
   2. Normal = VNML (§6).
   3. Optionally bake the landscape normal maps (§6).
   4. Lanczos-downscale the cell image into **every enabled ancestor LOD image** (4/8/16/32) that contains the cell (§1).
4. **Per finished tile** (`TSaveTextureThread.Execute`):
   1. Apply brightness/contrast, then gamma (§5).
   2. Lanczos-resize to the output size, or to the "default size" if the tile is a default tile (§7).
   3. Save an uncompressed DDS (R8G8B8).
   4. Queue a texconv command (§8).
5. **`TConvertTexturesThread.Execute`** batches identical texconv command lines into `Texconv_<ws>_<n>_<m>.txt` file lists and runs `texconv ... -flist`.

---

## 1. Tile/quad mapping

- **Coverage (observed).** A LOD-L tile named `<ws>.L.X.Y.dds` covers cells `X..X+L-1` × `Y..Y+L-1`. (X, Y) is the south-west cell.
  - Tiles are aligned to the LODSettings grid. For Tamriel the log says "Level 4 to 32 stride 256 from -96,-96". The tile-local cell index is `((cx - originX mod L) mod L)`, folded to be non-negative (`BlitCellToLevel`).
  - The root quad is the stride (32 cells). It recurses 32→16→8→4.
  - A chunk is processed only if its **origin cell** lies within the worldspace cell bounds. This check is in TQuadThread.
- **Internal tile size (observed, main near the `TerrainDiffuseSizeLOD` loop).**
  - Internal diffuse size per level = `max(TerrainDiffuseSizeLOD, 512)`. The default is 256 when the INI value does not parse.
  - Internal normal size per level:
    - with baking on: `TerrainNormalSizeLOD`, with no 512 clamp;
    - with baking off: `L*64`, i.e. 64 px per cell, the native VNML resolution.
  - In the golden run the internal diffuse sizes are LOD4=1024 and LOD8/16/32=512.
- **Pixels per cell in the tile** = internal size / L.
  - Golden diffuse: LOD4 = 256, LOD8 = 64, LOD16 = 32, LOD32 = 16.
  - Golden normal: LOD4 = 256, LOD8 = 64, LOD16 = 32, LOD32 = 16.
- **How a cell gets into a tile (observed).**
  - Each cell is rendered **once** at a "cell master" resolution (§2).
  - For every enabled level in the ancestor stack, the cell image is cloned and **Lanczos-resized on its own** to `(size/L)²`. It is then copied into the level image at `x = localX*cs` and `y = (L-1-localY)*cs`, so north is at the top.
  - There are **no border or overlap pixels.** Cells abut exactly.
  - Because each cell is filtered independently, **no filtering happens across cell boundaries** inside a tile. Lanczos clamps at the edges of the cell image.
- **Texel-centre convention (observed geometry; filter maths inferred from Vampyre).**
  - Images use pixel-area texels. Pixel i of an N-px cell covers `[i, i+1)·4096/N` world units.
  - Vertex grids are mapped so that vertex 0 lies on the cell (or quadrant) west/south edge and the last vertex lies on the east/north edge. See §3 and §4 for the exact stretch-and-crop construction.
- **Final orientation (observed).**
  - Diffuse and layer images are built with row 0 = south. The cell diffuse is flipped vertically (`FlipImage`) before the blit.
  - VNML is written north-up directly.
  - The output DDS has row 0 = north.

## 2. Landscape texture sampling

- **LTEX→TXST (observed, `ResolveLTEX`).**
  - LTEX `TNAM` → TXST. `TX00` = diffuse, `TX01` = normal.
  - Specular source: TES5/SSE use `TX01` again, with spec in the normal map's alpha. FO4 uses `TX07`, and an FO4 `MNAM` BGSM can override.
  - Paths are normalised with a `textures\` prefix.
  - A missing TNAM gives `<Warning: No TNAM - Texture Set defined for ...>` and the default path is kept.
  - A TNAM pointing at a non-LTEX record logs "Found X instead of LTEX in ..., using <default>".
  - A missing layer record logs "texture layer n/a". Both cases use the default texture.
  - A missing file logs `<Warning: Texture not found A, using default B>`.
- **Default textures (observed, globals/3d0/3d8).**
  - TES5/SSE: `textures\landscape\dirt02.dds`, `dirt02_n.dds`, and `dirt02_n.dds` again for spec.
  - FO3/FNV: `dirtwasteland01*`.
  - FO4: `ground\commonwealthdefault01_d/_n/_s`.
- **Repeats per cell (observed constant).** The landscape texture is tiled **24 times per cell edge** in TES5/SSE, which is **12 per quadrant**. FO3/FNV use 16 and FO4 uses 12.
- **Source resolution (observed formula).**
  - `ppc = max over enabled levels of (internalDiffuseSize/L)`, with a floor of 64.
  - `texPx = nextPow2(floor(ppc/24) + 1)` for diffuse. Normals use the same formula with the normal sizes.
  - Golden: ppc = 1024/4 = 256, so texPx = nextPow2(11) = **16**. The log line "Building terrain LOD textures [...] 16/16" prints exactly these two numbers (diffuse/normal).
  - So the cell master image is **24·16 = 384 px**, and each quadrant is 192 px.
- **How a texture is sampled (observed, `CachedTexture`).**
  1. Load the DDS. Inferred: the top/main image only.
  2. Warn if it is not square.
  3. **Lanczos-resize the whole texture to texPx × texPx.**
  4. Tile it 12×12 with exact copies (`CopyRect`) into the quadrant image.
  - There is no per-pixel UV sampling, so there is no bilinear or box sampling of the source. The only filter is the Lanczos downscale.
  - The final 384 → 256 (LOD4) and 384 → 64/32/16 steps are also Lanczos.
  - The tiled image goes through R8G8B8 and back to A8R8G8B8. This **discards the texture's alpha**, so alpha becomes 255.
  - Loaded images go into an LRU cache with eviction above 200 entries. The pinned entries "default", "square" and "diamond" are never evicted.

## 3. Layer blending

- **Layer collection (observed, `CacheLandData`).**
  - Each quadrant gets an array indexed by `slot = layer+1`:
    - BTXT goes into slot 0, with layer = -1.
    - ATXT goes into slot `ATXT.Layer + 1`.
  - The ATXT quadrant comes from the header. An ATXT without a VTXT is **ignored**.
  - If two entries share a slot, the later one overwrites the earlier.
  - Unused slots stay empty: FormID 0, no VTXT.
- **Per quadrant q** (0=SW, 1=SE, 2=NW, 3=NE), slots in ascending order (`RenderCellDiffuse` → `DrawLayer`):
  - **Opacity mask:** a 33×33 image per quadrant (17 vertices → 2k+1 samples).
    - If there is no VTXT (BTXT, or an empty slot), the mask is fully opaque (255).
    - Otherwise each VTXT entry (u16 position p, 2 pad bytes, float opacity) is handled like this:
      - col = p mod 17 and row = p div 17. Row 0 is south.
      - a = Trunc(clamp(opacity, 0, 1)·255).
      - Stamp a 3×3 brush centred on mask pixel (2·col, 2·row), additively: `mask.R += brush.R · a/255`, using DrawBlend with SrcAlpha/One.
      - The brush is "square" when p is even and "diamond" when p is odd. Because 17 is odd, this is a checkerboard on (row+col).
      - Both brushes have centre 1.0. "square" has 0.502 (0x80) on all 8 neighbours. "diamond" has 0.502 on the 4 edge neighbours and 0 on the corners.
    - Result:
      - vertex pixels = a;
      - edge midpoints ≈ 0.502·(a₁+a₂);
      - quad-centre pixels ≈ 0.502·(sum of the two **even-parity diagonal** vertices).
      - This is equivalent to bilinear along edges plus a fixed alternating triangle split inside each quad. It is not true bilinear.
  - **Applying the mask** (`ApplyMask`):
    1. Lanczos-resize the 33² mask to (W + W/32)² = 198² for a 192-px quadrant.
    2. Crop at offset x+3, **y+4** (the extra +1 in y is a quirk, about 1 px southward).
    3. Multiply all four channels of the layer image by mask.R/255 using integer truncation. The result is premultiplied RGBA with alpha = mask.
  - **Compositing** (`CompositeQuadrant`; quadrant offsets are (0,0), (W/2,0), (0,H/2), (W/2,H/2) with row 0 = south):
    - Slot 0 (base) is drawn straight into the cell image with factors One / OneMinusSrcAlpha. Its mask is opaque, so it replaces that quadrant.
    - Slots ≥ 1 are accumulated **additively** (One / One) into a separate zero-initialised cell-sized buffer.
  - After all four quadrants, the accumulation buffer is composited over the cell with One / OneMinusSrcAlpha:
    `out = clamp(Σ wᵢ·cᵢ) + base·(1 − clamp(Σ wᵢ))`
    - Each step is quantised to 8 bits and clamped. Layer order only matters through clamping.
- **Missing BTXT (observed).** The cell image starts as a clone of the pre-rendered "default" cell, which is the default diffuse tiled 24×.
  - A quadrant with no layers at all stays default.
  - A quadrant with ATXT but no BTXT has an empty slot 0. That slot resolves to "texture layer n/a", which means the default texture drawn opaque.
  - The default comes from the globals above. For TES5-family seasons it can be replaced by the season swap of LTEX FormID **(address removed)** (§9).
- **Quirk.** An *empty middle slot* (for example layers 0 and 2 present, 1 missing) is drawn as the default texture with an **opaque** mask into the additive buffer. Inferred from the code path, not tested.
- **Quadrant borders.** Each quadrant's mask covers only its own 17×17 vertices, and quadrants are composited side by side. Shared border vertices are evaluated independently in each quadrant. Lanczos ringing is clipped to each quadrant's own image.

## 4. Vertex colours (VCLR)

Source: `BuildVCImage` and `ApplyVC` /. All observed.

- VCLR is used only when it is exactly 33·33·3 = 3267 bytes.
- It becomes a 33×33 image:
  - R, G, B = VCLR bytes 0, 1, 2. Row 0 = south, the same orientation as the layers.
  - A = byte((1 − TerrainVertexColorMultiplier)·255). The rounding helper is probably Round.
  - The multiplier is read from INI section `<game> LOD Options`, with default 1.0.
- The 33×33 image is Lanczos-resized to Trunc(W·33/32) = 396 px for a 384-px cell and centre-cropped (offset 6, symmetric). Vertex k therefore lands at continuous coordinate k·W/32.
- **Per pixel, in float on raw stored (gamma-space) values:**
  `out.rgb = lerp(cell.rgb · vc.rgb, cell.rgb, vc.a)`, where `vc.a = 1 − multiplier`, and `out.a = cell.a`.
  - With the multiplier at 1.00 this is a plain multiply by VCLR/255.
  - No sRGB conversion is done.
- Timing: VCLR is applied **after** all layers and **before** the vertical flip and the level downscale. It is applied only when diffuse building is on.

## 5. Gamma / brightness / contrast

Source: `TSaveTextureThread`. Canvas helpers,,,.

- The settings are per level: `TerrainBrightnessLODn` (default 0), `TerrainContrastLODn` (default 0) and `TerrainGammaLODn` (default 1.0).
- They are applied to the **finished internal-resolution tile**, after all cells are composited and **before** the final Lanczos resize to the output size, the DDS save and compression.
- They are skipped entirely when B=0, C=0 and G=1.
- **Formulas.** Pixels are floats in 0..1 on the raw 8-bit values, and the alpha channel is untouched.
  1. If B≠0 or C≠0: `v = v·(1 + C/100) + B/100`. There is no 0.5 pivot.
  2. Then, if G≠1: `v = v^(1/G)`. Golden G=1.25 gives v^0.8, which brightens.
  - Clamp to [0,1] and requantise to 8 bits on write (inferred from Vampyre's FP pixel setters).
- **sRGB.** None anywhere. Every operation (layer blend, VC multiply, Lanczos resizes, gamma) works on stored 8-bit values. texconv is called without `-srgb*`, and the output is `BC7_UNORM`.

## 6. Normal textures

- **Source (observed, `RenderCellNormal`).** The source is **VNML**, not VHGT.
  - The 33×33 signed bytes are encoded as **R = 128+nx, G = 128+nz (up), B = 128+ny**. Each signed byte gets (internal field), so this is offset binary.
  - Rows are written north-up: y = 32 − row.
  - The image is Lanczos-resized 33→66 and the interior 64×64 is cropped (vertex k → 2k). The result is **not renormalised**. Alpha = 255.
  - **Space:** object/model space with Z up stored in green. This matches the flat default "defaultm" = (address removed). FO3/FNV swap B and G.
  - A cell without LAND or VNML gets a flat 64×64 default.
- **Baking** (TerrainNormalBakeMaps; `RenderCell`, `BakePixel`, `TBN`, `Mat3MulRow`):
  1. The landscape `_n` textures are layer-blended exactly like diffuse (§3), at 24·texPxN per cell.
  2. Flip that image vertically, then **swap its B and G channels**.
  3. Lanczos-resize the terrain normal (64²) up to the baked size (384²).
  4. Per pixel:
     - **t** = tangent normal from the landscape map, (R, G, B)·2−1 in original channel terms, then normalised.
     - **N** = terrain normal, (R, B, G)·2−1, then normalised.
     - `a = normalize(cross(+X, N))`, `T = normalize(cross(N, a))`, `Bt = −a`.
     - `out = normalize(tx·T + ty·Bt + tz·N)`.
     - Re-encode as R=x, G=z, B=y, each `(v+1)/2`. Alpha comes from the baked map, which is 255 unless baking spec.
   - For a flat N this gives T=+X (east) and Bt=+Y (north).
- **Bake specular** (off in golden):
  - The spec image is spec `_n` with R and A swapped (non-FO4), blended the same way.
  - Its channel 2 is copied into the alpha of the terrain normal.
  - The normal is then saved as A8R8G8B8 instead of R8G8B8.
- **Raise steepness.** Only used when normal mipmaps are on. It is implemented by generating the mips internally. Off in golden; formula not analysed.

## 7. "Default" tiles

Observed in, and.

- Every level record carries two flags, "diffuse has content" and "normal has content". Both start false.
- **Diffuse.** Each rendered cell sets the flag on all ancestor levels unless the cell is "default". A cell is default only if **both** of these hold:
  - every layer slot in all four quadrants resolves to a diffuse path equal to the current default diffuse path (dirt02, or its season swap);
  - the cell has **no 3267-byte VCLR**. Any VCLR at all makes the cell non-default.
- **Normal.** The cell is default if it has no LAND or VNML. Baking does not affect this.
- **At save time:**
  - If the flag is false and `TerrainDefaultDiffuseSize` ≠ "None", the internal image is Lanczos-resized to `StrToIntDef(TerrainDefaultDiffuseSize, 128)`², which is 32 in golden.
  - Otherwise it is resized to `TerrainDiffuseSizeLODn`.
  - Normals work the same way with `TerrainDefaultNormalSize` (128 in golden).
- Brightness, contrast and gamma are still applied before this resize.

## 8. Compression

Observed: format map, command /, runner.

- The tile is first saved by Vampyre as an **uncompressed DDS**:
  - diffuse as R8G8B8 (format 0x57);
  - normal as R8G8B8, or as A8R8G8B8 when baking spec.
- If the configured format differs, a command is queued:
  `"<Edit Scripts>\Texconvx64.exe" -nologo -y -aw 256 -sepalpha [-m 1] <fmt> -o "<dir>"`
  Commands are grouped by identical command line and run through `-flist`.
- `-m 1` (no mips) is dropped when `TerrainDiffuseMipMapLODn` / `TerrainNormalMipMapLODn` is set. In golden this happens only for LOD4 diffuse. LODForge's `terrain-textures --mips golden` reproduces that; its default (`full`, since branch `terrain-mips-grass-atlas`) writes the same box chain on every level and kind, because un-mipped terrain LOD textures alias at distance.
- **Format ids:**

  | id | texconv arguments |
  |---|---|
  | 0xE1 "BC7 Quick" | `-f BC7_UNORM -bc q` |
  | 0xE2 (BC7 max) | `-f BC7_UNORM -bc x` |
  | 200 | `-f DXT1 -bc d` |
  | 201 | `-f DXT3 -bc d` |
  | 202 | `-f DXT5 -bc d` |
  | 0xCC | `-f BC4_UNORM` |
  | 0xCD | `-f BC5_UNORM` |
  | 0x52 | `-f B5G6R5_UNORM -bc d` |
  | anything else | no conversion |

- **Encoder.** DirectXTex via texconv; xLODGen has no BC7 encoder of its own.
  - texconv's default is GPU (DirectCompute) BC7 when a device exists. `-bc q` maps to TEX_COMPRESS_BC7_QUICK: on the CPU path that means mode 6 only, and on the GPU path it skips modes 0 and 2. `-aw 256` is the GPU alpha weight.
  - Which path actually ran is unknown. The texconv build is custom (file version 666.666.666.666).
- **Mip filter for LOD4 (inferred from DirectXTex).**
  - texconv's default filter plus `-sepalpha` (TEX_FILTER_SEPARATE_ALPHA) forces DirectXTex's non-WIC path. For power-of-two sizes that is a **2×2 box filter** on the UNORM values (no sRGB), with a full chain down to 1×1.
  - Confidence is medium because the build is custom.
- texconv is called synchronously, one at a time, under a global critical section. A non-zero exit logs "Error: Executing Texconv failure".

## 9. Seasons

Observed in main (season loop), `SeasonSwap` and `SeasonEnabled`.

- The season list is iterated with "Default" first. For each enabled season, the **whole worldspace is regenerated**:
  - the image cache is cleared;
  - the default/brush cells are re-rendered;
  - the output filenames get the suffix `.<SEASON>.dds` / `.<SEASON>_n.dds`.
- The swap is applied only at the **LTEX level**, at the moment a layer's LTEX is resolved:
  1. Take the winning override.
  2. If the season is not the default, look it up in the season map loaded from `Data\Seasons\*_SPR.ini` etc.
  3. If there is an entry, replace the record with the swap target's winning override.
  4. Re-resolve TNAM→TXST.
- For TES5-family modes (mode mask 0x760), LTEX **(address removed)** (looked up in the master file) is checked for a season swap. If one exists, its TXST becomes that season's default diffuse/normal/spec. That affects both the base fill and the "is default" test.
- **No incremental skipping (observed).** Each season produced the full 2040 files in the golden log, including `_n`. The only skip mechanism is "Skipping chunk ... because file already exists" for an existing **top-level (stride) tile** file. It is not season-aware beyond the filename.
- Baked normals are regenerated per season because the landscape `_n` textures swap too.

## 10. Parallelism

> Moved to the quarantine folder on 2026-10-20: this section listed Ghidra addresses and the thread structure of the reference binary, which the behavioural description does not need.

## Evidence table

> Moved to the quarantine folder on 2026-10-20: this section listed Ghidra addresses and the thread structure of the reference binary, which the behavioural description does not need.

## Open unknowns / caveats

1. **DDS load mip level.** I infer that the main (largest) surface is loaded (`LoadDDS` called with 0). I did not verify how BC-compressed sources are decoded (Vampyre's DXT/BC7 decoders).
2. **Lanczos details.** Vampyre `rfLanczos` kernel radius and edge mode are from library knowledge (radius 3, clamp). The exact numeric output should be verified against Vampyre `ImagingFormats.StretchResample`.
3. **Rounding helpers.** is Trunc: it is used for cell index i/4 and must truncate., used for the VC alpha, is probably Round. The exact 8-bit quantisation in Vampyre's FP setters is also unconfirmed.
4. **DrawBlend factor enum order.** I took it from Vampyre. If it were different, base/layer compositing would change. The code structure (base vs. accumulation buffer) strongly supports the reading here.
5. **texconv path.** GPU vs CPU BC7 and the mip filter are unknown/inferred, because Texconvx64.exe is a custom build (666.666.666.666). The mip filter is inferred to be box.
6. **Raise steepness and bake specular.** Internals were not analysed (both off in golden).
7. **Bounds check by chunk origin only** (§1). It could drop edge chunks in worldspaces whose minimum cell is not aligned. Not tested.
8. **Empty middle ATXT slot behaviour** (§3) is read from the code path, not validated on data.
9. **Path compare case-sensitivity** for the "is default" test (, CompareText vs CompareStr) was not confirmed.
10. **Water and LOD-mesh interaction.** The texture code does not read water height or the BTR meshes. No evidence of any other dependency.
