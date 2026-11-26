#!/usr/bin/env python3
"""Generate Homebrew Formula, Scoop manifest, and WinGet manifests from release-manifest.json."""

import argparse
import json
import sys
from pathlib import Path


def render_homebrew_formula(manifest: dict, repo: str) -> str:
    ver = manifest["version"]
    p = manifest["platforms"]
    mac_arm = p["darwin-aarch64"]
    mac_x64 = p["darwin-x86_64"]
    lin_arm = p["linux-aarch64"]
    lin_x64 = p["linux-x86_64"]
    return f'''class SaintHarness < Formula
  desc "Pure Rust Coding Agent Harness (TUI, HTTP/SSE daemon, ACP IDE bridge)"
  homepage "https://github.com/{repo}"
  version "{ver}"
  license "MIT"

  on_macos do
    on_arm do
      url "{mac_arm["url"]}"
      sha256 "{mac_arm["sha256"]}"
    end
    on_intel do
      url "{mac_x64["url"]}"
      sha256 "{mac_x64["sha256"]}"
    end
  end

  on_linux do
    on_arm do
      url "{lin_arm["url"]}"
      sha256 "{lin_arm["sha256"]}"
    end
    on_intel do
      url "{lin_x64["url"]}"
      sha256 "{lin_x64["sha256"]}"
    end
  end

  def install
    bin.install "saint-harness"
  end

  test do
    assert_match "saint-harness {ver}", shell_output("#{{bin}}/saint-harness --version")
  end
end
'''


def render_scoop_manifest(manifest: dict, repo: str) -> dict:
    ver = manifest["version"]
    win = manifest["platforms"]["windows-x86_64"]
    return {
        "version": ver,
        "description": "Pure Rust Coding Agent Harness (TUI, HTTP/SSE daemon, ACP IDE bridge)",
        "homepage": f"https://github.com/{repo}",
        "license": "MIT",
        "architecture": {
            "64bit": {
                "url": win["url"],
                "hash": win["sha256"],
            }
        },
        "bin": "saint-harness.exe",
        "checkver": {"github": f"https://github.com/{repo}"},
        "autoupdate": {
            "architecture": {
                "64bit": {
                    "url": f"https://github.com/{repo}/releases/download/v$version/saint-harness_$version_windows-x86_64.zip"
                }
            }
        },
    }


def write_winget_manifests(manifest: dict, repo: str, out_dir: Path) -> None:
    ver = manifest["version"]
    win = manifest["platforms"]["windows-x86_64"]
    winget_dir = out_dir / "winget"
    winget_dir.mkdir(parents=True, exist_ok=True)

    (winget_dir / "SaintHarness.SaintHarness.yaml").write_text(
        f"""PackageIdentifier: SaintHarness.SaintHarness
PackageVersion: {ver}
DefaultLocale: en-US
ManifestType: version
ManifestVersion: 1.6.0
""",
        encoding="utf-8",
    )

    (winget_dir / "SaintHarness.SaintHarness.installer.yaml").write_text(
        f"""PackageIdentifier: SaintHarness.SaintHarness
PackageVersion: {ver}
InstallerType: zip
NestedInstallerType: portable
NestedInstallerFiles:
  - RelativeFilePath: saint-harness.exe
    PortableCommandAlias: saint-harness
Installers:
  - Architecture: x64
    InstallerUrl: {win["url"]}
    InstallerSha256: {win["sha256"].upper()}
ManifestType: installer
ManifestVersion: 1.6.0
""",
        encoding="utf-8",
    )

    (winget_dir / "SaintHarness.SaintHarness.locale.en-US.yaml").write_text(
        f"""PackageIdentifier: SaintHarness.SaintHarness
PackageVersion: {ver}
PackageLocale: en-US
Publisher: SaintHarness
PublisherUrl: https://github.com/{repo}
PackageName: SaintHarness
PackageUrl: https://github.com/{repo}
License: MIT
ShortDescription: Pure Rust Coding Agent Harness (TUI, daemon, ACP bridge)
ManifestType: defaultLocale
ManifestVersion: 1.6.0
""",
        encoding="utf-8",
    )


def generate_all(manifest_path: Path, repo: str, out_dir: Path) -> None:
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    out_dir.mkdir(parents=True, exist_ok=True)

    formula = render_homebrew_formula(manifest, repo)
    (out_dir / "saint-harness.rb").write_text(formula, encoding="utf-8")

    scoop = render_scoop_manifest(manifest, repo)
    (out_dir / "saint-harness.scoop.json").write_text(
        json.dumps(scoop, indent=2) + "\n", encoding="utf-8"
    )

    write_winget_manifests(manifest, repo, out_dir)


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Generate Homebrew, Scoop, and WinGet package manager manifests."
    )
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--repo", required=True)
    parser.add_argument("--out-dir", type=Path, required=True)
    args = parser.parse_args()

    try:
        generate_all(args.manifest, args.repo, args.out_dir)
        print(
            f"[generate-package-manifests] Wrote saint-harness.rb, saint-harness.scoop.json, and winget/ to {args.out_dir}"
        )
        return 0
    except Exception as exc:
        print(f"[generate-package-manifests] ERROR: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
