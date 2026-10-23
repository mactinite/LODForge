# Licensing and provenance review

**Status (2026-10-20): the code is clean; publication waits on the maintainer
steps at the end of this document.** LODForge is licensed **GPL-3.0-only**
(decision 2026-10-19). Every module that had been ported from LODGen (MPL 1.1,
GPL-incompatible) or written after disassembling a closed tool has been
replaced on `main` by code written in fresh contexts from scrubbed
specifications, partly ported from AnvilLOD (GPL-3.0-only):

| Replacement | Merged into main | Record |
|---|---|---|
| Billboard shader (TexGen GLSL transcription) | `f79eb6e` | docs/baseline.md "Billboard shader re-derivation" |
| Grass generator removed (NGIO GPL + engine disassembly; product decision) | `5a9dc95` | docs/re/grass-cache-spec.md header |
| Terrain LOD mesher, BTR assembly, NIF serializer (stage B1) | `b96c5b9` | [terrain-lod-spec.md](re/terrain-lod-spec.md) sections 5.5, 6, 8 |
| Occlusion / TVDT (Target 1) | `7cac5ed`, record `3036063` | [occlusion-spec.md](re/occlusion-spec.md) sections 11, 12 |
| Object LOD builder and the LODGen-mirroring CLI pieces (stage B2) | `c5b4f4e` | [object-lod-spec.md](re/object-lod-spec.md) sections 11, 12 |

`LICENSE` (GPL-3.0 text), `THIRD-PARTY-NOTICES.txt`, `licenses/` and the Cargo
license metadata were added on 2026-10-20 (branch `license-mechanics`).

**Maintainer decisions (2026-10-19):**

1. No partnership with, and no inquiries to, the authors of the reference tools
   (xLODGen/LODGen, DynDOLOD, AnvilLOD). The release stands on evidence we hold;
   the authors can decide for themselves whether to use LODForge once it is open.
2. **GPL-3.0-only.** The LODGen-derived crates were rewritten from scrubbed
   specifications, and deriving from AnvilLOD (GPL-3.0-only) is allowed.

This is an engineering provenance assessment, not legal advice or a completed
file-by-file audit. Obtain qualified review where necessary before distributing
source **or** binaries.

## Key finding: LODGen is licensed, in writing, by its distributor — under MPL 1.1

DynDOLOD 3.0 Alpha-215 ships `docs\DynDOLOD-LICENSE.txt` (local copy
`D:\Modlists\skyrim-cs\tools\DynDOLOD\docs\DynDOLOD-LICENSE.txt`, SHA-256
`24f381e2d7b4777597b4a99d89ae4825fb04dff45f3bf46ae3d3abedd95e4825`, 152 lines,
file dated 2026-07-19). Verbatim:

> LODGen.exe by Ehamloptiran and Zilav
> Updated by Sheson
>
> The contents of this file are subject to the Mozilla Public License
> Version 1.1 (the 'License'); you may not use this file except in
> compliance with the License. You may obtain a copy of the License at
> http://www.mozilla.org/MPL/

The same file states that `DynDOLOD.exe and TexGen.exe are based on xEdit`
under MPL 1.1, that `Assets [are] Licensed under creative commons : CC BY-NC-SA`,
and lists LODGen's bundled components with their own terms: DelaunayTriangulator
(s-hull.org 2011, "contributors beerware license"), SharpZipLib (GPL-2.0-or-later
with the linking exception), LZ4 (BSD-2), Texconv (MIT).

Binaries the statement ships with (SHA-256, same install):

| File | SHA-256 |
|---|---|
| `DynDOLOD\DynDOLODx64.exe` (3.0 Alpha-215, per its log) | `8e4fe9169131512af15ce48ffcc63b9bb05d070e91946d33d2c36fab8683e32c` |
| `DynDOLOD\TexGenx64.exe` | `c3a45354befd8f98f2c2b97d06948350e2443193c3bcbd6d713332db57c73c50` |
| `DynDOLOD\Edit Scripts\LODGenx64.exe` (3.1.8, net48) | `2a49babce8304ecc2ba75bdddee8a015888abdd0f965c3bd9b53d493df798c6a` |
| `DynDOLOD\Edit Scripts\LODGenx64Win10.exe` (3.1.8, .NET 8) | `6c937789ceff81ecf88abbb415eafaa9b53dca0d234ddeb82c2742a08474d1c1` |
| `DynDOLOD\Edit Scripts\LODGenx64Win6.exe` | `508faaceeb3131708183df06c8107ff31a8c45c5a6fcfd469eb49910f3e23312` |

The xLODGen archive (`D:\Modlists\skyrim-cs\tools\xLODGen`, `xLODGenx64.exe`
`738b2ac4…`, `Edit Scripts\LODGenx64.exe` 3.0.36
`d644e90aee638bfa870a34487fb6b1606a4547e9b65984aeb0628d16e89cac62`) ships **no
license text** (checked 2026-10-19). The dyndolod.info site has no license page
(`/Help/License` and `/Help/Licenses` return 404).

## Why MPL 1.1 forced the rewrite under GPL

Quoted from the license texts (fetched 2026-10-19):

- MPL 1.1 §6.2: "You may also choose to use such Covered Code under the terms of
  any subsequent version of the License" — so LODGen code may be used under MPL
  2.0.
- MPL 2.0 §1.5: "Incompatible With Secondary Licenses" means … "that the Covered
  Software was made available under the terms of version 1.1 or earlier of the
  License, but not also under the terms of a Secondary License." LODGen was made
  available under 1.1 only; §3.3's route into GPL works does **not** apply to it.
- FSF license list on MPL 1.1: "a module covered by the GPL and a module covered
  by the MPL cannot legally be linked together."

So a GPL-3.0-only LODForge cannot contain any code derived from LODGen. The
xEdit-derived ports are different: TES5Edit's `wbLOD.pas` is plain **MPL-2.0**
(no Exhibit B), which MPL 2.0 §3.3 lets us distribute inside a GPL Larger Work
with the covered files staying MPL-2.0/GPL dual. They stay.

## Decision record

| | Path A — MPL-2.0 project | **Path B — GPL-3.0-only project (chosen 2026-10-19, executed 2026-10-20)** |
|---|---|---|
| LODGen ports (objlod, btr, nif writer/BTR assembly, CLI pieces; ~10,000 lines) | Keep, licensed | **Removed and rewritten from scrubbed specs** |
| Occlusion (`occlusion.rs`, DynDOLOD decompile) | Re-implement | Re-implemented; **derived in part from AnvilLOD** |
| Billboard shader (`billboard.wgsl`) | Re-derive | Re-derived from a functional spec |
| Grass generator (NGIO GPL-3 + engine disassembly) | Remove | License-compatible; removed anyway (product decision) |
| AnvilLOD code | Not usable | **Used**, GPL-3.0-only obligations (attribution per file) |
| Who can reuse LODForge | Anyone at file level incl. closed tools | GPL-compatible projects only |

Consequences accepted: parity with the reference tools is tolerance-level (the
rewrites were validated against golden output and in game, not against bits);
the published repository starts from a clean commit (see "Repository history
and publication").

## Evidence and obligations

Reviewed 2026-10-19, statuses updated 2026-10-20.

| Origin / affected code | Evidence | Status |
|---|---|---|
| xEdit (TES5Edit) MPL-2.0 ports: [treelod](../crates/treelod/src/lib.rs), [atlas_build.rs](../crates/objlod/src/atlas_build.rs), [source.rs](../crates/objlod/src/source.rs) | TES5Edit commit `9fb016884bec138ea6c7b872cec831537d464c3e`; [wbLOD.pas header](https://github.com/TES5Edit/TES5Edit/blob/9fb016884bec138ea6c7b872cec831537d464c3e/Core/wbLOD.pas) says MPL-2.0, plain. | **Kept.** MPL 2.0 §3.3; the files carry MPL-2.0 headers, `crates/treelod` declares `license = "MPL-2.0"`, THIRD-PARTY-NOTICES part 1.2. These writers also define the export and atlas-map formats the rewritten object LOD reads. |
| LODGen 3.1.8 / 3.0.36 ports (former objlod, btr, nif writer/BTR assembly, CLI atlas and grass-billboard pieces) | Shipped `DynDOLOD-LICENSE.txt`: MPL 1.1. | **Replaced** (B1 `b96c5b9`, B2 `c5b4f4e`). Kept from the old crate because their origin is not LODGen: [delaunay.rs](../crates/objlod/src/delaunay.rs) (s-hull.org C# release, beerware; notice kept). The .NET-numerics shim and its fixtures were deleted. Terra 0.7 (public domain) is the basis of the new mesher; Forsyth's published algorithm the basis of `vcache.rs`. |
| DynDOLOD occlusion internals (former `occlusion.rs`, Ghidra on `DynDOLODx64.exe`) | The only `[decomp]` area of the DynDOLOD spec. | **Replaced** (`7cac5ed`): clean-room module from [occlusion-spec.md](re/occlusion-spec.md) and AnvilLOD's `Occlusion/*.cs`; harness PASS, block Jaccard 0.875–0.883, in-game sweep without LOD holes (spec 12.3). |
| TexGen shaders (former `billboard.wgsl`) | Shipped GLSL, no OSI-compatible grant. | **Replaced** (`f79eb6e`) from a functional spec; the old file was never opened by the implementer. [stitch.rs](../crates/texgen/src/stitch.rs) and the un-premultiply/bleed code were assessed as independent and stay. |
| Shape ordering in DynDOLOD side files ([sidefiles.rs](../crates/dyndolod/src/sidefiles.rs)) | DynDOLOD's AltTextures/VC side files index shapes in a model traversal order. | **Interoperability; kept.** Re-verified from golden side files and NIF trees ([object-lod-spec.md](re/object-lod-spec.md) 7.3); comments reworded to the observed format fact. |
| NGIO NG grass generation (former `crates/grass` generator) | GPL-3 text plus EXCEPTIONS.md. | **Removed** (`5a9dc95`); the `.cgid` reader ([cgid.rs](../crates/grass/src/cgid.rs)) is format interoperability. The generator remains on branch `g1-grass-cache`, GPL-compatible should it ever return. |
| xLODGen terrain-texture bake ([spec](re/xlodgen-terrain-textures.md), [terrain crate](../crates/terrain)) | Behavioural spec from a Ghidra pass on `xLODGenx64.exe` 4.1.5o (a build of MPL xEdit with sheson's unpublished terrain beta); no decompiled code; crate validated against golden textures. | **Kept.** Behavioural re-expression of MPL-covered software. The spec's Ghidra addresses, evidence table and thread-structure section were moved out of the repository on 2026-10-20; the behaviour description remains. |
| DynDOLOD DLL data formats ([dynplugins.rs](../crates/dyndolod/src/dynplugins.rs), `[pdb]`/`[dll-string]`) | The DLL consumes the data LODForge writes. | **Interoperability; kept.** |
| Game engine grass research ([grass-cache-spec.md](re/grass-cache-spec.md)) | Ghidra on SkyrimSE.exe for grass placement; also informed the `.cgid` format. | Format sections kept; the engine/NGIO placement sections (addresses, pseudo-code) were moved out of the repository on 2026-10-20 together with the generator they described. |
| [AnvilLOD](https://github.com/CageTV/AnvilLOD) | GPL-3.0-only, "Copyright (c) 2026 CageTV"; clone `D:\Modlists\ref\AnvilLOD` at `606eece` (v0.2.60). Its own provenance statements are output-observation based (TVDT "decoded from xLODGen's output", occlusion "fitted to … xLODGen's own output", BTO layout "verified vertex-for-vertex against existing output"); zero LODGen signature names. | **Derivation source.** Eleven files carry CageTV's copyright, the source file and commit, and modification marks (GPL §5a): `crates/dyndolod/src/occlusion/{grid,tvdt,visibility}.rs`, `crates/objlod/src/{build,grid,hidden,lib,material,mesh,refs,transform}.rs`. Listed in THIRD-PARTY-NOTICES part 1.1. |
| [TreeLODGen](https://github.com/Cl3mus33/TreeLODGen) | GPL-3.0, "Copyright (C) 2026 Clemus"; clone `D:\Modlists\ref\TreeLODGen` at `dc4cc8a` (v1.0.6), read as data (not built or run). | **Derivation source** for generated 3D tree LOD models (Ultra phase 3, docs/ultra-tree-models-plan.md). Algorithm spec: docs/re/treelodgen-algorithm.md. Ported files carry Clemus' copyright, the source files and commit, and modification marks: `crates/treemodel/src/*.rs`. Listed in THIRD-PARTY-NOTICES part 1.7. |
| Runtime scripts, resource packs, configuration, game/mod assets | Read from the user's installation. "Assets Licensed under creative commons : CC BY-NC-SA". | Never bundled. The repository holds no game/mod assets, decompiles, local output or private profiles (checked at each release per [releases.md](releases.md)). |
| Cargo dependencies and native components | `cargo metadata --filter-platform x86_64-pc-windows-msvc`: 507 third-party crates, all MIT, Apache-2.0, BSD, ISC, Zlib, Unicode-3.0, MPL-2.0 or CC0 (one crate Apache-2.0 WITH LLVM-exception); no GPL-incompatible dependency. The UI frontend (`apps/ui/dist`) is hand-written; WebView2 is a system runtime, not distributed. | **Inventoried.** `scripts/third_party_notices.py` generates part 2 of THIRD-PARTY-NOTICES.txt and fails when a license text is missing from `licenses/`; `--check` in CI keeps it current. |

## Research material and the quarantine

The rewrites are defensible because the implementers never saw the replaced
expression. Their process records (what was read, what was never opened, which
AnvilLOD files were ported) are in the three spec documents. The decompile- and
disassembly-derived text was moved out of the repository to
`D:\Modlists\ref\quarantine\` (private, not for implementers):

- `dyndolod-occlusion-decomp.md`, `object-lod-lodgen-{delta,notes}.md`,
  `terrain-lod-lodgen-notes.md` (the B0 scrubs, 2026-10-19/20);
- `baseline-lodgen-notes.md`, `gap-audit-lodgen-notes.md`,
  `dyndolod-spec-lodgen-notes.md`, `xlodgen-terrain-textures-ghidra-notes.md`,
  `grass-cache-engine-notes.md` (the 2026-10-20 doc pass: baseline.md port
  sections keep their measurement tables and a pointer; parity.md lost its
  decompile reference list; the gap audit lost the lines that cited
  disassembly; the DynDOLOD spec lost the `[lodgen-src]` sections; the two
  reverse-engineering specs lost addresses, evidence tables and
  internals sections).

The decompiles themselves (`D:\Modlists\ref\lodgen-*`, `ref\net10-numerics`,
`ref\skyrimse-*`, the REA/Ghidra snapshots) were never in the repository. The
replaced modules survive only in the private history (see below).

## Repository history and publication

The development history contained the LODGen-derived code (MPL 1.1) combined
with GPL code (the grass generator), the decompile-derived occlusion module and
the shader transcription. Publishing that history would have distributed
non-compliant combinations, so the published repository starts from a single
**root commit** holding the cleared tree and nothing before it.

**What was done (2026-10-20, maintainer decision):** the history of
`github.com/mactinite/LODForge` was replaced in place. The cleared tree at
private commit `8bf0341` became an orphan root commit, force-pushed as `main`
(the only remote branch; there were no remote tags), and the repository was
then switched to public. The full development history stays local on the
branch `private-history` and the development worktree branches; it is not
pushed anywhere. The pre-push check is that the tree holds no quarantined
content, game or mod assets, local output or private profiles (path strings
such as the quarantine folder's name are fine).

**Residual:** GitHub can keep commits that are no longer reachable from any
branch for a while (caches, "activity" views, forks); the repository had no
forks and was private throughout, so nobody holds the old commit ids. A
GitHub Support request to purge unreachable objects removes the remainder;
that is the maintainer's call.

Tags and the release workflow operate on the published history only.

**Corresponding Source (GPL §6).** Each binary release carries a source archive
of the exact tagged commit (`git archive`, produced by the release workflow)
and this repository's build instructions (docs/development.md, pinned
toolchain). Third-party crates are not vendored: they are fetched by Cargo from
crates.io at the versions pinned in Cargo.lock, and `cargo vendor` reproduces
their sources on demand. Their license texts and notices ship in the archive
(THIRD-PARTY-NOTICES.txt, licenses/).

## Clearance checklist

- [x] Stage B0 specs written and the decompile-derived material quarantined
      outside the repository (terrain LOD, object LOD, occlusion).
- [x] Stage B1 (terrain LOD meshes + NIF writer/BTR assembly) rewritten from a
      fresh context; process recorded; old modules deleted (`b96c5b9`).
- [x] Stage B2 (object LOD, CLI atlas/grass pieces) rewritten from a fresh
      context; process recorded; old modules deleted (`c5b4f4e`).
- [x] Target 1 (occlusion) and Target 3 (billboard shader) landed from fresh
      contexts; process recorded (`7cac5ed`, `f79eb6e`).
- [x] Grass generator: removed (`5a9dc95`); decision recorded.
- [x] Research documents: decompile detail replaced by pointers / quarantined
      (2026-10-20); `grep -rn "LODGen's"` reviewed, remaining mentions are
      format or compatibility statements.
- [x] AnvilLOD-derived files carry CageTV's copyright and GPL notice with
      modification marks; xEdit files keep MPL-2.0 notices; s-hull notice kept.
- [x] `LICENSE` (GPL-3.0 text) at the root; `license = "GPL-3.0-only"` inherited
      by every crate (treelod: `MPL-2.0`); SPDX headers on crate roots;
      `THIRD-PARTY-NOTICES.txt` and `licenses/` complete and generated.
- [x] Corresponding Source and the clean-commit publication procedure
      documented (above and in releases.md).
- [ ] Maintainer: confirm ownership / contributor authority for original code,
      docs and art (all commits are the maintainer's with Claude as co-author).
- [ ] Maintainer: in-game validation to the agreed standard (terrain rewrite
      leftovers: Winterhold vantage, volumetric-lighting underside check;
      occlusion and object LOD sweeps done 2026-10-20).
- [ ] Maintainer: publish from a clean commit as described above; inspect
      archives and tracked configuration before changing visibility; approve the
      exact release commit per [releases.md](releases.md).

The release gate checks that approval and required documents exist; it cannot
determine whether their legal contents are adequate.

## Names and attribution

Use LODForge as the product name. Keep upstream names where they identify
authorship, evidence, compatible file formats, CLI options, runtime requirements,
process names or dialog captions. Avoid implying endorsement. Removing such
references would neither cure licensing issues nor safely preserve compatibility.
