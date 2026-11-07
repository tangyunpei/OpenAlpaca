#!/usr/bin/env bash
# scaffold.sh — Create the full directory structure for a new skill.
#
# Usage: scaffold.sh --id=<skill-id> [--scope=project|user] [--dirs=scripts,examples,templates,assets]
#
# Creates:
#   config/skills/<id>/
#   config/skills/<id>/SKILL.md          (empty placeholder)
#   config/skills/<id>/scripts/          (if requested)
#   config/skills/<id>/examples/         (if requested)
#   config/skills/<id>/templates/        (if requested)
#   config/skills/<id>/assets/           (if requested)
#   config/skills/<id>/README.md         (if requested)

set -euo pipefail

# Resolve project root from script location:
# script is at <project>/config/skills/create-skill/scripts/scaffold.sh
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"

SKILL_ID=""
SCOPE="project"
DIRS=""
README="false"

for arg in "$@"; do
  case "$arg" in
    --id=*) SKILL_ID="${arg#--id=}" ;;
    --scope=*) SCOPE="${arg#--scope=}" ;;
    --dirs=*) DIRS="${arg#--dirs=}" ;;
    --readme=*) README="${arg#--readme=}" ;;
    *) echo "Unknown argument: $arg" >&2; exit 1 ;;
  esac
done

if [[ -z "$SKILL_ID" ]]; then
  echo "Error: --id is required" >&2
  exit 1
fi

# Validate skill ID: lowercase kebab-case
if ! echo "$SKILL_ID" | grep -qE '^[a-z][a-z0-9]*(-[a-z0-9]+)*$'; then
  echo "Error: skill ID must be lowercase kebab-case (e.g., 'my-skill')" >&2
  exit 1
fi

# Determine base path
if [[ "$SCOPE" == "user" ]]; then
  BASE_DIR="$HOME/.config/openalpaca/skills"
else
  BASE_DIR="$PROJECT_ROOT/config/skills"
fi

SKILL_DIR="$BASE_DIR/$SKILL_ID"

# Check for existing skill
if [[ -f "$SKILL_DIR/SKILL.md" ]]; then
  echo "Error: skill '$SKILL_ID' already exists at $SKILL_DIR/SKILL.md" >&2
  exit 1
fi

# Create main directory
mkdir -p "$SKILL_DIR"

# Create optional subdirectories
if [[ -n "$DIRS" ]]; then
  IFS=',' read -ra DIR_LIST <<< "$DIRS"
  for dir in "${DIR_LIST[@]}"; do
    dir="$(echo "$dir" | xargs)"  # trim whitespace
    case "$dir" in
      scripts|examples|templates|assets)
        mkdir -p "$SKILL_DIR/$dir"
        ;;
      *)
        echo "Warning: unknown directory '$dir' — skipping" >&2
        ;;
    esac
  done
fi

# Create README if requested
if [[ "$README" == "true" ]]; then
  cat > "$SKILL_DIR/README.md" << EOF
# $SKILL_ID

> TODO: Add skill documentation here.
EOF
fi

# Output summary as JSON
echo "{"
echo "  \"skill_id\": \"$SKILL_ID\","
echo "  \"path\": \"$SKILL_DIR\","
echo "  \"created_dirs\": ["

FIRST=true
echo -n "    \"$SKILL_DIR\""
if [[ -n "$DIRS" ]]; then
  IFS=',' read -ra DIR_LIST <<< "$DIRS"
  for dir in "${DIR_LIST[@]}"; do
    dir="$(echo "$dir" | xargs)"
    case "$dir" in
      scripts|examples|templates|assets)
        echo ","
        echo -n "    \"$SKILL_DIR/$dir\""
        ;;
    esac
  done
fi
echo ""
echo "  ],"
echo "  \"readme\": $README"
echo "}"
