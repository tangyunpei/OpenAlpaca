use std::path::PathBuf;
use std::sync::Arc;

use axum::body::Body;
use axum::http::Request;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

use openalpaca_channels::EchoChannel;
use openalpaca_config::OpenAlpacaConfig;
use openalpaca_core::context::AppContext;
use openalpaca_core::types::AccountId;
use openalpaca_gateway::http::build_router;
use openalpaca_gateway::server::build_state_for_cli;

fn make_app_ctx() -> Arc<AppContext> {
    Arc::new(AppContext {
        config_path: PathBuf::from("/tmp/test-config.yaml"),
        state_dir: PathBuf::from("/tmp/test-state"),
        home_dir: PathBuf::from("/tmp"),
        http_client: reqwest::Client::new(),
        shutdown: CancellationToken::new(),
    })
}

#[tokio::test]
async fn test_ready_with_registered_channel() {
    let state = build_state_for_cli(
        make_app_ctx(),
        OpenAlpacaConfig::default(),
        PathBuf::from("/tmp/test-sessions.jsonl"),
    );

    {
        let mut registry = state.channel_registry.write().await;
        registry.register(AccountId("default".into()), Box::new(EchoChannel::new()));
    }

    let app = build_router(state);
    let request = Request::builder()
        .uri("/ready")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["ok"], true);
    assert_eq!(json["channels"], 1);
}

#[tokio::test]
async fn test_ready_channel_count_matches_registry() {
    let state = build_state_for_cli(
        make_app_ctx(),
        OpenAlpacaConfig::default(),
        PathBuf::from("/tmp/test-sessions.jsonl"),
    );

    {
        let mut registry = state.channel_registry.write().await;
        registry.register(AccountId("a1".into()), Box::new(EchoChannel::new()));
        registry.register(AccountId("a2".into()), Box::new(EchoChannel::new()));
    }

    let app = build_router(state);
    let request = Request::builder()
        .uri("/ready")
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["ok"], true);
    assert_eq!(json["channels"], 2);
}
