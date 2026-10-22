# LOD viewer

`lodforge view` shows a worldspace's generated LOD the way the game arranges it, so a build can
be inspected and checked without launching Skyrim: terrain LOD (`.btr` with its diffuse and
normal tiles), object LOD (`.bto`, including the tree and grass billboards DynDOLOD-style output
puts there), classic tree LOD (`.lst`/`.btt` over the tree atlas) and the Grass Optimizations
meshes listed for reference. The same renderer works without a window for automation.

## Running it

```powershell
# Window on an output folder (a build's --out, or an MO2 mod folder).
.\target\release\lodforge.exe view --out 'D:\Modlists\LODForge-out\lane-after' --world Tamriel

# With the load order behind it: textures and models the output references but does not carry
# (full textures of HD shapes, passthru models) resolve as in game, and lodsettings\*.lod gives
# the LOD grid origin.
.\target\release\lodforge.exe view --out '...' --world Tamriel --instance 'D:\Modlists\skyrim-cs' --profile Default

# A season's files (falling back to the default season's where a season has none).
.\target\release\lodforge.exe view --out '...' --world Tamriel --season WIN
```

The UI's Done screen has an "Inspect LODs" button that runs the same command on the build's
output. Builds whose output is an MO2 mod are read from `<instance>\mods\<name>`.

By default the LOD files shown are the output's own, which is what a build check wants. With
`--instance --merge-load-order` the load order's LOD files fill the blocks the output lacks
(vanilla and other mods' `.btr`/`.bto`/`.btt`), which is what the game displays.

Options that matter for big worlds: `--load-levels 4` loads one level only; `--texture-budget-mb`
(default 3072) caps the VRAM the terrain tiles take, dropping top mip levels per level when the
world's tiles would exceed it (the status line and the selection panel say when that happened);
`--no-normals` skips normal maps.

With `--instance`, `--out` may be left out: the viewer then opens the profile's generated LOD mod
(the highest-priority enabled mod that LODForge or DynDOLOD wrote), and it starts on Tamriel when
the output has it. That keeps an MO2 tool entry down to `view --instance "<instance>"`. MO2 stores
executable arguments with Qt's QSettings, whose INI reader drops every unescaped `"` in a value; an
entry that needs quotes (a path with a space) must be written in the escaped form
(`13\arguments="view --out \"...\" ..."`), or entered through MO2's own executables dialog. When
started from MO2 (a console of its own), the viewer releases that console so no console window
stays open beside it.

## Window

| Control | Effect |
|---|---|
| Right mouse (hold) | Look around |
| `W A S D`, `Q`/`E`, Shift, Ctrl, wheel | Move, down/up, fast, slow, change speed |
| Left click | Pick the shape under the cursor (details in the right panel) |
| `F` | Fly to the selection; `Esc` clears it |
| `M` | Top-down overview: drag to pan, wheel to zoom, double-click to drop the free camera there |
| `1` `2` `3` `4` | Show LOD4 / LOD8 / LOD16 / LOD32 everywhere |
| `G` | The game's arrangement: LOD4 near the camera, LOD8, LOD16, LOD32 with distance |
| `C` | Run the checks (issues panel; `go` flies to an issue) |
| `F12` | Save a screenshot in the working directory |

**Levels.** "Game" mode splits the quadtree the way the game does with `fBlockLevel0Distance`,
`fBlockLevel1Distance` and `fBlockMaximumDistance` from the profile's `skyrimprefs.ini` (with
`--instance`; else the one in Documents\My Games, else the game's defaults 35000 / 70000 / 250000;
the slider scales them), so level transitions and their seams sit where they sit in game. In
the overview the distances are taken from the overview's centre, which makes the quadtree itself
visible with "block outlines" on. A single level shows every block of that level, for example
to look at LOD16 up close.

**Layers** switch terrain, water, object LOD, the tree and grass billboards inside object LOD,
classic tree LOD and a "large references only" filter.

**Shading.** Textured (sun and ambient, normal maps), unlit (the textures as stored), normals
(world-space, as lit), flat (geometry only), by level (LOD4 green, LOD8 yellow, LOD16 orange,
LOD32 purple), by kind (terrain, water, object, tree billboard, grass) and normal map (the
normal texture as stored; terrain LOD normal maps keep the up axis in green). "Weather sky / fog / water" (on by default, `--no-atmosphere` headless; free camera only) takes
the day values of a weather record (`--weather`, default `SkyrimClear_A`, read from the load
order's winning WTHR with `--instance`): the sky gradient (far fog colour at the horizon, then
horizon, sky lower and sky upper colours), the distance fog (`FNAM` near / far / power / max; the
amount is `((d - near) / (far - near)) ^ (1 / power)`, capped at max, colour from the near to the
far fog colour) and sunlight + ambient instead of the plain grey light, all in gamma space as the
game computes them without linear lighting. Distant water uses the worldspace's LOD water type
(`WRLD NAM3`, `WATR DNAM`): its deep colour, giving way to the reflected sky (the far fog colour)
by a fresnel term. Clouds, the sun disc, specular and the game's image-space exposure are not
reproduced, so the game reads brighter under a cloudy sky; the fog exponent was checked against
in-game captures. "LOD snow / ash" (on by default, `--no-snow` headless) projects the directional material the game puts on
`objSnow…` / `objSnowHD…` / `objAsh…` / `objAshHD…` shapes: the game picks SnowLODMaterial,
SnowLODMaterialHD (Skyrim.esm) or the Dragonborn ash LOD materials by the shape's name and blends
`textures\effects\projecteddiffuse.dds` onto faces along the material's projection vector, weighted
by `-falloff scale * noise + dot(normal, projection) * vertex alpha - falloff bias` (HD shapes:
no alpha factor, plus `(alpha - 0.5) * 2.5`). The parameters are read from the winning MATO
records with `--instance` (vanilla values otherwise). The snow brightness is calibrated on an
in-game capture, since the game's snow specular is not reproduced. "Force alpha test" applies one threshold
to every shape, which shows what alpha-tested foliage and grass billboards look like at the
game's cut-off. Missing textures render as a magenta checker.

**Selection** shows the file and block, the shape's name and block index, its kind, geometry
counts, shader type and flags, alpha and vertex-colour settings, the stored bounding sphere,
the vertex box and the `BSMultiBoundAABB`, and every texture slot with where it resolves from
(output, a mod folder, an archive) or `MISSING`. The orange box is the vertex extent, the blue
one the stored multibound; when they disagree the game culls by the stored one.

**Camera.** The camera line is `x,y,z,yaw,pitch` (yaw clockwise from north, pitch up). The field
of view is horizontal, as the game's `fov` command takes it, so the same number frames the same. "copy
camera" puts it on the clipboard for `--camera`; "go" accepts the same text, so a position from
an in-game console (`player.getpos`) or from skyrim-mcp's free camera can be reproduced.

## Headless: screenshots and checks

```powershell
# A PNG from a camera (defaults 1920x1080, 4x MSAA, game-mode levels, all layers).
lodforge view --out '...' --world Tamriel --screenshot shot.png --camera "-50000,117500,-13300,120,-10" --fov 50

# A top-down map of the whole world at LOD32, coloured by level.
lodforge view --out '...' --world Tamriel --screenshot map.png --levels 32 --shading level --size 2048x2048

# The checks without a GPU; exit code 2 when there are errors.
lodforge view --out '...' --world Tamriel --check --json issues.json
```

`--levels game|4|8|16|32`, `--lod-distance`, `--layers terrain,objects,trees,...`, `--shading`
and `--load-levels` apply to screenshots as in the window. `--lod-center x,y` picks the levels
around another point than the camera (the game picks them around the player).

A screenshot from the same camera as an in-game capture is the quickest way to compare generated
LOD with what the game shows. Capture the game in first person with the player at the spot (the
eye is about 120 units above the player's z) rather than with the free camera: with the free
camera active, distant object LOD can drop in and out between frames. Turn off the idle orbit
camera first (`setini "fAutoVanityModeDelay:Camera" 99999`) and pass the season the game shows
(`--season SUM` for a summer month with Seasons of Skyrim). On an HDR display capture the
presented frame (skyrim-mcp `source: frame`), which converts HDR to sRGB; the engine screenshot
saves the HDR frame as if it were sRGB and looks washed out. Sky, fog and water shading are not
reproduced, so compare structure, placement and textures rather than overall image similarity.

## Checks

| Kind | Severity | What it means |
|---|---|---|
| `parse_error`, `empty_file` | error / info | A `.btr`/`.bto` did not parse, or holds no shapes |
| `block_missing` | warning (terrain), info (objects) | A coarser block is missing although finer blocks under it exist: a hole at distance |
| `terrain_texture_missing`, `terrain_normal_missing` | error / warning | A terrain block's diffuse or normal tile is absent |
| `texture_missing`, `texture_empty` | error / warning | A shape's texture does not resolve in the output or the attached load order (magenta in game) |
| `texture_not_in_output` | info | Without `--instance`: a full texture the output does not carry (the load order normally provides it) |
| `vertex_non_finite`, `index_out_of_range` | error | Broken geometry |
| `degenerate_triangles` | info / warning | Zero-area triangles (warning above 5% of the shape) |
| `bound_sphere`, `multibound_aabb` | warning | Vertices outside the stored bounds: the game may cull the shape while it is in view |
| `uv_outside_atlas` | warning | UVs beyond an atlas page: bleeding from neighbouring tiles |
| `terrain_seam` | warning | Heights along a shared edge of two blocks of the same level differ by more than 2 units per level unit (skirt vertices below the border are ignored) |
| `tree_type_unknown`, `tree_type_uv`, `tree_type_size`, `tree_list` | error | Classic tree LOD references a type the list lacks, or the list is malformed |
| `tree_floating`, `tree_sunken` | warning | Tree billboards start more than 400 units above / 800 below the finest terrain LOD (trees stand on the full terrain, which LOD4 only approximates on slopes) |
| `large_refs` | info | Count of large-reference shapes |

Each issue carries a position where it has one, so the window can fly to it; the JSON report
has the same fields. Tolerances are in `lodforge_viewer::checks::Tolerances`.

## Limits

- Classic tree billboards face the camera and stand on their reference position; the game's
  exact billboard shader is not reproduced (nor the sky, fog, weather or water shaders), so the
  viewer is for geometry, placement, texture and LOD-level inspection rather than final looks.
- Terrain LOD normal maps are read as model-space normals with the up axis in green, as the LOD
  landscape shader does.
- Grass Optimizations meshes (`meshes\lod\grass\*_lod1.nif`) are listed but not placed: their
  positions come from the grass cache at run time.
- Dynamic LOD placed by DynDOLOD.esp at run time is not shown.
- Picking covers terrain and object LOD shapes, not classic tree billboards.
