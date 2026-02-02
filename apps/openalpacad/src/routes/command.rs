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
    pub args: HashMap<String, serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
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
    State(_state): State<Arc<AppState>>,
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
