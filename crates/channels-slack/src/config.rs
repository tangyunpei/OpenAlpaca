use openalpaca_config::OpenAlpacaConfig;
use openalpaca_config::types_channels::SlackAccountConfig;
use openalpaca_core::types::AccountId;

pub const DEFAULT_ACCOUNT_ID: &str = "default";

pub fn list_account_ids(config: &OpenAlpacaConfig) -> Vec<AccountId> {
    let Some(channels) = &config.channels else {
        return vec![];
    };
    let Some(slack) = &channels.slack else {
        return vec![];
    };

    if let Some(accounts) = &slack.accounts {
        accounts.keys().map(|k| AccountId(k.clone())).collect()
    } else if slack.default.bot_token.is_some() {
        vec![AccountId(DEFAULT_ACCOUNT_ID.into())]
    } else {
        vec![]
    }
}

pub fn resolve_account_config<'a>(
    config: &'a OpenAlpacaConfig,
    account_id: &AccountId,
) -> Option<&'a SlackAccountConfig> {
    let slack = config.channels.as_ref()?.slack.as_ref()?;

    if let Some(accounts) = &slack.accounts {
        accounts.get(&account_id.0)
    } else if account_id.0 == DEFAULT_ACCOUNT_ID {
        Some(&slack.default)
    } else {
        None
    }
}

pub fn is_account_enabled(config: &OpenAlpacaConfig, account_id: &AccountId) -> bool {
    resolve_account_config(config, account_id)
        .map(|c| c.enabled.unwrap_or(true) && c.bot_token.is_some())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_slack_config() {
        let config = OpenAlpacaConfig::default();
        assert!(list_account_ids(&config).is_empty());
    }

    #[test]
    fn test_default_account() {
        let yaml = r#"
channels:
  slack:
    bot_token: "xoxb-test"
    app_token: "xapp-test"
    enabled: true
"#;
        let config: OpenAlpacaConfig = serde_yml::from_str(yaml).unwrap();
        let ids = list_account_ids(&config);
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0].0, "default");
        assert!(is_account_enabled(&config, &ids[0]));
    }

    #[test]
    fn test_resolve_account() {
        let yaml = r#"
channels:
  slack:
    bot_token: "xoxb-test"
    app_token: "xapp-test"
"#;
        let config: OpenAlpacaConfig = serde_yml::from_str(yaml).unwrap();
        let acc = resolve_account_config(&config, &AccountId(DEFAULT_ACCOUNT_ID.into())).unwrap();
        assert_eq!(acc.bot_token.as_deref(), Some("xoxb-test"));
        assert_eq!(acc.app_token.as_deref(), Some("xapp-test"));
    }
}
