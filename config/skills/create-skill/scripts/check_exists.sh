#!/usr/bin/env bash
# check_exists.sh — Check if a skill ID already exists.
#
# Usage: check_exists.sh --id=<skill-id> [--scope=project]
#
# Outputs JSON: {"exists": true, "path": "..."} for a skill of the id in the
# skills root; {"exists": false, "bundled": true, "path": "..."} when only a
# built-in skill holds the id, so a new one there overrides it; else
# {"exists": false, "path": "..."}. The path is always where a skill of the id
# is (or would be written) in the skills root.

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

SKILL_ID=""
SCOPE="project"

for arg in "$@"; do
  case "$arg" in
    --id=*) SKILL_ID="${arg#--id=}" ;;
    --scope=*) SCOPE="${arg#--scope=}" ;;
    *) echo "Unknown argument: $arg" >&2; exit 1 ;;
  esac
done

# One root, so one scope: `user` looked under ~/.config/openalpaca/skills,
# which nothing reads, so it answered for a directory the daemon never loads.
if [[ "$SCOPE" != "project" ]]; then
  echo "Error: --scope=$SCOPE is refused: the daemon reads skills only from $SKILLS_ROOT (--scope=project, the default)" >&2
  exit 1
fi

if [[ -z "$SKILL_ID" ]]; then
  echo "Error: --id is required" >&2
  exit 1
fi

SKILL_PATH="$SKILLS_ROOT/$SKILL_ID/SKILL.md"

if [[ -f "$SKILL_PATH" ]]; then
  echo "{\"exists\": true, \"path\": \"$SKILL_PATH\"}"
elif [[ -n "$BUNDLED_ROOT" && -f "$BUNDLED_ROOT/$SKILL_ID/SKILL.md" ]]; then
  echo "{\"exists\": false, \"bundled\": true, \"path\": \"$SKILL_PATH\"}"
else
  echo "{\"exists\": false, \"path\": \"$SKILL_PATH\"}"
fi
