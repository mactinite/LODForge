# LODForge UI options inventory (2026-10-24)

What the UI has to cover: every `lodforge build` option and every flat file the CLI reads.
Sources: `apps/cli/src/main.rs`, `build.rs`, `dyndolod_build.rs`, `profile.rs`; UI backend
`apps/ui/src/main.rs`. The UI's option catalog is `apps/ui/web/src/options.ts`.

## Build options by area

- **General:** `--instance`, `--profile`, `--game`, `--exclude-mod`, `--exclude-plugin`,
  `--worlds`, `--seasons`, `--mode` (UI: always `dyndolod`), `--materials pbr|standard`,
  `--keep-lod-mods`, `--no-terrain/--no-objects/--no-trees`, `--json`.
- **DynDOLOD:** `--preset Low|Medium|High`, `--child-parent Low|Medium|High`,
  `--dyndolod-config`, `--dyndolod`, `--static-lod`, `--no-plugins`, `--no-occlusion`,
  `--new-token`, `--renumber`, `--no-large-ref-workarounds`, `--windows`, `--work-dir`.
- **Terrain:** `--no-lod-fill`, `--terrain-mips full|golden|none`, `--terrain-filter hq|xlodgen`,
  `--lod32 step|golden`, `--terrain-pbr-scale` (0.65..1), `--no-underside`.
- **Objects / atlas:** `--atlas-max-tile`, `--atlas-full-max-tile` (pow2 64..4096),
  `--lod-texture-tone flat|dyndolod`, `--far-lod-match` (0..1), `--keep-far-lod-models`,
  `--lod-models`, `--lod-model-cache`, `--lod-model-min-size` (3 sizes).
- **Trees:** `--texgen`, `--tree-lod classic|ultra`, `--tree-models off|generate|all`,
  `--tree-model-levels keep:area:bark:floor[:max_scale]`×3, `--tree-model-crown cloud|prune`,
  global `--billboard-scale`, `--billboard-supersample 1..8`, `--installed-billboards
  render|keep`, `--billboard-density 0.1..1`, `--billboard-soft`.
- **Grass:** `--grass off|bto|go`, `--grass-source auto|cache`, `--grass-density` (%).
- **Textures / output:** global `--bc7 fast|high|best`, `--out` | `--mo2-mod` [`--enable`].

## Files read at build time

`<cfg>` = `tools\DynDOLOD\Edit Scripts\DynDOLOD` (`--dyndolod-config`).

| File | Format | Controls | Override before this change |
|---|---|---|---|
| `<cfg>\DynDOLOD_SSE.ini` `[DynDOLOD]` | `key=value` | atlas, grids, occlusion, grass, flat LOD colours, ignore lists | none |
| `<cfg>\Rules\DynDOLOD_SSE_<Preset>.ini`, `_all*.ini`, per plugin | `LODGen<n>=mask,LOD4,LOD8,LOD16[,LOD32],Grid,Reference,Flags` | LOD per mesh / reference | `Data\DynDOLOD\<name>` via VFS |
| `Configs\DynDOLOD_SSE_mesh_crc32.txt` | `meshes\x.nif;CRC` | replaced vanilla meshes | none |
| `Configs\DynDOLOD_SSE_xesplod.txt` | `Plugin;FormID` | enable parents treated as absent | none |
| `Configs\DynDOLOD_SSE_misnamed.txt` | Data path per line | excluded LOD models | none |
| `Configs\DynDOLOD_SSE_mod_world_ignore.txt` | `plugin=World,World` | refs a plugin adds/overrides in worlds | none |
| `Configs\DynDOLOD_SSE_childworld_<Child>.ini` | INI `[Parent]` keys | child world copies | none |
| `Configs\DynDOLOD_SSE_childworldfull.txt` | `plugin;FORMID` | copies keep full model | none |
| `Configs\DynDOLOD_SSE_ChildworldMatches.txt` | `childPlugin;childID;parentPlugin;parentID;...` | known pairs | none |
| `Configs\DynDOLOD_SSE_TexGen_{Billboards,Required_Billboards}_<plugin>.txt` | `Plugin;FORMID[;EDID][,True\|False][,Scale=x]` | billboard selection | VFS |
| `Configs\DynDOLOD_SSE_TexGen_{noalpha,alpha,copy}_<plugin>.txt` | tab-separated stitch rows | mini atlases | VFS |

`TexGen_SSE.ini` is not read. Also read: MO2 profile files, `lodsettings\*.lod`, `seasons\*.ini`,
`ParallaxGen_Diff.json`, NGIO `grass\*.cgid`, the previous `DynDOLOD.esm/.esp`,
`LODForge_TreeModels.txt`.

## `build --json` events

`begin {world, stage, season}`, `stage {world, stage, season, ms}`, `done {ms, failed}`. A
`begin` without a `stage` = the stage failed (stderr has the error). Some plain lines are not
JSON; ignore lines not starting with `{`.

## Reports a build writes

`LODForge_TreeReport_<World>.csv` (`plugin,form_id,editor_id,model,model_crc32,references,lod4,
lod8,lod16,lod4_model,lod4_source,note,generator_candidate`), `LODForge_TreeModels.txt`,
`textures\terrain\lodgen\TexGen_SSE_Tree_Billboards.txt`.
