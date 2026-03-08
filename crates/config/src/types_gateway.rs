use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::types_base::RateLimitConfig;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GatewayConfig {
    pub port: Option<u16>,
    pub host: Option<String>,
    pub auth: Option<GatewayAuthConfig>,
    pub tls: Option<GatewayTlsConfig>,
    pub reload: Option<GatewayReloadConfig>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_yml::Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GatewayAuthConfig {
    pub token: Option<String>,
    pub password: Option<String>,
    pub rate_limit: Option<RateLimitConfig>,
}

/// TLS configuration for the gateway HTTPS server.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GatewayTlsConfig {
    /// Path to PEM-encoded certificate file.
    pub cert: Option<String>,
    /// Path to PEM-encoded private key file.
    pub key: Option<String>,
}

/// Config reload behavior for the gateway.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct GatewayReloadConfig {
    /// Reload mode: "off", "restart", "hot", "hybrid" (default: "hot")
    pub mode: Option<String>,
    /// Debounce window in milliseconds (default: 300)
    pub debounce_ms: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DiscoveryConfig {
    pub enabled: Option<bool>,
    pub service_name: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_yml::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gateway_config_without_tls() {
        let yaml = r#"
port: 3777
host: "0.0.0.0"
"#;
        let config: GatewayConfig = serde_yml::from_str(yaml).unwrap();
        assert_eq!(config.port, Some(3777));
        assert!(config.tls.is_none());
    }

    #[test]
    fn gateway_config_with_tls() {
        let yaml = r#"
port: 3777
tls:
  cert: "/path/to/cert.pem"
  key: "/path/to/key.pem"
"#;
        let config: GatewayConfig = serde_yml::from_str(yaml).unwrap();
        let tls = config.tls.unwrap();
        assert_eq!(tls.cert.as_deref(), Some("/path/to/cert.pem"));
        assert_eq!(tls.key.as_deref(), Some("/path/to/key.pem"));
    }

    #[test]
    fn gateway_tls_config_partial() {
        let yaml = r#"
tls:
  cert: "/path/to/cert.pem"
"#;
        let config: GatewayConfig = serde_yml::from_str(yaml).unwrap();
        let tls = config.tls.unwrap();
        assert_eq!(tls.cert.as_deref(), Some("/path/to/cert.pem"));
        assert!(tls.key.is_none());
    }

    #[test]
    fn gateway_config_empty_tls_section() {
        let yaml = r#"
port: 8080
tls:
"#;
        let config: GatewayConfig = serde_yml::from_str(yaml).unwrap();
        assert_eq!(config.port, Some(8080));
        // Empty tls section (null value) deserializes as None
        assert!(config.tls.is_none());
    }

    #[test]
    fn gateway_config_with_reload() {
        let yaml = r#"
port: 3777
reload:
  mode: "hot"
  debounce_ms: 500
"#;
        let config: GatewayConfig = serde_yml::from_str(yaml).unwrap();
        let reload = config.reload.unwrap();
        assert_eq!(reload.mode.as_deref(), Some("hot"));
        assert_eq!(reload.debounce_ms, Some(500));
    }

    #[test]
    fn gateway_config_reload_defaults() {
        let yaml = "port: 3777";
        let config: GatewayConfig = serde_yml::from_str(yaml).unwrap();
        assert!(config.reload.is_none());
    }
}
