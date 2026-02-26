#!/usr/bin/env bash
set -euo pipefail

die() {
  echo "ERROR: $*" >&2
  exit 1
}

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "Required command not found: $1"
}

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
DIST_DIR="$REPO_ROOT/dist"

[[ "$(uname -s)" == "Linux" ]] || die "package-linux.sh must run on Linux."

require_cmd cargo
require_cmd rustc
require_cmd bun
require_cmd bunx
require_cmd tar
require_cmd sha256sum
require_cmd git
require_cmd awk
require_cmd sed
require_cmd date

HOST_TARGET="$(rustc -vV | awk '/^host:/ {print $2}')"
case "$HOST_TARGET" in
  x86_64-unknown-linux-gnu|aarch64-unknown-linux-gnu) ;;
  *) die "Unsupported host target for Linux packaging: $HOST_TARGET" ;;
esac

VERSION="$(cargo pkgid -p openalpaca | sed -E 's/.*#//')"
[[ -n "$VERSION" ]] || die "Failed to resolve package version."

GIT_SHA="$(git -C "$REPO_ROOT" rev-parse --short HEAD 2>/dev/null || echo "unknown")"
BUILT_AT_UTC="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
PACKAGE_NAME="openalpaca-linux-${HOST_TARGET}-v${VERSION}"
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
  bunx tauri build --bundles appimage
)

APPIMAGE_PATH=""
for candidate in \
  "$REPO_ROOT/target/release/bundle/appimage/openalpaca-gui_"*.AppImage \
  "$REPO_ROOT/apps/openalpaca-gui/src-tauri/target/release/bundle/appimage/openalpaca-gui_"*.AppImage; do
  if [[ -f "$candidate" ]]; then
    APPIMAGE_PATH="$candidate"
    break
  fi
done
[[ -n "$APPIMAGE_PATH" ]] || die "Tauri AppImage not found."

OPENALPACA_BIN="$REPO_ROOT/target/release/openalpaca"
OPENALPACAD_BIN="$REPO_ROOT/target/release/openalpacad"
[[ -x "$OPENALPACA_BIN" ]] || die "Binary not found: $OPENALPACA_BIN"
[[ -x "$OPENALPACAD_BIN" ]] || die "Binary not found: $OPENALPACAD_BIN"

echo "==> Preparing staging directory"
rm -rf "$STAGING_DIR"
mkdir -p "$STAGING_DIR/bin" "$STAGING_DIR/libexec" "$STAGING_DIR/gui" "$STAGING_DIR/config"

install -m 755 "$OPENALPACA_BIN" "$STAGING_DIR/bin/openalpaca"
install -m 755 "$OPENALPACAD_BIN" "$STAGING_DIR/libexec/openalpacad"
cp "$APPIMAGE_PATH" "$STAGING_DIR/gui/openalpaca-gui.AppImage"
chmod 755 "$STAGING_DIR/gui/openalpaca-gui.AppImage"

# Include icon for desktop entry creation during install
ICON_SRC="$REPO_ROOT/apps/openalpaca-gui/src-tauri/icons/128x128.png"
if [[ -f "$ICON_SRC" ]]; then
  cp "$ICON_SRC" "$STAGING_DIR/icon.png"
fi

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

SHA256_SUM="$(sha256sum "$ARCHIVE_PATH" | awk '{print $1}')"
printf "%s  %s\n" "$SHA256_SUM" "$(basename "$ARCHIVE_PATH")" >"$SHA_PATH"

echo "Done."
echo "Archive: $ARCHIVE_PATH"
echo "SHA256 : $SHA_PATH"
