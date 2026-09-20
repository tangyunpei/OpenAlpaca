use openalpaca_llm::{ChatMessage, ContentPart, Role, ToolDefinition};

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

/// Estimate tokens for a tool surface using the 1 token ≈ 4 bytes heuristic
/// over each tool's description, JSON parameter schema, and input examples.
///
/// This is the single source of truth for the "tools" section cost: the
/// agentic loop's Router-retry tool-token estimate and every
/// `register_section("tools", …)` surface builder (lead agent, main-loop
/// simple-query handler, skill invocation) call this rather than pricing
/// tools at a flat per-tool constant, which under-counts non-trivial schemas.
pub(crate) fn estimate_tools_tokens(tools: &[ToolDefinition]) -> usize {
    let tool_bytes: usize = tools
        .iter()
        .map(|t| {
            let base = t.description.len() + t.parameters.to_string().len();
            let examples = t.input_examples.as_ref().map_or(0, |ex| {
                ex.iter().map(|e| e.to_string().len()).sum()
            });
            base + examples
        })
        .sum();
    tool_bytes / 4
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

    /// A4's byte-based tools estimate must be one function shared by the
    /// agentic loop and every `register_section("tools", …)` surface builder
    /// (`runner/lead_agent/mod.rs`, `simple_query_handler.rs`,
    /// `orchestrator/skill/invocation.rs`). A flat `tools.len() * 200` guess
    /// under-counts non-trivial schemas — this test's independent oracle
    /// (the formula duplicated here, not by calling the function under test)
    /// proves `estimate_tools_tokens` matches it and diverges sharply from
    /// the old flat constant for a realistic tool surface.
    #[test]
    fn test_estimate_tools_tokens_matches_byte_formula_not_flat_200() {
        use openalpaca_llm::ToolDefinition;

        let tools = vec![
            ToolDefinition {
                name: "file_read".to_string(),
                description: "Read the contents of a file from the workspace filesystem, \
                    optionally restricted to a byte range. Returns UTF-8 text or an error \
                    if the file does not exist or is not valid UTF-8."
                    .to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "path": {"type": "string", "description": "Workspace-relative path"},
                        "start_byte": {"type": "integer", "description": "Optional start offset"},
                        "end_byte": {"type": "integer", "description": "Optional end offset"}
                    },
                    "required": ["path"]
                }),
                strict: None,
                input_examples: Some(vec![serde_json::json!({"path": "notes/todo.md"})]),
            },
            ToolDefinition {
                name: "web_search".to_string(),
                description: "Search the web for up-to-date information and return a list of \
                    ranked results with titles, URLs, and short snippets."
                    .to_string(),
                parameters: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "query": {"type": "string", "description": "The search query"},
                        "max_results": {"type": "integer", "description": "Cap on result count"}
                    },
                    "required": ["query"]
                }),
                strict: None,
                input_examples: None,
            },
        ];

        // Independent oracle: the same "(description + parameters +
        // input_examples bytes) / 4" formula the loop uses, written out
        // separately here rather than by calling `estimate_tools_tokens`.
        let expected_bytes: usize = tools
            .iter()
            .map(|t| {
                let base = t.description.len() + t.parameters.to_string().len();
                let examples = t.input_examples.as_ref().map_or(0, |ex| {
                    ex.iter().map(|e| e.to_string().len()).sum()
                });
                base + examples
            })
            .sum();
        let expected = expected_bytes / 4;

        assert_eq!(estimate_tools_tokens(&tools), expected);

        // Regression guard: the old flat guess must not coincide with the
        // byte-based estimate for this realistic, non-trivial tool surface.
        let flat_200 = tools.len() * 200;
        assert_ne!(
            estimate_tools_tokens(&tools),
            flat_200,
            "byte estimate should diverge from the flat 200-per-tool guess"
        );

        // And registering it on a real budget must carry the exact same
        // number through — no re-derivation, no drift.
        use crate::context_budget::ContextBudgetManager;
        use crate::daemon_config::ContextBudgetConfig;
        let mut budget = ContextBudgetManager::new(200_000, &ContextBudgetConfig::default());
        budget.register_section("tools", estimate_tools_tokens(&tools));
        let tools_section = budget
            .section_breakdown()
            .into_iter()
            .find(|(name, _)| *name == "tools")
            .map(|(_, tokens)| tokens);
        assert_eq!(tools_section, Some(expected));
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
