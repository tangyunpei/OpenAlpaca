#!/usr/bin/env bun
/**
 * Stage the openalpacad daemon as the Tauri sidecar.
 *
 * Usage (from apps/openalpaca-gui):
 *   bun run scripts/prepare-sidecar.ts                   # Tauri's hook: Tauri's triple and profile; by hand: dev, the host
 *   bun run scripts/prepare-sidecar.ts --release         # release profile (by hand, as the release scripts call it)
 *   bun run scripts/prepare-sidecar.ts --target <triple> # build for <triple>, into <target dir>/<triple>/ (the hook of `tauri build --target <triple>` does the same)
 *   bun run scripts/prepare-sidecar.ts --reuse           # dev only: keep the staged copy, build nothing
 *   bun run scripts/prepare-sidecar.ts --dry-run         # print what would happen, build nothing
 *
 * Run by Tauri's CLI. `tauri dev` and `tauri build` run this one script with
 * no flags first — `bun run prepare:sidecar`, in src-tauri/tauri.conf.json's
 * beforeDevCommand and beforeBuildCommand — and hand every before-hook the
 * build they are making (tauri-cli 2.11.4: `helpers::command_env` and
 * `interface::rust::Rust::env`, which `helpers::run_hook` and dev.rs both
 * pass; tauri-utils documents the variables):
 *   - TAURI_ENV_TARGET_TRIPLE: tauri's `--target`, else src-tauri's cargo
 *     `build.target`, else the host from `rustc -vV`.
 *   - TAURI_ENV_PLATFORM_VERSION: the OS version, set for every hook.
 *   - TAURI_ENV_DEBUG: `true` for a debug build and not set at all for a
 *     release one: `tauri build` stages a release daemon, `tauri build --debug`
 *     and `tauri dev` a dev one. `tauri dev` sets it whatever its flags, so
 *     `tauri dev --release` stages a dev daemon too. Any other value is refused.
 * A run is a hook run when TAURI_ENV_TARGET_TRIPLE and
 * TAURI_ENV_PLATFORM_VERSION are both set. Tauri sets the pair together, so
 * any of the three set without the pair complete — a lone TAURI_ENV_DEBUG
 * among them — was exported in this shell, and the run is refused.
 * In a hook run the triple and the profile are Tauri's, never re-derived: the
 * script refuses `--target` naming another triple and `--release` in a debug
 * build. Without `--target` cargo builds for the host, so Tauri's triple is
 * compared with the host — `rustc -vV`, run for that comparison only, on the
 * paths that build. The host's: cargo runs as it does by hand, with no
 * `--target`, and builds the artifact a by-hand call builds (the release
 * scripts' hash check rests on that). Another (`tauri build --target
 * <triple>`): cargo is given `--target <triple>` and builds into
 * `<target dir>/<triple>/<profile>/`, or (expected, not shown here) fails in
 * rustc when that target is not installed (`rustup target add <triple>`).
 * `universal-apple-darwin` is refused everywhere: cargo has no target of that
 * name, and a universal app wants a lipo'd sidecar this script does not make.
 * Outside a hook the triple is `--target`, else the host, and the profile
 * `--release`, else dev.
 *
 * What the environment cannot show. Tauri sets TAURI_ENV_DEBUG for a debug
 * build and never removes one the shell exported, so an exported
 * TAURI_ENV_DEBUG=true reaches `tauri build`'s release hook beside Tauri's own
 * variables, and nothing here can tell that run from `tauri build --debug`:
 * the hook stages a dev daemon into the release bundle. The refusal of a
 * TAURI_ENV_DEBUG without the pair is an early warning for a run by hand, not
 * that protection. What protects a release: the release scripts run this
 * script by hand first, with `--release`, where an exported TAURI_ENV_DEBUG
 * is refused before `tauri build` starts; they hold the bundled daemon to the
 * sha256 that first run recorded; and every stage's provenance file names its
 * profile and where it came from (`profile_from`). Never export TAURI_ENV_*.
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
 *      `<name>.provenance.json` (the staged sha256, git HEAD, dirty flag, the
 *      profile and the target triple with where each came from, `rustc -V`,
 *      the Cargo.lock's sha256, a UTC timestamp, cargo's artifact path). Values
 *      are read, never produced by running the daemon.
 *
 * Reuse during development. Tauri's config gives a hook command no
 * conditional, so the switch is an environment variable the script itself
 * reads:
 *
 *     OPENALPACA_SIDECAR_REUSE=1 bun run tauri dev
 *
 * is `--reuse`: no cargo call, and in a hook run no toolchain command at all.
 * The staged copy is kept only when its sha256 matches its .sha256 file and
 * its provenance file names that sha256, this run's profile and this run's
 * triple — so a release stage, or one for another triple, is never kept for a
 * dev build; otherwise the script exits 1 and says to run without it. A
 * release never reuses: `--reuse` (or the variable) in a release build is
 * refused.
 *
 * Target directories. The daemon builds into whatever cargo resolves at the
 * workspace root — `target/` unless CARGO_TARGET_DIR says otherwise; the
 * script sets none. The Tauri shell builds into `target/gui`:
 * src-tauri/.cargo/config.toml says so for `tauri dev` and `tauri build`,
 * which run cargo from src-tauri/, and a `cargo check -p openalpaca_gui` from
 * the root must be run with `CARGO_TARGET_DIR=target/gui` by hand (CLAUDE.md,
 * build hygiene; nothing sets it there). tauri-build copies the staged sidecar
 * to `<shell target dir>/<profile>/openalpacad` — in target/gui never the
 * daemon cargo built; in a shared target dir, cargo's own dev daemon (measured:
 * a root check with the release copy staged put the release bytes there).
 * tauri-build re-copies only when the staged file is newer than its last run
 * (`cargo:rerun-if-changed`), so the stage is stamped with the time it was
 * made: a copy can carry cargo's older artifact time and go unseen.
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
  utimesSync,
  writeFileSync,
} from "node:fs";
import { basename, join, resolve } from "node:path";
import { createInterface } from "node:readline";

const TAG = "[prepare-sidecar]";
const DAEMON_CRATE = "openalpacad";
const REUSE_ENV = "OPENALPACA_SIDECAR_REUSE";
// What Tauri's CLI hands its before-hooks (tauri-cli 2.11.4): the first two
// for every hook, the third for a debug build only.
const TRIPLE_ENV = "TAURI_ENV_TARGET_TRIPLE";
const PLATFORM_VERSION_ENV = "TAURI_ENV_PLATFORM_VERSION";
const DEBUG_ENV = "TAURI_ENV_DEBUG";
const UNIVERSAL_TRIPLE = "universal-apple-darwin";

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
    `       Run as a Tauri hook (${TRIPLE_ENV} and ${PLATFORM_VERSION_ENV} set), ${TRIPLE_ENV} is the triple and ${DEBUG_ENV} the profile.`,
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
      const value = arg.slice("--target=".length);
      if (value === "") fail(`--target needs a triple\n${usage()}`);
      opts.target = value;
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

/** An environment variable's value; an empty one reads as unset. */
function envValue(name: string): string | null {
  const value = process.env[name];
  return value === undefined || value === "" ? null : value;
}

// ── Tauri's hook environment ─────────────────────────────────────────────
/** The build Tauri's CLI is making, when it runs this script as a hook. */
interface TauriHook {
  triple: string;
  debug: boolean;
}

/**
 * Tauri's hook variables, or null outside a hook. A hook run has
 * TAURI_ENV_TARGET_TRIPLE and TAURI_ENV_PLATFORM_VERSION both set; any of the
 * three set without the pair complete, or a TAURI_ENV_DEBUG other than
 * `true`, exits.
 */
function tauriHook(): TauriHook | null {
  const triple = envValue(TRIPLE_ENV);
  const platformVersion = envValue(PLATFORM_VERSION_ENV);
  const debug = envValue(DEBUG_ENV);
  if (debug !== null && debug !== "true") {
    fail(
      `${DEBUG_ENV} is "${debug}": Tauri's CLI sets it to true for a debug build and leaves it unset for a release one`,
    );
  }
  if (triple !== null && platformVersion !== null) {
    return { triple, debug: debug === "true" };
  }
  const exported = [TRIPLE_ENV, PLATFORM_VERSION_ENV, DEBUG_ENV].filter(
    (name) => envValue(name) !== null,
  );
  if (exported.length === 0) return null;
  const missing = [TRIPLE_ENV, PLATFORM_VERSION_ENV].filter(
    (name) => envValue(name) === null,
  );
  const one = exported.length === 1;
  fail(
    `${exported.join(" and ")} ${one ? "is" : "are"} set without ${missing.join(" and ")}, so this is no Tauri hook (Tauri's CLI sets ${TRIPLE_ENV} and ${PLATFORM_VERSION_ENV} for every hook): ${one ? "it was" : "they were"} exported in this shell. Unset ${one ? "it" : "them"}` +
      (debug !== null
        ? `; an exported ${DEBUG_ENV} reaches \`tauri build\`'s release hook too, where nothing can tell it from \`tauri build --debug\`'s, and a dev daemon is staged into the bundle`
        : ""),
  );
}

/** `universal-apple-darwin` is a Tauri target, not a cargo one. */
function refuseUniversal(triple: string, source: string): void {
  if (triple === UNIVERSAL_TRIPLE) {
    fail(
      `${source} is ${UNIVERSAL_TRIPLE}: cargo has no target of that name, and a universal app wants a lipo'd openalpacad-${UNIVERSAL_TRIPLE}, which this script does not make`,
    );
  }
}

/** The fields of a stage's provenance file that `--reuse` holds the run to. */
interface StagedProvenance {
  sha256: string;
  profile: string;
  target_triple: string;
}

/** Reads a stage's provenance file (no toolchain command); a missing field exits. */
function readProvenance(path: string): StagedProvenance {
  let parsed: unknown;
  try {
    parsed = JSON.parse(readFileSync(path, "utf8"));
  } catch (error) {
    fail(
      `--reuse: ${path} is not JSON (${(error as Error).message}); run without --reuse to build`,
    );
  }
  if (typeof parsed !== "object" || parsed === null) {
    fail(`--reuse: ${path} holds no JSON object; run without --reuse to build`);
  }
  const p = parsed as Record<string, unknown>;
  for (const key of ["sha256", "profile", "target_triple"]) {
    if (typeof p[key] !== "string") {
      fail(`--reuse: ${path} names no ${key}; run without --reuse to build`);
    }
  }
  return {
    sha256: p.sha256 as string,
    profile: p.profile as string,
    target_triple: p.target_triple as string,
  };
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
const hook = tauriHook();

// ── The profile: Tauri's in a hook run, else --release or dev ────────────
if (hook?.debug && opts.release) {
  fail(
    `--release, but Tauri's CLI is making a debug build (${DEBUG_ENV}=true)`,
  );
}
const release = hook ? !hook.debug : opts.release;
const profileFrom = hook
  ? `${DEBUG_ENV} ${hook.debug ? "true" : "unset"}`
  : opts.release
    ? "--release"
    : "default";

// ── The triple: Tauri's in a hook run, else --target or the host ─────────
if (opts.target !== null) refuseUniversal(opts.target, "--target");
if (hook) {
  refuseUniversal(hook.triple, TRIPLE_ENV);
  if (opts.target !== null && opts.target !== hook.triple) {
    fail(
      `--target ${opts.target}, but Tauri's CLI is building for ${hook.triple} (${TRIPLE_ENV})`,
    );
  }
}
// In a hook run the triple is Tauri's; rustc -vV is never asked for it
// (resolveCargoTarget may ask for the host, to compare).
const triple = hook?.triple ?? opts.target ?? hostTriple();
const tripleSource = hook
  ? opts.target !== null
    ? `${TRIPLE_ENV}, equal to --target`
    : TRIPLE_ENV
  : opts.target !== null
    ? "--target"
    : "rustc -vV (the host)";

const isWindows = triple.includes("windows");
const exe = isWindows ? ".exe" : "";
const sidecarName = `openalpacad-${triple}${exe}`;
const destPath = join(binDir, sidecarName);
const shaPath = `${destPath}.sha256`;
const provenancePath = `${destPath}.provenance.json`;
const profile = release ? "release" : "dev";
const profileDir = release ? "release" : "debug";

// ── A release never reuses ───────────────────────────────────────────────
if (opts.reuse && release) {
  fail(
    `--reuse (or ${REUSE_ENV}=1) is for development builds only; a release stages a fresh build`,
  );
}

/** The `--target` cargo is given, and where the triple came from. */
interface CargoTarget {
  target: string | null;
  tripleFrom: string;
}

/**
 * `--target` when given. From Tauri's hook without one, cargo builds for the
 * host unless told otherwise, so Tauri's triple is compared with the host
 * (`rustc -vV`, run for that comparison only): the host's — no `--target`,
 * cargo runs as it does by hand and builds the same artifact; another —
 * `--target <Tauri's triple>`, into `<target dir>/<triple>/<profile>/`.
 * Called on the paths that build and on the dry run, never on `--reuse`.
 */
function resolveCargoTarget(): CargoTarget {
  if (opts.target !== null || !hook) {
    return { target: opts.target, tripleFrom: tripleSource };
  }
  const host = hostTriple();
  return host === hook.triple
    ? {
        target: null,
        tripleFrom: `${TRIPLE_ENV}, the host (no cargo --target)`,
      }
    : {
        target: hook.triple,
        tripleFrom: `${TRIPLE_ENV}, not the host (${host}), passed to cargo as --target`,
      };
}

function cargoArgsFor(target: string | null): string[] {
  const args = ["build", "-p", DAEMON_CRATE, "--locked"];
  if (release) args.push("--release");
  if (target !== null) args.push("--target", target);
  args.push("--message-format=json-render-diagnostics");
  return args;
}

function describeRun(tripleFrom: string): void {
  log(
    hook
      ? `run by Tauri's CLI: ${TRIPLE_ENV}=${hook.triple}, ${DEBUG_ENV} ${hook.debug ? "true" : "unset"}`
      : "run by hand (no Tauri hook variables)",
  );
  log(`profile:             ${profile}  (from ${profileFrom})`);
  log(`target triple:       ${triple}  (from ${tripleFrom})`);
}

// ── --dry-run: say what would happen, build nothing ──────────────────────
if (opts.dryRun && opts.reuse) {
  log("dry run — nothing is built or written");
  describeRun(tripleSource);
  log(
    `would reuse:         ${destPath}, if its sha256 equals ${basename(shaPath)}'s and ${basename(provenancePath)} names that sha256, ${profile} and ${triple}`,
  );
  log("cargo would not run");
  process.exit(0);
}

if (opts.dryRun) {
  const planned = resolveCargoTarget();
  const metadata = JSON.parse(
    capture(
      "cargo",
      ["metadata", "--format-version", "1", "--no-deps", "--locked"],
      workspaceRoot,
    ),
  ) as { target_directory: string };
  const predicted = join(
    metadata.target_directory,
    ...(planned.target !== null ? [planned.target] : []),
    profileDir,
    `${DAEMON_CRATE}${exe}`,
  );
  log("dry run — nothing is built or written");
  describeRun(planned.tripleFrom);
  log(`workspace root:      ${workspaceRoot}`);
  log(`cargo target dir:    ${metadata.target_directory}`);
  log(`would run:           cargo ${cargoArgsFor(planned.target).join(" ")}`);
  log(
    `artifact (predicted): ${predicted}  — the real run takes the path from cargo's compiler-artifact message`,
  );
  log(`would stage:         ${destPath}`);
  log(`would write:         ${shaPath}`);
  log(`would write:         ${provenancePath}`);
  process.exit(0);
}

// ── --reuse: keep what is staged, if it is what its records say ──────────
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
  if (!existsSync(provenancePath)) {
    fail(`--reuse: ${provenancePath} is missing; run without --reuse to build`);
  }
  const staged = readProvenance(provenancePath);
  if (staged.sha256 !== actual) {
    fail(
      `--reuse: ${basename(provenancePath)} describes another build (sha256 ${staged.sha256}, staged ${actual}); run without --reuse to rebuild`,
    );
  }
  if (staged.profile !== profile) {
    fail(
      `--reuse: the staged ${sidecarName} is a ${staged.profile} build (its provenance), and this is a ${profile} build (from ${profileFrom}); run without --reuse to rebuild`,
    );
  }
  if (staged.target_triple !== triple) {
    fail(
      `--reuse: the staged ${sidecarName} was built for ${staged.target_triple} (its provenance), and this build is for ${triple} (from ${tripleSource}); run without --reuse to rebuild`,
    );
  }
  log(
    `--reuse: keeping ${destPath} (sha256 ${actual}; ${staged.profile}, ${staged.target_triple}, as its provenance records); cargo not run`,
  );
  process.exit(0);
}

// ── Build, then stage cargo's artifact ───────────────────────────────────
const cargoTarget = resolveCargoTarget();
describeRun(cargoTarget.tripleFrom);
const artifact = await cargoBuild(cargoArgsFor(cargoTarget.target));
if (!existsSync(artifact.executable)) {
  fail(`cargo named ${artifact.executable} but it does not exist`);
}

mkdirSync(binDir, { recursive: true });
for (const stale of [destPath, shaPath, provenancePath]) {
  rmSync(stale, { force: true });
}
copyFileSync(artifact.executable, destPath);
if (!isWindows) chmodSync(destPath, 0o755);
// The copy can keep the artifact's mtime (measured on APFS); stamp it now so
// tauri-build's rerun-if-changed sees every new stage, even an older build.
const stagedAt = new Date();
utimesSync(destPath, stagedAt, stagedAt);

const sha256 = sha256File(destPath);
writeFileSync(shaPath, `${sha256}  ${sidecarName}\n`);

const gitHead = tryCapture("git", ["rev-parse", "HEAD"], workspaceRoot);
const gitStatus = tryCapture("git", ["status", "--porcelain"], workspaceRoot);
const provenance = {
  sidecar: sidecarName,
  sha256,
  profile,
  profile_from: profileFrom,
  target_triple: triple,
  target_triple_from: cargoTarget.tripleFrom,
  features: artifact.features,
  cargo_executable: artifact.executable,
  rustc: capture("rustc", ["-V"], workspaceRoot).trim(),
  cargo_lock_sha256: sha256File(join(workspaceRoot, "Cargo.lock")),
  git_head: gitHead === null ? null : gitHead.trim(),
  git_dirty: gitStatus === null ? null : gitStatus.trim().length > 0,
  staged_at: stagedAt.toISOString(),
};
writeFileSync(provenancePath, `${JSON.stringify(provenance, null, 2)}\n`);

log(`staged ${artifact.executable} → ${destPath}`);
log(`sha256 ${sha256}  (${basename(shaPath)})`);
log(
  `provenance ${basename(provenancePath)}: ${profile} (from ${profileFrom}), ${triple} (from ${cargoTarget.tripleFrom}), ${provenance.rustc}`,
);
