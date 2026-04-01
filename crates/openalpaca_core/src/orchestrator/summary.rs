use super::ConversationContext;
use crate::daemon_config::DaemonConfig;
use arc_swap::ArcSwap;
use openalpaca_llm::{ChatMessage, LlmRouter, RequestContext, RouterRequest};
use openalpaca_storage::ConversationRepository;
use openalpaca_storage::Database;
use openalpaca_storage::repository::LlmUsageRepository;
use std::sync::Arc;

/// Incrementally update the conversation summary if enough new older messages exist.
/// Reuses data from build_context() to avoid a second DB read.
///
/// Standalone function (not on `&self`) so it can be called from a `tokio::spawn` block.
pub(super) async fn update_summary_background(
    db: Database,
    router: Arc<LlmRouter>,
    daemon_config: Arc<ArcSwap<DaemonConfig>>,
    lane_key: String,
    ctx: ConversationContext,
) {
    // Count new older messages since last summary
    let new_older: Vec<_> = ctx
        .older_window
        .iter()
        .filter(|(id, _, _)| *id > ctx.last_summarized_id)
        .collect();
    let dcfg = daemon_config.load();
    if new_older.len() < dcfg.orchestrator.memory.summary_min_new_older_messages {
        return;
    }

    // D12: Budget pre-check — agent-specific cost for "orchestrator_summary"
    let summary_cost = router
        .cost_tracker
        .get_agent_usage("orchestrator_summary")
        .await
        .map(|s| s.total_cost_usd)
        .unwrap_or(0.0);
    if summary_cost > dcfg.orchestrator.costs.summary_max_daily_cost_usd {
        tracing::debug!("Summary update skipped: summary cost ${summary_cost:.2} exceeds cap");
        return;
    }

    // Build summarizer prompt
    let mut user_prompt = String::new();
    if !ctx.old_summary_text.is_empty() {
        user_prompt.push_str("## Previous Summary\n");
        user_prompt.push_str(&ctx.old_summary_text);
        user_prompt.push_str("\n\n");
    }
    user_prompt.push_str("## New Messages\n");
    for (_, role, content) in &new_older {
        let truncated: String = content
            .chars()
            .take(dcfg.orchestrator.memory.msg_trunc_chars)
            .collect();
        user_prompt.push_str(&format!("{}: {}\n", role, truncated));
    }
    user_prompt.push_str(&format!(
        "\nUpdate the summary incorporating these new messages. Max {} characters. Output JSON only.",
        dcfg.orchestrator.memory.summary_max_chars
    ));

    if let Some(ref m) = dcfg.orchestrator.costs.summary_model {
        tracing::debug!(model = %m, "summary using configured model");
    }
    let request = RouterRequest {
        model: dcfg.orchestrator.costs.summary_model.clone(),
        messages: Arc::new(vec![
            ChatMessage::system(
                "<role>You are a conversation summarizer for OpenAlpaca.</role>\n\n\
                 <task>Produce an updated summary incorporating new messages into the existing summary.</task>\n\n\
                 <guidelines>\n\
                 - Preserve key decisions, constraints, user preferences, and open questions\n\
                 - Be concise but retain actionable context that will help future responses\n\
                 - Focus on the human-to-assistant dialogue and decisions made\n\
                 - Ignore machine-readable JSON responses, status dumps, task listings, and slash-command outputs \
                 — these are system artifacts\n\
                 </guidelines>\n\n\
                 <output_format>Output ONLY a JSON object: {\"summary\": \"...\"}</output_format>",
            ),
            ChatMessage::user(&user_prompt),
        ]),
        tools: Arc::new(vec![]),
        temperature: Some(0.0),
        max_tokens: Some(1536),
        context: RequestContext {
            agent_id: Some("orchestrator_summary".to_string()),
            task_id: None,
        },
        tool_choice: None,
        tools_token_estimate: None,
        enable_caching: false,
        thinking: None,
        context_management: None,
        fallback_models: Vec::new(),
    };

    let call_start = std::time::Instant::now();
    let response = match router.complete(request).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("Summary update LLM call failed: {e}");
            return;
        }
    };
    let latency_ms = call_start.elapsed().as_millis() as i64;

    // D8: Record LLM usage for summarizer call
    let actual_model = response.model.as_str();
    let resolved_provider = router
        .model_registry()
        .resolve_provider(actual_model)
        .map(|p| p.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let call_cost = router.cost_tracker.calculate_cost(
        actual_model,
        response.usage.input_tokens as u32,
        response.usage.output_tokens as u32,
    );
    let usage_repo = LlmUsageRepository::new(&db);

    // Parse response (try raw JSON, then ```json fence, then plain ``` fence)
    let parsed: serde_json::Value = match serde_json::from_str(response.content.trim()) {
        Ok(v) => v,
        Err(_) => {
            let trimmed = response.content.trim();
            let json_str = if let Some(start) = trimmed.find("```json") {
                let after = &trimmed[start + 7..];
                after
                    .find("```")
                    .map(|end| &after[..end])
                    .unwrap_or(trimmed)
            } else if let Some(start) = trimmed.find("```") {
                let after = &trimmed[start + 3..];
                after
                    .find("```")
                    .map(|end| &after[..end])
                    .unwrap_or(trimmed)
            } else {
                trimmed
            };
            match serde_json::from_str(json_str.trim()) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!("Summary update: malformed JSON from LLM: {e}");
                    let _ = usage_repo.record_and_log(
                        "orchestrator_summary",
                        None,
                        &resolved_provider,
                        actual_model,
                        response.usage.input_tokens as i32,
                        response.usage.output_tokens as i32,
                        call_cost,
                        latency_ms,
                        "error",
                        Some(&format!("JSON parse: {e}")),
                    );
                    return;
                }
            }
        }
    };

    let new_summary = match parsed.get("summary").and_then(|s| s.as_str()) {
        Some(s) => s,
        None => {
            tracing::warn!("Summary update: LLM response missing 'summary' field");
            let _ = usage_repo.record_and_log(
                "orchestrator_summary",
                None,
                &resolved_provider,
                actual_model,
                response.usage.input_tokens as i32,
                response.usage.output_tokens as i32,
                call_cost,
                latency_ms,
                "error",
                Some("Missing 'summary' field in LLM response"),
            );
            return;
        }
    };

    // Log successful usage (after validating the response payload)
    if let Err(e) = usage_repo.record_and_log(
        "orchestrator_summary",
        None,
        &resolved_provider,
        actual_model,
        response.usage.input_tokens as i32,
        response.usage.output_tokens as i32,
        call_cost,
        latency_ms,
        "success",
        None,
    ) {
        tracing::warn!("Failed to persist summary LLM usage: {e}");
    }

    let new_summary: String = new_summary
        .chars()
        .take(dcfg.orchestrator.memory.summary_max_chars)
        .collect();
    let new_last_id = new_older
        .last()
        .map(|(id, _, _)| *id)
        .unwrap_or(ctx.last_summarized_id);

    // Save with optimistic locking to conversations table
    let repo = ConversationRepository::new(&db);
    match repo.update_summary_optimistic(
        &lane_key,
        ctx.summary_version,
        &new_summary,
        new_last_id,
    ) {
        Ok(true) => tracing::debug!("Summary updated successfully"),
        Ok(false) => {
            tracing::warn!("Summary update: version mismatch, retrying once");
            if let Ok((_, new_version, _)) = repo.get_summary(&lane_key) {
                match repo.update_summary_optimistic(
                    &lane_key,
                    new_version,
                    &new_summary,
                    new_last_id,
                ) {
                    Ok(true) => tracing::debug!("Summary updated on retry"),
                    Ok(false) => {
                        tracing::warn!("Summary update: version conflict persists, discarding")
                    }
                    Err(e) => tracing::warn!("Summary retry failed: {e}"),
                }
            }
        }
        Err(e) => tracing::warn!("Summary update: save failed: {e}"),
    }
}
