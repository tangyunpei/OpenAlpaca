# Plugin SDK Phase 1: Foundation + Tool Plugins

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the plugin infrastructure and get tool plugins working end-to-end — from manifest parsing to MCP tool execution — so any existing MCP server can be dropped into `~/.openalpaca/plugins/` and used by agents.

**Architecture:** New `openalpaca_plugins` crate containing PluginManager (discovery, process lifecycle, health, permissions). Prep refactors make ToolRegistry mutable at runtime. Plugin executor traits live in `openalpaca_api` to avoid circular deps. StdioChannel multiplexes JSON-RPC over a single stdio pipe per plugin. Tool bridge registers plugin tools into existing ToolRegistry with namespace prefixes.

**Tech Stack:** Rust, tokio (process spawning, channels, fs watcher), serde_json (JSON-RPC), notify (filesystem watcher), DashMap (concurrent registry)

**Spec:** `docs/superpowers/specs/2026-03-29-plugin-sdk-design.md`

---

## File Map

### New Files

| File | Responsibility |
|---|---|
| `crates/openalpaca_plugins/Cargo.toml` | Crate manifest |
| `crates/openalpaca_plugins/src/lib.rs` | Public API: PluginManager, PluginStatus, PluginManifest |
| `crates/openalpaca_plugins/src/manifest.rs` | Parse `plugin.toml` + config schema |
| `crates/openalpaca_plugins/src/stdio_channel.rs` | JSON-RPC multiplexing over stdio |
| `crates/openalpaca_plugins/src/process_pool.rs` | Spawn/kill child processes |
| `crates/openalpaca_plugins/src/discovery.rs` | Watch plugin directory for changes |
| `crates/openalpaca_plugins/src/permission_gate.rs` | First-load approval, `.permissions.toml` persistence |
| `crates/openalpaca_plugins/src/health_monitor.rs` | Periodic heartbeats, crash recovery |
| `crates/openalpaca_plugins/src/event_relay.rs` | Route `$/event` notifications to EventBus |
| `crates/openalpaca_plugins/src/bridge/mod.rs` | Registry bridge module |
| `crates/openalpaca_plugins/src/bridge/tool_bridge.rs` | PluginToolProxy implementing PluginToolExecutor |
| `crates/openalpaca_plugins/src/error.rs` | PluginError enum |
| `apps/openalpacad/src/routes/plugins.rs` | Plugin HTTP API endpoints |
| `apps/openalpaca/src/commands/plugin.rs` | CLI plugin subcommands |
| `tests/fixtures/echo-plugin/plugin.toml` | Test fixture: minimal tool plugin manifest |
| `tests/fixtures/echo-plugin/echo-server.sh` | Test fixture: bash MCP server that echoes tool calls |

### Modified Files

| File | Change |
|---|---|
| `Cargo.toml` (root) | Add `openalpaca_plugins` to workspace members + deps |
| `crates/openalpaca_api/src/lib.rs` | Add `pub mod plugin_traits;` |
| `crates/openalpaca_api/src/plugin_traits.rs` | NEW: PluginToolExecutor trait |
| `crates/openalpaca_api/src/events/mod.rs` | Add 6 plugin SystemEvent variants (after line 235) |
| `crates/openalpaca_core/src/tools/registry/mod.rs` | ToolBackend::Plugin variant, DashMap migration, register(&self), remove() |
| `apps/openalpacad/src/services/mod.rs` | Add PluginManager to InitializedServices |
| `apps/openalpacad/src/services/tools.rs` | Adapt to DashMap-backed ToolRegistry |
| `apps/openalpacad/src/state.rs` | Add `plugin_manager: Option<Arc<PluginManager>>` to AppState |
| `apps/openalpacad/src/main.rs` | Initialize PluginManager after services, pass to AppState |
| `apps/openalpacad/src/router.rs` | Add `/v1/plugins/*` routes |
| `apps/openalpacad/src/routes/mod.rs` | Add `pub mod plugins;` |
| `apps/openalpaca/src/main.rs` | Add `Plugin` variant to Commands enum |
| `apps/openalpaca/src/commands/mod.rs` | Add `pub mod plugin;` |

---

## Chunk 1: Prep Refactors (unblock plugin infrastructure)

### Task 1: Add PluginToolExecutor trait to openalpaca_api

This trait lives in the leaf crate so both `openalpaca_core` (consumer) and `openalpaca_plugins` (implementor) can depend on it without circular imports.

**Files:**
- Create: `crates/openalpaca_api/src/plugin_traits.rs`
- Modify: `crates/openalpaca_api/src/lib.rs`

- [ ] **Step 1: Create the trait file**

```rust
// crates/openalpaca_api/src/plugin_traits.rs
use async_trait::async_trait;
use serde_json::Value;

/// Trait for executing tools via a plugin subprocess.
/// Defined in openalpaca_api to avoid circular dependencies:
/// openalpaca_core uses it in ToolBackend::Plugin, openalpaca_plugins implements it.
#[async_trait]
pub trait PluginToolExecutor: Send + Sync {
    /// Execute a tool by name with JSON arguments.
    /// The tool_name is the bare name (without plugin namespace prefix).
    async fn execute(&self, tool_name: &str, arguments: &Value) -> Result<String, String>;

    /// The plugin this executor belongs to.
    fn plugin_id(&self) -> &str;
}
```

- [ ] **Step 2: Export from lib.rs**

In `crates/openalpaca_api/src/lib.rs`, add:
```rust
pub mod plugin_traits;
```

- [ ] **Step 3: Add async-trait dep to openalpaca_api**

In `crates/openalpaca_api/Cargo.toml`, add under `[dependencies]`:
```toml
async-trait = { workspace = true }
serde_json = { workspace = true }
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo check -p openalpaca_api`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/openalpaca_api/
git commit -m "feat: add PluginToolExecutor trait to openalpaca_api

Trait for executing tools via plugin subprocesses. Defined in the leaf
crate to avoid circular deps between openalpaca_core and openalpaca_plugins."
```

---

### Task 2: Add plugin SystemEvent variants to openalpaca_api

**Files:**
- Modify: `crates/openalpaca_api/src/events/mod.rs`

- [ ] **Step 1: Add 6 plugin event variants**

In `crates/openalpaca_api/src/events/mod.rs`, after the last variant `SoulUpdated` (line ~235), add:

```rust
    PluginLoaded {
        plugin_id: String,
        tools: Vec<String>,
    },
    PluginUnloaded {
        plugin_id: String,
    },
    PluginCrashed {
        plugin_id: String,
        error: String,
        restart_in_secs: u64,
    },
    PluginDisabled {
        plugin_id: String,
        reason: String,
    },
    PluginPendingApproval {
        plugin_id: String,
        capabilities: Vec<String>,
    },
    PluginNeedsConfig {
        plugin_id: String,
        missing_keys: Vec<String>,
    },
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p openalpaca_api`
Expected: PASS

- [ ] **Step 3: Check for exhaustive match arms**

Run: `cargo check --all-targets 2>&1 | grep "non-exhaustive"`

If any exhaustive matches on `ServerEvent` exist elsewhere, add `_ => {}` or named arms for the new variants. The event bridge in `apps/openalpacad/src/event_bridge.rs` likely needs updating.

- [ ] **Step 4: Fix any exhaustive match sites**

For each match site found, add arms that forward plugin events to the WebSocket broadcaster (same pattern as existing variants).

- [ ] **Step 5: Commit**

```bash
git add crates/openalpaca_api/ apps/openalpacad/src/event_bridge.rs
git commit -m "feat: add plugin lifecycle events to ServerEvent

Six new variants: PluginLoaded, PluginUnloaded, PluginCrashed,
PluginDisabled, PluginPendingApproval, PluginNeedsConfig."
```

---

### Task 3: Migrate ToolRegistry from HashMap to DashMap

This is the critical blocker — ToolRegistry must support runtime add/remove for plugin tools.

**Files:**
- Modify: `crates/openalpaca_core/src/tools/registry/mod.rs`
- Modify: `crates/openalpaca_core/Cargo.toml`

- [ ] **Step 1: Add DashMap dependency**

In `Cargo.toml` (root workspace), add to `[workspace.dependencies]`:
```toml
dashmap = "6"
```

In `crates/openalpaca_core/Cargo.toml`, add:
```toml
dashmap = { workspace = true }
```

- [ ] **Step 2: Replace HashMap with DashMap in ToolRegistry**

In `crates/openalpaca_core/src/tools/registry/mod.rs`:

Replace the struct (lines 64–67):
```rust
// BEFORE:
pub struct ToolRegistry {
    tools: HashMap<String, RegisteredTool>,
    http_client: reqwest::Client,
}

// AFTER:
pub struct ToolRegistry {
    tools: dashmap::DashMap<String, RegisteredTool>,
    http_client: reqwest::Client,
}
```

- [ ] **Step 3: Update `new()` method**

```rust
// BEFORE:
pub fn new() -> Self {
    Self {
        tools: HashMap::new(),
        http_client: reqwest::Client::new(),
    }
}

// AFTER:
pub fn new() -> Self {
    Self {
        tools: dashmap::DashMap::new(),
        http_client: reqwest::Client::new(),
    }
}
```

- [ ] **Step 4: Change `register()` from `&mut self` to `&self`**

```rust
// BEFORE:
pub fn register(&mut self, tool: RegisteredTool) {

// AFTER:
pub fn register(&self, tool: RegisteredTool) {
```

Update the body: `self.tools.insert(tool.definition.name.clone(), tool);`

- [ ] **Step 5: Add `remove()` method**

After `register()`, add:
```rust
/// Remove a tool by name. Returns true if the tool existed.
pub fn remove(&self, name: &str) -> bool {
    self.tools.remove(name).is_some()
}
```

- [ ] **Step 6: Update all read methods to use DashMap API**

DashMap's `.get()` returns `Option<Ref<K,V>>` not `Option<&V>`. Update methods:

- `get()`: `self.tools.get(name).map(|r| r.value().clone())` or return the Ref
- `execute()` / `execute_with_context()`: use `self.tools.get(name)` with Ref guard
- `tools_for_capabilities()` / `tools_for_capabilities_with_deny()`: iterate with `self.tools.iter()`
- `all_definitions()`: iterate with `self.tools.iter()`
- `command_backend_tool_names()`: iterate with `self.tools.iter()`

- [ ] **Step 7: Update Clone impl**

```rust
impl Clone for ToolRegistry {
    fn clone(&self) -> Self {
        let new_tools = dashmap::DashMap::new();
        for entry in self.tools.iter() {
            new_tools.insert(entry.key().clone(), entry.value().clone());
        }
        Self {
            tools: new_tools,
            http_client: self.http_client.clone(),
        }
    }
}
```

- [ ] **Step 8: Add ToolBackend::Plugin variant**

Add to the `ToolBackend` enum (after `Command`):
```rust
    Plugin(Arc<dyn openalpaca_api::plugin_traits::PluginToolExecutor>),
```

Add `openalpaca_api` to `openalpaca_core`'s Cargo.toml if not already present.

- [ ] **Step 9: Handle Plugin variant in execute() match arms**

In `execute()`:
```rust
ToolBackend::Plugin(ref executor) => {
    executor.execute(&tool.definition.name, arguments).await
}
```

In `execute_with_context()`:
```rust
ToolBackend::Plugin(ref executor) => {
    executor.execute(&tool.definition.name, arguments).await
}
```

In `command_backend_tool_names()`:
```rust
ToolBackend::Plugin(_) => None,
```

- [ ] **Step 10: Update services/tools.rs**

In `apps/openalpacad/src/services/tools.rs`, the `build_tool_registry()` function calls `registry.register()` with `&mut registry`. Since `register()` is now `&self`, remove any `mut` qualifiers:

```rust
// BEFORE:
let mut tool_registry = ToolRegistry::new();

// AFTER:
let tool_registry = ToolRegistry::new();
```

- [ ] **Step 11: Verify full workspace compiles**

Run: `cargo check --all-targets`
Expected: PASS with no new warnings

- [ ] **Step 12: Run existing tests**

Run: `cargo test -p openalpaca_core -- tools`
Expected: All existing tool tests pass

- [ ] **Step 13: Commit**

```bash
git add Cargo.toml crates/openalpaca_core/ apps/openalpacad/src/services/tools.rs
git commit -m "refactor: migrate ToolRegistry to DashMap for runtime add/remove

ToolRegistry now uses DashMap instead of HashMap, enabling runtime
tool registration and removal for plugin support. register() takes
&self instead of &mut self. Added remove() method and Plugin variant
to ToolBackend."
```

---

## Chunk 2: New openalpaca_plugins Crate — Core Infrastructure

### Task 4: Create crate skeleton + manifest parsing

**Files:**
- Create: `crates/openalpaca_plugins/Cargo.toml`
- Create: `crates/openalpaca_plugins/src/lib.rs`
- Create: `crates/openalpaca_plugins/src/manifest.rs`
- Create: `crates/openalpaca_plugins/src/error.rs`
- Modify: `Cargo.toml` (root)

- [ ] **Step 1: Create crate directory**

```bash
mkdir -p crates/openalpaca_plugins/src
```

- [ ] **Step 2: Write Cargo.toml**

```toml
[package]
name = "openalpaca_plugins"
version = "0.1.0"
edition = "2024"

[dependencies]
openalpaca_api = { workspace = true }
openalpaca_core = { workspace = true }
openalpaca_llm = { workspace = true }
tokio = { workspace = true, features = ["process", "io-util", "sync", "time", "fs"] }
serde = { workspace = true, features = ["derive"] }
serde_json = { workspace = true }
async-trait = { workspace = true }
tracing = { workspace = true }
thiserror = { workspace = true }
toml = { workspace = true }
notify = { workspace = true }
uuid = { workspace = true }
chrono = { workspace = true }
```

- [ ] **Step 3: Write error.rs**

```rust
// crates/openalpaca_plugins/src/error.rs
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PluginError {
    #[error("manifest not found: {0}")]
    ManifestNotFound(String),
    #[error("invalid manifest: {0}")]
    InvalidManifest(String),
    #[error("process spawn failed: {0}")]
    SpawnFailed(String),
    #[error("process crashed")]
    ProcessCrashed,
    #[error("timeout waiting for plugin response")]
    Timeout,
    #[error("plugin returned error: {code} {message}")]
    RpcError { code: i64, message: String },
    #[error("channel closed")]
    ChannelClosed,
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("plugin unavailable: {0}")]
    Unavailable(String),
    #[error("config missing required keys: {0:?}")]
    MissingConfig(Vec<String>),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
}
```

- [ ] **Step 4: Write manifest.rs**

```rust
// crates/openalpaca_plugins/src/manifest.rs
use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::error::PluginError;

#[derive(Debug, Clone, Deserialize)]
pub struct PluginManifest {
    pub plugin: PluginMeta,
    #[serde(default)]
    pub capabilities: CapabilitiesSection,
    #[serde(default)]
    pub types: TypesSection,
    #[serde(default)]
    pub config: HashMap<String, ConfigField>,
    #[serde(default)]
    pub connector: Option<ConnectorMeta>,
    #[serde(default)]
    pub provider: Option<ProviderMeta>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PluginMeta {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub license: String,
    pub entry: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent: usize,
    #[serde(default)]
    pub mcp_compatible: bool,
}

fn default_max_concurrent() -> usize { 10 }

#[derive(Debug, Clone, Default, Deserialize)]
pub struct CapabilitiesSection {
    #[serde(default)]
    pub requires: Vec<String>,
    #[serde(default)]
    pub provides: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TypesSection {
    #[serde(default)]
    pub tools: bool,
    #[serde(default)]
    pub connector: bool,
    #[serde(default)]
    pub provider: bool,
    #[serde(default)]
    pub skill: bool,
    #[serde(default)]
    pub agent: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ConfigField {
    #[serde(rename = "type")]
    pub field_type: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub default: Option<toml::Value>,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ConnectorMeta {
    pub platform: String,
    #[serde(default)]
    pub supports_files: bool,
    #[serde(default)]
    pub supports_reactions: bool,
    #[serde(default = "default_max_message_length")]
    pub max_message_length: usize,
}

fn default_max_message_length() -> usize { 4096 }

#[derive(Debug, Clone, Deserialize)]
pub struct ProviderMeta {
    #[serde(default)]
    pub supports_streaming: bool,
    #[serde(default)]
    pub supports_tools: bool,
    #[serde(default)]
    pub default_models: Vec<String>,
}

impl PluginManifest {
    /// Parse a plugin.toml file from a plugin directory.
    pub fn from_dir(plugin_dir: &Path) -> Result<Self, PluginError> {
        let manifest_path = plugin_dir.join("plugin.toml");
        if !manifest_path.exists() {
            return Err(PluginError::ManifestNotFound(
                manifest_path.display().to_string(),
            ));
        }
        let content = std::fs::read_to_string(&manifest_path)
            .map_err(|e| PluginError::InvalidManifest(e.to_string()))?;
        let manifest: PluginManifest = toml::from_str(&content)
            .map_err(|e| PluginError::InvalidManifest(e.to_string()))?;
        Ok(manifest)
    }

    /// Return all required config keys that are not yet set.
    pub fn missing_config_keys(&self, provided: &HashMap<String, toml::Value>) -> Vec<String> {
        self.config
            .iter()
            .filter(|(_, field)| field.required && !provided.contains_key(field.description.as_str()))
            .filter(|(key, _)| !provided.contains_key(key.as_str()))
            .map(|(key, _)| key.clone())
            .collect()
    }

    /// Resolve the entry command as an absolute path relative to plugin_dir.
    pub fn entry_path(&self, plugin_dir: &Path) -> PathBuf {
        let entry = Path::new(&self.plugin.entry);
        if entry.is_absolute() {
            entry.to_path_buf()
        } else {
            plugin_dir.join(entry)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_minimal_manifest() {
        let toml_str = r#"
[plugin]
name = "test-plugin"
version = "0.1.0"
entry = "./test-server"

[types]
tools = true
"#;
        let manifest: PluginManifest = toml::from_str(toml_str).unwrap();
        assert_eq!(manifest.plugin.name, "test-plugin");
        assert!(manifest.types.tools);
        assert!(!manifest.types.connector);
        assert_eq!(manifest.plugin.max_concurrent, 10);
    }

    #[test]
    fn test_missing_config_keys() {
        let toml_str = r#"
[plugin]
name = "test"
version = "0.1.0"
entry = "./test"

[config.api_key]
type = "secret"
required = true
description = "API key"

[config.rate_limit]
type = "number"
required = false
description = "Rate limit"
"#;
        let manifest: PluginManifest = toml::from_str(toml_str).unwrap();
        let provided = HashMap::new();
        let missing = manifest.missing_config_keys(&provided);
        assert_eq!(missing, vec!["api_key"]);
    }
}
```

- [ ] **Step 5: Write lib.rs skeleton**

```rust
// crates/openalpaca_plugins/src/lib.rs
pub mod error;
pub mod manifest;

pub use error::PluginError;
pub use manifest::PluginManifest;
```

- [ ] **Step 6: Add to workspace**

In root `Cargo.toml`, add `"crates/openalpaca_plugins"` to `workspace.members` and add to `[workspace.dependencies]`:
```toml
openalpaca_plugins = { path = "crates/openalpaca_plugins" }
```

- [ ] **Step 7: Verify it compiles and tests pass**

Run: `cargo test -p openalpaca_plugins`
Expected: 2 tests pass

- [ ] **Step 8: Commit**

```bash
git add Cargo.toml crates/openalpaca_plugins/
git commit -m "feat: create openalpaca_plugins crate with manifest parsing

New crate for plugin infrastructure. Parses plugin.toml manifests
with support for all 5 plugin types, capability declarations,
config schema, and MCP compatibility flag."
```

---

### Task 5: StdioChannel — JSON-RPC multiplexing over stdio

This is the lowest-level transport component. Everything else depends on it.

**Files:**
- Create: `crates/openalpaca_plugins/src/stdio_channel.rs`
- Modify: `crates/openalpaca_plugins/src/lib.rs`

- [ ] **Step 1: Write stdio_channel.rs**

```rust
// crates/openalpaca_plugins/src/stdio_channel.rs
use crate::error::PluginError;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout};
use tokio::sync::{mpsc, oneshot, Mutex, Semaphore};
use tracing::{debug, error, trace, warn};

/// A multiplexed JSON-RPC channel over a child process's stdin/stdout.
///
/// Supports concurrent requests via JSON-RPC request IDs.
/// Notifications (no `id` field) are forwarded to a separate channel.
#[derive(Clone)]
pub struct StdioChannel {
    writer: mpsc::Sender<Vec<u8>>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, PluginError>>>>>,
    next_id: Arc<AtomicU64>,
    semaphore: Arc<Semaphore>,
    notification_tx: mpsc::Sender<Value>,
    default_timeout: Duration,
}

impl StdioChannel {
    /// Create a new StdioChannel, spawning reader and writer tasks.
    /// Returns the channel and a receiver for `$/event` notifications.
    pub fn new(
        stdin: ChildStdin,
        stdout: ChildStdout,
        max_concurrent: usize,
        default_timeout: Duration,
    ) -> (Self, mpsc::Receiver<Value>) {
        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, PluginError>>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let (writer_tx, writer_rx) = mpsc::channel::<Vec<u8>>(64);
        let (notification_tx, notification_rx) = mpsc::channel::<Value>(256);

        // Spawn writer task
        tokio::spawn(Self::writer_loop(stdin, writer_rx));

        // Spawn reader task
        tokio::spawn(Self::reader_loop(
            stdout,
            pending.clone(),
            notification_tx.clone(),
        ));

        let channel = Self {
            writer: writer_tx,
            pending,
            next_id: Arc::new(AtomicU64::new(1)),
            semaphore: Arc::new(Semaphore::new(max_concurrent)),
            notification_tx,
            default_timeout,
        };

        (channel, notification_rx)
    }

    /// Send a JSON-RPC request and wait for the response.
    pub async fn call(
        &self,
        method: &str,
        params: Value,
    ) -> Result<Value, PluginError> {
        self.call_with_timeout(method, params, self.default_timeout).await
    }

    /// Send a JSON-RPC request with a custom timeout.
    pub async fn call_with_timeout(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, PluginError> {
        let _permit = self.semaphore.acquire().await
            .map_err(|_| PluginError::ChannelClosed)?;

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();

        self.pending.lock().await.insert(id, tx);

        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        let body = serde_json::to_string(&request)
            .map_err(|e| PluginError::Json(e))?;
        let frame = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);

        trace!(id, method, "sending JSON-RPC request");

        self.writer.send(frame.into_bytes()).await
            .map_err(|_| PluginError::ChannelClosed)?;

        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => {
                // oneshot dropped = process crashed
                self.pending.lock().await.remove(&id);
                Err(PluginError::ProcessCrashed)
            }
            Err(_) => {
                // Timeout — remove the pending entry
                self.pending.lock().await.remove(&id);
                Err(PluginError::Timeout)
            }
        }
    }

    /// Send a JSON-RPC notification (no response expected).
    pub async fn notify(&self, method: &str, params: Value) -> Result<(), PluginError> {
        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });

        let body = serde_json::to_string(&notification)?;
        let frame = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);

        self.writer.send(frame.into_bytes()).await
            .map_err(|_| PluginError::ChannelClosed)?;
        Ok(())
    }

    /// Drop all pending requests (e.g., on process crash).
    pub async fn drain_pending(&self) {
        let mut pending = self.pending.lock().await;
        for (_, tx) in pending.drain() {
            let _ = tx.send(Err(PluginError::ProcessCrashed));
        }
    }

    async fn writer_loop(mut stdin: ChildStdin, mut rx: mpsc::Receiver<Vec<u8>>) {
        while let Some(data) = rx.recv().await {
            if let Err(e) = stdin.write_all(&data).await {
                error!("plugin stdin write error: {e}");
                break;
            }
            if let Err(e) = stdin.flush().await {
                error!("plugin stdin flush error: {e}");
                break;
            }
        }
        debug!("plugin writer loop exited");
    }

    async fn reader_loop(
        stdout: ChildStdout,
        pending: Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, PluginError>>>>>,
        notification_tx: mpsc::Sender<Value>,
    ) {
        let mut reader = BufReader::new(stdout);
        let mut header_buf = String::new();

        loop {
            // Read Content-Length header
            header_buf.clear();
            match reader.read_line(&mut header_buf).await {
                Ok(0) => break, // EOF
                Err(e) => {
                    error!("plugin stdout read error: {e}");
                    break;
                }
                Ok(_) => {}
            }

            let content_length = match parse_content_length(&header_buf) {
                Some(len) => len,
                None => {
                    warn!("invalid Content-Length header: {:?}", header_buf.trim());
                    continue;
                }
            };

            // Read blank line separator
            header_buf.clear();
            if reader.read_line(&mut header_buf).await.is_err() {
                break;
            }

            // Read body
            let mut body = vec![0u8; content_length];
            if reader.read_exact(&mut body).await.is_err() {
                error!("plugin stdout: incomplete body read");
                break;
            }

            let msg: Value = match serde_json::from_slice(&body) {
                Ok(v) => v,
                Err(e) => {
                    warn!("plugin sent invalid JSON: {e}");
                    continue;
                }
            };

            if let Some(id) = msg.get("id").and_then(|v| v.as_u64()) {
                // Response to a request
                let mut pending_lock = pending.lock().await;
                if let Some(tx) = pending_lock.remove(&id) {
                    if let Some(error) = msg.get("error") {
                        let code = error.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
                        let message = error.get("message").and_then(|m| m.as_str())
                            .unwrap_or("unknown error").to_string();
                        let _ = tx.send(Err(PluginError::RpcError { code, message }));
                    } else {
                        let result = msg.get("result").cloned().unwrap_or(Value::Null);
                        let _ = tx.send(Ok(result));
                    }
                } else {
                    warn!(id, "received response for unknown request");
                }
            } else {
                // Notification (no id) — forward to event relay
                let _ = notification_tx.send(msg).await;
            }
        }

        // Process exited — drain all pending
        debug!("plugin reader loop exited, draining pending requests");
        let mut pending_lock = pending.lock().await;
        for (_, tx) in pending_lock.drain() {
            let _ = tx.send(Err(PluginError::ProcessCrashed));
        }
    }
}

fn parse_content_length(header: &str) -> Option<usize> {
    let header = header.trim();
    if let Some(value) = header.strip_prefix("Content-Length:") {
        value.trim().parse().ok()
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_content_length() {
        assert_eq!(parse_content_length("Content-Length: 42\r\n"), Some(42));
        assert_eq!(parse_content_length("Content-Length:100"), Some(100));
        assert_eq!(parse_content_length("invalid"), None);
    }
}
```

- [ ] **Step 2: Export from lib.rs**

Add to `crates/openalpaca_plugins/src/lib.rs`:
```rust
pub mod stdio_channel;
pub use stdio_channel::StdioChannel;
```

- [ ] **Step 3: Verify it compiles and tests pass**

Run: `cargo test -p openalpaca_plugins`
Expected: 3 tests pass (2 manifest + 1 content-length)

- [ ] **Step 4: Commit**

```bash
git add crates/openalpaca_plugins/
git commit -m "feat: add StdioChannel for JSON-RPC multiplexing over stdio

Single reader/writer task architecture with request ID correlation,
Content-Length framing, concurrent request support via Semaphore,
and automatic pending request drain on process exit."
```

---

### Task 6: ProcessPool — spawn and manage plugin child processes

**Files:**
- Create: `crates/openalpaca_plugins/src/process_pool.rs`
- Modify: `crates/openalpaca_plugins/src/lib.rs`

- [ ] **Step 1: Write process_pool.rs**

```rust
// crates/openalpaca_plugins/src/process_pool.rs
use crate::error::PluginError;
use crate::manifest::PluginManifest;
use crate::stdio_channel::StdioChannel;
use serde_json::Value;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tracing::{debug, error, info};

/// Handle to a running plugin process.
pub struct PluginProcess {
    pub child: Child,
    pub channel: StdioChannel,
    pub notification_rx: mpsc::Receiver<Value>,
}

impl PluginProcess {
    /// Spawn a plugin process from its manifest and directory.
    pub async fn spawn(
        manifest: &PluginManifest,
        plugin_dir: &Path,
    ) -> Result<Self, PluginError> {
        let entry_path = manifest.entry_path(plugin_dir);

        info!(
            plugin = %manifest.plugin.name,
            entry = %entry_path.display(),
            "spawning plugin process"
        );

        let mut cmd = Command::new(&entry_path);
        cmd.args(&manifest.plugin.args)
            .current_dir(plugin_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        for (key, value) in &manifest.plugin.env {
            cmd.env(key, value);
        }

        let mut child = cmd.spawn().map_err(|e| {
            PluginError::SpawnFailed(format!(
                "{}: {}",
                entry_path.display(),
                e
            ))
        })?;

        let stdin = child.stdin.take()
            .ok_or_else(|| PluginError::SpawnFailed("no stdin".into()))?;
        let stdout = child.stdout.take()
            .ok_or_else(|| PluginError::SpawnFailed("no stdout".into()))?;

        let default_timeout = Duration::from_secs(60);
        let (channel, notification_rx) = StdioChannel::new(
            stdin,
            stdout,
            manifest.plugin.max_concurrent,
            default_timeout,
        );

        Ok(Self {
            child,
            channel,
            notification_rx,
        })
    }

    /// Send the `initialize` RPC and wait for `{ ready: true }`.
    pub async fn initialize(
        &self,
        plugin_id: &str,
        version: &str,
        capabilities_granted: &[String],
        config: Value,
    ) -> Result<(), PluginError> {
        let params = serde_json::json!({
            "plugin_id": plugin_id,
            "version": version,
            "capabilities_granted": capabilities_granted,
            "config": config,
            "daemon_version": env!("CARGO_PKG_VERSION"),
        });

        let result = self.channel
            .call_with_timeout("initialize", params, Duration::from_secs(10))
            .await?;

        let ready = result.get("ready").and_then(|v| v.as_bool()).unwrap_or(false);
        if !ready {
            return Err(PluginError::RpcError {
                code: -1,
                message: "plugin did not report ready".into(),
            });
        }

        debug!(plugin_id, "plugin initialized successfully");
        Ok(())
    }

    /// Send the `shutdown` RPC and wait briefly for acknowledgement.
    pub async fn shutdown(&self) -> Result<(), PluginError> {
        let _ = self.channel
            .call_with_timeout("shutdown", Value::Null, Duration::from_secs(3))
            .await;
        Ok(())
    }

    /// Kill the child process.
    pub async fn kill(&mut self) {
        if let Err(e) = self.child.kill().await {
            error!("failed to kill plugin process: {e}");
        }
    }

    /// Check if the process is still running.
    pub fn try_wait(&mut self) -> Option<std::process::ExitStatus> {
        self.child.try_wait().ok().flatten()
    }
}
```

- [ ] **Step 2: Export from lib.rs**

Add to `crates/openalpaca_plugins/src/lib.rs`:
```rust
pub mod process_pool;
pub use process_pool::PluginProcess;
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p openalpaca_plugins`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add crates/openalpaca_plugins/
git commit -m "feat: add PluginProcess for spawning plugin child processes

Handles process spawning with stdin/stdout/stderr capture,
StdioChannel setup, initialize RPC with 10s timeout,
graceful shutdown, and kill."
```

---

### Task 7: PermissionGate — first-load approval and config validation

**Files:**
- Create: `crates/openalpaca_plugins/src/permission_gate.rs`
- Modify: `crates/openalpaca_plugins/src/lib.rs`

- [ ] **Step 1: Write permission_gate.rs**

```rust
// crates/openalpaca_plugins/src/permission_gate.rs
use crate::error::PluginError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use tracing::{debug, info};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PermissionsFile {
    #[serde(flatten)]
    pub plugins: HashMap<String, PluginPermission>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginPermission {
    pub approved: bool,
    #[serde(default)]
    pub approved_at: Option<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

pub struct PermissionGate {
    permissions_path: PathBuf,
    config_dir: PathBuf,
}

impl PermissionGate {
    pub fn new(plugin_dir: &Path) -> Self {
        Self {
            permissions_path: plugin_dir.join(".permissions.toml"),
            config_dir: plugin_dir.join(".config"),
        }
    }

    /// Check if a plugin is approved. Returns None if never seen.
    pub fn is_approved(&self, plugin_name: &str) -> Option<bool> {
        let file = self.load_permissions();
        file.plugins.get(plugin_name).map(|p| p.approved)
    }

    /// Approve a plugin with its declared capabilities.
    pub fn approve(&self, plugin_name: &str, capabilities: &[String]) -> Result<(), PluginError> {
        let mut file = self.load_permissions();
        file.plugins.insert(
            plugin_name.to_string(),
            PluginPermission {
                approved: true,
                approved_at: Some(chrono::Utc::now().to_rfc3339()),
                capabilities: capabilities.to_vec(),
            },
        );
        self.save_permissions(&file)?;
        info!(plugin_name, "plugin approved");
        Ok(())
    }

    /// Deny a plugin.
    pub fn deny(&self, plugin_name: &str) -> Result<(), PluginError> {
        let mut file = self.load_permissions();
        file.plugins.insert(
            plugin_name.to_string(),
            PluginPermission {
                approved: false,
                approved_at: None,
                capabilities: vec![],
            },
        );
        self.save_permissions(&file)?;
        info!(plugin_name, "plugin denied");
        Ok(())
    }

    /// Load user-provided config for a plugin. Returns empty map if no config file.
    pub fn load_plugin_config(
        &self,
        plugin_name: &str,
    ) -> HashMap<String, toml::Value> {
        let path = self.config_dir.join(format!("{plugin_name}.toml"));
        if !path.exists() {
            return HashMap::new();
        }
        match std::fs::read_to_string(&path) {
            Ok(content) => toml::from_str(&content).unwrap_or_default(),
            Err(_) => HashMap::new(),
        }
    }

    /// Save a config value for a plugin.
    pub fn set_plugin_config(
        &self,
        plugin_name: &str,
        key: &str,
        value: toml::Value,
    ) -> Result<(), PluginError> {
        std::fs::create_dir_all(&self.config_dir)?;
        let path = self.config_dir.join(format!("{plugin_name}.toml"));
        let mut config: HashMap<String, toml::Value> = if path.exists() {
            let content = std::fs::read_to_string(&path)?;
            toml::from_str(&content).unwrap_or_default()
        } else {
            HashMap::new()
        };
        config.insert(key.to_string(), value);
        let content = toml::to_string_pretty(&config)
            .map_err(|e| PluginError::InvalidManifest(e.to_string()))?;
        std::fs::write(&path, content)?;
        debug!(plugin_name, key, "plugin config updated");
        Ok(())
    }

    fn load_permissions(&self) -> PermissionsFile {
        if !self.permissions_path.exists() {
            return PermissionsFile::default();
        }
        match std::fs::read_to_string(&self.permissions_path) {
            Ok(content) => toml::from_str(&content).unwrap_or_default(),
            Err(_) => PermissionsFile::default(),
        }
    }

    fn save_permissions(&self, file: &PermissionsFile) -> Result<(), PluginError> {
        let content = toml::to_string_pretty(file)
            .map_err(|e| PluginError::InvalidManifest(e.to_string()))?;
        std::fs::write(&self.permissions_path, content)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_approve_and_check() {
        let dir = TempDir::new().unwrap();
        let gate = PermissionGate::new(dir.path());

        assert_eq!(gate.is_approved("test-plugin"), None);
        gate.approve("test-plugin", &["network".into()]).unwrap();
        assert_eq!(gate.is_approved("test-plugin"), Some(true));
    }

    #[test]
    fn test_deny() {
        let dir = TempDir::new().unwrap();
        let gate = PermissionGate::new(dir.path());

        gate.deny("bad-plugin").unwrap();
        assert_eq!(gate.is_approved("bad-plugin"), Some(false));
    }

    #[test]
    fn test_plugin_config() {
        let dir = TempDir::new().unwrap();
        let gate = PermissionGate::new(dir.path());

        gate.set_plugin_config("test", "api_key", toml::Value::String("secret".into()))
            .unwrap();
        let config = gate.load_plugin_config("test");
        assert_eq!(
            config.get("api_key"),
            Some(&toml::Value::String("secret".into()))
        );
    }
}
```

- [ ] **Step 2: Add tempfile dev-dependency**

In `crates/openalpaca_plugins/Cargo.toml`, add:
```toml
[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 3: Export from lib.rs**

Add to `crates/openalpaca_plugins/src/lib.rs`:
```rust
pub mod permission_gate;
pub use permission_gate::PermissionGate;
```

- [ ] **Step 4: Verify tests pass**

Run: `cargo test -p openalpaca_plugins`
Expected: 6 tests pass

- [ ] **Step 5: Commit**

```bash
git add crates/openalpaca_plugins/
git commit -m "feat: add PermissionGate for plugin approval and config management

Handles first-load approval persistence in .permissions.toml,
plugin deny, user-provided config in .config/<plugin>.toml,
and config get/set operations."
```

---

### Task 8: ToolBridge — PluginToolProxy implementing PluginToolExecutor

**Files:**
- Create: `crates/openalpaca_plugins/src/bridge/mod.rs`
- Create: `crates/openalpaca_plugins/src/bridge/tool_bridge.rs`
- Modify: `crates/openalpaca_plugins/src/lib.rs`

- [ ] **Step 1: Create bridge module**

```rust
// crates/openalpaca_plugins/src/bridge/mod.rs
pub mod tool_bridge;
pub use tool_bridge::PluginToolProxy;
```

- [ ] **Step 2: Write tool_bridge.rs**

```rust
// crates/openalpaca_plugins/src/bridge/tool_bridge.rs
use crate::stdio_channel::StdioChannel;
use async_trait::async_trait;
use openalpaca_api::plugin_traits::PluginToolExecutor;
use serde_json::Value;
use std::sync::Arc;

/// Executes tool calls by proxying to a plugin process via StdioChannel.
pub struct PluginToolProxy {
    plugin_id: String,
    channel: StdioChannel,
}

impl PluginToolProxy {
    pub fn new(plugin_id: String, channel: StdioChannel) -> Self {
        Self { plugin_id, channel }
    }
}

#[async_trait]
impl PluginToolExecutor for PluginToolProxy {
    async fn execute(&self, tool_name: &str, arguments: &Value) -> Result<String, String> {
        let params = serde_json::json!({
            "name": tool_name,
            "arguments": arguments,
        });

        match self.channel.call("tools/call", params).await {
            Ok(result) => {
                // MCP tools/call returns { content: [{ type: "text", text: "..." }] }
                if let Some(content) = result.get("content") {
                    if let Some(arr) = content.as_array() {
                        let texts: Vec<&str> = arr.iter()
                            .filter_map(|item| item.get("text").and_then(|t| t.as_str()))
                            .collect();
                        return Ok(texts.join("\n"));
                    }
                }
                // Fallback: return stringified result
                Ok(result.to_string())
            }
            Err(e) => Err(format!("plugin {}::{}: {}", self.plugin_id, tool_name, e)),
        }
    }

    fn plugin_id(&self) -> &str {
        &self.plugin_id
    }
}
```

- [ ] **Step 3: Export from lib.rs**

Add to `crates/openalpaca_plugins/src/lib.rs`:
```rust
pub mod bridge;
pub use bridge::PluginToolProxy;
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo check -p openalpaca_plugins`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/openalpaca_plugins/
git commit -m "feat: add PluginToolProxy implementing PluginToolExecutor

Proxies tool calls to plugin processes via StdioChannel.
Strips namespace prefix before sending, parses MCP content
response format."
```

---

## Chunk 3: PluginManager + Discovery + Daemon Wiring

### Task 9: PluginManager — core orchestrator

This task wires together all components: manifest parsing, permissions, process spawning, tool discovery, and tool registration.

**Files:**
- Create: `crates/openalpaca_plugins/src/manager.rs`
- Modify: `crates/openalpaca_plugins/src/lib.rs`

- [ ] **Step 1: Write manager.rs**

This is the largest file. It implements:
- Plugin directory scanning
- Hot-load sequence (parse → approve check → config check → spawn → initialize → discover tools → register)
- Hot-unload sequence (drain → shutdown → kill → unregister)
- Plugin state tracking
- Public API for CLI/routes (list, approve, deny, enable, disable)

```rust
// crates/openalpaca_plugins/src/manager.rs
use crate::bridge::PluginToolProxy;
use crate::error::PluginError;
use crate::manifest::PluginManifest;
use crate::permission_gate::PermissionGate;
use crate::process_pool::PluginProcess;
use openalpaca_core::tools::{RegisteredTool, ToolBackend, ToolDefinition, ToolRegistry};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

/// Current status of a plugin.
#[derive(Debug, Clone)]
pub enum PluginStatus {
    Loading,
    WaitingApproval,
    NeedsConfig { missing_keys: Vec<String> },
    Running,
    Crashed { error: String, backoff_until: Instant },
    Disabled,
    Stopped,
}

impl std::fmt::Display for PluginStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Loading => write!(f, "loading"),
            Self::WaitingApproval => write!(f, "waiting_approval"),
            Self::NeedsConfig { missing_keys } => write!(f, "needs_config({:?})", missing_keys),
            Self::Running => write!(f, "running"),
            Self::Crashed { error, .. } => write!(f, "crashed: {error}"),
            Self::Disabled => write!(f, "disabled"),
            Self::Stopped => write!(f, "stopped"),
        }
    }
}

/// State for a single plugin.
pub struct PluginState {
    pub manifest: PluginManifest,
    pub status: PluginStatus,
    pub process: Option<PluginProcess>,
    pub registered_tools: Vec<String>,
    pub restart_count: u32,
    pub last_health: Option<Instant>,
    pub plugin_dir: PathBuf,
}

/// Manages all plugins: discovery, lifecycle, and registry bridging.
pub struct PluginManager {
    plugins: Arc<RwLock<HashMap<String, PluginState>>>,
    plugin_dir: PathBuf,
    permission_gate: PermissionGate,
    tool_registry: Arc<ToolRegistry>,
}

impl PluginManager {
    pub fn new(
        plugin_dir: PathBuf,
        tool_registry: Arc<ToolRegistry>,
    ) -> Self {
        let permission_gate = PermissionGate::new(&plugin_dir);
        Self {
            plugins: Arc::new(RwLock::new(HashMap::new())),
            plugin_dir,
            permission_gate,
            tool_registry,
        }
    }

    /// Scan plugin directory and load all approved plugins.
    pub async fn start(&self) -> Result<(), PluginError> {
        std::fs::create_dir_all(&self.plugin_dir)?;
        info!(dir = %self.plugin_dir.display(), "scanning plugin directory");

        let entries = std::fs::read_dir(&self.plugin_dir)?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && !path.file_name().map_or(true, |n| n.to_string_lossy().starts_with('.')) {
                if let Err(e) = self.try_load_plugin(&path).await {
                    warn!(dir = %path.display(), error = %e, "failed to load plugin");
                }
            }
        }

        Ok(())
    }

    /// Attempt to load a single plugin from its directory.
    pub async fn try_load_plugin(&self, plugin_dir: &Path) -> Result<(), PluginError> {
        let manifest = PluginManifest::from_dir(plugin_dir)?;
        let name = manifest.plugin.name.clone();

        info!(plugin = %name, "discovered plugin");

        // Step 1: Check approval
        match self.permission_gate.is_approved(&name) {
            Some(false) => {
                info!(plugin = %name, "plugin denied — skipping");
                let mut plugins = self.plugins.write().await;
                plugins.insert(name, PluginState {
                    manifest,
                    status: PluginStatus::Disabled,
                    process: None,
                    registered_tools: vec![],
                    restart_count: 0,
                    last_health: None,
                    plugin_dir: plugin_dir.to_path_buf(),
                });
                return Ok(());
            }
            None => {
                info!(plugin = %name, "new plugin — awaiting approval");
                let mut plugins = self.plugins.write().await;
                plugins.insert(name, PluginState {
                    manifest,
                    status: PluginStatus::WaitingApproval,
                    process: None,
                    registered_tools: vec![],
                    restart_count: 0,
                    last_health: None,
                    plugin_dir: plugin_dir.to_path_buf(),
                });
                return Ok(());
            }
            Some(true) => {} // Continue to spawn
        }

        // Step 2: Config validation
        let provided_config = self.permission_gate.load_plugin_config(&name);
        let missing = manifest.missing_config_keys(&provided_config);
        if !missing.is_empty() {
            info!(plugin = %name, ?missing, "plugin needs config");
            let mut plugins = self.plugins.write().await;
            plugins.insert(name, PluginState {
                manifest,
                status: PluginStatus::NeedsConfig { missing_keys: missing },
                process: None,
                registered_tools: vec![],
                restart_count: 0,
                last_health: None,
                plugin_dir: plugin_dir.to_path_buf(),
            });
            return Ok(());
        }

        // Step 3: Spawn
        self.spawn_plugin(plugin_dir, manifest, provided_config).await
    }

    async fn spawn_plugin(
        &self,
        plugin_dir: &Path,
        manifest: PluginManifest,
        config: HashMap<String, toml::Value>,
    ) -> Result<(), PluginError> {
        let name = manifest.plugin.name.clone();
        let version = manifest.plugin.version.clone();
        let capabilities = manifest.capabilities.requires.clone();

        let process = PluginProcess::spawn(&manifest, plugin_dir).await?;

        // Initialize (skip for MCP-compatible plugins)
        if !manifest.plugin.mcp_compatible {
            let config_json = serde_json::to_value(&config)
                .unwrap_or(Value::Object(Default::default()));
            process.initialize(&name, &version, &capabilities, config_json).await?;
        }

        // Discover tools
        let mut registered_tools = Vec::new();
        if manifest.types.tools {
            match process.channel.call("tools/list", Value::Object(Default::default())).await {
                Ok(result) => {
                    if let Some(tools) = result.get("tools").and_then(|t| t.as_array()) {
                        for tool_json in tools {
                            if let Some(tool_name) = tool_json.get("name").and_then(|n| n.as_str()) {
                                let namespaced = format!("{}::{}", name, tool_name);
                                let description = tool_json.get("description")
                                    .and_then(|d| d.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let parameters = tool_json.get("inputSchema")
                                    .cloned()
                                    .unwrap_or(Value::Object(Default::default()));

                                let proxy = PluginToolProxy::new(
                                    name.clone(),
                                    process.channel.clone(),
                                );

                                let registered = RegisteredTool {
                                    definition: ToolDefinition {
                                        name: namespaced.clone(),
                                        description,
                                        parameters,
                                    },
                                    backend: ToolBackend::Plugin(Arc::new(proxy)),
                                    provides_capabilities: manifest.capabilities.provides.clone(),
                                    exempt_from_timeout: false,
                                };

                                self.tool_registry.register(registered);
                                registered_tools.push(namespaced);
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!(plugin = %name, error = %e, "tools/list failed");
                }
            }
        }

        info!(
            plugin = %name,
            tools = registered_tools.len(),
            "plugin loaded successfully"
        );

        let mut plugins = self.plugins.write().await;
        plugins.insert(name, PluginState {
            manifest,
            status: PluginStatus::Running,
            process: Some(process),
            registered_tools,
            restart_count: 0,
            last_health: Some(Instant::now()),
            plugin_dir: plugin_dir.to_path_buf(),
        });

        Ok(())
    }

    /// Unload a plugin: unregister tools, shutdown process.
    pub async fn unload_plugin(&self, name: &str) -> Result<(), PluginError> {
        let mut plugins = self.plugins.write().await;
        if let Some(mut state) = plugins.remove(name) {
            // Unregister tools
            for tool_name in &state.registered_tools {
                self.tool_registry.remove(tool_name);
            }

            // Shutdown process
            if let Some(ref process) = state.process {
                let _ = process.shutdown().await;
            }
            if let Some(ref mut process) = state.process {
                process.kill().await;
            }

            info!(plugin = %name, "plugin unloaded");
        }
        Ok(())
    }

    /// Approve a pending plugin and trigger load.
    pub async fn approve_plugin(&self, name: &str) -> Result<(), PluginError> {
        let plugin_dir;
        let manifest;
        {
            let plugins = self.plugins.read().await;
            let state = plugins.get(name)
                .ok_or_else(|| PluginError::Unavailable(name.to_string()))?;
            plugin_dir = state.plugin_dir.clone();
            manifest = state.manifest.clone();
        }

        self.permission_gate.approve(name, &manifest.capabilities.requires)?;

        // Remove old state and re-load
        self.plugins.write().await.remove(name);
        self.try_load_plugin(&plugin_dir).await
    }

    /// Deny a pending plugin.
    pub async fn deny_plugin(&self, name: &str) -> Result<(), PluginError> {
        self.permission_gate.deny(name)?;
        let mut plugins = self.plugins.write().await;
        if let Some(state) = plugins.get_mut(name) {
            state.status = PluginStatus::Disabled;
        }
        Ok(())
    }

    /// Disable a running plugin.
    pub async fn disable_plugin(&self, name: &str) -> Result<(), PluginError> {
        self.unload_plugin(name).await?;
        let mut plugins = self.plugins.write().await;
        plugins.insert(name.to_string(), PluginState {
            manifest: PluginManifest::from_dir(&self.plugin_dir.join(name))?,
            status: PluginStatus::Disabled,
            process: None,
            registered_tools: vec![],
            restart_count: 0,
            last_health: None,
            plugin_dir: self.plugin_dir.join(name),
        });
        Ok(())
    }

    /// Enable a disabled plugin.
    pub async fn enable_plugin(&self, name: &str) -> Result<(), PluginError> {
        let plugin_dir = self.plugin_dir.join(name);
        self.plugins.write().await.remove(name);
        self.try_load_plugin(&plugin_dir).await
    }

    /// List all plugins with their status.
    pub async fn list_plugins(&self) -> Vec<(String, String, String, Vec<String>)> {
        let plugins = self.plugins.read().await;
        plugins
            .iter()
            .map(|(name, state)| {
                (
                    name.clone(),
                    state.manifest.plugin.version.clone(),
                    state.status.to_string(),
                    state.registered_tools.clone(),
                )
            })
            .collect()
    }

    /// Set a config value for a plugin. Re-attempts load if plugin was in NeedsConfig.
    pub async fn set_plugin_config(
        &self,
        name: &str,
        key: &str,
        value: toml::Value,
    ) -> Result<(), PluginError> {
        self.permission_gate.set_plugin_config(name, key, value)?;

        // Check if this resolves NeedsConfig
        let should_retry = {
            let plugins = self.plugins.read().await;
            matches!(
                plugins.get(name).map(|s| &s.status),
                Some(PluginStatus::NeedsConfig { .. })
            )
        };

        if should_retry {
            let plugin_dir = self.plugin_dir.join(name);
            self.plugins.write().await.remove(name);
            self.try_load_plugin(&plugin_dir).await?;
        }

        Ok(())
    }
}
```

- [ ] **Step 2: Export from lib.rs**

Update `crates/openalpaca_plugins/src/lib.rs`:
```rust
pub mod error;
pub mod manifest;
pub mod stdio_channel;
pub mod process_pool;
pub mod permission_gate;
pub mod bridge;
pub mod manager;

pub use error::PluginError;
pub use manifest::PluginManifest;
pub use stdio_channel::StdioChannel;
pub use process_pool::PluginProcess;
pub use permission_gate::PermissionGate;
pub use bridge::PluginToolProxy;
pub use manager::{PluginManager, PluginStatus};
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check -p openalpaca_plugins`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add crates/openalpaca_plugins/
git commit -m "feat: add PluginManager — core plugin lifecycle orchestrator

Handles plugin directory scanning, hot-load sequence (approval check,
config validation, process spawn, initialize, tool discovery, registry
bridging), hot-unload, approve/deny/enable/disable operations."
```

---

### Task 10: Wire PluginManager into daemon

**Files:**
- Modify: `apps/openalpacad/src/state.rs`
- Modify: `apps/openalpacad/src/main.rs`
- Modify: `apps/openalpacad/Cargo.toml`

- [ ] **Step 1: Add openalpaca_plugins dependency to daemon**

In `apps/openalpacad/Cargo.toml`, add:
```toml
openalpaca_plugins = { workspace = true }
```

- [ ] **Step 2: Add plugin_manager to AppState**

In `apps/openalpacad/src/state.rs`, add after the last field:
```rust
pub plugin_manager: Option<Arc<openalpaca_plugins::PluginManager>>,
```

- [ ] **Step 3: Initialize PluginManager in main.rs**

In `apps/openalpacad/src/main.rs`, after services are initialized and the Orchestrator is created (around line 340), add:

```rust
// Initialize PluginManager
let plugin_dir = openalpaca_core::paths::app_dir().join("plugins");
let plugin_manager = Arc::new(openalpaca_plugins::PluginManager::new(
    plugin_dir,
    services.tool_registry.clone(),
));
if let Err(e) = plugin_manager.start().await {
    tracing::warn!("plugin manager startup: {e}");
}
```

- [ ] **Step 4: Pass plugin_manager to AppState**

In the `AppState` construction in `main.rs`, add:
```rust
plugin_manager: Some(plugin_manager),
```

- [ ] **Step 5: Verify daemon compiles**

Run: `cargo check -p openalpacad`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add apps/openalpacad/
git commit -m "feat: wire PluginManager into daemon startup

PluginManager initialized after services, scans ~/.openalpaca/plugins/
on startup, registered in AppState for route handler access."
```

---

### Task 11: Add plugin CLI subcommands

**Files:**
- Create: `apps/openalpaca/src/commands/plugin.rs`
- Modify: `apps/openalpaca/src/commands/mod.rs`
- Modify: `apps/openalpaca/src/main.rs`

- [ ] **Step 1: Write plugin.rs**

```rust
// apps/openalpaca/src/commands/plugin.rs
use clap::{Args, Subcommand};
use crate::commands::daemon::discover_daemon;

#[derive(Debug, Args)]
pub struct PluginArgs {
    #[command(subcommand)]
    pub command: PluginCommand,
}

#[derive(Debug, Subcommand)]
pub enum PluginCommand {
    /// List all plugins
    List,
    /// Approve a pending plugin
    Approve { name: String },
    /// Deny a pending plugin
    Deny { name: String },
    /// Enable a disabled plugin
    Enable { name: String },
    /// Disable a running plugin
    Disable { name: String },
    /// Show plugin info
    Info { name: String },
    /// Set plugin config
    Config {
        name: String,
        #[command(subcommand)]
        action: ConfigAction,
    },
}

#[derive(Debug, Subcommand)]
pub enum ConfigAction {
    /// Set a config value
    Set { key: String, value: String },
    /// Get config values
    Get { key: Option<String> },
}

pub async fn run(args: PluginArgs) -> anyhow::Result<()> {
    let discovery = discover_daemon().await?;
    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{}", discovery.port);

    match args.command {
        PluginCommand::List => {
            let resp = client.get(format!("{base}/v1/plugins"))
                .header("Authorization", format!("Bearer {}", discovery.token))
                .send().await?;
            let body: serde_json::Value = resp.json().await?;
            if let Some(plugins) = body.as_array() {
                if plugins.is_empty() {
                    println!("No plugins installed.");
                } else {
                    for p in plugins {
                        let name = p.get("name").and_then(|n| n.as_str()).unwrap_or("?");
                        let version = p.get("version").and_then(|v| v.as_str()).unwrap_or("?");
                        let status = p.get("status").and_then(|s| s.as_str()).unwrap_or("?");
                        let tools: Vec<&str> = p.get("tools")
                            .and_then(|t| t.as_array())
                            .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
                            .unwrap_or_default();
                        println!("{name} v{version} [{status}] tools: {}", tools.join(", "));
                    }
                }
            }
        }
        PluginCommand::Approve { name } => {
            let resp = client.post(format!("{base}/v1/plugins/{name}/approve"))
                .header("Authorization", format!("Bearer {}", discovery.token))
                .send().await?;
            if resp.status().is_success() {
                println!("Plugin '{name}' approved.");
            } else {
                println!("Failed: {}", resp.text().await?);
            }
        }
        PluginCommand::Deny { name } => {
            let resp = client.post(format!("{base}/v1/plugins/{name}/deny"))
                .header("Authorization", format!("Bearer {}", discovery.token))
                .send().await?;
            if resp.status().is_success() {
                println!("Plugin '{name}' denied.");
            } else {
                println!("Failed: {}", resp.text().await?);
            }
        }
        PluginCommand::Enable { name } => {
            let resp = client.post(format!("{base}/v1/plugins/{name}/enable"))
                .header("Authorization", format!("Bearer {}", discovery.token))
                .send().await?;
            if resp.status().is_success() {
                println!("Plugin '{name}' enabled.");
            } else {
                println!("Failed: {}", resp.text().await?);
            }
        }
        PluginCommand::Disable { name } => {
            let resp = client.post(format!("{base}/v1/plugins/{name}/disable"))
                .header("Authorization", format!("Bearer {}", discovery.token))
                .send().await?;
            if resp.status().is_success() {
                println!("Plugin '{name}' disabled.");
            } else {
                println!("Failed: {}", resp.text().await?);
            }
        }
        PluginCommand::Info { name } => {
            let resp = client.get(format!("{base}/v1/plugins/{name}"))
                .header("Authorization", format!("Bearer {}", discovery.token))
                .send().await?;
            let body: serde_json::Value = resp.json().await?;
            println!("{}", serde_json::to_string_pretty(&body)?);
        }
        PluginCommand::Config { name, action } => {
            match action {
                ConfigAction::Set { key, value } => {
                    let resp = client.post(format!("{base}/v1/plugins/{name}/config"))
                        .header("Authorization", format!("Bearer {}", discovery.token))
                        .json(&serde_json::json!({ "key": key, "value": value }))
                        .send().await?;
                    if resp.status().is_success() {
                        println!("Config '{key}' set for plugin '{name}'.");
                    } else {
                        println!("Failed: {}", resp.text().await?);
                    }
                }
                ConfigAction::Get { key } => {
                    let url = if let Some(k) = key {
                        format!("{base}/v1/plugins/{name}/config?key={k}")
                    } else {
                        format!("{base}/v1/plugins/{name}/config")
                    };
                    let resp = client.get(url)
                        .header("Authorization", format!("Bearer {}", discovery.token))
                        .send().await?;
                    let body: serde_json::Value = resp.json().await?;
                    println!("{}", serde_json::to_string_pretty(&body)?);
                }
            }
        }
    }

    Ok(())
}
```

- [ ] **Step 2: Add to commands/mod.rs**

Add `pub mod plugin;` to `apps/openalpaca/src/commands/mod.rs`.

- [ ] **Step 3: Add Plugin variant to Commands enum in main.rs**

In `apps/openalpaca/src/main.rs`, add to the `Commands` enum:
```rust
    /// Manage plugins
    Plugin(commands::plugin::PluginArgs),
```

And add the match arm:
```rust
    Commands::Plugin(args) => commands::plugin::run(args).await,
```

- [ ] **Step 4: Verify CLI compiles**

Run: `cargo check -p openalpaca`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add apps/openalpaca/
git commit -m "feat: add plugin CLI subcommands

Subcommands: list, approve, deny, enable, disable, info, config set/get.
All call daemon HTTP endpoints."
```

---

### Task 12: Add plugin daemon routes

**Files:**
- Create: `apps/openalpacad/src/routes/plugins.rs`
- Modify: `apps/openalpacad/src/routes/mod.rs`
- Modify: `apps/openalpacad/src/router.rs`

- [ ] **Step 1: Write plugins.rs route handlers**

```rust
// apps/openalpacad/src/routes/plugins.rs
use axum::{extract::{Path, State}, Json};
use axum::http::StatusCode;
use serde_json::{json, Value};
use crate::state::AppState;

pub async fn list_plugins(
    State(state): State<AppState>,
) -> Json<Value> {
    let Some(ref pm) = state.plugin_manager else {
        return Json(json!([]));
    };
    let plugins = pm.list_plugins().await;
    let list: Vec<Value> = plugins.into_iter().map(|(name, version, status, tools)| {
        json!({ "name": name, "version": version, "status": status, "tools": tools })
    }).collect();
    Json(json!(list))
}

pub async fn approve_plugin(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> StatusCode {
    let Some(ref pm) = state.plugin_manager else {
        return StatusCode::SERVICE_UNAVAILABLE;
    };
    match pm.approve_plugin(&name).await {
        Ok(()) => StatusCode::OK,
        Err(_) => StatusCode::BAD_REQUEST,
    }
}

pub async fn deny_plugin(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> StatusCode {
    let Some(ref pm) = state.plugin_manager else {
        return StatusCode::SERVICE_UNAVAILABLE;
    };
    match pm.deny_plugin(&name).await {
        Ok(()) => StatusCode::OK,
        Err(_) => StatusCode::BAD_REQUEST,
    }
}

pub async fn enable_plugin(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> StatusCode {
    let Some(ref pm) = state.plugin_manager else {
        return StatusCode::SERVICE_UNAVAILABLE;
    };
    match pm.enable_plugin(&name).await {
        Ok(()) => StatusCode::OK,
        Err(_) => StatusCode::BAD_REQUEST,
    }
}

pub async fn disable_plugin(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> StatusCode {
    let Some(ref pm) = state.plugin_manager else {
        return StatusCode::SERVICE_UNAVAILABLE;
    };
    match pm.disable_plugin(&name).await {
        Ok(()) => StatusCode::OK,
        Err(_) => StatusCode::BAD_REQUEST,
    }
}

pub async fn plugin_config_set(
    State(state): State<AppState>,
    Path(name): Path<String>,
    Json(body): Json<Value>,
) -> StatusCode {
    let Some(ref pm) = state.plugin_manager else {
        return StatusCode::SERVICE_UNAVAILABLE;
    };
    let key = body.get("key").and_then(|k| k.as_str()).unwrap_or("");
    let value = body.get("value").and_then(|v| v.as_str()).unwrap_or("");
    match pm.set_plugin_config(&name, key, toml::Value::String(value.to_string())).await {
        Ok(()) => StatusCode::OK,
        Err(_) => StatusCode::BAD_REQUEST,
    }
}
```

- [ ] **Step 2: Add module and exports to routes/mod.rs**

Add `pub mod plugins;` and export the handler functions.

- [ ] **Step 3: Add routes to router.rs**

In the protected routes section of `apps/openalpacad/src/router.rs`, add:
```rust
.route("/v1/plugins", get(routes::plugins::list_plugins))
.route("/v1/plugins/{name}/approve", post(routes::plugins::approve_plugin))
.route("/v1/plugins/{name}/deny", post(routes::plugins::deny_plugin))
.route("/v1/plugins/{name}/enable", post(routes::plugins::enable_plugin))
.route("/v1/plugins/{name}/disable", post(routes::plugins::disable_plugin))
.route("/v1/plugins/{name}/config", post(routes::plugins::plugin_config_set))
```

- [ ] **Step 4: Verify daemon compiles**

Run: `cargo check -p openalpacad`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add apps/openalpacad/src/routes/plugins.rs apps/openalpacad/src/routes/mod.rs apps/openalpacad/src/router.rs
git commit -m "feat: add plugin management HTTP API routes

Endpoints: GET /v1/plugins, POST /v1/plugins/{name}/approve|deny|enable|disable|config"
```

---

## Chunk 4: End-to-End Verification

### Task 13: Create test fixture + integration test

**Files:**
- Create: `tests/fixtures/echo-plugin/plugin.toml`
- Create: `tests/fixtures/echo-plugin/echo-server.sh`

- [ ] **Step 1: Create test fixture manifest**

```toml
# tests/fixtures/echo-plugin/plugin.toml
[plugin]
name = "echo-test"
version = "0.1.0"
description = "Test plugin that echoes tool calls"
entry = "./echo-server.sh"
mcp_compatible = true

[capabilities]
provides = ["testing"]

[types]
tools = true
```

- [ ] **Step 2: Create echo MCP server script**

```bash
#!/bin/bash
# tests/fixtures/echo-plugin/echo-server.sh
# Minimal MCP server that responds to tools/list and tools/call

while IFS= read -r line; do
    # Skip Content-Length header
    if [[ "$line" == Content-Length:* ]]; then
        length="${line#Content-Length: }"
        length="${length%$'\r'}"
        read -r blank  # empty line
        body=$(head -c "$length")

        method=$(echo "$body" | python3 -c "import sys,json; print(json.loads(sys.stdin.read()).get('method',''))")
        id=$(echo "$body" | python3 -c "import sys,json; print(json.loads(sys.stdin.read()).get('id',''))")

        if [ "$method" = "tools/list" ]; then
            response="{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"tools\":[{\"name\":\"echo\",\"description\":\"Echo back the input\",\"inputSchema\":{\"type\":\"object\",\"properties\":{\"message\":{\"type\":\"string\"}}}}]}}"
        elif [ "$method" = "tools/call" ]; then
            msg=$(echo "$body" | python3 -c "import sys,json; print(json.loads(sys.stdin.read()).get('params',{}).get('arguments',{}).get('message','no message'))")
            response="{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{\"content\":[{\"type\":\"text\",\"text\":\"echo: $msg\"}]}}"
        else
            response="{\"jsonrpc\":\"2.0\",\"id\":$id,\"result\":{}}"
        fi

        echo -ne "Content-Length: ${#response}\r\n\r\n$response"
    fi
done
```

- [ ] **Step 3: Make executable**

```bash
chmod +x tests/fixtures/echo-plugin/echo-server.sh
```

- [ ] **Step 4: Verify full workspace builds**

Run: `cargo build --all-targets`
Expected: PASS

- [ ] **Step 5: Run all existing tests**

Run: `cargo test`
Expected: All existing tests pass. New plugin tests pass.

- [ ] **Step 6: Commit**

```bash
git add tests/fixtures/echo-plugin/
git commit -m "test: add echo-plugin test fixture for plugin SDK integration testing

Minimal bash MCP server that responds to tools/list with an 'echo' tool
and tools/call by echoing back the message argument."
```

---

## Final Verification

- [ ] **Full workspace build**: `cargo build --all-targets`
- [ ] **All tests pass**: `cargo test`
- [ ] **No new warnings**: `cargo clippy --all-targets`
- [ ] **Manual smoke test**: Drop `echo-plugin` fixture into `~/.openalpaca/plugins/`, start daemon, approve via CLI, verify `echo-test::echo` tool appears in registry
