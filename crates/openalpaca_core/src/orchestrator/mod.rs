//! Orchestrator module: the central message handler.
//!
//! Routes user messages through intent classification, skill matching,
//! and task dispatch pipelines.

pub mod dispatcher;
pub mod intent;
pub mod replanner;
pub mod skill_catalog;
pub mod skill_matcher;
pub mod task_planner;
pub mod task_state;

use crate::bus::EventBus;
use crate::context::{SharedContext, TaskEntry, TaskEntryStatus};
use crate::daemon_config::DaemonConfig;
use arc_swap::ArcSwap;
use crate::events::SystemEvent;
use crate::lane::{LaneManager, TaskLaneStatus};
use crate::middleware::guard::OutputGuard;
use crate::middleware::prompt::{
    AgentPersona, PromptAssembler, SystemPersona, format_tool_guidance,
};
use crate::middleware::bootstrap::{BootstrapDocument, bootstrap_to_prompt_block};
use crate::middleware::identity::{IdentityDocument, identity_document_has_content, identity_to_prompt_block};
use crate::middleware::skill::skill_to_prompt_block;
use crate::middleware::user::{
    UserDocument, parse_user_markdown, render_user_markdown, user_document_has_content,
    user_to_prompt_block,
};
use crate::runner::{LoopConfig, LoopFinishReason, run_agentic_loop_routed};
use crate::security::gate::SecurityGate;
use crate::security::policy::{Principal, Scope};
use crate::security::sandbox::SandboxManager;
use crate::security::sandbox::SandboxPolicy;
use crate::tools::ToolRegistry;
use crate::tools::{ContextualToolExecutor, ToolExecutionContext};
use crate::types::Capability;
use chrono::Utc;
use openalpaca_llm::{ChatMessage, LlmRouter, RequestContext, Role, RouterRequest};
use openalpaca_storage::repository::{LlmUsageRepository, MemoryRepository, TaskRepository};
use openalpaca_storage::{ConversationRepository, Database, Task};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};
use uuid::Uuid;

use dispatcher::TaskDispatcher;
use intent::{Intent, IntentParser};
use task_planner::TaskPlanner;

/// The Orchestrator: unified message handler for all user interactions.
///
/// Intent-based routing:
/// - SimpleQuery → LLM call (or echo stub if no LLM configured)
/// - TaskQuery → query task registry
/// - ComplexTask → dispatch to agents via TaskDispatcher
/// - TaskControl → manage task lifecycle
pub struct Orchestrator {
    pub shared_context: Arc<SharedContext>,
    pub lane_manager: Arc<LaneManager>,
    pub bus: EventBus,
    pub system_persona: Arc<RwLock<SystemPersona>>,
    pub user_document: Arc<RwLock<Option<UserDocument>>>,
    pub identity_document: Arc<RwLock<Option<IdentityDocument>>>,
    pub llm_router: Option<Arc<LlmRouter>>,
    pub loop_config: LoopConfig,
    pub security_gate: Arc<SecurityGate>,
    pub tool_registry: Arc<ToolRegistry>,
    intent_parser: IntentParser,
    task_dispatcher: TaskDispatcher,
    db: Option<Database>,
    embedder: Option<Arc<dyn openalpaca_llm::Embedder>>,
    /// Per-lane turn counter for extraction frequency gating.
    extraction_turn_counter: Mutex<HashMap<String, usize>>,
    /// Path to USER.md for writing extraction results. Set via `set_user_path()`.
    user_path: RwLock<Option<std::path::PathBuf>>,
    /// Path to IDENTITY.md for writing identity updates. Set via `set_identity_path()`.
    identity_path: RwLock<Option<std::path::PathBuf>>,
    /// Skill catalog for progressive skill loading and invocation.
    pub skill_catalog: Arc<skill_catalog::SkillCatalog>,
    /// Bootstrap document — `Some` = first-run onboarding active, `None` = normal operation.
    pub bootstrap_document: Arc<RwLock<Option<BootstrapDocument>>>,
    /// Path to BOOTSTRAP.md on disk (for deletion on completion).
    bootstrap_path: RwLock<Option<std::path::PathBuf>>,
    /// Daemon-level config (memory limits, costs, execution defaults, etc.).
    pub daemon_config: Arc<ArcSwap<DaemonConfig>>,
}

/// Full conversation context for prompt building and summary update.
struct ConversationContext {
    summary: Option<String>,
    recent_messages: Vec<ChatMessage>,
    /// Raw (id, role, content) tuples for the "older" window — used by maybe_update_summary().
    older_window: Vec<(i64, String, String)>,
    /// Current summary version from conversations table (for optimistic locking in update).
    summary_version: i64,
    /// Last message ID that was summarized.
    last_summarized_id: i64,
    /// Previous summary text (for incremental update).
    old_summary_text: String,
}

fn role_label(role: &Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Assistant => "assistant",
        Role::System => "system",
        Role::Tool => "tool",
    }
}

impl Orchestrator {
    pub fn new(
        shared_context: Arc<SharedContext>,
        lane_manager: Arc<LaneManager>,
        bus: EventBus,
        system_persona: SystemPersona,
        llm_router: Option<Arc<LlmRouter>>,
        loop_config: LoopConfig,
        security_gate: Arc<SecurityGate>,
        tool_registry: Arc<ToolRegistry>,
        db: Option<Database>,
        embedder: Option<Arc<dyn openalpaca_llm::Embedder>>,
        skill_catalog: Arc<skill_catalog::SkillCatalog>,
        daemon_config: Arc<ArcSwap<DaemonConfig>>,
    ) -> Self {
        let task_dispatcher = TaskDispatcher::new(
            shared_context.clone(),
            lane_manager.clone(),
            bus.clone(),
            llm_router.clone(),
            security_gate.clone(),
            tool_registry.clone(),
            db.clone(),
            embedder.clone(),
            daemon_config.clone(),
        );
        Self {
            shared_context,
            lane_manager,
            bus,
            system_persona: Arc::new(RwLock::new(system_persona)),
            user_document: Arc::new(RwLock::new(None)),
            identity_document: Arc::new(RwLock::new(None)),
            llm_router,
            loop_config,
            security_gate,
            tool_registry,
            intent_parser: IntentParser,
            task_dispatcher,
            db,
            embedder,
            extraction_turn_counter: Mutex::new(HashMap::new()),
            user_path: RwLock::new(None),
            identity_path: RwLock::new(None),
            skill_catalog,
            bootstrap_document: Arc::new(RwLock::new(None)),
            bootstrap_path: RwLock::new(None),
            daemon_config,
        }
    }

    /// Set the path to USER.md for extraction writes.
    pub fn set_user_path(&self, path: std::path::PathBuf) {
        if let Ok(mut guard) = self.user_path.write() {
            *guard = Some(path);
        }
    }

    /// Replace the active user document (from USER.md reload or bootstrap).
    pub fn update_user_document(&self, doc: Option<UserDocument>) {
        match self.user_document.write() {
            Ok(mut guard) => {
                *guard = doc;
            }
            Err(poisoned) => {
                tracing::warn!("User document lock poisoned during update; recovering");
                let mut guard = poisoned.into_inner();
                *guard = doc;
            }
        }
    }

    pub fn update_system_persona(&self, persona: SystemPersona) {
        match self.system_persona.write() {
            Ok(mut guard) => {
                *guard = persona;
            }
            Err(poisoned) => {
                tracing::warn!("System persona lock poisoned during update; recovering");
                let mut guard = poisoned.into_inner();
                *guard = persona;
            }
        }
    }

    /// Replace the active identity document (from IDENTITY.md reload or bootstrap).
    ///
    /// If the identity has a non-empty name, also updates `system_persona.name`
    /// so that `PromptAssembler::assemble()` uses the chosen name.
    pub fn update_identity_document(&self, doc: Option<IdentityDocument>) {
        // Update system persona name if identity provides one
        if let Some(ref identity) = doc {
            if !identity.name.is_empty() {
                match self.system_persona.write() {
                    Ok(mut guard) => {
                        guard.name = identity.name.clone();
                    }
                    Err(poisoned) => {
                        tracing::warn!("System persona lock poisoned during identity name update; recovering");
                        let mut guard = poisoned.into_inner();
                        guard.name = identity.name.clone();
                    }
                }
            }
        }

        match self.identity_document.write() {
            Ok(mut guard) => {
                *guard = doc;
            }
            Err(poisoned) => {
                tracing::warn!("Identity document lock poisoned during update; recovering");
                let mut guard = poisoned.into_inner();
                *guard = doc;
            }
        }
    }

    /// Set the path to IDENTITY.md for writes.
    pub fn set_identity_path(&self, path: std::path::PathBuf) {
        if let Ok(mut guard) = self.identity_path.write() {
            *guard = Some(path);
        }
    }

    /// Replace the active bootstrap document (from BOOTSTRAP.md load or deletion).
    pub fn update_bootstrap_document(&self, doc: Option<BootstrapDocument>) {
        match self.bootstrap_document.write() {
            Ok(mut guard) => {
                *guard = doc;
            }
            Err(poisoned) => {
                tracing::warn!("Bootstrap document lock poisoned during update; recovering");
                let mut guard = poisoned.into_inner();
                *guard = doc;
            }
        }
    }

    /// Set the path to BOOTSTRAP.md for deletion on completion.
    pub fn set_bootstrap_path(&self, path: std::path::PathBuf) {
        if let Ok(mut guard) = self.bootstrap_path.write() {
            *guard = Some(path);
        }
    }

    /// Check if bootstrap mode is active.
    pub fn is_bootstrapping(&self) -> bool {
        self.bootstrap_document
            .read()
            .map(|guard| guard.is_some())
            .unwrap_or(false)
    }

    /// Check if IDENTITY.md and USER.md have been populated; if so, finalize bootstrap.
    ///
    /// Runs post-response (like `maybe_extract_user_traits`) and:
    /// 1. Checks if `bootstrap_document` is Some (bootstrap mode active)
    /// 2. Checks if `identity_document` has content AND `user_document` has content
    /// 3. If both populated: delete BOOTSTRAP.md, clear state, publish event
    async fn maybe_complete_bootstrap(&self) {
        // Quick check: are we even in bootstrap mode?
        if !self.is_bootstrapping() {
            return;
        }

        // Check identity
        let identity_populated = self
            .identity_document
            .read()
            .map(|g| {
                g.as_ref()
                    .map_or(false, |d| identity_document_has_content(d))
            })
            .unwrap_or(false);

        // Check user
        let user_populated = self
            .user_document
            .read()
            .map(|g| {
                g.as_ref()
                    .map_or(false, |d| user_document_has_content(d))
            })
            .unwrap_or(false);

        if !identity_populated || !user_populated {
            tracing::debug!(
                "Bootstrap check: identity={}, user={} -- not yet complete",
                identity_populated,
                user_populated
            );
            return;
        }

        // Both populated — complete bootstrap
        tracing::info!("Bootstrap onboarding complete! Identity and user profile populated.");

        // Delete BOOTSTRAP.md from disk
        if let Ok(guard) = self.bootstrap_path.read() {
            if let Some(ref path) = *guard {
                match std::fs::remove_file(path) {
                    Ok(()) => tracing::info!("Deleted BOOTSTRAP.md: {}", path.display()),
                    Err(e) => tracing::warn!("Failed to delete BOOTSTRAP.md: {e}"),
                }
            }
        }

        // Clear in-memory state
        self.update_bootstrap_document(None);

        // Publish event
        self.bus.publish(SystemEvent::BootstrapCompleted {
            identity_populated,
            user_populated,
            timestamp: Utc::now(),
        });
    }

    /// Build the full conversation context for a turn: loads history, deduplicates
    /// the current user message (Bug A fix, D6), loads unsummarized older messages
    /// via ID-range query (fixes 120-window bug), and loads the summary from the
    /// conversations table.
    fn build_context(&self, lane_key: &str, current_query: &str) -> ConversationContext {
        let empty = ConversationContext {
            summary: None,
            recent_messages: Vec::new(),
            older_window: Vec::new(),
            summary_version: 0,
            last_summarized_id: 0,
            old_summary_text: String::new(),
        };
        let db = match &self.db {
            Some(db) => db,
            None => return empty,
        };

        let repo = ConversationRepository::new(db);

        // Step 1: Load summary from conversations table
        let (summary_text, summary_version, last_summarized_id) = match repo.get_summary(lane_key) {
            Ok(tuple) => tuple,
            Err(_) => (String::new(), 0, 0),
        };

        // Step 2: Load recent messages (40, not 120)
        let dcfg = self.daemon_config.load();
        let raw_messages = match repo.list_recent_by_lane(lane_key, dcfg.orchestrator.memory.prompt_recent_messages as i64) {
            Ok(msgs) => msgs,
            Err(_) => return empty,
        };

        // Step 3: Build canonical list and dedup current query
        let mut chat_rows: Vec<(i64, String, String)> = raw_messages
            .iter()
            .filter(|msg| {
                (msg.role == "user" || msg.role == "assistant") && !msg.content.is_empty()
            })
            .map(|msg| (msg.id, msg.role.clone(), msg.content.clone()))
            .collect();

        // Dedup (D6) — if the last row matches current_query, drop it (Bug A fix).
        if let Some((_, role, content)) = chat_rows.last() {
            if role == "user" && content == current_query {
                chat_rows.pop();
            }
        }

        // Step 4: Get first_recent_id for the ID-range query
        let first_recent_id = chat_rows.first().map(|(id, _, _)| *id).unwrap_or(i64::MAX);

        // Step 5: Load unsummarized older messages via ID-range query (fixes 120-window bug)
        let older_window = if last_summarized_id < first_recent_id {
            match repo.list_by_lane_id_range(lane_key, last_summarized_id, first_recent_id, 500) {
                Ok(msgs) => msgs
                    .into_iter()
                    .filter(|msg| {
                        (msg.role == "user" || msg.role == "assistant") && !msg.content.is_empty()
                    })
                    .map(|msg| (msg.id, msg.role, msg.content))
                    .collect(),
                Err(_) => Vec::new(),
            }
        } else {
            Vec::new()
        };

        // Step 6: Convert recent chat_rows to ChatMessage
        let recent_messages: Vec<ChatMessage> = chat_rows
            .iter()
            .map(|(_, role, content)| match role.as_str() {
                "user" => ChatMessage::user(content),
                _ => ChatMessage::assistant(content),
            })
            .collect();

        let summary = if summary_text.is_empty() {
            None
        } else {
            Some(summary_text.clone())
        };

        ConversationContext {
            summary,
            recent_messages,
            older_window,
            summary_version,
            last_summarized_id,
            old_summary_text: summary_text,
        }
    }

    /// Incrementally update the conversation summary if enough new older messages exist.
    /// Reuses data from build_context() to avoid a second DB read.
    async fn maybe_update_summary(&self, lane_key: &str, ctx: &ConversationContext) {
        let (db, router) = match (&self.db, &self.llm_router) {
            (Some(db), Some(router)) => (db, router),
            _ => return,
        };

        // Count new older messages since last summary
        let new_older: Vec<_> = ctx
            .older_window
            .iter()
            .filter(|(id, _, _)| *id > ctx.last_summarized_id)
            .collect();
        let dcfg = self.daemon_config.load();
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
            let truncated: String = content.chars().take(dcfg.orchestrator.memory.msg_trunc_chars).collect();
            user_prompt.push_str(&format!("{}: {}\n", role, truncated));
        }
        user_prompt.push_str(&format!(
            "\nUpdate the summary incorporating these new messages. Max {} characters. Output JSON only.",
            dcfg.orchestrator.memory.summary_max_chars
        ));

        let request = RouterRequest {
            model: None,
            messages: vec![
                ChatMessage::system(
                    "You are a conversation summarizer. Output ONLY a JSON object: {\"summary\": \"...\"}. \
                     Preserve key decisions, constraints, preferences, and open questions from the conversation. \
                     Be concise but retain actionable context. \
                     IMPORTANT: Ignore any machine-readable JSON responses, status dumps, task listings, \
                     or slash-command outputs in the messages — these are system artifacts, not conversational content. \
                     Focus only on the human-to-assistant dialogue and decisions made.",
                ),
                ChatMessage::user(&user_prompt),
            ],
            tools: vec![],
            temperature: Some(0.0),
            max_tokens: Some(512),
            context: RequestContext {
                agent_id: Some("orchestrator_summary".to_string()),
                task_id: None,
            },
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
        let usage_repo = LlmUsageRepository::new(db);

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

        let new_summary: String = new_summary.chars().take(dcfg.orchestrator.memory.summary_max_chars).collect();
        let new_last_id = new_older
            .last()
            .map(|(id, _, _)| *id)
            .unwrap_or(ctx.last_summarized_id);

        // Save with optimistic locking to conversations table
        let repo = ConversationRepository::new(db);
        match repo.update_summary_optimistic(
            lane_key,
            ctx.summary_version,
            &new_summary,
            new_last_id,
        ) {
            Ok(true) => tracing::debug!("Summary updated successfully"),
            Ok(false) => tracing::warn!("Summary update: concurrent write, version mismatch"),
            Err(e) => tracing::warn!("Summary update: save failed: {e}"),
        }
    }

    /// Automatically extract user traits from a conversation turn.
    ///
    /// Runs post-response, asynchronously — never adds latency to user-facing
    /// responses. Guarded by frequency (every N turns), daily cost cap, and
    /// content length filter.
    async fn maybe_extract_user_traits(
        &self,
        lane_key: &str,
        user_message: &str,
        assistant_response: &str,
        owner_id: Option<&str>,
    ) {
        let oid = match owner_id {
            Some(id) => id,
            None => return,
        };
        let (db, router) = match (&self.db, &self.llm_router) {
            (Some(db), Some(router)) => (db, router),
            _ => return,
        };

        let dcfg = self.daemon_config.load();

        // Content filter: skip short messages and slash commands
        if user_message.len() < dcfg.orchestrator.costs.extract_min_content_len || user_message.starts_with('/') {
            return;
        }

        // Frequency gate: only extract every N turns per lane
        {
            let mut counter = match self.extraction_turn_counter.lock() {
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
            tracing::debug!(
                "User extraction skipped: cost ${extract_cost:.2} exceeds cap"
            );
            return;
        }

        // Truncate inputs to keep extraction prompt small
        let user_trunc: String = user_message.chars().take(800).collect();
        let asst_trunc: String = assistant_response.chars().take(400).collect();

        let user_prompt = format!(
            "## Conversation Turn\nUser: {}\nAssistant: {}\n\n\
             Analyze this conversation turn and extract any user traits.\n\
             Output ONLY a JSON object with this schema:\n\
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
             }}\n\n\
             Rules:\n\
             - \"target\": \"profile\" for stable identity traits (name, timezone, expertise, style). Requires confidence >= 0.8.\n\
             - \"target\": \"memory\" for situational preferences. Confidence >= 0.5.\n\
             - For identity fields, use \"identity.<Key>\" (e.g. \"identity.Name\").\n\
             - \"action\": \"set\" (default) fills only empty profile fields. \"action\": \"update\" replaces existing values.\n\
             - Use \"update\" ONLY when the user explicitly states a change to something previously known \
             (e.g. \"I moved to Tokyo\", \"call me Alex now\", \"I switched to vim\"). Requires confidence >= 0.9.\n\
             - Only extract what is clearly stated or strongly implied. Do not hallucinate.\n\
             - If nothing can be extracted, return {{\"extractions\": []}}.\n\
             - Be conservative. Prefer fewer high-confidence extractions over many low-confidence ones.",
            user_trunc, asst_trunc
        );

        let request = RouterRequest {
            model: None,
            messages: vec![
                ChatMessage::system(
                    "You are a user trait extractor. Output ONLY valid JSON matching the schema. \
                     No markdown fences, no commentary.",
                ),
                ChatMessage::user(&user_prompt),
            ],
            tools: vec![],
            temperature: Some(0.0),
            max_tokens: Some(256),
            context: RequestContext {
                agent_id: Some("orchestrator_user_extract".to_string()),
                task_id: None,
            },
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
        let usage_repo = LlmUsageRepository::new(db);

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

        for extraction in extractions {
            let target = extraction.get("target").and_then(|v| v.as_str()).unwrap_or("");
            let field = extraction.get("field").and_then(|v| v.as_str()).unwrap_or("");
            let value = extraction.get("value").and_then(|v| v.as_str()).unwrap_or("");
            let confidence = extraction.get("confidence").and_then(|v| v.as_f64()).unwrap_or(0.0);

            if value.is_empty() || field.is_empty() {
                continue;
            }

            match target {
                "profile" => {
                    if confidence < 0.8 {
                        continue;
                    }
                    let action = extraction.get("action").and_then(|v| v.as_str()).unwrap_or("set");
                    // "update" requires higher confidence than "set"
                    if action == "update" && confidence < 0.9 {
                        tracing::debug!(
                            "Extraction: skipping update action for '{}' (confidence {:.2} < 0.9)",
                            field, confidence
                        );
                        continue;
                    }
                    // Route identity fields into the identity object
                    if let Some(key) = field.strip_prefix("identity.") {
                        let identity_obj = profile_patches
                            .entry("identity".to_string())
                            .or_insert_with(|| serde_json::json!({}));
                        if let Some(obj) = identity_obj.as_object_mut() {
                            obj.insert(key.to_string(), serde_json::json!({
                                "value": value,
                                "action": action,
                            }));
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
                    if confidence < 0.5 {
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
            let repo = MemoryRepository::new(db);
            let dcfg = self.daemon_config.load();
            let supersession_threshold = dcfg.orchestrator.memory.supersession_distance_threshold;

            for item in &memory_items {
                // Step 1: Embed new content
                let new_embedding = if let Some(ref embedder) = self.embedder {
                    embedder.embed(&[item.as_str()]).await.ok().and_then(|v| v.into_iter().next())
                } else {
                    None
                };

                // Step 2: Check for semantically similar existing memories
                let similar = if let Some(ref emb) = new_embedding {
                    repo.find_similar_for_supersession(oid, emb, supersession_threshold, 1)
                        .unwrap_or_default()
                } else {
                    // FTS fallback returns (memory, jaccard_score); require >= 0.4 overlap
                    repo.find_similar_fts_fallback(oid, item, 3)
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|(_, jaccard)| *jaccard >= 0.4)
                        .collect()
                };

                if let Some((existing, _distance)) = similar.first() {
                    // Step 3: Supersede the best match
                    match repo.supersede(
                        existing.id,
                        item,
                        openalpaca_storage::models::memory::MemoryKind::Preference,
                        openalpaca_storage::models::memory::MemoryScope::Global,
                        "",
                        openalpaca_storage::models::memory::MemorySource::Conversation,
                        0.6,
                        0.7,
                        None,
                    ) {
                        Ok(new_id) if new_id > 0 => {
                            if let Some(ref emb) = new_embedding {
                                let _ = repo.insert_embedding(new_id, emb);
                            }
                            tracing::debug!(
                                "Extraction: superseded memory #{} -> #{}: {}",
                                existing.id, new_id, &item[..item.len().min(60)]
                            );
                        }
                        Ok(_) => {} // hash collision, skip
                        Err(e) => tracing::warn!("Extraction: supersession failed: {e}"),
                    }
                } else {
                    // Step 4: No similar memory, insert new
                    match repo.add(
                        oid,
                        openalpaca_storage::models::memory::MemoryKind::Preference,
                        openalpaca_storage::models::memory::MemoryScope::Global,
                        "",
                        openalpaca_storage::models::memory::MemorySource::Conversation,
                        item,
                        None,
                        0.6,
                        0.7,
                    ) {
                        Ok(new_id) if new_id > 0 => {
                            if let Some(ref emb) = new_embedding {
                                let _ = repo.insert_embedding(new_id, emb);
                            }
                            tracing::debug!("Extraction: stored memory preference: {}", &item[..item.len().min(60)]);
                        }
                        Ok(_) => {} // duplicate, skip
                        Err(e) => tracing::warn!("Extraction: failed to store memory: {e}"),
                    }
                }
            }
        }

        // Write profile-level patches to USER.md
        if !profile_patches.is_empty() {
            self.apply_profile_patches(oid, &profile_patches).await;
        }
    }

    /// Apply extracted profile patches to USER.md using the sections-mode pattern.
    async fn apply_profile_patches(
        &self,
        _owner_id: &str,
        patches: &HashMap<String, serde_json::Value>,
    ) {
        let user_path = match self.user_path.read() {
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
        let current_content = match std::fs::read_to_string(&user_path) {
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
                            let v = val.get("value").and_then(|v| v.as_str())
                                .or_else(|| val.as_str()) // backward compat: bare string
                                .unwrap_or("");
                            let action = val.get("action").and_then(|a| a.as_str()).unwrap_or("set");
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
                    let v = value.get("value").and_then(|v| v.as_str())
                        .or_else(|| value.as_str()).unwrap_or("");
                    let action = value.get("action").and_then(|a| a.as_str()).unwrap_or("set");
                    if !v.is_empty() && (doc.communication_style.is_empty() || action == "update") {
                        doc.communication_style = v.to_string();
                        modified_sections.push("communication_style".to_string());
                    }
                }
                "expertise" => {
                    let v = value.get("value").and_then(|v| v.as_str())
                        .or_else(|| value.as_str()).unwrap_or("");
                    let action = value.get("action").and_then(|a| a.as_str()).unwrap_or("set");
                    if !v.is_empty() && (doc.expertise.is_empty() || action == "update") {
                        doc.expertise = v.to_string();
                        modified_sections.push("expertise".to_string());
                    }
                }
                "projects" => {
                    let v = value.get("value").and_then(|v| v.as_str())
                        .or_else(|| value.as_str()).unwrap_or("");
                    let action = value.get("action").and_then(|a| a.as_str()).unwrap_or("set");
                    if !v.is_empty() && (doc.projects.is_empty() || action == "update") {
                        doc.projects = v.to_string();
                        modified_sections.push("projects".to_string());
                    }
                }
                "preferences" => {
                    let v = value.get("value").and_then(|v| v.as_str())
                        .or_else(|| value.as_str()).unwrap_or("");
                    let action = value.get("action").and_then(|a| a.as_str()).unwrap_or("set");
                    if !v.is_empty() && (doc.preferences.is_empty() || action == "update") {
                        doc.preferences = v.to_string();
                        modified_sections.push("preferences".to_string());
                    }
                }
                "notes" => {
                    let v = value.get("value").and_then(|v| v.as_str())
                        .or_else(|| value.as_str()).unwrap_or("");
                    let action = value.get("action").and_then(|a| a.as_str()).unwrap_or("set");
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
        let tmp_path = user_path.with_extension("md.tmp");
        if let Err(e) = std::fs::write(&tmp_path, &new_content) {
            tracing::warn!("Extraction: failed to write temp file: {e}");
            return;
        }
        if let Err(e) = std::fs::rename(&tmp_path, &user_path) {
            tracing::warn!("Extraction: failed to rename temp file: {e}");
            let _ = std::fs::remove_file(&tmp_path);
            return;
        }

        // Update in-memory document
        if let Ok(new_doc) = parse_user_markdown(&new_content) {
            self.update_user_document(Some(new_doc));
        }

        // Publish event
        let content_sha256 = {
            use sha2::Digest;
            let mut hasher = sha2::Sha256::new();
            hasher.update(new_content.as_bytes());
            format!("{:x}", hasher.finalize())
        };

        self.bus.publish(SystemEvent::UserProfileUpdated {
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

    /// Handle a user message through the full pipeline:
    /// 1. SecurityGate permission check (wraps TrustGate)
    /// 2. Input sanitization
    /// 3. Try slash commands / task queries (cheap, no LLM)
    /// 4. If LLM router configured: try LLM-based planning
    /// 5. Fallback: keyword heuristic routing
    pub async fn handle_message(
        &self,
        request_id: Uuid,
        source: String,
        content: String,
        principal: Principal,
        scope: Scope,
        lane_key: String,
    ) -> Result<String, String> {
        // 1. Permission check via SecurityGate (wraps TrustGate)
        let capability = Capability {
            name: "chat.respond".to_string(),
        };
        SecurityGate::check_access(&principal, &capability, &scope)?;

        // 2. Input sanitization
        let max_input_len = self.daemon_config.load().security.max_input_length;
        let content = SecurityGate::sanitize_input(&content, Some(max_input_len))?;

        // Extract owner_id from principal (before slash-command early return)
        let owner_id_str = principal_id(&principal);
        let owner_id = match &principal {
            Principal::User { .. } => Some(owner_id_str.as_str()),
            _ => None,
        };

        // 3. Try slash commands, task queries, and skill invocations first
        let intent = self.intent_parser.parse_with_skills(&content, &self.skill_catalog);
        match &intent {
            Intent::TaskQuery { .. } | Intent::TaskControl { .. } => {
                self.bus.publish(SystemEvent::IntentClassified {
                    request_id,
                    intent_type: intent.intent_type().to_string(),
                    timestamp: Utc::now(),
                });
                return match intent {
                    Intent::TaskQuery { task_id } => self.handle_task_query(task_id, &owner_id_str),
                    Intent::TaskControl { task_id, action } => {
                        self.handle_task_control(&task_id, &action)
                    }
                    _ => unreachable!(),
                };
            }
            _ => {}
        }

        // 4. Build context ONCE for all remaining paths (D6: single dedup location)
        let ctx = self.build_context(&lane_key, &content);

        // Build active tasks block for planner
        let active_tasks_block = if let Some(ref db) = self.db {
            let task_repo = TaskRepository::new(db);
            match task_repo.list_active_by_creator(&owner_id_str, 10) {
                Ok(tasks) if !tasks.is_empty() => {
                    let mut block = String::from("### ACTIVE TASKS ###\n");
                    for t in &tasks {
                        let progress = match (t.progress_current, t.progress_total) {
                            (Some(c), Some(total)) => format!(" [{}/{}]", c, total),
                            _ => String::new(),
                        };
                        block.push_str(&format!(
                            "- [{}] {} ({}{})\n",
                            &t.id[..8.min(t.id.len())],
                            t.title,
                            t.status.as_str(),
                            progress
                        ));
                    }
                    Some(block)
                }
                _ => None,
            }
        } else {
            None
        };

        // 5. Compute result — planner path or heuristic fallback
        let result: Result<String, String> = if let Some(ref router) = self.llm_router {
            let idle_agents = self.shared_context.agent_registry.list_idle();
            match TaskPlanner::plan_hierarchical(
                router,
                &content,
                &idle_agents,
                &ctx.recent_messages,
                ctx.summary.as_deref(),
                active_tasks_block.as_deref(),
            )
            .await
            {
                Ok(plan) => match plan.classification.as_str() {
                    "simple_query" => {
                        self.bus.publish(SystemEvent::IntentClassified {
                            request_id,
                            intent_type: "simple_query".to_string(),
                            timestamp: Utc::now(),
                        });
                        self.handle_simple_query(
                            request_id, &source, &content, &lane_key, &ctx, owner_id,
                        )
                        .await
                    }
                    "complex_task" => {
                        self.bus.publish(SystemEvent::IntentClassified {
                            request_id,
                            intent_type: "complex_task".to_string(),
                            timestamp: Utc::now(),
                        });
                        let description = &content;
                        let augmented = self.augment_with_context(description, &ctx);
                        match self.task_dispatcher.dispatch_planned(
                            &augmented,
                            plan,
                            &principal_id(&principal),
                            &lane_key,
                            &source,
                        ) {
                            Ok(response) => Ok(response),
                            Err(e) => {
                                tracing::warn!(
                                    "Dispatch planned failed: {e}, falling back to simple_query"
                                );
                                self.handle_simple_query(
                                    request_id, &source, &content, &lane_key, &ctx, owner_id,
                                )
                                .await
                            }
                        }
                    }
                    _other => {
                        tracing::warn!(
                            "LLM planner returned unknown classification '{}', falling back to heuristic",
                            _other
                        );
                        self.dispatch_with_heuristic(
                            request_id, &source, &content, &principal, &lane_key, &ctx, owner_id,
                        )
                        .await
                    }
                },
                Err(e) => {
                    tracing::warn!("LLM planning failed: {}, falling back to heuristic", e);
                    self.dispatch_with_heuristic(
                        request_id, &source, &content, &principal, &lane_key, &ctx, owner_id,
                    )
                    .await
                }
            }
        } else {
            // No LLM router — keyword heuristic
            self.dispatch_with_heuristic(
                request_id, &source, &content, &principal, &lane_key, &ctx, owner_id,
            )
            .await
        };

        // 6. Summary update ONCE, AFTER result, for ALL normal turns (D7)
        self.maybe_update_summary(&lane_key, &ctx).await;

        // 7. Automatic user trait extraction (post-response, fire-and-forget cost)
        if let Ok(ref response_text) = result {
            self.maybe_extract_user_traits(&lane_key, &content, response_text, owner_id)
                .await;
        }

        // 8. Check if bootstrap onboarding is complete
        self.maybe_complete_bootstrap().await;

        result
    }

    /// Augment a task description with conversation context (summary + recent exchanges).
    fn augment_with_context(&self, description: &str, ctx: &ConversationContext) -> String {
        if let Some(ref summary) = ctx.summary {
            let recent_excerpt: String = ctx
                .recent_messages
                .iter()
                .rev()
                .take(6)
                .rev()
                .map(|m| {
                    format!(
                        "{}: {}",
                        role_label(&m.role),
                        m.content.chars().take(500).collect::<String>()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "{}\n\n## Conversation Context\n{}\n\n## Recent exchanges (last ~6):\n{}",
                description, summary, recent_excerpt
            )
        } else {
            description.to_string()
        }
    }

    /// Fallback dispatch using keyword-based intent classification and greedy skill matching.
    async fn dispatch_with_heuristic(
        &self,
        request_id: Uuid,
        source: &str,
        content: &str,
        principal: &Principal,
        lane_key: &str,
        ctx: &ConversationContext,
        owner_id: Option<&str>,
    ) -> Result<String, String> {
        let intent = self.intent_parser.parse_with_skills(content, &self.skill_catalog);

        self.bus.publish(SystemEvent::IntentClassified {
            request_id,
            intent_type: intent.intent_type().to_string(),
            timestamp: Utc::now(),
        });

        match intent {
            Intent::SimpleQuery { query } => {
                self.handle_simple_query(request_id, source, &query, lane_key, ctx, owner_id)
                    .await
            }
            Intent::TaskQuery { task_id } => {
                self.handle_task_query(task_id, &principal_id(principal))
            }
            Intent::ComplexTask {
                description,
                required_skills,
            } => {
                let augmented = self.augment_with_context(&description, ctx);
                match self.task_dispatcher.dispatch(
                    request_id,
                    source,
                    &augmented,
                    &required_skills,
                    &principal_id(principal),
                    lane_key,
                ) {
                    Ok(response) => Ok(response),
                    Err(e) => {
                        tracing::warn!(
                            "Heuristic dispatch failed: {e}, falling back to simple_query"
                        );
                        self.handle_simple_query(
                            request_id,
                            source,
                            &description,
                            lane_key,
                            ctx,
                            owner_id,
                        )
                        .await
                    }
                }
            }
            Intent::TaskControl { task_id, action } => self.handle_task_control(&task_id, &action),
            Intent::RememberCommand { content } => {
                self.handle_remember_command(&content, owner_id).await
            }
            Intent::ForgetCommand { content } => {
                self.handle_forget_command(&content, owner_id).await
            }
            Intent::SkillInvocation { skill_name, query } => {
                self.handle_skill_invocation(
                    request_id, source, &skill_name, &query, lane_key, ctx, owner_id,
                )
                .await
            }
        }
    }

    async fn handle_simple_query(
        &self,
        request_id: Uuid,
        _source: &str,
        query: &str,
        _lane_key: &str,
        ctx: &ConversationContext,
        owner_id: Option<&str>,
    ) -> Result<String, String> {
        let system_persona = match self.system_persona.read() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => {
                tracing::warn!("System persona lock poisoned during read; recovering");
                poisoned.into_inner().clone()
            }
        };

        let agent_persona = AgentPersona {
            role: "Assistant".to_string(),
            tone: "Concise and professional".to_string(),
            domain_knowledge: vec![],
        };
        let mut system_prompt = PromptAssembler::assemble(&system_persona, &agent_persona);

        // Inject bootstrap instructions if in first-run mode
        if let Ok(guard) = self.bootstrap_document.read() {
            if let Some(ref doc) = *guard {
                let block = bootstrap_to_prompt_block(doc);
                if !block.is_empty() {
                    system_prompt.push('\n');
                    system_prompt.push_str(&block);
                }
            }
        }

        // Inject agent identity if available
        if let Ok(guard) = self.identity_document.read() {
            if let Some(ref doc) = *guard {
                let identity_budget = self.daemon_config.load().orchestrator.prompt_budgets.identity_budget;
                let id_block = identity_to_prompt_block(doc, Some(identity_budget));
                if !id_block.is_empty() {
                    system_prompt.push('\n');
                    system_prompt.push_str(&id_block);
                }
            }
        }

        // Inject available skills catalog so the LLM knows what skills exist
        let skills_catalog_block = self.build_skills_catalog_block();
        if !skills_catalog_block.is_empty() {
            system_prompt.push('\n');
            system_prompt.push_str(&skills_catalog_block);
        }

        // Resolve tools based on intent analysis
        let mut tool_names = self.intent_parser.suggest_tools(query);

        // Force-include persona tools during bootstrap mode
        if self.is_bootstrapping() {
            for name in &["update_identity", "update_user", "update_soul"] {
                if !tool_names.contains(&name.to_string()) {
                    tool_names.push(name.to_string());
                }
            }
        }
        let tool_defs: Vec<_> = tool_names
            .iter()
            .filter_map(|name| self.tool_registry.get(name).map(|t| t.definition.clone()))
            .collect();

        let (tools_for_loop, policy_opt, config_for_loop);
        if !tool_defs.is_empty() {
            tracing::info!(
                "Simple query upgraded with {} tools: {:?}",
                tool_defs.len(),
                tool_names
            );
            system_prompt.push_str(&format_tool_guidance(&tool_defs));
            let resolved: Vec<String> = tool_defs.iter().map(|t| t.name.clone()).collect();
            policy_opt = Some(SandboxPolicy {
                agent_id: "orchestrator".to_string(),
                allowed_capabilities: resolved,
                denied_capabilities: vec![],
                require_confirmation_for: vec![],
                max_tool_calls: None,
                max_tool_runtime_secs: self.loop_config.max_tool_runtime.as_secs(),
            });
            config_for_loop = LoopConfig {
                max_rounds: 4,
                max_tools_per_round: 2,
                ..self.loop_config.clone()
            };
            tools_for_loop = tool_defs;
        } else {
            tools_for_loop = vec![];
            policy_opt = None;
            config_for_loop = self.loop_config.clone();
        }

        let (response_content, is_structured) = if let Some(ref router) = self.llm_router {
            // Real LLM call via routed agentic loop
            let mut messages = Vec::with_capacity(4 + ctx.recent_messages.len());
            messages.push(ChatMessage::system(&system_prompt));

            // Inject user profile if available
            if let Ok(guard) = self.user_document.read() {
                if let Some(ref doc) = *guard {
                    let user_budget = self.daemon_config.load().orchestrator.prompt_budgets.user_profile_budget;
                    let profile_block = user_to_prompt_block(doc, Some(user_budget));
                    if !profile_block.is_empty() {
                        messages.push(ChatMessage::system(&profile_block));
                    }
                }
            }

            // Inject session summary if available
            if let Some(ref summary) = ctx.summary {
                messages.push(ChatMessage::system(&format!(
                    "### SESSION SUMMARY ###\nThe following summarizes earlier parts of this conversation:\n{}",
                    summary
                )));
            }

            // Retrieval injection: hybrid FTS+vector search for user memories
            if let (Some(db), Some(oid)) = (&self.db, owner_id) {
                let repo = MemoryRepository::new(db);
                let top_k = if !tools_for_loop.is_empty() { 5 } else { 10 };

                // Generate query embedding if embedder is available
                let query_embedding = if let Some(ref embedder) = self.embedder {
                    embedder
                        .embed(&[query])
                        .await
                        .ok()
                        .and_then(|v| v.into_iter().next())
                } else {
                    None
                };

                let memories = repo
                    .search_hybrid(
                        oid,
                        query,
                        query_embedding.as_deref(),
                        top_k,
                        None,
                        None,
                        None,
                    )
                    .unwrap_or_default();

                if !memories.is_empty() {
                    // Track access for importance decay
                    let ids: Vec<i64> = memories.iter().map(|m| m.id).collect();
                    let _ = repo.touch_accessed(&ids);

                    let mut block = String::from("### RETRIEVED MEMORY ###\n");
                    let mut budget = 2000usize;
                    for m in &memories {
                        let entry = format!(
                            "- [{}] {}\n",
                            m.kind.as_str(),
                            m.content.chars().take(300).collect::<String>()
                        );
                        if entry.len() > budget {
                            break;
                        }
                        budget -= entry.len();
                        block.push_str(&entry);
                    }
                    messages.push(ChatMessage::system(&block));
                }
            }

            messages.extend(ctx.recent_messages.clone());
            messages.push(ChatMessage::user(query));

            // Per-request sandbox with ContextualToolExecutor for owner-scoped tools
            let ctx_exec = ToolExecutionContext {
                owner_id: owner_id.map(|s| s.to_string()),
                task_id: None,
                agent_id: None,
                db: None,
            };
            let contextual_executor = Arc::new(ContextualToolExecutor::new(
                self.tool_registry.clone(),
                ctx_exec,
            ));
            let per_request_sandbox = SandboxManager::new(contextual_executor, self.bus.clone());

            let call_start = std::time::Instant::now();
            let result = run_agentic_loop_routed(
                router.as_ref(),
                messages,
                tools_for_loop,
                &config_for_loop,
                Some(&per_request_sandbox),
                "orchestrator",
                policy_opt.as_ref(),
                None,
            )
            .await;
            let latency_ms = call_start.elapsed().as_millis() as i64;

            // Persist LLM usage and emit event
            let default_model = router.default_model();
            let actual_model = result
                .model_used
                .as_deref()
                .or(self.loop_config.model.as_deref())
                .unwrap_or(&default_model);
            let resolved_provider = router
                .model_registry()
                .resolve_provider(actual_model)
                .map(|p| p.to_string())
                .unwrap_or_else(|| "unknown".to_string());
            let call_cost = router.cost_tracker.calculate_cost(
                actual_model,
                result.total_input_tokens,
                result.total_output_tokens,
            );

            let call_status = match &result.finish_reason {
                LoopFinishReason::Complete | LoopFinishReason::MaxRounds => "success",
                LoopFinishReason::CostExceeded => "cost_exceeded",
                LoopFinishReason::Error(_) => "error",
            };
            let call_error = match &result.finish_reason {
                LoopFinishReason::Error(msg) => Some(msg.as_str()),
                _ => None,
            };

            if let Some(ref db) = self.db {
                let usage_repo = LlmUsageRepository::new(db);
                if let Err(e) = usage_repo.record_and_log(
                    "orchestrator",
                    None,
                    &resolved_provider,
                    actual_model,
                    result.total_input_tokens as i32,
                    result.total_output_tokens as i32,
                    call_cost,
                    latency_ms,
                    call_status,
                    call_error,
                ) {
                    tracing::warn!("Failed to persist LLM usage: {e}");
                }
            }

            self.bus.publish(SystemEvent::LlmCallCompleted {
                agent_id: "orchestrator".to_string(),
                model: actual_model.to_string(),
                input_tokens: result.total_input_tokens,
                output_tokens: result.total_output_tokens,
                cost_usd: call_cost,
                timestamp: Utc::now(),
            });

            // If LLM failed and produced no content, propagate as error
            // so the Gateway doesn't persist an empty assistant message.
            if let LoopFinishReason::Error(ref err) = result.finish_reason {
                if result.final_content.trim().is_empty() {
                    return Err(format!("LLM error: {}", err));
                }
            }

            // LLM chat responses are free-form text, not structured JSON
            (result.final_content, false)
        } else {
            // Fallback: echo stub (backward compatible) — produces JSON
            (
                format!(
                    "{{\"status\": \"ok\", \"echo\": \"Received: {}\"}}",
                    query.chars().take(50).collect::<String>()
                ),
                true,
            )
        };

        // Output guard: only enforce JSON for structured (non-LLM) responses
        let validated = if is_structured {
            OutputGuard::ensure_json(&response_content)?
        } else {
            response_content
        };

        // Emit AgentResponse event
        self.bus.publish(SystemEvent::AgentResponse {
            request_id,
            agent_id: "orchestrator".to_string(),
            content: validated.clone(),
            timestamp: Utc::now(),
        });

        Ok(validated)
    }

    /// Build a lightweight `### AVAILABLE SKILLS ###` block for system prompt injection.
    ///
    /// Lists all registered skills with their slash commands and descriptions.
    /// Budget: ~500 chars. Returns empty string if no skills are loaded.
    fn build_skills_catalog_block(&self) -> String {
        let summaries = self.skill_catalog.catalog_summary();
        if summaries.is_empty() {
            return String::new();
        }

        let mut block = String::from("### AVAILABLE SKILLS ###\nThe user can invoke these specialized skills with slash commands:\n");
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
        block
    }

    /// Handle a skill invocation: load full SKILL.md, inject as context, run agentic loop.
    ///
    /// Mirrors `handle_simple_query()` with an extra `### SKILL CONTEXT ###` block.
    async fn handle_skill_invocation(
        &self,
        request_id: Uuid,
        _source: &str,
        skill_name: &str,
        query: &str,
        _lane_key: &str,
        ctx: &ConversationContext,
        owner_id: Option<&str>,
    ) -> Result<String, String> {
        // Load full skill (Level 2)
        let skill_doc = self.skill_catalog.load_full(skill_name)
            .map_err(|e| format!("Failed to load skill '{}': {}", skill_name, e))?;

        let system_persona = match self.system_persona.read() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => {
                tracing::warn!("System persona lock poisoned during read; recovering");
                poisoned.into_inner().clone()
            }
        };

        let agent_persona = AgentPersona {
            role: "Assistant".to_string(),
            tone: "Concise and professional".to_string(),
            domain_knowledge: vec![],
        };
        let mut system_prompt = PromptAssembler::assemble(&system_persona, &agent_persona);

        // Inject bootstrap instructions if in first-run mode
        if let Ok(guard) = self.bootstrap_document.read() {
            if let Some(ref doc) = *guard {
                let block = bootstrap_to_prompt_block(doc);
                if !block.is_empty() {
                    system_prompt.push('\n');
                    system_prompt.push_str(&block);
                }
            }
        }

        // Inject skill context block
        let skill_block = skill_to_prompt_block(&skill_doc);
        if !skill_block.is_empty() {
            system_prompt.push('\n');
            system_prompt.push_str(&skill_block);
        }

        // Inject agent identity if available
        if let Ok(guard) = self.identity_document.read() {
            if let Some(ref doc) = *guard {
                let identity_budget = self.daemon_config.load().orchestrator.prompt_budgets.identity_budget;
                let id_block = identity_to_prompt_block(doc, Some(identity_budget));
                if !id_block.is_empty() {
                    system_prompt.push('\n');
                    system_prompt.push_str(&id_block);
                }
            }
        }

        // Resolve tools: merge skill.tools_required with intent-suggested tools
        let mut tool_names: Vec<String> = skill_doc.frontmatter.tools_required.clone();
        let intent_tools = self.intent_parser.suggest_tools(query);
        for t in intent_tools {
            if !tool_names.contains(&t) {
                tool_names.push(t);
            }
        }

        // Force-include persona tools during bootstrap mode
        if self.is_bootstrapping() {
            for name in &["update_identity", "update_user", "update_soul"] {
                if !tool_names.contains(&name.to_string()) {
                    tool_names.push(name.to_string());
                }
            }
        }

        let tool_defs: Vec<_> = tool_names
            .iter()
            .filter_map(|name| self.tool_registry.get(name).map(|t| t.definition.clone()))
            .collect();

        let (tools_for_loop, policy_opt, config_for_loop);
        if !tool_defs.is_empty() {
            tracing::info!(
                "Skill invocation '{}' with {} tools: {:?}",
                skill_name,
                tool_defs.len(),
                tool_names
            );
            system_prompt.push_str(&format_tool_guidance(&tool_defs));
            let resolved: Vec<String> = tool_defs.iter().map(|t| t.name.clone()).collect();
            policy_opt = Some(SandboxPolicy {
                agent_id: "orchestrator".to_string(),
                allowed_capabilities: resolved,
                denied_capabilities: vec![],
                require_confirmation_for: vec![],
                max_tool_calls: None,
                max_tool_runtime_secs: self.loop_config.max_tool_runtime.as_secs(),
            });
            config_for_loop = LoopConfig {
                max_rounds: 6, // Skills may need more rounds than simple queries
                max_tools_per_round: 3,
                ..self.loop_config.clone()
            };
            tools_for_loop = tool_defs;
        } else {
            tools_for_loop = vec![];
            policy_opt = None;
            config_for_loop = self.loop_config.clone();
        }

        let (response_content, is_structured) = if let Some(ref router) = self.llm_router {
            let mut messages = Vec::with_capacity(4 + ctx.recent_messages.len());
            messages.push(ChatMessage::system(&system_prompt));

            // Inject user profile if available
            if let Ok(guard) = self.user_document.read() {
                if let Some(ref doc) = *guard {
                    let user_budget = self.daemon_config.load().orchestrator.prompt_budgets.user_profile_budget;
                    let profile_block = user_to_prompt_block(doc, Some(user_budget));
                    if !profile_block.is_empty() {
                        messages.push(ChatMessage::system(&profile_block));
                    }
                }
            }

            // Inject session summary if available
            if let Some(ref summary) = ctx.summary {
                messages.push(ChatMessage::system(&format!(
                    "### SESSION SUMMARY ###\nThe following summarizes earlier parts of this conversation:\n{}",
                    summary
                )));
            }

            // Retrieval injection: hybrid FTS+vector search for user memories
            if let (Some(db), Some(oid)) = (&self.db, owner_id) {
                let repo = MemoryRepository::new(db);
                let top_k = if !tools_for_loop.is_empty() { 5 } else { 10 };

                let query_embedding = if let Some(ref embedder) = self.embedder {
                    embedder
                        .embed(&[query])
                        .await
                        .ok()
                        .and_then(|v| v.into_iter().next())
                } else {
                    None
                };

                let memories = repo
                    .search_hybrid(
                        oid,
                        query,
                        query_embedding.as_deref(),
                        top_k,
                        None,
                        None,
                        None,
                    )
                    .unwrap_or_default();

                if !memories.is_empty() {
                    // Track access for importance decay
                    let ids: Vec<i64> = memories.iter().map(|m| m.id).collect();
                    let _ = repo.touch_accessed(&ids);

                    let mut block = String::from("### RETRIEVED MEMORY ###\n");
                    let mut budget = 2000usize;
                    for m in &memories {
                        let entry = format!(
                            "- [{}] {}\n",
                            m.kind.as_str(),
                            m.content.chars().take(300).collect::<String>()
                        );
                        if entry.len() > budget {
                            break;
                        }
                        budget -= entry.len();
                        block.push_str(&entry);
                    }
                    messages.push(ChatMessage::system(&block));
                }
            }

            messages.extend(ctx.recent_messages.clone());
            messages.push(ChatMessage::user(query));

            // Per-request sandbox with ContextualToolExecutor
            let ctx_exec = ToolExecutionContext {
                owner_id: owner_id.map(|s| s.to_string()),
                task_id: None,
                agent_id: None,
                db: None,
            };
            let contextual_executor = Arc::new(ContextualToolExecutor::new(
                self.tool_registry.clone(),
                ctx_exec,
            ));
            let per_request_sandbox = SandboxManager::new(contextual_executor, self.bus.clone());

            let call_start = std::time::Instant::now();
            let result = run_agentic_loop_routed(
                router.as_ref(),
                messages,
                tools_for_loop,
                &config_for_loop,
                Some(&per_request_sandbox),
                "orchestrator",
                policy_opt.as_ref(),
                None,
            )
            .await;
            let latency_ms = call_start.elapsed().as_millis() as i64;

            // Persist LLM usage and emit event
            let default_model = router.default_model();
            let actual_model = result
                .model_used
                .as_deref()
                .or(self.loop_config.model.as_deref())
                .unwrap_or(&default_model);
            let resolved_provider = router
                .model_registry()
                .resolve_provider(actual_model)
                .map(|p| p.to_string())
                .unwrap_or_else(|| "unknown".to_string());
            let call_cost = router.cost_tracker.calculate_cost(
                actual_model,
                result.total_input_tokens,
                result.total_output_tokens,
            );

            let call_status = match &result.finish_reason {
                LoopFinishReason::Complete | LoopFinishReason::MaxRounds => "success",
                LoopFinishReason::CostExceeded => "cost_exceeded",
                LoopFinishReason::Error(_) => "error",
            };
            let call_error = match &result.finish_reason {
                LoopFinishReason::Error(msg) => Some(msg.as_str()),
                _ => None,
            };

            if let Some(ref db) = self.db {
                let usage_repo = LlmUsageRepository::new(db);
                if let Err(e) = usage_repo.record_and_log(
                    "orchestrator",
                    None,
                    &resolved_provider,
                    actual_model,
                    result.total_input_tokens as i32,
                    result.total_output_tokens as i32,
                    call_cost,
                    latency_ms,
                    call_status,
                    call_error,
                ) {
                    tracing::warn!("Failed to persist LLM usage: {e}");
                }
            }

            self.bus.publish(SystemEvent::LlmCallCompleted {
                agent_id: "orchestrator".to_string(),
                model: actual_model.to_string(),
                input_tokens: result.total_input_tokens,
                output_tokens: result.total_output_tokens,
                cost_usd: call_cost,
                timestamp: Utc::now(),
            });

            if let LoopFinishReason::Error(ref err) = result.finish_reason {
                if result.final_content.trim().is_empty() {
                    return Err(format!("LLM error: {}", err));
                }
            }

            (result.final_content, false)
        } else {
            // Fallback: echo stub with skill info
            (
                format!(
                    "{{\"status\": \"ok\", \"skill\": \"{}\", \"echo\": \"Skill invocation: {}\"}}",
                    skill_name,
                    query.chars().take(50).collect::<String>()
                ),
                true,
            )
        };

        // Output guard
        let validated = if is_structured {
            OutputGuard::ensure_json(&response_content)?
        } else {
            response_content
        };

        // Emit AgentResponse event
        self.bus.publish(SystemEvent::AgentResponse {
            request_id,
            agent_id: "orchestrator".to_string(),
            content: validated.clone(),
            timestamp: Utc::now(),
        });

        Ok(validated)
    }

    fn handle_task_query(
        &self,
        task_id: Option<String>,
        created_by: &str,
    ) -> Result<String, String> {
        match task_id {
            Some(id) => {
                // Try DB first, fall back to in-memory registry
                if let Some(ref db) = self.db {
                    let repo = TaskRepository::new(db);
                    if let Ok(Some(task)) = repo.get(&id) {
                        return Ok(db_task_to_json(&task));
                    }
                }
                match self.shared_context.task_registry.get(&id) {
                    Some(entry) => Ok(task_entry_to_json(&entry)),
                    None => Ok(serde_json::json!({
                        "error": "not_found",
                        "message": format!("Task '{}' not found", id)
                    })
                    .to_string()),
                }
            }
            None => {
                // Try DB first, fall back to in-memory registry
                if let Some(ref db) = self.db {
                    let repo = TaskRepository::new(db);
                    if let Ok(tasks) = repo.list_active_by_creator(created_by, 20) {
                        let task_list: Vec<serde_json::Value> = tasks
                            .iter()
                            .map(|t| {
                                serde_json::json!({
                                    "task_id": t.id,
                                    "title": t.title,
                                    "status": t.status.as_str(),
                                    "progress_current": t.progress_current,
                                    "progress_total": t.progress_total,
                                    "created_at": t.created_at.to_rfc3339(),
                                })
                            })
                            .collect();
                        return Ok(serde_json::json!({
                            "tasks": task_list,
                            "count": task_list.len(),
                        })
                        .to_string());
                    }
                }
                let active = self.shared_context.task_registry.list_active();
                let tasks: Vec<serde_json::Value> = active
                    .iter()
                    .map(|e| {
                        let mut v = serde_json::json!({
                            "task_id": e.task_id,
                            "title": e.title,
                            "status": e.status.as_str(),
                            "progress_current": e.progress_current,
                            "progress_total": e.progress_total,
                        });
                        if let Some(ref dag) = e.dag_summary {
                            v.as_object_mut().unwrap().insert(
                                "dag_summary".to_string(),
                                serde_json::json!({
                                    "total_nodes": dag.total_nodes,
                                    "completed_nodes": dag.completed_nodes,
                                    "running_nodes": dag.running_nodes,
                                    "failed_nodes": dag.failed_nodes,
                                }),
                            );
                        }
                        v
                    })
                    .collect();
                Ok(serde_json::json!({
                    "tasks": tasks,
                    "count": tasks.len(),
                })
                .to_string())
            }
        }
    }

    fn handle_task_control(&self, task_id: &str, action: &str) -> Result<String, String> {
        // Fetch current state
        let entry = self
            .shared_context
            .task_registry
            .get(task_id)
            .ok_or_else(|| format!("Task '{}' not found", task_id))?;

        // Validate state transition
        let new_status = match action {
            "cancel" => {
                if entry.status.is_terminal() {
                    return Err(format!(
                        "Cannot cancel task in '{}' state",
                        entry.status.as_str()
                    ));
                }
                TaskEntryStatus::Cancelled
            }
            "pause" => {
                if entry.status != TaskEntryStatus::Running {
                    return Err(format!(
                        "Can only pause a running task, current: '{}'",
                        entry.status.as_str()
                    ));
                }
                TaskEntryStatus::Paused
            }
            "resume" => {
                if entry.status != TaskEntryStatus::Paused {
                    return Err(format!(
                        "Can only resume a paused task, current: '{}'",
                        entry.status.as_str()
                    ));
                }
                TaskEntryStatus::Running
            }
            _ => return Err(format!("Unknown action: '{}'", action)),
        };

        // Update task registry
        self.shared_context
            .task_registry
            .update_status(task_id, new_status);

        // Update task lane if present
        if let Some(lane) = self.lane_manager.get_task_lane(task_id) {
            let lane_status = match new_status {
                TaskEntryStatus::Queued => TaskLaneStatus::Queued,
                TaskEntryStatus::Running => TaskLaneStatus::Running,
                TaskEntryStatus::Completed => TaskLaneStatus::Completed,
                TaskEntryStatus::Failed => TaskLaneStatus::Failed,
                TaskEntryStatus::Cancelled => TaskLaneStatus::Cancelled,
                TaskEntryStatus::Paused => TaskLaneStatus::Paused,
            };
            lane.set_status(lane_status);
        }

        // Emit TaskUpdated event
        self.bus.publish(SystemEvent::TaskUpdated {
            task_id: task_id.to_string(),
            status: new_status.as_str().to_string(),
            progress_current: None,
            progress_total: None,
            timestamp: Utc::now(),
        });

        Ok(serde_json::json!({
            "task_id": task_id,
            "action": action,
            "new_status": new_status.as_str(),
        })
        .to_string())
    }

    /// Handle "remember X" commands by storing in Memory v2 as a Preference.
    /// Checks for semantically similar existing memories and supersedes if found.
    async fn handle_remember_command(
        &self,
        content: &str,
        owner_id: Option<&str>,
    ) -> Result<String, String> {
        let oid = owner_id.ok_or_else(|| "Cannot store memory without an owner_id".to_string())?;

        if let Some(ref db) = self.db {
            let repo = MemoryRepository::new(db);
            let dcfg = self.daemon_config.load();
            let threshold = dcfg.orchestrator.memory.supersession_distance_threshold;

            // Embed new content for similarity check
            let new_embedding = if let Some(ref embedder) = self.embedder {
                embedder.embed(&[content]).await.ok().and_then(|v| v.into_iter().next())
            } else {
                None
            };

            // Check for semantically similar existing memories
            let similar = if let Some(ref emb) = new_embedding {
                repo.find_similar_for_supersession(oid, emb, threshold, 1)
                    .unwrap_or_default()
            } else {
                // FTS fallback returns (memory, jaccard_score); require >= 0.4 overlap
                repo.find_similar_fts_fallback(oid, content, 3)
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|(_, jaccard)| *jaccard >= 0.4)
                    .collect()
            };

            if let Some((existing, _)) = similar.first() {
                // Supersede the best match
                let result = repo.supersede(
                    existing.id,
                    content,
                    openalpaca_storage::models::memory::MemoryKind::Preference,
                    openalpaca_storage::models::memory::MemoryScope::Global,
                    "",
                    openalpaca_storage::models::memory::MemorySource::Conversation,
                    0.9,
                    1.0,
                    None,
                ).map_err(|e| format!("Failed to supersede memory: {}", e))?;

                if result > 0 {
                    if let Some(ref emb) = new_embedding {
                        let _ = repo.insert_embedding(result, emb);
                    }
                    Ok(format!(
                        "Got it, I've updated my memory (was: \"{}\"): {}",
                        existing.content.chars().take(50).collect::<String>(),
                        content
                    ))
                } else {
                    Ok("I already have that noted.".to_string())
                }
            } else {
                // No similar memory, insert new
                let result = repo
                    .add(
                        oid,
                        openalpaca_storage::models::memory::MemoryKind::Preference,
                        openalpaca_storage::models::memory::MemoryScope::Global,
                        "",
                        openalpaca_storage::models::memory::MemorySource::Conversation,
                        content,
                        None,
                        0.9,
                        1.0,
                    )
                    .map_err(|e| format!("Failed to store memory: {}", e))?;

                if result == 0 {
                    Ok("I already have that noted.".to_string())
                } else {
                    if let Some(ref emb) = new_embedding {
                        let _ = repo.insert_embedding(result, emb);
                    }
                    Ok(format!("Got it, I'll remember that: {}", content))
                }
            }
        } else {
            Err("Memory system is not available.".to_string())
        }
    }

    /// Handle "forget X" commands by searching and removing from Memory v2.
    async fn handle_forget_command(
        &self,
        content: &str,
        owner_id: Option<&str>,
    ) -> Result<String, String> {
        let oid = owner_id.ok_or_else(|| "Cannot search memory without an owner_id".to_string())?;

        if let Some(ref db) = self.db {
            let repo = MemoryRepository::new(db);

            // Search for matching memories
            let memories = repo
                .search_fts(
                    oid,
                    content,
                    5,
                    Some(openalpaca_storage::models::memory::MemoryKind::Preference),
                    None,
                    None,
                )
                .map_err(|e| format!("Failed to search memory: {}", e))?;

            if memories.is_empty() {
                return Ok(format!("I don't have any memory matching: {}", content));
            }

            // Delete the best match (first result)
            let best_match = &memories[0];
            repo.delete(best_match.id)
                .map_err(|e| format!("Failed to delete memory: {}", e))?;

            Ok(format!(
                "Done, I've forgotten: {}",
                best_match.content.chars().take(100).collect::<String>()
            ))
        } else {
            Err("Memory system is not available.".to_string())
        }
    }
}

fn principal_id(principal: &Principal) -> String {
    match principal {
        Principal::System => "system".to_string(),
        Principal::User { global_id } => global_id.clone(),
        Principal::External { provider, id } => format!("{}:{}", provider, id),
    }
}

fn task_entry_to_json(entry: &TaskEntry) -> String {
    let mut obj = serde_json::json!({
        "task_id": entry.task_id,
        "title": entry.title,
        "status": entry.status.as_str(),
        "progress_current": entry.progress_current,
        "progress_total": entry.progress_total,
        "created_at": entry.created_at.to_rfc3339(),
        "updated_at": entry.updated_at.to_rfc3339(),
    });
    if let Some(ref dag) = entry.dag_summary {
        obj.as_object_mut().unwrap().insert(
            "dag_summary".to_string(),
            serde_json::json!({
                "total_nodes": dag.total_nodes,
                "completed_nodes": dag.completed_nodes,
                "running_nodes": dag.running_nodes,
                "failed_nodes": dag.failed_nodes,
            }),
        );
    }
    obj.to_string()
}

fn db_task_to_json(task: &Task) -> String {
    let mut obj = serde_json::json!({
        "task_id": task.id,
        "title": task.title,
        "status": task.status.as_str(),
        "progress_current": task.progress_current,
        "progress_total": task.progress_total,
        "result_summary": task.result_summary,
        "created_at": task.created_at.to_rfc3339(),
        "updated_at": task.updated_at.to_rfc3339(),
    });

    // Parse state_json to extract DAG node details if available
    if let Some(ref sj) = task.state_json {
        if let Ok(state) = serde_json::from_str::<task_state::TaskState>(sj) {
            if let Some(ref dag) = state.dag {
                let nodes_summary: Vec<serde_json::Value> = dag
                    .nodes
                    .iter()
                    .map(|n| {
                        serde_json::json!({
                            "node_id": n.node_id,
                            "title": n.title,
                            "agent_id": n.agent_id,
                            "status": n.status,
                        })
                    })
                    .collect();
                obj.as_object_mut().unwrap().insert(
                    "dag_nodes".to_string(),
                    serde_json::json!(nodes_summary),
                );
            }
        }
    }

    obj.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::subagent::{
        AgentConstraints, AgentLlmConfig, AgentPreset, AgentStatus, Skill, SubAgent,
    };
    use crate::security::sandbox::SandboxManager;
    use crate::tools::{RegistryToolExecutor, ToolRegistry};

    fn make_tool_registry() -> Arc<ToolRegistry> {
        Arc::new(ToolRegistry::new())
    }

    fn make_security_gate(bus: &EventBus) -> Arc<SecurityGate> {
        let registry = make_tool_registry();
        let executor = Arc::new(RegistryToolExecutor::new(registry));
        let sandbox = Arc::new(SandboxManager::new(executor, bus.clone()));
        Arc::new(SecurityGate::new(sandbox))
    }

    fn make_orchestrator() -> Orchestrator {
        let ctx = Arc::new(SharedContext::new());
        let lanes = Arc::new(LaneManager::new());
        let bus = EventBus::default();
        let gate = make_security_gate(&bus);
        let registry = make_tool_registry();
        Orchestrator::new(
            ctx,
            lanes,
            bus,
            SystemPersona::default(),
            None,
            LoopConfig::default(),
            gate,
            registry,
            None,
            None,
            Arc::new(skill_catalog::SkillCatalog::new()),
            Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
        )
    }

    fn make_orchestrator_with_agents(agents: Vec<SubAgent>) -> Orchestrator {
        let ctx = Arc::new(SharedContext::new());
        for a in agents {
            ctx.agent_registry.register(a);
        }
        let lanes = Arc::new(LaneManager::new());
        let bus = EventBus::default();
        let gate = make_security_gate(&bus);
        let registry = make_tool_registry();
        Orchestrator::new(
            ctx,
            lanes,
            bus,
            SystemPersona::default(),
            None,
            LoopConfig::default(),
            gate,
            registry,
            None,
            None,
            Arc::new(skill_catalog::SkillCatalog::new()),
            Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
        )
    }

    #[test]
    fn test_update_system_persona_updates_active_snapshot() {
        let orch = make_orchestrator();
        let mut replacement = SystemPersona::default();
        replacement.name = "Soul Reloaded".to_string();

        orch.update_system_persona(replacement.clone());

        let active = orch
            .system_persona
            .read()
            .expect("system_persona lock should be readable")
            .clone();
        assert_eq!(active.name, replacement.name);
    }

    fn make_agent(id: &str, skills: Vec<&str>) -> SubAgent {
        SubAgent {
            id: id.to_string(),
            name: format!("Agent {}", id),
            description: Some(format!("{} agent", id)),
            icon: None,
            status: AgentStatus::Idle,
            current_task: None,
            skills: skills
                .into_iter()
                .map(|s| Skill {
                    name: s.to_string(),
                    category: "test".to_string(),
                    proficiency: 1.0,
                })
                .collect(),
            preset: AgentPreset::default(),
            constraints: AgentConstraints::default(),
            llm_config: AgentLlmConfig::default(),
        }
    }

    #[tokio::test]
    async fn test_simple_query_echo() {
        let orch = make_orchestrator();
        let result = orch
            .handle_message(
                Uuid::new_v4(),
                "cli".to_string(),
                "hello world".to_string(),
                Principal::System,
                Scope::Global,
                "test:cli".to_string(),
            )
            .await;
        assert!(result.is_ok());
        let json: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert_eq!(json["status"], "ok");
        assert!(json["echo"].as_str().unwrap().contains("hello world"));
    }

    #[tokio::test]
    async fn test_task_query_empty() {
        let orch = make_orchestrator();
        let result = orch
            .handle_message(
                Uuid::new_v4(),
                "cli".to_string(),
                "/status".to_string(),
                Principal::System,
                Scope::Global,
                "test:cli".to_string(),
            )
            .await;
        assert!(result.is_ok());
        let json: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert_eq!(json["count"], 0);
    }

    #[tokio::test]
    async fn test_complex_task_dispatch() {
        let orch = make_orchestrator_with_agents(vec![
            make_agent("a1", vec!["web_search"]),
            make_agent("a2", vec!["text_generate"]),
        ]);
        let result = orch
            .handle_message(
                Uuid::new_v4(),
                "cli".to_string(),
                "please research and write about Rust".to_string(),
                Principal::System,
                Scope::Global,
                "test:cli".to_string(),
            )
            .await;
        assert!(result.is_ok());
        let text = result.unwrap();
        // Response is now human-readable, not JSON
        assert!(text.contains("assigned"));
    }

    #[tokio::test]
    async fn test_task_control_cancel() {
        let orch = make_orchestrator();
        // Register a task first
        orch.shared_context
            .task_registry
            .register("t1".to_string(), "test task".to_string());

        let result = orch
            .handle_message(
                Uuid::new_v4(),
                "cli".to_string(),
                "/cancel t1".to_string(),
                Principal::System,
                Scope::Global,
                "test:cli".to_string(),
            )
            .await;
        assert!(result.is_ok());
        let json: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert_eq!(json["new_status"], "cancelled");
    }

    #[tokio::test]
    async fn test_permission_denied_external() {
        let orch = make_orchestrator();
        let result = orch
            .handle_message(
                Uuid::new_v4(),
                "telegram".to_string(),
                "hello".to_string(),
                Principal::External {
                    provider: "telegram".to_string(),
                    id: "unknown".to_string(),
                },
                Scope::Global,
                "unknown:telegram".to_string(),
            )
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Permission Denied"));
    }

    #[tokio::test]
    async fn test_full_lifecycle_events() {
        let orch = make_orchestrator_with_agents(vec![make_agent("a1", vec!["web_search"])]);
        let mut rx = orch.bus.subscribe();

        // Send a complex task
        let _result = orch
            .handle_message(
                Uuid::new_v4(),
                "cli".to_string(),
                "can you search for Rust tutorials".to_string(),
                Principal::System,
                Scope::Global,
                "test:cli".to_string(),
            )
            .await;

        // Collect events
        let mut events = Vec::new();
        while let Ok(event) = rx.try_recv() {
            events.push(event);
        }

        // Should have: IntentClassified, AgentStatusChanged, TaskCreated
        let event_types: Vec<String> = events
            .iter()
            .map(|e| match e {
                SystemEvent::IntentClassified { .. } => "intent_classified".to_string(),
                SystemEvent::AgentStatusChanged { .. } => "agent_status_changed".to_string(),
                SystemEvent::TaskCreated { .. } => "task_created".to_string(),
                other => format!("{:?}", other),
            })
            .collect();

        assert!(
            event_types.contains(&"intent_classified".to_string()),
            "Missing IntentClassified event. Got: {:?}",
            event_types
        );
        assert!(
            event_types.contains(&"task_created".to_string()),
            "Missing TaskCreated event. Got: {:?}",
            event_types
        );
    }

    #[tokio::test]
    async fn test_simple_query_with_mock_llm() {
        use async_trait::async_trait;
        use openalpaca_llm::{
            ChatRequest, ChatResponse, FinishReason, LlmError, LlmProvider, LlmRouter,
            ProviderType, Usage,
        };

        struct MockLlm;

        #[async_trait]
        impl LlmProvider for MockLlm {
            fn name(&self) -> &str {
                "mock"
            }
            fn supports_tools(&self) -> bool {
                false
            }
            async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
                Ok(ChatResponse {
                    content: r#"{"status": "ok", "answer": "Mock LLM response"}"#.to_string(),
                    tool_calls: vec![],
                    model: "mock-model".to_string(),
                    usage: Usage {
                        input_tokens: 10,
                        output_tokens: 20,
                        ..Default::default()
                    },
                    finish_reason: FinishReason::Stop,
                })
            }
        }

        let ctx = Arc::new(SharedContext::new());
        let lanes = Arc::new(LaneManager::new());
        let bus = EventBus::default();
        let gate = make_security_gate(&bus);
        let registry = make_tool_registry();
        let router = LlmRouter::single_provider(
            Arc::new(MockLlm),
            ProviderType::Anthropic,
            "claude-sonnet-4-5-20250929".to_string(),
        );
        let orch = Orchestrator::new(
            ctx,
            lanes,
            bus,
            SystemPersona::default(),
            Some(Arc::new(router)),
            LoopConfig::default(),
            gate,
            registry,
            None,
            None,
            Arc::new(skill_catalog::SkillCatalog::new()),
            Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
        );

        let result = orch
            .handle_message(
                Uuid::new_v4(),
                "cli".to_string(),
                "What is Rust?".to_string(),
                Principal::System,
                Scope::Global,
                "test:cli".to_string(),
            )
            .await;
        assert!(result.is_ok());
        let json: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert_eq!(json["status"], "ok");
        assert_eq!(json["answer"], "Mock LLM response");
    }

    #[tokio::test]
    async fn test_input_sanitization_blocks_null_bytes() {
        let orch = make_orchestrator();
        let result = orch
            .handle_message(
                Uuid::new_v4(),
                "cli".to_string(),
                "hello\0world".to_string(),
                Principal::System,
                Scope::Global,
                "test:cli".to_string(),
            )
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("null bytes"));
    }

    #[tokio::test]
    async fn test_security_gate_replaces_trust_gate() {
        // Verify that SecurityGate (wrapping TrustGate) still blocks external users
        let orch = make_orchestrator();
        let result = orch
            .handle_message(
                Uuid::new_v4(),
                "telegram".to_string(),
                "hello".to_string(),
                Principal::External {
                    provider: "telegram".to_string(),
                    id: "unknown".to_string(),
                },
                Scope::Global,
                "unknown:telegram".to_string(),
            )
            .await;
        assert!(result.is_err());
        // SecurityGate wraps TrustGate error as "Access denied: Permission Denied: ..."
        assert!(result.unwrap_err().contains("denied"));
    }

    // --- LLM Task Planning integration tests ---

    /// Helper: create a mock LLM that returns a fixed response string.
    fn make_planning_mock_llm(response: &str) -> Arc<LlmRouter> {
        use async_trait::async_trait;
        use openalpaca_llm::{
            ChatRequest, ChatResponse, FinishReason, LlmError, LlmProvider, Usage,
        };
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct PlanningMockLlm {
            response: String,
            call_count: AtomicUsize,
        }

        #[async_trait]
        impl LlmProvider for PlanningMockLlm {
            fn name(&self) -> &str {
                "planning-mock"
            }
            fn supports_tools(&self) -> bool {
                false
            }
            async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
                self.call_count.fetch_add(1, Ordering::SeqCst);
                Ok(ChatResponse {
                    content: self.response.clone(),
                    tool_calls: vec![],
                    model: "mock-model".to_string(),
                    usage: Usage {
                        input_tokens: 10,
                        output_tokens: 20,
                        ..Default::default()
                    },
                    finish_reason: FinishReason::Stop,
                })
            }
        }

        let mock = PlanningMockLlm {
            response: response.to_string(),
            call_count: AtomicUsize::new(0),
        };
        let router = openalpaca_llm::LlmRouter::single_provider(
            Arc::new(mock),
            openalpaca_llm::ProviderType::Anthropic,
            "claude-sonnet-4-5-20250929".to_string(),
        );
        Arc::new(router)
    }

    fn make_orchestrator_with_llm_and_agents(
        router: Arc<LlmRouter>,
        agents: Vec<SubAgent>,
    ) -> Orchestrator {
        let ctx = Arc::new(SharedContext::new());
        for a in agents {
            ctx.agent_registry.register(a);
        }
        let lanes = Arc::new(LaneManager::new());
        let bus = EventBus::default();
        let gate = make_security_gate(&bus);
        let registry = make_tool_registry();
        Orchestrator::new(
            ctx,
            lanes,
            bus,
            SystemPersona::default(),
            Some(router),
            LoopConfig::default(),
            gate,
            registry,
            None,
            None,
            Arc::new(skill_catalog::SkillCatalog::new()),
            Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
        )
    }

    #[tokio::test]
    async fn test_llm_planning_complex_task() {
        let plan_json = r#"{"classification": "complex_task", "title": "Research Rust patterns", "assignments": [{"agent_id": "a1", "agent_name": "Agent a1", "role_description": "Research agent", "matched_skills": ["web_search"]}], "reasoning": "User wants research"}"#;
        let router = make_planning_mock_llm(plan_json);
        let orch = make_orchestrator_with_llm_and_agents(
            router,
            vec![make_agent("a1", vec!["web_search"])],
        );

        let result = orch
            .handle_message(
                Uuid::new_v4(),
                "cli".to_string(),
                "research Rust async patterns".to_string(),
                Principal::System,
                Scope::Global,
                "test:cli".to_string(),
            )
            .await;

        assert!(result.is_ok());
        let text = result.unwrap();
        assert!(
            text.contains("assigned"),
            "Expected 'assigned' in: {}",
            text
        );

        // Verify task is registered
        assert_eq!(orch.shared_context.task_registry.count(), 1);
    }

    #[tokio::test]
    async fn test_llm_planning_simple_query() {
        let plan_json = r#"{"classification": "simple_query", "title": null, "assignments": [], "reasoning": "This is a greeting"}"#;
        let router = make_planning_mock_llm(plan_json);
        let orch = make_orchestrator_with_llm_and_agents(router, vec![]);

        let result = orch
            .handle_message(
                Uuid::new_v4(),
                "cli".to_string(),
                "hello".to_string(),
                Principal::System,
                Scope::Global,
                "test:cli".to_string(),
            )
            .await;

        assert!(result.is_ok());
        // Should NOT dispatch a task
        assert_eq!(orch.shared_context.task_registry.count(), 0);
    }

    #[tokio::test]
    async fn test_llm_planning_fallback_on_malformed() {
        // LLM returns garbage — should fall back to keyword heuristic
        let router = make_planning_mock_llm("this is not valid json at all");
        let orch = make_orchestrator_with_llm_and_agents(
            router,
            vec![make_agent("a1", vec!["web_search"])],
        );

        let result = orch
            .handle_message(
                Uuid::new_v4(),
                "cli".to_string(),
                "can you search for Rust tutorials".to_string(),
                Principal::System,
                Scope::Global,
                "test:cli".to_string(),
            )
            .await;

        // Should still work via heuristic fallback
        assert!(result.is_ok());
        let text = result.unwrap();
        assert!(
            text.contains("assigned"),
            "Expected heuristic fallback to dispatch. Got: {}",
            text
        );
    }

    #[tokio::test]
    async fn test_slash_commands_bypass_llm() {
        use async_trait::async_trait;
        use openalpaca_llm::{ChatRequest, ChatResponse, LlmError, LlmProvider};

        // Mock LLM that panics if called — slash commands must bypass it
        struct PanickingLlm;

        #[async_trait]
        impl LlmProvider for PanickingLlm {
            fn name(&self) -> &str {
                "panicking"
            }
            fn supports_tools(&self) -> bool {
                false
            }
            async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
                panic!("LLM should not be called for slash commands");
            }
        }

        let router = openalpaca_llm::LlmRouter::single_provider(
            Arc::new(PanickingLlm),
            openalpaca_llm::ProviderType::Anthropic,
            "claude-sonnet-4-5-20250929".to_string(),
        );
        let orch = make_orchestrator_with_llm_and_agents(Arc::new(router), vec![]);

        let result = orch
            .handle_message(
                Uuid::new_v4(),
                "cli".to_string(),
                "/status".to_string(),
                Principal::System,
                Scope::Global,
                "test:cli".to_string(),
            )
            .await;

        assert!(result.is_ok());
        let json: serde_json::Value = serde_json::from_str(&result.unwrap()).unwrap();
        assert_eq!(json["count"], 0);
    }

    // --- Tool-capable simple_query + dispatch fallback tests ---

    use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend};

    fn make_security_gate_with_registry(
        bus: &EventBus,
        registry: Arc<ToolRegistry>,
    ) -> Arc<SecurityGate> {
        let executor = Arc::new(RegistryToolExecutor::new(registry));
        let sandbox = Arc::new(SandboxManager::new(executor, bus.clone()));
        Arc::new(SecurityGate::new(sandbox))
    }

    struct MockBuiltInTool;

    #[async_trait::async_trait]
    impl BuiltInTool for MockBuiltInTool {
        async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
            Ok("mock tool result".to_string())
        }
    }

    fn make_mock_tool(name: &str) -> RegisteredTool {
        RegisteredTool {
            definition: openalpaca_llm::ToolDefinition {
                name: name.to_string(),
                description: format!("{} tool", name),
                parameters: serde_json::json!({"type": "object", "properties": {}}),
            },
            backend: ToolBackend::BuiltIn(Arc::new(MockBuiltInTool)),
        }
    }

    fn make_orchestrator_with_tools_and_llm(
        router: Arc<LlmRouter>,
        tool_names: &[&str],
    ) -> Orchestrator {
        let mut registry = ToolRegistry::new();
        for name in tool_names {
            registry.register(make_mock_tool(name));
        }
        let registry = Arc::new(registry);
        let ctx = Arc::new(SharedContext::new());
        let lanes = Arc::new(LaneManager::new());
        let bus = EventBus::default();
        let gate = make_security_gate_with_registry(&bus, registry.clone());
        Orchestrator::new(
            ctx,
            lanes,
            bus,
            SystemPersona::default(),
            Some(router),
            LoopConfig::default(),
            gate,
            registry,
            None,
            None,
            Arc::new(skill_catalog::SkillCatalog::new()),
            Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
        )
    }

    #[tokio::test]
    async fn test_tool_intent_detected_and_executes() {
        use openalpaca_llm::{
            ChatRequest, ChatResponse, FinishReason, LlmError, LlmProvider,
            ToolCall as LlmToolCall, Usage,
        };
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct ToolMockLlm {
            call_count: AtomicUsize,
        }

        #[async_trait::async_trait]
        impl LlmProvider for ToolMockLlm {
            fn name(&self) -> &str {
                "tool-mock"
            }
            fn supports_tools(&self) -> bool {
                true
            }
            async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
                let n = self.call_count.fetch_add(1, Ordering::SeqCst);
                match n {
                    // Call 0: planner call — return simple_query classification
                    0 => Ok(ChatResponse {
                        content: r#"{"classification": "simple_query", "title": null, "assignments": [], "reasoning": "User wants to fetch a URL"}"#.to_string(),
                        tool_calls: vec![],
                        model: "mock-model".to_string(),
                        usage: Usage { input_tokens: 10, output_tokens: 20, ..Default::default() },
                        finish_reason: FinishReason::Stop,
                    }),
                    // Call 1: agentic loop — return tool use
                    1 => Ok(ChatResponse {
                        content: String::new(),
                        tool_calls: vec![LlmToolCall {
                            id: "tc_1".to_string(),
                            name: "web_fetch".to_string(),
                            arguments: serde_json::json!({"url": "https://example.com"}),
                        }],
                        model: "mock-model".to_string(),
                        usage: Usage { input_tokens: 10, output_tokens: 20, ..Default::default() },
                        finish_reason: FinishReason::ToolUse,
                    }),
                    // Call 2+: return final answer with Stop
                    _ => Ok(ChatResponse {
                        content: "Here is the fetched content from example.com.".to_string(),
                        tool_calls: vec![],
                        model: "mock-model".to_string(),
                        usage: Usage { input_tokens: 10, output_tokens: 20, ..Default::default() },
                        finish_reason: FinishReason::Stop,
                    }),
                }
            }
        }

        let mock = ToolMockLlm {
            call_count: AtomicUsize::new(0),
        };
        let router = openalpaca_llm::LlmRouter::single_provider(
            Arc::new(mock),
            openalpaca_llm::ProviderType::Anthropic,
            "claude-sonnet-4-5-20250929".to_string(),
        );
        let orch = make_orchestrator_with_tools_and_llm(Arc::new(router), &["web_fetch"]);

        let result = orch
            .handle_message(
                Uuid::new_v4(),
                "cli".to_string(),
                "fetch https://example.com".to_string(),
                Principal::System,
                Scope::Global,
                "test:cli".to_string(),
            )
            .await;

        assert!(result.is_ok(), "Expected Ok, got: {:?}", result);
        let content = result.unwrap();
        assert!(!content.is_empty(), "Expected non-empty response");
    }

    #[tokio::test]
    async fn test_tool_max_rounds_enforcement() {
        use openalpaca_llm::{
            ChatRequest, ChatResponse, FinishReason, LlmError, LlmProvider,
            ToolCall as LlmToolCall, Usage,
        };
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct AlwaysToolUseLlm {
            call_count: AtomicUsize,
        }

        #[async_trait::async_trait]
        impl LlmProvider for AlwaysToolUseLlm {
            fn name(&self) -> &str {
                "always-tool"
            }
            fn supports_tools(&self) -> bool {
                true
            }
            async fn chat(&self, _request: ChatRequest) -> Result<ChatResponse, LlmError> {
                let n = self.call_count.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    // Planner call
                    return Ok(ChatResponse {
                        content: r#"{"classification": "simple_query", "title": null, "assignments": [], "reasoning": "simple"}"#.to_string(),
                        tool_calls: vec![],
                        model: "mock-model".to_string(),
                        usage: Usage { input_tokens: 10, output_tokens: 20, ..Default::default() },
                        finish_reason: FinishReason::Stop,
                    });
                }
                // Always return ToolUse
                Ok(ChatResponse {
                    content: String::new(),
                    tool_calls: vec![LlmToolCall {
                        id: format!("tc_{}", n),
                        name: "web_fetch".to_string(),
                        arguments: serde_json::json!({"url": "https://example.com"}),
                    }],
                    model: "mock-model".to_string(),
                    usage: Usage {
                        input_tokens: 10,
                        output_tokens: 20,
                        ..Default::default()
                    },
                    finish_reason: FinishReason::ToolUse,
                })
            }
        }

        let mock = AlwaysToolUseLlm {
            call_count: AtomicUsize::new(0),
        };
        let router = openalpaca_llm::LlmRouter::single_provider(
            Arc::new(mock),
            openalpaca_llm::ProviderType::Anthropic,
            "claude-sonnet-4-5-20250929".to_string(),
        );
        let orch = make_orchestrator_with_tools_and_llm(Arc::new(router), &["web_fetch"]);

        let result = orch
            .handle_message(
                Uuid::new_v4(),
                "cli".to_string(),
                "fetch https://example.com".to_string(),
                Principal::System,
                Scope::Global,
                "test:cli".to_string(),
            )
            .await;

        // Should complete without hanging (max_rounds=4 cap kicks in)
        assert!(
            result.is_ok(),
            "Expected Ok (max_rounds should cap), got: {:?}",
            result
        );
    }

    #[tokio::test]
    async fn test_tool_intent_but_not_in_registry() {
        // Query triggers web_fetch suggestion but registry is empty — graceful degradation
        let plan_json = r#"{"classification": "simple_query", "title": null, "assignments": [], "reasoning": "simple"}"#;
        let router = make_planning_mock_llm(plan_json);
        // Build orchestrator with NO tools in registry
        let orch = make_orchestrator_with_llm_and_agents(router, vec![]);

        let result = orch
            .handle_message(
                Uuid::new_v4(),
                "cli".to_string(),
                "fetch https://example.com".to_string(),
                Principal::System,
                Scope::Global,
                "test:cli".to_string(),
            )
            .await;

        // Should succeed without error — just proceeds tool-less
        assert!(result.is_ok(), "Expected Ok, got: {:?}", result);
    }

    #[tokio::test]
    async fn test_dispatch_error_falls_back_to_simple_query() {
        // Planner returns complex_task with nonexistent agent → dispatch fails → fallback to simple_query
        let plan_json = r#"{"classification": "complex_task", "title": "Do something", "assignments": [{"agent_id": "nonexistent_agent", "agent_name": "Ghost", "role_description": "Ghost role", "matched_skills": ["web_search"]}], "reasoning": "complex"}"#;
        let router = make_planning_mock_llm(plan_json);
        // No agents registered → dispatch_planned will fail
        let orch = make_orchestrator_with_llm_and_agents(router, vec![]);

        let result = orch
            .handle_message(
                Uuid::new_v4(),
                "cli".to_string(),
                "do something complex".to_string(),
                Principal::System,
                Scope::Global,
                "test:cli".to_string(),
            )
            .await;

        // Should succeed via fallback to simple_query (echo stub since mock LLM returns plan JSON)
        assert!(
            result.is_ok(),
            "Expected Ok via fallback, got: {:?}",
            result
        );
        // No tasks should be registered (dispatch failed)
        assert_eq!(orch.shared_context.task_registry.count(), 0);
    }
}
