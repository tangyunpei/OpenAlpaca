//! History endpoint for querying persisted events
//!
//! GET /v1/events/history - Query historical events (requires Bearer token)

use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use openalpaca_storage::repository::EventLogRepository;
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;

/// Query parameters for history endpoint
#[derive(Debug, Deserialize)]
pub struct HistoryParams {
    pub limit: Option<usize>,
    pub agent_id: Option<String>,
}

/// Handle GET /v1/events/history
pub async fn events_history_handler(
    State(state): State<Arc<AppState>>,
    Query(params): Query<HistoryParams>,
) -> impl IntoResponse {
    let limit = params.limit.unwrap_or(50).min(1000); // Cap at 1000
    let repo = EventLogRepository::new(&state.db);

    let result = if let Some(agent_id) = &params.agent_id {
        repo.by_agent(agent_id, limit)
    } else {
        repo.recent(limit)
    };

    match result {
        Ok(events) => (StatusCode::OK, Json(events)).into_response(),
        Err(e) => {
            tracing::error!("Failed to query event history: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": "Failed to query event history",
                    "details": e.to_string()
                })),
            )
                .into_response()
        }
    }
}
