#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Uninstall OpenAlpaca from macOS or Linux.

Usage:
  uninstall.sh [--prefix <dir>] [--app-dir <dir>] [--yes]

Options:
  --prefix <dir>   Where binaries were installed. Default: ~/.local/openalpaca
  --app-dir <dir>  Where the macOS app bundle was installed. Default: ~/Applications
                   (ignored on Linux)
  --yes            Non-interactive: skip confirmation prompt.
EOF
}

die() {
  echo "ERROR: $*" >&2
  exit 1
}

sed_inplace() {
  if [[ "$(uname -s)" == "Darwin" ]]; then
    sed -i '' "$@"
  else
    sed -i "$@"
  fi
}

# ── OS Detection ──────────────────────────────────────────────

OS="$(uname -s)"
case "$OS" in
  Darwin) PLATFORM="macos" ;;
  Linux)  PLATFORM="linux" ;;
  *)      die "Unsupported OS: $OS. On Windows, use uninstall-windows.ps1." ;;
esac

data_dir() {
  case "$PLATFORM" in
    macos) echo "$HOME/Library/Application Support/OpenAlpaca" ;;
    linux) echo "$HOME/.local/share/openalpaca" ;;
  esac
}

# ── Argument Parsing ─────────────────────────────────────────

PREFIX="$HOME/.local/openalpaca"
APP_DIR="$HOME/Applications"
NON_INTERACTIVE=0

while [[ $# -gt 0 ]]; do
  case "$1" in
    --prefix)
      [[ $# -ge 2 ]] || { echo "ERROR: --prefix requires a value" >&2; exit 1; }
      PREFIX="$2"; shift 2 ;;
    --app-dir)
      [[ $# -ge 2 ]] || { echo "ERROR: --app-dir requires a value" >&2; exit 1; }
      APP_DIR="$2"; shift 2 ;;
    --yes)
      NON_INTERACTIVE=1; shift ;;
    -h|--help)
      usage; exit 0 ;;
    *)
      echo "ERROR: Unknown argument: $1" >&2; exit 1 ;;
  esac
done

DATA_DIR="$(data_dir)"

echo "This will remove:"
echo "  Binaries:  $PREFIX"
if [[ "$PLATFORM" == "macos" ]]; then
  echo "  App:       $APP_DIR/openalpaca-gui.app"
elif [[ "$PLATFORM" == "linux" ]]; then
  echo "  GUI:       $PREFIX/gui/"
  echo "  Desktop:   $HOME/.local/share/applications/openalpaca-gui.desktop"
  echo "  Icon:      $HOME/.local/share/icons/hicolor/128x128/apps/openalpaca.png"
fi
echo "  Symlinks:  $HOME/.local/bin/openalpaca"
echo "  PATH block in ~/.zshrc and ~/.bashrc"
echo ""
echo "User data at $DATA_DIR will NOT be removed."

if [[ "$NON_INTERACTIVE" -ne 1 ]]; then
  read -r -p "Proceed with uninstall? [y/N] " confirm
  case "$confirm" in
    y|Y|yes|YES) ;;
    *) echo "Uninstall cancelled."; exit 0 ;;
  esac
fi

# ── Stop running daemon ──────────────────────────────────────

DISCOVERY_JSON="$DATA_DIR/discovery.json"
if [[ -f "$DISCOVERY_JSON" ]]; then
  pid="$(grep -Eo '"pid"[[:space:]]*:[[:space:]]*[0-9]+' "$DISCOVERY_JSON" | head -n1 | grep -Eo '[0-9]+' || true)"
  if [[ -n "$pid" ]] && kill -0 "$pid" >/dev/null 2>&1; then
    echo "Stopping running daemon (pid=$pid)..."
    kill "$pid" >/dev/null 2>&1 || true
    waited=0
    while kill -0 "$pid" >/dev/null 2>&1 && [ "$waited" -lt 10 ]; do
      sleep 1
      waited=$((waited + 1))
    done
  fi
fi

# ── Remove binaries ──────────────────────────────────────────

if [[ -d "$PREFIX" ]]; then
  rm -rf "$PREFIX"
  echo "Removed $PREFIX"
fi

# ── Remove GUI ────────────────────────────────────────────────

if [[ "$PLATFORM" == "macos" ]]; then
  if [[ -d "$APP_DIR/openalpaca-gui.app" ]]; then
    rm -rf "$APP_DIR/openalpaca-gui.app"
    echo "Removed $APP_DIR/openalpaca-gui.app"
  fi
elif [[ "$PLATFORM" == "linux" ]]; then
  # Desktop entry
  DESKTOP_FILE="$HOME/.local/share/applications/openalpaca-gui.desktop"
  if [[ -f "$DESKTOP_FILE" ]]; then
    rm -f "$DESKTOP_FILE"
    echo "Removed $DESKTOP_FILE"
  fi
  # Icon
  ICON_FILE="$HOME/.local/share/icons/hicolor/128x128/apps/openalpaca.png"
  if [[ -f "$ICON_FILE" ]]; then
    rm -f "$ICON_FILE"
    echo "Removed $ICON_FILE"
  fi
fi

# ── Remove symlinks ──────────────────────────────────────────

for link in openalpaca openalpacad; do
  target="$HOME/.local/bin/$link"
  if [[ -L "$target" || -e "$target" ]]; then
    rm -f "$target"
    echo "Removed $target"
  fi
done

# ── Remove PATH block from shell rc files ────────────────────

MARKER_BEGIN="# >>> openalpaca path >>>"
MARKER_END="# <<< openalpaca path <<<"
for rc in "$HOME/.zshrc" "$HOME/.bashrc"; do
  if [[ -f "$rc" ]] && grep -Fq "$MARKER_BEGIN" "$rc"; then
    sed_inplace "/$MARKER_BEGIN/,/$MARKER_END/d" "$rc"
    echo "Removed PATH block from $rc"
  fi
done

# ── Done ─────────────────────────────────────────────────────

echo ""
echo "OpenAlpaca has been uninstalled."
echo "Note: User data remains at $DATA_DIR"
echo "      Remove it manually if you want a complete cleanup."
