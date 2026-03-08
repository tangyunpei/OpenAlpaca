use std::sync::Arc;

use openalpaca_channels::{ChannelRegistry, InboundHandler};
use openalpaca_config::OpenAlpacaConfig;

/// Register all built-in channel plugins based on config.
///
/// Each channel is gated behind a compile-time feature flag so the binary
/// can be built without unused channel dependencies.
pub fn register_builtin_channels(
    registry: &mut ChannelRegistry,
    config: &OpenAlpacaConfig,
    http_client: &reqwest::Client,
    handler: Arc<dyn InboundHandler>,
) {
    #[cfg(feature = "telegram")]
    register_telegram(registry, config, http_client, &handler);

    #[cfg(feature = "discord")]
    register_discord(registry, config, &handler);

    #[cfg(feature = "slack")]
    register_slack(registry, config, http_client, &handler);

    let _ = (config, http_client, handler);
}

#[cfg(feature = "telegram")]
fn register_telegram(
    registry: &mut ChannelRegistry,
    config: &OpenAlpacaConfig,
    http_client: &reqwest::Client,
    handler: &Arc<dyn InboundHandler>,
) {
    use openalpaca_channels_telegram::TelegramChannel;

    let account_ids = openalpaca_channels_telegram::config::list_account_ids(config);
    for account_id in &account_ids {
        let Some(acc) =
            openalpaca_channels_telegram::config::resolve_account_config(config, account_id)
        else {
            continue;
        };
        let Some(bot_token) = &acc.bot_token else {
            continue;
        };

        let api = Arc::new(openalpaca_channels_telegram::api::TelegramApi::new(
            http_client.clone(),
            bot_token,
        ));
        let chunk_limit = acc.text_chunk_limit.map(|v| v as usize);
        let channel = TelegramChannel::new(api, handler.clone(), chunk_limit);
        registry.register(account_id.clone(), Box::new(channel));
        tracing::info!(
            "registered telegram channel for account {}",
            account_id.as_str()
        );
    }
}

#[cfg(feature = "discord")]
fn register_discord(
    registry: &mut ChannelRegistry,
    config: &OpenAlpacaConfig,
    handler: &Arc<dyn InboundHandler>,
) {
    use openalpaca_channels_discord::DiscordChannel;

    let account_ids = openalpaca_channels_discord::config::list_account_ids(config);
    for account_id in &account_ids {
        let Some(acc) =
            openalpaca_channels_discord::config::resolve_account_config(config, account_id)
        else {
            continue;
        };
        let Some(bot_token) = &acc.bot_token else {
            continue;
        };

        let channel = DiscordChannel::new(bot_token.clone(), handler.clone(), None);
        registry.register(account_id.clone(), Box::new(channel));
        tracing::info!(
            "registered discord channel for account {}",
            account_id.as_str()
        );
    }
}

#[cfg(feature = "slack")]
fn register_slack(
    registry: &mut ChannelRegistry,
    config: &OpenAlpacaConfig,
    http_client: &reqwest::Client,
    handler: &Arc<dyn InboundHandler>,
) {
    use openalpaca_channels_slack::SlackChannel;

    let account_ids = openalpaca_channels_slack::config::list_account_ids(config);
    for account_id in &account_ids {
        let Some(acc) =
            openalpaca_channels_slack::config::resolve_account_config(config, account_id)
        else {
            continue;
        };
        let Some(bot_token) = &acc.bot_token else {
            continue;
        };
        let Some(app_token) = &acc.app_token else {
            tracing::warn!(
                "slack account {} missing app_token, skipping",
                account_id.as_str()
            );
            continue;
        };

        let api = Arc::new(openalpaca_channels_slack::api::SlackApi::new(
            http_client.clone(),
            bot_token,
        ));
        let channel = SlackChannel::new(
            api,
            handler.clone(),
            http_client.clone(),
            app_token.clone(),
            None,
        );
        registry.register(account_id.clone(), Box::new(channel));
        tracing::info!(
            "registered slack channel for account {}",
            account_id.as_str()
        );
    }
}
