#!/usr/bin/env bun
/**
 * Stage the openalpacad daemon as the Tauri sidecar.
 *
 * Usage (from apps/openalpaca-gui):
 *   bun run scripts/prepare-sidecar.ts                   # dev profile
 *   bun run scripts/prepare-sidecar.ts --release         # release profile
 *   bun run scripts/prepare-sidecar.ts --target <triple> # cross-compile; pair with `tauri build --target <triple>`
 *   bun run scripts/prepare-sidecar.ts --reuse           # dev only: keep the staged copy, build nothing
 *   bun run scripts/prepare-sidecar.ts --dry-run         # print what would happen, build nothing
 *
 * What it does:
 *   1. Runs `cargo build -p openalpacad --locked [--release] [--target <triple>]
 *      --message-format=json-render-diagnostics` at the workspace root. Cargo
 *      decides freshness, and the daemon's executable path is read from cargo's
 *      own `compiler-artifact` message — so CARGO_TARGET_DIR, a `[build]
 *      target-dir`, a `--target` triple and the profile are all followed, and
 *      nothing here hard-codes `target/<profile>` or compares two binaries'
 *      mtimes. `--locked` builds the committed Cargo.lock as it is; a manifest
 *      change wants its own `cargo update` first.
 *   2. Copies that artifact to src-tauri/bin/openalpacad-<triple>[.exe] and
 *      writes beside it `<name>.sha256` (the sha256 of the staged bytes, in
 *      `shasum -a 256` format, so `shasum -c` / `sha256sum -c` verify it) and
 *      `<name>.provenance.json` (git HEAD, dirty flag, profile, target triple,
 *      `rustc -V`, the Cargo.lock's sha256, a UTC timestamp, cargo's artifact
 *      path). Values are read, never produced by running the daemon.
 *
 * Reuse during development. `tauri dev` runs this script through
 * `beforeDevCommand` in src-tauri/tauri.conf.json — a plain command string
 * Tauri gives no conditional for — so the switch is an environment variable the
 * script itself reads:
 *
 *     OPENALPACA_SIDECAR_REUSE=1 bun run tauri dev
 *
 * is `--reuse`: no cargo call; the staged copy is kept only when its sha256
 * matches its .sha256 file, and otherwise the script exits 1 and says to run
 * without it. A release never reuses: `--reuse` (or the variable) with
 * `--release` is refused.
 *
 * Target directories. The daemon builds into whatever cargo resolves at the
 * workspace root — `target/` unless CARGO_TARGET_DIR says otherwise; the
 * script sets none. The Tauri shell builds into `target/gui`:
 * src-tauri/.cargo/config.toml says so for `tauri dev` and `tauri build`,
 * which run cargo from src-tauri/, and a root `cargo check -p openalpaca_gui`
 * sets `CARGO_TARGET_DIR=target/gui` (CLAUDE.md, build hygiene). tauri-build
 * copies the staged sidecar to `<shell target dir>/<profile>/openalpacad`,
 * which is therefore never the daemon cargo built.
 */

import { spawn, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { basename, join, resolve } from "node:path";
import { createInterface } from "node:readline";

const TAG = "[prepare-sidecar]";
const DAEMON_CRATE = "openalpacad";
const REUSE_ENV = "OPENALPACA_SIDECAR_REUSE";

// ── Arguments ────────────────────────────────────────────────────────────
interface Options {
  release: boolean;
  reuse: boolean;
  dryRun: boolean;
  target: string | null;
}

function usage(): string {
  return [
    "usage: bun run scripts/prepare-sidecar.ts [--release] [--target <triple>] [--reuse] [--dry-run]",
    `       ${REUSE_ENV}=1 is --reuse (for tauri.conf.json's beforeDevCommand).`,
  ].join("\n");
}

function parseArgs(argv: string[]): Options {
  const opts: Options = {
    release: false,
    reuse: process.env[REUSE_ENV] === "1",
    dryRun: false,
    target: null,
  };
  for (let i = 0; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === "--release") opts.release = true;
    else if (arg === "--reuse") opts.reuse = true;
    else if (arg === "--dry-run") opts.dryRun = true;
    else if (arg === "--target") {
      const value: string | undefined = argv[i + 1];
      if (value === undefined || value.startsWith("--")) {
        fail(`--target needs a triple\n${usage()}`);
      }
      opts.target = value;
      i += 1;
    } else if (arg.startsWith("--target=")) {
      opts.target = arg.slice("--target=".length);
    } else if (arg === "--help" || arg === "-h") {
      console.log(usage());
      process.exit(0);
    } else {
      fail(`unknown argument ${arg}\n${usage()}`);
    }
  }
  return opts;
}

function log(message: string): void {
  console.log(`${TAG} ${message}`);
}

function fail(message: string): never {
  console.error(`${TAG} ERROR: ${message}`);
  process.exit(1);
}

// ── Small helpers ────────────────────────────────────────────────────────
/** Runs a command to completion and returns its stdout; fails on a non-zero exit. */
function capture(command: string, args: string[], cwd: string): string {
  const result = spawnSync(command, args, { cwd, encoding: "utf8" });
  if (result.error) {
    fail(`could not run ${command}: ${result.error.message}`);
  }
  if (result.status !== 0) {
    fail(
      `${command} ${args.join(" ")} exited with ${result.status}\n${result.stderr}`,
    );
  }
  return result.stdout;
}

/** Like `capture`, but a failure yields null (for values that may be absent, e.g. outside a git checkout). */
function tryCapture(
  command: string,
  args: string[],
  cwd: string,
): string | null {
  const result = spawnSync(command, args, { cwd, encoding: "utf8" });
  if (result.error || result.status !== 0) return null;
  return result.stdout;
}

function sha256File(path: string): string {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

function hostTriple(): string {
  const rustcOutput = capture("rustc", ["-vV"], workspaceRoot);
  const match = rustcOutput.match(/^host:\s*(.+)$/m);
  if (!match) {
    fail("could not determine the host target triple from `rustc -vV`");
  }
  return match[1].trim();
}

/** Reads the hex digest out of a `shasum -a 256`-format file (`<hex>  <name>`). */
function readSha256File(path: string): string {
  const digest = readFileSync(path, "utf8").trim().split(/\s+/)[0];
  if (!/^[0-9a-f]{64}$/.test(digest)) {
    fail(`${path} holds no sha256 digest`);
  }
  return digest;
}

// ── Cargo's messages ─────────────────────────────────────────────────────
interface CompilerArtifact {
  reason: "compiler-artifact";
  target: { name: string; kind: string[] };
  executable: string;
  features: string[];
}

/** The `compiler-artifact` message for the daemon's bin target, if `msg` is one. */
function daemonArtifact(msg: unknown): CompilerArtifact | null {
  if (typeof msg !== "object" || msg === null) return null;
  const m = msg as Record<string, unknown>;
  if (m.reason !== "compiler-artifact") return null;
  const target = m.target;
  if (typeof target !== "object" || target === null) return null;
  const t = target as Record<string, unknown>;
  if (t.name !== DAEMON_CRATE) return null;
  if (!Array.isArray(t.kind) || !t.kind.includes("bin")) return null;
  if (typeof m.executable !== "string") return null;
  if (!Array.isArray(m.features)) return null;
  return m as unknown as CompilerArtifact;
}

function buildFinishedSuccess(msg: unknown): boolean | null {
  if (typeof msg !== "object" || msg === null) return null;
  const m = msg as Record<string, unknown>;
  if (m.reason !== "build-finished") return null;
  return m.success === true;
}

/**
 * Runs the build and returns cargo's own artifact message for the daemon.
 * Cargo's JSON goes to a pipe read line by line (a whole build's messages can
 * run past a default `maxBuffer`); the rendered diagnostics and the progress
 * lines stay on stderr, which the developer sees as usual.
 */
async function cargoBuild(args: string[]): Promise<CompilerArtifact> {
  log(`building: cargo ${args.join(" ")}  (cwd ${workspaceRoot})`);
  const child = spawn("cargo", args, {
    cwd: workspaceRoot,
    stdio: ["ignore", "pipe", "inherit"],
  });
  const stdout = child.stdout;
  if (!stdout) fail("cargo's stdout was not piped");
  // A spawn failure (no `cargo` on PATH) rejects this and ends the read below.
  const spawnFailed = new Promise<never>((_, reject) => {
    child.on("error", reject);
  });
  spawnFailed.catch(() => {});
  const exit = new Promise<number | null>((done) => {
    child.on("close", (code) => done(code));
  });

  interface Seen {
    artifact: CompilerArtifact | null;
    finished: boolean | null;
  }
  const consume = async (): Promise<Seen> => {
    const seen: Seen = { artifact: null, finished: null };
    for await (const line of createInterface({ input: stdout })) {
      if (!line.startsWith("{")) continue;
      let msg: unknown;
      try {
        msg = JSON.parse(line);
      } catch {
        continue;
      }
      const found = daemonArtifact(msg);
      if (found) seen.artifact = found;
      const success = buildFinishedSuccess(msg);
      if (success !== null) seen.finished = success;
    }
    return seen;
  };
  let seen: Seen;
  try {
    seen = await Promise.race([consume(), spawnFailed]);
  } catch (error) {
    fail(`could not run cargo: ${(error as Error).message}`);
  }

  const code = await exit;
  if (code !== 0 || seen.finished === false) {
    fail(`cargo build exited with ${code ?? "a signal"}`);
  }
  if (!seen.artifact) {
    fail(
      `cargo reported no bin artifact named ${DAEMON_CRATE}; nothing was staged`,
    );
  }
  return seen.artifact;
}

// ── Paths ────────────────────────────────────────────────────────────────
const guiRoot = resolve(import.meta.dirname, "..");
const workspaceRoot = resolve(guiRoot, "../..");
const binDir = join(guiRoot, "src-tauri", "bin");

const opts = parseArgs(process.argv.slice(2));
const triple = opts.target ?? hostTriple();
const isWindows = triple.includes("windows");
const exe = isWindows ? ".exe" : "";
const sidecarName = `openalpacad-${triple}${exe}`;
const destPath = join(binDir, sidecarName);
const shaPath = `${destPath}.sha256`;
const provenancePath = `${destPath}.provenance.json`;
const profile = opts.release ? "release" : "dev";
const profileDir = opts.release ? "release" : "debug";

const cargoArgs = ["build", "-p", DAEMON_CRATE, "--locked"];
if (opts.release) cargoArgs.push("--release");
if (opts.target) cargoArgs.push("--target", opts.target);
cargoArgs.push("--message-format=json-render-diagnostics");

// ── A release never reuses ───────────────────────────────────────────────
if (opts.reuse && opts.release) {
  fail(
    `--reuse (or ${REUSE_ENV}=1) is for development builds only; a release stages a fresh build`,
  );
}

// ── --dry-run: say what would happen, build nothing ──────────────────────
if (opts.dryRun && opts.reuse) {
  log("dry run — nothing is built or written");
  log(
    `would reuse:         ${destPath}, if its sha256 equals ${basename(shaPath)}'s`,
  );
  log("cargo would not run");
  process.exit(0);
}

if (opts.dryRun) {
  const metadata = JSON.parse(
    capture(
      "cargo",
      ["metadata", "--format-version", "1", "--no-deps", "--locked"],
      workspaceRoot,
    ),
  ) as { target_directory: string };
  const predicted = join(
    metadata.target_directory,
    ...(opts.target ? [opts.target] : []),
    profileDir,
    `${DAEMON_CRATE}${exe}`,
  );
  log("dry run — nothing is built or written");
  log(`workspace root:      ${workspaceRoot}`);
  log(`cargo target dir:    ${metadata.target_directory}`);
  log(`would run:           cargo ${cargoArgs.join(" ")}`);
  log(
    `artifact (predicted): ${predicted}  — the real run takes the path from cargo's compiler-artifact message`,
  );
  log(`would stage:         ${destPath}`);
  log(`would write:         ${shaPath}`);
  log(`would write:         ${provenancePath}`);
  process.exit(0);
}

// ── --reuse: keep what is staged, if it is what its .sha256 says ─────────
if (opts.reuse) {
  if (!existsSync(destPath)) {
    fail(
      `--reuse: nothing is staged at ${destPath}; run without --reuse to build`,
    );
  }
  if (!existsSync(shaPath)) {
    fail(`--reuse: ${shaPath} is missing; run without --reuse to build`);
  }
  const recorded = readSha256File(shaPath);
  const actual = sha256File(destPath);
  if (recorded !== actual) {
    fail(
      `--reuse: ${sidecarName} does not match its .sha256 (recorded ${recorded}, staged ${actual}); run without --reuse to rebuild`,
    );
  }
  log(`--reuse: keeping ${destPath} (sha256 ${actual}); cargo not run`);
  process.exit(0);
}

// ── Build, then stage cargo's artifact ───────────────────────────────────
const artifact = await cargoBuild(cargoArgs);
if (!existsSync(artifact.executable)) {
  fail(`cargo named ${artifact.executable} but it does not exist`);
}

mkdirSync(binDir, { recursive: true });
for (const stale of [destPath, shaPath, provenancePath]) {
  rmSync(stale, { force: true });
}
copyFileSync(artifact.executable, destPath);
if (!isWindows) chmodSync(destPath, 0o755);

const sha256 = sha256File(destPath);
writeFileSync(shaPath, `${sha256}  ${sidecarName}\n`);

const gitHead = tryCapture("git", ["rev-parse", "HEAD"], workspaceRoot);
const gitStatus = tryCapture("git", ["status", "--porcelain"], workspaceRoot);
const provenance = {
  sidecar: sidecarName,
  sha256,
  profile,
  target_triple: triple,
  features: artifact.features,
  cargo_executable: artifact.executable,
  rustc: capture("rustc", ["-V"], workspaceRoot).trim(),
  cargo_lock_sha256: sha256File(join(workspaceRoot, "Cargo.lock")),
  git_head: gitHead === null ? null : gitHead.trim(),
  git_dirty: gitStatus === null ? null : gitStatus.trim().length > 0,
  staged_at: new Date().toISOString(),
};
writeFileSync(provenancePath, `${JSON.stringify(provenance, null, 2)}\n`);

log(`staged ${artifact.executable} → ${destPath}`);
log(`sha256 ${sha256}  (${basename(shaPath)})`);
log(
  `provenance ${basename(provenancePath)}: ${profile}, ${triple}, ${provenance.rustc}`,
);
