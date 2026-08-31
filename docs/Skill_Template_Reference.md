# Skill Template Reference

> Complete reference for writing `SKILL.md` skill definitions in OpenAlpaca.

## 1. Introduction

A **skill** is a discoverable workflow instruction package that tells the orchestrator *how* to handle a specific class of user requests. Each skill is defined by a `SKILL.md` file containing YAML frontmatter (configuration) and a Markdown body (instructions).

Skills fit into the orchestrator pipeline like this:

1. **Discovery** — The `SkillCatalog` scans the skills directory at startup and loads frontmatter (Level 1).
2. **Routing** — A message is matched to a skill by slash command, or scored by the `SkillRouter` using weighted intent/keyword matching.
3. **Invocation** — The winning skill's full `SKILL.md` is loaded on demand (Level 2), context is injected, and an agentic loop runs with skill-scoped tools and instructions.

This two-level progressive loading means:
- **Level 1 (startup)**: Only YAML frontmatter is parsed — lightweight catalog scan.
- **Level 2 (on-demand)**: Full markdown body + sections loaded when the skill is actually invoked.

Skills are **hot-reloadable**: the daemon's file watcher monitors the skills directory and reloads a skill's catalog entry when its `SKILL.md` changes, is renamed, or deleted (emitting a `SkillCatalogUpdated` event).

---

## 2. Directory Structure

```
config/skills/<skill-id>/
  SKILL.md           # REQUIRED — skill definition
  scripts/           # Optional — executable scripts exposed as callable tools
  examples/          # Optional — example inputs for testing
  templates/         # Optional — prompt or output templates
  assets/            # Optional — static data files
  README.md          # Optional — human-readable documentation
```

The **directory name is the canonical skill ID** (lowercased). Every direct child directory of the skills directory that contains a `SKILL.md` is treated as a skill.

### Scopes

The catalog API supports two scopes with project-overrides-user semantics (`SkillCatalog::scan_multi_scope`):

| Scope | Path | Priority |
|-------|------|----------|
| **Project** | `config/skills/` (relative to config dir) | Higher — overrides User scope |
| **User** | e.g. `~/.config/openalpaca/skills/` | Lower — fallback |

**In practice, the daemon currently loads only the project scope**: at startup it scans `<config-dir>/skills/` as `Project` scope and does not pass a user directory. Multi-scope discovery is implemented in the catalog but not wired into the daemon.

Skills with `invoke.mode: "disabled"` are skipped during the scan and never enter the catalog.

---

## 3. SKILL.md Format

A `SKILL.md` file has two parts:

1. **YAML frontmatter** between `---` delimiters
2. **Markdown body** with `## Section` headings

```markdown
---
name: My Skill
description: What this skill does
# ... more YAML fields ...
---

## Instructions

Step-by-step workflow...

## Style

Tone and formatting guidance...
```

**Required fields**: Only `name` and `description` are required (both must be non-empty).

**Unknown fields**: Silently ignored (no `deny_unknown_fields`). Old skill files with extra fields still parse.

**Malformed frontmatter**: If any declared field fails to deserialize (e.g. a `scripts:` entry missing `file`, `name`, or `description`), the whole frontmatter fails to parse and the **entire skill is not loaded** — a warning is logged in the daemon log (`SkillCatalog: failed to load ...: parse error: ...`).

---

## 4. Complete Field Reference

Every field is documented with its type, default value, and enforcement status:

- **ENFORCED** — Runtime code actively checks and acts on this field.
- **PARSED** — Deserialized and stored, but has no runtime effect.
- **DEPRECATED** — Parsed for backward compatibility; the catalog records a deprecation warning at scan time.

### 4.1 Top-Level Fields

| Field | Type | Default | Required | Status | Description |
|-------|------|---------|----------|--------|-------------|
| `id` | `Option<String>` | `None` | No | ENFORCED (warning) | Skill identifier. The directory name is the canonical ID; if `id` differs from the directory name, a validation warning is logged and recorded in the catalog's validation errors. |
| `name` | `String` | `""` | **Yes** | ENFORCED | Human-readable skill name. Must be non-empty. Used in catalog display and as the `name` attribute of the `<skill_context>` prompt block. Skills can also be looked up by name (case-insensitive). |
| `version` | `Option<String>` | `None` | No | PARSED | Semver version string (e.g., `"0.1.0"`). Informational only. |
| `description` | `String` | `""` | **Yes** | ENFORCED | What this skill does. Must be non-empty. Shown in catalog listings and the `<skill_context>` block. |
| `requires_capabilities` | `Vec<String>` | `[]` | No | ENFORCED | Capability identifiers used for capability-based tool resolution. **When non-empty, this replaces `tools.allow` entirely** — see Section 7. |
| `depends_on` | `Vec<String>` | `[]` | No | ENFORCED | Skill IDs this skill can delegate to. Each dependency becomes a synthetic `invoke_skill:<id>` tool — see Section 8 (Skill Composition). |
| `scripts` | `Vec<ScriptConfig>` | `[]` | No | ENFORCED | Bundled executable scripts exposed as `skill_script:<name>` tools — see Section 4.9. |

### 4.2 `invoke` — Invocation Configuration

Controls how the skill is triggered.

| Field | Type | Default | Status | Description |
|-------|------|---------|--------|-------------|
| `invoke.mode` | `String` | `"manual"` | ENFORCED | `"manual"` — only via slash command or explicit selection. `"auto"` — eligible for auto-routing. `"scheduled"` — cron-triggered via `invoke.cron` (a `"scheduled"` skill without a cron expression logs a warning and is never fired automatically). `"disabled"` — skipped at scan time, never enters the catalog. |
| `invoke.slash` | `Option<String>` | `None` | ENFORCED | Slash command (e.g., `"/review"`). Registered in the catalog's command index. The leading `/` is stripped and the command is lowercased for matching. |
| `invoke.aliases` | `Vec<String>` | `[]` | ENFORCED | Alternative slash commands. Maintained in a dedicated alias index; `get_by_command()` resolves aliases exactly like the primary slash command (leading `/` stripped, lowercased). |
| `invoke.cron` | `Option<String>` | `None` | ENFORCED | Cron expression (6/7-field, seconds first — e.g. `"0 0 9 * * *"` for 09:00 daily). The daemon registers a wake-scheduler job (`skill:<id>`) at boot and re-syncs it on skill hot-reload; each fire injects the skill's slash command (or `/<skill-id>` when no slash command is declared) as a fresh turn on the local user's `scheduled` lane. Invalid expressions are logged and skipped. Any skill with a cron expression is scheduled, regardless of `invoke.mode`. Gated globally by `[orchestrator.routing] scheduled_skills_enabled` in `daemon.toml` (default `true`). |
| `invoke.max_depth` | `usize` | `2` | PARSED | Intended maximum skill nesting depth. Currently parsed only — the nested-invocation executor hardcodes a maximum depth of 3 (see Section 8). |

Slash-command conflicts between skills (two skills claiming the same command) are detected at scan time; the later-loaded skill wins and a warning is logged.

Command resolution order: primary slash command → alias → skill ID (directory name). The ID fallback means `/<skill-id>` always invokes the skill deterministically, even without an explicit `invoke.slash`; explicit commands and aliases win on conflict.

### 4.3 `routing` — Auto-Routing Configuration

Controls how the skill is scored against user queries for automatic selection.

| Field | Type | Default | Status | Description |
|-------|------|---------|--------|-------------|
| `routing.intent` | `Vec<String>` | `[]` | ENFORCED | Intent phrases: the `SkillRouter` matches each phrase as a case-insensitive substring of the query for scoring. |
| `routing.keywords` | `Vec<String>` | `[]` | ENFORCED | Individual keywords matched as case-insensitive substrings. Score contribution is proportional: `(matched / total) * keyword_weight`. |
| `routing.negative_keywords` | `Vec<String>` | `[]` | ENFORCED | Keywords that penalize the score. Any match subtracts `negative_penalty` (default 0.6). |
| `routing.weights` | `ScoreWeights` | *(see below)* | ENFORCED | Per-skill weight overrides for the scoring formula. Alias: `routing.score` (backward compatibility). |

**`routing.weights` (alias: `routing.score`) sub-fields:**

| Field | Type | Default | Status | Description |
|-------|------|---------|--------|-------------|
| `routing.weights.base` | `f64` | `0.2` | ENFORCED | Base score awarded to every non-disabled skill. |
| `routing.weights.intent_weight` | `f64` | `0.45` | ENFORCED | Score added when any intent phrase matches. |
| `routing.weights.keyword_weight` | `f64` | `0.35` | ENFORCED | Maximum score from keyword matching (scaled by match ratio). |
| `routing.weights.recency_weight` | `f64` | `0.2` | ENFORCED | Score added if the skill was recently used. |
| `routing.weights.negative_penalty` | `f64` | `0.6` | ENFORCED | Score subtracted when any negative keyword matches. Configurable per skill. |

> Removed in the 2026-08-30 cleanup (unknown YAML keys are silently ignored, so old skills keep parsing): `invoke.hotkey`, `routing.examples`, `routing.weights.health_weight`, `context.read_when`, `permissions.confirm.message`, `permissions.sandbox.enabled`, `permissions.sandbox.fs_writable`, `tools.rate_limit.window_secs`, `tests.smoke`, and the `shell` context source type.

### 4.4 `context` — Context Injection

Defines external content to inject into the LLM prompt when the skill is invoked.

| Field | Type | Default | Status | Description |
|-------|------|---------|--------|-------------|
| `context.sources` | `Vec<ContextSource>` | `[]` | ENFORCED | List of context sources to inject (see source types below). |
| `context.summarize` | `SummarizeConfig` | `{enabled: false, max_tokens: null}` | DEPRECATED | Has no runtime effect. Setting `enabled: true` triggers a scan-time deprecation warning ("is deprecated and has no effect"). Use `context.budget_tokens` for context size control. |
| `context.budget_tokens` | `usize` | `0` | ENFORCED | Token budget for both the injected context and the skill-body prompt block (estimated as 1 token ≈ 4 characters). `0` means the default of 4,000 tokens (16,000 characters). |

**Context Source Types** (discriminated by `type` field):

```yaml
context:
  sources:
    - type: file            # Read a single file
      path: "assets/rules.md"
      max_bytes: 50000      # Default: 50,000

    - type: file_glob       # Read files matching a glob pattern
      pattern: "**/*.md"
      max_files: 10          # Default: 10
      max_bytes_each: 200000 # Default: 200,000

```

| Source Type | Required Fields | Optional Fields |
|-------------|----------------|-----------------|
| `file` | `path` | `max_bytes` (default: 50,000) |
| `file_glob` | `pattern` | `max_files` (default: 10), `max_bytes_each` (default: 200,000) |

The glob matcher is deliberately simple: it supports `**/` (any directory depth) and a single `*` wildcard per pattern; anything else is an exact path match.

### 4.5 `permissions` — Permission Model

Controls what the skill is allowed to do.

| Field | Type | Default | Status | Description |
|-------|------|---------|--------|-------------|
| `permissions.level` | `String` | `"readonly"` | ENFORCED | Permission tier. Valid values: `"readonly"`, `"readwrite"`, `"admin"`. Unknown values are rejected at preflight (invocation blocked). |
| `permissions.confirm` | `ConfirmAction` | `{tools: []}` | ENFORCED | Tools requiring user confirmation before execution. |
| `permissions.sandbox` | `SandboxConfig` | `{net: false}` | ENFORCED | `net` is enforced at preflight. |

**`permissions.confirm` sub-fields:**

| Field | Type | Default | Status | Description |
|-------|------|---------|--------|-------------|
| `permissions.confirm.tools` | `Vec<String>` | `[]` | ENFORCED | Tool names requiring confirmation. Passed to `SandboxPolicy.require_confirmation_for`. |

> **Interactive Confirmation**: When a tool listed in `confirm.tools` is invoked, the sandbox pauses execution and requests approval through the `ConfirmationBroker` (a `ToolConfirmationRequested` event routed to the active client; approvals come back via `POST /v1/chat/confirmations/{request_id}`). The approval timeout comes from `execution.agent_defaults.confirmation_timeout_secs` in `daemon.toml` (default 300s). Two escape hatches exist:
> - `security.auto_approve_confirmations = true` in `daemon.toml` auto-approves all confirmations (logged as `tool_auto_approved`).
> - If no `ConfirmationBroker` is available (headless/background execution), confirmation-required tools are **fail-closed** — blocked immediately.

**`permissions.sandbox` sub-fields:**

| Field | Type | Default | Status | Description |
|-------|------|---------|--------|-------------|
| `permissions.sandbox.net` | `bool` | `false` | ENFORCED | Preflight rejects the invocation when `tools.allow` contains `web_fetch`, permission level is `readwrite` or `admin`, and `net` is `false`. (The check inspects `tools.allow` only, not capability-resolved tools.) |

### 4.6 `tools` — Tool Configuration

Controls which tools the skill can use. **Ignored when `requires_capabilities` is non-empty** (except `tools.deny`, which always applies) — see Section 7.

| Field | Type | Default | Status | Description |
|-------|------|---------|--------|-------------|
| `tools.allow` | `Vec<String>` | `[]` | ENFORCED | Tool allowlist (legacy name-based path). Only these tools are available during the skill's agentic loop. Empty (with no `requires_capabilities`) means no tools. |
| `tools.deny` | `Vec<String>` | `[]` | ENFORCED | Tool denylist. Removed from the resolved tool set (both resolution paths), combined with the global deny list (`execution.skill_defaults.global_tool_deny` in `daemon.toml`). |
| `tools.defaults` | `HashMap<String, Value>` | `{}` | DEPRECATED | No runtime effect. A non-empty map triggers a scan-time deprecation warning. |
| `tools.rate_limit` | `RateLimitConfig` | `{max_calls: null}` | ENFORCED | `max_calls` is enforced as a total cap for the invocation. |

**`tools.rate_limit` sub-fields:**

| Field | Type | Default | Status | Description |
|-------|------|---------|--------|-------------|
| `tools.rate_limit.max_calls` | `Option<usize>` | `None` | ENFORCED | Propagated to `SandboxPolicy.max_tool_calls`: a **total cap on tool calls for the entire invocation** (there is no time window). |

### 4.7 `output` — Output Validation

Controls validation and constraints on the skill's output.

| Field | Type | Default | Status | Description |
|-------|------|---------|--------|-------------|
| `output.format` | `Option<String>` | `None` | ENFORCED | Expected output format: `"json"` and `"markdown"` trigger validation; `"text"`, unset, or any other value performs no validation. |
| `output.max_length` | `Option<usize>` | `None` | ENFORCED | Maximum output characters. **Hard truncation**: output longer than `max_length` is cut to exactly `max_length` characters (after validation/repair), with an info log. |
| `output.required_sections` | `Vec<String>` | `[]` | ENFORCED | Required H2 headings (e.g., `"Summary"` checks for `## Summary`). Only checked when `format` is `"markdown"`. |
| `output.max_tokens` | `Option<usize>` | `None` | ENFORCED (soft) | Maximum output tokens (estimated as chars/4). Logs a warning if exceeded but does **not** reject or truncate. |
| `output.auto_repair` | `bool` | `false` | ENFORCED | When `true`, validation failures trigger a deterministic repair attempt — see Section 9. |

### 4.8 `tests` — Test Metadata

| Field | Type | Default | Status | Description |
|-------|------|---------|--------|-------------|
| `tests.inputs` | `Vec<String>` | `[]` | PARSED | Test input strings. Not executed. |
| `tests.expect` | `ExpectConfig` | `{contains: [], format: null}` | PARSED | Expected output assertions. Not executed. |

**`tests.expect` sub-fields:**

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `tests.expect.contains` | `Vec<String>` | `[]` | Strings that must appear in the output. |
| `tests.expect.format` | `Option<String>` | `None` | Expected output format for assertion. |

### 4.9 `scripts` — Bundled Script Tools

Skills can bundle executable scripts in a `scripts/` directory and expose them as callable tools during invocation. The LLM can invoke these tools just like any built-in tool — they appear in the tool list alongside registry tools like `file_read`.

| Field | Type | Default | Status | Description |
|-------|------|---------|--------|-------------|
| `scripts` | `Vec<ScriptConfig>` | `[]` | ENFORCED | List of script tool declarations. Each entry maps a script file to a namespaced tool. |

**`ScriptConfig` fields:**

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `scripts[].file` | `String` | — (required) | Script filename relative to the skill's `scripts/` directory. |
| `scripts[].name` | `String` | — (required) | Tool name exposed to the LLM. Automatically prefixed with `skill_script:`. |
| `scripts[].description` | `String` | — (required) | Human-readable description shown in the tool definition. |
| `scripts[].parameters` | JSON Schema | `{"type": "object", "properties": {}}` | JSON Schema describing the tool's input parameters. |
| `scripts[].interpreter` | `Option<String>` | `None` | Interpreter to run the script (e.g., `"python3"`, `"node"`). If omitted, the script is executed directly — it must be executable (`chmod +x`) with a shebang. |
| `scripts[].timeout_secs` | `u64` | `30` | Maximum execution time in seconds before the script is killed. |

A missing required field (`file`, `name`, or `description`) is a YAML parse error that prevents the **whole skill** from loading.

**How it works:**

1. Scripts are declared in the `scripts:` frontmatter section and placed in `<skill-dir>/scripts/`.
2. At invocation time, each declared script becomes a tool named `skill_script:<name>`, wrapped in a `ScriptToolBuiltIn` and registered into a per-invocation **clone** of the tool registry. Path validation happens at this registration step — a missing or traversal-escaping script file fails the entire invocation.
3. When the LLM calls the tool, JSON arguments are converted to `--key=value` CLI arguments.
4. The script's stdout is returned as the tool result on success; on failure, exit code plus the first 500 characters of stderr.
5. Script tools are **skill-scoped** — they exist only in the cloned registry for a single invocation and are not visible to other skills, sub-agents, or the global tool registry.

**Execution flow:**

```
LLM calls skill_script:<name>({"key": "value", ...})
  → SandboxManager: capability check, confirmation gating, circuit breaker
    → per-invocation cloned ToolRegistry → ScriptToolBuiltIn
      → tokio::process::Command (direct exec — NOT through a shell)
        → args: ["--key=value", ...] (json_to_cli_args conversion)
        → cwd: skill directory (<skill-dir>/)
        → timeout: per-script (default 30s), enforced via tokio::time::timeout
      → stdout (capped at 512 KB) → Ok(result)
      → non-zero exit → Err("Script failed (exit N): <stderr, first 500 chars>")
```

**Argument conversion (`json_to_cli_args`):**

| JSON Type | CLI Format | Example |
|-----------|-----------|---------|
| `"string"` | `--key=value` | `{"path": "src/"}` → `--path=src/` |
| `true/false` | `--key=true/false` | `{"fix": true}` → `--fix=true` |
| `123` | `--key=123` | `{"count": 5}` → `--count=5` |
| nested object/array | `--key=<json>` | `{"data": [1,2]}` → `--data=[1,2]` |

**Working directory:**

Scripts run with their working directory set to the skill directory (`<skill-dir>/`). To locate the project root or other resources, scripts should resolve paths from their own location (`$0`) rather than relying on the working directory:

```bash
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
```

**Security:**

- **Path traversal prevention**: Script paths are canonicalized via `canonicalize()` and verified to resolve within the skill's `scripts/` directory. Paths that escape (including via symlinks) are blocked at registration time.
- **Timeout enforcement**: Each script has a configurable timeout (default 30s); timed-out scripts are killed and the tool call fails with `"Script timed out after {N}s"`.
- **Output cap**: stdout/stderr are capped at 512 KB to prevent memory pressure.
- **Namespace isolation**: The `skill_script:` prefix prevents collision with global registry tools; script tools never leak outside their skill invocation.
- **Sandbox integration**: Script tool names are added to `SandboxPolicy.allowed_capabilities` and pass through the full sandbox pipeline.
- **No shell injection**: Arguments are passed directly to the process (no shell), so shell metacharacters in argument values are inert.

> **Note**: Listing script tool names in `permissions.confirm.tools` prompts the user for approval each time the script tool is invoked. In non-interactive contexts (no broker available), the tool is blocked (fail-closed). See Section 7.

**Example:**

```yaml
scripts:
  - file: lint.sh
    name: lint_code
    description: "Run project linter on specified files"
    parameters:
      type: object
      properties:
        path:
          type: string
          description: "File or directory to lint"
        fix:
          type: boolean
          description: "Auto-fix issues"
      required: [path]
    timeout_secs: 60
  - file: analyze.py
    name: analyze_deps
    description: "Analyze dependency graph"
    interpreter: python3
    parameters:
      type: object
      properties:
        format:
          type: string
          enum: [json, text]
```

The LLM sees tools `skill_script:lint_code` and `skill_script:analyze_deps`. When it calls `skill_script:lint_code` with `{"path": "src/", "fix": true}`, the system executes:

```
<skill-dir>/scripts/lint.sh --path=src/ --fix=true
```

**Script best practices:**

- Make scripts executable (`chmod +x`) and include a shebang (`#!/usr/bin/env bash`), or set `interpreter` explicitly.
- Return structured output (JSON) on stdout so the LLM can parse results programmatically.
- Use `set -euo pipefail` in bash scripts for robust error handling.
- Resolve paths from `$0` (the script's own location), not from the working directory.
- Keep scripts focused — one script per task. Let the LLM orchestrate multi-step workflows.

For a working example, see the shipped `create-skill` skill (`config/skills/create-skill/`), which bundles four shell scripts.

### 4.10 Legacy Fields

These fields exist for backward compatibility with older skill definitions. They are parsed but **never serialized** (`#[serde(skip_serializing)]`). At parse time, they are mapped to their modern equivalents via `apply_legacy_compat()`.

| Legacy Field | Type | Default | Maps To | Condition |
|-------------|------|---------|---------|-----------|
| `command` | `Option<String>` | `None` | `invoke.slash` (with `/` prefix) | Only if `invoke.slash` is `None` |
| `auto_load` | `bool` | `false` | `invoke.mode = "auto"` | Only if `invoke.mode` is still `"manual"` |
| `trigger_patterns` | `Vec<String>` | `[]` | `routing.intent` | Only if `routing.intent` is empty |
| `tools_required` | `Vec<String>` | `[]` | `tools.allow` | Only if `tools.allow` is empty |
| `read_when` | `Vec<String>` | `[]` | *(dropped — parsed and ignored)* | — |

**Migration**: New-spec fields always take precedence over legacy fields. If both are present, legacy values are ignored.

---

## 5. Scoring & Auto-Routing

Slash commands short-circuit routing: a message starting with `/<command>` that matches a skill's `invoke.slash` or `invoke.aliases` invokes that skill directly, regardless of mode or score. The text after the command becomes the query (or the whole message if nothing follows).

For non-slash messages, the `SkillRouter` scores every non-disabled skill against the query using a weighted formula.

### Formula

```
score = base
      + (intent_match ? intent_weight : 0)
      + (keyword_ratio * keyword_weight)
      + (recency_bonus * recency_weight)
      - (negative_hit ? negative_penalty : 0)
```

| Term | Range | Description |
|------|-------|-------------|
| `base` | Fixed (default: 0.2) | Every skill gets this baseline score. |
| `intent_match` | Boolean | `true` if any `routing.intent` phrase is a case-insensitive substring of the query. |
| `keyword_ratio` | 0.0 – 1.0 | `matched_keywords / total_keywords`, each keyword tested as a case-insensitive substring. |
| `recency_bonus` | 0.0 or 1.0 | `1.0` if the skill is among the last 10 used skills (the recency window). |
| `negative_hit` | Boolean | `true` if any `routing.negative_keywords` word appears in the query. |

All five weights, including `negative_penalty` (default 0.6), are configurable per skill via `routing.weights`.

### Thresholds

| Threshold | Default | `daemon.toml` key | Behavior |
|-----------|---------|-------------------|----------|
| Auto-select | `0.65` | `execution.skill_defaults.router_auto_select_threshold` | Skill is auto-selected if `invoke.mode` is `"auto"` and no higher-scoring skill was already selected. |
| Suggest | `0.45` | `execution.skill_defaults.router_suggest_threshold` | Skill is offered as a suggestion. |
| Below suggest | — | — | Skill is excluded from results (still appears in debug scores). |

Both thresholds are clamped to [0, 1] at config load.

Auto-selection requires **all three conditions**:
1. Score >= auto-select threshold
2. `invoke.mode == "auto"`
3. No other skill has already been auto-selected (scores sorted descending; first qualifying skill wins)

An auto-selection emits a `SkillSelected` event with the score and a query preview.

### Worked Example: `code-review` Skill

Given this routing config:
```yaml
routing:
  intent: ["review code", "code review", "check for bugs", "review file"]
  keywords: ["review", "bugs", "style", "security"]
  negative_keywords: ["write", "create"]
  weights:
    base: 0.2
    intent_weight: 0.45
    keyword_weight: 0.35
    recency_weight: 0.2
```

| User Query | Intent? | Keywords | Negative? | Recency? | Score | Result |
|------------|---------|----------|-----------|----------|-------|--------|
| "please review code for me" | yes ("review code") | 1/4 = 0.25 | no | no | 0.2 + 0.45 + 0.09 = **0.74** | Auto-selected |
| "review code for bugs" | yes | 2/4 = 0.50 | no | no | 0.2 + 0.45 + 0.18 = **0.83** | Auto-selected |
| "check the style" | no | 1/4 = 0.25 | no | no | 0.2 + 0.09 = **0.29** | Skip |
| "write a new function" | no | 0/4 = 0.0 | yes ("write") | no | 0.2 - 0.6 = **-0.4** | Skip |
| "review code" (recently used) | yes | 1/4 = 0.25 | no | yes | 0.2 + 0.45 + 0.09 + 0.2 = **0.94** | Auto-selected |

---

## 6. Context Injection & Prompt Blocks

When a skill is invoked, two pieces of skill content enter the system prompt.

### Skill Body Block

The skill's markdown body (instructions) is rendered as an XML-tagged block:

```
<skill_context name="<skill-name>" description="<description>">
<body text>
</skill_context>
```

The body is truncated to `context.budget_tokens * 4` characters — **16,000 characters (~4,000 tokens) by default**. An empty body produces no block at all.

### Injected Context Sources

`context.sources` are resolved and assembled into a separate system block named `skill_context` in the compose engine, containing one section per source:

```
--- context: <relative-path> ---
<file contents>
--- context: <relative-path> ---
<file contents>
```

When context is injected, a `SkillContextInjected` event is emitted with the byte count.

### Source Resolution

1. **File**: The path is resolved relative to the skill directory (`config/skills/<skill-id>/`).
2. **FileGlob**: The pattern is matched against files within the skill directory tree (up to `max_files`).

### Security Rules

File-source injection enforces two path-traversal checks:

1. **Literal `..` check**: Any path containing `..` is immediately blocked.
2. **Canonical path validation**: The resolved path is canonicalized and verified to be within the skill directory. Paths that escape (e.g., via symlinks) are blocked.

Both checks log a warning and skip the offending source (no error returned).

### Budget Enforcement

The total injected context is capped at `budget_tokens * 4` characters (16,000 characters if `budget_tokens` is 0).

- Each source is read up to its `max_bytes` / `max_bytes_each` limit, further capped by the remaining budget.
- Sources are processed in order; once the budget is exhausted, remaining sources are skipped.

---

## 7. Tool Resolution & Permissions

### Permission Levels

| Level | Value | Description |
|-------|-------|-------------|
| **Readonly** | `"readonly"` | Default. |
| **Read-Write** | `"readwrite"` | Subject to the `sandbox.net` preflight check. |
| **Admin** | `"admin"` | Subject to the `sandbox.net` preflight check. |

An unknown permission level (e.g., `"superuser"`) causes a preflight error that **blocks skill invocation**: `Unknown permission level '<X>'. Valid levels: readonly, readwrite, admin`.

### Preflight Checks

Before a skill runs, `preflight_permissions()` validates:

1. `permissions.level` is one of the three valid values.
2. If `tools.allow` lists `web_fetch` and the permission level is `readwrite` or `admin`, then `permissions.sandbox.net` must be `true` — otherwise the invocation is rejected with `Skill requires web_fetch tool but sandbox.net is false`.

### Tool Set Algorithm

The invocation handler builds the tool set as follows:

1. **Capability path** — if `requires_capabilities` is non-empty: resolve tools via the registry's capability index (`tools_for_capabilities`) — every registered tool whose `provides_capabilities` includes one of the listed capabilities. **`tools.allow` is ignored on this path.**
2. **Legacy path** — otherwise, if `tools.allow` is non-empty: resolve each name against the tool registry. Unknown names trigger a warning (`Skill '<x>' references unknown tools: [...]`) but don't block invocation.
3. Otherwise: no tools (pure LLM prompt).
4. **Bootstrap exception**: during persona bootstrap mode, `update_persona` is force-added to the tool set even if not allowed.
5. **Deny lists**: remove tools in `tools.deny` and in the global deny list (`execution.skill_defaults.global_tool_deny` in `daemon.toml`). Applies to both resolution paths.
6. Append `skill_script:*` tools from `scripts` (Section 4.9) and `invoke_skill:*` tools from `depends_on` (Section 8).

Intent-suggested tools from the orchestrator are intentionally **not** merged, preserving skill-level tool isolation.

Capability names provided by built-in tools: `file_read`, `file_write`, `shell_execute`, `web_access` (both `web_search` and `web_fetch`), `memory_read` (memory_search), `workspace_read`, `workspace_write`, `messaging` (send).

### Loop Limits

The skill agentic loop runs with limits from `daemon.toml` `[execution.skill_defaults]`:

| Key | Default | Meaning |
|-----|---------|---------|
| `max_rounds` | `6` | Maximum LLM round-trips per invocation. |
| `max_tools_per_round` | `3` | Maximum tool calls per round. |
| `global_tool_deny` | `[]` | Tool names denied for all skills. |
| `router_auto_select_threshold` | `0.65` | See Section 5. |
| `router_suggest_threshold` | `0.45` | See Section 5. |

`tools.rate_limit.max_calls` (if set) additionally caps the total number of tool calls for the invocation via `SandboxPolicy.max_tool_calls`.

### Confirmation Tools

Tools listed in `permissions.confirm.tools` are passed to `SandboxPolicy.require_confirmation_for`. When such a tool is invoked:

- With a `ConfirmationBroker` available: execution pauses, a `ToolConfirmationRequested` event is emitted to the active client, and the user approves or denies (`POST /v1/chat/confirmations/{request_id}`). Timeout: 300 seconds (fixed for skill invocations). Denied or timed-out calls return an error.
- With `security.auto_approve_confirmations = true` in `daemon.toml`: automatically approved.
- With no broker (headless/background execution): **fail-closed** — the tool is blocked immediately.

---

## 8. Skill Composition (`depends_on`)

A skill can declare dependencies on other skills:

```yaml
depends_on:
  - explain-code
  - commit-message
```

Each dependency becomes a synthetic tool `invoke_skill:<id>` ("Invoke the '<name>' skill: <description>") taking a single required `query` string. When the LLM calls it, the dependency skill runs as a nested invocation via `SkillInvocationToolExecutor`, carrying:

- a **call stack** (`skill_stack`) of the invoking chain,
- a **hardcoded maximum nesting depth of 3** (`invoke.max_depth` is parsed but not consulted),
- the parent invocation's cost budget.

Dependency declarations are validated by `SkillCatalog::validate_dependencies()`: it reports references to non-existent skills and detects dependency cycles (`Cycle detected: 'a' -> 'b' -> 'a'`), recording errors in the catalog's validation list. This runs automatically after `scan_multi_scope()`; the daemon's startup path (`scan_directory`) does not run it, so in production a missing dependency surfaces as a warning at invocation time (`Skill '<x>' depends on '<y>' which is not in catalog`) and the `invoke_skill` tool is simply not registered.

---

## 9. Output Validation & Repair

After the agentic loop completes, the skill's output is validated against the `output` config.

### Format-Specific Validation

**`format: "markdown"`**
- Checks that every string in `required_sections` appears as an H2 heading (`## SectionName`).
- Missing sections produce a `MissingSections` validation error.

**`format: "json"`**
- Attempts to parse the output as JSON.
- If direct parsing fails, tries to extract JSON from a ```` ```json ```` code block or from the first `{` to the last `}` (the extracted JSON replaces the raw output on success).
- Invalid JSON produces an `InvalidJson` validation error.

**`format: "text"`, unset, or any other value**
- No validation performed.

### Failure Behavior

On validation failure:

- **`auto_repair: false` (default)** — logs `Skill '<name>' output validation failed: <error>. Passing through as-is.` and returns the original output unchanged.
- **`auto_repair: true`** — attempts a deterministic repair:
  - `MissingSections`: appends each missing heading with a placeholder body — `## <Section>` / `(Section not generated by skill.)` — and marks the repair successful.
  - `InvalidJson`: no deterministic fix is available (JSON extraction was already attempted during validation); the original output passes through.
  - `repair_attempted` / `repair_succeeded` are recorded in the invocation telemetry (see Section 10).

LLM-based self-repair (`repair_prompt()`) exists in the code but is not wired up.

### Soft `max_tokens` Warning and Hard `max_length` Truncation

- If `output.max_tokens` is set and the estimated token count (`chars / 4`) exceeds it, a warning is logged; the output is not modified.
- If `output.max_length` is set and the output is longer, it is **hard-truncated** to `max_length` characters (after validation/repair).

---

## 10. Observability & Hot Reload

### Lifecycle Events

Skill activity is published on the system event bus:

| Event | When |
|-------|------|
| `SkillSelected` | Router auto-selects a skill (includes score + query preview). |
| `SkillInvocationStarted` | Invocation begins. |
| `SkillCompleted` / `SkillFailed` | Invocation ends (duration, output preview / error). |
| `SkillContextInjected` | `context.sources` produced injected context (byte count). |
| `ContextBudgetComputed` | Token budget breakdown for the invocation's prompt. |
| `SkillCatalogUpdated` | A skill was hot-reloaded by the file watcher. |

### Execution Telemetry

Every invocation is persisted to the `skill_execution_log` SQLite table: status, finish reason, error message, validation failures, duration, rounds used, tool calls, input/output tokens, cost, model used, route score, `was_auto_selected`, `repair_attempted`, `repair_succeeded`, and (when `telemetry.store_query_preview` is enabled in `daemon.toml`) a 200-character query preview.

`GET /v1/skills/health` on the daemon returns per-skill health metrics aggregated from this table.

### Catalog Validation Errors

Scan-time warnings (deprecated fields in use, `id`/directory mismatches, slash-command conflicts, dependency errors) are collected in the catalog's validation-error list (capped at 100 entries), available via `SkillCatalog::validation_errors()`. They are also logged as warnings in the daemon log.

### Hot Reload

The daemon's file watcher monitors the skills directory. When a file under `config/skills/<id>/` changes, the catalog entry for that skill is reloaded (`reload_skill`): renamed or deleted skills are removed, changed skills are re-parsed, and a `SkillCatalogUpdated` event is emitted. Parse failures leave the previous entry removed and log a warning.

### Plugin-Backed Skills

The catalog also supports skills registered by plugins (`SkillSource::Plugin`, via `register_plugin_skill()`): they have no `SKILL.md` on disk. Invocation works like any other skill (slash command or router selection), but instead of running the LLM agentic loop the orchestrator delegates to the plugin's `PluginSkillExecutor` out-of-process; tool callbacks the plugin requests are proxied through the sandboxed execute path (capability checks, confirmation gating, timeouts). The plugin host system is early-stage; file-based skills are the only kind in practical use today.

---

## 11. Complete Annotated Example

A realistic skill using the available fields:

```yaml
---
# ── Top-level identification ──────────────────────────────────
id: api-security-audit           # Optional; must match the directory name (mismatch = warning)
name: API Security Audit         # REQUIRED — displayed in catalog
version: "1.0.0"                 # Informational semver
description: >                   # REQUIRED — used for catalog listings
  Analyze API endpoints for OWASP Top 10 vulnerabilities,
  authentication issues, and data exposure risks.

# ── Invocation ────────────────────────────────────────────────
invoke:
  mode: auto                     # auto | manual | scheduled | disabled
  slash: /security-audit         # Slash command (leading / stripped for matching)
  aliases:                       # Alternative commands — fully functional
    - /sec-audit
    - /audit-api

# ── Routing / Auto-selection ──────────────────────────────────
routing:
  intent:                        # Substring-scored AND regex-compiled as triggers
    - "security audit"
    - "check for vulnerabilities"
    - "OWASP check"
    - "API security"
  keywords:                      # Proportional scoring: matched/total * keyword_weight
    - "security"
    - "vulnerability"
    - "authentication"
    - "authorization"
    - "injection"
    - "OWASP"
  negative_keywords:             # Any match subtracts negative_penalty from the score
    - "frontend"
    - "CSS"
    - "design"
  weights:                       # Per-skill weight overrides (alias: score)
    base: 0.25                   # Slightly higher than default 0.2
    intent_weight: 0.45
    keyword_weight: 0.35
    recency_weight: 0.15         # Lower recency boost for security audits
    negative_penalty: 0.8        # Harsher penalty than default 0.6

# ── Context Injection ─────────────────────────────────────────
context:
  sources:
    - type: file                 # Inject a single reference file
      path: "assets/owasp_top10.md"
      max_bytes: 30000           # Cap at 30KB (default: 50,000)
    - type: file_glob            # Inject matching files from skill directory
      pattern: "templates/*.md"
      max_files: 5               # Default: 10
      max_bytes_each: 10000      # Default: 200,000
  budget_tokens: 3000            # Total context budget: 3000 * 4 = 12,000 chars

# ── Permissions ───────────────────────────────────────────────
permissions:
  level: readwrite               # readonly | readwrite | admin
  confirm:
    tools:                       # These tools require user confirmation
      - shell_execute
      - file_write
  sandbox:
    net: true                    # Required: tools.allow includes web_fetch

# ── Tool Configuration ────────────────────────────────────────
tools:
  allow:                         # Allowlist (used because requires_capabilities is absent)
    - file_read
    - file_write
    - shell_execute
    - web_fetch
    - memory_search
  deny:                          # Removed from allow (combined with global deny)
    - update_persona
  rate_limit:
    max_calls: 50                # Total tool-call cap for the invocation

# ── Output Validation ────────────────────────────────────────
output:
  format: markdown               # json | markdown trigger validation
  max_length: 10000              # Hard truncation to 10,000 chars
  required_sections:             # Required H2 headings (ENFORCED for markdown format)
    - "Critical Vulnerabilities"
    - "Warnings"
    - "Recommendations"
    - "Summary"
  max_tokens: 3000               # Soft limit — logs warning if exceeded
  auto_repair: true              # Append missing sections deterministically

# ── Bundled Scripts ──────────────────────────────────────────
scripts:                           # Scripts in <skill-dir>/scripts/ exposed as tools
  - file: scan_endpoints.sh        # Filename relative to scripts/ directory
    name: scan_endpoints           # LLM sees this as skill_script:scan_endpoints
    description: "Scan API endpoints and return a JSON listing"
    parameters:
      type: object
      properties:
        directory:
          type: string
          description: "Root directory to scan for route definitions"
        framework:
          type: string
          enum: [express, fastapi, actix, rails]
          description: "API framework to target"
      required: [directory]
    timeout_secs: 120              # 2 minutes for large codebases
  - file: check_deps.py            # Python script with explicit interpreter
    name: check_dependencies
    description: "Check project dependencies against known vulnerability databases"
    interpreter: python3           # Omit to execute directly (exec bit + shebang required)
    parameters:
      type: object
      properties:
        manifest:
          type: string
          description: "Path to package manifest (package.json, Cargo.toml, etc.)"
      required: [manifest]
---

## Instructions

You are a security auditor specialized in API security. Follow this workflow:

1. **Read** the target API files using `file_read`
2. **Identify** the framework and authentication mechanism
3. **Check** each endpoint against the OWASP Top 10
4. **Search** memory for past audit findings using `memory_search`
5. **Classify** each finding as Critical, High, Medium, or Low severity
6. **Produce** the structured report with all required sections

## Style

- Use clear, actionable language
- Include line numbers and code snippets for each finding
- Reference CWE numbers for each vulnerability type

## Output Rules

- Every finding must include: severity, CWE, affected file:line, description, remediation
- The Summary section must include a risk score (1-10) and top 3 priorities
- If no vulnerabilities found, state this explicitly in each required section
```

---

## 12. Minimal Examples

### Absolute Minimum

The smallest valid `SKILL.md`:

```yaml
---
name: Quick Answer
description: Give a brief, direct answer to the user's question
---

## Instructions

Answer the user's question in 1-3 sentences. Be direct and concise.
```

This skill:
- Defaults to `invoke.mode: "manual"` (no auto-routing)
- Has no slash command, no tools, no context
- Permission level defaults to `"readonly"`
- No output validation

### Practical Minimum

A realistic minimal skill with invocation and permissions (modeled on the shipped `code-review` skill):

```yaml
---
name: Explain Code
description: Explain what a piece of code does in plain language
invoke:
  mode: auto
  slash: /explain-code
routing:
  intent:
    - "explain code"
    - "what does this do"
  keywords:
    - "explain"
    - "understand"
requires_capabilities:
  - file_read
permissions:
  level: readonly
output:
  format: markdown
---

## Instructions

1. Read the specified code file
2. Provide a one-sentence summary
3. Walk through the core logic step by step
4. Note any non-obvious patterns or dependencies

## Style

Use plain language. Avoid jargon. Use analogies where helpful.
```

---

## 13. Best Practices

1. **Start with `readonly` permissions.** Only escalate to `readwrite` or `admin` when the skill genuinely needs to modify files or run commands.

2. **Prefer `requires_capabilities` over `tools.allow`.** Capability-based resolution survives tool renames and automatically picks up any tool providing the capability. Remember it *replaces* `tools.allow` entirely when set.

3. **Use `negative_keywords` to prevent false matches.** If your `code-review` skill shouldn't fire on "write code" queries, add `"write"` to `negative_keywords`; tune `weights.negative_penalty` if 0.6 is too strong or too weak.

4. **Set `required_sections` for structured output**, and consider `auto_repair: true` so missing sections are patched deterministically instead of silently passing through.

5. **Keep intent phrases natural — and regex-safe.** Phrases are substring-scored *and* compiled as case-insensitive regexes for trigger matching, so regex metacharacters (`?`, `(`, `+`, ...) change trigger behavior or drop the pattern entirely.

6. **Set `budget_tokens` for context-heavy skills.** Without it, both the skill body and injected context each default to ~4,000 tokens (16,000 characters), which may be too much or too little.

7. **Use `permissions.confirm.tools` for destructive operations.** In non-interactive contexts these tools are blocked (fail-closed) — plan for that in background/scheduled use.

8. **Enable `sandbox.net: true` when using `web_fetch`** with `readwrite`/`admin` — preflight rejects the invocation otherwise.

9. **Cap runaway loops with `tools.rate_limit.max_calls`.** It is a hard total-call budget for the invocation (no time window).

10. **Use the correct tool names.** Names are flat (`file_read`, `shell_execute`), not dot-namespaced. See Section 14 for the list.

11. **Use bundled scripts for deterministic operations.** When a skill needs filesystem scaffolding, linters, or external tools with specific flags, bundle scripts rather than relying on `shell_execute` with LLM-generated commands.

12. **Make scripts CWD-independent.** Scripts run with CWD set to the skill directory; resolve paths from `$0`:
    ```bash
    SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
    ```

13. **Return JSON from scripts** with status fields, paths, and error details so the LLM can act on results.

14. **Declare explicit `parameters` schemas for scripts** with descriptions and `required` fields so the LLM calls them correctly.

15. **Remember loop limits are low by default** (`max_rounds: 6`, `max_tools_per_round: 3` from `[execution.skill_defaults]`). Raise them in `daemon.toml` for tool-heavy skills.

---

## 14. Available Built-in Tools Reference

These are the tool names usable in `tools.allow` / `tools.deny`. Names must match exactly.

| Tool Name | Capability | Description | Availability |
|-----------|-----------|-------------|--------------|
| `file_read` | `file_read` | Read file contents from the workspace | Always registered |
| `file_write` | `file_write` | Write/create files in the workspace | Always registered |
| `shell_execute` | `shell_execute` | Execute shell commands | Always registered |
| `web_search` | `web_access` | Search the web | Always registered; needs the `[web_search]` section (API key) in `config/llm.toml` |
| `web_fetch` | `web_access` | Fetch and read content from a URL | Always registered |
| `memory_search` | `memory_read` | Search stored memories (hybrid FTS + vector when an embedder is configured) | Registered only when a database is available |
| `workspace_read` | `workspace_read` | Read entries from the shared task workspace | Always registered; requires database + task context at execution time |
| `workspace_write` | `workspace_write` | Write entries to the shared task workspace (32 KB per entry, optimistic-locking retries) | Always registered; requires database + task context at execution time |
| `update_persona` | `persona_write` | Update persona documents (SOUL.md, USER.md, or IDENTITY.md via `target` param) | Registered by the daemon with persona context |
| `send` | `messaging` | Send an outbound message through a connector channel (e.g. Telegram) | Registered only when a connector send provider is configured |

**`send` in skill loops** gets special handling: when `send` is in the resolved tool set, the loop's initial tool choice is forced to `send`, a send-context block (available channels) is injected into the prompt, and a post-hoc guard detects "hallucinated send" responses (confirmation text without an actual tool call) and replaces them with a warning asking the user to retry.

**Beyond built-ins**, the registry can also contain:
- **MCP tools** from `config/mcp.toml`, named `<server>__<tool>`;
- **Custom declarative tools** from `config/tools/*.toml` (HTTP-backed).

Both are addressable in `tools.allow` by their registered names.

### Script Tools (`skill_script:*`)

Script tools are **not** globally registered. They are created from a skill's `scripts:` frontmatter and registered into a per-invocation registry clone.

- **Namespace**: prefixed `skill_script:` (e.g., `skill_script:lint_code`).
- **Scope**: only the declaring skill's invocation can use them.
- **Do not list in `tools.allow`**: script tools are automatically available to their declaring skill; adding `skill_script:<name>` to `tools.allow` produces an "unknown tools" warning because registry lookup happens before script tools are attached.
- **Collision safety**: the prefix guarantees no collision with global tools — a script named `file_read` becomes `skill_script:file_read`.

### Nested Skill Tools (`invoke_skill:*`)

Created from `depends_on` (Section 8); like script tools, they are invocation-scoped and must not be listed in `tools.allow`.

---

## 15. Troubleshooting

### Common Errors

**"Missing field: name" / "Missing field: description"**
- Both `name` and `description` are required and must be non-empty strings.

**"Skill requires web_fetch tool but sandbox.net is false"**
- If `tools.allow` includes `web_fetch` and `permissions.level` is `readwrite` or `admin`, set `permissions.sandbox.net: true`.
- Alternatively keep `permissions.level: "readonly"` (readonly skills skip the `net` check).

**"Unknown permission level 'X'. Valid levels: readonly, readwrite, admin"**
- Only those three values are valid.

**Slash command or alias not found**
- The leading `/` is stripped and the command lowercased for matching; both `/review` and `review` resolve.
- A skill with `invoke.mode: "disabled"` is never loaded into the catalog at all.
- Check the daemon log for slash-command conflict warnings — another skill may have claimed the command.

**Skill not auto-selected despite high keyword match**
- Auto-selection requires `invoke.mode: "auto"` AND score >= the auto-select threshold (default 0.65, `execution.skill_defaults.router_auto_select_threshold`).
- Skills with `mode: "manual"` only appear as suggestions, never auto-select.
- Check for `negative_keywords` matches dragging the score down (by `weights.negative_penalty`, default 0.6).

**"Skill '<x>' references unknown tools: [...]" (warning in logs)**
- Use exact registered names (Section 14). This is the legacy `tools.allow` path; unknown names are skipped, not fatal.
- If `requires_capabilities` is set, `tools.allow` is ignored entirely — a common source of confusion when the tool set doesn't match expectations.
- `memory_search` requires a database; `send` requires a connector send provider.

**`routing.score` vs `routing.weights`**
- Both keys are accepted (serde alias). `weights` is canonical; `score` is kept for backward compatibility (and is what the shipped skills use).

**Intent phrase behaves oddly / never triggers**
- Phrases are compiled as case-insensitive regexes for trigger matching. Regex metacharacters are interpreted; an invalid pattern is dropped with a warning (`invalid trigger pattern ...`). Substring scoring by the router is unaffected.

**Context injection skips files silently**
- Check for `..` in paths (blocked) and that files resolve inside the skill directory (canonical path check).
- Check budget — once `budget_tokens * 4` characters are consumed, remaining sources are skipped.

**Output validation warning but output returned normally**
- Expected with `auto_repair: false`: validation failures log a warning and pass the output through. Set `auto_repair: true` to have missing markdown sections appended deterministically.

**Deprecation warnings at startup**
- `context.summarize.enabled: true` and non-empty `tools.defaults` produce "is deprecated and has no effect" warnings in the catalog's validation errors. Remove the fields.

**Legacy fields ignored when new-spec fields are present**
- Legacy compat only applies when the corresponding new field is empty/None. If both `command` and `invoke.slash` are set, `command` is ignored.

### Script-Related Errors

**"Script 'X' not found: ..." / "Scripts directory not found: ..."**
- Raised at invocation time when the declared file (or the `scripts/` directory) doesn't exist. This **fails the whole invocation**, not just the one tool. Check `config/skills/<id>/scripts/<file>`.

**"Script 'X' resolves outside scripts/ directory (path traversal blocked)"**
- The script path contains `../` or a symlink resolving outside `scripts/` after `canonicalize()`. Also fails the invocation.

**"Script timed out after Ns"**
- The script exceeded its `timeout_secs` (default 30). Increase the timeout in the `scripts:` declaration or optimize the script.

**"Script failed (exit N): ..."**
- Non-zero exit status; the first 500 characters of stderr are included. Debug manually: `cd config/skills/<id> && ./scripts/<file> --key=value`.

**Script runs but produces wrong paths / "file not found"**
- Scripts run with CWD set to the skill directory, not the project root. Resolve paths from `$0` (see Section 4.9).

**Script tool not appearing in the LLM tool list**
- Ensure the `scripts:` section is inside the YAML frontmatter (between `---` delimiters), not the markdown body.
- Each entry requires `file`, `name`, and `description`. A missing required field is a **frontmatter parse error — the entire skill fails to load** (check the daemon log for `SkillCatalog: failed to load ...`).

**Script output truncated**
- stdout is capped at 512 KB. Emit summaries, not full dumps; write large artifacts to files and return their paths.
