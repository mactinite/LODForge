# Documentation map

Read one focused page first. The large evidence logs are references, not onboarding.

| Task | Start here |
|---|---|
| Build or use LODForge | [Project README](../README.md) |
| Find the code that owns a feature | [Architecture](architecture.md) |
| Run tests, formatting, linting or API documentation | [Development](development.md) |
| Build release archives or prepare a draft | [Builds and releases](releases.md) |
| Review license selection and distribution blockers | [Licensing and provenance](licensing.md) |
| Replace provenance-blocked code | [Re-implementation plan](reimplementation-plan.md) |
| Compare output or test in game | [Validation](validation.md) |
| Look at generated LOD without the game | [LOD viewer](viewer.md) |
| Decide what still blocks compatibility | [Parity assessment](parity.md) |
| Read the latest measured result | [Full-output qualification](validation-full-20261019.md) |
| Implement the remaining fixes | [Gap-closure plan](gap-closure-plan.md) |
| Choose the next task | [Roadmap](../PLAN.md) |

## Evidence library (read on demand)

| Subject | Reference |
|---|---|
| Measurements, profiles, tolerances and historical runs | [Baseline log](baseline.md) |
| DynDOLOD rules, exports, plugins and DLL formats | [Behavioral specification](re/dyndolod-spec.md) |
| LODGen 3.0.36 vs 3.1.8 | [Port delta](re/lodgen-318-delta.md) |
| Terrain texture algorithm | [xLODGen texture specification](re/xlodgen-terrain-textures.md) |
| Grass placements / cache format | [Grass cache specification](re/grass-cache-spec.md) |
| Generating missing object models | [Own LOD resources](re/own-lod-resources.md) |
| Original broad gap inventory | [2026-10-19 audit](re/gap-audit-20261019.md) |
| Per-key settings inventory | [INI-key audit](re/gap-audit-20261019-ini-keys.md) |
| How the project reached this point | [Archived phase plan](history/roadmap-20261018.md) |
| Driving local reference-tool runs | [Golden automation](../scripts/golden/dyndolod/README.md) |

## Documentation ownership

- **README:** supported entry points, requirements and warnings; no benchmark diary.
- **Architecture:** what exists, not a proposed design.
- **Parity:** current gaps, evidence strength and completion tests.
- **Plan:** priorities only. Do not repeat the entire gap inventory.
- **Baselines / RE:** dated evidence. Old numbers stay attributed to their run.
- **Rustdoc:** public APIs and non-obvious format/algorithm contracts near the code.

When behavior changes, update its focused page and link the measurement or test.
Do not require developers to read the full baseline/specification to find current
status. Historical "open" or "done" labels do not override the current assessment.
