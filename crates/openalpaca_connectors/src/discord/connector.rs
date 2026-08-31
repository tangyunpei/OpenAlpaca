//! Discord Connector Implementation
//!
//! Handles the integration between Discord Bot API (via twilight) and the
//! OpenAlpaca agent system. Uses CancellationToken for graceful shutdown
//! (same pattern as iMessage connector).

use crate::common::{
    LinkResult, format_confirmation_prompt, format_denial_message, handle_link_token,
    intercept_confirmation_reply, redact_token, resolve_principal,
};
use crate::{Connector, ConnectorError};
use arc_swap::ArcSwap;
use async_trait::async_trait;
use dashmap::DashMap;
use openalpaca_api::events::EventSource;
use openalpaca_core::{
    bus::EventBus,
    daemon_config::DaemonConfig,
    events::SystemEvent,
    gateway::{Gateway, GatewayRequest, ResolvedAttachment},
    security::confirmation::ConfirmationBroker,
    security::policy::Scope,
    types::Capability,
};
use openalpaca_storage::{Database, IdentityRepository, PreferenceRepository};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};
use twilight_gateway::{EventTypeFlags, Intents, Shard, ShardId, StreamExt as _};
use twilight_model::gateway::event::Event;

/// Discord's max message length
const DISCORD_MAX_LENGTH: usize = 2000;

/// Split a message into chunks that fit within Discord's message limit.
/// Prefers splitting at paragraph boundaries (\n\n), then sentence boundaries (. ),
/// then falls back to hard cut at a valid UTF-8 char boundary.
pub fn chunk_message(text: &str) -> Vec<String> {
    if text.len() <= DISCORD_MAX_LENGTH {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut remaining = text;

    while !remaining.is_empty() {
        if remaining.len() <= DISCORD_MAX_LENGTH {
            chunks.push(remaining.to_string());
            break;
        }

        // Find a safe byte boundary to slice up to (avoids panic on multi-byte UTF-8)
        let boundary = remaining.floor_char_boundary(DISCORD_MAX_LENGTH);
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

/// Send a message with exponential backoff retry (3 attempts: 1s, 2s, 4s).
pub async fn send_with_retry(
    http: &twilight_http::Client,
    channel_id: twilight_model::id::Id<twilight_model::id::marker::ChannelMarker>,
    text: &str,
) -> Result<(), String> {
    let chunks = chunk_message(text);

    for chunk in &chunks {
        let mut attempts = 0;
        let max_retries = 3;

        loop {
            match http.create_message(channel_id).content(chunk).await {
                Ok(_) => break,
                Err(e) => {
                    attempts += 1;
                    if attempts >= max_retries {
                        error!(
                            "Failed to send Discord message after {} retries: {}",
                            max_retries, e
                        );
                        return Err(format!("Send failed after {max_retries} retries: {e}"));
                    }
                    let delay = Duration::from_secs(1 << (attempts - 1)); // 1s, 2s, 4s
                    warn!(
                        "Discord send failed (attempt {}/{}), retrying in {:?}: {}",
                        attempts, max_retries, delay, e
                    );
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }

    Ok(())
}

/// Resolve the Discord channel to deliver a tool-confirmation prompt to,
/// for a lane key ending in `:discord`.
///
/// Tries the `discord.last_channel_id` preference (most recent channel for
/// the lane's user) first, then falls back to the conversation_map entry
/// recorded for the lane. Mirrors the Telegram connector's resolution order.
fn resolve_confirmation_channel(db: &Database, lane_key: &str) -> Option<u64> {
    let user_id = lane_key.strip_suffix(":discord").unwrap_or("");
    let pref_repo = PreferenceRepository::new(db);
    pref_repo
        .get(user_id, "discord.last_channel_id")
        .ok()
        .flatten()
        .and_then(|p| p.value.parse::<u64>().ok())
        .or_else(|| {
            IdentityRepository::new(db)
                .get_conversation_id_str_by_lane_key(lane_key, "discord")
                .ok()
                .flatten()
                .and_then(|id| id.parse::<u64>().ok())
        })
        // Guard against Id::new(0) panicking downstream
        .filter(|id| *id != 0)
}

/// Simple per-channel rate limiter. Allows at most 1 message per `min_interval` per channel.
struct ChannelRateLimiter {
    last_sent: Mutex<HashMap<u64, Instant>>,
    min_interval: Duration,
}

impl ChannelRateLimiter {
    fn new(min_interval: Duration) -> Self {
        Self {
            last_sent: Mutex::new(HashMap::new()),
            min_interval,
        }
    }

    /// Check if a message can be sent to this channel. Returns wait duration if rate limited.
    fn check(&self, channel_id: u64) -> Option<Duration> {
        let mut map = self.last_sent.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(last) = map.get(&channel_id) {
            let elapsed = last.elapsed();
            if elapsed < self.min_interval {
                return Some(self.min_interval - elapsed);
            }
        }
        map.insert(channel_id, Instant::now());
        None
    }
}

/// DiscordConnector manages the Discord bot lifecycle and message handling.
///
/// Uses twilight's `Shard::next_event()` event loop with `tokio::select!`
/// for cancellation (same pattern as iMessage connector's `run_loop()`).
pub struct DiscordConnector {
    token: String,
    db: Arc<Database>,
    bus: Arc<EventBus>,
    gateway: Arc<Gateway>,
    daemon_config: Arc<ArcSwap<DaemonConfig>>,
    cancel_token: CancellationToken,
    rate_limiter: Arc<ChannelRateLimiter>,
    confirmation_broker: Option<Arc<ConfirmationBroker>>,
    /// Maps channel_id -> queue of request_ids for pending tool confirmations.
    /// VecDeque allows FIFO processing when multiple tools need confirmation.
    /// (Same pattern — and same per-conversation-key caveat — as Telegram.)
    pending_confirmations: Arc<DashMap<u64, VecDeque<String>>>,
}

impl DiscordConnector {
    /// Create a new DiscordConnector.
    pub fn new(
        token: String,
        db: Arc<Database>,
        bus: Arc<EventBus>,
        gateway: Arc<Gateway>,
        daemon_config: Arc<ArcSwap<DaemonConfig>>,
        cancel_token: CancellationToken,
    ) -> Self {
        Self {
            token,
            db,
            bus,
            gateway,
            daemon_config,
            cancel_token,
            rate_limiter: Arc::new(ChannelRateLimiter::new(Duration::from_secs(1))),
            confirmation_broker: None,
            pending_confirmations: Arc::new(DashMap::new()),
        }
    }

    /// Attach a confirmation broker for interactive tool approval.
    pub fn with_confirmation_broker(mut self, broker: Arc<ConfirmationBroker>) -> Self {
        self.confirmation_broker = Some(broker);
        self
    }

    /// Main event loop.
    pub async fn run_loop(&self) -> Result<(), ConnectorError> {
        info!("Starting Discord connector...");

        // Initialize rustls provider (idempotent — safe to call multiple times)
        rustls::crypto::ring::default_provider()
            .install_default()
            .ok();

        let intents =
            Intents::GUILD_MESSAGES | Intents::DIRECT_MESSAGES | Intents::MESSAGE_CONTENT;
        let mut shard = Shard::new(ShardId::ONE, self.token.clone(), intents);
        let http = Arc::new(twilight_http::Client::new(self.token.clone()));
        let mut bot_user_id: Option<twilight_model::id::Id<twilight_model::id::marker::UserMarker>> = None;

        // Spawn confirmation listener (if broker available)
        if self.confirmation_broker.is_some() {
            self.spawn_confirmation_listener(http.clone());
        }

        loop {
            tokio::select! {
                _ = self.cancel_token.cancelled() => {
                    info!("Discord connector shutting down");
                    return Ok(());
                }
                event = shard.next_event(EventTypeFlags::all()) => {
                    match event {
                        Some(Ok(Event::MessageCreate(msg))) => {
                            if let Err(e) = self.handle_message(&msg.0, &http, bot_user_id).await {
                                error!("Failed to handle Discord message: {e}");
                            }
                        }
                        Some(Ok(Event::Ready(ready))) => {
                            info!("Discord bot ready: {} (id: {})", ready.user.name, ready.user.id);
                            bot_user_id = Some(ready.user.id);
                        }
                        Some(Err(e)) => {
                            use twilight_gateway::error::ReceiveMessageErrorType;
                            match e.kind() {
                                ReceiveMessageErrorType::Reconnect => {
                                    error!("Fatal Discord gateway error: {e}");
                                    return Err(ConnectorError::ConnectionError(format!(
                                        "Fatal gateway error: {e}"
                                    )));
                                }
                                _ => {
                                    warn!("Discord gateway error: {e}");
                                }
                            }
                        }
                        None => {
                            info!("Discord gateway stream ended");
                            return Ok(());
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    /// Spawn a background task that listens for `ToolConfirmationRequested`
    /// events targeting Discord lanes and sends confirmation prompts.
    fn spawn_confirmation_listener(&self, http: Arc<twilight_http::Client>) {
        let mut rx = self.bus.subscribe();
        let db = self.db.clone();
        let pending = self.pending_confirmations.clone();
        let cancel = self.cancel_token.clone();
        tokio::spawn(async move {
            loop {
                let event = tokio::select! {
                    _ = cancel.cancelled() => {
                        info!("Discord confirmation listener shutting down");
                        break;
                    }
                    event = rx.recv() => event,
                };
                match event {
                    Ok(SystemEvent::ToolConfirmationRequested {
                        request_id,
                        tool_name,
                        tool_arguments,
                        lane_key: Some(ref lane_key),
                        ..
                    }) if lane_key.ends_with(":discord") => {
                        // Resolve channel_id from lane_key via DB lookup
                        let Some(channel_id) = resolve_confirmation_channel(&db, lane_key) else {
                            warn!(
                                "Could not resolve Discord channel_id for lane_key={}, skipping confirmation",
                                lane_key
                            );
                            continue;
                        };

                        // Store pending confirmation mapping (queue per channel)
                        pending
                            .entry(channel_id)
                            .or_default()
                            .push_back(request_id.clone());
                        let queue_len = pending.get(&channel_id).map(|q| q.len()).unwrap_or(1);

                        let prompt =
                            format_confirmation_prompt(&tool_name, &tool_arguments, queue_len);

                        if let Err(e) = send_with_retry(
                            &http,
                            twilight_model::id::Id::new(channel_id),
                            &prompt,
                        )
                        .await
                        {
                            error!(
                                "Failed to send confirmation prompt to channel {}: {}",
                                channel_id, e
                            );
                            // Remove the one we just added (last in queue)
                            if let Some(mut q) = pending.get_mut(&channel_id) {
                                q.pop_back();
                            }
                        } else {
                            debug!(
                                "Sent confirmation prompt for request {} to channel {}",
                                request_id, channel_id
                            );
                        }
                    }
                    Ok(_) => {} // ignore other events
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!("Confirmation listener lagged by {} events", n);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        info!("EventBus closed, confirmation listener exiting");
                        break;
                    }
                }
            }
        });
    }

    /// Handle a single incoming Discord message.
    async fn handle_message(
        &self,
        msg: &twilight_model::channel::Message,
        http: &Arc<twilight_http::Client>,
        bot_user_id: Option<twilight_model::id::Id<twilight_model::id::marker::UserMarker>>,
    ) -> Result<(), String> {
        // Step 1: Ignore bot messages
        if msg.author.bot {
            return Ok(());
        }

        let channel_id = msg.channel_id;
        let user_id = msg.author.id;
        let guild_id = msg.guild_id;
        let display_name = msg.author.name.clone();

        // Step 1.5: Intercept confirmation responses (/yes, /y, /no, /n)
        // BEFORE normal processing (including the guild mention gate, so a
        // bare "/yes" works in guild channels the prompt was sent to).
        if let Some(broker) = self.confirmation_broker.as_ref()
            && let Some(reply) = intercept_confirmation_reply(
                &msg.content,
                &channel_id.get(),
                broker,
                &self.pending_confirmations,
            )
        {
            if let Err(e) = send_with_retry(http, channel_id, &reply).await {
                error!(
                    "Failed to send confirmation acknowledgment to channel {}: {}",
                    channel_id, e
                );
            }
            return Ok(());
        }

        // Step 2: Check if mentioned or DM
        // Guild channels require @bot mention; DMs always process
        let is_dm = guild_id.is_none();
        let content = if is_dm {
            msg.content.clone()
        } else {
            // Check if this bot is mentioned (by its own user ID)
            let bot_mentioned = bot_user_id
                .map(|bid| msg.mentions.iter().any(|m| m.id == bid))
                .unwrap_or(false);
            if !bot_mentioned {
                return Ok(());
            }
            // Strip this bot's mention prefix from content
            let mut text = msg.content.clone();
            if let Some(bid) = bot_user_id {
                for mention in &msg.mentions {
                    if mention.id == bid {
                        let mention_str = format!("<@{}>", mention.id);
                        text = text.replace(&mention_str, "");
                        let mention_str_nick = format!("<@!{}>", mention.id);
                        text = text.replace(&mention_str_nick, "");
                    }
                }
            }
            text.trim().to_string()
        };

        // Check guild_id restriction
        let config_repo = openalpaca_storage::ConfigRepository::new(&self.db);
        if let Ok(Some(allowed_guild)) = config_repo.get("discord.guild_id") {
            if !allowed_guild.is_empty() {
                if let Some(gid) = guild_id {
                    if gid.to_string() != allowed_guild {
                        debug!("Discord message from non-allowed guild {gid}, skipping");
                        return Ok(());
                    }
                } else {
                    // DM — block when guild restriction is active
                    debug!("Discord DM blocked by guild_id restriction");
                    return Ok(());
                }
            }
        }

        // Step 3: Rate limit check
        if let Some(wait) = self.rate_limiter.check(channel_id.get()) {
            warn!(
                "Rate limited Discord channel {}, need to wait {:?}",
                channel_id, wait
            );
            return Ok(());
        }

        // Step 4: Resolve Principal
        let identity_repo = IdentityRepository::new(&self.db);
        let (principal, external_identity_id) = resolve_principal(
            &identity_repo,
            "discord",
            &user_id.to_string(),
            Some(&display_name),
        )?;

        // Step 5: Handle /link and /unlink commands
        if content.starts_with("/link ") {
            let token = content.strip_prefix("/link ").unwrap().trim();
            info!(
                "Processing /link command for Discord user {} with token {}",
                user_id,
                redact_token(token)
            );

            match handle_link_token(&identity_repo, token, external_identity_id) {
                Ok(LinkResult::Success(global_user_id)) => {
                    info!(
                        "Successfully linked discord:{} -> global_user:{}",
                        user_id, global_user_id
                    );
                    if let Err(e) = identity_repo.migrate_lane_on_link(
                        &user_id.to_string(),
                        &global_user_id,
                        "discord",
                        &channel_id.to_string(),
                    ) {
                        warn!("Lane migration failed: {e}");
                    }
                    let _ = send_with_retry(
                        http,
                        channel_id,
                        &format!(
                            "Account linked successfully! You are now connected as `{}`.",
                            global_user_id
                        ),
                    )
                    .await;
                }
                Ok(LinkResult::InvalidToken) => {
                    warn!("Invalid/expired link token: {}", redact_token(token));
                    let _ = send_with_retry(
                        http,
                        channel_id,
                        "Invalid or expired token. Please generate a new one.",
                    )
                    .await;
                }
                Err(e) => {
                    error!("Link error: {e}");
                    let _ = send_with_retry(http, channel_id, "An error occurred during linking.")
                        .await;
                }
            }
            return Ok(());
        } else if content == "/unlink" || content == "/unbind" {
            match identity_repo.unlink_external_identity(external_identity_id) {
                Ok(()) => {
                    info!("Unlinked discord:{}", user_id);
                    let _ = send_with_retry(http, channel_id, "Account unlinked successfully.")
                        .await;
                }
                Err(e) => {
                    error!("Unlink error: {e}");
                    let _ = send_with_retry(http, channel_id, "An error occurred during unlinking.")
                        .await;
                }
            }
            return Ok(());
        }

        // Step 6: Pre-check TrustGate
        let capability = Capability {
            name: "chat.respond".to_string(),
        };
        let scope = Scope::Conversation {
            id: channel_id.to_string(),
        };

        if let Err(e) =
            openalpaca_core::security::policy::TrustGate::check(&principal, &capability, &scope)
        {
            warn!("TrustGate denied Discord request: {}", e);
            let _ = send_with_retry(http, channel_id, &format_denial_message(&e)).await;
            return Ok(());
        }

        // Extract global_id before principal is consumed by gateway
        let global_id = match &principal {
            openalpaca_core::security::policy::Principal::User { global_id } => {
                Some(global_id.clone())
            }
            _ => None,
        };

        // Step 7: Typing indicator
        let _ = http.create_typing_trigger(channel_id).await;

        // Step 8: Handle attachments
        let owner_id = match &global_id {
            Some(gid) => gid.clone(),
            None => user_id.to_string(),
        };
        let mut attachments: Vec<ResolvedAttachment> = Vec::new();
        let upload_cfg = self.daemon_config.load();
        let max_file_size = upload_cfg.upload.max_file_size_bytes;
        let max_img_dim = upload_cfg.upload.governance.max_image_dimension;

        for att in &msg.attachments {
            let url = &att.url;
            match reqwest::get(url).await {
                Ok(resp) => match resp.bytes().await {
                    Ok(data) => {
                        let filename = att.filename.clone();
                        let mime = att
                            .content_type
                            .clone()
                            .unwrap_or_else(|| "application/octet-stream".to_string());
                        match crate::common::store_attachment(
                            &self.db,
                            &owner_id,
                            &filename,
                            &mime,
                            &data,
                            max_file_size,
                            max_img_dim,
                        ) {
                            Ok(resolved) => attachments.push(resolved),
                            Err(e) => warn!("Failed to store Discord attachment: {e}"),
                        }
                    }
                    Err(e) => warn!("Failed to read Discord attachment bytes: {e}"),
                },
                Err(e) => warn!("Failed to download Discord attachment: {e}"),
            }
        }

        // Skip if nothing useful
        if content.is_empty() && attachments.is_empty() {
            return Ok(());
        }

        info!(
            channel_id = %channel_id,
            user_id = %user_id,
            guild_id = ?guild_id,
            "Accepted Discord message for processing"
        );

        // Step 9: Route through Gateway
        let response = self
            .gateway
            .handle_event(GatewayRequest {
                source: EventSource::Discord {
                    channel_id: channel_id.to_string(),
                    user_id: user_id.to_string(),
                    guild_id: guild_id.map(|g| g.to_string()),
                },
                content,
                attachments,
                principal,
                scope: Scope::Conversation {
                    id: channel_id.to_string(),
                },
                workspace_path: None,
                stream_id: None,
                lane_override: None,
            })
            .await;

        // Step 10: Map external channel_id to internal lane_key
        let lane_key = response.lane_key.to_string();
        if let Err(e) = identity_repo.update_conversation_map_lane_key(
            "discord",
            &channel_id.to_string(),
            &lane_key,
        ) {
            warn!("Failed to update conversation_map lane_key: {e}");
        }

        // Step 11: Persist discord.last_channel_id for cross-channel delivery
        if let Some(ref gid) = global_id {
            let pref_repo = PreferenceRepository::new(&self.db);
            if let Err(e) =
                pref_repo.set(gid, "discord.last_channel_id", &channel_id.to_string(), None)
            {
                warn!("Failed to persist discord.last_channel_id: {e}");
            }
        }

        // Step 12: Send response with chunking + retry
        if let Err(e) = send_with_retry(http, channel_id, &response.content).await {
            error!(
                "Failed to send response to Discord channel {}: {}",
                channel_id, e
            );
        }

        Ok(())
    }
}

#[async_trait]
impl Connector for DiscordConnector {
    fn name(&self) -> &str {
        "discord"
    }

    async fn run(&self) -> Result<(), ConnectorError> {
        self.run_loop().await
    }

    async fn shutdown(&self) -> Result<(), ConnectorError> {
        info!("Discord connector shutdown requested");
        self.cancel_token.cancel();
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
        let text = "a".repeat(DISCORD_MAX_LENGTH);
        let chunks = chunk_message(&text);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].len(), DISCORD_MAX_LENGTH);
    }

    #[test]
    fn test_chunk_message_over_limit() {
        let text = "a".repeat(DISCORD_MAX_LENGTH + 100);
        let chunks = chunk_message(&text);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].len(), DISCORD_MAX_LENGTH);
        assert_eq!(chunks[1].len(), 100);
    }

    #[test]
    fn test_chunk_message_paragraph_split() {
        let para1 = "a".repeat(1500);
        let para2 = "b".repeat(1000);
        let text = format!("{}\n\n{}", para1, para2);
        let chunks = chunk_message(&text);
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].ends_with('\n'));
    }

    #[test]
    fn test_chunk_message_empty() {
        let chunks = chunk_message("");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "");
    }

    #[test]
    fn test_chunk_message_multibyte_utf8() {
        // 3-byte chars: each char is 3 bytes
        let text = "\u{4e16}".repeat(700); // 700 * 3 = 2100 bytes > 2000
        let chunks = chunk_message(&text);
        assert!(chunks.len() >= 2);
        // Verify no panic from slicing mid-character
        for chunk in &chunks {
            assert!(chunk.is_char_boundary(chunk.len()));
        }
    }

    #[test]
    fn test_rate_limiter_allows_first() {
        let limiter = ChannelRateLimiter::new(Duration::from_secs(1));
        assert!(limiter.check(12345).is_none());
    }

    #[test]
    fn test_rate_limiter_blocks_second() {
        let limiter = ChannelRateLimiter::new(Duration::from_secs(1));
        limiter.check(12345);
        assert!(limiter.check(12345).is_some());
    }

    #[test]
    fn test_rate_limiter_different_channels() {
        let limiter = ChannelRateLimiter::new(Duration::from_secs(1));
        limiter.check(12345);
        assert!(limiter.check(67890).is_none());
    }

    fn test_db() -> Database {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(&dir.path().join("test.db")).unwrap();
        // Keep the tempdir alive for the lifetime of the process — tests are short.
        std::mem::forget(dir);
        db
    }

    #[test]
    fn test_resolve_confirmation_channel_via_preference() {
        let db = test_db();
        let pref_repo = PreferenceRepository::new(&db);
        pref_repo
            .set("global1", "discord.last_channel_id", "111222333", None)
            .unwrap();

        assert_eq!(
            resolve_confirmation_channel(&db, "global1:discord"),
            Some(111222333)
        );
    }

    #[test]
    fn test_resolve_confirmation_channel_via_conversation_map() {
        let db = test_db();
        let identity_repo = IdentityRepository::new(&db);
        identity_repo
            .update_conversation_map_lane_key("discord", "444555666", "global2:discord")
            .unwrap();

        // No preference set — falls back to conversation_map
        assert_eq!(
            resolve_confirmation_channel(&db, "global2:discord"),
            Some(444555666)
        );
    }

    #[test]
    fn test_resolve_confirmation_channel_unknown_lane() {
        let db = test_db();
        assert_eq!(resolve_confirmation_channel(&db, "nobody:discord"), None);
    }

    #[test]
    fn test_confirmation_intercept_broker_roundtrip() {
        use openalpaca_core::security::confirmation::ConfirmationRequest;

        let broker = ConfirmationBroker::new();
        let mut rx = broker.request(&ConfirmationRequest {
            request_id: "req-discord-1".to_string(),
            agent_id: "agent-1".to_string(),
            tool_name: "shell_exec".to_string(),
            tool_arguments: serde_json::json!({"cmd": "ls"}),
            stream_id: None,
            lane_key: Some("global1:discord".to_string()),
            timestamp: chrono::Utc::now(),
        });

        // Simulate the listener queuing the request for a channel
        let pending: DashMap<u64, VecDeque<String>> = DashMap::new();
        pending
            .entry(111222333u64)
            .or_default()
            .push_back("req-discord-1".to_string());

        // Reply from a different channel falls through to normal processing
        assert!(
            intercept_confirmation_reply("/yes", &999u64, &broker, &pending).is_none()
        );

        // Reply from the prompted channel approves via the broker
        let reply =
            intercept_confirmation_reply("/yes", &111222333u64, &broker, &pending).unwrap();
        assert!(reply.contains("Approved"));
        assert!(rx.try_recv().unwrap().approved);
    }
}
