use crate::AppState;
use axum::http::StatusCode;
use axum::{
    Json,
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Utc};
use openalpaca_core::events::SystemEvent;
use openalpaca_llm::SetProviderEnabledError;
use openalpaca_llm::config::settings_service::{
    AddKeyRequest, OrchestratorConfigResponse, ReorderKeysRequest, SetKeyPriorityRequest,
    UpdateOrchestratorRequest, ValidateKeyRequest,
};
use std::sync::Arc;

use super::api_error;
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

/// Total spend on one **UTC** date, summed from the DB's `llm_usage_daily`
/// aggregate across every agent/model row for that day — **not**
/// `CostTracker::total_cost()`, which only measures spend since the daemon
/// booted (GAP-08a). A day's distinct `(agent_id, model)` rows are few; the
/// query's limit is a generous cap, not a real pagination bound.
///
/// Backs `GET /v1/orchestrator/config.daily_cost_usd` only. `GET
/// /v1/usage/summary` used to call this for its own `total_usd`, but that made
/// the total a second writer that could disagree with `by_provider` (R64) —
/// the summary route now sums `by_provider` instead and never calls this.
fn cost_for_utc_date(db: &openalpaca_storage::Database, date: &str) -> f64 {
    let repo = openalpaca_storage::repository::LlmUsageRepository::new(db);
    repo.query_daily_usage(None, Some(date), 10_000)
        .map(|rows| rows.iter().map(|r| r.total_cost_usd).sum())
        .unwrap_or(0.0)
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
            let daily_cost_usd =
                cost_for_utc_date(&state.db, &Utc::now().format("%Y-%m-%d").to_string());

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

/// PUT /v1/orchestrator/config — pick the model the daemon chats with.
///
/// The service writes `llm.toml` under the config lock **and points the router
/// at the new default in the same call** (R62): the write is deduped against
/// the watcher's ring, so nothing else would. `GET` reads the file, so any gap
/// between the two is a divergence with nothing on the wire saying so.
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

    let result = if let Some(ref task_id) = query.task_id {
        repo.get_task_usage(task_id, limit)
    } else if let Some(ref agent_id) = query.agent_id {
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

// ── Usage summary (GAP-08c, T50) ────────────────────────────────────

/// `?window=` on `GET /v1/usage/summary`. `today` is the only window the route
/// can answer, and an absent one means it — an empty query string behaves the
/// same as asking explicitly. Anything else is reported back as the token
/// itself, so the 400 names what it did not recognise instead of quietly
/// answering a different question. Same code word as `GET /v1/agent-templates`
/// (T48): one unknown-window refusal across the API.
fn resolve_usage_window(raw: Option<&str>) -> Result<&'static str, String> {
    match raw.unwrap_or("today") {
        "today" => Ok("today"),
        other => Err(other.to_string()),
    }
}

/// UTC midnight of `now`'s date, in `llm_call_log.timestamp`'s own text form.
fn utc_day_start(now: DateTime<Utc>) -> String {
    now.format("%Y-%m-%d 00:00:00").to_string()
}

/// Assemble the summary. Pure, so the shape — and N4's absence of any daily
/// budget in it — is tested without a daemon.
///
/// `total_usd` is **not** a separate figure: it is `sum(by_provider[].usd)`,
/// computed here from the very rows the breakdown is built from (R64). There
/// is deliberately no second source (the `llm_usage_daily` rollup) for this
/// route to disagree with — the breakdown adds up to the total by
/// construction, not by convention.
fn usage_summary(
    date: String,
    providers: Vec<openalpaca_storage::ProviderCallUsage>,
    caps: UsageCaps,
) -> UsageSummaryResponse {
    let by_provider: Vec<ProviderUsageRow> = providers
        .into_iter()
        .map(|p| ProviderUsageRow {
            provider: p.provider,
            usd: p.cost_usd,
            calls: p.calls,
            tokens: p.tokens,
        })
        .collect();
    let total_usd = by_provider.iter().map(|p| p.usd).sum();
    UsageSummaryResponse {
        date,
        total_usd,
        by_provider,
        caps,
    }
}

/// GET /v1/usage/summary?window=today — today's spend, and the caps that
/// actually bound it (GAP-08c).
///
/// `total_usd` and `by_provider` are one source: both come out of today's
/// `llm_call_log` rows (`provider_usage_since`), so the breakdown always sums
/// to the total (R64). The `llm_usage_daily` rollup is a second writer with
/// its own upsert path and is **not** served by this route — it still backs
/// `GET /v1/orchestrator/config.daily_cost_usd` and `GET
/// /v1/llm/usage/daily`, which is where a caller wanting that number goes.
/// `date` is the UTC day the figures cover, echoed because the GUI's own
/// `todayIsoDate()` is local and disagrees for up to twelve hours.
///
/// Per **N4** the caps are per-workflow and per-turn `max_cost`. There is no
/// daily budget, no `daily_*` key, and today's total ships with no denominator.
pub async fn get_usage_summary(
    State(state): State<Arc<AppState>>,
    Query(query): Query<UsageSummaryQuery>,
) -> Response {
    if let Err(bad) = resolve_usage_window(query.window.as_deref()) {
        return api_error(
            StatusCode::BAD_REQUEST,
            "UNKNOWN_WINDOW",
            format!("Unknown window '{bad}' — use today"),
        );
    }

    let now = Utc::now();
    let date = now.format("%Y-%m-%d").to_string();

    // A read failure costs the breakdown, not the summary: the panel then
    // shows the total with no per-provider rows, which is what a day with no
    // calls looks like too — never an invented figure.
    let providers = openalpaca_storage::repository::LlmUsageRepository::new(&state.db)
        .provider_usage_since(&utc_day_start(now))
        .unwrap_or_else(|e| {
            tracing::warn!("Failed to read today's per-provider usage: {e}");
            Vec::new()
        });

    let execution = &state.daemon_config.load().execution;
    let caps = UsageCaps {
        workflow_max_cost_usd: execution.lead_agent_defaults.max_cost,
        agent_max_cost_usd: execution.agent_defaults.max_cost,
    };

    let summary = usage_summary(date, providers, caps);
    (StatusCode::OK, Json(summary)).into_response()
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

// ── Provider enable/disable (GAP-15) ────────────────────────────────

/// PUT /v1/settings/llm/providers/{provider}/enabled — turn one provider on
/// or off.
///
/// The bit lives in `llm.toml`, so the write lands first and the router is
/// only touched once it has: a disable unloads the provider (in-flight calls
/// finish, new ones fall through to the fallback chain), an enable
/// re-registers it and refreshes its models.
///
/// **The 409 rule.** `[orchestrator] model` is resolved to a provider by three
/// rungs in order: the live model registry, the config's own `[models]` table,
/// then what the model id itself says (`claude-…` → Anthropic, `gpt-…`/`o3-…`
/// → OpenAI, or an explicit `provider/model` prefix). The disable is refused
/// with `409 PROVIDER_IS_DEFAULT` when that resolves to the provider being
/// turned off — nothing would be left to answer with — and with
/// `409 DEFAULT_MODEL_UNRESOLVED` when it resolves to nothing at all, in which
/// case every provider disable is refused and the message names the unresolved
/// default so the owner fixes it first. Two code words because the two assert
/// different facts: the first says this provider serves the default model,
/// which is precisely what the second could not establish. The guard fails
/// closed: it never allows a disable on a guess.
///
/// The 200 body is `{id, enabled, loaded, warning}`. `enabled` is the
/// disposition now on disk; `loaded` is whether the router holds the provider.
/// An enable that could not register — no usable key, or the provider is not
/// compiled in — is still a 200, because the write happened and a restart
/// reaches the same state, but it answers `loaded: false` and carries the
/// daemon's reason in `warning` rather than leaving it in the log.
pub async fn set_provider_enabled(
    State(state): State<Arc<AppState>>,
    Path(provider): Path<String>,
    Json(body): Json<SetProviderEnabledRequest>,
) -> impl IntoResponse {
    let Some(service) = &state.llm_settings_service else {
        return api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "LLM_NOT_CONFIGURED",
            "LLM router is not configured",
        );
    };
    provider_enabled_response(service, &provider, body.enabled).await
}

/// The route's whole decision table, minus the `AppState` lookup above — so
/// every status code it can answer with is reachable from a test.
pub(crate) async fn provider_enabled_response(
    service: &openalpaca_llm::LlmSettingsService,
    provider: &str,
    enabled: bool,
) -> Response {
    match service.set_provider_enabled(provider, enabled).await {
        Ok(outcome) => {
            if let Some(warning) = &outcome.warning {
                tracing::warn!(provider = %outcome.id, warning = %warning, "provider toggled but not loaded");
            }
            (
                StatusCode::OK,
                Json(ProviderEnabledResponse {
                    id: outcome.id,
                    enabled: outcome.enabled,
                    loaded: outcome.loaded,
                    warning: outcome.warning,
                }),
            )
                .into_response()
        }
        Err(e @ SetProviderEnabledError::UnknownProvider(_)) => {
            api_error(StatusCode::NOT_FOUND, "PROVIDER_NOT_FOUND", e.to_string())
        }
        // A code word per arm. Both are 409 and both have the same remedy, but
        // they assert different facts: `PROVIDER_IS_DEFAULT` says *this*
        // provider serves the default model, which is exactly what the second
        // arm could not establish. A client that renders per code — the GUI
        // does — would otherwise say a false thing about the provider the owner
        // just tried to turn off (R61a).
        Err(e @ SetProviderEnabledError::IsDefaultProvider { .. }) => {
            api_error(StatusCode::CONFLICT, "PROVIDER_IS_DEFAULT", e.to_string())
        }
        Err(e @ SetProviderEnabledError::DefaultModelUnresolved { .. }) => api_error(
            StatusCode::CONFLICT,
            "DEFAULT_MODEL_UNRESOLVED",
            e.to_string(),
        ),
        Err(e @ SetProviderEnabledError::Persist(_)) => api_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "DISK_WRITE_FAILED",
            e.to_string(),
        ),
    }
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
///
/// Through the settings service, which holds `llm.toml.lock` across the whole
/// read-modify-write and rotates a backup. It used to read → mutate → write the
/// document itself with an unlocked `fs::write`, so a request overlapping a
/// provider toggle wrote the pre-toggle `enabled` bit back over it.
pub async fn update_web_search_config(
    State(state): State<Arc<AppState>>,
    Json(body): Json<UpdateWebSearchRequest>,
) -> impl IntoResponse {
    let Some(service) = &state.llm_settings_service else {
        return settings_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "LLM_NOT_CONFIGURED",
            "LLM settings service is not configured, so llm.toml cannot be written",
        )
        .into_response();
    };

    let updated_ws = match service.update_web_search_config(body.api_key, body.timeout_secs) {
        Ok(ws) => ws,
        Err(e) => {
            return settings_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "DISK_WRITE_FAILED",
                &format!("Failed to write llm.toml: {}", e),
            )
            .into_response();
        }
    };

    // The hot path, applied here rather than left to the watcher — which is
    // load-bearing, not an optimisation (R62): the write goes through
    // `persist_only`, whose writer records the hash, so the watcher swallows
    // the event this write raised and step 5 of the tick never runs for it.
    // This is the same `Arc<ArcSwap<_>>` the watcher stores into, so an
    // external edit still lands on it.
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
mod provider_enabled_tests;

#[cfg(test)]
mod tests {
    use super::{
        ProviderUsageRow, UsageCaps, cost_for_utc_date, load_cli_backends_config,
        resolve_usage_window, usage_summary, utc_day_start,
    };
    use chrono::TimeZone;
    use openalpaca_storage::{Database, LlmUsageDaily, LlmUsageRepository, ProviderCallUsage};
    use std::fs;
    use std::path::PathBuf;

    fn temp_dir(prefix: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("{prefix}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("temp dir should be creatable");
        dir
    }

    fn test_db() -> (tempfile::TempDir, Database) {
        let dir = tempfile::tempdir().expect("create tempdir");
        let db = Database::open(&dir.path().join("test.db")).expect("open test db");
        (dir, db)
    }

    #[test]
    fn total_daily_cost_usd_sums_todays_rows_across_agents_and_models() {
        let (_dir, db) = test_db();
        let repo = LlmUsageRepository::new(&db);
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();

        repo.upsert_daily_usage(&LlmUsageDaily {
            date: today.clone(),
            agent_id: "orchestrator".to_string(),
            model: "claude-sonnet-4-5-20250929".to_string(),
            total_requests: 3,
            total_input_tokens: 900,
            total_output_tokens: 300,
            total_cost_usd: 0.30,
        })
        .unwrap();
        repo.upsert_daily_usage(&LlmUsageDaily {
            date: today.clone(),
            agent_id: "researcher".to_string(),
            model: "gpt-4o".to_string(),
            total_requests: 1,
            total_input_tokens: 200,
            total_output_tokens: 100,
            total_cost_usd: 0.05,
        })
        .unwrap();
        // A different day must not be counted.
        repo.upsert_daily_usage(&LlmUsageDaily {
            date: "2020-01-01".to_string(),
            agent_id: "orchestrator".to_string(),
            model: "claude-sonnet-4-5-20250929".to_string(),
            total_requests: 100,
            total_input_tokens: 100_000,
            total_output_tokens: 100_000,
            total_cost_usd: 99.0,
        })
        .unwrap();

        let total = cost_for_utc_date(&db, &today);
        assert!(
            (total - 0.35).abs() < 1e-9,
            "expected 0.35, got {total}"
        );
    }

    #[test]
    fn total_daily_cost_usd_is_zero_with_no_usage() {
        let (_dir, db) = test_db();
        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
        assert_eq!(cost_for_utc_date(&db, &today), 0.0);
    }

    // ── GET /v1/usage/summary (GAP-08c, T50) ────────────────────────────

    /// An empty query string asks the only question the route can answer.
    #[test]
    fn usage_window_defaults_to_today() {
        assert_eq!(resolve_usage_window(None), Ok("today"));
        assert_eq!(resolve_usage_window(Some("today")), Ok("today"));
    }

    /// Anything else is the caller's mistake, reported back as the token
    /// itself rather than silently answered with a different window.
    #[test]
    fn usage_window_rejects_every_other_token() {
        assert_eq!(resolve_usage_window(Some("7d")), Err("7d".to_string()));
        assert_eq!(resolve_usage_window(Some("all")), Err("all".to_string()));
        assert_eq!(resolve_usage_window(Some("")), Err(String::new()));
    }

    /// The call-log cutoff is UTC midnight of the day the summary reports —
    /// the client's `todayIsoDate()` is local and the two disagree for up to
    /// twelve hours a day, which is exactly why `date` is echoed.
    #[test]
    fn day_start_is_utc_midnight_of_the_reported_date() {
        let now = chrono::Utc
            .with_ymd_and_hms(2026, 9, 8, 23, 45, 10)
            .unwrap();
        assert_eq!(utc_day_start(now), "2026-09-08 00:00:00");
    }

    fn provider(name: &str, cost_usd: f64, calls: i64, tokens: i64) -> ProviderCallUsage {
        ProviderCallUsage {
            provider: name.to_string(),
            cost_usd,
            calls,
            tokens,
        }
    }

    fn caps() -> UsageCaps {
        UsageCaps {
            workflow_max_cost_usd: 5.0,
            agent_max_cost_usd: 1.0,
        }
    }

    #[test]
    fn summary_reports_the_total_and_one_row_per_provider() {
        let summary = usage_summary(
            "2026-09-08".to_string(),
            vec![
                provider("anthropic", 0.30, 4, 1200),
                provider("openai", 0.05, 1, 300),
            ],
            caps(),
        );

        assert_eq!(summary.date, "2026-09-08");
        assert!((summary.total_usd - 0.35).abs() < 1e-9);
        assert_eq!(summary.by_provider.len(), 2);
        let first: &ProviderUsageRow = &summary.by_provider[0];
        assert_eq!(first.provider, "anthropic");
        assert!((first.usd - 0.30).abs() < 1e-9);
        assert_eq!(first.calls, 4);
        assert_eq!(first.tokens, 1200);
    }

    /// R64: `total_usd` is not a second source that can disagree with the
    /// breakdown — it is computed FROM `by_provider`, so the two add up by
    /// construction. A two-provider fixture with an amount that would expose
    /// float-summation slip if the total came from anywhere else.
    #[test]
    fn summary_total_usd_is_the_sum_of_by_provider_usd_for_two_providers() {
        let summary = usage_summary(
            "2026-09-08".to_string(),
            vec![
                provider("anthropic", 0.0184, 71, 41125),
                provider("openai", 0.0512, 3, 900),
            ],
            caps(),
        );

        let summed: f64 = summary.by_provider.iter().map(|p| p.usd).sum();
        assert!(
            (summary.total_usd - summed).abs() < 1e-12,
            "total_usd ({}) must equal sum(by_provider.usd) ({summed}) exactly, by construction",
            summary.total_usd
        );
        assert!((summary.total_usd - 0.0696).abs() < 1e-9);
    }

    /// No providers today means no spend today — not a total left over from
    /// some other source.
    #[test]
    fn summary_total_usd_is_zero_with_no_provider_rows() {
        let summary = usage_summary("2026-09-08".to_string(), vec![], caps());
        assert_eq!(summary.total_usd, 0.0);
        assert!(summary.by_provider.is_empty());
    }

    /// N4, on the wire: the caps are the per-workflow and per-turn `max_cost`,
    /// named as such, and **no** `daily_*` key exists to be drawn as a
    /// denominator under today's total.
    #[test]
    fn summary_carries_the_two_caps_and_no_daily_budget() {
        let value = serde_json::to_value(usage_summary(
            "2026-09-08".to_string(),
            vec![provider("anthropic", 0.30, 4, 1200)],
            caps(),
        ))
        .unwrap();

        let object = value.as_object().expect("summary is an object");
        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["by_provider", "caps", "date", "total_usd"]);

        let caps_object = object["caps"].as_object().expect("caps is an object");
        let mut cap_keys: Vec<&str> = caps_object.keys().map(String::as_str).collect();
        cap_keys.sort_unstable();
        assert_eq!(cap_keys, ["agent_max_cost_usd", "workflow_max_cost_usd"]);
        assert_eq!(caps_object["workflow_max_cost_usd"], 5.0);
        assert_eq!(caps_object["agent_max_cost_usd"], 1.0);

        let rendered = serde_json::to_string(&value).unwrap();
        assert!(
            !rendered.contains("daily"),
            "N4: no daily budget may appear anywhere on this route — {rendered}"
        );
        assert!(!rendered.contains("budget"), "N4: no budget key — {rendered}");
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
