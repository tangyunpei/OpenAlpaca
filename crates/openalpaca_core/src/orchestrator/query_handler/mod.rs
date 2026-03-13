use super::Orchestrator;
use openalpaca_llm::{ChatMessage, ContentPart, ImageSource, Role, ToolChoice, ToolDefinition};
use openalpaca_storage::repository::PreferenceRepository;

/// Optional overrides for handle_simple_query loop configuration.
/// Used by the deep_query triage tier to expand budget and provide LLM-suggested tools.
pub(super) struct LoopOverrides {
    pub max_rounds: usize,
    pub max_tools_per_round: usize,
    /// When non-empty, replaces the keyword-heuristic tool list.
    pub override_tools: Vec<ToolDefinition>,
}

mod simple_query_handler;

#[cfg(test)]
mod tests;

fn sanitize_parts_for_dispatch(parts: Vec<ContentPart>) -> Vec<ContentPart> {
    parts
        .into_iter()
        .filter_map(|part| match part {
            ContentPart::Image {
                source: ImageSource::FileAsset {
                    file_id,
                    media_type,
                },
                ..
            } => {
                tracing::warn!(
                    file_id = %file_id,
                    media_type = %media_type,
                    "Unresolved FileAsset image part reached query handler; replacing with placeholder"
                );
                Some(ContentPart::Text {
                    text: "[image attached — unresolved file asset reference]".to_string(),
                })
            }
            ContentPart::Text { text } if text.trim().is_empty() => {
                tracing::debug!("Dropping empty text content part before model dispatch");
                None
            }
            other => Some(other),
        })
        .collect()
}

/// Hints for whether the send tool should be kept alive across turns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct ActiveSendHints {
    send: bool,
}

/// Analyze recent conversation to determine whether the send tool should be kept alive.
///
/// Uses a tiered priority system per assistant message (last 2 within 6-message window):
/// - **Tier 1**: Literal tool name (`send(`, `send tool`, `"send"`, `` `send` ``, `call send`, `use send`) — highest confidence.
/// - **Tier 2**: Channel + recipient-solicitation keywords — defaults to send active.
pub(super) fn detect_active_send_hints(
    recent_messages: &[ChatMessage],
    active_channels: &[String],
) -> ActiveSendHints {
    const FALLBACK_CHANNEL_KW: &[&str] = &["telegram", "imessage", "slack", "discord", "whatsapp", "wechat", "signal"];
    const SEND_KW: &[&str] = &["send(", "send tool", "\"send\"", "`send`", "call send", "use send"];
    const RECIPIENT_KW: &[&str] = &[
        "recipient", "chat_id", "收件人", "发给谁", "发送给",
        "send to whom", "send it to",
    ];

    let mut hints = ActiveSendHints::default();

    // Collect assistant messages within lookback window
    let lowered: Vec<String> = recent_messages
        .iter()
        .rev()
        .take(6)
        .filter(|m| m.role == Role::Assistant)
        .take(2)
        .map(|m| m.content.to_lowercase())
        .collect();

    for lower in &lowered {
        // Tier 1: tool name match (highest priority)
        if SEND_KW.iter().any(|k| lower.contains(k)) {
            hints.send = true;
            continue;
        }

        // Tier 2: channel + recipient-solicitation
        let has_channel = active_channels.iter().any(|k| lower.contains(&k.to_lowercase()))
            || FALLBACK_CHANNEL_KW.iter().any(|k| lower.contains(k));
        if !has_channel {
            continue;
        }
        let has_recipient = RECIPIENT_KW.iter().any(|k| lower.contains(k));
        if has_recipient {
            hints.send = true;
        }
    }

    hints
}

/// Resolve `initial_tool_choice` for the send tool.
fn resolve_send_tool_choice(has_send: bool) -> Option<ToolChoice> {
    if has_send {
        Some(ToolChoice::Tool("send".to_string()))
    } else {
        None
    }
}

/// Apply send-tool keep-alive injection to the tool list.
///
/// Snapshots intent-level flag before injection, then appends the send tool
/// if indicated by `detect_active_send_hints()` and not already present.
///
/// Returns whether the intent originally suggested send.
fn apply_send_keepalive(
    tool_names: &mut Vec<String>,
    recent_messages: &[ChatMessage],
    active_channels: &[String],
) -> bool {
    let intent_has_send = tool_names.contains(&"send".to_string());

    let keepalive = detect_active_send_hints(recent_messages, active_channels);
    if !intent_has_send && keepalive.send {
        tool_names.push("send".to_string());
    }

    intent_has_send
}

impl Orchestrator {
    /// Build a deterministic `<send_context>` block with resolved recipient info.
    /// This removes ambiguity: the LLM sees facts, not hints.
    pub(in crate::orchestrator) fn build_send_context(&self, owner_id: Option<&str>) -> String {
        let (db, owner) = match (&self.db, owner_id) {
            (Some(db), Some(id)) => (db, id),
            _ => return String::new(),
        };
        let sendable = self
            .connector_sender
            .read()
            .ok()
            .and_then(|g| g.as_ref().map(|p| p.sendable_channels()))
            .unwrap_or_default();
        if sendable.is_empty() {
            return String::new();
        }

        let pref_repo = PreferenceRepository::new(db);
        let mut block = String::from("<send_context>\n");
        for ch in &sendable {
            let has_default = match ch.as_str() {
                "telegram" => pref_repo
                    .get(owner, "telegram.last_chat_id")
                    .ok()
                    .flatten()
                    .and_then(|p| p.value.parse::<i64>().ok())
                    .is_some(),
                "imessage" => {
                    pref_repo
                        .get(owner, "imessage.last_reply_target")
                        .ok()
                        .flatten()
                        .is_some()
                        || pref_repo
                            .get(owner, "imessage.last_chat_id")
                            .ok()
                            .flatten()
                            .is_some()
                }
                _ => false,
            };

            let recipient_fmt = match ch.as_str() {
                "telegram" => "\"default\" | numeric chat_id",
                "imessage" => "\"default\" | phone | email",
                _ => "\"default\"",
            };

            let detail = if has_default {
                match ch.as_str() {
                    "telegram" => "most recent Telegram chat",
                    "imessage" => "most recent iMessage conversation via AppleScript",
                    _ => "most recent conversation",
                }
            } else {
                "no recent conversation"
            };

            block.push_str(&format!(
                "- {}: default={} ({})\n  recipient: {}\n",
                ch, has_default, detail, recipient_fmt
            ));
        }
        block.push_str("</send_context>");
        block
    }

    /// Build a lightweight `<available_skills>` block for system prompt injection.
    ///
    /// Lists all registered skills with their slash commands and descriptions.
    /// Budget: ~500 chars. Returns empty string if no skills are loaded.
    pub(super) fn build_skills_catalog_block(&self) -> String {
        let summaries = self.skill_catalog.catalog_summary();
        if summaries.is_empty() {
            return String::new();
        }

        let mut block = String::from(
            "<available_skills>\nThe user can invoke these specialized skills with slash commands:\n",
        );
        let mut budget = 500usize;
        for (name, description, command) in &summaries {
            let line = if let Some(cmd) = command {
                format!("- {} (/{}): {}\n", name, cmd, description)
            } else {
                format!("- {}: {}\n", name, description)
            };
            if line.len() > budget {
                break;
            }
            budget -= line.len();
            block.push_str(&line);
        }
        block.push_str("</available_skills>");
        block
    }
}
