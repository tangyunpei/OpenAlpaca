//! ChatService — Core chat logic decoupled from route handlers
//!
//! Orchestrates gateway calls, stream management, and message persistence.
//! The turn's text reaches the client while the model is producing it (S1):
//! the service hands the turn a [`TurnSinkHandle`] over its own stream, the
//! agentic loop forwards every provider delta into it, and `Done` closes with
//! the authoritative content.

use crate::bus::EventBus;
use crate::chat::stream_manager::ChatStreamManager;
use crate::chat::turn_sink::TurnSinkHandle;
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

/// Every named attachment exists and belongs to `principal`.
///
/// One rule, one place: the route runs it as the last of its **pure** checks,
/// before anything activates a session (D16), and
/// [`ChatService::send_message`] runs it for the callers that do not come
/// through the route. The message wording is part of the contract — the route
/// maps `Attachment not found` to `404 ATTACHMENT_NOT_FOUND` and `Access denied
/// to attachment` to `403 ATTACHMENT_ACCESS_DENIED`.
pub fn preflight_attachments(
    db: &Database,
    attachment_refs: &[AttachmentRef],
    principal: &str,
) -> Result<()> {
    let file_repo = FileAssetRepository::new(db);
    for att_ref in attachment_refs {
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
    Ok(())
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
    /// 2. `Delta { content }` × N — the provider's own text deltas, forwarded
    ///    as they arrive (S1). A turn that streams nothing — a deterministic
    ///    tier, a provider with no streaming, a stream that failed and fell
    ///    back — sends the finished answer as the one delta instead, so a
    ///    client that renders deltas still has something to render.
    /// 3. `Done { content, model, tokens_in, tokens_out, duration_ms }` — full text + metadata
    ///
    /// `Done.content` is authoritative: a client rebuilds the bubble from it,
    /// so a dropped or duplicated delta costs a flicker, never the answer.
    ///
    /// On error: `Thinking` → `Error { message }`.
    ///
    /// `model_override` (GAP-13) runs this one turn on a named model. The
    /// route validated the id against the model registry before calling —
    /// this path only carries it — and nothing persists it, so the next turn
    /// on the lane is back on the daemon default.
    /// `unattended` (M6) is the caller's declaration that it cannot answer a
    /// tool confirmation — the CLI's one-shot and piped paths. It reaches the
    /// turn's sandbox policy and any workflow the turn starts, where a tool
    /// needing approval is refused at once instead of waiting out the
    /// confirmation timeout with no responder. `false` is today's behaviour.
    pub fn send_message(
        &self,
        content: String,
        attachment_refs: Vec<AttachmentRef>,
        principal: &str,
        workspace_path: Option<String>,
        model_override: Option<String>,
        unattended: bool,
    ) -> Result<ChatSendResponse> {
        // Fast preflight check so invalid attachment IDs still fail the request
        // immediately. `POST /v1/chat` runs the same function *before* it
        // activates a session (D16), so a refused turn never re-homes a lane;
        // this call is what covers every other caller of `send_message`.
        preflight_attachments(&self.db, &attachment_refs, principal)?;

        let lane_key = format!("{principal}:gui");

        let (stream_id, _rx, sink) = self.stream_manager.create_stream(&lane_key);
        // S1: the same stream, seen by the turn as a place to put text while
        // the model is still writing it.
        let turn_sink = TurnSinkHandle::new(Arc::new(sink.clone()));

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
                    lane_override: None,
                    model_override,
                    unattended,
                    turn_sink: Some(turn_sink.clone()),
                })
                .await;

            let duration_ms = start.elapsed().as_millis() as u64;

            // Note: Message persistence is now handled by Gateway (GatewayPersistence).
            // ChatService only manages the SSE stream events.

            if response.is_error {
                sink.send_error(&response.content);
            } else {
                // S1: the deltas are already gone — the loop forwarded each
                // one as the provider produced it. What is left is the case
                // where nothing streamed at all: a deterministic tier with no
                // model in it, a provider without streaming, a stream that
                // failed and was answered by the non-streaming fallback. Those
                // turns owe the client its text once, here.
                if !turn_sink.saw_text() && !response.content.is_empty() {
                    sink.send_delta(&response.content);
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

    /// Get one session's transcript (migration 039).
    ///
    /// Session-scoped, not lane-scoped: a lane now holds many conversations,
    /// and reading them together would splice two transcripts into one.
    pub fn get_history(
        &self,
        session_id: &str,
        limit: i64,
        offset: i64,
    ) -> Result<(Vec<ConversationMessage>, i64)> {
        let repo = ConversationRepository::new(&self.db);
        let messages = repo.list_by_session(session_id, limit, offset)?;
        let total = repo.count_by_session(session_id)?;
        Ok((messages, total))
    }

    /// Clear one session's transcript. The session row survives — this empties
    /// a conversation, it does not delete it (`DELETE /v1/sessions/{id}` does).
    pub fn clear_history(&self, session_id: &str) -> Result<u64> {
        let repo = ConversationRepository::new(&self.db);
        repo.delete_by_session(session_id)
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

    // ── S1: real streaming ────────────────────────────────────────────

    use crate::chat::ChatStreamEvent;
    use crate::context::SharedContext;
    use crate::gateway::{HandleRequest, HandleResult, MessageHandler};
    use crate::lane::LaneManager;
    use tokio::sync::{Mutex as AsyncMutex, oneshot};

    /// A handler that writes two deltas into the turn's sink and then parks
    /// until the test releases it. The park is what makes the ordering
    /// assertion structural: the turn *cannot* have completed while the test
    /// is reading the deltas.
    struct ParkedStreamingHandler {
        release: AsyncMutex<Option<oneshot::Receiver<()>>>,
    }

    #[async_trait::async_trait]
    impl MessageHandler for ParkedStreamingHandler {
        async fn handle(&self, request: HandleRequest) -> Result<HandleResult, String> {
            let sink = request
                .turn_sink
                .expect("a chat turn must carry the sink of the stream it answers into");
            // S2: the model thinks out loud before it writes.
            sink.reasoning_delta("the user said hi");
            sink.text_delta("Hel");
            sink.text_delta("lo");
            let parked = self
                .release
                .lock()
                .await
                .take()
                .expect("the handler runs once per test");
            parked.await.expect("the test releases the turn");
            Ok(HandleResult::text("Hello".to_string()))
        }
    }

    /// A handler that streams nothing at all — a deterministic tier, or a
    /// provider with no streaming whose answer arrives whole.
    struct SilentHandler;

    #[async_trait::async_trait]
    impl MessageHandler for SilentHandler {
        async fn handle(&self, _request: HandleRequest) -> Result<HandleResult, String> {
            Ok(HandleResult::text("the whole answer".to_string()))
        }
    }

    fn service_with(handler: Arc<dyn MessageHandler>) -> (ChatService, Arc<ChatStreamManager>) {
        let bus = EventBus::default();
        let gateway = Arc::new(crate::gateway::Gateway::new(
            Arc::new(SharedContext::new()),
            Arc::new(LaneManager::new()),
            handler,
            bus.clone(),
            None,
        ));
        let streams = Arc::new(ChatStreamManager::new());
        let service = ChatService::new(
            gateway,
            streams.clone(),
            setup_db(),
            bus,
            Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
        );
        (service, streams)
    }

    /// Collect stream events until `Done`, with a deadline per event so a
    /// turn that only speaks after it finishes fails loudly instead of
    /// hanging. `on_delta` runs for each delta as it arrives.
    async fn drain_until_done(
        rx: &mut tokio::sync::broadcast::Receiver<ChatStreamEvent>,
        mut on_delta: impl FnMut(&str),
    ) -> ChatStreamEvent {
        loop {
            let event = tokio::time::timeout(Duration::from_secs(10), rx.recv())
                .await
                .expect("the stream must produce an event within 10s")
                .expect("the stream must not lag or close early");
            match event {
                ChatStreamEvent::Delta { ref content } => on_delta(content),
                ChatStreamEvent::Done { .. } => return event,
                ChatStreamEvent::Error { message } => panic!("stream error: {message}"),
                ChatStreamEvent::Thinking
                | ChatStreamEvent::Reasoning { .. }
                | ChatStreamEvent::ConfirmationRequested { .. } => {}
            }
        }
    }

    /// **S1.** The provider's text reaches the subscriber *while the turn is
    /// still running* — not re-cut from the finished answer afterwards.
    ///
    /// The ordering is asserted against the turn's own completion, not a
    /// clock: the handler cannot return until the test has both deltas in
    /// hand, so a service that streamed only after `handle_event` returned
    /// would deadlock and trip the per-event deadline.
    #[tokio::test]
    async fn deltas_arrive_before_the_turn_completes() {
        let (release_tx, release_rx) = oneshot::channel();
        let (service, streams) = service_with(Arc::new(ParkedStreamingHandler {
            release: AsyncMutex::new(Some(release_rx)),
        }));

        let sent = service
            .send_message("hi".to_string(), vec![], "u1", None, None, false)
            .expect("send");
        let mut rx = streams
            .get_receiver(&sent.stream_id)
            .expect("the stream exists as soon as send_message returns");

        // Read exactly the two live deltas. The turn is parked until we do.
        let mut live = Vec::new();
        let mut reasoning = Vec::new();
        while live.len() < 2 {
            let event = tokio::time::timeout(Duration::from_secs(10), rx.recv())
                .await
                .expect("a delta must arrive while the turn is still running")
                .expect("the stream must not lag or close early");
            match event {
                ChatStreamEvent::Delta { content } => live.push(content),
                ChatStreamEvent::Reasoning { text } => reasoning.push(text),
                _ => {}
            }
        }
        assert_eq!(live, vec!["Hel".to_string(), "lo".to_string()]);
        // S2: the reasoning arrived live, on its own event, ahead of the text.
        assert_eq!(reasoning, vec!["the user said hi".to_string()]);

        // Only now can the turn finish.
        release_tx.send(()).expect("the turn is still parked");

        let mut extra = Vec::new();
        let done = drain_until_done(&mut rx, |c| extra.push(c.to_string())).await;
        assert!(
            extra.is_empty(),
            "the finished answer must not be re-chunked on top of the live deltas, got {extra:?}"
        );
        match done {
            ChatStreamEvent::Done { content, .. } => {
                assert_eq!(content, "Hello", "done.content stays authoritative");
                assert!(
                    !content.contains("the user said hi"),
                    "reasoning is surfaced, never persisted into the answer"
                );
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    /// **S1.** A turn that streams nothing still hands the client its text
    /// once — one delta carrying the whole answer, then `Done`.
    #[tokio::test]
    async fn a_turn_that_streams_nothing_sends_one_delta() {
        let (service, streams) = service_with(Arc::new(SilentHandler));

        let sent = service
            .send_message("hi".to_string(), vec![], "u1", None, None, false)
            .expect("send");
        let mut rx = streams.get_receiver(&sent.stream_id).expect("stream");

        let mut deltas = Vec::new();
        let done = drain_until_done(&mut rx, |c| deltas.push(c.to_string())).await;
        assert_eq!(
            deltas,
            vec!["the whole answer".to_string()],
            "exactly one delta, the answer itself — no word chunking"
        );
        match done {
            ChatStreamEvent::Done { content, .. } => assert_eq!(content, "the whole answer"),
            other => panic!("expected Done, got {other:?}"),
        }
    }
}
