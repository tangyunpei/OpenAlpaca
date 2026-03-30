//! LLM router, secret store, embedder, token manager, and cost tracker initialization.

use openalpaca_llm::CostSnapshot;
use openalpaca_storage::Database;
use std::path::Path;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

pub(super) fn initialize_secret_store(
    llm_config_path: &Path,
) -> (Arc<dyn openalpaca_llm::SecretStore>, bool) {
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

pub(super) fn build_llm_router(
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

pub(super) async fn build_llm_settings_service(
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

pub(super) fn build_embedder(
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

pub(super) async fn build_token_manager(
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

/// Restore CostTracker state from today's persisted `llm_usage_daily` rows.
///
/// Called at daemon startup so budget enforcement is accurate across restarts.
pub async fn restore_cost_tracker(
    router: &openalpaca_llm::LlmRouter,
    db: &Database,
) {
    let repo = openalpaca_storage::LlmUsageRepository::new(db);
    let rows = match repo.get_today_usage() {
        Ok(r) => r,
        Err(e) => {
            warn!("Failed to load today's LLM usage from DB: {e}");
            return;
        }
    };
    if rows.is_empty() {
        return;
    }

    let mut snapshot = CostSnapshot::default();
    for row in &rows {
        let stats = snapshot
            .agent_usage
            .entry(row.agent_id.clone())
            .or_default();
        stats.total_requests += row.total_requests as u64;
        stats.total_input_tokens += row.total_input_tokens as u64;
        stats.total_output_tokens += row.total_output_tokens as u64;
        stats.total_cost_usd += row.total_cost_usd;

        let model_stats = stats
            .by_model
            .entry(row.model.clone())
            .or_default();
        model_stats.requests += row.total_requests as u64;
        model_stats.input_tokens += row.total_input_tokens as u64;
        model_stats.output_tokens += row.total_output_tokens as u64;
        model_stats.cost_usd += row.total_cost_usd;

        // Resolve model → provider for provider_usage
        let provider_name = router
            .model_registry()
            .resolve_provider_name(&row.model)
            .unwrap_or_else(|| "unknown".to_string());

        let pstats = snapshot
            .provider_usage
            .entry(provider_name)
            .or_default();
        pstats.total_requests += row.total_requests as u64;
        pstats.total_input_tokens += row.total_input_tokens as u64;
        pstats.total_output_tokens += row.total_output_tokens as u64;
        pstats.total_cost_usd += row.total_cost_usd;

        let pmodel_stats = pstats.by_model.entry(row.model.clone()).or_default();
        pmodel_stats.requests += row.total_requests as u64;
        pmodel_stats.input_tokens += row.total_input_tokens as u64;
        pmodel_stats.output_tokens += row.total_output_tokens as u64;
        pmodel_stats.cost_usd += row.total_cost_usd;
    }

    let total_cost: f64 = rows.iter().map(|r| r.total_cost_usd).sum();
    router.cost_tracker.load_snapshot(snapshot).await;
    info!(
        "Restored CostTracker from DB: {} row(s), ${:.4} total",
        rows.len(),
        total_cost
    );
}

/// Flush CostTracker state to DB on shutdown (defense-in-depth).
///
/// Uses REPLACE semantics since CostTracker holds cumulative totals
/// that include data already persisted by per-call `record_and_log()`.
/// Skips flush if the date has changed since startup (midnight crossing)
/// to avoid corrupting the new day's per-call data with stale cumulative totals.
pub async fn flush_cost_tracker(
    router: &openalpaca_llm::LlmRouter,
    db: &Database,
    startup_date: &str,
) {
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    if today != startup_date {
        info!(
            "Skipping CostTracker flush: date changed ({startup_date} → {today})"
        );
        return;
    }

    let snapshot = router.cost_tracker.snapshot_for_flush().await;
    let repo = openalpaca_storage::LlmUsageRepository::new(db);

    let mut flushed = 0usize;
    for (agent_id, stats) in &snapshot.agent_usage {
        for (model, model_stats) in &stats.by_model {
            let daily = openalpaca_storage::LlmUsageDaily {
                date: today.clone(),
                agent_id: agent_id.clone(),
                model: model.clone(),
                total_requests: model_stats.requests as i32,
                total_input_tokens: model_stats.input_tokens as i64,
                total_output_tokens: model_stats.output_tokens as i64,
                total_cost_usd: model_stats.cost_usd,
            };
            if let Err(e) = repo.replace_daily_usage(&daily) {
                warn!("Failed to flush usage for {agent_id}/{model}: {e}");
            } else {
                flushed += 1;
            }
        }
    }
    info!("Flushed CostTracker to DB: {flushed} row(s)");
}
