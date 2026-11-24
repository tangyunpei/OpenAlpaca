//! The API contract's samples for `/v1/settings/llm*`, `/v1/models*`,
//! `/v1/orchestrator/config` and the usage reads (API-CONTRACT).
//!
//! A settings service over a seeded `llm.toml` with one key for an
//! OpenAI-compatible server and a switched-off Ollama, both on a port
//! nothing binds (a memory secret store, the binary's test master key), so
//! nothing reaches a model, a keychain or a credential file: Anthropic stays
//! off, since its discovery calls its own endpoint whatever `base_url`
//! says. Four bodies come from a service no unit test may run
//! — credential discovery reads the machine's credentials, the CLI-backend
//! probe searches its `PATH`, a key's validation calls the provider, and
//! the provider-usage and orchestrator-config reads need the gateway — so
//! their samples serialize the very type the handler serializes, built
//! here.

use super::*;

use crate::contract::{Samples, ok_body};
use crate::test_util::HomeStoreGuard;
use openalpaca_llm::config::settings_types::{AddKeyRequest, KeyInput, KeyValidationResult};
use openalpaca_storage::Database;
use openalpaca_storage::repository::llm_usage::LlmCallLog;

const CONFIG: &str = r#"[orchestrator]
model = "llama3.1"
fallback_models = ["gpt-sample"]

[providers.anthropic]
enabled = false
strategy = "round_robin"

[providers.openai]
enabled = true
base_url = "http://127.0.0.1:1/v1"
strategy = "round_robin"

[providers.ollama]
enabled = false
base_url = "http://127.0.0.1:1/v1"

[models."llama3.1"]
provider = "ollama"
context = 8192
"#;

pub(crate) async fn contract_samples(out: &mut Samples) {
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
    service
        .upsert_key(AddKeyRequest {
            provider: "openai".to_string(),
            key: KeyInput {
                id: Some("key-1".to_string()),
                secret: "sk-contract-sample-not-a-key".to_string(),
                tier: Some("build".to_string()),
                priority: Some("primary".to_string()),
                source: Some("manual".to_string()),
                notes: Some("a note".to_string()),
            },
        })
        .await
        .expect("add a key");

    let settings = serde_json::to_value(service.get_config().await.expect("config")).unwrap();
    out.response_with_maps("get-settings-llm", settings, &["providers"]);
    let health = serde_json::to_value(service.key_health().await).unwrap();
    out.response_with_maps("get-settings-llm-status", health, &[""]);
    let body = ok_body(provider_enabled_response(&service, "ollama", true).await).await;
    out.response("put-settings-llm-providers-provider-enabled", body);
    out.response(
        "get-models",
        serde_json::to_value(service.available_models()).unwrap(),
    );
    let body = ok_body(refresh_models_response(&service).await).await;
    out.response("post-models-refresh", body);

    let validation = KeyValidationResult {
        valid: true,
        tier: Some("build".to_string()),
        detected_source: Some("manual".to_string()),
        models_available: vec!["claude-haiku-4-5-20251001".to_string()],
        rate_limits: Some("50 rpm".to_string()),
        format_error: None,
    };
    out.response(
        "post-settings-llm-validate",
        serde_json::to_value(validation).unwrap(),
    );
    let credentials = vec![openalpaca_llm::DiscoveredCredentialInfo {
        source: openalpaca_llm::CredentialSource::ClaudeCode,
        provider: "anthropic".to_string(),
        status: "valid".to_string(),
        expires_at: Some(1),
        auto_refresh: true,
    }];
    out.response(
        "get-settings-llm-credentials",
        serde_json::to_value(&credentials).unwrap(),
    );
    out.response(
        "post-settings-llm-credentials-rescan",
        serde_json::to_value(&credentials).unwrap(),
    );
    let backends = vec![openalpaca_llm::CliBackendStatus {
        name: "claude-code".to_string(),
        available: true,
        path: Some("claude".to_string()),
        enabled: false,
    }];
    out.response(
        "get-settings-llm-cli-backends",
        serde_json::to_value(backends).unwrap(),
    );
    let usage = vec![openalpaca_llm::ProviderUsageSummary {
        provider: "anthropic".to_string(),
        total_cost_usd: 0.5,
        total_tokens: 10,
        total_requests: 1,
        health: "healthy".to_string(),
        external_usage: Some(openalpaca_llm::ExternalUsage {
            period: "month".to_string(),
            cost_usd: 1.0,
            token_count: 10,
            rate_limit_remaining: Some(5),
            fetched_at: "2026-11-16T12:00:00Z".to_string(),
            approximate: true,
        }),
    }];
    out.response(
        "get-settings-llm-providers-usage",
        serde_json::to_value(usage).unwrap(),
    );
    let (model, fallback_models) = service.get_orchestrator_config().expect("orchestrator");
    let orchestrator = OrchestratorConfigResponse {
        model,
        fallback_models,
        active_agents: 1,
        active_tasks: 1,
        daily_cost_usd: 0.5,
    };
    out.response(
        "get-orchestrator-config",
        serde_json::to_value(orchestrator).unwrap(),
    );

    let db = Database::open(&home.path().join("test.db")).expect("open db");
    openalpaca_storage::repository::LlmUsageRepository::new(&db)
        .insert_call_log(&LlmCallLog {
            id: None,
            timestamp: Utc::now(),
            agent_id: Some("orchestrator".to_string()),
            task_id: Some("task-1".to_string()),
            provider: "anthropic".to_string(),
            model: "claude-haiku-4-5-20251001".to_string(),
            key_id: Some("key-1".to_string()),
            input_tokens: 100,
            output_tokens: 50,
            cost_usd: 0.001,
            status: "success".to_string(),
            latency_ms: Some(250),
            error_message: Some("none".to_string()),
        })
        .expect("a call");
    openalpaca_storage::repository::LlmUsageRepository::new(&db)
        .record_and_log(
            "orchestrator",
            Some("task-1"),
            "anthropic",
            "claude-haiku-4-5-20251001",
            10,
            5,
            0.001,
            100,
            "success",
            None,
        )
        .expect("a logged call");
    let body = ok_body(llm_usage(
        &db,
        LlmUsageQuery {
            task_id: None,
            agent_id: None,
            key_id: None,
            limit: None,
        },
    ))
    .await;
    out.response("get-llm-usage", body);
    let body = ok_body(llm_usage_daily(
        &db,
        LlmUsageDailyQuery {
            agent_id: None,
            date: None,
            limit: None,
        },
    ))
    .await;
    out.response("get-llm-usage-daily", body);
    let summary = today_usage(
        &db,
        Utc::now(),
        UsageCaps {
            workflow_max_cost_usd: 5.0,
            agent_max_cost_usd: 1.0,
        },
    );
    out.response("get-usage-summary", serde_json::to_value(summary).unwrap());
}
