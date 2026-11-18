# Documentation map

This folder documents the code that is here and how to use it. Plans, dated measurements and
research notes are kept outside the repository.

| Task | Start here |
|---|---|
| Build or use LODForge | [Project README](../README.md) |
| Look up a build option, a config file or a report | [Build options](build-options.md) |
| Avoid a known trap | [Gotchas](gotchas.md) |
| See what is unsupported or uncertified | [Known limitations](limitations.md) |
| Look at generated LOD without the game | [LOD viewer](viewer.md) |
| Compare output or test in game | [Validation](validation.md) |
| Find the code that owns a feature | [Architecture](architecture.md) |
| Run tests, formatting, linting or API documentation | [Development](development.md) |
| Build release archives or prepare a draft | [Builds and releases](releases.md) |
| Check license and provenance | [Licensing and provenance](licensing.md) |
| Drive reference-tool runs | [Golden automation](../scripts/golden/dyndolod/README.md) |

## Reference

Specifications the code was written against. Code comments cite them by path and section.

| Subject | Reference |
|---|---|
| Terrain LOD meshes (`.btr`) | [Terrain LOD specification](re/terrain-lod-spec.md) |
| Object LOD (`.bto`), exports, atlas | [Object LOD specification](re/object-lod-spec.md) |
| DynDOLOD rules, exports, plugins and DLL data | [DynDOLOD behavioral specification](re/dyndolod-spec.md) |
| Occlusion (`Occlusion.esp`, TVDT) | [Occlusion specification](re/occlusion-spec.md) |
| Terrain texture algorithm | [xLODGen terrain textures](re/xlodgen-terrain-textures.md) |
| NGIO grass cache format | [`.cgid` format](re/grass-cache-spec.md) |
| Generated LOD models (`--lod-models`) | [Generated LOD models](re/own-lod-resources.md) |
| Generated tree models (`--tree-models`) | [Tree models](tree-models.md), [TreeLODGen algorithm](re/treelodgen-algorithm.md) |
| LODForge Runtime files | [Instance formats](runtime/instance-formats.md), [impostors](runtime/impostors.md) |

The terrain, object LOD and occlusion specifications end with process records that are part of
the [provenance record](licensing.md). Those records name documents by the path they had at the
time (`docs/baseline.md`, `docs/parity.md`, the gap audits, the dated validation runs and
`docs/reimplementation-plan.md`); these now live with the development notes.

## Keeping it current

- **README:** entry points, requirements and warnings.
- **Architecture:** what exists, not a proposed design.
- **Build options:** every option, input file, event and report.
- **Gotchas / limitations:** short, current, without dated run logs.
- **Rustdoc:** public APIs and format or algorithm contracts near the code.

When behavior changes, update its page in the same change. Plans, status and measurements do
not belong here.
