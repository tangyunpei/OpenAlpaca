use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;

use crate::openai_compat::openai_chat_completions;
use crate::state::GatewayState;
use crate::ws::ws_upgrade_handler;

/// Build the axum router with all HTTP routes.
pub fn build_router(state: Arc<GatewayState>) -> Router {
    Router::new()
        .route("/health", get(health_handler))
        .route("/healthz", get(health_handler))
        .route("/ready", get(ready_handler))
        .route("/v1/chat/completions", post(openai_chat_completions))
        .route("/ws", get(ws_upgrade_handler))
        .layer(TraceLayer::new_for_http())
        .layer(CorsLayer::permissive())
        .with_state(state)
}

async fn health_handler() -> impl IntoResponse {
    (StatusCode::OK, Json(serde_json::json!({ "ok": true })))
}

async fn ready_handler(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let channels = state.channel_registry.read().await;
    let channel_count = channels.len();
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "ok": true,
            "channels": channel_count,
        })),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::test_helpers::make_test_state;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    #[tokio::test]
    async fn test_health_endpoint() {
        let state = make_test_state().await;
        let app = build_router(state);

        let request = Request::builder()
            .uri("/health")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["ok"], true);
    }

    #[tokio::test]
    async fn test_healthz_endpoint() {
        let state = make_test_state().await;
        let app = build_router(state);

        let request = Request::builder()
            .uri("/healthz")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_ready_endpoint() {
        let state = make_test_state().await;
        let app = build_router(state);

        let request = Request::builder()
            .uri("/ready")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["ok"], true);
    }

    #[tokio::test]
    async fn test_openai_completions_stub() {
        let state = make_test_state().await;
        let app = build_router(state);

        let body = serde_json::json!({
            "model": "test-model",
            "messages": [{"role": "user", "content": "hello"}]
        });

        let request = Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["object"], "chat.completion");
        assert_eq!(json["model"], "test-model");
    }

    #[tokio::test]
    async fn test_404_for_unknown_route() {
        let state = make_test_state().await;
        let app = build_router(state);

        let request = Request::builder()
            .uri("/nonexistent")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
