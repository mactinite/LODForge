# Gotchas

Things that cost time during development and are easy to walk into again. Each entry says what
goes wrong and what to do about it.

## Building and installing output

- **Rebuild into the same output folder.** A rebuild reads the previous `DynDOLOD.esm` /
  `DynDOLOD.esp` from the output to keep form IDs and the run token, so saves made on the earlier
  build keep working. `--renumber` assigns new IDs (and implies `--new-token`): use it only with a
  new save.
- **Output is not written atomically.** Files are written as stages finish. A nonzero exit code
  means the output must not be installed. An exit code of 0 can still include best-effort skips
  (a missing input asset, for example); read the log.
- **Never enable two LOD output sets together.** A DynDOLOD output and a LODForge output in the
  same profile mix plugins, meshes and textures from both.
- **Earlier generated mods are recognized, not detected.** The build leaves out the LOD output
  mods it knows and its own generated plugins. Name any other xLODGen, TexGen or DynDOLOD output
  mod with `--exclude-mod`. The diagnostic subcommands do not apply this filtering on their own.
- **`--enable` edits `modlist.txt`.** It needs Mod Organizer 2 closed, or MO2 overwrites the
  change.
- **Keep the "xLODGen Resource - SSE Terrain Tamriel" plugins disabled.** LODForge fills
  Tamriel's outer cells from the terrain LOD meshes the game ships, so the plugins are not needed.
  `Occlusion.esp` copies the winning worldspace record, so the Extend plugin's enlarged object
  bounds would stay in it after the plugin is removed.
- **An old installed copy lacks newer features without saying so.** A build started from a stale
  `tools\LODForge` copy runs the old pipeline. Every build logs its version, arguments and stage
  times to `%LOCALAPPDATA%\LODForge\logs\build-<time>.jsonl`.
- **Windows throttles background console programs.** Started from a hidden or background window,
  a process gets EcoQoS and a build runs on a few cores. The CLI opts out at start; set
  `LODFORGE_NO_HIGH_QOS` to keep Windows' default.
- **Mod Organizer 2 tool entries drop unescaped quotes.** MO2 stores tool arguments through
  QSettings; add the entry in MO2's dialog or escape the quotes in the INI.
- **The UI restarts itself outside MO2's virtual file system.** Started as an MO2 tool, it
  relaunches unhooked and passes its arguments through
  `%LOCALAPPDATA%\LODForge\launch-args.json`.
- **Portable builds need the Visual C++ v14 x64 redistributable**, and the UI needs WebView2. The
  binaries are unsigned, so SmartScreen can warn.

## Inputs

- **PGPatcher / ParallaxGen changes mesh CRCs.** DynDOLOD's LOD models are matched by the CRC of
  the full model. LODForge maps a patched mesh back to its original CRC through
  `ParallaxGen_Diff.json`; `LODFORGE_NO_PARALLAXGEN` turns that off.
- **`--materials pbr` needs the `textures\pbr\` sources in the load order.** `standard` ignores
  them. Object LOD does not get a True PBR shader on every shape: non-passthrough shapes use the
  ordinary lighting shader, as DynDOLOD's output does.
- **`--grass bto` needs an NGIO grass cache.** Cells without a `.cgid` file get no grass, with one
  warning per worldspace. LODForge does not generate grass placements. `--grass go` (Grass
  Optimizations meshes) needs no cache. The two modes are different outputs and cannot be compared
  with each other.
- **NGIO's precache crashes now and then.** Starting the game again continues where it stopped;
  `scripts/grass/Run-GrassPrecache.ps1` automates the loop. A cache names grass by load-order form
  ID; LODForge matches cache types by model and object ID, so a cache made before a plugin was
  added still works.
- **Classic tree LOD holds at most 256 tree types per worldspace.** `.btt` trees also draw only
  where LOD4 blocks are shown.
- **Object LOD has no alpha property.** The game alpha-tests every LOD shape at 128. On snow
  shapes the vertex alpha is the snow amount, not transparency.
- **PBR detail bakes on PBR pages render about 25% brighter** in Community Shaders 1.9.1, so
  `--pbr-lod-pages` is off by default.

## Comparing against reference tools

- **Pin a golden export's side files.** `AltTextures`, `VC`, `FlatTextures`, terrain data and
  atlas maps in the reference tool's export folder are overwritten by its next run. Copy them with
  the export and make their absolute paths point at the copies. Reading side files from another
  run once turned a 98% match into 34%.
- **A rebuild must not read its own previous output.** Apart from the two plugins, nothing from
  the output folder may reach the build through the virtual file system, or files drift between
  identical builds.
- **Green tests are not a golden comparison.** Golden tests skip when `LODFORGE_GOLDEN_BTR`
  fixtures are absent, and GPU tests skip without an adapter.
- **DynDOLOD and TexGen remember their last options, worldspaces and output path**, which
  override `-o:`. DynDOLOD refuses an output folder inside MO2's `mods` and a seasonal swap INI
  that contains "DynDOLOD" lines.
- **DynDOLOD's end-of-run dialog cannot be pressed remotely.** Dismissing it loses the plugins;
  click "Save and Exit" by hand.
- **For a DynDOLOD grass run, set `CSGrassOptimizations=-1` in both INIs first**, or Community
  Shaders' option replaces the Grass LOD checkbox.
- **`Invoke-QuickAutoClean.ps1` cleans plugins in place** in `Stock Game\Data`.

## Checking in game

- **Community Shaders HDR and frame generation break window captures.** Turn HDR off and capture
  the frame, not the window. Take at least two shots per vantage point to learn the noise floor
  before reading a difference into a comparison.
- **Put the player under the camera.** A free camera far from the player does not load the same
  LOD grid, and LOD drops in and out in ways that look like missing output.
- **A new game started with `coc` pauses on the Survival Mode prompt.** Answer it and confirm the
  game is running before measuring anything.
- **Turn off the vanity camera** for long captures (`setini "fAutoVanityModeDelay:Camera" 99999`),
  and give the viewer the season the game is showing.
- **Flicker needs consecutive frames.** Still images cannot show that it is absent. The distant
  hazy/crisp alternation seen during development also appears with DynDOLOD's output, so it is a
  renderer effect and not a property of the generated files. Keep `uLargeRefLODGridSize=11`.
- **Water reflections draw object LOD a second time.** `setini "bReflectLODObjects:Water" 0`
  turns that off at run time when measuring draw cost.
- **Byte-identical plugins do not prove a rebuild is safe for saves.** Load a save made before
  the rebuild against the whole rebuilt output, then save and reload.
- **ESM overrides of temporary references need `ONAM` in the plugin header.** That the records
  parse offline does not show the game uses them.
- **Load order:** `DynDOLOD.esm` as the last ESM, `DynDOLOD.esp` late, `Occlusion.esp` last.

## The viewer

- **The viewer shows LOD everywhere.** The game hides LOD under loaded cells; the viewer has no
  loaded cells. It does not draw `DynDOLOD.esp`'s dynamic LOD.
- **By default the viewer shows only the output folder's own files.** `--merge-load-order` shows
  what the game would display with the rest of the profile.

## Repository

- **The workspace is not uniformly rustfmt-formatted.** A broad format check fails without a
  regression, and CI treats formatting as advisory. Do not reformat files alongside a functional
  change.
