use crate::AppState;
use axum::http::StatusCode;
use axum::{
    Json,
    extract::{Path, State},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Serialize)]
pub struct ConnectorStatus {
    pub id: String,
    pub name: String,
    pub status: String,
    pub configured: bool,
}

#[derive(Deserialize)]
pub struct ConnectorActionBody {
    pub action: String,
}

/// GET /v1/connectors
/// List all connectors and their status
pub async fn list_connectors_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let statuses = state.connector_manager.list_status().await;

    let response: Vec<ConnectorStatus> = statuses
        .into_iter()
        .map(|(id, status)| {
            let name = match id.as_str() {
                "telegram" => "Telegram",
                "imessage" => "iMessage",
                _ => &id,
            };
            ConnectorStatus {
                id: id.clone(),
                name: name.to_string(),
                status,
                configured: true, // For now assume configured if we know about it
            }
        })
        .collect();

    Json(response)
}

/// POST /v1/connectors/:id/action
/// Perform action on a connector
pub async fn connector_action_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<ConnectorActionBody>,
) -> impl IntoResponse {
    match body.action.as_str() {
        "enable" => {
            if let Err(e) = state.connector_manager.enable(&id).await {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": e.to_string() })),
                )
                    .into_response();
            }
            (StatusCode::OK, Json(serde_json::json!({ "status": "ok" }))).into_response()
        }
        "disable" => {
            if let Err(e) = state.connector_manager.disable(&id).await {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": e.to_string() })),
                )
                    .into_response();
            }
            (StatusCode::OK, Json(serde_json::json!({ "status": "ok" }))).into_response()
        }
        "delete" => {
            if let Err(e) = state.connector_manager.delete(&id).await {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": e.to_string() })),
                )
                    .into_response();
            }
            (StatusCode::OK, Json(serde_json::json!({ "status": "ok" }))).into_response()
        }
        _ => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid action" })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
pub struct ConnectorConfigBody {
    pub token: String,
}

/// POST /v1/connectors/:id/config
/// Update connector configuration
pub async fn connector_config_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<ConnectorConfigBody>,
) -> impl IntoResponse {
    if let Err(e) = state
        .connector_manager
        .update_config(&id, &body.token)
        .await
    {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response();
    }
    (StatusCode::OK, Json(serde_json::json!({ "status": "ok" }))).into_response()
}
