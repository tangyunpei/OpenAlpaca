//! iMessage routing logic: trigger prefix stripping, handle normalization,
//! and message processing decisions.

use super::reader::IncomingMessage;

/// Built-in trigger prefixes. A message must start with one of these to be processed.
pub(super) const TRIGGER_PREFIXES: &[&str] = &["/ask", "@openalpaca"];

/// Check if the message starts with any trigger prefix.
/// Returns the content after the prefix (trimmed), or None if no prefix matched.
pub(super) fn strip_trigger_prefix(text: &str) -> Option<String> {
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
pub(super) fn normalize_handle(handle: &str) -> String {
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

/// Derive the send addressing for a tool-confirmation prompt from a
/// conversation_map chat identifier.
///
/// - Group chats (identifier starts with "chat") use chat-id addressing.
/// - DMs use the participant handle, extracted from GUID-style identifiers
///   like `iMessage;-;+15551234567` (plain handles pass through unchanged).
///
/// Mirrors `handle_message`'s reply_target logic: groups by chat id,
/// DMs by handle.
pub(super) fn confirmation_reply_target(chat_id: &str) -> (String, bool) {
    if chat_id.starts_with("chat") {
        (chat_id.to_string(), true)
    } else {
        let handle = chat_id.rsplit(';').next().unwrap_or(chat_id);
        (handle.to_string(), false)
    }
}

/// Runtime configuration for iMessage message routing.
pub(super) struct IMessageConfig {
    /// Whether to include self-sent messages in polling.
    pub(super) allow_from_me: bool,
    /// Require trigger prefix for direct (1-to-1) messages.
    pub(super) direct_require_prefix: bool,
    /// Require trigger prefix for group chat messages.
    pub(super) group_require_prefix: bool,
    /// Normalized owner handles for identifying self-sent DMs.
    pub(super) owner_handles: Vec<String>,
    /// The bot's iMessage account identifier (e.g. "e:bot@icloud.com" or "p:+1234567890").
    /// Messages from this account are skipped to prevent the bot from processing its own replies.
    pub(super) bot_handle: Option<String>,
}

/// Result of evaluating whether a message should be processed.
pub(super) enum ProcessDecision {
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
pub(super) fn should_process(msg: &IncomingMessage, config: &IMessageConfig) -> ProcessDecision {
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
