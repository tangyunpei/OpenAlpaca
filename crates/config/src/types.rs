use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::types_agents::{AgentBinding, AgentsConfig};
use crate::types_base::{AuthConfig, LoggingConfig, SessionConfig};
use crate::types_channels::ChannelsConfig;
use crate::types_gateway::{DiscoveryConfig, GatewayConfig};

// Opaque stubs for config sections not yet needed (phases 6-7).
pub type CliConfig = serde_yml::Value;
pub type EnvConfig = serde_yml::Value;
pub type ConfigMeta = serde_yml::Value;
pub type ModelsConfig = serde_yml::Value;
pub type ToolsConfig = serde_yml::Value;
pub type SkillsConfig = serde_yml::Value;
pub type SecretsConfig = serde_yml::Value;
pub type PluginsConfig = serde_yml::Value;
pub type HooksConfig = serde_yml::Value;
pub type MemoryConfig = serde_yml::Value;
pub type CronConfig = serde_yml::Value;
pub type MessagesConfig = serde_yml::Value;
pub type ApprovalsConfig = serde_yml::Value;
pub type BrowserConfig = serde_yml::Value;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OpenAlpacaConfig {
    pub meta: Option<ConfigMeta>,
    pub auth: Option<AuthConfig>,
    pub env: Option<EnvConfig>,
    pub logging: Option<LoggingConfig>,
    pub cli: Option<CliConfig>,
    pub session: Option<SessionConfig>,
    pub channels: Option<ChannelsConfig>,
    pub agents: Option<AgentsConfig>,
    pub bindings: Option<Vec<AgentBinding>>,
    pub gateway: Option<GatewayConfig>,
    pub discovery: Option<DiscoveryConfig>,
    pub models: Option<ModelsConfig>,
    pub tools: Option<ToolsConfig>,
    pub skills: Option<SkillsConfig>,
    pub secrets: Option<SecretsConfig>,
    pub plugins: Option<PluginsConfig>,
    pub hooks: Option<HooksConfig>,
    pub memory: Option<MemoryConfig>,
    pub cron: Option<CronConfig>,
    pub messages: Option<MessagesConfig>,
    pub approvals: Option<ApprovalsConfig>,
    pub browser: Option<BrowserConfig>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_yml::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_config_roundtrip() {
        let config = OpenAlpacaConfig::default();
        let yaml = serde_yml::to_string(&config).unwrap();
        let loaded: OpenAlpacaConfig = serde_yml::from_str(&yaml).unwrap();
        // Default config round-trips without error
        assert!(loaded.channels.is_none());
        assert!(loaded.gateway.is_none());
    }

    #[test]
    fn test_config_with_gateway() {
        let yaml = r#"
gateway:
  port: 3777
  host: "0.0.0.0"
"#;
        let config: OpenAlpacaConfig = serde_yml::from_str(yaml).unwrap();
        let gw = config.gateway.unwrap();
        assert_eq!(gw.port, Some(3777));
        assert_eq!(gw.host.as_deref(), Some("0.0.0.0"));
    }

    #[test]
    fn test_config_with_channels() {
        let yaml = r#"
channels:
  telegram:
    bot_token: "test-token"
    enabled: true
"#;
        let config: OpenAlpacaConfig = serde_yml::from_str(yaml).unwrap();
        let tg = config.channels.unwrap().telegram.unwrap();
        assert_eq!(tg.default.bot_token.as_deref(), Some("test-token"));
        assert_eq!(tg.default.enabled, Some(true));
    }

    #[test]
    fn test_config_with_agents() {
        let yaml = r#"
agents:
  list:
    - id: "assistant"
      model: "gpt-4"
bindings:
  - agent: "assistant"
    channel: "telegram"
"#;
        let config: OpenAlpacaConfig = serde_yml::from_str(yaml).unwrap();
        let agents = config.agents.unwrap();
        assert_eq!(agents.list.as_ref().unwrap().len(), 1);
        assert_eq!(agents.list.as_ref().unwrap()[0].id, "assistant");
        let bindings = config.bindings.unwrap();
        assert_eq!(bindings[0].agent, "assistant");
        assert_eq!(bindings[0].channel.as_deref(), Some("telegram"));
    }

    #[test]
    fn test_config_preserves_extra_fields() {
        let yaml = r#"
custom_field: "hello"
gateway:
  port: 3000
  custom_gw_field: 42
"#;
        let config: OpenAlpacaConfig = serde_yml::from_str(yaml).unwrap();
        assert!(config.extra.contains_key("custom_field"));
        let gw = config.gateway.unwrap();
        assert!(gw.extra.contains_key("custom_gw_field"));
    }

    #[test]
    fn test_full_config_roundtrip() {
        let yaml = r#"
gateway:
  port: 3777
  host: "localhost"
  auth:
    token: "secret-token"
channels:
  telegram:
    bot_token: "tg-token"
    enabled: true
  discord:
    bot_token: "dc-token"
agents:
  list:
    - id: "default"
      model: "gpt-4"
bindings:
  - agent: "default"
    channel: "telegram"
logging:
  level: "info"
session:
  dm_scope: "per-peer"
"#;
        let config: OpenAlpacaConfig = serde_yml::from_str(yaml).unwrap();
        let yaml_out = serde_yml::to_string(&config).unwrap();
        let reloaded: OpenAlpacaConfig = serde_yml::from_str(&yaml_out).unwrap();

        assert_eq!(
            reloaded.gateway.as_ref().unwrap().port,
            config.gateway.as_ref().unwrap().port
        );
        assert_eq!(
            reloaded.logging.as_ref().unwrap().level,
            config.logging.as_ref().unwrap().level
        );
    }

    #[test]
    fn test_malformed_yaml_returns_error() {
        let bad = "{{not: valid: yaml:";
        let result = serde_yml::from_str::<OpenAlpacaConfig>(bad);
        assert!(result.is_err());
    }
}
