use crate::AppState;
use axum::http::StatusCode;
use axum::{
    Json,
    extract::{Path, State},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
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
    let config_repo = openalpaca_storage::ConfigRepository::new(&state.db);

    let response: Vec<ConnectorStatus> = statuses
        .into_iter()
        .map(|(id, status)| {
            let name = match id.as_str() {
                "telegram" => "Telegram",
                "imessage" => "iMessage",
                _ => &id,
            };
            // Token-optional connectors (iMessage) are always "configured" on their platform.
            // Token-required connectors are configured only if a token is set.
            let configured = match id.as_str() {
                "imessage" => true,
                _ => config_repo
                    .get(&format!("{}.token", id))
                    .ok()
                    .flatten()
                    .filter(|t| !t.is_empty())
                    .is_some(),
            };
            ConnectorStatus {
                id: id.clone(),
                name: name.to_string(),
                status,
                configured,
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

/// GET /v1/connectors/:id/settings
/// Get all settings for a connector (reads config keys with the connector prefix)
pub async fn connector_settings_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let config_repo = openalpaca_storage::ConfigRepository::new(&state.db);

    // Collect all config keys matching this connector's prefix
    let prefix = format!("{}.", id);
    let keys: Vec<&str> = openalpaca_storage::config_schema::CONFIG_KEYS
        .iter()
        .filter(|k| k.key.starts_with(&prefix) && k.key != format!("{}.token", id) && k.key != format!("{}.enabled", id))
        .map(|k| k.key)
        .collect();

    let mut settings: HashMap<String, serde_json::Value> = HashMap::new();
    for key in keys {
        let value = config_repo
            .get_or_default(key)
            .ok()
            .flatten();
        settings.insert(
            key.to_string(),
            match value {
                Some(v) => serde_json::Value::String(v),
                None => serde_json::Value::Null,
            },
        );
    }

    Json(settings)
}

#[derive(Deserialize)]
pub struct ConnectorSettingsBody {
    pub settings: HashMap<String, String>,
}

/// PUT /v1/connectors/:id/settings
/// Update settings for a connector
pub async fn update_connector_settings_handler(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<ConnectorSettingsBody>,
) -> impl IntoResponse {
    let config_repo = openalpaca_storage::ConfigRepository::new(&state.db);
    let prefix = format!("{}.", id);

    for (key, value) in &body.settings {
        // Validate key belongs to this connector
        if !key.starts_with(&prefix) {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": format!("Key '{}' does not belong to connector '{}'", key, id) })),
            )
                .into_response();
        }

        // Validate against schema
        let def = openalpaca_storage::config_schema::lookup(key);
        if def.is_none() {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": format!("Unknown config key: {}", key) })),
            )
                .into_response();
        }

        let def = def.unwrap();
        let kind = def.kind.as_db_kind();
        if let Err(e) = config_repo.set(key, value, kind) {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": format!("Failed to set {}: {}", key, e) })),
            )
                .into_response();
        }
    }

    (StatusCode::OK, Json(serde_json::json!({ "status": "ok" }))).into_response()
}
