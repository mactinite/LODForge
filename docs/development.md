# Development

## Toolchain

Use the Rust version pinned in [rust-toolchain.toml](../rust-toolchain.toml)
for `x86_64-pc-windows-msvc`, Visual Studio C++ build tools
and the Windows SDK. Texture tests/builds can need a GPU and the UI needs WebView2.
The frontend bundle is checked in (`apps/ui/dist`), so building needs no npm; changing the
frontend does (see "UI frontend" below).

```powershell
rustup component add rustfmt clippy
cargo build --locked -p lodforge
cargo build --locked -p lodforge-ui --features custom-protocol
```

Keep `Cargo.lock` when reproducing a result. The workspace declares Rust 1.85,
but most packages do not inherit that field and some code uses newer syntax.
The minimum supported toolchain needs a separate validation pass; do not infer it
from the workspace declaration.

## Quality checks

[Check.ps1](../scripts/Check.ps1) is a thin wrapper around Cargo. It changes to the
repository root, propagates failures and never formats files in place.

```powershell
# Prefer the packages affected by your change.
.\scripts\Check.ps1 -Check Test,Lint -Package lodforge-objlod,lodforge-dyndolod
.\scripts\Check.ps1 -Check Format -Package lodforge-grass

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

**Formatting debt:** the repository is not uniformly rustfmt-formatted. A broad Format check
can fail without a regression. Clippy and rustdoc warnings are reported, not hidden or globally
suppressed. Do not reformat files alongside a functional change; make a separate formatting
change.

To format an agreed package deliberately:

```powershell
cargo fmt --package lodforge-grass
```

Rustfmt uses Cargo's edition information and standard defaults; no custom style
layer or new package dependencies are necessary. GitHub Actions runs Windows
tests, lint, docs, production builds and packaging checks; formatting remains
advisory. See [builds and releases](releases.md) for the release approval gate
and local packaging commands.

## UI frontend

The frontend source is `apps/ui/web` (React, TypeScript, Vite). `apps/ui/dist` is its committed
bundle, which the Tauri build embeds. After changing the frontend:

```powershell
cd apps\ui\web
npm ci
npm run build
```

Commit the regenerated `apps/ui/dist` with the source change. The option catalog the UI renders
is `apps/ui/web/src/options.ts`; keep it and [build options](build-options.md) in step with the
CLI.

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
- The `nif` and `btr` golden tests use `LODFORGE_GOLDEN_BTR` or a local default fixture
  directory. They can skip when fixtures are absent: green is not proof of a
  golden comparison.
- GPU tests may skip when an adapter is unavailable. Record that separately.
- Ignored tests and reference-tool/game runs are not part of an ordinary Cargo run.
- Use [validation](validation.md) for material/output and runtime claims.

## Contribution boundaries

Preserve attribution and provenance ([licensing](licensing.md)); do not remove source-origin
comments to make code appear independently authored.

Keep game/mod assets and reference output outside version control. Add small
synthetic fixtures for binary edge cases where possible. Never overwrite a
shared profile or reference export for a test.

Do not change pipeline behavior in a documentation or formatting patch. Keep the
[architecture](architecture.md), [build options](build-options.md) and
[limitations](limitations.md) pages consistent when a feature lands.
