use std::collections::HashMap;

use serde::{Deserialize, Serialize};

pub type DmScope = String;
pub type DmPolicy = String;
pub type GroupPolicy = String;
pub type ReplyToMode = String;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionConfig {
    pub scope: Option<String>,
    pub dm_scope: Option<DmScope>,
    pub identity_links: Option<HashMap<String, Vec<String>>>,
    pub idle_minutes: Option<u32>,
    pub main_key: Option<String>,
    pub store: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    pub level: Option<String>,
    pub file: Option<String>,
    pub max_file_bytes: Option<u64>,
    pub console_level: Option<String>,
    pub console_style: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OutboundRetryConfig {
    pub attempts: Option<u32>,
    pub min_delay_ms: Option<u64>,
    pub max_delay_ms: Option<u64>,
    pub jitter: Option<f64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AuthConfig {
    pub profiles: Option<HashMap<String, serde_yaml::Value>>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_yaml::Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct IdentityConfig {
    pub name: Option<String>,
    pub avatar_url: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RateLimitConfig {
    pub max_requests: Option<u32>,
    pub window_secs: Option<u64>,
}
