//! `llm keys set-primary` and `llm keys reorder` (*Rev 142*'s item 7, (b)):
//! the bodies they send are taken by `PUT /v1/settings/llm/keys/reorder`.
//!
//! The CLI's test pins both bodies whole in
//! `apps/openalpaca/src/commands/llm_keys/keys-bodies.json`; this test reads
//! each as the route's own [`ReorderKeysRequest`] and drives it through
//! `reorder_keys_response`, the handler past its service lookup (an
//! `AppState` is not constructible in a unit test), over a settings
//! service on a seeded `llm.toml` with two `openai` keys on a port nothing
//! binds (a memory secret store, the binary's test master key; Anthropic
//! off), under the home-store guard: nothing reaches a model, a keychain or
//! a credential file.

use super::*;

use crate::test_util::HomeStoreGuard;
use openalpaca_llm::config::settings_types::{AddKeyRequest, KeyInput};

const BODIES: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../openalpaca/src/commands/llm_keys/keys-bodies.json"
);

const CONFIG: &str = r#"[orchestrator]
model = "gpt-sample"

[providers.anthropic]
enabled = false
strategy = "round_robin"

[providers.openai]
enabled = true
base_url = "http://127.0.0.1:1/v1"
strategy = "primary_fallback"
"#;

/// The `openai` keys as `GET /v1/settings/llm` lists them: id and priority,
/// in order.
async fn keys(service: &openalpaca_llm::LlmSettingsService) -> Vec<(String, String)> {
    let config = serde_json::to_value(service.get_config().await.expect("config")).unwrap();
    config["providers"]["openai"]["keys"]
        .as_array()
        .expect("the openai keys")
        .iter()
        .map(|k| {
            (
                k["id"].as_str().unwrap().to_string(),
                k["priority"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect()
}

fn owned(rows: &[(&str, &str)]) -> Vec<(String, String)> {
    rows.iter()
        .map(|(id, priority)| (id.to_string(), priority.to_string()))
        .collect()
}

#[tokio::test]
async fn the_cli_keys_bodies_are_taken_by_the_reorder_route() {
    let home = tempfile::TempDir::new().expect("home");
    let _env = HomeStoreGuard::set_with_master_key(home.path());
    let config = tempfile::TempDir::new().expect("config");
    let path = config.path().join("llm.toml");
    std::fs::write(&path, CONFIG).expect("seed llm.toml");
    let router = Arc::new(openalpaca_llm::build_router(&path).expect("router"));
    let service = openalpaca_llm::LlmSettingsService::new_with_secret_store(
        router,
        path.clone(),
        Arc::new(openalpaca_llm::MemorySecretStore::new()),
    )
    .expect("settings service");
    for (id, priority) in [("key-1", "primary"), ("key-2", "fallback")] {
        service
            .upsert_key(AddKeyRequest {
                provider: "openai".to_string(),
                key: KeyInput {
                    id: Some(id.to_string()),
                    secret: format!("sk-{id}-not-a-key"),
                    tier: None,
                    priority: Some(priority.to_string()),
                    source: Some("manual".to_string()),
                    notes: None,
                },
            })
            .await
            .expect("add a key");
    }
    let before = owned(&[("key-1", "primary"), ("key-2", "fallback")]);
    assert_eq!(keys(&service).await, before);

    let bodies: serde_json::Value =
        serde_json::from_slice(&std::fs::read(BODIES).expect("the CLI's pinned bodies")).unwrap();

    // `set-primary` read the order the route lists: it is the CLI's fixture.
    let order: Vec<String> = before.iter().map(|(id, _)| id.clone()).collect();
    assert_eq!(bodies["set_primary"]["key_order"], serde_json::json!(order));

    for (verb, after) in [
        (
            "set_primary",
            owned(&[("key-1", "fallback"), ("key-2", "primary")]),
        ),
        (
            "reorder",
            owned(&[("key-2", "primary"), ("key-1", "fallback")]),
        ),
    ] {
        let body = bodies[verb].clone();
        let request: ReorderKeysRequest = serde_json::from_value(body.clone())
            .unwrap_or_else(|e| panic!("the route refuses the CLI's {verb} body ({e}): {body}"));
        let response = reorder_keys_response(&service, request).await;
        assert_eq!(response.status(), StatusCode::OK, "{verb}: {body}");
        assert_eq!(keys(&service).await, after, "{verb}: {body}");
    }
}
