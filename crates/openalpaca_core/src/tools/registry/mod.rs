use async_trait::async_trait;
use dashmap::DashMap;
use openalpaca_llm::ToolDefinition;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::orchestrator::skill::constraints::EffectiveToolSet;
use crate::tools::extensions::{ExtensionId, ExtensionLedger};

pub mod capabilities;
pub use capabilities::{
    annotation_capabilities, validate_annotation_capability, AnnotationCapabilityProvider,
    CapabilityProvider, ProviderHandle, ANNOTATION_CAPABILITY_NAMES,
};

/// Per-invocation execution context passed to tools that need identity.
/// Lightweight — no Arc deps, no DB handles. Just identity values.
#[derive(Debug, Clone, Default)]
pub struct ToolContext {
    pub agent_id: Option<String>,
    pub task_id: Option<String>,
    pub owner_id: Option<String>,
    pub workspace_id: Option<String>,
    /// Skill invocation chain, oldest first. Empty at top level.
    pub skill_stack: Vec<String>,
    /// Effective tool constraints inherited from parent skill chain.
    /// None at top level; Some when inside a nested skill invocation.
    pub effective_constraints: Option<EffectiveToolSet>,
    /// Conversation lane the invocation belongs to (canonical "user_id:source"
    /// form). Present on conversational paths; None inside detached task loops
    /// that have no lane threading (Routing V2).
    pub lane_key: Option<String>,
    /// Message source channel (e.g. "cli", "telegram"). Present where the
    /// caller has one.
    pub source: Option<String>,
    /// The originating request id, when the invocation belongs to a user turn.
    pub request_id: Option<uuid::Uuid>,
    /// The requesting principal, for tools that re-enter the front door or
    /// need identity beyond `owner_id`. None where not threaded.
    pub principal: Option<crate::security::policy::Principal>,
    /// The resource scope of the originating request. None where not threaded.
    pub scope: Option<crate::security::policy::Scope>,
    /// Filesystem workspace path of the originating request (from GUI/CLI),
    /// when one was provided. Persisted with steering/follow-up items so
    /// re-entry turns can restore the workspace scope (Routing V2).
    pub workspace_path: Option<String>,
}

impl ToolContext {
    /// Clone this context and append a skill ID to the call stack.
    pub fn with_skill_pushed(&self, skill_id: impl Into<String>) -> Self {
        let mut next = self.clone();
        next.skill_stack.push(skill_id.into());
        next
    }
}

/// Coarse permission tier derived from MCP tool annotations.
/// Used for introspection and policy decisions; not stored on RegisteredTool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionTier {
    ReadOnly,
    ReadWrite,
    Admin,
}

/// Derive the permission tier from a tool's annotations.
///
/// - `destructive_hint = Some(true)` → Admin
/// - `read_only_hint = Some(true)` → ReadOnly
/// - otherwise (including None) → ReadWrite
pub fn permission_tier(annotations: Option<&openalpaca_mcp::ToolAnnotations>) -> PermissionTier {
    match annotations {
        Some(a) if a.destructive_hint == Some(true) => PermissionTier::Admin,
        Some(a) if a.read_only_hint == Some(true) => PermissionTier::ReadOnly,
        _ => PermissionTier::ReadWrite,
    }
}

/// Backend that executes a tool's logic.
#[derive(Clone)]
pub enum ToolBackend {
    BuiltIn(Arc<dyn BuiltInTool>),
    Http {
        method: String,
        url: String,
        headers: HashMap<String, String>,
        timeout_secs: u64,
    },
    Command {
        command: String,
        args_template: Option<String>,
        timeout_secs: u64,
    },
    Plugin(Arc<dyn openalpaca_api::plugin_traits::PluginToolExecutor>),
    Mcp {
        client: Arc<openalpaca_mcp::McpClient>,
        remote_name: String,
        server_name: String,
        /// Which load of the server this handle belongs to (design §3.0 Fact 3).
        /// Stamped once, at the single construction site
        /// (`tools/mcp/bridge.rs`), from the number E0 handed the supervisor.
        /// The gate compares it against the ledger record's current generation,
        /// so a snapshot holding a sealed client from a previous load is
        /// refused as `Stale` rather than talking to a dead transport.
        generation: u64,
    },
}

/// Trait for built-in tool implementations.
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

/// A tool registered in the registry: its LLM-facing definition + execution backend.
#[derive(Clone)]
pub struct RegisteredTool {
    pub definition: ToolDefinition,
    /// The execution backend.
    ///
    /// **Reading this outside `ToolRegistry` bypasses the extension gate**
    /// (design §6.3): every path to an MCP client or a plugin executor must go
    /// through `execute`/`execute_with_context`, which check the ledger before
    /// they dispatch. The field cannot be made private — plugin and MCP
    /// registration build the struct by literal — so this is enforced by test
    /// and comment, not by the type system.
    pub backend: ToolBackend,
    pub provides_capabilities: Vec<String>,
    /// When true, SandboxManager skips the per-tool timeout for this tool.
    /// Used for coordination tools that manage their own timeouts.
    pub exempt_from_timeout: bool,
    /// MCP tool annotations (destructiveHint, idempotentHint, etc.).
    /// Populated for tools registered from MCP servers; `None` otherwise in P2.
    /// P3 may populate for built-ins as part of destructive-tool enforcement.
    pub annotations: Option<openalpaca_mcp::ToolAnnotations>,
    /// Tool version string (e.g., "1.0.0"). Sourced from MCP `_meta.version`
    /// when available; defaults to a registry-provided value otherwise.
    pub version: String,
    /// Author identifier (e.g., "built-in", "mcp:<server>", "plugin:<id>").
    /// Used for audit trails and provenance display.
    pub author: String,
    /// Registration timestamp (UTC) — when this tool entered the registry.
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl RegisteredTool {
    /// Which extension owns this tool, derived — never stored on the struct,
    /// which has 79 construction sites and no `Default` (design §3.1).
    ///
    /// `None` for `BuiltIn` / `Http` / `Command` — those are never on the
    /// ENABLE axis. Both identity producers are single-site and already exist:
    /// `bridge.rs` (`"mcp:{server}"`) and `manager.rs` (`"plugin:{name}"`).
    pub fn extension_id(&self) -> Option<ExtensionId> {
        match &self.backend {
            ToolBackend::Mcp { server_name, .. } => Some(ExtensionId::mcp(server_name)),
            ToolBackend::Plugin(_) => self.author.strip_prefix("plugin:").map(ExtensionId::plugin),
            _ => None,
        }
    }

    /// Which load of the extension this handle belongs to (design §3.0 Fact 3).
    /// `None` for non-extension tools.
    pub fn incarnation(&self) -> Option<u64> {
        match &self.backend {
            ToolBackend::Mcp { generation, .. } => Some(*generation),
            ToolBackend::Plugin(executor) => Some(executor.generation()),
            _ => None,
        }
    }
}

/// Central registry mapping tool names to definitions and execution backends.
///
/// Backed by `DashMap` for lock-free concurrent reads and writes.
/// Shared as `Arc<ToolRegistry>` — tools can be registered and removed at runtime
/// (e.g. when plugins load/unload) without requiring `&mut self`.
pub struct ToolRegistry {
    tools: DashMap<String, RegisteredTool>,
    capability_index: DashMap<String, Vec<String>>, // capability → tool names
    /// The ENABLE axis's bookkeeping. Shared by `Arc::clone` in `Clone`, which
    /// is what makes a deep snapshot read *live* extension state at the gate
    /// (design §3.0).
    extensions: Arc<ExtensionLedger>,
    http_client: reqwest::Client,
    capability_providers: DashMap<ProviderHandle, Arc<dyn capabilities::CapabilityProvider>>,
    next_provider_handle: AtomicU64,
    provider_mutex: std::sync::Mutex<()>,
}

impl Clone for ToolRegistry {
    /// NOTE: Not atomic. Concurrent register/remove during clone may produce
    /// an incomplete snapshot. Use Arc::clone() for shared access instead.
    fn clone(&self) -> Self {
        let tools = DashMap::new();
        for entry in self.tools.iter() {
            tools.insert(entry.key().clone(), entry.value().clone());
        }
        let capability_index = DashMap::new();
        for entry in self.capability_index.iter() {
            capability_index.insert(entry.key().clone(), entry.value().clone());
        }
        let capability_providers = DashMap::new();
        for entry in self.capability_providers.iter() {
            capability_providers.insert(*entry.key(), entry.value().clone());
        }
        Self {
            tools,
            capability_index,
            // ONE Arc::clone covers all four snapshot sites and every future
            // one, by construction — this is the whole design (§3.0 Fact 1).
            extensions: Arc::clone(&self.extensions),
            http_client: self.http_client.clone(),
            capability_providers,
            next_provider_handle: AtomicU64::new(
                self.next_provider_handle.load(Ordering::Relaxed),
            ),
            provider_mutex: std::sync::Mutex::new(()),
        }
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new().expect("HTTP client creation should not fail with default config")
    }
}

impl ToolRegistry {
    pub fn new() -> Result<Self, String> {
        Self::build(ExtensionLedger::new())
    }

    /// A registry whose ledger can announce on the event bus (design §7.1).
    ///
    /// `new()` and `Default` are unchanged and stay arg-free — of the 101
    /// construction sites exactly one is production, and it moves to this
    /// constructor in C4 together with the event variant it exists to publish.
    /// A ledger with no bus logs via `tracing` and returns.
    pub fn with_event_bus(bus: crate::bus::EventBus) -> Result<Self, String> {
        Self::build(ExtensionLedger::with_event_bus(bus))
    }

    /// The ENABLE axis's bookkeeping. Supervisors reach it from the
    /// `Arc<ToolRegistry>` they already hold.
    pub fn extensions(&self) -> &Arc<ExtensionLedger> {
        &self.extensions
    }

    fn build(ledger: ExtensionLedger) -> Result<Self, String> {
        let redirect_policy = reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 10 {
                attempt.error("too many redirects")
            } else if let Err(e) =
                crate::tools::url_validation::validate_url(attempt.url().as_str())
            {
                attempt.error(format!("redirect blocked by SSRF policy: {}", e))
            } else {
                attempt.follow()
            }
        });
        let http_client = reqwest::Client::builder()
            .redirect(redirect_policy)
            .build()
            .map_err(|e| format!("Failed to create HTTP client: {e}"))?;

        let registry = Self {
            tools: DashMap::new(),
            capability_index: DashMap::new(),
            extensions: Arc::new(ledger),
            http_client,
            capability_providers: DashMap::new(),
            next_provider_handle: AtomicU64::new(0),
            provider_mutex: std::sync::Mutex::new(()),
        };

        // Install the default AnnotationCapabilityProvider.
        // Handle is discarded — default provider is not externally tracked.
        let _ = registry.register_capability_provider(Arc::new(AnnotationCapabilityProvider));

        Ok(registry)
    }

    /// Register a tool. Safe to call at any time, including after wrapping in Arc.
    ///
    /// Returns `Err` if the tool name is empty, exceeds 256 characters, or
    /// contains null bytes.
    pub fn register(&self, tool: RegisteredTool) -> Result<(), String> {
        let name = &tool.definition.name;
        if name.is_empty() {
            return Err("Tool name cannot be empty".to_string());
        }
        if name.len() > 256 {
            return Err(format!(
                "Tool name '{}...' exceeds 256 char limit",
                &name[..32.min(name.len())]
            ));
        }
        if name.contains('\0') {
            return Err("Tool name cannot contain null bytes".to_string());
        }

        let mut tool = tool;
        if let Some(ext) = tool.extension_id() {
            // T3's bound on an in-flight call is an invariant this design
            // enforces, not a property of the sandbox: the timeout wrapper is
            // skipped entirely for exempt tools, so an extension tool could
            // outlive the drain forever (design §3.2 T3).
            if tool.exempt_from_timeout {
                tracing::warn!(
                    tool = %tool.definition.name,
                    extension = %ext,
                    "extension tools may never be exempt from the per-call timeout; forcing false"
                );
                tool.exempt_from_timeout = false;
            }
            // Fail-open is safe only while an unrecorded registration is
            // visible (design §6.2a). Expected on every extension registration
            // from the first boot after C1 until the supervisors land.
            if self.extensions.record(&ext).is_none() {
                tracing::warn!(
                    tool = %tool.definition.name,
                    extension = %ext,
                    "extension tool registered with no ledger record"
                );
            }
        }
        let name = &tool.definition.name;

        let _guard = self.provider_mutex.lock().unwrap_or_else(|p| p.into_inner());

        // String capabilities
        for cap in &tool.provides_capabilities {
            self.capability_index
                .entry(cap.clone())
                .or_default()
                .push(tool.definition.name.clone());
        }
        // Virtual capabilities from registered providers (DashMap iteration)
        for provider_entry in self.capability_providers.iter() {
            for cap in provider_entry.value().derive_capabilities(&tool) {
                self.capability_index
                    .entry(cap)
                    .or_default()
                    .push(tool.definition.name.clone());
            }
        }
        self.tools.insert(name.clone(), tool);
        Ok(())
    }

    /// Remove a tool by name. Returns true if the tool existed.
    pub fn remove(&self, name: &str) -> bool {
        if let Some((_, tool)) = self.tools.remove(name) {
            let _guard = self.provider_mutex.lock().unwrap_or_else(|p| p.into_inner());

            // Keys whose last provider just left, dropped after the retains so
            // no `get_mut` guard is alive when they are removed.
            let mut emptied: Vec<String> = Vec::new();

            for cap in &tool.provides_capabilities {
                if let Some(mut names) = self.capability_index.get_mut(cap) {
                    names.retain(|n| n != name);
                    if names.is_empty() {
                        emptied.push(cap.clone());
                    }
                }
            }
            // Scrub virtual capabilities from registered providers.
            for provider_entry in self.capability_providers.iter() {
                for cap in provider_entry.value().derive_capabilities(&tool) {
                    if let Some(mut names) = self.capability_index.get_mut(&cap) {
                        names.retain(|n| n != name);
                        if names.is_empty() {
                            emptied.push(cap.clone());
                        }
                    }
                }
            }
            // Drop the key rather than leaving it mapped to `[]` — otherwise a
            // disabled extension's capabilities survive as phantom keys in
            // anything built from the index (design §6.2 #4).
            for cap in emptied {
                self.capability_index.remove_if(&cap, |_, names| names.is_empty());
            }
            true
        } else {
            false
        }
    }

    /// Remove-then-register. **Mandatory** on every reload path, not defensive:
    /// `register` overwrites `tools` but *appends* to `capability_index` with no
    /// dedupe, while only `remove` scrubs — so an enable/disable/enable cycle
    /// that skipped the remove would leak duplicate index edges that reads
    /// survive (they dedupe via a `seen` set) and no test would ever fail on
    /// (design §3.3 E4).
    pub fn replace(&self, tool: RegisteredTool) -> Result<(), String> {
        self.remove(&tool.definition.name);
        self.register(tool)
    }

    /// Look up a tool by name. Returns a clone because DashMap guards
    /// must not be held across await points.
    pub fn get(&self, name: &str) -> Option<RegisteredTool> {
        self.tools.get(name).map(|r| r.value().clone())
    }

    /// Execute a tool by name.
    ///
    /// Performs basic JSON Schema pre-validation (required fields and type check)
    /// before dispatching to the backend, catching obvious argument errors early.
    pub async fn execute(
        &self,
        tool_name: &str,
        arguments: &serde_json::Value,
    ) -> Result<String, String> {
        self.dispatch(tool_name, arguments, None).await
    }

    /// Execute a tool by name with per-invocation context.
    /// Routes to BuiltInTool::execute_with_context() for BuiltIn backends.
    /// For Http/Command/Plugin backends, context is ignored (they don't need identity).
    pub async fn execute_with_context(
        &self,
        tool_name: &str,
        arguments: &serde_json::Value,
        ctx: &ToolContext,
    ) -> Result<String, String> {
        self.dispatch(tool_name, arguments, Some(ctx)).await
    }

    /// The single dispatch path — **and the single place the extension gate
    /// runs** (design §6.2 #1, §6.3).
    ///
    /// Both public execute methods funnel here so the gate is taken exactly
    /// once per call: before the refactor, `execute_with_context`'s
    /// `Http | Command | Plugin` arm delegated to `execute`, which would have
    /// double-taken the `CallGuard` and double-counted the drain.
    ///
    /// The gate has **two arms**, because registry removal alone is not
    /// enforcement:
    ///
    /// * **hit** — a run holding a deep snapshot still finds the entry
    ///   (§3.0 Fact 1). Check the entry's extension, including the generation
    ///   its handle carries.
    /// * **miss** — the ordinary skill runs against the live registry and sees
    ///   the entry vanish at T1 (§3.0 Fact 2). The DashMap miss would
    ///   short-circuit with an unattributed *"not found"*, so ask the ledger
    ///   who retains the name and return the same attributed refusal.
    async fn dispatch(
        &self,
        tool_name: &str,
        arguments: &serde_json::Value,
        ctx: Option<&ToolContext>,
    ) -> Result<String, String> {
        // Extract definition + backend and drop the DashMap guard before any .await
        let looked_up = self.tools.get(tool_name).map(|entry| {
            (
                entry.definition.clone(),
                entry.backend.clone(),
                entry.extension_id(),
                entry.incarnation(),
            )
        });

        let Some((definition, backend, extension, incarnation)) = looked_up else {
            // MISS ARM. `check` returns `Ok` for an unknown owner, for an
            // `Enabled` owner that is not server-withdrawing the name, and for
            // an extension with no ledger record at all — all of which fall
            // through to the ordinary not-found error.
            if let Some(ext) = self.extensions.owner_of(tool_name) {
                self.extensions.check(&ext, tool_name, None, ctx)?;
            }
            return Err(match ctx {
                Some(_) => format!("Tool '{}' not found in registry", tool_name),
                None => format!("Unknown tool: '{}'", tool_name),
            });
        };

        // HIT ARM. `None` is a builtin / http / command tool — never gated.
        // The guard lives across the awaited backend call below, which is what
        // T3's drain counts.
        let _guard = match &extension {
            Some(ext) => Some(self.extensions.check(ext, tool_name, incarnation, ctx)?),
            None => None,
        };

        // Pre-validate arguments against the tool's JSON Schema definition.
        validate_tool_arguments(&definition, arguments)?;

        if ctx.is_some()
            && matches!(
                backend,
                ToolBackend::Http { .. } | ToolBackend::Command { .. } | ToolBackend::Plugin(_)
            )
        {
            tracing::trace!(
                tool_name,
                agent_id = ?ctx.and_then(|c| c.agent_id.as_ref()),
                "Tool context discarded (non-BuiltIn backend)"
            );
        }

        match &backend {
            ToolBackend::BuiltIn(handler) => match ctx {
                Some(ctx) => handler.execute_with_context(arguments, ctx).await,
                None => handler.execute(arguments).await,
            },
            ToolBackend::Http {
                method,
                url,
                headers,
                timeout_secs,
            } => {
                execute_http(&self.http_client, method, url, headers, *timeout_secs, arguments)
                    .await
            }
            ToolBackend::Command {
                command,
                args_template,
                timeout_secs,
            } => execute_command(command, args_template.as_deref(), *timeout_secs, arguments).await,
            ToolBackend::Plugin(executor) => executor.execute(tool_name, arguments).await,
            ToolBackend::Mcp {
                client,
                remote_name,
                server_name,
                ..
            } => {
                match ctx {
                    Some(ctx) => tracing::debug!(
                        server = %server_name,
                        remote_name = %remote_name,
                        agent_id = ?ctx.agent_id,
                        skill_stack = ?ctx.skill_stack,
                        "MCP tool call"
                    ),
                    None => tracing::debug!(
                        server = %server_name,
                        remote_name = %remote_name,
                        "MCP tool call (no context)"
                    ),
                }
                match client.call_tool(remote_name, arguments.clone(), None).await {
                    Ok(result) => crate::tools::mcp::bridge::serialize_call_result(result),
                    Err(e) => {
                        tracing::warn!(
                            server = %server_name,
                            remote_name = %remote_name,
                            error_category = ?e.category(),
                            error = %e,
                            "MCP tool call failed"
                        );
                        Err(format!(
                            "MCP server '{server_name}' tool '{remote_name}' failed: {e}"
                        ))
                    }
                }
            }
        }
    }

    /// List registered tool names.
    pub fn registered_tool_names(&self) -> Vec<String> {
        self.tools.iter().map(|e| e.key().clone()).collect()
    }

    /// Iterate over registered tools. Snapshot-style — the DashMap iterator
    /// guard is dropped per-entry via clone(), so the returned iterator
    /// does not hold any lock across await points.
    pub fn iter_registered_tools(&self) -> impl Iterator<Item = (String, RegisteredTool)> + '_ {
        self.tools.iter().map(|e| (e.key().clone(), e.value().clone()))
    }

    /// Returns all virtual capability names known to registered providers.
    /// Used by config validation.
    pub fn known_virtual_capabilities(&self) -> Vec<String> {
        self.capability_providers
            .iter()
            .flat_map(|e| e.value().known_capability_names().into_iter())
            .collect()
    }

    /// Register a capability provider and return a handle for later removal.
    ///
    /// Triggers a full rebuild of `capability_index` so tools registered
    /// before this call also gain the new provider's virtual capabilities.
    ///
    /// Rebuild cost: O(tools × providers × caps_per_call). For realistic
    /// deployments this is microseconds. Provider lifecycle events are
    /// infrequent — not a hot path.
    ///
    /// Safe to call concurrently from any Arc<ToolRegistry> owner. Internal
    /// serialization via `provider_mutex` prevents two rebuilds from racing.
    pub fn register_capability_provider(
        &self,
        provider: Arc<dyn capabilities::CapabilityProvider>,
    ) -> ProviderHandle {
        let handle = ProviderHandle(self.next_provider_handle.fetch_add(1, Ordering::Relaxed));
        let _guard = self.provider_mutex.lock().unwrap_or_else(|p| p.into_inner());

        self.capability_providers.insert(handle, provider);
        self.rebuild_virtual_capability_index();

        tracing::info!(
            handle = %handle,
            total_providers = self.capability_providers.len(),
            "registered capability provider"
        );
        handle
    }

    /// Remove a previously-registered capability provider by handle.
    ///
    /// Returns `true` if a provider with that handle existed and was removed,
    /// `false` if the handle was unknown. Idempotent on unknown handles.
    ///
    /// Triggers a full rebuild of `capability_index`. Virtual capabilities
    /// produced only by the removed provider are scrubbed. Entries produced
    /// by other still-active providers remain.
    pub fn remove_capability_provider(&self, handle: ProviderHandle) -> bool {
        let _guard = self.provider_mutex.lock().unwrap_or_else(|p| p.into_inner());

        let removed = self.capability_providers.remove(&handle).is_some();
        if removed {
            self.rebuild_virtual_capability_index();
            tracing::info!(
                handle = %handle,
                total_providers = self.capability_providers.len(),
                "removed capability provider"
            );
        } else {
            tracing::debug!(handle = %handle, "remove on unknown provider handle");
        }
        removed
    }

    /// Return all currently-active provider handles, for introspection.
    /// Order is not guaranteed.
    pub fn provider_handles(&self) -> Vec<ProviderHandle> {
        self.capability_providers
            .iter()
            .map(|e| *e.key())
            .collect()
    }

    /// Clear and rebuild the capability_index from current state.
    ///
    /// MUST be called with provider_mutex held. Iterates every tool and
    /// every active provider, accumulating entries into a fresh HashMap,
    /// then replaces the index contents.
    ///
    /// Readers of capability_index during rebuild may observe a transient
    /// empty-or-partial state (typically sub-millisecond). Acceptable for
    /// infrequent provider lifecycle events.
    fn rebuild_virtual_capability_index(&self) {
        let mut new_index: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();

        for entry in self.tools.iter() {
            let tool_name = entry.key().clone();
            let tool = entry.value();

            // String capabilities from the tool itself.
            for cap in &tool.provides_capabilities {
                new_index.entry(cap.clone()).or_default().push(tool_name.clone());
            }

            // Virtual capabilities from all registered providers.
            for provider_entry in self.capability_providers.iter() {
                for cap in provider_entry.value().derive_capabilities(tool) {
                    new_index.entry(cap).or_default().push(tool_name.clone());
                }
            }
        }

        // Swap in: clear existing entries, repopulate.
        self.capability_index.clear();
        for (k, v) in new_index {
            self.capability_index.insert(k, v);
        }
    }

    /// Number of registered tools.
    pub fn count(&self) -> usize {
        self.tools.len()
    }

    /// Check if a tool is exempt from the per-tool sandbox timeout.
    pub fn is_exempt_from_timeout(&self, tool_name: &str) -> bool {
        self.tools
            .get(tool_name)
            .map(|t| t.value().exempt_from_timeout)
            .unwrap_or(false)
    }

    /// Returns tool definitions for all tools whose `provides_capabilities`
    /// intersects with the requested capabilities. Empty capabilities returns empty.
    ///
    /// Uses the inverted capability index for O(capabilities * tools_per_cap) lookup
    /// instead of scanning all registered tools.
    pub fn tools_for_capabilities(&self, capabilities: &[String]) -> Vec<ToolDefinition> {
        self.resolve_capabilities(capabilities, &[]).defs
    }

    /// Returns tool definitions for all tools whose `provides_capabilities`
    /// intersects with the requested capabilities, excluding tools that provide
    /// any denied capability.
    ///
    /// Uses the inverted capability index for efficient lookup, then filters
    /// out tools that provide any denied capability.
    pub fn tools_for_capabilities_with_deny(
        &self,
        capabilities: &[String],
        denied: &[String],
    ) -> Vec<ToolDefinition> {
        self.resolve_capabilities(capabilities, denied).defs
    }

    /// Resolve capabilities to tool definitions **and say what was lost**.
    ///
    /// This is the single point at which a disabled extension's disappearance
    /// stops being invisible (design §6.2 #3, §7.2): today an unindexed
    /// capability simply contributes nothing — no error, no warn, no
    /// diagnostic — and with two providers of one capability, disabling one
    /// shrinks the tool set with no signal at all.
    ///
    /// Classification per requested capability:
    /// * **withheld** — the index lookup is empty *and* the tombstone set is
    ///   non-empty: every provider is gone.
    /// * **partially withheld** — the lookup is non-empty but the tombstone set
    ///   records a provider: A is disabled while B still serves it.
    /// * **unknown** — both empty: nothing ever provided it. `debug!` only,
    ///   because a typo and a withdrawal are indistinguishable today and
    ///   promoting unattributed misses would fire on every existing install.
    ///
    /// `defs` is byte-for-byte what the two wrappers returned before.
    pub fn resolve_capabilities(
        &self,
        capabilities: &[String],
        denied: &[String],
    ) -> CapabilityResolution {
        let mut resolution = CapabilityResolution::default();
        if capabilities.is_empty() {
            return resolution;
        }
        let mut seen = std::collections::HashSet::new();
        for cap in capabilities {
            let mut resolved_any = false;
            if let Some(names) = self.capability_index.get(cap) {
                resolved_any = !names.value().is_empty();
                for name in names.value() {
                    if seen.insert(name.clone())
                        && let Some(tool) = self.tools.get(name)
                    {
                        // Full capability set = string caps + virtual caps from all providers.
                        let has_denied = !denied.is_empty() && {
                            let mut all_caps: Vec<String> = tool.provides_capabilities.clone();
                            for provider_entry in self.capability_providers.iter() {
                                all_caps.extend(
                                    provider_entry.value().derive_capabilities(tool.value()),
                                );
                            }
                            all_caps.iter().any(|c| denied.contains(c))
                        };
                        if !has_denied {
                            resolution.defs.push(tool.definition.clone());
                        }
                    }
                }
            }

            let recorded = self.extensions.recorded_providers(cap);
            if recorded.is_empty() {
                if !resolved_any {
                    tracing::debug!(capability = %cap, "capability resolves to nothing and was never provided");
                    resolution.unknown.push(cap.clone());
                }
                continue;
            }
            let blocked: Vec<WithheldBy> = self
                .extensions
                .blocked_providers(cap)
                .into_iter()
                .map(|(extension, server_withdrawn)| WithheldBy {
                    extension,
                    server_withdrawn,
                })
                .collect();
            let entry = WithheldCapability {
                capability: cap.clone(),
                providers: blocked,
            };
            if resolved_any {
                resolution.partially_withheld.push(entry);
            } else {
                resolution.withheld.push(entry);
            }
        }
        resolution
    }

    /// Tool definitions for all "extension" tools — those bridged from MCP
    /// servers (`ToolBackend::Mcp`, registered as `<server>__<tool>`) or
    /// provided by plugins (`ToolBackend::Plugin`, registered as
    /// `<plugin>::<tool>`) — excluding any name on `deny` (the
    /// `execution.skill_defaults.global_tool_deny` list, the opt-out for
    /// surfaces that include extension tools by default).
    ///
    /// The backend variant is the origin marker: builtins register as
    /// `BuiltIn`, custom TOML tools as `Http`/`Command`. Sorted by name so
    /// callers get a deterministic surface (DashMap iteration is unordered,
    /// and tool ordering feeds prompt-cache fingerprints).
    /// Tools whose extension is not `Enabled` are dropped — **hygiene only**,
    /// so the model does not burn a round on a call the gate will refuse. An
    /// absent ledger entry counts as enabled (design §6.2 #2, §6.2a).
    pub fn extension_tool_defs(&self, deny: &[String]) -> Vec<ToolDefinition> {
        let mut defs: Vec<ToolDefinition> = self
            .tools
            .iter()
            .filter(|e| {
                matches!(
                    e.value().backend,
                    ToolBackend::Mcp { .. } | ToolBackend::Plugin(_)
                ) && !deny.contains(e.key())
                    && self.extension_is_available(e.value())
            })
            .map(|e| e.value().definition.clone())
            .collect();
        defs.sort_by(|a, b| a.name.cmp(&b.name));
        defs
    }

    /// The `extension_tool_defs` state filter, reusable by the main loop's
    /// `tool_selection = "full"` branch, which builds its surface from
    /// `registered_tool_names()` + `get()` and so never passes through the
    /// filter above (design §6.2 #2).
    pub fn extension_is_available(&self, tool: &RegisteredTool) -> bool {
        match tool.extension_id() {
            None => true,
            Some(ext) => self
                .extensions
                .state(&ext)
                .is_none_or(|state| state.is_enabled()),
        }
    }

    /// Return the names of tools that use command backends (i.e., execute via shell).
    /// These should be treated like shell tools for injection sanitization.
    pub fn command_backend_tool_names(&self) -> Vec<String> {
        self.tools
            .iter()
            .filter_map(|entry| match &entry.value().backend {
                ToolBackend::Command { .. } => Some(entry.key().clone()),
                ToolBackend::BuiltIn(_) | ToolBackend::Http { .. } | ToolBackend::Plugin(_) | ToolBackend::Mcp { .. } => None,
            })
            .collect()
    }
}

/// What `resolve_capabilities` found, and what it could not find (design §7.2).
#[derive(Debug, Clone, Default)]
pub struct CapabilityResolution {
    /// The tool definitions, exactly as the two wrappers returned them.
    pub defs: Vec<ToolDefinition>,
    /// Every provider is gone. `CapabilityOracle::is_satisfiable` is "this list
    /// is empty" — one predicate for invocation, the router, the catalog, the
    /// cron skip and `/slash`.
    pub withheld: Vec<WithheldCapability>,
    /// A provider was withdrawn but another still serves the capability. Warned
    /// and reported, **never gating**.
    pub partially_withheld: Vec<WithheldCapability>,
    /// Nothing ever provided it — a typo, or an annotation capability. `debug!`
    /// only, so no existing install changes behaviour on upgrade.
    pub unknown: Vec<String>,
}

/// One capability and the recorded providers that cannot currently serve it.
#[derive(Debug, Clone)]
pub struct WithheldCapability {
    pub capability: String,
    pub providers: Vec<WithheldBy>,
}

/// The attribution a warning, a `/slash` refusal or a cron skip carries.
#[derive(Debug, Clone)]
pub struct WithheldBy {
    pub extension: ExtensionId,
    /// `true` when the extension is still `Enabled` and the *server* withdrew
    /// the name (design §3.7): the attribution names the owner as **still
    /// enabled**, never as disabled.
    pub server_withdrawn: bool,
}

/// Execute an HTTP backend tool call using the shared HTTP client.
async fn execute_http(
    client: &reqwest::Client,
    method: &str,
    url_template: &str,
    headers: &HashMap<String, String>,
    timeout_secs: u64,
    arguments: &serde_json::Value,
) -> Result<String, String> {
    // Replace {param_name} placeholders in URL with URL-encoded argument values
    let mut url = url_template.to_string();
    if let Some(obj) = arguments.as_object() {
        for (key, value) in obj {
            let replacement = match value {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            let encoded = urlencoding::encode(&replacement);
            url = url.replace(&format!("{{{}}}", key), &encoded);
        }
    }

    // Check for unsubstituted {param_name} placeholders.
    // These indicate missing required arguments and would produce malformed URLs.
    if let Some(start) = url.find('{')
        && let Some(end) = url[start..].find('}')
    {
        let placeholder = &url[start..start + end + 1];
        return Err(format!(
            "URL template has unsubstituted placeholder '{}' — \
             the required parameter was not provided in tool arguments",
            placeholder
        ));
    }

    // SSRF protection: validate the resolved URL before making the request.
    // The URL template may contain user-controlled placeholders that could
    // redirect requests to internal/private endpoints.
    crate::tools::url_validation::validate_url(&url)?;

    let timeout = std::time::Duration::from_secs(timeout_secs);

    let mut request = match method.to_uppercase().as_str() {
        "POST" => client.post(&url),
        "PUT" => client.put(&url),
        "DELETE" => client.delete(&url),
        _ => client.get(&url),
    };

    for (k, v) in headers {
        request = request.header(k.as_str(), v.as_str());
    }

    request = request.timeout(timeout);

    let response = request
        .send()
        .await
        .map_err(|e| format!("HTTP request failed: {}", e))?;

    let status = response.status();

    // Stream body with size cap to prevent memory exhaustion.
    // Read at most MAX_HTTP_RESPONSE_BYTES before stopping.
    const MAX_HTTP_RESPONSE_BYTES: usize = 1_048_576; // 1 MB
    let body = {
        use futures_util::StreamExt;
        let mut body_bytes = Vec::with_capacity(8192);
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| format!("Failed to read response body: {}", e))?;
            body_bytes.extend_from_slice(&chunk);
            if body_bytes.len() >= MAX_HTTP_RESPONSE_BYTES {
                body_bytes.truncate(MAX_HTTP_RESPONSE_BYTES);
                break;
            }
        }
        String::from_utf8_lossy(&body_bytes).into_owned()
    };

    if status.is_success() {
        // Truncate to 8KB
        Ok(body.chars().take(8192).collect())
    } else {
        Err(format!(
            "HTTP {} — {}",
            status,
            body.chars().take(1024).collect::<String>()
        ))
    }
}

/// Execute a command backend tool call.
async fn execute_command(
    command: &str,
    args_template: Option<&str>,
    timeout_secs: u64,
    arguments: &serde_json::Value,
) -> Result<String, String> {
    let mut full_args = args_template.unwrap_or("").to_string();
    if let Some(obj) = arguments.as_object() {
        for (key, value) in obj {
            let replacement = match value {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            let escaped = shell_escape::escape(replacement.into());
            full_args = full_args.replace(&format!("{{{}}}", key), &escaped);
        }
    }

    // Check for unsubstituted {param_name} placeholders.
    // These indicate missing required arguments and would produce malformed commands.
    if let Some(start) = full_args.find('{')
        && let Some(end) = full_args[start..].find('}')
    {
        let placeholder = &full_args[start..start + end + 1];
        return Err(format!(
            "Command template has unsubstituted placeholder '{}' — \
             the required parameter was not provided in tool arguments",
            placeholder
        ));
    }

    let timeout = std::time::Duration::from_secs(timeout_secs);
    let full_command = format!("{} {}", command, full_args);

    let output = tokio::time::timeout(timeout, {
        crate::tools::platform::shell_command(&full_command).output()
    })
    .await
    .map_err(|_| format!("Command timed out after {}s", timeout_secs))?
    .map_err(|e| format!("Failed to execute command: {}", e))?;

    // Cap output size to prevent memory pressure from commands producing
    // unbounded stdout/stderr.
    const MAX_OUTPUT_BYTES: usize = 512 * 1024; // 512 KB
    let stdout_raw = &output.stdout;
    let stderr_raw = &output.stderr;
    let stdout = String::from_utf8_lossy(&stdout_raw[..stdout_raw.len().min(MAX_OUTPUT_BYTES)]);
    let stderr = String::from_utf8_lossy(&stderr_raw[..stderr_raw.len().min(MAX_OUTPUT_BYTES)]);

    if output.status.success() {
        Ok(stdout.to_string())
    } else {
        Err(format!(
            "Command failed (exit {}): {}{}",
            output.status.code().unwrap_or(-1),
            stdout,
            stderr
        ))
    }
}

/// Basic JSON Schema pre-validation for tool arguments.
///
/// Checks:
/// - Arguments must be a JSON object (if the schema specifies `type: "object"`)
/// - All fields listed in `required` must be present
/// - Field types match the schema's `properties.<name>.type` when specified
///
/// This is not a full JSON Schema validator — it catches the most common
/// argument errors early (missing required fields, wrong root type) to provide
/// clear error messages instead of cryptic tool failures.
fn validate_tool_arguments(
    definition: &ToolDefinition,
    arguments: &serde_json::Value,
) -> Result<(), String> {
    let schema = &definition.parameters;

    // Check root type is object (if schema says so)
    if schema.get("type").and_then(|t| t.as_str()) == Some("object") && !arguments.is_object() {
        return Err(format!(
            "Tool '{}': arguments must be a JSON object, got {}",
            definition.name,
            json_type_name(arguments)
        ));
    }

    // Check required fields
    if let Some(required) = schema.get("required").and_then(|r| r.as_array())
        && let Some(obj) = arguments.as_object()
    {
        for req in required {
            if let Some(field_name) = req.as_str()
                && !obj.contains_key(field_name)
            {
                return Err(format!(
                    "Tool '{}': missing required parameter '{}'",
                    definition.name, field_name
                ));
            }
        }
    }

    // Check field types and enum constraints against schema properties
    if let (Some(properties), Some(obj)) = (
        schema.get("properties").and_then(|p| p.as_object()),
        arguments.as_object(),
    ) {
        for (key, value) in obj {
            if let Some(prop_schema) = properties.get(key) {
                // Type check
                if let Some(expected_type) = prop_schema.get("type").and_then(|t| t.as_str())
                    && !json_value_matches_type(value, expected_type)
                {
                    return Err(format!(
                        "Tool '{}': parameter '{}' should be {}, got {}",
                        definition.name,
                        key,
                        expected_type,
                        json_type_name(value)
                    ));
                }
                // Enum check
                if let Some(enum_values) = prop_schema.get("enum").and_then(|e| e.as_array()) {
                    if !enum_values.contains(value) {
                        return Err(format!(
                            "Tool '{}': parameter '{}' value {} not in allowed values",
                            definition.name, key, value
                        ));
                    }
                }
            }
        }
    }

    Ok(())
}

/// Return a human-readable type name for a JSON value.
fn json_type_name(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(n) => {
            if n.is_i64() || n.is_u64() {
                "integer"
            } else {
                "number"
            }
        }
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

/// Check if a JSON value matches the expected JSON Schema type.
fn json_value_matches_type(value: &serde_json::Value, expected: &str) -> bool {
    match expected {
        "string" => value.is_string(),
        "number" => value.is_number(),
        "integer" => value.is_i64() || value.is_u64(),
        "boolean" => value.is_boolean(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        "null" => value.is_null(),
        _ => {
            tracing::debug!(expected_type = expected, "Unknown JSON Schema type — rejecting");
            false
        }
    }
}

#[cfg(test)]
mod tests;
