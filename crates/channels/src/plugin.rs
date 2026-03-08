use async_trait::async_trait;

use openalpaca_config::OpenAlpacaConfig;
use openalpaca_core::types::{AccountId, ChannelId, ChatType};

use crate::error::ChannelError;
use crate::meta::{ChannelAccountSnapshot, ChannelCapabilities, ChannelMeta};

/// How outbound messages are delivered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DeliveryMode {
    #[default]
    Direct,
    Gateway,
    Hybrid,
}

/// How the channel handles reply threading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReplyToMode {
    #[default]
    Off,
    First,
    All,
}

impl ReplyToMode {
    /// Parse a config string into a `ReplyToMode`.
    pub fn parse(s: &str) -> Self {
        match s.trim().to_lowercase().as_str() {
            "first" => Self::First,
            "all" => Self::All,
            _ => Self::Off,
        }
    }
}

/// Context for outbound media delivery.
#[derive(Debug)]
pub struct MediaOutboundContext {
    pub channel_id: ChannelId,
    pub account_id: AccountId,
    pub target: String,
    pub media_url: String,
    pub caption: Option<String>,
    pub reply_to_id: Option<String>,
    pub thread_id: Option<String>,
}

/// A detected health/configuration issue for a channel account.
#[derive(Debug, Clone)]
pub struct ChannelStatusIssue {
    pub channel_id: ChannelId,
    pub account_id: String,
    pub kind: StatusIssueKind,
    pub message: String,
    pub fix: Option<String>,
}

/// Category of a status issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusIssueKind {
    Intent,
    Permissions,
    Config,
    Auth,
    Runtime,
}

/// Context provided to channel gateway operations (start/stop).
#[derive(Debug, Clone)]
pub struct ChannelGatewayContext {
    pub account_id: AccountId,
    pub config: std::sync::Arc<OpenAlpacaConfig>,
}

/// Context for outbound message delivery.
#[derive(Debug)]
pub struct OutboundContext {
    pub channel_id: ChannelId,
    pub account_id: AccountId,
    pub target: String,
    pub text: String,
    pub reply_to_id: Option<String>,
    pub thread_id: Option<String>,
}

/// Result of an outbound send operation.
#[derive(Debug, Clone)]
pub struct OutboundResult {
    pub message_id: Option<String>,
}

/// The core channel plugin trait.
///
/// All messaging channels implement this trait. Default methods return safe
/// no-ops or `NotSupported` errors, so channels only override what they support.
///
/// # Design Decision: Monolithic vs Split Traits
///
/// This trait is intentionally monolithic rather than split into sub-traits
/// (MediaCapable, ThreadCapable, etc.). Rationale:
/// - Default methods make optional capabilities zero-cost for channels that don't support them
/// - Trait-object downcasting (`as_any().downcast_ref::<dyn MediaCapable>()`) is fragile in Rust
/// - The `ChannelCapabilities` struct already declares what each channel supports
/// - Adding a new optional method only requires a default impl here, not touching every channel
///
/// Uses `#[async_trait]` because `ChannelPlugin` is used as `dyn ChannelPlugin`
/// (trait object dispatch requires it).
#[async_trait]
pub trait ChannelPlugin: Send + Sync + 'static {
    /// Unique channel identifier.
    fn id(&self) -> &ChannelId;

    /// Channel metadata (label, blurb, order, aliases).
    fn meta(&self) -> &ChannelMeta;

    /// Supported capabilities.
    fn capabilities(&self) -> &ChannelCapabilities;

    // --- Config Adapter (required) ---

    /// List all configured account IDs for this channel.
    fn list_account_ids(&self, config: &OpenAlpacaConfig) -> Vec<AccountId>;

    /// Check if a specific account is enabled.
    fn is_account_enabled(&self, config: &OpenAlpacaConfig, account_id: &AccountId) -> bool;

    // --- Gateway Adapter (optional) ---

    /// Start a channel account (connect to API, begin polling/webhook).
    async fn start_account(&self, _ctx: &ChannelGatewayContext) -> Result<(), ChannelError> {
        Ok(())
    }

    /// Stop a channel account (disconnect, cleanup).
    async fn stop_account(&self, _ctx: &ChannelGatewayContext) -> Result<(), ChannelError> {
        Ok(())
    }

    // --- Outbound Adapter (optional) ---

    /// Send a text message to a target.
    async fn send_text(&self, _ctx: &OutboundContext) -> Result<OutboundResult, ChannelError> {
        Err(ChannelError::NotSupported("send_text".into()))
    }

    /// The delivery mode for this channel.
    fn delivery_mode(&self) -> DeliveryMode {
        DeliveryMode::Direct
    }

    /// The character limit for text message chunking.
    /// Returns None to use the platform default from the delivery crate.
    fn text_chunk_limit(&self) -> Option<usize> {
        None
    }

    /// Send a media message (image, file, etc.) to a target.
    async fn send_media(
        &self,
        _ctx: &MediaOutboundContext,
    ) -> Result<OutboundResult, ChannelError> {
        Err(ChannelError::NotSupported("send_media".into()))
    }

    // --- Threading Adapter (optional) ---

    /// Resolve how reply-to IDs should be attached to outbound messages.
    fn resolve_reply_to_mode(
        &self,
        _config: &OpenAlpacaConfig,
        _account_id: Option<&AccountId>,
        _chat_type: Option<&ChatType>,
    ) -> ReplyToMode {
        ReplyToMode::Off
    }

    // --- Mention Adapter (optional) ---

    /// Return regex patterns that should be stripped from inbound message text.
    /// Stored as a field, returned by reference (zero allocation per call).
    fn mention_strip_patterns(&self) -> &[String] {
        &[]
    }

    // --- Group Adapter (optional) ---

    /// Whether group messages require a @mention of the bot to trigger processing.
    fn resolve_require_mention(
        &self,
        _config: &OpenAlpacaConfig,
        _account_id: Option<&AccountId>,
        _group_id: Option<&str>,
    ) -> Option<bool> {
        None
    }

    // --- Messaging Adapter (optional) ---

    /// Normalize a target identifier for outbound delivery.
    /// Default trims whitespace and returns `None` for empty targets.
    fn normalize_target(&self, raw: &str) -> Option<String> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    }

    // --- Heartbeat Adapter (optional) ---

    /// Check if the channel account is ready to send/receive messages.
    async fn check_ready(
        &self,
        _config: &OpenAlpacaConfig,
        _account_id: &AccountId,
    ) -> Result<bool, ChannelError> {
        Ok(true)
    }

    // --- Security Adapter (optional) ---

    /// Resolve the DM policy for an account (e.g., "open", "pairing", "allowlist").
    fn resolve_dm_policy(
        &self,
        _config: &OpenAlpacaConfig,
        _account_id: &AccountId,
    ) -> Option<String> {
        None
    }

    /// Resolve the allow-from list for an account.
    fn resolve_allow_from(
        &self,
        _config: &OpenAlpacaConfig,
        _account_id: &AccountId,
    ) -> Option<Vec<String>> {
        None
    }

    // --- Status Adapter (optional) ---

    /// Build a snapshot of account status for health reporting.
    async fn build_account_snapshot(
        &self,
        _config: &OpenAlpacaConfig,
        _account_id: &AccountId,
    ) -> Option<ChannelAccountSnapshot> {
        None
    }

    /// Collect diagnostic issues for the channel's accounts.
    fn collect_status_issues(
        &self,
        _snapshots: &[ChannelAccountSnapshot],
    ) -> Vec<ChannelStatusIssue> {
        vec![]
    }

    // --- Config Adapter (optional, continued) ---

    /// Resolve the default target for heartbeat messages.
    fn resolve_default_to(
        &self,
        _config: &OpenAlpacaConfig,
        _account_id: &AccountId,
    ) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_delivery_mode_default() {
        assert_eq!(DeliveryMode::default(), DeliveryMode::Direct);
    }

    #[test]
    fn test_reply_to_mode_default() {
        assert_eq!(ReplyToMode::default(), ReplyToMode::Off);
    }

    #[test]
    fn test_reply_to_mode_parse() {
        assert_eq!(ReplyToMode::parse("first"), ReplyToMode::First);
        assert_eq!(ReplyToMode::parse("FIRST"), ReplyToMode::First);
        assert_eq!(ReplyToMode::parse("all"), ReplyToMode::All);
        assert_eq!(ReplyToMode::parse("  All  "), ReplyToMode::All);
        assert_eq!(ReplyToMode::parse("off"), ReplyToMode::Off);
        assert_eq!(ReplyToMode::parse("unknown"), ReplyToMode::Off);
        assert_eq!(ReplyToMode::parse(""), ReplyToMode::Off);
    }

    #[test]
    fn test_channel_status_issue_construction() {
        let issue = ChannelStatusIssue {
            channel_id: ChannelId("telegram".into()),
            account_id: "default".into(),
            kind: StatusIssueKind::Config,
            message: "missing bot token".into(),
            fix: Some("set channels.telegram.bot_token".into()),
        };
        assert_eq!(issue.kind, StatusIssueKind::Config);
        assert_eq!(issue.account_id, "default");
        assert!(issue.fix.is_some());
    }
}
