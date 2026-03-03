//! Layer 1: Deterministic direct-send fast path.
//!
//! When the user's intent is an unambiguous "send X to channel", bypass the LLM
//! entirely and call the connector backend directly.

use super::Orchestrator;
use openalpaca_storage::repository::PreferenceRepository;
use regex::Regex;
use std::sync::LazyLock;

/// Extracted send parameters from a user message.
pub(super) struct DirectSendParams {
    pub channel: String,
    pub content: String,
}

// ── Compiled regexes ──────────────────────────────────────────────────────

static QUESTION_GUARD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)[\?？]|能不能|可以.*吗|吗$|\bcan\b|\bcould\b|\bwould\b|\bis it\b|\bhow to\b")
        .unwrap()
});

// Chinese: "请给telegram发送你好", "帮我给imessage发测试"
static CN_CHANNEL_FIRST: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:请|帮我)?(?:给|往|向|用|通过)(?:我的)?(?P<ch>telegram|imessage|discord)\s*(?:发送|转发)\s*(?P<content>.+)",
    )
    .unwrap()
});

// Chinese reverse: "发你好到telegram"
// Chinese reverse: "发你好到telegram", "发送你好到telegram"
static CN_CONTENT_FIRST: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:请|帮我)?(?:发送|转发|发)\s*(?P<content>.+?)\s*(?:到|给|去)\s*(?P<ch>telegram|imessage|discord)",
    )
    .unwrap()
});

// English: "send hello to telegram"
static EN_SEND_TO: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\bsend\s+(?P<content>.+?)\s+(?:to|via|through)\s+(?P<ch>telegram|imessage|discord)\b")
        .unwrap()
});

// English: "telegram send hello"
static EN_CHANNEL_FIRST: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?P<ch>telegram|imessage|discord)\s+(?:send|message)\s+(?P<content>.+)").unwrap()
});

// ── Literal content guard ─────────────────────────────────────────────────

/// Check if the extracted content is explicitly quoted in the original text,
/// signaling the user wants to send it verbatim.
///
/// Direct send only fires when the user marks the content with quotes.
/// All other cases fall through to the LLM path so it can compose or
/// clarify the message before calling `send_message`.
/// Strip surrounding quote marks from a string, if present.
/// The regex `.+` captures quotes as part of the content group;
/// this recovers the inner text so `is_explicitly_quoted` can match.
fn strip_surrounding_quotes(s: &str) -> Option<String> {
    let pairs: &[(&str, &str)] = &[
        ("\"", "\""),
        ("'", "'"),
        ("\u{201c}", "\u{201d}"), // ""
        ("\u{300c}", "\u{300d}"), // 「」
    ];
    for &(open, close) in pairs {
        if s.starts_with(open) && s.ends_with(close) && s.len() > open.len() + close.len() {
            let inner = &s[open.len()..s.len() - close.len()];
            let trimmed = inner.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

fn is_explicitly_quoted(original_text: &str, content: &str) -> bool {
    let quoted_patterns = [
        format!("\"{}\"", content),
        format!("'{}'", content),
        format!("\u{201c}{}\u{201d}", content), // ""
        format!("\u{300c}{}\u{300d}", content), // 「」
    ];
    quoted_patterns
        .iter()
        .any(|pat| original_text.contains(pat.as_str()))
}

// ── Extraction ────────────────────────────────────────────────────────────

/// Conservatively extract send parameters from user text.
///
/// Returns `Some` only when **all** of:
/// 1. Exactly one channel keyword present.
/// 2. Message content extractable, non-empty, and explicitly quoted.
/// 3. No question indicators.
pub(super) fn extract_send_params(text: &str) -> Option<DirectSendParams> {
    // Question guard
    if QUESTION_GUARD.is_match(text) {
        return None;
    }

    // Ambiguity guard: multiple channels mentioned
    let lower = text.to_lowercase();
    let channels_mentioned = [
        lower.contains("telegram"),
        lower.contains("imessage"),
        lower.contains("discord"),
    ];
    if channels_mentioned.iter().filter(|&&x| x).count() > 1 {
        return None;
    }

    // Try each regex in order
    let patterns: &[&LazyLock<Regex>] =
        &[&CN_CHANNEL_FIRST, &CN_CONTENT_FIRST, &EN_SEND_TO, &EN_CHANNEL_FIRST];

    for pat in patterns {
        if let Some(caps) = pat.captures(text) {
            let channel = caps.name("ch")?.as_str().to_lowercase();
            let raw_content = caps.name("content")?.as_str().trim();
            if raw_content.is_empty() {
                return None;
            }
            // The regex `.+` may capture surrounding quotes as part of the
            // content group. Strip them to get the inner message text, then
            // verify the original text actually contains the quoted form.
            let content = match strip_surrounding_quotes(raw_content) {
                Some(inner) => inner,
                None => raw_content.to_string(),
            };
            // Only direct-send when the user explicitly quoted the content.
            // Unquoted content falls through to the LLM path so it can
            // compose or refine the message before calling send_message.
            if !is_explicitly_quoted(text, &content) {
                return None;
            }
            return Some(DirectSendParams { channel, content });
        }
    }

    None
}

// ── Orchestrator integration ──────────────────────────────────────────────

impl Orchestrator {
    /// Attempt a deterministic direct send. Returns:
    /// - `Some(Ok(summary))` — message sent, use this as the response.
    /// - `Some(Err(reason))` — send attempted but failed.
    /// - `None` — fast path not applicable, fall through to LLM.
    pub(super) async fn try_direct_send(
        &self,
        query: &str,
        owner_id: Option<&str>,
    ) -> Option<Result<String, String>> {
        // 1. Quick reject: intent parser must think this is a send
        let suggestions = self.intent_parser.suggest_tools(query);
        if !suggestions.iter().any(|s| s == "send_message") {
            return None;
        }

        // 2. Extract params
        let params = extract_send_params(query)?;

        // 3. Verify connector is available for this channel
        let sender = self.connector_sender.read().ok()?.as_ref()?.clone();
        if !sender.sendable_channels().contains(&params.channel) {
            return None;
        }

        // 4. Verify a default recipient exists
        let (db, owner) = match (&self.db, owner_id) {
            (Some(db), Some(id)) => (db, id),
            _ => return None,
        };
        let pref_repo = PreferenceRepository::new(db);
        let has_default = match params.channel.as_str() {
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
            "discord" => pref_repo
                .get(owner, "discord.last_channel_id")
                .ok()
                .flatten()
                .and_then(|p| p.value.parse::<u64>().ok())
                .is_some(),
            _ => false,
        };
        if !has_default {
            return None;
        }

        // 5. Send directly — no LLM in the loop
        let result = sender
            .send_message(&params.channel, "default", &params.content)
            .await;
        Some(result)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Quoted content → direct send ──────────────────────────────────

    #[test]
    fn test_extract_quoted_chinese_direct() {
        let p = extract_send_params("请给telegram发送\"晚上8点开会\"").unwrap();
        assert_eq!(p.channel, "telegram");
        assert_eq!(p.content, "晚上8点开会");
    }

    #[test]
    fn test_extract_quoted_english_direct() {
        let p = extract_send_params("send \"hello world\" to telegram").unwrap();
        assert_eq!(p.channel, "telegram");
        assert_eq!(p.content, "hello world");
    }

    #[test]
    fn test_extract_quoted_short_allowed() {
        // Quotes override any length concern — user explicitly marked the content.
        let p = extract_send_params("请给telegram发送\"你好\"").unwrap();
        assert_eq!(p.channel, "telegram");
        assert_eq!(p.content, "你好");
    }

    // ── Unquoted content → fallback to LLM ────────────────────────────

    #[test]
    fn test_extract_unquoted_chinese_rejected() {
        // No quotes → LLM should compose/refine
        assert!(extract_send_params("请给telegram发送你好").is_none());
    }

    #[test]
    fn test_extract_unquoted_imessage_rejected() {
        assert!(extract_send_params("请给imessage发送测试").is_none());
    }

    #[test]
    fn test_extract_unquoted_reverse_rejected() {
        assert!(extract_send_params("发你好到telegram").is_none());
    }

    #[test]
    fn test_extract_unquoted_english_rejected() {
        assert!(extract_send_params("send hello to telegram").is_none());
    }

    #[test]
    fn test_extract_unquoted_long_content_rejected() {
        // Even long content without quotes falls through to LLM
        assert!(extract_send_params("请给telegram发送明天下午来开会").is_none());
    }

    #[test]
    fn test_extract_generic_cn_rejected() {
        assert!(extract_send_params("请给telegram发送测试短信").is_none());
    }

    #[test]
    fn test_extract_generic_en_rejected() {
        assert!(extract_send_params("send test message to telegram").is_none());
    }

    // ── Guards (unchanged) ────────────────────────────────────────────

    #[test]
    fn test_extract_question_rejected() {
        assert!(extract_send_params("can you send via telegram?").is_none());
    }

    #[test]
    fn test_extract_chinese_question_rejected() {
        assert!(extract_send_params("能不能给telegram发消息？").is_none());
    }

    #[test]
    fn test_extract_no_channel() {
        assert!(extract_send_params("send hello to bob").is_none());
    }

    #[test]
    fn test_extract_no_content() {
        assert!(extract_send_params("请给telegram发送").is_none());
    }

    #[test]
    fn test_extract_ambiguous_multi_channel() {
        assert!(extract_send_params("send to telegram and imessage").is_none());
    }

    // ── Discord extraction tests ─────────────────────────────────────

    #[test]
    fn test_extract_discord_quoted_chinese() {
        let p = extract_send_params("请给discord发送\"开会通知\"").unwrap();
        assert_eq!(p.channel, "discord");
        assert_eq!(p.content, "开会通知");
    }

    #[test]
    fn test_extract_discord_quoted_english() {
        let p = extract_send_params("send \"hello world\" to discord").unwrap();
        assert_eq!(p.channel, "discord");
        assert_eq!(p.content, "hello world");
    }

    #[test]
    fn test_extract_discord_channel_first_english() {
        let p = extract_send_params("discord send \"test message\"").unwrap();
        assert_eq!(p.channel, "discord");
        assert_eq!(p.content, "test message");
    }

    #[test]
    fn test_extract_discord_unquoted_rejected() {
        assert!(extract_send_params("send hello to discord").is_none());
    }

    #[test]
    fn test_extract_discord_case_insensitive() {
        let p = extract_send_params("send \"hi\" to Discord").unwrap();
        assert_eq!(p.channel, "discord");
        assert_eq!(p.content, "hi");
    }

    // ── Three-channel ambiguity tests ────────────────────────────────

    #[test]
    fn test_extract_ambiguous_telegram_discord() {
        assert!(extract_send_params("send \"hi\" to telegram and discord").is_none());
    }

    #[test]
    fn test_extract_ambiguous_imessage_discord() {
        assert!(extract_send_params("send \"hi\" to imessage and discord").is_none());
    }

    #[test]
    fn test_extract_ambiguous_all_three() {
        assert!(extract_send_params("send to telegram imessage discord").is_none());
    }
}
