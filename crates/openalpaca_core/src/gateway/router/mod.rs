use crate::bus::EventBus;
use crate::context::SharedContext;
use crate::gateway::persistence::GatewayPersistence;
use crate::lane::{LaneKey, LaneManager};
use crate::security::policy::{Principal, Scope};
use async_trait::async_trait;
use openalpaca_api::events::EventSource;
use openalpaca_storage::Database;
use std::sync::Arc;
use uuid::Uuid;

/// A resolved attachment with metadata, ready for processing by handlers.
#[derive(Debug, Clone)]
pub struct ResolvedAttachment {
    pub file_id: String,
    pub filename: String,
    pub mime_type: String,
    pub size_bytes: i64,
    pub extracted_text: Option<String>,
    pub storage_path: String,
}

/// Structured metadata for a delegated task, carried alongside the ack text
/// so clients can track the created task without parsing prose.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DelegationInfo {
    pub task_id: String,
    pub title: String,
}

/// Rich result from message handling, carrying optional LLM metadata.
///
/// Non-LLM paths (task queries, commands, etc.) use `HandleResult::text()` which
/// sets all metadata fields to `None`.
#[derive(Debug)]
pub struct HandleResult {
    pub content: String,
    pub model: Option<String>,
    pub tokens_in: Option<u32>,
    pub tokens_out: Option<u32>,
    /// File IDs of attachments consumed during handling.
    pub attachments_used: Vec<String>,
    /// Set when handling delegated the message to a background task.
    pub delegation: Option<DelegationInfo>,
}

impl HandleResult {
    /// Create a HandleResult for non-LLM responses (no metadata).
    pub fn text(content: String) -> Self {
        Self {
            content,
            model: None,
            tokens_in: None,
            tokens_out: None,
            attachments_used: Vec::new(),
            delegation: None,
        }
    }
}

/// One inbound turn, as a [`MessageHandler`] receives it.
///
/// This used to be a positional argument list, and GAP-13's `model_override`
/// would have been its ninth entry — eight already needed
/// `#[allow(clippy::too_many_arguments)]`, and a call site passing
/// `None, None, None` in a row says nothing about which `None` is which. Named
/// fields make each caller legible and let the next field be added without
/// re-punctuating every one of them.
#[derive(Debug, Clone)]
pub struct HandleRequest {
    pub request_id: Uuid,
    /// The derived source name ("gui", "cli", "telegram", …), not the raw
    /// `EventSource`.
    pub source: String,
    pub content: String,
    pub principal: Principal,
    pub scope: Scope,
    /// The lane this turn landed on, canonical `"{user_id}:{source}"`.
    pub lane_key: String,
    /// Workspace path the client provided (GUI project dir, CLI cwd).
    pub workspace_path: Option<String>,
    /// SSE stream ID for routing tool confirmation prompts to the chat stream.
    pub stream_id: Option<String>,
    /// GAP-13 — the model this **one** turn runs on, already validated against
    /// the model registry by the route that accepted it. `None` leaves the
    /// daemon default in place. Request-scoped by construction: nothing is
    /// persisted, so the next turn on the same lane is back on the default.
    pub model_override: Option<String>,
}

impl HandleRequest {
    /// A turn with no workspace, no stream and no model override — the three
    /// optional fields most callers leave empty. Set one with struct update
    /// syntax: `HandleRequest { stream_id: Some(id), ..HandleRequest::new(…) }`.
    pub fn new(
        request_id: Uuid,
        source: impl Into<String>,
        content: impl Into<String>,
        principal: Principal,
        scope: Scope,
        lane_key: impl Into<String>,
    ) -> Self {
        Self {
            request_id,
            source: source.into(),
            content: content.into(),
            principal,
            scope,
            lane_key: lane_key.into(),
            workspace_path: None,
            stream_id: None,
            model_override: None,
        }
    }
}

/// Trait for processing messages through the pipeline.
/// The daemon implements this by delegating to the Orchestrator.
#[async_trait]
pub trait MessageHandler: Send + Sync {
    async fn handle(&self, request: HandleRequest) -> Result<HandleResult, String>;

    /// Handle a message with attachments. Default delegates to `handle()`.
    async fn handle_with_attachments(
        &self,
        request: HandleRequest,
        attachments: Vec<ResolvedAttachment>,
    ) -> Result<HandleResult, String> {
        let _ = attachments; // default: ignore attachments
        self.handle(request).await
    }
}

/// Inbound request to the Gateway.
pub struct GatewayRequest {
    pub source: EventSource,
    pub content: String,
    /// Resolved file attachments for multimodal messages.
    pub attachments: Vec<ResolvedAttachment>,
    pub principal: Principal,
    pub scope: Scope,
    /// Optional workspace path provided by the client (GUI project dir, CLI cwd).
    /// Used for memory scoping instead of the daemon's CWD.
    pub workspace_path: Option<String>,
    /// SSE stream ID for routing tool confirmation prompts to the active chat stream.
    /// Set by `ChatService::send_message()`, `None` for connector/API requests.
    pub stream_id: Option<String>,
    /// Optional explicit lane override in canonical "user_id:source" form
    /// (Routing V2): when set and well-formed, the turn lands on this exact
    /// lane instead of the one derived from `source`/`principal`. Set by the
    /// follow-up runner so re-entered turns continue the ORIGINATING
    /// conversation; `None` everywhere else.
    pub lane_override: Option<String>,
    /// GAP-13 — the model this one turn runs on, validated by the route that
    /// accepted it (`POST /v1/chat`). `None` — every other entry point —
    /// leaves the daemon default in place.
    pub model_override: Option<String>,
}

/// Response from the gateway after handling a message.
#[derive(Debug)]
pub struct GatewayResponse {
    pub lane_key: LaneKey,
    pub content: String,
    /// Structured error flag — `true` when the handler returned `Err`.
    /// Replaces ad-hoc string prefix matching ("[error]", "Error:", etc.).
    pub is_error: bool,
    /// LLM model used (if any).
    pub model: Option<String>,
    /// Input tokens consumed (if LLM was called).
    pub tokens_in: Option<u32>,
    /// Output tokens generated (if LLM was called).
    pub tokens_out: Option<u32>,
    /// File IDs of attachments consumed during handling.
    pub attachments_used: Vec<String>,
    /// Set when handling delegated the message to a background task.
    pub delegation: Option<DelegationInfo>,
}

/// The unified entry point for all inbound messages.
pub struct Gateway {
    pub shared_context: Arc<SharedContext>,
    pub lane_manager: Arc<LaneManager>,
    pub handler: Arc<dyn MessageHandler>,
    pub bus: EventBus,
    persistence: Option<GatewayPersistence>,
}

impl Gateway {
    pub fn new(
        shared_context: Arc<SharedContext>,
        lane_manager: Arc<LaneManager>,
        handler: Arc<dyn MessageHandler>,
        bus: EventBus,
        db: Option<Database>,
    ) -> Self {
        let persistence = db.map(GatewayPersistence::new);
        Self {
            shared_context,
            lane_manager,
            handler,
            bus,
            persistence,
        }
    }

    /// Handle an inbound event from any source.
    pub async fn handle_event(&self, req: GatewayRequest) -> GatewayResponse {
        let (mut user_id, source_name) = derive_user_and_source(&req.source);

        // Principal-aware: linked users use their global_id as lane user
        if let Principal::User { ref global_id } = req.principal {
            user_id = global_id.clone();
        }

        // Lane override (Routing V2): a well-formed override pins the turn to
        // its originating lane (follow-up re-entry); malformed values fall
        // back to the derived lane.
        let key = match req.lane_override.as_deref().and_then(LaneKey::from_str) {
            Some(overridden) => overridden,
            None => {
                if let Some(ref raw) = req.lane_override {
                    tracing::warn!(
                        lane_override = %raw,
                        "Malformed lane_override; falling back to derived lane"
                    );
                }
                LaneKey::new(&user_id, &source_name)
            }
        };
        let lane_key_str = key.to_string();
        let lane = self.lane_manager.get_or_create_conversation(key.clone());

        let request_id = Uuid::new_v4();

        // Record message on the lane
        lane.record_message();

        // Persist user message. This is where the turn resolves lane →
        // session (§5.1): the workspace the request carried binds the
        // session's project the first time one is seen. The id it resolves is
        // held for the whole turn — the handler below runs a full agentic loop,
        // and the assistant half must land in the conversation the question was
        // asked in, not wherever a mid-turn "New chat" left the lane pointing.
        let mut turn_session: Option<String> = None;
        let mut turn_log: Option<crate::session_log::SessionLogHandle> = None;
        // The user half's own content and attachment ids, kept for the log
        // record below — `req.content` moves into the handler.
        let content_for_log = req.content.clone();
        let attachment_ids: Vec<String> = req
            .attachments
            .iter()
            .map(|a| a.file_id.clone())
            .collect();
        if let Some(ref p) = self.persistence {
            // The canonical project root, not the raw header: a session's
            // `workspace_id` is the same key `task.workspace_id` and memory
            // scoping use, and `MemoryScopeContext::for_request` is the one
            // resolver (R22). `None` — no header, or a path under no marker —
            // means no project, and the session stays unbound.
            let workspace_root =
                crate::memory::scope_context::MemoryScopeContext::for_request(
                    req.workspace_path.as_deref(),
                )
                .request_workspace_root;
            let workspace_path = workspace_root.as_deref();
            let persisted = if req.attachments.is_empty() {
                p.persist_user_message(&lane_key_str, &req.content, &source_name, workspace_path)
            } else {
                p.persist_user_message_with_attachments(
                    &lane_key_str,
                    &req.content,
                    &source_name,
                    workspace_path,
                    &req.attachments,
                )
            };
            match persisted {
                Ok(turn) => {
                    // §5.1: the turn's project was not this lane's session's
                    // project, so a new conversation was opened for it. Say so,
                    // the way `POST /v1/sessions` does — a second window's
                    // sidebar must not keep showing the session that just
                    // stepped down.
                    if turn.project_switched {
                        let _ = self.bus.publish(crate::events::SystemEvent::SessionChanged {
                            session_id: turn.session_id.clone(),
                            lane_key: lane_key_str.clone(),
                            status: "active".to_string(),
                            task_id: None,
                            timestamp: chrono::Utc::now(),
                        });
                    }
                    // §5.5: the gateway opens the turn's session log and
                    // writes the user half. Content stays in the DB — the
                    // record carries the message id and a preview (§5.3's
                    // one-source-of-truth table), never a second copy of the
                    // conversation.
                    if let Some(service) = self.shared_context.session_log() {
                        let log = service.open(
                            &turn.session_id,
                            Some(&lane_key_str),
                            Some(&source_name),
                            workspace_path,
                        );
                        log.emit(
                            crate::session_log::Record::new(
                                crate::session_log::RecordType::UserMsg,
                            )
                            .with_data(serde_json::json!({
                                "msg_id": turn.message_id,
                                "preview": preview(&content_for_log),
                                "source": source_name,
                                "attachments": attachment_ids,
                                "project_switched": turn.project_switched,
                            })),
                        );
                        turn_log = Some(log);
                    }
                    turn_session = Some(turn.session_id);
                }
                Err(e) => tracing::warn!("Failed to persist user message: {e}"),
            }
        }

        let start = std::time::Instant::now();

        // Delegate to the handler — use attachment-aware path when attachments present
        let handle_request = HandleRequest {
            request_id,
            source: source_name.clone(),
            content: req.content,
            principal: req.principal,
            scope: req.scope,
            lane_key: lane_key_str.clone(),
            workspace_path: req.workspace_path,
            stream_id: req.stream_id,
            model_override: req.model_override,
        };
        let handler_result = if req.attachments.is_empty() {
            self.handler.handle(handle_request).await
        } else {
            self.handler
                .handle_with_attachments(handle_request, req.attachments)
                .await
        };

        match handler_result {
            Ok(result) => {
                let duration_ms = start.elapsed().as_millis() as i64;
                let mut assistant_msg_id: i64 = 0;
                // Persist assistant message and link to skill execution log
                if let Some(ref p) = self.persistence {
                    match p.persist_assistant_message(
                        &lane_key_str,
                        &result.content,
                        Some(duration_ms),
                        &source_name,
                        // GAP-23: a delegating turn is stored carrying the run
                        // it started, so the link survives the reload that the
                        // SSE `done` frame below does not.
                        result.delegation.as_ref().map(|d| d.task_id.as_str()),
                        // §5.1: the session this turn resolved when it began.
                        turn_session.as_deref(),
                    ) {
                        Ok(message_id) if message_id > 0 => {
                            if let Err(e) = openalpaca_storage::SkillExecutionRepository::new(p.db())
                                .link_response(&request_id.to_string(), message_id)
                            {
                                tracing::debug!("No skill execution to link for request {request_id}: {e}");
                            }
                            assistant_msg_id = message_id;
                        }
                        Err(e) => tracing::warn!("Failed to persist assistant message: {e}"),
                        _ => {}
                    }
                }

                // §5.5: the assistant half, and — beside where
                // `result.delegation` is read — the `delegation` record. Both
                // land in the session the turn *started* in, which is the
                // whole point of pinning it above.
                if let Some(ref log) = turn_log {
                    log.emit(
                        crate::session_log::Record::new(
                            crate::session_log::RecordType::AssistantMsg,
                        )
                        .with_data(serde_json::json!({
                            "msg_id": assistant_msg_id,
                            "preview": preview(&result.content),
                            "model": result.model,
                            "tokens_in": result.tokens_in,
                            "tokens_out": result.tokens_out,
                            "duration_ms": duration_ms,
                        })),
                    );
                    if let Some(ref delegation) = result.delegation {
                        log.emit(
                            crate::session_log::Record::new(
                                crate::session_log::RecordType::Delegation,
                            )
                            .task(Some(&delegation.task_id))
                            .with_data(serde_json::json!({
                                "task_id": delegation.task_id,
                                "title": delegation.title,
                            })),
                        );
                    }
                }
                GatewayResponse {
                    lane_key: key,
                    content: result.content,
                    is_error: false,
                    model: result.model,
                    tokens_in: result.tokens_in,
                    tokens_out: result.tokens_out,
                    attachments_used: result.attachments_used,
                    delegation: result.delegation,
                }
            }
            Err(e) => GatewayResponse {
                lane_key: key,
                content: e,
                is_error: true,
                model: None,
                tokens_in: None,
                tokens_out: None,
                attachments_used: Vec::new(),
                delegation: None,
            },
        }
    }

    /// Health check.
    pub fn is_healthy(&self) -> bool {
        true
    }
}

/// §5.4: a chat record carries "msg_id, ≤512 preview" — the content itself
/// lives in `conversation_messages` and is never copied into the log.
fn preview(content: &str) -> String {
    content.chars().take(512).collect()
}

/// Derive user_id and source_name from EventSource.
fn derive_user_and_source(source: &EventSource) -> (String, String) {
    match source {
        EventSource::Telegram { user_id, .. } => (user_id.clone(), "telegram".to_string()),
        EventSource::IMessage { sender, .. } => (sender.clone(), "imessage".to_string()),
        EventSource::Discord { user_id, .. } => (user_id.clone(), "discord".to_string()),
        EventSource::Gui { connection_id } => (connection_id.clone(), "gui".to_string()),
        EventSource::Api { request_id } => (request_id.clone(), "api".to_string()),
        EventSource::Internal => ("system".to_string(), "internal".to_string()),
    }
}

#[cfg(test)]
mod tests;
