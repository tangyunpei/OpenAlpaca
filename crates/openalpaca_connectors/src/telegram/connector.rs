//! Telegram Connector Implementation
//!
//! Handles the integration between Telegram Bot API and the OpenAlpaca agent system.

use crate::common::{
    LinkResult, format_denial_message, handle_link_token, redact_token, resolve_principal,
};
use crate::{Connector, ConnectorError};
use arc_swap::ArcSwap;
use async_trait::async_trait;
use openalpaca_api::events::EventSource;
use openalpaca_core::{
    bus::EventBus,
    daemon_config::DaemonConfig,
    gateway::{Gateway, GatewayRequest, ResolvedAttachment},
    security::policy::Scope,
    types::Capability,
};
use openalpaca_storage::{Database, IdentityRepository, PreferenceRepository};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use teloxide::net::Download;
use teloxide::prelude::*;
use teloxide::types::ChatAction;
use tracing::{error, info, warn};

/// Telegram's max message length
const TELEGRAM_MAX_LENGTH: usize = 4096;

/// Split a message into chunks that fit within Telegram's message limit.
/// Prefers splitting at paragraph boundaries (\n\n), then sentence boundaries (. ),
/// then falls back to hard cut at a valid UTF-8 char boundary.
fn chunk_message(text: &str) -> Vec<String> {
    if text.len() <= TELEGRAM_MAX_LENGTH {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut remaining = text;

    while !remaining.is_empty() {
        if remaining.len() <= TELEGRAM_MAX_LENGTH {
            chunks.push(remaining.to_string());
            break;
        }

        // Find a safe byte boundary to slice up to (avoids panic on multi-byte UTF-8)
        let boundary = remaining.floor_char_boundary(TELEGRAM_MAX_LENGTH);
        let slice = &remaining[..boundary];

        // Try paragraph boundary
        let split_at = slice
            .rfind("\n\n")
            .map(|i| i + 2) // include the newlines
            // Try sentence boundary
            .or_else(|| slice.rfind(". ").map(|i| i + 2))
            // Try any newline
            .or_else(|| slice.rfind('\n').map(|i| i + 1))
            // Hard cut at safe char boundary
            .unwrap_or(boundary);

        chunks.push(remaining[..split_at].to_string());
        remaining = &remaining[split_at..];
    }

    chunks
}

/// Escape special characters for Telegram MarkdownV2 format.
/// Characters that must be escaped: _ * [ ] ( ) ~ ` > # + - = | { } . !
#[allow(dead_code)]
fn escape_markdown_v2(text: &str) -> String {
    let special_chars = [
        '_', '*', '[', ']', '(', ')', '~', '`', '>', '#', '+', '-', '=', '|', '{', '}', '.', '!',
    ];
    let mut result = String::with_capacity(text.len() * 2);
    for ch in text.chars() {
        if special_chars.contains(&ch) {
            result.push('\\');
        }
        result.push(ch);
    }
    result
}

/// Send a message with exponential backoff retry (3 attempts: 1s, 2s, 4s).
async fn send_with_retry(
    bot: &Bot,
    chat_id: ChatId,
    text: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let chunks = chunk_message(text);

    for chunk in &chunks {
        let mut attempts = 0;
        let max_retries = 3;

        loop {
            match bot.send_message(chat_id, chunk).await {
                Ok(_) => break,
                Err(e) => {
                    attempts += 1;
                    if attempts >= max_retries {
                        error!(
                            "Failed to send message after {} retries: {}",
                            max_retries, e
                        );
                        return Err(Box::new(e));
                    }
                    let delay = Duration::from_secs(1 << (attempts - 1)); // 1s, 2s, 4s
                    warn!(
                        "Send failed (attempt {}/{}), retrying in {:?}: {}",
                        attempts, max_retries, delay, e
                    );
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }

    Ok(())
}

/// Simple per-chat rate limiter. Allows at most 1 message per `min_interval` per chat.
struct ChatRateLimiter {
    last_sent: Mutex<HashMap<i64, Instant>>,
    min_interval: Duration,
}

impl ChatRateLimiter {
    fn new(min_interval: Duration) -> Self {
        Self {
            last_sent: Mutex::new(HashMap::new()),
            min_interval,
        }
    }

    /// Check if a message can be sent to this chat. Returns wait duration if rate limited.
    fn check(&self, chat_id: i64) -> Option<Duration> {
        let mut map = self.last_sent.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(last) = map.get(&chat_id) {
            let elapsed = last.elapsed();
            if elapsed < self.min_interval {
                return Some(self.min_interval - elapsed);
            }
        }
        map.insert(chat_id, Instant::now());
        None
    }
}

/// Download a file from Telegram using the Bot API.
async fn download_telegram_file(
    bot: &Bot,
    file_id: &str,
) -> Result<Vec<u8>, Box<dyn std::error::Error + Send + Sync>> {
    use teloxide::types::FileId;
    let file = bot.get_file(FileId(file_id.to_string())).await?;
    let mut buf = Vec::new();
    bot.download_file(&file.path, &mut buf).await?;
    Ok(buf)
}

/// TelegramConnector manages the Telegram bot lifecycle and message handling.
pub struct TelegramConnector {
    bot: Bot,
    db: Arc<Database>,
    bus: Arc<EventBus>,
    gateway: Arc<Gateway>,
    daemon_config: Arc<ArcSwap<DaemonConfig>>,
    rate_limiter: Arc<ChatRateLimiter>,
}

impl TelegramConnector {
    /// Create a new TelegramConnector with the given bot token.
    pub fn new(
        token: String,
        db: Arc<Database>,
        bus: Arc<EventBus>,
        gateway: Arc<Gateway>,
        daemon_config: Arc<ArcSwap<DaemonConfig>>,
    ) -> Self {
        let bot = Bot::new(token);
        Self {
            bot,
            db,
            bus,
            gateway,
            daemon_config,
            rate_limiter: Arc::new(ChatRateLimiter::new(Duration::from_secs(1))),
        }
    }

    /// Start the connector (blocking, runs the teloxide dispatcher).
    /// Returns a ShutdownToken to stop the dispatcher.
    pub async fn run_with_signal(self) -> teloxide::dispatching::ShutdownToken {
        info!("Starting Telegram Connector...");

        let handler = Update::filter_message().endpoint(Self::handle_message);

        // Clone state for the handler
        let db = self.db.clone();
        let bus = self.bus.clone();
        let gateway = self.gateway.clone();
        let daemon_config = self.daemon_config.clone();
        let rate_limiter = self.rate_limiter.clone();

        let mut dispatcher = Dispatcher::builder(self.bot, handler)
            .dependencies(dptree::deps![db, bus, gateway, daemon_config, rate_limiter])
            .build();

        let token = dispatcher.shutdown_token();

        // Spawn dispatcher loop so we can return token
        tokio::spawn(async move {
            dispatcher.dispatch().await;
            info!("Telegram connector dispatcher finished");
        });

        token
    }

    /// Handle an incoming Telegram message.
    async fn handle_message(
        bot: Bot,
        msg: Message,
        db: Arc<Database>,
        bus: Arc<EventBus>,
        gateway: Arc<Gateway>,
        daemon_config: Arc<ArcSwap<DaemonConfig>>,
        rate_limiter: Arc<ChatRateLimiter>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let text = msg.text().unwrap_or("").to_string();

        let chat_id = msg.chat.id;
        let from = match msg.from.as_ref() {
            Some(user) => user,
            None => {
                // Channel posts, anonymous admin messages, etc. have no sender.
                // We cannot resolve identity without a user ID, so skip.
                return Ok(());
            }
        };
        let user_id = from.id.0.to_string();
        let display_name = Some(from.first_name.clone());

        info!(
            "Received message from Telegram user {} in chat {}: {}",
            user_id,
            chat_id,
            text.chars().take(50).collect::<String>()
        );

        // Check rate limiter
        if let Some(wait) = rate_limiter.check(chat_id.0) {
            warn!("Rate limited chat {}, need to wait {:?}", chat_id, wait);
            bot.send_message(
                chat_id,
                "Please wait a moment before sending another message.",
            )
            .await?;
            return Ok(());
        }

        // Step 1: Resolve Principal
        let identity_repo = IdentityRepository::new(&db);
        let (principal, external_identity_id) = resolve_principal(
            &identity_repo,
            "telegram",
            &user_id,
            display_name.as_deref(),
        )?;

        let is_linked = matches!(
            principal,
            openalpaca_core::security::policy::Principal::User { .. }
        );
        if is_linked {
            info!("User {} is linked (Trusted)", user_id);
        } else {
            info!("User {} is NOT linked (Untrusted)", user_id);
        }

        // Step 2: Handle /link and /unlink commands (connector-specific)
        if text.starts_with("/link ") {
            let token = text.strip_prefix("/link ").unwrap().trim();
            return Self::handle_link_command(
                &bot,
                chat_id,
                &user_id,
                token,
                external_identity_id,
                &identity_repo,
            )
            .await;
        } else if text == "/unlink" || text == "/unbind" {
            return Self::handle_unlink_command(
                &bot,
                chat_id,
                &user_id,
                external_identity_id,
                &identity_repo,
            )
            .await;
        }

        // Step 3: Pre-check TrustGate (for early denial message to user)
        let capability = Capability {
            name: "chat.respond".to_string(),
        };
        let scope = Scope::Conversation {
            id: chat_id.0.to_string(),
        };

        if let Err(e) =
            openalpaca_core::security::policy::TrustGate::check(&principal, &capability, &scope)
        {
            warn!("TrustGate denied request: {}", e);
            bot.send_message(chat_id, format_denial_message(&e)).await?;
            return Ok(());
        }

        // Extract global_id before principal is consumed by gateway
        let global_id_for_pref = match &principal {
            openalpaca_core::security::policy::Principal::User { global_id } => {
                Some(global_id.clone())
            }
            _ => None,
        };

        // Send typing indicator
        if let Err(e) = bot.send_chat_action(chat_id, ChatAction::Typing).await {
            warn!("Failed to send typing indicator: {}", e);
        }

        // Step 3.5: Handle photo and document attachments
        let owner_id = match &global_id_for_pref {
            Some(gid) => gid.clone(),
            None => user_id.clone(),
        };
        let mut attachments: Vec<ResolvedAttachment> = Vec::new();
        let upload_cfg = daemon_config.load();
        let max_file_size = upload_cfg.upload.max_file_size_bytes;
        let max_img_dim = upload_cfg.upload.governance.max_image_dimension;

        // Handle photos — msg.photo() returns Option<&[PhotoSize]>
        if let Some(photos) = msg.photo()
            && let Some(largest) = photos.last()
        {
            // sorted by size, largest last
            match download_telegram_file(&bot, &largest.file.id.0).await {
                Ok(data) => {
                    let uid = &largest.file.unique_id.0;
                    let suffix = &uid[..8.min(uid.len())];
                    let fname = format!("photo_{suffix}.jpg");
                    match crate::common::store_attachment(
                        &db,
                        &owner_id,
                        &fname,
                        "image/jpeg",
                        &data,
                        max_file_size,
                        max_img_dim,
                    ) {
                        Ok(att) => attachments.push(att),
                        Err(e) => warn!("Failed to store telegram photo: {e}"),
                    }
                }
                Err(e) => warn!("Failed to download telegram photo: {e}"),
            }
        }

        // Handle documents — msg.document() returns Option<&Document>
        if let Some(doc) = msg.document() {
            let file_name = doc.file_name.as_deref().unwrap_or("document");
            let mime = doc
                .mime_type
                .as_ref()
                .map(|m| m.to_string())
                .unwrap_or_else(|| "application/octet-stream".into());
            match download_telegram_file(&bot, &doc.file.id.0).await {
                Ok(data) => {
                    match crate::common::store_attachment(
                        &db,
                        &owner_id,
                        file_name,
                        &mime,
                        &data,
                        max_file_size,
                        max_img_dim,
                    ) {
                        Ok(att) => attachments.push(att),
                        Err(e) => warn!("Failed to store telegram doc: {e}"),
                    }
                }
                Err(e) => warn!("Failed to download telegram doc: {e}"),
            }
        }

        // Skip if nothing useful
        if text.is_empty() && attachments.is_empty() {
            return Ok(());
        }

        // Step 4: Route through Gateway (replaces manual pipeline)
        let response = gateway
            .handle_event(GatewayRequest {
                source: EventSource::Telegram {
                    chat_id: chat_id.0.to_string(),
                    user_id: user_id.clone(),
                },
                content: text.clone(),
                attachments,
                principal,
                scope: Scope::Conversation {
                    id: chat_id.0.to_string(),
                },
                workspace_path: None, // Telegram has no workspace context; uses Global scope only
            })
            .await;

        // Step 4.5: Map external Telegram chat_id to internal lane_key
        let lane_key = response.lane_key.to_string();
        if let Err(e) = identity_repo.update_conversation_map_lane_key(
            "telegram",
            &chat_id.0.to_string(),
            &lane_key,
        ) {
            warn!("Failed to update conversation_map lane_key: {e}");
        }

        // Step 5.2: Persist telegram.last_chat_id for cross-channel delivery
        if let Some(ref gid) = global_id_for_pref {
            let pref_repo = PreferenceRepository::new(&db);
            if let Err(e) =
                pref_repo.set(gid, "telegram.last_chat_id", &chat_id.0.to_string(), None)
            {
                warn!("Failed to persist telegram.last_chat_id: {e}");
            }
        }

        // Step 5: Send response back to Telegram with retry and chunking
        if let Err(e) = send_with_retry(&bot, chat_id, &response.content).await {
            error!(
                "Failed to send response to Telegram chat {}: {}",
                chat_id, e
            );
        }

        // Note: EventBus events (UserRequest + AgentResponse) are now emitted
        // by Gateway and the MessageHandler, not by the connector.
        let _ = bus; // Keep bus in deps for /link events if needed

        Ok(())
    }

    /// Handle the /link command.
    async fn handle_link_command(
        bot: &Bot,
        chat_id: ChatId,
        user_id: &str,
        token: &str,
        external_identity_id: i64,
        identity_repo: &IdentityRepository<'_>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!(
            "Processing /link command for user {} with token {}",
            user_id,
            redact_token(token)
        );

        match handle_link_token(identity_repo, token, external_identity_id) {
            Ok(LinkResult::Success(global_user_id)) => {
                info!(
                    "Successfully linked telegram:{} -> global_user:{}",
                    user_id, global_user_id
                );

                // Migrate old lane to global lane
                if let Err(e) = identity_repo.migrate_lane_on_link(
                    user_id,
                    &global_user_id,
                    "telegram",
                    &chat_id.0.to_string(),
                ) {
                    warn!("Lane migration failed: {e}");
                }

                bot.send_message(
                    chat_id,
                    format!(
                        "✅ Account linked successfully!\nYou are now connected as `{}`.",
                        global_user_id
                    ),
                )
                .await?;
            }
            Ok(LinkResult::InvalidToken) => {
                warn!("Invalid/expired link token: {}", redact_token(token));
                bot.send_message(
                    chat_id,
                    "❌ Invalid or expired token.\nPlease generate a new token from the GUI or CLI.",
                )
                .await?;
            }
            Err(e) => {
                error!("Error consuming link token: {}", e);
                bot.send_message(chat_id, format!("❌ Error: {}", e))
                    .await?;
            }
        }

        Ok(())
    }
    /// Handle the /unlink command.
    async fn handle_unlink_command(
        bot: &Bot,
        chat_id: ChatId,
        user_id: &str,
        external_identity_id: i64,
        identity_repo: &IdentityRepository<'_>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        info!("Processing /unlink command for user {}", user_id);

        match identity_repo.unlink_external_identity(external_identity_id) {
            Ok(_) => {
                info!("Successfully unlinked telegram:{}", user_id);
                bot.send_message(
                    chat_id,
                    "✅ Account unlinked successfully.\nYou are now acting as an anonymous/external user.",
                )
                .await?;
            }
            Err(e) => {
                error!("Error unlinking identity: {}", e);
                bot.send_message(chat_id, format!("❌ Error: {}", e))
                    .await?;
            }
        }
        Ok(())
    }
}

#[async_trait]
impl Connector for TelegramConnector {
    fn name(&self) -> &'static str {
        "telegram"
    }

    async fn run(&self) -> Result<(), ConnectorError> {
        // Note: The actual run is done via run_with_signal() which consumes self.
        // This trait method cannot consume self, so we just return Ok for now.
        // The startup logic uses run_with_signal() directly.
        Ok(())
    }

    async fn shutdown(&self) -> Result<(), ConnectorError> {
        info!("Telegram connector shutdown requested");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_message_short() {
        let text = "Hello, world!";
        let chunks = chunk_message(text);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "Hello, world!");
    }

    #[test]
    fn test_chunk_message_exact_limit() {
        let text = "a".repeat(TELEGRAM_MAX_LENGTH);
        let chunks = chunk_message(&text);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), TELEGRAM_MAX_LENGTH);
    }

    #[test]
    fn test_chunk_message_paragraph_boundary() {
        let paragraph1 = "a".repeat(2000);
        let paragraph2 = "b".repeat(2000);
        let paragraph3 = "c".repeat(2000);
        let text = format!("{}\n\n{}\n\n{}", paragraph1, paragraph2, paragraph3);
        let chunks = chunk_message(&text);
        assert!(chunks.len() >= 2);
        // First chunk should split at paragraph boundary
        assert!(chunks[0].ends_with("\n\n"));
    }

    #[test]
    fn test_chunk_message_sentence_boundary() {
        // Create a long string with sentence boundaries but no paragraph boundaries
        let sentence = "a".repeat(2000);
        let text = format!("{}. {}. {}", sentence, sentence, sentence);
        let chunks = chunk_message(&text);
        assert!(chunks.len() >= 2);
        // First chunk should split at sentence boundary
        assert!(chunks[0].ends_with(". "));
    }

    #[test]
    fn test_chunk_message_hard_cut() {
        // No boundaries at all
        let text = "a".repeat(5000);
        let chunks = chunk_message(&text);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].len(), TELEGRAM_MAX_LENGTH);
        assert_eq!(chunks[1].len(), 5000 - TELEGRAM_MAX_LENGTH);
    }

    #[test]
    fn test_escape_markdown_v2_basic() {
        assert_eq!(escape_markdown_v2("hello"), "hello");
        assert_eq!(escape_markdown_v2("hello_world"), "hello\\_world");
        assert_eq!(escape_markdown_v2("a*b*c"), "a\\*b\\*c");
        assert_eq!(escape_markdown_v2("test."), "test\\.");
        assert_eq!(escape_markdown_v2("1+1=2"), "1\\+1\\=2");
    }

    #[test]
    fn test_escape_markdown_v2_all_special() {
        let input = "_*[]()~`>#+-=|{}.!";
        let expected = "\\_\\*\\[\\]\\(\\)\\~\\`\\>\\#\\+\\-\\=\\|\\{\\}\\.\\!";
        assert_eq!(escape_markdown_v2(input), expected);
    }

    #[test]
    fn test_escape_markdown_v2_empty() {
        assert_eq!(escape_markdown_v2(""), "");
    }

    #[test]
    fn test_rate_limiter_allows_first_message() {
        let limiter = ChatRateLimiter::new(Duration::from_secs(1));
        assert!(limiter.check(12345).is_none());
    }

    #[test]
    fn test_rate_limiter_blocks_rapid_messages() {
        let limiter = ChatRateLimiter::new(Duration::from_secs(1));
        assert!(limiter.check(12345).is_none());
        // Second check immediately should be rate limited
        let wait = limiter.check(12345);
        assert!(wait.is_some());
        assert!(wait.unwrap() <= Duration::from_secs(1));
    }

    #[test]
    fn test_rate_limiter_independent_chats() {
        let limiter = ChatRateLimiter::new(Duration::from_secs(1));
        assert!(limiter.check(111).is_none());
        assert!(limiter.check(222).is_none()); // Different chat, should pass
        assert!(limiter.check(111).is_some()); // Same chat, should be limited
    }

    #[test]
    fn test_chunk_message_utf8_boundary() {
        // Build a string of multi-byte chars that would cause a panic
        // if we slice at a raw byte offset.
        // Each CJK char is 3 bytes in UTF-8.
        let cjk_char = "\u{4e16}"; // '世' = 3 bytes
        // Fill slightly over the limit with 3-byte chars
        let count = (TELEGRAM_MAX_LENGTH / 3) + 100;
        let text: String = cjk_char.repeat(count);
        assert!(text.len() > TELEGRAM_MAX_LENGTH);
        // Must not panic
        let chunks = chunk_message(&text);
        assert!(chunks.len() >= 2);
        // All chunks must be valid UTF-8 (they are Strings, so this is guaranteed)
        for chunk in &chunks {
            assert!(!chunk.is_empty());
            // Verify each chunk is within the limit
            assert!(chunk.len() <= TELEGRAM_MAX_LENGTH);
        }
    }

    #[test]
    fn test_chunk_message_emoji_boundary() {
        // Emoji are 4 bytes in UTF-8
        let emoji = "\u{1F600}"; // grinning face = 4 bytes
        let count = (TELEGRAM_MAX_LENGTH / 4) + 100;
        let text: String = emoji.repeat(count);
        assert!(text.len() > TELEGRAM_MAX_LENGTH);
        let chunks = chunk_message(&text);
        assert!(chunks.len() >= 2);
        for chunk in &chunks {
            assert!(!chunk.is_empty());
            assert!(chunk.len() <= TELEGRAM_MAX_LENGTH);
        }
    }

    #[test]
    fn test_floor_char_boundary_std() {
        let s = "Hello\u{4e16}\u{754c}"; // "Hello世界" = 5 + 3 + 3 = 11 bytes
        // Boundary in the middle of '世' (bytes 5..8)
        assert_eq!(s.floor_char_boundary(6), 5);
        assert_eq!(s.floor_char_boundary(7), 5);
        assert_eq!(s.floor_char_boundary(8), 8); // exactly on boundary
        assert_eq!(s.floor_char_boundary(100), 11); // beyond end
        assert_eq!(s.floor_char_boundary(0), 0);
    }
}
