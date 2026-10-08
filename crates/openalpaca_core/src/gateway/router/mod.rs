use crate::bus::EventBus;
use crate::context::SharedContext;
use crate::gateway::persistence::GatewayPersistence;
use crate::lane::{LaneKey, LaneManager};
use crate::memory::learning::LearningQueue;
use crate::memory::service::MemoryUsed;
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

/// U3 — one of the turn's attachments that never reached the model, and why.
///
/// The counterpart of `attachments_used`: an attachment the adaptation
/// replaced with a placeholder (no vision, no document support and no
/// extracted text) is *not* listed as used, and appears here instead with the
/// reason a person can read. Travels `HandleResult` → `GatewayResponse` → the
/// SSE `done` frame, exactly as `attachments_used` does.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SkippedAttachment {
    pub id: String,
    pub reason: String,
}

/// Design 11 §5 (owner decision CQ-7): the previous conversation a turn was
/// given (§4 (b)) — its session and its working state's topic, `""` when it
/// carried no state, only its last exchange. The SSE `done` frame's
/// `continued_from`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ContinuedFrom {
    pub session_id: String,
    pub topic: String,
}

/// Design 11 §5 (CQ-7): what a turn says about its conversation's continuity.
/// Travels `HandleResult` → `GatewayResponse` → the SSE `done` frame, like
/// `memory_used`, and is persisted nowhere.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnContinuity {
    /// The turn compacted, and its working state goes to the session row at
    /// turn end (§3.3).
    pub working_state_updated: bool,
    /// The request carried the previous conversation's block (§4 (b)).
    pub continued_from: Option<ContinuedFrom>,
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
    /// File IDs of attachments that reached the model in some form — natively,
    /// or (U2) as the labelled text of a document. An attachment the
    /// adaptation withheld is **not** here; it is in `attachments_skipped`.
    pub attachments_used: Vec<String>,
    /// U3 — the turn's attachments that never reached the model, with a reason.
    pub attachments_skipped: Vec<SkippedAttachment>,
    /// Plan 3.3 (design §7a) — the memory notes the turn delivered: the
    /// automatic recall (`pinned`, `recall`) and what the model fetched
    /// through `memory_search`/`memory_read` (`tool`). Live only, like
    /// `attachments_skipped`: nothing persists it.
    pub memory_used: Vec<MemoryUsed>,
    /// Set when handling delegated the message to a background task.
    pub delegation: Option<DelegationInfo>,
    /// Design 11 §5 (CQ-7) — the working state written by the turn's
    /// compaction and the previous conversation it was given. Live only.
    pub continuity: TurnContinuity,
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
            attachments_skipped: Vec::new(),
            memory_used: Vec::new(),
            delegation: None,
            continuity: TurnContinuity::default(),
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
    /// The turn's project root. From the gateway, the root it resolved and
    /// recorded from the path the client sent (GUI project dir, CLI cwd) —
    /// the root the conversation row is bound to — never the raw path, so
    /// the handler cannot resolve it to another project (the final fix
    /// wave's R7); `None` for a turn under no project.
    pub workspace_path: Option<String>,
    /// SSE stream ID for routing tool confirmation prompts to the chat stream.
    pub stream_id: Option<String>,
    /// GAP-13 — the model this **one** turn runs on, already validated against
    /// the model registry by the route that accepted it. `None` leaves the
    /// daemon default in place. Request-scoped by construction: nothing is
    /// persisted, so the next turn on the same lane is back on the default.
    pub model_override: Option<String>,
    /// M6 — see [`GatewayRequest::unattended`]. `false` for every caller that
    /// does not set it, which is today's behaviour.
    pub unattended: bool,
    /// S1 — see [`GatewayRequest::turn_sink`]. `None` for every caller that
    /// has nowhere to put a live delta.
    pub turn_sink: Option<crate::chat::TurnSinkHandle>,
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
            unattended: false,
            turn_sink: None,
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
    /// With none the turn has no project; the daemon's CWD is never read.
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
    /// M6 — the client sending this turn declares that it cannot answer a
    /// tool confirmation: the one-shot and piped `openalpaca chat` paths
    /// (`unattended: true` on `POST /v1/chat`), and the scheduled-skill
    /// runner, which has no client at all.
    ///
    /// It travels with the turn and with any workflow the turn starts, so a
    /// tool needing approval is refused at once with a message saying where
    /// it *can* be approved, instead of waiting out the 300-second
    /// confirmation timeout for an answer nobody is there to give. Never an
    /// approval — fail-closed stays.
    ///
    /// `false` is every client that says nothing, and is today's behaviour.
    pub unattended: bool,
    /// S1 — where this turn's text goes **while the model is producing it**.
    ///
    /// Set by [`ChatService::send_message`](crate::chat::ChatService::send_message),
    /// which owns the SSE stream the deltas land in; `None` for a connector,
    /// a scheduled skill, a follow-up — every caller with no client watching a
    /// stream. It travels to the main loop's `LoopConfig.stream_callback`, so
    /// each provider delta reaches the client as it arrives instead of after
    /// the turn. The turn's `done` still carries the authoritative content.
    pub turn_sink: Option<crate::chat::TurnSinkHandle>,
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
    /// File IDs of attachments that reached the model in some form.
    pub attachments_used: Vec<String>,
    /// U3 — the turn's attachments that never reached the model, with a reason.
    pub attachments_skipped: Vec<SkippedAttachment>,
    /// Plan 3.3 — the memory notes the turn delivered; empty when none.
    pub memory_used: Vec<MemoryUsed>,
    /// Set when handling delegated the message to a background task.
    pub delegation: Option<DelegationInfo>,
    /// Design 11 §5 (CQ-7) — see [`HandleResult::continuity`].
    pub continuity: TurnContinuity,
}

/// The unified entry point for all inbound messages.
pub struct Gateway {
    pub shared_context: Arc<SharedContext>,
    pub lane_manager: Arc<LaneManager>,
    pub handler: Arc<dyn MessageHandler>,
    pub bus: EventBus,
    persistence: Option<GatewayPersistence>,
    /// Learning v2 (design 09 §1.5): the queue every persisted turn is fed
    /// to. `None` learns nothing — every gateway but the daemon's own.
    learning: Option<Arc<LearningQueue>>,
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
            learning: None,
        }
    }

    /// Feeds every turn this gateway persists to `queue` (design 09 §1.5).
    /// The daemon sets it on its one production gateway.
    pub fn with_learning(mut self, queue: Arc<LearningQueue>) -> Self {
        self.learning = Some(queue);
        self
    }

    /// The learning queue this gateway feeds, if any.
    pub fn learning(&self) -> Option<&Arc<LearningQueue>> {
        self.learning.as_ref()
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
        // Storage design §3: a chat turn accepts work for its root, so the
        // root is resolved and recorded under `projects/` before anything of
        // the turn exists — no in-flight learning entry, no row, no handler
        // run. A root that cannot be recorded ends the turn here, so no row
        // ever names a project with no folder. A gateway without a database
        // records it too: its handler places content all the same.
        let turn_scope = match crate::memory::scope_context::MemoryScopeContext::for_turn(
            req.workspace_path.as_deref(),
        ) {
            Ok(scope) => scope,
            Err(e) => {
                let path = req.workspace_path.as_deref().unwrap_or_default();
                tracing::warn!(path, "could not record the turn's project: {e:#}");
                return GatewayResponse {
                    lane_key: key,
                    content: format!("Could not record the project for {path}: {e:#}"),
                    is_error: true,
                    model: None,
                    tokens_in: None,
                    tokens_out: None,
                    attachments_used: Vec::new(),
                    attachments_skipped: Vec::new(),
                    memory_used: Vec::new(),
                    delegation: None,
                    continuity: TurnContinuity::default(),
                };
            }
        };
        let lane_key_str = key.to_string();
        let lane = self.lane_manager.get_or_create_conversation(key.clone());

        let request_id = Uuid::new_v4();

        // Learning v2 (design 09 §1.5): the turn is in flight from here, before
        // its user row exists, so a flush reading the window meanwhile stops
        // before the row rather than learning a turn whose answer has not
        // landed. The guard's drop — after the assistant row is written, or on
        // any earlier exit — ends it. Three in-memory updates in all: the
        // reply path does no learning work.
        let turn_guard = self.learning.as_ref().map(|queue| {
            queue.turn_started(
                request_id,
                &lane_key_str,
                crate::memory::learning::is_user_authored_turn(&source_name, &req.content),
            )
        });

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
            // scoping use, resolved once above (R22). `None` — no header, or a
            // path under no project — means no project, and the session stays
            // unbound.
            let workspace_root = turn_scope.request_workspace_root.clone();
            let workspace_path = workspace_root.as_deref();
            let persisted = if req.attachments.is_empty() {
                p.persist_user_message(
                    &lane_key_str,
                    &req.content,
                    &source_name,
                    workspace_path,
                    Some(request_id),
                )
            } else {
                p.persist_user_message_with_attachments(
                    &lane_key_str,
                    &req.content,
                    &source_name,
                    workspace_path,
                    &req.attachments,
                    Some(request_id),
                )
            };
            match persisted {
                Ok(turn) => {
                    if let Some(guard) = &turn_guard {
                        guard.attach_session(&turn.session_id);
                    }
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
            // R7: the root `for_turn` recorded and the user row is bound to,
            // not the raw path, which the handler would resolve again.
            workspace_path: turn_scope.request_workspace_root.clone(),
            stream_id: req.stream_id,
            model_override: req.model_override,
            unattended: req.unattended,
            turn_sink: req.turn_sink,
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
                    // The whole result: GAP-23's run link and GAP-13's
                    // answering model are both read off *it*, so both survive
                    // the reload that the SSE `done` frame below does not.
                    match p.persist_assistant_message(
                        &lane_key_str,
                        &result,
                        Some(duration_ms),
                        &source_name,
                        // §5.1: the session this turn resolved when it began.
                        turn_session.as_deref(),
                        Some(request_id),
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
                    attachments_skipped: result.attachments_skipped,
                    memory_used: result.memory_used,
                    delegation: result.delegation,
                    continuity: result.continuity,
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
                attachments_skipped: Vec::new(),
                memory_used: Vec::new(),
                delegation: None,
                continuity: TurnContinuity::default(),
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
