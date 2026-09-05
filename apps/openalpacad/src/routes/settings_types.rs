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
