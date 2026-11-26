#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

chmod +x scripts/check-firewall.sh scripts/check-cleanroom.sh scripts/check-prototype-parity.sh scripts/setup-dev.sh scripts/install-hooks.sh .githooks/pre-commit

git config core.hooksPath .githooks
cp .githooks/pre-commit .git/hooks/pre-commit
chmod +x .git/hooks/pre-commit

echo "[hooks] Installed pre-commit hook (cleanroom + firewall + 100% UI prototype parity) in .githooks/pre-commit and .git/hooks/pre-commit."
