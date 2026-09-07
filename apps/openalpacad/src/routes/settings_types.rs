//! Request/response types and helpers for settings endpoints.

use axum::{Json, http::StatusCode, response::IntoResponse};
use serde::{Deserialize, Serialize};

pub(super) fn settings_error(status: StatusCode, code: &str, message: &str) -> impl IntoResponse {
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

/// Body of `PUT /v1/settings/llm/providers/{provider}/enabled` (GAP-15).
#[derive(Debug, Deserialize)]
pub struct SetProviderEnabledRequest {
    pub enabled: bool,
}

/// Its answer: the row as it now stands, and whether the router actually holds
/// the provider.
///
/// `enabled` is the disposition in `llm.toml`; `loaded` is what the daemon did
/// with it. They differ when an enable cannot register — no usable key, or the
/// provider is not compiled in — and `warning` is then the daemon's own
/// sentence about why. A disable is `loaded: false` with no warning: that is
/// what it asked for. Silent degradation is the one outcome the settled rules
/// reject, so this travels on the wire rather than only in the log (R60).
#[derive(Debug, Serialize)]
pub(super) struct ProviderEnabledResponse {
    pub id: String,
    pub enabled: bool,
    pub loaded: bool,
    pub warning: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct LlmUsageQuery {
    /// GAP-08b: checked first in `get_llm_usage` — a task-scoped query wins
    /// over `agent_id`/`key_id` when more than one is present.
    pub task_id: Option<String>,
    pub agent_id: Option<String>,
    pub key_id: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct LlmUsageDailyQuery {
    pub agent_id: Option<String>,
    /// Exact-match filter on the aggregate date (`YYYY-MM-DD`).
    pub date: Option<String>,
    pub limit: Option<usize>,
}

/// `?window=` on `GET /v1/usage/summary` (GAP-08c, T50). `today` is the only
/// window; absent means `today`.
#[derive(Debug, Deserialize)]
pub struct UsageSummaryQuery {
    pub window: Option<String>,
}

/// `GET /v1/usage/summary?window=today` (GAP-08c, T50).
///
/// `date` is the **UTC** day the figures cover, echoed because the client's
/// own `todayIsoDate()` is local: the two disagree for up to twelve hours a
/// day, and a total labelled with the wrong day is worse than no total.
///
/// `total_usd` is an unbounded figure. Per **N4** there is no daily budget and
/// none is to be added, so there is no `daily_*` key here and nothing to draw
/// a progress bar against — `caps` are the per-workflow and per-turn limits
/// that actually exist, which is what the panel says instead.
#[derive(Debug, Serialize)]
pub(super) struct UsageSummaryResponse {
    pub date: String,
    pub total_usd: f64,
    pub by_provider: Vec<ProviderUsageRow>,
    pub caps: UsageCaps,
}

/// One provider's share of the day, out of today's `llm_call_log` rows — not
/// the lifetime `CostTracker::all_provider_usage()` the Models panel showed
/// under a "today" heading.
#[derive(Debug, Serialize)]
pub(super) struct ProviderUsageRow {
    pub provider: String,
    pub usd: f64,
    pub calls: i64,
    /// Input + output tokens. Serves the design's `41k tok today` per
    /// provider, which was the other half of GAP-08c.
    pub tokens: i64,
}

/// The two cost caps the daemon actually enforces (N4): `max_cost` per
/// workflow (`execution.lead_agent_defaults`) and per agent turn
/// (`execution.agent_defaults`). Neither is a daily budget, and adding one
/// would be a new enforcement point in the router, not a label.
#[derive(Debug, Serialize)]
pub(super) struct UsageCaps {
    pub workflow_max_cost_usd: f64,
    pub agent_max_cost_usd: f64,
}

#[derive(Serialize)]
pub(super) struct DaemonProvidersResponse {
    pub web_search: WebSearchConfigResponse,
}

#[derive(Serialize)]
pub(super) struct WebSearchConfigResponse {
    pub api_key_configured: bool,
    pub api_key_hint: String,
    pub timeout_secs: u64,
}

#[derive(Deserialize)]
pub struct UpdateWebSearchRequest {
    pub api_key: Option<String>,
    pub timeout_secs: Option<u64>,
}

pub(super) fn load_cli_backends_config(
    llm_config_path: &std::path::Path,
) -> openalpaca_llm::CliBackendsConfig {
    if llm_config_path.exists() {
        openalpaca_llm::read_config(llm_config_path)
            .ok()
            .and_then(|cfg| cfg.cli_backends)
            .unwrap_or_default()
    } else {
        openalpaca_llm::CliBackendsConfig::default()
    }
}
