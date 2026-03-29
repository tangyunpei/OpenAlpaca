use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use serde_json::Value;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use openalpaca_core::tools::registry::{RegisteredTool, ToolBackend};
use openalpaca_core::tools::ToolRegistry;
use openalpaca_llm::ToolDefinition;

use crate::bridge::PluginToolProxy;
use crate::error::PluginError;
use crate::manifest::PluginManifest;
use crate::permission_gate::PermissionGate;
use crate::process_pool::PluginProcess;

// ── PluginStatus ────────────────────────────────────────────────────────

/// Current lifecycle status of a plugin.
#[derive(Debug, Clone)]
pub enum PluginStatus {
    /// Manifest parsed, load in progress.
    Loading,
    /// First-time load, waiting for user approval.
    WaitingApproval,
    /// Plugin requires configuration keys before it can start.
    NeedsConfig { missing_keys: Vec<String> },
    /// Plugin process is running and tools are registered.
    Running,
    /// Plugin process crashed; will retry after `backoff_until`.
    Crashed {
        error: String,
        backoff_until: Instant,
    },
    /// Explicitly disabled by the user.
    Disabled,
    /// Gracefully stopped (unloaded).
    Stopped,
}

impl fmt::Display for PluginStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PluginStatus::Loading => write!(f, "loading"),
            PluginStatus::WaitingApproval => write!(f, "waiting-approval"),
            PluginStatus::NeedsConfig { missing_keys } => {
                write!(f, "needs-config ({})", missing_keys.join(", "))
            }
            PluginStatus::Running => write!(f, "running"),
            PluginStatus::Crashed { error, .. } => write!(f, "crashed: {error}"),
            PluginStatus::Disabled => write!(f, "disabled"),
            PluginStatus::Stopped => write!(f, "stopped"),
        }
    }
}

// ── PluginState ─────────────────────────────────────────────────────────

/// Runtime state for a single loaded plugin.
pub struct PluginState {
    pub manifest: PluginManifest,
    pub status: PluginStatus,
    pub process: Option<PluginProcess>,
    pub registered_tools: Vec<String>,
    pub restart_count: u32,
    pub last_health: Option<Instant>,
    pub plugin_dir: PathBuf,
}

// ── PluginManager ───────────────────────────────────────────────────────

/// Core orchestrator for plugin lifecycle: discovery, hot-load/unload,
/// permission gating, tool registration, and state tracking.
pub struct PluginManager {
    plugins: Arc<RwLock<HashMap<String, PluginState>>>,
    plugin_dir: PathBuf,
    permission_gate: PermissionGate,
    tool_registry: Arc<ToolRegistry>,
}

impl PluginManager {
    /// Create a new `PluginManager`.
    ///
    /// - `plugin_dir`: root directory containing plugin subdirectories.
    /// - `tool_registry`: shared tool registry where discovered plugin tools are registered.
    pub fn new(plugin_dir: PathBuf, tool_registry: Arc<ToolRegistry>) -> Self {
        let permission_gate = PermissionGate::new(&plugin_dir);
        Self {
            plugins: Arc::new(RwLock::new(HashMap::new())),
            plugin_dir,
            permission_gate,
            tool_registry,
        }
    }

    /// Scan the plugin directory and attempt to load all plugins.
    ///
    /// Each subdirectory containing a `plugin.toml` is treated as a plugin.
    /// Errors in individual plugins are logged but do not abort the scan.
    pub async fn start(&self) -> Result<(), PluginError> {
        info!(dir = %self.plugin_dir.display(), "scanning plugin directory");

        let mut entries = tokio::fs::read_dir(&self.plugin_dir).await.map_err(|e| {
            PluginError::Io(std::io::Error::new(
                e.kind(),
                format!("failed to read plugin dir {}: {}", self.plugin_dir.display(), e),
            ))
        })?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            // Only consider directories that contain a plugin.toml
            if !path.join("plugin.toml").exists() {
                continue;
            }
            if let Err(e) = self.try_load_plugin(&path).await {
                warn!(
                    plugin_dir = %path.display(),
                    error = %e,
                    "failed to load plugin, skipping"
                );
            }
        }

        let plugins = self.plugins.read().await;
        info!(count = plugins.len(), "plugin scan complete");
        Ok(())
    }

    /// Load a single plugin from its directory.
    ///
    /// # Hot-load sequence
    ///
    /// 1. Parse `plugin.toml`
    /// 2. Check approval via `PermissionGate`
    /// 3. Validate required config keys
    /// 4. Spawn the plugin process
    /// 5. Send `initialize` RPC (for non-MCP plugins)
    /// 6. Discover tools via `tools/list` RPC
    /// 7. Register discovered tools in the shared `ToolRegistry`
    /// 8. Track the plugin as `Running`
    pub async fn try_load_plugin(&self, plugin_dir: &Path) -> Result<(), PluginError> {
        // Step 1: Parse manifest
        let manifest = PluginManifest::from_dir(plugin_dir)?;
        let name = manifest.plugin.name.clone();
        info!(plugin = %name, "loading plugin");

        // Insert initial Loading state
        {
            let mut plugins = self.plugins.write().await;
            plugins.insert(
                name.clone(),
                PluginState {
                    manifest: manifest.clone(),
                    status: PluginStatus::Loading,
                    process: None,
                    registered_tools: Vec::new(),
                    restart_count: 0,
                    last_health: None,
                    plugin_dir: plugin_dir.to_path_buf(),
                },
            );
        }

        // Step 2: Check approval
        match self.permission_gate.is_approved(&name) {
            None => {
                // Never seen — park in WaitingApproval
                info!(plugin = %name, "plugin awaiting approval");
                let mut plugins = self.plugins.write().await;
                if let Some(state) = plugins.get_mut(&name) {
                    state.status = PluginStatus::WaitingApproval;
                }
                return Ok(());
            }
            Some(false) => {
                // Explicitly denied
                debug!(plugin = %name, "plugin is denied, marking disabled");
                let mut plugins = self.plugins.write().await;
                if let Some(state) = plugins.get_mut(&name) {
                    state.status = PluginStatus::Disabled;
                }
                return Ok(());
            }
            Some(true) => {
                // Approved — continue loading
            }
        }

        // Step 3: Config validation
        let provided_config = self.permission_gate.load_plugin_config(&name);
        let missing = manifest.missing_config_keys(&provided_config);
        if !missing.is_empty() {
            info!(
                plugin = %name,
                missing = ?missing,
                "plugin needs configuration"
            );
            let mut plugins = self.plugins.write().await;
            if let Some(state) = plugins.get_mut(&name) {
                state.status = PluginStatus::NeedsConfig {
                    missing_keys: missing,
                };
            }
            return Ok(());
        }

        // Step 4: Spawn process
        let process = PluginProcess::spawn(&manifest, plugin_dir)?;

        // Step 5: Initialize (non-MCP plugins)
        if !manifest.plugin.mcp_compatible {
            let config_json: HashMap<String, Value> = provided_config
                .iter()
                .map(|(k, v)| {
                    let json_val = toml_to_json(v);
                    (k.clone(), json_val)
                })
                .collect();

            process
                .initialize(
                    &name,
                    &manifest.plugin.version,
                    &manifest.capabilities.provides,
                    config_json,
                )
                .await?;
        }

        // Step 6: Discover tools
        let mut registered_tools = Vec::new();
        if manifest.types.tools {
            let tools = self.discover_tools(&name, &manifest, &process).await?;
            registered_tools = tools;
        }

        // Step 8: Track state as Running
        {
            let mut plugins = self.plugins.write().await;
            if let Some(state) = plugins.get_mut(&name) {
                state.status = PluginStatus::Running;
                state.process = Some(process);
                state.registered_tools = registered_tools;
                state.last_health = Some(Instant::now());
            }
        }

        info!(plugin = %name, "plugin loaded successfully");
        Ok(())
    }

    /// Unload a plugin: unregister tools, send shutdown RPC, kill process.
    pub async fn unload_plugin(&self, name: &str) -> Result<(), PluginError> {
        let mut plugins = self.plugins.write().await;
        let state = plugins.remove(name).ok_or_else(|| {
            PluginError::Unavailable(format!("plugin '{}' not found", name))
        })?;

        // Unregister all tools from the shared registry
        for tool_name in &state.registered_tools {
            self.tool_registry.remove(tool_name);
            debug!(plugin = name, tool = %tool_name, "unregistered tool");
        }

        // Graceful shutdown + kill
        if let Some(mut process) = state.process {
            if let Err(e) = process.shutdown().await {
                warn!(plugin = name, error = %e, "shutdown RPC failed, killing");
            }
            process.kill();
        }

        info!(plugin = name, "plugin unloaded");
        Ok(())
    }

    /// Approve a plugin and trigger loading.
    pub async fn approve_plugin(&self, name: &str) -> Result<(), PluginError> {
        // Look up the manifest to get capabilities for the approval record
        let (capabilities, plugin_dir) = {
            let plugins = self.plugins.read().await;
            let state = plugins.get(name).ok_or_else(|| {
                PluginError::Unavailable(format!("plugin '{}' not found", name))
            })?;
            (
                state.manifest.capabilities.provides.clone(),
                state.plugin_dir.clone(),
            )
        };

        self.permission_gate.approve(name, &capabilities)?;
        info!(plugin = name, "plugin approved, loading");

        // Re-trigger load
        self.try_load_plugin(&plugin_dir).await
    }

    /// Deny a plugin.
    pub async fn deny_plugin(&self, name: &str) -> Result<(), PluginError> {
        self.permission_gate.deny(name)?;

        let mut plugins = self.plugins.write().await;
        if let Some(state) = plugins.get_mut(name) {
            state.status = PluginStatus::Disabled;
        }

        info!(plugin = name, "plugin denied");
        Ok(())
    }

    /// Re-enable a disabled plugin and trigger loading.
    pub async fn enable_plugin(&self, name: &str) -> Result<(), PluginError> {
        let plugin_dir = {
            let plugins = self.plugins.read().await;
            let state = plugins.get(name).ok_or_else(|| {
                PluginError::Unavailable(format!("plugin '{}' not found", name))
            })?;
            state.plugin_dir.clone()
        };

        // Record approval
        let capabilities = {
            let plugins = self.plugins.read().await;
            plugins
                .get(name)
                .map(|s| s.manifest.capabilities.provides.clone())
                .unwrap_or_default()
        };
        self.permission_gate.approve(name, &capabilities)?;

        info!(plugin = name, "plugin re-enabled, loading");
        self.try_load_plugin(&plugin_dir).await
    }

    /// Disable a plugin: unload it and mark as disabled.
    pub async fn disable_plugin(&self, name: &str) -> Result<(), PluginError> {
        // Unload first (removes from map, unregisters tools, kills process)
        // Capture manifest and dir before unload removes the entry.
        let (manifest, plugin_dir) = {
            let plugins = self.plugins.read().await;
            let state = plugins.get(name).ok_or_else(|| {
                PluginError::Unavailable(format!("plugin '{}' not found", name))
            })?;
            (state.manifest.clone(), state.plugin_dir.clone())
        };

        // Perform the actual unload
        self.unload_plugin(name).await?;

        // Re-insert with Disabled status so it still appears in listings
        {
            let mut plugins = self.plugins.write().await;
            plugins.insert(
                name.to_string(),
                PluginState {
                    manifest,
                    status: PluginStatus::Disabled,
                    process: None,
                    registered_tools: Vec::new(),
                    restart_count: 0,
                    last_health: None,
                    plugin_dir,
                },
            );
        }

        self.permission_gate.deny(name)?;
        info!(plugin = name, "plugin disabled");
        Ok(())
    }

    /// List all tracked plugins: `(name, version, status, tools)`.
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

    /// Set a configuration key for a plugin, and retry loading if the plugin
    /// was in `NeedsConfig` status.
    pub async fn set_plugin_config(
        &self,
        name: &str,
        key: &str,
        value: toml::Value,
    ) -> Result<(), PluginError> {
        self.permission_gate.set_plugin_config(name, key, value)?;

        // If plugin is in NeedsConfig, re-check and potentially load
        let should_retry = {
            let plugins = self.plugins.read().await;
            plugins
                .get(name)
                .map(|s| matches!(s.status, PluginStatus::NeedsConfig { .. }))
                .unwrap_or(false)
        };

        if should_retry {
            let plugin_dir = {
                let plugins = self.plugins.read().await;
                plugins.get(name).map(|s| s.plugin_dir.clone())
            };
            if let Some(dir) = plugin_dir {
                info!(plugin = name, "config updated, retrying load");
                self.try_load_plugin(&dir).await?;
            }
        }

        Ok(())
    }

    // ── internal helpers ─────────────────────────────────────────────

    /// Discover tools from a running plugin via `tools/list` RPC and register
    /// them in the shared `ToolRegistry`.
    async fn discover_tools(
        &self,
        plugin_name: &str,
        manifest: &PluginManifest,
        process: &PluginProcess,
    ) -> Result<Vec<String>, PluginError> {
        let result = process
            .channel
            .call("tools/list", serde_json::json!({}))
            .await?;

        let tools_array = result
            .get("tools")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();

        let mut registered = Vec::with_capacity(tools_array.len());

        for tool_val in &tools_array {
            let bare_name = tool_val
                .get("name")
                .and_then(|n| n.as_str())
                .unwrap_or_default();
            if bare_name.is_empty() {
                warn!(plugin = plugin_name, "skipping tool with empty name");
                continue;
            }

            let description = tool_val
                .get("description")
                .and_then(|d| d.as_str())
                .unwrap_or("")
                .to_string();

            let input_schema = tool_val
                .get("inputSchema")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({"type": "object"}));

            let namespaced_name = format!("{}::{}", plugin_name, bare_name);

            let definition = ToolDefinition {
                name: namespaced_name.clone(),
                description,
                parameters: input_schema,
                strict: None,
                input_examples: None,
            };

            let proxy = PluginToolProxy::new(
                plugin_name.to_string(),
                process.channel.clone(),
            );

            let registered_tool = RegisteredTool {
                definition,
                backend: ToolBackend::Plugin(Arc::new(proxy)),
                provides_capabilities: manifest.capabilities.provides.clone(),
                exempt_from_timeout: false,
            };

            self.tool_registry.register(registered_tool);
            registered.push(namespaced_name.clone());

            debug!(
                plugin = plugin_name,
                tool = %namespaced_name,
                "registered plugin tool"
            );
        }

        info!(
            plugin = plugin_name,
            count = registered.len(),
            "discovered and registered tools"
        );

        Ok(registered)
    }
}

/// Convert a `toml::Value` to a `serde_json::Value`.
fn toml_to_json(v: &toml::Value) -> Value {
    match v {
        toml::Value::String(s) => Value::String(s.clone()),
        toml::Value::Integer(i) => Value::Number((*i).into()),
        toml::Value::Float(f) => {
            serde_json::Number::from_f64(*f)
                .map(Value::Number)
                .unwrap_or(Value::Null)
        }
        toml::Value::Boolean(b) => Value::Bool(*b),
        toml::Value::Datetime(dt) => Value::String(dt.to_string()),
        toml::Value::Array(arr) => Value::Array(arr.iter().map(toml_to_json).collect()),
        toml::Value::Table(tbl) => {
            let map: serde_json::Map<String, Value> = tbl
                .iter()
                .map(|(k, v)| (k.clone(), toml_to_json(v)))
                .collect();
            Value::Object(map)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plugin_status_display() {
        assert_eq!(PluginStatus::Loading.to_string(), "loading");
        assert_eq!(PluginStatus::WaitingApproval.to_string(), "waiting-approval");
        assert_eq!(PluginStatus::Running.to_string(), "running");
        assert_eq!(PluginStatus::Disabled.to_string(), "disabled");
        assert_eq!(PluginStatus::Stopped.to_string(), "stopped");

        let needs = PluginStatus::NeedsConfig {
            missing_keys: vec!["api_key".into(), "secret".into()],
        };
        assert_eq!(needs.to_string(), "needs-config (api_key, secret)");

        let crashed = PluginStatus::Crashed {
            error: "segfault".into(),
            backoff_until: Instant::now(),
        };
        assert!(crashed.to_string().starts_with("crashed: segfault"));
    }

    #[test]
    fn test_toml_to_json_primitives() {
        assert_eq!(
            toml_to_json(&toml::Value::String("hello".into())),
            Value::String("hello".into())
        );
        assert_eq!(
            toml_to_json(&toml::Value::Integer(42)),
            Value::Number(42.into())
        );
        assert_eq!(
            toml_to_json(&toml::Value::Boolean(true)),
            Value::Bool(true)
        );
    }

    #[test]
    fn test_toml_to_json_nested() {
        let mut tbl = toml::map::Map::new();
        tbl.insert("key".into(), toml::Value::String("val".into()));
        tbl.insert(
            "arr".into(),
            toml::Value::Array(vec![toml::Value::Integer(1), toml::Value::Integer(2)]),
        );
        let json = toml_to_json(&toml::Value::Table(tbl));
        assert!(json.is_object());
        assert_eq!(json["key"], "val");
        assert_eq!(json["arr"], serde_json::json!([1, 2]));
    }
}
