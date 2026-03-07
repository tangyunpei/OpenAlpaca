# Tool System — Design Document

> **Status:** Living document · **Last updated:** 2026-03-07
>
> Covers the architecture, design decisions, and conceptual model of the
> OpenAlpaca tool system.  For implementation details, API surfaces, and
> code-level reference see the companion [TECHNICAL.md](./TECHNICAL.md).

---

## 1. Purpose & Scope

The tool system gives OpenAlpaca agents the ability to *act* on the world
beyond pure language generation.  A **tool** is any callable unit of work
— reading a file, searching the web, running a shell command — that an
agent may invoke during an agentic loop iteration.

### Design Goals

| # | Goal | Rationale |
|---|------|-----------|
| 1 | **Least-privilege by default** | Agents receive *zero* tools unless explicitly assigned via skills. |
| 2 | **Defense-in-depth security** | Four independent security layers (capability check → input sanitization → confirmation gate → circuit breaker) protect every tool call. |
| 3 | **Extensibility without recompilation** | Users add custom tools via TOML configuration files; no Rust code required. |
| 4 | **Parallel execution** | All tool calls within a single LLM round execute concurrently via `join_all`. |
| 5 | **Hot-reloadable configuration** | Runtime config (web search keys, daemon limits) updates without restart via `ArcSwap`. |
| 6 | **Owner-scoped data isolation** | Tools accessing user-specific data (memory, persona) enforce authenticated ownership. |

### Non-Goals

- **Arbitrary plugin binaries.** Tools are either built-in Rust code, HTTP
  endpoints, or shell commands.  There is no dynamic `.so`/`.dylib` loading.
- **Cross-agent tool sharing at runtime.** Each agent's tool set is resolved
  at dispatch time and immutable for the lifetime of that agent run.
- **Fine-grained per-field authorization.** The security model operates at
  tool-name granularity, not per-parameter.

---

## 2. Conceptual Model

### 2.1 Tool Lifecycle

```
              ┌─────────────┐
              │  DEFINITION  │   TOML config or Rust code
              │  (startup)   │   defines name, schema, backend
              └──────┬───────┘
                     │
              ┌──────▼───────┐
              │ REGISTRATION │   ToolRegistry stores all tools
              │  (startup)   │   as HashMap<name, RegisteredTool>
              └──────┬───────┘
                     │
              ┌──────▼───────┐
              │  RESOLUTION  │   resolve_agent_tools() maps
              │  (dispatch)  │   agent skills → tool definitions
              └──────┬───────┘
                     │
              ┌──────▼───────┐
              │   INVOCATION │   LLM returns ToolCall in response;
              │   (runtime)  │   agentic loop dispatches execution
              └──────┬───────┘
                     │
              ┌──────▼───────┐
              │  EXECUTION   │   Security sandbox → context injection
              │  (runtime)   │   → backend dispatch → result formatting
              └──────┬───────┘
                     │
              ┌──────▼───────┐
              │   FEEDBACK   │   Tool result (≤32 KB) appended to
              │  (runtime)   │   conversation as tool_result message
              └──────────────┘
```

### 2.2 Tool Categories

| Category | Examples | Registered In | Handled By |
|----------|----------|---------------|------------|
| **Built-in** | `shell_execute`, `file_read`, `web_search` | `ToolRegistry` | `RegistryToolExecutor` |
| **Custom (HTTP)** | User-defined REST API wrappers | `ToolRegistry` (from TOML) | `ToolRegistry.execute_http()` |
| **Custom (Command)** | User-defined CLI wrappers | `ToolRegistry` (from TOML) | `ToolRegistry.execute_command()` |
| **Workspace** | `workspace_read`, `workspace_write` | Runtime (not in registry) | `ContextualToolExecutor` |
| **Owner-scoped** | `memory_search`, `update_persona` | `ToolRegistry` | `ContextualToolExecutor` (injects owner) → `RegistryToolExecutor` |
| **Lead Agent** | `spawn_subagent`, `check_subagent_status` | Runtime (not in registry) | `LeadAgentToolExecutor` |
| **Skill Scripts** | `skill_script:analyze` | Runtime (not in registry) | `ContextualToolExecutor` |
| **Stub/Future** | `text_generate`, `summarize` | `ToolRegistry` | Return "not implemented" error |

---

## 3. Architecture

### 3.1 Layer Stack

The tool system is organized as a stack of layers, each adding a concern:

```
┌──────────────────────────────────────────────────────────────────┐
│                        AGENTIC LOOP                              │
│  run_agentic_loop_routed()                                       │
│  • Receives LLM response with tool_calls                         │
│  • Enforces max_tools_per_round budget                           │
│  • Executes tools via SandboxManager                             │
│  • Formats results, feeds back to LLM                            │
├──────────────────────────────────────────────────────────────────┤
│                     SECURITY SANDBOX                             │
│  SandboxManager                                                  │
│  1. Capability check  (CapabilityManager)                        │
│  2. Input sanitization (InputSanitizer)                          │
│  3. Confirmation gate  (ConfirmationBroker)                      │
│  4. Circuit breaker    (ToolCircuitBreaker)                      │
│  5. Timeout enforcement                                          │
│  6. Event emission     (ToolExecuted / SecurityViolation)        │
├──────────────────────────────────────────────────────────────────┤
│                   CONTEXTUAL EXECUTOR                            │
│  ContextualToolExecutor                                          │
│  • Injects owner_id for owner-scoped tools                       │
│  • Handles workspace_read / workspace_write directly             │
│  • Dispatches skill_script:* tools                               │
│  • Forwards all others to RegistryToolExecutor                   │
├──────────────────────────────────────────────────────────────────┤
│                   REGISTRY EXECUTOR                              │
│  RegistryToolExecutor                                            │
│  • Strips LLM-supplied owner_id/workspace_id (anti-spoofing)     │
│  • Delegates to ToolRegistry.execute()                           │
├──────────────────────────────────────────────────────────────────┤
│                      TOOL REGISTRY                               │
│  ToolRegistry                                                    │
│  • JSON Schema argument validation                               │
│  • Dispatches to BuiltIn / HTTP / Command backends               │
│  • Shared reqwest::Client for connection pooling                  │
└──────────────────────────────────────────────────────────────────┘
```

### 3.2 Component Relationships

```
                        ┌─────────────────┐
                        │   DaemonConfig   │
                        │   (ArcSwap)      │
                        └────────┬────────┘
                                 │ execution limits, circuit breaker config
                                 ▼
┌───────────┐    ┌──────────────────────────────────┐
│  Agent    │    │         SandboxManager            │
│ Template  ├───►│  ┌─────────────┐  ┌───────────┐  │
│ (skills)  │    │  │ Capability  │  │  Input    │  │
└───────────┘    │  │  Manager    │  │ Sanitizer │  │
                 │  └─────────────┘  └───────────┘  │
  resolve_       │  ┌─────────────┐  ┌───────────┐  │
  agent_tools()  │  │ Confirm.    │  │  Circuit  │  │
       │         │  │  Broker     │  │  Breaker  │  │
       ▼         │  └─────────────┘  └───────────┘  │
┌───────────┐    │         │ wraps                   │
│  Tool     │    └─────────┼────────────────────────┘
│ Definitions│              ▼
│ (for LLM) │    ┌──────────────────────────────────┐
└───────────┘    │    ContextualToolExecutor          │
                 │    ┌─────────────┐                │
                 │    │ ToolExec.   │                │
                 │    │  Context    │                │
                 │    │ (owner_id,  │                │
                 │    │  task_id,   │                │
                 │    │  workspace) │                │
                 │    └─────────────┘                │
                 │           │ delegates              │
                 └───────────┼────────────────────────┘
                             ▼
                 ┌──────────────────────────────────┐
                 │       ToolRegistry               │
                 │  ┌────────┐ ┌─────┐ ┌─────────┐ │
                 │  │BuiltIn │ │HTTP │ │ Command │ │
                 │  │ Tools  │ │ API │ │ Scripts │ │
                 │  └────────┘ └─────┘ └─────────┘ │
                 └──────────────────────────────────┘
```

---

## 4. Design Decisions

### 4.1 Skills-to-Tools Mapping (not direct tool assignment)

**Decision:** Agents are assigned *skills* (e.g., `web_search`, `file_read`),
and the system resolves skills to tools at dispatch time via
`resolve_agent_tools()`.

**Rationale:**
- Decouples agent configuration from tool implementation details.
- Allows a single skill name to map to different tool backends depending
  on environment (e.g., a mock for testing).
- Future: a skill could map to *multiple* tools (e.g., `research` →
  `web_search` + `web_fetch` + `memory_search`).

**Current mapping:** 1:1 — skill name = tool name.  The indirection exists
for future extensibility.

### 4.2 Immutable Registry After Startup

**Decision:** `ToolRegistry` is built once at daemon startup, wrapped in
`Arc<ToolRegistry>`, and never mutated.

**Rationale:**
- Eliminates need for `RwLock` on the hot path.
- All tool queries (`get()`, `definitions_for_skills()`, `execute()`) are
  `&self` — zero synchronization cost.
- Custom tools from TOML are loaded at startup; changes require daemon restart.

### 4.3 Owner-ID Injection (Anti-Spoofing)

**Decision:** Owner-scoped tools (`memory_search`, `update_persona`) have
their `owner_id` and `workspace_id` fields stripped by `RegistryToolExecutor`
and re-injected from authenticated context by `ContextualToolExecutor`.

**Rationale:**
- The LLM controls tool arguments — a model could fabricate an `owner_id`
  to access another user's memories.
- Stripping at the executor level (not the LLM provider level) ensures no
  path bypasses the protection.
- Two-layer defense: strip first, then inject from trusted context.

### 4.4 Parallel Tool Execution

**Decision:** All tool calls in a single LLM response execute concurrently
via `futures::join_all()`.

**Rationale:**
- LLMs often emit multiple independent tool calls (e.g., `web_search` +
  `file_read`).  Sequential execution would be unnecessarily slow.
- The security sandbox is stateless per-call (circuit breaker state is
  shared but accessed atomically via `Mutex`), so parallel execution is safe.
- Cancellation token is raced against `join_all` for responsive task
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

### 4.6 Confirmation Gate

**Decision:** Security-critical tools can require interactive human
confirmation before execution, routed via `ConfirmationBroker`.

**Rationale:**
- Some tools (e.g., `file_write` to sensitive paths, `shell_execute` with
  destructive commands) should not execute without explicit approval.
- The broker uses `DashMap<request_id, oneshot::Sender>` for lock-free
  concurrent access.
- Any interface (CLI, GUI, Telegram) can deliver the user's decision via
  `broker.respond()`.
- Fail-closed: if no confirmation broker is available or timeout expires,
  the tool call is rejected.

### 4.7 Workspace Tools (Inter-Agent Collaboration)

**Decision:** `workspace_read` and `workspace_write` are runtime-injected
tools (not in the registry) that provide key-value storage within a task.

**Rationale:**
- Agents in a multi-agent task need a shared data plane.
- Optimistic locking (`state_version`) prevents lost updates when multiple
  agents write concurrently.
- Content size capped at 32 KB per entry to prevent context window overflow.

### 4.8 Protected Tool Names

**Decision:** 18 tool names are protected and cannot be overridden by
user-supplied TOML configurations.

**Rationale:**
- Prevents users from accidentally (or maliciously) replacing core
  security-critical tools with arbitrary HTTP/command backends.
- Protected names include: `shell_execute`, `file_read`, `file_write`,
  `memory_search`, `update_persona`, `send`, `workspace_read`,
  `workspace_write`, `spawn_subagent`, `spawn_subagents_batch`,
  `check_subagent_status`, `wait_for_subagents`.

---

## 5. Security Model

### 5.1 Threat Model

| Threat | Mitigation |
|--------|------------|
| LLM invokes unauthorized tool | CapabilityManager deny/allow lists per agent |
| LLM crafts malicious arguments | InputSanitizer: path traversal, command injection, null bytes |
| LLM spoofs owner_id | RegistryToolExecutor strips LLM-supplied owner fields |
| Tool endpoint returns hostile data | Result truncation (32 KB), no code execution on results |
| External tool endpoint is down | Circuit breaker prevents runaway retries/costs |
| Tool performs destructive action | ConfirmationBroker gates with interactive approval |
| Tool accesses internal network | SSRF validation blocks private IPs, cloud metadata, localhost |
| Tool runs indefinitely | Per-tool timeout enforcement via `tokio::time::timeout` |
| LLM calls excessive tools | `max_tools_per_round` and `max_tool_calls` budget enforcement |

### 5.2 SSRF Protection

All HTTP tool backends (both built-in `web_fetch`/`web_search` and custom
HTTP tools from TOML) pass through `url_validation::validate_url()`:

**Blocked targets:**
- Non-HTTP(S) schemes (`file://`, `ftp://`, etc.)
- Cloud metadata endpoints (`169.254.169.254`, `metadata.google.internal`)
- Localhost variants (`localhost`, `127.0.0.1`, `[::1]`, `0.0.0.0`)
- Private IP ranges: `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`
- Carrier-grade NAT: `100.64.0.0/10`
- IPv6 private: `fc00::/7` (ULA), `fe80::/10` (link-local)
- IPv4-mapped IPv6 with private embedded addresses (`::ffff:10.0.0.1`)
- HTTP redirect chains are also SSRF-checked (max 10 redirects)

### 5.3 Input Sanitization

The `InputSanitizer` provides two levels of validation:

**User input sanitization:**
- Maximum length: 32 KB (configurable)
- Null byte detection and rejection

**Tool argument sanitization:**
- Tool name allowlist validation (if configured)
- Path traversal detection in all string values (`..`, absolute paths)
- Command injection detection for shell-like tools:
  - Backtick execution (`` `cmd` ``)
  - Subshell execution (`$(cmd)`)
  - Newline injection
- Null byte detection in all string values

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
│    Gate          │    │ denies or      │
│ (if required)    │    │ timeout        │
└──────┬───────────┘    └───────────────┘
       │ PASS / SKIPPED
       ▼
┌──────────────────┐    ┌───────────────┐
│ 4. Circuit       │───►│ BLOCK if too   │
│    Breaker       │    │ many failures  │
└──────┬───────────┘    └───────────────┘
       │ PASS
       ▼
┌──────────────────┐    ┌───────────────┐
│ 5. Timeout       │───►│ ABORT if       │
│    Enforcement   │    │ exceeds limit  │
└──────┬───────────┘    └───────────────┘
       │ COMPLETE
       ▼
   Execute Tool
       │
       ▼
┌──────────────────┐
│ 6. Record        │
│    Outcome       │
│ (circuit breaker,│
│  event emission) │
└──────────────────┘
```

---

## 6. Extensibility

### 6.1 Adding a Custom Tool (TOML)

Create a file in `config/tools/` (e.g., `config/tools/my_tools.toml`):

```toml
[[tools]]
name = "weather_lookup"
description = "Get current weather for a city or coordinates"

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

Then assign the tool's name as a skill to an agent:

```yaml
# config/agents/researcher.md (frontmatter)
---
id: researcher
skills:
  - web_search
  - weather_lookup    # ← matches tool name
---
```

### 6.2 Adding a Built-in Tool (Rust)

1. Create a new file in `crates/openalpaca_core/src/tools/builtins/`.
2. Implement the `BuiltInTool` trait:
   ```rust
   #[async_trait]
   impl BuiltInTool for MyTool {
       async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
           // implementation
       }
   }
   ```
3. Create a factory function returning `RegisteredTool`.
4. Register in `builtin_tools()` or `builtin_tools_with_persona_context()`.
5. Optionally add to `PROTECTED_BUILTINS` if security-critical.

### 6.3 Skill-Bundled Scripts

Skills can bundle executable scripts in their `SKILL.md` frontmatter:

```yaml
scripts:
  - name: "analyze"
    file: "scripts/analyze.py"
    interpreter: "python3"
    timeout_secs: 30
```

These become invocable as `skill_script:analyze` during execution.
Path traversal protection ensures scripts stay within `skill_dir/scripts/`.

---

## 7. Execution Modes & Tool Availability

### 7.1 How Tools Reach Agents

| Dispatch Mode | Tool Resolution | Special Tools |
|---------------|----------------|---------------|
| **Sequential Pipeline** | `resolve_agent_tools()` per agent | Workspace tools |
| **DAG Executor** | `resolve_agent_tools()` per node | Workspace tools |
| **Lead Agent** | `resolve_agent_tools()` + lead tools | `spawn_subagent`, `spawn_subagents_batch`, `check_subagent_status`, `wait_for_subagents`, workspace tools |

### 7.2 Budget Enforcement

| Limit | Scope | Default | Configurable Via |
|-------|-------|---------|-----------------|
| `max_tools_per_round` | Per LLM round | 5 | `LoopConfig` / `daemon.toml` |
| `max_tool_calls` | Per agent lifetime | From `AgentConstraints` | Agent TOML config |
| `max_tool_runtime_secs` | Per individual tool | 60s | `SandboxPolicy` / `daemon.toml` |
| `max_rounds` | Per agentic loop | 15 | `LoopConfig` / `daemon.toml` |
| `max_cost` | Per agent run | $1.00 | `LoopConfig` / `daemon.toml` |

---

## 8. Observability

### 8.1 Events

Every tool execution emits a `SystemEvent::ToolExecuted` event via the
`EventBus`, containing `agent_id`, `tool_name`, `success`, `duration_ms`,
and `timestamp`.

Security violations emit `SystemEvent::SecurityViolation` with the reason.

Circuit breaker trips emit `SystemEvent::CircuitBreakerTripped` with
failure count and reset timeout.

### 8.2 Telemetry Storage

| Table | Retention | Content |
|-------|-----------|---------|
| `tool_execution_log` | 7 days | Per-call: agent, tool, success, duration, error |
| `skill_execution_log` | 90 days | Per-agent-run: tool_calls_made, rounds, tokens, cost |
| `event_log` | Configurable | All system events including tool events |

Automated cleanup runs daily via `spawn_telemetry_cleanup()`.

### 8.3 WebSocket Events

`ServerEvent::ToolExecuted` is broadcast to all connected WebSocket clients,
enabling real-time tool execution monitoring in the GUI.

---

## 9. Future Considerations

| Area | Current State | Potential Enhancement |
|------|--------------|----------------------|
| **Tool discovery** | Static TOML at startup | Dynamic registration via MCP or plugin protocol |
| **Composite skills** | 1:1 skill→tool mapping | 1:N mapping (`research` → multiple tools) |
| **Per-field authorization** | Tool-name granularity | JSON path-based argument restrictions |
| **Tool marketplace** | Local config only | Shared tool definitions across teams |
| **Result streaming** | Buffered (32 KB cap) | SSE streaming for long-running tools |
| **Tool versioning** | None | Semantic versioning for custom tool schemas |
| **Sandboxed execution** | Process-level timeout | Container/WASM isolation for command tools |
