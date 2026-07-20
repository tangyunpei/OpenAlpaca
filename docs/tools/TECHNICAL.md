# Tool System — Technical Reference

> **Status:** Living document · **Last updated:** 2026-07-19
>
> Code-level reference for the OpenAlpaca tool system.  For architecture
> and design rationale see the companion [DESIGN.md](./DESIGN.md).

---

## Table of Contents

1. [File Map](#1-file-map)
2. [Core Types](#2-core-types)
3. [ToolRegistry](#3-toolregistry)
4. [Capability-Based Tool Resolution](#4-capability-based-tool-resolution)
5. [Built-in Tools](#5-built-in-tools)
6. [Custom Tools (TOML Configuration)](#6-custom-tools-toml-configuration)
7. [MCP Tools](#7-mcp-tools)
8. [Plugin Tools](#8-plugin-tools)
9. [Security Layers](#9-security-layers)
10. [Agentic Loop Integration](#10-agentic-loop-integration)
11. [Tool Result Handling](#11-tool-result-handling)
12. [Lead Agent Coordination Tools](#12-lead-agent-coordination-tools)
13. [Telemetry & Storage](#13-telemetry--storage)
14. [URL Validation](#14-url-validation)
15. [Platform Helpers](#15-platform-helpers)
16. [Configuration Reference](#16-configuration-reference)
17. [Testing](#17-testing)

---

## 1. File Map

### Core Tool System

| File | Purpose |
|------|---------|
| `crates/openalpaca_core/src/tools/mod.rs` | Module root, `resolve_agent_tools()` |
| `crates/openalpaca_core/src/tools/registry/mod.rs` | `ToolRegistry`, `RegisteredTool`, `ToolBackend`, `BuiltInTool` trait, `ToolContext`, `PermissionTier` |
| `crates/openalpaca_core/src/tools/registry/capabilities.rs` | `CapabilityProvider`, `AnnotationCapabilityProvider`, `ProviderHandle`, annotation capability names |
| `crates/openalpaca_core/src/tools/registry/tests.rs` | Registry unit tests |
| `crates/openalpaca_core/src/tools/config/mod.rs` | TOML config parsing (`ToolConfigFile`, `ToolConfig`, `ToolBackendConfig`) |
| `crates/openalpaca_core/src/tools/config/annotations.rs` | `ToolAnnotationsConfig` (annotations block in user TOML) |
| `crates/openalpaca_core/src/tools/config/tests.rs` | Config parsing tests |
| `crates/openalpaca_core/src/tools/mcp/mod.rs` | MCP integration module root |
| `crates/openalpaca_core/src/tools/mcp/config.rs` | `McpConfig` — `config/mcp.toml` parsing |
| `crates/openalpaca_core/src/tools/mcp/bridge.rs` | `rmcp_tool_to_registered()`, `serialize_call_result()` |
| `crates/openalpaca_core/src/tools/mcp/client_set.rs` | `McpClientSet`, `McpServerStatus`, `McpServerSummary` |
| `crates/openalpaca_core/src/tools/stats.rs` | `ToolStats` — invocation stats over `tool_execution_log` |
| `crates/openalpaca_core/src/tools/url_validation.rs` | `validate_url()` SSRF protection |
| `crates/openalpaca_core/src/tools/platform.rs` | `shell_command()` platform abstraction |

### Built-in Tools

| File | Tool(s) |
|------|---------|
| `crates/openalpaca_core/src/tools/builtins/mod.rs` | Registration functions, `WorkspaceReadTool`/`WorkspaceWriteTool`, `ScriptToolBuiltIn`, built-in annotations |
| `crates/openalpaca_core/src/tools/builtins/web_search.rs` | `web_search` |
| `crates/openalpaca_core/src/tools/builtins/web_fetch.rs` | `web_fetch` |
| `crates/openalpaca_core/src/tools/builtins/file_ops.rs` | `file_read`, `file_write` |
| `crates/openalpaca_core/src/tools/builtins/shell_execute.rs` | `shell_execute` |
| `crates/openalpaca_core/src/tools/builtins/memory_search.rs` | `memory_search` |
| `crates/openalpaca_core/src/tools/builtins/update_persona/mod.rs` | `update_persona` (dispatcher) |
| `crates/openalpaca_core/src/tools/builtins/update_persona/soul.rs` | SOUL.md update logic |
| `crates/openalpaca_core/src/tools/builtins/update_persona/user.rs` | USER.md update logic |
| `crates/openalpaca_core/src/tools/builtins/update_persona/identity.rs` | IDENTITY.md update logic |
| `crates/openalpaca_core/src/tools/builtins/update_persona/common.rs` | Shared persona update helpers |
| `crates/openalpaca_core/src/tools/builtins/update_persona/tests.rs` | Persona update tests |
| `crates/openalpaca_core/src/tools/builtins/send.rs` | `send` (connector message/file delivery) |
| `crates/openalpaca_core/src/tools/builtins/helpers/mod.rs` | Workspace path validation, backup management |
| `crates/openalpaca_core/src/tools/builtins/helpers/tests.rs` | Helper function tests |
| `crates/openalpaca_core/src/tools/builtins/tests.rs` | Built-in tool integration tests |

### Security

| File | Purpose |
|------|---------|
| `crates/openalpaca_core/src/security/mod.rs` | Module root, re-exports |
| `crates/openalpaca_core/src/security/sandbox/mod.rs` | `SandboxManager`, `SandboxPolicy`, `effective_confirmation_set()` |
| `crates/openalpaca_core/src/security/sandbox/tests.rs` | Sandbox tests |
| `crates/openalpaca_core/src/security/capabilities/mod.rs` | `CapabilityManager`, `SecurityViolation` |
| `crates/openalpaca_core/src/security/capabilities/tests.rs` | Capability tests |
| `crates/openalpaca_core/src/security/sanitizer/mod.rs` | `InputSanitizer` |
| `crates/openalpaca_core/src/security/sanitizer/tests.rs` | Sanitizer tests |
| `crates/openalpaca_core/src/security/circuit_breaker/mod.rs` | `ToolCircuitBreaker`, `is_transient_tool_error()` |
| `crates/openalpaca_core/src/security/circuit_breaker/tests.rs` | Circuit breaker tests |
| `crates/openalpaca_core/src/security/gate.rs` | `SecurityGate` facade |
| `crates/openalpaca_core/src/security/policy.rs` | `Principal`, `Scope`, `TrustGate` |
| `crates/openalpaca_core/src/security/confirmation.rs` | `ConfirmationBroker`, `ApprovalCache`, `ApprovalScope`, `hash_canonical_args()` |

### Agentic Loop

| File | Purpose |
|------|---------|
| `crates/openalpaca_core/src/runner/agentic_loop/mod.rs` | `run_agentic_loop()`, `run_agentic_loop_routed()` |
| `crates/openalpaca_core/src/runner/agentic_loop/tool_helpers.rs` | `truncate_tool_result()`, `format_tool_error()`, `format_tool_error_with_hint()` |
| `crates/openalpaca_core/src/runner/agentic_loop/backend.rs` | LLM backend abstraction |
| `crates/openalpaca_core/src/runner/agentic_loop/config.rs` | `LoopConfig` |
| `crates/openalpaca_core/src/runner/agentic_loop/context.rs` | Loop state management |
| `crates/openalpaca_core/src/runner/agentic_loop/cost.rs` | `LoopCostAccumulator` |
| `crates/openalpaca_core/src/runner/agentic_loop/tests.rs` | Loop tests |

### Lead Agent & DAG

| File | Purpose |
|------|---------|
| `crates/openalpaca_core/src/runner/lead_agent/mod.rs` | Lead agent orchestration, per-request registry construction |
| `crates/openalpaca_core/src/runner/lead_agent/tools.rs` | Coordination tools + `register_coordination_tools()` |
| `crates/openalpaca_core/src/runner/lead_agent/tracker.rs` | `SubagentTracker` |
| `crates/openalpaca_core/src/runner/lead_agent/guard.rs` | `AgentBusyGuard` claim/release |
| `crates/openalpaca_core/src/runner/lead_agent/prompt.rs` | Lead agent prompt assembly |
| `crates/openalpaca_core/src/runner/lead_agent/tests.rs` | Lead agent tests |
| `crates/openalpaca_core/src/runner/dag_executor/mod.rs` | DAG node execution |
| `crates/openalpaca_core/src/runner/dag_executor/node_runner.rs` | Per-node tool resolution |
| `crates/openalpaca_core/src/runner/dag_executor/progress.rs` | DAG progress tracking |
| `crates/openalpaca_core/src/runner/dag_executor/tests.rs` | DAG tests |

### LLM Types

| File | Purpose |
|------|---------|
| `crates/openalpaca_llm/src/types.rs` | `ToolDefinition`, `ToolCall`, `ToolChoice`, `ChatResponse` |

### MCP Client SDK

| File | Purpose |
|------|---------|
| `crates/openalpaca_mcp/` | `McpClient` wrapper around the `rmcp` SDK (stdio + streamable-HTTP transports, reconnect/retry). Re-exports `Tool`, `ToolAnnotations`, `CallToolResult`, etc. |

### Daemon Wiring

| File | Purpose |
|------|---------|
| `apps/openalpacad/src/services/tools.rs` | `build_tool_registry()` — startup registration (built-ins + custom TOML) |
| `apps/openalpacad/src/services/mcp.rs` | `register_mcp_servers()` — MCP bootstrap from `config/mcp.toml` |
| `apps/openalpacad/src/background.rs` | `spawn_telemetry_cleanup()` |
| `apps/openalpacad/src/event_bridge.rs` + `events/` | `ToolExecuted` event → WebSocket broadcast + persistence |

### Configuration

| File | Purpose |
|------|---------|
| `config/tools/example.toml` | Example custom tool configuration (commented out) |
| `config/mcp.toml` | MCP server declarations (commented examples by default) |

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

**Location:** `crates/openalpaca_core/src/tools/registry/mod.rs`

The identity-carrying spine of tool execution.  Lightweight — plain strings,
no `Arc` dependencies or DB handles.  Built by the caller (query handler,
DAG node runner, lead agent, skill handler) and threaded through
`SandboxManager` into `BuiltInTool::execute_with_context()`.

```rust
#[derive(Debug, Clone, Default)]
pub struct ToolContext {
    pub agent_id: Option<String>,
    pub task_id: Option<String>,
    pub owner_id: Option<String>,
    pub workspace_id: Option<String>,
    /// Skill invocation chain, oldest first. Empty at top level.
    pub skill_stack: Vec<String>,
    /// Effective tool constraints inherited from parent skill chain.
    /// None at top level; Some inside a nested skill invocation.
    pub effective_constraints: Option<EffectiveToolSet>,
}
```

`ToolContext::with_skill_pushed(skill_id)` clones the context and appends a
skill ID to `skill_stack` for nested skill invocations.

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
    /// Set for MCP tools and most built-ins; None for custom tools
    /// unless declared in TOML.
    pub annotations: Option<openalpaca_mcp::ToolAnnotations>,
    /// Version string. From MCP server info for MCP tools, the crate
    /// version for built-ins, "0.0.0" default for TOML tools.
    pub version: String,
    /// Provenance: "builtin", "user", "mcp:<server>", or "plugin:<id>".
    pub author: String,
    /// Registration timestamp (UTC).
    pub created_at: chrono::DateTime<chrono::Utc>,
}
```

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

Identity-dependent tools (`memory_search`, `update_persona`,
`workspace_read`, `workspace_write`, `send`) override
`execute_with_context()`; context-free tools implement only `execute()`.

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

**Location:** `crates/openalpaca_core/src/tools/registry/mod.rs`

### Structure

```rust
pub struct ToolRegistry {
    tools: DashMap<String, RegisteredTool>,
    capability_index: DashMap<String, Vec<String>>, // capability → tool names
    http_client: reqwest::Client,                    // shared, SSRF-checked redirects
    capability_providers: DashMap<ProviderHandle, Arc<dyn CapabilityProvider>>,
    next_provider_handle: AtomicU64,
    provider_mutex: std::sync::Mutex<()>,            // serializes index rebuilds
}
```

Backed by `DashMap` for lock-free concurrent reads and writes.  Shared as
`Arc<ToolRegistry>` — tools can be **registered and removed at runtime**
(e.g. when plugins load/unload or MCP servers connect) without `&mut self`.

`ToolRegistry` implements `Clone`, but the clone is a non-atomic snapshot;
concurrent register/remove during clone can produce an incomplete copy.
Use `Arc::clone()` for shared access.  The lead agent uses the value-clone
deliberately to build a per-request registry (see [Section 12](#12-lead-agent-coordination-tools)).

`ToolRegistry::new()` returns `Result<Self, String>` because it builds the
HTTP client with a **custom redirect policy**: every redirect target is
re-validated with `validate_url()` (SSRF) and redirects are capped at 10.
`new()` also installs the default `AnnotationCapabilityProvider`.

### Key Methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `new()` | `() -> Result<Self, String>` | Create registry with SSRF-checked HTTP client |
| `register()` | `(&self, RegisteredTool) -> Result<(), String>` | Add/replace a tool at any time. Rejects empty names, names > 256 chars, or names containing null bytes. Updates the capability index (string + virtual capabilities). |
| `remove()` | `(&self, name: &str) -> bool` | Remove a tool; scrubs its capability index entries |
| `get()` | `(&self, name: &str) -> Option<RegisteredTool>` | Look up by name (returns a clone — DashMap guards must not cross `.await`) |
| `execute()` | `async (&self, name, args) -> Result<String, String>` | Validate args + dispatch to backend (no context) |
| `execute_with_context()` | `async (&self, name, args, &ToolContext) -> Result<String, String>` | Same, but routes BuiltIn backends through `execute_with_context()`. Context is discarded for Http/Command/Plugin backends. |
| `registered_tool_names()` | `(&self) -> Vec<String>` | All tool names |
| `iter_registered_tools()` | `(&self) -> impl Iterator<Item = (String, RegisteredTool)>` | Snapshot iteration (per-entry clones) |
| `count()` | `(&self) -> usize` | Number of registered tools |
| `is_exempt_from_timeout()` | `(&self, name: &str) -> bool` | Whether the sandbox should skip the per-tool timeout |
| `tools_for_capabilities()` | `(&self, &[String]) -> Vec<ToolDefinition>` | Capability-intersection lookup via the inverted index. Empty input → empty output. |
| `tools_for_capabilities_with_deny()` | `(&self, caps, denied) -> Vec<ToolDefinition>` | Same, excluding tools that provide any denied capability (string or virtual) |
| `command_backend_tool_names()` | `(&self) -> Vec<String>` | Command-backend tools (treated as shell-like by the sanitizer) |
| `register_capability_provider()` | `(&self, Arc<dyn CapabilityProvider>) -> ProviderHandle` | Add a virtual-capability provider; triggers a full index rebuild |
| `remove_capability_provider()` | `(&self, ProviderHandle) -> bool` | Remove a provider; rebuilds the index |
| `provider_handles()` | `(&self) -> Vec<ProviderHandle>` | Active provider handles |
| `known_virtual_capabilities()` | `(&self) -> Vec<String>` | Union of all providers' capability names (used by config validation) |

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

---

## 4. Capability-Based Tool Resolution

Agents do not list tools — they declare **capabilities**, and the registry
resolves capabilities to tools at dispatch time.

### Capability strings

Every `RegisteredTool` carries `provides_capabilities: Vec<String>`
(e.g. `file_read`, `web_access`, `messaging`, `orchestration`).  The
registry maintains an inverted `capability_index` (capability → tool
names), kept up to date on every register/remove.

### resolving an agent's tools

**Location:** `crates/openalpaca_core/src/tools/mod.rs`

```rust
pub fn resolve_agent_tools(
    agent: &SubAgent,
    tool_registry: &Arc<ToolRegistry>,
) -> Vec<ToolDefinition>
```

A tool is included if **any** of its capabilities matches any of the
agent's `capabilities`, and **none** of its capabilities (string or
virtual) appear in the agent's `denied_capabilities`.  Agents with an
empty capability list get no tools.

Agent templates declare capabilities in their YAML frontmatter
(`config/agents/*.md`):

```yaml
---
id: "code_agent"
capabilities:
  - "file_read"
  - "file_write"
  - "shell_execute"
  - "memory_read"
  - "workspace_read"
  - "workspace_write"
denied_capabilities:
  - "web_access"
max_tool_calls: 50
timeout_seconds: 600
---
```

### Virtual capabilities (annotation:*)

**Location:** `crates/openalpaca_core/src/tools/registry/capabilities.rs`

`CapabilityProvider` is an extension point that derives additional
("virtual") capability strings from a `RegisteredTool`:

```rust
pub trait CapabilityProvider: Send + Sync {
    fn derive_capabilities(&self, tool: &RegisteredTool) -> Vec<String>;
    fn known_capability_names(&self) -> Vec<String>;
}
```

The default `AnnotationCapabilityProvider` (installed by
`ToolRegistry::new()`) maps MCP annotation hints to 8 capability names:

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
exclude every destructive-flagged tool, whatever its source.

Registering or removing a provider triggers a full rebuild of the
capability index (serialized by `provider_mutex`; readers may observe a
sub-millisecond transient partial state).  `ProviderHandle` values are
process-unique and do not survive restarts.

---

## 5. Built-in Tools

### 5.1 Tool Inventory

All of these are registered in the global `ToolRegistry` at daemon startup.

| Tool | Parameters | Capability | Key Constraints |
|------|-----------|------------|-----------------|
| `web_search` | `query` (required), `count` (default 5, max 20) | `web_access` | Brave Search API; requires `web_search.api_key` in `llm.toml` |
| `web_fetch` | `url` (required) | `web_access` | SSRF-protected; response cap 1 MB |
| `file_read` | `path` (required) | `file_read` | Workspace-scoped (relative paths only); max 10 MB |
| `file_write` | `path`, `content` (required) | `file_write` | Workspace-scoped; max 10 MB; blocks SOUL.md/USER.md/IDENTITY.md (use `update_persona`) |
| `shell_execute` | `command` (required) | `shell_execute` | 300s internal safety timeout; output cap 512 KB; injection patterns blocked by sanitizer |
| `memory_search` | `query` (required), `limit` | `memory_read` | Registered only when DB + daemon config provided; hybrid FTS5 + sqlite-vec when an embedder is present; cascading workspace → global scope; owner from `ToolContext` |
| `workspace_read` | `key` (optional) | `workspace_read` | Reads shared task workspace; requires `ToolContext.task_id` |
| `workspace_write` | `key`, `content` (required), `entry_type`, `file_asset_id` | `workspace_write` | 32 KB content cap; optimistic locking with up to 5 jittered-backoff retries; `file_asset_id` enables file delivery to channels |
| `update_persona` | `target` (`soul`/`user`/`identity`), `mode` (`replace`/`sections`), `content_b64`, `sections` | `persona_write` | Edits persona docs with validation + timestamped backups |
| `send` | `action` (`message`/`file`), `channel`, `recipient` (required); `content` / `file_path` + `filename` | `messaging` | Registered only when a `ConnectorSendLock` is supplied; delivers via connector (e.g. Telegram) |
| `skill_script:<name>` | per-script | per skill config | Skill-bundled scripts wrapped as `ScriptToolBuiltIn`; registered when a skill declares scripts |

Built-in tools also carry MCP-style annotations
(`annotations_for_builtin()` in `builtins/mod.rs`):

- read-only, closed-world: `file_read`, `workspace_read`, `memory_search`
- read-only, open-world: `web_fetch`, `web_search`
- destructive, closed-world: `file_write`, `workspace_write`, `update_persona`
- destructive, open-world: `shell_execute`, `send`

The destructive annotations feed the default confirmation set
([Section 9.1](#91-sandboxmanager)) and the `annotation:*` virtual
capabilities.

### 5.2 Registration Functions

**Location:** `crates/openalpaca_core/src/tools/builtins/mod.rs`

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
//          workspace_read, workspace_write (always)
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
daemon captures the workspace root once at startup.

### 5.3 Skill Scripts (`ScriptToolBuiltIn`)

Skills may bundle executable scripts under `<skill>/scripts/`.  Each is
registered as a `BuiltInTool` named `skill_script:<name>`:

- `ScriptToolBuiltIn::new()` canonicalizes the script path and rejects
  anything resolving outside the skill's `scripts/` directory (path
  traversal blocked).
- Arguments are converted to `--key=value` CLI flags via
  `json_to_cli_args()`.
- Executed with the configured interpreter (or directly), a per-script
  timeout, working directory = the skill directory, and a 512 KB output
  cap.

### 5.4 Workspace Path Helpers

**Location:** `crates/openalpaca_core/src/tools/builtins/helpers/mod.rs`

| Function | Purpose |
|----------|---------|
| `validate_workspace_path(path)` | Reject absolute paths and `..` components |
| `resolve_workspace_path(rel, root)` | Canonicalize + verify stays within workspace |
| `resolve_workspace_path_for_write(rel, root)` | Like above but for new files (parent may not exist) |
| `is_soul_path(path)` / `is_user_path(path)` / `is_identity_path(path)` | Case-insensitive protected-file checks |
| `unique_backup_path(dir, prefix)` | Generate timestamped backup path with unique suffix |
| `prune_backups(dir, max, prefix)` | Remove oldest backups exceeding retention limit |

### 5.5 Size Limits

| Constant | Value | Location |
|----------|-------|----------|
| `MAX_FILE_READ_SIZE` | 10 MB | `builtins/helpers/mod.rs` |
| `MAX_FILE_WRITE_SIZE` | 10 MB | `builtins/file_ops.rs` |
| `MAX_WORKSPACE_CONTENT_SIZE` | 32 KB | `builtins/mod.rs` |
| `MAX_SCRIPT_OUTPUT_BYTES` | 512 KB | `builtins/mod.rs` |
| `MAX_TOOL_RESULT_SIZE` (LLM-facing truncation) | 32 KB | `agentic_loop/tool_helpers.rs` |
| HTTP backend response cap | 1 MB (stream), 8 KB returned | `registry/mod.rs` |
| Command backend output cap | 512 KB per stream | `registry/mod.rs` |
| `shell_execute` output cap | 512 KB per stream | `builtins/shell_execute.rs` |

---

## 6. Custom Tools (TOML Configuration)

**Location:** `crates/openalpaca_core/src/tools/config/mod.rs`

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
8 known annotation capability names.

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
   after built-in registration — includes `web_search`, `web_fetch`,
   `file_read`, `file_write`, `shell_execute`, `memory_search`,
   `workspace_read`, `workspace_write`, `update_persona`, and `send` when
   registered), or
2. the runtime-injected lead-agent tool names: `spawn_subagent`,
   `spawn_subagents_batch`, `check_subagent_status`, `wait_for_subagents`.

Tools that fail registry name validation are also skipped with a warning.

---

## 7. MCP Tools

OpenAlpaca connects **out** to external MCP servers and imports their
tools.  It does not expose its own tools over MCP.

### Configuration — `config/mcp.toml`

**Parser:** `crates/openalpaca_core/src/tools/mcp/config.rs`.  A missing
file simply means no MCP servers.  The shipped `config/mcp.toml` contains
only commented-out examples.

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
# env = { ... }  cwd = "..."  enabled = true (default)
# connect_timeout_secs / request_timeout_secs override defaults

[servers.remote]
transport = "http"              # MCP streamable-HTTP
url = "https://example.com/mcp"
auth = { bearer_env = "MY_TOKEN" }
# auth variants: { bearer = "..." }
#                { api_key_header = "X-API-Key", api_key_env = "ENV_VAR" }
# extra_headers = { ... }
```

Env-var-based auth values are resolved at boot; a missing variable fails
that server only.

### Bootstrap flow

**Location:** `apps/openalpacad/src/services/mcp.rs`

At daemon startup, `register_mcp_servers()`:

1. Loads `config/mcp.toml` (missing file → empty set; parse errors are fatal).
2. For each enabled server: builds an `McpClientConfig`, connects with the
   per-server (or default) connect timeout, calls `list_tools`, and
   registers every discovered tool via
   `bridge::rmcp_tool_to_registered()`.
3. Per-server failures (invalid config, connect failure/timeout,
   `list_tools` error) are logged and recorded in the summary — **never
   fatal** to daemon startup.
4. Returns an `McpClientSet`: a daemon-lifetime holder of client `Arc`s
   plus per-server `McpServerSummary` records
   (`status: Connected { server_version, protocol_version } | Failed { reason } | Disabled`,
   `discovered_tools`).

### Tool bridging

**Location:** `crates/openalpaca_core/src/tools/mcp/bridge.rs`

- Registered name is **namespaced**: `<server_name>__<remote_name>`
  (e.g. `fs__read_file`) so MCP tools cannot collide with built-ins or
  other servers.
- `author = "mcp:<server>"`; `version` comes from the server's reported
  version (or `"unknown"`); `annotations` are taken from the server's tool
  metadata; `provides_capabilities` starts empty (annotation-derived
  virtual capabilities still apply).
- `serialize_call_result()` flattens a `CallToolResult` into
  `Result<String, String>`: text blocks are concatenated with newlines;
  `is_error = true` becomes a tool error.

### Limitations (current state)

- **Non-text content is dropped**: image/audio/resource blocks in tool
  results are replaced with a bracketed placeholder — they are not
  surfaced to the model.
- **MCP resources and prompts are not implemented**: the client's
  `list_resources`, `read_resource`, `list_prompts`, and `get_prompt`
  return "not implemented" errors.  Only tools work.
- Retriable transport errors trigger the client's internal
  reconnect/retry loop (`max_reconnect_attempts`, exponential backoff).

---

## 8. Plugin Tools

`ToolBackend::Plugin` wraps an
`openalpaca_api::plugin_traits::PluginToolExecutor`.  The out-of-process
plugin system (`crates/openalpaca_plugins`) registers plugin-provided
tools into the shared registry at plugin load under the namespaced name
`<plugin>::<tool>` with `author = "plugin:<name>"`, and removes them on
unload — this is why the registry supports runtime register/remove.

Plugins can also declare virtual capabilities in their manifest, which
are attached via a registered `CapabilityProvider`.

Plugin **tools, skills, and agents** flow through the normal registries.
Plugin *connector* and *LLM-provider* bridges exist in the plugin crate
but are not yet wired into the daemon's connector manager or LLM router —
treat those plugin types as not yet functional.

`ToolContext` is not forwarded to plugin backends; plugin tools receive
only the tool name and arguments.

---

## 9. Security Layers

### 9.1 SandboxManager

**Location:** `crates/openalpaca_core/src/security/sandbox/mod.rs`

```rust
pub struct SandboxManager {
    registry: Arc<ToolRegistry>,
    bus: EventBus,
    circuit_breaker: ToolCircuitBreaker,
    db: Option<Database>,                              // audit logging
    confirmation_broker: Option<Arc<ConfirmationBroker>>,
    approval_cache: ApprovalCache,                     // session-scoped
}
```

Constructors: `new(registry, bus, &CircuitBreakerConfig)`,
`with_db(...)`, `with_defaults(registry, bus)`.  The broker is attached
post-construction via `set_confirmation_broker()`.

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

1. `CapabilityManager::check_agent_capability()` — deny/allow lists
2. `InputSanitizer::sanitize_tool_args()` — injection/traversal checks
   (allowlist = registered tool names; shell-like = command-backend tools)
3. **Confirmation gate** — the *effective confirmation set* is
   `policy.require_confirmation_for` if non-empty, otherwise all
   registered tools with `destructive_hint = Some(true)`.  For a tool in
   the set:
   - **ApprovalCache hit** (prior user approval this session for these
     args or the whole tool) → proceed without prompting
   - `policy.auto_approve` → proceed; decision persisted to `event_log`
     as `tool_auto_approved`
   - Broker present → publish `SystemEvent::ToolConfirmationRequested`,
     await the user's `ConfirmationResponse` (timeout
     `confirmation_timeout_secs`, default 300s).  On approval, the
     decision is cached with its `ApprovalScope` (defaults to
     `TheseArgs`).
   - No broker → **fail-closed**: blocked immediately
4. `circuit_breaker.check()` — per (agent, tool) consecutive-failure state
5. Timeout-wrapped execution of
   `registry.execute_with_context(name, args, ctx)` — skipped for tools
   whose `RegisteredTool.exempt_from_timeout` is true (checked via
   `registry.is_exempt_from_timeout()`)
6. `circuit_breaker.record_success/record_failure()` — failures are only
   recorded when `is_transient_tool_error()` classifies them as transient
7. Emit `SystemEvent::ToolExecuted` or `SystemEvent::SecurityViolation`
   (violations are also persisted to `event_log` when a DB is attached)

### 9.2 SandboxPolicy

```rust
pub struct SandboxPolicy {
    pub agent_id: String,
    pub allowed_capabilities: Vec<String>,
    pub denied_capabilities: Vec<String>,
    pub require_confirmation_for: Vec<String>,
    pub max_tool_calls: Option<u32>,
    pub max_tool_runtime_secs: u64,
    pub stream_id: Option<String>,        // SSE confirmation routing
    pub lane_key: Option<String>,         // connector confirmation routing
    pub confirmation_timeout_secs: Option<u64>,  // default 300
    pub auto_approve: bool,
}
```

`SandboxPolicy::from_constraints(agent_id, &AgentConstraints)` builds a
policy from an agent template's constraints
(`max_tool_runtime_secs` defaults to 60 when the template sets no
timeout).  The daemon's `security.auto_approve_confirmations` config flag
forces `auto_approve = true` globally.

### 9.3 CapabilityManager

**Location:** `crates/openalpaca_core/src/security/capabilities/mod.rs`

```rust
pub fn check_agent_capability(
    agent_id: &str,
    tool_name: &str,
    constraints: &AgentConstraints,
) -> Result<(), SecurityViolation>
```

**Rules** (matching is **case-insensitive** — the tool name is lowercased
and compared to pre-normalized constraint entries):

1. If `denied_capabilities` contains the tool name → **DENIED**
2. If `allowed_capabilities` is non-empty AND does not contain the tool name → **DENIED**
3. Otherwise → **ALLOWED**

`check_model_access()` applies the same deny/allow pattern to model IDs.

### 9.4 InputSanitizer

**Location:** `crates/openalpaca_core/src/security/sanitizer/mod.rs`

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

### 9.5 ToolCircuitBreaker

**Location:** `crates/openalpaca_core/src/security/circuit_breaker/mod.rs`

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

### 9.6 ConfirmationBroker & ApprovalCache

**Location:** `crates/openalpaca_core/src/security/confirmation.rs`

```rust
pub struct ConfirmationBroker {
    pending: DashMap<String, oneshot::Sender<ConfirmationResponse>>,
}
```

| Method | Description |
|--------|-------------|
| `request(&ConfirmationRequest) -> oneshot::Receiver<ConfirmationResponse>` | Register pending confirmation |
| `respond(id, ConfirmationResponse) -> Result<(), String>` | Deliver user's decision |
| `cancel(id)` | Clean up on timeout |
| `pending_count()` / `pending_keys()` | Diagnostics |

**ConfirmationRequest fields:** `request_id`, `agent_id`, `tool_name`,
`tool_arguments`, `stream_id`, `lane_key`, `timestamp`.

**ConfirmationResponse:** `{ approved: bool, approval_scope: Option<ApprovalScope> }`
with `ApprovalScope::TheseArgs | EntireTool` (`snake_case` on the wire;
missing scope defaults to `TheseArgs` at enforcement time).

**ApprovalCache** — session-scoped, lock-free (`DashSet` behind `Arc`),
cleared on daemon restart.  Keys are the tool name (EntireTool) or
`(tool_name, args_hash)` (TheseArgs), where `args_hash` comes from
`hash_canonical_args()` — a 64-bit hash over JSON with recursively sorted
object keys, so argument key order does not defeat the cache (array order
still matters).

### 9.7 TrustGate (Principal-Level)

**Location:** `crates/openalpaca_core/src/security/policy.rs`

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

## 10. Agentic Loop Integration

**Location:** `crates/openalpaca_core/src/runner/agentic_loop/mod.rs`

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

### Tool Execution Phase (per round)

When the LLM response contains tool calls:

1. Append the assistant message (with tool calls) to the conversation.
2. Compute the remaining budget:
   `policy.max_tool_calls - state.tool_calls_made` (if a policy limit is
   set).
3. Apply the per-round cap: at most `config.max_tools_per_round` calls
   execute this round.
4. Partition into *executable* and *over-limit* calls.  Over-budget calls
   get `format_tool_error("max_tool_calls limit reached ...")`; overflow
   beyond the per-round cap gets an error result as well — every tool
   call ID receives a tool-result message.
5. Executable calls run **in parallel** via
   `futures_util::future::join_all()` (each future calls
   `SandboxManager::execute_tool(tool_call, policy, ctx)`), raced against
   the cancellation token.
6. Per result: success → `truncate_tool_result()` (32 KB);
   error → `format_tool_error_with_hint()`.
7. Push `ChatMessage::tool_result(id, text)` per call, bump
   `state.tool_calls_made`, continue to the next round (until
   `max_rounds`, `EndTurn`, cost limit, cancellation, or error).

---

## 11. Tool Result Handling

**Location:** `crates/openalpaca_core/src/runner/agentic_loop/tool_helpers.rs`

### Result Truncation

```rust
const MAX_TOOL_RESULT_SIZE: usize = 32 * 1024;  // 32 KB

pub fn truncate_tool_result(text: String) -> String
```

**Smart boundary detection** (priority order): sentence boundary
(`. `/`.\n`/`! `/`? ` etc.) → line boundary → word boundary → char
boundary, but a boundary is only used if it keeps at least 75% of the
limit (avoids discarding most of the content for a distant sentence end).
Appends `[... truncated: showing first X of Y bytes]`.

### Error Formatting

```rust
pub fn format_tool_error(msg: &str) -> String
// Returns: "[tool_error] {msg}"

pub fn format_tool_error_with_hint(tool_name: &str, msg: &str) -> String
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

## 12. Lead Agent Coordination Tools

**Location:** `crates/openalpaca_core/src/runner/lead_agent/tools.rs`

These tools are **not in the global registry**.  For each lead-agent run,
`run_lead_agent` value-clones the shared registry
(`(*tool_registry).clone()`) and calls `register_coordination_tools()` to
add them to that per-request registry (capability `orchestration`), which
then backs the run's `SandboxManager`.  This is also why custom TOML
tools may not use these four names.

### spawn_subagent

Spawns a background subagent from an agent template.

**Parameters:** `agent_id` (template ID from the catalog embedded in the
tool description), `objective` — both required.

**Guards:**
- Self-spawn prevention (a lead cannot spawn its own template)
- Depth limit: `MAX_SUBAGENT_DEPTH` (3)
- Concurrency semaphore sized by `max_concurrent_subagents`

**Returns:** a `run_id` immediately (non-blocking; execution is queued if
LLM capacity is limited).

### spawn_subagents_batch

Spawns 1–8 subagents in a single call (`subagents` array; > 8 items is an
error).  Only registered when
`execution.lead_agent_defaults.batch_spawn_enabled` is true (shipped
default: `false`).

### check_subagent_status

Polls one or all spawned subagents.  Registered with
`exempt_from_timeout: true`.

### wait_for_subagents

Blocks until all spawned subagents complete.  Registered with
`exempt_from_timeout: true` — it manages its own waiting and must not be
killed by the per-tool sandbox timeout.

---

## 13. Telemetry & Storage

### Database Tables

**Migration:** `crates/openalpaca_storage/src/migrations/030_skill_tool_execution_log.sql`

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

**Indexes:** `idx_tel_tool_ts (tool_name, timestamp DESC)`,
`idx_tel_request (request_id)`.

#### skill_execution_log

| Column | Type | Description |
|--------|------|-------------|
| `id` | INTEGER PK | Auto-increment row ID |
| `request_id` | TEXT NOT NULL | Correlation ID (UNIQUE index) |
| `skill_id` | TEXT NOT NULL | Skill executed |
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

**Indexes:** unique `idx_sel_request_id (request_id)`,
`idx_sel_skill_ts (skill_id, timestamp DESC)`,
`idx_sel_status (skill_id, status)`, `idx_sel_agent (agent_id, skill_id)`.

### Event Flow

```
SandboxManager.execute_tool()
         │ emit
SystemEvent::ToolExecuted { agent_id, tool_name, success, duration_ms }
         │ EventBus broadcast
Daemon event bridge (apps/openalpacad/src/event_bridge.rs → events/)
         ├──► ServerEvent::ToolExecuted (WebSocket broadcast)
         ├──► event_log table (audit)
         └──► tool_execution_log (SkillExecutionRepository::record_tool)
```

### ToolStats Queries

**Location:** `crates/openalpaca_core/src/tools/stats.rs`

```rust
pub struct ToolStats {
    pub last_invoked_at: Option<DateTime<Utc>>,
    pub invocation_count: u64,
    pub error_count: u64,
}

ToolStats::for_tool(&db, "tool_name")   // zeros if never invoked
ToolStats::for_all_tools(&db)           // HashMap keyed by tool name
```

Thin wrappers over `SkillExecutionRepository::tool_stats[_all]`, which
aggregate `tool_execution_log`.

### Retention & Cleanup

`spawn_telemetry_cleanup(db, cancel)`
(`apps/openalpacad/src/background.rs`) runs daily (86,400 s interval),
deleting `skill_execution_log` rows older than **90 days** and
`tool_execution_log` rows older than **7 days**.  Cancellable via
`CancellationToken`.

---

## 14. URL Validation

**Location:** `crates/openalpaca_core/src/tools/url_validation.rs`

```rust
pub fn validate_url(url: &str) -> Result<(), String>
```

Applied to `web_fetch`, HTTP-backend tool URLs, **and every HTTP redirect**
followed by the registry's shared client.

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

## 15. Platform Helpers

**Location:** `crates/openalpaca_core/src/tools/platform.rs`

```rust
pub fn shell_command(cmd: &str) -> tokio::process::Command
```

- **macOS/Linux:** `sh -c "<cmd>"`
- **Windows:** `cmd /c "<cmd>"`

Used by the `shell_execute` built-in and Command-backend tools.

---

## 16. Configuration Reference

### daemon.toml

```toml
[execution.agent_defaults]
max_rounds = 15                # Max LLM loop iterations per agent
max_tools_per_round = 5        # Max tool calls executed per LLM round
max_tool_runtime_secs = 60     # Per-tool sandbox timeout
max_cost = 1                   # Max API cost per agent run (USD)
confirmation_timeout_secs = 300

[execution.lead_agent_defaults]
batch_spawn_enabled = false    # Enables spawn_subagents_batch
max_concurrent_subagents = 6
max_rounds = 18
max_tools_per_round = 3
max_tool_runtime_secs = 300
max_cost = 5

[security]
# max_input_length = 32768
# auto_approve_confirmations = false   # global confirmation bypass (dev use)

# Circuit breaker lives under [security.circuit_breaker]; the shipped
# daemon.toml omits it, so code defaults apply:
#   enabled = true, failure_threshold = 5, reset_timeout_secs = 300
# [security.circuit_breaker]
# enabled = true
# failure_threshold = 5
# reset_timeout_secs = 300
```

### Agent Templates (`config/agents/*.md`)

Markdown files with YAML frontmatter.  Tool access is declared via
`capabilities` / `denied_capabilities` (see
[Section 4](#4-capability-based-tool-resolution)); execution constraints
(`max_tool_calls`, `timeout_seconds`, `max_cost_per_task`,
`require_confirmation_for`) are also frontmatter fields.

### Custom Tool TOML

`config/tools/*.toml` — see
[Section 6](#6-custom-tools-toml-configuration).

### MCP Servers

`config/mcp.toml` — see [Section 7](#7-mcp-tools).

### Web Search

`web_search` requires a Brave Search API key configured as
`web_search.api_key` in `config/llm.toml` (hot-reloadable via `ArcSwap`).

---

## 17. Testing

### Test File Locations

| Test File | Coverage |
|-----------|----------|
| `tools/registry/tests.rs` | Registration, removal, argument/enum validation, backend dispatch, capability index |
| `tools/registry/capabilities.rs` (inline) | Annotation capability derivation, validation, provider handles |
| `tools/config/tests.rs` | TOML parsing, timeout validation, backend types, capabilities/annotations fields |
| `tools/config/annotations.rs` (inline) | Annotation config → MCP annotations conversion |
| `tools/builtins/tests.rs` | Built-in registration, definition completeness, workspace tools |
| `tools/builtins/helpers/tests.rs` | Path validation, backup generation, pruning |
| `tools/builtins/update_persona/tests.rs` | Persona update logic, backup creation |
| `tools/mcp/bridge.rs` / `client_set.rs` / `config.rs` (inline) | Namespacing, result serialization, config parsing, server-name validation |
| `tools/stats.rs` (inline) | Stats aggregation over `tool_execution_log` |
| `tools/url_validation.rs` (inline) | SSRF validation: all blocked categories, public URLs |
| `tools/platform.rs` (inline) | Shell command creation |
| `security/sandbox/tests.rs` | Full sandbox flow, capability denial, confirmation, approval cache, timeout |
| `security/capabilities/tests.rs` | Deny/allow list logic |
| `security/sanitizer/tests.rs` | Path traversal, command injection, null bytes, uploads |
| `security/circuit_breaker/tests.rs` | State transitions, transient classification, pruning |
| `security/confirmation.rs` (inline) | Broker lifecycle, approval cache, canonical args hashing |
| `runner/agentic_loop/tests.rs` | Tool call limiting, budget enforcement, loop behavior |
| `runner/lead_agent/tests.rs` | Coordination tool behavior, spawn guards |
| `runner/dag_executor/tests.rs` | DAG node execution |

### Running Tests

```bash
# All tool-related tests
cargo test -p openalpaca_core -- tools::
cargo test -p openalpaca_core -- security::

# Specific module
cargo test -p openalpaca_core -- tools::registry
cargo test -p openalpaca_core -- security::circuit_breaker
cargo test -p openalpaca_core -- tools::url_validation
```
