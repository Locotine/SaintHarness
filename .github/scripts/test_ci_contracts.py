#!/usr/bin/env python3
"""Executable contract tests for SaintHarness CI and Release workflows.

Modeled on saint-hyper-agent/.github/scripts/ci-contracts.test.mjs:
executes the actual aggregate gate shell block from ci.yml across every
pass/fail/cancel/skip/docs-only state, validates cross-platform matrix coverage
(macOS, Linux Debian, Linux Ubuntu, Windows), and unit-tests release helpers.
"""

import importlib.util
import json
import os
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
CI_YML = ROOT / ".github/workflows/ci.yml"
NIGHTLY_YML = ROOT / ".github/workflows/ci-nightly.yml"
RELEASE_YML = ROOT / ".github/workflows/release.yml"
SETUP_ACTION = ROOT / ".github/actions/setup-rust/action.yml"
BUILD_DEB = ROOT / "scripts/release/build-deb.sh"


def load_module(name: str, path: Path):
    spec = importlib.util.spec_from_file_location(name, path)
    assert spec and spec.loader
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


def extract_aggregate_script(ci_text: str) -> str:
    match = re.search(
        r"- name: Aggregate lane results[\s\S]*?        run: \|\n([\s\S]*?)(?=\n  [a-z_]+:|\Z)",
        ci_text,
    )
    assert match, "Could not locate Aggregate lane results shell block in ci.yml"
    return "\n".join(line[10:] if line.startswith("          ") else line for line in match.group(1).splitlines())


class TestCiContracts(unittest.TestCase):
    @classmethod
    def setUpClass(cls) -> None:
        cls.ci_text = CI_YML.read_text(encoding="utf-8")
        cls.nightly_text = NIGHTLY_YML.read_text(encoding="utf-8")
        cls.release_text = RELEASE_YML.read_text(encoding="utf-8")
        cls.setup_text = SETUP_ACTION.read_text(encoding="utf-8")
        cls.build_deb_text = BUILD_DEB.read_text(encoding="utf-8")
        cls.gate_script = extract_aggregate_script(cls.ci_text)
        cls.lanes = ["TEST_UBUNTU", "TEST_DEBIAN", "TEST_MACOS", "TEST_WINDOWS"]

    def run_gate(self, **overrides: str) -> int:
        env = dict(os.environ)
        env.update(
            {
                "CHANGES": "success",
                "CODE": "true",
                "STATIC_FAST": "success",
                **{lane: "success" for lane in self.lanes},
            }
        )
        env.update(overrides)
        res = subprocess.run(
            ["bash", "-e", "-c", self.gate_script],
            env=env,
            capture_output=True,
            text=True,
            check=False,
        )
        return res.returncode

    def test_yaml_parses_cleanly_if_pyyaml_present(self) -> None:
        try:
            import yaml  # type: ignore
        except ImportError:
            return
        for wf in [CI_YML, NIGHTLY_YML, RELEASE_YML, SETUP_ACTION]:
            parsed = yaml.safe_load(wf.read_text(encoding="utf-8"))
            self.assertIsInstance(parsed, dict, f"{wf} failed to parse as YAML mapping")

    def test_aggregate_gate_passes_full_and_docs_only_runs(self) -> None:
        self.assertEqual(self.run_gate(), 0)
        docs_overrides = {"CODE": "false", **{lane: "skipped" for lane in self.lanes}}
        self.assertEqual(self.run_gate(**docs_overrides), 0)

    def test_aggregate_gate_fails_closed_on_any_heavy_lane_issue(self) -> None:
        for lane in self.lanes:
            for bad in ["failure", "cancelled", "skipped", ""]:
                self.assertNotEqual(
                    self.run_gate(**{lane: bad}),
                    0,
                    f"{lane}={bad!r} must fail the aggregate gate",
                )

    def test_aggregate_gate_fails_closed_on_planning_or_static_issues(self) -> None:
        for bad in ["failure", "cancelled", "skipped", ""]:
            self.assertNotEqual(self.run_gate(CHANGES=bad), 0)
            self.assertNotEqual(self.run_gate(STATIC_FAST=bad), 0)
        for bad_code in ["", "undefined", "TRUE"]:
            self.assertNotEqual(self.run_gate(CODE=bad_code), 0)

    def test_docs_only_cannot_hide_executed_or_failed_heavy_lane(self) -> None:
        docs_base = {"CODE": "false", **{lane: "skipped" for lane in self.lanes}}
        for lane in self.lanes:
            for status in ["success", "failure", "cancelled", ""]:
                self.assertNotEqual(
                    self.run_gate(**{**docs_base, lane: status}),
                    0,
                    f"docs-only with {lane}={status!r} must fail closed",
                )

    def test_ci_required_check_independent_static_and_four_os_coverage(self) -> None:
        self.assertIn(
            "name: Format, Lint, Firewall, Parity, Cross-Platform Test",
            self.ci_text,
        )
        static_section = self.ci_text.split("  static-fast:\n", 1)[1].split("  test-ubuntu:\n", 1)[0]
        self.assertNotIn("needs:", static_section)
        self.assertNotIn("continue-on-error", self.ci_text)

        # All 4 platforms requested: Ubuntu, Debian, macOS, Windows
        for token in [
            "ubuntu-24.04",
            "ubuntu-22.04",
            "rust:1-bookworm",
            "macos-15",
            "windows-2022",
            "scripts/check-cleanroom.sh",
            "scripts/check-firewall.sh",
            "scripts/check-prototype-parity.sh",
            "cargo clippy --workspace --all-targets -- -D warnings",
        ]:
            self.assertIn(token, self.ci_text, f"ci.yml missing required coverage: {token}")

    def test_release_matrix_glibc_baseline_and_debian_ubuntu_verification(self) -> None:
        for token in [
            '"id":"linux-x64"',
            '"id":"linux-arm64"',
            '"id":"mac-arm64"',
            '"id":"mac-x64"',
            '"id":"win-x64"',
            '"runner":"ubuntu-22.04"',
            '"runner":"ubuntu-22.04-arm"',
            '"runner":"macos-15"',
            '"runner":"windows-2022"',
            "debian:bookworm",
            "debian:trixie",
            "ubuntu:22.04",
            "ubuntu:24.04",
            "scripts/release/build-deb.sh",
            "scripts/release/generate-manifest.py",
            "scripts/release/generate-package-manifests.py",
            "scripts/install.sh",
            "scripts/install.ps1",
        ]:
            self.assertIn(token, self.release_text, f"release.yml missing required item: {token}")

        install_sh = (ROOT / "scripts/install.sh").read_text(encoding="utf-8")
        self.assertIn("# SaintHarness PATH", install_sh)
        self.assertIn("--local", install_sh)
        self.assertIn("--no-modify-path", install_sh)

        # Ensure Debian/Ubuntu .deb guard enforces non-t64 Depends and GLIBC <= 2.35
        self.assertIn("*t64*", self.build_deb_text)
        self.assertIn('MAX_GLIBC="2.35"', self.build_deb_text)
        self.assertIn("readelf -V", self.build_deb_text)

    def test_generate_package_manifests_writes_homebrew_scoop_and_winget(self) -> None:
        manifest_mod = load_module("generate_manifest_pkg", ROOT / "scripts/release/generate-manifest.py")
        pkg_mod = load_module("generate_pkg", ROOT / "scripts/release/generate-package-manifests.py")
        with tempfile.TemporaryDirectory() as tmp:
            art = Path(tmp) / "art"
            art.mkdir()
            for fname in manifest_mod.expected_platform_files("0.2.0").values():
                (art / fname).write_bytes(f"pkg:{fname}\n".encode("utf-8"))
            manifest_mod.generate_release_outputs(
                artifacts_dir=art,
                version="0.2.0",
                tag="v0.2.0",
                repo="Locotine/SaintHarness",
                commit="cafebabe",
                out_dir=art,
            )
            pkg_mod.generate_all(art / "release-manifest.json", "Locotine/SaintHarness", art)
            formula = (art / "saint-harness.rb").read_text(encoding="utf-8")
            self.assertIn("class SaintHarness < Formula", formula)
            self.assertIn('version "0.2.0"', formula)
            self.assertIn("darwin-arm64.tar.gz", formula)
            self.assertIn("linux-x86_64.tar.gz", formula)

            scoop = json.loads((art / "saint-harness.scoop.json").read_text(encoding="utf-8"))
            self.assertEqual(scoop["version"], "0.2.0")
            self.assertEqual(scoop["bin"], "saint-harness.exe")

            winget_inst = (art / "winget/SaintHarness.SaintHarness.installer.yaml").read_text(encoding="utf-8")
            self.assertIn("PortableCommandAlias: saint-harness", winget_inst)

    def test_sync_version_validation(self) -> None:
        mod = load_module("sync_version", ROOT / "scripts/release/sync-version.py")
        self.assertEqual(mod.normalize_release_version("0.1.0"), "0.1.0")
        self.assertEqual(mod.normalize_release_version("v1.2.3"), "1.2.3")
        self.assertEqual(mod.normalize_release_version("refs/tags/v2.4.9"), "2.4.9")
        for invalid in ["", "1.0", "v1.0.0-beta.1", "latest", "v1.2.3+meta"]:
            with self.assertRaises(ValueError):
                mod.normalize_release_version(invalid)

    def test_generate_manifest_fails_closed_on_missing_and_writes_all_platforms(self) -> None:
        mod = load_module("generate_manifest", ROOT / "scripts/release/generate-manifest.py")
        with tempfile.TemporaryDirectory() as tmp:
            art = Path(tmp) / "artifacts"
            art.mkdir()
            # Missing all files -> fail closed
            with self.assertRaises(RuntimeError):
                mod.generate_release_outputs(
                    artifacts_dir=art,
                    version="0.1.0",
                    tag="v0.1.0",
                    repo="Locotine/SaintHarness",
                    commit="deadbeef",
                    out_dir=art,
                )

            # Populate all 7 expected platform artifacts + installers
            for fname in mod.expected_platform_files("0.1.0").values():
                (art / fname).write_bytes(f"payload:{fname}\n".encode("utf-8"))
            (art / "install.sh").write_text("#!/usr/bin/env bash\n", encoding="utf-8")
            (art / "install.ps1").write_text("# ps1\n", encoding="utf-8")

            manifest = mod.generate_release_outputs(
                artifacts_dir=art,
                version="0.1.0",
                tag="v0.1.0",
                repo="Locotine/SaintHarness",
                commit="deadbeef",
                out_dir=art,
            )
            self.assertEqual(len(manifest["platforms"]), 7)
            sums = (art / "SHA256SUMS.txt").read_text(encoding="utf-8")
            self.assertIn("saint-harness_0.1.0_amd64.deb", sums)
            self.assertIn("saint-harness_0.1.0_darwin-arm64.tar.gz", sums)
            self.assertIn("saint-harness_0.1.0_windows-x86_64.zip", sums)
            parsed = json.loads((art / "release-manifest.json").read_text(encoding="utf-8"))
            self.assertEqual(parsed["version"], "0.1.0")
            self.assertEqual(parsed["platforms"]["linux-x86_64-deb"]["filename"], "saint-harness_0.1.0_amd64.deb")


if __name__ == "__main__":
    unittest.main(verbosity=2)
