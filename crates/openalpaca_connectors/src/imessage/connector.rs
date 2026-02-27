//! iMessage Connector Implementation
//!
//! Handles the integration between macOS iMessage (via chat.db polling
//! and AppleScript sending) and the OpenAlpaca agent system.
//!
//! Unlike bot-based connectors (Telegram), iMessage is a native macOS
//! integration where the Mac owner is always the trusted principal.
//! Messages are optionally filtered by a trigger prefix (`/ask` or `@openalpaca`).
//! Prefix requirements are configurable per-chat-type via `direct_require_prefix`
//! and `group_require_prefix` settings.

use crate::{Connector, ConnectorError};
use arc_swap::ArcSwap;
use async_trait::async_trait;
use openalpaca_api::events::EventSource;
use openalpaca_core::{
    daemon_config::DaemonConfig,
    gateway::{Gateway, GatewayRequest, ResolvedAttachment},
    security::policy::{Principal, Scope},
};
use openalpaca_storage::{Database, IdentityRepository, PreferenceRepository};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use super::reader::{ChatDbReader, IncomingMessage};
use super::sender::IMessageSender;

/// Built-in trigger prefixes. A message must start with one of these to be processed.
const TRIGGER_PREFIXES: &[&str] = &["/ask", "@openalpaca"];

/// Check if the message starts with any trigger prefix.
/// Returns the content after the prefix (trimmed), or None if no prefix matched.
fn strip_trigger_prefix(text: &str) -> Option<String> {
    for prefix in TRIGGER_PREFIXES {
        if let Some(rest) = text.strip_prefix(prefix) {
            let content = rest.trim_start().to_string();
            return Some(content);
        }
    }
    None
}

/// Normalize an iMessage handle for comparison.
///
/// - Phone numbers: strip all non-digit characters (e.g. "+1 (555) 123-4567" -> "15551234567")
/// - Emails: lowercase and trim whitespace
/// - Empty/whitespace-only: return empty string
fn normalize_handle(handle: &str) -> String {
    let trimmed = handle.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    // Emails: detect by '@' first to avoid misclassifying digit-prefixed emails
    // (e.g. "123user@example.com") as phone numbers.
    if trimmed.contains('@') {
        return trimmed.to_lowercase();
    }
    // Phone numbers: starts with + or digit, strip all non-digits
    if trimmed.starts_with('+') || trimmed.chars().next().is_some_and(|c| c.is_ascii_digit()) {
        return trimmed.chars().filter(|c| c.is_ascii_digit()).collect();
    }
    // Otherwise lowercase
    trimmed.to_lowercase()
}

/// Runtime configuration for iMessage message routing.
struct IMessageConfig {
    /// Whether to include self-sent messages in polling.
    allow_from_me: bool,
    /// Require trigger prefix for direct (1-to-1) messages.
    direct_require_prefix: bool,
    /// Require trigger prefix for group chat messages.
    group_require_prefix: bool,
    /// Normalized owner handles for identifying self-sent DMs.
    owner_handles: Vec<String>,
    /// The bot's iMessage account identifier (e.g. "e:bot@icloud.com" or "p:+1234567890").
    /// Messages from this account are skipped to prevent the bot from processing its own replies.
    bot_handle: Option<String>,
}

/// Result of evaluating whether a message should be processed.
enum ProcessDecision {
    /// Accept the message with the given content (after prefix stripping if applicable).
    Process { content: String },
    /// Skip the message for the given reason.
    Skip(&'static str),
}

/// Decide whether an incoming message should be processed based on chat type,
/// sender identity, and prefix configuration.
///
/// For DMs (non-group):
///   - If the message is from the owner (is_from_me or sender in owner_handles),
///     apply `direct_require_prefix` to decide whether a prefix is needed.
///   - If the message is from a non-owner, always require a prefix.
///
/// For groups:
///   - Apply `group_require_prefix` to decide whether a prefix is needed.
fn should_process(msg: &IncomingMessage, config: &IMessageConfig) -> ProcessDecision {
    // Filter out messages sent by the bot itself (prevents feedback loop).
    // Only match is_from_me=true messages: in chat.db, msg.account records the LOCAL
    // iMessage account for ALL messages (both sent and received on that account).
    // Without the is_from_me guard, incoming messages would be incorrectly skipped.
    if let Some(ref bot_handle) = config.bot_handle
        && msg.is_from_me
        && !msg.account.is_empty()
        && msg.account.ends_with(bot_handle.as_str())
    {
        return ProcessDecision::Skip("bot_own_message");
    }

    let has_attachments = !msg.attachments.is_empty();
    let text = msg.text.trim();

    if msg.is_group {
        // Group messages
        if config.group_require_prefix {
            return match strip_trigger_prefix(text) {
                Some(c) if !c.is_empty() => ProcessDecision::Process { content: c },
                Some(_) if has_attachments => ProcessDecision::Process {
                    content: String::new(),
                },
                Some(_) => ProcessDecision::Skip("group_empty_after_prefix"),
                None => ProcessDecision::Skip("group_no_prefix"),
            };
        }
        // No prefix required for groups
        let content = strip_trigger_prefix(text).unwrap_or_else(|| text.to_string());
        if content.is_empty() && !has_attachments {
            return ProcessDecision::Skip("group_empty");
        }
        return ProcessDecision::Process { content };
    }

    // Direct messages (1-to-1)
    let is_owner = msg.is_from_me || {
        let normalized_sender = normalize_handle(&msg.sender);
        !normalized_sender.is_empty()
            && config.owner_handles.contains(&normalized_sender)
    };

    let require_prefix = if is_owner {
        config.direct_require_prefix
    } else {
        // Non-owner DMs always require prefix for security
        true
    };

    if require_prefix {
        return match strip_trigger_prefix(text) {
            Some(c) if !c.is_empty() => ProcessDecision::Process { content: c },
            Some(_) if has_attachments => ProcessDecision::Process {
                content: String::new(),
            },
            Some(_) => ProcessDecision::Skip("dm_empty_after_prefix"),
            None if is_owner => ProcessDecision::Skip("dm_owner_no_prefix"),
            None => ProcessDecision::Skip("dm_non_owner_no_prefix"),
        };
    }

    // Owner DM, no prefix required — use full text or strip prefix if present
    let content = strip_trigger_prefix(text).unwrap_or_else(|| text.to_string());
    if content.is_empty() && !has_attachments {
        return ProcessDecision::Skip("dm_empty");
    }
    ProcessDecision::Process { content }
}

/// IMessageConnector manages the iMessage integration lifecycle.
///
/// It polls `~/Library/Messages/chat.db` for new incoming messages and
/// routes them through the OpenAlpaca gateway, sending responses back
/// via AppleScript (`osascript`).
///
/// The macOS system user is always the trusted principal — no `/link`
/// flow is needed.
pub struct IMessageConnector {
    db: Arc<Database>,
    gateway: Arc<Gateway>,
    daemon_config: Arc<ArcSwap<DaemonConfig>>,
    cancel_token: CancellationToken,
    chat_db_path: String,
    local_user_id: Option<String>,
}

impl IMessageConnector {
    /// Create a new IMessageConnector.
    ///
    /// The chat.db path defaults to `~/Library/Messages/chat.db`.
    /// If `local_user_id` is provided, it is used as the principal identity
    /// for all messages (bypassing the heuristic `resolve_owner` fallback).
    pub fn new(
        db: Arc<Database>,
        gateway: Arc<Gateway>,
        daemon_config: Arc<ArcSwap<DaemonConfig>>,
        cancel_token: CancellationToken,
        local_user_id: Option<String>,
    ) -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/Users".to_string());
        let chat_db_path = format!("{}/Library/Messages/chat.db", home);
        Self {
            db,
            gateway,
            daemon_config,
            cancel_token,
            chat_db_path,
            local_user_id,
        }
    }

    /// Main polling loop.
    ///
    /// Restores the persisted ROWID watermark so that messages received
    /// while the connector was offline are still processed. On first run
    /// (no persisted watermark), initializes to the current max ROWID to
    /// avoid replaying the entire history.
    pub async fn run_loop(&self) -> Result<(), ConnectorError> {
        info!("Starting iMessage connector...");
        let mut reader =
            ChatDbReader::new(&self.chat_db_path).map_err(ConnectorError::InitFailed)?;

        // Restore persisted watermark, or initialize to current max ROWID
        let config_repo = openalpaca_storage::ConfigRepository::new(&self.db);
        let persisted_watermark = config_repo
            .get("imessage.last_rowid")
            .ok()
            .flatten()
            .and_then(|v| match v.parse::<i64>() {
                Ok(n) => Some(n),
                Err(e) => {
                    warn!("Corrupt imessage.last_rowid '{v}': {e}, re-initializing");
                    None
                }
            });

        if let Some(watermark) = persisted_watermark {
            reader.set_watermark(watermark);
            info!(
                "iMessage connector restored watermark to ROWID {}",
                watermark
            );
        } else {
            reader
                .initialize_watermark()
                .map_err(ConnectorError::InitFailed)?;
            info!("iMessage connector initialized, watermark set to current max ROWID");
        }
        // Log initial config on startup
        let initial_config = Self::load_imessage_config(&self.db);
        info!(
            allow_from_me = initial_config.allow_from_me,
            direct_require_prefix = initial_config.direct_require_prefix,
            group_require_prefix = initial_config.group_require_prefix,
            owner_handles_count = initial_config.owner_handles.len(),
            bot_handle = initial_config.bot_handle.as_deref().unwrap_or("(not set)"),
            "iMessage connector started with routing config"
        );

        loop {
            tokio::select! {
                _ = self.cancel_token.cancelled() => {
                    info!("iMessage connector shutting down");
                    Self::persist_watermark(&self.db, reader.watermark());
                    return Ok(());
                }
                _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => {
                    // Re-read config each poll cycle so GUI/CLI changes take effect immediately
                    let imsg_config = Self::load_imessage_config(&self.db);

                    match reader.poll_new_messages(imsg_config.allow_from_me) {
                        Ok(messages) => {
                            let had_messages = !messages.is_empty();
                            for msg in messages {
                                if let Err(e) = self.handle_message(msg, &imsg_config).await {
                                    error!("Failed to handle iMessage: {}", e);
                                }
                            }
                            // Persist watermark after processing so offline
                            // messages are not lost on restart.
                            if had_messages {
                                Self::persist_watermark(&self.db, reader.watermark());
                            }
                        }
                        Err(e) => {
                            warn!("Failed to poll chat.db: {}", e);
                        }
                    }
                }
            }
        }
    }

    /// Persist the ROWID watermark to the database.
    fn persist_watermark(db: &Database, watermark: i64) {
        let config_repo = openalpaca_storage::ConfigRepository::new(db);
        if let Err(e) = config_repo.set(
            "imessage.last_rowid",
            &watermark.to_string(),
            "int",
        ) {
            warn!("Failed to persist iMessage watermark: {e}");
        }
    }

    /// Load iMessage routing configuration from the database.
    ///
    /// Called on each poll cycle so that changes made via the GUI or CLI
    /// take effect without restarting the connector.
    fn load_imessage_config(db: &Database) -> IMessageConfig {
        let config_repo = openalpaca_storage::ConfigRepository::new(db);

        let allow_from_me = config_repo
            .get_or_default("imessage.allow_from_me")
            .ok()
            .flatten()
            .map(|v| matches!(v.as_str(), "true" | "1" | "yes"))
            .unwrap_or(true);

        let direct_require_prefix = config_repo
            .get_or_default("imessage.direct_require_prefix")
            .ok()
            .flatten()
            .map(|v| matches!(v.as_str(), "true" | "1" | "yes"))
            .unwrap_or(false);

        // Canonical values: "true"/"false". Also accepts "1"/"yes" for convenience.
        let group_require_prefix = config_repo
            .get_or_default("imessage.group_require_prefix")
            .ok()
            .flatten()
            .map(|v| matches!(v.as_str(), "true" | "1" | "yes"))
            .unwrap_or(true);

        let mut owner_handles: Vec<String> = config_repo
            .get("imessage.owner_handles")
            .ok()
            .flatten()
            .map(|v| {
                v.split(',')
                    .map(normalize_handle)
                    .filter(|h| !h.is_empty())
                    .collect()
            })
            .unwrap_or_default();

        let bot_handle = config_repo
            .get("imessage.bot_handle")
            .ok()
            .flatten()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());

        // Auto-populate owner_handles from bot_handle when empty.
        // This ensures that self-sent messages (e.g. from iPhone) are recognized
        // as owner messages without requiring explicit owner_handles configuration.
        if owner_handles.is_empty()
            && let Some(ref bh) = bot_handle
        {
            let normalized = normalize_handle(bh);
            if !normalized.is_empty() {
                owner_handles.push(normalized);
            }
        }

        // Safety: force-disable allow_from_me when bot_handle is missing
        // to prevent feedback loops (bot processes its own replies)
        let allow_from_me = if bot_handle.is_none() && allow_from_me {
            use std::sync::atomic::{AtomicBool, Ordering};
            static WARNED: AtomicBool = AtomicBool::new(false);
            if !WARNED.swap(true, Ordering::Relaxed) {
                warn!(
                    "imessage.bot_handle not configured — forcing allow_from_me=false to prevent feedback loop. \
                     Set imessage.bot_handle to the Mac's iMessage sending address to enable allow_from_me."
                );
            }
            false
        } else {
            allow_from_me
        };

        IMessageConfig {
            allow_from_me,
            direct_require_prefix,
            group_require_prefix,
            owner_handles,
            bot_handle,
        }
    }

    /// Resolve the macOS owner as a GlobalUser principal.
    ///
    /// Resolution order:
    /// 1. Use the explicit `local_user_id` passed at construction (from daemon bootstrap).
    /// 2. Read `identity.local_user_id` from `system_config` (standalone mode).
    /// 3. Last resort: create a new global user from the macOS username.
    fn resolve_owner(&self) -> Result<Principal, String> {
        // 1. Prefer explicit local_user_id from daemon bootstrap
        if let Some(ref id) = self.local_user_id {
            return Ok(Principal::User {
                global_id: id.clone(),
            });
        }

        // 2. Fallback: read from system_config (standalone mode)
        let config_repo = openalpaca_storage::ConfigRepository::new(&self.db);
        if let Ok(Some(id)) = config_repo.get("identity.local_user_id") {
            return Ok(Principal::User { global_id: id });
        }

        // 3. Last resort: create from macOS username
        let identity_repo = IdentityRepository::new(&self.db);
        let username = std::env::var("USER")
            .or_else(|_| std::env::var("LOGNAME"))
            .unwrap_or_else(|_| "mac-owner".to_string());
        let global_id = uuid::Uuid::new_v4().to_string();

        identity_repo
            .create_global_user(&global_id, Some(&username))
            .map_err(|e| format!("Failed to create global user: {e}"))?;

        Ok(Principal::User { global_id })
    }

    /// Handle a single incoming iMessage.
    async fn handle_message(
        &self,
        msg: IncomingMessage,
        config: &IMessageConfig,
    ) -> Result<(), String> {
        // Compute stable reply target:
        // - DMs: use sender handle (phone/email) — consistent across chat_identifier variants
        // - Groups: use chat_id (group identifier required by AppleScript "chat id")
        let (reply_target, reply_is_group) = if msg.is_group {
            (msg.chat_id.clone(), true)
        } else if !msg.sender.is_empty() {
            (msg.sender.clone(), false)
        } else {
            // Fallback for is_from_me=1 with empty sender
            (msg.chat_id.clone(), false)
        };

        if reply_target != msg.chat_id {
            debug!(
                reply_target = %reply_target,
                chat_id = %msg.chat_id,
                "Reply target differs from chat_id (DM sender-based addressing)"
            );
        }

        // Step 1: Evaluate routing decision
        let content = match should_process(&msg, config) {
            ProcessDecision::Process { content } => {
                info!(
                    chat_id = %msg.chat_id,
                    sender = %msg.sender,
                    is_group = msg.is_group,
                    is_from_me = msg.is_from_me,
                    account = %msg.account,
                    reply_target = %reply_target,
                    "Accepted iMessage for processing"
                );
                content
            }
            ProcessDecision::Skip(reason) => {
                debug!(
                    chat_id = %msg.chat_id,
                    sender = %msg.sender,
                    is_group = msg.is_group,
                    is_from_me = msg.is_from_me,
                    account = %msg.account,
                    reason,
                    "Skipped iMessage"
                );
                // Send usage hint for empty-after-prefix cases
                if reason.ends_with("empty_after_prefix") {
                    IMessageSender::send(
                        &reply_target,
                        "Usage: /ask <your question> or @openalpaca <your question>",
                        reply_is_group,
                    )
                    .await
                    .ok();
                }
                return Ok(());
            }
        };

        // Step 2: Resolve macOS owner as principal (always Principal::User)
        let principal = self.resolve_owner()?;

        let global_id = match &principal {
            Principal::User { global_id } => global_id.clone(),
            _ => unreachable!("resolve_owner always returns Principal::User"),
        };

        // Step 2.5: Store attachments
        let mut attachments: Vec<ResolvedAttachment> = Vec::new();
        let upload_cfg = self.daemon_config.load();
        let max_file_size = upload_cfg.upload.max_file_size_bytes;
        let max_img_dim = upload_cfg.upload.governance.max_image_dimension;
        for att in &msg.attachments {
            let path = std::path::Path::new(&att.file_path);
            if !path.exists() {
                warn!("iMessage attachment file not found: {}", att.file_path);
                continue;
            }
            match std::fs::read(path) {
                Ok(data) => {
                    let name = if att.transfer_name.is_empty() {
                        path.file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_else(|| "attachment".into())
                    } else {
                        att.transfer_name.clone()
                    };
                    match crate::common::store_attachment(
                        &self.db,
                        &global_id,
                        &name,
                        &att.mime_type,
                        &data,
                        max_file_size,
                        max_img_dim,
                    ) {
                        Ok(resolved) => attachments.push(resolved),
                        Err(e) => warn!("Failed to store iMessage attachment: {e}"),
                    }
                }
                Err(e) => warn!("Failed to read iMessage attachment {}: {e}", att.file_path),
            }
        }

        // If all attachments failed and content is empty, skip to avoid nonsensical response
        if content.is_empty() && attachments.is_empty() && !msg.attachments.is_empty() {
            warn!(
                chat_id = %msg.chat_id,
                "All iMessage attachments failed to store and message content is empty, skipping"
            );
            return Ok(());
        }

        // Step 3: Route through Gateway
        let response = self
            .gateway
            .handle_event(GatewayRequest {
                source: EventSource::IMessage {
                    chat_id: msg.chat_id.clone(),
                    sender: msg.sender.clone(),
                },
                content,
                attachments,
                principal,
                scope: Scope::Conversation {
                    id: msg.chat_id.clone(),
                },
                workspace_path: None,
            })
            .await;

        // Step 3.5: Map external chat_id to internal lane_key
        let identity_repo = IdentityRepository::new(&self.db);
        let lane_key = response.lane_key.to_string();
        if let Err(e) =
            identity_repo.update_conversation_map_lane_key("imessage", &msg.chat_id, &lane_key)
        {
            warn!("Failed to update conversation_map lane_key: {e}");
        }

        // Step 3.6: Persist reply metadata for cross-channel delivery / notifications
        let pref_repo = PreferenceRepository::new(&self.db);
        if let Err(e) = pref_repo.set(&global_id, "imessage.last_reply_target", &reply_target, None) {
            warn!("Failed to persist imessage.last_reply_target: {e}");
        }
        if let Err(e) = pref_repo.set(&global_id, "imessage.last_is_group", &reply_is_group.to_string(), None) {
            warn!("Failed to persist imessage.last_is_group: {e}");
        }
        // Persist the receiving account so outbound sends use the same one
        if !msg.account.is_empty()
            && let Err(e) = pref_repo.set(&global_id, "imessage.last_send_account", &msg.account, None)
        {
            warn!("Failed to persist imessage.last_send_account: {e}");
        }

        // Step 4: Send response using stable reply target + the same account that received
        let from_account = if msg.account.is_empty() {
            None
        } else {
            Some(msg.account.as_str())
        };
        IMessageSender::send_from(&reply_target, &response.content, reply_is_group, from_account)
            .await
            .map_err(|e| format!("Send failed: {}", e))?;

        Ok(())
    }
}

#[async_trait]
impl Connector for IMessageConnector {
    fn name(&self) -> &'static str {
        "imessage"
    }

    async fn run(&self) -> Result<(), ConnectorError> {
        self.run_loop().await
    }

    async fn shutdown(&self) -> Result<(), ConnectorError> {
        info!("iMessage connector shutdown requested");
        self.cancel_token.cancel();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_msg(text: &str, sender: &str, chat_id: &str, is_from_me: bool) -> IncomingMessage {
        IncomingMessage {
            rowid: 1,
            text: text.to_string(),
            sender: sender.to_string(),
            chat_id: chat_id.to_string(),
            is_group: chat_id.starts_with("chat"),
            is_from_me,
            account: "e:user@iphone.com".to_string(),
            attachments: Vec::new(),
        }
    }

    fn make_msg_with_account(
        text: &str,
        sender: &str,
        chat_id: &str,
        is_from_me: bool,
        account: &str,
    ) -> IncomingMessage {
        IncomingMessage {
            rowid: 1,
            text: text.to_string(),
            sender: sender.to_string(),
            chat_id: chat_id.to_string(),
            is_group: chat_id.starts_with("chat"),
            is_from_me,
            account: account.to_string(),
            attachments: Vec::new(),
        }
    }

    fn make_msg_with_attachments(
        text: &str,
        sender: &str,
        chat_id: &str,
        is_from_me: bool,
    ) -> IncomingMessage {
        use super::super::reader::IMessageAttachment;
        IncomingMessage {
            rowid: 1,
            text: text.to_string(),
            sender: sender.to_string(),
            chat_id: chat_id.to_string(),
            is_group: chat_id.starts_with("chat"),
            is_from_me,
            account: "e:user@iphone.com".to_string(),
            attachments: vec![IMessageAttachment {
                filename: "photo.jpg".into(),
                mime_type: "image/jpeg".into(),
                transfer_name: "photo.jpg".into(),
                file_path: "/tmp/photo.jpg".into(),
                total_bytes: 1024,
            }],
        }
    }

    fn owner_config() -> IMessageConfig {
        IMessageConfig {
            allow_from_me: true,
            direct_require_prefix: false,
            group_require_prefix: true,
            owner_handles: vec!["15551234567".into(), "owner@example.com".into()],
            bot_handle: Some("bot@mac.com".into()),
        }
    }

    fn no_owner_config() -> IMessageConfig {
        IMessageConfig {
            allow_from_me: true,
            direct_require_prefix: false,
            group_require_prefix: true,
            owner_handles: Vec::new(),
            bot_handle: None,
        }
    }

    fn prefix_required_config() -> IMessageConfig {
        IMessageConfig {
            allow_from_me: true,
            direct_require_prefix: true,
            group_require_prefix: true,
            owner_handles: Vec::new(),
            bot_handle: None,
        }
    }

    fn is_process(decision: &ProcessDecision) -> bool {
        matches!(decision, ProcessDecision::Process { .. })
    }

    fn get_content(decision: ProcessDecision) -> String {
        match decision {
            ProcessDecision::Process { content } => content,
            ProcessDecision::Skip(reason) => panic!("Expected Process, got Skip({reason})"),
        }
    }

    fn get_skip_reason(decision: ProcessDecision) -> &'static str {
        match decision {
            ProcessDecision::Skip(reason) => reason,
            ProcessDecision::Process { .. } => panic!("Expected Skip, got Process"),
        }
    }

    // ── strip_trigger_prefix ──

    #[test]
    fn test_strip_trigger_prefix_ask() {
        assert_eq!(
            strip_trigger_prefix("/ask what is rust"),
            Some("what is rust".into())
        );
    }

    #[test]
    fn test_strip_trigger_prefix_at() {
        assert_eq!(
            strip_trigger_prefix("@openalpaca hello"),
            Some("hello".into())
        );
    }

    #[test]
    fn test_strip_trigger_prefix_no_match() {
        assert_eq!(strip_trigger_prefix("hello world"), None);
    }

    #[test]
    fn test_strip_trigger_prefix_empty_after() {
        assert_eq!(strip_trigger_prefix("/ask"), Some(String::new()));
        assert_eq!(strip_trigger_prefix("/ask   "), Some(String::new()));
    }

    // ── normalize_handle ──

    #[test]
    fn test_normalize_phone_international() {
        assert_eq!(normalize_handle("+1 (555) 123-4567"), "15551234567");
    }

    #[test]
    fn test_normalize_phone_local() {
        assert_eq!(normalize_handle("5551234567"), "5551234567");
    }

    #[test]
    fn test_normalize_email() {
        assert_eq!(normalize_handle("Owner@Example.COM"), "owner@example.com");
    }

    #[test]
    fn test_normalize_email_whitespace() {
        assert_eq!(normalize_handle("  user@test.com  "), "user@test.com");
    }

    #[test]
    fn test_normalize_empty() {
        assert_eq!(normalize_handle(""), "");
        assert_eq!(normalize_handle("   "), "");
    }

    // ── should_process: DM from owner (is_from_me) ──

    #[test]
    fn test_dm_owner_from_me_no_prefix_required() {
        let config = owner_config();
        let msg = make_msg("hello world", "", "+15551234567", true);
        let decision = should_process(&msg, &config);
        assert_eq!(get_content(decision), "hello world");
    }

    #[test]
    fn test_dm_owner_from_me_with_prefix() {
        let config = owner_config();
        let msg = make_msg("/ask hello world", "", "+15551234567", true);
        let decision = should_process(&msg, &config);
        assert_eq!(get_content(decision), "hello world");
    }

    #[test]
    fn test_dm_owner_from_me_prefix_required() {
        let mut config = owner_config();
        config.direct_require_prefix = true;
        let msg = make_msg("hello world", "", "+15551234567", true);
        let decision = should_process(&msg, &config);
        assert_eq!(get_skip_reason(decision), "dm_owner_no_prefix");
    }

    // ── should_process: DM from owner (via owner_handles) ──

    #[test]
    fn test_dm_owner_handle_match_phone() {
        let config = owner_config();
        let msg = make_msg("hello", "+1 (555) 123-4567", "+15551234567", false);
        let decision = should_process(&msg, &config);
        assert_eq!(get_content(decision), "hello");
    }

    #[test]
    fn test_dm_owner_handle_match_email() {
        let config = owner_config();
        let msg = make_msg("hello", "Owner@Example.COM", "+15551234567", false);
        let decision = should_process(&msg, &config);
        assert_eq!(get_content(decision), "hello");
    }

    // ── should_process: DM from non-owner ──

    #[test]
    fn test_dm_non_owner_no_prefix() {
        let config = owner_config();
        let msg = make_msg("hello", "+19995550000", "+19995550000", false);
        let decision = should_process(&msg, &config);
        assert_eq!(get_skip_reason(decision), "dm_non_owner_no_prefix");
    }

    #[test]
    fn test_dm_non_owner_with_prefix() {
        let config = owner_config();
        let msg = make_msg("/ask hello", "+19995550000", "+19995550000", false);
        let decision = should_process(&msg, &config);
        assert_eq!(get_content(decision), "hello");
    }

    // ── should_process: Group messages ──

    #[test]
    fn test_group_with_prefix() {
        let config = owner_config();
        let msg = make_msg("/ask question", "+15551234567", "chat12345", false);
        let decision = should_process(&msg, &config);
        assert_eq!(get_content(decision), "question");
    }

    #[test]
    fn test_group_no_prefix_required() {
        let config = owner_config();
        let msg = make_msg("question", "+15551234567", "chat12345", false);
        let decision = should_process(&msg, &config);
        assert_eq!(get_skip_reason(decision), "group_no_prefix");
    }

    #[test]
    fn test_group_prefix_not_required() {
        let mut config = owner_config();
        config.group_require_prefix = false;
        let msg = make_msg("question", "+15551234567", "chat12345", false);
        let decision = should_process(&msg, &config);
        assert_eq!(get_content(decision), "question");
    }

    #[test]
    fn test_group_empty_after_prefix() {
        let config = owner_config();
        let msg = make_msg("/ask", "+15551234567", "chat12345", false);
        let decision = should_process(&msg, &config);
        assert_eq!(get_skip_reason(decision), "group_empty_after_prefix");
    }

    // ── should_process: Edge cases ──

    #[test]
    fn test_dm_empty_text() {
        let config = owner_config();
        let msg = make_msg("", "", "+15551234567", true);
        let decision = should_process(&msg, &config);
        assert_eq!(get_skip_reason(decision), "dm_empty");
    }

    #[test]
    fn test_dm_whitespace_only() {
        let config = owner_config();
        let msg = make_msg("   ", "", "+15551234567", true);
        let decision = should_process(&msg, &config);
        assert_eq!(get_skip_reason(decision), "dm_empty");
    }

    #[test]
    fn test_dm_attachment_only_no_prefix_required() {
        let config = owner_config();
        let msg = make_msg_with_attachments("", "", "+15551234567", true);
        let decision = should_process(&msg, &config);
        assert!(is_process(&decision));
        assert_eq!(get_content(decision), "");
    }

    #[test]
    fn test_dm_attachment_only_prefix_required() {
        let mut config = owner_config();
        config.direct_require_prefix = true;
        let msg = make_msg_with_attachments("", "", "+15551234567", true);
        let decision = should_process(&msg, &config);
        // Empty text with no prefix match -> skip
        assert_eq!(get_skip_reason(decision), "dm_owner_no_prefix");
    }

    #[test]
    fn test_dm_attachment_with_prefix_only() {
        let config = prefix_required_config();
        let msg = make_msg_with_attachments("/ask", "+15551234567", "+15551234567", true);
        let decision = should_process(&msg, &config);
        assert!(is_process(&decision));
        assert_eq!(get_content(decision), "");
    }

    #[test]
    fn test_no_owner_handles_is_from_me_no_prefix_needed() {
        // With no owner_handles, is_from_me=true DMs still work without prefix
        let config = no_owner_config();
        let msg = make_msg("hello", "", "+15551234567", true);
        let decision = should_process(&msg, &config);
        assert_eq!(get_content(decision), "hello");
    }

    #[test]
    fn test_no_owner_handles_non_owner_requires_prefix() {
        // With no owner_handles, is_from_me=false requires prefix (can't verify sender)
        let config = no_owner_config();
        let msg = make_msg("hello", "+19995550000", "+19995550000", false);
        let decision = should_process(&msg, &config);
        assert_eq!(get_skip_reason(decision), "dm_non_owner_no_prefix");
    }

    #[test]
    fn test_no_owner_handles_with_prefix_works() {
        let config = no_owner_config();
        let msg = make_msg("/ask hello", "", "+15551234567", true);
        let decision = should_process(&msg, &config);
        assert_eq!(get_content(decision), "hello");
    }

    #[test]
    fn test_explicit_prefix_required_forces_prefix() {
        // When direct_require_prefix is explicitly true, prefix is needed even for owner
        let config = prefix_required_config();
        let msg = make_msg("hello", "", "+15551234567", true);
        let decision = should_process(&msg, &config);
        assert_eq!(get_skip_reason(decision), "dm_owner_no_prefix");
    }

    // ── should_process: bot_handle filtering ──

    #[test]
    fn test_bot_handle_skips_own_message() {
        let config = owner_config(); // bot_handle = Some("bot@mac.com")
        // is_from_me=true: this is the bot's own outgoing reply
        let msg = make_msg_with_account("hello", "", "+15551234567", true, "e:bot@mac.com");
        let decision = should_process(&msg, &config);
        assert_eq!(get_skip_reason(decision), "bot_own_message");
    }

    #[test]
    fn test_bot_handle_does_not_skip_incoming_message() {
        // is_from_me=false: incoming message where account matches bot_handle
        // (because chat.db records the LOCAL receiving account for incoming msgs too).
        // This must NOT be skipped — it's a real incoming message.
        let config = owner_config(); // bot_handle = Some("bot@mac.com")
        let msg = make_msg_with_account(
            "/ask hello",
            "+19995550000",
            "+19995550000",
            false,
            "e:bot@mac.com",
        );
        let decision = should_process(&msg, &config);
        assert!(is_process(&decision));
        assert_eq!(get_content(decision), "hello");
    }

    #[test]
    fn test_bot_handle_allows_other_account() {
        let config = owner_config(); // bot_handle = Some("bot@mac.com")
        let msg = make_msg_with_account("hello", "", "+15551234567", true, "e:user@iphone.com");
        let decision = should_process(&msg, &config);
        assert!(is_process(&decision));
        assert_eq!(get_content(decision), "hello");
    }

    #[test]
    fn test_no_bot_handle_allows_all() {
        let config = no_owner_config(); // bot_handle = None
        let msg = make_msg_with_account("hello", "", "+15551234567", true, "e:bot@mac.com");
        let decision = should_process(&msg, &config);
        assert!(is_process(&decision));
    }

    #[test]
    fn test_bot_handle_empty_account_not_skipped() {
        let config = owner_config(); // bot_handle = Some("bot@mac.com")
        let msg = make_msg_with_account("hello", "", "+15551234567", true, "");
        let decision = should_process(&msg, &config);
        // Empty account should NOT match bot_handle
        assert!(is_process(&decision));
    }

    #[test]
    fn test_bot_handle_suffix_match() {
        // bot_handle uses ends_with() matching to handle "p:"/"e:" prefixes
        let mut config = owner_config();
        config.bot_handle = Some("+1234567890".into());
        let msg = make_msg_with_account("hello", "", "+15551234567", true, "p:+1234567890");
        let decision = should_process(&msg, &config);
        assert_eq!(get_skip_reason(decision), "bot_own_message");
    }

    #[test]
    fn test_bot_handle_no_over_match() {
        // Short bot_handle should not accidentally match unrelated accounts
        let mut config = owner_config();
        config.bot_handle = Some("bot".into());
        let msg = make_msg_with_account("hello", "", "+15551234567", true, "e:notbot@mac.com");
        let decision = should_process(&msg, &config);
        // "notbot@mac.com" does not end with "bot", so should NOT be skipped
        assert!(is_process(&decision));
    }

    #[test]
    fn test_normalize_digit_prefixed_email() {
        // Digit-prefixed emails should not be misclassified as phone numbers
        assert_eq!(
            normalize_handle("123user@example.com"),
            "123user@example.com"
        );
        assert_eq!(
            normalize_handle("+user@example.com"),
            "+user@example.com"
        );
    }

    #[test]
    fn test_bot_handle_group_message_also_filtered() {
        // Bot's own messages in group chats should also be filtered
        let config = owner_config(); // bot_handle = Some("bot@mac.com")
        let msg = make_msg_with_account("/ask hello", "", "chat12345", true, "e:bot@mac.com");
        let decision = should_process(&msg, &config);
        assert_eq!(get_skip_reason(decision), "bot_own_message");
    }

    // ── Reply target computation ──

    #[test]
    fn test_dm_reply_target_uses_sender() {
        // For DMs, reply_target should be the sender handle (not the varying chat_id)
        let msg = make_msg("/ask hello", "+19995550000", "iMessage;-;+19995550000", false);
        // Simulate the reply_target logic from handle_message
        let (reply_target, reply_is_group) = if msg.is_group {
            (msg.chat_id.clone(), true)
        } else if !msg.sender.is_empty() {
            (msg.sender.clone(), false)
        } else {
            (msg.chat_id.clone(), false)
        };
        assert_eq!(reply_target, "+19995550000");
        assert!(!reply_is_group);
    }

    #[test]
    fn test_group_reply_target_uses_chat_id() {
        // For groups, reply_target should be the chat_id (group identifier)
        let msg = make_msg("/ask hello", "+19995550000", "chat12345", false);
        let (reply_target, reply_is_group) = if msg.is_group {
            (msg.chat_id.clone(), true)
        } else if !msg.sender.is_empty() {
            (msg.sender.clone(), false)
        } else {
            (msg.chat_id.clone(), false)
        };
        assert_eq!(reply_target, "chat12345");
        assert!(reply_is_group);
    }

    #[test]
    fn test_dm_empty_sender_falls_back_to_chat_id() {
        // is_from_me=true with empty sender should fall back to chat_id
        let msg = make_msg("hello", "", "+15551234567", true);
        let (reply_target, reply_is_group) = if msg.is_group {
            (msg.chat_id.clone(), true)
        } else if !msg.sender.is_empty() {
            (msg.sender.clone(), false)
        } else {
            (msg.chat_id.clone(), false)
        };
        assert_eq!(reply_target, "+15551234567");
        assert!(!reply_is_group);
    }
}
