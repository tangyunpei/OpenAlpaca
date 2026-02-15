//! Task output memory extraction: extracts learnable knowledge from
//! completed task outputs and persists to MemoryV2 with supersession.
//!
//! Called as fire-and-forget from the dispatcher after lead agent, DAG,
//! or pipeline execution completes successfully.

use arc_swap::ArcSwap;
use openalpaca_llm::{ChatMessage, Embedder, LlmRouter, RequestContext, RouterRequest};
use openalpaca_storage::models::memory::{MemoryKind, MemoryScope, MemorySource};
use openalpaca_storage::repository::{LlmUsageRepository, MemoryRepository};
use openalpaca_storage::Database;
use std::sync::Arc;

use crate::daemon_config::DaemonConfig;

/// Parameters for task memory extraction.
pub struct TaskExtractionParams {
    pub owner_id: String,
    pub task_id: String,
    pub task_description: String,
    pub task_output: String,
    /// Source path: "lead_agent", "dag", or "pipeline".
    pub source_path: String,
}

const AGENT_LABEL: &str = "orchestrator_task_extract";

/// Extract learnable knowledge from a task output and save to MemoryV2.
///
/// Designed to be called from `tokio::spawn` in a fire-and-forget pattern.
/// All errors are logged, never propagated.
pub async fn extract_task_memories(
    params: TaskExtractionParams,
    db: Database,
    router: Arc<LlmRouter>,
    embedder: Option<Arc<dyn Embedder>>,
    daemon_config: Arc<ArcSwap<DaemonConfig>>,
) {
    let dcfg = daemon_config.load();

    // Guard: skip short output
    let min_len = dcfg.orchestrator.costs.task_extract_min_content_len;
    if params.task_output.len() < min_len {
        tracing::debug!(
            "Task extraction skipped: output too short ({} < {min_len})",
            params.task_output.len()
        );
        return;
    }

    // Guard: cost budget
    let extract_cost = router
        .cost_tracker
        .get_agent_usage(AGENT_LABEL)
        .await
        .map(|s| s.total_cost_usd)
        .unwrap_or(0.0);
    let max_daily = dcfg.orchestrator.costs.task_extract_max_daily_cost_usd;
    if extract_cost > max_daily {
        tracing::debug!(
            "Task extraction skipped: cost ${extract_cost:.2} exceeds cap ${max_daily:.2}"
        );
        return;
    }

    // Build extraction prompt
    let desc_trunc: String = params.task_description.chars().take(500).collect();
    let output_trunc: String = params.task_output.chars().take(2000).collect();

    let user_prompt = format!(
        "## Task Output\nTask: {}\nOutput:\n{}\n\n\
         Analyze this task output and extract learnable knowledge.\n\
         Output ONLY a JSON object with this schema:\n\
         {{\n\
           \"extractions\": [\n\
             {{\n\
               \"content\": \"a concise factual statement\",\n\
               \"kind\": \"fact\" or \"preference\",\n\
               \"scope\": \"global\" or \"workspace\",\n\
               \"importance\": 0.0 to 1.0,\n\
               \"confidence\": 0.0 to 1.0\n\
             }}\n\
           ]\n\
         }}\n\n\
         Rules:\n\
         - Extract key findings, decisions made, technical learnings, and user-specific insights.\n\
         - \"fact\" for objective learnings (e.g. \"Project X uses PostgreSQL 15\").\n\
         - \"preference\" for user-specific insights (e.g. \"User prefers concise summaries\").\n\
         - \"global\" scope for universally applicable knowledge.\n\
         - \"workspace\" scope for project/task-specific knowledge.\n\
         - importance: how useful is this for future tasks (0.3 = trivial, 0.9 = critical).\n\
         - confidence: how certain is this extraction (0.5 = inferred, 1.0 = explicitly stated).\n\
         - Only extract what is clearly stated or strongly implied. Do not hallucinate.\n\
         - Prefer fewer high-quality extractions over many low-quality ones.\n\
         - Maximum 5 extractions per task output.\n\
         - If nothing can be extracted, return {{\"extractions\": []}}.",
        desc_trunc, output_trunc
    );

    let request = RouterRequest {
        model: None,
        messages: vec![
            ChatMessage::system(
                "You are a knowledge extractor for task outputs. \
                 Extract factual learnings and insights. \
                 Output ONLY valid JSON matching the schema. \
                 No markdown fences, no commentary.",
            ),
            ChatMessage::user(&user_prompt),
        ],
        tools: vec![],
        temperature: Some(0.0),
        max_tokens: Some(512),
        context: RequestContext {
            agent_id: Some(AGENT_LABEL.to_string()),
            task_id: Some(params.task_id.clone()),
        },
    };

    // LLM call
    let call_start = std::time::Instant::now();
    let response = match router.complete(request).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("Task extraction LLM call failed: {e}");
            return;
        }
    };
    let latency_ms = call_start.elapsed().as_millis() as i64;

    // Record LLM usage
    let actual_model = response.model.clone();
    let resolved_provider = router
        .model_registry()
        .resolve_provider(&actual_model)
        .map(|p| p.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let call_cost = router.cost_tracker.calculate_cost(
        &actual_model,
        response.usage.input_tokens as u32,
        response.usage.output_tokens as u32,
    );
    let usage_repo = LlmUsageRepository::new(&db);

    // Parse JSON
    let parsed: serde_json::Value = match parse_json_response(&response.content) {
        Some(v) => v,
        None => {
            let _ = usage_repo.record_and_log(
                AGENT_LABEL,
                Some(&params.task_id),
                &resolved_provider,
                &actual_model,
                response.usage.input_tokens as i32,
                response.usage.output_tokens as i32,
                call_cost,
                latency_ms,
                "error",
                Some("JSON parse failure"),
            );
            tracing::warn!("Task extraction: malformed JSON from LLM");
            return;
        }
    };

    // Log successful usage
    if let Err(e) = usage_repo.record_and_log(
        AGENT_LABEL,
        Some(&params.task_id),
        &resolved_provider,
        &actual_model,
        response.usage.input_tokens as i32,
        response.usage.output_tokens as i32,
        call_cost,
        latency_ms,
        "success",
        None,
    ) {
        tracing::warn!("Failed to persist task extraction LLM usage: {e}");
    }

    // Process extractions
    let extractions = match parsed.get("extractions").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => {
            tracing::debug!("Task extraction: no extractions array in response");
            return;
        }
    };

    if extractions.is_empty() {
        return;
    }

    let repo = MemoryRepository::new(&db);
    let supersession_threshold = dcfg.orchestrator.memory.supersession_distance_threshold;

    let metadata = serde_json::json!({
        "task_id": params.task_id,
        "source_path": params.source_path,
    });

    let mut stored = 0usize;
    for extraction in extractions.iter().take(5) {
        let content = match extraction.get("content").and_then(|v| v.as_str()) {
            Some(c) if !c.is_empty() => c,
            _ => continue,
        };
        let kind_str = extraction
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or("fact");
        let scope_str = extraction
            .get("scope")
            .and_then(|v| v.as_str())
            .unwrap_or("global");
        let importance = extraction
            .get("importance")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.6);
        let confidence = extraction
            .get("confidence")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.7);

        if confidence < 0.5 {
            continue;
        }

        let kind = match kind_str {
            "preference" => MemoryKind::Preference,
            _ => MemoryKind::Fact,
        };
        let scope = match scope_str {
            "workspace" => MemoryScope::Workspace,
            _ => MemoryScope::Global,
        };
        let scope_id = if matches!(scope, MemoryScope::Workspace) {
            &params.task_id
        } else {
            ""
        };

        persist_memory_item(
            &repo,
            &embedder,
            &params.owner_id,
            content,
            kind,
            scope,
            scope_id,
            MemorySource::Tool,
            importance,
            confidence,
            Some(&metadata),
            supersession_threshold,
        )
        .await;
        stored += 1;
    }

    tracing::info!(
        "Task extraction for '{}' ({}) completed: {stored} items stored",
        params.task_id,
        params.source_path,
    );
}

/// Persist a single memory item with embed + supersession logic.
pub async fn persist_memory_item(
    repo: &MemoryRepository<'_>,
    embedder: &Option<Arc<dyn Embedder>>,
    owner_id: &str,
    content: &str,
    kind: MemoryKind,
    scope: MemoryScope,
    scope_id: &str,
    source: MemorySource,
    importance: f64,
    confidence: f64,
    metadata: Option<&serde_json::Value>,
    supersession_threshold: f64,
) {
    // Step 1: Embed
    let new_embedding = if let Some(emb) = embedder {
        emb.embed(&[content])
            .await
            .ok()
            .and_then(|v| v.into_iter().next())
    } else {
        None
    };

    // Step 2: Check for similar existing memories
    let similar = if let Some(ref emb) = new_embedding {
        repo.find_similar_for_supersession(owner_id, emb, supersession_threshold, 1)
            .unwrap_or_default()
    } else {
        repo.find_similar_fts_fallback(owner_id, content, 3)
            .unwrap_or_default()
            .into_iter()
            .filter(|(_, jaccard)| *jaccard >= 0.4)
            .collect()
    };

    if let Some((existing, _distance)) = similar.first() {
        // Step 3: Supersede
        match repo.supersede(
            existing.id,
            content,
            kind,
            scope,
            scope_id,
            source,
            importance,
            confidence,
            metadata,
        ) {
            Ok(new_id) if new_id > 0 => {
                if let Some(ref emb) = new_embedding {
                    if let Err(e) = repo.insert_embedding(new_id, emb) {
                        tracing::warn!(
                            "Failed to insert embedding for superseded memory #{new_id}: {e}"
                        );
                    }
                }
                tracing::debug!(
                    "Task extraction: superseded memory #{} -> #{}: {}",
                    existing.id,
                    new_id,
                    &content[..content.len().min(60)]
                );
            }
            Ok(_) => {} // hash collision
            Err(e) => tracing::warn!("Task extraction: supersession failed: {e}"),
        }
    } else {
        // Step 4: Insert new
        match repo.add(
            owner_id, kind, scope, scope_id, source, content, metadata, importance, confidence,
        ) {
            Ok(new_id) if new_id > 0 => {
                if let Some(ref emb) = new_embedding {
                    if let Err(e) = repo.insert_embedding(new_id, emb) {
                        tracing::warn!("Failed to insert embedding for memory #{new_id}: {e}");
                    }
                }
                tracing::debug!(
                    "Task extraction: stored memory: {}",
                    &content[..content.len().min(60)]
                );
            }
            Ok(_) => {} // duplicate
            Err(e) => tracing::warn!("Task extraction: failed to store memory: {e}"),
        }
    }
}

/// Try to parse JSON from LLM response, handling both raw and fenced formats.
pub fn parse_json_response(content: &str) -> Option<serde_json::Value> {
    let trimmed = content.trim();
    if let Ok(v) = serde_json::from_str(trimmed) {
        return Some(v);
    }
    // Try ```json fence
    if let Some(start) = trimmed.find("```json") {
        let after = &trimmed[start + 7..];
        if let Some(end) = after.find("```") {
            if let Ok(v) = serde_json::from_str(after[..end].trim()) {
                return Some(v);
            }
        }
    }
    // Try plain ``` fence
    if let Some(start) = trimmed.find("```") {
        let after = &trimmed[start + 3..];
        if let Some(end) = after.find("```") {
            if let Ok(v) = serde_json::from_str(after[..end].trim()) {
                return Some(v);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_json_response_raw() {
        let input = r#"{"extractions": [{"content": "test", "kind": "fact"}]}"#;
        let result = parse_json_response(input);
        assert!(result.is_some());
        let arr = result.unwrap()["extractions"].as_array().unwrap().len();
        assert_eq!(arr, 1);
    }

    #[test]
    fn test_parse_json_response_fenced() {
        let input = "```json\n{\"extractions\": []}\n```";
        let result = parse_json_response(input);
        assert!(result.is_some());
    }

    #[test]
    fn test_parse_json_response_plain_fence() {
        let input = "```\n{\"extractions\": [{\"content\": \"hello\"}]}\n```";
        let result = parse_json_response(input);
        assert!(result.is_some());
    }

    #[test]
    fn test_parse_json_response_invalid() {
        let input = "This is not JSON at all";
        assert!(parse_json_response(input).is_none());
    }

    #[test]
    fn test_parse_json_response_with_whitespace() {
        let input = "  \n  {\"extractions\": []}  \n  ";
        let result = parse_json_response(input);
        assert!(result.is_some());
    }
}
