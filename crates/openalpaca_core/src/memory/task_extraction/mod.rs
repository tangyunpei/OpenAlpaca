//! Task output memory extraction: extracts learnable knowledge from
//! completed task outputs and persists to MemoryV2 with supersession.
//!
//! Called as fire-and-forget from the dispatcher after lead agent, DAG,
//! or pipeline execution completes successfully.

use arc_swap::ArcSwap;
use openalpaca_llm::{ChatMessage, Embedder, LlmRouter, RequestContext, RouterRequest};
use openalpaca_storage::Database;
use openalpaca_storage::models::memory::{MemoryKind, MemoryScope, MemorySource};
use openalpaca_storage::repository::{LlmUsageRepository, MemoryRepository};
use std::sync::Arc;

use crate::daemon_config::DaemonConfig;

/// Result of persisting a memory item via [`persist_memory_item`].
pub enum PersistResult {
    /// New memory inserted. Contains the new row id.
    Inserted(i64),
    /// Existing memory superseded. Contains old content, old id, and new id.
    Superseded {
        old_id: i64,
        old_content: String,
        new_id: i64,
    },
    /// Content already exists (hash collision / duplicate).
    Duplicate,
    /// Operation failed.
    Error(String),
}

/// Parameters for task memory extraction.
pub struct TaskExtractionParams {
    pub owner_id: String,
    pub task_id: String,
    pub task_description: String,
    pub task_output: String,
    /// Source path: "lead_agent", "dag", or "pipeline".
    pub source_path: String,
    /// Optional workspace ID for Workspace-scoped memories.
    /// When set, extracted memories are scoped to this workspace.
    /// When None, memories are stored as Global.
    pub workspace_id: Option<String>,
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
               \"importance\": 0.0 to 1.0,\n\
               \"confidence\": 0.0 to 1.0\n\
             }}\n\
           ]\n\
         }}\n\n\
         Rules:\n\
         - Extract key findings, decisions made, technical learnings, and user-specific insights.\n\
         - \"fact\" for objective learnings (e.g. \"Project X uses PostgreSQL 15\").\n\
         - \"preference\" for user-specific insights (e.g. \"User prefers concise summaries\").\n\
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
        messages: Arc::new(vec![
            ChatMessage::system(
                "You are a knowledge extractor for task outputs. \
                 Extract factual learnings and insights. \
                 Output ONLY valid JSON matching the schema. \
                 No markdown fences, no commentary.",
            ),
            ChatMessage::user(&user_prompt),
        ]),
        tools: Arc::new(vec![]),
        temperature: Some(0.0),
        max_tokens: Some(512),
        context: RequestContext {
            agent_id: Some(AGENT_LABEL.to_string()),
            task_id: Some(params.task_id.clone()),
        },
        tool_choice: None,
        tools_token_estimate: None,
        enable_caching: false,
        thinking: None,
        context_management: None,
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
            if let Err(e) = usage_repo.record_and_log(
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
            ) {
                tracing::warn!("Failed to persist task extraction LLM usage: {e}");
            }
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
    let jaccard_threshold = dcfg.orchestrator.memory.fts_jaccard_threshold;

    let metadata = serde_json::json!({
        "task_id": params.task_id,
        "source_path": params.source_path,
    });

    // Opt-9: Collect valid extractions first, then batch-embed once.
    struct ValidExtraction<'a> {
        content: &'a str,
        kind: MemoryKind,
        importance: f64,
        confidence: f64,
    }

    let mut valid: Vec<ValidExtraction<'_>> = Vec::new();
    for extraction in extractions.iter().take(5) {
        let content = match extraction.get("content").and_then(|v| v.as_str()) {
            Some(c) if !c.is_empty() => c,
            _ => continue,
        };
        let confidence = extraction
            .get("confidence")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.7);
        if confidence < 0.5 {
            continue;
        }
        let kind_str = extraction
            .get("kind")
            .and_then(|v| v.as_str())
            .unwrap_or("fact");
        let kind = match kind_str {
            "preference" => MemoryKind::Preference,
            _ => MemoryKind::Fact,
        };
        let importance = extraction
            .get("importance")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.6);
        valid.push(ValidExtraction {
            content,
            kind,
            importance,
            confidence,
        });
    }

    if valid.is_empty() {
        return;
    }

    // Batch-embed all content strings in a single API call.
    let batch_embeddings: Vec<Option<Vec<f32>>> = if let Some(ref emb) = embedder {
        let texts: Vec<&str> = valid.iter().map(|v| v.content).collect();
        match emb.embed(&texts).await {
            Ok(embeddings) => embeddings.into_iter().map(Some).collect(),
            Err(e) => {
                tracing::warn!("Batch embedding failed, falling back to per-item: {e}");
                vec![None; valid.len()]
            }
        }
    } else {
        vec![None; valid.len()]
    };

    // Use workspace scope if available, otherwise Global.
    let (scope, scope_id) = if let Some(ref ws) = params.workspace_id {
        (MemoryScope::Workspace, ws.as_str())
    } else {
        (MemoryScope::Global, "")
    };

    let mut stored = 0usize;
    for (ext, pre_emb) in valid.iter().zip(batch_embeddings.into_iter()) {
        let result = persist_memory_item_with_embedding(
            &repo,
            &embedder,
            &params.owner_id,
            ext.content,
            ext.kind,
            scope,
            scope_id,
            MemorySource::Tool,
            ext.importance,
            ext.confidence,
            Some(&metadata),
            supersession_threshold,
            jaccard_threshold,
            pre_emb,
        )
        .await;
        match result {
            PersistResult::Inserted(_) | PersistResult::Superseded { .. } => stored += 1,
            _ => {}
        }
    }

    tracing::info!(
        "Task extraction for '{}' ({}) completed: {stored} items stored",
        params.task_id,
        params.source_path,
    );
}

/// Persist a single memory item with embed + supersession logic.
///
/// Handles the full flow: embed content → check for similar existing memories
/// (vector or FTS fallback) → supersede or insert → store embedding.
///
/// Used by task extraction, auto-extraction, and the /remember handler.
#[allow(clippy::too_many_arguments)]
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
    fts_jaccard_threshold: f64,
) -> PersistResult {
    persist_memory_item_with_embedding(
        repo,
        embedder,
        owner_id,
        content,
        kind,
        scope,
        scope_id,
        source,
        importance,
        confidence,
        metadata,
        supersession_threshold,
        fts_jaccard_threshold,
        None,
    )
    .await
}

/// Persist a memory item with an optional pre-computed embedding (Opt-9).
/// When `pre_embedding` is provided, skips the per-item embed call.
#[allow(clippy::too_many_arguments)]
pub async fn persist_memory_item_with_embedding(
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
    fts_jaccard_threshold: f64,
    pre_embedding: Option<Vec<f32>>,
) -> PersistResult {
    // Step 1: Use pre-computed embedding or embed on demand
    let new_embedding = if let Some(emb) = pre_embedding {
        Some(emb)
    } else if let Some(emb) = embedder {
        emb.embed(&[content])
            .await
            .ok()
            .and_then(|v| v.into_iter().next())
    } else {
        None
    };

    // Step 2: Check for similar existing memories (scoped to prevent cross-scope supersession)
    let similar = if let Some(ref emb) = new_embedding {
        repo.find_similar_for_supersession(
            owner_id,
            emb,
            supersession_threshold,
            1,
            Some(scope),
            Some(scope_id),
        )
        .unwrap_or_default()
    } else {
        repo.find_similar_fts_fallback(owner_id, content, 3, Some(scope), Some(scope_id))
            .unwrap_or_default()
            .into_iter()
            .filter(|(_, jaccard)| *jaccard >= fts_jaccard_threshold)
            .collect()
    };

    if let Some((existing, _distance)) = similar.first() {
        let old_content = existing.content.clone();
        let old_id = existing.id;
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
                if let Some(ref emb) = new_embedding
                    && let Err(e) = repo.insert_embedding(new_id, emb)
                {
                    tracing::warn!(
                        "Failed to insert embedding for superseded memory #{new_id}: {e}"
                    );
                }
                tracing::debug!(
                    "Memory persist: superseded #{} -> #{}: {}",
                    old_id,
                    new_id,
                    &content[..content.len().min(60)]
                );
                PersistResult::Superseded {
                    old_id,
                    old_content,
                    new_id,
                }
            }
            Ok(_) => PersistResult::Duplicate, // hash collision
            Err(e) => {
                tracing::warn!("Memory persist: supersession failed: {e}");
                PersistResult::Error(e.to_string())
            }
        }
    } else {
        // Step 4: Insert new
        match repo.add(
            owner_id, kind, scope, scope_id, source, content, metadata, importance, confidence,
        ) {
            Ok(new_id) if new_id > 0 => {
                if let Some(ref emb) = new_embedding
                    && let Err(e) = repo.insert_embedding(new_id, emb)
                {
                    tracing::warn!("Failed to insert embedding for memory #{new_id}: {e}");
                }
                tracing::debug!(
                    "Memory persist: stored new: {}",
                    &content[..content.len().min(60)]
                );
                PersistResult::Inserted(new_id)
            }
            Ok(_) => PersistResult::Duplicate, // duplicate
            Err(e) => {
                tracing::warn!("Memory persist: failed to store: {e}");
                PersistResult::Error(e.to_string())
            }
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
        if let Some(end) = after.find("```")
            && let Ok(v) = serde_json::from_str(after[..end].trim())
        {
            return Some(v);
        }
    }
    // Try plain ``` fence
    if let Some(start) = trimmed.find("```") {
        let after = &trimmed[start + 3..];
        if let Some(end) = after.find("```")
            && let Ok(v) = serde_json::from_str(after[..end].trim())
        {
            return Some(v);
        }
    }
    None
}

#[cfg(test)]
mod tests;
