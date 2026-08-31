# Tool System — Design Document

> **Status:** Living document · **Last updated:** 2026-07-19
>
> Covers the architecture, design decisions, and conceptual model of the
> OpenAlpaca tool system.  For implementation details, API surfaces, and
> code-level reference see the companion [TECHNICAL.md](./TECHNICAL.md).

---

## 1. Purpose & Scope

The tool system gives OpenAlpaca agents the ability to *act* on the world
beyond pure language generation.  A **tool** is any callable unit of work
— reading a file, searching the web, running a shell command, calling an
MCP server — that an agent may invoke during an agentic loop iteration.

### Design Goals

| # | Goal | Rationale |
|---|------|-----------|
| 1 | **Least-privilege by default** | Agents receive *zero* tools unless their declared capabilities intersect a tool's provided capabilities. |
| 2 | **Defense-in-depth security** | Independent security layers (capability check → input sanitization → confirmation gate → circuit breaker → timeout) protect every tool call. |
| 3 | **Extensibility without recompilation** | Users add tools via TOML config files, external MCP servers (`config/mcp.toml`), or out-of-process plugins; no Rust code required. |
| 4 | **Parallel execution** | All tool calls within a single LLM round execute concurrently via `join_all`. |
| 5 | **Runtime registration** | The registry is lock-free and mutable: MCP, plugin, and lead-agent coordination tools register (and unregister) while the daemon runs. |
| 6 | **Owner-scoped data isolation** | Tools accessing user-specific data (memory, persona) take identity only from a trusted per-invocation `ToolContext`, never from LLM-supplied arguments. |

### Non-Goals

- **In-process plugin binaries.** There is no dynamic `.so`/`.dylib`
  loading.  Extension code runs out of process: MCP servers are external
  programs, and plugins are child processes speaking JSON-RPC over stdio.
- **MCP server mode.** OpenAlpaca connects *out* to MCP servers and imports
  their tools; it does not expose its own tools over MCP.
- **Cross-agent tool sharing at runtime.** Each agent's tool set is resolved
  at dispatch time and immutable for the lifetime of that agent run.
- **Fine-grained per-field authorization.** The security model operates at
  tool-name granularity, not per-parameter.

---

## 2. Conceptual Model

### 2.1 Tool Lifecycle

```
              ┌──────────────┐
              │  DEFINITION  │   TOML config, Rust code, MCP server,
              │              │   or plugin manifest defines name,
              └──────┬───────┘   schema, backend
                     │
              ┌──────▼───────┐
              │ REGISTRATION │   ToolRegistry (DashMap) stores tools;
              │              │   built-ins + TOML at startup, MCP at
              └──────┬───────┘   boot, plugins & lead tools at runtime
                     │
              ┌──────▼───────┐
              │  RESOLUTION  │   resolve_agent_tools() intersects
              │  (dispatch)  │   agent capabilities with each tool's
              └──────┬───────┘   provides_capabilities (minus denials)
                     │
              ┌──────▼───────┐
              │  INVOCATION  │   LLM returns ToolCall in response;
              │  (runtime)   │   agentic loop dispatches execution
              └──────┬───────┘
                     │
              ┌──────▼───────┐
              │  EXECUTION   │   SandboxManager security checks
              │  (runtime)   │   → ToolRegistry backend dispatch
              └──────┬───────┘   (with ToolContext identity)
                     │
              ┌──────▼───────┐
              │   FEEDBACK   │   Tool result (≤32 KB) appended to
              │  (runtime)   │   conversation as tool_result message
              └──────────────┘
```

### 2.2 Tool Categories (by backend)

Every registered tool has a `ToolBackend` that executes its logic:

| Backend | Examples | Source | Notes |
|---------|----------|--------|-------|
| **BuiltIn** | `shell_execute`, `file_read`, `web_search`, `memory_search`, `workspace_read`/`workspace_write`, `update_persona`, `send` | Rust code, registered at startup | Tools needing identity override `execute_with_context()` |
| **BuiltIn (runtime)** | `spawn_subagent`, `spawn_subagents_batch`, `check_subagent_status`, `wait_for_subagents` | Registered when a lead-agent run starts | Provide the `orchestration` capability; status/wait tools are exempt from the sandbox timeout |
| **BuiltIn (skill scripts)** | `skill_script:analyze` | Skill `SKILL.md` frontmatter | Available only during that skill's invocation |
| **Http** | User-defined REST API wrappers | `config/tools/*.toml` | SSRF-validated, response capped |
| **Command** | User-defined CLI wrappers | `config/tools/*.toml` | Treated as shell-like for injection sanitization |
| **Mcp** | `<server>__<tool>` (e.g. `fs__read_file`) | `config/mcp.toml`, discovered at boot | Author recorded as `mcp:<server>` |
| **Plugin** | `<plugin>::<tool>` | Plugin directory, approval-gated | Executes via out-of-process JSON-RPC; author `plugin:<name>` |

Every `RegisteredTool` also carries provenance and policy metadata:
`provides_capabilities`, `exempt_from_timeout`, optional MCP-style
`annotations`, `version`, `author`, and `created_at`.

---

## 3. Architecture

### 3.1 Layer Stack

```
┌──────────────────────────────────────────────────────────────────┐
│                        AGENTIC LOOP                              │
│  run_agentic_loop_routed()                                       │
│  • Receives LLM response with tool_calls                         │
│  • Enforces max_tools_per_round budget (overflow calls errored)  │
│  • Executes tools in parallel via SandboxManager                 │
│  • Truncates results to 32 KB, feeds back to LLM                 │
├──────────────────────────────────────────────────────────────────┤
│                     SECURITY SANDBOX                             │
│  SandboxManager::execute_tool(tool_call, policy, ctx)            │
│  1. Capability check   (CapabilityManager)                       │
│  2. Input sanitization (InputSanitizer)                          │
│  3. Confirmation gate  (annotation-derived set, ApprovalCache,   │
│                         auto-approve bypass, ConfirmationBroker) │
│  4. Circuit breaker    (ToolCircuitBreaker)                      │
│  5. Timeout-wrapped execution (skipped for exempt tools)         │
│  6. Outcome recording + event emission                           │
├──────────────────────────────────────────────────────────────────┤
│                      TOOL REGISTRY                               │
│  ToolRegistry::execute_with_context(name, args, &ToolContext)    │
│  • JSON Schema argument pre-validation (required fields,         │
│    per-property types, enum values)                              │
│  • Dispatches to BuiltIn / Http / Command / Plugin / Mcp backend │
│  • BuiltIn backends receive the ToolContext; other backends      │
│    don't need identity and ignore it                             │
│  • Shared reqwest::Client with SSRF-checked redirect policy      │
└──────────────────────────────────────────────────────────────────┘
```

Identity flows through **`ToolContext`**, a lightweight per-invocation
struct built by the sandbox caller (never by the LLM):

```rust
pub struct ToolContext {
    pub agent_id: Option<String>,
    pub task_id: Option<String>,
    pub owner_id: Option<String>,
    pub workspace_id: Option<String>,
    pub skill_stack: Vec<String>,          // nested skill invocation chain
    pub effective_constraints: Option<EffectiveToolSet>, // inherited tool limits
}
```

### 3.2 Skill Invocation Executor

Synthetic `invoke_skill:*` tool calls are handled by a separate
`SkillInvocationToolExecutor` (`orchestrator/skill/invoke_executor.rs`).
It runs the invoked skill as a nested agentic loop, carrying a call stack
(bounded depth), sharing the parent's cost accumulator and cancellation
token, and intersecting the parent's tool constraints with the child
skill's (`compose_constraints` / `filter_tools_by_constraints`).

---

## 4. Design Decisions

### 4.1 Capabilities-to-Tools Mapping

**Decision:** Agents declare *capabilities* (and optionally
`denied_capabilities`) in their template frontmatter; tools declare
`provides_capabilities`.  `resolve_agent_tools()` includes a tool when any
of its capabilities matches any agent capability and none is denied.

**Rationale:**
- Decouples agent configuration from tool implementation details.
- One capability can map to multiple tools (e.g. `web_access` →
  `web_search` + `web_fetch`), and one tool can serve multiple capabilities.
- Deny lists let an agent exclude whole capability groups (e.g.
  `denied_capabilities: [web_access]` for an offline coding agent).

**Built-in capability names:** `file_read`, `file_write`, `shell_execute`,
`memory_read` (memory_search), `web_access` (web_search, web_fetch),
`workspace_read`, `workspace_write`, `persona_write` (update_persona),
`messaging` (send), `orchestration` (lead-agent coordination tools).

**Virtual capabilities:** a `CapabilityProvider` extension point derives
additional capabilities from tool metadata.  The built-in
`AnnotationCapabilityProvider` derives eight `annotation:*` capabilities
from MCP-style annotation hints (`annotation:readonly`,
`annotation:destructive`, `annotation:idempotent`, `annotation:open_world`,
plus their `annotation:non_*` inverses), letting an agent request e.g.
"all read-only tools" without naming them.

### 4.2 Lock-Free Mutable Registry

**Decision:** `ToolRegistry` is backed by `DashMap` and shared as
`Arc<ToolRegistry>`.  `register()` and `remove()` take `&self` and are safe
at any time, including after the Arc is shared.

**Rationale:**
- MCP servers register discovered tools at boot; plugins register/unregister
  tools as they load and unload; lead-agent coordination tools are
  registered when a lead run starts.  A build-once immutable map cannot
  support this.
- DashMap gives lock-free concurrent reads on the hot path without an
  `RwLock` around the whole registry.
- Registration validates tool names (non-empty, ≤ 256 chars, no null bytes)
  and updates an inverted capability index (string capabilities plus
  provider-derived virtual capabilities) used by
  `tools_for_capabilities_with_deny()`.
- TOML custom tools are still loaded only at daemon startup; changing
  `config/tools/*.toml` requires a restart.

### 4.3 Trusted-Context Identity Injection (Anti-Spoofing)

**Decision:** Owner-scoped tools take identity exclusively from the
`ToolContext` constructed by the sandbox caller.  `memory_search`
*overwrites* any LLM-supplied `owner_id`/`workspace_id` arguments with the
context values inside its `execute_with_context()` implementation, and
errors if the context carries no owner.  `update_persona` writes only to
the paths in its `PersonaToolContext`, configured at startup.

**Rationale:**
- The LLM controls tool arguments — a model could fabricate an `owner_id`
  to access another user's memories.
- Injection happens inside the tool itself, at the last possible moment, so
  no dispatch path can bypass it.
- Non-BuiltIn backends (HTTP, Command, Plugin) never receive identity;
  the registry discards the context for them.

### 4.4 Parallel Tool Execution

**Decision:** All tool calls in a single LLM response execute concurrently
via `futures::join_all()`, capped at `max_tools_per_round`; calls beyond
the cap receive error results instead of executing.

**Rationale:**
- LLMs often emit multiple independent tool calls (e.g., `web_search` +
  `file_read`).  Sequential execution would be unnecessarily slow.
- The security sandbox is stateless per-call (circuit breaker state is
  shared but accessed atomically via `Mutex`), so parallel execution is safe.
- The task's cancellation token is raced against `join_all` for responsive
  cancellation.

### 4.5 Circuit Breaker Pattern

**Decision:** A per-(agent, tool) circuit breaker tracks consecutive
transient failures and temporarily disables tools that repeatedly fail.

**Rationale:**
- Prevents runaway API costs when an external tool endpoint is down.
- Three-state model: Closed → Open (blocking) → HalfOpen (probe) → Closed.
- Only *transient* errors (timeouts, 5xx, connection refused) trip the
  breaker.  Permanent errors (bad arguments, 404) do not.
- Stale entry pruning prevents memory growth from dynamic agent IDs.
- Trips emit `SystemEvent::CircuitBreakerTripped`.

### 4.6 Confirmation Gate

**Decision:** Security-critical tools require interactive human
confirmation before execution, routed via `ConfirmationBroker`.

**How the confirmation set is chosen:** an explicit non-empty
`require_confirmation_for` list on the agent's sandbox policy wins;
otherwise the set is *derived from annotations* — every registered tool
with `destructive_hint = true` (built-ins: `file_write`,
`workspace_write`, `update_persona`, `shell_execute`, `send`) requires
confirmation.

**Behavior:**
- A session-scoped **`ApprovalCache`** remembers prior user approvals,
  scoped either to the exact argument hash (default) or to the whole tool;
  cached approvals skip the prompt.  The cache clears on daemon restart.
- An **auto-approve bypass** (`security.auto_approve_confirmations`
  globally, or per-agent `auto_approve`) skips prompts entirely; each
  bypass is audit-logged to the `event_log` table.
- Otherwise a `ToolConfirmationRequested` event is published (carrying
  `stream_id`/`lane_key` so SSE clients or connector lanes can surface the
  prompt), and execution blocks on a `oneshot` channel until the user
  responds or the timeout expires (default **300 s**, configurable via
  `confirmation_timeout_secs`).
- The broker uses `DashMap<request_id, oneshot::Sender>` for lock-free
  concurrent access; any interface (CLI, GUI, Telegram) can deliver the
  decision.
- **Fail-closed:** if no broker is available, or the user denies, or the
  timeout expires, the tool call is rejected.

### 4.7 Workspace Tools (Inter-Agent Collaboration)

**Decision:** `workspace_read` and `workspace_write` are registered
built-in tools providing key-value storage scoped to a task.

**Rationale:**
- Agents in a multi-agent task need a shared data plane.
- Optimistic locking prevents lost updates; on version conflict the write
  retries up to 5 times with jittered exponential backoff.
- Content is capped at 32 KB per entry to prevent context window overflow.
- Entries carry a type (`text`/`artifact`/`summary`/`context`) and an
  optional `file_asset_id` enabling file delivery to external channels
  (e.g. Telegram).

### 4.8 Protected Tool Names

**Decision:** Custom TOML tools cannot shadow built-in or runtime tool
names.  At startup the daemon collects the names of all registered
built-ins *dynamically*, adds the four runtime-registered coordination
tools (`spawn_subagent`, `spawn_subagents_batch`, `check_subagent_status`,
`wait_for_subagents`), and skips (with a warning) any TOML tool whose name
collides.

**Rationale:**
- Prevents users from accidentally (or maliciously) replacing
  security-critical tools with arbitrary HTTP/command backends.
- Collecting names dynamically means new built-ins are protected
  automatically — there is no hardcoded list to keep in sync.
- Note the protection lives in the daemon's TOML-load path only; the
  registry's `register()` API itself permits overwriting (needed for
  runtime re-registration).

---

## 5. Security Model

### 5.1 Threat Model

| Threat | Mitigation |
|--------|------------|
| LLM invokes unauthorized tool | CapabilityManager deny/allow lists per agent |
| LLM crafts malicious arguments | InputSanitizer: path traversal, command injection, null bytes |
| LLM spoofs owner_id | Identity comes only from trusted `ToolContext`; owner-scoped tools overwrite LLM-supplied identity args |
| Tool endpoint returns hostile data | Result truncation (32 KB to LLM; backend-level caps), no code execution on results |
| External tool endpoint is down | Circuit breaker prevents runaway retries/costs |
| Tool performs destructive action | Confirmation gate on `destructive_hint` tools (or explicit list), fail-closed |
| Tool accesses internal network | SSRF validation blocks private IPs, cloud metadata, localhost |
| Tool runs indefinitely | Per-tool timeout via `tokio::time::timeout` (unless explicitly exempt) |
| LLM calls excessive tools | `max_tools_per_round` and `max_tool_calls` budget enforcement |

### 5.2 SSRF Protection

All HTTP tool backends (built-in `web_fetch`/`web_search` and custom HTTP
tools from TOML) pass through `url_validation::validate_url()`:

**Blocked targets:**
- Non-HTTP(S) schemes (`file://`, `ftp://`, etc.)
- Cloud metadata endpoints (`169.254.169.254`, `metadata.google.internal`)
- Localhost variants (`localhost`, `127.0.0.1`, `[::1]`, `0.0.0.0`)
- Private IP ranges: `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`
- Carrier-grade NAT: `100.64.0.0/10`
- IPv6 private: `fc00::/7` (ULA), `fe80::/10` (link-local)
- IPv4-mapped IPv6 with private embedded addresses (`::ffff:10.0.0.1`)
- HTTP redirect chains are also SSRF-checked hop by hop (max 10 redirects)

### 5.3 Input Sanitization

The `InputSanitizer` provides three validation surfaces:

**User input sanitization:**
- Maximum length: 32 KB default (configurable via `security.max_input_length`)
- Null byte detection and rejection

**Tool argument sanitization** (recursive over all string values):
- Tool name must be in the registered-tool list
- Path traversal detection: `../` and `..\` are rejected in any string value
- Null byte detection in all string values
- Command injection detection — **only** for shell-like tools
  (`shell_execute` plus any command-backend tool from TOML):
  - Backtick execution (`` `cmd` ``)
  - Subshell execution (`$(cmd)`)
  - Newline and carriage-return injection (`\n`, `\r`)
  - Normal shell operators (pipes, redirections, `&&`) are intentionally
    allowed — the agent constructs full commands on purpose.  Non-shell
    tools (e.g. `file_write`) may legitimately contain multi-line content
    and are not injection-checked.

**File upload validation** (separate path, used by upload endpoints):
filename traversal and absolute-path rejection, size limit, polyglot
detection (declared MIME vs magic bytes), ZIP-bomb heuristic
(compression ratio > 100:1), and image dimension bounds.

### 5.4 Execution Flow (Security Path)

```
Tool Call arrives
       │
       ▼
┌──────────────────┐    ┌───────────────┐
│ 1. Capability    │───►│ BLOCK if tool  │
│    Check         │    │ denied or not  │
│                  │    │ in allow-list  │
└──────┬───────────┘    └───────────────┘
       │ PASS
       ▼
┌──────────────────┐    ┌───────────────┐
│ 2. Input         │───►│ BLOCK if path  │
│    Sanitization  │    │ traversal or   │
│                  │    │ injection      │
└──────┬───────────┘    └───────────────┘
       │ PASS
       ▼
┌──────────────────┐    ┌───────────────┐
│ 3. Confirmation  │───►│ BLOCK if user  │
│  (destructive or │    │ denies, times  │
│   listed tools;  │    │ out (300s), or │
│   approval cache │    │ no broker      │
│   / auto-approve │    │ (fail-closed)  │
│   may bypass)    │    └───────────────┘
└──────┬───────────┘
       │ PASS / SKIPPED
       ▼
┌──────────────────┐    ┌───────────────┐
│ 4. Circuit       │───►│ BLOCK if too   │
│    Breaker       │    │ many failures  │
└──────┬───────────┘    └───────────────┘
       │ PASS
       ▼
┌──────────────────┐    ┌───────────────┐
│ 5. Timeout-      │───►│ ABORT if       │
│    wrapped exec  │    │ exceeds limit  │
│ (skipped for     │    └───────────────┘
│  exempt tools)   │
└──────┬───────────┘
       │ COMPLETE
       ▼
┌──────────────────┐
│ 6. Record        │
│    Outcome       │
│ (circuit breaker,│
│  event emission) │
└──────────────────┘
```

---

## 6. Tool Annotations & Permission Tiers

Tools carry optional MCP-style annotations (`read_only_hint`,
`destructive_hint`, `idempotent_hint`, `open_world_hint`).  They come from
three places: `annotations_for_builtin()` for built-ins, the optional
`annotations` table in TOML tool configs, and the MCP server itself for
imported tools.

**Built-in annotation profiles:**

| Tools | Profile |
|-------|---------|
| `file_read`, `workspace_read`, `memory_search` | read-only, idempotent, closed-world |
| `web_fetch`, `web_search` | read-only, idempotent, open-world |
| `file_write`, `workspace_write`, `update_persona` | destructive, closed-world |
| `shell_execute`, `send` | destructive, open-world |

Annotations feed three mechanisms:

1. **Confirmation gating** — `destructive_hint = true` tools require user
   confirmation by default (§4.6).
2. **Virtual capabilities** — the eight `annotation:*` capabilities (§4.1).
3. **Permission tiers** — `permission_tier()` derives a coarse tier for
   introspection and policy: destructive → `Admin`, read-only →
   `ReadOnly`, otherwise `ReadWrite`.

---

## 7. Extensibility

### 7.1 Adding a Custom Tool (TOML)

Create a file in `config/tools/` (e.g., `config/tools/my_tools.toml`):

```toml
[[tools]]
name = "weather_lookup"
description = "Get current weather for a city or coordinates"
provides_capabilities = ["weather"]   # required for agents to resolve it

[tools.parameters]
type = "object"
required = ["location"]

[tools.parameters.properties.location]
type = "string"
description = "City name or lat,lon coordinates"

[tools.backend]
type = "http"
url = "https://api.weatherapi.com/v1/current.json?q={location}"
method = "GET"
timeout_secs = 10
```

Optional fields: `version` (default `"0.0.0"`), `author` (default
`"user"`), and an `annotations` table (feeds confirmation gating and
virtual capabilities).  Validation rules enforced at load time:

- HTTP URLs must start with `http://` or `https://`
- `timeout_secs` must be in `[1, 300]` (default 30)
- Any `annotation:*` entries in `provides_capabilities` must be one of the
  eight known annotation capability names
- Names colliding with protected tools are skipped (§4.8)

A custom tool with an empty `provides_capabilities` can never be resolved
to any agent — resolution matches only on capabilities.  Then declare the
capability on an agent template:

```yaml
# config/agents/researcher.md (frontmatter)
---
id: researcher
capabilities:
  - web_access
  - weather        # ← matches the tool's provides_capabilities
---
```

### 7.2 Adding a Built-in Tool (Rust)

1. Create a new file in `crates/openalpaca_core/src/tools/builtins/`.
2. Implement the `BuiltInTool` trait:
   ```rust
   #[async_trait]
   impl BuiltInTool for MyTool {
       async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
           // implementation
       }
       // Override execute_with_context() instead if the tool needs
       // identity (owner_id, task_id, workspace_id) from ToolContext.
   }
   ```
3. Create a factory function returning `RegisteredTool` — populate
   `definition`, `backend`, `provides_capabilities`, `exempt_from_timeout`
   (normally `false`), `annotations`, `version`, `author`, `created_at`.
4. Add an annotation profile for the new name in `annotations_for_builtin()`
   if the tool should be classified (read-only vs destructive drives
   confirmation gating and virtual capabilities).
5. Register it in `builtin_tools()` (or
   `builtin_tools_with_persona_context()` if it needs persona/connector
   wiring).  Name-collision protection for TOML tools picks it up
   automatically.

### 7.3 MCP Servers

External MCP servers are configured in `config/mcp.toml` and connected at
daemon boot; a missing file simply means no MCP servers.

```toml
[defaults]
connect_timeout_secs = 30      # per-server overridable
request_timeout_secs = 30
max_reconnect_attempts = 3
reconnect_backoff_ms = 100

[servers.fs]
transport = "stdio"            # or "http"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
# enabled = true               # per-server kill switch
```

HTTP transport supports `url`, `extra_headers`, and `auth` as a bearer
token or API-key header (with `*_env` variants resolved from environment
variables at boot).

- Discovered tools register as **`<server>__<tool>`** (e.g.
  `fs__read_file`) with author `mcp:<server>`, so they cannot collide with
  built-ins or other servers.  Server-provided annotations are preserved
  and participate in confirmation gating and virtual capabilities.
- Per-server failures (bad config, connect timeout, list failure) are
  logged and skipped — never fatal to daemon startup.
- Tool-call errors (`is_error` results) surface as tool errors to the LLM.
- **Limitations:** non-text MCP content (images, resources, audio) is
  replaced with a bracketed placeholder; MCP *resources* and *prompts* are
  not implemented — only tools are imported.  Server mode (exposing
  OpenAlpaca's tools over MCP) is a non-goal.

### 7.4 Plugins

Out-of-process plugins (child processes speaking JSON-RPC 2.0 over stdio)
can also contribute tools, registered as **`<plugin>::<tool>`** with a
`ToolBackend::Plugin` that proxies calls to the plugin process.  Plugins
are approval-gated on first load and managed via `openalpaca plugin …`
CLI commands and `/v1/plugins` routes.  See the plugin documentation for
the manifest schema and lifecycle; from the tool system's perspective a
plugin tool is just another registry entry with author `plugin:<name>`.

### 7.5 Skill-Bundled Scripts

Skills can bundle executable scripts in their `SKILL.md` frontmatter:

```yaml
scripts:
  - name: "analyze"
    file: "scripts/analyze.py"
    description: "Analyze the input data and report findings"   # required
    interpreter: "python3"        # optional — auto-detected if omitted
    timeout_secs: 30              # optional — default 30
    parameters:                   # optional JSON Schema; default {}
      type: object
      properties:
        input:
          type: string
```

These become invocable as `skill_script:<name>` during that skill's
execution.  Arguments are passed as `--key=value` CLI flags; the script
runs with the skill directory as its working directory.  Path traversal
protection (canonicalize + prefix check) ensures scripts stay within
`<skill>/scripts/`, and stdout/stderr are capped at 512 KB.

---

## 8. Execution Modes & Tool Availability

### 8.1 How Tools Reach Agents

| Role | Tool Resolution | Special Tools |
|---------------|----------------|---------------|
| **Lead Agent** | Coordination tools + workspace tools + `memory_search` | `spawn_subagent`, `spawn_subagents_batch`, `check_subagent_status`, `wait_for_subagents`, plus `post_update` / `queue_followup` when steering is enabled |
| **Subagents** | `resolve_agent_tools()` per spawned agent | Workspace tools (via capabilities) |

(The legacy sequential-pipeline and DAG-executor dispatch modes were deleted in Routing V2 Phase 5; the lead agent is the only multi-agent dispatch mode.)

Coordination tools are registered into the shared registry when the lead
run starts (`register_coordination_tools`), so they pass through the same
sandbox/registry path as every other tool.  `check_subagent_status` and
`wait_for_subagents` are flagged `exempt_from_timeout` — they manage their
own deadlines and must not be killed by the per-tool sandbox timeout.

### 8.2 Budget Enforcement

| Limit | Scope | Default | Configurable Via |
|-------|-------|---------|-----------------|
| `max_tools_per_round` | Per LLM round | 5 | `LoopConfig` / `daemon.toml` |
| `max_tool_calls` | Per agent lifetime | From `AgentConstraints` | Agent template frontmatter (`config/agents/*.md`) |
| `max_tool_runtime_secs` | Per individual tool | 60 s | `SandboxPolicy` / `daemon.toml` |
| `confirmation_timeout_secs` | Per confirmation prompt | 300 s | `SandboxPolicy` / `daemon.toml` |
| `max_rounds` | Per agentic loop | 15 | `LoopConfig` / `daemon.toml` |
| `max_cost` | Per agent run | $1.00 | `LoopConfig` / `daemon.toml` |

### 8.3 Output Limits

| Stage | Cap |
|-------|-----|
| Tool result fed back to the LLM | 32 KB (truncated with notice) |
| HTTP backend response body | 1 MB streamed read, then first 8192 chars returned |
| Command backend stdout/stderr | 512 KB each |
| Skill script stdout/stderr | 512 KB each |
| Workspace entry content | 32 KB |

---

## 9. Observability

### 9.1 Events

Every tool execution emits a `SystemEvent::ToolExecuted` event via the
`EventBus`, containing `agent_id`, `tool_name`, `success`, `duration_ms`,
and `timestamp`.

Security violations emit `SystemEvent::SecurityViolation` with the reason
(also persisted to the `event_log` table when a database is attached).

Circuit breaker trips emit `SystemEvent::CircuitBreakerTripped`.

Pending confirmations emit `SystemEvent::ToolConfirmationRequested` with
the request ID, tool name, arguments, and `stream_id`/`lane_key` routing
hints so the prompt reaches the right SSE stream or connector lane.
Auto-approve bypasses are audit-logged to `event_log`.

### 9.2 Telemetry Storage

| Table | Retention | Content |
|-------|-----------|---------|
| `tool_execution_log` | 7 days | Per-call: agent, tool, success, duration, error |
| `skill_execution_log` | 90 days | Per-agent-run: tool_calls_made, rounds, tokens, cost |
| `event_log` | Configurable | Security violations, auto-approve audits, other system events |

Automated cleanup runs daily via `spawn_telemetry_cleanup()`.

### 9.3 WebSocket Events

`ToolExecuted` events are bridged to all connected WebSocket clients,
enabling real-time tool execution monitoring in the GUI.

---

## 10. Future Considerations

| Area | Current State | Potential Enhancement |
|------|--------------|----------------------|
| **MCP resources & prompts** | Stubbed (tools only) | Import server resources/prompts into agent context |
| **Non-text MCP content** | Replaced with placeholder text | Surface images/resources to multimodal models |
| **Per-field authorization** | Tool-name granularity | JSON path-based argument restrictions |
| **Hot-reload of TOML tools** | Startup-only load | Watch `config/tools/` and re-register at runtime |
| **Result streaming** | Buffered (32 KB cap) | SSE streaming for long-running tools |
| **Sandboxed execution** | Process-level timeout | Container/WASM isolation for command tools |
