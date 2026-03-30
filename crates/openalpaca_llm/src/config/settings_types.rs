use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmSettingsResponse {
    pub orchestrator: OrchestratorInfo,
    pub providers: HashMap<String, ProviderInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestratorInfo {
    pub model: String,
    pub fallback_models: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderInfo {
    pub enabled: bool,
    pub key_selection_strategy: String,
    pub keys: Vec<KeyInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyInfo {
    pub id: String,
    pub masked_secret: String,
    pub tier: Option<String>,
    pub priority: String,
    pub source: String,
    pub notes: Option<String>,
    pub status: String,
    pub monthly_usage_usd: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub managed: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credential_expires_at: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_usage: Option<crate::routing::provider_usage::ExternalUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyInput {
    pub id: Option<String>,
    pub secret: String,
    pub tier: Option<String>,
    pub priority: Option<String>,
    pub source: Option<String>,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddKeyRequest {
    pub provider: String,
    pub key: KeyInput,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReorderKeysRequest {
    pub provider: String,
    pub key_order: Vec<String>,
    pub primary_key_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SetKeyPriorityRequest {
    pub provider: String,
    pub key_id: String,
    pub priority: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidateKeyRequest {
    pub provider: String,
    pub secret: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyValidationResult {
    pub valid: bool,
    pub tier: Option<String>,
    pub detected_source: Option<String>,
    pub models_available: Vec<String>,
    pub rate_limits: Option<String>,
    pub format_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestratorConfigResponse {
    pub model: String,
    pub fallback_models: Vec<String>,
    pub active_agents: usize,
    pub active_tasks: usize,
    pub daily_cost_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateOrchestratorRequest {
    pub model: String,
    pub fallback_models: Vec<String>,
}
