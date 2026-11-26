#!/usr/bin/env bash
set -euo pipefail

export PATH="$HOME/.cargo/bin:$PATH"
echo "rustc: $(rustc --version)"
echo "cargo: $(cargo --version)"

if ! command -v cargo-nextest >/dev/null 2>&1; then
  echo "Note: cargo-nextest is not installed; Makefile falls back to cargo test."
  echo "Install optionally with: cargo install cargo-nextest --locked"
fi

if command -v clang >/dev/null 2>&1 && command -v mold >/dev/null 2>&1; then
  echo "clang + mold available; you may copy .cargo/config.toml.mold to .cargo/config.toml"
fi

./scripts/install-hooks.sh
