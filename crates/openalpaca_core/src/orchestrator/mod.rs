//! Orchestrator module: the central message handler.
//!
//! Routes user messages through intent classification, skill matching,
//! and task dispatch pipelines.

pub mod dispatcher;
pub mod intent;
pub mod skill_matcher;
pub mod task_planner;

use crate::bus::EventBus;
use crate::context::{SharedContext, TaskEntry, TaskEntryStatus};
use crate::events::SystemEvent;
use crate::lane::{LaneManager, TaskLaneStatus};
use crate::middleware::guard::OutputGuard;
use crate::middleware::prompt::{AgentPersona, PromptAssembler, SystemPersona, format_tool_guidance};
use crate::runner::{LoopConfig, LoopFinishReason, run_agentic_loop_routed};
use crate::security::gate::SecurityGate;
use crate::security::policy::{Principal, Scope};
use crate::security::sandbox::SandboxPolicy;
use crate::tools::ToolRegistry;
use crate::types::Capability;
use chrono::Utc;
use openalpaca_llm::{ChatMessage, LlmRouter, RequestContext, Role, RouterRequest};
use openalpaca_storage::{ConversationRepository, Database};
use openalpaca_storage::repository::LlmUsageRepository;
use std::sync::Arc;
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
    pub system_persona: SystemPersona,
    pub llm_router: Option<Arc<LlmRouter>>,
    pub loop_config: LoopConfig,
    pub security_gate: Arc<SecurityGate>,
    pub tool_registry: Arc<ToolRegistry>,
    intent_parser: IntentParser,
    task_dispatcher: TaskDispatcher,
    db: Option<Database>,
}

const PROMPT_RECENT_MESSAGES: usize = 40;
const SUMMARY_MIN_NEW_OLDER_MESSAGES: usize = 12;
const SUMMARY_MAX_CHARS: usize = 4000;
const MSG_TRUNC_CHARS: usize = 1500;
const SUMMARY_MAX_DAILY_COST_USD: f64 = 0.50;

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
    ) -> Self {
        let task_dispatcher = TaskDispatcher::new(
            shared_context.clone(),
            lane_manager.clone(),
            bus.clone(),
            llm_router.clone(),
            security_gate.clone(),
            tool_registry.clone(),
            db.clone(),
        );
        Self {
            shared_context,
            lane_manager,
            bus,
            system_persona,
            llm_router,
            loop_config,
            security_gate,
            tool_registry,
            intent_parser: IntentParser,
            task_dispatcher,
            db,
        }
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
        let (summary_text, summary_version, last_summarized_id) =
            match repo.get_summary(lane_key) {
                Ok(tuple) => tuple,
                Err(_) => (String::new(), 0, 0),
            };

        // Step 2: Load recent messages (40, not 120)
        let raw_messages = match repo.list_recent_by_lane(lane_key, PROMPT_RECENT_MESSAGES as i64) {
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
        if new_older.len() < SUMMARY_MIN_NEW_OLDER_MESSAGES {
            return;
        }

        // D12: Budget pre-check — agent-specific cost for "orchestrator_summary"
        let summary_cost = router
            .cost_tracker
            .get_agent_usage("orchestrator_summary")
            .await
            .map(|s| s.total_cost_usd)
            .unwrap_or(0.0);
        if summary_cost > SUMMARY_MAX_DAILY_COST_USD {
            tracing::debug!(
                "Summary update skipped: summary cost ${summary_cost:.2} exceeds cap"
            );
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
            let truncated: String = content.chars().take(MSG_TRUNC_CHARS).collect();
            user_prompt.push_str(&format!("{}: {}\n", role, truncated));
        }
        user_prompt.push_str(&format!(
            "\nUpdate the summary incorporating these new messages. Max {} characters. Output JSON only.",
            SUMMARY_MAX_CHARS
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
                     Focus only on the human-to-assistant dialogue and decisions made."
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
                    after.find("```").map(|end| &after[..end]).unwrap_or(trimmed)
                } else if let Some(start) = trimmed.find("```") {
                    let after = &trimmed[start + 3..];
                    after.find("```").map(|end| &after[..end]).unwrap_or(trimmed)
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

        let new_summary: String = new_summary.chars().take(SUMMARY_MAX_CHARS).collect();
        let new_last_id = new_older
            .last()
            .map(|(id, _, _)| *id)
            .unwrap_or(ctx.last_summarized_id);

        // Save with optimistic locking to conversations table
        let repo = ConversationRepository::new(db);
        match repo.update_summary_optimistic(lane_key, ctx.summary_version, &new_summary, new_last_id) {
            Ok(true) => tracing::debug!("Summary updated successfully"),
            Ok(false) => tracing::warn!("Summary update: concurrent write, version mismatch"),
            Err(e) => tracing::warn!("Summary update: save failed: {e}"),
        }
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
        let content = SecurityGate::sanitize_input(&content)?;

        // 3. Try slash commands and task queries first (cheap, no context needed)
        let intent = self.intent_parser.parse(&content);
        match &intent {
            Intent::TaskQuery { .. } | Intent::TaskControl { .. } => {
                self.bus.publish(SystemEvent::IntentClassified {
                    request_id,
                    intent_type: intent.intent_type().to_string(),
                    timestamp: Utc::now(),
                });
                return match intent {
                    Intent::TaskQuery { task_id } => self.handle_task_query(task_id),
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

        // 5. Compute result — planner path or heuristic fallback
        let result: Result<String, String> = if let Some(ref router) = self.llm_router {
            let idle_agents = self.shared_context.agent_registry.list_idle();
            match TaskPlanner::plan(
                router,
                &content,
                &idle_agents,
                &ctx.recent_messages,
                ctx.summary.as_deref(),
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
                        self.handle_simple_query(request_id, &source, &content, &lane_key, &ctx)
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
                                tracing::warn!("Dispatch planned failed: {e}, falling back to simple_query");
                                self.handle_simple_query(request_id, &source, &content, &lane_key, &ctx).await
                            }
                        }
                    }
                    _other => {
                        tracing::warn!(
                            "LLM planner returned unknown classification '{}', falling back to heuristic",
                            _other
                        );
                        self.dispatch_with_heuristic(
                            request_id, &source, &content, &principal, &lane_key, &ctx,
                        )
                        .await
                    }
                },
                Err(e) => {
                    tracing::warn!("LLM planning failed: {}, falling back to heuristic", e);
                    self.dispatch_with_heuristic(
                        request_id, &source, &content, &principal, &lane_key, &ctx,
                    )
                    .await
                }
            }
        } else {
            // No LLM router — keyword heuristic
            self.dispatch_with_heuristic(
                request_id, &source, &content, &principal, &lane_key, &ctx,
            )
            .await
        };

        // 6. Summary update ONCE, AFTER result, for ALL normal turns (D7)
        self.maybe_update_summary(&lane_key, &ctx).await;

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
    ) -> Result<String, String> {
        let intent = self.intent_parser.parse(content);

        self.bus.publish(SystemEvent::IntentClassified {
            request_id,
            intent_type: intent.intent_type().to_string(),
            timestamp: Utc::now(),
        });

        match intent {
            Intent::SimpleQuery { query } => {
                self.handle_simple_query(request_id, source, &query, lane_key, ctx)
                    .await
            }
            Intent::TaskQuery { task_id } => self.handle_task_query(task_id),
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
                        tracing::warn!("Heuristic dispatch failed: {e}, falling back to simple_query");
                        self.handle_simple_query(request_id, source, &description, lane_key, ctx).await
                    }
                }
            }
            Intent::TaskControl { task_id, action } => {
                self.handle_task_control(&task_id, &action)
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
    ) -> Result<String, String> {
        let agent_persona = AgentPersona {
            role: "Assistant".to_string(),
            tone: "Concise and professional".to_string(),
            domain_knowledge: vec![],
        };
        let mut system_prompt = PromptAssembler::assemble(&self.system_persona, &agent_persona);
        system_prompt.push_str("\n\n### STYLE RULES ###\n");
        system_prompt.push_str("- Be concise and direct. Avoid filler words.\n");
        system_prompt.push_str("- Do NOT use emojis.\n");
        system_prompt.push_str("- If the message is casual (greeting, number, short phrase), respond briefly and naturally.\n");

        // Resolve tools based on intent analysis
        let tool_names = self.intent_parser.suggest_tools(query);
        let tool_defs: Vec<_> = tool_names.iter()
            .filter_map(|name| self.tool_registry.get(name).map(|t| t.definition.clone()))
            .collect();

        let (tools_for_loop, policy_opt, config_for_loop);
        if !tool_defs.is_empty() {
            tracing::info!("Simple query upgraded with {} tools: {:?}", tool_defs.len(), tool_names);
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
            config_for_loop = LoopConfig { max_rounds: 4, max_tools_per_round: 2, ..self.loop_config.clone() };
            tools_for_loop = tool_defs;
        } else {
            tools_for_loop = vec![];
            policy_opt = None;
            config_for_loop = self.loop_config.clone();
        }

        let (response_content, is_structured) = if let Some(ref router) = self.llm_router {
            // Real LLM call via routed agentic loop
            let mut messages = Vec::with_capacity(3 + ctx.recent_messages.len());
            messages.push(ChatMessage::system(&system_prompt));

            // Inject session summary if available
            if let Some(ref summary) = ctx.summary {
                messages.push(ChatMessage::system(&format!(
                    "### SESSION SUMMARY ###\nThe following summarizes earlier parts of this conversation:\n{}",
                    summary
                )));
            }

            messages.extend(ctx.recent_messages.clone());
            messages.push(ChatMessage::user(query));
            let call_start = std::time::Instant::now();
            let result = run_agentic_loop_routed(
                router.as_ref(),
                messages,
                tools_for_loop,
                &config_for_loop,
                Some(self.security_gate.sandbox()),
                "orchestrator",
                policy_opt.as_ref(),
                None,
            )
            .await;
            let latency_ms = call_start.elapsed().as_millis() as i64;

            // Persist LLM usage and emit event
            let actual_model = result.model_used.as_deref()
                .or(self.loop_config.model.as_deref())
                .unwrap_or(router.default_model());
            let resolved_provider = router.model_registry()
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
            (format!(
                "{{\"status\": \"ok\", \"echo\": \"Received: {}\"}}",
                query.chars().take(50).collect::<String>()
            ), true)
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

    fn handle_task_query(&self, task_id: Option<String>) -> Result<String, String> {
        match task_id {
            Some(id) => {
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
                let active = self.shared_context.task_registry.list_active();
                let tasks: Vec<serde_json::Value> =
                    active.iter().map(|e| serde_json::json!({
                        "task_id": e.task_id,
                        "title": e.title,
                        "status": e.status.as_str(),
                    })).collect();
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
}

fn principal_id(principal: &Principal) -> String {
    match principal {
        Principal::System => "system".to_string(),
        Principal::User { global_id } => global_id.clone(),
        Principal::External { provider, id } => format!("{}:{}", provider, id),
    }
}

fn task_entry_to_json(entry: &TaskEntry) -> String {
    serde_json::json!({
        "task_id": entry.task_id,
        "title": entry.title,
        "status": entry.status.as_str(),
        "created_at": entry.created_at.to_rfc3339(),
        "updated_at": entry.updated_at.to_rfc3339(),
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::subagent::{AgentConstraints, AgentLlmConfig, AgentPreset, AgentStatus, Skill, SubAgent};
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
        )
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
        use openalpaca_llm::{ChatRequest, ChatResponse, FinishReason, LlmError, LlmProvider, LlmRouter, ProviderType, Usage};

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
        use openalpaca_llm::{ChatRequest, ChatResponse, FinishReason, LlmError, LlmProvider, Usage};
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
        assert!(text.contains("assigned"), "Expected 'assigned' in: {}", text);

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

    fn make_security_gate_with_registry(bus: &EventBus, registry: Arc<ToolRegistry>) -> Arc<SecurityGate> {
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
        )
    }

    #[tokio::test]
    async fn test_tool_intent_detected_and_executes() {
        use openalpaca_llm::{ChatRequest, ChatResponse, FinishReason, LlmError, LlmProvider, Usage, ToolCall as LlmToolCall};
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct ToolMockLlm {
            call_count: AtomicUsize,
        }

        #[async_trait::async_trait]
        impl LlmProvider for ToolMockLlm {
            fn name(&self) -> &str { "tool-mock" }
            fn supports_tools(&self) -> bool { true }
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

        let mock = ToolMockLlm { call_count: AtomicUsize::new(0) };
        let router = openalpaca_llm::LlmRouter::single_provider(
            Arc::new(mock),
            openalpaca_llm::ProviderType::Anthropic,
            "claude-sonnet-4-5-20250929".to_string(),
        );
        let orch = make_orchestrator_with_tools_and_llm(
            Arc::new(router),
            &["web_fetch"],
        );

        let result = orch.handle_message(
            Uuid::new_v4(),
            "cli".to_string(),
            "fetch https://example.com".to_string(),
            Principal::System,
            Scope::Global,
            "test:cli".to_string(),
        ).await;

        assert!(result.is_ok(), "Expected Ok, got: {:?}", result);
        let content = result.unwrap();
        assert!(!content.is_empty(), "Expected non-empty response");
    }

    #[tokio::test]
    async fn test_tool_max_rounds_enforcement() {
        use openalpaca_llm::{ChatRequest, ChatResponse, FinishReason, LlmError, LlmProvider, Usage, ToolCall as LlmToolCall};
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct AlwaysToolUseLlm {
            call_count: AtomicUsize,
        }

        #[async_trait::async_trait]
        impl LlmProvider for AlwaysToolUseLlm {
            fn name(&self) -> &str { "always-tool" }
            fn supports_tools(&self) -> bool { true }
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
                    usage: Usage { input_tokens: 10, output_tokens: 20, ..Default::default() },
                    finish_reason: FinishReason::ToolUse,
                })
            }
        }

        let mock = AlwaysToolUseLlm { call_count: AtomicUsize::new(0) };
        let router = openalpaca_llm::LlmRouter::single_provider(
            Arc::new(mock),
            openalpaca_llm::ProviderType::Anthropic,
            "claude-sonnet-4-5-20250929".to_string(),
        );
        let orch = make_orchestrator_with_tools_and_llm(
            Arc::new(router),
            &["web_fetch"],
        );

        let result = orch.handle_message(
            Uuid::new_v4(),
            "cli".to_string(),
            "fetch https://example.com".to_string(),
            Principal::System,
            Scope::Global,
            "test:cli".to_string(),
        ).await;

        // Should complete without hanging (max_rounds=4 cap kicks in)
        assert!(result.is_ok(), "Expected Ok (max_rounds should cap), got: {:?}", result);
    }

    #[tokio::test]
    async fn test_tool_intent_but_not_in_registry() {
        // Query triggers web_fetch suggestion but registry is empty — graceful degradation
        let plan_json = r#"{"classification": "simple_query", "title": null, "assignments": [], "reasoning": "simple"}"#;
        let router = make_planning_mock_llm(plan_json);
        // Build orchestrator with NO tools in registry
        let orch = make_orchestrator_with_llm_and_agents(router, vec![]);

        let result = orch.handle_message(
            Uuid::new_v4(),
            "cli".to_string(),
            "fetch https://example.com".to_string(),
            Principal::System,
            Scope::Global,
            "test:cli".to_string(),
        ).await;

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

        let result = orch.handle_message(
            Uuid::new_v4(),
            "cli".to_string(),
            "do something complex".to_string(),
            Principal::System,
            Scope::Global,
            "test:cli".to_string(),
        ).await;

        // Should succeed via fallback to simple_query (echo stub since mock LLM returns plan JSON)
        assert!(result.is_ok(), "Expected Ok via fallback, got: {:?}", result);
        // No tasks should be registered (dispatch failed)
        assert_eq!(orch.shared_context.task_registry.count(), 0);
    }
}
