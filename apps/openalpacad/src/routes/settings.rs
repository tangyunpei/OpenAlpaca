use crate::AppState;
use axum::http::StatusCode;
use axum::{
    Json,
    extract::{Path, State},
    response::IntoResponse,
};
use openalpaca_llm::settings_service::{
    AddKeyRequest, ReorderKeysRequest, ValidateKeyRequest,
};
use std::sync::Arc;

fn settings_error(status: StatusCode, code: &str, message: &str) -> impl IntoResponse {
    (
        status,
        Json(serde_json::json!({
            "error": {
                "code": code,
                "status": status.as_u16(),
                "message": message
            }
        })),
    )
}

/// GET /v1/settings/llm — returns masked config
pub async fn get_llm_settings(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let service = match &state.llm_settings_service {
        Some(s) => s,
        None => return settings_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "LLM_NOT_CONFIGURED",
            "LLM router is not configured",
        ).into_response(),
    };

    match service.get_config().await {
        Ok(config) => (StatusCode::OK, Json(serde_json::to_value(config).unwrap())).into_response(),
        Err(e) => settings_error(StatusCode::INTERNAL_SERVER_ERROR, "DISK_WRITE_FAILED", &e).into_response(),
    }
}

/// PUT /v1/settings/llm — add/update key
pub async fn upsert_key(
    State(state): State<Arc<AppState>>,
    Json(body): Json<AddKeyRequest>,
) -> impl IntoResponse {
    let service = match &state.llm_settings_service {
        Some(s) => s,
        None => return settings_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "LLM_NOT_CONFIGURED",
            "LLM router is not configured",
        ).into_response(),
    };

    // Validate key format
    if body.key.secret.is_empty() {
        return settings_error(
            StatusCode::BAD_REQUEST,
            "INVALID_KEY_FORMAT",
            "Key secret cannot be empty",
        ).into_response();
    }

    match service.upsert_key(body).await {
        Ok(()) => {
            // Emit key status changed event
            state.event_broadcaster.key_status_changed(
                "", "", "added",
            );
            (StatusCode::OK, Json(serde_json::json!({ "status": "ok" }))).into_response()
        }
        Err(e) if e.contains("Encryption") => {
            settings_error(StatusCode::INTERNAL_SERVER_ERROR, "ENCRYPTION_FAILED", &e).into_response()
        }
        Err(e) => {
            settings_error(StatusCode::INTERNAL_SERVER_ERROR, "DISK_WRITE_FAILED", &e).into_response()
        }
    }
}

/// DELETE /v1/settings/llm/keys/{provider}/{key_id} — remove key
pub async fn delete_key(
    State(state): State<Arc<AppState>>,
    Path((provider, key_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let service = match &state.llm_settings_service {
        Some(s) => s,
        None => return settings_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "LLM_NOT_CONFIGURED",
            "LLM router is not configured",
        ).into_response(),
    };

    match service.remove_key(&provider, &key_id).await {
        Ok(()) => {
            state.event_broadcaster.key_status_changed(
                &provider, &key_id, "removed",
            );
            (StatusCode::OK, Json(serde_json::json!({ "status": "ok" }))).into_response()
        }
        Err(e) => {
            settings_error(StatusCode::INTERNAL_SERVER_ERROR, "DISK_WRITE_FAILED", &e).into_response()
        }
    }
}

/// PUT /v1/settings/llm/keys/reorder — reorder keys + set primary
pub async fn reorder_keys(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ReorderKeysRequest>,
) -> impl IntoResponse {
    let service = match &state.llm_settings_service {
        Some(s) => s,
        None => return settings_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "LLM_NOT_CONFIGURED",
            "LLM router is not configured",
        ).into_response(),
    };

    match service.reorder_keys(body).await {
        Ok(()) => {
            state.event_broadcaster.key_status_changed(
                "", "", "reordered",
            );
            (StatusCode::OK, Json(serde_json::json!({ "status": "ok" }))).into_response()
        }
        Err(e) => {
            settings_error(StatusCode::INTERNAL_SERVER_ERROR, "DISK_WRITE_FAILED", &e).into_response()
        }
    }
}

/// POST /v1/settings/llm/validate — test key validity
pub async fn validate_key(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ValidateKeyRequest>,
) -> impl IntoResponse {
    let service = match &state.llm_settings_service {
        Some(s) => s,
        None => return settings_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "LLM_NOT_CONFIGURED",
            "LLM router is not configured",
        ).into_response(),
    };

    if body.secret.is_empty() {
        return settings_error(
            StatusCode::BAD_REQUEST,
            "INVALID_KEY_FORMAT",
            "Key secret cannot be empty",
        ).into_response();
    }

    match service.validate_key(body).await {
        Ok(result) => (StatusCode::OK, Json(serde_json::to_value(result).unwrap())).into_response(),
        Err(e) => settings_error(StatusCode::GATEWAY_TIMEOUT, "KEY_VALIDATION_TIMEOUT", &e).into_response(),
    }
}

/// GET /v1/settings/llm/status — live health
pub async fn get_key_status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let service = match &state.llm_settings_service {
        Some(s) => s,
        None => return settings_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "LLM_NOT_CONFIGURED",
            "LLM router is not configured",
        ).into_response(),
    };

    let health = service.key_health().await;
    (StatusCode::OK, Json(serde_json::to_value(health).unwrap())).into_response()
}
