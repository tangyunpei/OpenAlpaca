use crate::bus::EventBus;
use crate::context::SharedContext;
use crate::events::SystemEvent;
use crate::gateway::persistence::GatewayPersistence;
use crate::lane::{LaneKey, LaneManager};
use crate::security::policy::{Principal, Scope};
use async_trait::async_trait;
use chrono::Utc;
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

/// Trait for processing messages through the pipeline.
/// The daemon implements this by delegating to the Orchestrator.
#[allow(clippy::too_many_arguments)]
#[async_trait]
pub trait MessageHandler: Send + Sync {
    async fn handle(
        &self,
        request_id: Uuid,
        source: String,
        content: String,
        principal: Principal,
        scope: Scope,
        lane_key: String,
        workspace_path: Option<String>,
        stream_id: Option<String>,
    ) -> Result<HandleResult, String>;

    /// Handle a message with attachments. Default delegates to `handle()`.
    async fn handle_with_attachments(
        &self,
        request_id: Uuid,
        source: String,
        content: String,
        attachments: Vec<ResolvedAttachment>,
        principal: Principal,
        scope: Scope,
        lane_key: String,
        workspace_path: Option<String>,
        stream_id: Option<String>,
    ) -> Result<HandleResult, String> {
        let _ = attachments; // default: ignore attachments
        self.handle(
            request_id,
            source,
            content,
            principal,
            scope,
            lane_key,
            workspace_path,
            stream_id,
        )
        .await
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

        // Emit UserRequest event
        self.bus.publish(SystemEvent::UserRequest {
            request_id,
            source: source_name.clone(),
            content: req.content.clone(),
            timestamp: Utc::now(),
        });

        // Record message on the lane
        lane.record_message();

        // Persist user message
        if let Some(ref p) = self.persistence {
            if req.attachments.is_empty() {
                if let Err(e) = p.persist_user_message(&lane_key_str, &req.content, &source_name) {
                    tracing::warn!("Failed to persist user message: {e}");
                }
            } else if let Err(e) = p.persist_user_message_with_attachments(
                &lane_key_str,
                &req.content,
                &source_name,
                &req.attachments,
            ) {
                tracing::warn!("Failed to persist user message with attachments: {e}");
            }
        }

        let start = std::time::Instant::now();

        // Delegate to the handler — use attachment-aware path when attachments present
        let handler_result = if req.attachments.is_empty() {
            self.handler
                .handle(
                    request_id,
                    source_name.clone(),
                    req.content,
                    req.principal,
                    req.scope,
                    lane_key_str.clone(),
                    req.workspace_path,
                    req.stream_id,
                )
                .await
        } else {
            self.handler
                .handle_with_attachments(
                    request_id,
                    source_name.clone(),
                    req.content,
                    req.attachments,
                    req.principal,
                    req.scope,
                    lane_key_str.clone(),
                    req.workspace_path,
                    req.stream_id,
                )
                .await
        };

        match handler_result {
            Ok(result) => {
                let duration_ms = start.elapsed().as_millis() as i64;
                // Persist assistant message and link to skill execution log
                if let Some(ref p) = self.persistence {
                    match p.persist_assistant_message(
                        &lane_key_str,
                        &result.content,
                        Some(duration_ms),
                        &source_name,
                    ) {
                        Ok(message_id) if message_id > 0 => {
                            if let Err(e) = openalpaca_storage::SkillExecutionRepository::new(p.db())
                                .link_response(&request_id.to_string(), message_id)
                            {
                                tracing::debug!("No skill execution to link for request {request_id}: {e}");
                            }
                        }
                        Err(e) => tracing::warn!("Failed to persist assistant message: {e}"),
                        _ => {}
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
