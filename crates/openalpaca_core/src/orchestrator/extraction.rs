use crate::bus::EventBus;
use crate::daemon_config::DaemonConfig;
use crate::events::SystemEvent;
use crate::memory::task_extraction::{PersistResult, persist_memory_item};
use crate::middleware::user::{UserDocument, parse_user_markdown, render_user_markdown};
use arc_swap::ArcSwap;
use chrono::Utc;
use openalpaca_llm::{ChatMessage, LlmRouter, RequestContext, RouterRequest};
use openalpaca_storage::Database;
use openalpaca_storage::repository::{LlmUsageRepository, MemoryRepository};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};

/// Automatically extract user traits from a conversation turn.
///
/// Runs post-response in a background spawn — never adds latency to user-facing
/// responses. Guarded by frequency (every N turns), daily cost cap, and
/// content length filter.
///
/// Standalone function (not on `&self`) so it can be called from a `tokio::spawn` block.
#[allow(clippy::too_many_arguments)]
pub(super) async fn extract_user_traits_background(
    db: Database,
    router: Arc<LlmRouter>,
    daemon_config: Arc<ArcSwap<DaemonConfig>>,
    extraction_turn_counter: Arc<Mutex<HashMap<String, usize>>>,
    embedder: Option<Arc<dyn openalpaca_llm::Embedder>>,
    user_path: Arc<RwLock<Option<PathBuf>>>,
    user_document: Arc<RwLock<Option<UserDocument>>>,
    bus: EventBus,
    lane_key: String,
    user_message: String,
    assistant_response: String,
    owner_id: String,
) {
    let dcfg = daemon_config.load();

    // Content filter: skip short messages and slash commands
    if user_message.len() < dcfg.orchestrator.costs.extract_min_content_len
        || user_message.starts_with('/')
    {
        return;
    }

    // Frequency gate: only extract every N turns per lane
    {
        let mut counter = match extraction_turn_counter.lock() {
            Ok(c) => c,
            Err(poisoned) => poisoned.into_inner(),
        };
        let count = counter.entry(lane_key.to_string()).or_insert(0);
        *count += 1;
        if *count < dcfg.orchestrator.costs.extract_every_n_turns {
            return;
        }
        *count = 0;
    }

    // Budget pre-check — agent-specific cost for "orchestrator_user_extract"
    let extract_cost = router
        .cost_tracker
        .get_agent_usage("orchestrator_user_extract")
        .await
        .map(|s| s.total_cost_usd)
        .unwrap_or(0.0);
    if extract_cost > dcfg.orchestrator.costs.extract_max_daily_cost_usd {
        tracing::debug!("User extraction skipped: cost ${extract_cost:.2} exceeds cap");
        return;
    }

    // Truncate inputs to keep extraction prompt small
    let user_trunc: String = user_message.chars().take(800).collect();
    let asst_trunc: String = assistant_response.chars().take(400).collect();

    let user_prompt = format!(
        "<conversation_turn>\nUser: {}\nAssistant: {}\n</conversation_turn>\n\n\
         Analyze this conversation turn and extract any user traits.\n\n\
         <output_schema>\n\
         {{\n\
           \"extractions\": [\n\
             {{\n\
               \"target\": \"profile\" or \"memory\",\n\
               \"field\": \"identity.Name\" | \"identity.Timezone\" | \"identity.Pronouns\" | \
         \"communication_style\" | \"expertise\" | \"projects\" | \"preferences\" | \"notes\",\n\
               \"value\": \"the extracted value\",\n\
               \"confidence\": 0.0 to 1.0,\n\
               \"action\": \"set\" or \"update\"\n\
             }}\n\
           ]\n\
         }}\n\
         </output_schema>\n\n\
         <extraction_rules>\n\
         - \"target\": \"profile\" for stable identity traits (name, timezone, expertise, style). Requires confidence >= 0.8.\n\
         - \"target\": \"memory\" for situational preferences. Confidence >= 0.5.\n\
         - For identity fields, use \"identity.<Key>\" (e.g. \"identity.Name\").\n\
         - \"action\": \"set\" (default) fills only empty profile fields. \"action\": \"update\" replaces existing values.\n\
         - Use \"update\" ONLY when the user explicitly states a change to something previously known \
         (e.g. \"I moved to Tokyo\", \"call me Alex now\", \"I switched to vim\"). Requires confidence >= 0.9.\n\
         - Only extract information that is clearly stated or strongly implied in the conversation.\n\
         - Be conservative. Prefer fewer high-confidence extractions over many low-confidence ones.\n\
         - If nothing can be extracted, return {{\"extractions\": []}}.\n\
         </extraction_rules>",
        user_trunc, asst_trunc
    );

    if let Some(ref m) = dcfg.orchestrator.costs.extraction_model {
        tracing::debug!(model = %m, "extraction using configured model");
    }
    let request = RouterRequest {
        model: dcfg.orchestrator.costs.extraction_model.clone(),
        messages: Arc::new(vec![
            ChatMessage::system(
                "<role>You are a user trait extractor for OpenAlpaca.</role>\n\
                 <output_format>Output ONLY valid JSON matching the schema. No markdown fences, no commentary.</output_format>",
            ),
            ChatMessage::user(&user_prompt),
        ]),
        tools: Arc::new(vec![]),
        temperature: Some(0.0),
        max_tokens: Some(256),
        context: RequestContext {
            agent_id: Some("orchestrator_user_extract".to_string()),
            task_id: None,
        },
        tool_choice: None,
        tools_token_estimate: None,
        enable_caching: false,
        thinking: None,
    };

    let call_start = std::time::Instant::now();
    let response = match router.complete(request).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("User extraction LLM call failed: {e}");
            return;
        }
    };
    let latency_ms = call_start.elapsed().as_millis() as i64;

    // Record LLM usage
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

    // Parse response JSON (try raw, then ```json fence)
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
                    tracing::warn!("User extraction: malformed JSON from LLM: {e}");
                    let _ = usage_repo.record_and_log(
                        "orchestrator_user_extract",
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

    // Log successful usage
    if let Err(e) = usage_repo.record_and_log(
        "orchestrator_user_extract",
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
        tracing::warn!("Failed to persist extraction LLM usage: {e}");
    }

    // Process extractions
    let extractions = match parsed.get("extractions").and_then(|v| v.as_array()) {
        Some(arr) => arr,
        None => {
            tracing::debug!("User extraction: no extractions array in response");
            return;
        }
    };

    if extractions.is_empty() {
        return;
    }

    // Collect profile-level patches and memory-level items separately
    let mut profile_patches: HashMap<String, serde_json::Value> = HashMap::new();
    let mut memory_items: Vec<String> = Vec::new();

    let mem_cfg = &daemon_config.load().orchestrator.memory;
    let profile_threshold = mem_cfg.profile_confidence_threshold;
    let profile_update_threshold = mem_cfg.profile_update_confidence_threshold;
    let memory_threshold = mem_cfg.memory_confidence_threshold;

    for extraction in extractions {
        let target = extraction
            .get("target")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let field = extraction
            .get("field")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let value = extraction
            .get("value")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let confidence = extraction
            .get("confidence")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);

        if value.is_empty() || field.is_empty() {
            continue;
        }

        match target {
            "profile" => {
                if confidence < profile_threshold {
                    continue;
                }
                let action = extraction
                    .get("action")
                    .and_then(|v| v.as_str())
                    .unwrap_or("set");
                // "update" requires higher confidence than "set"
                if action == "update" && confidence < profile_update_threshold {
                    tracing::debug!(
                        "Extraction: skipping update action for '{}' (confidence {:.2} < {:.1})",
                        field,
                        confidence,
                        profile_update_threshold
                    );
                    continue;
                }
                // Route identity fields into the identity object
                if let Some(key) = field.strip_prefix("identity.") {
                    let identity_obj = profile_patches
                        .entry("identity".to_string())
                        .or_insert_with(|| serde_json::json!({}));
                    if let Some(obj) = identity_obj.as_object_mut() {
                        obj.insert(
                            key.to_string(),
                            serde_json::json!({
                                "value": value,
                                "action": action,
                            }),
                        );
                    }
                } else {
                    // Direct section field (e.g. "expertise", "preferences")
                    profile_patches.insert(
                        field.to_string(),
                        serde_json::json!({
                            "value": value,
                            "action": action,
                        }),
                    );
                }
            }
            "memory" => {
                if confidence < memory_threshold {
                    continue;
                }
                memory_items.push(value.to_string());
            }
            _ => {
                tracing::debug!("User extraction: unknown target '{target}', skipping");
            }
        }
    }

    // Write memory-level items to Memory v2 (with supersession)
    if !memory_items.is_empty() {
        let repo = MemoryRepository::new(&db);
        let dcfg = daemon_config.load();
        let supersession_threshold = dcfg.orchestrator.memory.supersession_distance_threshold;
        let jaccard_threshold = dcfg.orchestrator.memory.fts_jaccard_threshold;

        for item in &memory_items {
            // User trait memories use Global scope intentionally:
            // user preferences and personality traits are universal,
            // not workspace-specific (e.g., "prefers dark mode" applies everywhere).
            let result = persist_memory_item(
                &repo,
                &embedder,
                &owner_id,
                item,
                openalpaca_storage::models::memory::MemoryKind::Preference,
                openalpaca_storage::models::memory::MemoryScope::Global,
                "",
                openalpaca_storage::models::memory::MemorySource::Conversation,
                0.6,
                0.7,
                None,
                supersession_threshold,
                jaccard_threshold,
            )
            .await;
            if let PersistResult::Error(e) = &result {
                tracing::warn!("Failed to persist extracted memory item: {e}");
            }
        }
    }

    // Write profile-level patches to USER.md
    if !profile_patches.is_empty() {
        apply_profile_patches(&user_path, &user_document, &bus, &profile_patches).await;
    }
}

/// Apply extracted profile patches to USER.md using the sections-mode pattern.
async fn apply_profile_patches(
    user_path: &Arc<RwLock<Option<PathBuf>>>,
    user_document: &Arc<RwLock<Option<UserDocument>>>,
    bus: &EventBus,
    patches: &HashMap<String, serde_json::Value>,
) {
    let path = match user_path.read() {
        Ok(guard) => match guard.clone() {
            Some(p) => p,
            None => {
                tracing::debug!("Extraction: no user_path set, skipping profile patches");
                return;
            }
        },
        Err(_) => return,
    };

    // Read current USER.md
    let current_content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("Extraction: failed to read USER.md: {e}");
            return;
        }
    };

    let mut doc = match parse_user_markdown(&current_content) {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!("Extraction: failed to parse USER.md: {e}");
            return;
        }
    };

    let mut modified_sections: Vec<String> = Vec::new();

    for (field, value) in patches {
        match field.as_str() {
            "identity" => {
                if let Some(obj) = value.as_object() {
                    for (key, val) in obj {
                        let v = val
                            .get("value")
                            .and_then(|v| v.as_str())
                            .or_else(|| val.as_str()) // backward compat: bare string
                            .unwrap_or("");
                        let action =
                            val.get("action").and_then(|a| a.as_str()).unwrap_or("set");
                        if v.is_empty() {
                            continue;
                        }
                        let existing = doc.identity.get(key).cloned().unwrap_or_default();
                        if existing.is_empty() || action == "update" {
                            doc.identity.insert(key.clone(), v.to_string());
                            modified_sections.push(format!("identity.{}", key));
                        }
                    }
                }
            }
            "communication_style" => {
                let v = value
                    .get("value")
                    .and_then(|v| v.as_str())
                    .or_else(|| value.as_str())
                    .unwrap_or("");
                let action = value
                    .get("action")
                    .and_then(|a| a.as_str())
                    .unwrap_or("set");
                if !v.is_empty() && (doc.communication_style.is_empty() || action == "update") {
                    doc.communication_style = v.to_string();
                    modified_sections.push("communication_style".to_string());
                }
            }
            "expertise" => {
                let v = value
                    .get("value")
                    .and_then(|v| v.as_str())
                    .or_else(|| value.as_str())
                    .unwrap_or("");
                let action = value
                    .get("action")
                    .and_then(|a| a.as_str())
                    .unwrap_or("set");
                if !v.is_empty() && (doc.expertise.is_empty() || action == "update") {
                    doc.expertise = v.to_string();
                    modified_sections.push("expertise".to_string());
                }
            }
            "projects" => {
                let v = value
                    .get("value")
                    .and_then(|v| v.as_str())
                    .or_else(|| value.as_str())
                    .unwrap_or("");
                let action = value
                    .get("action")
                    .and_then(|a| a.as_str())
                    .unwrap_or("set");
                if !v.is_empty() && (doc.projects.is_empty() || action == "update") {
                    doc.projects = v.to_string();
                    modified_sections.push("projects".to_string());
                }
            }
            "preferences" => {
                let v = value
                    .get("value")
                    .and_then(|v| v.as_str())
                    .or_else(|| value.as_str())
                    .unwrap_or("");
                let action = value
                    .get("action")
                    .and_then(|a| a.as_str())
                    .unwrap_or("set");
                if !v.is_empty() && (doc.preferences.is_empty() || action == "update") {
                    doc.preferences = v.to_string();
                    modified_sections.push("preferences".to_string());
                }
            }
            "notes" => {
                let v = value
                    .get("value")
                    .and_then(|v| v.as_str())
                    .or_else(|| value.as_str())
                    .unwrap_or("");
                let action = value
                    .get("action")
                    .and_then(|a| a.as_str())
                    .unwrap_or("set");
                if !v.is_empty() && (doc.notes.is_empty() || action == "update") {
                    doc.notes = v.to_string();
                    modified_sections.push("notes".to_string());
                }
            }
            _ => {
                tracing::debug!("Extraction: unknown profile field '{field}', skipping");
            }
        }
    }

    if modified_sections.is_empty() {
        return;
    }

    // Render and atomic-write
    let new_content = render_user_markdown(&doc);

    // Validate round-trip
    if parse_user_markdown(&new_content).is_err() {
        tracing::warn!("Extraction: rendered USER.md failed round-trip validation, aborting");
        return;
    }

    // Atomic write: temp file → rename
    let tmp_path = path.with_extension("md.tmp");
    if let Err(e) = std::fs::write(&tmp_path, &new_content) {
        tracing::warn!("Extraction: failed to write temp file: {e}");
        return;
    }
    if let Err(e) = std::fs::rename(&tmp_path, &path) {
        tracing::warn!("Extraction: failed to rename temp file: {e}");
        let _ = std::fs::remove_file(&tmp_path);
        return;
    }

    // Update in-memory document
    if let Ok(new_doc) = parse_user_markdown(&new_content) {
        match user_document.write() {
            Ok(mut guard) => {
                *guard = Some(new_doc);
            }
            Err(poisoned) => {
                tracing::warn!("User document lock poisoned during extraction update; recovering");
                let mut guard = poisoned.into_inner();
                *guard = Some(new_doc);
            }
        }
    }

    // Publish event
    let content_sha256 = {
        use sha2::Digest;
        let mut hasher = sha2::Sha256::new();
        hasher.update(new_content.as_bytes());
        format!("{:x}", hasher.finalize())
    };

    bus.publish(SystemEvent::UserProfileUpdated {
        actor: "extraction".to_string(),
        mode: "sections".to_string(),
        content_sha256,
        modified_sections: modified_sections.clone(),
        backup_path: None,
        timestamp: Utc::now(),
    });

    tracing::info!(
        "Extraction: updated USER.md sections: {:?}",
        modified_sections
    );
}
