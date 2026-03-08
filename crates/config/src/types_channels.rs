use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::types_base::OutboundRetryConfig;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ChannelsConfig {
    pub defaults: Option<ChannelDefaultsConfig>,
    pub telegram: Option<TelegramConfig>,
    pub discord: Option<DiscordConfig>,
    pub slack: Option<SlackConfig>,
    pub signal: Option<serde_yml::Value>,
    pub imessage: Option<serde_yml::Value>,
    pub whatsapp: Option<serde_yml::Value>,
    #[serde(flatten)]
    pub extensions: HashMap<String, serde_yml::Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ChannelDefaultsConfig {
    pub dm_policy: Option<String>,
    pub group_policy: Option<String>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_yml::Value>,
}

// --- Telegram ---

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TelegramConfig {
    pub accounts: Option<HashMap<String, TelegramAccountConfig>>,
    pub default_account: Option<String>,
    #[serde(flatten)]
    pub default: TelegramAccountConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TelegramAccountConfig {
    pub enabled: Option<bool>,
    pub bot_token: Option<String>,
    pub token_file: Option<String>,
    pub dm_policy: Option<String>,
    pub group_policy: Option<String>,
    pub allow_from: Option<Vec<serde_yml::Value>>,
    pub webhook_url: Option<String>,
    pub streaming: Option<serde_yml::Value>,
    pub text_chunk_limit: Option<u32>,
    pub reply_to_mode: Option<String>,
    pub retry: Option<OutboundRetryConfig>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_yml::Value>,
}

// --- Discord ---

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DiscordConfig {
    pub accounts: Option<HashMap<String, DiscordAccountConfig>>,
    pub default_account: Option<String>,
    #[serde(flatten)]
    pub default: DiscordAccountConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DiscordAccountConfig {
    pub enabled: Option<bool>,
    pub bot_token: Option<String>,
    pub application_id: Option<String>,
    pub dm_policy: Option<String>,
    pub group_policy: Option<String>,
    pub allow_from: Option<Vec<serde_yml::Value>>,
    pub streaming: Option<serde_yml::Value>,
    pub reply_to_mode: Option<String>,
    pub retry: Option<OutboundRetryConfig>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_yml::Value>,
}

// --- Slack ---

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SlackConfig {
    pub accounts: Option<HashMap<String, SlackAccountConfig>>,
    pub default_account: Option<String>,
    #[serde(flatten)]
    pub default: SlackAccountConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SlackAccountConfig {
    pub enabled: Option<bool>,
    pub bot_token: Option<String>,
    pub app_token: Option<String>,
    pub signing_secret: Option<String>,
    pub dm_policy: Option<String>,
    pub group_policy: Option<String>,
    pub allow_from: Option<Vec<serde_yml::Value>>,
    pub streaming: Option<serde_yml::Value>,
    pub reply_to_mode: Option<String>,
    pub retry: Option<OutboundRetryConfig>,
    #[serde(flatten)]
    pub extra: HashMap<String, serde_yml::Value>,
}
