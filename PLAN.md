# Roadmap

Goal: faster Skyrim SE LOD generation for modlists, with DynDOLOD-compatible
in-game output on Community Shaders / PBR load orders.

**Current status:** experimental end-to-end generator, not certified full parity.
See [remaining gaps and evidence](docs/parity.md), not the historical phase labels,
for the compatibility boundary. CLI default mode is still `xlodgen`; select
`--mode dyndolod` explicitly for this target.

## Work queue

| Order | Outcome | Completion evidence |
|---|---|---|
| Measured baseline | Fresh full Tamriel BTO-grass build and matched runtime checks | [Full qualification](docs/validation-full-20261019.md): zero checked missing assets/structural errors, exact classic tree placements, quantified geometry/runtime gaps. Not full parity. |
| 0 | Resolve flicker and qualify rebuilt saves | Reproduce distant glacier/terrain flicker with consecutive frames. Load pre-rebuild saves against rebuilt output and test runtime state/lifecycle; changing mesh/atlas bytes alone is not a blocker. |
| 1 | Finish DLL data and functional grass checks | Reset selection now 57/57; 33 tree Delay mismatches fixed. Finish runtime omissions/metadata and verify grass formats, coverage, density and near/far transitions. Exact golden grass triangles are diagnostic, not a release gate. [Evidence](docs/validation-runtime-batch-20261019.md). |
| 2 | Mip/material fidelity | Matched-tile alpha tests, tree packing, directional materials and full shader overrides; classify remaining HD differences consistently. |
| 3 | Reproducible settings and optional CS paths | Effective-settings contract; world/season policies; complex/Ultra trees and PBR slot lookup with control fixtures. |
| 4 | Certify the supported workflow | Pinned golden runs plus in-game matrix across worldspaces, seasons, grass modes and rebuilds. Publish exceptions rather than declaring blanket parity. |

The [gap-closure plan](docs/gap-closure-plan.md) gives implementation order,
dependencies and exit criteria. [Parity](docs/parity.md) defines the support
boundary; [validation](docs/validation.md) defines the evidence to collect.

## Later, not implemented infrastructure

- Shared orchestration library: the CLI owns the pipeline today; `core` is a placeholder.
- Persistent general-purpose job graph / content-addressed chunk cache.
- Saved LODForge presets, external modlist-runner integration, broader UI controls.
- Remove the DynDOLOD installation/configuration dependency. Generating missing LOD
  models does not replace its rules, templates or runtime requirements.

## Updating this plan

Keep this file to priorities and exit criteria. Put implementation details beside
the code or in the relevant focused document; put measured results in
[baselines](docs/baseline.md). When closing a gap, update
[parity](docs/parity.md) with the commit, profile, settings and test evidence.

The [old phase-by-phase plan](docs/history/roadmap-20261018.md) is retained only
for historical context.
