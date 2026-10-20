#!/usr/bin/env bash
set -euo pipefail

die() {
  echo "ERROR: $*" >&2
  exit 1
}

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "Required command not found: $1"
}

# Dies unless `bun --version` prints the version the root package.json pins
# ("packageManager": "bun@<version>"), naming both (G-06). The pin is read with
# sed, not bun, so the bun on PATH runs once here, as `bun --version`.
require_pinned_bun() {
  local manifest="$REPO_ROOT/package.json" pinned actual
  require_cmd bun
  pinned="$(sed -n 's/^[[:space:]]*"packageManager"[[:space:]]*:[[:space:]]*"bun@\([^"]*\)".*$/\1/p' "$manifest")" \
    || die "Cannot read $manifest."
  [[ -n "$pinned" ]] || die "No \"packageManager\": \"bun@<version>\" in $manifest."
  actual="$(bun --version)" || die "bun --version failed."
  [[ "$actual" == "$pinned" ]] \
    || die "bun $actual is first on PATH, but $manifest pins bun@$pinned (packageManager); install Bun $pinned or put it first on PATH."
}

# ── Known Limitations ───────────────────────────────────────────
# - Binaries and the .app bundle are NOT codesigned.
#   Users will see Gatekeeper "unidentified developer" warnings.
#   The installer works around this with: xattr -dr com.apple.quarantine
#   For production releases, add: codesign --sign "Developer ID" ...
# ────────────────────────────────────────────────────────────────

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
DIST_DIR="$REPO_ROOT/dist"
GUI_DIR="$REPO_ROOT/apps/openalpaca-gui"

# Sets TARGET_DIRECTORY to the target directory cargo resolves when run in $1,
# or dies. cargo reads its config from the directory it runs in, so the answer
# is that of a build started there: an exported CARGO_TARGET_DIR, else
# build.target-dir from the cargo config read there, else the workspace's
# target/. `cargo metadata` reports it as target_directory; its `..` segments
# are resolved lexically. `die` runs in the calling shell, not in a subshell.
read_target_directory() {
  local dir="$1" metadata
  require_cmd cargo
  require_pinned_bun
  metadata="$(cd "$dir" && cargo metadata --format-version 1 --no-deps --locked)" \
    || die "cargo metadata failed in $dir."
  TARGET_DIRECTORY="$(
    printf '%s' "$metadata" | bun -e '
      const path = require("node:path");
      const targetDir = JSON.parse(require("node:fs").readFileSync(0, "utf8")).target_directory;
      if (typeof targetDir !== "string" || targetDir === "") process.exit(1);
      console.log(path.resolve(targetDir));
    '
  )" || die "cargo metadata in $dir reported no target_directory."
}

# Sets OPENALPACA_BIN to the CLI `cargo build --release -p openalpaca` writes
# when run at the repository root (the script builds it there), or dies naming
# the path: <target directory at the root>/release/openalpaca. Nothing else is
# looked at: a CLI under any other directory is an older build's.
locate_cli() {
  read_target_directory "$REPO_ROOT"
  OPENALPACA_BIN="$TARGET_DIRECTORY/release/openalpaca"
  [[ -f "$OPENALPACA_BIN" && -x "$OPENALPACA_BIN" ]] \
    || die "CLI not found or not executable: $OPENALPACA_BIN (cargo's target directory at the repository root; no other directory is read)."
}

# Sets APP_BUNDLE_PATH to the .app `tauri build --bundles app` writes, or dies
# naming the path: <target directory in src-tauri/>/release/bundle/macos/
# <productName>.app. tauri runs cargo from src-tauri/, where the target
# directory is an exported CARGO_TARGET_DIR, else src-tauri/.cargo/config.toml's
# target-dir (target/gui); productName is read from tauri.conf.json. Nothing
# else is looked at: a bundle under any other directory is an older build's.
# productName names the bundle tauri writes only; the package always carries
# it as gui/openalpaca-gui.app (install.sh).
locate_app_bundle() {
  local src_tauri="$GUI_DIR/src-tauri" conf product_name
  read_target_directory "$src_tauri"
  conf="$src_tauri/tauri.conf.json"
  product_name="$(
    TAURI_CONF="$conf" bun -e '
      const productName = JSON.parse(require("node:fs").readFileSync(process.env.TAURI_CONF, "utf8")).productName;
      if (typeof productName !== "string" || productName === "") process.exit(1);
      console.log(productName);
    '
  )" || die "Could not read productName from $conf."
  APP_BUNDLE_PATH="$TARGET_DIRECTORY/release/bundle/macos/$product_name.app"
  [[ -d "$APP_BUNDLE_PATH" ]] \
    || die "Tauri app bundle not found: $APP_BUNDLE_PATH (cargo's target directory for apps/openalpaca-gui/src-tauri; no other directory is read)."
}

# `source scripts/release/package-macos.sh` defines the functions above and
# builds nothing. It turns on `set -euo pipefail` in the sourcing shell, and a
# failing lookup's `die` exits that shell, an interactive one included: source
# it inside `bash -c`. A manual seam: no CI job or committed test drives it,
# unlike the Windows scripts' -DotSourceOnly, which Pester drives in CI
# (DU-33 keeps the CI legs deferred).
[[ "${BASH_SOURCE[0]}" == "$0" ]] || return 0

[[ "$(uname -s)" == "Darwin" ]] || die "package-macos.sh must run on macOS."

require_cmd cargo
require_cmd rustc
require_pinned_bun
require_cmd bunx
require_cmd tar
require_cmd shasum
require_cmd git
require_cmd awk
require_cmd sed
require_cmd date

HOST_TARGET="$(rustc -vV | awk '/^host:/ {print $2}')"
case "$HOST_TARGET" in
  aarch64-apple-darwin|x86_64-apple-darwin) ;;
  *) die "Unsupported host target for macOS packaging: $HOST_TARGET" ;;
esac

VERSION="$(cargo pkgid -p openalpaca | sed -E 's/.*#//')"
[[ -n "$VERSION" ]] || die "Failed to resolve package version."

GIT_SHA="$(git -C "$REPO_ROOT" rev-parse --short HEAD 2>/dev/null || echo "unknown")"
BUILT_AT_UTC="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
PACKAGE_NAME="openalpaca-macos-${HOST_TARGET}-v${VERSION}"
STAGING_BASE="$REPO_ROOT/target/release-package"
STAGING_DIR="$STAGING_BASE/$PACKAGE_NAME"
ARCHIVE_PATH="$DIST_DIR/${PACKAGE_NAME}.tar.gz"
SHA_PATH="${ARCHIVE_PATH}.sha256"

STAGED_SIDECAR="$GUI_DIR/src-tauri/bin/openalpacad-$HOST_TARGET"
STAGED_SHA_FILE="$STAGED_SIDECAR.sha256"

sha256_of() {
  shasum -a 256 "$1" | awk '{print $1}'
}

# The hex digest of a `shasum -a 256`-format file (`<hex>  <name>`), as
# prepare-sidecar.ts writes it beside the staged sidecar.
read_sha256_file() {
  local digest
  digest="$(awk 'NR == 1 { print $1 }' "$1")"
  [[ "$digest" =~ ^[0-9a-f]{64}$ ]] || die "No sha256 digest in $1"
  printf '%s\n' "$digest"
}

echo "==> Installing JS dependencies (frozen lockfile)"
(
  cd "$GUI_DIR"
  bun install --frozen-lockfile
)

# One canonical daemon-only build: prepare-sidecar.ts builds openalpacad
# through cargo, stages cargo's own artifact at src-tauri/bin/ and records its
# sha256. That staged file is the sidecar Tauri bundles AND the libexec/ daemon
# packaged below.
echo "==> Building and staging the daemon (release)"
(
  cd "$GUI_DIR"
  bun run scripts/prepare-sidecar.ts --release
)
[[ -f "$STAGED_SIDECAR" ]] || die "Staged sidecar not found: $STAGED_SIDECAR"
STAGED_SHA="$(read_sha256_file "$STAGED_SHA_FILE")"
[[ "$(sha256_of "$STAGED_SIDECAR")" == "$STAGED_SHA" ]] \
  || die "Staged sidecar does not match $STAGED_SHA_FILE"

echo "==> Building the CLI"
(
  cd "$REPO_ROOT"
  cargo build --release --locked -p openalpaca
)
locate_cli
echo "==> CLI: $OPENALPACA_BIN"

# beforeBuildCommand runs prepare-sidecar.ts again, as Tauri's hook: the
# host's triple (TAURI_ENV_TARGET_TRIPLE) and, with TAURI_ENV_DEBUG unset, the
# release profile; cargo finds the daemon fresh and re-stages the same bytes.
# The checks after the build hold the bundle to the hash recorded above,
# before `tauri build` ran.
echo "==> Building Tauri app bundle"
(
  cd "$GUI_DIR"
  bunx tauri build --bundles app
)

locate_app_bundle
echo "==> App bundle: $APP_BUNDLE_PATH"

echo "==> Verifying the bundled daemon against the staged sidecar"
BUNDLED_DAEMON="$APP_BUNDLE_PATH/Contents/MacOS/openalpacad"
[[ -f "$BUNDLED_DAEMON" ]] || die "Bundled daemon not found: $BUNDLED_DAEMON"
BUNDLED_SHA="$(sha256_of "$BUNDLED_DAEMON")"
[[ "$BUNDLED_SHA" == "$STAGED_SHA" ]] \
  || die "The daemon inside the app bundle ($BUNDLED_SHA) is not the staged sidecar ($STAGED_SHA). This check assumes an unsigned bundle: with APPLE_SIGNING_IDENTITY or APPLE_CERTIFICATE set, tauri signs the binaries and their bytes change."
[[ "$(read_sha256_file "$STAGED_SHA_FILE")" == "$STAGED_SHA" ]] \
  || die "The staged sidecar changed while the app bundle was built; rerun the script."

echo "==> Preparing staging directory"
rm -rf "$STAGING_DIR"
mkdir -p "$STAGING_DIR/bin" "$STAGING_DIR/libexec" "$STAGING_DIR/gui" "$STAGING_DIR/config"

install -m 755 "$OPENALPACA_BIN" "$STAGING_DIR/bin/openalpaca"
install -m 755 "$STAGED_SIDECAR" "$STAGING_DIR/libexec/openalpacad"
[[ "$(sha256_of "$STAGING_DIR/libexec/openalpacad")" == "$STAGED_SHA" ]] \
  || die "libexec/openalpacad does not match the staged sidecar."
cp -R "$APP_BUNDLE_PATH" "$STAGING_DIR/gui/openalpaca-gui.app"

# Never package repository runtime config directly (it may contain secrets).
cp -R "$REPO_ROOT/scripts/release/templates/config/." "$STAGING_DIR/config/"
install -m 755 "$REPO_ROOT/scripts/release/install.sh" "$STAGING_DIR/install.sh"
install -m 755 "$REPO_ROOT/scripts/release/uninstall.sh" "$STAGING_DIR/uninstall.sh"

cat >"$STAGING_DIR/manifest.json" <<EOF
{
  "name": "openalpaca",
  "version": "$VERSION",
  "target": "$HOST_TARGET",
  "built_at_utc": "$BUILT_AT_UTC",
  "git_sha": "$GIT_SHA"
}
EOF

echo "==> Packaging archive"
mkdir -p "$DIST_DIR"
tar -C "$STAGING_BASE" -czf "$ARCHIVE_PATH" "$PACKAGE_NAME"

SHA256_SUM="$(shasum -a 256 "$ARCHIVE_PATH" | awk '{print $1}')"
printf "%s  %s\n" "$SHA256_SUM" "$(basename "$ARCHIVE_PATH")" >"$SHA_PATH"

echo "Done."
echo "Archive: $ARCHIVE_PATH"
echo "SHA256 : $SHA_PATH"
