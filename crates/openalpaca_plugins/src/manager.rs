use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use serde_json::Value;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use openalpaca_core::agent::registry::AgentRegistry;
use openalpaca_core::agent::template::{AgentSource, AgentTemplate, AgentTemplateFrontmatter};
use openalpaca_core::middleware::skill::{
    InvokeConfig, RoutingConfig, SkillFrontmatter,
};
use openalpaca_core::orchestrator::skill_catalog::SkillCatalog;
use openalpaca_core::tools::registry::{RegisteredTool, ToolBackend};
use openalpaca_core::tools::ToolRegistry;
use openalpaca_llm::ToolDefinition;

use crate::bridge::{PluginAgentBridge, PluginSkillBridge, PluginToolProxy};
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
    pub registered_connector: Option<String>,
    pub registered_provider: Option<String>,
    pub registered_models: Vec<String>,
    pub registered_skills: Vec<String>,
    pub registered_agents: Vec<String>,
    pub restart_count: u32,
    pub last_health: Option<Instant>,
    pub plugin_dir: PathBuf,
}

// ── PluginInfo ──────────────────────────────────────────────────────

/// Summary of a loaded plugin returned by [`PluginManager::list_plugins`].
#[derive(Debug, Clone)]
pub struct PluginInfo {
    pub name: String,
    pub version: String,
    pub status: String,
    pub tools: Vec<String>,
    pub connector: Option<String>,
    pub provider: Option<String>,
    pub models: Vec<String>,
    pub skills: Vec<String>,
    pub agents: Vec<String>,
}

// ── PluginManager ───────────────────────────────────────────────────────

/// Core orchestrator for plugin lifecycle: discovery, hot-load/unload,
/// permission gating, tool registration, and state tracking.
pub struct PluginManager {
    plugins: Arc<RwLock<HashMap<String, PluginState>>>,
    plugin_dir: PathBuf,
    permission_gate: PermissionGate,
    tool_registry: Arc<ToolRegistry>,
    skill_catalog: Option<Arc<SkillCatalog>>,
    agent_registry: Option<Arc<AgentRegistry>>,
}

impl PluginManager {
    /// Create a new `PluginManager`.
    ///
    /// - `plugin_dir`: root directory containing plugin subdirectories.
    /// - `tool_registry`: shared tool registry where discovered plugin tools are registered.
    /// - `skill_catalog`: optional skill catalog for registering plugin-backed skills.
    /// - `agent_registry`: optional agent registry for registering plugin-backed agents.
    pub fn new(
        plugin_dir: PathBuf,
        tool_registry: Arc<ToolRegistry>,
        skill_catalog: Option<Arc<SkillCatalog>>,
        agent_registry: Option<Arc<AgentRegistry>>,
    ) -> Self {
        let permission_gate = PermissionGate::new(&plugin_dir);
        Self {
            plugins: Arc::new(RwLock::new(HashMap::new())),
            plugin_dir,
            permission_gate,
            tool_registry,
            skill_catalog,
            agent_registry,
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
                    registered_connector: None,
                    registered_provider: None,
                    registered_models: Vec::new(),
                    registered_skills: Vec::new(),
                    registered_agents: Vec::new(),
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

        // Discover connector
        let mut registered_connector = None;
        if manifest.types.connector {
            match process.channel.call("connector/info", Value::Object(Default::default())).await {
                Ok(info) => {
                    let platform = info.get("platform")
                        .and_then(|p| p.as_str())
                        .unwrap_or(&name)
                        .to_string();
                    info!(plugin = %name, platform = %platform, "discovered connector");
                    registered_connector = Some(platform);
                }
                Err(e) => warn!(plugin = %name, error = %e, "connector/info failed"),
            }
        }

        // Discover provider
        let mut registered_provider = None;
        let mut registered_models = Vec::new();
        if manifest.types.provider {
            match process.channel.call("provider/info", Value::Object(Default::default())).await {
                Ok(info) => {
                    let provider_name = info.get("provider_name")
                        .and_then(|p| p.as_str())
                        .unwrap_or(&name)
                        .to_string();
                    if let Some(models) = info.get("models").and_then(|m| m.as_array()) {
                        for model in models {
                            if let Some(model_id) = model.get("id").and_then(|id| id.as_str()) {
                                registered_models.push(model_id.to_string());
                            }
                        }
                    }
                    info!(plugin = %name, provider = %provider_name, models = registered_models.len(), "discovered provider");
                    registered_provider = Some(provider_name);
                }
                Err(e) => warn!(plugin = %name, error = %e, "provider/info failed"),
            }
        }

        // Discover skill
        let mut registered_skills = Vec::new();
        if manifest.types.skill {
            match process.channel.call("skill/info", Value::Object(Default::default())).await {
                Ok(info) => {
                    let id = info.get("id")
                        .and_then(|i| i.as_str())
                        .unwrap_or(&name)
                        .to_string();

                    if let Some(ref catalog) = self.skill_catalog {
                        let bridge = Arc::new(PluginSkillBridge::new(
                            name.clone(),
                            id.clone(),
                            process.channel.clone(),
                        ));

                        let frontmatter = build_skill_frontmatter_from_info(&info, &name);

                        catalog.register_plugin_skill(
                            id.clone(),
                            frontmatter,
                            bridge,
                            name.clone(),
                        );
                        registered_skills.push(id.clone());
                        info!(plugin = %name, skill = %id, "registered plugin skill");
                    }
                }
                Err(e) => warn!(plugin = %name, error = %e, "skill/info failed"),
            }
        }

        // Discover agent
        let mut registered_agents = Vec::new();
        if manifest.types.agent {
            match process.channel.call("agent/info", Value::Object(Default::default())).await {
                Ok(info) => {
                    let id = info.get("id")
                        .and_then(|i| i.as_str())
                        .unwrap_or(&name)
                        .to_string();

                    if let Some(ref registry) = self.agent_registry {
                        let bridge = Arc::new(PluginAgentBridge::new(
                            name.clone(),
                            id.clone(),
                            process.channel.clone(),
                        ));

                        let template = build_agent_template_from_info(&info, &name, bridge);

                        registry.register_template(template);
                        registered_agents.push(id.clone());
                        info!(plugin = %name, agent = %id, "registered plugin agent");
                    }
                }
                Err(e) => warn!(plugin = %name, error = %e, "agent/info failed"),
            }
        }

        // Step 8: Track state as Running
        {
            let mut plugins = self.plugins.write().await;
            if let Some(state) = plugins.get_mut(&name) {
                state.status = PluginStatus::Running;
                state.process = Some(process);
                state.registered_tools = registered_tools;
                state.registered_connector = registered_connector;
                state.registered_provider = registered_provider;
                state.registered_models = registered_models;
                state.registered_skills = registered_skills;
                state.registered_agents = registered_agents;
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

        // Unregister plugin skills from the skill catalog
        if let Some(ref catalog) = self.skill_catalog {
            for skill_id in &state.registered_skills {
                catalog.remove(skill_id);
                debug!(plugin = name, skill = %skill_id, "unregistered plugin skill");
            }
        }

        // Unregister plugin agent templates from the agent registry
        if let Some(ref registry) = self.agent_registry {
            for agent_id in &state.registered_agents {
                registry.remove_template(agent_id);
                debug!(plugin = name, agent = %agent_id, "unregistered plugin agent template");
            }
        }

        // NOTE: Connector deregistration from ConnectorManager and provider
        // deregistration from LlmRouter are the daemon's responsibility, since
        // PluginManager does not hold references to those subsystems.

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
                    registered_connector: None,
                    registered_provider: None,
                    registered_models: Vec::new(),
                    registered_skills: Vec::new(),
                    registered_agents: Vec::new(),
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

    /// List all tracked plugins with full metadata.
    pub async fn list_plugins(&self) -> Vec<PluginInfo> {
        let plugins = self.plugins.read().await;
        plugins
            .iter()
            .map(|(name, state)| PluginInfo {
                name: name.clone(),
                version: state.manifest.plugin.version.clone(),
                status: state.status.to_string(),
                tools: state.registered_tools.clone(),
                connector: state.registered_connector.clone(),
                provider: state.registered_provider.clone(),
                models: state.registered_models.clone(),
                skills: state.registered_skills.clone(),
                agents: state.registered_agents.clone(),
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

    // ── bridge accessors ─────────────────────────────────────────────

    /// Get the [`PluginConnector`](crate::bridge::PluginConnector) for a loaded
    /// connector plugin, or `None` if the plugin is not loaded or does not
    /// provide a connector.
    pub async fn get_plugin_connector(&self, name: &str) -> Option<crate::bridge::PluginConnector> {
        let plugins = self.plugins.read().await;
        let state = plugins.get(name)?;
        let platform = state.registered_connector.as_ref()?.clone();
        let channel = state.process.as_ref()?.channel.clone();
        Some(crate::bridge::PluginConnector::new(name.to_string(), platform, channel))
    }

    /// Get the [`PluginLlmProvider`](crate::bridge::PluginLlmProvider) for a
    /// loaded provider plugin, along with its discovered model IDs. Returns
    /// `None` if the plugin is not loaded or does not provide an LLM provider.
    pub async fn get_plugin_provider(&self, name: &str) -> Option<(crate::bridge::PluginLlmProvider, Vec<String>)> {
        let plugins = self.plugins.read().await;
        let state = plugins.get(name)?;
        let provider_name = state.registered_provider.as_ref()?.clone();
        let channel = state.process.as_ref()?.channel.clone();
        let models = state.registered_models.clone();
        let provider = crate::bridge::PluginLlmProvider::new(
            name.to_string(),
            provider_name,
            true,  // supports_tools (TODO: get from provider/info)
            false, // supports_streaming (TODO: get from provider/info)
            channel,
        );
        Some((provider, models))
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
                annotations: None,
            };

            match self.tool_registry.register(registered_tool) {
                Ok(()) => {
                    registered.push(namespaced_name.clone());
                    debug!(
                        plugin = plugin_name,
                        tool = %namespaced_name,
                        "registered plugin tool"
                    );
                }
                Err(e) => {
                    warn!(
                        plugin = plugin_name,
                        tool = %namespaced_name,
                        error = %e,
                        "failed to register plugin tool — skipping"
                    );
                }
            }
        }

        info!(
            plugin = plugin_name,
            count = registered.len(),
            "discovered and registered tools"
        );

        Ok(registered)
    }
}

/// Build a [`SkillFrontmatter`] from a `skill/info` JSON response.
///
/// Extracts name, description, invoke config, and routing patterns from the
/// plugin's response, using sensible defaults for any missing fields.
fn build_skill_frontmatter_from_info(info: &Value, plugin_name: &str) -> SkillFrontmatter {
    let name = info
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or(plugin_name)
        .to_string();

    let description = info
        .get("description")
        .and_then(|d| d.as_str())
        .unwrap_or("")
        .to_string();

    let mode = info
        .get("invoke")
        .and_then(|inv| inv.get("mode"))
        .and_then(|m| m.as_str())
        .unwrap_or("manual")
        .to_string();

    let slash = info
        .get("invoke")
        .and_then(|inv| inv.get("slash"))
        .and_then(|s| s.as_str())
        .map(|s| s.to_string());

    let aliases: Vec<String> = info
        .get("invoke")
        .and_then(|inv| inv.get("aliases"))
        .and_then(|a| a.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let intent: Vec<String> = info
        .get("routing")
        .and_then(|r| r.get("intent"))
        .and_then(|i| i.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let keywords: Vec<String> = info
        .get("routing")
        .and_then(|r| r.get("keywords"))
        .and_then(|k| k.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    SkillFrontmatter {
        name,
        description,
        invoke: InvokeConfig {
            mode,
            slash,
            aliases,
            ..Default::default()
        },
        routing: RoutingConfig {
            intent,
            keywords,
            ..Default::default()
        },
        ..Default::default()
    }
}

/// Build an [`AgentTemplate`] from an `agent/info` JSON response.
///
/// Creates a minimal template with the plugin bridge as the execution source.
fn build_agent_template_from_info(
    info: &Value,
    plugin_name: &str,
    bridge: Arc<PluginAgentBridge>,
) -> AgentTemplate {
    let id = info
        .get("id")
        .and_then(|i| i.as_str())
        .unwrap_or(plugin_name)
        .to_string();

    let name = info
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or(plugin_name)
        .to_string();

    let description = info
        .get("description")
        .and_then(|d| d.as_str())
        .unwrap_or("")
        .to_string();

    let capabilities: Vec<String> = info
        .get("capabilities")
        .and_then(|c| c.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let model = info
        .get("model")
        .and_then(|m| m.as_str())
        .map(|s| s.to_string());

    let singleton = info
        .get("singleton")
        .and_then(|s| s.as_bool())
        .unwrap_or(false);

    AgentTemplate {
        frontmatter: AgentTemplateFrontmatter {
            id,
            name,
            description,
            icon: None,
            singleton,
            capabilities,
            denied_capabilities: Vec::new(),
            temperature: 0.5,
            verbosity: "normal".to_string(),
            model,
            fallback_models: Vec::new(),
            max_tool_calls: None,
            timeout_seconds: None,
            max_cost_per_task: None,
            max_rounds: None,
            require_confirmation_for: Vec::new(),
        },
        body: String::new(),
        sections: HashMap::new(),
        source: AgentSource::Plugin {
            plugin_id: plugin_name.to_string(),
            executor: bridge,
        },
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
