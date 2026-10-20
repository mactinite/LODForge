# Builds and releases

## Current status

LODForge is GPL-3.0-only; the code-level provenance work is complete and the
license documents (`LICENSE`, `THIRD-PARTY-NOTICES.txt`, `licenses/`) are in
place (2026-10-20). Public distribution still waits on the maintainer items of
the [clearance checklist](licensing.md#clearance-checklist) and must start
from a clean commit (see "Publication from a clean commit" below). Do not
switch repository visibility or publish source/binaries merely because CI
passes. This setup creates drafts only; it never publishes a release automatically.

The first release target is an experimental, unsigned **Windows x64 portable ZIP**
containing both the CLI and UI. There is no installer, auto-update service, code
signing, bundled WebView2 installer or bundled game/mod/runtime assets.
Users must install WebView2 for the UI and the Microsoft Visual C++ v14 x64
Redistributable for the CLI. The latter imports `VCRUNTIME140.dll` and
`VCRUNTIME140_1.dll`; it is not a self-contained static-CRT build.

## Continuous integration

[CI](../.github/workflows/ci.yml) runs on branch pushes, pull requests and manual
dispatch. It uses Windows Server 2022, the MSVC toolchain pinned in
[rust-toolchain.toml](../rust-toolchain.toml), and the committed Cargo lockfile.
It runs release-guard tests, workspace tests and Clippy, API docs, a production
CLI/UI build and a portable-package smoke test. No binary artifacts are uploaded.

Formatting is explicitly advisory until existing workspace formatting debt is
resolved. Clippy warnings remain visible but are not promoted to errors.
Missing local golden fixtures / GPU adapters can cause existing tests to skip:
CI is not certification of in-game behavior, GPU support or runtime parity.

The UI production build enables `lodforge-ui/custom-protocol`. Its HTML/CSS/JS
is embedded from the checked-in frontend; Node/npm and a development server are
not release dependencies.

## Local private build and packaging

Run from the repository root in PowerShell:

```powershell
.\scripts\Test-ReleaseTools.ps1
cargo build --locked --release -p lodforge -p lodforge-ui --features lodforge-ui/custom-protocol
.\scripts\New-ReleaseArchive.ps1
```

The packager writes `target\dist\LODForge-vX.Y.Z-windows-x64.zip` and
`SHA256SUMS.txt`. It extracts the ZIP and checks CLI version, CLI/build help and
the neighboring UI executable. It records commit, dirty-worktree status and
toolchain in `build-info.json`; a private dirty build is not a release candidate.
Use `-Destination target\another-private-build` for another run: existing output
is never silently overwritten.

The packager warns when legal documents are absent; it can be used for local
testing without granting distribution rights. Only the separate release workflow
can upload artifacts, after its clearance gate. The archive includes docs;
links into source or developer scripts require the matching source checkout.

## Clearing and preparing a release

1. Resolve **all** licensing blockers and add the reviewed `LICENSE`,
   `THIRD-PARTY-NOTICES.txt` and upstream license texts under `licenses/`.
   Check source/dependency availability obligations, not only notices.
2. Set matching `X.Y.Z` versions in both application Cargo manifests and
   [Tauri configuration](../apps/ui/tauri.conf.json). Update Cargo.lock as needed.
   Library crate versions can be managed independently.
3. Commit the complete release candidate. Run CI and the [validation
   procedure](validation.md); record unsupported combinations and runtime gaps.
   Test the extracted UI on a clean Windows desktop with WebView2, without Rust
   or a dev server. Verify that it invokes the adjacent CLI. GPU/game checks
   require a separate qualified machine and legitimately installed inputs.
4. Have a maintainer review that exact commit and set the repository Actions
   variable `RELEASE_APPROVED_COMMIT` to its **full lowercase 40-character SHA**.
   Do not set it now or use it as a routine automation bypass. Any source change
   needs fresh review and approval. Restrict who can change variables, tags and
   workflows; configure repository rules / branch protection separately.
5. Create and push a tag `vX.Y.Z` identifying that commit. Tags trigger
   [Draft release](../.github/workflows/release.yml); manual dispatch accepts an
   existing tag in the same format. Version suffixes / leading zeroes are
   deliberately rejected; releases are marked experimental prereleases even
   though the tag is `vX.Y.Z`.
6. The gate checks approval, tag/commit identity, a clean checkout, CLI/UI/Tauri
   version agreement and committed nonempty license/notice files plus upstream
   license texts. It does not substitute for legal review.
7. The workflow tests and builds the approved commit, packages the portable ZIP,
   uses `git archive` for matching tracked source, and attaches both ZIPs plus
   SHA-256 checksums to a **draft prerelease**. Dependency sources are not
   automatically vendored; resolve any obligation to supply them before approval.
   Artifacts are retained for seven days; draft attachments persist.
8. Inspect all three assets, notices, source, checksum values and release notes.
   Complete the repository/history review before making the repository public.
   Publish the draft manually only after both legal and runtime qualification.
   Clear the approval variable afterward.

## Publication from a clean commit and Corresponding Source

The development history contained code that was later replaced for license
reasons (see [licensing](licensing.md#repository-history-and-publication)).
The published repository therefore starts **without history**: an orphan root
commit holding the cleared tree, force-pushed as `main` on 2026-10-20 before
the repository was made public. Mapping: the root commit of the published
history is the tree of private commit `8bf0341` (development branch
`private-history`, kept locally, never pushed). Tags and the release workflow
operate on the published history only.

Corresponding Source for a binary release is the matching source ZIP the
workflow attaches (`git archive` of the tagged commit) plus the build
instructions in [development.md](development.md) and the pinned toolchain.
Third-party crates are fetched by Cargo from crates.io at the versions in
Cargo.lock; `cargo vendor` reproduces their sources on demand. Their notices and
license texts ship in the archive (`THIRD-PARTY-NOTICES.txt`, `licenses/`,
regenerated with `scripts/third_party_notices.py`; `--check` fails CI when it
is stale).

An existing release is never overwritten automatically. A partial upload or
duplicate-tag error stops the job: inspect the draft and decide how to recover
rather than silently replacing distributed files. Revoking approval blocks new
workflow uploads but does not withdraw already uploaded assets.

Pinning Rust, Cargo.lock and Actions revisions improves repeatability; this is
not a claim of byte-for-byte reproducible builds or publisher authentication.
Unsigned binaries can trigger SmartScreen. Users should obtain downloads from
the official repository, verify SHA-256 checksums and retain the source/notices.
