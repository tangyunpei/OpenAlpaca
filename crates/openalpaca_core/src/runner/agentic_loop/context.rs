use openalpaca_llm::{ChatMessage, ContentPart, Role};

/// Estimate tokens for a single content part.
fn estimate_part_tokens(part: &ContentPart) -> u32 {
    match part {
        ContentPart::Text { text } => (text.len() / 4) as u32,
        ContentPart::Image { detail, .. } => {
            match detail.as_deref() {
                Some("low") => 85,
                _ => 1590, // high/default — one Anthropic tile
            }
        }
        ContentPart::Audio { data, .. } => {
            // ~25 tokens/sec; ensure non-empty audio gets at least 25 tokens
            ((data.len() as f64 / 4096.0) * 25.0).ceil().max(25.0) as u32
        }
        ContentPart::Document { extracted_text, .. } => extracted_text
            .as_ref()
            .map_or(500, |t| (t.len() / 4) as u32),
        ContentPart::FileRef { .. } => 50,
    }
}

/// Estimate tokens in a message list using the 1 token ≈ 4 bytes heuristic.
/// When multimodal parts are present, estimates per-part tokens instead.
/// Consistent with `estimate_request_tokens` in the LLM router.
pub(crate) fn estimate_messages_tokens(messages: &[ChatMessage]) -> u32 {
    let tokens: u32 = messages
        .iter()
        .map(|m| {
            let content_tokens: u32 = if let Some(ref parts) = m.parts {
                parts.iter().map(estimate_part_tokens).sum()
            } else {
                (m.content.len() / 4) as u32
            };
            let tool_call_tokens: u32 = m.tool_calls.as_ref().map_or(0, |tcs| {
                tcs.iter()
                    .map(|tc| ((tc.name.len() + tc.arguments.to_string().len()) / 4) as u32)
                    .sum()
            });
            content_tokens + tool_call_tokens
        })
        .sum();
    tokens
}

/// Compress context by replacing older rounds with a compact summary.
///
/// When `budget` is provided:
///   1. Discard social message pairs first (may be sufficient alone)
///   2. Use token-aware boundary to determine what to compress
///   3. Include user messages in summary (fixes previous omission)
///   4. Group summary by conversation rounds
///
/// When `budget` is `None`, uses legacy `tail_keep × 3` boundary.
pub(crate) fn compress_context(
    messages: &mut Vec<ChatMessage>,
    tail_keep: usize,
    budget: Option<&crate::context_budget::ContextBudgetManager>,
) {
    // Phase 1 (DiscardSocial) removed — the graduated compactor already
    // handles DiscardSocial as its own tier before reaching HeuristicSummary,
    // and the legacy fallback path passes budget: None which skipped this block.

    // Phase 2: Determine compression boundary
    let keep_tail = if let Some(b) = budget {
        let target = b.compaction_target_tokens();
        if target == 0 {
            // No free zone — fall back to keeping a fixed tail
            tail_keep * 3
        } else {
            // Token-aware: walk backwards counting tokens until we hit the target
            let mut tail_tokens = 0usize;
            let mut boundary = messages.len();
            for (i, msg) in messages.iter().enumerate().rev() {
                if i <= 1 {
                    break; // Never compress system + initial query
                }
                let msg_tokens = if let Some(ref parts) = msg.parts {
                    parts.iter().map(|p| estimate_part_tokens(p) as usize).sum()
                } else {
                    msg.content.len() / 4
                };
                if tail_tokens + msg_tokens > target && boundary < messages.len() {
                    break;
                }
                tail_tokens += msg_tokens;
                boundary = i;
            }
            messages.len() - boundary
        }
    } else {
        // Legacy: fixed tail_keep × 3
        tail_keep * 3
    };

    if messages.len() <= 2 + keep_tail {
        return; // Nothing to compress
    }

    let compress_end = messages.len() - keep_tail;

    // Phase 3: Build round-grouped summary from messages[2..compress_end]
    let mut summary_parts = Vec::new();
    let mut round = 1u32;
    let mut current_round_parts: Vec<String> = Vec::new();
    // Routing V2: steering interjections are kept verbatim — heuristic
    // compression must never discard or truncate a mid-workflow correction
    // (only the LLM-summary tier may absorb them). They are re-inserted
    // right after the summary message, preserving their relative order.
    let mut kept_interjections: Vec<ChatMessage> = Vec::new();

    for msg in &messages[2..compress_end] {
        if msg.role == Role::User
            && msg.parts.is_none()
            && msg
                .content
                .starts_with(crate::runner::steering::USER_INTERJECTION_PREFIX)
        {
            kept_interjections.push(msg.clone());
            continue;
        }
        // Handle multimodal parts
        if let Some(ref parts) = msg.parts {
            let role_label = match msg.role {
                Role::User => "User",
                Role::Assistant => "Assistant",
                Role::System => "System",
                Role::Tool => "Tool",
            };
            for part in parts {
                let desc = match part {
                    ContentPart::Image { .. } => format!("{role_label}: [sent an image]"),
                    ContentPart::Audio { .. } => format!("{role_label}: [sent audio]"),
                    ContentPart::Document { filename, extracted_text, .. } => {
                        let excerpt = extracted_text
                            .as_ref()
                            .map(|t| truncate_for_summary(t, 150))
                            .unwrap_or_default();
                        format!("{role_label}: [attached: {filename}] {excerpt}")
                    }
                    ContentPart::FileRef { filename, .. } => {
                        format!("{role_label}: [attached: {filename}]")
                    }
                    ContentPart::Text { text } if !text.is_empty() => {
                        format!("{role_label}: {}", truncate_for_summary(text, 150))
                    }
                    _ => continue,
                };
                current_round_parts.push(format!("  {desc}"));
            }
            continue;
        }

        match msg.role {
            Role::User => {
                // Start a new round when we see a user message (except the first)
                if !current_round_parts.is_empty() {
                    summary_parts.push(format!("Round {round}:"));
                    summary_parts.append(&mut current_round_parts);
                    round += 1;
                }
                current_round_parts.push(format!(
                    "  User: {}",
                    truncate_for_summary(&msg.content, 150)
                ));
            }
            Role::Assistant => {
                if !msg.content.is_empty() {
                    current_round_parts.push(format!(
                        "  Assistant: {}",
                        truncate_for_summary(&msg.content, 150)
                    ));
                }
                if let Some(ref tcs) = msg.tool_calls {
                    for tc in tcs {
                        current_round_parts.push(format!("  Called: {}", tc.name));
                    }
                }
            }
            Role::Tool => {
                current_round_parts.push(format!(
                    "  Result: {}",
                    truncate_for_summary(&msg.content, 100)
                ));
            }
            Role::System => {
                // Include system messages in summary (previously dropped)
                current_round_parts.push(format!(
                    "  System: {}",
                    truncate_for_summary(&msg.content, 100)
                ));
            }
        }
    }

    // Flush last round
    if !current_round_parts.is_empty() {
        summary_parts.push(format!("Round {round}:"));
        summary_parts.extend(current_round_parts);
    }

    let mut summary = format!(
        "[Context compressed: {} earlier messages in {} rounds]\n{}",
        compress_end - 2 - kept_interjections.len(),
        round,
        summary_parts.join("\n")
    );

    // Cap summary size if budget is available
    if let Some(b) = budget {
        let max_summary_chars = b.compaction_target_tokens() * 4; // rough chars estimate
        if summary.len() > max_summary_chars {
            let end = summary.floor_char_boundary(max_summary_chars);
            summary.truncate(end);
            summary.push_str("\n[...summary truncated]");
        }
    }

    // Replace messages[2..compress_end] with the summary, followed by any
    // preserved interjections (kept verbatim).
    messages.splice(
        2..compress_end,
        std::iter::once(ChatMessage::user(&summary)).chain(kept_interjections),
    );
}

/// Truncate text for inclusion in a compressed summary.
pub(crate) fn truncate_for_summary(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        text.to_string()
    } else {
        let end = text.floor_char_boundary(max_chars);
        format!("{}...", &text[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_estimate_empty_messages_returns_zero() {
        assert_eq!(estimate_messages_tokens(&[]), 0);
    }

    #[test]
    fn test_compress_context_preserves_interjections_verbatim() {
        // Routing V2: a steering interjection inside the compressed range
        // must survive heuristic compression verbatim — not truncated into
        // the summary (spec §3 compaction rule).
        let interjection = "<user_interjection ts=\"2026-08-30T00:00:00Z\">use the staging DB instead</user_interjection>";
        let mut messages = vec![ChatMessage::system("sys"), ChatMessage::user("query")];
        for i in 0..10 {
            messages.push(ChatMessage::assistant(&format!("resp {i}")));
            messages.push(ChatMessage::user(&"x".repeat(200)));
            if i == 4 {
                messages.push(ChatMessage::user(interjection));
            }
        }

        compress_context(&mut messages, 1, None); // legacy: keep tail of 3

        // The summary replaces the compressed range…
        assert!(messages[2].content.starts_with("[Context compressed"));
        assert!(
            !messages[2].content.contains("staging DB"),
            "interjection must not be absorbed into the summary"
        );
        // …and the interjection is re-inserted verbatim right after it.
        assert_eq!(messages[3].content, interjection);
        assert_eq!(messages[3].role, Role::User);
    }

    #[test]
    fn test_compress_context_zero_target_preserves_tail() {
        use crate::context_budget::ContextBudgetManager;
        use crate::daemon_config::ContextBudgetConfig;

        let cfg = ContextBudgetConfig::default();
        let mut budget = ContextBudgetManager::new(100, &cfg);
        // Register enough fixed sections to consume the entire window,
        // making free_zone_capacity (and thus compaction_target_tokens) 0.
        budget.register_section("huge_system", 100);

        let mut messages = vec![
            ChatMessage::system("sys"),
            ChatMessage::user("query"),
        ];
        for i in 0..10 {
            messages.push(ChatMessage::assistant(&format!("resp {i}")));
            messages.push(ChatMessage::user(&format!("msg {i}")));
        }
        let len_before = messages.len();

        compress_context(&mut messages, 4, Some(&budget));

        assert!(
            messages.len() > 2,
            "Should preserve more than just system + query, got {}",
            messages.len()
        );
        assert!(
            messages.len() < len_before,
            "Should still compress something ({} >= {})",
            messages.len(),
            len_before
        );
    }
}
