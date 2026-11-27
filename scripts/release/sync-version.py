#!/usr/bin/env python3
"""Synchronize and validate SaintHarness release versions across Cargo, npm, and Tauri Desktop."""

import argparse
import json
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


def read_package_version(cargo_toml: Path) -> str:
    text = cargo_toml.read_text(encoding="utf-8")
    match = re.search(r'(\[package\][\s\S]*?\nversion\s*=\s*)"([^"]+)"', text)
    if not match:
        raise RuntimeError(f"Could not locate [package].version in {cargo_toml}")
    return match.group(2)


def _sync_json_version(path: Path, normalized: str) -> None:
    if not path.is_file():
        return
    data = json.loads(path.read_text(encoding="utf-8"))
    data["version"] = normalized
    path.write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")


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

    # Sync npm CLI wrapper and Tauri v2 Desktop manifests when present.
    _sync_json_version(root / "packages/npm-cli/package.json", normalized)
    _sync_json_version(root / "apps/desktop/package.json", normalized)
    _sync_json_version(root / "apps/desktop/src-tauri/tauri.conf.json", normalized)

    desktop_cargo = root / "apps/desktop/src-tauri/Cargo.toml"
    if desktop_cargo.is_file():
        d_text = desktop_cargo.read_text(encoding="utf-8")
        d_updated, d_count = re.subn(
            r'(\[package\][\s\S]*?\nversion\s*=\s*)"[^"]+"',
            rf'\1"{normalized}"',
            d_text,
            count=1,
        )
        if d_count != 1:
            raise RuntimeError(f"Failed to update [package].version in {desktop_cargo}")
        desktop_cargo.write_text(d_updated, encoding="utf-8")

    return normalized


def verify_all_versions(expected: str, root: Path = ROOT) -> str:
    clean = normalize_release_version(expected)
    checks: list[tuple[str, str]] = [
        ("Cargo.toml", read_workspace_version(root / "Cargo.toml")),
    ]
    for rel in [
        "packages/npm-cli/package.json",
        "apps/desktop/package.json",
        "apps/desktop/src-tauri/tauri.conf.json",
    ]:
        p = root / rel
        if p.is_file():
            checks.append((rel, str(json.loads(p.read_text(encoding="utf-8")).get("version", ""))))
    desktop_cargo = root / "apps/desktop/src-tauri/Cargo.toml"
    if desktop_cargo.is_file():
        checks.append(("apps/desktop/src-tauri/Cargo.toml", read_package_version(desktop_cargo)))

    mismatches = [f"{name}={actual}" for name, actual in checks if actual != clean]
    if mismatches:
        raise ValueError(
            f"Version mismatch (expected {clean}): " + ", ".join(mismatches)
        )
    return clean


def main() -> int:
    parser = argparse.ArgumentParser(description="Validate or sync SaintHarness release version.")
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--normalize", help="Validate and print clean X.Y.Z version")
    group.add_argument("--sync", help="Write clean X.Y.Z version to Cargo, npm, and Tauri manifests")
    group.add_argument("--verify", help="Verify all Cargo, npm, and Tauri manifests match X.Y.Z version")
    parser.add_argument("--root", type=Path, default=ROOT, help="Workspace root override")
    args = parser.parse_args()

    try:
        if args.normalize:
            sys.stdout.write(normalize_release_version(args.normalize))
            return 0
        if args.sync:
            clean = sync_workspace_version(args.sync, args.root)
            print(f"[sync-version] Synchronized workspace + desktop versions to {clean}")
            return 0
        if args.verify:
            clean = verify_all_versions(args.verify, args.root)
            print(f"[sync-version] Verified workspace + desktop version {clean}")
            return 0
    except Exception as exc:
        print(f"[sync-version] ERROR: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
