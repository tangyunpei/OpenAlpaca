#!/usr/bin/env bash
# check_exists.sh — Check if a skill ID already exists.
#
# Usage: check_exists.sh --id=<skill-id> [--scope=project|user]
#
# Outputs JSON: {"exists": true/false, "path": "..."}

set -euo pipefail

# Resolve project root from script location
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"

SKILL_ID=""
SCOPE="project"

for arg in "$@"; do
  case "$arg" in
    --id=*) SKILL_ID="${arg#--id=}" ;;
    --scope=*) SCOPE="${arg#--scope=}" ;;
    *) echo "Unknown argument: $arg" >&2; exit 1 ;;
  esac
done

if [[ -z "$SKILL_ID" ]]; then
  echo "Error: --id is required" >&2
  exit 1
fi

if [[ "$SCOPE" == "user" ]]; then
  BASE_DIR="$HOME/.config/openalpaca/skills"
else
  BASE_DIR="$PROJECT_ROOT/config/skills"
fi

SKILL_PATH="$BASE_DIR/$SKILL_ID/SKILL.md"

if [[ -f "$SKILL_PATH" ]]; then
  echo "{\"exists\": true, \"path\": \"$SKILL_PATH\"}"
else
  echo "{\"exists\": false, \"path\": \"$SKILL_PATH\"}"
fi
