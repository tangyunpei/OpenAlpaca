//! ChatService — Core chat logic decoupled from route handlers
//!
//! Orchestrates gateway calls, stream management, and message persistence.

use crate::chat::stream_manager::{ChatStreamEvent, ChatStreamManager};
use crate::gateway::{Gateway, GatewayRequest};
use crate::security::policy::{Principal, Scope};
use anyhow::Result;
use openalpaca_api::events::EventSource;
use openalpaca_storage::{ConversationMessage, ConversationRepository, Database};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Instant;
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
}

impl ChatService {
    pub fn new(
        gateway: Arc<Gateway>,
        stream_manager: Arc<ChatStreamManager>,
        db: Database,
    ) -> Self {
        Self {
            gateway,
            stream_manager,
            db,
        }
    }

    /// Send a message and start streaming the response.
    ///
    /// Returns immediately with a stream_id. The actual LLM call happens
    /// in a background task that sends events to the stream.
    pub fn send_message(&self, content: String, principal: &str) -> Result<ChatSendResponse> {
        let lane_key = format!("{principal}:gui");

        let (stream_id, _rx) = self.stream_manager.create_stream(&lane_key);

        // Emit Thinking event
        let _ = self.stream_manager.send(&stream_id, ChatStreamEvent::Thinking);

        // Spawn background task for the actual gateway call
        let gateway = self.gateway.clone();
        let stream_manager = self.stream_manager.clone();
        let sid = stream_id.clone();
        let user_content = content.clone();
        let principal_owned = principal.to_string();

        tokio::spawn(async move {
            // Give browser time to connect to SSE endpoint
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;

            let start = Instant::now();

            let response = gateway
                .handle_event(GatewayRequest {
                    source: EventSource::Gui {
                        connection_id: principal_owned.clone(),
                    },
                    content: user_content.clone(),
                    principal: Principal::User { global_id: principal_owned.clone() },
                    scope: Scope::Global,
                })
                .await;

            let duration_ms = start.elapsed().as_millis() as u64;

            // Note: Message persistence is now handled by Gateway (GatewayPersistence).
            // ChatService only manages the SSE stream events.

            let is_error = response.content.starts_with("Error:");

            if is_error {
                let _ = stream_manager.send(
                    &sid,
                    ChatStreamEvent::Error {
                        message: response.content,
                    },
                );
            } else {
                let _ = stream_manager.send(
                    &sid,
                    ChatStreamEvent::Done {
                        content: response.content,
                        model: "default".to_string(),
                        tokens_in: 0,
                        tokens_out: 0,
                        duration_ms,
                    },
                );
            }

            info!("Chat stream {sid} completed in {duration_ms}ms");

            // Delay removal to allow late SSE subscribers
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
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
