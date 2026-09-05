use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use serde_json::Value;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

use openalpaca_api::events::ServerEvent;
use openalpaca_core::agent::registry::AgentRegistry;
use openalpaca_core::agent::template::{AgentSource, AgentTemplate, AgentTemplateFrontmatter};
use openalpaca_core::middleware::skill::{
    InvokeConfig, RoutingConfig, SkillFrontmatter,
};
use openalpaca_core::orchestrator::skill_catalog::SkillCatalog;
use openalpaca_core::tools::registry::{
    CapabilityProvider, ProviderHandle, RegisteredTool, ToolBackend,
};
use openalpaca_core::tools::ToolRegistry;
use openalpaca_llm::ToolDefinition;

use crate::bridge::{PluginAgentBridge, PluginSkillBridge, PluginToolProxy};
use crate::error::PluginError;
use crate::manifest::PluginManifest;
use crate::permission_gate::PermissionGate;
use crate::process_pool::PluginProcess;

/// How long a teardown waits for a killed plugin child to actually exit
/// before giving up and letting the kernel finish the job.
const CHILD_EXIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

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
    /// The user refused the first-load approval. A consent decision, not a
    /// toggle position: `disabled` says "off for now", `denied` says "never
    /// without a fresh approval".
    Denied,
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
            PluginStatus::Denied => write!(f, "denied"),
            PluginStatus::Stopped => write!(f, "stopped"),
        }
    }
}

// ── PluginCapabilityProvider ────────────────────────────────────────────

/// A capability provider synthesized from a plugin's manifest-declared
/// virtual capabilities. In-process; no RPC cost at lookup time.
///
/// Emits the declared caps for every tool whose `author` field matches
/// `plugin:<plugin_name>` — i.e., every tool this plugin registered.
pub(crate) struct PluginCapabilityProvider {
    #[allow(dead_code)]
    plugin_name: String,
    author_prefix: String,
    virtual_caps: Vec<String>,
}

impl PluginCapabilityProvider {
    pub(crate) fn new(plugin_name: String, virtual_caps: Vec<String>) -> Self {
        let author_prefix = format!("plugin:{}", plugin_name);
        Self {
            plugin_name,
            author_prefix,
            virtual_caps,
        }
    }
}

impl CapabilityProvider for PluginCapabilityProvider {
    fn derive_capabilities(&self, tool: &RegisteredTool) -> Vec<String> {
        if tool.author == self.author_prefix {
            self.virtual_caps.clone()
        } else {
            Vec::new()
        }
    }

    fn known_capability_names(&self) -> Vec<String> {
        self.virtual_caps.clone()
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
    // NEW P3e:
    pub capability_provider_handle: Option<ProviderHandle>,
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

/// Callback through which the `PluginManager` emits lifecycle events
/// (`ServerEvent::Plugin*` variants). The daemon wires this to its
/// `EventBroadcaster` so WebSocket clients (e.g. the GUI plugin panel)
/// see plugin state changes live. A plain callback keeps this crate free
/// of any dependency on the daemon's event infrastructure.
pub type PluginEventSink = Arc<dyn Fn(ServerEvent) + Send + Sync>;

/// Core orchestrator for plugin lifecycle: discovery, hot-load/unload,
/// permission gating, tool registration, and state tracking.
pub struct PluginManager {
    plugins: Arc<RwLock<HashMap<String, PluginState>>>,
    plugin_dir: PathBuf,
    permission_gate: PermissionGate,
    tool_registry: Arc<ToolRegistry>,
    skill_catalog: Option<Arc<SkillCatalog>>,
    agent_registry: Option<Arc<AgentRegistry>>,
    event_sink: Option<PluginEventSink>,
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
            event_sink: None,
        }
    }

    /// Attach a lifecycle event sink. Events emitted before this is called
    /// are dropped, so attach it before [`start`](Self::start).
    pub fn with_event_sink(mut self, sink: PluginEventSink) -> Self {
        self.event_sink = Some(sink);
        self
    }

    /// Emit a lifecycle event to the attached sink (no-op when absent).
    fn emit(&self, event: ServerEvent) {
        if let Some(ref sink) = self.event_sink {
            sink(event);
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

        // Insert initial Loading state.
        //
        // Refuse to replace an entry that still holds a live handle — a child
        // process or a capability provider. Overwriting it would orphan both:
        // the provider is only released through `unload_plugin`, and the child
        // would die incidentally, if at all. Every legitimate load path (boot
        // scan, approve, enable, config retry) reaches this point with nothing
        // held, so this is a guard, not a branch (design §2.2).
        {
            let mut plugins = self.plugins.write().await;
            if let Some(existing) = plugins.get(&name)
                && (existing.process.is_some() || existing.capability_provider_handle.is_some())
            {
                error!(
                    plugin = %name,
                    status = %existing.status,
                    "refusing to load over a plugin that still holds a process or capability provider"
                );
                return Err(PluginError::HandleHeld(name));
            }
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
                    capability_provider_handle: None,
                },
            );
        }

        // Step 2: Check approval
        match self.permission_gate.is_approved(&name) {
            None => {
                // Never seen — park in WaitingApproval
                info!(plugin = %name, "plugin awaiting approval");
                {
                    let mut plugins = self.plugins.write().await;
                    if let Some(state) = plugins.get_mut(&name) {
                        state.status = PluginStatus::WaitingApproval;
                    }
                }
                self.emit(ServerEvent::PluginPendingApproval {
                    plugin_id: name.clone(),
                    capabilities: manifest.capabilities.provides.clone(),
                });
                return Ok(());
            }
            Some(false) => {
                // Explicitly denied — a consent decision, so it reads `denied`.
                debug!(plugin = %name, "plugin is denied, not loading");
                let mut plugins = self.plugins.write().await;
                if let Some(state) = plugins.get_mut(&name) {
                    state.status = PluginStatus::Denied;
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
            {
                let mut plugins = self.plugins.write().await;
                if let Some(state) = plugins.get_mut(&name) {
                    state.status = PluginStatus::NeedsConfig {
                        missing_keys: missing.clone(),
                    };
                }
            }
            self.emit(ServerEvent::PluginNeedsConfig {
                plugin_id: name.clone(),
                missing_keys: missing,
            });
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

        // P3e: if manifest declares virtual capabilities, register a PluginCapabilityProvider.
        let provider_handle = if !manifest.capabilities.virtual_.provides.is_empty() {
            let provider = PluginCapabilityProvider::new(
                manifest.plugin.name.clone(),
                manifest.capabilities.virtual_.provides.clone(),
            );
            let handle = self.tool_registry.register_capability_provider(Arc::new(provider));
            tracing::info!(
                plugin = %name,
                handle = %handle,
                cap_count = manifest.capabilities.virtual_.provides.len(),
                "registered plugin capability provider"
            );
            Some(handle)
        } else {
            None
        };

        // Step 8: Track state as Running
        {
            let mut plugins = self.plugins.write().await;
            if let Some(state) = plugins.get_mut(&name) {
                state.status = PluginStatus::Running;
                state.process = Some(process);
                state.registered_tools = registered_tools.clone();
                state.registered_connector = registered_connector;
                state.registered_provider = registered_provider;
                state.registered_models = registered_models;
                state.registered_skills = registered_skills;
                state.registered_agents = registered_agents;
                state.last_health = Some(Instant::now());
                state.capability_provider_handle = provider_handle;
            }
        }

        self.emit(ServerEvent::PluginLoaded {
            plugin_id: name.clone(),
            tools: registered_tools,
        });

        info!(plugin = %name, "plugin loaded successfully");
        Ok(())
    }

    /// Unload a plugin: unregister tools, send shutdown RPC, kill process.
    pub async fn unload_plugin(&self, name: &str) -> Result<(), PluginError> {
        let mut plugins = self.plugins.write().await;
        let state = plugins.remove(name).ok_or_else(|| {
            PluginError::Unavailable(format!("plugin '{}' not found", name))
        })?;

        // P3e: remove capability provider FIRST — triggers index rebuild before
        // per-tool removals start. Ensures plugin virtual caps are scrubbed cleanly.
        if let Some(handle) = state.capability_provider_handle {
            let removed = self.tool_registry.remove_capability_provider(handle);
            if removed {
                tracing::debug!(
                    plugin = name, handle = %handle,
                    "removed plugin capability provider"
                );
            } else {
                tracing::warn!(
                    plugin = name, handle = %handle,
                    "capability provider handle not found during unload"
                );
            }
        }

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

        // Graceful shutdown + kill, then wait for the child to actually go.
        // `kill()` is `Child::start_kill`, which only *initiates* termination:
        // without the wait, "the plugin's process is gone" would be a race the
        // caller cannot win.
        if let Some(mut process) = state.process {
            if let Err(e) = process.shutdown().await {
                warn!(plugin = name, error = %e, "shutdown RPC failed, killing");
            }
            process.kill();
            match tokio::time::timeout(CHILD_EXIT_TIMEOUT, process.child.wait()).await {
                Ok(Ok(status)) => debug!(plugin = name, ?status, "plugin child exited"),
                Ok(Err(e)) => {
                    warn!(plugin = name, error = %e, "failed to wait for plugin child")
                }
                Err(_) => warn!(
                    plugin = name,
                    "child did not exit after kill within {}s",
                    CHILD_EXIT_TIMEOUT.as_secs()
                ),
            }
        }

        self.emit(ServerEvent::PluginUnloaded {
            plugin_id: name.to_string(),
        });

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

    /// Deny a plugin: record the refusal, then tear the plugin down.
    ///
    /// **Write-first.** The consent decision reaches `.permissions.toml`
    /// before anything is unloaded, so a crash between the two cannot lose it
    /// — the worst case is a denied plugin that is still loaded until the next
    /// boot, never a plugin the owner refused that boots as approved. A failed
    /// write returns the error and changes nothing: the child keeps running
    /// and its tools, skills and agent templates stay registered.
    ///
    /// The plugin is then parked as [`PluginStatus::Denied`] — a consent word,
    /// not the toggle position `disabled` — so it still appears in listings.
    pub async fn deny_plugin(&self, name: &str) -> Result<(), PluginError> {
        // W-deny: `approved = false`, `capabilities = []`.
        self.permission_gate.deny(name)?;

        // Teardown: whatever the map holds for this plugin goes, exactly as a
        // disable would tear it down.
        if !self.unload_and_park(name, PluginStatus::Denied).await? {
            debug!(plugin = name, "denied a plugin that is not tracked");
        }

        self.emit(ServerEvent::PluginDisabled {
            plugin_id: name.to_string(),
            reason: "denied by user".to_string(),
        });

        info!(plugin = name, "plugin denied");
        Ok(())
    }

    /// Re-enable a disabled plugin and trigger loading.
    ///
    /// Enabling a plugin that is already running is a no-op success, never a
    /// reload (design §3.3 E0): a reload would insert a fresh `PluginState`
    /// over the live one and orphan the capability provider it holds, which
    /// only [`unload_plugin`](Self::unload_plugin) can release.
    pub async fn enable_plugin(&self, name: &str) -> Result<(), PluginError> {
        let (plugin_dir, capabilities) = {
            let plugins = self.plugins.read().await;
            let state = plugins.get(name).ok_or_else(|| {
                PluginError::Unavailable(format!("plugin '{}' not found", name))
            })?;
            if matches!(state.status, PluginStatus::Running) {
                info!(plugin = name, "plugin is already running, enable is a no-op");
                return Ok(());
            }
            (
                state.plugin_dir.clone(),
                state.manifest.capabilities.provides.clone(),
            )
        };

        // Record approval
        self.permission_gate.approve(name, &capabilities)?;

        info!(plugin = name, "plugin re-enabled, loading");
        self.try_load_plugin(&plugin_dir).await
    }

    /// Disable a plugin: unload it and mark as disabled.
    pub async fn disable_plugin(&self, name: &str) -> Result<(), PluginError> {
        if !self.unload_and_park(name, PluginStatus::Disabled).await? {
            return Err(PluginError::Unavailable(format!(
                "plugin '{}' not found",
                name
            )));
        }

        self.permission_gate.deny(name)?;

        self.emit(ServerEvent::PluginDisabled {
            plugin_id: name.to_string(),
            reason: "disabled by user".to_string(),
        });

        info!(plugin = name, "plugin disabled");
        Ok(())
    }

    /// Unload a plugin and re-park its entry with `status`.
    ///
    /// The teardown is [`unload_plugin`](Self::unload_plugin) — it unregisters
    /// the tools, skills and agent templates, releases the capability provider
    /// and kills the child — followed by a re-insert carrying no handles, so
    /// the plugin still appears in listings under its new status.
    ///
    /// Returns `false` (and does nothing) when the plugin is not tracked;
    /// callers decide whether that is an error.
    async fn unload_and_park(
        &self,
        name: &str,
        status: PluginStatus,
    ) -> Result<bool, PluginError> {
        // Capture what the re-insert needs before the unload removes the entry.
        let Some((manifest, plugin_dir)) = ({
            let plugins = self.plugins.read().await;
            plugins
                .get(name)
                .map(|state| (state.manifest.clone(), state.plugin_dir.clone()))
        }) else {
            return Ok(false);
        };

        self.unload_plugin(name).await?;

        let mut plugins = self.plugins.write().await;
        plugins.insert(
            name.to_string(),
            PluginState {
                manifest,
                status,
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
                capability_provider_handle: None,
            },
        );
        Ok(true)
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
                version: manifest.plugin.version.clone(),
                author: format!("plugin:{}", manifest.plugin.name),
                created_at: chrono::Utc::now(),
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
        assert_eq!(PluginStatus::Denied.to_string(), "denied");
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

#[cfg(test)]
mod event_sink_tests {
    use super::*;
    use openalpaca_core::tools::ToolRegistry;
    use std::sync::Mutex as StdMutex;

    /// Stub sink that records every emitted event.
    fn recording_sink() -> (PluginEventSink, Arc<StdMutex<Vec<ServerEvent>>>) {
        let events: Arc<StdMutex<Vec<ServerEvent>>> = Arc::new(StdMutex::new(Vec::new()));
        let events_clone = Arc::clone(&events);
        let sink: PluginEventSink = Arc::new(move |event| {
            events_clone
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(event);
        });
        (sink, events)
    }

    fn write_manifest(plugin_dir: &Path, name: &str, extra: &str) {
        std::fs::create_dir_all(plugin_dir).unwrap();
        std::fs::write(
            plugin_dir.join("plugin.toml"),
            format!(
                r#"
[plugin]
name = "{name}"
version = "0.1.0"
entry = "./nonexistent-entry"
{extra}
"#
            ),
        )
        .unwrap();
    }

    fn manager_with_sink(root: &Path) -> (PluginManager, Arc<StdMutex<Vec<ServerEvent>>>) {
        let (sink, events) = recording_sink();
        let manager = PluginManager::new(
            root.to_path_buf(),
            Arc::new(ToolRegistry::new().unwrap()),
            None,
            None,
        )
        .with_event_sink(sink);
        (manager, events)
    }

    #[tokio::test]
    async fn emits_pending_approval_on_first_load_and_unloaded_on_unload() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin_dir = tmp.path().join("my-plugin");
        write_manifest(&plugin_dir, "my-plugin", "");

        let (manager, events) = manager_with_sink(tmp.path());

        // First-time load parks in WaitingApproval and emits PluginPendingApproval.
        manager.try_load_plugin(&plugin_dir).await.unwrap();
        {
            let recorded = events.lock().unwrap();
            assert_eq!(recorded.len(), 1);
            match &recorded[0] {
                ServerEvent::PluginPendingApproval { plugin_id, .. } => {
                    assert_eq!(plugin_id, "my-plugin");
                }
                other => panic!("expected PluginPendingApproval, got {other:?}"),
            }
        }

        // Unload emits PluginUnloaded.
        manager.unload_plugin("my-plugin").await.unwrap();
        let recorded = events.lock().unwrap();
        assert_eq!(recorded.len(), 2);
        match &recorded[1] {
            ServerEvent::PluginUnloaded { plugin_id } => {
                assert_eq!(plugin_id, "my-plugin");
            }
            other => panic!("expected PluginUnloaded, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn emits_needs_config_when_required_keys_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin_dir = tmp.path().join("cfg-plugin");
        write_manifest(
            &plugin_dir,
            "cfg-plugin",
            r#"
[config.api_key]
type = "secret"
required = true
"#,
        );

        let (manager, events) = manager_with_sink(tmp.path());

        // Pre-approve so the load proceeds to config validation.
        manager
            .permission_gate
            .approve("cfg-plugin", &[])
            .unwrap();

        manager.try_load_plugin(&plugin_dir).await.unwrap();

        let recorded = events.lock().unwrap();
        assert_eq!(recorded.len(), 1);
        match &recorded[0] {
            ServerEvent::PluginNeedsConfig {
                plugin_id,
                missing_keys,
            } => {
                assert_eq!(plugin_id, "cfg-plugin");
                assert_eq!(missing_keys, &vec!["api_key".to_string()]);
            }
            other => panic!("expected PluginNeedsConfig, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn emits_disabled_on_deny() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin_dir = tmp.path().join("deny-plugin");
        write_manifest(&plugin_dir, "deny-plugin", "");

        let (manager, events) = manager_with_sink(tmp.path());
        manager.try_load_plugin(&plugin_dir).await.unwrap();
        manager.deny_plugin("deny-plugin").await.unwrap();

        let recorded = events.lock().unwrap();
        let disabled = recorded.iter().find_map(|e| match e {
            ServerEvent::PluginDisabled { plugin_id, reason } => {
                Some((plugin_id.clone(), reason.clone()))
            }
            _ => None,
        });
        assert_eq!(
            disabled,
            Some(("deny-plugin".to_string(), "denied by user".to_string()))
        );
    }

    #[tokio::test]
    async fn no_sink_is_a_noop() {
        let tmp = tempfile::tempdir().unwrap();
        let plugin_dir = tmp.path().join("silent-plugin");
        write_manifest(&plugin_dir, "silent-plugin", "");

        let manager = PluginManager::new(
            tmp.path().to_path_buf(),
            Arc::new(ToolRegistry::new().unwrap()),
            None,
            None,
        );
        // Must not panic without a sink attached.
        manager.try_load_plugin(&plugin_dir).await.unwrap();
    }
}

/// Lifecycle tests that drive a real child process.
///
/// Everything above tests the manager against plugins that never spawn
/// (`entry = "./nonexistent-entry"`). Deny and enable are teardown paths, so
/// they need a plugin that actually holds a process, a tool, a skill, an agent
/// template and a capability provider — the committed stub plugin at
/// `tests/fixtures/echo-plugin/` provides that.
#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use openalpaca_core::agent::registry::AgentRegistry;
    use openalpaca_core::orchestrator::skill_catalog::SkillCatalog;
    use openalpaca_core::tools::ToolRegistry;

    /// The committed stub: Content-Length-framed JSON-RPC over stdio, answers
    /// `tools/list` with one `echo` tool and every other method with an empty
    /// result — enough for `skill/info` and `agent/info` to register a skill
    /// and an agent template under the plugin's own name.
    fn stub_script() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../tests/fixtures/echo-plugin/echo-server.sh")
    }

    /// Lay out a plugin directory under `root` holding the stub script and a
    /// manifest with `extra` appended (types, virtual capabilities, …).
    fn install_stub_plugin(root: &Path, name: &str, extra: &str) -> PathBuf {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        // fs::copy carries the mode across on Unix, so the entry stays executable.
        std::fs::copy(stub_script(), dir.join("echo-server.sh")).unwrap();
        std::fs::write(
            dir.join("plugin.toml"),
            format!(
                r#"
[plugin]
name = "{name}"
version = "0.1.0"
entry = "./echo-server.sh"
mcp_compatible = true
{extra}
"#
            ),
        )
        .unwrap();
        dir
    }

    struct Harness {
        manager: PluginManager,
        tools: Arc<ToolRegistry>,
        skills: Arc<SkillCatalog>,
        agents: Arc<AgentRegistry>,
    }

    impl Harness {
        fn new(root: &Path) -> Self {
            let tools = Arc::new(ToolRegistry::new().unwrap());
            let skills = Arc::new(SkillCatalog::new());
            let agents = Arc::new(AgentRegistry::new());
            let manager = PluginManager::new(
                root.to_path_buf(),
                Arc::clone(&tools),
                Some(Arc::clone(&skills)),
                Some(Arc::clone(&agents)),
            );
            Self {
                manager,
                tools,
                skills,
                agents,
            }
        }

        async fn info(&self, name: &str) -> PluginInfo {
            self.manager
                .list_plugins()
                .await
                .into_iter()
                .find(|p| p.name == name)
                .unwrap_or_else(|| panic!("plugin '{name}' is not tracked"))
        }

        /// PID of the plugin's child process. Panics unless one is held.
        async fn child_pid(&self, name: &str) -> u32 {
            let plugins = self.manager.plugins.read().await;
            plugins
                .get(name)
                .and_then(|s| s.process.as_ref())
                .and_then(|p| p.child.id())
                .unwrap_or_else(|| panic!("plugin '{name}' holds no child process"))
        }

        /// The capability-provider handle the manager tracks for this plugin.
        async fn provider_handle(&self, name: &str) -> ProviderHandle {
            let plugins = self.manager.plugins.read().await;
            plugins
                .get(name)
                .and_then(|s| s.capability_provider_handle)
                .unwrap_or_else(|| panic!("plugin '{name}' registered no capability provider"))
        }

        /// How many providers currently emit the stub's virtual capability.
        /// `known_virtual_capabilities` does not de-duplicate, so a duplicate
        /// provider shows up as a second occurrence.
        fn stub_caps(&self) -> usize {
            self.tools
                .known_virtual_capabilities()
                .iter()
                .filter(|c| *c == "annotation:echo_stub")
                .count()
        }

        /// `try_wait` on the held child: `None` while it is still running.
        async fn child_exited(&self, name: &str) -> Option<std::process::ExitStatus> {
            let mut plugins = self.manager.plugins.write().await;
            plugins
                .get_mut(name)
                .and_then(|s| s.process.as_mut())
                .unwrap_or_else(|| panic!("plugin '{name}' holds no child process"))
                .try_wait()
        }
    }

    /// Is `pid` still a live process? `sh -c "kill -0"` uses the shell builtin,
    /// so this needs no `/bin/kill` and no extra dependency.
    fn pid_alive(pid: u32) -> bool {
        std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("kill -0 {pid} 2>/dev/null"))
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    /// Load the stub as an approved plugin and assert it came up running, with
    /// its tool registered and its child alive. Returns the loaded info so a
    /// caller can check the contributions its manifest asked for.
    async fn load_running_stub(harness: &Harness, dir: &Path, name: &str) -> PluginInfo {
        harness.manager.permission_gate.approve(name, &[]).unwrap();
        harness.manager.try_load_plugin(dir).await.unwrap();

        let info = harness.info(name).await;
        assert_eq!(info.status, "running", "stub plugin failed to start");
        assert_eq!(info.tools, vec![format!("{name}::echo")]);
        assert!(
            harness.child_exited(name).await.is_none(),
            "stub child exited during load"
        );
        info
    }

    /// A1 (bug B): deny must tear the plugin down, not just relabel it.
    #[tokio::test]
    async fn deny_unloads_the_plugin_and_kills_its_child() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = install_stub_plugin(
            tmp.path(),
            "echo-test",
            "[types]\ntools = true\nskill = true\nagent = true\n",
        );
        let h = Harness::new(tmp.path());
        let loaded = load_running_stub(&h, &dir, "echo-test").await;
        assert_eq!(loaded.skills, vec!["echo-test".to_string()]);
        assert_eq!(loaded.agents, vec!["echo-test".to_string()]);
        let pid = h.child_pid("echo-test").await;

        h.manager.deny_plugin("echo-test").await.unwrap();

        // The consent decision is persisted and reported as a consent word.
        assert_eq!(
            h.manager.permission_gate.is_approved("echo-test"),
            Some(false)
        );
        // Nothing of the plugin is left on any surface.
        let info = h.info("echo-test").await;
        assert!(info.tools.is_empty(), "tools survived deny: {:?}", info.tools);
        assert!(info.skills.is_empty(), "skills survived deny");
        assert!(info.agents.is_empty(), "agents survived deny");
        assert!(
            !h.tools
                .registered_tool_names()
                .iter()
                .any(|n| n == "echo-test::echo"),
            "the tool is still in the registry"
        );
        assert!(h.skills.get("echo-test").is_none(), "skill still catalogued");
        assert!(
            h.agents.get_template("echo-test").is_none(),
            "agent template still registered"
        );

        // And the child is gone, not merely forgotten.
        assert!(!pid_alive(pid), "plugin child {pid} outlived the denial");

        // The reported word is the consent decision, never "disabled".
        assert_eq!(info.status, "denied");
    }

    /// A1 write-first (design §3.2 W-deny): if the denial cannot be persisted,
    /// nothing is torn down — a half-applied deny would leave a plugin running
    /// that the next boot considers approved.
    #[tokio::test]
    async fn deny_that_cannot_be_persisted_changes_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = install_stub_plugin(tmp.path(), "echo-test", "[types]\ntools = true\n");
        let h = Harness::new(tmp.path());
        load_running_stub(&h, &dir, "echo-test").await;
        let pid = h.child_pid("echo-test").await;

        // Make the permissions write fail: a directory cannot be overwritten
        // by a file.
        let permissions = tmp.path().join(".permissions.toml");
        std::fs::remove_file(&permissions).unwrap();
        std::fs::create_dir(&permissions).unwrap();

        let err = h.manager.deny_plugin("echo-test").await.unwrap_err();
        assert!(
            matches!(err, PluginError::Io(_)),
            "expected the failed write to surface, got {err:?}"
        );

        let info = h.info("echo-test").await;
        assert_eq!(info.status, "running", "a failed write tore the plugin down");
        assert_eq!(info.tools, vec!["echo-test::echo".to_string()]);
        assert!(h.child_exited("echo-test").await.is_none());
        assert!(pid_alive(pid), "plugin child {pid} was killed anyway");
    }

    /// A2 (bug C): enabling a plugin that is already running must not reload
    /// it. A reload overwrites the map entry with a fresh `PluginState` whose
    /// `capability_provider_handle` is `None`, orphaning the provider that
    /// only `unload_plugin` can release — one permanent leak per redundant
    /// enable.
    #[tokio::test]
    async fn redundant_enable_registers_no_second_capability_provider() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = install_stub_plugin(
            tmp.path(),
            "echo-test",
            "[types]\ntools = true\n\n[capabilities.virtual]\nprovides = [\"annotation:echo_stub\"]\n",
        );
        let h = Harness::new(tmp.path());
        load_running_stub(&h, &dir, "echo-test").await;

        let pid = h.child_pid("echo-test").await;
        let handle = h.provider_handle("echo-test").await;
        let providers = h.tools.provider_handles().len();
        assert_eq!(h.stub_caps(), 1, "the stub's virtual cap is registered once");

        h.manager.enable_plugin("echo-test").await.unwrap();

        assert_eq!(
            h.tools.provider_handles().len(),
            providers,
            "redundant enable registered a second capability provider"
        );
        assert_eq!(h.stub_caps(), 1, "the stub's virtual cap is now duplicated");
        assert_eq!(
            h.provider_handle("echo-test").await,
            handle,
            "the tracked provider handle was replaced"
        );
        assert_eq!(h.child_pid("echo-test").await, pid, "the child was restarted");
        assert_eq!(h.info("echo-test").await.status, "running");

        // The decisive check: a leaked provider would survive the unload,
        // because nothing holds its handle any more.
        h.manager.unload_plugin("echo-test").await.unwrap();
        assert_eq!(h.stub_caps(), 0, "a capability provider outlived the plugin");
    }

    /// A2, second half (design §2.2): the map insert refuses to replace an
    /// entry that still holds a handle, so no load path — a redundant enable,
    /// an approve, a second directory using the same manifest name — can
    /// orphan a live process or provider by overwriting its state.
    #[tokio::test]
    async fn loading_over_a_plugin_that_holds_a_handle_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = install_stub_plugin(tmp.path(), "echo-test", "[types]\ntools = true\n");
        let h = Harness::new(tmp.path());
        load_running_stub(&h, &dir, "echo-test").await;
        let pid = h.child_pid("echo-test").await;

        let err = h.manager.try_load_plugin(&dir).await.unwrap_err();
        assert!(
            matches!(&err, PluginError::HandleHeld(name) if name == "echo-test"),
            "expected HandleHeld, got {err:?}"
        );

        // The running plugin is untouched.
        let info = h.info("echo-test").await;
        assert_eq!(info.status, "running");
        assert_eq!(info.tools, vec!["echo-test::echo".to_string()]);
        assert_eq!(h.child_pid("echo-test").await, pid);
    }
}

#[cfg(test)]
mod p3e_provider_tests {
    use super::*;
    use openalpaca_core::tools::registry::{RegisteredTool, ToolBackend};
    use openalpaca_llm::ToolDefinition;

    fn mock_tool(name: &str, author: &str) -> RegisteredTool {
        RegisteredTool {
            definition: ToolDefinition {
                name: name.to_string(),
                description: "test".to_string(),
                parameters: serde_json::json!({"type": "object"}),
                strict: None,
                input_examples: None,
            },
            backend: ToolBackend::Http {
                method: "GET".into(),
                url: "http://example.com".into(),
                headers: Default::default(),
                timeout_secs: 10,
            },
            provides_capabilities: vec![],
            exempt_from_timeout: false,
            annotations: None,
            version: "0.0.0".into(),
            author: author.to_string(),
            created_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn plugin_provider_emits_caps_for_matching_author() {
        let p = PluginCapabilityProvider::new(
            "foo".to_string(),
            vec!["annotation:test".to_string()],
        );
        let tool_match = mock_tool("x", "plugin:foo");
        let tool_nomatch = mock_tool("y", "plugin:bar");
        let tool_builtin = mock_tool("z", "builtin");

        assert_eq!(
            p.derive_capabilities(&tool_match),
            vec!["annotation:test".to_string()]
        );
        assert!(p.derive_capabilities(&tool_nomatch).is_empty());
        assert!(p.derive_capabilities(&tool_builtin).is_empty());
    }

    #[test]
    fn plugin_provider_known_names_returns_declared_list() {
        let p = PluginCapabilityProvider::new(
            "foo".to_string(),
            vec!["annotation:a".to_string(), "annotation:b".to_string()],
        );
        let names = p.known_capability_names();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"annotation:a".to_string()));
        assert!(names.contains(&"annotation:b".to_string()));
    }

    #[test]
    fn plugin_provider_with_empty_caps_is_noop() {
        let p = PluginCapabilityProvider::new("foo".to_string(), vec![]);
        let tool = mock_tool("x", "plugin:foo");
        assert!(p.derive_capabilities(&tool).is_empty());
        assert!(p.known_capability_names().is_empty());
    }

    #[test]
    fn plugin_provider_handles_non_annotation_caps() {
        let p = PluginCapabilityProvider::new(
            "foo".to_string(),
            vec!["plugin:mytag".to_string(), "annotation:safe".to_string()],
        );
        let tool = mock_tool("x", "plugin:foo");
        let caps = p.derive_capabilities(&tool);
        assert_eq!(caps.len(), 2);
        assert!(caps.contains(&"plugin:mytag".to_string()));
        assert!(caps.contains(&"annotation:safe".to_string()));
    }
}

#[cfg(test)]
mod p3e_integration_tests {
    use super::*;
    use openalpaca_core::tools::registry::{RegisteredTool, ToolRegistry, ToolBackend};
    use openalpaca_llm::ToolDefinition;
    use std::sync::Arc;

    fn mock_plugin_tool(name: &str, plugin_name: &str) -> RegisteredTool {
        RegisteredTool {
            definition: ToolDefinition {
                name: name.to_string(),
                description: "test".into(),
                parameters: serde_json::json!({"type": "object"}),
                strict: None,
                input_examples: None,
            },
            backend: ToolBackend::Http {
                method: "GET".into(),
                url: "http://example.com".into(),
                headers: Default::default(),
                timeout_secs: 10,
            },
            provides_capabilities: vec![],
            exempt_from_timeout: false,
            annotations: None,
            version: "0.0.0".into(),
            author: format!("plugin:{}", plugin_name),
            created_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn plugin_provider_integrates_with_tool_registry() {
        let registry = ToolRegistry::new().unwrap();
        registry.register(mock_plugin_tool("foo_read", "myplugin")).unwrap();

        let provider = PluginCapabilityProvider::new(
            "myplugin".to_string(),
            vec!["annotation:test_tag".to_string()],
        );
        let _handle = registry.register_capability_provider(Arc::new(provider));

        let known = registry.known_virtual_capabilities();
        assert!(known.iter().any(|k| k == "annotation:test_tag"));

        let tools = registry.tools_for_capabilities(&vec!["annotation:test_tag".to_string()]);
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "foo_read");
    }

    #[test]
    fn plugin_provider_removal_scrubs_virtual_caps() {
        let registry = ToolRegistry::new().unwrap();
        registry.register(mock_plugin_tool("foo_read", "myplugin")).unwrap();

        let provider = PluginCapabilityProvider::new(
            "myplugin".to_string(),
            vec!["annotation:test_tag".to_string()],
        );
        let handle = registry.register_capability_provider(Arc::new(provider));

        let before = registry.tools_for_capabilities(&vec!["annotation:test_tag".to_string()]);
        assert_eq!(before.len(), 1);

        registry.remove_capability_provider(handle);

        let after = registry.tools_for_capabilities(&vec!["annotation:test_tag".to_string()]);
        assert!(after.is_empty());

        // Tool itself still registered
        assert!(registry.registered_tool_names().iter().any(|n| n == "foo_read"));

        // Known virtual caps no longer includes the plugin's tag
        let known = registry.known_virtual_capabilities();
        assert!(!known.iter().any(|k| k == "annotation:test_tag"));
    }

    #[test]
    fn plugin_provider_reload_issues_fresh_handle() {
        let registry = ToolRegistry::new().unwrap();
        registry.register(mock_plugin_tool("foo_read", "myplugin")).unwrap();

        let provider1 = PluginCapabilityProvider::new(
            "myplugin".to_string(),
            vec!["annotation:test_tag".to_string()],
        );
        let h1 = registry.register_capability_provider(Arc::new(provider1));
        registry.remove_capability_provider(h1);

        let provider2 = PluginCapabilityProvider::new(
            "myplugin".to_string(),
            vec!["annotation:test_tag".to_string()],
        );
        let h2 = registry.register_capability_provider(Arc::new(provider2));
        assert_ne!(h1, h2);

        let tools = registry.tools_for_capabilities(&vec!["annotation:test_tag".to_string()]);
        assert_eq!(tools.len(), 1);
    }
}
