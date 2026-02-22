//! Dispatch decisions history endpoint
//!
//! GET /v1/orchestrator/decisions — Query dispatch decision history

use axum::{
    Json,
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
};
use openalpaca_storage::repository::dispatch_decision::DispatchDecisionRepository;
use serde::Deserialize;
use std::sync::Arc;

use crate::AppState;

/// Query parameters for decisions endpoint
#[derive(Debug, Deserialize)]
pub struct DecisionParams {
    pub mode: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub limit: Option<usize>,
}

/// Handle GET /v1/orchestrator/decisions
pub async fn dispatch_decisions_handler(
    State(state): State<Arc<AppState>>,
    Query(params): Query<DecisionParams>,
) -> impl IntoResponse {
    let limit = params.limit.unwrap_or(100).min(1000);
    let repo = DispatchDecisionRepository::new(&state.db);

    match repo.query(
        params.mode.as_deref(),
        params.from.as_deref(),
        params.to.as_deref(),
        limit,
    ) {
        Ok(records) => (
            StatusCode::OK,
            Json(serde_json::json!({ "records": records })),
        )
            .into_response(),
        Err(e) => {
            tracing::error!("Failed to query dispatch decisions: {}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({
                    "error": "Failed to query dispatch decisions",
                    "details": e.to_string()
                })),
            )
                .into_response()
        }
    }
}
