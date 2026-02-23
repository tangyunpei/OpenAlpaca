use arc_swap::ArcSwap;
use openalpaca_core::chat::ChatStreamManager;
use openalpaca_storage::Database;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::events::EventBroadcaster;

/// Spawn background embedding indexer task.
///
/// Periodically scans for memories missing embeddings and indexes them.
/// Re-reads poll interval and batch size from ArcSwap each tick for hot-reload support.
pub fn spawn_embedding_indexer(
    embedder: Arc<dyn openalpaca_llm::Embedder>,
    db: Database,
    daemon_config: Arc<ArcSwap<openalpaca_core::daemon_config::DaemonConfig>>,
    cancel: CancellationToken,
) {
    tokio::spawn(async move {
        loop {
            let ei_cfg = daemon_config.load();
            let poll_secs = ei_cfg.server.embedding_indexer.poll_interval_secs;
            let batch_size = ei_cfg.server.embedding_indexer.batch_size;
            tokio::select! {
                _ = tokio::time::sleep(tokio::time::Duration::from_secs(poll_secs)) => {}
                _ = cancel.cancelled() => {
                    tracing::info!("Embedding indexer shutting down");
                    break;
                }
            }
            let repo = openalpaca_storage::MemoryRepository::new(&db);

            let owner_ids = match repo.list_owner_ids() {
                Ok(ids) => ids,
                Err(e) => {
                    tracing::warn!("Embedding indexer: failed to list owners: {e}");
                    continue;
                }
            };

            let mut total_count = 0usize;
            for owner_id in &owner_ids {
                let missing = match repo.list_missing_embeddings(owner_id, batch_size) {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                if missing.is_empty() {
                    continue;
                }

                let texts: Vec<&str> = missing.iter().map(|(_, c)| c.as_str()).collect();
                match embedder.embed(&texts).await {
                    Ok(embeddings) => {
                        for ((id, _), embedding) in missing.iter().zip(embeddings.iter()) {
                            if embedding.len() == embedder.dimensions() as usize {
                                if let Err(e) = repo.insert_embedding(*id, embedding) {
                                    tracing::warn!(
                                        "Failed to insert embedding for memory #{id}: {e}"
                                    );
                                }
                                total_count += 1;
                            }
                        }
                    }
                    Err(e) => tracing::warn!("Embedding batch failed for owner {owner_id}: {e}"),
                }
            }
            if total_count > 0 {
                tracing::info!("Indexed {total_count} embeddings across {} owners", owner_ids.len());
            }
        }
    });
}

/// Spawn background memory decay task.
///
/// Periodically applies importance decay and prunes low-importance memories.
pub fn spawn_memory_decay(
    db: Database,
    daemon_config: Arc<ArcSwap<openalpaca_core::daemon_config::DaemonConfig>>,
    cancel: CancellationToken,
) {
    tokio::spawn(async move {
        loop {
            let dcfg = daemon_config.load();
            let decay_cfg = &dcfg.orchestrator.memory.decay;
            let poll_secs = decay_cfg.poll_interval_secs;

            // Wait for next poll interval, or exit on shutdown
            tokio::select! {
                _ = tokio::time::sleep(tokio::time::Duration::from_secs(poll_secs)) => {}
                _ = cancel.cancelled() => {
                    tracing::info!("Memory decay task shutting down");
                    break;
                }
            }

            let half_life = dcfg.orchestrator.memory.decay.half_life_days;
            let min_importance = dcfg.orchestrator.memory.decay.min_importance;
            let soft_cap = dcfg.orchestrator.memory.decay.soft_cap;

            let repo = openalpaca_storage::MemoryRepository::new(&db);

            let owner_ids = match repo.list_owner_ids() {
                Ok(ids) => ids,
                Err(e) => {
                    tracing::warn!("Memory decay: failed to list owners: {e}");
                    continue;
                }
            };

            let mut total_decayed = 0usize;
            let mut total_pruned = 0usize;

            for owner_id in &owner_ids {
                match repo.apply_importance_decay(owner_id, half_life, min_importance) {
                    Ok(n) => total_decayed += n,
                    Err(e) => tracing::warn!("Memory decay failed for {owner_id}: {e}"),
                }
                match repo.prune_low_importance(owner_id, min_importance, soft_cap) {
                    Ok(n) => total_pruned += n,
                    Err(e) => tracing::warn!("Memory pruning failed for {owner_id}: {e}"),
                }
            }

            if total_decayed > 0 || total_pruned > 0 {
                tracing::info!(
                    "Memory lifecycle: decayed {total_decayed} memories, pruned {total_pruned}"
                );
            }
        }
    });
}

/// Spawn daemon-level heartbeat task.
///
/// Re-reads interval from ArcSwap each tick for hot-reload support.
pub fn spawn_heartbeat(
    eb: EventBroadcaster,
    daemon_config: Arc<ArcSwap<openalpaca_core::daemon_config::DaemonConfig>>,
    cancel: CancellationToken,
) {
    tokio::spawn(async move {
        loop {
            let secs = daemon_config.load().server.heartbeat_interval_secs;
            tokio::select! {
                _ = tokio::time::sleep(tokio::time::Duration::from_secs(secs)) => {}
                _ = cancel.cancelled() => {
                    tracing::info!("Heartbeat task shutting down");
                    break;
                }
            }
            eb.heartbeat();
        }
    });
}

/// Spawn chat stream cleanup task.
///
/// Re-reads interval and stale timeout from ArcSwap each tick for hot-reload support.
pub fn spawn_chat_cleanup(
    csm: Arc<ChatStreamManager>,
    daemon_config: Arc<ArcSwap<openalpaca_core::daemon_config::DaemonConfig>>,
    cancel: CancellationToken,
) {
    tokio::spawn(async move {
        loop {
            let cfg = daemon_config.load();
            let cleanup_secs = cfg.server.chat_streams.cleanup_interval_secs;
            let stale_secs = cfg.server.chat_streams.stale_timeout_secs;
            tokio::select! {
                _ = tokio::time::sleep(tokio::time::Duration::from_secs(cleanup_secs)) => {}
                _ = cancel.cancelled() => {
                    tracing::info!("Chat cleanup task shutting down");
                    break;
                }
            }
            csm.cleanup_stale(std::time::Duration::from_secs(stale_secs));
        }
    });
}
