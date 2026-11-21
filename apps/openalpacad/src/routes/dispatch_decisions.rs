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
    decision_records(&state.db, params)
}

/// `GET /v1/orchestrator/decisions`' answer (split out for the API
/// contract's sample, API-CONTRACT).
fn decision_records(
    db: &openalpaca_storage::Database,
    params: DecisionParams,
) -> axum::response::Response {
    let limit = params.limit.unwrap_or(100).min(1000);
    let repo = DispatchDecisionRepository::new(db);

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

/// The decisions read's sample for the API contract's folder (API-CONTRACT):
/// one recorded dispatch.
#[cfg(test)]
pub(crate) async fn contract_samples(out: &mut crate::contract::Samples) {
    use openalpaca_storage::repository::dispatch_decision::DispatchDecisionRecord;
    let dir = tempfile::tempdir().expect("tempdir");
    let db = openalpaca_storage::Database::open(&dir.path().join("test.db")).expect("db");
    DispatchDecisionRepository::new(&db)
        .record(&DispatchDecisionRecord {
            id: None,
            request_id: "req-1".to_string(),
            task_id: Some("task-1".to_string()),
            mode: "lead_agent".to_string(),
            reason: "a run".to_string(),
            agent_count: 1,
            dag_node_count: Some(1),
            predictability_score: Some(0.5),
            error_message: Some("none".to_string()),
            timestamp: None,
        })
        .expect("record");
    let params = DecisionParams {
        mode: None,
        from: None,
        to: None,
        limit: None,
    };
    let body = crate::contract::ok_body(decision_records(&db, params)).await;
    out.response("get-orchestrator-decisions", body);
}
