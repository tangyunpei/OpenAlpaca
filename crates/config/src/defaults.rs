use crate::types::OpenAlpacaConfig;
use crate::types_base::{LoggingConfig, SessionConfig};
use crate::types_channels::{ChannelDefaultsConfig, ChannelsConfig};
use crate::types_gateway::GatewayConfig;

/// Apply runtime defaults to config values that serde `#[serde(default)]` cannot handle.
pub fn apply_defaults(config: &mut OpenAlpacaConfig) {
    let gw = config.gateway.get_or_insert_with(GatewayConfig::default);
    if gw.port.is_none() {
        gw.port = Some(3777);
    }
    if gw.host.is_none() {
        gw.host = Some("127.0.0.1".into());
    }

    let logging = config.logging.get_or_insert_with(LoggingConfig::default);
    if logging.level.is_none() {
        logging.level = Some("info".into());
    }
    if logging.console_style.is_none() {
        logging.console_style = Some("pretty".into());
    }

    let session = config.session.get_or_insert_with(SessionConfig::default);
    if session.main_key.is_none() {
        session.main_key = Some("main".into());
    }
    if session.dm_scope.is_none() {
        session.dm_scope = Some("per-peer".into());
    }

    let channels = config.channels.get_or_insert_with(ChannelsConfig::default);
    let ch_defaults = channels
        .defaults
        .get_or_insert_with(ChannelDefaultsConfig::default);
    if ch_defaults.dm_policy.is_none() {
        ch_defaults.dm_policy = Some("allow".into());
    }
    if ch_defaults.group_policy.is_none() {
        ch_defaults.group_policy = Some("ignore".into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_apply_defaults_fills_gateway() {
        let mut config = OpenAlpacaConfig::default();
        apply_defaults(&mut config);

        let gw = config.gateway.unwrap();
        assert_eq!(gw.port, Some(3777));
        assert_eq!(gw.host.as_deref(), Some("127.0.0.1"));
    }

    #[test]
    fn test_apply_defaults_fills_console_style() {
        let mut config = OpenAlpacaConfig::default();
        apply_defaults(&mut config);
        let logging = config.logging.unwrap();
        assert_eq!(logging.console_style.as_deref(), Some("pretty"));
    }

    #[test]
    fn test_apply_defaults_fills_session() {
        let mut config = OpenAlpacaConfig::default();
        apply_defaults(&mut config);
        let session = config.session.unwrap();
        assert_eq!(session.main_key.as_deref(), Some("main"));
        assert_eq!(session.dm_scope.as_deref(), Some("per-peer"));
    }

    #[test]
    fn test_apply_defaults_fills_channel_defaults() {
        let mut config = OpenAlpacaConfig::default();
        apply_defaults(&mut config);
        let ch = config.channels.unwrap();
        let defaults = ch.defaults.unwrap();
        assert_eq!(defaults.dm_policy.as_deref(), Some("allow"));
        assert_eq!(defaults.group_policy.as_deref(), Some("ignore"));
    }

    #[test]
    fn test_apply_defaults_preserves_existing() {
        let mut config = OpenAlpacaConfig::default();
        config.gateway = Some(GatewayConfig {
            port: Some(8080),
            ..Default::default()
        });
        apply_defaults(&mut config);

        let gw = config.gateway.unwrap();
        assert_eq!(gw.port, Some(8080));
        assert_eq!(gw.host.as_deref(), Some("127.0.0.1"));
    }
}
