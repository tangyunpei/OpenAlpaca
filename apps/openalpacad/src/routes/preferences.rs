//! Preference management endpoints
//!
//! GET    /v1/preferences        -> list all preferences for local user
//! GET    /v1/preferences/{key}  -> get a single preference
//! PUT    /v1/preferences/{key}  -> set (upsert) a preference
//! DELETE /v1/preferences/{key}  -> delete a preference

use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use openalpaca_storage::PreferenceRepository;

use crate::AppState;

// ── Request / Response Types ──────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct SetPreferenceRequest {
    pub value: String,
    pub version: Option<i64>, // For optimistic locking
}

#[derive(Debug, Serialize)]
pub struct PreferenceResponse {
    pub key: String,
    pub value: String,
    pub version: i64,
}

#[derive(Debug, Serialize)]
pub struct PreferenceListResponse {
    pub preferences: Vec<PreferenceResponse>,
    pub count: usize,
}

// ── Handlers ──────────────────────────────────────────────────────

/// GET /v1/preferences
pub async fn list_preferences_handler(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let repo = PreferenceRepository::new(&state.db);

    match repo.list_for_user(&state.local_user_id) {
        Ok(prefs) => {
            let items: Vec<PreferenceResponse> = prefs
                .into_iter()
                .map(|p| PreferenceResponse {
                    key: p.key,
                    value: p.value,
                    version: p.version,
                })
                .collect();
            let count = items.len();
            (
                StatusCode::OK,
                Json(serde_json::to_value(PreferenceListResponse {
                    preferences: items,
                    count,
                }).unwrap()),
            )
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        ),
    }
}

/// GET /v1/preferences/{key}
pub async fn get_preference_handler(
    State(state): State<Arc<AppState>>,
    Path(key): Path<String>,
) -> impl IntoResponse {
    let repo = PreferenceRepository::new(&state.db);

    match repo.get(&state.local_user_id, &key) {
        Ok(Some(pref)) => (
            StatusCode::OK,
            Json(serde_json::to_value(PreferenceResponse {
                key: pref.key,
                value: pref.value,
                version: pref.version,
            }).unwrap()),
        ),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": format!("Preference '{}' not found", key) })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        ),
    }
}

/// PUT /v1/preferences/{key}
pub async fn set_preference_handler(
    State(state): State<Arc<AppState>>,
    Path(key): Path<String>,
    Json(request): Json<SetPreferenceRequest>,
) -> impl IntoResponse {
    let repo = PreferenceRepository::new(&state.db);

    match repo.set(&state.local_user_id, &key, &request.value, request.version) {
        Ok(()) => {
            // Re-read to return current state
            match repo.get(&state.local_user_id, &key) {
                Ok(Some(pref)) => (
                    StatusCode::OK,
                    Json(serde_json::to_value(PreferenceResponse {
                        key: pref.key,
                        value: pref.value,
                        version: pref.version,
                    }).unwrap()),
                ),
                _ => (
                    StatusCode::OK,
                    Json(serde_json::json!({ "status": "ok" })),
                ),
            }
        }
        Err(e) => {
            let msg = e.to_string();
            if msg.contains("Optimistic lock conflict") {
                (
                    StatusCode::CONFLICT,
                    Json(serde_json::json!({ "error": msg })),
                )
            } else {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": msg })),
                )
            }
        }
    }
}

/// DELETE /v1/preferences/{key}
pub async fn delete_preference_handler(
    State(state): State<Arc<AppState>>,
    Path(key): Path<String>,
) -> impl IntoResponse {
    let repo = PreferenceRepository::new(&state.db);

    match repo.delete(&state.local_user_id, &key) {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({ "status": "deleted", "key": key })),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        ),
    }
}
