use openalpaca_config::OpenAlpacaConfig;
use openalpaca_config::types_channels::TelegramAccountConfig;
use openalpaca_core::types::AccountId;

/// Default account ID when no explicit accounts are configured.
pub const DEFAULT_ACCOUNT_ID: &str = "default";

/// Resolve the list of configured Telegram account IDs.
pub fn list_account_ids(config: &OpenAlpacaConfig) -> Vec<AccountId> {
    let Some(channels) = &config.channels else {
        return vec![];
    };
    let Some(tg) = &channels.telegram else {
        return vec![];
    };

    if let Some(accounts) = &tg.accounts {
        accounts.keys().map(|k| AccountId(k.clone())).collect()
    } else if tg.default.bot_token.is_some() {
        vec![AccountId(DEFAULT_ACCOUNT_ID.into())]
    } else {
        vec![]
    }
}

/// Resolve the Telegram account config for a given account ID.
pub fn resolve_account_config<'a>(
    config: &'a OpenAlpacaConfig,
    account_id: &AccountId,
) -> Option<&'a TelegramAccountConfig> {
    let tg = config.channels.as_ref()?.telegram.as_ref()?;

    if let Some(accounts) = &tg.accounts {
        accounts.get(&account_id.0)
    } else if account_id.0 == DEFAULT_ACCOUNT_ID {
        Some(&tg.default)
    } else {
        None
    }
}

/// Check if an account is enabled.
pub fn is_account_enabled(config: &OpenAlpacaConfig, account_id: &AccountId) -> bool {
    resolve_account_config(config, account_id)
        .map(|c| c.enabled.unwrap_or(true) && c.bot_token.is_some())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_channels_config() {
        let config = OpenAlpacaConfig::default();
        assert!(list_account_ids(&config).is_empty());
    }

    #[test]
    fn test_default_account_from_flat_config() {
        let yaml = r#"
channels:
  telegram:
    bot_token: "123:ABC"
    enabled: true
"#;
        let config: OpenAlpacaConfig = serde_yml::from_str(yaml).unwrap();
        let ids = list_account_ids(&config);
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0].0, "default");
    }

    #[test]
    fn test_named_accounts() {
        let yaml = r#"
channels:
  telegram:
    accounts:
      work:
        bot_token: "111:AAA"
      personal:
        bot_token: "222:BBB"
"#;
        let config: OpenAlpacaConfig = serde_yml::from_str(yaml).unwrap();
        let ids = list_account_ids(&config);
        assert_eq!(ids.len(), 2);
    }

    #[test]
    fn test_resolve_default_account() {
        let yaml = r#"
channels:
  telegram:
    bot_token: "123:ABC"
"#;
        let config: OpenAlpacaConfig = serde_yml::from_str(yaml).unwrap();
        let acc = resolve_account_config(&config, &AccountId(DEFAULT_ACCOUNT_ID.into()));
        assert!(acc.is_some());
        assert_eq!(acc.unwrap().bot_token.as_deref(), Some("123:ABC"));
    }

    #[test]
    fn test_is_enabled() {
        let yaml = r#"
channels:
  telegram:
    bot_token: "123:ABC"
    enabled: true
"#;
        let config: OpenAlpacaConfig = serde_yml::from_str(yaml).unwrap();
        assert!(is_account_enabled(
            &config,
            &AccountId(DEFAULT_ACCOUNT_ID.into())
        ));
    }

    #[test]
    fn test_disabled_account() {
        let yaml = r#"
channels:
  telegram:
    bot_token: "123:ABC"
    enabled: false
"#;
        let config: OpenAlpacaConfig = serde_yml::from_str(yaml).unwrap();
        assert!(!is_account_enabled(
            &config,
            &AccountId(DEFAULT_ACCOUNT_ID.into())
        ));
    }
}
