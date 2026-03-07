# Tool System — Technical Reference

> **Status:** Living document · **Last updated:** 2026-03-07
>
> Code-level reference for the OpenAlpaca tool system.  For architecture
> and design rationale see the companion [DESIGN.md](./DESIGN.md).

---

## Table of Contents

1. [File Map](#1-file-map)
2. [Core Types](#2-core-types)
3. [ToolRegistry](#3-toolregistry)
4. [Built-in Tools](#4-built-in-tools)
5. [Custom Tools (TOML Configuration)](#5-custom-tools-toml-configuration)
6. [Executor Hierarchy](#6-executor-hierarchy)
7. [Security Layers](#7-security-layers)
8. [Agentic Loop Integration](#8-agentic-loop-integration)
9. [Tool Result Handling](#9-tool-result-handling)
10. [Lead Agent Tools](#10-lead-agent-tools)
11. [Workspace Tools](#11-workspace-tools)
12. [Telemetry & Storage](#12-telemetry--storage)
13. [URL Validation](#13-url-validation)
14. [Platform Helpers](#14-platform-helpers)
15. [Configuration Reference](#15-configuration-reference)
16. [Testing](#16-testing)

---

## 1. File Map

### Core Tool System

| File | Purpose |
|------|---------|
| `crates/openalpaca_core/src/tools/mod.rs` | Module root, `resolve_agent_tools()` |
| `crates/openalpaca_core/src/tools/registry/mod.rs` | `ToolRegistry`, `RegisteredTool`, `ToolBackend`, `BuiltInTool` trait |
| `crates/openalpaca_core/src/tools/registry/tests.rs` | Registry unit tests |
| `crates/openalpaca_core/src/tools/executor.rs` | `RegistryToolExecutor` (owner-id stripping) |
| `crates/openalpaca_core/src/tools/contextual_executor/mod.rs` | `ContextualToolExecutor`, `ToolExecutionContext`, `ScriptExecutionContext` |
| `crates/openalpaca_core/src/tools/contextual_executor/tests.rs` | Contextual executor tests |
| `crates/openalpaca_core/src/tools/config/mod.rs` | TOML config parsing (`ToolConfigFile`, `ToolConfig`, `ToolBackendConfig`) |
| `crates/openalpaca_core/src/tools/config/tests.rs` | Config parsing tests |
| `crates/openalpaca_core/src/tools/url_validation.rs` | `validate_url()` SSRF protection |
| `crates/openalpaca_core/src/tools/platform.rs` | `shell_command()` platform abstraction |

### Built-in Tools

| File | Tool(s) |
|------|---------|
| `crates/openalpaca_core/src/tools/builtins/mod.rs` | Registration functions, workspace tool definitions |
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
| `crates/openalpaca_core/src/tools/builtins/send.rs` | `send` (connector message delivery) |
| `crates/openalpaca_core/src/tools/builtins/text_generate.rs` | `text_generate` (stub) |
| `crates/openalpaca_core/src/tools/builtins/summarize.rs` | `summarize` (stub) |
| `crates/openalpaca_core/src/tools/builtins/helpers/mod.rs` | Workspace path validation, backup management |
| `crates/openalpaca_core/src/tools/builtins/helpers/tests.rs` | Helper function tests |
| `crates/openalpaca_core/src/tools/builtins/tests.rs` | Built-in tool integration tests |

### Security

| File | Purpose |
|------|---------|
| `crates/openalpaca_core/src/security/mod.rs` | Module root, re-exports |
| `crates/openalpaca_core/src/security/sandbox/mod.rs` | `SandboxManager`, `SandboxPolicy`, `ToolExecutor` trait |
| `crates/openalpaca_core/src/security/sandbox/tests.rs` | Sandbox tests |
| `crates/openalpaca_core/src/security/capabilities/mod.rs` | `CapabilityManager` |
| `crates/openalpaca_core/src/security/capabilities/tests.rs` | Capability tests |
| `crates/openalpaca_core/src/security/sanitizer/mod.rs` | `InputSanitizer` |
| `crates/openalpaca_core/src/security/sanitizer/tests.rs` | Sanitizer tests |
| `crates/openalpaca_core/src/security/circuit_breaker/mod.rs` | `ToolCircuitBreaker`, `is_transient_tool_error()` |
| `crates/openalpaca_core/src/security/circuit_breaker/tests.rs` | Circuit breaker tests |
| `crates/openalpaca_core/src/security/gate.rs` | `SecurityGate` facade |
| `crates/openalpaca_core/src/security/policy.rs` | `Principal`, `Scope`, `TrustGate` |
| `crates/openalpaca_core/src/security/confirmation.rs` | `ConfirmationBroker`, `ConfirmationRequest`, `ConfirmationResponse` |

### Agentic Loop

| File | Purpose |
|------|---------|
| `crates/openalpaca_core/src/runner/agentic_loop/mod.rs` | `run_agentic_loop()`, `run_agentic_loop_routed()` |
| `crates/openalpaca_core/src/runner/agentic_loop/tool_helpers.rs` | `truncate_tool_result()`, `format_tool_error()`, `format_tool_error_with_hint()` |
| `crates/openalpaca_core/src/runner/agentic_loop/backend.rs` | LLM backend abstraction |
| `crates/openalpaca_core/src/runner/agentic_loop/config.rs` | `LoopConfig` |
| `crates/openalpaca_core/src/runner/agentic_loop/context.rs` | Loop state management |

### Lead Agent & DAG

| File | Purpose |
|------|---------|
| `crates/openalpaca_core/src/runner/lead_agent/tools.rs` | `SpawnSubagentTool`, `CheckSubagentStatusTool`, `WaitForSubagentsTool` |
| `crates/openalpaca_core/src/runner/lead_agent/mod.rs` | Lead agent orchestration |
| `crates/openalpaca_core/src/runner/lead_agent/tracker.rs` | `SubagentTracker` |
| `crates/openalpaca_core/src/runner/dag_executor/mod.rs` | DAG node execution |
| `crates/openalpaca_core/src/runner/dag_executor/node_runner.rs` | Per-node tool resolution |

### LLM Types

| File | Purpose |
|------|---------|
| `crates/openalpaca_llm/src/types.rs` | `ToolDefinition`, `ToolCall`, `ToolChoice`, `ChatResponse` |

### Daemon Wiring

| File | Purpose |
|------|---------|
| `apps/openalpacad/src/services/tools.rs` | `build_tool_registry()` — startup registration |

### Configuration

| File | Purpose |
|------|---------|
| `config/tools/example.toml` | Example custom tool configuration |

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

### ToolChoice (LLM control)

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ToolChoice {
    Auto,           // Model decides whether to use tools
    Any,            // Model must call some tool
    Tool(String),   // Model must call specific tool
}
```

### RegisteredTool (Internal)

**Location:** `crates/openalpaca_core/src/tools/registry/mod.rs`

```rust
pub struct RegisteredTool {
    pub definition: ToolDefinition,  // Schema for LLM
    pub backend: ToolBackend,        // Execution backend
}
```

### ToolBackend

```rust
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
}
```

### BuiltInTool Trait

```rust
#[async_trait]
pub trait BuiltInTool: Send + Sync {
    async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String>;
}
```

### ToolExecutor Trait

**Location:** `crates/openalpaca_core/src/security/sandbox/mod.rs`

```rust
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    async fn execute(&self, tool_name: &str, arguments: &serde_json::Value)
        -> Result<String, String>;

    fn registered_tools(&self) -> Vec<String>;

    fn shell_like_tools(&self) -> Vec<String> {
        Vec::new()  // Override to identify command-injection-checked tools
    }
}
```

---

## 3. ToolRegistry

**Location:** `crates/openalpaca_core/src/tools/registry/mod.rs`

### Structure

```rust
pub struct ToolRegistry {
    tools: HashMap<String, RegisteredTool>,
    http_client: reqwest::Client,  // Shared for connection pooling
}
```

### Key Methods

| Method | Signature | Description |
|--------|-----------|-------------|
| `new()` | `() -> Self` | Create empty registry with configured HTTP client |
| `register()` | `(&mut self, tool: RegisteredTool)` | Add tool (startup only) |
| `get()` | `(&self, name: &str) -> Option<&RegisteredTool>` | Look up by name |
| `definitions_for_skills()` | `(&self, skills: &[String]) -> Vec<ToolDefinition>` | Resolve skill names to tool definitions |
| `execute()` | `async (&self, name: &str, args: &Value) -> Result<String, String>` | Validate args + dispatch to backend |
| `registered_tool_names()` | `(&self) -> Vec<String>` | All tool names |
| `command_backend_tool_names()` | `(&self) -> Vec<String>` | Tools with Command backend |

### Argument Validation

Before executing, `validate_tool_arguments()` performs:

1. **Root type check:** If schema specifies `"type": "object"`, args must be an object.
2. **Required fields:** All entries in `"required"` array must be present.
3. **Field type matching:** Each property's type is validated against the schema
   (`string`, `number`, `integer`, `boolean`, `array`, `object`, `null`).

### HTTP Backend Execution

```rust
async fn execute_http(
    client: &reqwest::Client,
    method: &str,
    url_template: &str,
    headers: &HashMap<String, String>,
    timeout_secs: u64,
    arguments: &serde_json::Value,
) -> Result<String, String>
```

**Steps:**
1. Replace `{param}` placeholders with URL-encoded argument values
2. Detect unsubstituted placeholders (error)
3. SSRF validation via `validate_url()`
4. Send HTTP request with method, headers, timeout
5. Check for 2xx status
6. Stream response body (cap: 1 MB)
7. Return body (truncated to 8 KB for display)

### Command Backend Execution

```rust
async fn execute_command(
    command: &str,
    args_template: Option<&str>,
    timeout_secs: u64,
    arguments: &serde_json::Value,
) -> Result<String, String>
```

**Steps:**
1. Replace `{param}` placeholders with shell-escaped argument values
2. Detect unsubstituted placeholders (error)
3. Build command via `platform::shell_command()`
4. Execute with `tokio::time::timeout`
5. Capture stdout + stderr (cap: 512 KB each)
6. Check exit code (0 = success)

---

## 4. Built-in Tools

### 4.1 Tool Inventory

| Tool | Parameters | Key Constraints |
|------|-----------|-----------------|
| `web_search` | `query: string` | Brave Search API; requires `api_key` in web search config |
| `web_fetch` | `url: string` | SSRF-protected; response cap 1 MB |
| `file_read` | `path: string` | Workspace-scoped (relative only); max 10 MB |
| `file_write` | `path: string, content: string` | Workspace-scoped; blocks SOUL.md/USER.md/IDENTITY.md |
| `shell_execute` | `command: string` | Timeout configurable; output cap 512 KB |
| `memory_search` | `query: string` | Owner-scoped; hybrid FTS5 + sqlite-vec KNN |
| `update_persona` | `target: string, content: string` | Owner-scoped; targets `soul`/`user`/`identity`; creates backups |
| `send` | `message: string, channel: string` | Requires `ConnectorSendProvider`; supports file attachments |
| `text_generate` | `prompt: string` | **Stub** — returns "not implemented" |
| `summarize` | `text: string` | **Stub** — returns "not implemented" |

### 4.2 Registration Functions

```rust
/// Core built-in tools (no persona context needed)
pub fn builtin_tools(
    db: Option<Database>,
    embedder: Option<Arc<dyn Embedder>>,
    daemon_config: Option<Arc<ArcSwap<DaemonConfig>>>,
    web_search_config: Option<Arc<ArcSwap<WebSearchConfig>>>,
    workspace_root: Option<PathBuf>,
) -> Vec<RegisteredTool>
// Returns: web_search, web_fetch, file_read, file_write, shell_execute
//          + memory_search (if db + daemon_config provided)

/// Full built-in tools with persona and connector context
pub fn builtin_tools_with_persona_context(
    db: Option<Database>,
    embedder: Option<Arc<dyn Embedder>>,
    persona_ctx: PersonaContext,
    daemon_config: Option<Arc<ArcSwap<DaemonConfig>>>,
    web_search_config: Option<Arc<ArcSwap<WebSearchConfig>>>,
    workspace_root: Option<PathBuf>,
    connector_send_provider: Option<ConnectorSendLock>,
) -> Vec<RegisteredTool>
// Returns: all of builtin_tools() PLUS update_persona, send

/// Workspace tool definitions (not registered in ToolRegistry)
pub fn workspace_tool_definitions() -> Vec<ToolDefinition>
// Returns: workspace_read, workspace_write
```

### 4.3 Workspace Path Helpers

**Location:** `crates/openalpaca_core/src/tools/builtins/helpers/mod.rs`

| Function | Purpose |
|----------|---------|
| `validate_workspace_path(path)` | Reject absolute paths and `..` components |
| `resolve_workspace_path(rel, root)` | Canonicalize + verify stays within workspace |
| `resolve_workspace_path_for_write(rel, root)` | Like above but for new files (parent may not exist) |
| `is_soul_path(path)` | Case-insensitive check for SOUL.md |
| `is_user_path(path)` | Case-insensitive check for USER.md |
| `is_identity_path(path)` | Case-insensitive check for IDENTITY.md |
| `unique_backup_path(dir, prefix)` | Generate timestamped backup path with UUID suffix |
| `prune_backups(dir, max, prefix)` | Remove oldest backups exceeding retention limit |

### 4.4 Constants

| Constant | Value | Location |
|----------|-------|----------|
| `MAX_FILE_READ_SIZE` | 10 MB | `builtins/helpers/mod.rs` |
| `MAX_TOOL_RESULT_SIZE` | 32 KB | `agentic_loop/tool_helpers.rs` |
| `MAX_HTTP_RESPONSE_SIZE` | 1 MB | `registry/mod.rs` |
| `MAX_COMMAND_OUTPUT_SIZE` | 512 KB | `registry/mod.rs` |
| `MAX_HTTP_DISPLAY_SIZE` | 8 KB | `registry/mod.rs` |

---

## 5. Custom Tools (TOML Configuration)

**Location:** `crates/openalpaca_core/src/tools/config/mod.rs`

### File Format

```toml
# config/tools/my_tools.toml

[[tools]]
name = "my_api_tool"
description = "Calls my REST API"

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

# Optional for HTTP backend:
# [tools.backend.headers]
# Authorization = "Bearer ${MY_API_KEY}"
```

### Types

```rust
#[derive(Deserialize)]
pub struct ToolConfigFile {
    #[serde(default)]
    pub tools: Vec<ToolConfig>,
}

#[derive(Deserialize)]
pub struct ToolConfig {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
    pub backend: ToolBackendConfig,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
pub enum ToolBackendConfig {
    #[serde(rename = "http")]
    Http {
        url: String,
        method: Option<String>,
        headers: Option<HashMap<String, String>>,
        timeout_secs: Option<u64>,
    },
    #[serde(rename = "command")]
    Command {
        command: String,
        args_template: Option<String>,
        timeout_secs: Option<u64>,
    },
}
```

### Loading Functions

```rust
/// Load tools from a single TOML file
pub fn load_tools_from_file(path: &Path) -> Result<Vec<RegisteredTool>, String>

/// Scan directory for *.toml files and load all tools (errors logged, not fatal)
pub fn load_tools_from_dir(dir: &Path) -> Vec<RegisteredTool>
```

### Template Substitution

Both HTTP and Command backends support `{param}` template placeholders:

- **HTTP URLs:** Values are URL-encoded (`urlencoding::encode()`)
- **Command args:** Values are shell-escaped (`shell_escape::escape()`)
- **Unsubstituted placeholders** are detected and cause an error

---

## 6. Executor Hierarchy

### 6.1 RegistryToolExecutor

**Location:** `crates/openalpaca_core/src/tools/executor.rs`

```rust
pub struct RegistryToolExecutor {
    registry: Arc<ToolRegistry>,
}
```

**Key behavior:** Strips `owner_id` and `workspace_id` from owner-scoped
tool arguments before forwarding to the registry.  This prevents LLM
spoofing.

**Owner-scoped tools list:**
```rust
const OWNER_SCOPED_TOOLS: &[&str] = &["memory_search", "update_persona"];
```

### 6.2 ContextualToolExecutor

**Location:** `crates/openalpaca_core/src/tools/contextual_executor/mod.rs`

```rust
pub struct ContextualToolExecutor {
    registry: Arc<ToolRegistry>,
    context: ToolExecutionContext,
    scripts: Option<ScriptExecutionContext>,
}

pub struct ToolExecutionContext {
    pub owner_id: Option<String>,
    pub task_id: Option<String>,
    pub agent_id: Option<String>,
    pub db: Option<Database>,
    pub workspace_id: Option<String>,
}

pub struct ScriptExecutionContext {
    scripts: HashMap<String, (PathBuf, Option<String>, u64)>,
    // tool_name → (resolved_path, interpreter, timeout_secs)
    skill_dir: PathBuf,
}
```

**Dispatch logic:**

| Tool Name | Handler |
|-----------|---------|
| `workspace_read` | Direct: reads from task state JSON |
| `workspace_write` | Direct: upserts with optimistic locking (5 retries) |
| `skill_script:*` | Direct: executes bundled script |
| Owner-scoped tool | Injects `owner_id`/`workspace_id`, delegates to `RegistryToolExecutor` |
| Everything else | Delegates to `RegistryToolExecutor` |

### 6.3 Execution Chain

```
Agentic Loop
     │
     ▼
SandboxManager.execute_tool()
     │ security checks
     ▼
ContextualToolExecutor.execute()
     │ workspace/owner/script handling
     ▼
RegistryToolExecutor.execute()
     │ owner_id stripping
     ▼
ToolRegistry.execute()
     │ validation + backend dispatch
     ▼
BuiltInTool.execute() / execute_http() / execute_command()
```

---

## 7. Security Layers

### 7.1 SandboxManager

**Location:** `crates/openalpaca_core/src/security/sandbox/mod.rs`

```rust
pub struct SandboxManager {
    executor: Arc<dyn ToolExecutor>,
    bus: EventBus,
    circuit_breaker: ToolCircuitBreaker,
    db: Option<Database>,
    confirmation_broker: Option<Arc<ConfirmationBroker>>,
}
```

**`execute_tool()` flow:**

```rust
pub async fn execute_tool(
    &self,
    agent_id: &str,
    tool_call: &ToolCall,
    policy: &SandboxPolicy,
) -> Result<String, String>
```

1. `CapabilityManager::check_agent_capability()` — deny/allow lists
2. `InputSanitizer::sanitize_tool_args()` — injection/traversal checks
3. Confirmation gate (if tool in `require_confirmation_for`)
4. `circuit_breaker.check()` — consecutive failure threshold
5. `tokio::time::timeout()` — enforce `max_tool_runtime_secs`
6. `executor.execute()` — actual tool execution
7. `circuit_breaker.record_success/failure()` — update state
8. Emit `SystemEvent::ToolExecuted` or `SystemEvent::SecurityViolation`

**Special bypass:** Coordination tools (`wait_for_subagents`,
`check_subagent_status`) skip the sandbox timeout wrapper.

### 7.2 SandboxPolicy

```rust
pub struct SandboxPolicy {
    pub agent_id: String,
    pub allowed_capabilities: Vec<String>,
    pub denied_capabilities: Vec<String>,
    pub require_confirmation_for: Vec<String>,
    pub max_tool_calls: Option<u32>,
    pub max_tool_runtime_secs: u64,
    pub stream_id: Option<String>,        // For SSE confirmation routing
    pub lane_key: Option<String>,         // For connector confirmation routing
    pub confirmation_timeout_secs: Option<u64>,
    pub auto_approve: bool,
}
```

### 7.3 CapabilityManager

**Location:** `crates/openalpaca_core/src/security/capabilities/mod.rs`

```rust
impl CapabilityManager {
    pub fn check_agent_capability(
        agent_id: &str,
        tool_name: &str,
        constraints: &AgentConstraints,
    ) -> Result<(), SecurityViolation>
}
```

**Rules:**
1. If `denied_capabilities` contains `tool_name` → **DENIED**
2. If `allowed_capabilities` is non-empty AND does not contain `tool_name` → **DENIED**
3. Otherwise → **ALLOWED**

### 7.4 InputSanitizer

**Location:** `crates/openalpaca_core/src/security/sanitizer/mod.rs`

```rust
impl InputSanitizer {
    pub fn sanitize_tool_args(
        tool_name: &str,
        arguments: &serde_json::Value,
        allowed_tools: &[String],
        extra_shell_tools: &[String],
    ) -> Result<(), SecurityViolation>
}
```

**Checks performed:**
- Tool name in allowlist (if non-empty)
- Path traversal in all string values (`..` components, absolute paths)
- Null bytes in string values
- For shell-like tools: backtick execution, `$(...)` subshell, newline injection

**Shell-like tools:** `shell_execute` (hardcoded) + Command-backend tools +
`extra_shell_tools` list.

### 7.5 ToolCircuitBreaker

**Location:** `crates/openalpaca_core/src/security/circuit_breaker/mod.rs`

```rust
pub struct ToolCircuitBreaker {
    state: Mutex<HashMap<(String, String), ToolState>>,  // (agent_id, tool_name)
    failure_threshold: usize,
    reset_timeout: Duration,
    enabled: bool,
    bus: EventBus,
}
```

**State machine:**

```
   ┌─────────┐  failure >= threshold  ┌──────────┐
   │ CLOSED  ├───────────────────────►│   OPEN   │
   │ (allow) │                        │  (block) │
   └────▲────┘                        └────┬─────┘
        │                                  │ timeout elapsed
        │ probe succeeds                   │
   ┌────┴────────┐                    ┌────▼─────┐
   │             │◄───────────────────│ HALF-OPEN│
   │             │                    │ (1 probe)│
   │             │  probe fails       └──────────┘
   │             │───────────────────►│   OPEN   │
   └─────────────┘                    └──────────┘
```

**Error classification** (`is_transient_tool_error()`):
- **Transient** (trips breaker): timeouts, HTTP 5xx, connection refused/reset, network errors
- **Permanent** (does not trip): bad arguments, 404, tool not found

**Memory management:** Entries are pruned when map exceeds 10,000 entries
(removes entries idle > 1 hour).

### 7.6 ConfirmationBroker

**Location:** `crates/openalpaca_core/src/security/confirmation.rs`

```rust
pub struct ConfirmationBroker {
    pending: DashMap<String, oneshot::Sender<ConfirmationResponse>>,
}
```

| Method | Signature | Description |
|--------|-----------|-------------|
| `request()` | `(&self, req: &ConfirmationRequest) -> oneshot::Receiver<ConfirmationResponse>` | Register pending confirmation |
| `respond()` | `(&self, id: &str, resp: ConfirmationResponse) -> Result<(), String>` | Deliver user's decision |
| `cancel()` | `(&self, id: &str)` | Clean up on timeout |
| `pending_count()` | `(&self) -> usize` | Diagnostic count |
| `pending_keys()` | `(&self) -> Vec<String>` | List pending IDs |

**ConfirmationRequest fields:** `request_id`, `agent_id`, `tool_name`,
`tool_arguments`, `stream_id`, `lane_key`, `timestamp`.

### 7.7 TrustGate (Principal-Level)

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

impl TrustGate {
    pub fn check(principal: &Principal, capability: &Capability, scope: &Scope)
        -> Result<(), String>
}
```

**Rules:**
- `System` → always allowed
- `External` → blocked on high-risk actions (`system.*`, `fs.write`, `net.connect`) and `chat.respond`
- `User` → generally allowed (future: granular ACLs)

---

## 8. Agentic Loop Integration

**Location:** `crates/openalpaca_core/src/runner/agentic_loop/mod.rs`

### Entry Points

```rust
/// Legacy entry point (direct provider, for testing)
pub async fn run_agentic_loop(
    provider: &dyn LlmProvider,
    initial_messages: Vec<ChatMessage>,
    tools: Vec<ToolDefinition>,
    config: &LoopConfig,
    sandbox: Option<&SandboxManager>,
    agent_id: &str,
    sandbox_policy: Option<&SandboxPolicy>,
    cancel_token: Option<CancellationToken>,
) -> LoopResult

/// Production entry point (router + cost tracking)
pub async fn run_agentic_loop_routed(
    router: &LlmRouter,
    initial_messages: Vec<ChatMessage>,
    tools: Vec<ToolDefinition>,
    config: &LoopConfig,
    sandbox: Option<&SandboxManager>,
    agent_id: &str,
    sandbox_policy: Option<&SandboxPolicy>,
    task_id: Option<&str>,
    cancel_token: Option<CancellationToken>,
) -> LoopResult
```

### Tool Execution Phase (per round)

```
LLM Response (finish_reason: ToolUse)
         │
         ▼
    Extract tool_calls from response
         │
         ▼
    Add assistant message with tool_calls to conversation
         │
         ▼
    Compute budget: remaining = policy.max_tool_calls - state.tool_calls_made
         │
         ▼
    Apply per-round limit: min(tool_calls.len(), config.max_tools_per_round)
         │
         ▼
    Partition: executable vs over-budget
         │
    ┌────┴────┐
    │         │
    ▼         ▼
Executable  Over-budget → format_tool_error("max_tool_calls limit reached")
    │
    ▼
Create async futures (each calls SandboxManager.execute_tool())
    │
    ▼
join_all() — parallel execution, raced against cancel_token
    │
    ▼
For each result:
  • Success → truncate_tool_result() (32 KB limit)
  • Error → format_tool_error_with_hint()
    │
    ▼
Push ChatMessage::tool_result(id, text) for each
    │
    ▼
state.tool_calls_made += executable.len()
    │
    ▼
Continue to next round (or exit if max_rounds / EndTurn / error)
```

---

## 9. Tool Result Handling

**Location:** `crates/openalpaca_core/src/runner/agentic_loop/tool_helpers.rs`

### Result Truncation

```rust
const MAX_TOOL_RESULT_SIZE: usize = 32 * 1024;  // 32 KB

pub fn truncate_tool_result(text: String) -> String
```

**Smart boundary detection** (priority order):
1. Sentence boundary (`. `, `! `, `? `)
2. Line boundary (`\n`)
3. Word boundary (` `)
4. Character boundary (byte-aware)

Appends `[... truncated: showing first X of Y bytes]` when truncated.

### Error Formatting

```rust
pub fn format_tool_error(msg: &str) -> String
// Returns: "[tool_error] {msg}"

pub fn format_tool_error_with_hint(tool_name: &str, msg: &str) -> String
```

**Recovery hints by tool:**

| Tool | Error Pattern | Hint |
|------|--------------|------|
| `file_read` | "not found" | "Verify the path exists. Try listing the directory first." |
| `web_fetch` | "404" | "Use web_search to find the correct URL." |
| `shell_execute` | "timed out" | "Break the operation into smaller steps." |
| `memory_search` | "no results" | "Try broader search terms or different keywords." |

---

## 10. Lead Agent Tools

**Location:** `crates/openalpaca_core/src/runner/lead_agent/tools.rs`

These tools are **runtime-injected** (not in `ToolRegistry`) and only
available to the lead agent in lead-agent dispatch mode.

### spawn_subagent

Spawns a background subagent task from an agent template.

**Parameters:**
- `agent_id: string` — template ID to instantiate
- `objective: string` — task description for the subagent

**Security:**
- Self-spawn prevention (cannot spawn own template)
- Depth limit: `MAX_SUBAGENT_DEPTH` (3)
- Concurrency semaphore: `max_concurrent_subagents`

**Returns:** `run_id` immediately (non-blocking)

### spawn_subagents_batch

Spawn 1-8 subagents in a single call.

### check_subagent_status

Poll status of one or all spawned subagents.

**Returns:** JSON status (`Queued`, `Running`, `Completed`, `Failed`)
with output if complete.

### wait_for_subagents

Block until all spawned subagents complete.

**Note:** This tool bypasses the sandbox timeout wrapper since it
intentionally waits for external completion.

---

## 11. Workspace Tools

**Not in `ToolRegistry`** — handled directly by `ContextualToolExecutor`.

### workspace_read

```json
{
  "name": "workspace_read",
  "parameters": {
    "type": "object",
    "properties": {
      "key": {
        "type": "string",
        "description": "Specific key to read (omit to list all)"
      }
    }
  }
}
```

Returns JSON array of workspace entries.

### workspace_write

```json
{
  "name": "workspace_write",
  "parameters": {
    "type": "object",
    "required": ["key", "content"],
    "properties": {
      "key": { "type": "string" },
      "content": { "type": "string", "maxLength": 32768 },
      "entry_type": { "enum": ["text", "artifact", "summary", "context"] },
      "file_asset_id": { "type": "string" }
    }
  }
}
```

**Concurrency:** Optimistic locking via `state_version` column with
exponential backoff (up to 5 retries on version conflict).

---

## 12. Telemetry & Storage

### Database Tables

**Migration 030:** `crates/openalpaca_storage/src/migrations/030_skill_tool_execution_log.sql`

#### tool_execution_log

| Column | Type | Description |
|--------|------|-------------|
| `id` | INTEGER PK | Auto-increment row ID |
| `request_id` | TEXT | Correlation to parent request/task |
| `agent_id` | TEXT NOT NULL | Executing agent |
| `tool_name` | TEXT NOT NULL | Tool invoked |
| `success` | INTEGER | 0 = failed, 1 = succeeded |
| `duration_ms` | INTEGER | Execution time |
| `error_message` | TEXT | Error text if failed |
| `timestamp` | TEXT DEFAULT | ISO timestamp |

**Indexes:**
- `idx_tel_tool_ts`: `(tool_name, timestamp DESC)`
- `idx_tel_request`: `(request_id)`

#### skill_execution_log

| Column | Type | Description |
|--------|------|-------------|
| `id` | INTEGER PK | Auto-increment row ID |
| `request_id` | TEXT | Correlation ID |
| `agent_id` | TEXT NOT NULL | Agent template |
| `skill_name` | TEXT | Skill used |
| `tool_calls_made` | INTEGER | Total tool calls |
| `rounds_used` | INTEGER | LLM loop rounds |
| `input_tokens` | INTEGER | LLM input tokens |
| `output_tokens` | INTEGER | LLM output tokens |
| `cost_usd` | REAL | Estimated API cost |
| `success` | INTEGER | Overall success |
| `duration_ms` | INTEGER | Total execution time |
| `error_message` | TEXT | Error if failed |
| `timestamp` | TEXT DEFAULT | ISO timestamp |

### Event Flow

```
SandboxManager.execute_tool()
         │
         ▼ emit
SystemEvent::ToolExecuted { agent_id, tool_name, success, duration_ms }
         │
         ▼ EventBus broadcast
Event Bridge (main.rs)
         │
         ├──► EventBroadcaster.tool_executed()
         │        │
         │        ├──► ServerEvent::ToolExecuted (WebSocket broadcast)
         │        └──► EventLogRepository.insert() (event_log table)
         │
         └──► SkillExecutionRepository.record_tool() (tool_execution_log table)
```

### Retention & Cleanup

```rust
pub fn spawn_telemetry_cleanup(db: Database, cancel: CancellationToken)
```

- **Interval:** Daily (86,400s)
- **Tool execution logs:** 7-day retention
- **Skill execution logs:** 90-day retention
- Runs as background task, cancellable via `CancellationToken`

---

## 13. URL Validation

**Location:** `crates/openalpaca_core/src/tools/url_validation.rs`

```rust
pub fn validate_url(url: &str) -> Result<(), String>
```

### Blocked Categories

| Category | Examples |
|----------|---------|
| Non-HTTP schemes | `file://`, `ftp://`, `gopher://` |
| Cloud metadata | `169.254.169.254`, `metadata.google.internal` |
| Localhost | `localhost`, `127.0.0.1`, `[::1]`, `0.0.0.0` |
| IPv4 private | `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16` |
| IPv4 CGN | `100.64.0.0/10` |
| IPv4 link-local | `169.254.0.0/16` |
| IPv6 loopback | `::1` |
| IPv6 ULA | `fc00::/7` (fc and fd prefixes) |
| IPv6 link-local | `fe80::/10` |
| IPv4-mapped IPv6 | `::ffff:10.0.0.1` (private embedded) |

### Helper Functions

```rust
fn is_ipv6_unique_local(ip: &Ipv6Addr) -> bool  // fc00::/7
fn is_ipv6_link_local(ip: &Ipv6Addr) -> bool     // fe80::/10
fn is_ipv4_mapped_private(ip: &Ipv6Addr) -> bool  // ::ffff:x.x.x.x
```

---

## 14. Platform Helpers

**Location:** `crates/openalpaca_core/src/tools/platform.rs`

```rust
pub fn shell_command(cmd: &str) -> tokio::process::Command
```

- **macOS/Linux:** `sh -c "<cmd>"`
- **Windows:** `cmd /c "<cmd>"`

Used by `shell_execute` built-in and Command-backend tools.

---

## 15. Configuration Reference

### daemon.toml

```toml
[execution.agent_defaults]
max_rounds = 15               # Max LLM loop iterations per agent
max_tools_per_round = 5        # Max tool calls per LLM round
max_tool_runtime_secs = 60     # Per-tool execution timeout
max_cost = 1.00                # Max API cost per agent run (USD)

[execution.circuit_breaker]
enabled = true
failure_threshold = 5          # Consecutive failures before opening
reset_timeout_secs = 60        # Seconds before half-open probe
```

### Agent Config (TOML/YAML)

```yaml
---
id: researcher
skills:
  - web_search
  - web_fetch
  - memory_search
  - workspace_read
  - workspace_write

[constraints]
max_tool_calls = 50
timeout_seconds = 300
require_confirmation_for = ["shell_execute"]
allowed_capabilities = []      # Empty = allow all (not denied)
denied_capabilities = ["file_write"]
auto_approve = false
---
```

### Custom Tool TOML

See [Section 5](#5-custom-tools-toml-configuration) for full format.

### Protected Tool Names

Cannot be overridden by user TOML configs:

```
update_persona, shell_execute, file_read, file_write,
memory_search, workspace_read, workspace_write,
spawn_subagent, spawn_subagents_batch,
check_subagent_status, wait_for_subagents, send
```

---

## 16. Testing

### Test File Locations

| Test File | Coverage |
|-----------|----------|
| `tools/registry/tests.rs` | Registry delegation, argument validation, backend dispatch |
| `tools/config/tests.rs` | TOML parsing, timeout validation, backend type deserialization |
| `tools/executor.rs` (inline) | Owner-ID stripping for memory_search, workspace_id stripping |
| `tools/contextual_executor/tests.rs` | Workspace R/W, script execution, owner injection |
| `tools/builtins/tests.rs` | Built-in tool registration, definition completeness |
| `tools/builtins/helpers/tests.rs` | Path validation, backup generation, pruning |
| `tools/builtins/update_persona/tests.rs` | Persona update logic, backup creation |
| `tools/url_validation.rs` (inline) | SSRF validation: all blocked categories, public URLs |
| `tools/platform.rs` (inline) | Shell command creation |
| `security/sandbox/tests.rs` | Full sandbox flow, capability denial, timeout |
| `security/capabilities/tests.rs` | Deny/allow list logic |
| `security/sanitizer/tests.rs` | Path traversal, command injection, null bytes |
| `security/circuit_breaker/tests.rs` | State transitions, transient error classification, pruning |
| `security/confirmation.rs` (inline) | Broker request/respond/cancel lifecycle |
| `runner/agentic_loop/` (inline) | Tool call limiting, budget enforcement |

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
