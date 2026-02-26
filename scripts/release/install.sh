#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Install OpenAlpaca on macOS or Linux (no Cargo required on target machine).

Usage:
  install.sh --file <archive.tar.gz> [--prefix <dir>] [--app-dir <dir>] [--yes]
  install.sh --url  <https://.../archive.tar.gz> [--prefix <dir>] [--app-dir <dir>] [--yes]

Options:
  --file <path>    Install from a local tar.gz archive.
  --url <url>      Install from a remote tar.gz URL.
  --prefix <dir>   Install binaries under this prefix. Default: ~/.local/openalpaca
  --app-dir <dir>  Install macOS app bundle under this directory. Default: ~/Applications
                   (ignored on Linux; GUI goes under prefix)
  --yes            Non-interactive overwrite of existing install.
EOF
}

die() {
  echo "ERROR: $*" >&2
  exit 1
}

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "Required command not found: $1"
}

read_manifest_value() {
  local key="$1"
  local manifest="$2"
  local value

  if command -v jq >/dev/null 2>&1; then
    value="$(jq -r ".[\"$key\"] // empty" "$manifest")"
  else
    # Fallback: grep+sed for single-key-per-line pretty-printed JSON
    value="$(grep -E "\"${key}\"[[:space:]]*:" "$manifest" | head -n1 | sed -E 's/.*:[[:space:]]*"([^"]+)".*/\1/')"
  fi

  [[ -n "$value" ]] || die "Could not read '${key}' from manifest: $manifest"
  printf '%s\n' "$value"
}

copy_missing_tree() {
  local src="$1"
  local dst="$2"

  [[ -d "$src" ]] || return 0
  mkdir -p "$dst"

  while IFS= read -r -d '' dir; do
    local rel="${dir#"$src"/}"
    if [[ "$dir" == "$src" ]]; then
      continue
    fi
    mkdir -p "$dst/$rel"
  done < <(find "$src" -type d -print0)

  while IFS= read -r -d '' file; do
    local rel="${file#"$src"/}"
    local target="$dst/$rel"
    if [[ -e "$target" ]]; then
      continue
    fi
    mkdir -p "$(dirname "$target")"
    cp "$file" "$target"
  done < <(find "$src" -type f -print0)
}

append_path_block() {
  local rc="$1"
  local marker_begin="# >>> openalpaca path >>>"
  local marker_end="# <<< openalpaca path <<<"

  touch "$rc"
  if grep -Fq "$marker_begin" "$rc"; then
    return 0
  fi

  {
    echo ""
    echo "$marker_begin"
    echo 'if [ -d "$HOME/.local/bin" ] && ! echo ":$PATH:" | grep -q ":$HOME/.local/bin:"; then'
    echo '  export PATH="$HOME/.local/bin:$PATH"'
    echo "fi"
    echo "$marker_end"
  } >>"$rc"
}

host_target() {
  local arch
  arch="$(uname -m)"
  case "$(uname -s)" in
    Darwin)
      case "$arch" in
        arm64)  echo "aarch64-apple-darwin" ;;
        x86_64) echo "x86_64-apple-darwin" ;;
        *) die "Unsupported macOS architecture: $arch" ;;
      esac
      ;;
    Linux)
      case "$arch" in
        x86_64)  echo "x86_64-unknown-linux-gnu" ;;
        aarch64) echo "aarch64-unknown-linux-gnu" ;;
        *) die "Unsupported Linux architecture: $arch" ;;
      esac
      ;;
    *)
      die "Unsupported OS: $(uname -s)"
      ;;
  esac
}

compute_sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

sed_inplace() {
  if [[ "$(uname -s)" == "Darwin" ]]; then
    sed -i '' "$@"
  else
    sed -i "$@"
  fi
}

data_dir() {
  case "$PLATFORM" in
    macos) echo "$HOME/Library/Application Support/OpenAlpaca" ;;
    linux) echo "$HOME/.local/share/openalpaca" ;;
  esac
}

verify_sha_if_available() {
  local archive_path="$1"
  local local_sha_file="$2"
  local expected=""

  if [[ -f "$local_sha_file" ]]; then
    expected="$(awk '{print $1}' "$local_sha_file" | head -n1)"
  fi

  if [[ -z "$expected" ]]; then
    return 0
  fi

  local actual
  actual="$(compute_sha256 "$archive_path")"
  [[ "$actual" == "$expected" ]] || die "SHA256 mismatch. expected=$expected actual=$actual"
  echo "Verified SHA256 checksum."
}

stop_running_daemon_if_any() {
  local discovery_json
  discovery_json="$(data_dir)/discovery.json"
  if [[ ! -f "$discovery_json" ]]; then
    return 0
  fi

  local pid=""
  pid="$(grep -Eo '"pid"[[:space:]]*:[[:space:]]*[0-9]+' "$discovery_json" | head -n1 | grep -Eo '[0-9]+' || true)"
  if [[ -z "$pid" ]]; then
    return 0
  fi

  if kill -0 "$pid" >/dev/null 2>&1; then
    echo "Stopping running daemon (pid=$pid)..."
    kill "$pid" >/dev/null 2>&1 || true
    local waited=0
    while kill -0 "$pid" >/dev/null 2>&1 && [ "$waited" -lt 10 ]; do
      sleep 1
      waited=$((waited + 1))
    done
    if kill -0 "$pid" >/dev/null 2>&1; then
      echo "Warning: daemon did not exit within 10s, proceeding anyway."
    fi
  fi
}

# ── OS Detection ──────────────────────────────────────────────

OS="$(uname -s)"
case "$OS" in
  Darwin) PLATFORM="macos" ;;
  Linux)  PLATFORM="linux" ;;
  *)      die "Unsupported OS: $OS. On Windows, use install-windows.ps1." ;;
esac

# ── Argument Parsing ─────────────────────────────────────────

SOURCE_FILE=""
SOURCE_URL=""
PREFIX="$HOME/.local/openalpaca"
APP_DIR="$HOME/Applications"
NON_INTERACTIVE=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --file)
      [[ $# -ge 2 ]] || die "--file requires a value"
      SOURCE_FILE="$2"
      shift 2
      ;;
    --url)
      [[ $# -ge 2 ]] || die "--url requires a value"
      SOURCE_URL="$2"
      shift 2
      ;;
    --prefix)
      [[ $# -ge 2 ]] || die "--prefix requires a value"
      PREFIX="$2"
      shift 2
      ;;
    --app-dir)
      [[ $# -ge 2 ]] || die "--app-dir requires a value"
      APP_DIR="$2"
      shift 2
      ;;
    --yes)
      NON_INTERACTIVE=1
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      die "Unknown argument: $1"
      ;;
  esac
done

[[ -n "$SOURCE_FILE" || -n "$SOURCE_URL" ]] || die "Provide either --file or --url."
[[ -z "$SOURCE_FILE" || -z "$SOURCE_URL" ]] || die "Use only one source: --file or --url."

require_cmd tar
require_cmd awk
require_cmd sed
require_cmd grep
if [[ -n "$SOURCE_URL" ]]; then
  require_cmd curl
fi

if [[ -n "$SOURCE_FILE" ]]; then
  [[ -f "$SOURCE_FILE" ]] || die "Archive not found: $SOURCE_FILE"
fi

TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/openalpaca-install-XXXXXX")"
trap 'rm -rf "$TMP_DIR"' EXIT

ARCHIVE_PATH="$TMP_DIR/openalpaca.tar.gz"
SHA_FILE_PATH="$TMP_DIR/openalpaca.tar.gz.sha256"

if [[ -n "$SOURCE_URL" ]]; then
  echo "Downloading archive from $SOURCE_URL ..."
  curl -fsSL "$SOURCE_URL" -o "$ARCHIVE_PATH"
  if curl -fsSL "${SOURCE_URL}.sha256" -o "$SHA_FILE_PATH"; then
    :
  else
    rm -f "$SHA_FILE_PATH"
  fi
else
  cp "$SOURCE_FILE" "$ARCHIVE_PATH"
  if [[ -f "${SOURCE_FILE}.sha256" ]]; then
    cp "${SOURCE_FILE}.sha256" "$SHA_FILE_PATH"
  fi
fi

verify_sha_if_available "$ARCHIVE_PATH" "$SHA_FILE_PATH"

echo "Extracting archive ..."
tar -xzf "$ARCHIVE_PATH" -C "$TMP_DIR" --strip-components=1
PACKAGE_ROOT="$TMP_DIR"
MANIFEST_PATH="$PACKAGE_ROOT/manifest.json"
[[ -f "$MANIFEST_PATH" ]] || die "manifest.json not found in archive."

MANIFEST_TARGET="$(read_manifest_value target "$MANIFEST_PATH")"
MANIFEST_VERSION="$(read_manifest_value version "$MANIFEST_PATH")"
HOST_TARGET="$(host_target)"
[[ "$MANIFEST_TARGET" == "$HOST_TARGET" ]] || die "Target mismatch: host=$HOST_TARGET package=$MANIFEST_TARGET"

# ── Confirm overwrite ────────────────────────────────────────

EXISTING_INSTALL=0
if [[ -e "$PREFIX/bin/openalpaca" ]]; then
  EXISTING_INSTALL=1
fi
if [[ "$PLATFORM" == "macos" && -e "$APP_DIR/openalpaca-gui.app" ]]; then
  EXISTING_INSTALL=1
fi
if [[ "$PLATFORM" == "linux" && -e "$PREFIX/gui/openalpaca-gui.AppImage" ]]; then
  EXISTING_INSTALL=1
fi

if [[ "$EXISTING_INSTALL" -eq 1 && "$NON_INTERACTIVE" -ne 1 ]]; then
  read -r -p "Existing installation detected. Overwrite binaries and app? [y/N] " confirm
  case "$confirm" in
    y|Y|yes|YES) ;;
    *) echo "Installation cancelled."; exit 0 ;;
  esac
fi

stop_running_daemon_if_any

# ── Install binaries ─────────────────────────────────────────

echo "Installing binaries to $PREFIX ..."
mkdir -p "$PREFIX/bin" "$PREFIX/libexec"
install -m 755 "$PACKAGE_ROOT/bin/openalpaca" "$PREFIX/bin/openalpaca"
install -m 755 "$PACKAGE_ROOT/libexec/openalpacad" "$PREFIX/libexec/openalpacad"

# ── Install GUI ──────────────────────────────────────────────

if [[ "$PLATFORM" == "macos" ]]; then
  echo "Installing GUI app to $APP_DIR ..."
  mkdir -p "$APP_DIR"
  rm -rf "$APP_DIR/openalpaca-gui.app"
  cp -R "$PACKAGE_ROOT/gui/openalpaca-gui.app" "$APP_DIR/openalpaca-gui.app"
elif [[ "$PLATFORM" == "linux" ]]; then
  GUI_DIR="$PREFIX/gui"
  echo "Installing GUI AppImage to $GUI_DIR ..."
  mkdir -p "$GUI_DIR"
  install -m 755 "$PACKAGE_ROOT/gui/"*.AppImage "$GUI_DIR/openalpaca-gui.AppImage" 2>/dev/null || true

  # Desktop entry
  DESKTOP_DIR="$HOME/.local/share/applications"
  mkdir -p "$DESKTOP_DIR"
  cat >"$DESKTOP_DIR/openalpaca-gui.desktop" <<DEOF
[Desktop Entry]
Type=Application
Name=OpenAlpaca
Comment=OpenAlpaca AI Assistant
Exec=$GUI_DIR/openalpaca-gui.AppImage
Icon=openalpaca
Terminal=false
Categories=Utility;
StartupWMClass=openalpaca-gui
DEOF

  # Icon
  if [[ -f "$PACKAGE_ROOT/icon.png" ]]; then
    ICON_DIR="$HOME/.local/share/icons/hicolor/128x128/apps"
    mkdir -p "$ICON_DIR"
    cp "$PACKAGE_ROOT/icon.png" "$ICON_DIR/openalpaca.png"
  fi
fi

# ── Install config ───────────────────────────────────────────

DATA_DIR="$(data_dir)"
CONFIG_DIR="$DATA_DIR/config"
mkdir -p "$CONFIG_DIR"
copy_missing_tree "$PACKAGE_ROOT/config" "$CONFIG_DIR"

# ── Symlink + PATH ───────────────────────────────────────────

mkdir -p "$HOME/.local/bin"
ln -sfn "$PREFIX/bin/openalpaca" "$HOME/.local/bin/openalpaca"

append_path_block "$HOME/.zshrc"
append_path_block "$HOME/.bashrc"

if [ -n "${SHELL:-}" ] && ! echo "$SHELL" | grep -qE '(bash|zsh)$'; then
  echo "Note: Your shell ($SHELL) is not bash/zsh."
  echo "      Add $HOME/.local/bin to your PATH manually to use 'openalpaca' from anywhere."
fi

# ── macOS-only: remove quarantine ────────────────────────────

if [[ "$PLATFORM" == "macos" ]]; then
  if command -v xattr >/dev/null 2>&1; then
    xattr -dr com.apple.quarantine "$PREFIX" "$APP_DIR/openalpaca-gui.app" >/dev/null 2>&1 || true
  fi
fi

# ── Summary ──────────────────────────────────────────────────

echo ""
echo "OpenAlpaca $MANIFEST_VERSION installed successfully."
echo "CLI: $HOME/.local/bin/openalpaca"
if [[ "$PLATFORM" == "macos" ]]; then
  echo "GUI: $APP_DIR/openalpaca-gui.app"
elif [[ "$PLATFORM" == "linux" ]]; then
  echo "GUI: $PREFIX/gui/openalpaca-gui.AppImage"
fi
echo "Config: $CONFIG_DIR"
echo "If this is your first install, restart your shell or run: export PATH=\"\$HOME/.local/bin:\$PATH\""
