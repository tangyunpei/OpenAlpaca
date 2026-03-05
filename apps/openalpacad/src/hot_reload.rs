use crate::bootstrap;
use crate::events::EventBroadcaster;
use arc_swap::ArcSwap;
use openalpaca_core::{
    bus::EventBus, daemon_config::load_daemon_config, orchestrator::Orchestrator,
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
    pub skills_dir: PathBuf,

    pub orchestrator: Arc<Orchestrator>,
    pub llm_router: Option<Arc<openalpaca_llm::LlmRouter>>,
    pub secret_store: Arc<dyn openalpaca_llm::SecretStore>,
    pub skill_catalog: Arc<openalpaca_core::orchestrator::skill_catalog::SkillCatalog>,
    pub daemon_config: Arc<ArcSwap<openalpaca_core::daemon_config::DaemonConfig>>,
    pub web_search_config: Arc<ArcSwap<openalpaca_llm::WebSearchConfig>>,
    pub bus: EventBus,
    pub fs_watch_handle: Option<openalpaca_wake::FileWatchHandle>,

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
                }

                // Skills directory hot-reload
                if changed_path.starts_with(&ctx.skills_dir) {
                    handle_skills_change(&ctx, &changed_path).await;
                }
            }

            eb.wake(event);
        }
    });
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
                                match openalpaca_llm::settings_service::build_key_pool_from_provider_config(
                                    provider_config,
                                    provider_type,
                                    Some(&*ctx.secret_store),
                                ) {
                                    Ok(pool) => {
                                        router.reload_keys(provider_type, pool);
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
