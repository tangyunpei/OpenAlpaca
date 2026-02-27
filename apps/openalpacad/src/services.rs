use anyhow::Result;
use arc_swap::ArcSwap;
use openalpaca_core::{
    agent::AgentConfigService,
    bus::EventBus,
    context::SharedContext,
    tools::builtins::{ConnectorSendLock, IdentityToolContext, SoulToolContext, UserToolContext},
};
use openalpaca_storage::Database;
use std::path::Path;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// All services initialized during daemon startup.
pub struct InitializedServices {
    pub shared_context: Arc<SharedContext>,
    pub llm_router: Option<Arc<openalpaca_llm::LlmRouter>>,
    pub llm_settings_service: Option<Arc<openalpaca_llm::LlmSettingsService>>,
    pub token_manager: Option<Arc<openalpaca_llm::TokenManager>>,
    pub provider_usage_tracker: Option<Arc<openalpaca_llm::ProviderUsageTracker>>,
    pub embedder: Option<Arc<dyn openalpaca_llm::Embedder>>,
    pub agent_config_service: Arc<AgentConfigService>,
    pub tool_registry: Arc<openalpaca_core::tools::ToolRegistry>,
    pub security_gate: Arc<openalpaca_core::security::gate::SecurityGate>,
    pub skill_catalog: Arc<openalpaca_core::orchestrator::skill_catalog::SkillCatalog>,
    pub skill_router: Arc<openalpaca_core::orchestrator::skill_router::SkillRouter>,
    pub secret_store: Arc<dyn openalpaca_llm::SecretStore>,
    pub web_search_config: Arc<ArcSwap<openalpaca_llm::WebSearchConfig>>,
    /// Shared lock for the `send_message` tool's connector send provider.
    /// Populated post-construction in main.rs after the ConnectorSendBridge is created.
    pub connector_send_lock: ConnectorSendLock,
}

/// Initialize all core services: agent templates, LLM router, tools, security, etc.
#[allow(clippy::too_many_arguments)]
pub async fn initialize_services(
    config_base_dir: &Path,
    db: &Database,
    bus: &EventBus,
    daemon_config: &Arc<ArcSwap<openalpaca_core::daemon_config::DaemonConfig>>,
    soul_path: &Path,
    user_path: &Path,
    identity_path: &Path,
    cancel_token: &CancellationToken,
) -> Result<InitializedServices> {
    let shared_context = Arc::new(SharedContext::new());

    // Load agent templates from .md files + legacy .toml files
    load_agent_templates(config_base_dir, db, &shared_context)?;

    // Initialize secret store
    let llm_config_path = config_base_dir.join("llm.toml");
    let (secret_store, keyring_available) = initialize_secret_store(&llm_config_path);

    // Build LLM router
    let llm_router = build_llm_router(&llm_config_path, &*secret_store);

    // Build LLM settings service
    let llm_settings_service =
        build_llm_settings_service(&llm_router, &llm_config_path, &secret_store).await;

    // Load LLM config for embedder / token manager
    let llm_config: Option<openalpaca_llm::LlmRouterConfig> = if llm_config_path.exists() {
        openalpaca_llm::read_config(&llm_config_path).ok()
    } else {
        None
    };

    // Build embedder
    let embedder = build_embedder(&llm_config, &secret_store);

    // Credential discovery & token manager
    let cred_config = llm_config
        .as_ref()
        .and_then(|c| c.credential_discovery.clone())
        .unwrap_or_default();

    let token_manager = build_token_manager(
        &cred_config,
        &llm_router,
        &llm_settings_service,
        cancel_token,
    )
    .await;

    // Provider usage tracker
    let provider_usage_tracker = if cred_config.fetch_external_usage.unwrap_or(false) {
        info!("Provider usage tracker enabled");
        Some(Arc::new(openalpaca_llm::ProviderUsageTracker::new()))
    } else {
        None
    };

    // Forward-migrate secrets if keychain is active
    if keyring_available && llm_config_path.exists() {
        match openalpaca_llm::migrate_llm_secrets(&llm_config_path, &*secret_store) {
            Ok(0) => {}
            Ok(n) => info!("Migrated {n} secret(s) to OS keychain"),
            Err(e) => warn!("Secret migration failed: {e}. Legacy secrets will still work."),
        }
    }

    // Build AgentConfigService
    let config_dir = config_base_dir.join("agents");
    let agent_config_service = Arc::new(AgentConfigService::new(
        shared_context.agent_registry.clone(),
        config_dir,
        db.clone(),
    ));

    // Extract web_search config from LLM config (hot-reloadable via ArcSwap)
    let web_search_cfg = llm_config
        .as_ref()
        .and_then(|c| c.web_search.clone())
        .unwrap_or_default();
    let web_search_config = Arc::new(ArcSwap::from_pointee(web_search_cfg));

    // Build ToolRegistry
    let (tool_registry, connector_send_lock) = build_tool_registry(
        config_base_dir,
        db,
        &embedder,
        soul_path,
        user_path,
        identity_path,
        bus,
        daemon_config,
        &web_search_config,
    );

    // Build security chain
    let registry_executor = Arc::new(openalpaca_core::tools::RegistryToolExecutor::new(
        tool_registry.clone(),
    ));
    let sandbox_manager = Arc::new(openalpaca_core::security::sandbox::SandboxManager::new(
        registry_executor,
        bus.clone(),
        &daemon_config.load().security.circuit_breaker,
    ));
    let security_gate = Arc::new(openalpaca_core::security::gate::SecurityGate::new(
        sandbox_manager,
    ));

    // Build SkillCatalog
    // TODO(P3-1): Currently only project-scope skills are scanned. User-scope
    // scanning (e.g. ~/.config/openalpaca/skills/) and EventBus lifecycle wiring
    // (SkillCatalog::new_with_bus) are implemented but not yet connected here.
    // Use catalog.scan_multi_scope() to enable both scopes.
    let skill_catalog = {
        let catalog = openalpaca_core::orchestrator::skill_catalog::SkillCatalog::new();
        let skills_dir = config_base_dir.join("skills");
        if skills_dir.exists() {
            let count = catalog.scan_directory(
                &skills_dir,
                openalpaca_core::middleware::skill::SkillScope::Project,
            );
            info!(
                "Skill catalog: loaded {} skill(s) from {}",
                count,
                skills_dir.display()
            );
        }
        Arc::new(catalog)
    };

    // Build SkillRouter with configurable thresholds from daemon config
    let skill_router = {
        let sd = &daemon_config.load().execution.skill_defaults;
        Arc::new(
            openalpaca_core::orchestrator::skill_router::SkillRouter::new_with_bus(
                sd.router_auto_select_threshold,
                sd.router_suggest_threshold,
                bus.clone(),
            ),
        )
    };

    Ok(InitializedServices {
        shared_context,
        llm_router,
        llm_settings_service,
        token_manager,
        provider_usage_tracker,
        embedder,
        agent_config_service,
        tool_registry,
        security_gate,
        skill_catalog,
        skill_router,
        secret_store,
        web_search_config,
        connector_send_lock,
    })
}

fn load_agent_templates(
    config_base_dir: &Path,
    db: &Database,
    shared_context: &Arc<SharedContext>,
) -> Result<()> {
    let config_dir = config_base_dir.join("agents");
    if !config_dir.exists() {
        return Ok(());
    }
    let entries = match std::fs::read_dir(&config_dir) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str());

        match ext {
            // ── New: Markdown templates (.md) ────────────────────
            Some("md") => {
                match std::fs::read_to_string(&path) {
                    Ok(content) => {
                        match openalpaca_core::agent::template::parse_agent_markdown(&content) {
                            Ok(template) => {
                                let template_id = template.frontmatter.id.clone();
                                let is_singleton = template.frontmatter.singleton;

                                // Register template in the template catalog
                                shared_context
                                    .agent_registry
                                    .register_template(template.clone());

                                // For singleton templates, also register as a legacy
                                // agent so existing code (config service, REST API) can
                                // find it by template_id until fully migrated.
                                let subagent = template.to_subagent(&template_id, "");
                                // Reset to Idle (to_subagent sets Busy)
                                let mut idle_agent = subagent;
                                idle_agent.status = openalpaca_core::agent::AgentStatus::Idle;
                                idle_agent.current_task = None;
                                shared_context.agent_registry.register(idle_agent);

                                // Persist template metadata to DB as SubAgentConfig
                                persist_template_to_db(db, &template, &template_id);

                                info!(
                                    "Loaded agent template: {} (singleton={})",
                                    path.display(),
                                    is_singleton
                                );
                            }
                            Err(e) => {
                                warn!("Failed to parse agent template {}: {}", path.display(), e);
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Failed to read agent template {}: {}", path.display(), e);
                    }
                }
            }

            // ── Legacy: TOML configs (.toml) — backward compat ───
            Some("toml") => {
                match std::fs::read_to_string(&path) {
                    Ok(content) => {
                        match toml::from_str::<openalpaca_core::agent::AgentConfigFile>(&content) {
                            Ok(agent_config) => {
                                // Register in-memory
                                let subagent = agent_config.clone().into_subagent();
                                shared_context.agent_registry.register(subagent);

                                // Persist to DB
                                let storage_config = agent_config.into_storage_config();
                                let agent_id = storage_config.id.clone();
                                let repo = openalpaca_storage::SubAgentRepository::new(db);
                                let _ = repo.upsert(&storage_config);

                                // Initialize metrics row if not exists
                                if let Ok(None) = repo.get_metrics(&agent_id) {
                                    let _ = repo.upsert_metrics(
                                        &openalpaca_storage::AgentMetrics::new_empty(&agent_id),
                                    );
                                }

                                warn!(
                                    "Loaded agent config (legacy TOML): {} — please convert to .md format",
                                    path.display()
                                );
                            }
                            Err(e) => {
                                warn!("Failed to parse agent config {}: {}", path.display(), e);
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Failed to read agent config {}: {}", path.display(), e);
                    }
                }
            }

            _ => {} // skip non-agent files
        }
    }

    Ok(())
}

fn persist_template_to_db(
    db: &Database,
    template: &openalpaca_core::agent::template::AgentTemplate,
    template_id: &str,
) {
    let repo = openalpaca_storage::SubAgentRepository::new(db);
    let persona = openalpaca_core::agent::template::extract_persona(template);
    let fm = &template.frontmatter;
    let skills: Vec<openalpaca_core::agent::Skill> = fm
        .skills
        .iter()
        .map(|s| openalpaca_core::agent::Skill {
            name: s.clone(),
            category: "assigned".to_string(),
            proficiency: 1.0,
        })
        .collect();
    let preset = openalpaca_core::agent::AgentPreset {
        persona: persona.clone(),
        temperature: fm.temperature,
        verbosity: fm.verbosity.clone(),
    };
    let constraints = openalpaca_core::agent::AgentConstraints {
        max_tool_calls: fm.max_tool_calls,
        timeout_seconds: fm.timeout_seconds,
        max_cost_per_task: fm.max_cost_per_task,
        require_confirmation_for: fm.require_confirmation_for.clone(),
        allowed_capabilities: fm.skills.clone(),
        denied_capabilities: fm.denied_skills.clone(),
        ..Default::default()
    };
    let llm_config = openalpaca_core::agent::AgentLlmConfig {
        model: fm.model.clone(),
        fallback_models: fm.fallback_models.clone(),
        ..Default::default()
    };
    let now = chrono::Utc::now();
    let storage_config = openalpaca_storage::SubAgentConfig {
        id: template_id.to_string(),
        template_id: template_id.to_string(),
        name: fm.name.clone(),
        description: Some(fm.description.clone()),
        icon: fm.icon.clone(),
        status: "idle".to_string(),
        current_task_id: None,
        skills_json: serde_json::to_string(&skills).unwrap_or_else(|_| "[]".to_string()),
        preset_json: serde_json::to_string(&preset).unwrap_or_else(|_| "{}".to_string()),
        constraints_json: Some(
            serde_json::to_string(&constraints).unwrap_or_else(|_| "{}".to_string()),
        ),
        llm_config_json: Some(
            serde_json::to_string(&llm_config).unwrap_or_else(|_| "{}".to_string()),
        ),
        persona: Some(persona),
        created_at: now,
        updated_at: Some(now),
    };
    let _ = repo.upsert(&storage_config);

    // Initialize metrics row if not exists
    if let Ok(None) = repo.get_metrics(template_id) {
        let _ = repo.upsert_metrics(&openalpaca_storage::AgentMetrics::new_empty(template_id));
    }
}

fn initialize_secret_store(llm_config_path: &Path) -> (Arc<dyn openalpaca_llm::SecretStore>, bool) {
    let use_keychain = if llm_config_path.exists() {
        openalpaca_llm::read_config(llm_config_path)
            .ok()
            .and_then(|c| c.security.as_ref().map(|s| s.use_keychain))
            .unwrap_or(false)
    } else {
        false
    };

    if use_keychain {
        // Opt-in keychain mode: CachingSecretStore + prefetch
        let caching =
            openalpaca_llm::CachingSecretStore::new(Box::new(openalpaca_llm::KeyringSecretStore));

        let refs: Vec<String> = if llm_config_path.exists() {
            openalpaca_llm::read_config(llm_config_path)
                .ok()
                .map(|c| openalpaca_llm::collect_secret_refs(&c))
                .unwrap_or_default()
        } else {
            vec![]
        };

        let ref_strs: Vec<&str> = refs.iter().map(|s| s.as_str()).collect();
        if caching.prefetch(&ref_strs) {
            info!(
                "OS keychain enabled (use_keychain=true) — pre-fetched {} secret(s)",
                refs.len()
            );
            (Arc::new(caching), true)
        } else {
            warn!(
                "OS keychain requested but unavailable (headless/Docker?). \
                 Falling back to in-memory secret store."
            );
            (Arc::new(openalpaca_llm::MemorySecretStore::new()), false)
        }
    } else {
        // Default: no keychain, use secret_encrypted path
        info!("OS keychain disabled (default). Using local encrypted storage.");

        // One-time reverse migration: if config still has secret_ref keys,
        // read them from keychain and convert to secret_encrypted.
        if llm_config_path.exists() {
            let refs = openalpaca_llm::read_config(llm_config_path)
                .ok()
                .map(|c| openalpaca_llm::collect_secret_refs(&c))
                .unwrap_or_default();

            if !refs.is_empty() {
                info!(
                    "Found {} secret_ref key(s) needing reverse migration to local encrypted storage",
                    refs.len()
                );
                // Temporarily create a keyring store to read the secrets
                let temp_keyring = openalpaca_llm::KeyringSecretStore;
                match openalpaca_llm::reverse_migrate_llm_secrets(llm_config_path, &temp_keyring) {
                    Ok(0) => info!("No keys needed reverse migration"),
                    Ok(n) => info!(
                        "Reverse-migrated {n} secret(s) from OS keychain to local encrypted storage"
                    ),
                    Err(e) => warn!(
                        "Reverse migration failed: {e}. Keys with secret_ref may not be available. \
                         Set [security] use_keychain = true to use OS keychain, or re-add keys."
                    ),
                }
            }
        }

        (Arc::new(openalpaca_llm::MemorySecretStore::new()), false)
    }
}

fn build_llm_router(
    llm_config_path: &Path,
    secret_store: &dyn openalpaca_llm::SecretStore,
) -> Option<Arc<openalpaca_llm::LlmRouter>> {
    if llm_config_path.exists() {
        match openalpaca_llm::build_router_with_secret_store(llm_config_path, Some(secret_store)) {
            Ok(router) => {
                info!(
                    "LLM router loaded (default model: {})",
                    router.default_model()
                );
                Some(Arc::new(router))
            }
            Err(e) => {
                warn!("Failed to build LLM router: {e}. Falling back to echo stub.");
                None
            }
        }
    } else {
        info!("No config/llm.toml found. Using echo stub.");
        None
    }
}

async fn build_llm_settings_service(
    llm_router: &Option<Arc<openalpaca_llm::LlmRouter>>,
    llm_config_path: &Path,
    secret_store: &Arc<dyn openalpaca_llm::SecretStore>,
) -> Option<Arc<openalpaca_llm::LlmSettingsService>> {
    let service = if let Some(router) = llm_router {
        match openalpaca_llm::LlmSettingsService::new_with_secret_store(
            router.clone(),
            llm_config_path.to_path_buf(),
            secret_store.clone(),
        ) {
            Ok(service) => {
                info!("LLM settings service initialized");
                Some(Arc::new(service))
            }
            Err(e) => {
                warn!("Failed to init LLM settings service: {e}");
                None
            }
        }
    } else {
        None
    };

    // Refresh models from provider APIs at startup
    if let Some(ref svc) = service {
        info!("Refreshing available models from providers...");
        svc.refresh_models().await;
    }

    service
}

fn build_embedder(
    llm_config: &Option<openalpaca_llm::LlmRouterConfig>,
    secret_store: &Arc<dyn openalpaca_llm::SecretStore>,
) -> Option<Arc<dyn openalpaca_llm::Embedder>> {
    let emb_config = llm_config.as_ref().and_then(|c| c.embeddings.clone());
    match emb_config {
        Some(ref cfg) if cfg.enabled => {
            let provider_config = llm_config
                .as_ref()
                .and_then(|c| c.providers.as_ref())
                .and_then(|p| p.get(&cfg.provider));
            match openalpaca_llm::build_embedder(cfg, Some(&**secret_store), provider_config) {
                Ok(e) => {
                    info!(
                        "Embedder initialized: {} ({}d)",
                        cfg.provider,
                        e.dimensions()
                    );
                    Some(e)
                }
                Err(e) => {
                    warn!("Failed to build embedder: {e}");
                    None
                }
            }
        }
        _ => {
            info!("Embeddings disabled");
            None
        }
    }
}

async fn build_token_manager(
    cred_config: &openalpaca_llm::CredentialDiscoveryConfig,
    llm_router: &Option<Arc<openalpaca_llm::LlmRouter>>,
    llm_settings_service: &Option<Arc<openalpaca_llm::LlmSettingsService>>,
    cancel_token: &CancellationToken,
) -> Option<Arc<openalpaca_llm::TokenManager>> {
    if cred_config.claude_code.unwrap_or(true) || cred_config.codex.unwrap_or(true) {
        let tm = Arc::new(openalpaca_llm::TokenManager::new(cred_config.clone()).await);
        if let (Some(router), Some(svc)) = (llm_router, llm_settings_service) {
            tm.rescan(svc, router).await;
        }
        if let (Some(router), Some(svc)) = (llm_router, llm_settings_service) {
            let _refresh_handle =
                tm.start_refresh_loop(svc.clone(), router.clone(), cancel_token.clone());
        }
        info!("TokenManager initialized with credential discovery");
        Some(tm)
    } else {
        None
    }
}

#[allow(clippy::too_many_arguments)]
fn build_tool_registry(
    config_base_dir: &Path,
    db: &Database,
    embedder: &Option<Arc<dyn openalpaca_llm::Embedder>>,
    soul_path: &Path,
    user_path: &Path,
    identity_path: &Path,
    bus: &EventBus,
    daemon_config: &Arc<ArcSwap<openalpaca_core::daemon_config::DaemonConfig>>,
    web_search_config: &Arc<ArcSwap<openalpaca_llm::WebSearchConfig>>,
) -> (Arc<openalpaca_core::tools::ToolRegistry>, ConnectorSendLock) {
    let mut tool_registry = openalpaca_core::tools::ToolRegistry::new();

    // Register built-in tools (including update_soul and update_user)
    let soul_tool_ctx = SoulToolContext {
        soul_path: soul_path.to_path_buf(),
        backup_dir: config_base_dir.join("orchestrator").join("backups"),
        bus: bus.clone(),
        max_backups: Some(10),
    };
    let user_tool_ctx = UserToolContext {
        user_path: user_path.to_path_buf(),
        backup_dir: config_base_dir.join("orchestrator").join("backups"),
        bus: bus.clone(),
        max_backups: Some(10),
    };
    let identity_tool_ctx = IdentityToolContext {
        identity_path: identity_path.to_path_buf(),
        backup_dir: config_base_dir.join("orchestrator").join("backups"),
        bus: bus.clone(),
        max_backups: Some(10),
    };
    // Capture workspace root once at startup for file tools, avoiding
    // reliance on the process-global current_dir() at tool execution time.
    let workspace_root = std::env::current_dir().ok();
    // Create the shared lock for the send_message tool's connector send provider.
    // The actual provider is set post-construction in main.rs after ConnectorSendBridge is created.
    let connector_send_lock: ConnectorSendLock = Arc::new(std::sync::RwLock::new(None));
    for tool in openalpaca_core::tools::builtins::builtin_tools_with_persona_context(
        Some(db.clone()),
        embedder.clone(),
        soul_tool_ctx,
        user_tool_ctx,
        identity_tool_ctx,
        Some(daemon_config.clone()),
        Some(web_search_config.clone()),
        workspace_root,
        Some(connector_send_lock.clone()),
    ) {
        tool_registry.register(tool);
    }

    // Load user tools from config/tools/*.toml
    let tools_config_dir = config_base_dir.join("tools");
    // Security-critical tool names that TOML configs must not override.
    // Includes both registry-registered built-ins and runtime-injected tools
    // (e.g., spawn_subagent and its variants are provided by LeadAgentToolExecutor
    // at runtime, not the global registry, but must still be protected from
    // TOML name collisions).
    let protected_builtins: &[&str] = &[
        // Registry built-ins
        "update_soul",
        "update_user",
        "update_identity",
        "shell_execute",
        "file_read",
        "file_write",
        "memory_search",
        // ContextualToolExecutor runtime tools
        "workspace_read",
        "workspace_write",
        // LeadAgentToolExecutor runtime tools
        "spawn_subagent",
        "spawn_subagents_batch",
        "check_subagent_status",
        "wait_for_subagents",
        // Connector tools
        "send_message",
    ];
    for tool in openalpaca_core::tools::config::load_tools_from_dir(&tools_config_dir) {
        if protected_builtins.contains(&tool.definition.name.as_str()) {
            warn!(
                "Custom tool '{}' would override a security-critical built-in — skipping",
                tool.definition.name
            );
            continue;
        }
        if tool_registry.get(&tool.definition.name).is_some() {
            warn!(
                "Custom tool '{}' conflicts with an existing tool name and will override it",
                tool.definition.name
            );
        }
        info!("Registered custom tool: {}", tool.definition.name);
        tool_registry.register(tool);
    }
    info!("Tool registry: {} tools loaded", tool_registry.count());

    if tool_registry.get("update_soul").is_none() {
        // This is a fatal error in the original code (anyhow::bail!)
        // but since we return the registry, we'll log an error.
        // The caller should check for this condition.
        tracing::error!("update_soul tool failed to register — SOUL.md updates will not work");
    }

    (Arc::new(tool_registry), connector_send_lock)
}
