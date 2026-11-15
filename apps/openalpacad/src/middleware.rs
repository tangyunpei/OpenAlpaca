//! Authentication middleware for HTTP endpoints
//!
//! Bearer token validation for protected routes.
//! Note: WebSocket endpoints handle auth internally (query token).

use axum::{
    extract::State,
    http::{Request, StatusCode},
    middleware::Next,
    response::Response,
};
use std::sync::Arc;

use crate::discovery_rotation::{AcceptsToken, DaemonTokens};
use crate::routes::api_error;

/// Middleware to validate Bearer token for protected HTTP endpoints: the
/// token `discovery.json` names now, or the one it replaced until that one's
/// own expiry (T21 (2)).
pub async fn auth_middleware(
    State(tokens): State<Arc<DaemonTokens>>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let auth_header = request.headers().get("Authorization");

    match auth_header.and_then(|h| h.to_str().ok()) {
        Some(header) if header.starts_with("Bearer ") => {
            let token = &header[7..];
            if tokens.accepts_token(token) {
                next.run(request).await
            } else {
                api_error(StatusCode::UNAUTHORIZED, "unauthorized", "Invalid token")
            }
        }
        _ => api_error(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Missing or invalid Authorization header",
        ),
    }
}
