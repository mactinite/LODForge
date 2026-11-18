# LODForge

Fast, automatable LOD generation for Skyrim Special Edition modlists.
LODForge generates terrain, object and tree LOD, textures and dynamic LOD
plugins while retaining compatible in-game files.
It uses Rust, parallel CPU processing and wgpu GPU baking, with a CLI and a Tauri UI.

**Status: experimental.** Releases are prereleases. Build into a separate test profile and keep
backed-up saves. Read the [known limitations](docs/limitations.md) and the
[gotchas](docs/gotchas.md) before replacing a working LOD setup.

## What it does

- Terrain meshes and diffuse/normal textures; seasonal texture delta builds.
- Object LOD by DynDOLOD's rules, with the object texture atlas.
- Tree billboards, stitched and rendered LOD textures (its own TexGen stage), classic tree LOD or
  Ultra tree LOD, and optionally [generated 3D tree LOD models](docs/tree-models.md).
- DynDOLOD-compatible plugins, DLL NG data, occlusion and terrain undersides.
- Optional BTO grass LOD or Community Shaders Grass Optimizations meshes.
- Optional [generated LOD models](docs/re/own-lod-resources.md) where installed ones are missing.
- Optional data for the LODForge Runtime plugin (`--runtime lodforge`): instanced trees with
  impostors, instanced statics, far grass and dynamic LOD.
- A [viewer](docs/viewer.md) that shows and checks the generated LOD without the game.

PBR-compatible output follows DynDOLOD's treatment of PBR inputs. It does **not**
mean every distant mesh uses a True PBR shader. Lists without Community Shaders
PBR use `--materials standard` for plain output (see below). Grass Optimizations meshes and
DynDOLOD BTO grass are different output modes, not interchangeable parity tests.

## Requirements

- Windows, a Skyrim SE installation and a portable-layout Mod Organizer 2 instance
  (`mods`, `profiles`, `overwrite`). Game defaults to `Stock Game` inside the instance.
- For source builds: the pinned Rust MSVC toolchain (via rustup), Visual Studio
  C++ build tools and Windows SDK. The advertised workspace 1.85
  minimum is not a verified support promise. See [development](docs/development.md).
- A suitable wgpu GPU/driver for texture generation (Vulkan or D3D12 on Windows).
  There is no complete CPU-only replacement for all build stages.
- Portable Windows builds require the Microsoft Visual C++ v14 x64 Redistributable
  (the CLI imports `VCRUNTIME140.dll` and `VCRUNTIME140_1.dll`).
- The user's DynDOLOD installation, configuration and
  appropriate input resources. LODForge does not bundle these.
- For generated dynamic plugins: the compatible DynDOLOD scripts / DLL NG runtime
  and their prerequisites. Install Community Shaders, Seasons of Skyrim and grass
  runtime features separately when the chosen workflow needs them.
- The UI additionally needs WebView2.

## Required and recommended mods

LODForge reads these from your MO2 profile's virtual file system (or the game's
Data folder) and never bundles them. Install them in the profile you build against.

**Needed for a build**

| Mod | Why |
|---|---|
| DynDOLOD (the tool install, `Edit Scripts\DynDOLOD`) | Its INI, rules, configs and billboard templates are read as configuration. |
| DynDOLOD Resources SE | Supplies the LOD models for most references (about 93% of LOD4 refs on a large list), billboards and `_dyndolod_lod` models. Optional if you pass `--lod-models`: LODForge then generates missing models itself, with lower geometry parity. |
| DynDOLOD DLL NG (plus its Papyrus scripts/data) | Runtime for the generated `DynDOLOD.esm/.esp` and `SKSE\Plugins\DynDOLOD_Data`. Its copies of scripts win over Resources' older ones. |
| TexGen output | Only without `--texgen`. With `--texgen` LODForge renders the billboards and the stitched and rendered LOD textures itself. |
| SKSE64, Address Library for SKSE Plugins | Prerequisites of DLL NG. |
| Cleaned masters (Skyrim, DLC, CC; xEdit-cleaned plugins) | Needed for results close to DynDOLOD. |

**Needed only for the matching workflow**

| Mod | When |
|---|---|
| Community Shaders (with PBR / Grass Optimizations) | CS-targeted output, PBR LOD, or `meshes\lod\grass` Grass Optimizations meshes. |
| Seasons of Skyrim (and its swap data) | `--seasons` builds and `Seasons\...` runtime swaps. |
| No Grass In Objects NG (grass cache) | `--grass bto` and the LODForge Runtime's far grass read its cache; LODForge does not generate grass placements. |
| LODForge Runtime (`LODForgeRuntime.dll`) | `--runtime lodforge` builds. |
| Any PBR texture mods you use | DynDOLOD-style PBR handling needs the source RMAOS files present. |

Remove other LOD generators' outputs (xLODGen, an earlier DynDOLOD run) from the
profile, or exclude them with `--exclude-mod`, before building; the generated
mod's `DynDOLOD.esm`, `DynDOLOD.esp` and `Occlusion.esp` are dropped from the
input set automatically.

Not needed: the "xLODGen Resource - SSE Terrain Tamriel" plugins. LODForge fills
Tamriel's outer cells, whose LAND records Skyrim.esm lacks, from the terrain LOD
meshes the game ships (`docs/re/terrain-lod-spec.md` 2.3; `--no-lod-fill` turns
it off). Keep those plugins disabled:
`Occlusion.esp` copies the winning worldspace record, and the Extend plugin's
enlarged object bounds would stay in it after the plugin is removed.

## Build and run

From the repository root in PowerShell:

```powershell
cargo build --locked --release -p lodforge
.\target\release\lodforge.exe info --instance 'D:\Modlists\skyrim-cs' --profile 'LODForge-E2E' --json
.\target\release\lodforge.exe build --help
```

The instance and profile above are examples; substitute your own. Use `--game`
when the game is not in the instance's `Stock Game` folder.

Start with one worldspace and a new output folder:

```powershell
.\target\release\lodforge.exe build `
  --instance 'D:\Modlists\skyrim-cs' --profile 'LODForge-E2E' `
  --preset Medium --texgen --grass off `
  --worlds Tamriel --out 'D:\LODForge-output\trial-01'
```

- Add `--seasons SPR,SUM,AUT,WIN` for extra seasons; default season is always built.
- Use `--grass go` for Grass Optimizations meshes, or `--grass bto` for grass in object LOD
  (needs an NGIO grass cache; `--grass-density`, default 50).
- `--tree-lod ultra` puts every tree in object LOD; `--tree-models generate` adds generated 3D
  tree LOD models.
- `--runtime lodforge` also writes `SKSE\Plugins\LODForgeRuntime\*` for the LODForge Runtime
  plugin.
- Override nonstandard tool paths with `--dyndolod` (installation root) and
  `--dyndolod-config` (the `Edit Scripts\DynDOLOD` directory).
- Use `--lod-models` to generate missing LOD models. It does not remove the other DynDOLOD
  dependencies.
- [Build options](docs/build-options.md) lists every option, the configuration files a build
  reads and the reports it writes.
- `--materials pbr|standard` (default `pbr`) picks the list's material setup. `pbr`
  (Community Shaders with PBR) makes `pbr_lod` texture twins, PBR texture sets and
  `linear.dds` copies wherever a `textures\pbr\` source exists. `standard` (non-PBR:
  ENB complex material, parallax or vanilla shading) writes plain textures only and
  ignores `textures\pbr\` sources. LOD carries no complex-material or parallax maps
  (DynDOLOD documents no such object-LOD handling), so `standard` simply means "not
  PBR"; `complex` is accepted as an alias. Complex grass is detected per shader in
  both modes.

Output is a **Data-layout folder**, suitable for installing as an MO2 mod.
Alternatively, replace `--out` with `--mo2-mod 'LODForge Trial'`.
Enable the mod and its required plugins deliberately after validation; `--enable`
only edits the mod list and requires MO2 to be closed.
Never enable competing DynDOLOD/LODForge output sets together for a comparison.

Builds exclude recognized earlier generated LOD mods and generated plugins from
input; this is not a universal detector for every old TexGen/LOD output.
Use `--exclude-mod` for other output mods. Keep rebuild output together to preserve
rolling-update IDs; `--renumber` breaks that identity. Output updates are not
transactional, so do not install a failed or interrupted build.

## UI

```powershell
cargo build --locked --release -p lodforge -p lodforge-ui --features lodforge-ui/custom-protocol
.\target\release\lodforge-ui.exe
```

The UI launches the neighboring CLI executable, or the path in `LODFORGE_CLI`. It builds
DynDOLOD-style output with its own TexGen stage by default, in a guided view or an advanced view
with every exposed option; it can edit the DynDOLOD configuration files a build reads, shows the
running stage, and ends on a Done screen with the build's reports that can open the viewer. The
CLI has more options than the UI.

The frontend source is in `apps/ui/web` (React, Vite). Its bundle is checked in as
`apps/ui/dist`, so building the UI needs no npm; changing the frontend does
(see [development](docs/development.md)).

`lodforge-ui.exe --instance <MO2 folder> [--profile <name>]` prefills the first step and
scans the load order at start. For an MO2 tool entry, set the arguments to
`--instance "D:/Modlists/skyrim-cs"` (your instance folder); the UI restarts itself outside
MO2's virtual file system and carries the arguments over through
`%LOCALAPPDATA%\LODForge\launch-args.json`.

## Inspecting the output

```powershell
.\target\release\lodforge.exe view --out <output folder> --world Tamriel [--instance <MO2 folder> --profile <name>]
```

The viewer shows a worldspace's generated LOD as the game arranges it (LOD4 near the camera
through LOD32 at distance, or one level everywhere), with a free camera, a top-down map, a
shape inspector and checks for missing blocks and textures, broken geometry, culling bounds,
terrain seams and floating trees. The UI's Done screen opens it on the build just made. It also
works headlessly: `--screenshot shot.png --camera x,y,z,yaw,pitch` renders a PNG and `--check
--json report.json` writes the issue list. See [the viewer page](docs/viewer.md).

## Releases

Windows x64 CI builds and checks the CLI and UI. The release workflow prepares a portable ZIP,
the matching source and SHA-256 checksums as a draft prerelease; a maintainer publishes it by
hand. See [builds and releases](docs/releases.md).

LODForge is an independent project, not endorsed by the authors of the
reference tools or required runtimes. Compatibility names identify interfaces
and dependencies, not affiliation.

## Documentation

- [Documentation map](docs/README.md): one starting point per task.
- [Build options](docs/build-options.md), [gotchas](docs/gotchas.md) and
  [known limitations](docs/limitations.md).
- [Architecture](docs/architecture.md): crates and build flow.
- [Development and quality checks](docs/development.md); [validation](docs/validation.md).

## License

LODForge is free software under the **GNU General Public License, version 3
only (GPL-3.0-only)**; see [LICENSE](LICENSE).

**Why GPL-3.0-only.** Parts of the occlusion and object-LOD code are ported
from [AnvilLOD](https://github.com/CageTV/AnvilLOD) (Copyright (c) 2026
CageTV, GPL-3.0-only); the combined work is therefore GPL-3.0-only. Everything
that was once ported from LODGen (MPL 1.1, which cannot be combined with GPL
code) or written after disassembling a closed tool has been replaced by
clean-room re-implementations written from the specifications in
[docs/re](docs/re); the [licensing review](docs/licensing.md) records how.

**Third-party components** are listed in
[THIRD-PARTY-NOTICES.txt](THIRD-PARTY-NOTICES.txt) with their license texts
under [licenses/](licenses/): AnvilLOD (GPL-3.0-only), three files ported from
xEdit's `wbLOD.pas` (MPL-2.0, distributed inside this GPL work under MPL 2.0
section 3.3), the s-hull.org Delaunay triangulator (beerware), Terra 0.7
(public domain), and the Rust dependencies (MIT, Apache-2.0 and similar).

**Not covered.** LODForge contains no code or assets from DynDOLOD, TexGen,
xLODGen or Skyrim; it reads the LOD meshes, rule files and billboards installed
in your own setup at run time. The files LODForge generates can contain other
mods' assets and must not be redistributed without those authors' permission.

Do not commit game/mod assets, decompilations, local output or private profiles
to this repository.
