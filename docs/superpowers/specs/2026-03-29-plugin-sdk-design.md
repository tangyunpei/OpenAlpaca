# Plugin SDK v1 — Design Spec

**Date:** 2026-03-29
**Status:** Approved
**Branch:** TBD (created at implementation time)

---

## Goal

Build a full plugin system so users can extend OpenAlpaca without recompiling — add tools, connectors, LLM providers, skills, and agents as loadable plugins. Plugins are subprocess-based (any language), discovered from a directory, hot-loadable at runtime, and permission-gated.

## Design Decisions

| Decision | Choice | Rationale |
|---|---|---|
| Execution model | Subprocess (JSON-RPC over stdio) | Language-agnostic, free process isolation, MCP-compatible |
| Wire protocol | MCP for tools + custom RPC for connectors/providers/skills/agents | Get MCP ecosystem for free, extend for our needs |
| Plugin lifecycle | Hot-loadable | Add/remove/restart plugins without killing active conversations |
| Packaging | Directory convention now, archive distribution later | Start simple, forward-compatible manifest |
| Plugin types (v1) | All five: tools, connectors, providers, skills, agents | Maximum flexibility from day one |
| Permissions | Manifest-declared capabilities, user-approved on first load | Mobile-style permission model, integrates with existing CapabilityManager |

---

## 1. Plugin Protocol

### Transport

JSON-RPC 2.0 over stdio. Every plugin is a child process. Daemon writes to plugin's stdin, reads from stdout. Stderr captured for logging.

**Framing:** LSP/MCP-standard `Content-Length` framing: `Content-Length: <bytes>\r\n\r\n<json>`. This is what MCP already uses, so existing MCP servers work without modification. All plugins must use this framing.

### Protocol Namespaces

Five namespaces on a single connection, plus lifecycle methods:

#### Lifecycle (all plugins)

| Method | Direction | Purpose |
|---|---|---|
| `initialize` | daemon -> plugin | Send config, granted capabilities, plugin ID |
| `health/check` | daemon -> plugin | Heartbeat (see schema below) |
| `shutdown` | daemon -> plugin | Graceful shutdown request |
| `$/event` | plugin -> daemon | Plugin pushes events (inbound messages, status changes) |

`initialize` request (daemon -> plugin):
```json
{
  "plugin_id": "playwright-browser",
  "version": "0.1.0",
  "capabilities_granted": ["network", "filesystem.read"],
  "config": { "rate_limit": 30, "webhook_url": null },
  "daemon_version": "0.6.0"
}
```

`initialize` response (plugin -> daemon):
```json
{
  "protocol_version": "1.0",
  "ready": true
}
```

The `config` field contains all user-provided values from `~/.openalpaca/plugins/.config/<plugin_name>.toml`. Missing optional fields are `null`. If required config fields are missing, the plugin is not spawned (see Section 3, Config Validation). Timeout for `initialize`: 10s. If the plugin does not respond within 10s, it transitions to `Crashed` and enters backoff.

`health/check` request (daemon -> plugin):
```json
{}
```

`health/check` response (plugin -> daemon):
```json
{
  "healthy": true,
  "uptime_secs": 3600,
  "active_requests": 2,
  "error": null
}
```

Health checks run every 30s per plugin. If the plugin does not respond within 5s, it is considered unhealthy. Three consecutive unhealthy checks trigger crash recovery (same as process exit).

### StdioChannel Multiplexing

A single `StdioChannel` per plugin process handles all concurrent RPC calls. Architecture:

1. **Request/response correlation:** Every JSON-RPC request includes an `id` field (incrementing u64). The daemon maintains a `HashMap<u64, oneshot::Sender<JsonRpcResponse>>` for pending requests.
2. **Single reader task:** One tokio task reads stdout continuously, parsing `Content-Length` framed messages. For each message:
   - If it has an `id` field → look up the matching `oneshot::Sender` and send the response.
   - If it has no `id` (notification, i.e. `$/event`) → dispatch to EventRelay.
3. **Single writer task:** A `mpsc::Sender<JsonRpcRequest>` feeds a serialization task that writes to stdin with `Content-Length` framing. Callers hold the `mpsc::Sender` clone and await the `oneshot::Receiver`.
4. **Concurrency limit:** The `max_concurrent` Semaphore is acquired before sending a request and released when the response arrives (or timeout fires).
5. **Cleanup on crash:** When the process exits, all pending `oneshot::Sender`s are dropped, causing callers to receive `RecvError` which maps to error code `-32002`.

```rust
pub struct StdioChannel {
    writer: mpsc::Sender<JsonRpcRequest>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResult>>>>,
    next_id: AtomicU64,
    semaphore: Arc<Semaphore>,
}

impl StdioChannel {
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, PluginError> {
        let _permit = self.semaphore.acquire().await?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);
        self.writer.send(JsonRpcRequest { id, method, params }).await?;
        match tokio::time::timeout(self.timeout, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(PluginError::ProcessCrashed),
            Err(_) => Err(PluginError::Timeout),
        }
    }
}
```

**MCP-only detection:** If the manifest has `mcp_compatible = true` (or if no `plugin.toml` exists — bare MCP server dropped into the plugin dir), the daemon uses pure MCP client mode: skip `initialize`, go straight to `tools/list`. If `plugin.toml` is absent, the daemon looks for a `mcp.json` manifest (standard MCP server config) as fallback. For plugins with a `plugin.toml` and `mcp_compatible = false` (default), the daemon sends `initialize` first.

#### Tools (MCP standard)

| Method | Direction | Purpose |
|---|---|---|
| `tools/list` | daemon -> plugin | Discover available tools |
| `tools/call` | daemon -> plugin | Execute a tool |
| `resources/list` | daemon -> plugin | List available resources |
| `resources/read` | daemon -> plugin | Read a resource |
| `prompts/list` | daemon -> plugin | List prompt templates |
| `prompts/get` | daemon -> plugin | Get a prompt template |

Any existing MCP server works unmodified as a tool plugin.

#### Connectors (`connector/*`)

| Method | Direction | Purpose |
|---|---|---|
| `connector/info` | daemon -> plugin | Get connector metadata (platform, capabilities) |
| `connector/start` | daemon -> plugin | Begin listening for inbound messages |
| `connector/stop` | daemon -> plugin | Stop listening |
| `connector/send` | daemon -> plugin | Send outbound message to a chat |

Inbound messages: plugin -> `$/event` notification -> daemon EventBus.

Inbound message `$/event` schema:
```json
{
  "type": "message",
  "chat_id": "...",
  "sender_id": "...",
  "sender_name": "...",
  "text": "...",
  "attachments": [{ "name": "photo.jpg", "mime_type": "image/jpeg", "url": "https://..." }],
  "reply_to_message_id": null,
  "timestamp": "2026-03-29T12:00:00Z"
}
```

`connector/send` request (daemon -> plugin):
```json
{
  "chat_id": "...",
  "text": "...",
  "reply_to": null,
  "files": ["/absolute/path.png"]
}
```

`connector/send` response (plugin -> daemon):
```json
{
  "success": true,
  "message_id": "platform-specific-id",
  "error": null
}
```

`connector/info` response (plugin -> daemon):
```json
{
  "platform": "telegram",
  "supports_files": true,
  "supports_reactions": true,
  "supports_threads": false,
  "max_message_length": 4096
}
```

`connector/start` request: `{}` (no parameters). Response: `{ "started": true }`.
`connector/stop` request: `{}`. Response: `{ "stopped": true }`.

#### Providers (`provider/*`)

| Method | Direction | Purpose |
|---|---|---|
| `provider/info` | daemon -> plugin | Get provider metadata (models, capabilities) |
| `provider/chat` | daemon -> plugin | Non-streaming completion |
| `provider/chat_streaming` | daemon -> plugin | Streaming completion (token chunks via `$/event` notifications, terminated by final response) |
| `provider/list_models` | daemon -> plugin | List available models |

**Wire format:** `provider/chat` and `provider/chat_streaming` use the **OpenAI chat completion JSON format** as the canonical wire schema (messages array with role/content, tool definitions, tool_choice, etc.). This is the de facto standard — any OpenAI-compatible proxy (LiteLLM, vLLM, Ollama's OpenAI endpoint) can be a provider plugin with minimal glue. The daemon translates between internal `ChatRequest`/`ChatResponse` and the OpenAI JSON schema at the bridge layer.

`provider/info` response (plugin -> daemon):
```json
{
  "provider_name": "deepseek",
  "models": [
    {
      "id": "deepseek-chat",
      "context_window": 64000,
      "supports_tools": true,
      "supports_streaming": true,
      "input_price_per_mtok": 0.14,
      "output_price_per_mtok": 0.28
    }
  ]
}
```

The daemon registers each model in `ModelRegistry` with the plugin provider. `provider/list_models` returns the same `models` array (for refresh).

Streaming: The daemon generates the `request_id` (UUID) and includes it in the `provider/chat_streaming` request params. `provider/chat_streaming` sends a JSON-RPC response immediately with `{ "streaming": true, "request_id": "..." }`. Token chunks arrive as `$/event` notifications: `{ "type": "stream_chunk", "request_id": "...", "delta": { "content": "token" } }`. Final chunk: `{ "type": "stream_done", "request_id": "...", "usage": { "prompt_tokens": N, "completion_tokens": N } }`.

#### Skills (`skill/*`)

| Method | Direction | Purpose |
|---|---|---|
| `skill/info` | daemon -> plugin | Get skill metadata (triggers, routing, capabilities) |
| `skill/invoke` | daemon -> plugin | Invoke the skill with query + context |
| `skill/invoke_continue` | daemon -> plugin | Send tool execution results back to skill (loop) |
| `skill/triggers` | daemon -> plugin | Get trigger patterns for routing |

`skill/info` response (plugin -> daemon):
```json
{
  "id": "code-review",
  "name": "Code Review",
  "description": "Review code for bugs, style, security",
  "invoke": { "mode": "auto", "slash": "/review", "aliases": ["/cr"] },
  "routing": {
    "intent": ["review code", "check for bugs"],
    "keywords": ["review", "bugs", "style"],
    "negative_keywords": ["write", "create"]
  },
  "requires_capabilities": ["file_read"],
  "permissions": { "level": "readonly" }
}
```

This maps directly to `SkillFrontmatter` fields. The `SkillCatalog` creates a `SkillEntry` with `source: Plugin { plugin_id }` and compiles triggers from the `routing.intent` patterns.

`skill/invoke_continue` response uses the same schema as `skill/invoke` response (`result` + optional `tool_calls`).

Skills can request tool calls back to the daemon during invocation. The daemon executes the tool through SecurityGate and returns the result. This makes skills composable — a plugin skill can use any registered tool.

`skill/invoke` request:
```json
{
  "query": "review the auth module",
  "context": { "conversation_id": "...", "user_id": "...", "attachments": [] },
  "available_tools": ["file_read", "web_search"]
}
```

`skill/invoke` response:
```json
{
  "result": "...",
  "tool_calls": [
    { "tool": "file_read", "arguments": { "path": "src/auth.rs" } }
  ]
}
```

If `tool_calls` is present, the daemon executes them through SecurityGate and sends a follow-up `skill/invoke_continue` with tool results:

```json
{
  "tool_results": [
    { "tool": "file_read", "result": "pub fn authenticate(...) { ... }" }
  ]
}
```

This loop continues until the skill returns a response with an empty `tool_calls` array (or omits it). Max iterations enforced by PluginManager (default 20, configurable) to prevent runaway plugins.

#### Agents (`agent/*`)

| Method | Direction | Purpose |
|---|---|---|
| `agent/info` | daemon -> plugin | Get agent metadata (capabilities, constraints, preferred models) |
| `agent/spawn` | daemon -> plugin | Create an agent instance for a task |
| `agent/step` | daemon -> plugin | Poll for progress (health-check fallback) |
| `agent/tool_results` | daemon -> plugin | Send tool execution results back to agent |
| `agent/stop` | daemon -> plugin | Cancel a running agent instance |

Agent plugins manage their own LLM calls and reasoning loop. The daemon dispatches tasks and collects results.

`agent/info` response (plugin -> daemon):
```json
{
  "id": "research-agent",
  "name": "Research Agent",
  "role": "Deep research and analysis",
  "capabilities": ["web_search", "web_fetch", "file_read"],
  "constraints": { "max_concurrent": 1, "singleton": true },
  "preferred_models": ["claude-sonnet-4-5"]
}
```

`agent/spawn` response (plugin -> daemon):
```json
{
  "accepted": true,
  "instance_id": "research-agent::a1b2c3",
  "error": null
}
```

If `accepted` is `false`, the daemon reports the error to the task pipeline and does not proceed with polling/listening. Reasons for rejection: capacity exceeded, invalid instructions, missing dependencies.

`agent/tool_results` request (daemon -> plugin):
```json
{
  "instance_id": "research-agent::a1b2c3",
  "tool_results": [
    { "tool": "web_search", "result": "..." },
    { "tool": "file_read", "error": "permission denied" }
  ]
}
```

`agent/tool_results` response: `{ "received": true }`. The agent continues processing and pushes progress via `$/event`.

**Push-based progress (preferred):** Agent plugins push progress via `$/event` notifications: `{ "type": "agent_progress", "instance_id": "...", "status": "working", "output": "partial..." }`. When the agent needs tools, it pushes `{ "type": "agent_tool_request", "instance_id": "...", "tool_calls": [...] }`. The daemon executes the tools and sends results via `agent/tool_results`. `agent/step` is only used as a health-check fallback if no `$/event` has been received within 30s.

`agent/spawn` request:
```json
{
  "instance_id": "...",
  "task_id": "...",
  "instructions": "...",
  "context": { "conversation_history": [], "workspace_id": "..." }
}
```

`agent/step` response:
```json
{
  "status": "working | tool_request | complete | failed",
  "output": "...",
  "tool_calls": [
    { "tool": "shell_execute", "arguments": { "command": "cargo test" } }
  ]
}
```

When `status` is `tool_request`, the daemon executes the requested tools through SecurityGate and sends another `agent/step` with results.

---

## 2. Plugin Manifest & Packaging

### Manifest Format (`plugin.toml`)

```toml
[plugin]
name = "playwright-browser"
version = "0.1.0"
description = "Browser automation via Playwright"
author = "OpenAlpaca"
license = "MIT"
entry = "./node_modules/.bin/playwright-mcp"
args = ["--headless"]
env = { DISPLAY = ":0" }
max_concurrent = 10   # max in-flight RPC calls (default 10, enforced via Semaphore)

[capabilities]
requires = ["network", "filesystem.read"]
provides = ["browser", "screenshot"]

[types]
tools = true
connector = false
provider = false
skill = false
agent = false

# Connector-specific (only if connector = true)
[connector]
platform = "telegram"
supports_files = true
supports_reactions = true
max_message_length = 4096

# Provider-specific (only if provider = true)
[provider]
supports_streaming = true
supports_tools = true
default_models = ["gpt-4o", "gpt-4o-mini"]
```

A plugin can implement multiple types (e.g., `tools = true` and `skill = true`).

Additional manifest fields:

```toml
# MCP compatibility (default false)
mcp_compatible = false

# Runtime configuration schema — user provides values via CLI or GUI
[config.telegram_token]
type = "secret"
required = true
description = "Telegram bot token from @BotFather"

[config.rate_limit]
type = "number"
required = false
default = 30
description = "Max messages per minute"

[config.webhook_url]
type = "string"
required = false
description = "Optional webhook endpoint"
```

User-provided config values stored in `~/.openalpaca/plugins/.config/<plugin_name>.toml`. Passed to the plugin in the `initialize` RPC payload. CLI: `openalpaca plugin config <name> set <key> <value>` and `openalpaca plugin config <name> get <key>`.

### Directory Layout

```
~/.openalpaca/plugins/
├── playwright-browser/
│   ├── plugin.toml
│   ├── node_modules/...
│   └── README.md
├── whatsapp-connector/
│   ├── plugin.toml
│   └── whatsapp-bridge
└── deepseek-provider/
    ├── plugin.toml
    └── main.py
```

Language-agnostic: `entry` is any executable (Rust binary, Python script, Node module, shell script).

### Permission Persistence

```
~/.openalpaca/plugins/.permissions.toml
```

```toml
[playwright-browser]
approved = true
approved_at = "2026-03-29T12:00:00Z"
capabilities = ["network", "filesystem.read"]

[whatsapp-connector]
approved = false
```

---

## 3. PluginManager Architecture

### New Crate: `openalpaca_plugins`

Dependencies: `openalpaca_api`, `openalpaca_core`, `openalpaca_llm`, `openalpaca_connectors`, `tokio`, `serde_json`, `notify` (fs watcher). Depends on core for registry types (`ToolRegistry`, `SkillCatalog`, `AgentRegistry`, `EventBus`) — one-way dependency, no circular imports.

### Components

```
PluginManager
├── Discovery       — watches plugin directory, detects add/remove/change
├── ProcessPool     — spawns/kills child processes, manages stdio pipes
├── HealthMonitor   — periodic heartbeats, crash detection, auto-restart
├── PermissionGate  — first-load approval flow, capability enforcement
├── RegistryBridge  — registers plugin tools/connectors/providers/skills/agents into existing registries
└── EventRelay      — routes $/event notifications from plugins to EventBus
```

### Core Types

```rust
pub struct PluginManager {
    plugins: Arc<RwLock<HashMap<String, PluginState>>>,
    process_pool: ProcessPool,
    health_monitor: HealthMonitor,
    permission_gate: PermissionGate,
    event_tx: broadcast::Sender<PluginEvent>,
    plugin_dir: PathBuf,
    watcher: Option<RecommendedWatcher>,
}

pub struct PluginState {
    pub manifest: PluginManifest,
    pub status: PluginStatus,
    pub process: Option<ChildHandle>,
    pub registered_tools: Vec<String>,         // namespaced tool names
    pub registered_connector: Option<String>,  // connector platform name
    pub registered_provider: Option<String>,   // provider name
    pub registered_models: Vec<String>,        // model IDs from provider
    pub registered_skills: Vec<String>,        // skill IDs
    pub registered_agents: Vec<String>,        // agent template IDs
    pub restart_count: u32,
    pub last_health: Option<Instant>,
}

pub enum PluginStatus {
    Loading,
    WaitingApproval,
    NeedsConfig { missing_keys: Vec<String> },  // required config not yet provided
    Running,
    Crashed { error: String, backoff_until: Instant },
    Disabled,
    Stopped,
}
```

### Hot-Load Sequence (new plugin directory detected)

1. Parse `plugin.toml` -> check `.permissions.toml`
2. If never seen: set `WaitingApproval`, emit `PluginPendingApproval`, notify user. Stop here until approved.
3. **Config validation:** Check all `required = true` config fields in manifest against `.config/<plugin_name>.toml`. If any are missing, set status `NeedsConfig { missing_keys }`, emit event, notify user: *"Plugin 'X' needs configuration. Run `openalpaca plugin config X set <key> <value>`"*. Stop here until config is provided.
4. **Spawn:** Start child process with CWD set to the plugin's own directory (`~/.openalpaca/plugins/<name>/`). This makes relative paths in `entry` resolve naturally. Concurrency gated by `max_concurrent` Semaphore (from manifest, default 10).
5. Send `initialize` RPC with plugin ID, granted capabilities, user config, daemon version. Wait for `{ ready: true }`.
6. Discover capabilities: call `tools/list`, `connector/info`, `provider/info`, `skill/info`, `agent/info` based on manifest `[types]`
7. Register into existing registries (ToolRegistry, ConnectorManager, LlmRouter, SkillCatalog, AgentRegistry). Track all registered component IDs in `PluginState`.
8. Set status `Running`, emit `PluginLoaded`, start health monitoring

### Hot-Unload Sequence (directory removed or `plugin disable`)

1. Stop routing new calls to this plugin
2. Wait up to 5s for in-flight calls to complete
3. Send `shutdown` RPC, wait 3s, then SIGKILL if still alive
4. Remove tools/connectors/providers/skills/agents from registries
5. Emit `PluginUnloaded` event

### Crash Recovery

1. Process exits unexpectedly -> set status `Crashed`
2. Exponential backoff: 1s, 2s, 4s, 8s, max 60s
3. After 5 consecutive crashes -> set `Disabled`, emit `PluginDisabled`, notify user
4. User re-enables via `openalpaca plugin enable <name>`

### Error Propagation

When a JSON-RPC call to a plugin fails, the daemon maps it to standard error codes:

| Code | Meaning | Daemon behavior |
|---|---|---|
| `-32001` | Plugin timeout | Configurable per-plugin (default 60s for tools, 300s for agents). Returns error to agentic loop. |
| `-32002` | Plugin crashed during execution | Triggers crash recovery. In-flight call returns error. |
| `-32003` | Permission denied | Capability not granted. Logged + returned as tool error. |
| `-32004` | Plugin unavailable | Plugin disabled/stopped/loading. Returned as tool error. |
| `-32600` | Malformed response | Plugin returned invalid JSON-RPC. Logged, returned as tool error. |

All errors propagate as `Result<String, String>` through the existing tool execution pipeline. The agentic loop already handles tool errors gracefully (reports to LLM, continues reasoning).

### Plugin Version Upgrades

When the filesystem watcher detects changes to `plugin.toml` in an existing plugin directory:

1. Hot-unload the old version (drain in-flight calls, kill process, unregister)
2. Hot-load the new version (parse manifest, spawn, register)
3. If the new version requires additional capabilities not previously approved, set `WaitingApproval` and notify user

Plugins are stateless from the daemon's perspective. If a plugin needs to persist state, it manages its own files within its plugin directory.

---

## 4. Registry Bridge

Existing registries don't know about plugins. The PluginManager bridges them via proxy types.

### Tool Bridge

New `ToolBackend` variant:

```rust
pub enum ToolBackend {
    BuiltIn(Arc<dyn BuiltInTool>),
    Http(HttpToolConfig),
    Command(CommandToolConfig),
    Plugin(PluginToolProxy),        // NEW
}
```

`PluginToolProxy` holds a reference to the plugin's stdio channel and executes tools via `tools/call` JSON-RPC.

**Tool namespacing — consistency across 4 layers:**

The namespaced name `{plugin_name}::{tool_name}` is used everywhere consistently:
1. **Registration key** in ToolRegistry: `"playwright::browser_click"`
2. **`ToolDefinition.name`** sent to LLM: `"playwright::browser_click"`
3. **LLM tool call name** echoed back: `"playwright::browser_click"`
4. **Registry lookup** at execution time: exact match on `"playwright::browser_click"`

The PluginManager wraps each tool from `tools/list` by prefixing the plugin name to the tool's `name` field before inserting into ToolRegistry. When `tools/call` is dispatched to the plugin, the prefix is stripped — the plugin sees the original bare name.

On plugin load: call `tools/list`, prefix each tool name, wrap as `RegisteredTool { backend: Plugin(proxy) }`, insert into ToolRegistry.

On plugin unload: remove those entries from ToolRegistry (using tracked `registered_tools` names from PluginState).

### Connector Bridge

`PluginConnector` implements the existing `Connector` trait:

```rust
pub struct PluginConnector {
    plugin_id: String,
    stdio_channel: Arc<StdioChannel>,
    bus: Arc<EventBus>,
}

#[async_trait]
impl Connector for PluginConnector {
    fn name(&self) -> &str { &self.plugin_id }
    async fn run(&self) -> Result<(), ConnectorError> {
        // Send connector/start
        // Listen for $/event notifications -> publish UserRequest on EventBus
    }
    async fn shutdown(&self) -> Result<(), ConnectorError> {
        // Send connector/stop
    }
}
```

Inbound: plugin process -> `$/event` -> EventRelay -> EventBus -> Orchestrator.
Outbound: Orchestrator -> `send` tool -> ConnectorManager -> `PluginConnector` -> `connector/send` RPC.

### Provider Bridge

`PluginLlmProvider` implements the existing `LlmProvider` trait:

```rust
pub struct PluginLlmProvider {
    plugin_id: String,
    stdio_channel: Arc<StdioChannel>,
}

#[async_trait]
impl LlmProvider for PluginLlmProvider {
    fn name(&self) -> &str { &self.plugin_id }
    fn supports_tools(&self) -> bool { /* from provider/info */ }
    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        // Serialize request -> provider/chat RPC -> deserialize response
    }
    async fn chat_streaming(&self, request: ChatRequest) -> Result<ChatStream, LlmError> {
        // provider/chat_streaming -> collect $/event notifications into stream
    }
}
```

Models registered in ModelRegistry via metadata from `provider/info`.

### Skill Bridge

`PluginSkillEntry` replaces a SKILL.md-backed SkillEntry for plugin skills:

- `skill/info` provides the equivalent of SKILL.md frontmatter (triggers, routing, capabilities)
- `skill/invoke` replaces the agentic loop execution — the plugin handles its own logic
- Tool callbacks during invocation are proxied through SecurityGate

Same routing/scoring algorithm applies — plugin skills compete with directory-scanned skills on equal footing.

### Agent Bridge

`PluginAgentTemplate` replaces a `.md`/`.toml`-backed AgentTemplate:

- `agent/info` provides the equivalent of agent frontmatter (capabilities, constraints)
- `agent/spawn` + `agent/step` replace the internal agentic loop — the plugin runs its own
- Tool requests during execution are proxied through SecurityGate

TaskDispatcher can assign tasks to plugin agents the same way it assigns to built-in agents.

### What Stays the Same

- ToolRegistry API (agents still call `resolve_agent_tools`)
- ConnectorManager API (still manages connector lifecycle)
- LlmRouter API (still routes by model ID)
- SecurityGate (still wraps all tool execution — plugin tools go through SandboxManager)
- SkillCatalog routing (same scoring algorithm)
- AgentRegistry dispatch (same task assignment)
- Existing builtins, compiled connectors, compiled providers all keep working alongside plugins

---

## 5. CLI & User Interaction

### Commands

| Command | What it does |
|---|---|
| `plugin list` | Show all plugins: name, version, status, type(s) |
| `plugin install <path-or-url>` | Copy directory into `~/.openalpaca/plugins/`, trigger discovery |
| `plugin remove <name>` | Graceful unload + delete directory |
| `plugin enable <name>` | Re-enable a disabled/crashed plugin |
| `plugin disable <name>` | Graceful unload, mark disabled (keeps files) |
| `plugin approve <name>` | Approve pending capabilities for first-load plugin |
| `plugin deny <name>` | Deny a pending plugin |
| `plugin info <name>` | Show manifest, capabilities, registered components, health stats |
| `plugin logs <name>` | Tail the plugin's stderr output |
| `plugin config <name> set <key> <value>` | Set a runtime config value for a plugin |
| `plugin config <name> get [key]` | Show plugin config (all keys or specific key) |

### First-Load Approval Flow

1. New plugin directory appears (or `plugin install`)
2. Daemon parses manifest, sets status `WaitingApproval`
3. Emits `PluginPendingApproval` event — GUI shows notification, active connectors message user
4. User runs `plugin approve <name>` (CLI) or clicks approve (GUI)
5. Plugin spawns and registers

### GUI Integration

New **Plugins panel** in the Tauri GUI (alongside Tasks, Agents, Events):
- List of plugins with status badges
- Approve/deny buttons for pending plugins
- Enable/disable toggle
- Health indicator (green/yellow/red)
- Expandable detail: registered tools, connector platform, provider models

### EventBus Events

```rust
pub enum SystemEvent {
    // ... existing variants ...
    PluginLoaded { plugin_id: String, tools: Vec<String> },
    PluginUnloaded { plugin_id: String },
    PluginCrashed { plugin_id: String, error: String, restart_in_secs: u64 },
    PluginDisabled { plugin_id: String, reason: String },
    PluginPendingApproval { plugin_id: String, capabilities: Vec<String> },
    PluginNeedsConfig { plugin_id: String, missing_keys: Vec<String> },
}
```

---

## 6. Agent Awareness of Plugins

Agents need to know what plugins are available and what they can do. Without this, agents can't reason about when to use plugin tools vs builtins, or which connector to send through, or what models are available from plugin providers.

### Tool Discovery

Plugin tools are registered in ToolRegistry with namespace prefixes (`playwright::browser_navigate`). The existing capability-based resolution (`resolve_agent_tools`) works as-is — plugin tools declare `provides_capabilities` in the manifest, agents request capabilities, and the registry matches them.

**What changes:** The system prompt builder (`PromptBuilder`) already includes available tools in the agent's context. Plugin tools appear alongside builtins — the agent sees `playwright::browser_click` just like it sees `file_read`. No special prompting needed.

### Plugin Metadata in System Prompt

When plugins are loaded, the Orchestrator injects a **plugin summary** into the system prompt context:

```
## Available Plugins
- **playwright-browser** (tools): browser_navigate, browser_click, browser_type, browser_screenshot
- **whatsapp-connector** (connector): WhatsApp messaging via plugin
- **deepseek-provider** (provider): models deepseek-chat, deepseek-coder
```

This is generated from PluginManager's state and included in the PromptBuilder's context assembly. Updated whenever plugins load/unload (hot-reload triggers prompt rebuild on next turn).

### Connector Awareness

When the `send` tool is invoked, the agent needs to know which connectors are available (both compiled-in and plugin). The `send` tool already resolves by connector name — plugin connectors register under their platform name (e.g., `whatsapp`), so `send(connector: "whatsapp", ...)` routes to the plugin connector automatically.

The agent's tool description for `send` is dynamically generated to list available connectors:
```
send: Send a message to a user via a connected platform.
  connector: one of "telegram", "discord", "imessage", "whatsapp" (plugin)
```

### Model Awareness

Plugin provider models are registered in ModelRegistry. The agent doesn't select models directly — the LlmRouter handles routing. But if an agent's `AgentLlmConfig` specifies a model provided by a plugin (`model: "deepseek-chat"`), the router resolves it to the plugin provider. This already works via the existing model registry lookup path.

### Skill Awareness

Plugin skills participate in the same routing/scoring as file-based skills. The SkillRouter's `route()` method queries all skills (both file-based and plugin-backed) — no change needed. If a plugin skill wins the routing score, the Orchestrator dispatches to it via `skill/invoke` instead of the internal agentic loop.

### Agent Delegation

When the TaskDispatcher assigns a task to a plugin-backed agent, it:
1. Detects the agent template has `source: Plugin { plugin_id, ... }`
2. Instead of calling `run_agentic_loop`, calls `agent/spawn` on the plugin
3. Listens for `$/event` progress notifications (or falls back to `agent/step` polling)
4. Proxies any tool requests through SecurityGate
5. Collects the final output and returns it to the task pipeline

The Orchestrator's planner doesn't need to know whether an agent is built-in or plugin-backed — the dispatch layer handles the routing transparently.

### Dynamic Updates

When a plugin loads or unloads at runtime:
1. ToolRegistry is updated (tools added/removed)
2. SkillCatalog is updated (skills added/removed)
3. AgentRegistry is updated (agent templates added/removed)
4. LlmRouter/ModelRegistry is updated (models added/removed)
5. ConnectorManager is updated (connectors added/removed)
6. `PluginLoaded`/`PluginUnloaded` event emitted on EventBus
7. Next conversation turn picks up the new state automatically (prompt rebuilt with current tools/plugins)

Active conversations are not disrupted — they continue with the tools they had. New turns get the updated tool set.

---

## 7. Dependency Graph (Updated)

### Avoiding Circular Dependencies

The key constraint: `openalpaca_core` defines `ToolBackend`, `SkillEntry`, and `AgentTemplate` — but the `Plugin(...)` variants need types from `openalpaca_plugins` which depends on `openalpaca_core`. This would create a cycle.

**Solution: trait objects in core, concrete types in plugins.**

- `openalpaca_core` defines `ToolBackend::Plugin(Arc<dyn PluginToolExecutor>)` where `PluginToolExecutor` is a trait in core with `async fn execute(&self, tool_name: &str, args: Value) -> Result<String, String>`.
- `openalpaca_plugins` implements `PluginToolProxy: PluginToolExecutor` — the concrete type that holds `StdioChannel` and calls `tools/call`.
- Same pattern for skills: `SkillEntry` gets `source: SkillSource` where `SkillSource::Plugin(Arc<dyn PluginSkillExecutor>)`. Trait in core, impl in plugins.
- Same for agents: `AgentTemplate` gets `source: AgentSource` where `AgentSource::Plugin(Arc<dyn PluginAgentExecutor>)`.
- `PluginConnector` and `PluginLlmProvider` already implement traits defined in their respective crates (`Connector` and `LlmProvider`), so no cycle issue there.

This keeps the dependency graph as a DAG:

```
openalpaca_api              (leaf — event types, PluginToolExecutor/PluginSkillExecutor/
                             PluginAgentExecutor traits defined here for max reuse)

openalpaca_llm              (leaf — LlmProvider trait, ProviderType changes)
openalpaca_storage          (leaf)

openalpaca_connectors       (depends: openalpaca_api, openalpaca_core)
                            Changes: Connector::name() -> &str, Plugin variant on ConnectorHandle

openalpaca_core             (depends: openalpaca_api, openalpaca_llm, openalpaca_storage)
                            Changes: ToolBackend::Plugin(Arc<dyn PluginToolExecutor>),
                            SkillSource::Plugin(Arc<dyn PluginSkillExecutor>),
                            AgentSource::Plugin(Arc<dyn PluginAgentExecutor>),
                            ToolRegistry backed by DashMap for runtime add/remove,
                            SkillEntry.source: SkillSource (paths become Optional),
                            TaskDispatcher plugin agent dispatch path

openalpaca_plugins          (NEW — depends: openalpaca_api, openalpaca_core, openalpaca_llm,
                             openalpaca_connectors, tokio, serde_json, notify)
                            Contains: PluginManager, ProcessPool, HealthMonitor, PermissionGate,
                            StdioChannel, PluginToolProxy (impl PluginToolExecutor),
                            PluginConnector (impl Connector), PluginLlmProvider (impl LlmProvider),
                            PluginSkillEntry (impl PluginSkillExecutor),
                            PluginAgentTemplate (impl PluginAgentExecutor), EventRelay

openalpacad                 (depends: all — wire PluginManager into daemon startup, add plugin routes)
openalpaca (CLI)            (depends: minimal — add plugin subcommands, HTTP client only)
openalpaca-gui              (depends: openalpaca_api — add Plugins panel, plugin event types)
```

### Crate Change Summary

| Crate | Changes |
|---|---|
| `openalpaca_api` | Add `PluginToolExecutor`, `PluginSkillExecutor`, `PluginAgentExecutor` traits. Add plugin `SystemEvent` variants. |
| `openalpaca_core` | `ToolBackend::Plugin(Arc<dyn PluginToolExecutor>)`. `ToolRegistry`: replace `HashMap` with `DashMap`, `register(&self)` instead of `(&mut self)`, add `remove(&self, name)`. `SkillEntry`: add `source: SkillSource` enum, make `skill_md_path`/`skill_dir` `Option<PathBuf>`. `AgentTemplate`: add `source: AgentSource` enum. `TaskDispatcher`: detect plugin agents, delegate to `PluginAgentExecutor::spawn/step`. |
| `openalpaca_llm` | Change `ProviderType` from closed enum to `String`-keyed (remove `Copy` derive, audit all match sites). Add `ModelRegistry::remove()`, `LlmRouter::deregister_provider()`. |
| `openalpaca_connectors` | Change `Connector::name() -> &str` (update 3 compiled connectors — trivial, just change return type). Add `ConnectorHandle::Plugin(CancellationToken, Arc<AtomicBool>)` with `is_alive()`/`shutdown()` match arms. |
| `openalpaca_plugins` | Entire new crate (see Components in Section 3). |
| `openalpacad` | Initialize `PluginManager` after services, wire to `AppState`, add `/plugins/*` routes. |
| `openalpaca` (CLI) | Add `commands/plugin.rs` with 11 subcommands. |
| `openalpaca-gui` | Add Plugins panel, plugin store, plugin event types in `daemon.ts`. |

---

## 8. Out of Scope (v1)

- Archive distribution (`plugin install <url>` downloads a .tar.gz) — deferred, directory convention for now
- Plugin registry (central catalog of community plugins) — future
- Plugin sandboxing beyond process isolation (no seccomp/AppArmor) — future
- Plugin-to-plugin communication — future
- Plugin auto-update — future
