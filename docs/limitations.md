# Known limitations

LODForge aims at output the game and the DynDOLOD runtime accept, close to what the reference
tools produce. It is not byte-identical to them and does not claim full parity. This page lists
what is known to differ or to be unsupported. [Validation](validation.md) says how to check a
build; [gotchas](gotchas.md) lists operational traps.

## By design

- **Tolerance-level parity.** The object LOD, terrain LOD and occlusion code was written from
  specifications of the file formats and observable behavior, not from the reference tools'
  code ([licensing](licensing.md)). Geometry and textures match within tolerances; files are not
  byte-identical to xLODGen's or DynDOLOD's.
- **PBR means DynDOLOD's treatment of PBR inputs.** Non-passthrough object LOD shapes get the
  ordinary lighting shader with a converted (`linear.dds`) albedo; passthrough shapes keep their
  shader. True PBR is not enabled on every LOD shape.
- **No grass placement generation.** BTO grass and the runtime's far grass come from an NGIO
  grass cache. Cells without a cache file get no grass.
- **DynDOLOD's install is still an input.** Builds read its INI, rules, config
  lists and billboard templates, and by default the LOD models of DynDOLOD Resources.
  `--lod-models` and `--tree-models` generate missing models; they do not replace the rules or the
  runtime.
- **Windows only**, with a GPU that wgpu supports. There is no CPU-only path for every stage.

## Not supported

- **Complex tree atlas.** Classic tree LOD writes `ComplexAtlasTexture=0` and a diffuse atlas
  only.
- **TexGen presets.** `TexGen_SSE.ini` is not read; the TexGen stage runs with TexGen's default
  settings plus the command-line options.
- **Dynamic LOD in the viewer.** `lodforge view` does not draw `DynDOLOD.esp`'s references.

## Compatibility not certified

These work in the tested setups; they have not been checked across load orders, worldspaces and
saves widely enough to promise.

- DLL NG runtime data: the sections a build writes are a subset of what DynDOLOD writes for some
  load orders (base flag updates, disabled references), and non-default dynamic LOD options are
  untested.
- Save compatibility across rebuilds: form IDs and the run token are kept, and rebuilds of
  unchanged input are reproducible, but only single-save upgrades were tested.
- Settings: non-default values of rarely used `DynDOLOD_SSE.ini` keys and edited rules may not
  change the output the way DynDOLOD's would.
- The LODForge Runtime path (`--runtime lodforge`) is under active development; its file formats
  are versioned ([instance formats](runtime/instance-formats.md)) and change between releases.
