//! Command endpoint for executing daemon commands
//!
//! POST /v1/command - Execute a command (requires Bearer token)

use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};
use uuid::Uuid;

use crate::AppState;

/// Command request body
#[derive(Debug, Deserialize)]
pub struct CommandRequest {
    pub command: String,
    #[serde(default)]
    #[allow(dead_code)]
    pub args: HashMap<String, serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[allow(dead_code)]
    pub target_agent: Option<String>,
}

/// Command response
#[derive(Debug, Serialize)]
pub struct CommandResponse {
    pub request_id: String,
    pub status: String,
}

/// Handle POST /v1/command
///
/// Currently implements echo for testing; will be extended for real commands.
pub async fn command_handler(
    State(state): State<Arc<AppState>>,
    Json(request): Json<CommandRequest>,
) -> impl IntoResponse {
    let request_id = Uuid::new_v4().to_string();

    tracing::info!(
        request_id = %request_id,
        command = %request.command,
        "Command received"
    );

    // Phase 1: Echo command for testing
    match request.command.as_str() {
        "echo" => {
            // Broadcast and persist the command event
            state
                .event_broadcaster
                .command_received(&request_id, &request.command);

            let response = CommandResponse {
                request_id,
                status: "accepted".to_string(),
            };
            (StatusCode::ACCEPTED, Json(response))
        }
        _ => {
            let response = CommandResponse {
                request_id,
                status: "rejected".to_string(),
            };
            (StatusCode::BAD_REQUEST, Json(response))
        }
    }
}
