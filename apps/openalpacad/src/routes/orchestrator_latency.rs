//! Orchestrator latency metrics endpoint
//!
//! GET /v1/orchestrator/latency - Query orchestrator stage latencies

use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use openalpaca_storage::repository::orchestrator_latency::OrchestratorLatencyRepository;
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;

/// Query parameters for latency endpoint
#[derive(Debug, Deserialize)]
pub struct LatencyParams {
    pub mode: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub limit: Option<usize>,
}

/// Handle GET /v1/orchestrator/latency
pub async fn orchestrator_latency_handler(
    State(state): State<Arc<AppState>>,
    Query(params): Query<LatencyParams>,
) -> impl IntoResponse {
    let limit = params.limit.unwrap_or(100).min(1000);
    let repo = OrchestratorLatencyRepository::new(&state.db);

    let result = repo.query_by_mode(
        params.mode.as_deref(),
        params.from.as_deref(),
        params.to.as_deref(),
        limit,
    );

    match result {
        Ok(records) => (StatusCode::OK, Json(serde_json::json!({ "records": records }))).into_response(),
        Err(e) => {
            tracing::error!("Failed to query orchestrator latency: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": "Failed to query orchestrator latency",
                    "details": e.to_string()
                })),
            )
                .into_response()
        }
    }
}
