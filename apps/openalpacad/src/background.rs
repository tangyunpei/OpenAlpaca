use arc_swap::ArcSwap;
use openalpaca_core::chat::ChatStreamManager;
use openalpaca_storage::{Database, FileAssetRepository, FileAssetStatus};
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

/// Spawn background file processing worker.
///
/// Polls for newly-uploaded file assets and extracts text content (PDF, plain text).
/// Vision-model-native formats (images) are left with empty extracted_text.
/// Failed extractions go to `Error` status and are retried up to `extraction_retry_count` times.
pub fn spawn_file_processing_worker(
    db: Database,
    daemon_config: Arc<ArcSwap<openalpaca_core::daemon_config::DaemonConfig>>,
    cancel: CancellationToken,
) {
    tokio::spawn(async move {
        loop {
            let cfg = daemon_config.load();
            let gov = &cfg.upload.governance;
            let poll_secs = gov.processing_poll_interval_secs;
            let batch_size = gov.processing_batch_size;
            let max_text_chars = gov.max_extracted_text_chars;
            let max_concurrent = gov.max_concurrent_extractions;
            let retry_count = gov.extraction_retry_count;

            tokio::select! {
                _ = tokio::time::sleep(tokio::time::Duration::from_secs(poll_secs)) => {}
                _ = cancel.cancelled() => {
                    tracing::info!("File processing worker shutting down");
                    break;
                }
            }

            let repo = FileAssetRepository::new(&db);

            // Collect assets to process: new uploads + retryable errors
            let mut assets = match repo.list_by_status(&FileAssetStatus::Uploaded, batch_size) {
                Ok(a) => a,
                Err(e) => {
                    tracing::warn!("File processing worker: failed to list assets: {e}");
                    continue;
                }
            };

            // Also pick up Error-state assets eligible for retry
            if retry_count > 0 && assets.len() < batch_size {
                let remaining = batch_size - assets.len();
                if let Ok(error_assets) = repo.list_by_status(&FileAssetStatus::Error, remaining) {
                    for ea in error_assets {
                        let attempts = parse_retry_count(ea.extract_error.as_deref());
                        if attempts <= retry_count {
                            assets.push(ea);
                        }
                    }
                }
            }

            if assets.is_empty() {
                continue;
            }

            let semaphore = Arc::new(tokio::sync::Semaphore::new(max_concurrent));
            let mut handles = Vec::new();

            for asset in assets {
                let sem = semaphore.clone();
                let db2 = db.clone();
                let max_text = max_text_chars;

                handles.push(tokio::spawn(async move {
                    let _permit = sem.acquire().await.expect("semaphore closed");
                    let repo = FileAssetRepository::new(&db2);

                    if let Err(e) =
                        repo.update_status(&asset.id, &FileAssetStatus::Processing, None, None)
                    {
                        tracing::warn!("Failed to mark asset {} as processing: {e}", asset.id);
                        return;
                    }

                    let prev_attempts = parse_retry_count(asset.extract_error.as_deref());
                    let storage_path = asset.storage_path.clone();
                    let mime_type = asset.mime_type.clone();
                    let (text, error) = tokio::task::spawn_blocking(move || {
                        extract_text(&storage_path, &mime_type, max_text)
                    })
                    .await
                    .unwrap_or((None, Some("Extraction task panicked".to_string())));

                    let final_status = if error.is_some() {
                        FileAssetStatus::Error
                    } else {
                        FileAssetStatus::Ready
                    };
                    let annotated_error =
                        error.map(|e| format!("[attempt:{}] {}", prev_attempts + 1, e));
                    if let Err(e) = repo.update_status(
                        &asset.id,
                        &final_status,
                        text.as_deref(),
                        annotated_error.as_deref(),
                    ) {
                        tracing::warn!("Failed to update asset {} status: {e}", asset.id);
                    }
                }));
            }

            let results = futures_util::future::join_all(handles).await;
            let processed = results.len();
            if processed > 0 {
                tracing::info!("File processing worker: processed {processed} assets");
            }
        }
    });
}

/// Parse the retry attempt count from an annotated error string like "[attempt:2] PDF extraction failed".
fn parse_retry_count(error: Option<&str>) -> u32 {
    error
        .and_then(|e| e.strip_prefix("[attempt:"))
        .and_then(|e| e.split(']').next())
        .and_then(|n| n.parse::<u32>().ok())
        .unwrap_or(0)
}

/// Extract text content from a file based on its MIME type.
fn extract_text(
    storage_path: &str,
    mime_type: &str,
    max_chars: usize,
) -> (Option<String>, Option<String>) {
    if mime_type.starts_with("text/") {
        match std::fs::read_to_string(storage_path) {
            Ok(text) => {
                let truncated = if text.len() > max_chars {
                    text[..text.floor_char_boundary(max_chars)].to_string()
                } else {
                    text
                };
                (Some(truncated), None)
            }
            Err(e) => (None, Some(format!("Failed to read text file: {e}"))),
        }
    } else if mime_type == "application/pdf" {
        match pdf_extract::extract_text(storage_path) {
            Ok(text) => {
                let truncated = if text.len() > max_chars {
                    text[..text.floor_char_boundary(max_chars)].to_string()
                } else {
                    text
                };
                (Some(truncated), None)
            }
            Err(e) => (None, Some(format!("PDF extraction failed: {e}"))),
        }
    } else if mime_type.starts_with("image/") || mime_type.starts_with("audio/") {
        // Images/audio handled natively by vision/audio models — no text extraction
        (Some(String::new()), None)
    } else {
        // Unknown type — mark as ready with empty text
        (Some(String::new()), None)
    }
}

/// Spawn background asset cleanup task.
///
/// Periodically deletes orphaned file assets (not linked to any message)
/// that are older than the configured grace period.
pub fn spawn_asset_cleanup(
    db: Database,
    daemon_config: Arc<ArcSwap<openalpaca_core::daemon_config::DaemonConfig>>,
    cancel: CancellationToken,
) {
    tokio::spawn(async move {
        loop {
            let cfg = daemon_config.load();
            let interval_secs = cfg.upload.governance.cleanup_interval_hours * 3600;
            let grace_hours = cfg.upload.governance.orphan_grace_period_hours as i64;

            tokio::select! {
                _ = tokio::time::sleep(tokio::time::Duration::from_secs(interval_secs)) => {}
                _ = cancel.cancelled() => {
                    tracing::info!("Asset cleanup task shutting down");
                    break;
                }
            }

            let repo = FileAssetRepository::new(&db);
            let orphans = match repo.list_orphaned(grace_hours) {
                Ok(o) => o,
                Err(e) => {
                    tracing::warn!("Asset cleanup: failed to list orphans: {e}");
                    continue;
                }
            };

            let mut cleaned = 0usize;
            for orphan in &orphans {
                // Delete file from disk
                if let Err(e) = std::fs::remove_file(&orphan.storage_path) {
                    // File may already be gone — just warn
                    tracing::debug!("Failed to remove orphan file {}: {e}", orphan.storage_path);
                }
                // Delete DB row
                if let Err(e) = repo.delete_by_id(&orphan.id) {
                    tracing::warn!("Failed to delete orphan asset {}: {e}", orphan.id);
                } else {
                    cleaned += 1;
                }
            }

            if cleaned > 0 {
                tracing::info!("Asset cleanup: removed {cleaned} orphaned assets");
            }
        }
    });
}
