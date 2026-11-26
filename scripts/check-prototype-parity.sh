#!/usr/bin/env bash
set -euo pipefail

export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}"
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR"

PYTHON_BIN="python3"
if ! command -v "$PYTHON_BIN" >/dev/null 2>&1; then
  PYTHON_BIN="python"
fi

echo "[parity] Checking 100% UI parity with prototypes/saint-harness-cli-prototype.html & prototypes/tui-parity-contract.json..."

# Gate 1: Verify HTML prototype CSS tokens <-> tui-parity-contract.json (all 56 RGB tokens + WCAG AA contrast >= 4.5:1)
"$PYTHON_BIN" - <<'PYEOF'
import json, re, sys
from pathlib import Path

root = Path(".")
contract = json.loads((root / "prototypes/tui-parity-contract.json").read_text(encoding="utf-8"))
html = (root / "prototypes/saint-harness-cli-prototype.html").read_text(encoding="utf-8")

def strip_css_comments(text: str) -> str:
    return re.sub(r"/\*.*?\*/", "", text, flags=re.S)

style_match = re.search(r"<style>(.*?)</style>", html, flags=re.S)
assert style_match, "Missing <style> block in prototype HTML"
css = strip_css_comments(style_match.group(1))

root_block = re.search(r":root\s*\{([^}]*)\}", css, flags=re.S)
dark_block = re.search(r"html\.dark\s*\{([^}]*)\}", css, flags=re.S)
assert root_block and dark_block, "Missing :root or html.dark CSS blocks"

def parse_vars(block: str) -> dict:
    out = {}
    for m in re.finditer(r"--([a-z0-9-]+)\s*:\s*([^;]+);", block):
        out[m.group(1).strip()] = m.group(2).strip()
    return out

def parse_color(val: str, bg: tuple[int, int, int]) -> tuple[int, int, int]:
    val = val.strip()
    if val.startswith("#"):
        h = val[1:]
        return (int(h[0:2], 16), int(h[2:4], 16), int(h[4:6], 16))
    m = re.match(r"rgba?\(\s*([0-9.]+)\s*,\s*([0-9.]+)\s*,\s*([0-9.]+)(?:\s*,\s*([0-9.]+))?\s*\)", val)
    assert m, f"Unsupported CSS color: {val}"
    r, g, b = float(m.group(1)), float(m.group(2)), float(m.group(3))
    a = float(m.group(4)) if m.group(4) is not None else 1.0
    return (
        round(r * a + bg[0] * (1.0 - a)),
        round(g * a + bg[1] * (1.0 - a)),
        round(b * a + bg[2] * (1.0 - a)),
    )

CSS_TO_CONTRACT = {
    "surface": "surface",
    "surface-under": "surfaceUnder",
    "sidebar-bg": "sidebarBg",
    "panel": "panel",
    "control-bg": "controlBg",
    "elevated-bg": "elevatedBg",
    "user-bubble": "userBubble",
    "ink": "inkPrimary",
    "ink-secondary": "inkSecondary",
    "ink-tertiary": "inkTertiary",
    "border": "border",
    "border-heavy": "borderHeavy",
    "divider": "dividerHairline",
    "accent": "accent",
    "text-accent": "textAccent",
    "accent-bg": "accentBg",
    "full-access": "fullAccess",
    "full-access-ink": "fullAccessInk",
    "status-done": "statusDone",
    "status-done-ink": "statusDoneInk",
    "status-blocked": "statusBlocked",
    "status-blocked-ink": "statusBlockedInk",
    "status-working": "statusWorking",
    "status-working-ink": "statusWorkingInk",
    "status-error": "statusError",
    "status-error-ink": "statusErrorInk",
    "diff-added-bg": "diffAddedBg",
    "diff-removed-bg": "diffRemovedBg",
}

def rel_lum(rgb):
    def chan(c):
        s = c / 255.0
        return s / 12.92 if s <= 0.03928 else ((s + 0.055) / 1.055) ** 2.4
    return 0.2126 * chan(rgb[0]) + 0.7152 * chan(rgb[1]) + 0.0722 * chan(rgb[2])

def contrast(fg, bg):
    l1, l2 = rel_lum(fg), rel_lum(bg)
    hi, lo = max(l1, l2), min(l1, l2)
    return (hi + 0.05) / (lo + 0.05)

errors = []
for theme_name, css_block in [("light", root_block.group(1)), ("dark", dark_block.group(1))]:
    vars_map = parse_vars(css_block)
    surface_rgb = parse_color(vars_map["surface"], (0, 0, 0))
    contract_theme = contract["theme"][theme_name]
    for css_k, contract_k in CSS_TO_CONTRACT.items():
        actual = list(parse_color(vars_map[css_k], surface_rgb))
        expected = contract_theme[contract_k]
        if actual != expected:
            errors.append(f"{theme_name}.{contract_k}: prototype CSS={actual} != contract={expected}")
    # Check WCAG AA >= 4.5:1 for all text inks across key surfaces
    min_ratio = contract["theme"]["minTextContrastRatio"]
    surfaces = ["surface", "sidebarBg", "panel", "controlBg", "elevatedBg", "userBubble"]
    inks = ["inkPrimary", "inkSecondary", "inkTertiary", "textAccent", "fullAccessInk", "statusDoneInk", "statusBlockedInk", "statusWorkingInk", "statusErrorInk"]
    for sk in surfaces:
        bg = contract_theme[sk]
        for ik in inks:
            fg = contract_theme[ik]
            r = contrast(fg, bg)
            if r < min_ratio:
                errors.append(f"WCAG AA fail {theme_name} {ik} on {sk}: {r:.2f} < {min_ratio}")

# Verify anti-slop invariants in prototype HTML and spec
if "\u2014" in html:
    errors.append("Prototype HTML contains forbidden em dash (U+2014)")

if errors:
    for e in errors:
        print(f"[parity] FAIL: {e}", file=sys.stderr)
    sys.exit(1)

print("[parity] OK: 56/56 prototype CSS tokens match tui-parity-contract.json and pass WCAG AA >= 4.5:1.")
PYEOF

# Gate 2: Static anti-slop & single-source theme guard on crates/saint-harness-tui/src
if [[ -d crates/saint-harness-tui/src ]]; then
  "$PYTHON_BIN" - <<'PYEOF'
import re, sys
from pathlib import Path

tui_src = Path("crates/saint-harness-tui/src")
errors = []

for path in sorted(tui_src.rglob("*.rs")):
    text = path.read_text(encoding="utf-8")
    # Zero em dash in UI code/strings
    if "\u2014" in text:
        errors.append(f"{path}: contains forbidden em dash U+2014")
    # Zero hardcoded Color::Rgb outside theme.rs
    if path.name != "theme.rs" and re.search(r"Color::Rgb\s*\(", text):
        errors.append(f"{path}: hardcoded Color::Rgb(...) outside theme.rs")
    # Zero decorative emoji in UI strings
    if re.search(r"[\U0001F300-\U0001FAFF]", text):
        errors.append(f"{path}: contains forbidden decorative emoji")
    # Zero fake window traffic light strings
    if re.search(r"●\s*●\s*●|traffic.?light", text, flags=re.I):
        errors.append(f"{path}: contains forbidden fake window traffic lights")

if errors:
    for e in errors:
        print(f"[parity] FAIL: {e}", file=sys.stderr)
    sys.exit(1)

print("[parity] OK: saint-harness-tui static anti-slop & centralized theme checks pass.")
PYEOF

  # Gate 3: Run ratatui::backend::TestBackend prototype parity test suite (all 5 breakpoints, dark/light/no-color, all invariants)
  echo "[parity] Running ratatui TestBackend prototype parity suite..."
  cargo test -p saint-harness-tui --test prototype_parity -- --nocapture
else
  echo "[parity] FAIL: crates/saint-harness-tui/src not found" >&2
  exit 1
fi

echo "[parity] PASS: 100% UI parity with prototype verified."
