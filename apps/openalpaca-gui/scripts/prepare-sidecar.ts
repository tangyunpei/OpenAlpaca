#!/usr/bin/env bun
/**
 * Prepare the openalpacad sidecar binary for Tauri bundling.
 *
 * Usage:
 *   bun run scripts/prepare-sidecar.ts           # debug build
 *   bun run scripts/prepare-sidecar.ts --release  # release build
 *
 * Determines the host target triple from `rustc -vV`, builds the daemon
 * crate, and copies it to src-tauri/bin/openalpacad-{triple}[.exe].
 * Skips rebuild if the binary is already newer than the source.
 */

import { execSync } from "node:child_process";
import { existsSync, mkdirSync, statSync, copyFileSync } from "node:fs";
import { join, resolve } from "node:path";

const isRelease = process.argv.includes("--release");

// ── Resolve the workspace root (two levels up from openalpaca-gui) ───────
const guiRoot = resolve(import.meta.dirname, "..");
const workspaceRoot = resolve(guiRoot, "../..");
const binDir = join(guiRoot, "src-tauri", "bin");

// ── Get host target triple ───────────────────────────────────────────────
function getTargetTriple(): string {
  const rustcOutput = execSync("rustc -vV", { encoding: "utf-8" });
  const match = rustcOutput.match(/^host:\s*(.+)$/m);
  if (!match) {
    throw new Error("Could not determine host target triple from `rustc -vV`");
  }
  return match[1].trim();
}

const triple = getTargetTriple();
const isWindows = triple.includes("windows");
const ext = isWindows ? ".exe" : "";
const sidecarName = `openalpacad-${triple}${ext}`;
const destPath = join(binDir, sidecarName);

// ── Source binary path ───────────────────────────────────────────────────
const profile = isRelease ? "release" : "debug";
const sourcePath = join(
  workspaceRoot,
  "target",
  profile,
  `openalpacad${ext}`
);

// ── Skip rebuild if binary already exists and is up-to-date ──────────────
function shouldRebuild(): boolean {
  if (!existsSync(destPath)) return true;
  if (!existsSync(sourcePath)) return true;
  const destStat = statSync(destPath);
  const srcStat = statSync(sourcePath);
  return srcStat.mtimeMs > destStat.mtimeMs;
}

if (!shouldRebuild()) {
  console.log(`[prepare-sidecar] ${sidecarName} is up-to-date, skipping build`);
  process.exit(0);
}

// ── Build the daemon crate ───────────────────────────────────────────────
const cargoArgs = ["cargo", "build", "-p", "openalpacad"];
if (isRelease) cargoArgs.push("--release");

console.log(`[prepare-sidecar] Building: ${cargoArgs.join(" ")}`);
execSync(cargoArgs.join(" "), {
  cwd: workspaceRoot,
  stdio: "inherit",
});

// ── Copy to sidecar bin directory ────────────────────────────────────────
if (!existsSync(binDir)) {
  mkdirSync(binDir, { recursive: true });
}

if (!existsSync(sourcePath)) {
  console.error(`[prepare-sidecar] ERROR: Expected binary at ${sourcePath} but it does not exist`);
  process.exit(1);
}

copyFileSync(sourcePath, destPath);
console.log(`[prepare-sidecar] Copied ${sourcePath} → ${destPath}`);
