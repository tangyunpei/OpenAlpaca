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
use openalpaca_storage::{ConversationMessage, ConversationRepository, Database};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::info;

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
    pub fn send_message(&self, content: String, attachments: Vec<crate::gateway::ResolvedAttachment>, principal: &str, workspace_path: Option<String>) -> Result<ChatSendResponse> {
        let lane_key = format!("{principal}:gui");

        let (stream_id, _rx, sink) = self.stream_manager.create_stream(&lane_key);

        // Spawn background task for the actual gateway call
        let gateway = self.gateway.clone();
        let stream_manager = self.stream_manager.clone();
        let sid = stream_id.clone();
        let user_content = content.clone();
        let principal_owned = principal.to_string();
        let bus = self.bus.clone();
        let daemon_config = self.daemon_config.clone();
        let lk = lane_key.clone();

        tokio::spawn(async move {
            // Give browser time to connect to SSE endpoint
            tokio::time::sleep(Duration::from_millis(100)).await;

            // Emit Thinking AFTER sleep — client has subscribed by now
            sink.send_thinking();

            let start = Instant::now();

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
                if response.attachments_used.is_empty() {
                    sink.send_done(&response.content, model, tokens_in, tokens_out, duration_ms);
                } else {
                    sink.send_done_with_attachments(&response.content, model, tokens_in, tokens_out, duration_ms, response.attachments_used);
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
}
