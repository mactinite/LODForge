# Ultra phase 3: generated 3D tree LOD models

Status: started 2026-10-23. Phases 1-2 (trees in object LOD, installed 3D LOD models matched by
mesh CRC) are done; see `docs/parity.md`.

## Why

On skyrim-cs-base (Ultra, High) 433 tree bases have 122,802 Tamriel references. Only one base gets
a 3D LOD4 model; 78% of the references show billboards and 22% get nothing (shrubs and thickets
without billboards). Ulvenwald replaces almost every tree mesh, so DynDOLOD Resources' plain-named
models no longer fit (mesh CRC rule) and Ulvenwald ships none of its own. TreeLODGen's size test
picks 305 bases / 46,286 references as worth a 3D model (`lodforge tree-report`,
`LODForge_TreeReport_<World>.csv`).

## Approach

Port TreeLODGen (https://github.com/Cl3mus33/TreeLODGen, GPL-3.0, commit `dc4cc8a`), whose
algorithm is specified in [re/treelodgen-algorithm.md](re/treelodgen-algorithm.md). LODForge is
GPL-3.0-only, so porting is allowed; ported files carry TreeLODGen's copyright and source
reference (GPL §5a) and are listed in `THIRD-PARTY-NOTICES.txt` and `docs/licensing.md`, as the
AnvilLOD files are.

Per tree model and LOD level (4, 8, 16):

1. **Collect** the shapes (static copy preferred over the skinned one), node transforms baked
   except the root's.
2. **Split** bark from foliage: name hints, then "no alpha property = bark", then per piece the
   share of area that samples opaque texels (90%).
3. **Crown:** keep a share of the leaf pieces in Morton order by error diffusion (deterministic),
   enlarge the kept ones (at most 2x) to restore the crown's area, pull them back inside its
   24-band outline; never below a per-level triangle floor.
4. **Bark:** TreeLODGen's own quadric half-edge collapse to a per-level budget (ported literally;
   meshoptimizer gives different results).
5. **Write** a `BSFadeNode` with `... Crown` / `... Trunk` shapes (`noatlas` for tiling bark) as
   `meshes\dyndolod\lod\trees\<stem>_<CRC>passthru_lod[_1|_2].nif`, CRC of the full model (the
   pre-patch one for PGPatcher meshes).

Defaults: keep 0.4 / 0.15 / 0.06, area 0.8 / 0.7 / 0.6, bark 400 / 150 / 60 triangles, crown
floors 2500 / 1000 / 400.

## Where it lives

- `crates/treemodel`: the generator (collect, split, prune, simplify, write), no I/O of its own.
- `lodforge tree-models`: generate into a folder (or `--inspect <model>` to print the split).
- `build --tree-lod ultra --tree-models generate`: generate before the export into the output
  mod, add the files to the LOD model index so the export picks them up like installed ones, and
  list them in `LODForge_TreeModels.txt` so later builds replace them.
- The tree report gains the generated models (source "LODForge").

## Decisions

| Topic | TreeLODGen | LODForge |
|---|---|---|
| Shapes | everything with triangles (switch alternates, hidden, effect shaders included) | same at first, so outputs compare; filtering hidden/switch/effect shapes is a later, documented option |
| Bark simplifier | custom QEM | port literally, ties broken by (from, to) |
| PBR / parallax / env shaders | stripped to diffuse + normal (copied PBR made distant trees glow) | follow TreeLODGen for now; revisit with Community Shaders' LOD shader once models work in game |
| Trees with installed 3D LOD | skipped unless `--all` | same (`--tree-models all` regenerates) |
| Partial output on errors | possible | all-or-nothing per tree |
| LOD8/16 3D | rule choice, heavy | off by default (DynDOLOD's High tree rule: 3D at LOD4 only) |

## Milestones

1. **Tree report** - done: `lodforge tree-report`, written by every DynDOLOD-mode build.
2. **Read and split** - `crates/treemodel` collects shapes, classifies bark/foliage and finds
   leaf pieces; `tree-models --inspect` prints it for real trees (vanilla pine, Ulvenwald pine,
   `srg_oak`). Unit tests on synthetic meshes.
3. **Prune and simplify** - done 2026-10-23 (`prune.rs`, `simplify.rs`, `levels.rs`). Bark lands
   on budget (398 / 148 / 61 of 400 / 150 / 60); Ulvenwald and vanilla spruces (12.5k leaf
   triangles) keep 5.1k / 2.0k / 0.8-0.9k crown triangles, `srg_oak5` (4.5k) holds the floors
   (2,515 / 998 / 399); 15-32 ms per tree for all three levels. The spruces are about three times
   TreeLODGen's "typical pine" (1,740 crown triangles at LOD4), so object LOD size needs watching
   in milestone 5; a lighter preset (TreeLODGen's Light: keep 0.3 / 0.1 / 0.04) is the first knob.
4. **Write and integrate** - NIF writer clone semantics (shader, texture set, alpha, vertex
   format), file naming, build integration, tree report source.
5. **Verify** - `lodforge view` on generated models; in game: the Whiterun oak and pine spots at
   the switch distance (frames on both sides of a cell boundary, as for billboard density),
   object LOD sizes, load time and frame rate. Optional: compare with TreeLODGen's own output on
   the same load order (needs running TreeLODGen.exe; ask first).
6. **Polish** - snow/seasonal models (seasonal swaps use other models, so they come for free
   once those models are generated), CS PBR decision, per-tree choices (`lod4` / `all` /
   `billboard`) and the DynDOLOD-style rule file, triangle budget warnings.

## Risks

- Object LOD size: TreeLODGen saw Tamriel's object LOD grow from about 5 GB to over 8 GB with 3D at
  LOD 8/16 and the game stop loading. LOD4 only by default; report sizes per build.
- Shape-local spaces: welding, Morton box, outline and simplifier welding all run before the
  transform bake; getting this wrong scales crowns wrongly (Ulvenwald scales on nodes).
- NIF clone fidelity: the writer must keep the source vertex format and shader blocks exactly.
