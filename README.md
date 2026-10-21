# LODForge

Fast, automatable LOD generation for Skyrim Special Edition modlists.
LODForge generates terrain, object and tree LOD, textures and dynamic LOD
plugins while retaining compatible in-game files.
It uses Rust, parallel CPU processing and wgpu GPU baking, with a CLI and a Tauri UI.

**Status: experimental.** End-to-end builds and targeted golden comparisons work;
full Community Shaders / PBR and DynDOLOD runtime parity is not yet certified.
Use a separate test profile and backed-up saves. See the
[compatibility assessment](docs/parity.md) before replacing a working output.

## What it does

- Terrain meshes and diffuse/normal textures; seasonal texture delta builds.
- Object LOD using xLODGen and DynDOLOD LODGen behaviors.
- Tree billboards, stitched/rendered object textures and classic tree LOD.
- DynDOLOD-compatible plugins, DLL NG data, occlusion and terrain undersides.
- Optional BTO grass LOD or Community Shaders Grass Optimizations meshes.
- Optional LOD model generation when installed LOD models are missing.

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
- For DynDOLOD mode / TexGen: the user's DynDOLOD installation, configuration and
  appropriate input resources. LODForge does not bundle these.
- For generated dynamic plugins: the compatible DynDOLOD scripts / DLL NG runtime
  and their prerequisites. Install Community Shaders, Seasons of Skyrim and grass
  runtime features separately when the chosen workflow needs them.
- The UI additionally needs WebView2.

## Required and recommended mods

LODForge reads these from your MO2 profile's virtual file system (or the game's
Data folder) and never bundles them. Install them in the profile you build against.

**Needed for DynDOLOD mode (`build --mode dyndolod`)**

| Mod | Why |
|---|---|
| DynDOLOD (the tool install, `Edit Scripts\DynDOLOD`) | Its INI, rules, configs and billboard templates are read as configuration. |
| DynDOLOD Resources SE | Supplies the LOD models for most references (about 93% of LOD4 refs on a large list), billboards and `_dyndolod_lod` models. Optional if you pass `--lod-models`: LODForge then generates missing models itself, with lower geometry parity. |
| DynDOLOD DLL NG (plus its Papyrus scripts/data) | Runtime for the generated `DynDOLOD.esm/.esp` and `SKSE\Plugins\DynDOLOD_Data`. Its copies of scripts win over Resources' older ones. |
| TexGen output (stitched `textures\lod\*`) | Still taken from the profile; LODForge's own TexGen currently produces billboards only. |
| SKSE64, Address Library for SKSE Plugins | Prerequisites of DLL NG. |
| Cleaned masters (Skyrim, DLC, CC; xEdit-cleaned plugins) | Needed for results close to DynDOLOD. |

**Needed only for the matching workflow**

| Mod | When |
|---|---|
| Community Shaders (with PBR / Grass Optimizations) | CS-targeted output, PBR LOD, or `meshes\lod\grass` Grass Optimizations meshes. |
| Seasons of Skyrim (and its swap data) | `--seasons` builds and `Seasons\...` runtime swaps. |
| NGIO (Grass Cache), Grass Optimizations | Only if you choose runtime grass instead of BTO grass LOD. |
| Any PBR texture mods you use | DynDOLOD-style PBR handling needs the source RMAOS files present. |

Remove other LOD generators' outputs (xLODGen, an earlier DynDOLOD run) from the
profile, or exclude them with `--exclude-mod`, before building; the generated
mod's `DynDOLOD.esm`, `DynDOLOD.esp` and `Occlusion.esp` are dropped from the
input set automatically.

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
  --mode dyndolod --preset Medium --texgen --grass off `
  --worlds Tamriel --out 'D:\LODForge-output\trial-01'
```

- Omit `--mode dyndolod` for the default xLODGen-style workflow.
- Add `--seasons SPR,SUM,AUT,WIN` for extra seasons; default season is always built.
- Use `--grass go` for Grass Optimizations meshes. For a BTO grass parity run,
  use `--grass bto --grass-source cache --grass-density 100` with a matching cache.
  Automatic/generated placements are not NGIO raycast-equivalent.
- Override nonstandard tool paths with `--dyndolod` (installation root) and
  `--dyndolod-config` (the `Edit Scripts\DynDOLOD` directory).
- Use `--lod-models` to generate missing models, not to eliminate all external dependencies.
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

The UI launches the neighboring CLI executable, or the path in `LODFORGE_CLI`.
The frontend is checked-in HTML/CSS/JavaScript; no npm build is required.
The CLI exposes more options than the UI. The UI builds DynDOLOD-style output, renders the tree
billboards and stitched LOD textures itself by default (no TexGen output needs to be installed),
shows the running stage in the build table and ends on a Done screen that can close the program.

`lodforge-ui.exe --instance <MO2 folder> [--profile <name>]` prefills the first step and
scans the load order at start. For an MO2 tool entry, set the arguments to
`--instance "D:/Modlists/skyrim-cs"` (your instance folder); the UI restarts itself outside
MO2's virtual file system and carries the arguments over through
`%LOCALAPPDATA%\LODForge\launch-args.json`.

## Release preparation

Windows x64 CI builds and checks the CLI/UI; the release workflow prepares a
portable ZIP, matching source and SHA-256 checksums as a draft prerelease.
**Public distribution remains blocked pending licensing/provenance clearance.**
No release is published automatically. See [builds and releases](docs/releases.md)
and the [licensing assessment](docs/licensing.md).

LODForge is an independent project, not endorsed by the authors of the
reference tools or required runtimes. Compatibility names identify interfaces
and dependencies, not affiliation.

## Documentation

- [Documentation map](docs/README.md) - choose a short, task-specific starting point.
- [Architecture](docs/architecture.md) - actual crate boundaries and build flow.
- [Development and quality checks](docs/development.md).
- [Validation and safe comparisons](docs/validation.md).
- [Remaining parity gaps](docs/parity.md) and [roadmap](PLAN.md).

## License

LODForge is free software under the **GNU General Public License, version 3
only (GPL-3.0-only)**; see [LICENSE](LICENSE).

**Why GPL-3.0-only.** Parts of the occlusion and object-LOD code are ported
from [AnvilLOD](https://github.com/CageTV/AnvilLOD) (Copyright (c) 2026
CageTV, GPL-3.0-only); the combined work is therefore GPL-3.0-only. Everything
that was once ported from LODGen (MPL 1.1, which cannot be combined with GPL
code) or written after disassembling a closed tool has been replaced by
clean-room re-implementations written from the specifications in
[docs/re](docs/re); the [licensing review](docs/licensing.md) and the
[re-implementation plan](docs/reimplementation-plan.md) record how.

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
to this repository. Public distribution waits on the clearance checklist in the
licensing review.
