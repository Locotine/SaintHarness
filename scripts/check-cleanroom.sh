#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

fail=0

# Law #7 Clean-Room: forbid external agent/harness project names and config paths in source/profiles/prompts.
SCAN_DIRS=()
for d in crates profiles prompts; do
  [[ -d "$d" ]] && SCAN_DIRS+=("$d")
done

if [[ ${#SCAN_DIRS[@]} -eq 0 ]]; then
  echo "[cleanroom] No source directories to scan yet; PASS."
  exit 0
fi

# External harness / coding-agent brand identifiers (encoded as split tokens to stay self-clean).
FORBIDDEN_PATTERNS=(
  'open''code'
  'claude''-code'
  'cursor''rules'
  '\.cursor/'
  'wind''surf'
  'aider''\.chat'
  'cline''rules'
  'roo''code'
  'amp''code'
  'codex''-cli'
  'gemini''-cli'
)

for pat in "${FORBIDDEN_PATTERNS[@]}"; do
  if grep -rnEi "$pat" "${SCAN_DIRS[@]}" >/dev/null 2>&1; then
    echo "[cleanroom] FAIL: forbidden external reference matched /$pat/:" >&2
    grep -rnEi "$pat" "${SCAN_DIRS[@]}" | head -n 10 >&2
    fail=1
  fi
done

# Law #2 Zero hardcoded Claude/Gemini version numbers or beta dates in Rust source, profiles, or prompts.
# Only dynamic selectors (latest_flagship, latest_fast, latest_family:*) belong in code; test fixtures use synthetic IDs.
MODEL_VERSION_PATTERNS=(
  'claude-(3|3-5|3\.5|3-7|3\.7|4|4-0|4\.0|4-5|4\.5|4-6|4\.6|sonnet-4|opus-4|haiku-4)'
  'gemini-(1\.5|2\.0|2\.5|3\.0|3\.5)'
  'anthropic-beta.*202[4-9]-[0-9]{2}-[0-9]{2}'
)

RS_FILES=()
while IFS= read -r -d '' f; do
  # Exclude test-only files that feed synthetic future catalog fixtures.
  if [[ "$f" == */tests/* ]]; then
    continue
  fi
  RS_FILES+=("$f")
done < <(find "${SCAN_DIRS[@]}" -type f \( -name '*.rs' -o -name '*.toml' -o -name '*.txt' \) -print0)

if [[ ${#RS_FILES[@]} -gt 0 ]]; then
  for pat in "${MODEL_VERSION_PATTERNS[@]}"; do
    if grep -nE "$pat" "${RS_FILES[@]}" >/dev/null 2>&1; then
      echo "[cleanroom] FAIL: hardcoded provider model version or beta date matched /$pat/:" >&2
      grep -nE "$pat" "${RS_FILES[@]}" | head -n 10 >&2
      fail=1
    fi
  done
fi

if [[ "$fail" -ne 0 ]]; then
  exit 1
fi

echo "[cleanroom] PASS: zero external references and zero hardcoded provider model versions."
