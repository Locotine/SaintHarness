#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DESKTOP_DIR="$ROOT_DIR/apps/desktop"
BIN_SRC="$DESKTOP_DIR/src-tauri/target/release/saint-harness-desktop"
ICON_128="$DESKTOP_DIR/src-tauri/icons/128x128.png"
ICON_256="$DESKTOP_DIR/src-tauri/icons/128x128@2x.png"
ICON_512="$DESKTOP_DIR/src-tauri/icons/icon.png"

if [[ ! -x "$BIN_SRC" ]]; then
  echo "error: release desktop binary not found at $BIN_SRC" >&2
  echo "run: make desktop-deb" >&2
  exit 1
fi

BIN_DIR="${HOME}/.local/bin"
APP_DIR="${HOME}/.local/share/applications"
ICON_128_DIR="${HOME}/.local/share/icons/hicolor/128x128/apps"
ICON_256_DIR="${HOME}/.local/share/icons/hicolor/256x256/apps"
ICON_512_DIR="${HOME}/.local/share/icons/hicolor/512x512/apps"

mkdir -p "$BIN_DIR" "$APP_DIR" "$ICON_128_DIR" "$ICON_256_DIR" "$ICON_512_DIR"

install -m 0755 "$BIN_SRC" "$BIN_DIR/saint-harness-desktop"
install -m 0644 "$ICON_128" "$ICON_128_DIR/saint-harness.png"
install -m 0644 "$ICON_256" "$ICON_256_DIR/saint-harness.png"
install -m 0644 "$ICON_512" "$ICON_512_DIR/saint-harness.png"
rm -f "${HOME}/.local/share/icons/hicolor/scalable/apps/saint-harness.svg"

cat >"$APP_DIR/saint-harness.desktop" <<EOF
[Desktop Entry]
Type=Application
Version=1.0
Name=SaintHarness
GenericName=Coding Agent Workspace
Comment=Pure Rust coding agent desktop workspace
Exec=${BIN_DIR}/saint-harness-desktop
Icon=saint-harness
Terminal=false
Categories=Development;IDE;Utility;
StartupNotify=true
StartupWMClass=saint-harness-desktop
EOF

if [[ -d "${HOME}/Desktop" ]]; then
  cp "$APP_DIR/saint-harness.desktop" "${HOME}/Desktop/saint-harness.desktop"
  chmod +x "${HOME}/Desktop/saint-harness.desktop"
fi

if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database "$APP_DIR" >/dev/null 2>&1 || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -f -t "${HOME}/.local/share/icons/hicolor" >/dev/null 2>&1 || true
fi

echo "==> Installed SaintHarness Desktop binary to ${BIN_DIR}/saint-harness-desktop"
echo "==> Registered desktop launcher at ${APP_DIR}/saint-harness.desktop"
if [[ -f "${HOME}/Desktop/saint-harness.desktop" ]]; then
  echo "==> Added desktop shortcut at ${HOME}/Desktop/saint-harness.desktop"
fi
