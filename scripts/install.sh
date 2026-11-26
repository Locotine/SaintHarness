#!/usr/bin/env bash
# SaintHarness cross-platform installer for macOS, Debian, and Ubuntu.
# Usage:
#   curl -fsSL https://github.com/Locotine/SaintHarness/releases/latest/download/install.sh | bash
#   bash install.sh [--version v0.1.0] [--deb] [--local target/release/saint-harness]
#                   [--install-dir ~/.local/bin] [--no-modify-path] [--uninstall]

set -euo pipefail

REPO="${SAINT_HARNESS_REPO:-Locotine/SaintHarness}"
VERSION="${SAINT_HARNESS_VERSION:-latest}"
INSTALL_DIR="${SAINT_HARNESS_INSTALL_DIR:-$HOME/.local/bin}"
USE_DEB=false
LOCAL_BIN=""
MODIFY_PATH=true
UNINSTALL=false

say() { printf '\033[1m==>\033[0m %s\n' "$1"; }
fail() { printf 'Error: %s\n' "$1" >&2; exit 1; }

while [[ $# -gt 0 ]]; do
  case "$1" in
    --repo) REPO="$2"; shift 2 ;;
    --version) VERSION="$2"; shift 2 ;;
    --install-dir) INSTALL_DIR="$2"; shift 2 ;;
    --local) LOCAL_BIN="$2"; shift 2 ;;
    --deb) USE_DEB=true; shift ;;
    --no-modify-path) MODIFY_PATH=false; shift ;;
    --uninstall) UNINSTALL=true; shift ;;
    -h|--help)
      echo "Usage: $0 [--version vX.Y.Z|latest] [--repo owner/repo] [--install-dir dir] [--local bin] [--deb] [--no-modify-path] [--uninstall]"
      exit 0
      ;;
    *) fail "Unknown option: $1" ;;
  esac
done

run_as_root() {
  if [[ "$(id -u)" -eq 0 ]]; then
    "$@"
  elif command -v sudo >/dev/null 2>&1; then
    sudo "$@"
  else
    local cmd
    printf -v cmd '%q ' "$@"
    su -c "$cmd"
  fi
}

append_path_if_missing() {
  local rc_file="$1"
  local line="$2"
  local marker="# SaintHarness PATH"
  if [[ -f "$rc_file" ]] && grep -Fq "$marker" "$rc_file"; then
    return 0
  fi
  if [[ -f "$rc_file" ]] && grep -Fq "$INSTALL_DIR" "$rc_file"; then
    return 0
  fi
  mkdir -p "$(dirname "$rc_file")"
  {
    printf '\n%s\n' "$marker"
    printf '%s\n' "$line"
  } >> "$rc_file"
  say "Added $INSTALL_DIR to $rc_file"
}

ensure_shell_path() {
  if [[ "$MODIFY_PATH" != true ]]; then
    return 0
  fi
  case ":$PATH:" in
    *":$INSTALL_DIR:"*) return 0 ;;
  esac

  local shell_name
  shell_name="$(basename "${SHELL:-}")"
  local updated=false

  case "$shell_name" in
    zsh)
      append_path_if_missing "$HOME/.zshrc" "export PATH=\"$INSTALL_DIR:\$PATH\""
      updated=true
      ;;
    bash)
      if [[ -f "$HOME/.bashrc" ]]; then
        append_path_if_missing "$HOME/.bashrc" "export PATH=\"$INSTALL_DIR:\$PATH\""
      else
        append_path_if_missing "$HOME/.profile" "export PATH=\"$INSTALL_DIR:\$PATH\""
      fi
      updated=true
      ;;
    fish)
      append_path_if_missing "$HOME/.config/fish/config.fish" "fish_add_path -g \"$INSTALL_DIR\""
      updated=true
      ;;
  esac

  if [[ "$updated" != true ]]; then
    append_path_if_missing "$HOME/.profile" "export PATH=\"$INSTALL_DIR:\$PATH\""
  fi
  say "Restart your shell or run: export PATH=\"$INSTALL_DIR:\$PATH\""
}

verify_sha256() {
  local file="$1"
  local sums="$2"
  local base expected actual
  base="$(basename "$file")"
  expected="$(awk -v f="$base" '$2 == f {print $1}' "$sums" | head -n1)"
  [[ -n "$expected" ]] || fail "No SHA-256 checksum entry found for $base in SHA256SUMS.txt"
  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$file" | awk '{print $1}')"
  elif command -v shasum >/dev/null 2>&1; then
    actual="$(shasum -a 256 "$file" | awk '{print $1}')"
  else
    fail "Neither sha256sum nor shasum is installed to verify download integrity"
  fi
  if [[ "$actual" != "$expected" ]]; then
    fail "SHA-256 mismatch for $base (expected $expected, got $actual)"
  fi
}

if [[ "$UNINSTALL" == true ]]; then
  rm -f "$INSTALL_DIR/saint-harness"
  if command -v dpkg-query >/dev/null 2>&1 && dpkg-query -W -f='${Status}' saint-harness 2>/dev/null | grep -q 'install ok installed'; then
    say "Removing Debian package saint-harness..."
    run_as_root apt-get remove -y saint-harness
  fi
  say "Removed saint-harness. Your settings in ~/.saint-harness and session data are preserved."
  exit 0
fi

if [[ -n "$LOCAL_BIN" ]]; then
  [[ -x "$LOCAL_BIN" ]] || fail "Local binary not found or not executable: $LOCAL_BIN"
  mkdir -p "$INSTALL_DIR"
  install -m 0755 "$LOCAL_BIN" "$INSTALL_DIR/saint-harness.new"
  mv -f "$INSTALL_DIR/saint-harness.new" "$INSTALL_DIR/saint-harness"
  ensure_shell_path
  say "Installed $("$INSTALL_DIR/saint-harness" --version) to $INSTALL_DIR/saint-harness"
  exit 0
fi

OS="$(uname -s)"
ARCH_RAW="$(uname -m)"

case "$ARCH_RAW" in
  x86_64|amd64) ARCH="x86_64"; DEB_ARCH="amd64" ;;
  arm64|aarch64) ARCH="arm64"; DEB_ARCH="arm64" ;;
  *) fail "Unsupported architecture: $ARCH_RAW" ;;
esac

if [[ "$VERSION" == "latest" ]]; then
  TAG="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
    | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n1)"
  [[ -n "$TAG" ]] || fail "Could not resolve latest release tag from GitHub API for ${REPO}"
else
  TAG="$VERSION"
  [[ "$TAG" == v* ]] || TAG="v$TAG"
fi
CLEAN_VER="${TAG#v}"

BASE_URL="https://github.com/${REPO}/releases/download/${TAG}"
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

say "Downloading checksums for SaintHarness ${TAG}..."
curl -fsSL "${BASE_URL}/SHA256SUMS.txt" -o "$TMP_DIR/SHA256SUMS.txt"

if [[ "$OS" == "Linux" && "$USE_DEB" == true ]]; then
  command -v dpkg >/dev/null 2>&1 || fail "--deb requires Debian or Ubuntu (dpkg not found)"
  ASSET="saint-harness_${CLEAN_VER}_${DEB_ARCH}.deb"
  say "Downloading ${ASSET}..."
  curl -fsSL "${BASE_URL}/${ASSET}" -o "$TMP_DIR/${ASSET}"
  verify_sha256 "$TMP_DIR/${ASSET}" "$TMP_DIR/SHA256SUMS.txt"
  say "Installing ${ASSET} via apt-get..."
  run_as_root apt-get install -y "$TMP_DIR/${ASSET}"
  say "Installed: $(saint-harness --version)"
  exit 0
fi

case "$OS" in
  Linux)
    if [[ "$ARCH" == "arm64" ]]; then
      ASSET="saint-harness_${CLEAN_VER}_linux-aarch64.tar.gz"
    else
      ASSET="saint-harness_${CLEAN_VER}_linux-x86_64.tar.gz"
    fi
    ;;
  Darwin)
    if [[ "$ARCH" == "arm64" ]]; then
      ASSET="saint-harness_${CLEAN_VER}_darwin-arm64.tar.gz"
    else
      ASSET="saint-harness_${CLEAN_VER}_darwin-x86_64.tar.gz"
    fi
    ;;
  *)
    fail "Unsupported OS: $OS (on Windows, use install.ps1)"
    ;;
esac

say "Downloading ${ASSET}..."
curl -fsSL "${BASE_URL}/${ASSET}" -o "$TMP_DIR/${ASSET}"
verify_sha256 "$TMP_DIR/${ASSET}" "$TMP_DIR/SHA256SUMS.txt"

tar -xzf "$TMP_DIR/${ASSET}" -C "$TMP_DIR"
BIN_SRC="$(find "$TMP_DIR" -type f -name 'saint-harness' | head -n1)"
[[ -n "$BIN_SRC" ]] || fail "Archive did not contain saint-harness binary"

mkdir -p "$INSTALL_DIR"
install -m 0755 "$BIN_SRC" "$INSTALL_DIR/saint-harness.new"
mv -f "$INSTALL_DIR/saint-harness.new" "$INSTALL_DIR/saint-harness"
ensure_shell_path
say "Installed $("$INSTALL_DIR/saint-harness" --version) to $INSTALL_DIR/saint-harness"
