#!/usr/bin/env bash
set -euo pipefail

# Build a static APT repository directory from built amd64/arm64 .deb files
# so Debian and Ubuntu users can install via `apt-get install saint-harness`.

DEB_DIR=""
OUT_DIR=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --deb-dir) DEB_DIR="$2"; shift 2 ;;
    --out-dir) OUT_DIR="$2"; shift 2 ;;
    *) echo "[build-apt-repo] Unknown argument: $1" >&2; exit 2 ;;
  esac
done

if [[ -z "$DEB_DIR" || -z "$OUT_DIR" ]]; then
  echo "Usage: $0 --deb-dir <dir-with-debs> --out-dir <apt-repo-dir>" >&2
  exit 2
fi

mkdir -p "$OUT_DIR/pool/main"
cp -v "$DEB_DIR"/*.deb "$OUT_DIR/pool/main/"

for arch in amd64 arm64; do
  mkdir -p "$OUT_DIR/dists/stable/main/binary-$arch"
  (
    cd "$OUT_DIR"
    dpkg-scanpackages --arch "$arch" --multiversion pool/main \
      > "dists/stable/main/binary-$arch/Packages"
    gzip -9c "dists/stable/main/binary-$arch/Packages" \
      > "dists/stable/main/binary-$arch/Packages.gz"
  )
done

cat > "$OUT_DIR/dists/stable/Release" <<EOF
Origin: SaintHarness
Label: SaintHarness
Suite: stable
Codename: stable
Architectures: amd64 arm64
Components: main
Description: SaintHarness official Debian/Ubuntu APT repository
Date: $(LC_ALL=C date -Ru)
EOF

echo "[build-apt-repo] Built static APT repository in $OUT_DIR"
