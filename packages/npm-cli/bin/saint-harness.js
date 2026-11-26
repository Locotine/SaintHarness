#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const ext = process.platform === "win32" ? ".exe" : "";
const binPath = join(here, "..", "vendor", `saint-harness${ext}`);

if (!existsSync(binPath)) {
  console.error(
    "SaintHarness native binary is missing. Re-run `npm rebuild @saintharness/cli` or install via scripts/install.sh.",
  );
  process.exit(1);
}

const result = spawnSync(binPath, process.argv.slice(2), { stdio: "inherit" });
if (result.error) {
  console.error(result.error.message);
  process.exit(1);
}
process.exit(result.status ?? 1);
