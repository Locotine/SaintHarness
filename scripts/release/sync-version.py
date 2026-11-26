#!/usr/bin/env python3
"""Synchronize and validate SaintHarness release versions across Cargo.toml."""

import argparse
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SEMVER_RE = re.compile(r"^\d+\.\d+\.\d+$")


def normalize_release_version(raw: str) -> str:
    cleaned = re.sub(r"^refs/tags/", "", raw.strip())
    cleaned = re.sub(r"^v", "", cleaned)
    if not SEMVER_RE.match(cleaned):
        raise ValueError(
            f"Invalid stable release version (expected X.Y.Z or vX.Y.Z): {raw!r}"
        )
    return cleaned


def read_workspace_version(cargo_toml: Path) -> str:
    text = cargo_toml.read_text(encoding="utf-8")
    match = re.search(
        r'(\[workspace\.package\][\s\S]*?\nversion\s*=\s*)"([^"]+)"', text
    )
    if not match:
        raise RuntimeError(f"Could not locate [workspace.package].version in {cargo_toml}")
    return match.group(2)


def sync_workspace_version(version: str, root: Path = ROOT) -> str:
    normalized = normalize_release_version(version)
    cargo_toml = root / "Cargo.toml"
    text = cargo_toml.read_text(encoding="utf-8")
    updated, count = re.subn(
        r'(\[workspace\.package\][\s\S]*?\nversion\s*=\s*)"[^"]+"',
        rf'\1"{normalized}"',
        text,
        count=1,
    )
    if count != 1:
        raise RuntimeError(f"Failed to update [workspace.package].version in {cargo_toml}")
    cargo_toml.write_text(updated, encoding="utf-8")

    npm_pkg = root / "packages/npm-cli/package.json"
    if npm_pkg.is_file():
        import json

        data = json.loads(npm_pkg.read_text(encoding="utf-8"))
        data["version"] = normalized
        npm_pkg.write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")

    return normalized


def main() -> int:
    parser = argparse.ArgumentParser(description="Validate or sync SaintHarness release version.")
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--normalize", help="Validate and print clean X.Y.Z version")
    group.add_argument("--sync", help="Write clean X.Y.Z version to root Cargo.toml")
    group.add_argument("--verify", help="Verify root Cargo.toml matches X.Y.Z version")
    parser.add_argument("--root", type=Path, default=ROOT, help="Workspace root override")
    args = parser.parse_args()

    try:
        if args.normalize:
            sys.stdout.write(normalize_release_version(args.normalize))
            return 0
        if args.sync:
            clean = sync_workspace_version(args.sync, args.root)
            print(f"[sync-version] Synchronized workspace version to {clean}")
            return 0
        if args.verify:
            expected = normalize_release_version(args.verify)
            actual = read_workspace_version(args.root / "Cargo.toml")
            if actual != expected:
                raise ValueError(
                    f"Cargo.toml version ({actual}) does not match release version ({expected})"
                )
            print(f"[sync-version] Verified workspace version {actual}")
            return 0
    except Exception as exc:
        print(f"[sync-version] ERROR: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
