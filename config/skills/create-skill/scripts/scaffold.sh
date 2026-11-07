#!/usr/bin/env bash
# scaffold.sh — Create the full directory structure for a new skill.
#
# Usage: scaffold.sh --id=<skill-id> [--scope=project] [--dirs=scripts,examples,templates,assets]
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

# The skills root, the one directory the daemon reads skills from: the daemon
# names it in OPENALPACA_SKILLS_DIR (<config>/skills) when it runs a skill
# script, since the built-in create-skill runs from its own copy under
# state/cache/bundled-skills/. Run by hand, the script finds it from its own
# location: at <config>/skills/create-skill/scripts/, the root is the skills
# directory this skill was loaded from.
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SKILLS_ROOT="${OPENALPACA_SKILLS_DIR:-$(cd "$SCRIPT_DIR/../.." && pwd)}"

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

# One root, so one scope: `user` wrote under ~/.config/openalpaca/skills,
# which nothing reads, and reported success.
if [[ "$SCOPE" != "project" ]]; then
  echo "Error: --scope=$SCOPE is refused: the daemon reads skills only from $SKILLS_ROOT (--scope=project, the default)" >&2
  exit 1
fi

if [[ -z "$SKILL_ID" ]]; then
  echo "Error: --id is required" >&2
  exit 1
fi

# Validate skill ID: lowercase kebab-case
if ! echo "$SKILL_ID" | grep -qE '^[a-z][a-z0-9]*(-[a-z0-9]+)*$'; then
  echo "Error: skill ID must be lowercase kebab-case (e.g., 'my-skill')" >&2
  exit 1
fi

# The chat memory commands are answered by the daemon before any skill, so a
# skill with one of their names would never be reached by its slash command.
case "$SKILL_ID" in
  remember|forget|memories)
    echo "Error: '$SKILL_ID' is reserved for a chat memory command and cannot be a skill ID" >&2
    exit 1
    ;;
esac

SKILL_DIR="$SKILLS_ROOT/$SKILL_ID"

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
