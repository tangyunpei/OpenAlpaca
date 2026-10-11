//! Common utilities shared across all connectors.

use openalpaca_core::gateway::ResolvedAttachment;
use openalpaca_core::security::policy::Principal;
use openalpaca_storage::store::StoreScope;
use openalpaca_storage::{Database, IdentityRepository, NewUpload, UploadStore};

/// Resolve a Principal from an external identity.
///
/// Returns:
/// - `Principal::User` if the external identity is linked to a global_user
/// - `Principal::External` if not linked (untrusted)
pub fn resolve_principal(
    identity_repo: &IdentityRepository<'_>,
    provider: &str,
    provider_user_id: &str,
    display_name: Option<&str>,
) -> Result<(Principal, i64), String> {
    let external_identity = identity_repo
        .get_or_create_external_identity(provider, provider_user_id, display_name)
        .map_err(|e| format!("Failed to get/create identity: {}", e))?;

    let principal = match &external_identity.global_user_id {
        Some(global_id) => Principal::User {
            global_id: global_id.clone(),
        },
        None => Principal::External {
            provider: provider.to_string(),
            id: provider_user_id.to_string(),
        },
    };

    Ok((principal, external_identity.id))
}

/// Format a denial message for TrustGate rejection.
pub fn format_denial_message(error: &str) -> String {
    format!("⚠️ {}\n\nUse /link <token> to link your account.", error)
}

/// Redact a token for safe logging (show only first 4 chars).
pub fn redact_token(token: &str) -> String {
    if token.len() <= 4 {
        "****".to_string()
    } else {
        let prefix: String = token.chars().take(4).collect();
        format!("{}****", prefix)
    }
}

/// Handle the /link command logic.
///
/// Uses an atomic consume-and-link transaction so that the token is not
/// consumed if linking the identity fails.
pub fn handle_link_token(
    identity_repo: &IdentityRepository<'_>,
    token: &str,
    external_identity_id: i64,
) -> Result<LinkResult, String> {
    match identity_repo.consume_and_link(token, external_identity_id) {
        Ok(Some(global_user_id)) => Ok(LinkResult::Success(global_user_id)),
        Ok(None) => Ok(LinkResult::InvalidToken),
        Err(e) => Err(e.to_string()),
    }
}

/// Result of a link operation.
pub enum LinkResult {
    /// Successfully linked to the given global_user_id
    Success(String),
    /// Token was invalid, expired, or already used
    InvalidToken,
}

/// How much of the pretty-printed, escaped arguments a chat prompt shows.
const PROMPT_ARGUMENTS_BYTES: usize = 500;

/// Format the confirmation prompt sent to a chat platform when a
/// confirm-listed tool awaits interactive approval — the one prompt Telegram,
/// Discord and iMessage all send: the tool name, the arguments cut to
/// `PROMPT_ARGUMENTS_BYTES` (500 bytes), the `/yes` / `/no` instructions
/// and, when others wait before it, its place in the chat's queue.
///
/// `preview` is the daemon's own text for the card (design 09 §4.4; storage
/// design R34: the folder a command starts in, where a write lands), printed
/// under `From OpenAlpaca:` between the tool's name and its arguments, as the
/// app and `openalpaca chat` print it. It is OpenAlpaca's prose; whatever it
/// quotes from the call is already escaped (`utils::text`), so its characters
/// are printed as written — but every line of it is indented two spaces
/// under its label, as the CLI indents it: `file_edit`'s card quotes the
/// model's `old_string` and `new_string` with their line breaks kept as
/// layout, and a line of theirs at the margin would read as the prompt's own
/// `Arguments:` block or `/yes` line. A blank line of it is printed empty,
/// not as its indent alone. Absent or empty, the prompt is the ordinary one.
///
/// A cut says how much it left out — `... (31 more characters not shown)` —
/// because what it leaves out still runs: a model can pad a shell command
/// past the cut, or put a key that sorts first (`serde_json` prints keys
/// sorted) in front of the one that matters, and a bare `...` reads as the
/// end of a value that goes on.
///
/// `queue_len` is the queue's length with this request just pushed to its
/// back, so it is also this prompt's place. A reply answers the **oldest**
/// waiting request, not the prompt last on screen, so a queued prompt says
/// both: `(2 of 2 pending; each /yes or /no answers the oldest first)`.
///
/// The name and the arguments are what the person approves, and both come
/// from outside OpenAlpaca (an MCP `<server>__<tool>` or plugin
/// `<plugin>::<tool>` name, model-written arguments), so every hidden
/// character in them — a bidi override, a zero-width character, a variation
/// selector, a C1 control, a blank that is not the plain space — is shown as
/// `<U+XXXX>` by [`escape_hidden`], escaped before the cut so the cut never
/// splits an escape, and one notice line under the arguments counts them,
/// those the cut left out included. The name is one line, so it goes
/// through [`escape_hidden_line`], which also shows a newline and a tab: a
/// name cannot break the prompt's lines, nor wrap its tail onto a line of
/// its own with a run of wide blanks. A run of plain spaces can still wrap
/// it there — residue, recorded at [`escape_hidden_line`]'s predicate.
/// Display only: `tool_arguments` is borrowed, and a reply is matched by
/// request id, not by anything shown here.
///
/// [`escape_hidden`]: openalpaca_core::utils::text::escape_hidden
/// [`escape_hidden_line`]: openalpaca_core::utils::text::escape_hidden_line
pub fn format_confirmation_prompt(
    tool_name: &str,
    tool_arguments: &serde_json::Value,
    preview: Option<&str>,
    queue_len: usize,
) -> String {
    use openalpaca_core::utils::text::{
        cut_notice, escape_hidden_line, escape_hidden_within, hidden_notice,
    };

    let name = escape_hidden_line(tool_name);
    let pretty =
        serde_json::to_string_pretty(tool_arguments).unwrap_or_else(|_| tool_arguments.to_string());
    let args = escape_hidden_within(&pretty, PROMPT_ARGUMENTS_BYTES);
    let rest = cut_notice(args.left_out).unwrap_or_default();
    let notice = hidden_notice(name.hidden + args.hidden)
        .map(|line| format!("\n{line}"))
        .unwrap_or_default();

    let queue_info = if queue_len > 1 {
        format!(" ({queue_len} of {queue_len} pending; each /yes or /no answers the oldest first)")
    } else {
        String::new()
    };

    // The daemon's own text for the card (R34: the folder a command starts
    // in, where a write lands), above the arguments, every line of it
    // indented under its label: `file_edit`'s card quotes the model's two
    // strings with their line breaks, and a line at the margin could pass
    // for the prompt's own `Arguments:` or `/yes` line (d3-approvals-1).
    let from_openalpaca = preview
        .filter(|text| !text.is_empty())
        .map(|text| {
            // A blank line stays empty — never its indent alone, which a
            // chunk's edge could begin or end with (Y.2).
            let indented = text
                .lines()
                .map(|line| {
                    if line.is_empty() {
                        String::new()
                    } else {
                        format!("  {line}")
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
            format!("From OpenAlpaca:\n{indented}\n")
        })
        .unwrap_or_default();

    format!(
        "A tool requires your confirmation before executing{queue_info}:\n\n\
         Tool: {name}\n\
         {from_openalpaca}\
         Arguments:\n{args}{rest}{notice}\n\n\
         Reply /yes or /no to approve or deny.",
        name = name.text,
        args = args.text,
    )
}

/// Intercept a potential confirmation reply (`/yes`, `/y`, `/no`, `/n`).
///
/// Mirrors the Telegram connector's intercept: pops the oldest pending
/// request for the conversation (FIFO), delivers the decision to the
/// [`ConfirmationBroker`](openalpaca_core::security::confirmation::ConfirmationBroker),
/// and returns the acknowledgment text to send back to the chat.
///
/// The oldest request need not be the prompt last on screen, so the
/// acknowledgement names the tool it answered (`Approved: shell_exec. …`),
/// escaped on one line like the prompt's name
/// (`openalpaca_core::utils::text::escape_hidden_line`). It carries no
/// notice line: it repeats a name the prompt already showed and counted.
/// When the broker no longer holds the request (it timed out, was cancelled,
/// or was answered elsewhere — the GUI or `openalpaca tasks confirmations`),
/// the acknowledgement names no tool. Which request a reply answers is
/// unchanged: the oldest, by request id.
///
/// Returns `None` when the text is not a confirmation command or the
/// conversation has no pending confirmation — the caller should fall
/// through to normal message handling.
pub fn intercept_confirmation_reply<K>(
    text: &str,
    key: &K,
    broker: &openalpaca_core::security::confirmation::ConfirmationBroker,
    pending: &dashmap::DashMap<K, std::collections::VecDeque<String>>,
) -> Option<String>
where
    K: Eq + std::hash::Hash + std::fmt::Debug,
{
    use openalpaca_core::security::confirmation::ConfirmationResponse;

    let text_lower = text.trim().to_lowercase();
    if !matches!(text_lower.as_str(), "/yes" | "/y" | "/no" | "/n") {
        return None;
    }

    // No pending confirmation — fall through to normal handling
    let request_id = pending.get_mut(key).and_then(|mut q| q.pop_front())?;

    let approved = matches!(text_lower.as_str(), "/yes" | "/y");
    let remaining = pending.get(key).map(|q| q.len()).unwrap_or(0);
    let tool = broker
        .pending_requests()
        .into_iter()
        .find(|request| request.request_id == request_id)
        .map(|request| {
            format!(
                ": {}",
                openalpaca_core::utils::text::escape_hidden_line(&request.tool_name).text
            )
        })
        .unwrap_or_default();
    let answered = if approved {
        format!("Approved{tool}. Tool execution will proceed.")
    } else {
        format!("Denied{tool}. Tool execution has been cancelled.")
    };
    let reply = if remaining > 0 {
        format!("{answered}\n({remaining} more pending — reply /yes or /no)")
    } else {
        answered
    };

    match broker.respond(
        &request_id,
        ConfirmationResponse {
            approved,
            approval_scope: None,
        },
    ) {
        Ok(()) => {
            tracing::info!(
                "Confirmation {} for request {} in conversation {:?}",
                if approved { "approved" } else { "denied" },
                request_id,
                key
            );
        }
        Err(e) => {
            tracing::warn!("Failed to deliver confirmation response: {}", e);
        }
    }

    Some(reply)
}

/// Store an inbound attachment from a connector.
///
/// Validates the bytes (defence in depth for connector-sourced files), then
/// hands them to [`UploadStore`] — the one upload writer, shared with
/// `POST /v1/files/upload`. Hashing, the owner-scoped sha256 dedup, placement
/// and the row all live there; this function owns only the validation policy
/// and the `ResolvedAttachment` shape `GatewayRequest` wants.
pub fn store_attachment(
    db: &Database,
    owner_id: &str,
    filename: &str,
    mime_type: &str,
    data: &[u8],
    max_file_size: u64,
    max_image_dimension: u32,
) -> Result<ResolvedAttachment, String> {
    use openalpaca_core::security::sanitizer::InputSanitizer;

    if let Err(violation) = InputSanitizer::validate_upload_with_image_limit(
        filename,
        data,
        mime_type,
        max_file_size,
        max_image_dimension,
    ) {
        return Err(format!("Upload validation failed: {violation}"));
    }

    let stored = UploadStore::new(db)
        .put(NewUpload {
            owner_id,
            filename,
            mime_type,
            data,
            // A chat attachment arrives with no project signal of any kind, so
            // it takes the home store (D2). Nothing here may invent one from
            // the daemon's working directory (R22).
            scope: &StoreScope::Home,
            created: chrono::Utc::now(),
        })
        .map_err(|e| format!("Failed to store attachment: {e}"))?;

    Ok(ResolvedAttachment {
        file_id: stored.asset.id,
        filename: stored.asset.filename,
        mime_type: stored.asset.mime_type,
        size_bytes: stored.asset.size_bytes,
        extracted_text: stored.asset.extracted_text,
        storage_path: stored.asset.storage_path,
    })
}

/// Split a message into chunks of at most `max_bytes` bytes, a chat
/// platform's message-length limit. Prefers splitting at paragraph boundaries
/// (\n\n), then sentence boundaries (. ), then any newline, then falls back to
/// a hard cut at a valid UTF-8 char boundary. Every separator stays with the
/// chunk it ends, so the chunks concatenate back to `text`; empty text is one
/// empty chunk.
///
/// `max_bytes` must fit the longest UTF-8 character (4 bytes), or a hard cut
/// could back off to 0 and never advance. Callers pass their platform's
/// constant.
#[cfg(any(feature = "telegram", feature = "discord"))]
pub(crate) fn chunk_message(text: &str, max_bytes: usize) -> Vec<String> {
    assert!(max_bytes >= 4, "a chunk must fit any UTF-8 character");
    if text.len() <= max_bytes {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut remaining = text;

    while !remaining.is_empty() {
        if remaining.len() <= max_bytes {
            chunks.push(remaining.to_string());
            break;
        }

        // Find a safe byte boundary to slice up to (avoids panic on multi-byte UTF-8)
        let boundary = remaining.floor_char_boundary(max_bytes);
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

/// The line every message after the first of an approval prompt sent in
/// several begins with (the lead's second residual pass, Y.2): the prompt's
/// own words, so the first line of a later message — which a platform may
/// show with its leading whitespace trimmed, as Discord and Telegram both
/// trim a message's — is never text from the preview.
#[cfg(any(feature = "telegram", feature = "discord"))]
pub(crate) const APPROVAL_CONTINUED: &str = "(approval prompt, continued)";

/// [`chunk_message`] for an approval prompt (the lead's ruling R-T32): a
/// split falls only between lines, never at a sentence end inside one, so a
/// preview line [`format_confirmation_prompt`] indented stays indented in
/// whichever message carries it. A line longer than a message's room is cut
/// at a character boundary and each continuation starts with the same two
/// spaces. Every message after the first begins with `continued` — the
/// platform's spelling of [`APPROVAL_CONTINUED`] — on a line of its own
/// (second residual pass, Y.2): Discord and Telegram trim a message's
/// leading whitespace, so without it a later message whose first line is an
/// indented preview line would show that line at the margin. So the only
/// lines at the margin, trimmed or not, are the prompt's own (`Tool:`,
/// `Arguments:`, the arguments' braces, `Reply /yes …`, `continued`). No
/// message begins or ends with a blank or whitespace-only line, none has
/// one right under `continued`, and none is empty; a prompt that fits one
/// message is sent as it is, such edge lines aside.
///
/// `max_bytes` must fit `continued`, its line break, the continuation's two
/// spaces and the longest UTF-8 character (4 bytes), or a cut could not
/// advance.
#[cfg(any(feature = "telegram", feature = "discord"))]
pub(crate) fn chunk_approval_prompt(text: &str, max_bytes: usize, continued: &str) -> Vec<String> {
    const CONTINUATION: &str = "  ";
    let blank = |line: &str| line.trim().is_empty();
    // Under `continued` and its line break: the room every piece must fit,
    // since any piece may be the first of a later message.
    let room = max_bytes
        .checked_sub(continued.len() + 1)
        .filter(|room| *room >= CONTINUATION.len() + 4)
        .expect("a chunk must fit the continued line and an indented UTF-8 character");
    let lines: Vec<&str> = text.split('\n').collect();
    let (Some(first), Some(last)) = (
        lines.iter().position(|line| !blank(line)),
        lines.iter().rposition(|line| !blank(line)),
    ) else {
        return Vec::new();
    };
    let lines = &lines[first..=last];
    if text.len() <= max_bytes {
        return vec![lines.join("\n")];
    }
    // Every line as pieces of at most `room`: the first as written, the rest
    // of an over-long line indented.
    let mut pieces: Vec<String> = Vec::new();
    for line in lines {
        let mut rest = *line;
        let mut prefix = "";
        loop {
            let fits = room - prefix.len();
            if rest.len() <= fits {
                pieces.push(format!("{prefix}{rest}"));
                break;
            }
            let (head, tail) = rest.split_at(rest.floor_char_boundary(fits));
            pieces.push(format!("{prefix}{head}"));
            rest = tail;
            prefix = CONTINUATION;
        }
    }
    // The pieces packed into messages, a line break between two in one; a
    // message's room is `max_bytes` for the first and `room` under
    // `continued` for every later one.
    fn close(chunks: &mut Vec<String>, message: &mut Vec<String>, continued: &str) {
        while message.last().is_some_and(|line| line.trim().is_empty()) {
            message.pop();
        }
        if message.is_empty() {
            return;
        }
        let body = message.join("\n");
        message.clear();
        chunks.push(if chunks.is_empty() {
            body
        } else {
            format!("{continued}\n{body}")
        });
    }
    let mut chunks: Vec<String> = Vec::new();
    let mut message: Vec<String> = Vec::new();
    let mut used = 0;
    for piece in pieces {
        if message.is_empty() && blank(&piece) {
            continue;
        }
        let limit = if chunks.is_empty() { max_bytes } else { room };
        let grown = if message.is_empty() {
            piece.len()
        } else {
            used + 1 + piece.len()
        };
        if grown <= limit {
            message.push(piece);
            used = grown;
            continue;
        }
        close(&mut chunks, &mut message, continued);
        if blank(&piece) {
            used = 0;
            continue;
        }
        used = piece.len();
        message.push(piece);
    }
    close(&mut chunks, &mut message, continued);
    chunks
}

/// A `file_edit` prompt over `window` bytes whose preview forges the
/// prompt's own lines (the lead's ruling R-T32): an `edit_card`-shaped
/// preview whose `new_string` repeats `...done. Arguments:` with a JSON block
/// under it and a `Reply /yes …` line, among code lines with no sentence end
/// and blank lines, so a chunker that splits at a sentence end inside the
/// preview starts a message at the margin with the forgery, and one that
/// splits at a blank line can begin or end a message with it. Returns the
/// tool's arguments and the preview.
#[cfg(all(test, any(feature = "telegram", feature = "discord")))]
pub(crate) fn forging_edit_prompt(window: usize) -> (serde_json::Value, String) {
    let forged = "        log(\"...done. Arguments:\n{\n  \"path\": \"docs/notes.md\"\n}\n\n\
                  Reply /yes or /no to approve or deny.\n\n";
    let mut new_string = String::new();
    let mut line = 0;
    while new_string.len() < window * 2 {
        line += 1;
        new_string.push_str(&format!("        let value_{line} = compute({line});\n"));
        if line % 4 == 0 {
            new_string.push_str(forged);
        }
    }
    let preview = format!(
        "Edits docs/notes.md in the project folder /w/app: 1 replacement, the first at line \
         3. Its current content is kept in this conversation's log.\nReplace:\n        \
         let value = 1;\nWith:\n{new_string}"
    );
    let args = serde_json::json!({
        "path": "docs/notes.md",
        "old_string": "        let value = 1;",
        "new_string": new_string,
    });
    (args, preview)
}

/// What the lead's rulings R-T32 and Y.2 hold of an approval prompt sent in
/// several messages, `shown` as the platform shows each one: every message
/// after the first begins with [`APPROVAL_CONTINUED`], no message begins or
/// ends with a blank or whitespace-only line (nor has one right under that
/// line), and — each message's leading whitespace trimmed, as Discord and
/// Telegram trim it — exactly one line across them all starts `Arguments:`
/// and one `Reply /yes`: the prompt's own.
#[cfg(all(test, any(feature = "telegram", feature = "discord")))]
pub(crate) fn assert_approval_messages(shown: &[String]) {
    let blank = |line: &str| line.trim().is_empty();
    assert!(shown.len() > 1, "the prompt is long enough to be split");
    for (index, message) in shown.iter().enumerate() {
        let lines: Vec<&str> = message.split('\n').collect();
        let (first, last) = (lines[0], lines[lines.len() - 1]);
        assert!(
            !blank(first) && !blank(last),
            "message {index} begins or ends with a blank line: {message:?}"
        );
        if index > 0 {
            assert_eq!(first, APPROVAL_CONTINUED, "message {index}: {message:?}");
            assert!(
                lines.len() > 1 && !blank(lines[1]),
                "message {index} has nothing, or a blank line, under its first: {message:?}"
            );
        }
    }
    let starting = |head: &str| {
        shown
            .iter()
            .flat_map(|message| message.trim_start().lines())
            .filter(|line| line.starts_with(head))
            .count()
    };
    assert_eq!(starting("Arguments:"), 1, "{shown:#?}");
    assert_eq!(starting("Reply /yes"), 1, "{shown:#?}");
}

/// Per-conversation limiter on **inbound** messages: at most one message per
/// `min_interval` per key (a Telegram chat id, a Discord channel id). Each
/// connector owns its own instance, so equal ids on two platforms never
/// share a slot.
#[cfg(any(feature = "telegram", feature = "discord"))]
pub(crate) struct KeyedRateLimiter<K> {
    last_accepted: std::sync::Mutex<std::collections::HashMap<K, std::time::Instant>>,
    min_interval: std::time::Duration,
}

#[cfg(any(feature = "telegram", feature = "discord"))]
impl<K: Eq + std::hash::Hash> KeyedRateLimiter<K> {
    pub(crate) fn new(min_interval: std::time::Duration) -> Self {
        Self {
            last_accepted: std::sync::Mutex::new(std::collections::HashMap::new()),
            min_interval,
        }
    }

    /// `None` lets the message through and restarts this key's interval;
    /// `Some(wait)` refuses it and leaves the interval where it was.
    pub(crate) fn check(&self, key: K) -> Option<std::time::Duration> {
        let mut map = self.last_accepted.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(last) = map.get(&key) {
            let elapsed = last.elapsed();
            if elapsed < self.min_interval {
                return Some(self.min_interval - elapsed);
            }
        }
        map.insert(key, std::time::Instant::now());
        None
    }
}

#[cfg(test)]
mod tests;
