# SaintHarness CI & Release Pipeline

Modeled on the CI and release discipline of `saint-hyper-agent`, adapted for a pure Rust 15-crate workspace targeting **macOS**, **Linux Debian**, **Linux Ubuntu**, and **Windows**.

## CI Pipeline (`.github/workflows/ci.yml`)

- **Required branch-protection check:** `Format, Lint, Firewall, Parity, Cross-Platform Test` (`quality` job, aggregates results only and fails closed).
- **Docs-only fast path:** `changes` uses `dorny/paths-filter@v3` with `predicate-quantifier: "every"` so markdown/doc-only PRs run `static-fast` + `quality` and deliberately skip heavy cross-platform lanes.
- **Independent `static-fast` lane:** starts immediately without waiting for `changes`, running:
  1. `python3 .github/scripts/test_ci_contracts.py` (executes the real `quality` gate shell block across all state combinations + release helper tests)
  2. `bash scripts/check-cleanroom.sh` (Laws #2 & #7)
  3. `cargo fmt --all -- --check`
  4. `bash scripts/check-firewall.sh` (Law #1 + single `tree-sitter` ABI)
  5. `bash scripts/check-prototype-parity.sh` (56/56 prototype CSS RGB tokens, WCAG AA >= 4.5:1, anti-slop static check, `ratatui::backend::TestBackend` parity across all 5 breakpoints)
  6. `cargo clippy --workspace --all-targets -- -D warnings` (`unwrap_used / expect_used / panic / dbg_macro = "deny"`)
  7. Thin TUI (`--no-default-features --features tui`) & ACP (`--features acp`) feature compilation checks
- **Native cross-platform test lanes (all 4 target OSes):**
  - `test-ubuntu`: `ubuntu-24.04` + `ubuntu-22.04` (full `cargo test --workspace --locked`, Landlock sandbox tests, plus `.deb` + `GLIBC <= 2.35` smoke on `ubuntu-22.04`).
  - `test-debian`: `rust:1-bookworm` container (native Debian 12 Bookworm tests + all 3 quality gates).
  - `test-macos`: `macos-15` (Apple Silicon native tests + all 3 quality gates).
  - `test-windows`: `windows-2022` (native x64 MSVC tests including SQLite WAL, Windows shell/ConPTY, CLI E2E, firewall, and UI parity).

## Nightly Pipeline (`.github/workflows/ci-nightly.yml`)

Runs daily at `02:00 UTC` (and on `workflow_dispatch`) across `ubuntu-24.04`, `macos-15`, and `windows-2022` to verify optional heavy accelerator features (`fast-lint`, `ts-plugins`, `monorepo-index`, `snapcompact`, `full`) without slowing down PR feedback.

## Release Pipeline (`.github/workflows/release.yml`)

- **Triggers:** tag push `v*.*.*` (auto-publishes) or manual `workflow_dispatch` (defaults to `publish_release: false` for safe dry-runs, with optional single-platform scope filter).
- **Preflight:** validates strict SemVer (`scripts/release/sync-version.py`) and runs all static, parity, clippy, and workspace test gates before building release artifacts.
- **5-target build matrix:**
  - `linux-x64` (`ubuntu-22.04`, `x86_64-unknown-linux-gnu`): `.tar.gz` + `amd64.deb`
  - `linux-arm64` (`ubuntu-22.04-arm`, `aarch64-unknown-linux-gnu`): `.tar.gz` + `arm64.deb`
  - `mac-arm64` (`macos-15`, `aarch64-apple-darwin`): `.tar.gz` + `lipo` arch verification + codesign
  - `mac-x64` (`macos-15`, `x86_64-apple-darwin`): `.tar.gz` + `lipo` arch verification + codesign
  - `win-x64` (`windows-2022`, `x86_64-pc-windows-msvc`): `.zip` + native Windows CLI smoke
- **Debian & Ubuntu compatibility guarantees:**
  - Linux binaries are built on `ubuntu-22.04` / `ubuntu-22.04-arm` and verified via `scripts/release/build-deb.sh` (`readelf -V` rejects non-weak `GLIBC > 2.35`; `dpkg-deb` rejects Ubuntu-24-only `*t64*` dependencies).
  - `verify_debian_ubuntu` installs the produced `amd64.deb` via `apt-get install -y` inside four clean official containers (`debian:bookworm`, `debian:trixie`, `ubuntu:22.04`, `ubuntu:24.04`) and runs CLI smoke checks before publication.
- **Publication:** stages `scripts/install.sh` (macOS/Debian/Ubuntu) and `scripts/install.ps1` (Windows), generates `SHA256SUMS.txt` and `release-manifest.json` (fail-closed if any of the 7 bundles is missing), and idempotently publishes to GitHub Releases.

## Local Verification

```bash
make all
```
