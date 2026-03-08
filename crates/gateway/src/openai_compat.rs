use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};

use crate::state::GatewayState;

#[derive(Debug, Deserialize)]
pub struct ChatCompletionRequest {
    pub model: Option<String>,
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub stream: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Serialize)]
pub struct ChatCompletionResponse {
    pub id: String,
    pub object: String,
    pub created: u64,
    pub model: String,
    pub choices: Vec<Choice>,
}

#[derive(Debug, Serialize)]
pub struct Choice {
    pub index: u32,
    pub message: ChatMessage,
    pub finish_reason: String,
}

/// POST /v1/chat/completions
/// Phase 4 stub — returns a placeholder response. Phase 6 wires to agent runtime.
pub async fn openai_chat_completions(
    State(_state): State<Arc<GatewayState>>,
    Json(body): Json<ChatCompletionRequest>,
) -> impl IntoResponse {
    let model = body.model.unwrap_or_else(|| "stub".to_string());
    let response = ChatCompletionResponse {
        id: format!("chatcmpl-{}", uuid::Uuid::new_v4()),
        object: "chat.completion".to_string(),
        created: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
        model,
        choices: vec![Choice {
            index: 0,
            message: ChatMessage {
                role: "assistant".to_string(),
                content: "This is a stub response. The agent runtime is not yet connected."
                    .to_string(),
            },
            finish_reason: "stop".to_string(),
        }],
    };

    (StatusCode::OK, Json(response))
}
