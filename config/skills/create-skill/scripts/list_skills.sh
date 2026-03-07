#!/usr/bin/env bash
# list_skills.sh — List existing skills with their slash commands.
#
# Usage: list_skills.sh [--scope=project|user|both]
#
# Outputs JSON array of {id, name, slash, path}.

set -euo pipefail

# Resolve project root from script location
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"

SCOPE="both"

for arg in "$@"; do
  case "$arg" in
    --scope=*) SCOPE="${arg#--scope=}" ;;
    *) echo "Unknown argument: $arg" >&2; exit 1 ;;
  esac
done

echo "["
FIRST=true

scan_dir() {
  local dir="$1"
  local scope_label="$2"
  if [[ ! -d "$dir" ]]; then return; fi

  for skill_dir in "$dir"/*/; do
    [[ -d "$skill_dir" ]] || continue
    local skill_md="$skill_dir/SKILL.md"
    [[ -f "$skill_md" ]] || continue

    local skill_id
    skill_id="$(basename "$skill_dir")"

    # Extract name and slash from frontmatter (simple grep)
    local name slash
    name="$(grep -m1 '^name:' "$skill_md" 2>/dev/null | sed 's/^name: *//' | sed 's/^"//' | sed 's/"$//' || echo "$skill_id")"
    slash="$(grep -m1 'slash:' "$skill_md" 2>/dev/null | sed 's/.*slash: *//' | sed 's/^"//' | sed 's/"$//' || echo "/$skill_id")"

    if [[ "$FIRST" == "true" ]]; then
      FIRST=false
    else
      echo ","
    fi
    echo -n "  {\"id\": \"$skill_id\", \"name\": \"$name\", \"slash\": \"$slash\", \"scope\": \"$scope_label\", \"path\": \"$skill_dir\"}"
  done
}

if [[ "$SCOPE" == "project" || "$SCOPE" == "both" ]]; then
  scan_dir "$PROJECT_ROOT/config/skills" "project"
fi

if [[ "$SCOPE" == "user" || "$SCOPE" == "both" ]]; then
  scan_dir "$HOME/.config/openalpaca/skills" "user"
fi

echo ""
echo "]"
