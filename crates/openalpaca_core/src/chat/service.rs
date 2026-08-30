//! ChatService — Core chat logic decoupled from route handlers
//!
//! Orchestrates gateway calls, stream management, and message persistence.
//! Simulates progressive token streaming by chunking the complete LLM response
//! and emitting `Delta` events with a configurable delay.

use crate::bus::EventBus;
use crate::chat::stream_manager::{ChatStreamManager, chunk_by_words};
use crate::daemon_config::DaemonConfig;
use crate::events::SystemEvent;
use crate::gateway::{Gateway, GatewayRequest};
use crate::security::policy::{Principal, Scope};
use anyhow::Result;
use arc_swap::ArcSwap;
use chrono::Utc;
use openalpaca_api::events::EventSource;
use openalpaca_storage::{
    AttachmentRef, ConversationMessage, ConversationRepository, Database, FileAsset,
    FileAssetRepository, FileAssetStatus,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{info, warn};

/// Response returned after sending a chat message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatSendResponse {
    pub stream_id: String,
    pub lane_key: String,
}

/// Core chat service that manages conversations via the Gateway.
pub struct ChatService {
    gateway: Arc<Gateway>,
    stream_manager: Arc<ChatStreamManager>,
    db: Database,
    bus: EventBus,
    daemon_config: Arc<ArcSwap<DaemonConfig>>,
}

impl ChatService {
    pub fn new(
        gateway: Arc<Gateway>,
        stream_manager: Arc<ChatStreamManager>,
        db: Database,
        bus: EventBus,
        daemon_config: Arc<ArcSwap<DaemonConfig>>,
    ) -> Self {
        Self {
            gateway,
            stream_manager,
            db,
            bus,
            daemon_config,
        }
    }

    /// Send a message and start streaming the response.
    ///
    /// Returns immediately with a stream_id. The actual LLM call happens
    /// in a background task that sends events to the stream.
    ///
    /// Event sequence (client-visible):
    /// 1. `Thinking` — emitted AFTER 100ms sleep so the client has time to subscribe
    /// 2. `Delta { content }` × N — word-chunked pieces of the full response
    /// 3. `Done { content, model, tokens_in, tokens_out, duration_ms }` — full text + metadata
    ///
    /// On error: `Thinking` → `Error { message }`.
    pub fn send_message(
        &self,
        content: String,
        attachment_refs: Vec<AttachmentRef>,
        principal: &str,
        workspace_path: Option<String>,
    ) -> Result<ChatSendResponse> {
        // Fast preflight check so invalid attachment IDs still fail the request immediately.
        let file_repo = FileAssetRepository::new(&self.db);
        for att_ref in &attachment_refs {
            match file_repo.get_by_id(&att_ref.file_id) {
                Ok(Some(asset)) => {
                    if asset.owner_id != principal {
                        anyhow::bail!("Access denied to attachment: {}", att_ref.file_id);
                    }
                }
                Ok(None) => {
                    anyhow::bail!("Attachment not found: {}", att_ref.file_id);
                }
                Err(e) => {
                    anyhow::bail!("Failed to resolve attachment {}: {}", att_ref.file_id, e);
                }
            }
        }

        let lane_key = format!("{principal}:gui");

        let (stream_id, _rx, sink) = self.stream_manager.create_stream(&lane_key);

        // Spawn background task for the actual gateway call
        let gateway = self.gateway.clone();
        let stream_manager = self.stream_manager.clone();
        let sid = stream_id.clone();
        let user_content = content.clone();
        let principal_owned = principal.to_string();
        let attachment_refs_owned = attachment_refs.clone();
        let bus = self.bus.clone();
        let daemon_config = self.daemon_config.clone();
        let db = self.db.clone();
        let lk = lane_key.clone();

        tokio::spawn(async move {
            // Give browser time to connect to SSE endpoint
            tokio::time::sleep(Duration::from_millis(100)).await;

            // Emit Thinking AFTER sleep — client has subscribed by now
            sink.send_thinking();

            let start = Instant::now();

            let upload_governance = daemon_config.load().upload.governance.clone();
            let attachments = match Self::resolve_attachments_with_wait(
                db,
                &attachment_refs_owned,
                &principal_owned,
                upload_governance.attachment_ready_wait_ms,
                upload_governance.attachment_ready_poll_interval_ms,
            )
            .await
            {
                Ok(v) => v,
                Err(e) => {
                    sink.send_error(&format!("Failed to resolve attachments: {e}"));
                    let _ = bus.publish(SystemEvent::ChatStreamEnded {
                        stream_id: sid.clone(),
                        lane_key: lk.clone(),
                        status: "error".to_string(),
                        timestamp: Utc::now(),
                    });
                    info!("Chat stream {sid} failed while resolving attachments");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    stream_manager.remove(&sid);
                    return;
                }
            };

            let response = gateway
                .handle_event(GatewayRequest {
                    source: EventSource::Gui {
                        connection_id: principal_owned.clone(),
                    },
                    content: user_content.clone(),
                    attachments,
                    principal: Principal::User {
                        global_id: principal_owned.clone(),
                    },
                    scope: Scope::Global,
                    workspace_path,
                    stream_id: Some(sid.clone()),
                })
                .await;

            let duration_ms = start.elapsed().as_millis() as u64;

            // Note: Message persistence is now handled by Gateway (GatewayPersistence).
            // ChatService only manages the SSE stream events.

            if response.is_error {
                sink.send_error(&response.content);
            } else {
                // Emit delta chunks (simulated progressive streaming)
                let cfg = daemon_config.load();
                let delay_ms = cfg.server.chat_streams.stream_chunk_delay_ms;
                let chunk_words = cfg.server.chat_streams.stream_chunk_words;

                let chunks = chunk_by_words(&response.content, chunk_words);
                for chunk in &chunks {
                    sink.send_delta(chunk);
                    if delay_ms > 0 {
                        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    }
                }

                // Send Done with real metadata
                let model = response.model.as_deref().unwrap_or("default");
                let tokens_in = response.tokens_in.unwrap_or(0) as u64;
                let tokens_out = response.tokens_out.unwrap_or(0) as u64;
                let delegation = response.delegation.clone();
                if response.attachments_used.is_empty() {
                    sink.send_done(
                        &response.content,
                        model,
                        tokens_in,
                        tokens_out,
                        duration_ms,
                        delegation,
                    );
                } else {
                    sink.send_done_with_attachments(
                        &response.content,
                        model,
                        tokens_in,
                        tokens_out,
                        duration_ms,
                        response.attachments_used,
                        delegation,
                    );
                }
            }

            // Emit ChatStreamEnded event
            let status = if response.is_error {
                "error"
            } else {
                "completed"
            };
            let _ = bus.publish(SystemEvent::ChatStreamEnded {
                stream_id: sid.clone(),
                lane_key: lk,
                status: status.to_string(),
                timestamp: Utc::now(),
            });

            info!("Chat stream {sid} completed in {duration_ms}ms");

            // Delay removal to allow late SSE subscribers
            tokio::time::sleep(Duration::from_secs(5)).await;
            stream_manager.remove(&sid);
        });

        Ok(ChatSendResponse {
            stream_id,
            lane_key,
        })
    }

    /// Get conversation history for a lane.
    pub fn get_history(
        &self,
        lane_key: &str,
        limit: i64,
        offset: i64,
    ) -> Result<(Vec<ConversationMessage>, i64)> {
        let repo = ConversationRepository::new(&self.db);
        let messages = repo.list_by_lane(lane_key, limit, offset)?;
        let total = repo.count_by_lane(lane_key)?;
        Ok((messages, total))
    }

    /// Clear conversation history for a lane.
    pub fn clear_history(&self, lane_key: &str) -> Result<u64> {
        let repo = ConversationRepository::new(&self.db);
        repo.delete_by_lane(lane_key)
    }

    /// Get a reference to the stream manager.
    pub fn stream_manager(&self) -> &Arc<ChatStreamManager> {
        &self.stream_manager
    }

    /// Resolve chat attachments from DB and wait briefly for processing completion.
    ///
    /// Returns all attachments even when some are not ready yet (timeout/error path),
    /// with `extracted_text=None` for those assets so downstream logic can render
    /// a safe "pending" fallback instead of stale/partial text.
    async fn resolve_attachments_with_wait(
        db: Database,
        attachment_refs: &[AttachmentRef],
        principal: &str,
        wait_ms: u64,
        poll_interval_ms: u64,
    ) -> Result<Vec<crate::gateway::ResolvedAttachment>> {
        if attachment_refs.is_empty() {
            return Ok(Vec::new());
        }

        let poll_interval_ms = poll_interval_ms.max(1);
        let refs = attachment_refs.to_vec();
        let principal_owned = principal.to_string();
        let mut assets =
            Self::load_attachment_assets(db.clone(), refs.clone(), principal_owned).await?;

        let mut pending_ids: Vec<String> = assets
            .iter()
            .filter(|a| Self::is_pending_status(&a.status))
            .map(|a| a.id.clone())
            .collect();

        if wait_ms > 0 && !pending_ids.is_empty() {
            info!(
                attachments_total = assets.len(),
                attachments_pending = pending_ids.len(),
                wait_ms,
                poll_interval_ms,
                "Waiting for attachments to become ready"
            );

            let start = tokio::time::Instant::now();
            let max_wait = Duration::from_millis(wait_ms);
            while !pending_ids.is_empty() && start.elapsed() < max_wait {
                tokio::time::sleep(Duration::from_millis(poll_interval_ms)).await;
                assets =
                    Self::load_attachment_assets(db.clone(), refs.clone(), principal.to_string())
                        .await?;
                pending_ids = assets
                    .iter()
                    .filter(|a| Self::is_pending_status(&a.status))
                    .map(|a| a.id.clone())
                    .collect();
            }

            if !pending_ids.is_empty() {
                warn!(
                    attachments_pending = pending_ids.len(),
                    wait_ms, "Timed out waiting for attachments; proceeding with pending assets"
                );
            }
        }

        let ready_count = assets
            .iter()
            .filter(|a| matches!(a.status, FileAssetStatus::Ready))
            .count();
        info!(
            attachments_total = assets.len(),
            attachments_ready = ready_count,
            "Attachment resolution completed"
        );

        Ok(assets
            .into_iter()
            .map(|asset| {
                let extracted_text = if matches!(asset.status, FileAssetStatus::Ready) {
                    asset.extracted_text
                } else {
                    None
                };
                crate::gateway::ResolvedAttachment {
                    file_id: asset.id,
                    filename: asset.filename,
                    mime_type: asset.mime_type,
                    size_bytes: asset.size_bytes,
                    extracted_text,
                    storage_path: asset.storage_path,
                }
            })
            .collect())
    }

    async fn load_attachment_assets(
        db: Database,
        attachment_refs: Vec<AttachmentRef>,
        principal: String,
    ) -> Result<Vec<FileAsset>> {
        let handle = tokio::task::spawn_blocking(move || -> Result<Vec<FileAsset>> {
            let repo = FileAssetRepository::new(&db);
            let mut out = Vec::with_capacity(attachment_refs.len());
            for att_ref in &attachment_refs {
                let asset = match repo.get_by_id(&att_ref.file_id)? {
                    Some(a) => a,
                    None => anyhow::bail!("Attachment not found: {}", att_ref.file_id),
                };
                if asset.owner_id != principal {
                    anyhow::bail!("Access denied to attachment: {}", att_ref.file_id);
                }
                out.push(asset);
            }
            Ok(out)
        });

        match handle.await {
            Ok(res) => res,
            Err(e) => Err(anyhow::anyhow!("Attachment DB task failed: {e}")),
        }
    }

    fn is_pending_status(status: &FileAssetStatus) -> bool {
        matches!(
            status,
            FileAssetStatus::Uploaded | FileAssetStatus::Processing
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openalpaca_storage::FileAsset;

    fn build_asset(id: &str, status: FileAssetStatus, extracted_text: Option<&str>) -> FileAsset {
        FileAsset {
            id: id.to_string(),
            owner_id: "u1".to_string(),
            sha256: format!("sha-{id}"),
            filename: format!("{id}.txt"),
            mime_type: "text/plain".to_string(),
            size_bytes: 12,
            storage_path: "/tmp/dummy.txt".to_string(),
            status,
            extracted_text: extracted_text.map(ToString::to_string),
            extract_error: None,
            metadata_json: None,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    fn setup_db() -> Database {
        let db_dir = std::env::temp_dir().join(format!(
            "openalpaca-chat-service-tests-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&db_dir).expect("create db dir");
        let db_path = db_dir.join("test.db");
        Database::open(&db_path).expect("open db")
    }

    #[tokio::test]
    async fn test_resolve_attachments_with_wait_ready() {
        let db = setup_db();
        let repo = FileAssetRepository::new(&db);
        repo.insert(&build_asset(
            "a-ready",
            FileAssetStatus::Ready,
            Some("hello"),
        ))
        .unwrap();

        let refs = vec![AttachmentRef {
            file_id: "a-ready".to_string(),
            caption: None,
        }];
        let resolved = ChatService::resolve_attachments_with_wait(db, &refs, "u1", 200, 20)
            .await
            .unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].file_id, "a-ready");
        assert_eq!(resolved[0].extracted_text.as_deref(), Some("hello"));
    }

    #[tokio::test]
    async fn test_resolve_attachments_with_wait_error_clears_text() {
        let db = setup_db();
        let repo = FileAssetRepository::new(&db);
        repo.insert(&build_asset(
            "a-error",
            FileAssetStatus::Error,
            Some("stale text"),
        ))
        .unwrap();

        let refs = vec![AttachmentRef {
            file_id: "a-error".to_string(),
            caption: None,
        }];
        let resolved = ChatService::resolve_attachments_with_wait(db, &refs, "u1", 200, 20)
            .await
            .unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].file_id, "a-error");
        assert!(resolved[0].extracted_text.is_none());
    }

    #[tokio::test]
    async fn test_resolve_attachments_with_wait_timeout_keeps_attachment() {
        let db = setup_db();
        let repo = FileAssetRepository::new(&db);
        repo.insert(&build_asset(
            "a-pending",
            FileAssetStatus::Uploaded,
            Some("not-ready"),
        ))
        .unwrap();

        let refs = vec![AttachmentRef {
            file_id: "a-pending".to_string(),
            caption: None,
        }];
        let resolved = ChatService::resolve_attachments_with_wait(db, &refs, "u1", 60, 20)
            .await
            .unwrap();
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].file_id, "a-pending");
        assert!(resolved[0].extracted_text.is_none());
    }
}
