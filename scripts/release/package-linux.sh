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
require_cmd find
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

GUI_DIR="$REPO_ROOT/apps/openalpaca-gui"
STAGED_SIDECAR="$GUI_DIR/src-tauri/bin/openalpacad-$HOST_TARGET"
STAGED_SHA_FILE="$STAGED_SIDECAR.sha256"

sha256_of() {
  sha256sum "$1" | awk '{print $1}'
}

# The hex digest of a `sha256sum`-format file (`<hex>  <name>`), as
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

# beforeBuildCommand runs prepare-sidecar.ts again, as Tauri's hook: the
# host's triple (TAURI_ENV_TARGET_TRIPLE) and, with TAURI_ENV_DEBUG unset, the
# release profile; cargo finds the daemon fresh and re-stages the same bytes.
# The checks after the build hold libexec/ to the hash recorded above, before
# `tauri build` ran, and compare the AppImage's own copy with it (linuxdeploy
# rewrites that copy; see below).
echo "==> Building Tauri app bundle"
(
  cd "$GUI_DIR"
  bunx tauri build --bundles appimage
)

APPIMAGE_PATH=""
# The shell builds into target/gui (src-tauri/.cargo/config.toml); the other
# two are where an older tree put the bundle.
for candidate in \
  "$REPO_ROOT/target/gui/release/bundle/appimage/openalpaca-gui_"*.AppImage \
  "$REPO_ROOT/target/release/bundle/appimage/openalpaca-gui_"*.AppImage \
  "$REPO_ROOT/apps/openalpaca-gui/src-tauri/target/release/bundle/appimage/openalpaca-gui_"*.AppImage; do
  if [[ -f "$candidate" ]]; then
    APPIMAGE_PATH="$candidate"
    break
  fi
done
[[ -n "$APPIMAGE_PATH" ]] || die "Tauri AppImage not found."

# The daemon inside the AppImage: `--appimage-extract` unpacks the image into
# ./squashfs-root without FUSE and without running the app.
echo "==> Verifying the bundled daemon against the staged sidecar"
EXTRACT_DIR="$STAGING_BASE/appimage-extract"
rm -rf "$EXTRACT_DIR"
mkdir -p "$EXTRACT_DIR"
(
  cd "$EXTRACT_DIR"
  "$APPIMAGE_PATH" --appimage-extract >/dev/null
)
BUNDLED_DAEMON="$(find "$EXTRACT_DIR/squashfs-root" -type f -name openalpacad -print -quit)"
[[ -n "$BUNDLED_DAEMON" ]] || die "openalpacad not found inside the AppImage."
BUNDLED_SHA="$(sha256_of "$BUNDLED_DAEMON")"
# linuxdeploy, which tauri-bundler runs to make the AppImage, sets a RUNPATH
# ($ORIGIN/../lib) with patchelf on every dynamically linked ELF under usr/bin,
# so the bundled bytes are expected to differ from the staged ones (read in
# linuxdeploy's source, not yet seen on a Linux host). A difference is
# reported with the RUNPATH readelf finds, not fatal; the byte identity held on
# Linux is libexec/openalpacad == the staged sidecar, below.
if [[ "$BUNDLED_SHA" == "$STAGED_SHA" ]]; then
  echo "    the AppImage's daemon is the staged sidecar ($STAGED_SHA)"
else
  echo "WARNING: the daemon inside the AppImage ($BUNDLED_SHA) is not byte-identical to the staged sidecar ($STAGED_SHA); expected when linuxdeploy has set its RUNPATH:" >&2
  if command -v readelf >/dev/null 2>&1; then
    readelf -d "$BUNDLED_DAEMON" | grep -E '\((RUNPATH|RPATH)\)' | sed 's/^/    /' >&2 || echo "    (readelf finds no RUNPATH or RPATH)" >&2
  else
    echo "    (readelf not installed; RUNPATH not read)" >&2
  fi
fi
[[ "$(read_sha256_file "$STAGED_SHA_FILE")" == "$STAGED_SHA" ]] \
  || die "The staged sidecar changed while the AppImage was built; rerun the script."
rm -rf "$EXTRACT_DIR"

OPENALPACA_BIN="$REPO_ROOT/target/release/openalpaca"
[[ -x "$OPENALPACA_BIN" ]] || die "Binary not found: $OPENALPACA_BIN"

echo "==> Preparing staging directory"
rm -rf "$STAGING_DIR"
mkdir -p "$STAGING_DIR/bin" "$STAGING_DIR/libexec" "$STAGING_DIR/gui" "$STAGING_DIR/config"

install -m 755 "$OPENALPACA_BIN" "$STAGING_DIR/bin/openalpaca"
install -m 755 "$STAGED_SIDECAR" "$STAGING_DIR/libexec/openalpacad"
[[ "$(sha256_of "$STAGING_DIR/libexec/openalpacad")" == "$STAGED_SHA" ]] \
  || die "libexec/openalpacad does not match the staged sidecar."
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
