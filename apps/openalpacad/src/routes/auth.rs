use crate::AppState;
use axum::http::StatusCode;
use axum::{Json, extract::State, response::IntoResponse};
use openalpaca_storage::IdentityRepository;
use rand::RngExt;
use rand::distr::Alphanumeric;
use serde::Serialize;
use std::sync::Arc;

#[derive(Serialize)]
pub struct LinkTokenResponse {
    pub token: String,
}

/// POST /v1/auth/link
/// Generate a new link token for the default user (mock for now or from auth)
pub async fn generate_link_token_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let repo = IdentityRepository::new(&state.db);

    // Generate a secure random token
    let token: String = rand::rng()
        .sample_iter(&Alphanumeric)
        .take(8)
        .map(char::from)
        .collect();
    let token = token.to_uppercase();

    // For now, we use a default "admin" user or similar if not specified
    // In a real system, this would come from the verified session user
    let global_user_id = "admin"; // Mock default user

    // Ensure admin user exists to satisfy foreign key constraints
    if repo
        .get_global_user(global_user_id)
        .ok()
        .flatten()
        .is_none()
    {
        let _ = repo.create_global_user(global_user_id, Some("Administrator"));
    }

    match repo.create_link_token(global_user_id, &token) {
        Ok(_) => (StatusCode::OK, Json(LinkTokenResponse { token })).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}
