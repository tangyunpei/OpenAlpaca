use crate::AppState;
use axum::{extract::State, response::IntoResponse, Json};
use openalpaca_storage::SkillExecutionRepository;
use std::sync::Arc;

pub async fn skill_health_handler(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let repo = SkillExecutionRepository::new(&state.db);
    match repo.all_skill_health() {
        Ok(metrics) => Json(metrics).into_response(),
        Err(e) => {
            tracing::warn!("Failed to query skill health: {e}");
            (
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to query skill health: {e}"),
            )
                .into_response()
        }
    }
}
