use crate::AppState;
use crate::managers::connector::ConnectorDetail;
use axum::http::StatusCode;
use axum::{
    Json,
    extract::{Path, State},
    response::IntoResponse,
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Serialize)]
pub struct ConnectorStatus {
    pub id: String,
    pub name: String,
    pub status: String,
    pub configured: bool,
    /// The message-attribution token this connector's traffic is filed under —
    /// the same string as `id`, carried explicitly because it is what
    /// `messages_7d` was grouped by (GAP-17, T49).
    pub source: String,
    /// Whether the connector manager holds a spawned handle for it. A plugin
    /// that *declares* a connector and never registers one is the design's
    /// `unwired` badge, which the client derives from the extension rows; this
    /// bit is the same question asked of a compiled-in connector.
    pub registered: bool,
    /// Messages attributed to this connector in the last seven UTC days.
    pub messages_7d: i64,
}

#[derive(Deserialize)]
pub struct ConnectorActionBody {
    pub action: String,
}

/// The `messages_7d` window, in `conversation_messages.created_at`'s own UTC
/// text form (`datetime('now')`, not RFC 3339).
fn seven_day_cutoff(now: DateTime<Utc>) -> String {
    (now - Duration::days(7))
        .format("%Y-%m-%d %H:%M:%S")
        .to_string()
}

/// Assemble one row from what the manager, the config and the grouped message
/// count each know. Pure, so the shape is tested without a daemon.
fn connector_row(
    detail: &ConnectorDetail,
    configured: bool,
    messages: &HashMap<String, i64>,
) -> ConnectorStatus {
    ConnectorStatus {
        id: detail.id.clone(),
        name: detail.name.clone(),
        status: detail.status.clone(),
        configured,
        source: detail.id.clone(),
        registered: detail.registered,
        messages_7d: messages.get(&detail.id).copied().unwrap_or(0),
    }
}

/// GET /v1/connectors
/// List all connectors and their status
pub async fn list_connectors_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let details = state.connector_manager.list_detail().await;
    let config_repo = openalpaca_storage::ConfigRepository::new(&state.db);

    // GAP-17/T49: one grouped count for the whole list, never one per row. A
    // read failure costs the counts, not the list — every row then reports the
    // `0` an empty message table would, so nothing renders as a made-up number.
    let messages = openalpaca_storage::repository::ConversationRepository::new(&state.db)
        .message_counts_by_source_since(&seven_day_cutoff(Utc::now()))
        .unwrap_or_else(|e| {
            tracing::warn!("Failed to read connector message counts: {e}");
            HashMap::new()
        });

    let response: Vec<ConnectorStatus> = details
        .iter()
        .map(|detail| {
            // Token-optional connectors (iMessage) are always "configured" on their platform.
            // Token-required connectors are configured only if a token is set.
            let configured = match detail.id.as_str() {
                "imessage" => true,
                id => config_repo
                    .get(&format!("{}.token", id))
                    .ok()
                    .flatten()
                    .filter(|t| !t.is_empty())
                    .is_some(),
            };
            connector_row(detail, configured, &messages)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::managers::connector::ConnectorDetail;
    use chrono::TimeZone;

    fn detail(id: &str, name: &str, registered: bool) -> ConnectorDetail {
        ConnectorDetail {
            id: id.to_string(),
            name: name.to_string(),
            status: "active".to_string(),
            registered,
        }
    }

    /// GAP-17/T49: the display name is the connector's own. The bug this
    /// replaces was a `match` on `telegram`/`imessage`, which left Discord
    /// rendering as `discord`.
    #[test]
    fn row_takes_its_name_from_the_connector_not_a_route_side_match() {
        let row = connector_row(&detail("discord", "Discord", true), true, &HashMap::new());
        assert_eq!(row.name, "Discord");
        assert_eq!(row.id, "discord");
    }

    /// `source` is the attribution token `messages_7d` is grouped by — the
    /// connector id — so the count and the thing it counted cannot drift.
    #[test]
    fn row_counts_the_messages_filed_under_its_own_source() {
        let counts = HashMap::from([
            ("telegram".to_string(), 184_i64),
            ("gui".to_string(), 12),
        ]);
        let row = connector_row(&detail("telegram", "Telegram", true), true, &counts);
        assert_eq!(row.source, "telegram");
        assert_eq!(row.messages_7d, 184);
    }

    /// A connector nobody messaged this week reports zero, not "unknown": an
    /// absent group is an honest zero.
    #[test]
    fn row_reports_zero_for_a_connector_with_no_traffic() {
        let row = connector_row(&detail("imessage", "iMessage", false), true, &HashMap::new());
        assert_eq!(row.messages_7d, 0);
    }

    /// `registered` is the manager's handle registry, not the config bit: a
    /// connector can be configured and still hold no spawned handle.
    #[test]
    fn row_carries_the_registry_bit_apart_from_configured() {
        let unregistered = connector_row(&detail("discord", "Discord", false), true, &HashMap::new());
        assert!(!unregistered.registered);
        assert!(unregistered.configured);

        let registered = connector_row(&detail("discord", "Discord", true), false, &HashMap::new());
        assert!(registered.registered);
        assert!(!registered.configured);
    }

    /// The window is seven days back in the table's own UTC text form.
    #[test]
    fn cutoff_is_seven_utc_days_back_in_the_tables_own_format() {
        let now = Utc.with_ymd_and_hms(2026, 9, 8, 4, 30, 0).unwrap();
        assert_eq!(seven_day_cutoff(now), "2026-09-01 04:30:00");
    }
}
