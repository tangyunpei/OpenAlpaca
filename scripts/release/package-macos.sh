#!/usr/bin/env bash
set -euo pipefail

die() {
  echo "ERROR: $*" >&2
  exit 1
}

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "Required command not found: $1"
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

[[ "$(uname -s)" == "Darwin" ]] || die "package-macos.sh must run on macOS."

require_cmd cargo
require_cmd rustc
require_cmd bun
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

GUI_DIR="$REPO_ROOT/apps/openalpaca-gui"
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
cargo build --release --locked -p openalpaca

# beforeBuildCommand runs prepare-sidecar.ts --release again; cargo finds the
# daemon fresh and re-stages the same bytes. The checks after the build hold
# the bundle to the hash recorded above, before `tauri build` ran.
echo "==> Building Tauri app bundle"
(
  cd "$GUI_DIR"
  bunx tauri build --bundles app
)

# The shell builds into target/gui (src-tauri/.cargo/config.toml); the other
# two are where an older tree put the bundle.
APP_BUNDLE_PATH=""
for candidate in \
  "$REPO_ROOT/target/gui/release/bundle/macos/openalpaca-gui.app" \
  "$REPO_ROOT/target/release/bundle/macos/openalpaca-gui.app" \
  "$REPO_ROOT/apps/openalpaca-gui/src-tauri/target/release/bundle/macos/openalpaca-gui.app"; do
  if [[ -d "$candidate" ]]; then
    APP_BUNDLE_PATH="$candidate"
    break
  fi
done
[[ -n "$APP_BUNDLE_PATH" ]] || die "Tauri app bundle not found under target/gui/, target/ or src-tauri/target/ (release/bundle/macos/)."

echo "==> Verifying the bundled daemon against the staged sidecar"
BUNDLED_DAEMON="$APP_BUNDLE_PATH/Contents/MacOS/openalpacad"
[[ -f "$BUNDLED_DAEMON" ]] || die "Bundled daemon not found: $BUNDLED_DAEMON"
BUNDLED_SHA="$(sha256_of "$BUNDLED_DAEMON")"
[[ "$BUNDLED_SHA" == "$STAGED_SHA" ]] \
  || die "The daemon inside the app bundle ($BUNDLED_SHA) is not the staged sidecar ($STAGED_SHA). This check assumes an unsigned bundle: with APPLE_SIGNING_IDENTITY or APPLE_CERTIFICATE set, tauri signs the binaries and their bytes change."
[[ "$(read_sha256_file "$STAGED_SHA_FILE")" == "$STAGED_SHA" ]] \
  || die "The staged sidecar changed while the app bundle was built; rerun the script."

OPENALPACA_BIN="$REPO_ROOT/target/release/openalpaca"
[[ -x "$OPENALPACA_BIN" ]] || die "Binary not found: $OPENALPACA_BIN"

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
