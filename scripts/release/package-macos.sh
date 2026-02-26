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

echo "==> Building Rust binaries"
cargo build --release -p openalpaca -p openalpacad

echo "==> Building Tauri app bundle"
(
  cd "$REPO_ROOT/apps/openalpaca-gui"
  bun install
  bunx tauri build --bundles app
)

APP_BUNDLE_PATH="$REPO_ROOT/target/release/bundle/macos/openalpaca-gui.app"
if [[ ! -d "$APP_BUNDLE_PATH" ]]; then
  APP_BUNDLE_PATH="$REPO_ROOT/apps/openalpaca-gui/src-tauri/target/release/bundle/macos/openalpaca-gui.app"
fi
[[ -d "$APP_BUNDLE_PATH" ]] || die "Tauri app bundle not found: $APP_BUNDLE_PATH"

OPENALPACA_BIN="$REPO_ROOT/target/release/openalpaca"
OPENALPACAD_BIN="$REPO_ROOT/target/release/openalpacad"
[[ -x "$OPENALPACA_BIN" ]] || die "Binary not found: $OPENALPACA_BIN"
[[ -x "$OPENALPACAD_BIN" ]] || die "Binary not found: $OPENALPACAD_BIN"

echo "==> Preparing staging directory"
rm -rf "$STAGING_DIR"
mkdir -p "$STAGING_DIR/bin" "$STAGING_DIR/libexec" "$STAGING_DIR/gui" "$STAGING_DIR/config"

install -m 755 "$OPENALPACA_BIN" "$STAGING_DIR/bin/openalpaca"
install -m 755 "$OPENALPACAD_BIN" "$STAGING_DIR/libexec/openalpacad"
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
