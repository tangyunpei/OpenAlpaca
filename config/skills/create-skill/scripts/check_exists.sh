#!/usr/bin/env bash
# check_exists.sh — Check if a skill ID already exists.
#
# Usage: check_exists.sh --id=<skill-id> [--scope=project]
#
# Outputs JSON: {"exists": true/false, "path": "..."}

set -euo pipefail

# The skills root, from the script's own location: the script is at
# <config>/skills/create-skill/scripts/, so the root is the skills directory
# this skill was loaded from, the one directory the daemon reads skills from.
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SKILLS_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

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
else
  echo "{\"exists\": false, \"path\": \"$SKILL_PATH\"}"
fi
