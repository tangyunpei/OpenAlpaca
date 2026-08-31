use crate::AppState;
use axum::http::StatusCode;
use axum::{
    Json,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use chrono::Utc;
use openalpaca_core::events::SystemEvent;
use openalpaca_llm::config::settings_service::{
    AddKeyRequest, OrchestratorConfigResponse, ReorderKeysRequest, SetKeyPriorityRequest,
    UpdateOrchestratorRequest, ValidateKeyRequest,
};
use std::sync::Arc;

use super::settings_types::*;

/// GET /v1/settings/llm — returns masked config
pub async fn get_llm_settings(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let service = match &state.llm_settings_service {
        Some(s) => s,
        None => {
            return settings_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "LLM_NOT_CONFIGURED",
                "LLM router is not configured",
            )
            .into_response();
        }
    };

    match service.get_config().await {
        Ok(config) => (StatusCode::OK, Json(serde_json::to_value(config).unwrap())).into_response(),
        Err(e) => settings_error(StatusCode::INTERNAL_SERVER_ERROR, "DISK_WRITE_FAILED", &e)
            .into_response(),
    }
}

/// PUT /v1/settings/llm — add/update key
pub async fn upsert_key(
    State(state): State<Arc<AppState>>,
    Json(body): Json<AddKeyRequest>,
) -> impl IntoResponse {
    let service = match &state.llm_settings_service {
        Some(s) => s,
        None => {
            return settings_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "LLM_NOT_CONFIGURED",
                "LLM router is not configured",
            )
            .into_response();
        }
    };

    // Validate key format
    if body.key.secret.is_empty() {
        return settings_error(
            StatusCode::BAD_REQUEST,
            "INVALID_KEY_FORMAT",
            "Key secret cannot be empty",
        )
        .into_response();
    }

    let event_provider = body.provider.clone();
    let event_key_id = body.key.id.clone().unwrap_or_default();
    match service.upsert_key(body).await {
        Ok(()) => {
            // Emit key status changed event via EventBus
            let _ = state.gateway.bus.publish(SystemEvent::KeyStatusChanged {
                provider: event_provider,
                key_id: event_key_id,
                status: "added".to_string(),
                timestamp: Utc::now(),
            });
            (StatusCode::OK, Json(serde_json::json!({ "status": "ok" }))).into_response()
        }
        Err(e) if e.contains("Encryption") => {
            settings_error(StatusCode::INTERNAL_SERVER_ERROR, "ENCRYPTION_FAILED", &e)
                .into_response()
        }
        Err(e) => settings_error(StatusCode::INTERNAL_SERVER_ERROR, "DISK_WRITE_FAILED", &e)
            .into_response(),
    }
}

/// DELETE /v1/settings/llm/keys/{provider}/{key_id} — remove key
pub async fn delete_key(
    State(state): State<Arc<AppState>>,
    Path((provider, key_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let service = match &state.llm_settings_service {
        Some(s) => s,
        None => {
            return settings_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "LLM_NOT_CONFIGURED",
                "LLM router is not configured",
            )
            .into_response();
        }
    };

    match service.remove_key(&provider, &key_id).await {
        Ok(()) => {
            let _ = state.gateway.bus.publish(SystemEvent::KeyStatusChanged {
                provider: provider.clone(),
                key_id: key_id.clone(),
                status: "removed".to_string(),
                timestamp: Utc::now(),
            });
            (StatusCode::OK, Json(serde_json::json!({ "status": "ok" }))).into_response()
        }
        Err(e) if e.contains("not found") => {
            settings_error(StatusCode::NOT_FOUND, "KEY_NOT_FOUND", &e).into_response()
        }
        Err(e) => settings_error(StatusCode::INTERNAL_SERVER_ERROR, "DISK_WRITE_FAILED", &e)
            .into_response(),
    }
}

/// PUT /v1/settings/llm/keys/reorder — reorder keys + set primary
pub async fn reorder_keys(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ReorderKeysRequest>,
) -> impl IntoResponse {
    let service = match &state.llm_settings_service {
        Some(s) => s,
        None => {
            return settings_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "LLM_NOT_CONFIGURED",
                "LLM router is not configured",
            )
            .into_response();
        }
    };

    let event_provider = body.provider.clone();
    match service.reorder_keys(body).await {
        Ok(()) => {
            let _ = state.gateway.bus.publish(SystemEvent::KeyStatusChanged {
                provider: event_provider,
                key_id: "*".to_string(),
                status: "reordered".to_string(),
                timestamp: Utc::now(),
            });
            (StatusCode::OK, Json(serde_json::json!({ "status": "ok" }))).into_response()
        }
        Err(e) => settings_error(StatusCode::INTERNAL_SERVER_ERROR, "DISK_WRITE_FAILED", &e)
            .into_response(),
    }
}

/// PUT /v1/settings/llm/keys/priority — set per-key priority
pub async fn set_key_priority(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SetKeyPriorityRequest>,
) -> impl IntoResponse {
    let service = match &state.llm_settings_service {
        Some(s) => s,
        None => {
            return settings_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "LLM_NOT_CONFIGURED",
                "LLM router is not configured",
            )
            .into_response();
        }
    };

    if body.priority != "primary" && body.priority != "fallback" {
        return settings_error(
            StatusCode::BAD_REQUEST,
            "INVALID_PRIORITY",
            "Priority must be 'primary' or 'fallback'",
        )
        .into_response();
    }

    let provider = body.provider.clone();
    let key_id = body.key_id.clone();

    match service.set_key_priority(body).await {
        Ok(()) => {
            let _ = state.gateway.bus.publish(SystemEvent::KeyStatusChanged {
                provider: provider.clone(),
                key_id: key_id.clone(),
                status: "priority_changed".to_string(),
                timestamp: Utc::now(),
            });
            (StatusCode::OK, Json(serde_json::json!({ "status": "ok" }))).into_response()
        }
        Err(e) if e.contains("not found") => {
            settings_error(StatusCode::NOT_FOUND, "KEY_NOT_FOUND", &e).into_response()
        }
        Err(e) => settings_error(StatusCode::INTERNAL_SERVER_ERROR, "DISK_WRITE_FAILED", &e)
            .into_response(),
    }
}

/// POST /v1/settings/llm/validate — test key validity
pub async fn validate_key(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ValidateKeyRequest>,
) -> impl IntoResponse {
    let service = match &state.llm_settings_service {
        Some(s) => s,
        None => {
            return settings_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "LLM_NOT_CONFIGURED",
                "LLM router is not configured",
            )
            .into_response();
        }
    };

    if body.secret.is_empty() {
        return settings_error(
            StatusCode::BAD_REQUEST,
            "INVALID_KEY_FORMAT",
            "Key secret cannot be empty",
        )
        .into_response();
    }

    match service.validate_key(body).await {
        Ok(result) => (StatusCode::OK, Json(serde_json::to_value(result).unwrap())).into_response(),
        Err(e) => settings_error(StatusCode::GATEWAY_TIMEOUT, "KEY_VALIDATION_TIMEOUT", &e)
            .into_response(),
    }
}

/// GET /v1/settings/llm/status — live health
pub async fn get_key_status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let service = match &state.llm_settings_service {
        Some(s) => s,
        None => {
            return settings_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "LLM_NOT_CONFIGURED",
                "LLM router is not configured",
            )
            .into_response();
        }
    };

    let health = service.key_health().await;
    (StatusCode::OK, Json(serde_json::to_value(health).unwrap())).into_response()
}

/// GET /v1/models — list all available models
pub async fn list_models(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let service = match &state.llm_settings_service {
        Some(s) => s,
        None => {
            return settings_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "LLM_NOT_CONFIGURED",
                "LLM router is not configured",
            )
            .into_response();
        }
    };

    let models = service.available_models();
    (StatusCode::OK, Json(serde_json::to_value(models).unwrap())).into_response()
}

/// POST /v1/models/refresh — refresh models from provider APIs
pub async fn refresh_models(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let service = match &state.llm_settings_service {
        Some(s) => s,
        None => {
            return settings_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "LLM_NOT_CONFIGURED",
                "LLM router is not configured",
            )
            .into_response();
        }
    };

    service.refresh_models().await;
    let models = service.available_models();
    (StatusCode::OK, Json(serde_json::to_value(models).unwrap())).into_response()
}

/// GET /v1/orchestrator/config
pub async fn get_orchestrator_config(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let service = match &state.llm_settings_service {
        Some(s) => s,
        None => {
            return settings_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "LLM_NOT_CONFIGURED",
                "LLM router is not configured",
            )
            .into_response();
        }
    };

    match service.get_orchestrator_config() {
        Ok((model, fallback_models)) => {
            let active_agents = state.gateway.shared_context.agent_registry.count();
            let active_tasks = state
                .gateway
                .shared_context
                .task_registry
                .list_active()
                .len();
            let daily_cost_usd = 0.0; // Cost tracker requires async access via LlmRouter

            let resp = OrchestratorConfigResponse {
                model,
                fallback_models,
                active_agents,
                active_tasks,
                daily_cost_usd,
            };
            (StatusCode::OK, Json(serde_json::to_value(resp).unwrap())).into_response()
        }
        Err(e) => settings_error(StatusCode::INTERNAL_SERVER_ERROR, "CONFIG_READ_FAILED", &e)
            .into_response(),
    }
}

/// PUT /v1/orchestrator/config
pub async fn update_orchestrator_config(
    State(state): State<Arc<AppState>>,
    Json(body): Json<UpdateOrchestratorRequest>,
) -> impl IntoResponse {
    let service = match &state.llm_settings_service {
        Some(s) => s,
        None => {
            return settings_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "LLM_NOT_CONFIGURED",
                "LLM router is not configured",
            )
            .into_response();
        }
    };

    let model_name = body.model.clone();
    match service.update_orchestrator_config(body) {
        Ok(()) => {
            let _ = state
                .gateway
                .bus
                .publish(SystemEvent::OrchestratorConfigChanged {
                    model: model_name,
                    timestamp: Utc::now(),
                });
            (StatusCode::OK, Json(serde_json::json!({ "status": "ok" }))).into_response()
        }
        Err(e) => settings_error(StatusCode::INTERNAL_SERVER_ERROR, "DISK_WRITE_FAILED", &e)
            .into_response(),
    }
}

// ── LLM Usage endpoints ──────────────────────────────────────────

/// GET /v1/llm/usage — query LLM call logs
pub async fn get_llm_usage(
    State(state): State<Arc<AppState>>,
    Query(query): Query<LlmUsageQuery>,
) -> impl IntoResponse {
    let repo = openalpaca_storage::repository::LlmUsageRepository::new(&state.db);
    let limit = query.limit.unwrap_or(50).min(1000);

    let result = if let Some(ref agent_id) = query.agent_id {
        repo.get_agent_usage(agent_id, limit)
    } else if let Some(ref key_id) = query.key_id {
        repo.get_usage_by_key(key_id, limit)
    } else {
        repo.get_all_usage(limit)
    };

    match result {
        Ok(logs) => (StatusCode::OK, Json(serde_json::to_value(logs).unwrap())).into_response(),
        Err(e) => settings_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "USAGE_QUERY_FAILED",
            &e.to_string(),
        )
        .into_response(),
    }
}

/// GET /v1/llm/usage/daily — query daily usage aggregates
pub async fn get_llm_usage_daily(
    State(state): State<Arc<AppState>>,
    Query(query): Query<LlmUsageDailyQuery>,
) -> impl IntoResponse {
    let repo = openalpaca_storage::repository::LlmUsageRepository::new(&state.db);
    let limit = query.limit.unwrap_or(30).min(365);

    let result =
        repo.query_daily_usage(query.agent_id.as_deref(), query.date.as_deref(), limit);

    match result {
        Ok(usage) => (StatusCode::OK, Json(serde_json::to_value(usage).unwrap())).into_response(),
        Err(e) => settings_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "USAGE_QUERY_FAILED",
            &e.to_string(),
        )
        .into_response(),
    }
}

// ── Credential Discovery endpoints ──────────────────────────────────

/// GET /v1/settings/llm/credentials — list discovered credentials
pub async fn get_discovered_credentials(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let tm = match &state.token_manager {
        Some(tm) => tm,
        None => {
            return (StatusCode::OK, Json(serde_json::json!([]))).into_response();
        }
    };

    let creds = tm.discovered_sources().await;
    (StatusCode::OK, Json(serde_json::to_value(creds).unwrap())).into_response()
}

/// POST /v1/settings/llm/credentials/rescan — rescan credentials
pub async fn rescan_credentials(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let tm = match &state.token_manager {
        Some(tm) => tm,
        None => {
            return settings_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "CREDENTIAL_DISCOVERY_NOT_CONFIGURED",
                "Credential discovery is not enabled",
            )
            .into_response();
        }
    };

    let service = match &state.llm_settings_service {
        Some(s) => s,
        None => {
            return settings_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "LLM_NOT_CONFIGURED",
                "LLM router is not configured",
            )
            .into_response();
        }
    };

    let router = service.router();
    let creds = tm.rescan(service, router).await;
    (StatusCode::OK, Json(serde_json::to_value(creds).unwrap())).into_response()
}

/// GET /v1/settings/llm/cli-backends — list CLI backend status
pub async fn get_cli_backends(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let cli_config = load_cli_backends_config(&state.llm_config_path);

    let statuses = openalpaca_llm::detect_cli_backends(&cli_config);
    (
        StatusCode::OK,
        Json(serde_json::to_value(statuses).unwrap()),
    )
        .into_response()
}

/// GET /v1/settings/llm/providers/usage — provider-level usage summaries
pub async fn get_provider_usage(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let service = match &state.llm_settings_service {
        Some(s) => s,
        None => {
            return settings_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "LLM_NOT_CONFIGURED",
                "LLM router is not configured",
            )
            .into_response();
        }
    };

    let router = service.router();
    let provider_usage = router.cost_tracker.all_provider_usage().await;

    let mut summaries: Vec<openalpaca_llm::ProviderUsageSummary> = Vec::new();
    for (provider_name, stats) in &provider_usage {
        summaries.push(openalpaca_llm::ProviderUsageSummary {
            provider: provider_name.clone(),
            total_cost_usd: stats.total_cost_usd,
            total_tokens: stats.total_input_tokens + stats.total_output_tokens,
            total_requests: stats.total_requests,
            health: "healthy".to_string(),
            external_usage: None,
        });
    }

    // Add providers with no usage yet
    for provider_type in router.configured_providers() {
        let name = provider_type.to_string();
        if !provider_usage.contains_key(&name) {
            summaries.push(openalpaca_llm::ProviderUsageSummary {
                provider: name,
                total_cost_usd: 0.0,
                total_tokens: 0,
                total_requests: 0,
                health: "healthy".to_string(),
                external_usage: None,
            });
        }
    }

    (
        StatusCode::OK,
        Json(serde_json::to_value(summaries).unwrap()),
    )
        .into_response()
}

// ── Daemon config (providers) endpoints ─────────────────────────────

/// GET /v1/daemon/config/providers — read web search provider configuration (from llm.toml)
pub async fn get_daemon_providers(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let ws = state.web_search_config.load();

    let hint = if ws.api_key.len() > 4 {
        format!("****{}", &ws.api_key[ws.api_key.len() - 4..])
    } else if !ws.api_key.is_empty() {
        "****".to_string()
    } else {
        String::new()
    };

    let resp = DaemonProvidersResponse {
        web_search: WebSearchConfigResponse {
            api_key_configured: !ws.api_key.is_empty(),
            api_key_hint: hint,
            timeout_secs: ws.timeout_secs,
        },
    };

    (StatusCode::OK, Json(serde_json::to_value(resp).unwrap())).into_response()
}

/// PUT /v1/daemon/config/providers/web-search — update web search provider config (writes to llm.toml)
pub async fn update_web_search_config(
    State(state): State<Arc<AppState>>,
    Json(body): Json<UpdateWebSearchRequest>,
) -> impl IntoResponse {
    // Load current LLM config from disk to preserve all fields
    let mut llm_cfg = match openalpaca_llm::read_config(&state.llm_config_path) {
        Ok(c) => c,
        Err(e) => {
            return settings_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "CONFIG_READ_FAILED",
                &format!("Failed to read llm.toml: {}", e),
            )
            .into_response();
        }
    };

    // Ensure web_search section exists
    let ws = llm_cfg.web_search.get_or_insert_with(Default::default);

    // Apply patches
    if let Some(ref key) = body.api_key {
        ws.api_key = key.clone();
    }
    if let Some(timeout) = body.timeout_secs {
        ws.timeout_secs = timeout;
    }

    // Snapshot the updated config before releasing the mutable borrow.
    let updated_ws = ws.clone();

    // Write back to llm.toml
    match openalpaca_llm::write_config(&state.llm_config_path, &llm_cfg) {
        Ok(()) => {}
        Err(e) => {
            return settings_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "DISK_WRITE_FAILED",
                &format!("Failed to write llm.toml: {}", e),
            )
            .into_response();
        }
    }

    // Immediately update the in-memory ArcSwap so subsequent GET reads
    // return the fresh value without waiting for the file-watcher hot-reload.
    state
        .web_search_config
        .store(std::sync::Arc::new(updated_ws));

    // Also publish event for GUI reactivity (WebSocket push).
    let _ = state.gateway.bus.publish(SystemEvent::DaemonConfigChanged {
        timestamp: Utc::now(),
    });

    (StatusCode::OK, Json(serde_json::json!({ "status": "ok" }))).into_response()
}

#[cfg(test)]
mod tests {
    use super::load_cli_backends_config;
    use std::fs;
    use std::path::PathBuf;

    fn temp_dir(prefix: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("{prefix}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("temp dir should be creatable");
        dir
    }

    #[test]
    fn cli_backends_config_uses_explicit_llm_config_path() {
        let dir = temp_dir("openalpacad-cli-backends");
        let llm_path = dir.join("llm.toml");
        fs::write(
            &llm_path,
            r#"
[cli_backends.claude_code]
enabled = false
"#,
        )
        .expect("llm.toml should be writable");

        let cfg = load_cli_backends_config(&llm_path);
        let claude = cfg.claude_code.expect("claude config should be present");
        assert_eq!(claude.enabled, Some(false));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn cli_backends_config_returns_default_when_missing() {
        let dir = temp_dir("openalpacad-cli-backends-missing");
        let missing = dir.join("missing.toml");

        let cfg = load_cli_backends_config(&missing);
        assert!(cfg.claude_code.is_none());
        assert!(cfg.codex.is_none());

        let _ = fs::remove_dir_all(dir);
    }
}
