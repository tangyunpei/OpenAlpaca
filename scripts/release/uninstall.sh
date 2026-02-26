#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Uninstall OpenAlpaca from macOS.

Usage:
  uninstall.sh [--prefix <dir>] [--app-dir <dir>] [--yes]

Options:
  --prefix <dir>   Where binaries were installed. Default: ~/.local/openalpaca
  --app-dir <dir>  Where the app bundle was installed. Default: ~/Applications
  --yes            Non-interactive: skip confirmation prompt.
EOF
}

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

echo "This will remove:"
echo "  Binaries:  $PREFIX"
echo "  App:       $APP_DIR/openalpaca-gui.app"
echo "  Symlinks:  $HOME/.local/bin/openalpaca"
echo "  PATH block in ~/.zshrc and ~/.bashrc"
echo ""
echo "User data at ~/Library/Application Support/OpenAlpaca/ will NOT be removed."

if [[ "$NON_INTERACTIVE" -ne 1 ]]; then
  read -r -p "Proceed with uninstall? [y/N] " confirm
  case "$confirm" in
    y|Y|yes|YES) ;;
    *) echo "Uninstall cancelled."; exit 0 ;;
  esac
fi

# Stop running daemon
DISCOVERY_JSON="$HOME/Library/Application Support/OpenAlpaca/discovery.json"
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

# Remove binaries
if [[ -d "$PREFIX" ]]; then
  rm -rf "$PREFIX"
  echo "Removed $PREFIX"
fi

# Remove app bundle
if [[ -d "$APP_DIR/openalpaca-gui.app" ]]; then
  rm -rf "$APP_DIR/openalpaca-gui.app"
  echo "Removed $APP_DIR/openalpaca-gui.app"
fi

# Remove symlinks
for link in openalpaca openalpacad; do
  target="$HOME/.local/bin/$link"
  if [[ -L "$target" || -e "$target" ]]; then
    rm -f "$target"
    echo "Removed $target"
  fi
done

# Remove PATH block from shell rc files
MARKER_BEGIN="# >>> openalpaca path >>>"
MARKER_END="# <<< openalpaca path <<<"
for rc in "$HOME/.zshrc" "$HOME/.bashrc"; do
  if [[ -f "$rc" ]] && grep -Fq "$MARKER_BEGIN" "$rc"; then
    sed -i '' "/$MARKER_BEGIN/,/$MARKER_END/d" "$rc"
    echo "Removed PATH block from $rc"
  fi
done

echo ""
echo "OpenAlpaca has been uninstalled."
echo "Note: User data remains at ~/Library/Application Support/OpenAlpaca/"
echo "      Remove it manually if you want a complete cleanup."
