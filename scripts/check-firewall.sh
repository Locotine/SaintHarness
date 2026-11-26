#!/usr/bin/env bash
set -euo pipefail

export PATH="$HOME/.cargo/bin:$PATH"
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

fail() {
  echo "[firewall] FAIL: $*" >&2
  exit 1
}

echo "[firewall] Checking Law #1 dependency firewall..."

tui_tree="$(cargo tree -p saint-harness-tui --prefix none 2>&1)" || fail "cargo tree -p saint-harness-tui failed: $tui_tree"
if grep -qE '^saint-harness-(core|server)\b' <<<"$tui_tree"; then
  fail "saint-harness-tui must NOT depend on saint-harness-core or saint-harness-server"
fi

cli_tui_tree="$(cargo tree -p saint-harness-cli --no-default-features --features tui --prefix none 2>&1)" || fail "cargo tree -p saint-harness-cli (tui-only) failed: $cli_tui_tree"
if grep -qE '^saint-harness-(core|server)\b' <<<"$cli_tui_tree"; then
  fail "saint-harness-cli (--no-default-features --features tui) must NOT depend on saint-harness-core or saint-harness-server"
fi

core_tree="$(cargo tree -p saint-harness-core --prefix none 2>&1)" || fail "cargo tree -p saint-harness-core failed: $core_tree"
for forbidden in tree-sitter ratatui rquickjs tantivy ast-grep-core oxc_parser oxc_transformer oxc_codegen; do
  if grep -qE "^${forbidden}\b" <<<"$core_tree"; then
    fail "saint-harness-core must NOT depend directly on heavy leaf crate '${forbidden}'"
  fi
done

proto_tree="$(cargo tree -p saint-harness-protocol --prefix none 2>&1)" || fail "cargo tree -p saint-harness-protocol failed: $proto_tree"
if grep -qE '^saint-harness-(core|server|ai|token|edit|treesitter|lsp|shell|codemode|index)\b' <<<"$proto_tree"; then
  fail "saint-harness-protocol must have zero dependency on core/server/leaf crates"
fi

ts_versions="$(cargo tree -d --prefix none 2>/dev/null | grep -E '^tree-sitter v' | sort -u || true)"
if [[ -n "$ts_versions" ]]; then
  count="$(wc -l <<<"$ts_versions")"
  if [[ "$count" -gt 1 ]]; then
    fail "Duplicate tree-sitter versions detected in Cargo.lock:\n$ts_versions"
  fi
fi

echo "[firewall] PASS: all dependency firewall & single tree-sitter ABI invariants hold."
