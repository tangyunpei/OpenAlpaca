//! Plugin management endpoints
//!
//! GET  /v1/plugins              -> list all plugins
//! POST /v1/plugins/{name}/approve  -> approve a plugin
//! POST /v1/plugins/{name}/deny     -> deny a plugin
//! POST /v1/plugins/{name}/enable   -> enable a plugin
//! POST /v1/plugins/{name}/disable  -> disable a plugin
//! POST /v1/plugins/{name}/config   -> set a config key

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use openalpaca_core::tools::extensions::{ExtensionError, ExtensionId, ExtensionSupervisor};
use openalpaca_plugins::PluginError;
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;

// ── Request types ────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct SetConfigRequest {
    pub key: String,
    pub value: serde_json::Value,
}

// ── Error mapping ────────────────────────────────────────────────

/// HTTP status for a failed plugin lifecycle call.
///
/// A failed *write* is the daemon's fault, not the caller's: the plugin root
/// is unwritable, `.permissions.toml` is a directory, the disk is full. Design
/// §3.2 (W-deny) requires a consent decision that cannot be persisted to
/// answer `500` and change nothing — reporting it as `400` would tell the
/// client to fix a request that was never the problem.
///
/// Everything else stays `400`: an unknown plugin name, a load refused because
/// the plugin still holds a live handle, a manifest that will not parse.
fn plugin_error_status(error: &PluginError) -> StatusCode {
    match error {
        PluginError::Io(_) | PluginError::Json(_) | PluginError::StoreWriteFailed(_) => {
            StatusCode::INTERNAL_SERVER_ERROR
        }
        PluginError::StoreUnreadable(_) => StatusCode::CONFLICT,
        _ => StatusCode::BAD_REQUEST,
    }
}

/// HTTP status for a supervisor-level refusal (extension design §4, §8).
///
/// The status codes are the route's, not the supervisor's — C6 states the same
/// mapping on `/v1/extensions`; this is the legacy route wearing it until C7
/// deletes it. A **failed write** is `500` and took no transition; an
/// **unreadable store** is `409 store_unreadable` and took none either.
fn extension_error_status(error: &ExtensionError) -> StatusCode {
    match error {
        ExtensionError::NotFound(_) => StatusCode::NOT_FOUND,
        ExtensionError::NotLoaded
        | ExtensionError::StoreUnreadable(_)
        | ExtensionError::UnsupportedForKind
        | ExtensionError::NotOrphaned => StatusCode::CONFLICT,
        ExtensionError::WriteFailed(_) | ExtensionError::Internal(_) => {
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

// ── Handlers ─────────────────────────────────────────────────────

/// GET /v1/plugins
pub async fn list_plugins_handler(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let Some(ref pm) = state.plugin_manager else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "Plugin manager not available" })),
        );
    };

    let plugins = pm.list_plugins().await;
    let items: Vec<serde_json::Value> = plugins
        .into_iter()
        .map(|p| {
            serde_json::json!({
                "name": p.name,
                "version": p.version,
                "status": p.status,
                "tools": p.tools,
                "connector": p.connector,
                "provider": p.provider,
                "models": p.models,
            })
        })
        .collect();

    (StatusCode::OK, Json(serde_json::json!(items)))
}

/// POST /v1/plugins/{name}/approve
pub async fn approve_plugin_handler(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let Some(ref pm) = state.plugin_manager else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "Plugin manager not available" })),
        );
    };

    match pm.approve_plugin(&name).await {
        Ok(record) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "status": record.state.word(),
                "name": name,
            })),
        ),
        Err(e) => (
            extension_error_status(&e),
            Json(serde_json::json!({ "error": e.to_string() })),
        ),
    }
}

/// POST /v1/plugins/{name}/deny
pub async fn deny_plugin_handler(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let Some(ref pm) = state.plugin_manager else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "Plugin manager not available" })),
        );
    };

    match pm.deny_plugin(&name).await {
        Ok(_) => (
            StatusCode::OK,
            Json(serde_json::json!({ "status": "denied", "name": name })),
        ),
        Err(e) => (
            extension_error_status(&e),
            Json(serde_json::json!({ "error": e.to_string() })),
        ),
    }
}

/// POST /v1/plugins/{name}/enable
pub async fn enable_plugin_handler(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let Some(ref pm) = state.plugin_manager else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "Plugin manager not available" })),
        );
    };

    match pm.enable(&ExtensionId::plugin(name.clone())).await {
        Ok(record) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "status": record.state.word(),
                "name": name,
            })),
        ),
        Err(e) => (
            extension_error_status(&e),
            Json(serde_json::json!({ "error": e.to_string() })),
        ),
    }
}

/// POST /v1/plugins/{name}/disable
pub async fn disable_plugin_handler(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
) -> impl IntoResponse {
    let Some(ref pm) = state.plugin_manager else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "Plugin manager not available" })),
        );
    };

    match pm.disable(&ExtensionId::plugin(name.clone())).await {
        Ok(record) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "status": record.state.word(),
                "name": name,
            })),
        ),
        Err(e) => (
            extension_error_status(&e),
            Json(serde_json::json!({ "error": e.to_string() })),
        ),
    }
}

/// POST /v1/plugins/{name}/config
pub async fn set_plugin_config_handler(
    State(state): State<Arc<AppState>>,
    Path(name): Path<String>,
    Json(request): Json<SetConfigRequest>,
) -> impl IntoResponse {
    let Some(ref pm) = state.plugin_manager else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "Plugin manager not available" })),
        );
    };

    // Convert serde_json::Value to toml::Value
    let toml_value = json_to_toml(&request.value);

    match pm.set_plugin_config(&name, &request.key, toml_value).await {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "status": "ok",
                "name": name,
                "key": request.key,
            })),
        ),
        Err(e) => (
            plugin_error_status(&e),
            Json(serde_json::json!({ "error": e.to_string() })),
        ),
    }
}

/// Convert a `serde_json::Value` to a `toml::Value`.
fn json_to_toml(v: &serde_json::Value) -> toml::Value {
    match v {
        serde_json::Value::String(s) => toml::Value::String(s.clone()),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                toml::Value::Integer(i)
            } else if let Some(f) = n.as_f64() {
                toml::Value::Float(f)
            } else {
                toml::Value::String(n.to_string())
            }
        }
        serde_json::Value::Bool(b) => toml::Value::Boolean(*b),
        serde_json::Value::Array(arr) => {
            toml::Value::Array(arr.iter().map(json_to_toml).collect())
        }
        serde_json::Value::Object(obj) => {
            let mut map = toml::map::Map::new();
            for (k, v) in obj {
                map.insert(k.clone(), json_to_toml(v));
            }
            toml::Value::Table(map)
        }
        serde_json::Value::Null => toml::Value::String(String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use openalpaca_core::tools::ToolRegistry;
    use openalpaca_plugins::PluginManager;

    /// Design §3.2 W-deny: a consent decision that cannot be persisted must
    /// answer `500` and change nothing. The failure is the daemon's — here
    /// `.permissions.toml` is a directory, so every write to it fails — and
    /// reporting it as `400` tells the client to fix a request that was fine.
    ///
    /// Under C3 the refusal is the supervisor's `ExtensionError`, so this pins
    /// the mapping the route now owns.
    #[tokio::test]
    async fn a_denial_that_cannot_be_persisted_is_a_500() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let plugin_dir = tmp.path().join("some-plugin");
        std::fs::create_dir(&plugin_dir).unwrap();
        std::fs::write(
            plugin_dir.join("plugin.toml"),
            "[plugin]\nname = \"some-plugin\"\nversion = \"0.1.0\"\nentry = \"./nope\"\n",
        )
        .unwrap();

        let manager = PluginManager::new(
            tmp.path().to_path_buf(),
            Arc::new(ToolRegistry::new().unwrap()),
            None,
            None,
        );
        manager.start().await.unwrap();

        // Make the store unwritable — the writer cannot even create its lock
        // file — while leaving it readable, so this is a **write** failure and
        // not the `409 store_unreadable` of an unreadable one.
        let readonly = std::fs::Permissions::from_mode(0o555);
        std::fs::set_permissions(tmp.path(), readonly).unwrap();
        let error = manager.deny_plugin("some-plugin").await.unwrap_err();
        std::fs::set_permissions(tmp.path(), std::fs::Permissions::from_mode(0o755)).unwrap();

        assert!(
            matches!(error, ExtensionError::WriteFailed(_)),
            "expected the failed write to surface as WriteFailed, got {error:?}"
        );
        assert_eq!(
            extension_error_status(&error),
            StatusCode::INTERNAL_SERVER_ERROR,
            "a failed permissions write was reported as a client error"
        );
    }

    /// The other half of §4's split: a store nobody can *read* is
    /// `409 store_unreadable`, and no verb takes a transition over it.
    #[test]
    fn an_unreadable_store_is_a_409() {
        assert_eq!(
            extension_error_status(&ExtensionError::StoreUnreadable("bad toml".into())),
            StatusCode::CONFLICT
        );
        assert_eq!(
            extension_error_status(&ExtensionError::NotFound(ExtensionId::plugin("nope"))),
            StatusCode::NOT_FOUND
        );
    }

    /// The caller's own mistakes stay `400`, so the split is a real one.
    #[test]
    fn a_caller_mistake_is_still_a_400() {
        for error in [
            PluginError::Unavailable("no such plugin".into()),
            PluginError::HandleHeld("echo".into()),
            PluginError::InvalidManifest("bad toml".into()),
        ] {
            assert_eq!(plugin_error_status(&error), StatusCode::BAD_REQUEST);
        }
    }
}
