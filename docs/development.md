# Development

## Toolchain

Use the Rust version pinned in [rust-toolchain.toml](../rust-toolchain.toml)
for `x86_64-pc-windows-msvc`, Visual Studio C++ build tools
and the Windows SDK. Texture tests/builds can need a GPU and the UI needs WebView2.
There is no npm dependency installation for the checked-in frontend.

```powershell
rustup component add rustfmt clippy
cargo build --locked -p lodforge
cargo build --locked -p lodforge-ui --features custom-protocol
```

Keep `Cargo.lock` when reproducing a result. The workspace declares Rust 1.85,
but most packages do not inherit that field and some code uses newer syntax.
The minimum supported toolchain needs a separate validation pass; do not infer it
from the workspace declaration. This pass used Rust/Cargo 1.98.1.

## Quality checks

[Check.ps1](../scripts/Check.ps1) is a thin wrapper around Cargo. It changes to the
repository root, propagates failures and never formats files in place.

```powershell
# Prefer the packages affected by your change.
.\scripts\Check.ps1 -Check Test,Lint -Package lodforge-objlod,lodforge-dyndolod
.\scripts\Check.ps1 -Check Format -Package lodforge-core

# Broad checks (test/lint/docs exclude the platform-specific UI by default).
.\scripts\Check.ps1 -Check Test
.\scripts\Check.ps1 -Check Lint
.\scripts\Check.ps1 -Check Format
.\scripts\Check.ps1 -Check Docs

# Include the UI explicitly when changing it.
.\scripts\Check.ps1 -Check Lint -Package lodforge-ui
```

These correspond to `cargo test`, `cargo clippy --all-targets`,
`cargo fmt -- --check`, and `cargo doc --no-deps`. Non-format operations use
`--locked`. Multiple checks stop on the first failure.

Validation on 2026-10-19 (code baseline `2737410`, Rust 1.98.1):

- CLI + UI release build passed; documented build/info/comparison flags checked.
- 56 tests passed across core, objlod, dyndolod, texgen and treelod.
- Workspace Clippy (excluding UI) and rustdoc completed with existing warnings.
- Scoped core formatting passed; workspace formatting failed on existing differences.
- 75 local file/heading links in the current overview documents resolved.

This is a tooling/documentation check, not a new golden or in-game parity result.

**Existing debt:** the repository was not uniformly rustfmt-formatted at the
start of this pass. A broad Format check can fail without a regression.
Clippy/rustdoc warnings are reported, not hidden or globally suppressed.
Do not reformat the entire repository alongside a feature change, especially
while other sessions are editing it. Make a separately coordinated formatting
change; then make the broad check a merge gate.

To format an agreed package deliberately:

```powershell
cargo fmt --package lodforge-core
```

Rustfmt uses Cargo's edition information and standard defaults; no custom style
layer or new package dependencies are necessary. GitHub Actions runs Windows
tests, lint, docs, production builds and packaging checks; formatting remains
advisory. See [builds and releases](releases.md) for the release approval gate
and local packaging commands.

## API documentation

```powershell
.\scripts\Check.ps1 -Check Docs
Start-Process .\target\doc\lodforge_objlod\index.html
```

Generated rustdoc goes under ignored `target\doc`; do not commit it.
Use prose docs for workflows and crate-level/item documentation for API contracts.
Rustdoc does not validate Markdown links in this documentation tree.

## Test boundaries

- Unit tests normally live beside code; run `cargo test --locked -p <package>
  <test-filter>` for a narrow regression.
- `nif` integration tests use `LODFORGE_GOLDEN_BTR` or a local default fixture
  directory. They can skip when fixtures are absent: green is not proof of a
  golden comparison.
- GPU tests may skip when an adapter is unavailable. Record that separately.
- Ignored tests and reference-tool/game runs are not part of an ordinary Cargo run.
- Use [validation](validation.md) for material/output and runtime claims.

## Contribution boundaries

Public distribution is blocked by the [licensing/provenance review](licensing.md).
Preserve attribution and evidence; do not remove source-origin comments to make
code appear independently authored.

Keep game/mod assets and reference output outside version control. Add small
synthetic fixtures for binary edge cases where possible. Never overwrite a
shared profile or reference export for a test.

Before editing, check the worktree and coordinate overlapping files. Do not
change pipeline behavior in a documentation/formatting patch. Keep the
[architecture](architecture.md), [parity](parity.md) and [roadmap](../PLAN.md)
consistent when a feature lands.
