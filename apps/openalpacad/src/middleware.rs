//! Authentication middleware for HTTP endpoints
//!
//! Bearer token validation for protected routes.
//! Note: WebSocket endpoints handle auth internally (query token).

use axum::{
    extract::State,
    http::{Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::sync::Arc;

use crate::AppState;

/// Middleware to validate Bearer token for protected HTTP endpoints
pub async fn auth_middleware(
    State(state): State<Arc<AppState>>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let auth_header = request.headers().get("Authorization");

    match auth_header.and_then(|h| h.to_str().ok()) {
        Some(header) if header.starts_with("Bearer ") => {
            let token = &header[7..];
            if token == state.token {
                next.run(request).await
            } else {
                (StatusCode::UNAUTHORIZED, "Invalid token").into_response()
            }
        }
        _ => (
            StatusCode::UNAUTHORIZED,
            "Missing or invalid Authorization header",
        )
            .into_response(),
    }
}
