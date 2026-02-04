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

            (
                StatusCode::ACCEPTED,
                Json(serde_json::json!({
                    "request_id": request_id,
                    "status": "accepted"
                })),
            )
        }
        "process" => {
            // PR-1: Route through CoreCtx unified pipeline
            use openalpaca_core::middleware::prompt::AgentPersona;
            use openalpaca_core::security::policy::{Principal, Scope};

            let content = request
                .args
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("Hello from process command")
                .to_string();

            // Default to System principal (trusted, local call)
            let principal = Principal::System;
            let scope = Scope::Global;
            let agent_persona = AgentPersona {
                role: "Assistant".to_string(),
                tone: "Friendly".to_string(),
                domain_knowledge: vec![],
            };

            let req_uuid =
                uuid::Uuid::parse_str(&request_id).unwrap_or_else(|_| uuid::Uuid::new_v4());

            match state.core_ctx.handle_user_request(
                req_uuid,
                "http".to_string(),
                content,
                principal,
                scope,
                &agent_persona,
            ) {
                Ok(output) => {
                    state
                        .event_broadcaster
                        .command_received(&request_id, "process");
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({
                            "request_id": request_id,
                            "status": "completed",
                            "output": output.content
                        })),
                    )
                }
                Err(e) => (
                    StatusCode::FORBIDDEN,
                    Json(serde_json::json!({
                        "request_id": request_id,
                        "status": "rejected",
                        "error": e
                    })),
                ),
            }
        }
        "shutdown" => {
            tracing::info!("Shutdown command received via API");

            // Broadcast shutdown event
            state
                .event_broadcaster
                .command_received(&request_id, "shutdown");

            // Trigger shutdown signal (spawn a task to avoid blocking response)
            let shutdown_tx = state.shutdown_tx.clone();
            tokio::spawn(async move {
                // Determine small delay to allow response to be sent
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                if let Err(e) = shutdown_tx.send(()).await {
                    tracing::error!("Failed to send shutdown signal: {}", e);
                }
            });

            (
                StatusCode::OK,
                Json(serde_json::json!({
                    "request_id": request_id,
                    "status": "shutting_down"
                })),
            )
        }
        _ => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "request_id": request_id,
                "status": "rejected"
            })),
        ),
    }
}
