#!/usr/bin/env node
import { createHash } from "node:crypto";
import { chmodSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const pkg = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
const repo = process.env.SAINT_HARNESS_REPO || "Locotine/SaintHarness";
const tag = `v${pkg.version}`;

function platformKey() {
  const map = {
    "linux:x64": "linux-x86_64",
    "linux:arm64": "linux-aarch64",
    "darwin:arm64": "darwin-aarch64",
    "darwin:x64": "darwin-x86_64",
    "win32:x64": "windows-x86_64",
  };
  const key = map[`${process.platform}:${process.arch}`];
  if (!key) {
    throw new Error(`Unsupported platform for @saintharness/cli: ${process.platform}/${process.arch}`);
  }
  return key;
}

async function fetchBuffer(url) {
  const res = await fetch(url, { headers: { "User-Agent": `saintharness-npm/${pkg.version}` } });
  if (!res.ok) {
    throw new Error(`HTTP ${res.status} fetching ${url}`);
  }
  return Buffer.from(await res.arrayBuffer());
}

async function main() {
  if (process.env.SAINT_HARNESS_SKIP_DOWNLOAD === "1") {
    return;
  }
  const key = platformKey();
  const manifestUrl =
    process.env.SAINT_HARNESS_MANIFEST_URL ||
    `https://github.com/${repo}/releases/download/${tag}/release-manifest.json`;
  const manifest = JSON.parse((await fetchBuffer(manifestUrl)).toString("utf8"));
  const asset = manifest.platforms?.[key];
  if (!asset) {
    throw new Error(`Missing platform ${key} in ${manifestUrl}`);
  }

  const archiveBytes = await fetchBuffer(asset.url);
  const actualSha = createHash("sha256").update(archiveBytes).digest("hex");
  if (actualSha.toLowerCase() !== String(asset.sha256).toLowerCase()) {
    throw new Error(`SHA-256 mismatch for ${asset.filename} (expected ${asset.sha256}, got ${actualSha})`);
  }

  const vendorDir = join(root, "vendor");
  mkdirSync(vendorDir, { recursive: true });
  const archivePath = join(vendorDir, asset.filename);
  writeFileSync(archivePath, archiveBytes);

  if (asset.filename.endsWith(".tar.gz")) {
    const r = spawnSync("tar", ["-xzf", archivePath, "-C", vendorDir], { stdio: "inherit" });
    if (r.status !== 0) throw new Error("tar extraction failed");
  } else if (asset.filename.endsWith(".zip")) {
    const r = spawnSync(
      "powershell",
      ["-NoProfile", "-Command", `Expand-Archive -Path '${archivePath}' -DestinationPath '${vendorDir}' -Force`],
      { stdio: "inherit" },
    );
    if (r.status !== 0) throw new Error("PowerShell Expand-Archive failed");
  }
  rmSync(archivePath, { force: true });

  const ext = process.platform === "win32" ? ".exe" : "";
  const binPath = join(vendorDir, `saint-harness${ext}`);
  if (!existsSync(binPath)) {
    throw new Error(`Expected extracted binary at ${binPath}`);
  }
  if (process.platform !== "win32") {
    chmodSync(binPath, 0o755);
  }
}

main().catch((err) => {
  console.error(`[@saintharness/cli] postinstall failed: ${err.message}`);
  process.exit(1);
});
