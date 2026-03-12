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
    tokens.max(100)
}

/// Compress context by replacing older rounds with a compact summary.
///
/// Preserves:
/// - Message 0 (system prompt)
/// - Message 1 (initial user query)
/// - The last `tail_keep × 3` messages (most recent rounds)
///
/// Everything in between is replaced with a single user message summarizing
/// what happened in those earlier rounds (tool calls made, brief results).
pub(crate) fn compress_context(messages: &mut Vec<ChatMessage>, tail_keep: usize) {
    // Each "round" is roughly: 1 assistant message + N tool results ≈ 3 messages
    let keep_tail = tail_keep * 3;
    if messages.len() <= 2 + keep_tail {
        return; // Nothing to compress
    }

    let compress_end = messages.len() - keep_tail;

    // Build summary from messages[2..compress_end]
    let mut summary_parts = Vec::new();
    for msg in &messages[2..compress_end] {
        // Summarize multimodal parts when present
        if let Some(ref parts) = msg.parts {
            let role_label = match msg.role {
                Role::User => "User",
                Role::Assistant => "Assistant",
                Role::System => "System",
                Role::Tool => "Tool",
            };
            for part in parts {
                match part {
                    ContentPart::Image { .. } => {
                        summary_parts.push(format!("- {role_label}: [sent an image]"));
                    }
                    ContentPart::Audio { .. } => {
                        summary_parts.push(format!("- {role_label}: [sent audio]"));
                    }
                    ContentPart::Document {
                        filename,
                        extracted_text,
                        ..
                    } => {
                        let excerpt = extracted_text
                            .as_ref()
                            .map(|t| truncate_for_summary(t, 200))
                            .unwrap_or_default();
                        summary_parts
                            .push(format!("- {role_label}: [attached: {filename}] {excerpt}"));
                    }
                    ContentPart::FileRef { filename, .. } => {
                        summary_parts.push(format!("- {role_label}: [attached: {filename}]"));
                    }
                    ContentPart::Text { text } if !text.is_empty() => {
                        summary_parts.push(format!(
                            "- {role_label}: {}",
                            truncate_for_summary(text, 200)
                        ));
                    }
                    _ => {}
                }
            }
            continue;
        }

        match msg.role {
            Role::Assistant => {
                if !msg.content.is_empty() {
                    summary_parts.push(format!(
                        "- Agent: {}",
                        truncate_for_summary(&msg.content, 200)
                    ));
                }
                if let Some(ref tcs) = msg.tool_calls {
                    for tc in tcs {
                        summary_parts.push(format!("- Called: {}", tc.name));
                    }
                }
            }
            Role::Tool => {
                summary_parts.push(format!(
                    "- Result: {}",
                    truncate_for_summary(&msg.content, 300)
                ));
            }
            _ => {}
        }
    }

    let summary = format!(
        "[Context compressed: {} earlier messages summarized]\n{}",
        compress_end - 2,
        summary_parts.join("\n")
    );

    // Replace messages[2..compress_end] with a single user message
    messages.splice(
        2..compress_end,
        std::iter::once(ChatMessage::user(&summary)),
    );
}

/// Truncate text for inclusion in a compressed summary.
fn truncate_for_summary(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        text.to_string()
    } else {
        let end = text.floor_char_boundary(max_chars);
        format!("{}...", &text[..end])
    }
}
