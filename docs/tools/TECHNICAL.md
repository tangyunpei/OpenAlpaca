# Tool System — Technical Reference

> **Last verified against the code on 2026-09-20** — `crates/openalpaca_core/src/tools`,
> `crates/openalpaca_core/src/security`, `crates/openalpaca_core/src/runner`,
> `crates/openalpaca_core/src/orchestrator/skill`, `apps/openalpacad/src/services`,
> `apps/openalpacad/src/managers` and the shipped `config/`. Workspace version 0.1.0,
> schema version 42 (the `001_baseline.sql` baseline).
>
> Code-level reference for the OpenAlpaca tool system.  For architecture
> and design rationale see the companion [DESIGN.md](./DESIGN.md).  Struct
> listings here are trimmed to what a reader needs; the source files named
> beside them are authoritative.

---

## Table of Contents

1. [File Map](#1-file-map)
2. [Core Types](#2-core-types)
3. [ToolRegistry](#3-toolregistry)
4. [Capability-Based Tool Resolution](#4-capability-based-tool-resolution)
5. [Extension Ledger and Gate](#5-extension-ledger-and-gate)
6. [Built-in Tools](#6-built-in-tools)
7. [Per-Request Tools](#7-per-request-tools)
8. [Custom Tools (TOML Configuration)](#8-custom-tools-toml-configuration)
9. [MCP Tools](#9-mcp-tools)
10. [Plugin Tools](#10-plugin-tools)
11. [Security Layers](#11-security-layers)
12. [Agentic Loop Integration](#12-agentic-loop-integration)
13. [Tool Result Handling](#13-tool-result-handling)
14. [Telemetry & Storage](#14-telemetry--storage)
15. [URL Validation](#15-url-validation)
16. [Platform Helpers](#16-platform-helpers)
17. [Configuration Reference](#17-configuration-reference)
18. [Testing](#18-testing)

---

## 1. File Map

All `tools/…`, `security/…`, `runner/…` and `orchestrator/…` paths below are
under `crates/openalpaca_core/src/`.

### Core Tool System

| File | Purpose |
|------|---------|
| `tools/mod.rs` | Module root, `resolve_agent_tools()` |
| `tools/registry/mod.rs` | `ToolRegistry` (incl. the extension gate in `dispatch`), `RegisteredTool`, `ToolBackend`, `BuiltInTool` trait, `ToolContext`, `PermissionTier`, `CapabilityResolution` |
| `tools/registry/capabilities.rs` | `CapabilityProvider`, `AnnotationCapabilityProvider`, `ProviderHandle`, annotation capability names |
| `tools/registry/availability.rs` | `SkillRequirements`, `CapabilityOracle` — "can this skill still reach what it declares?" |
| `tools/registry/tests.rs`, `availability_tests.rs` | Registry unit tests |
| `tools/config/mod.rs` | TOML config parsing (`ToolConfigFile`, `ToolConfig`, `ToolBackendConfig`) |
| `tools/config/annotations.rs` | `ToolAnnotationsConfig` (annotations block in user TOML) |
| `tools/url_validation.rs` | `validate_url()` SSRF protection |
| `tools/platform.rs` | `shell_command()` platform abstraction |

### Extensions (the ENABLE axis)

| File | Purpose |
|------|---------|
| `tools/extensions/mod.rs` | `ExtensionId`, `ExtensionKind`, `ExtensionState`, `FailureReason`, `UnapprovedReason`, `Consent`, the `ExtensionSupervisor` trait |
| `tools/extensions/ledger.rs` | `ExtensionLedger` — state, generations, in-flight counters (`CallGuard`), retained tool names, `check()` |
| `tools/extensions/describe.rs` | The refusal and status wording, per audience (model / human) |
| `tools/extensions/scan.rs` | The dependent scan: which skills and agent templates lose something when an extension goes |
| `apps/openalpacad/src/managers/mcp.rs` | `McpSupervisor` — MCP lifecycle (enable, disable, reload, crash reaper, tool-list refresh, add/remove server) |
| `apps/openalpacad/src/managers/extensions.rs` | `Extensions` — the one handle the routes use; dispatches each verb to the right supervisor by `kind` |
| `crates/openalpaca_plugins/src/manager.rs` | `PluginManager` — plugin lifecycle and consent |
| `apps/openalpacad/src/routes/extensions.rs` | `/v1/extensions*` routes |
| `apps/openalpacad/src/routes/tools.rs`, `routes/skills.rs` | `GET /v1/tools`, `GET /v1/skills`, `GET /v1/skills/health` |
| `apps/openalpaca/src/commands/ext.rs` | `openalpaca ext …` |

### MCP Bridge

| File | Purpose |
|------|---------|
| `tools/mcp/config.rs` | `McpConfig` — `config/mcp.toml` parsing, `is_valid_server_name()` |
| `tools/mcp/bridge.rs` | `rmcp_tool_to_registered()`, `serialize_call_result()` |
| `tools/mcp/classify.rs` | `classify_bringup_failure()`, `classify_call_failure()` — which errors mark a server `failed` |
| `tools/mcp/fingerprint.rs` | `config_fingerprint()` — detects a real edit to a server block |
| `crates/openalpaca_mcp/` | `McpClient` wrapper around the `rmcp` SDK (stdio + streamable-HTTP transports, reconnect/retry, change notifications). Re-exports `Tool`, `ToolAnnotations`, `CallToolResult`, etc. |

### Built-in Tools

| File | Tool(s) |
|------|---------|
| `tools/builtins/mod.rs` | Registration functions, `WorkspaceReadTool`/`WorkspaceWriteTool`, `ScriptToolBuiltIn`, built-in annotations |
| `tools/builtins/web_search.rs` | `web_search` |
| `tools/builtins/web_fetch.rs` | `web_fetch` |
| `tools/builtins/file_ops.rs` | `file_read`, `file_write` (incl. the pre-edit snapshot) |
| `tools/builtins/artifact_write.rs` | `artifact_write`, the `workspace_write` artifact spill |
| `tools/builtins/read_result.rs` | `read_result` |
| `tools/builtins/shell_execute.rs` | `shell_execute` |
| `tools/builtins/memory_search.rs` | `memory_search` |
| `tools/builtins/update_persona/` | `update_persona` (`mod.rs` dispatcher; `soul.rs`, `user.rs`, `identity.rs`, `common.rs`) |
| `tools/builtins/send.rs` | `send` (connector message/file delivery) |
| `tools/builtins/helpers/mod.rs` | Workspace path validation, backup management |
| `tools/builtins/main_loop.rs` | `main_loop_tool_set()` — the chat loop's per-request tools |
| `tools/builtins/start_workflow.rs`, `task_status.rs`, `steer_workflow.rs`, `memory_ops.rs`, `invoke_skill.rs` | The per-request tools themselves |
| `tools/builtins/tests.rs`, `helpers/tests.rs`, `update_persona/tests.rs` | Tests |

### Security

| File | Purpose |
|------|---------|
| `security/sandbox/mod.rs` | `SandboxManager`, `SandboxPolicy`, `effective_confirmation_set()`, `UNAPPROVABLE_EVENT_TYPE` |
| `security/capabilities/mod.rs` | `Allowlist`, `CapabilityManager`, `SecurityViolation` |
| `security/sanitizer/mod.rs` | `InputSanitizer` |
| `security/circuit_breaker/mod.rs` | `ToolCircuitBreaker`, `is_transient_tool_error()` |
| `security/confirmation.rs` | `ConfirmationBroker`, `ConfirmationResolution`, `ApprovalCache`, `ApprovalScope`, `hash_canonical_args()` |
| `security/gate.rs` | `SecurityGate` facade |
| `security/policy.rs` | `Principal`, `Scope`, `TrustGate` |
| `security/*/tests.rs` | Tests |

### Agentic Loop and Runners

| File | Purpose |
|------|---------|
| `runner/agentic_loop/mod.rs` | `run_agentic_loop()`, `run_agentic_loop_routed()`, the tool phase, spill planning |
| `runner/agentic_loop/tool_helpers.rs` | `truncate_tool_result_to()`, `head_tail_tool_result()`, `format_tool_error()`, `format_tool_error_with_hint()` |
| `runner/agentic_loop/config.rs` | `LoopConfig`, `LoopResult` |
| `runner/agentic_loop/answer_guard.rs` | `AnswerGuard` — a last look at the finished answer (used by the chat loop) |
| `runner/agentic_loop/backend.rs`, `context.rs`, `cost.rs` | LLM backend abstraction, loop state, `LoopCostAccumulator` |
| `runner/lead_agent/mod.rs` | Lead agent run: surface assembly, per-run registry |
| `runner/lead_agent/tools.rs` | Coordination tools, `post_update`, `queue_followup`, `register_coordination_tools()`, the subagent spawn path |
| `runner/lead_agent/tracker.rs`, `guard.rs`, `prompt.rs` | `SubagentTracker`, `AgentBusyGuard`, prompt assembly |
| `runner/plugin_agent.rs` | Drives a plugin-contributed agent template |
| `session_log/` | The per-session event log; `writer.rs` writes the `results/` spill, `record.rs` holds `spill_stub()` |

### Skills

| File | Purpose |
|------|---------|
| `orchestrator/skill/invocation.rs` | Skill tool resolution, sandbox policy, loop, output validation |
| `orchestrator/skill/invoke_executor.rs` | `SkillInvocationToolExecutor` — nested skill invocations |
| `orchestrator/skill/constraints.rs` | `EffectiveToolSet`, `compose_constraints()` |
| `middleware/skill/types.rs` | `SkillFrontmatter`, `ScriptConfig` |

### Daemon Wiring

| File | Purpose |
|------|---------|
| `apps/openalpacad/src/services/tools.rs` | `build_tool_registry()` — built-ins + custom TOML, then the MCP supervisor |
| `apps/openalpacad/src/services/mcp.rs` | `build_mcp_supervisor()` — MCP bootstrap from `config/mcp.toml` |
| `apps/openalpacad/src/routes/chat.rs` | `GET /v1/chat/confirmations`, `POST /v1/chat/confirmations/{request_id}` |
| `apps/openalpacad/src/background.rs` | `spawn_telemetry_cleanup()` |
| `apps/openalpacad/src/event_bridge.rs` + `events/` | `SystemEvent` → `ServerEvent` (WebSocket broadcast + persistence) |

### Configuration

| File | Purpose |
|------|---------|
| `config/tools/example.toml` | Example custom tool configuration (commented out) |
| `config/mcp.toml` | MCP server declarations (commented examples by default) |
| `config/agents/*.md` | Agent templates — `capabilities` are the ALLOW axis |
| `config/daemon.toml` | Limits and timeouts (see [Section 17](#17-configuration-reference)) |

---

## 2. Core Types

### ToolDefinition (LLM-facing)

**Location:** `crates/openalpaca_llm/src/types.rs`

```rust
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolDefinition {
    pub name: String,                                    // Unique tool identifier
    pub description: String,                             // LLM-facing documentation
    pub parameters: serde_json::Value,                   // JSON Schema for arguments
    pub strict: Option<bool>,                            // Anthropic strict tool mode
    pub input_examples: Option<Vec<serde_json::Value>>,  // Example inputs for LLM
}
```

### ToolCall (LLM output)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,                    // Unique ID for correlation
    pub name: String,                  // Tool to invoke
    pub arguments: serde_json::Value,  // JSON args matching parameters schema
}
```

### ToolContext (per-invocation identity)

**Location:** `tools/registry/mod.rs`

The identity-carrying spine of tool execution.  Built by the caller (chat
query handler, lead agent, subagent spawn, skill handler) and threaded
through `SandboxManager` into `BuiltInTool::execute_with_context()`.  It
holds no database handle.

| Field | Type | Meaning |
|-------|------|---------|
| `agent_id` | `Option<String>` | Who violations and events are reported against: a template id for subagents, the lead's instance id for a lead agent, `orchestrator` for the chat loop, `skill:<name>` for a skill |
| `agent_instance_id` | `Option<String>` | The runtime instance (`research_agent::a1b2c3d4`); attributes a pending confirmation to the lane waiting on it |
| `task_id` | `Option<String>` | The workflow run; `None` outside one |
| `owner_id` | `Option<String>` | The owning user |
| `principal`, `scope` | `Option<…>` | The requesting principal and resource scope, where threaded |
| `workspace_id` | `Option<String>` | Workspace root for **memory scoping**; derived from the daemon's current directory when the request carried none |
| `request_workspace_root` | `Option<String>` | The project root the *request* supplied, resolved.  Never derived from the current directory.  The only field that may place content on disk (`artifact_write`, the workspace spill) |
| `workspace_path` | `Option<String>` | The path as sent; persisted with steering and follow-up items |
| `skill_stack` | `Vec<String>` | Skill invocation chain, oldest first |
| `effective_constraints` | `Option<EffectiveToolSet>` | Tool limits inherited from a parent skill chain |
| `lane_key`, `source`, `request_id` | `Option<…>` | The conversation lane, the channel (`cli`, `telegram`, …) and the originating request |
| `session_id` | `Option<String>` | The session whose log carries this call; scopes `read_result` |
| `session_log` | `Option<SessionLogHandle>` | Emit side of that log; `file_write` uses it for the pre-edit snapshot |
| `event_bus` | `Option<EventBus>` | Filled in by the sandbox when the caller left it empty, so a tool can announce what it produced |

Helpers: `with_skill_pushed(skill_id)` clones the context and appends to
`skill_stack`.  `created_by()` resolves the `task.created_by` value from the
principal, then the owner id — `start_workflow` writes it and `task_status`
reads by it.

Because identity comes from a server-side context object rather than from
LLM-provided arguments, the model cannot spoof `owner_id`, `task_id`, or
`workspace_id`.

### RegisteredTool

```rust
#[derive(Clone)]
pub struct RegisteredTool {
    pub definition: ToolDefinition,          // Schema for LLM
    pub backend: ToolBackend,                // Execution backend
    pub provides_capabilities: Vec<String>,  // Capability strings for resolution
    /// When true, SandboxManager skips the per-tool timeout
    /// (used by coordination tools that manage their own waits).
    pub exempt_from_timeout: bool,
    /// MCP tool annotations (destructive_hint, read_only_hint, ...).
    pub annotations: Option<openalpaca_mcp::ToolAnnotations>,
    pub version: String,
    /// Provenance for display: "builtin", "user", "mcp:<server>",
    /// "plugin:<id>", "skill:<name>".
    pub author: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}
```

Two derived accessors identify an extension tool:

- `extension_id() -> Option<ExtensionId>` — from the backend (`Mcp` →
  server name, `Plugin` → the executor's `plugin_id()`, the plugin
  directory name).  `None` for `BuiltIn`/`Http`/`Command`, which are never
  gated.  It is never read from `author`.
- `incarnation() -> Option<u64>` — which load of the extension the handle
  belongs to.

Reading `backend` outside `ToolRegistry` bypasses the extension gate; every
path to an MCP client or plugin executor must go through
`execute`/`execute_with_context`.

### ToolBackend

```rust
#[derive(Clone)]
pub enum ToolBackend {
    BuiltIn(Arc<dyn BuiltInTool>),
    Http {
        method: String,                       // GET, POST, PUT, DELETE
        url: String,                          // URL template with {param} placeholders
        headers: HashMap<String, String>,     // Static headers
        timeout_secs: u64,                    // 1-300
    },
    Command {
        command: String,                      // Binary name
        args_template: Option<String>,        // Args with {param} placeholders
        timeout_secs: u64,                    // 1-300
    },
    Plugin(Arc<dyn openalpaca_api::plugin_traits::PluginToolExecutor>),
    Mcp {
        client: Arc<openalpaca_mcp::McpClient>,
        remote_name: String,                  // Tool name on the server
        server_name: String,
        generation: u64,                      // Which load of the server
    },
}
```

### BuiltInTool Trait

```rust
#[async_trait]
pub trait BuiltInTool: Send + Sync {
    async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String>;

    /// Execute with per-invocation context. Default delegates to execute().
    /// Override for tools that need identity (owner_id, task_id, etc.).
    async fn execute_with_context(
        &self,
        arguments: &serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<String, String> {
        self.execute(arguments).await
    }
}
```

Context-dependent tools (`memory_search`, `update_persona`,
`workspace_read`, `workspace_write`, `artifact_write`, `read_result`,
`file_write`, `send`, and every per-request tool) override
`execute_with_context()`.  Several of them return an error from plain
`execute()` saying they need a context.

### PermissionTier

Derived from MCP annotations for introspection/policy — not stored on the
tool:

```rust
pub enum PermissionTier { ReadOnly, ReadWrite, Admin }

// destructive_hint = Some(true) → Admin
// read_only_hint  = Some(true) → ReadOnly
// otherwise (including None)   → ReadWrite
pub fn permission_tier(annotations: Option<&ToolAnnotations>) -> PermissionTier
```

---

## 3. ToolRegistry

**Location:** `tools/registry/mod.rs`

### Structure

```rust
pub struct ToolRegistry {
    tools: DashMap<String, RegisteredTool>,
    capability_index: DashMap<String, Vec<String>>, // capability → tool names
    extensions: Arc<ExtensionLedger>,                // the ENABLE axis
    http_client: reqwest::Client,                    // shared, SSRF-checked redirects
    capability_providers: DashMap<ProviderHandle, Arc<dyn CapabilityProvider>>,
    next_provider_handle: AtomicU64,
    provider_mutex: std::sync::Mutex<()>,            // serializes index rebuilds
}
```

Backed by `DashMap` for lock-free concurrent reads and writes.  Shared as
`Arc<ToolRegistry>` — tools can be **registered and removed at runtime**
(when plugins load/unload or MCP servers connect) without `&mut self`.

`ToolRegistry` implements `Clone`.  The clone copies the tool map, the
capability index and the providers, but **shares the ledger**
(`Arc::clone`).  That is what makes a per-request copy read *live* extension
state at the gate.  The copy is not atomic; concurrent register/remove
during a clone can produce an incomplete snapshot.  The chat loop, the lead
agent and a skill with scripts or dependencies each make such a copy to hold
their per-request tools (see [Section 7](#7-per-request-tools)).

Constructors: `new()` and `with_event_bus(bus)`, both `Result<Self, String>`
because they build the HTTP client with a **custom redirect policy** (every
redirect target is re-validated with `validate_url()`, capped at 10
redirects).  The daemon uses `with_event_bus` so the ledger can publish
extension events; `new()` is what tests use.  Both install the default
`AnnotationCapabilityProvider`.

### Key Methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `register()` | `(&self, RegisteredTool) -> Result<(), String>` | Add/replace a tool at any time. Rejects empty names, names longer than 256 **bytes** (the check is `str::len`, so a non-ASCII name is refused before it reaches 256 characters), or names containing null bytes. Updates the capability index (string + virtual capabilities). |
| `replace()` | `(&self, RegisteredTool) -> Result<(), String>` | `remove` then `register` — used on re-enable so the index gains no duplicate edges |
| `remove()` | `(&self, name: &str) -> bool` | Remove a tool; scrubs its capability index entries |
| `get()` | `(&self, name: &str) -> Option<RegisteredTool>` | Look up by name (returns a clone — DashMap guards must not cross `.await`) |
| `execute()` | `async (&self, name, args) -> Result<String, String>` | Gate + validate args + dispatch to backend (no context) |
| `execute_with_context()` | `async (&self, name, args, &ToolContext) -> Result<String, String>` | Same, routing BuiltIn backends through `execute_with_context()`. Context is discarded for Http/Command/Plugin/Mcp backends. |
| `extensions()` | `(&self) -> &Arc<ExtensionLedger>` | The ledger |
| `registered_tool_names()` | `(&self) -> Vec<String>` | All tool names |
| `iter_registered_tools()` | `(&self) -> impl Iterator<Item = (String, RegisteredTool)>` | Snapshot iteration (per-entry clones) |
| `count()` | `(&self) -> usize` | Number of registered tools |
| `is_exempt_from_timeout()` | `(&self, name: &str) -> bool` | Whether the sandbox should skip the per-tool timeout |
| `resolve_capabilities()` | `(&self, caps, denied) -> CapabilityResolution` | Capability → tools, plus what is `withheld`, `partially_withheld` or `unknown` (see [Section 4](#4-capability-based-tool-resolution)) |
| `tools_for_capabilities()` / `tools_for_capabilities_with_deny()` | `-> Vec<ToolDefinition>` | Thin wrappers returning only `resolve_capabilities(..).defs` |
| `announce_withheld()` / `announce_withheld_names()` | — | Log and publish what a resolution just lost, attributed to the extension |
| `skill_requirements()` | `(&self, &SkillFrontmatter) -> SkillRequirements` | Whether a skill can still reach what it declares; builds the refusal and the chat prefix |
| `extension_tool_defs()` | `(&self) -> Vec<ToolDefinition>` | Every MCP and plugin tool whose extension is enabled, sorted by name |
| `extension_is_available()` | `(&self, &RegisteredTool) -> bool` | The same state filter, for callers that iterate the registry themselves |
| `command_backend_tool_names()` | `(&self) -> Vec<String>` | Command-backend tools (treated as shell-like by the sanitizer) |
| `register_capability_provider()` | `(&self, Arc<dyn CapabilityProvider>) -> ProviderHandle` | Add a virtual-capability provider; triggers a full index rebuild |
| `remove_capability_provider()` | `(&self, ProviderHandle) -> bool` | Remove a provider; rebuilds the index |
| `known_virtual_capabilities()` | `(&self) -> Vec<String>` | Union of all providers' capability names (used by config validation) |

### Dispatch

Both public execute methods funnel into one private `dispatch()`, so the
extension gate is taken exactly once per call:

1. Look the tool up.  On a **miss**, ask the ledger who used to own the
   name; if that extension is not enabled, return its attributed refusal.
   Otherwise return `Tool '<name>' not found in registry` (or
   `Unknown tool: '<name>'` from the context-free `execute()`).
2. On a **hit**, if the tool belongs to an extension, call
   `ledger.check(ext, name, incarnation, ctx)`.  It refuses when the
   extension is not `Enabled`, when the handle's generation is not the
   current one, or when the server itself withdrew the name.  On success it
   returns a `CallGuard` held across the backend call — that is what a
   disable's drain counts.
3. Validate arguments (below).
4. Dispatch to the backend.

### Argument Validation

Before executing, `validate_tool_arguments()` performs:

1. **Root type check:** If schema specifies `"type": "object"`, args must be an object.
2. **Required fields:** All entries in `"required"` array must be present.
3. **Field type matching:** Each property's type is validated against the schema
   (`string`, `number`, `integer`, `boolean`, `array`, `object`, `null`).
4. **Enum constraints:** If a property schema declares `"enum": [...]`, the
   argument value must be one of the listed values.

This is not a full JSON Schema validator — it catches the most common
argument errors early with clear messages.

### HTTP Backend Execution

1. Replace `{param}` placeholders with URL-encoded argument values
2. Detect unsubstituted placeholders (error)
3. SSRF validation of the resolved URL via `validate_url()`
4. Send HTTP request with method, headers, timeout — every **redirect** is
   also SSRF-validated by the client's redirect policy (max 10 redirects)
5. Stream response body with a 1 MB cap
6. On 2xx: return body truncated to 8,192 characters; otherwise error with
   status + first 1,024 characters of the body

### Command Backend Execution

1. Replace `{param}` placeholders with shell-escaped argument values
2. Detect unsubstituted placeholders (error)
3. Build command via `platform::shell_command()`
4. Execute with `tokio::time::timeout`
5. Capture stdout + stderr (cap: 512 KB each)
6. Exit code 0 → stdout; otherwise error including exit code and output

### MCP Backend Execution

`client.call_tool(remote_name, args)`, then `serialize_call_result()`.  On an
error the registry classifies it (`classify_call_failure`); a terminal
class marks the server `failed` in the ledger — guarded so that only the
current generation of an `Enabled` server can be flipped.  A client sealed
by a disable reports `client sealed by disable`.

---

## 4. Capability-Based Tool Resolution

Agents do not list tools — they declare **capabilities**, and the registry
resolves capabilities to tools at dispatch time.

### Capability strings

Every `RegisteredTool` carries `provides_capabilities: Vec<String>`
(e.g. `file_read`, `web_access`, `messaging`, `orchestration`).  The
registry maintains an inverted `capability_index` (capability → tool
names), kept up to date on every register/remove.

| Source | What the tool provides |
|--------|------------------------|
| Built-in | The capability in the [inventory](#61-tool-inventory) |
| Custom TOML | Its `provides_capabilities` list |
| MCP | Its own namespaced name, `<server>__<tool>` |
| Plugin | The manifest's `capabilities.provides` list (the same list for every tool of that plugin) |
| Per-request tools, skill scripts | Nothing — they are handed to one caller directly. The exception is the lead agent's own tools, which declare `orchestration` |

### Resolving an agent's tools

**Location:** `tools/mod.rs`

```rust
pub fn resolve_agent_tools(
    agent: &SubAgent,
    tool_registry: &Arc<ToolRegistry>,
    ctx: Option<&ToolContext>,
) -> Vec<ToolDefinition>
```

A tool is included if **any** of its capabilities matches any of the
agent's `capabilities`, and **none** of its capabilities (string or
virtual) appear in the agent's `denied_capabilities`.  Agents with an
empty capability list get no tools.  Anything the resolution lost to a
disabled extension is announced (a warning plus an
`extension_capability_withheld` event, deduplicated per task), and the agent
runs with what is left.

`resolve_capabilities()` classifies each requested capability:

| Class | Meaning | Effect |
|-------|---------|--------|
| resolved | At least one enabled tool provides it | Its tools join `defs` |
| `withheld` | No tool provides it now, and the ledger remembers an extension that did | Announced; a **skill** with a withheld requirement is refused |
| `partially_withheld` | A tool still provides it, but a remembered provider is gone | Announced; never blocks |
| `unknown` | Nothing provides it and nothing ever did (a typo, or a never-connected server) | `debug!` only |

Agent templates declare capabilities in their YAML frontmatter
(`config/agents/*.md`):

```yaml
---
id: "code_agent"
capabilities:
  - "file_read"
  - "file_write"
  - "artifact_write"
  - "shell_execute"
  - "memory_read"
  - "workspace_read"
  - "workspace_write"
  - "read_result"
denied_capabilities:
  - "web_access"
max_tool_calls: 50
timeout_seconds: 600
---
```

`AgentTemplate::to_subagent()` always adds `workspace_read` and
`workspace_write`, and adds the coordination tool names for a template with
the `orchestration` capability.

### Virtual capabilities (annotation:*)

**Location:** `tools/registry/capabilities.rs`

`CapabilityProvider` is an extension point that derives additional
("virtual") capability strings from a `RegisteredTool`:

```rust
pub trait CapabilityProvider: Send + Sync {
    fn derive_capabilities(&self, tool: &RegisteredTool) -> Vec<String>;
    fn known_capability_names(&self) -> Vec<String>;
}
```

The default `AnnotationCapabilityProvider` maps MCP annotation hints to 8
capability names:

| Hint value | Capability |
|-----------|------------|
| `read_only_hint = Some(true)` | `annotation:readonly` |
| `read_only_hint = Some(false)` | `annotation:non_readonly` |
| `destructive_hint = Some(true)` | `annotation:destructive` |
| `destructive_hint = Some(false)` | `annotation:non_destructive` |
| `idempotent_hint = Some(true/false)` | `annotation:idempotent` / `annotation:non_idempotent` |
| `open_world_hint = Some(true/false)` | `annotation:open_world` / `annotation:non_open_world` |

`None` hints produce nothing.  This lets an agent declare
`denied_capabilities: ["annotation:destructive"]` and automatically
exclude every destructive-flagged tool, whatever its source.  A plugin whose
manifest declares `capabilities.virtual.provides` registers its own
provider when it loads and removes it when it unloads.

Registering or removing a provider triggers a full rebuild of the
capability index (serialized by `provider_mutex`; readers may observe a
sub-millisecond transient partial state).  `ProviderHandle` values are
process-unique and do not survive restarts.

---

## 5. Extension Ledger and Gate

**Location:** `tools/extensions/`

### Identity and state

```rust
pub enum ExtensionKind { Mcp, Plugin }
pub struct ExtensionId { pub kind: ExtensionKind, pub name: String }  // plugin name = its directory

pub enum ExtensionState {
    Enabled,
    Disabled,
    Unapproved { reason: UnapprovedReason },   // never_seen | denied | capabilities_grew
    Failed { reason: FailureReason, detail: String, since: DateTime<Utc> },
    Orphaned,                                  // plugin permissions entry whose directory is gone
    Enabling,                                  // transient
    Disabling,                                 // transient
}

pub enum FailureReason {
    NeedsAuthorization, NeedsConfig { missing: Vec<String> }, ConfigInvalid,  // actionable
    Unreachable, Crashed,                                                      // retry
}
```

State is observed reality and lives in memory only.  What persists is the
owner's toggle: `enabled` in `config/mcp.toml` for a server, and
`.permissions.toml` at the plugins root for a plugin (which also holds the
consent decision).

### ExtensionLedger

Pure bookkeeping — no clients, processes or paths.  The parts a tool author
meets:

| Method | Purpose |
|--------|---------|
| `check(ext, tool_name, incarnation, ctx)` | **The gate.** `Ok(CallGuard)` or the attributed refusal string. An extension with no record at all passes. |
| `begin(ext, target, cause)` / `commit(ext, state)` | Compare-and-set state transitions; `begin(…Enabling…)` bumps and returns the generation |
| `mark_failed(ext, generation, reason, detail)` | Flip an `Enabled` extension to `Failed` — ignored for a stale generation |
| `record_tools(ext, names)` / `owner_of(name)` | Remember which extension provides a name (case-insensitive lookup), so a miss can be attributed |
| `begin_run(ext, generation)` / `run_scoped(ext, fut)` | Guard a plugin's out-of-process skill or agent run, so a disable can drain it |
| `state(ext)`, `describe_state(ext, audience)` | Read the state, and its wording for the model or for a person |

### ExtensionSupervisor

```rust
#[async_trait]
pub trait ExtensionSupervisor: Send + Sync {
    async fn enable(&self, id: &ExtensionId) -> Result<ExtensionRecord, ExtensionError>;
    async fn disable(&self, id: &ExtensionId) -> Result<ExtensionRecord, ExtensionError>;
    async fn reload(&self, id: &ExtensionId) -> Result<ExtensionRecord, ExtensionError>;
    async fn reconcile(&self, id: &ExtensionId) -> Result<ExtensionRecord, ExtensionError>;
    async fn reconcile_all(&self);
    async fn list(&self) -> Vec<ExtensionRecord>;
    async fn shutdown_all(&self);
}
```

Implemented by `McpSupervisor` (daemon) and `PluginManager` (plugins crate).
The trait lives in `openalpaca_core` because both implementors are
downstream of it.

### Routes and CLI

| Route | Purpose |
|-------|---------|
| `GET /v1/extensions` | List every server and plugin (there is no per-extension GET) |
| `POST /v1/extensions/{kind}/{id}/{verb}` | `enable`, `disable`, `reload`, `approve`, `deny` |
| `GET\|POST /v1/extensions/{kind}/{id}/config` | Read (redacted) or write an extension's config |
| `POST /v1/extensions/{kind}` | Install a plugin / declare an MCP server |
| `POST /v1/extensions/plugin/validate` | Dry-run a plugin install |
| `PUT\|DELETE /v1/extensions/{kind}/{id}` | Update, or uninstall / remove |

These routes answer errors as a flat `{"error":"<word>"}`.  CLI:
`openalpaca ext list|info|enable|disable|reload|approve|deny|remove|install|update|uninstall|mcp`
(`openalpaca ext --help`).

---

## 6. Built-in Tools

### 6.1 Tool Inventory

All of these are registered in the shared `ToolRegistry` at daemon startup.

| Tool | Parameters | Capability | Key Constraints |
|------|-----------|------------|-----------------|
| `web_search` | `query` (required), `count` (default 5, max 20) | `web_access` | Brave Search API; requires `web_search.api_key` in `llm.toml` |
| `web_fetch` | `url` (required) | `web_access` | SSRF-protected; response cap 1 MB |
| `file_read` | `path` (required) | `file_read` | Workspace-scoped (relative paths only); max 10 MB |
| `file_write` | `path`, `content` (required) | `file_write` | Workspace-scoped; max 10 MB; blocks SOUL.md/USER.md/IDENTITY.md (use `update_persona`); inside a session it snapshots a file before overwriting it and refuses the write if the snapshot cannot be kept |
| `artifact_write` | `name`, `kind`, `content` (required); `note`, `summary`, `metadata` | `artifact_write` | `kind` ∈ `markdown`, `code`, `terminal`, `table`, `plan`, `image`, `html`, `binary`; versioned; max `max_artifact_bytes` (10 MB); store chosen from `request_workspace_root`, else the home store; needs a database |
| `read_result` | `result_ref` (required), `offset`, `limit` | `read_result` | Reads only inside the calling session's `results/`; page 8 KB default, 64 KB max; needs `ToolContext.session_id` |
| `shell_execute` | `command` (required) | `shell_execute` | 300s internal safety timeout; output cap 512 KB; injection patterns blocked by sanitizer |
| `memory_search` | `query` (required), `limit` | `memory_read` | Registered only when DB + daemon config provided; hybrid FTS5 + sqlite-vec when an embedder is present; cascading workspace → global scope; owner from `ToolContext` |
| `workspace_read` | `key` (optional) | `workspace_read` | Reads shared task workspace; requires `ToolContext.task_id` |
| `workspace_write` | `key`, `content` (required), `entry_type`, `file_asset_id` | `workspace_write` | 32 KB content cap; optimistic locking with up to 5 jittered-backoff retries; an `artifact` entry is also saved to the artifact store and keeps a 512-character preview |
| `update_persona` | `target` (`soul`/`user`/`identity`), `mode` (`replace`/`sections`), `content_b64`, `sections` | `persona_write` | Edits persona docs with validation + timestamped backups |
| `send` | `action` (`message`/`file`), `channel`, `recipient` (required); `content` / `file_path` + `filename` | `messaging` | Registered when a `ConnectorSendLock` is supplied (the daemon always supplies one); errors at call time when no connector can send |

Built-in tools also carry MCP-style annotations
(`annotations_for_builtin()` in `builtins/mod.rs`):

- read-only, closed-world: `file_read`, `workspace_read`, `memory_search`, `read_result`
- read-only, open-world: `web_fetch`, `web_search`
- destructive, closed-world: `file_write`, `workspace_write`, `artifact_write`, `update_persona`
- destructive, open-world: `shell_execute`, `send`

The destructive annotations feed the default confirmation set
([Section 11.1](#111-sandboxmanager)) and the `annotation:*` virtual
capabilities.

### 6.2 Registration Functions

**Location:** `tools/builtins/mod.rs`

```rust
/// Core built-in tools (no persona context needed)
pub fn builtin_tools(
    db: Option<Database>,
    embedder: Option<Arc<dyn Embedder>>,
    daemon_config: Option<Arc<ArcSwap<DaemonConfig>>>,
    web_search_config: Option<Arc<ArcSwap<WebSearchConfig>>>,
    workspace_root: Option<PathBuf>,
) -> Vec<RegisteredTool>
// Returns: web_search, web_fetch, file_read, file_write, shell_execute,
//          read_result, artifact_write, workspace_read, workspace_write (always)
//          + memory_search (only if db + daemon_config provided)

/// Full built-in tools with persona and connector context
pub fn builtin_tools_with_persona_context(
    db: Option<Database>,
    embedder: Option<Arc<dyn Embedder>>,
    persona_ctx: PersonaToolContext,
    daemon_config: Option<Arc<ArcSwap<DaemonConfig>>>,
    web_search_config: Option<Arc<ArcSwap<WebSearchConfig>>>,
    workspace_root: Option<PathBuf>,
    connector_send_provider: Option<ConnectorSendLock>,
) -> Vec<RegisteredTool>
// Returns: all of builtin_tools() PLUS update_persona,
//          and send (only when connector_send_provider is Some)

/// Workspace tool definitions (schemas only; builtin_tools() registers
/// them with BuiltIn backends)
pub fn workspace_tool_definitions() -> Vec<ToolDefinition>
```

`workspace_root` defaults to `std::env::current_dir()` when `None`.  The
daemon captures the workspace root once at startup; `file_read`,
`file_write` and nothing else use it.

### 6.3 Skill Scripts (`ScriptToolBuiltIn`)

Skills may bundle executable scripts under `<skill>/scripts/`.  Each is
registered as a `BuiltInTool` named `skill_script:<name>`, into a copy of
the registry made for that one invocation:

- `ScriptToolBuiltIn::new()` canonicalizes the script path and rejects
  anything resolving outside the skill's `scripts/` directory (path
  traversal blocked).  A missing script fails the whole invocation.
- Arguments are converted to `--key=value` CLI flags via
  `json_to_cli_args()`.
- Executed with the configured interpreter — or directly when none is set,
  which needs the exec bit and a shebang — with a per-script timeout
  (default 30 s), working directory = the skill directory, and a 512 KB
  output cap.

### 6.4 Workspace Path Helpers

**Location:** `tools/builtins/helpers/mod.rs`

| Function | Purpose |
|----------|---------|
| `validate_workspace_path(path)` | Reject absolute paths and `..` components |
| `resolve_workspace_path(rel, root)` | Canonicalize + verify stays within workspace |
| `resolve_workspace_path_for_write(rel, root)` | Like above but for new files (parent may not exist) |
| `is_soul_path(path)` / `is_user_path(path)` / `is_identity_path(path)` | Case-insensitive protected-file checks |
| `unique_backup_path(dir, prefix)` | Generate timestamped backup path with unique suffix |
| `prune_backups(dir, max, prefix)` | Remove oldest backups exceeding retention limit |

### 6.5 Size Limits

| Constant | Value | Location |
|----------|-------|----------|
| `MAX_FILE_READ_SIZE` | 10 MB | `builtins/helpers/mod.rs` |
| `MAX_FILE_WRITE_SIZE` | 10 MB | `builtins/file_ops.rs` |
| `max_artifact_bytes` (config) | 10 MB | `[execution.artifacts]` |
| `snapshot_max_bytes` (config) | 10 MB | `[orchestrator.sessions]` |
| `MAX_WORKSPACE_CONTENT_SIZE` | 32 KB | `builtins/mod.rs` |
| `SPILL_PREVIEW_CHARS` (workspace artifact entry) | 512 chars | `builtins/artifact_write.rs` |
| `MAX_SCRIPT_OUTPUT_BYTES` | 512 KB | `builtins/mod.rs` |
| `MAX_TOOL_RESULT_SIZE` (fallback for `tool_result_inline_bytes`) | 32 KB | `agentic_loop/tool_helpers.rs` |
| `PREVIEW_CHARS` (result spill preview) | 2048 chars | `session_log/record.rs` |
| `read_result` page | 8 KB default, 64 KB max | `builtins/read_result.rs` |
| HTTP backend response cap | 1 MB (stream), 8 KB returned | `registry/mod.rs` |
| Command backend output cap | 512 KB per stream | `registry/mod.rs` |
| `shell_execute` output cap | 512 KB per stream | `builtins/shell_execute.rs` |

---

## 7. Per-Request Tools

These tools are **not in the shared registry**.  Each caller value-clones
the registry, registers its own instances into the clone, and builds its
`SandboxManager` over that clone.  None carries annotations, and only the
lead's own tools (the coordination tools, `post_update`, `queue_followup`)
declare a capability, `orchestration`.  They reach the model because the
caller lists them, and the caller's allow list is built from that same list.

### 7.1 Chat Main Loop

**Location:** `tools/builtins/main_loop.rs` — `main_loop_tool_set()`

| Tool | Parameters | When offered |
|------|-----------|--------------|
| `start_workflow` | `goal` (required), `title` | Always |
| `task_status` | `task_id` (optional) | Always |
| `memory_store` | `content` (required) | With a database |
| `memory_forget` | `query` (required) | With a database |
| `memory_search` | see 6.1 (definition only; the backend is shared) | When registered |
| `read_result` | see 6.1 (definition only) | With a session log **and** a database |
| every `<server>__<tool>` / `<plugin>::<tool>` | — | When its extension is enabled (`extension_tool_defs()`) |
| `invoke_skill` | `skill`, `query` (both required) | With an LLM router |
| `steer_workflow` | `task_id`, `message` (both required) | Lane has an active workflow **and** `steering_enabled` |
| `queue_followup` | `description` (required) | Same condition |

The caller unions this set with its base picks:
`[orchestrator.routing] tool_selection = "core_union"` (default) adds
keyword-suggested built-ins; `"full"` adds the whole shared registry minus
tools of extensions that are not enabled.  The chat loop's tool identity is
`agent_id = "orchestrator"`.

### 7.2 Lead Agent

**Location:** `runner/lead_agent/mod.rs` (surface), `runner/lead_agent/tools.rs` (tools)

For each lead-agent run, `run_lead_agent` clones the registry and calls
`register_coordination_tools()` (capability `orchestration`).  This is also
why custom TOML tools may not use the four coordination names.

| Tool | Parameters | Notes |
|------|-----------|-------|
| `spawn_subagent` | `agent_id`, `objective` (both required) | Non-blocking; returns a `run_id`. The tool description embeds the catalog of worker templates. Guards: a lead cannot spawn its own template; depth limit `MAX_SUBAGENT_DEPTH` (3); concurrency semaphore sized by `max_concurrent_subagents` (waits up to 30 s for a slot) |
| `spawn_subagents_batch` | `subagents`: 1–8 × `{agent_id, objective}` | Registered when `execution.lead_agent_defaults.batch_spawn_enabled` (default `true`); more than 8 items is an error |
| `check_subagent_status` | `subagent_run_id` (required) | `exempt_from_timeout: true` |
| `wait_for_subagents` | none | Blocks until every spawned subagent finishes, or returns early when the user steers the run. `exempt_from_timeout: true` |
| `post_update` | `message` (required) | Posts a progress note to the lane. Only when `steering_enabled` |
| `queue_followup` | `description` (required) | Queues work to start after the run. Only when `steering_enabled` |
| `invoke_skill` | `skill`, `query` | Same tool as the chat loop's |

The lead's surface also lists the workspace tools, `memory_search`, every
enabled extension tool, and — only when its own template grants the
capability — `artifact_write` and `read_result`.  Its allow list is its
template's capabilities plus the names on the assembled surface; a template
that granted nothing is not back-filled.

A **subagent** gets `resolve_agent_tools()` for its template and a policy
from `SandboxPolicy::from_constraints()`, widened by
`admit_tool_surface()` with that same resolved surface
([11.2](#112-sandboxpolicy)) — so a tool it is offered is a tool its
allow list admits.  The one exception is a template that denies a tool by
*name*: the surface is filtered by capability, so the tool is still
offered, and the sandbox refuses the call because the deny list is checked
first.  A subagent does not inherit the lead's extension tools or
`invoke_skill`.  A plugin-contributed agent template runs
through `runner/plugin_agent.rs` instead of the internal loop, with its
tool requests proxied through the same sandbox.

### 7.3 Skill-Scoped Tools

`skill_script:<name>` ([6.3](#63-skill-scripts-scripttoolbuiltin)) and
`invoke_skill:<id>` (one per `depends_on` entry, `exempt_from_timeout:
true`) are registered into a registry copy made for one skill invocation,
with `author = "skill:<name>"`.  See
[Skill_Template_Reference.md](../Skill_Template_Reference.md).

---

## 8. Custom Tools (TOML Configuration)

**Location:** `tools/config/mod.rs`

The daemon loads every `*.toml` file in `config/tools/` at startup
(`load_tools_from_dir()` — parse failures are logged, not fatal).

### File Format

```toml
# config/tools/my_tools.toml

[[tools]]
name = "my_api_tool"
description = "Calls my REST API"
provides_capabilities = ["my_api"]   # REQUIRED in practice: resolution is
                                     # capability-based, so a tool with no
                                     # capabilities is never selected for
                                     # any agent
version = "1.0.0"                    # optional, default "0.0.0"
author = "me"                        # optional, default "user"

[tools.parameters]
type = "object"
required = ["query"]

[tools.parameters.properties.query]
type = "string"
description = "Search query"

[tools.backend]
type = "http"                    # or "command"
url = "https://api.example.com/search?q={query}"
method = "GET"                   # default: GET
timeout_secs = 30                # default: 30, range: 1-300

# Optional: MCP-style annotation hints (feed confirmation defaults and
# annotation:* capabilities)
[tools.annotations]
read_only_hint = true
# destructive_hint / idempotent_hint / open_world_hint also supported

# Optional for HTTP backend:
# [tools.backend.headers]
# Authorization = "Bearer my-token"
```

### Types

```rust
#[derive(Deserialize)]
pub struct ToolConfig {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    pub backend: ToolBackendConfig,
    #[serde(default)]
    pub provides_capabilities: Vec<String>,
    #[serde(default = "default_tool_version")]  // "0.0.0"
    pub version: String,
    #[serde(default = "default_tool_author")]   // "user"
    pub author: String,
    #[serde(default)]
    pub annotations: Option<ToolAnnotationsConfig>,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
pub enum ToolBackendConfig {
    #[serde(rename = "http")]
    Http { url: String, method: Option<String>,
           headers: Option<HashMap<String, String>>, timeout_secs: Option<u64> },
    #[serde(rename = "command")]
    Command { command: String, args_template: Option<String>,
              timeout_secs: Option<u64> },
}
```

Load-time validation: non-empty name; HTTP URLs must start with
`http://`/`https://`; `timeout_secs` must be in `[1, 300]`; any
`annotation:`-prefixed entry in `provides_capabilities` must be one of the
8 known annotation capability names.  One invalid tool rejects its whole
file.

### Template Substitution

Both HTTP and Command backends support `{param}` template placeholders:

- **HTTP URLs:** values are URL-encoded (`urlencoding::encode()`)
- **Command args:** values are shell-escaped (`shell_escape::escape()`)
- **Unsubstituted placeholders** are detected and cause an error

### Protected Tool Names

Protection is **dynamic**, not a fixed list
(`apps/openalpacad/src/services/tools.rs`): a custom tool is skipped with a
warning if its name collides with

1. any already-registered built-in tool name (collected from the registry
   after built-in registration — every name in the
   [inventory](#61-tool-inventory)), or
2. the lead-agent coordination names: `spawn_subagent`,
   `spawn_subagents_batch`, `check_subagent_status`, `wait_for_subagents`.

Tools that fail registry name validation are also skipped with a warning.
Custom tools load before MCP servers connect.

---

## 9. MCP Tools

OpenAlpaca connects **out** to external MCP servers and imports their
tools.  It does not expose its own tools over MCP.

### Configuration — `config/mcp.toml`

**Parser:** `tools/mcp/config.rs`.  A missing file simply means no MCP
servers.  The shipped `config/mcp.toml` contains only commented-out
examples.

```toml
[defaults]
connect_timeout_secs = 30       # default 30
request_timeout_secs = 30       # default 30
max_reconnect_attempts = 3      # default 3
reconnect_backoff_ms = 100      # default 100

[servers.fs]                    # name must match ^[a-zA-Z][a-zA-Z0-9_-]{0,30}$
transport = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "/tmp"]
# env = { RUST_LOG = "info" }                   literal values
# env_from = { GITHUB_TOKEN = "GH_PAT" }        child var ← daemon's env var
# cwd = "..."   enabled = true (default)
# connect_timeout_secs / request_timeout_secs override defaults

[servers.remote]
transport = "http"              # MCP streamable-HTTP
url = "https://example.com/mcp"
auth = { bearer_env = "MY_TOKEN" }
# auth variants: { bearer = "..." }
#                { api_key_header = "X-API-Key", api_key_env = "ENV_VAR" }
# extra_headers = { "X-Client-Name" = "openalpaca" }        literal values
# extra_headers_from = { Authorization = "MY_TOKEN" }       header ← daemon's env var
```

`env_from`, `extra_headers_from` and the `*_env` auth variants name an
environment variable of the **daemon**; the value is read when the server
is started.  A variable that is not set fails that server's start and names
the variable.  `openalpaca ext mcp add` refuses to write a literal secret
under a credential-shaped key or header.

`enabled` is the server's ENABLE bit.  The toggle writes it, preserving the
file's comments and key order, and keeps rotated backups under
`state/backups/`.

### Lifecycle

**Location:** `apps/openalpacad/src/services/mcp.rs`, `managers/mcp.rs`

Boot is the supervisor's first `reconcile_all()`:

1. Load `config/mcp.toml`.  A missing file is an empty set.  A file that
   does not parse is **not fatal**: it leaves one record naming the parse
   error, and the daemon boots.
2. A server with `enabled = false` gets a listable `Disabled` record and is
   not connected.
3. Enabled servers are brought up in parallel: build an `McpClientConfig`,
   connect within the per-server (or default) connect timeout, `list_tools`,
   and register every tool via `bridge::rmcp_tool_to_registered()`, stamped
   with the load's generation.
4. A failure (invalid config, connect failure/timeout, `list_tools` error)
   leaves a `Failed` record with a reason — never fatal.
5. After agent templates are loaded, a **reaper** task starts: when a call
   finds the connection lost for good, the server is marked `Failed` and its
   tools are withdrawn.

At runtime the supervisor also handles `enable`, `disable`, `reload`
(re-apply an edited block or a rotated credential), `add_server` /
`remove_server`, a hand edit of `config/mcp.toml` (reconciled against a
fingerprint of each block, so a comment or reordering changes nothing), and
a server's own "tool list changed" notification — new tools are registered,
and a tool the server dropped is refused as withdrawn by the server.

The supervisor holds its own `Arc<McpClient>` per server; teardown never
depends on the registry dropping the last reference.

### Tool bridging

**Location:** `tools/mcp/bridge.rs`

- Registered name is **namespaced**: `<server_name>__<remote_name>`
  (e.g. `fs__read_file`) so MCP tools cannot collide with built-ins or
  other servers.
- `author = "mcp:<server>"`; `version` comes from the server's reported
  version; `annotations` are taken from the server's tool metadata;
  `exempt_from_timeout` is always `false`; `provides_capabilities` is the
  namespaced tool name itself, so agent templates and skills that list
  `<server>__<tool>` in their capabilities resolve the tool through the
  registry's capability index (annotation-derived virtual capabilities
  still apply).
- `serialize_call_result()` flattens a `CallToolResult` into
  `Result<String, String>`: text blocks are concatenated with newlines;
  `is_error = true` becomes a tool error.

### Limitations (current state)

- **Non-text content is dropped**: image/audio/resource blocks in tool
  results are replaced with a bracketed placeholder — they are not
  surfaced to the model.
- **MCP resources and prompts are not implemented**: the client has no
  resource or prompt methods at all — it imports tools and nothing else.
  A future phase would add the methods rather than fill in placeholders,
  and ruling X-36 fixes their lifecycle in advance: once added, a server's
  resources and prompts are discovered, registered and withdrawn by the same
  supervisor, under the same toggle, as its tools.
- Retriable transport errors trigger the client's internal
  reconnect/retry loop (`max_reconnect_attempts`, exponential backoff).  A
  client sealed by a disable does not reconnect.

---

## 10. Plugin Tools

`ToolBackend::Plugin` wraps an
`openalpaca_api::plugin_traits::PluginToolExecutor`.  The out-of-process
plugin system (`crates/openalpaca_plugins`) registers plugin-provided
tools into the shared registry when the plugin loads:

- name `<plugin>::<tool>`, where `<plugin>` is the plugin's directory name;
- `author = "plugin:<id>"`, `version` from the manifest, no annotations,
  `exempt_from_timeout = false`;
- `provides_capabilities` = the manifest's `capabilities.provides` — the
  same list for every tool of the plugin;
- a name already held by another **enabled** extension is skipped with a
  warning.

They are removed when the plugin is disabled, denied, fails or is
uninstalled.  A plugin must be approved before its first load, and again if
a later version asks for more capabilities.

Plugins can also declare virtual capabilities in their manifest, which
are attached via a registered `CapabilityProvider`.

Plugin **tools, skills, and agents** flow through the normal registries.
Plugin *connector* and *LLM-provider* bridges exist in the plugin crate
but are not yet wired into the daemon's connector manager or LLM router —
treat those plugin types as not yet functional.

`ToolContext` is not forwarded to plugin backends; plugin tools receive
only the tool name and arguments.

---

## 11. Security Layers

### 11.1 SandboxManager

**Location:** `security/sandbox/mod.rs`

```rust
pub struct SandboxManager {
    registry: Arc<ToolRegistry>,
    bus: EventBus,
    circuit_breaker: ToolCircuitBreaker,
    db: Option<Database>,                              // audit logging
    confirmation_broker: Option<Arc<ConfirmationBroker>>,
    approval_cache: ApprovalCache,
}
```

Constructors: `new(registry, bus, &CircuitBreakerConfig)`,
`with_db(...)`, `with_defaults(registry, bus)` (tests).  The database and
the broker are attached post-construction via `set_db()` and
`set_confirmation_broker()`.

**Lifetime.**  Every executing path builds its own sandbox: one per chat
turn, per skill invocation, per nested skill, per lead-agent run and per
subagent.  The circuit-breaker state and the approval cache therefore live
for that unit of work and no longer.  A nested skill's sandbox is given no
broker, so confirmation-required tools are fail-closed there.

**`execute_tool()` flow:**

```rust
pub async fn execute_tool(
    &self,
    tool_call: &ToolCall,
    policy: &SandboxPolicy,
    ctx: &ToolContext,
) -> Result<String, String>
```

The agent identity comes from `ctx.agent_id` (falls back to `"unknown"`).

1. `CapabilityManager::check_agent_capability()` — deny list, then allow list
2. `InputSanitizer::sanitize_tool_args()` — injection/traversal checks
   (allowlist = registered tool names; shell-like = command-backend tools)
3. **Confirmation gate** — the *effective confirmation set* is
   `policy.require_confirmation_for` if non-empty, otherwise all
   registered tools with `destructive_hint = Some(true)`.  For a tool in
   the set, the first matching arm applies:
   - **ApprovalCache hit** (prior approval in this sandbox for these args
     or the whole tool) → proceed without prompting
   - `policy.auto_approve` → proceed; decision persisted to `event_log`
     as `tool_auto_approved`
   - `policy.unattended` → **refused at once** with a message saying the
     run cannot ask and where it can be approved; persisted to `event_log`
     as `tool_approval_unavailable` (`UNAPPROVABLE_EVENT_TYPE`), which a
     workflow's completion report reads back
   - Broker present → register the request, publish
     `SystemEvent::ToolConfirmationRequested`, then `broker.wait()` for up
     to `confirmation_timeout_secs` (default 300 s).  Whatever the wait
     returns, publish `SystemEvent::ToolConfirmationResolved` with the
     outcome.  On approval, the decision is cached with its `ApprovalScope`
     (defaults to `TheseArgs`); `denied`, `timed_out` and `cancelled`
     return an error.
   - No broker → **fail-closed**: blocked immediately
4. `circuit_breaker.check()` — per (agent, tool) consecutive-failure state
5. Timeout-wrapped execution of
   `registry.execute_with_context(name, args, ctx)` — skipped for tools
   whose `RegisteredTool.exempt_from_timeout` is true.  The context handed
   on has `event_bus` filled in.  The extension gate runs inside this call.
6. `circuit_breaker.record_success/record_failure_for_task()` — failures
   are only recorded when `is_transient_tool_error()` classifies them as
   transient **and** the error is not an extension-withheld refusal
7. Emit `SystemEvent::ToolExecuted` or `SystemEvent::SecurityViolation`
   (violations are also persisted to `event_log` when a DB is attached)

### 11.2 SandboxPolicy

```rust
pub struct SandboxPolicy {
    pub agent_id: String,
    pub allowed_capabilities: Allowlist,          // Only(vec![]) admits nothing
    pub denied_capabilities: Vec<String>,
    pub require_confirmation_for: Vec<String>,
    pub max_tool_calls: Option<u32>,
    pub max_tool_runtime_secs: u64,
    pub stream_id: Option<String>,                // SSE confirmation routing
    pub lane_key: Option<String>,                 // connector confirmation routing
    pub confirmation_timeout_secs: Option<u64>,   // default 300
    pub auto_approve: bool,
    pub unattended: bool,                         // nobody can answer a prompt
}
```

`SandboxPolicy::from_constraints(agent_id, &AgentConstraints)` builds a
policy from an agent template's constraints
(`max_tool_runtime_secs` defaults to 60 when the template sets no
timeout; `unattended` starts `false` and is set by the caller that knows
where the work came from).  The daemon's
`security.auto_approve_confirmations` config flag forces
`auto_approve = true` globally.  `auto_approve` is read before
`unattended`.

How each caller fills the allow list:

| Caller | `allowed_capabilities` |
|--------|------------------------|
| Chat loop | The names of the tools it was handed |
| Skill (file-based or nested) | The names of the tools resolved for it |
| Plugin-backed skill | The resolved names; an empty list admits nothing |
| Lead agent | Template capabilities + coordination names + the assembled surface |
| Subagent | Template capabilities + `workspace_read`/`workspace_write` + the surface those capabilities resolved to |

A template grants **capability names**; the check in 11.3 compares the
called **tool's name**.  The two are the same word for `file_read`,
`file_write`, `artifact_write`, `shell_execute`, `read_result`, the
workspace tools and any `<server>__<tool>`, and different words wherever
one capability is served by tools with names of their own: `web_access`
is `web_search` and `web_fetch`, `memory_read` is `memory_search`,
`messaging` is `send`.  So one rule holds for every row: **the allow list
admits the tools the loop was handed.**  The first three rows build their
list from the exposed definitions directly.  The lead and the subagent
start from `from_constraints` — the template's capability names — and
then call

```rust
impl SandboxPolicy {
    pub fn admit_tool_surface(&mut self, defs: &[ToolDefinition]);
}
```

with the surface they resolved: the lead's assembled surface
(`runner/lead_agent/mod.rs`), and for a subagent the output of
`resolve_agent_tools()` for its own template (`runner/lead_agent/tools.rs`,
the spawn path).  A subagent granted `web_access` is therefore handed
`web_search` and `web_fetch` **and** may call them.  A plugin-backed
subagent's proxied tool calls are checked against the same policy.

`admit_tool_surface` adds the names on the surface and nothing else:

- a tool that is registered but was not resolved onto this surface gains
  nothing from it — a `web_access`-only subagent cannot call
  `shell_execute`, and a subagent never inherits the lead's extension tools
  or `invoke_skill`;
- the list still carries the template's capability *strings*, and the check
  is a name match, so a registered tool whose **name equals** one of those
  strings is admitted whether or not it is on the surface.  That was true
  before the widening and is unchanged by it; none of the nine shipped
  templates lists a string that names an off-surface tool;
- the deny list is untouched and is checked first (11.3).  For a subagent,
  `resolve_capabilities()` has also kept a tool that provides a denied
  capability off the surface, so it is never added.  The lead's surface is
  assembled by hand rather than resolved, so there only the name check
  applies: a lead template must deny a tool by name to refuse it;
- a tool of an extension that is not `Enabled` is not registered, so it is
  never on a resolved surface, and the gate in section 5 refuses it
  regardless of any allow list;
- an `Only` list that is **empty stays empty** — a template that granted
  nothing is not back-filled from an assembled surface — and
  `Unrestricted` is left alone;
- names are lowercased and never duplicated.

### 11.3 Allowlist and CapabilityManager

**Location:** `security/capabilities/mod.rs`

```rust
pub enum Allowlist {
    Unrestricted,          // no production caller uses this
    Only(Vec<String>),     // exactly these, pre-lowercased; empty = nothing
}

pub fn check_agent_capability(
    agent_id: &str,
    tool_name: &str,
    allowed: &Allowlist,
    denied: &[String],
) -> Result<(), SecurityViolation>
```

**Rules** (matching is **case-insensitive** — the tool name is lowercased
and compared to pre-lowercased list entries; build lists with
`Allowlist::only(..)`, which lowercases):

1. If `denied` contains the tool name → **DENIED** (deny beats allow)
2. If `allowed` is `Only(list)` and the list does not contain the tool name → **DENIED**
3. Otherwise → **ALLOWED**

The comparison is against the **tool's name**.  `check_model_access()`
applies a deny/allow pattern to model IDs from `AgentConstraints`.

### 11.4 InputSanitizer

**Location:** `security/sanitizer/mod.rs`

```rust
pub fn sanitize_tool_args(
    tool_name: &str,
    arguments: &serde_json::Value,
    allowed_tools: &[String],
    extra_shell_tools: &[String],
) -> Result<(), SecurityViolation>
```

**Checks performed:**
- Tool name in allowlist (if non-empty)
- Recursively, for every string value: path traversal (`../`, `..\`)
  and null bytes
- For shell-like tools only: backtick command substitution, `$(`
  subshell, newline, and carriage return.  Ordinary shell operators
  (pipes, redirection, `&&`) are intentionally allowed.

**Shell-like tools:** `shell_execute` (hardcoded) + Command-backend tools
(supplied by the sandbox from `registry.command_backend_tool_names()`).

`InputSanitizer` also provides `sanitize_user_input()` (length + null
bytes) and `validate_upload()` (path traversal, size, MIME polyglot
detection, ZIP-bomb heuristic, image dimension bounds) for the upload
path.

### 11.5 ToolCircuitBreaker

**Location:** `security/circuit_breaker/mod.rs`

```rust
pub struct ToolCircuitBreaker {
    state: Mutex<HashMap<(String, String), ToolState>>,  // (agent_id, tool_name)
    failure_threshold: usize,
    reset_timeout: Duration,
    enabled: bool,
    bus: EventBus,
    reset_timeout_secs: u64,
}
```

**State machine:** Closed → (consecutive transient failures ≥ threshold)
→ Open → (reset timeout elapsed) → Half-Open (single probe) → Closed on
success / back to Open on failure.  While Half-Open, additional calls are
blocked until the probe resolves.  Trips emit
`SystemEvent::CircuitBreakerTripped`.

**Error classification** (`is_transient_tool_error()`):
- **Transient** (counts toward tripping): timeouts, HTTP 5xx, connection
  refused/reset, network errors
- **Permanent** (ignored by the breaker): bad arguments, 404, tool not found

**Memory management:** when the state map exceeds 10,000 entries, entries
idle for more than 1 hour are pruned.

### 11.6 ConfirmationBroker & ApprovalCache

**Location:** `security/confirmation.rs`

```rust
pub struct ConfirmationBroker {
    pending: DashMap<String, (ConfirmationRequest, oneshot::Sender<ConfirmationResponse>)>,
}

pub enum ConfirmationResolution { Answered(ConfirmationResponse), TimedOut, Cancelled }
```

| Method | Description |
|--------|-------------|
| `request(&ConfirmationRequest) -> oneshot::Receiver<ConfirmationResponse>` | Register pending confirmation |
| `wait(id, rx, timeout) -> ConfirmationResolution` | Await the answer. The broker owns the clock: a timeout clears the entry and is reported as `TimedOut` |
| `respond(id, ConfirmationResponse) -> Result<(), String>` | Deliver user's decision |
| `cancel(id)` | Withdraw a pending request |
| `pending_requests() -> Vec<ConfirmationRequest>` | Snapshot of what is still waiting — served by `GET /v1/chat/confirmations` and used to mark a blocked run |
| `pending_count()` / `pending_keys()` | Diagnostics |

**ConfirmationRequest fields:** `request_id`, `agent_id`, `tool_name`,
`tool_arguments`, `stream_id`, `lane_key`, `task_id`,
`agent_instance_id`, `timestamp`.

**ConfirmationResponse:** `{ approved: bool, approval_scope: Option<ApprovalScope> }`
with `ApprovalScope::TheseArgs | EntireTool` (`snake_case` on the wire;
missing scope defaults to `TheseArgs` at enforcement time).

`ConfirmationResolution::outcome()` maps to the wire `ConfirmationOutcome`:
`approved`, `denied`, `timed_out`, `cancelled`.

**ApprovalCache** — lock-free (`DashSet` behind `Arc`), owned by one
`SandboxManager`.  Keys are the tool name (EntireTool) or
`(tool_name, args_hash)` (TheseArgs), where `args_hash` comes from
`hash_canonical_args()` — a 64-bit hash over JSON with recursively sorted
object keys, so argument key order does not defeat the cache (array order
still matters).

**Answering.**  `POST /v1/chat/confirmations/{request_id}`.  Clients: the
GUI's approval card, the CLI's inline prompt at an interactive
`openalpaca chat`, and `openalpaca tasks confirmations list|watch|approve|deny`.

### 11.7 TrustGate (Principal-Level)

**Location:** `security/policy.rs`

```rust
pub enum Principal {
    System,                                    // Full access
    User { global_id: String },                // High trust
    External { provider: String, id: String }, // Low trust
}

pub enum Scope {
    Global,
    Workspace { path: String },
    Conversation { id: String },
}
```

**Rules:**
- `System` → always allowed
- `External` → blocked on high-risk actions (`system.*`, `fs.write*`,
  `net.connect*`) and on `chat.respond` (forces the account-link flow)
- `User` → generally allowed (granular ACLs are a future extension)

---

## 12. Agentic Loop Integration

**Location:** `runner/agentic_loop/mod.rs`.  The loop contract as a whole —
routing, steering, streaming, the no-answer line — is documented in
[agent-loop.md](../agent-loop.md).

### Entry Points

```rust
/// Legacy/test entry point (direct provider, no retry)
pub async fn run_agentic_loop(
    provider: &dyn LlmProvider,
    initial_messages: Vec<ChatMessage>,
    tools: Vec<ToolDefinition>,
    config: &LoopConfig,
    sandbox: Option<&SandboxManager>,
    agent_id: &str,
    sandbox_policy: Option<&SandboxPolicy>,
    context_budget: Option<&ContextBudgetManager>,
    cancel_token: Option<CancellationToken>,
    tool_context: Option<&ToolContext>,
) -> LoopResult

/// Production entry point (router + key rotation + fallback + cost tracking)
pub async fn run_agentic_loop_routed(
    router: &LlmRouter,
    initial_messages: Vec<ChatMessage>,
    tools: Vec<ToolDefinition>,
    config: &LoopConfig,
    sandbox: Option<&SandboxManager>,
    agent_id: &str,
    sandbox_policy: Option<&SandboxPolicy>,
    task_id: Option<&str>,
    context_budget: Option<&ContextBudgetManager>,
    cancel_token: Option<CancellationToken>,
    tool_context: Option<&ToolContext>,
    cost_accumulator: Option<LoopCostAccumulator>,
) -> LoopResult
```

When `sandbox` is `None`, tool calls return stub results with a warning —
a misconfiguration guard, not a supported mode.

`LoopConfig` fields that matter to tools: `max_tools_per_round`,
`max_tool_runtime`, `initial_tool_choice`, `session_log` (enables the
result spill), `tool_result_inline_bytes`, `span_id`, `event_bus`.
`LoopResult.last_tool_error` keeps the last tool error (capped) for the
runtime's no-answer line.

### Tool Execution Phase (per round)

When the LLM response contains tool calls:

1. Append the assistant message (with tool calls) to the conversation.
2. Compute the remaining budget:
   `policy.max_tool_calls - state.tool_calls_made` (if a policy limit is
   set).
3. Apply the per-round cap: at most `config.max_tools_per_round` calls
   execute this round.
4. Partition into *executable* and *over-limit* calls.  Over-budget calls
   get a `max_tool_calls limit reached` error; overflow beyond the
   per-round cap gets `max tools per round exceeded` — every tool call ID
   receives a tool-result message.
5. Executable calls run **in parallel** via
   `futures_util::future::join_all()` (each future calls
   `SandboxManager::execute_tool(tool_call, policy, ctx)`), raced against
   the cancellation token.
6. Per result: decide whether it spills, write the `tool_call` /
   `tool_result` records to the session log (stamped with the owning
   extension, when there is one), and compute the model-visible text
   ([Section 13](#13-tool-result-handling)).
7. Push `ChatMessage::tool_result(id, text)` per call, bump
   `state.tool_calls_made`, continue to the next round (until
   `max_rounds`, `EndTurn`, cost limit, cancellation, or error).

---

## 13. Tool Result Handling

**Location:** `runner/agentic_loop/mod.rs` (`spill_plan`, `model_visible_result`),
`runner/agentic_loop/tool_helpers.rs`, `session_log/record.rs`

### What the model is handed

The threshold is `LoopConfig.tool_result_inline_bytes`, set from
`[orchestrator.sessions] tool_result_inline_bytes` (default 32 KB;
`MAX_TOOL_RESULT_SIZE` is the compiled fallback).

| Result | Loop has a session log | Loop has none |
|--------|------------------------|---------------|
| `Ok`, within the threshold | As is | As is |
| `Ok`, over the threshold | **Spilled**: the full text goes to `sessions/<id>/results/`; the model gets the stub below | Cut at the threshold by `truncate_tool_result_to()` |
| `Err`, over the threshold | Head **and** tail inline (`head_tail_tool_result()`); the full text is still spilled for the log | Head and tail inline |

The chat loop, the lead agent and its subagents run with a session log.  A
skill invocation does not.

The stub, from `spill_stub()`:

```text
[result too large: <N> bytes; first 2 KB follow]
<first 2048 characters>
[full result: result_ref=file:results/<file> — use read_result to page]
```

`read_result(result_ref, offset?, limit?)` pages the file back in.  It is a
**grant, not ambient**: the chat loop offers it when the daemon has a
session log and a database; the lead and subagents get it when their
template lists the `read_result` capability (all nine shipped templates
do).  An agent without the grant is refused the call.  If the spill record
could not be written, the model gets the inline cut instead of a reference
to a file that does not exist.

### Truncation

```rust
pub(super) fn truncate_tool_result_to(text: String, limit: usize) -> String
```

**Smart boundary detection** (priority order): sentence boundary
(`. `/`.\n`/`! `/`? ` etc.) → line boundary → word boundary → char
boundary, but a boundary is only used if it keeps at least 75% of the
limit (avoids discarding most of the content for a distant sentence end).
Appends `[... truncated: showing first X of Y bytes]`.

`head_tail_tool_result()` keeps half the budget at each end, with a marker
saying how many bytes were elided.

### Error Formatting

```rust
pub(super) fn format_tool_error(msg: &str) -> String
// Returns: "[tool_error] {msg}"

pub(super) fn format_tool_error_with_hint(tool_name: &str, msg: &str) -> String
// Returns the same line, plus a "Hint: …" line when one applies
```

**Recovery hints by tool:**

| Tool | Error Pattern | Hint |
|------|--------------|------|
| `file_read` | "not found" / "No such file" | verify the path exists using shell_execute with `ls` |
| `file_write` | "Permission denied" | check file permissions or try a different output path |
| `web_fetch` | "404" / "not found" | use web_search to find the correct URL first |
| `web_fetch` | "timeout" | the URL may be unreachable; try a different source |
| `shell_execute` | "timed out" | break the command into smaller steps or increase timeout |
| `shell_execute` | "not found" | check if the command is installed or use the full path |
| `memory_search` | "no results" | try broader search terms or check workspace_read for shared context |

---

## 14. Telemetry & Storage

### Database Tables

Defined in `crates/openalpaca_storage/src/migrations/001_baseline.sql` (schema version 42).
The generated [schema reference](../api/database/schema.md) is
authoritative.

#### tool_execution_log

| Column | Type | Description |
|--------|------|-------------|
| `id` | INTEGER PK | Auto-increment row ID |
| `request_id` | TEXT | Correlation to parent request/task (nullable) |
| `agent_id` | TEXT NOT NULL | Executing agent |
| `tool_name` | TEXT NOT NULL | Tool invoked |
| `success` | INTEGER NOT NULL | 0 = failed, 1 = succeeded |
| `duration_ms` | INTEGER NOT NULL | Execution time |
| `error_message` | TEXT | Error text if failed |
| `timestamp` | TEXT | Defaults to `datetime('now')` |
| `session_id`, `task_id` | TEXT | The session and run the call belonged to |
| `log_seq` | INTEGER | Position of the call in the session log |
| `args_preview`, `result_preview` | TEXT | Bounded previews |
| `result_ref` | TEXT | The `results/` reference when the result spilled |

**Indexes:** `idx_tel_tool_ts (tool_name, timestamp DESC)`,
`idx_tel_request (request_id)`, `idx_tel_session (session_id, id)`,
`idx_tel_task (task_id, id)`, `idx_tel_timestamp (timestamp, tool_name)`.

#### skill_execution_log

| Column | Type | Description |
|--------|------|-------------|
| `id` | INTEGER PK | Auto-increment row ID |
| `request_id` | TEXT NOT NULL | Correlation ID (UNIQUE index) |
| `skill_id` | TEXT NOT NULL | Skill executed (holds the skill's frontmatter name) |
| `agent_id` | TEXT NOT NULL | Default `'orchestrator'` |
| `status` | TEXT NOT NULL | Outcome status |
| `finish_reason` | TEXT | Loop finish reason |
| `error_message` | TEXT | Error if failed |
| `validation_failures` | TEXT | Output validation failures |
| `duration_ms` | INTEGER NOT NULL | Total execution time |
| `rounds_used` | INTEGER | LLM loop rounds |
| `tool_calls_made` | INTEGER | Total tool calls |
| `input_tokens` / `output_tokens` | INTEGER | Token usage (default 0) |
| `cost_usd` | REAL | Estimated API cost (default 0.0) |
| `model_used` | TEXT | Model that served the run |
| `query_preview` | TEXT | Truncated originating query |
| `route_score` | REAL | Skill router score |
| `was_auto_selected` | INTEGER | Router auto-selection flag |
| `repair_attempted` / `repair_succeeded` | INTEGER | Output-repair flags |
| `timestamp` | TEXT | Defaults to `datetime('now')` |
| `response_message_id` | INTEGER | The assistant message the run produced |

**Indexes:** unique `idx_sel_request_id (request_id)`,
`idx_sel_skill_ts (skill_id, timestamp DESC)`,
`idx_sel_status (skill_id, status)`, `idx_sel_agent (agent_id, skill_id)`,
`idx_sel_response_msg (response_message_id)`,
`idx_sel_timestamp (timestamp, skill_id)`.

### Event Flow

```text
SandboxManager.execute_tool()
         │ emit
SystemEvent::ToolExecuted { agent_id, tool_name, success, duration_ms,
                            task_id, session_id, tool_use_id }
         │ EventBus broadcast
Daemon event bridge (apps/openalpacad/src/event_bridge.rs → events/)
         ├──► ServerEvent::ToolExecuted (WebSocket broadcast)
         ├──► event_log table (audit)
         └──► tool_execution_log (SkillExecutionRepository::record_tool)
```

When the loop is writing a session log, the session writer later merges the
call's `log_seq` and previews onto that same `tool_execution_log` row.

Other tool-related events on the same path: `SecurityViolation`,
`CircuitBreakerTripped`, `ToolConfirmationRequested`,
`ToolConfirmationResolved`, `ExtensionStateChanged`,
`ExtensionCapabilityWithheld`, `ExtensionCapabilityWithdrawn`,
`ArtifactWritten`.

### Retention & Cleanup

`spawn_telemetry_cleanup(db, cancel)`
(`apps/openalpacad/src/background.rs`) runs daily (86,400 s interval),
deleting `skill_execution_log` rows older than **90 days** and
`tool_execution_log` rows older than **7 days**.  Cancellable via
`CancellationToken`.

`event_log` is **not** pruned by anything periodic.  Its rows go away only
with a factory reset or a project purge.

---

## 15. URL Validation

**Location:** `tools/url_validation.rs`

```rust
pub fn validate_url(url: &str) -> Result<(), String>
```

Applied to `web_fetch`, HTTP-backend tool URLs, **and every HTTP redirect**
followed by either client.

### Blocked Categories

| Category | Examples |
|----------|---------|
| Non-HTTP schemes | `file://`, `ftp://`, `gopher://` |
| Cloud metadata | `169.254.169.254`, `metadata.google.internal`, `metadata.internal` |
| Localhost | `localhost`, `127.0.0.0/8`, `[::1]`, `0.0.0.0` |
| IPv4 private | `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16` |
| IPv4 CGN | `100.64.0.0/10` |
| IPv4 link-local | `169.254.0.0/16` |
| IPv6 loopback | `::1` |
| IPv6 ULA | `fc00::/7` |
| IPv6 link-local | `fe80::/10` |
| IPv4-mapped IPv6 | `::ffff:10.0.0.1` (private/loopback/link-local embedded) |

### Helper Functions

```rust
fn is_ipv6_unique_local(ip: &Ipv6Addr) -> bool   // fc00::/7
fn is_ipv6_link_local(ip: &Ipv6Addr) -> bool     // fe80::/10
fn is_ipv4_mapped_private(ip: &Ipv6Addr) -> bool // ::ffff:x.x.x.x
```

---

## 16. Platform Helpers

**Location:** `tools/platform.rs`

```rust
pub fn shell_command(cmd: &str) -> tokio::process::Command
```

- **macOS/Linux:** `sh -c "<cmd>"`
- **Windows:** `cmd /c "<cmd>"`

Used by the `shell_execute` built-in and Command-backend tools.

---

## 17. Configuration Reference

### daemon.toml

Values shown are the shipped defaults.  A line starting with `#` is a key
the shipped `config/daemon.toml` leaves commented out (or omits); the value
beside it is the code default.

```toml
[execution.agent_defaults]
max_rounds = 15                # Max LLM loop iterations per agent
max_tools_per_round = 5        # Max tool calls executed per LLM round
max_tool_runtime_secs = 60     # Per-tool sandbox timeout
max_cost = 1                   # Max API cost per agent run / chat turn (USD)
confirmation_timeout_secs = 300

[execution.lead_agent_defaults]
batch_spawn_enabled = true     # Enables spawn_subagents_batch
max_concurrent_subagents = 6
max_rounds = 18
max_tools_per_round = 3
max_tool_runtime_secs = 300
max_cost = 5

[execution.skill_defaults]     # not in the shipped file; code defaults:
# max_rounds = 6
# max_tools_per_round = 3
# router_auto_select_threshold = 0.65
# router_suggest_threshold = 0.45

[execution.artifacts]
# max_artifact_bytes = 10485760        # artifact_write body cap (10 MB)
# max_versions_per_artifact = 20       # versions kept per artifact

[extensions]
# drain_timeout_secs = 10      # wait for in-flight calls on disable/reload/shutdown

[orchestrator.routing]
main_loop_max_rounds = 8
main_loop_max_tools_per_round = 4
tool_selection = "core_union"          # or "full"
steering_enabled = true                # gates steer_workflow / post_update / queue_followup

[orchestrator.sessions]
tool_result_inline_bytes = 32768       # above this a result spills (or is cut)
snapshot_max_bytes = 10485760          # largest file file_write will snapshot, and so overwrite

[security]
max_input_length = 32768
auto_approve_confirmations = false     # global confirmation bypass (dev use)

[security.circuit_breaker]
enabled = true
failure_threshold = 5
reset_timeout_secs = 300
```

There is no per-tool deny key.  A `daemon.toml` that still carries the
retired `execution.skill_defaults.global_tool_deny` gets a warning at boot
and the key is ignored; switch the owning MCP server or plugin off instead.

### Agent Templates (`config/agents/*.md`)

Markdown files with YAML frontmatter.  Tool access is declared via
`capabilities` / `denied_capabilities` (see
[Section 4](#4-capability-based-tool-resolution)); execution constraints
(`max_tool_calls`, `timeout_seconds`, `max_cost_per_task`, `max_rounds`,
`require_confirmation_for`) are also frontmatter fields.  On first boot an
installed daemon seeds this directory with the nine shipped templates.

### Custom Tool TOML

`config/tools/*.toml` — see
[Section 8](#8-custom-tools-toml-configuration).

### MCP Servers

`config/mcp.toml` — see [Section 9](#9-mcp-tools).

### Web Search

`web_search` requires a Brave Search API key configured as
`web_search.api_key` in `config/llm.toml` (hot-reloadable via `ArcSwap`).
The key is a plain string in that file; it is not encrypted the way
provider keys are.  `web_search.timeout_secs` defaults to 15.

---

## 18. Testing

### Test File Locations

Paths are under `crates/openalpaca_core/src/` unless they start with
`apps/` or `crates/`.

| Test File | Coverage |
|-----------|----------|
| `tools/registry/tests.rs` | Registration, removal, argument/enum validation, backend dispatch, capability index, the extension gate (hit, miss, stale generation) |
| `tools/registry/availability_tests.rs` | Skill requirements: total loss, partial loss, legacy `tools.allow` branch |
| `tools/registry/capabilities.rs` (inline) | Annotation capability derivation, validation, provider handles |
| `tools/extensions/tests.rs`, `scan_tests.rs` | Ledger transitions, generations, drain counting, the dependent scan |
| `tools/config/tests.rs` | TOML parsing, timeout validation, backend types, capabilities/annotations fields |
| `tools/config/annotations.rs` (inline) | Annotation config → MCP annotations conversion |
| `tools/builtins/tests.rs` | Built-in registration, definition completeness, workspace tools, `read_result` |
| `tools/builtins/artifact_write.rs`, `main_loop.rs`, `invoke_skill.rs`, `file_ops.rs` (inline) | Artifact writes and the workspace spill, the chat loop's tool set, `invoke_skill`, the pre-edit snapshot |
| `tools/builtins/helpers/tests.rs` | Path validation, backup generation, pruning |
| `tools/builtins/update_persona/tests.rs` | Persona update logic, backup creation |
| `tools/mcp/bridge.rs` / `config.rs` / `classify.rs` / `fingerprint.rs` (inline) | Namespacing, capability-per-name, result serialization, config parsing, server-name validation, failure classes, block fingerprints |
| `crates/openalpaca_core/tests/mcp_integration.rs` | MCP bridge end to end |
| `apps/openalpacad/src/managers/mcp/tests.rs` | MCP supervisor lifecycle |
| `tools/url_validation.rs` (inline) | SSRF validation: all blocked categories, public URLs |
| `tools/platform.rs` (inline) | Shell command creation |
| `security/sandbox/tests.rs` | Full sandbox flow, capability denial, confirmation (incl. unattended and timed-out), approval cache, timeout, `admit_tool_surface` (empty stays empty, deny wins) |
| `security/capabilities/tests.rs` | Deny/allow list logic, empty allow list |
| `security/sanitizer/tests.rs` | Path traversal, command injection, null bytes, uploads |
| `security/circuit_breaker/tests.rs` | State transitions, transient classification, pruning |
| `security/confirmation.rs` (inline) | Broker lifecycle, resolutions, approval cache, canonical args hashing |
| `runner/agentic_loop/tests.rs` | Tool call limiting, budget enforcement, result spill, loop behavior |
| `runner/lead_agent/tests.rs` | Coordination tool behavior, spawn guards, the lead's surface and allow list, a subagent's allow list at the spawn path (capability → tool name, deny wins, fail closed, plugin-backed) |

### Running Tests

```bash
# All tool-related tests
cargo test -p openalpaca_core -- tools::
cargo test -p openalpaca_core -- security::

# Specific module
cargo test -p openalpaca_core -- tools::registry
cargo test -p openalpaca_core -- tools::extensions
cargo test -p openalpaca_core -- security::circuit_breaker
cargo test -p openalpaca_core -- tools::url_validation

# MCP supervisor (daemon crate)
cargo test -p openalpacad -- managers::mcp
```
