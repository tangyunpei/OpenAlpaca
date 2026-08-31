#!/usr/bin/env bash
# write_skill_md.sh — Write a complete SKILL.md file from structured input.
#
# Usage: write_skill_md.sh --id=<id> --name=<name> --description=<desc> [options...]
#
# Required:
#   --id            Skill ID (directory name)
#   --name          Human-readable name
#   --description   Short description
#
# Optional:
#   --scope         project (default) or user
#   --mode          manual (default) or auto
#   --slash         Slash command (default: /<id>)
#   --level         Permission level: readonly (default), readwrite, admin
#   --tools         Comma-separated tool list (e.g., file_read,file_write)
#   --confirm_tools Comma-separated tools requiring confirmation
#   --net           true/false — sandbox network access (default: false)
#   --intent        Comma-separated intent phrases for auto-routing
#   --keywords      Comma-separated keywords for scoring
#   --neg_keywords  Comma-separated negative keywords
#   --format        Output format: text, json, markdown (default: markdown)
#   --sections      Comma-separated required output sections
#   --instructions  The instructions body text
#   --style         The style guidance text
#   --scripts_yaml  Raw YAML for scripts section (if skill has bundled scripts)

set -euo pipefail

# Resolve project root from script location
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"

# Defaults
SKILL_ID=""
NAME=""
DESCRIPTION=""
SCOPE="project"
MODE="manual"
SLASH=""
LEVEL="readonly"
TOOLS=""
CONFIRM_TOOLS=""
NET="false"
INTENT=""
KEYWORDS=""
NEG_KEYWORDS=""
FORMAT="markdown"
SECTIONS=""
INSTRUCTIONS=""
STYLE=""
SCRIPTS_YAML=""

for arg in "$@"; do
  case "$arg" in
    --id=*) SKILL_ID="${arg#--id=}" ;;
    --name=*) NAME="${arg#--name=}" ;;
    --description=*) DESCRIPTION="${arg#--description=}" ;;
    --scope=*) SCOPE="${arg#--scope=}" ;;
    --mode=*) MODE="${arg#--mode=}" ;;
    --slash=*) SLASH="${arg#--slash=}" ;;
    --level=*) LEVEL="${arg#--level=}" ;;
    --tools=*) TOOLS="${arg#--tools=}" ;;
    --confirm_tools=*) CONFIRM_TOOLS="${arg#--confirm_tools=}" ;;
    --net=*) NET="${arg#--net=}" ;;
    --intent=*) INTENT="${arg#--intent=}" ;;
    --keywords=*) KEYWORDS="${arg#--keywords=}" ;;
    --neg_keywords=*) NEG_KEYWORDS="${arg#--neg_keywords=}" ;;
    --format=*) FORMAT="${arg#--format=}" ;;
    --sections=*) SECTIONS="${arg#--sections=}" ;;
    --instructions=*) INSTRUCTIONS="${arg#--instructions=}" ;;
    --style=*) STYLE="${arg#--style=}" ;;
    --scripts_yaml=*) SCRIPTS_YAML="${arg#--scripts_yaml=}" ;;
    *) echo "Unknown argument: $arg" >&2; exit 1 ;;
  esac
done

# Validation
if [[ -z "$SKILL_ID" ]]; then echo "Error: --id is required" >&2; exit 1; fi
if [[ -z "$NAME" ]]; then echo "Error: --name is required" >&2; exit 1; fi
if [[ -z "$DESCRIPTION" ]]; then echo "Error: --description is required" >&2; exit 1; fi

# Determine path
if [[ "$SCOPE" == "user" ]]; then
  BASE_DIR="$HOME/.config/openalpaca/skills"
else
  BASE_DIR="$PROJECT_ROOT/config/skills"
fi
SKILL_DIR="$BASE_DIR/$SKILL_ID"
OUTPUT_FILE="$SKILL_DIR/SKILL.md"

# Default slash command
if [[ -z "$SLASH" ]]; then
  SLASH="/$SKILL_ID"
fi

# Ensure directory exists
mkdir -p "$SKILL_DIR"

# Build YAML frontmatter
{
  echo "---"
  echo "id: $SKILL_ID"
  echo "name: $NAME"
  echo "version: 0.1.0"
  echo "description: >"
  echo "  $DESCRIPTION"
  echo ""
  echo "invoke:"
  echo "  mode: $MODE"
  echo "  slash: $SLASH"
  echo ""

  # Routing (only for auto mode with content)
  if [[ "$MODE" == "auto" && ( -n "$INTENT" || -n "$KEYWORDS" || -n "$NEG_KEYWORDS" ) ]]; then
    echo "routing:"
    if [[ -n "$INTENT" ]]; then
      echo "  intent:"
      IFS=',' read -ra ITEMS <<< "$INTENT"
      for item in "${ITEMS[@]}"; do
        item="$(echo "$item" | xargs)"
        echo "    - \"$item\""
      done
    fi
    if [[ -n "$KEYWORDS" ]]; then
      echo "  keywords:"
      IFS=',' read -ra ITEMS <<< "$KEYWORDS"
      for item in "${ITEMS[@]}"; do
        item="$(echo "$item" | xargs)"
        echo "    - \"$item\""
      done
    fi
    if [[ -n "$NEG_KEYWORDS" ]]; then
      echo "  negative_keywords:"
      IFS=',' read -ra ITEMS <<< "$NEG_KEYWORDS"
      for item in "${ITEMS[@]}"; do
        item="$(echo "$item" | xargs)"
        echo "    - \"$item\""
      done
    fi
    echo ""
  fi

  # Permissions
  echo "permissions:"
  echo "  level: $LEVEL"
  if [[ -n "$CONFIRM_TOOLS" ]]; then
    echo "  confirm:"
    echo "    tools:"
    IFS=',' read -ra ITEMS <<< "$CONFIRM_TOOLS"
    for item in "${ITEMS[@]}"; do
      item="$(echo "$item" | xargs)"
      echo "      - $item"
    done
  fi
  if [[ "$NET" == "true" || "$LEVEL" != "readonly" ]]; then
    echo "  sandbox:"
    echo "    net: $NET"
  fi
  echo ""

  # Tools
  if [[ -n "$TOOLS" ]]; then
    echo "tools:"
    echo "  allow:"
    IFS=',' read -ra ITEMS <<< "$TOOLS"
    for item in "${ITEMS[@]}"; do
      item="$(echo "$item" | xargs)"
      echo "    - $item"
    done
    echo ""
  fi

  # Output
  echo "output:"
  echo "  format: $FORMAT"
  if [[ -n "$SECTIONS" ]]; then
    echo "  required_sections:"
    IFS=',' read -ra ITEMS <<< "$SECTIONS"
    for item in "${ITEMS[@]}"; do
      item="$(echo "$item" | xargs)"
      echo "    - \"$item\""
    done
  fi
  echo ""

  # Scripts (if provided)
  if [[ -n "$SCRIPTS_YAML" ]]; then
    echo "$SCRIPTS_YAML"
    echo ""
  fi

  echo "---"
  echo ""

  # Body
  echo "## Instructions"
  echo ""
  if [[ -n "$INSTRUCTIONS" ]]; then
    echo "$INSTRUCTIONS"
  else
    echo "TODO: Add step-by-step instructions for this skill."
  fi
  echo ""

  if [[ -n "$STYLE" ]]; then
    echo "## Style"
    echo ""
    echo "$STYLE"
    echo ""
  fi

} > "$OUTPUT_FILE"

echo "{"
echo "  \"status\": \"ok\","
echo "  \"path\": \"$OUTPUT_FILE\","
echo "  \"skill_id\": \"$SKILL_ID\","
echo "  \"name\": \"$NAME\","
echo "  \"slash\": \"$SLASH\""
echo "}"
