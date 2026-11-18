# Generated 3D tree LOD models (`--tree-models`)

With `--tree-lod ultra`, trees go into object LOD and a tree base can show a 3D LOD model instead
of a billboard. DynDOLOD matches installed 3D tree LOD models by the CRC of the full model, so a
mod that replaces tree meshes loses them. `--tree-models generate` makes the missing models.

The generator is a port of [TreeLODGen](https://github.com/Cl3mus33/TreeLODGen) (GPL-3.0, commit
`dc4cc8a`); its algorithm is specified in [re/treelodgen-algorithm.md](re/treelodgen-algorithm.md).
Ported files carry TreeLODGen's copyright and source reference and are listed in
`THIRD-PARTY-NOTICES.txt` and [licensing.md](licensing.md).

## What it does

Per tree model and LOD level (4, 8, 16):

1. **Collect** the shapes (the static copy in preference to the skinned one) with node transforms
   baked, except the root's.
2. **Split** bark from foliage: name hints, then "no alpha property = bark", then per piece the
   share of its area that samples opaque texels (90%).
3. **Crown,** by `--tree-model-crown`:
   - `cloud` (default; LODForge's own method, `crates/treemodel/src/cloud.rs`): the leaves are
     projected onto a few textured planes per orientation (a billboard cloud, after Décoret et
     al. 2003) over the reduced 3D trunk.
   - `prune` (TreeLODGen's): keep a share of the leaf pieces in Morton order by error diffusion
     (deterministic), enlarge the kept ones (at most 2x) to restore the crown's area, and pull
     them back inside its 24-band outline. A per-level triangle floor is never undercut.
4. **Bark:** TreeLODGen's own quadric half-edge collapse to a per-level budget, ported literally
   (meshoptimizer gives different results).
5. **Write** a `BSFadeNode` with `... Crown` / `... Trunk` shapes (`noatlas` for tiling bark) as
   `meshes\dyndolod\lod\trees\<stem>_<CRC>passthru_lod[_1|_2].nif`. The CRC is the full model's
   (the pre-patch one for PGPatcher meshes).

Defaults: keep 0.4 / 0.15 / 0.06, area 0.8 / 0.7 / 0.6, bark 400 / 150 / 60 triangles, crown
floors 2500 / 1000 / 400. `--tree-model-levels` overrides them per level.

## Where it lives

- `crates/treemodel`: the generator (collect, split, prune, simplify, write). It does no I/O.
- `lodforge tree-models`: generate into a folder, or `--inspect <model>` to print the split.
- `build --tree-lod ultra --tree-models generate`: generates before the export into the output,
  adds the files to the LOD model index so the export picks them up like installed ones, and
  lists them in `LODForge_TreeModels.txt` so a later build replaces them.
- `lodforge tree-report` and `LODForge_TreeReport_<World>.csv` name generated models with the
  source "LODForge".

## Differences from TreeLODGen

| Topic | TreeLODGen | LODForge |
|---|---|---|
| Shapes | everything with triangles (switch alternates, hidden, effect shaders included) | same |
| Crown | pruned leaf cards | billboard cloud by default; `prune` gives TreeLODGen's |
| Bark simplifier | custom QEM | literal port, ties broken by (from, to) |
| PBR / parallax / environment shaders | stripped to diffuse + normal (copied PBR made distant trees glow) | same |
| Trees with installed 3D LOD | skipped unless `--all` | same (`--tree-models all` regenerates) |
| Partial output on errors | possible | all or nothing per tree |
| 3D models at LOD8/16 | by rule, heavy | off by default (DynDOLOD's High tree rule: 3D at LOD4 only) |
