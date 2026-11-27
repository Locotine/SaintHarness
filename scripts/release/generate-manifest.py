#!/usr/bin/env python3
"""Generate SHA256SUMS.txt and release-manifest.json for SaintHarness CLI & Desktop releases."""

import argparse
import datetime as dt
import hashlib
import importlib.util
import json
import sys
from pathlib import Path

_THIS_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(_THIS_DIR))

_sync_spec = importlib.util.spec_from_file_location("sync_version", _THIS_DIR / "sync-version.py")
assert _sync_spec and _sync_spec.loader
_sync_mod = importlib.util.module_from_spec(_sync_spec)
_sync_spec.loader.exec_module(_sync_mod)
normalize_release_version = _sync_mod.normalize_release_version


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def expected_platform_files(version: str) -> dict[str, str]:
    return {
        "linux-x86_64": f"saint-harness_{version}_linux-x86_64.tar.gz",
        "linux-x86_64-deb": f"saint-harness_{version}_amd64.deb",
        "linux-aarch64": f"saint-harness_{version}_linux-aarch64.tar.gz",
        "linux-aarch64-deb": f"saint-harness_{version}_arm64.deb",
        "darwin-aarch64": f"saint-harness_{version}_darwin-arm64.tar.gz",
        "darwin-x86_64": f"saint-harness_{version}_darwin-x86_64.tar.gz",
        "windows-x86_64": f"saint-harness_{version}_windows-x86_64.zip",
    }


def expected_desktop_files(version: str) -> dict[str, str]:
    return {
        "linux-x86_64-deb": f"saint-harness-desktop_{version}_amd64.deb",
        "darwin-aarch64-dmg": f"saint-harness-desktop_{version}_aarch64.dmg",
        "darwin-x86_64-dmg": f"saint-harness-desktop_{version}_x64.dmg",
        "windows-x86_64-exe": f"saint-harness-desktop_{version}_x64-setup.exe",
        "windows-x86_64-msi": f"saint-harness-desktop_{version}_x64.msi",
    }


def generate_release_outputs(
    artifacts_dir: Path,
    version: str,
    tag: str,
    repo: str,
    commit: str,
    out_dir: Path,
    allow_partial: bool = False,
    require_desktop: bool = False,
) -> dict:
    clean_version = normalize_release_version(version)
    expected = expected_platform_files(clean_version)
    expected_desktop = expected_desktop_files(clean_version)
    download_base = f"https://github.com/{repo}/releases/download/{tag}"

    platforms: dict[str, dict] = {}
    missing: list[str] = []

    for plat_key, filename in expected.items():
        candidate = artifacts_dir / filename
        if not candidate.is_file() or candidate.stat().st_size == 0:
            missing.append(f"{plat_key} ({filename})")
            continue
        platforms[plat_key] = {
            "filename": filename,
            "url": f"{download_base}/{filename}",
            "sha256": sha256_file(candidate),
            "size": candidate.stat().st_size,
        }

    if missing and not allow_partial:
        raise RuntimeError(
            "Missing required CLI release artifacts:\n  - " + "\n  - ".join(missing)
        )

    desktop: dict[str, dict] = {}
    missing_desktop: list[str] = []
    for desk_key, filename in expected_desktop.items():
        candidate = artifacts_dir / filename
        if not candidate.is_file() or candidate.stat().st_size == 0:
            missing_desktop.append(f"{desk_key} ({filename})")
            continue
        desktop[desk_key] = {
            "filename": filename,
            "url": f"{download_base}/{filename}",
            "sha256": sha256_file(candidate),
            "size": candidate.stat().st_size,
        }

    if require_desktop and missing_desktop and not allow_partial:
        raise RuntimeError(
            "Missing required Desktop release artifacts:\n  - "
            + "\n  - ".join(missing_desktop)
        )

    out_dir.mkdir(parents=True, exist_ok=True)

    # Write deterministic SHA256SUMS.txt covering all non-manifest files in artifacts_dir.
    checksum_lines: list[str] = []
    for file_path in sorted(artifacts_dir.iterdir(), key=lambda p: p.name):
        if not file_path.is_file():
            continue
        if file_path.name in {"SHA256SUMS.txt", "release-manifest.json"}:
            continue
        digest = sha256_file(file_path)
        checksum_lines.append(f"{digest}  {file_path.name}")

    sums_path = out_dir / "SHA256SUMS.txt"
    sums_path.write_text("\n".join(checksum_lines) + "\n", encoding="utf-8")

    manifest = {
        "schemaVersion": 1,
        "name": "saint-harness",
        "version": clean_version,
        "tag": tag,
        "commit": commit,
        "pub_date": dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "checksums_url": f"{download_base}/SHA256SUMS.txt",
        "platforms": platforms,
    }
    if desktop:
        manifest["desktop"] = desktop

    manifest_path = out_dir / "release-manifest.json"
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    return manifest


def main() -> int:
    parser = argparse.ArgumentParser(description="Generate SaintHarness release manifest and checksums.")
    parser.add_argument("--artifacts-dir", type=Path, required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--repo", required=True)
    parser.add_argument("--commit", required=True)
    parser.add_argument("--out-dir", type=Path, required=True)
    parser.add_argument("--allow-partial", action="store_true")
    parser.add_argument("--require-desktop", action="store_true")
    args = parser.parse_args()

    try:
        manifest = generate_release_outputs(
            artifacts_dir=args.artifacts_dir,
            version=args.version,
            tag=args.tag,
            repo=args.repo,
            commit=args.commit,
            out_dir=args.out_dir,
            allow_partial=args.allow_partial,
            require_desktop=args.require_desktop,
        )
        desk_count = len(manifest.get("desktop", {}))
        print(
            f"[generate-manifest] Wrote release-manifest.json ({len(manifest['platforms'])} CLI, {desk_count} Desktop) and SHA256SUMS.txt"
        )
        return 0
    except Exception as exc:
        print(f"[generate-manifest] ERROR: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
