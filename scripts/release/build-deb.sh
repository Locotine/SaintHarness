#!/usr/bin/env bash
set -euo pipefail

# Build and verify a Debian/Ubuntu .deb package for SaintHarness.
# Enforces Ubuntu 22.04 / Debian 12 compatibility: zero `t64` Depends and max non-weak GLIBC <= 2.35.

BINARY=""
VERSION=""
ARCH=""
OUT_DIR=""
MAX_GLIBC="2.35"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --binary) BINARY="$2"; shift 2 ;;
    --version) VERSION="$2"; shift 2 ;;
    --arch) ARCH="$2"; shift 2 ;;
    --out-dir) OUT_DIR="$2"; shift 2 ;;
    --max-glibc) MAX_GLIBC="$2"; shift 2 ;;
    *) echo "[build-deb] Unknown argument: $1" >&2; exit 2 ;;
  esac
done

if [[ -z "$BINARY" || -z "$VERSION" || -z "$ARCH" || -z "$OUT_DIR" ]]; then
  echo "Usage: $0 --binary <path> --version <X.Y.Z> --arch <amd64|arm64> --out-dir <dir> [--max-glibc 2.35]" >&2
  exit 2
fi

if [[ ! -x "$BINARY" ]]; then
  echo "[build-deb] Binary is missing or not executable: $BINARY" >&2
  exit 1
fi

case "$ARCH" in
  amd64|arm64) ;;
  *) echo "[build-deb] Unsupported Debian architecture: $ARCH (expected amd64 or arm64)" >&2; exit 2 ;;
esac

mkdir -p "$OUT_DIR"
WORK_DIR="$(mktemp -d)"
trap 'rm -rf "$WORK_DIR"' EXIT

PKG_ROOT="$WORK_DIR/pkg"
mkdir -p "$PKG_ROOT/DEBIAN" "$PKG_ROOT/usr/bin" "$PKG_ROOT/usr/share/doc/saint-harness"
chmod 0755 "$PKG_ROOT" "$PKG_ROOT/DEBIAN" "$PKG_ROOT/usr" "$PKG_ROOT/usr/bin" \
  "$PKG_ROOT/usr/share" "$PKG_ROOT/usr/share/doc" "$PKG_ROOT/usr/share/doc/saint-harness"

install -m 0755 "$BINARY" "$PKG_ROOT/usr/bin/saint-harness"
INSTALLED_KB="$(du -sk "$PKG_ROOT/usr" | awk '{print $1}')"

cat > "$PKG_ROOT/usr/share/doc/saint-harness/copyright" <<'EOF'
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: saint-harness
Source: https://github.com/Locotine/SaintHarness

Files: *
Copyright: 2026 SaintHarness Contributors
License: MIT
 Permission is hereby granted, free of charge, to any person obtaining a copy
 of this software and associated documentation files (the "Software"), to deal
 in the Software without restriction, including without limitation the rights
 to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
 copies of the Software, and to permit persons to whom the Software is
 furnished to do so, subject to the following conditions:
 .
 The above copyright notice and this permission notice shall be included in all
 copies or substantial portions of the Software.
 .
 THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
 AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
 OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
 SOFTWARE.
EOF

cat > "$PKG_ROOT/DEBIAN/control" <<EOF
Package: saint-harness
Version: ${VERSION}
Section: devel
Priority: optional
Architecture: ${ARCH}
Installed-Size: ${INSTALLED_KB}
Maintainer: SaintHarness Maintainers <maintainers@saintharness.dev>
Depends: libc6 (>= 2.31), libgcc-s1 | libgcc1
Description: Pure Rust Coding Agent Harness
 SaintHarness is a standalone pure Rust coding agent harness featuring
 interactive ratatui TUI, HTTP/SSE daemon, ACP IDE bridge, SQLite WAL
 event store, OS sandboxing, and multi-provider prompt cache determinism.
EOF

chmod 0644 "$PKG_ROOT/DEBIAN/control" "$PKG_ROOT/usr/share/doc/saint-harness/copyright"

DEB_PATH="$OUT_DIR/saint-harness_${VERSION}_${ARCH}.deb"
dpkg-deb --root-owner-group --build "$PKG_ROOT" "$DEB_PATH"

echo "[build-deb] Verifying package metadata and glibc baseline for $DEB_PATH..."
dpkg-deb --info "$DEB_PATH"

PKG_NAME="$(dpkg-deb --field "$DEB_PATH" Package)"
if [[ "$PKG_NAME" != "saint-harness" ]]; then
  echo "[build-deb] Unexpected Package name: $PKG_NAME" >&2
  exit 1
fi

DEPENDS="$(dpkg-deb --field "$DEB_PATH" Depends)"
case "$DEPENDS" in
  *t64*)
    echo "[build-deb] FAIL: Depends contains Ubuntu 24.04/Debian 13-only t64 package(s): $DEPENDS" >&2
    exit 1
    ;;
esac

EXTRACT_DIR="$WORK_DIR/extract"
mkdir -p "$EXTRACT_DIR"
dpkg-deb -x "$DEB_PATH" "$EXTRACT_DIR"
test -x "$EXTRACT_DIR/usr/bin/saint-harness"

if command -v readelf >/dev/null 2>&1; then
  MAX_REQUIRED="$(readelf -V "$EXTRACT_DIR/usr/bin/saint-harness" \
    | awk '/Name: GLIBC_/ && !/Flags: WEAK/ { for (i = 1; i <= NF; i++) if ($i ~ /^GLIBC_/) print $i }' \
    | sed 's/^GLIBC_//' | sort -uV | tail -n1 || true)"
  echo "[build-deb] usr/bin/saint-harness max non-weak GLIBC: ${MAX_REQUIRED:-none}"
  if [[ -n "$MAX_REQUIRED" ]] && ! dpkg --compare-versions "$MAX_REQUIRED" le "$MAX_GLIBC"; then
    echo "[build-deb] FAIL: binary requires GLIBC_${MAX_REQUIRED} (> ${MAX_GLIBC}), breaking Ubuntu 22.04 / Debian 12." >&2
    exit 1
  fi
fi

echo "[build-deb] PASS: built and verified $DEB_PATH"
