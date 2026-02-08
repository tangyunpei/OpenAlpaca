use crate::AppState;
use axum::http::StatusCode;
use axum::{
    Json,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use serde::Deserialize;
use openalpaca_llm::settings_service::{
    AddKeyRequest, OrchestratorConfigResponse, ReorderKeysRequest, SetKeyPriorityRequest,
    UpdateOrchestratorRequest, ValidateKeyRequest,
};
use std::sync::Arc;

fn settings_error(status: StatusCode, code: &str, message: &str) -> impl IntoResponse {
    (
        status,
        Json(serde_json::json!({
            "error": {
                "code": code,
                "status": status.as_u16(),
                "message": message
            }
        })),
    )
}

/// GET /v1/settings/llm — returns masked config
pub async fn get_llm_settings(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let service = match &state.llm_settings_service {
        Some(s) => s,
        None => return settings_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "LLM_NOT_CONFIGURED",
            "LLM router is not configured",
        ).into_response(),
    };

    match service.get_config().await {
        Ok(config) => (StatusCode::OK, Json(serde_json::to_value(config).unwrap())).into_response(),
        Err(e) => settings_error(StatusCode::INTERNAL_SERVER_ERROR, "DISK_WRITE_FAILED", &e).into_response(),
    }
}

/// PUT /v1/settings/llm — add/update key
pub async fn upsert_key(
    State(state): State<Arc<AppState>>,
    Json(body): Json<AddKeyRequest>,
) -> impl IntoResponse {
    let service = match &state.llm_settings_service {
        Some(s) => s,
        None => return settings_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "LLM_NOT_CONFIGURED",
            "LLM router is not configured",
        ).into_response(),
    };

    // Validate key format
    if body.key.secret.is_empty() {
        return settings_error(
            StatusCode::BAD_REQUEST,
            "INVALID_KEY_FORMAT",
            "Key secret cannot be empty",
        ).into_response();
    }

    match service.upsert_key(body).await {
        Ok(()) => {
            // Emit key status changed event
            state.event_broadcaster.key_status_changed(
                "", "", "added",
            );
            (StatusCode::OK, Json(serde_json::json!({ "status": "ok" }))).into_response()
        }
        Err(e) if e.contains("Encryption") => {
            settings_error(StatusCode::INTERNAL_SERVER_ERROR, "ENCRYPTION_FAILED", &e).into_response()
        }
        Err(e) => {
            settings_error(StatusCode::INTERNAL_SERVER_ERROR, "DISK_WRITE_FAILED", &e).into_response()
        }
    }
}

/// DELETE /v1/settings/llm/keys/{provider}/{key_id} — remove key
pub async fn delete_key(
    State(state): State<Arc<AppState>>,
    Path((provider, key_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let service = match &state.llm_settings_service {
        Some(s) => s,
        None => return settings_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "LLM_NOT_CONFIGURED",
            "LLM router is not configured",
        ).into_response(),
    };

    match service.remove_key(&provider, &key_id).await {
        Ok(()) => {
            state.event_broadcaster.key_status_changed(
                &provider, &key_id, "removed",
            );
            (StatusCode::OK, Json(serde_json::json!({ "status": "ok" }))).into_response()
        }
        Err(e) if e.contains("not found") => {
            settings_error(StatusCode::NOT_FOUND, "KEY_NOT_FOUND", &e).into_response()
        }
        Err(e) => {
            settings_error(StatusCode::INTERNAL_SERVER_ERROR, "DISK_WRITE_FAILED", &e).into_response()
        }
    }
}

/// PUT /v1/settings/llm/keys/reorder — reorder keys + set primary
pub async fn reorder_keys(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ReorderKeysRequest>,
) -> impl IntoResponse {
    let service = match &state.llm_settings_service {
        Some(s) => s,
        None => return settings_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "LLM_NOT_CONFIGURED",
            "LLM router is not configured",
        ).into_response(),
    };

    match service.reorder_keys(body).await {
        Ok(()) => {
            state.event_broadcaster.key_status_changed(
                "", "", "reordered",
            );
            (StatusCode::OK, Json(serde_json::json!({ "status": "ok" }))).into_response()
        }
        Err(e) => {
            settings_error(StatusCode::INTERNAL_SERVER_ERROR, "DISK_WRITE_FAILED", &e).into_response()
        }
    }
}

/// PUT /v1/settings/llm/keys/priority — set per-key priority
pub async fn set_key_priority(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SetKeyPriorityRequest>,
) -> impl IntoResponse {
    let service = match &state.llm_settings_service {
        Some(s) => s,
        None => return settings_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "LLM_NOT_CONFIGURED",
            "LLM router is not configured",
        ).into_response(),
    };

    if body.priority != "primary" && body.priority != "fallback" {
        return settings_error(
            StatusCode::BAD_REQUEST,
            "INVALID_PRIORITY",
            "Priority must be 'primary' or 'fallback'",
        ).into_response();
    }

    let provider = body.provider.clone();
    let key_id = body.key_id.clone();

    match service.set_key_priority(body).await {
        Ok(()) => {
            state.event_broadcaster.key_status_changed(
                &provider, &key_id, "priority_changed",
            );
            (StatusCode::OK, Json(serde_json::json!({ "status": "ok" }))).into_response()
        }
        Err(e) if e.contains("not found") => {
            settings_error(StatusCode::NOT_FOUND, "KEY_NOT_FOUND", &e).into_response()
        }
        Err(e) => {
            settings_error(StatusCode::INTERNAL_SERVER_ERROR, "DISK_WRITE_FAILED", &e).into_response()
        }
    }
}

/// POST /v1/settings/llm/validate — test key validity
pub async fn validate_key(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ValidateKeyRequest>,
) -> impl IntoResponse {
    let service = match &state.llm_settings_service {
        Some(s) => s,
        None => return settings_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "LLM_NOT_CONFIGURED",
            "LLM router is not configured",
        ).into_response(),
    };

    if body.secret.is_empty() {
        return settings_error(
            StatusCode::BAD_REQUEST,
            "INVALID_KEY_FORMAT",
            "Key secret cannot be empty",
        ).into_response();
    }

    match service.validate_key(body).await {
        Ok(result) => (StatusCode::OK, Json(serde_json::to_value(result).unwrap())).into_response(),
        Err(e) => settings_error(StatusCode::GATEWAY_TIMEOUT, "KEY_VALIDATION_TIMEOUT", &e).into_response(),
    }
}

/// GET /v1/settings/llm/status — live health
pub async fn get_key_status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let service = match &state.llm_settings_service {
        Some(s) => s,
        None => return settings_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "LLM_NOT_CONFIGURED",
            "LLM router is not configured",
        ).into_response(),
    };

    let health = service.key_health().await;
    (StatusCode::OK, Json(serde_json::to_value(health).unwrap())).into_response()
}

/// GET /v1/models — list all available models
pub async fn list_models(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let service = match &state.llm_settings_service {
        Some(s) => s,
        None => return settings_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "LLM_NOT_CONFIGURED",
            "LLM router is not configured",
        ).into_response(),
    };

    let models = service.available_models();
    (StatusCode::OK, Json(serde_json::to_value(models).unwrap())).into_response()
}

/// POST /v1/models/refresh — refresh models from provider APIs
pub async fn refresh_models(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let service = match &state.llm_settings_service {
        Some(s) => s,
        None => return settings_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "LLM_NOT_CONFIGURED",
            "LLM router is not configured",
        ).into_response(),
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
            let active_tasks = state.gateway.shared_context.task_registry.list_active().len();
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
        Err(e) => {
            settings_error(StatusCode::INTERNAL_SERVER_ERROR, "CONFIG_READ_FAILED", &e)
                .into_response()
        }
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
            state
                .event_broadcaster
                .orchestrator_config_changed(&model_name);
            (
                StatusCode::OK,
                Json(serde_json::json!({ "status": "ok" })),
            )
                .into_response()
        }
        Err(e) => {
            settings_error(StatusCode::INTERNAL_SERVER_ERROR, "DISK_WRITE_FAILED", &e)
                .into_response()
        }
    }
}

// ── LLM Usage endpoints ──────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct LlmUsageQuery {
    pub agent_id: Option<String>,
    pub key_id: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct LlmUsageDailyQuery {
    pub agent_id: Option<String>,
    pub date: Option<String>,
    pub limit: Option<usize>,
}

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

    let result = if let Some(ref agent_id) = query.agent_id {
        repo.get_daily_usage(agent_id, limit)
    } else {
        repo.get_all_daily_usage(limit)
    };

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
pub async fn get_discovered_credentials(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let tm = match &state.token_manager {
        Some(tm) => tm,
        None => {
            return (
                StatusCode::OK,
                Json(serde_json::json!([])),
            ).into_response();
        }
    };

    let creds = tm.discovered_sources().await;
    (StatusCode::OK, Json(serde_json::to_value(creds).unwrap())).into_response()
}

/// POST /v1/settings/llm/credentials/rescan — rescan credentials
pub async fn rescan_credentials(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let tm = match &state.token_manager {
        Some(tm) => tm,
        None => {
            return settings_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "CREDENTIAL_DISCOVERY_NOT_CONFIGURED",
                "Credential discovery is not enabled",
            ).into_response();
        }
    };

    let service = match &state.llm_settings_service {
        Some(s) => s,
        None => {
            return settings_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "LLM_NOT_CONFIGURED",
                "LLM router is not configured",
            ).into_response();
        }
    };

    let router = service.router();
    let creds = tm.rescan(service, router).await;
    (StatusCode::OK, Json(serde_json::to_value(creds).unwrap())).into_response()
}

/// GET /v1/settings/llm/cli-backends — list CLI backend status
pub async fn get_cli_backends(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    // Read CLI backends config from llm.toml
    let llm_config_path = std::env::current_dir()
        .unwrap_or_default()
        .join("config/llm.toml");

    let cli_config = if llm_config_path.exists() {
        openalpaca_llm::read_config(&llm_config_path)
            .ok()
            .and_then(|c| c.cli_backends)
            .unwrap_or_default()
    } else {
        openalpaca_llm::CliBackendsConfig::default()
    };

    let statuses = openalpaca_llm::detect_cli_backends(&cli_config);
    let _ = &state; // acknowledge state usage
    (StatusCode::OK, Json(serde_json::to_value(statuses).unwrap())).into_response()
}

/// GET /v1/settings/llm/providers/usage — provider-level usage summaries
pub async fn get_provider_usage(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    let service = match &state.llm_settings_service {
        Some(s) => s,
        None => {
            return settings_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "LLM_NOT_CONFIGURED",
                "LLM router is not configured",
            ).into_response();
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

    (StatusCode::OK, Json(serde_json::to_value(summaries).unwrap())).into_response()
}

// ── LLM Pricing endpoints ──────────────────────────────────────────

/// GET /v1/llm/pricing — list all models with their pricing information
pub async fn get_llm_pricing(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let service = match &state.llm_settings_service {
        Some(s) => s,
        None => return settings_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "LLM_NOT_CONFIGURED",
            "LLM router is not configured",
        ).into_response(),
    };

    let models = service.all_models_with_pricing();
    (StatusCode::OK, Json(serde_json::to_value(models).unwrap())).into_response()
}

#[derive(Debug, Deserialize)]
pub struct CostEstimateQuery {
    pub model: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
}

/// GET /v1/llm/pricing/estimate — estimate cost for given model and token counts
pub async fn estimate_cost(
    State(state): State<Arc<AppState>>,
    Query(query): Query<CostEstimateQuery>,
) -> impl IntoResponse {
    let service = match &state.llm_settings_service {
        Some(s) => s,
        None => return settings_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "LLM_NOT_CONFIGURED",
            "LLM router is not configured",
        ).into_response(),
    };

    let cost = service.estimate_cost(&query.model, query.input_tokens, query.output_tokens);
    (StatusCode::OK, Json(serde_json::json!({
        "model": query.model,
        "input_tokens": query.input_tokens,
        "output_tokens": query.output_tokens,
        "estimated_cost_usd": cost,
    }))).into_response()
}
