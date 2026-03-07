# Skill Template Reference

> Complete reference for writing `SKILL.md` skill definitions in OpenAlpaca.

## 1. Introduction

A **skill** is a discoverable workflow instruction package that tells the orchestrator *how* to handle a specific class of user requests. Each skill is defined by a `SKILL.md` file containing YAML frontmatter (configuration) and a Markdown body (instructions).

Skills fit into the orchestrator pipeline like this:

1. **Discovery** — The `SkillCatalog` scans skill directories at startup and loads frontmatter (Level 1).
2. **Routing** — The `SkillRouter` scores each skill against the user's query using weighted intent/keyword matching.
3. **Invocation** — The winning skill's full `SKILL.md` is loaded on demand (Level 2), context is injected, and the agentic loop runs with skill-scoped tools and instructions.

This two-level progressive loading means:
- **Level 1 (startup)**: Only YAML frontmatter is parsed — lightweight catalog scan.
- **Level 2 (on-demand)**: Full markdown body + sections loaded when the skill is actually invoked.

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

### Scopes

Skills are discovered from two locations, in order:

| Scope | Path | Priority |
|-------|------|----------|
| **Project** | `config/skills/` (relative to project root) | Higher — overrides User scope |
| **User** | `~/.config/openalpaca/skills/` | Lower — fallback |

If both scopes contain a skill with the same ID, the **Project** scope wins.

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

**Required fields**: Only `name` and `description` are required. All other fields have sensible defaults.

**Unknown fields**: Silently ignored (no `deny_unknown_fields`). This means old skill files with extra fields will still parse.

---

## 4. Complete Field Reference

Every field is documented with its type, default value, and enforcement status:

- **ENFORCED** — Runtime code actively checks and acts on this field.
- **PARSED** — Deserialized and stored, but no runtime enforcement yet (future work).

### 4.1 Top-Level Fields

| Field | Type | Default | Required | Status | Description |
|-------|------|---------|----------|--------|-------------|
| `id` | `Option<String>` | `None` | No | PARSED | Skill identifier. Not enforced at runtime; the directory name serves as the canonical ID. |
| `name` | `String` | `""` | **Yes** | ENFORCED | Human-readable skill name. Must be non-empty. Used in catalog display and the `### SKILL CONTEXT: <name> ###` prompt block. |
| `version` | `Option<String>` | `None` | No | PARSED | Semver version string (e.g., `"0.1.0"`). Informational only. |
| `description` | `String` | `""` | **Yes** | ENFORCED | What this skill does. Must be non-empty. Shown in skill catalog listings. |

### 4.2 `invoke` — Invocation Configuration

Controls how the skill is triggered.

| Field | Type | Default | Status | Description |
|-------|------|---------|--------|-------------|
| `invoke.mode` | `String` | `"manual"` | ENFORCED | `"manual"` — only via slash command or explicit selection. `"auto"` — eligible for auto-routing. `"scheduled"` — cron-triggered. `"disabled"` — excluded from catalog. |
| `invoke.slash` | `Option<String>` | `None` | ENFORCED | Slash command (e.g., `"/review"`). Registered in the catalog's command index. The leading `/` is stripped for matching. |
| `invoke.aliases` | `Vec<String>` | `[]` | PARSED | Alternative slash commands. Parsed but not yet wired to the command index. |
| `invoke.hotkey` | `Option<String>` | `None` | PARSED | Keyboard shortcut binding. Reserved for GUI integration. |
| `invoke.cron` | `Option<String>` | `None` | PARSED | Cron expression for `"scheduled"` mode. Reserved for wake system integration. |

### 4.3 `routing` — Auto-Routing Configuration

Controls how the skill is scored against user queries for automatic selection.

| Field | Type | Default | Status | Description |
|-------|------|---------|--------|-------------|
| `routing.intent` | `Vec<String>` | `[]` | ENFORCED | Phrases matched as case-insensitive substrings of the user query. Any match adds the full `intent_weight` to the score. |
| `routing.keywords` | `Vec<String>` | `[]` | ENFORCED | Individual keywords matched as case-insensitive substrings. Score contribution is proportional: `(matched / total) * keyword_weight`. |
| `routing.negative_keywords` | `Vec<String>` | `[]` | ENFORCED | Keywords that penalize the score. Any match subtracts a fixed `0.6` penalty. |
| `routing.weights` | `ScoreWeights` | *(see below)* | ENFORCED | Per-skill weight overrides for the scoring formula. Alias: `routing.score` (for backward compatibility). |
| `routing.examples` | `RoutingExamples` | `{positive: [], negative: []}` | PARSED | Example queries for intent classification. Reserved for future LLM-reranking. |

**`routing.weights` (alias: `routing.score`) sub-fields:**

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `routing.weights.base` | `f64` | `0.2` | Base score awarded to every non-disabled skill. |
| `routing.weights.intent_weight` | `f64` | `0.45` | Score added when any intent phrase matches. |
| `routing.weights.keyword_weight` | `f64` | `0.35` | Maximum score from keyword matching (scaled by match ratio). |
| `routing.weights.recency_weight` | `f64` | `0.2` | Score added if the skill was recently used. |

**`routing.examples` sub-fields:**

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `routing.examples.positive` | `Vec<String>` | `[]` | Queries that should trigger this skill. |
| `routing.examples.negative` | `Vec<String>` | `[]` | Queries that should NOT trigger this skill. |

### 4.4 `context` — Context Injection

Defines external content to inject into the LLM prompt when the skill is invoked.

| Field | Type | Default | Status | Description |
|-------|------|---------|--------|-------------|
| `context.sources` | `Vec<ContextSource>` | `[]` | ENFORCED | List of context sources to inject (see source types below). |
| `context.summarize` | `SummarizeConfig` | `{enabled: false, max_tokens: null}` | PARSED | Summarization of injected context. Not yet implemented. |
| `context.read_when` | `Vec<String>` | `[]` | PARSED | Controls when the skill description appears in the LLM catalog prompt. Not enforced at runtime. |
| `context.budget_tokens` | `usize` | `0` | ENFORCED | Maximum tokens for injected context (estimated as chars/4). `0` means default of 4000 tokens (16,000 chars). |

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

    - type: shell            # Run a shell command (DEFERRED — not yet implemented)
      command: "git log --oneline -5"
      max_bytes: 50000       # Default: 50,000
```

| Source Type | Required Fields | Optional Fields |
|-------------|----------------|-----------------|
| `file` | `path` | `max_bytes` (default: 50,000) |
| `file_glob` | `pattern` | `max_files` (default: 10), `max_bytes_each` (default: 200,000) |
| `shell` | `command` | `max_bytes` (default: 50,000) — **Not yet implemented** |

**`context.summarize` sub-fields:**

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `context.summarize.enabled` | `bool` | `false` | Enable context summarization before injection. |
| `context.summarize.max_tokens` | `Option<usize>` | `None` | Target token count for summarized context. |

### 4.5 `permissions` — Permission Model

Controls what the skill is allowed to do.

| Field | Type | Default | Status | Description |
|-------|------|---------|--------|-------------|
| `permissions.level` | `String` | `"readonly"` | ENFORCED | Permission tier. Valid values: `"readonly"`, `"readwrite"`, `"admin"`. Unknown values are rejected at preflight. |
| `permissions.confirm` | `ConfirmAction` | `{tools: [], message: null}` | ENFORCED | Tools requiring user confirmation before execution. |
| `permissions.sandbox` | `SandboxConfig` | `{enabled: false, net: false, fs_writable: []}` | ENFORCED (partial) | Sandbox restrictions. `net` is enforced at preflight; `enabled` and `fs_writable` are parsed only. See sub-fields below. |

**`permissions.confirm` sub-fields:**

| Field | Type | Default | Status | Description |
|-------|------|---------|--------|-------------|
| `permissions.confirm.tools` | `Vec<String>` | `[]` | ENFORCED | Tool names that require confirmation. Passed to `SandboxPolicy.require_confirmation_for`. **Warning:** Currently fail-closed — see note below. |
| `permissions.confirm.message` | `Option<String>` | `None` | PARSED | Custom confirmation prompt. Not yet displayed to users. |

> **Fail-Closed Confirmation Gate**: The confirmation mechanism is currently **fail-closed**. Tools listed in `confirm.tools` will be **blocked** during autonomous skill invocation because no interactive confirmation mechanism exists in the agentic loop. Only use `confirm.tools` if you intentionally want to prevent a tool from being called autonomously. A future interactive execution mode (CLI chat, GUI approval dialogs) will provide a confirmation callback.

**`permissions.sandbox` sub-fields:**

| Field | Type | Default | Status | Description |
|-------|------|---------|--------|-------------|
| `permissions.sandbox.enabled` | `bool` | `false` | PARSED | Whether sandboxing is active. The sandbox is policy-driven, not toggle-driven. |
| `permissions.sandbox.net` | `bool` | `false` | ENFORCED | Whether network access is allowed. Preflight rejects `web_fetch` in `tools.allow` when `net` is `false` and permission level is `readwrite` or `admin`. |
| `permissions.sandbox.fs_writable` | `Vec<String>` | `[]` | PARSED | Writable filesystem paths. Reserved for future sandbox enforcement. |

### 4.6 `tools` — Tool Configuration

Controls which tools the skill can use.

| Field | Type | Default | Status | Description |
|-------|------|---------|--------|-------------|
| `tools.allow` | `Vec<String>` | `[]` | ENFORCED | Tool allowlist. Only these tools are available during the skill's agentic loop. Empty means no tools. |
| `tools.deny` | `Vec<String>` | `[]` | ENFORCED | Tool denylist. Removed from the allow list before execution. Combined with the global deny list from `daemon.toml`. |
| `tools.defaults` | `HashMap<String, Value>` | `{}` | PARSED | Per-tool default parameters. Not yet injected at runtime (TODO). |
| `tools.rate_limit` | `RateLimitConfig` | `{max_calls: null, window_secs: null}` | PARSED | Per-skill tool rate limiting. Not yet enforced (TODO). |

**`tools.rate_limit` sub-fields:**

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `tools.rate_limit.max_calls` | `Option<usize>` | `None` | Maximum tool calls within the time window. |
| `tools.rate_limit.window_secs` | `Option<u64>` | `None` | Time window in seconds for rate limiting. |

### 4.7 `output` — Output Validation

Controls validation and constraints on the skill's output.

| Field | Type | Default | Status | Description |
|-------|------|---------|--------|-------------|
| `output.format` | `Option<String>` | `None` | ENFORCED | Expected output format: `"text"`, `"json"`, `"markdown"`. Triggers format-specific validation. |
| `output.max_length` | `Option<usize>` | `None` | PARSED | Maximum output characters. Not yet enforced (TODO). |
| `output.required_sections` | `Vec<String>` | `[]` | ENFORCED | Required H2 headings (e.g., `"Summary"` checks for `## Summary`). Only checked when `format` is `"markdown"`. |
| `output.max_tokens` | `Option<usize>` | `None` | ENFORCED (soft) | Maximum output tokens (estimated as chars/4). Logs a warning if exceeded but does **not** reject the output. |

### 4.8 `tests` — Smoke Testing

Defines test inputs and expected outputs for the skill.

| Field | Type | Default | Status | Description |
|-------|------|---------|--------|-------------|
| `tests.inputs` | `Vec<String>` | `[]` | PARSED | Test input strings for the smoke test runner. |
| `tests.expect` | `ExpectConfig` | `{contains: [], format: null}` | PARSED | Expected output assertions. |
| `tests.smoke` | `Vec<String>` | `[]` | PARSED | Paths to smoke test input files. |

**`tests.expect` sub-fields:**

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `tests.expect.contains` | `Vec<String>` | `[]` | Strings that must appear in the output. |
| `tests.expect.format` | `Option<String>` | `None` | Expected output format for assertion. |

### 4.9 `scripts` — Bundled Script Tools

Skills can bundle executable scripts in a `scripts/` directory and expose them as callable tools during invocation. The LLM agent can invoke these tools just like any built-in tool — they appear in the tool list alongside registry tools like `file_read`.

| Field | Type | Default | Status | Description |
|-------|------|---------|--------|-------------|
| `scripts` | `Vec<ScriptConfig>` | `[]` | ENFORCED | List of script tool declarations. Each entry maps a script file to a namespaced tool. |

**`ScriptConfig` fields:**

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `scripts[].file` | `String` | — (required) | Script filename relative to the skill's `scripts/` directory. |
| `scripts[].name` | `String` | — (required) | Tool name exposed to the LLM. Automatically prefixed with `skill_script:`. |
| `scripts[].description` | `String` | — (required) | Human-readable description shown in the tool definition. |
| `scripts[].parameters` | `JSON Schema` | `{"type": "object", "properties": {}}` | JSON Schema describing the tool's input parameters. |
| `scripts[].interpreter` | `Option<String>` | `None` | Interpreter to run the script (e.g., `"python3"`, `"node"`). If omitted, the script is executed directly (must be executable with a shebang). |
| `scripts[].timeout_secs` | `u64` | `30` | Maximum execution time in seconds before the script is killed. |

**How it works:**

1. Scripts are declared in the `scripts:` frontmatter section and placed in `<skill-dir>/scripts/`.
2. At invocation time, each declared script becomes a tool named `skill_script:<name>`.
3. When the LLM calls the tool, JSON arguments are converted to `--key=value` CLI arguments.
4. The script's stdout is returned as the tool result on success; stderr is returned on failure.
5. Script tools are **skill-scoped** — they only exist during a single skill invocation and are not visible to other skills, sub-agents, or the global tool registry.

**Execution flow:**

```
LLM calls skill_script:<name>({"key": "value", ...})
  → SandboxManager: capability check, input sanitization, circuit breaker
    → ContextualToolExecutor: intercepts skill_script: prefix
      → ScriptExecutionContext: resolves path, builds Command
        → tokio::process::Command (NOT through a shell — direct execve)
          → args: ["--key=value", ...] (json_to_cli_args conversion)
          → cwd: skill directory (<skill-dir>/)
          → timeout: per-script configurable (default 30s)
        → stdout → Ok(result) / stderr → Err(message)
```

**Argument conversion (`json_to_cli_args`):**

| JSON Type | CLI Format | Example |
|-----------|-----------|---------|
| `"string"` | `--key=value` | `{"path": "src/"}` → `--path=src/` |
| `true/false` | `--key=true/false` | `{"fix": true}` → `--fix=true` |
| `123` | `--key=123` | `{"count": 5}` → `--count=5` |
| nested object/array | `--key=<json>` | `{"data": [1,2]}` → `--data=[1,2]` |

**Working directory:**

Scripts run with their working directory set to the skill directory (`<skill-dir>/`). To locate the project root or other resources, scripts should resolve paths from their own location (`$0`) rather than relying on the working directory. Example:

```bash
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
```

**Security:**

- **Path traversal prevention**: Script paths are canonicalized via `canonicalize()` and verified to resolve within the `scripts/` directory. Paths containing `../` that escape the directory are blocked.
- **Timeout enforcement**: Each script has a configurable timeout (default 30s), enforced via `tokio::time::timeout`. Timed-out scripts are killed.
- **Namespace isolation**: The `skill_script:` prefix prevents collision with global registry tools. Script tools never leak outside their skill invocation.
- **Sandbox integration**: Script tool names are added to `SandboxPolicy.allowed_capabilities` and pass through the full sandbox pipeline (capability check → input sanitization → circuit breaker → timeout).
- **No shell injection**: Arguments are passed directly via `execve` (not through a shell), so shell metacharacters in argument values are inert.

> **Important**: Do NOT list script tools in `permissions.confirm.tools`. The confirmation gate is fail-closed in autonomous mode — confirmed tools are **blocked**, not prompted. See the [Permissions Model](#7-permissions-model) section.

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

The LLM sees tools `skill_script:lint_code` and `skill_script:analyze_deps`. When the LLM calls `skill_script:lint_code` with `{"path": "src/", "fix": true}`, the system executes:

```
<skill-dir>/scripts/lint.sh --path=src/ --fix=true
```

**Script best practices:**

- Make scripts executable (`chmod +x`) and include a shebang (`#!/usr/bin/env bash`).
- Return structured output (JSON) on stdout so the LLM can parse results programmatically.
- Use `set -euo pipefail` in bash scripts for robust error handling.
- Resolve paths from `$0` (the script's own location), not from the working directory, to ensure CWD-independence.
- Keep scripts focused — one script per task. Let the LLM orchestrate multi-step workflows by calling scripts in sequence.

### 4.10 Legacy Fields

These fields exist for backward compatibility with older skill definitions. They are parsed but **never serialized** (`#[serde(skip_serializing)]`). At parse time, they are mapped to their modern equivalents via `apply_legacy_compat()`.

| Legacy Field | Type | Default | Maps To | Condition |
|-------------|------|---------|---------|-----------|
| `command` | `Option<String>` | `None` | `invoke.slash` (with `/` prefix) | Only if `invoke.slash` is `None` |
| `auto_load` | `bool` | `false` | `invoke.mode = "auto"` | Only if `invoke.mode` is still `"manual"` |
| `trigger_patterns` | `Vec<String>` | `[]` | `routing.intent` | Only if `routing.intent` is empty |
| `tools_required` | `Vec<String>` | `[]` | `tools.allow` | Only if `tools.allow` is empty |
| `read_when` | `Vec<String>` | `[]` | `context.read_when` | Only if `context.read_when` is empty |

**Migration**: New-spec fields always take precedence over legacy fields. If both are present, legacy values are ignored.

---

## 5. Scoring & Auto-Routing

The `SkillRouter` scores every non-disabled skill against each user query using a weighted formula.

### Formula

```
score = base
      + (intent_match ? intent_weight : 0)
      + (keyword_ratio * keyword_weight)
      + (recency_bonus * recency_weight)
      - (negative_hit ? 0.6 : 0)
```

| Term | Range | Description |
|------|-------|-------------|
| `base` | Fixed (default: 0.2) | Every skill gets this baseline score. |
| `intent_match` | Boolean | `true` if any `routing.intent` phrase is a case-insensitive substring of the query. |
| `keyword_ratio` | 0.0 – 1.0 | `matched_keywords / total_keywords`. Each keyword is tested as a case-insensitive substring. |
| `recency_bonus` | 0.0 or 1.0 | `1.0` if the skill was used within the last 10 invocations (the recency window). |
| `negative_hit` | Boolean | `true` if any `routing.negative_keywords` word is found in the query. |

The **negative penalty** of `0.6` is a compile-time constant and cannot be configured per-skill.

### Thresholds

| Threshold | Default Value | Behavior |
|-----------|---------------|----------|
| Auto-select | `>= 0.65` | Skill is automatically selected if `invoke.mode` is `"auto"` and no other skill was already selected. |
| Suggest | `>= 0.45` | Skill is offered as a suggestion to the user. |
| Skip | `< 0.45` | Skill is excluded from results (still appears in debug scores). |

Auto-selection requires **all three conditions**:
1. Score >= 0.65
2. `invoke.mode == "auto"`
3. No other skill has already been auto-selected (first match wins; scores are sorted descending)

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

## 6. Context Injection

When a skill is invoked, its `context.sources` are resolved and injected into the system prompt.

### Source Resolution

1. **File**: The path is resolved relative to the skill directory (`config/skills/<skill-id>/`).
2. **FileGlob**: The pattern is matched against files within the skill directory tree.
3. **Shell**: **Not yet implemented** — logs a debug message and skips.

### Security Rules

Context injection enforces two security checks to prevent path traversal:

1. **Literal `..` check**: Any path containing `..` is immediately blocked.
2. **Canonical path validation**: The resolved path is canonicalized and verified to be within the skill directory. Paths that escape the skill directory (e.g., via symlinks) are blocked.

Both checks log a warning and skip the offending source (no error returned).

### Budget Enforcement

The total injected context is capped at `budget_tokens * 4` characters (since 1 token ≈ 4 characters).

- If `budget_tokens` is `0` (the default), the cap is **16,000 characters** (~4,000 tokens).
- Each source is read up to its `max_bytes` limit, but also capped by remaining budget.
- Sources are processed in order; once the budget is exhausted, remaining sources are skipped.

### Prompt Block Format

Injected context appears in the system prompt as:

```
### SKILL REFERENCE CONTEXT ###
--- context: <relative-path> ---
<file contents>
--- context: <relative-path> ---
<file contents>
```

The skill's own body (instructions) appears separately as:

```
### SKILL CONTEXT: <skill-name> ###
<body text, truncated to 4000 chars>
```

---

## 7. Permissions Model

### Three Permission Levels

| Level | Value | Description |
|-------|-------|-------------|
| **Readonly** | `"readonly"` | Default. No write operations. |
| **Read-Write** | `"readwrite"` | Can modify files, run commands. |
| **Admin** | `"admin"` | Full access including destructive operations. |

An unknown permission level (e.g., `"superuser"`, `"destructive"`) causes a preflight error that **blocks skill invocation**.

### Preflight Checks

Before a skill runs, `preflight_permissions()` validates:

1. The `permissions.level` is one of the three valid values.
2. If the skill lists `web_fetch` in `tools.allow` and the permission level is `readwrite` or `admin`, then `permissions.sandbox.net` must be `true`. Otherwise, the invocation is rejected.

### Tool Allow/Deny Enforcement

The handler builds the tool set as follows:

1. Start with `tools.allow` as the allowlist.
2. Remove any tools in `tools.deny`.
3. Remove any tools in the global deny list (`daemon.toml` → `execution.skill_defaults.global_tool_deny`).
4. Resolve remaining names against the tool registry. Unknown tool names trigger a warning but don't block invocation.

**Note**: The skill's `tools.allow` acts as a strict allowlist. Intent-suggested tools from the orchestrator are intentionally NOT merged, maintaining skill-level tool isolation.

### Confirmation Tools

Tools listed in `permissions.confirm.tools` are passed to the `SandboxPolicy.require_confirmation_for` field. The sandbox manager checks this list before executing those tools.

> **Warning: Fail-Closed Behavior.** The current implementation has no interactive confirmation mechanism. Tools in `require_confirmation_for` are **blocked** during autonomous agentic loop execution with:
>
> *"Tool 'X' requires human confirmation... but no interactive confirmation mechanism is available in this execution context. This is a fail-closed safety default."*
>
> This means `confirm.tools` currently acts as a **deny list**. Only use it to intentionally prevent a tool from running autonomously. A future interactive mode will provide a confirmation callback that enables actual prompting.

---

## 8. Output Validation

After the agentic loop completes, the skill's output is validated against `output` config.

### Format-Specific Validation

**`format: "markdown"`**
- Checks that every string in `required_sections` appears as an H2 heading (`## SectionName`) in the output.
- Missing sections produce an `OutputValidationError::MissingSections` error.

**`format: "json"`**
- Attempts to parse the output as JSON.
- If direct parsing fails, tries to extract JSON from markdown code blocks (`` ```json ... ``` ``) or from the first `{` to the last `}`.
- Invalid JSON produces an `OutputValidationError::InvalidJson` error.

**`format: "text"` or unset**
- No validation performed.

### Soft `max_tokens` Warning

If `output.max_tokens` is set, the handler estimates the output's token count as `output.len() / 4`. If this exceeds `max_tokens`, a warning is logged but the output is **not rejected**.

### Failure Behavior

On validation failure, the handler:
1. Logs a warning: `"Skill '<name>' output validation failed: <error>. Passing through as-is."`
2. Returns the original output unchanged (pass-through).

The `OutputValidationError` type provides a `repair_prompt()` method for potential future self-repair (re-prompting the LLM), but this is not currently wired up.

---

## 9. Complete Annotated Example

A realistic skill using all available fields:

```yaml
---
# ── Top-level identification ──────────────────────────────────
id: api-security-audit           # Optional; directory name is the canonical ID
name: API Security Audit         # REQUIRED — displayed in catalog
version: "1.0.0"                 # Informational semver
description: >                   # REQUIRED — used for catalog listings
  Analyze API endpoints for OWASP Top 10 vulnerabilities,
  authentication issues, and data exposure risks.

# ── Invocation ────────────────────────────────────────────────
invoke:
  mode: auto                     # auto | manual | scheduled | disabled
  slash: /security-audit         # Slash command (leading / stripped for matching)
  aliases:                       # Alternative commands (PARSED, not yet wired)
    - /sec-audit
    - /audit-api
  hotkey: ctrl+shift+s           # GUI hotkey (PARSED, reserved)
  cron: "0 8 * * 1"             # Weekly Monday 8am (PARSED, for scheduled mode)

# ── Routing / Auto-selection ──────────────────────────────────
routing:
  intent:                        # Substring-matched against user query (case-insensitive)
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
  negative_keywords:             # Any match subtracts 0.6 from score
    - "frontend"
    - "CSS"
    - "design"
  weights:                       # Per-skill weight overrides (alias: score)
    base: 0.25                   # Slightly higher than default 0.2
    intent_weight: 0.45
    keyword_weight: 0.35
    recency_weight: 0.15         # Lower recency boost for security audits
  examples:                      # For future LLM reranking (PARSED only)
    positive:
      - "Can you audit this REST API for security issues?"
      - "Check my Express.js routes for vulnerabilities"
    negative:
      - "Help me style the login page"
      - "Write a new API endpoint"

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
  summarize:                     # Context summarization (PARSED, not implemented)
    enabled: false
    max_tokens: 2000
  budget_tokens: 3000            # Total context budget: 3000 * 4 = 12,000 chars

# ── Permissions ───────────────────────────────────────────────
permissions:
  level: readwrite               # readonly | readwrite | admin
  confirm:
    tools:                       # These tools require user confirmation
      - shell_execute
      - file_write
    message: "This audit may modify configuration files. Continue?"  # PARSED
  sandbox:
    enabled: true                # PARSED — sandbox is policy-driven
    net: true                    # Required if tools.allow includes web_fetch
    fs_writable:                 # PARSED — reserved for future enforcement
      - "./reports"

# ── Tool Configuration ────────────────────────────────────────
tools:
  allow:                         # Strict allowlist — only these tools are available
    - file_read
    - file_write
    - shell_execute
    - web_fetch
    - memory_search
  deny:                          # Removed from allow (combined with global deny)
    - update_persona
  defaults:                      # Per-tool default params (PARSED, not injected)
    shell_execute:
      timeout: 30
  rate_limit:                    # Tool rate limiting (PARSED, not enforced)
    max_calls: 50
    window_secs: 300

# ── Output Validation ────────────────────────────────────────
output:
  format: markdown               # text | json | markdown
  max_length: 10000              # Max output chars (PARSED, not enforced)
  required_sections:             # Required H2 headings (ENFORCED for markdown format)
    - "Critical Vulnerabilities"
    - "Warnings"
    - "Recommendations"
    - "Summary"
  max_tokens: 3000               # Soft limit — logs warning if exceeded

# ── Testing ───────────────────────────────────────────────────
tests:
  inputs:
    - "Audit the /api/users endpoint for SQL injection"
    - "Check authentication on all POST routes"
  expect:
    contains:
      - "## Critical Vulnerabilities"
      - "## Summary"
    format: markdown
  smoke:
    - "examples/sample_api.json"

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
    interpreter: python3           # Omit to rely on shebang or direct execution
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
3. **Check** each endpoint against the OWASP Top 10:
   - A01: Broken Access Control
   - A02: Cryptographic Failures
   - A03: Injection (SQL, NoSQL, Command, LDAP)
   - A04: Insecure Design
   - A05: Security Misconfiguration
   - A06: Vulnerable Components
   - A07: Authentication Failures
   - A08: Data Integrity Failures
   - A09: Logging Failures
   - A10: SSRF
4. **Search** memory for past audit findings using `memory_search`
5. **Classify** each finding as Critical, High, Medium, or Low severity
6. **Produce** the structured report with all required sections

## Style

- Use clear, actionable language
- Include line numbers and code snippets for each finding
- Provide remediation code examples where possible
- Reference CWE numbers for each vulnerability type
- Prioritize findings by exploitability and impact

## Output Rules

- Every finding must include: severity, CWE, affected file:line, description, remediation
- The Summary section must include a risk score (1-10) and top 3 priorities
- If no vulnerabilities found, state this explicitly in each required section
```

---

## 10. Minimal Examples

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

A realistic minimal skill with invocation and permissions:

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
permissions:
  level: readonly
tools:
  allow:
    - file_read
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

## 11. Best Practices

1. **Start with `readonly` permissions.** Only escalate to `readwrite` or `admin` when the skill genuinely needs to modify files or run commands.

2. **Be specific with `tools.allow`.** List only the tools the skill actually needs. An empty `tools.allow` means no tools — the skill runs as a pure LLM prompt.

3. **Use `negative_keywords` to prevent false matches.** If your `code-review` skill shouldn't fire on "write code" queries, add `"write"` to `negative_keywords`.

4. **Set `required_sections` for structured output.** This gives the LLM a clear contract and enables automated validation.

5. **Keep intent phrases natural and varied.** They're substring-matched, so `"review code"` matches "please review code for me". Include several phrasings users might actually type.

6. **Use `routing.weights` when default scoring doesn't work.** Raise `intent_weight` for precision-critical skills or lower `recency_weight` for skills that shouldn't benefit from recent use.

7. **Set `budget_tokens` for context-heavy skills.** Without it, context defaults to ~4000 tokens, which may be too much or too little for your use case.

8. **Understand `permissions.confirm.tools` is fail-closed.** Currently, tools in `confirm.tools` are **blocked** in autonomous mode (no interactive confirmation exists). Only use this to intentionally prevent autonomous execution of a tool.

9. **Enable `sandbox.net: true` when using `web_fetch`.** The preflight check rejects `web_fetch` with `net: false` for non-readonly skills.

10. **Test with `routing.score` alias if migrating from older configs.** The `score` key is accepted as an alias for `weights` for backward compatibility.

11. **Use the correct tool names.** The implementation uses flat names (`file_read`, `shell_execute`) not dot-namespaced names (`repo.read_file`). See Section 12 for the complete list.

12. **Use bundled scripts for deterministic operations.** When a skill needs to perform filesystem scaffolding, run linters, or call external tools with specific flags, bundle shell/python scripts in `scripts/` rather than relying on `shell_execute` with LLM-generated commands. Scripts are more reliable, testable, and secure.

13. **Make scripts CWD-independent.** Scripts run with their working directory set to the skill directory. Resolve paths from `$0` (the script's own filesystem location), not the working directory:
    ```bash
    SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
    ```

14. **Return JSON from scripts.** Structured JSON output lets the LLM parse results and make decisions. Include status fields, paths, and error details:
    ```json
    {"status": "ok", "path": "/abs/path/to/output", "files_created": 3}
    ```

15. **Declare explicit `parameters` schemas for scripts.** A detailed JSON Schema with descriptions helps the LLM call tools correctly without guessing argument names or formats. Always include `required` fields.

---

## 12. Available Built-in Tools Reference

These are the tool names you can use in `tools.allow` and `tools.deny`. Names must match exactly.

| Tool Name | Description | Availability |
|-----------|-------------|--------------|
| `file_read` | Read file contents from the workspace | Always registered |
| `file_write` | Write/create files in the workspace | Always registered |
| `shell_execute` | Execute shell commands | Always registered |
| `web_search` | Search the web for information | Always registered; requires `providers.web_search.api_key` in `daemon.toml` |
| `web_fetch` | Fetch and read content from a URL | Always registered |
| `memory_search` | Search stored memories (hybrid FTS + vector) | Requires database |
| `workspace_read` | Read entries from the shared task workspace | Definition only (handled by ContextualToolExecutor) |
| `workspace_write` | Write entries to the shared task workspace | Definition only (handled by ContextualToolExecutor) |
| `update_persona` | Update persona documents (SOUL.md, USER.md, or IDENTITY.md via `target` param) | Requires persona context |

**Not registered** (stub implementations):
- `text_generate` — placeholder for future text generation tool
- `summarize` — placeholder for future summarization tool

### Script Tools (`skill_script:*`)

Script tools are **not** globally registered. They are dynamically created from a skill's `scripts:` frontmatter section and exist only for the duration of that skill's agentic loop invocation. They do not appear in the table above.

- **Namespace**: All script tools are prefixed with `skill_script:` (e.g., `skill_script:lint_code`).
- **Scope**: Only the skill that declares them can use them. Other skills, DAG sub-agents, pipeline steps, and lead agents cannot see or call them.
- **Do not list in `tools.allow`**: Script tools are automatically available to their declaring skill. Adding `skill_script:lint_code` to `tools.allow` would cause a "tool not found" warning since the registry lookup happens before script tools are attached.
- **Collision safety**: The `skill_script:` prefix ensures no collision with global registry tools. A script named `file_read` becomes `skill_script:file_read`, which is distinct from the global `file_read` tool.

---

## 13. Troubleshooting

### Common Errors

**"Missing field: name" / "Missing field: description"**
- Both `name` and `description` are required and must be non-empty strings.

**"Skill requires web_fetch tool but sandbox.net is false"**
- If `tools.allow` includes `web_fetch` and `permissions.level` is `readwrite` or `admin`, you must set `permissions.sandbox.net: true`.
- Alternatively, keep `permissions.level: "readonly"` (readonly skills skip the `net` check).

**"Unknown permission level 'X'"**
- Valid levels are exactly: `readonly`, `readwrite`, `admin`.
- The spec document mentions `write_repo`, `destructive`, and `network` — these are **not valid** in the implementation.

**Slash command not found**
- The leading `/` in `invoke.slash` is stripped for matching. Both `/review` and `review` will match.
- Check that `invoke.mode` is not `"disabled"`.

**Skill not auto-selected despite high keyword match**
- Auto-selection requires `invoke.mode: "auto"` AND score >= 0.65.
- Skills with `mode: "manual"` only appear as suggestions, never auto-select.
- Check for `negative_keywords` matches dragging the score below threshold.

**Tool name not recognized (warning in logs)**
- Use the exact names from Section 12 (e.g., `file_read`, not `repo.read_file` or `readFile`).
- `memory_search` requires a database connection; in CLI mode without a daemon, it may not be available.

**`routing.score` vs `routing.weights`**
- Both keys are accepted (they are serde aliases). `weights` is the canonical name; `score` is kept for backward compatibility.

**Context injection skips files silently**
- Check for `..` in paths (blocked for security).
- Ensure files exist within the skill directory (canonical path check).
- Check budget — once `budget_tokens * 4` characters are reached, remaining sources are skipped.

**Output validation warning but output returned normally**
- This is expected behavior. Validation failures log a warning and pass through the original output. The output is not rejected.

**Legacy fields ignored when new-spec fields are present**
- Legacy compat only applies when the corresponding new field is empty/None. If both `command` and `invoke.slash` are set, `command` is ignored.

### Script-Related Errors

**"Script 'X' not found: No such file or directory"**
- The `file` field in `scripts:` must match an actual file in the skill's `scripts/` directory.
- Check the filename for typos and ensure the file exists at `config/skills/<id>/scripts/<file>`.

**"Script 'X' resolves outside scripts/ directory (path traversal blocked)"**
- The script path contains `../` or a symlink that resolves outside the `scripts/` directory.
- All scripts must reside within the skill's `scripts/` directory after `canonicalize()`.

**"Script 'skill_script:X' timed out after Ys"**
- The script exceeded its `timeout_secs` (default: 30 seconds).
- Increase the timeout in the `scripts:` declaration, or optimize the script.

**"Script 'skill_script:X' failed (exit N): ..."**
- The script exited with a non-zero status. The first 500 characters of stderr are included in the error.
- Run the script manually to debug: `cd config/skills/<id> && ./scripts/<file> --key=value`.

**"Unknown script tool: skill_script:X"**
- The tool name was invoked but no matching script is declared in the skill's `scripts:` frontmatter.
- Check that the `name` field in `scripts:` matches (the LLM calls `skill_script:<name>`).

**Script runs but produces wrong paths / "file not found"**
- Scripts run with CWD set to the skill directory (`config/skills/<id>/`), not the project root.
- Resolve the project root from the script's own location using `SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"`, then navigate relative to that.
- Do not rely on the working directory for path resolution.

**Script tool not appearing in LLM tool list**
- Ensure the `scripts:` section is inside the YAML frontmatter (between `---` delimiters), not in the markdown body.
- Each entry requires `file`, `name`, and `description`. Missing required fields cause the entry to be silently skipped during deserialization.
