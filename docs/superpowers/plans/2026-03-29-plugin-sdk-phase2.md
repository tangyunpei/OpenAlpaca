# Plugin SDK Phase 2: Connector + Provider Bridges

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enable connector plugins (chat platforms) and provider plugins (LLM backends) to integrate with OpenAlpaca at runtime, so new channels and models can be added without recompiling.

**Architecture:** Two bridges in `openalpaca_plugins`: `PluginConnector` (implements `Connector` trait, proxies `connector/*` RPC) and `PluginLlmProvider` (implements `LlmProvider` trait, proxies `provider/*` RPC). Prep refactors change `Connector::name()` from `&'static str` to `&str` and migrate `ProviderType` from a closed enum to a String-keyed system.

**Tech Stack:** Rust, tokio, async-trait, serde_json, DashMap

**Spec:** `docs/superpowers/specs/2026-03-29-plugin-sdk-design.md` (Sections 1, 4)

**Depends on:** Phase 1 (plugin-sdk-phase1 branch)

---

## File Map

### New Files

| File | Responsibility |
|---|---|
| `crates/openalpaca_plugins/src/bridge/connector_bridge.rs` | PluginConnector implementing Connector trait |
| `crates/openalpaca_plugins/src/bridge/provider_bridge.rs` | PluginLlmProvider implementing LlmProvider trait |

### Modified Files

| File | Change |
|---|---|
| `crates/openalpaca_connectors/src/lib.rs` | Change `Connector::name() -> &str` |
| `crates/openalpaca_connectors/src/telegram/connector.rs` | Update trait impl return type |
| `crates/openalpaca_connectors/src/discord/connector.rs` | Update trait impl return type |
| `crates/openalpaca_connectors/src/imessage/connector.rs` | Update trait impl return type (if exists) |
| `crates/openalpaca_connectors/src/startup.rs` | Add `ConnectorHandle::Plugin` variant |
| `crates/openalpaca_llm/src/keys/key_pool/mod.rs` | Add `ProviderType::Plugin(String)`, remove `Copy` |
| `crates/openalpaca_llm/src/routing/router/mod.rs` | Add `deregister_provider()` |
| `crates/openalpaca_llm/src/routing/model_registry/mod.rs` | Add `remove()` method |
| `crates/openalpaca_plugins/src/bridge/mod.rs` | Add connector + provider bridge exports |
| `crates/openalpaca_plugins/src/manager.rs` | Wire connector + provider discovery into hot-load |
| `crates/openalpaca_plugins/src/lib.rs` | Re-export new bridge types |

---

## Chunk 1: Connector Prep Refactors

### Task 1: Change Connector::name() from &'static str to &str

This is a breaking change to the trait but the fix for each implementor is trivial.

**Files:**
- Modify: `crates/openalpaca_connectors/src/lib.rs:44`
- Modify: `crates/openalpaca_connectors/src/telegram/connector.rs:242`
- Modify: `crates/openalpaca_connectors/src/discord/connector.rs:504`
- Modify: `crates/openalpaca_connectors/src/imessage/connector.rs` (find Connector impl)

- [ ] **Step 1: Change the trait definition**

In `crates/openalpaca_connectors/src/lib.rs`, line 44:
```rust
// BEFORE:
fn name(&self) -> &'static str;

// AFTER:
fn name(&self) -> &str;
```

- [ ] **Step 2: Update Telegram impl**

In `crates/openalpaca_connectors/src/telegram/connector.rs`, line ~242:
```rust
// BEFORE:
fn name(&self) -> &'static str {

// AFTER:
fn name(&self) -> &str {
```

- [ ] **Step 3: Update Discord impl**

In `crates/openalpaca_connectors/src/discord/connector.rs`, line ~504:
```rust
// BEFORE:
fn name(&self) -> &'static str {

// AFTER:
fn name(&self) -> &str {
```

- [ ] **Step 4: Update iMessage impl**

Find the `Connector` impl in the imessage module and make the same change.

- [ ] **Step 5: Check for any other Connector impls**

Run: `grep -rn "fn name(&self) -> &'static str" crates/openalpaca_connectors/`
Fix any remaining occurrences.

- [ ] **Step 6: Verify**

Run: `cargo check --all-targets`
Expected: PASS

- [ ] **Step 7: Commit**

```bash
git add crates/openalpaca_connectors/
git commit -m "refactor: change Connector::name() from &'static str to &str

Enables plugin connectors to return dynamic names. All three compiled
connectors (Telegram, Discord, iMessage) updated trivially."
```

---

### Task 2: Add ConnectorHandle::Plugin variant

**Files:**
- Modify: `crates/openalpaca_connectors/src/startup.rs:28-37, 63-96`

- [ ] **Step 1: Add Plugin variant to ConnectorHandle enum**

In `crates/openalpaca_connectors/src/startup.rs`, add after the last feature-gated variant (before `None`):
```rust
    /// Plugin-backed connector managed by PluginManager.
    Plugin(CancellationToken, Arc<AtomicBool>),
```

Make sure `CancellationToken` and `AtomicBool` imports are present (they should be from existing variants).

- [ ] **Step 2: Add match arms for is_alive() and shutdown()**

In `is_alive()`:
```rust
ConnectorHandle::Plugin(_, running) => running.load(Ordering::Acquire),
```

In `shutdown()`:
```rust
ConnectorHandle::Plugin(token, _) => {
    token.cancel();
}
```

- [ ] **Step 3: Verify**

Run: `cargo check --all-targets`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add crates/openalpaca_connectors/
git commit -m "feat: add ConnectorHandle::Plugin variant for plugin connectors"
```

---

## Chunk 2: LLM Provider Prep Refactors

### Task 3: Migrate ProviderType from Copy enum to support Plugin

The `ProviderType` enum is `Copy + Hash + Eq` with 3 variants. Adding `Plugin(String)` breaks `Copy`. The cleanest approach: add the variant and remove `Copy`, then audit all sites that relied on `Copy` (they'll need `.clone()`).

**Files:**
- Modify: `crates/openalpaca_llm/src/keys/key_pool/mod.rs:9-37`
- Fix: All files that use `ProviderType` with `Copy` semantics

- [ ] **Step 1: Add Plugin variant, remove Copy derive**

In `crates/openalpaca_llm/src/keys/key_pool/mod.rs`, line 9:
```rust
// BEFORE:
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderType {
    Anthropic,
    OpenAI,
    Ollama,
}

// AFTER:
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderType {
    Anthropic,
    OpenAI,
    Ollama,
    Plugin(String),
}
```

- [ ] **Step 2: Update all() method**

```rust
pub fn all() -> &'static [ProviderType] {
    &[
        ProviderType::Anthropic,
        ProviderType::OpenAI,
        ProviderType::Ollama,
    ]
}
```
This still returns only the built-in types (plugin providers are dynamic, not static). No change needed.

- [ ] **Step 3: Update Display impl**

```rust
impl std::fmt::Display for ProviderType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Anthropic => write!(f, "anthropic"),
            Self::OpenAI => write!(f, "openai"),
            Self::Ollama => write!(f, "ollama"),
            Self::Plugin(name) => write!(f, "plugin:{name}"),
        }
    }
}
```

- [ ] **Step 4: Fix all compilation errors from removing Copy**

Run `cargo check --all-targets 2>&1 | head -50` and fix each error. Common patterns:
- `*entry.key()` → `entry.key().clone()` (DashMap refs)
- `provider_type` used after move → add `.clone()`
- Match arms taking by value → match by reference

Search for affected files: `grep -rn "ProviderType" crates/openalpaca_llm/src/ | grep -v test`

- [ ] **Step 5: Add Plugin arm to all ProviderType match statements**

Search: `grep -rn "ProviderType::" crates/openalpaca_llm/src/`
For each exhaustive match, add `ProviderType::Plugin(_)` arms. These should typically follow the same pattern as Ollama (local/custom provider).

- [ ] **Step 6: Verify**

Run: `cargo check --all-targets`
Run: `cargo test -p openalpaca_llm`
Expected: All pass

- [ ] **Step 7: Commit**

```bash
git add crates/openalpaca_llm/
git commit -m "refactor: add ProviderType::Plugin variant, remove Copy derive

Enables plugin LLM providers to register with dynamic names.
All match sites updated with Plugin(_) arms."
```

---

### Task 4: Add ModelRegistry::remove() and LlmRouter::deregister_provider()

**Files:**
- Modify: `crates/openalpaca_llm/src/routing/model_registry/mod.rs`
- Modify: `crates/openalpaca_llm/src/routing/router/mod.rs`

- [ ] **Step 1: Add ModelRegistry::remove()**

In `crates/openalpaca_llm/src/routing/model_registry/mod.rs`, after the `register()` method:
```rust
/// Remove a model from the registry. Returns true if it existed.
pub fn remove(&self, model_id: &str) -> bool {
    self.models.write().unwrap().remove(model_id).is_some()
}

/// Remove all models for a given provider type.
pub fn remove_by_provider(&self, provider_type: &ProviderType) -> Vec<String> {
    let mut models = self.models.write().unwrap();
    let to_remove: Vec<String> = models
        .iter()
        .filter(|(_, info)| &info.provider == provider_type)
        .map(|(id, _)| id.clone())
        .collect();
    for id in &to_remove {
        models.remove(id);
    }
    to_remove
}
```

- [ ] **Step 2: Add LlmRouter::deregister_provider()**

In `crates/openalpaca_llm/src/routing/router/mod.rs`, after `register_provider()`:
```rust
/// Remove a provider and all its registered models.
/// Returns the list of model IDs that were removed.
pub fn deregister_provider(&self, provider_type: &ProviderType) -> Vec<String> {
    self.providers.remove(provider_type);
    self.model_registry.remove_by_provider(provider_type)
}
```

- [ ] **Step 3: Verify**

Run: `cargo check --all-targets`
Run: `cargo test -p openalpaca_llm`
Expected: All pass

- [ ] **Step 4: Commit**

```bash
git add crates/openalpaca_llm/
git commit -m "feat: add ModelRegistry::remove() and LlmRouter::deregister_provider()

Enables runtime removal of plugin providers and their models
during plugin hot-unload."
```

---

## Chunk 3: Connector Bridge

### Task 5: Implement PluginConnector

**Files:**
- Create: `crates/openalpaca_plugins/src/bridge/connector_bridge.rs`
- Modify: `crates/openalpaca_plugins/src/bridge/mod.rs`

- [ ] **Step 1: Write connector_bridge.rs**

```rust
// crates/openalpaca_plugins/src/bridge/connector_bridge.rs
use crate::stdio_channel::StdioChannel;
use async_trait::async_trait;
use openalpaca_connectors::Connector;
use openalpaca_connectors::ConnectorError;
use serde_json::Value;
use tracing::{debug, error, info};

/// A connector backed by a plugin subprocess.
/// Implements the Connector trait by proxying to connector/* JSON-RPC methods.
pub struct PluginConnector {
    plugin_id: String,
    platform: String,
    channel: StdioChannel,
}

impl PluginConnector {
    pub fn new(plugin_id: String, platform: String, channel: StdioChannel) -> Self {
        Self {
            plugin_id,
            platform,
            channel,
        }
    }

    /// Send a message to a chat via the plugin connector.
    pub async fn send_message(
        &self,
        chat_id: &str,
        text: &str,
        reply_to: Option<&str>,
        files: &[String],
    ) -> Result<Value, String> {
        let params = serde_json::json!({
            "chat_id": chat_id,
            "text": text,
            "reply_to": reply_to,
            "files": files,
        });
        self.channel
            .call("connector/send", params)
            .await
            .map_err(|e| format!("plugin connector {}: {}", self.plugin_id, e))
    }
}

#[async_trait]
impl Connector for PluginConnector {
    fn name(&self) -> &str {
        &self.platform
    }

    async fn run(&self) -> Result<(), ConnectorError> {
        info!(
            plugin = %self.plugin_id,
            platform = %self.platform,
            "starting plugin connector"
        );

        // Send connector/start to the plugin
        if let Err(e) = self.channel.call("connector/start", Value::Object(Default::default())).await {
            error!(plugin = %self.plugin_id, error = %e, "connector/start failed");
            return Err(ConnectorError::StartupError(format!(
                "plugin {} connector/start: {}",
                self.plugin_id, e
            )));
        }

        debug!(plugin = %self.plugin_id, "plugin connector started, listening for events");

        // The connector runs by the plugin pushing $/event notifications
        // via the StdioChannel's notification_rx. The PluginManager's
        // EventRelay handles those. This method just needs to stay alive
        // until shutdown is requested.
        //
        // In practice, the PluginProcess holds the notification_rx and
        // the PluginManager runs the event relay loop. This run() method
        // is not called directly — the plugin's process lifecycle is
        // managed by PluginManager. This impl exists to satisfy the trait.
        Ok(())
    }

    async fn shutdown(&self) -> Result<(), ConnectorError> {
        info!(plugin = %self.plugin_id, "shutting down plugin connector");
        let _ = self.channel.call("connector/stop", Value::Object(Default::default())).await;
        Ok(())
    }
}
```

- [ ] **Step 2: Add to bridge/mod.rs**

```rust
pub mod connector_bridge;
pub use connector_bridge::PluginConnector;
```

- [ ] **Step 3: Add openalpaca_connectors dep to openalpaca_plugins Cargo.toml**

Check if it's already there. If not, add:
```toml
openalpaca_connectors = { workspace = true }
```

- [ ] **Step 4: Verify**

Run: `cargo check -p openalpaca_plugins`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add crates/openalpaca_plugins/
git commit -m "feat: add PluginConnector implementing Connector trait

Proxies connector/start, connector/stop, and connector/send
to plugin processes via StdioChannel."
```

---

## Chunk 4: Provider Bridge

### Task 6: Implement PluginLlmProvider

**Files:**
- Create: `crates/openalpaca_plugins/src/bridge/provider_bridge.rs`
- Modify: `crates/openalpaca_plugins/src/bridge/mod.rs`

- [ ] **Step 1: Write provider_bridge.rs**

```rust
// crates/openalpaca_plugins/src/bridge/provider_bridge.rs
use crate::stdio_channel::StdioChannel;
use async_trait::async_trait;
use openalpaca_llm::{
    ChatMessage, ChatRequest, ChatResponse, ChatStream, LlmError, LlmProvider, ToolDefinition,
};
use serde_json::Value;
use tracing::{debug, warn};

/// An LLM provider backed by a plugin subprocess.
/// Implements LlmProvider by proxying to provider/* JSON-RPC methods.
pub struct PluginLlmProvider {
    plugin_id: String,
    provider_name: String,
    supports_tools: bool,
    supports_streaming: bool,
    channel: StdioChannel,
}

impl PluginLlmProvider {
    pub fn new(
        plugin_id: String,
        provider_name: String,
        supports_tools: bool,
        supports_streaming: bool,
        channel: StdioChannel,
    ) -> Self {
        Self {
            plugin_id,
            provider_name,
            supports_tools,
            supports_streaming,
            channel,
        }
    }

    /// Convert a ChatRequest to OpenAI-compatible JSON for the wire format.
    fn request_to_json(request: &ChatRequest) -> Value {
        let messages: Vec<Value> = request
            .messages
            .iter()
            .map(|m| {
                serde_json::json!({
                    "role": m.role_str(),
                    "content": m.content_str(),
                })
            })
            .collect();

        let mut params = serde_json::json!({
            "messages": messages,
        });

        if let Some(model) = &request.model {
            params["model"] = Value::String(model.clone());
        }
        if let Some(temp) = request.temperature {
            params["temperature"] = serde_json::json!(temp);
        }
        if let Some(max) = request.max_tokens {
            params["max_tokens"] = serde_json::json!(max);
        }
        // Tools are serialized if the provider supports them
        if !request.tools.is_empty() {
            params["tools"] = serde_json::to_value(&request.tools).unwrap_or_default();
        }

        params
    }

    /// Parse an OpenAI-compatible response JSON into ChatResponse.
    fn json_to_response(result: Value) -> Result<ChatResponse, LlmError> {
        // The plugin returns OpenAI-format response
        // { choices: [{ message: { role, content, tool_calls } }], usage: { ... } }
        let choices = result.get("choices").and_then(|c| c.as_array());
        if let Some(choices) = choices {
            if let Some(choice) = choices.first() {
                let message = choice.get("message").cloned().unwrap_or_default();
                let content = message
                    .get("content")
                    .and_then(|c| c.as_str())
                    .unwrap_or("")
                    .to_string();

                let tool_calls = message
                    .get("tool_calls")
                    .and_then(|tc| tc.as_array())
                    .cloned();

                let usage = result.get("usage").cloned();

                return Ok(ChatResponse {
                    content,
                    tool_calls: tool_calls.unwrap_or_default(),
                    usage,
                    model: result
                        .get("model")
                        .and_then(|m| m.as_str())
                        .map(|s| s.to_string()),
                });
            }
        }

        // Fallback: treat entire result as content
        Ok(ChatResponse {
            content: result.to_string(),
            tool_calls: vec![],
            usage: None,
            model: None,
        })
    }
}

#[async_trait]
impl LlmProvider for PluginLlmProvider {
    fn name(&self) -> &str {
        &self.provider_name
    }

    fn supports_tools(&self) -> bool {
        self.supports_tools
    }

    fn supports_streaming(&self) -> bool {
        self.supports_streaming
    }

    async fn chat(&self, request: ChatRequest) -> Result<ChatResponse, LlmError> {
        let params = Self::request_to_json(&request);

        match self.channel.call("provider/chat", params).await {
            Ok(result) => Self::json_to_response(result),
            Err(e) => Err(LlmError::ApiError(format!(
                "plugin provider {}: {}",
                self.plugin_id, e
            ))),
        }
    }
}
```

**IMPORTANT:** Read the actual `ChatRequest`, `ChatResponse`, `ChatMessage`, and `LlmError` types from the codebase. The struct above is a starting point — adapt the field names and serialization to match what actually exists. In particular:
- `ChatMessage` may not have `role_str()` / `content_str()` methods — check how existing providers serialize requests
- `ChatResponse` may have different fields — check the actual struct definition
- `LlmError` may not have an `ApiError` variant — use whatever error variant exists

- [ ] **Step 2: Add to bridge/mod.rs**

```rust
pub mod provider_bridge;
pub use provider_bridge::PluginLlmProvider;
```

- [ ] **Step 3: Verify**

Run: `cargo check -p openalpaca_plugins`
Expected: PASS

- [ ] **Step 4: Commit**

```bash
git add crates/openalpaca_plugins/
git commit -m "feat: add PluginLlmProvider implementing LlmProvider trait

Proxies provider/chat to plugin processes via StdioChannel.
Uses OpenAI-compatible JSON as the wire format."
```

---

## Chunk 5: Wire Bridges into PluginManager

### Task 7: Extend PluginManager to discover and register connectors + providers

**Files:**
- Modify: `crates/openalpaca_plugins/src/manager.rs`
- Modify: `crates/openalpaca_plugins/src/lib.rs`

- [ ] **Step 1: Add connector and provider fields to PluginManager**

The PluginManager needs access to `ConnectorManager` (for registering connectors) and `LlmRouter` (for registering providers). But for now, we can track them in PluginState and let the daemon wire up the actual registration.

Add to `PluginState`:
```rust
pub registered_connector: Option<String>,  // connector platform name
pub registered_provider: Option<String>,   // provider name
pub registered_models: Vec<String>,        // model IDs from provider
```

- [ ] **Step 2: Add connector discovery to hot-load sequence**

After tool discovery in `spawn_plugin()`, add:
```rust
// Discover connector
if manifest.types.connector {
    match process.channel.call("connector/info", Value::Object(Default::default())).await {
        Ok(info) => {
            let platform = info.get("platform")
                .and_then(|p| p.as_str())
                .unwrap_or(&manifest.plugin.name)
                .to_string();
            // Store connector info — actual ConnectorManager registration
            // is done by the daemon after PluginManager reports the plugin loaded
            registered_connector = Some(platform);
        }
        Err(e) => warn!(plugin = %name, error = %e, "connector/info failed"),
    }
}
```

- [ ] **Step 3: Add provider discovery to hot-load sequence**

After connector discovery:
```rust
// Discover provider
let mut registered_models = Vec::new();
if manifest.types.provider {
    match process.channel.call("provider/info", Value::Object(Default::default())).await {
        Ok(info) => {
            let provider_name = info.get("provider_name")
                .and_then(|p| p.as_str())
                .unwrap_or(&manifest.plugin.name)
                .to_string();
            if let Some(models) = info.get("models").and_then(|m| m.as_array()) {
                for model in models {
                    if let Some(model_id) = model.get("id").and_then(|id| id.as_str()) {
                        registered_models.push(model_id.to_string());
                    }
                }
            }
            registered_provider = Some(provider_name);
        }
        Err(e) => warn!(plugin = %name, error = %e, "provider/info failed"),
    }
}
```

- [ ] **Step 4: Update PluginState construction to include new fields**

- [ ] **Step 5: Add methods to get connector/provider info for daemon wiring**

```rust
/// Get the connector bridge for a loaded plugin, if it's a connector plugin.
pub async fn get_plugin_connector(&self, name: &str) -> Option<PluginConnector> {
    let plugins = self.plugins.read().await;
    let state = plugins.get(name)?;
    let platform = state.registered_connector.as_ref()?.clone();
    let channel = state.process.as_ref()?.channel.clone();
    Some(PluginConnector::new(name.to_string(), platform, channel))
}

/// Get the provider bridge for a loaded plugin, if it's a provider plugin.
pub async fn get_plugin_provider(&self, name: &str) -> Option<(PluginLlmProvider, Vec<String>)> {
    let plugins = self.plugins.read().await;
    let state = plugins.get(name)?;
    let provider_name = state.registered_provider.as_ref()?.clone();
    let channel = state.process.as_ref()?.channel.clone();
    let models = state.registered_models.clone();
    let provider = PluginLlmProvider::new(
        name.to_string(),
        provider_name,
        true, // supports_tools — from provider/info
        true, // supports_streaming — from provider/info
        channel,
    );
    Some((provider, models))
}
```

- [ ] **Step 6: Update list_plugins() to include connector/provider info**

Update the return type to include richer information (or add a separate method).

- [ ] **Step 7: Update unload_plugin() to clean up connector/provider state**

When unloading, also clear `registered_connector`, `registered_provider`, `registered_models`.

- [ ] **Step 8: Verify**

Run: `cargo check --all-targets`
Expected: PASS

- [ ] **Step 9: Commit**

```bash
git add crates/openalpaca_plugins/
git commit -m "feat: extend PluginManager with connector and provider discovery

Hot-load sequence now calls connector/info and provider/info.
PluginState tracks registered connector platform, provider name,
and model IDs. Accessor methods for daemon bridge wiring."
```

---

## Final Verification

- [ ] **Full workspace build**: `cargo build --all-targets`
- [ ] **All tests pass**: `cargo test --workspace`
- [ ] **No new warnings**: `cargo clippy --all-targets`
