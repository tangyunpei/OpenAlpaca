use crate::bootstrap;
use crate::events::EventBroadcaster;
use arc_swap::ArcSwap;
use openalpaca_core::{
    bus::EventBus, daemon_config::load_daemon_config, orchestrator::Orchestrator,
    tools::extensions::ExtensionSupervisor,
};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{Mutex, broadcast, mpsc};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// Bounded ring buffer of recent content hashes for dedup between agent writes and file watcher.
pub type RecentHashes = Arc<Mutex<VecDeque<String>>>;

/// Create a new bounded hash ring for dedup.
pub fn new_recent_hashes() -> RecentHashes {
    Arc::new(Mutex::new(VecDeque::with_capacity(8)))
}

/// All context needed by the file watcher task.
pub struct FileWatcherContext {
    pub soul_path: PathBuf,
    pub user_path: PathBuf,
    pub identity_path: PathBuf,
    pub bootstrap_path: Option<PathBuf>,
    pub llm_config_path: PathBuf,
    pub daemon_config_path: PathBuf,
    pub mcp_config_path: PathBuf,
    pub skills_dir: PathBuf,
    pub agents_dir: PathBuf,

    pub orchestrator: Arc<Orchestrator>,
    pub agent_registry: Arc<openalpaca_core::agent::registry::AgentRegistry>,
    pub llm_router: Option<Arc<openalpaca_llm::LlmRouter>>,
    pub secret_store: Arc<dyn openalpaca_llm::SecretStore>,
    pub skill_catalog: Arc<openalpaca_core::orchestrator::skill_catalog::SkillCatalog>,
    /// The ENABLE axis's read side for the cron skip: a scheduled skill whose
    /// requirement is wholly withheld is skipped, not fired (design §6.2 #13).
    pub tool_registry: Arc<openalpaca_core::tools::ToolRegistry>,
    pub daemon_config: Arc<ArcSwap<openalpaca_core::daemon_config::DaemonConfig>>,
    pub web_search_config: Arc<ArcSwap<openalpaca_llm::WebSearchConfig>>,
    pub bus: EventBus,
    /// The MCP half of the ENABLE axis. Edge case 15's reload arm calls its
    /// `reconcile_all()`: `mcp.toml` **is** the store, so a hand edit is
    /// authoritative and there is no precedence rule to surprise anyone.
    pub mcp_supervisor: Arc<crate::managers::mcp::McpSupervisor>,
    pub fs_watch_handle: Option<openalpaca_wake::FileWatchHandle>,

    /// Gateway for injecting scheduled-skill turns (WakeEvent::Timer).
    pub gateway: Arc<openalpaca_core::gateway::Gateway>,
    /// Wake manager for re-syncing skill cron jobs on hot-reload.
    pub wake_manager: Arc<openalpaca_wake::WakeManager>,
    /// Local user id — scheduled-skill turns run as this principal.
    pub local_user_id: String,

    pub soul_hashes: RecentHashes,
    pub user_hashes: RecentHashes,
    pub identity_hashes: RecentHashes,
    pub llm_hashes: RecentHashes,
}

/// Spawn the file watcher task that handles hot-reloading of config files.
pub fn spawn_file_watcher(
    ctx: FileWatcherContext,
    mut wake_rx: mpsc::Receiver<openalpaca_api::events::WakeEvent>,
    eb: EventBroadcaster,
    cancel: CancellationToken,
) {
    tokio::spawn(async move {
        loop {
            let event = tokio::select! {
                Some(ev) = wake_rx.recv() => ev,
                _ = cancel.cancelled() => break,
            };
            info!("Received WakeEvent: {:?}", event);

            // Cron timer fires for scheduled skills (job id "skill:{id}").
            if let openalpaca_api::events::WakeEvent::Timer { job_id, .. } = &event
                && let Some(skill_id) = crate::scheduled_skills::parse_skill_job_id(job_id)
            {
                if scheduled_skills_enabled(&ctx) {
                    crate::scheduled_skills::spawn_timer_turn(
                        ctx.gateway.clone(),
                        ctx.skill_catalog.clone(),
                        ctx.tool_registry.clone(),
                        ctx.local_user_id.clone(),
                        skill_id.to_string(),
                    );
                } else {
                    info!(
                        skill = %skill_id,
                        "Ignoring scheduled-skill timer (scheduled_skills_enabled=false)"
                    );
                }
            }

            if let openalpaca_api::events::WakeEvent::FileChanged { path, .. } = &event {
                let changed_path = PathBuf::from(path);

                // SOUL.md file watcher
                if bootstrap::is_same_file_path(&changed_path, &ctx.soul_path) {
                    handle_soul_change(&ctx).await;
                }

                // USER.md file watcher
                if bootstrap::is_same_file_path(&changed_path, &ctx.user_path) {
                    handle_user_change(&ctx).await;
                }

                // IDENTITY.md file watcher
                if bootstrap::is_same_file_path(&changed_path, &ctx.identity_path) {
                    handle_identity_change(&ctx).await;
                }

                // BOOTSTRAP.md file watcher
                if let Some(ref bp) = ctx.bootstrap_path
                    && bootstrap::is_same_file_path(&changed_path, bp)
                {
                    handle_bootstrap_change(&ctx, bp).await;
                }

                // LLM config (llm.toml) hot-reload
                if bootstrap::is_same_file_path(&changed_path, &ctx.llm_config_path) {
                    handle_llm_config_change(&ctx).await;
                }

                // Daemon config (daemon.toml) hot-reload
                if bootstrap::is_same_file_path(&changed_path, &ctx.daemon_config_path) {
                    let mut new_cfg = load_daemon_config(&ctx.daemon_config_path);
                    new_cfg.validate();
                    ctx.daemon_config.store(Arc::new(new_cfg));
                    info!(
                        "Daemon config hot-reloaded from {}",
                        ctx.daemon_config_path.display()
                    );
                    // Re-sync skill cron jobs — picks up scheduled_skills_enabled
                    // toggles (registers all / deregisters all).
                    crate::scheduled_skills::sync_all(
                        &ctx.wake_manager,
                        &ctx.skill_catalog,
                        scheduled_skills_enabled(&ctx),
                    )
                    .await;
                }

                // MCP declaration + toggle store (mcp.toml) — edge case 15
                if bootstrap::is_same_file_path(&changed_path, &ctx.mcp_config_path) {
                    handle_mcp_config_change(&ctx).await;
                }

                // Skills directory hot-reload
                if changed_path.starts_with(&ctx.skills_dir) {
                    handle_skills_change(&ctx, &changed_path).await;
                }

                // Agents directory hot-reload
                if changed_path.starts_with(&ctx.agents_dir) {
                    handle_agents_change(&ctx, &changed_path).await;
                }
            }

            eb.wake(event);
        }
    });
}

/// Current value of the scheduled-skills kill switch (hot-reloadable).
fn scheduled_skills_enabled(ctx: &FileWatcherContext) -> bool {
    ctx.daemon_config
        .load()
        .orchestrator
        .routing
        .scheduled_skills_enabled
}

async fn handle_soul_change(ctx: &FileWatcherContext) {
    let should_skip = if let Ok(content) = std::fs::read(&ctx.soul_path) {
        use sha2::{Digest, Sha256};
        let file_hash = format!("{:x}", Sha256::digest(&content));
        let mut ring = ctx.soul_hashes.lock().await;
        if let Some(pos) = ring.iter().position(|h| *h == file_hash) {
            ring.remove(pos);
            info!(
                "Watcher dedup: skipping reload for hash {} (already applied via EventBus)",
                &file_hash[..16]
            );
            true
        } else {
            false
        }
    } else {
        false
    };

    if !should_skip {
        match bootstrap::load_system_persona_from_soul_file(&ctx.soul_path) {
            Ok(persona) => {
                ctx.orchestrator.update_system_persona(persona);
                info!("Soul reloaded (watcher): {}", ctx.soul_path.display());
            }
            Err(e) => {
                warn!(
                    "SOUL parse/validation failed for {}: {e}; keeping last active soul",
                    ctx.soul_path.display()
                );
            }
        }
    }
}

async fn handle_user_change(ctx: &FileWatcherContext) {
    let should_skip = if let Ok(content) = std::fs::read(&ctx.user_path) {
        use sha2::{Digest, Sha256};
        let file_hash = format!("{:x}", Sha256::digest(&content));
        let mut ring = ctx.user_hashes.lock().await;
        if let Some(pos) = ring.iter().position(|h| *h == file_hash) {
            ring.remove(pos);
            info!(
                "Watcher dedup: skipping USER reload for hash {} (already applied via EventBus)",
                &file_hash[..16]
            );
            true
        } else {
            false
        }
    } else {
        false
    };

    if !should_skip {
        match bootstrap::load_user_document_from_file(&ctx.user_path) {
            Ok(doc) => {
                ctx.orchestrator.update_user_document(Some(doc));
                info!(
                    "User profile reloaded (watcher): {}",
                    ctx.user_path.display()
                );
            }
            Err(e) => {
                warn!(
                    "USER parse/validation failed for {}: {e}; keeping last active profile",
                    ctx.user_path.display()
                );
            }
        }
    }
}

async fn handle_identity_change(ctx: &FileWatcherContext) {
    let should_skip = if let Ok(content) = std::fs::read(&ctx.identity_path) {
        use sha2::{Digest, Sha256};
        let file_hash = format!("{:x}", Sha256::digest(&content));
        let mut ring = ctx.identity_hashes.lock().await;
        if let Some(pos) = ring.iter().position(|h| *h == file_hash) {
            ring.remove(pos);
            info!(
                "Watcher dedup: skipping IDENTITY reload for hash {} (already applied via EventBus)",
                &file_hash[..16]
            );
            true
        } else {
            false
        }
    } else {
        false
    };

    if !should_skip {
        match bootstrap::load_identity_document_from_file(&ctx.identity_path) {
            Ok(doc) => {
                ctx.orchestrator.update_identity_document(Some(doc));
                info!(
                    "Identity reloaded (watcher): {}",
                    ctx.identity_path.display()
                );
            }
            Err(e) => {
                warn!(
                    "IDENTITY parse/validation failed for {}: {e}; keeping last active identity",
                    ctx.identity_path.display()
                );
            }
        }
    }
}

async fn handle_bootstrap_change(ctx: &FileWatcherContext, bp: &Path) {
    if !bp.exists() {
        // File was deleted (by agent completion or manual user action)
        ctx.orchestrator.update_bootstrap_document(None);
        info!(
            "Bootstrap document cleared (file deleted): {}",
            bp.display()
        );
        // Stop polling the deleted path to avoid log spam
        if let Some(ref handle) = ctx.fs_watch_handle
            && let Err(e) = handle.unwatch_path(bp)
        {
            warn!("Failed to unwatch bootstrap path: {e}");
        }
    } else {
        // File was modified — reload
        match bootstrap::load_bootstrap_document_from_file(bp) {
            Ok(doc) => {
                ctx.orchestrator.update_bootstrap_document(Some(doc));
                info!("Bootstrap reloaded (watcher): {}", bp.display());
            }
            Err(e) => {
                warn!(
                    "BOOTSTRAP parse failed for {}: {e}; keeping last state",
                    bp.display()
                );
            }
        }
    }
}

async fn handle_llm_config_change(ctx: &FileWatcherContext) {
    // Dedup: skip if this write was from settings_service
    let should_skip = if let Ok(content) = std::fs::read(&ctx.llm_config_path) {
        use sha2::{Digest, Sha256};
        let hash = format!("{:x}", Sha256::digest(&content));
        let hashes = ctx.llm_hashes.lock().await;
        hashes.contains(&hash)
    } else {
        false
    };

    if !should_skip {
        if let Some(ref router) = ctx.llm_router {
            match openalpaca_llm::read_config(&ctx.llm_config_path) {
                Ok(new_config) => {
                    // 1. Reload runtime config (timeouts, endpoints, env vars, provider defaults)
                    let runtime = openalpaca_llm::LlmRuntimeConfig::from(&new_config);
                    router.reload_runtime_config(runtime);

                    // 2. Reload model registry entries from config
                    if let Some(ref models) = new_config.models {
                        router.model_registry().reload_from_config(models);
                    }

                    // 3. Reload default model
                    if let Some(ref orch) = new_config.orchestrator {
                        router.set_default_model(orch.model.clone());
                    }

                    // 4. Reload key pools for each configured provider
                    if let Some(ref providers) = new_config.providers {
                        for (provider_name, provider_config) in providers {
                            if provider_config.enabled == Some(false) {
                                continue;
                            }
                            if let Some(provider_type) =
                                openalpaca_llm::config::parse_provider_type_pub(provider_name)
                            {
                                match openalpaca_llm::config::settings_service::build_key_pool_from_provider_config(
                                    provider_config,
                                    provider_type.clone(),
                                    Some(&*ctx.secret_store),
                                ) {
                                    Ok(pool) => {
                                        router.reload_keys(&provider_type, pool);
                                    }
                                    Err(e) => {
                                        warn!(
                                            "LLM config reload: failed to rebuild key pool for {}: {}",
                                            provider_name, e
                                        );
                                    }
                                }
                            }
                        }
                    }

                    // 5. Reload web_search config
                    let ws_cfg = new_config
                        .web_search
                        .clone()
                        .unwrap_or_default();
                    ctx.web_search_config.store(Arc::new(ws_cfg));

                    info!(
                        "LLM config hot-reloaded from {}",
                        ctx.llm_config_path.display()
                    );
                }
                Err(e) => {
                    warn!(
                        "LLM config reload failed for {}: {e}; keeping current config",
                        ctx.llm_config_path.display()
                    );
                }
            }
        }
    } else {
        info!("Skipping LLM config reload (settings-service write dedup)");
    }
}

/// **Edge case 15.** A change to `config/mcp.toml`.
///
/// The file *is* the store, so a hand edit is authoritative: `reconcile_all()`
/// diffs desired against actual on **presence + `enabled` bit +
/// `config_fingerprint`** and loads or unloads only what changed. An
/// unparseable rewrite keeps the last-good desired set and skips the diff, so
/// an editor's intermediate save tears nothing down.
///
/// The daemon's **own** write produces the same filesystem event a hand edit
/// does; the supervisor's dedup ring is what swallows it — it pushed the
/// post-write hash before the rename, so the route-driven toggle runs only its
/// in-process transition. Losing the event is tolerable (filesystem events are
/// `try_send` with drop-on-full) precisely because the route path never depends
/// on the watcher.
async fn handle_mcp_config_change(ctx: &FileWatcherContext) {
    if let Ok(contents) = std::fs::read_to_string(&ctx.mcp_config_path)
        && ctx.mcp_supervisor.swallow_own_write(&contents)
    {
        info!(
            "Skipping MCP reconcile for {} (the daemon wrote it)",
            ctx.mcp_config_path.display()
        );
        return;
    }
    ctx.mcp_supervisor.reconcile_all().await;
    info!(
        "MCP servers reconciled (watcher): {}",
        ctx.mcp_config_path.display()
    );
}

async fn handle_skills_change(ctx: &FileWatcherContext, changed_path: &Path) {
    // Determine which skill folder changed
    if let Ok(relative) = changed_path.strip_prefix(&ctx.skills_dir)
        && let Some(skill_folder) = relative.components().next()
    {
        let skill_dir = ctx.skills_dir.join(skill_folder);
        match ctx.skill_catalog.reload_skill(&skill_dir) {
            Ok(()) => {
                let skill_name = skill_folder.as_os_str().to_string_lossy().to_string();
                info!("Skill hot-reloaded: {}", skill_dir.display());
                // Re-sync the skill's cron job (catalog keys are lowercased
                // directory names). Handles add/change/removal of invoke.cron
                // and skill deletion alike.
                crate::scheduled_skills::resync_skill(
                    &ctx.wake_manager,
                    &ctx.skill_catalog,
                    &skill_name.to_lowercase(),
                    scheduled_skills_enabled(ctx),
                )
                .await;
                ctx.bus
                    .publish(openalpaca_core::events::SystemEvent::SkillCatalogUpdated {
                        skill_name,
                        action: "reloaded".to_string(),
                        timestamp: chrono::Utc::now(),
                    });
            }
            Err(e) => {
                warn!("Skill reload failed for {}: {}", skill_dir.display(), e)
            }
        }
    }
}

async fn handle_agents_change(ctx: &FileWatcherContext, changed_path: &Path) {
    // Only handle .md files (legacy .toml handled at startup only)
    if changed_path.extension().and_then(|e| e.to_str()) != Some("md") {
        return;
    }
    match std::fs::read_to_string(changed_path) {
        Ok(content) => {
            match openalpaca_core::agent::template::parse_agent_markdown(&content) {
                Ok(template) => {
                    let template_id = template.frontmatter.id.clone();

                    // Upsert template: remove_template() first because register_template()
                    // is insert-only and silently no-ops if the key already exists.
                    ctx.agent_registry.remove_template(&template_id);
                    ctx.agent_registry.register_template(template.clone());

                    // Also upsert the singleton agent instance — but only if Idle.
                    // If the agent is currently Busy running a task, leave it alone;
                    // it will pick up the new template on its next spawn.
                    //
                    // We use get_with_version() + update_config() (optimistic locking)
                    // to avoid a TOCTOU race where the dispatcher claims the agent
                    // between our status check and the replacement.
                    //
                    // Note: to_subagent() always constructs with AgentStatus::Busy;
                    // we override to Idle + clear current_task since this is a
                    // config reload, not a task dispatch.
                    if let Some((existing, version)) =
                        ctx.agent_registry.get_with_version(&template_id)
                    {
                        if existing.status.is_available() {
                            let mut fresh = template.to_subagent(&template_id, "");
                            fresh.status = openalpaca_core::agent::AgentStatus::Idle;
                            fresh.current_task = None;
                            if let Err(e) =
                                ctx.agent_registry
                                    .update_config(&template_id, fresh, version)
                            {
                                warn!(
                                    "Agent instance update skipped (concurrent claim): {e}"
                                );
                            }
                        }
                    } else {
                        // No instance yet — safe to register a new one
                        let mut idle_agent = template.to_subagent(&template_id, "");
                        idle_agent.status = openalpaca_core::agent::AgentStatus::Idle;
                        idle_agent.current_task = None;
                        ctx.agent_registry.register(idle_agent);
                    }

                    info!("Agent template hot-reloaded: {}", changed_path.display());
                }
                Err(e) => {
                    warn!(
                        "Agent template reload failed for {}: {e}; keeping last active template",
                        changed_path.display()
                    );
                }
            }
        }
        Err(e) => {
            warn!(
                "Failed to read agent template {}: {e}",
                changed_path.display()
            );
        }
    }
}

/// Spawn soul hot-reload subscriber via EventBus (agent-initiated updates).
///
/// When the update_persona tool writes a new SOUL file, it publishes SoulUpdated.
/// This subscriber reloads the persona immediately without waiting for the
/// file watcher, providing a more reliable activation path.
pub fn spawn_soul_reload_subscriber(
    bus: &EventBus,
    orchestrator: Arc<Orchestrator>,
    soul_path: PathBuf,
    hashes: RecentHashes,
    cancel: CancellationToken,
) {
    let mut rx = bus.subscribe();
    tokio::spawn(async move {
        loop {
            let event = tokio::select! {
                result = rx.recv() => match result {
                    Ok(ev) => ev,
                    Err(broadcast::error::RecvError::Closed) => break,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                },
                _ = cancel.cancelled() => break,
            };
            if let openalpaca_core::events::SystemEvent::SoulUpdated {
                actor,
                content_sha256,
                ..
            } = event
            {
                info!(
                    "SoulUpdated via EventBus (actor={}, sha256={}), reloading persona",
                    actor,
                    &content_sha256[..16.min(content_sha256.len())]
                );
                match bootstrap::load_system_persona_from_soul_file(&soul_path) {
                    Ok(persona) => {
                        orchestrator.update_system_persona(persona);

                        // Record this hash so the file watcher won't double-reload
                        let mut ring = hashes.lock().await;
                        ring.push_back(content_sha256.clone());
                        while ring.len() > 8 {
                            ring.pop_front();
                        }

                        info!("Soul hot-reloaded via EventBus: {}", soul_path.display());
                    }
                    Err(e) => {
                        warn!(
                            "Soul EventBus reload failed for {}: {e}; keeping last active soul",
                            soul_path.display()
                        );
                    }
                }
            }
        }
    });
}

/// Spawn user profile hot-reload subscriber via EventBus (agent-initiated updates).
pub fn spawn_user_reload_subscriber(
    bus: &EventBus,
    orchestrator: Arc<Orchestrator>,
    user_path: PathBuf,
    hashes: RecentHashes,
    cancel: CancellationToken,
) {
    let mut rx = bus.subscribe();
    tokio::spawn(async move {
        loop {
            let event = tokio::select! {
                result = rx.recv() => match result {
                    Ok(ev) => ev,
                    Err(broadcast::error::RecvError::Closed) => break,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                },
                _ = cancel.cancelled() => break,
            };
            if let openalpaca_core::events::SystemEvent::UserProfileUpdated {
                actor,
                content_sha256,
                ..
            } = event
            {
                info!(
                    "UserProfileUpdated via EventBus (actor={}, sha256={}), reloading profile",
                    actor,
                    &content_sha256[..16.min(content_sha256.len())]
                );
                match bootstrap::load_user_document_from_file(&user_path) {
                    Ok(doc) => {
                        orchestrator.update_user_document(Some(doc));

                        // Record this hash so the file watcher won't double-reload
                        let mut ring = hashes.lock().await;
                        ring.push_back(content_sha256.clone());
                        while ring.len() > 8 {
                            ring.pop_front();
                        }

                        info!(
                            "User profile hot-reloaded via EventBus: {}",
                            user_path.display()
                        );
                    }
                    Err(e) => {
                        warn!(
                            "User EventBus reload failed for {}: {e}; keeping last active profile",
                            user_path.display()
                        );
                    }
                }
            }
        }
    });
}

/// Spawn identity hot-reload subscriber via EventBus (agent-initiated updates).
pub fn spawn_identity_reload_subscriber(
    bus: &EventBus,
    orchestrator: Arc<Orchestrator>,
    identity_path: PathBuf,
    hashes: RecentHashes,
    cancel: CancellationToken,
) {
    let mut rx = bus.subscribe();
    tokio::spawn(async move {
        loop {
            let event = tokio::select! {
                result = rx.recv() => match result {
                    Ok(ev) => ev,
                    Err(broadcast::error::RecvError::Closed) => break,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                },
                _ = cancel.cancelled() => break,
            };
            if let openalpaca_core::events::SystemEvent::IdentityUpdated {
                actor,
                content_sha256,
                ..
            } = event
            {
                info!(
                    "IdentityUpdated via EventBus (actor={}, sha256={}), reloading identity",
                    actor,
                    &content_sha256[..16.min(content_sha256.len())]
                );
                match bootstrap::load_identity_document_from_file(&identity_path) {
                    Ok(doc) => {
                        orchestrator.update_identity_document(Some(doc));

                        // Record this hash so the file watcher won't double-reload
                        let mut ring = hashes.lock().await;
                        ring.push_back(content_sha256.clone());
                        while ring.len() > 8 {
                            ring.pop_front();
                        }

                        info!(
                            "Identity hot-reloaded via EventBus: {}",
                            identity_path.display()
                        );
                    }
                    Err(e) => {
                        warn!(
                            "Identity EventBus reload failed for {}: {e}; keeping last active identity",
                            identity_path.display()
                        );
                    }
                }
            }
        }
    });
}
