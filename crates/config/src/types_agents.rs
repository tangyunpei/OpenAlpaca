use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::types_base::IdentityConfig;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentsConfig {
    pub defaults: Option<AgentDefaultsConfig>,
    pub list: Option<Vec<AgentConfig>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentDefaultsConfig {
    pub model: Option<String>,
    pub system_prompt: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_yaml::Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    pub id: String,
    pub model: Option<String>,
    pub system_prompt: Option<String>,
    pub identity: Option<IdentityConfig>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_yaml::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentBinding {
    pub agent: String,
    #[serde(default = "default_binding_type")]
    pub binding_type: String,
    pub channel: Option<String>,
    pub account: Option<String>,
    pub peer: Option<PeerBinding>,
    pub guild: Option<String>,
    pub team: Option<String>,
    pub roles: Option<Vec<String>>,
}

fn default_binding_type() -> String {
    "route".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerBinding {
    pub kind: Option<String>,
    pub id: String,
}
