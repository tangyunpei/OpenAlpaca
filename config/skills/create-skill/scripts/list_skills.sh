#!/usr/bin/env bash
# list_skills.sh — List existing skills with their slash commands.
#
# Usage: list_skills.sh [--scope=project]
#
# Outputs JSON array of {id, name, slash, source, path}: the skills root's
# skills ("source": "config"), then the built-in ones ("source": "bundled"),
# which a skill of the same id in the skills root overrides.

set -euo pipefail

# The skills root, the one directory the daemon reads skills from: the daemon
# names it in OPENALPACA_SKILLS_DIR (<config>/skills) when it runs a skill
# script, since the built-in create-skill runs from its own copy under
# state/cache/bundled-skills/. Run by hand, the script finds it from its own
# location: at <config>/skills/create-skill/scripts/, the root is the skills
# directory this skill was loaded from.
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SKILLS_ROOT="${OPENALPACA_SKILLS_DIR:-$(cd "$SCRIPT_DIR/../.." && pwd)}"

# The built-in skills, which a skill of the same id overrides: where the daemon
# says they are (OPENALPACA_BUNDLED_SKILLS_DIR), else, run by hand, this
# script's own skills directory when that is not SKILLS_ROOT. None when the
# two are one.
OWN_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
BUNDLED_ROOT="${OPENALPACA_BUNDLED_SKILLS_DIR:-$OWN_ROOT}"
if [[ "$BUNDLED_ROOT" == "$SKILLS_ROOT" ]]; then
  BUNDLED_ROOT=""
fi

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
  local source_label="$2"
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
    echo -n "  {\"id\": \"$skill_id\", \"name\": \"$name\", \"slash\": \"$slash\", \"source\": \"$source_label\", \"path\": \"$skill_dir\"}"
  done
}

scan_dir "$SKILLS_ROOT" "config"
if [[ -n "$BUNDLED_ROOT" ]]; then
  scan_dir "$BUNDLED_ROOT" "bundled"
fi

echo ""
echo "]"
