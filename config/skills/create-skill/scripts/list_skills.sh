#!/usr/bin/env bash
# list_skills.sh — List existing skills with their slash commands.
#
# Usage: list_skills.sh [--scope=project]
#
# Outputs JSON array of {id, name, slash, path}.

set -euo pipefail

# The skills root, from the script's own location: the script is at
# <config>/skills/create-skill/scripts/, so the root is the skills directory
# this skill was loaded from, the one directory the daemon reads skills from.
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SKILLS_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

SCOPE="project"

for arg in "$@"; do
  case "$arg" in
    --scope=*) SCOPE="${arg#--scope=}" ;;
    *) echo "Unknown argument: $arg" >&2; exit 1 ;;
  esac
done

# One root, so one scope: `user` and `both` listed ~/.config/openalpaca/skills,
# which nothing reads, as if the daemon loaded the skills there.
if [[ "$SCOPE" != "project" ]]; then
  echo "Error: --scope=$SCOPE is refused: the daemon reads skills only from $SKILLS_ROOT (--scope=project, the default)" >&2
  exit 1
fi

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

    # Name and slash from the frontmatter only: the lines after the opening
    # `---` up to the first line that is `---` and nothing else, as the
    # daemon's parser delimits it (one leading BOM and CRs dropped), with
    # `slash` read only as `invoke:`'s own key — never a `slash:` in the body
    # or under another key. The daemon's catalog stays the reader of record
    # (`openalpaca skills list`); this is the conflict hint the wizard shows.
    local fields name slash
    fields="$(awk '
      NR == 1 { sub(/^\357\273\277/, "") }
      { sub(/\r$/, "") }
      !open { if ($0 ~ /^[ \t]*$/) next; if ($0 !~ /^[ \t]*---/) exit; open = 1; next }
      /^---[ \t]*$/ { exit }
      /^[^ \t#]/ { section = $0; sub(/:.*/, "", section) }
      /^name:/ { name = $0; sub(/^name:[ \t]*/, "", name) }
      section == "invoke" && /^[ \t]+slash:/ { slash = $0; sub(/^[ \t]+slash:[ \t]*/, "", slash) }
      END { print name; print slash }
    ' "$skill_md" 2>/dev/null || true)"
    name="$(printf '%s\n' "$fields" | sed -n 1p | sed 's/^["'"'"']//; s/["'"'"']$//')"
    slash="$(printf '%s\n' "$fields" | sed -n 2p | sed 's/^["'"'"']//; s/["'"'"']$//')"
    [[ -n "$name" ]] || name="$skill_id"
    [[ -n "$slash" ]] || slash="/$skill_id"

    if [[ "$FIRST" == "true" ]]; then
      FIRST=false
    else
      echo ","
    fi
    echo -n "  {\"id\": \"$skill_id\", \"name\": \"$name\", \"slash\": \"$slash\", \"scope\": \"$scope_label\", \"path\": \"$skill_dir\"}"
  done
}

scan_dir "$SKILLS_ROOT" "project"

echo ""
echo "]"
