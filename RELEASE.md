# SaintHarness Release Engineering Guide

This document describes the versioning policy, cross-platform build matrix, Linux Debian/Ubuntu ABI verification, GitHub Actions release pipeline, downstream package-manager distribution, and rollback procedures for **SaintHarness**.

---

## 1. Versioning Policy

SaintHarness follows strict Semantic Versioning (`MAJOR.MINOR.PATCH`):

- **Git tag format:** `vX.Y.Z` (for example, `v0.1.0`).
- **Single source of truth:** `scripts/release/sync-version.py` validates strict SemVer and synchronizes the version string across all CLI and Desktop manifests:
  - Root workspace `Cargo.toml` (`[workspace.package] version`)
  - npm thin wrapper `packages/npm-cli/package.json` (`version`)
  - Desktop wrapper `apps/desktop/package.json` (`version`)
  - Desktop Rust crate `apps/desktop/src-tauri/Cargo.toml` (`[package] version`)
  - Tauri v2 bundle config `apps/desktop/src-tauri/tauri.conf.json` (`version`)
- **Automated enforcement:** The `preflight` job in `.github/workflows/release.yml` normalizes and validates the tag or `workflow_dispatch` input before any build runner starts.

---

## 2. Target Matrix & Published Assets

### CLI / TUI Matrix (`build_release`)

Every production release compiles the stripped `saint-harness` binary with `--features acp` across five native targets:

| Platform ID | GitHub Runner | Rust Target | Archive Assets |
| :--- | :--- | :--- | :--- |
| `linux-x64` | `ubuntu-22.04` | `x86_64-unknown-linux-gnu` | `saint-harness_<ver>_linux-x86_64.tar.gz`, `saint-harness_<ver>_amd64.deb` |
| `linux-arm64` | `ubuntu-22.04-arm` | `aarch64-unknown-linux-gnu` | `saint-harness_<ver>_linux-aarch64.tar.gz`, `saint-harness_<ver>_arm64.deb` |
| `mac-arm64` | `macos-15` | `aarch64-apple-darwin` | `saint-harness_<ver>_darwin-arm64.tar.gz` |
| `mac-x64` | `macos-15` | `x86_64-apple-darwin` | `saint-harness_<ver>_darwin-x86_64.tar.gz` |
| `win-x64` | `windows-2022` | `x86_64-pc-windows-msvc` | `saint-harness_<ver>_windows-x86_64.zip` |

### Native Desktop App Matrix (`build_desktop`, Tauri v2)

When `include_desktop=true` (always enabled on `v*.*.*` tag pushes), `build_desktop` compiles the native desktop application embedding `saint-harness-server` in-process across four native targets:

| Desktop ID | GitHub Runner | Rust Target | Desktop Installer Assets |
| :--- | :--- | :--- | :--- |
| `desktop-linux-x64` | `ubuntu-22.04` | `x86_64-unknown-linux-gnu` | `saint-harness-desktop_<ver>_amd64.deb` |
| `desktop-mac-arm64` | `macos-15` | `aarch64-apple-darwin` | `saint-harness-desktop_<ver>_aarch64.dmg` |
| `desktop-mac-x64` | `macos-15` | `x86_64-apple-darwin` | `saint-harness-desktop_<ver>_x64.dmg` |
| `desktop-win-x64` | `windows-2022` | `x86_64-pc-windows-msvc` | `saint-harness-desktop_<ver>_x64-setup.exe`, `saint-harness-desktop_<ver>_x64.msi` |

### Complete GitHub Release Asset Inventory

The `publish_release` job fails closed if any of the 7 CLI platform bundles or (when `include_desktop=true`) 5 Desktop installers is missing, then publishes:

1. `saint-harness_<ver>_linux-x86_64.tar.gz`
2. `saint-harness_<ver>_amd64.deb`
3. `saint-harness_<ver>_linux-aarch64.tar.gz`
4. `saint-harness_<ver>_arm64.deb`
5. `saint-harness_<ver>_darwin-arm64.tar.gz`
6. `saint-harness_<ver>_darwin-x86_64.tar.gz`
7. `saint-harness_<ver>_windows-x86_64.zip`
8. `saint-harness-desktop_<ver>_amd64.deb`
9. `saint-harness-desktop_<ver>_aarch64.dmg`
10. `saint-harness-desktop_<ver>_x64.dmg`
11. `saint-harness-desktop_<ver>_x64-setup.exe`
12. `saint-harness-desktop_<ver>_x64.msi`
13. `SHA256SUMS.txt` (SHA-256 digests for all CLI and Desktop release assets)
14. `release-manifest.json` (structured `platforms` and `desktop` download URLs, sizes, and SHA-256 digests consumed by `saint-harness update` and `@saintharness/cli`)
15. `install.sh` (macOS / Debian / Ubuntu CLI installer)
16. `install.ps1` (Windows x64 PowerShell CLI installer)
17. `saint-harness.rb` (Homebrew Formula)
18. `saint-harness.scoop.json` (Scoop manifest)
19. `saint-harness_<ver>_winget-manifests.tar.gz` (WinGet `version`, `installer`, and `locale` YAML files)

---

## 3. Platform Compatibility Guarantees

### Linux Debian & Ubuntu ABI Contract

A single `.deb` and `.tar.gz` per architecture must run across Debian 12 (Bookworm), Debian 13 (Trixie), Ubuntu 22.04 (Jammy), and Ubuntu 24.04 (Noble):

1. **Build baseline:** Linux release jobs run strictly on `ubuntu-22.04` (`x86_64`) and `ubuntu-22.04-arm` (`aarch64`), keeping dynamic symbol versions at or below `GLIBC 2.35`.
2. **Static binary checks (`scripts/release/build-deb.sh`):**
   - Inspects `readelf -V` on the stripped binary and rejects any non-weak `GLIBC > 2.35` requirement (preventing accidental Ubuntu 24.04 `GLIBC_2.38` breakage on Debian 12 or Ubuntu 22.04).
   - Verifies the generated `.deb` control metadata contains zero Ubuntu 24.04-only `*t64*` package names.
   - Enforces explicit `0755` directory and `0644` control file permissions independent of host `umask`.
   - Uses pure Rust `rustls-tls` and bundled `rusqlite` (`modern_sqlite`) so the binary requires only base `libc6` (`>= 2.35`) and `ca-certificates` at runtime.
3. **Real container install verification (`verify_debian_ubuntu` job):**
   Before `publish_release` runs, CI downloads `saint-harness_<ver>_amd64.deb` and runs `apt-get install -y` plus CLI smoke checks inside four clean official containers:
   - `debian:bookworm` (Debian 12)
   - `debian:trixie` (Debian 13)
   - `ubuntu:22.04`
   - `ubuntu:24.04`

### macOS Verification & Code Signing

- `lipo -archs` validates that `mac-arm64` contains `arm64` and `mac-x64` contains `x86_64`.
- If the repository secret `APPLE_SIGNING_IDENTITY` is configured, binaries are signed with Hardened Runtime (`--options runtime --timestamp`) and verified with `codesign --verify --strict`.
- Without `APPLE_SIGNING_IDENTITY`, the pipeline applies ad-hoc signing (`codesign --force --sign -`) so Apple Silicon executes the binary without invalid Mach-O signature errors.
- Native CLI smoke checks (`--version`, `models --json`, offline `server status` exit code `1`) execute on `macos-15`.

### Windows Verification

- Compiles natively on `windows-2022` targeting `x86_64-pc-windows-msvc`.
- Executes native PowerShell smoke checks verifying `--version`, `models --json` (`"hardcodedProviderVersions": 0`), and `server status --url http://127.0.0.1:1 --json` exit code `1`.

---

## 4. Release Pipeline Overview

`.github/workflows/release.yml` runs five coordinated jobs (with `.github/workflows/ci-desktop.yml` gating Desktop PR and `main` changes across Ubuntu 24.04, macOS 15, and Windows 2022):

1. **`preflight` (`ubuntu-24.04`):** Normalizes SemVer, runs `test_ci_contracts.py`, `check-cleanroom.sh`, `check-firewall.sh`, `check-prototype-parity.sh`, `cargo fmt --check`, `cargo clippy -- -D warnings`, and `cargo test --workspace --locked`.
2. **`build_release` (5-runner CLI matrix):** Synchronizes workspace version, builds stripped `saint-harness --features acp`, runs OS-specific verification and smoke tests, packages `.tar.gz` / `.deb` / `.zip`, and uploads workflow artifacts.
3. **`build_desktop` (4-runner Tauri v2 matrix, runs in parallel with `build_release` when `include_desktop=true`):** Synchronizes versions across all 5 manifests, compiles the native desktop app (`npx tauri build --ci`) on `ubuntu-22.04`, `macos-15` (arm64 + x64), and `windows-2022`, standardizes `saint-harness-desktop_<ver>_*` installer filenames, and uploads workflow artifacts.
4. **`verify_debian_ubuntu` (4-distro container matrix):** Installs CLI `saint-harness_<ver>_amd64.deb` via `apt-get` inside Debian 12, Debian 13, Ubuntu 22.04, and Ubuntu 24.04 containers and executes CLI smoke tests.
5. **`publish_release` (`ubuntu-24.04`, scoped `contents: write`):** Downloads all CLI and Desktop artifacts, stages `install.sh` and `install.ps1`, generates `SHA256SUMS.txt` and `release-manifest.json` (with `--require-desktop` when `include_desktop=true`), builds Homebrew/Scoop/WinGet manifests, and creates or updates the GitHub Release via `gh release`.

---

## 5. Step-by-Step Release Runbook

### Step 1: Local Pre-Flight

Ensure `main` is clean and every local quality gate passes:

```bash
make all
make desktop-check
make release-full
```

Confirm the built binary works locally:

```bash
./target/release/saint-harness --version
./target/release/saint-harness doctor
```

### Step 2: Dry-Run Build (Recommended Before Tagging)

Trigger a dry-run in GitHub Actions without publishing a public GitHub Release:

```bash
gh workflow run release.yml \
  -f version=0.2.0 \
  -f publish_release=false \
  -f platform=all \
  -f include_desktop=true
```

To test a single CLI runner rapidly (for example, `linux-x64` or `win-x64`), set `-f platform=linux-x64 -f include_desktop=false`. Inspect the uploaded workflow artifacts and verify `verify_debian_ubuntu` passes across all four containers.

### Step 3: Bump Version and Push Release Tag

Synchronize the version across Cargo, npm, and Tauri Desktop manifests, commit, and push the `vX.Y.Z` tag:

```bash
python3 scripts/release/sync-version.py --sync 0.2.0
python3 scripts/release/sync-version.py --verify 0.2.0
git status
git add Cargo.toml packages/npm-cli/package.json apps/desktop/package.json apps/desktop/src-tauri/Cargo.toml apps/desktop/src-tauri/tauri.conf.json
git commit -m "chore(release): prepare v0.2.0"
git tag v0.2.0
git push origin main --tags
```

Pushing a `v*.*.*` tag automatically runs `.github/workflows/release.yml` with `publish_release=true`.

### Step 4: Publish Downstream Distribution Channels

Once the GitHub Release is live, download the generated manifests from the release page and publish to each channel you maintain:

#### Homebrew Tap

```bash
gh release download v0.2.0 -p saint-harness.rb
# Copy saint-harness.rb into your Locotine/homebrew-tap repository under Formula/saint-harness.rb
```

#### Scoop Bucket (Windows)

```bash
gh release download v0.2.0 -p saint-harness.scoop.json
# Copy to bucket/saint-harness.json in your Scoop bucket repository
```

#### WinGet (`microsoft/winget-pkgs`)

```bash
gh release download v0.2.0 -p saint-harness_0.2.0_winget-manifests.tar.gz
tar -xzf saint-harness_0.2.0_winget-manifests.tar.gz
# Submit winget/*.yaml under manifests/s/SaintHarness/SaintHarness/0.2.0/
```

#### Static Debian / Ubuntu APT Repository

```bash
mkdir -p deb-assets
gh release download v0.2.0 -p '*.deb' -D deb-assets
bash scripts/release/build-apt-repo.sh --deb-dir deb-assets --out-dir apt-repo
# Publish apt-repo/ to GitHub Pages, S3, or R2 and sign dists/stable/Release with GPG for production APT hosting
```

#### npm Thin Launcher (`@saintharness/cli`)

```bash
cd packages/npm-cli
npm publish --access public
```

The npm package downloads the matching native Rust binary from `release-manifest.json` during `postinstall` and verifies its SHA-256 digest before making `saint-harness` executable.

### Step 5: Post-Release End-User Verification

Verify live download, checksum verification, and self-update against the published release:

```bash
# Self-update dry check
saint-harness update --check

# Verify SHA-256 manifest integrity
gh release download v0.2.0 -p 'SHA256SUMS.txt' -p 'saint-harness_0.2.0_linux-x86_64.tar.gz'
sha256sum -c --ignore-missing SHA256SUMS.txt

# Test 1-line Unix installer into a temporary prefix
curl -fsSL https://github.com/Locotine/SaintHarness/releases/download/v0.2.0/install.sh \
  | bash -s -- --version v0.2.0 --install-dir /tmp/sh-verify --no-modify-path
/tmp/sh-verify/saint-harness --version
rm -rf /tmp/sh-verify
```

---

## 6. Secrets & Permissions

| Setting | Required | Purpose |
| :--- | :---: | :--- |
| `GITHUB_TOKEN` (automatic) | Yes | Scoped to `contents: read` for preflight/build/verify and `contents: write` strictly on `publish_release`. |
| `APPLE_SIGNING_IDENTITY` | Optional | Developer ID Application certificate CN for Hardened Runtime macOS signing. Falls back to ad-hoc signing when unset. |

The workflow sets top-level `permissions: {}` and uses `concurrency` with `cancel-in-progress: false` so active releases are never truncated mid-publish.

---

## 7. Re-Runs, Hotfixes & Rollback

### Re-Running a Partially Failed Release

`publish_release` is idempotent: if the tag already has a GitHub Release, it updates assets in place using `gh release upload "$TAG" release-artifacts/* --clobber`. You can re-run failed jobs directly from the GitHub Actions UI without deleting the tag.

### Rollback Procedure

Prefer a forward patch release (`vX.Y.(Z+1)`) over mutating published artifacts so cached SHA-256 digests never drift for existing users. If an immediate rollback of the `latest` pointer is required:

1. Mark the broken release as a pre-release or draft via `gh release edit vX.Y.Z --prerelease` (or `--draft`).
2. Promote the previous stable tag back to `latest`:
   ```bash
   gh release edit vX.Y.(Z-1) --latest
   ```
3. Users can explicitly pin the previous version using:
   ```bash
   saint-harness update --version vX.Y.(Z-1) --force
   ```

---

## 8. Release Helper Script Reference

| Script | Purpose |
| :--- | :--- |
| `scripts/release/sync-version.py` | Normalizes strict SemVer (`--normalize`), syncs (`--sync`), and verifies (`--verify`) all 5 manifests (`Cargo.toml`, `packages/npm-cli/package.json`, `apps/desktop/package.json`, `apps/desktop/src-tauri/Cargo.toml`, `apps/desktop/src-tauri/tauri.conf.json`). |
| `scripts/release/build-deb.sh` | Builds CLI `amd64`/`arm64` `.deb` packages, enforces `GLIBC <= 2.35`, rejects `*t64*` deps, and checks package layout. |
| `scripts/release/generate-manifest.py` | Computes `SHA256SUMS.txt` and `release-manifest.json` (`platforms` + optional `desktop`); fails closed if any of the 7 CLI bundles or (with `--require-desktop`) 5 Desktop installers is missing. |
| `scripts/release/generate-package-manifests.py` | Generates `saint-harness.rb`, `saint-harness.scoop.json`, and WinGet YAML manifests from `release-manifest.json`. |
| `scripts/release/build-apt-repo.sh` | Generates a static APT repository (`pool/main`, `Packages`, `Packages.gz`, `Release`) from built `.deb` files. |
