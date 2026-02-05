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
                Json(
                    serde_json::to_value(CommandResponse {
                        request_id,
                        status: "accepted".to_string(),
                    })
                    .unwrap(),
                ),
            )
        }
        "process" => {
            // Route through Gateway (Phase 4.3)
            use openalpaca_api::events::EventSource;
            use openalpaca_core::gateway::GatewayRequest;
            use openalpaca_core::security::policy::{Principal, Scope};

            let content = request
                .args
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("Hello from process command")
                .to_string();

            let response = state.gateway.handle_event(GatewayRequest {
                source: EventSource::Api {
                    request_id: request_id.clone(),
                },
                content,
                principal: Principal::System,
                scope: Scope::Global,
            }).await;

            // Check if the response is an error
            if response.content.starts_with("Error: ") {
                (
                    StatusCode::FORBIDDEN,
                    Json(serde_json::json!({
                        "request_id": request_id,
                        "status": "rejected",
                        "error": response.content.strip_prefix("Error: ").unwrap_or(&response.content)
                    })),
                )
            } else {
                state
                    .event_broadcaster
                    .command_received(&request_id, "process");
                (
                    StatusCode::OK,
                    Json(serde_json::json!({
                        "request_id": request_id,
                        "status": "completed",
                        "output": response.content
                    })),
                )
            }
        }
        // PR-2: /link command skeleton
        "link_generate" => {
            // Generate a link token for the current (local) user
            // In production, this would use the authenticated user's global_user_id
            use openalpaca_storage::IdentityRepository;
            use uuid::Uuid;

            let global_user_id = request
                .args
                .get("user_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| Uuid::new_v4().to_string());

            // Generate a 6-character alphanumeric token
            use rand::Rng;
            let mut rng = rand::rng();
            let token: String = (0..6)
                .map(|_| {
                    let idx = rng.random_range(0..36);
                    if idx < 10 {
                        (b'0' + idx as u8) as char
                    } else {
                        (b'A' + (idx - 10) as u8) as char
                    }
                })
                .collect();

            let identity_repo = IdentityRepository::new(&state.db);

            // Ensure global user exists or create one
            if identity_repo
                .get_global_user(&global_user_id)
                .unwrap()
                .is_none()
            {
                identity_repo
                    .create_global_user(&global_user_id, None)
                    .unwrap();
            }

            match identity_repo.create_link_token(&global_user_id, &token) {
                Ok(link_token) => {
                    state
                        .event_broadcaster
                        .command_received(&request_id, "link_generate");
                    tracing::info!(
                        "Generated link token '{}' for user '{}'",
                        token,
                        global_user_id
                    );
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({
                            "request_id": request_id,
                            "status": "ok",
                            "token": token,
                            "expires_at": link_token.expires_at.to_rfc3339(),
                            "user_id": global_user_id
                        })),
                    )
                }
                Err(e) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "request_id": request_id,
                        "status": "error",
                        "error": e.to_string()
                    })),
                ),
            }
        }
        "link_consume" => {
            // Consume a link token (for external identity linking)
            use openalpaca_storage::IdentityRepository;

            let token = request
                .args
                .get("token")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            let provider = request
                .args
                .get("provider")
                .and_then(|v| v.as_str())
                .unwrap_or("cli");

            let provider_user_id = request
                .args
                .get("provider_user_id")
                .and_then(|v| v.as_str())
                .unwrap_or(&request_id);

            let identity_repo = IdentityRepository::new(&state.db);

            match identity_repo.consume_link_token(token) {
                Ok(Some(global_user_id)) => {
                    // Create or get external identity and link it
                    let ext_identity = identity_repo
                        .get_or_create_external_identity(provider, provider_user_id, None)
                        .unwrap();

                    identity_repo
                        .link_external_identity(ext_identity.id, &global_user_id)
                        .unwrap();

                    state
                        .event_broadcaster
                        .command_received(&request_id, "link_consume");

                    tracing::info!(
                        "Linked {}:{} -> global_user:{}",
                        provider,
                        provider_user_id,
                        global_user_id
                    );

                    (
                        StatusCode::OK,
                        Json(serde_json::json!({
                            "request_id": request_id,
                            "status": "linked",
                            "global_user_id": global_user_id,
                            "provider": provider,
                            "provider_user_id": provider_user_id
                        })),
                    )
                }
                Ok(None) => (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({
                        "request_id": request_id,
                        "status": "invalid_token",
                        "error": "Token is invalid, expired, or already used"
                    })),
                ),
                Err(e) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "request_id": request_id,
                        "status": "error",
                        "error": e.to_string()
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
