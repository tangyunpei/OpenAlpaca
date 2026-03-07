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
    pub agent_id: Option<String>,
    pub key_id: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct LlmUsageDailyQuery {
    pub agent_id: Option<String>,
    pub date: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Deserialize)]
pub struct CostEstimateQuery {
    pub model: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
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
