use super::{ConversationContext, Orchestrator};
use crate::events::SystemEvent;
use crate::memory::scope_context::MemoryScopeContext;
use crate::middleware::bootstrap::bootstrap_to_prompt_block;
use crate::middleware::guard::{OutputGuard, detect_hallucinated_send};
use crate::middleware::identity::identity_to_prompt_block;
use crate::middleware::prompt::{
    AgentPersona, PromptAssembler, format_connector_guidance, format_message_source,
    format_tool_guidance,
};
use crate::middleware::user::user_to_prompt_block;
use crate::runner::{LoopConfig, LoopFinishReason, run_agentic_loop_routed};
use crate::security::sandbox::SandboxManager;
use crate::security::sandbox::SandboxPolicy;
use crate::tools::{ContextualToolExecutor, ToolExecutionContext};
use chrono::Utc;
use openalpaca_llm::{ChatMessage, ContentPart, ImageSource, Role, ToolChoice};
use openalpaca_storage::repository::{LlmUsageRepository, MemoryRepository, PreferenceRepository};
use std::sync::Arc;
use uuid::Uuid;

fn sanitize_parts_for_dispatch(parts: Vec<ContentPart>) -> Vec<ContentPart> {
    parts
        .into_iter()
        .filter_map(|part| match part {
            ContentPart::Image {
                source: ImageSource::FileAsset {
                    file_id,
                    media_type,
                },
                ..
            } => {
                tracing::warn!(
                    file_id = %file_id,
                    media_type = %media_type,
                    "Unresolved FileAsset image part reached query handler; replacing with placeholder"
                );
                Some(ContentPart::Text {
                    text: "[image attached — unresolved file asset reference]".to_string(),
                })
            }
            ContentPart::Text { text } if text.trim().is_empty() => {
                tracing::debug!("Dropping empty text content part before model dispatch");
                None
            }
            other => Some(other),
        })
        .collect()
}

/// Fine-grained hints for which send tools to keep alive across turns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct ActiveSendHints {
    send_message: bool,
    send_file: bool,
}

/// Analyze recent conversation to determine which send tools should be kept alive.
///
/// Returns per-tool hints so the caller injects only the relevant tool(s).
///
/// Uses a tiered priority system per assistant message (last 2 within 6-message window):
/// - **Tier 1**: Literal tool name (`send_message`, `send_file`) — highest confidence.
/// - **Tier 2**: Channel + recipient-solicitation keywords — cross-message backtrack
///   for Tier 1 clues, defaulting to `send_message` when no clue is found.
fn detect_active_send_hints(recent_messages: &[ChatMessage]) -> ActiveSendHints {
    const CHANNEL_KW: &[&str] = &["telegram", "imessage"];
    const FILE_KW: &[&str] = &["send_file", "send_file("];
    const MSG_KW: &[&str] = &["send_message", "send_message("];
    const RECIPIENT_KW: &[&str] = &[
        "recipient", "chat_id", "收件人", "发给谁", "发送给",
        "send to whom", "send it to",
    ];

    let mut hints = ActiveSendHints::default();

    // Collect assistant messages within lookback window
    let lowered: Vec<String> = recent_messages
        .iter()
        .rev()
        .take(6)
        .filter(|m| m.role == Role::Assistant)
        .take(2)
        .map(|m| m.content.to_lowercase())
        .collect();

    // Determine current channel context from most recent assistant message
    let current_turn_channels: Vec<&str> = lowered.first()
        .map(|first| CHANNEL_KW.iter().filter(|k| first.contains(*k)).copied().collect())
        .unwrap_or_default();

    for (i, lower) in lowered.iter().enumerate() {
        // Tier 1: tool name match (highest priority)
        let has_file = FILE_KW.iter().any(|k| lower.contains(k));
        let has_msg = MSG_KW.iter().any(|k| lower.contains(k));

        if has_file || has_msg {
            // Channel scoping for non-current messages:
            // If an older message explicitly mentions a channel that differs from
            // the current turn's channel, don't let its tool hints leak through.
            if i > 0 && !current_turn_channels.is_empty() {
                let msg_channels: Vec<&str> = CHANNEL_KW.iter()
                    .filter(|k| lower.contains(*k))
                    .copied()
                    .collect();
                if !msg_channels.is_empty()
                    && !msg_channels.iter().any(|c| current_turn_channels.contains(c))
                {
                    continue;
                }
            }
            hints.send_file |= has_file;
            hints.send_message |= has_msg;
            continue;
        }

        // Tier 2: channel + recipient-solicitation
        let has_channel = CHANNEL_KW.iter().any(|k| lower.contains(k));
        if !has_channel {
            continue;
        }
        let has_recipient = RECIPIENT_KW.iter().any(|k| lower.contains(k));
        if !has_recipient {
            continue;
        }

        // Channel binding: collect which channels the current message mentions
        let current_channels: Vec<&&str> = CHANNEL_KW.iter()
            .filter(|k| lower.contains(*k))
            .collect();

        // Cross-message backtrack: search OTHER assistant messages for Tier 1 clues
        // Only backtrack to messages that share at least one channel keyword.
        let mut found_clue = false;
        for (j, other_lower) in lowered.iter().enumerate() {
            if j == i {
                continue;
            }
            let shares_channel = current_channels.iter().any(|ch| other_lower.contains(**ch));
            if !shares_channel {
                continue;
            }
            let other_file = FILE_KW.iter().any(|k| other_lower.contains(k));
            let other_msg = MSG_KW.iter().any(|k| other_lower.contains(k));
            if other_file || other_msg {
                hints.send_file |= other_file;
                hints.send_message |= other_msg;
                found_clue = true;
            }
        }

        // GAP 2: no Tier 1 clue across other messages → default to send_message
        if !found_clue {
            hints.send_message = true;
        }
    }

    hints
}

/// Resolve `initial_tool_choice` for send tools based on intent-level flags.
///
/// When both `send_message` and `send_file` are in the resolved tool list,
/// uses the *current message's* intent flags to decide which tool to pin.
/// If neither was suggested by intent (e.g. both injected via multi-turn
/// keep-alive), returns `Any` so the LLM picks the right one.
fn resolve_send_tool_choice(
    intent_has_send_msg: bool,
    intent_has_send_file: bool,
    has_send_msg: bool,
    has_send_file: bool,
) -> Option<ToolChoice> {
    if has_send_msg && has_send_file {
        if intent_has_send_file {
            Some(ToolChoice::Tool("send_file".to_string()))
        } else if intent_has_send_msg {
            Some(ToolChoice::Tool("send_message".to_string()))
        } else {
            Some(ToolChoice::Any)
        }
    } else if has_send_msg {
        Some(ToolChoice::Tool("send_message".to_string()))
    } else if has_send_file {
        Some(ToolChoice::Tool("send_file".to_string()))
    } else {
        None
    }
}

/// Apply send-tool keep-alive injection to the tool list.
///
/// Snapshots intent-level flags before injection, then appends tools
/// indicated by `detect_active_send_hints()` that aren't already present.
///
/// Returns `(intent_has_send_msg, intent_has_send_file)` for downstream
/// `resolve_send_tool_choice()`.
fn apply_send_keepalive(
    tool_names: &mut Vec<String>,
    recent_messages: &[ChatMessage],
) -> (bool, bool) {
    let intent_has_send_msg = tool_names.contains(&"send_message".to_string());
    let intent_has_send_file = tool_names.contains(&"send_file".to_string());

    let keepalive = detect_active_send_hints(recent_messages);
    if !intent_has_send_msg && keepalive.send_message {
        tool_names.push("send_message".to_string());
    }
    if !intent_has_send_file && keepalive.send_file {
        tool_names.push("send_file".to_string());
    }

    (intent_has_send_msg, intent_has_send_file)
}

impl Orchestrator {
    /// Build a deterministic `<send_context>` block with resolved recipient info.
    /// This removes ambiguity: the LLM sees facts, not hints.
    pub(in crate::orchestrator) fn build_send_context(&self, owner_id: Option<&str>) -> String {
        let (db, owner) = match (&self.db, owner_id) {
            (Some(db), Some(id)) => (db, id),
            _ => return String::new(),
        };
        let sendable = self
            .connector_sender
            .read()
            .ok()
            .and_then(|g| g.as_ref().map(|p| p.sendable_channels()))
            .unwrap_or_default();
        if sendable.is_empty() {
            return String::new();
        }

        let pref_repo = PreferenceRepository::new(db);
        let mut block = String::from("<send_context>\n");
        for ch in &sendable {
            let has_default = match ch.as_str() {
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
                _ => false,
            };

            let recipient_fmt = match ch.as_str() {
                "telegram" => "\"default\" | numeric chat_id",
                "imessage" => "\"default\" | phone | email",
                _ => "\"default\"",
            };

            let detail = if has_default {
                match ch.as_str() {
                    "telegram" => "most recent Telegram chat",
                    "imessage" => "most recent iMessage conversation via AppleScript",
                    _ => "most recent conversation",
                }
            } else {
                "no recent conversation"
            };

            block.push_str(&format!(
                "- {}: default={} ({})\n  recipient: {}\n",
                ch, has_default, detail, recipient_fmt
            ));
        }
        block.push_str("</send_context>");
        block
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn handle_simple_query(
        &self,
        request_id: Uuid,
        source: &str,
        query: &str,
        tool_suggestion_query: &str,
        _lane_key: &str,
        ctx: &ConversationContext,
        owner_id: Option<&str>,
        scope_ctx: &MemoryScopeContext,
        current_parts: Option<&[ContentPart]>,
    ) -> Result<String, String> {
        // Layer 1: Deterministic direct send — bypass LLM entirely
        if let Some(result) = self.try_direct_send(tool_suggestion_query, owner_id).await {
            let response = match result {
                Ok(summary) => summary,
                Err(e) => format!("⚠️ 发送失败: {e}"),
            };
            self.bus.publish(SystemEvent::AgentResponse {
                request_id,
                agent_id: "orchestrator".to_string(),
                content: response.clone(),
                timestamp: Utc::now(),
            });
            return Ok(response);
        }

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
        if let Ok(guard) = self.bootstrap_document.read()
            && let Some(ref doc) = *guard
        {
            let block = bootstrap_to_prompt_block(doc);
            if !block.is_empty() {
                system_prompt.push('\n');
                system_prompt.push_str(&block);
            }
        }

        // Inject agent identity if available
        if let Ok(guard) = self.identity_document.read()
            && let Some(ref doc) = *guard
        {
            let identity_budget = self
                .daemon_config
                .load()
                .orchestrator
                .prompt_budgets
                .identity_budget;
            let id_block = identity_to_prompt_block(doc, Some(identity_budget));
            if !id_block.is_empty() {
                system_prompt.push('\n');
                system_prompt.push_str(&id_block);
            }
        }

        // Inject available skills catalog so the LLM knows what skills exist
        let skills_catalog_block = self.build_skills_catalog_block();
        if !skills_catalog_block.is_empty() {
            system_prompt.push('\n');
            system_prompt.push_str(&skills_catalog_block);
        }

        // Connector awareness: inject active channel list + message source
        let sendable_channels: Vec<String> = self
            .connector_sender
            .read()
            .ok()
            .and_then(|g| g.as_ref().map(|p| p.sendable_channels()))
            .unwrap_or_default();
        if let Ok(guard) = self.connector_status.read()
            && let Some(ref provider) = *guard
        {
            let statuses = provider.list_status();
            let sc_ref = if sendable_channels.is_empty() {
                None
            } else {
                Some(sendable_channels.as_slice())
            };
            let connector_block = format_connector_guidance(&statuses, sc_ref);
            if !connector_block.is_empty() {
                system_prompt.push('\n');
                system_prompt.push_str(&connector_block);
            }
        }
        let source_block = format_message_source(source);
        if !source_block.is_empty() {
            system_prompt.push('\n');
            system_prompt.push_str(&source_block);
        }

        // Resolve tools based on intent analysis
        // Tool suggestion should be based on raw user intent text, not attachment-injected context.
        let mut tool_names = self.intent_parser.suggest_tools(tool_suggestion_query);
        let (intent_has_send_msg, intent_has_send_file) =
            apply_send_keepalive(&mut tool_names, &ctx.recent_messages);

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

            // Deterministic send_context: when send_message or send_file is in the tool list,
            // inject factual recipient info so the LLM doesn't guess or ask unnecessarily.
            if tool_defs.iter().any(|d| d.name == "send_message" || d.name == "send_file") {
                let send_ctx = self.build_send_context(owner_id);
                if !send_ctx.is_empty() {
                    system_prompt.push('\n');
                    system_prompt.push_str(&send_ctx);
                }
                let mut send_rules = String::from("\n<send_rules>\n");
                if tool_defs.iter().any(|d| d.name == "send_message") {
                    send_rules.push_str(
                        "- If the user asks to send a message but did NOT provide specific text, \
                         compose a brief, natural message based on context, then call send_message.\n\
                         - NEVER claim a message was sent without calling the send_message tool.\n"
                    );
                }
                if tool_defs.iter().any(|d| d.name == "send_file") {
                    send_rules.push_str(
                        "- If the user asks to send a file, image, photo, or document, use send_file.\n\
                         - NEVER claim a file was sent without calling the send_file tool.\n"
                    );
                }
                send_rules.push_str("- Only report success/failure based on the tool's actual return value.\n</send_rules>");
                system_prompt.push_str(&send_rules);
            }

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
                initial_tool_choice: resolve_send_tool_choice(
                    intent_has_send_msg,
                    intent_has_send_file,
                    tool_defs.iter().any(|d| d.name == "send_message"),
                    tool_defs.iter().any(|d| d.name == "send_file"),
                ),
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
            if let Ok(guard) = self.user_document.read()
                && let Some(ref doc) = *guard
            {
                let user_budget = self
                    .daemon_config
                    .load()
                    .orchestrator
                    .prompt_budgets
                    .user_profile_budget;
                let profile_block = user_to_prompt_block(doc, Some(user_budget));
                if !profile_block.is_empty() {
                    messages.push(ChatMessage::system(&profile_block));
                }
            }

            // Inject session summary if available (user-role to prevent prompt injection)
            if let Some(ref summary) = ctx.summary {
                messages.push(ChatMessage::user(&super::wrap_untrusted_context(
                    summary,
                    "session_summary",
                    "user_derived",
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

                let cascade_scopes = scope_ctx.cascade_scopes();
                let memories = repo
                    .search_hybrid_cascade(
                        oid,
                        query,
                        query_embedding.as_deref(),
                        top_k,
                        None,
                        &cascade_scopes,
                    )
                    .unwrap_or_default();

                if !memories.is_empty() {
                    // Track access for importance decay + boost
                    let ids: Vec<i64> = memories.iter().map(|m| m.id).collect();
                    let boost = self
                        .daemon_config
                        .load()
                        .orchestrator
                        .memory
                        .decay
                        .access_boost;
                    if let Err(e) = repo.touch_accessed(&ids, boost) {
                        tracing::warn!("Failed to track memory access: {e}");
                    }

                    let mut inner = String::new();
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
                        inner.push_str(&entry);
                    }
                    messages.push(ChatMessage::user(&super::wrap_untrusted_context(
                        &inner,
                        "retrieved_memory",
                        "retrieved",
                    )));
                }
            }

            // Adapt multimodal parts in recent messages for the target model
            let default_model = router.default_model();
            let target_model = config_for_loop.model.as_deref().unwrap_or(&default_model);
            let adapted_messages: Vec<ChatMessage> = ctx
                .recent_messages
                .iter()
                .map(|msg| {
                    if msg.parts.is_some() {
                        let mut adapted = msg.clone();
                        adapted.parts = Some(self.adapt_parts_for_model(
                            sanitize_parts_for_dispatch(msg.parts.clone().unwrap_or_default()),
                            target_model,
                        ));
                        adapted
                    } else {
                        msg.clone()
                    }
                })
                .collect();
            messages.extend(adapted_messages);
            if let Some(parts) = current_parts {
                let adapted = self.adapt_parts_for_model(
                    sanitize_parts_for_dispatch(parts.to_vec()),
                    target_model,
                );
                messages.push(ChatMessage::user_with_parts(adapted));
            } else {
                messages.push(ChatMessage::user(query));
            }

            // Per-request sandbox with ContextualToolExecutor for owner-scoped tools
            let ctx_exec = ToolExecutionContext {
                owner_id: owner_id.map(|s| s.to_string()),
                task_id: None,
                agent_id: None,
                db: self.db.clone(),
                workspace_id: scope_ctx.workspace_id.clone(),
            };
            let contextual_executor = Arc::new(ContextualToolExecutor::new(
                self.tool_registry.clone(),
                ctx_exec,
            ));
            let per_request_sandbox =
                SandboxManager::with_defaults(contextual_executor, self.bus.clone());

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
                None, // cancel_token — interactive queries are not cancellable
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
                LoopFinishReason::Cancelled => "cancelled",
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

            // Store LLM metadata for bridge to read (keyed by request_id for concurrency safety)
            self.llm_metadata_map.insert(
                request_id,
                super::LlmMetadata {
                    model: actual_model.to_string(),
                    tokens_in: result.total_input_tokens,
                    tokens_out: result.total_output_tokens,
                },
            );

            // If LLM failed and produced no content, propagate as error
            // so the Gateway doesn't persist an empty assistant message.
            if let LoopFinishReason::Error(ref err) = result.finish_reason
                && result.final_content.trim().is_empty()
            {
                return Err(format!("LLM error: {}", err));
            }

            // Post-hoc guard: detect hallucinated send confirmations
            let tool_name_refs: Vec<&str> = tool_names.iter().map(|s| s.as_str()).collect();
            if detect_hallucinated_send(&tool_name_refs, result.tool_calls_made, &result.final_content) {
                tracing::warn!(
                    tool_calls = result.tool_calls_made,
                    "Detected hallucinated send confirmation; overriding response"
                );
                (
                    "⚠️ 消息未实际发送。模型生成了确认文本但未调用发送工具。请重新发送请求。\n\n\
                     ⚠️ Message was NOT actually sent. The model generated confirmation text \
                     without calling the send tool. Please retry your send request.".to_string(),
                    false,
                )
            } else {
                // LLM chat responses are free-form text, not structured JSON
                (result.final_content, false)
            }
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

    /// Build a lightweight `<available_skills>` block for system prompt injection.
    ///
    /// Lists all registered skills with their slash commands and descriptions.
    /// Budget: ~500 chars. Returns empty string if no skills are loaded.
    pub(super) fn build_skills_catalog_block(&self) -> String {
        let summaries = self.skill_catalog.catalog_summary();
        if summaries.is_empty() {
            return String::new();
        }

        let mut block = String::from(
            "<available_skills>\nThe user can invoke these specialized skills with slash commands:\n",
        );
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
        block.push_str("</available_skills>");
        block
    }
}

#[cfg(test)]
mod tests {
    use super::{detect_active_send_hints, ActiveSendHints, sanitize_parts_for_dispatch};
    use openalpaca_llm::{ChatMessage, ContentPart, ImageSource};

    #[test]
    fn sanitize_parts_for_dispatch_drops_empty_text_parts() {
        let parts = vec![
            ContentPart::Text {
                text: "".to_string(),
            },
            ContentPart::Text {
                text: "  \n\t".to_string(),
            },
            ContentPart::Text {
                text: "keep me".to_string(),
            },
        ];

        let sanitized = sanitize_parts_for_dispatch(parts);
        assert_eq!(sanitized.len(), 1);
        match &sanitized[0] {
            ContentPart::Text { text } => assert_eq!(text, "keep me"),
            other => panic!("expected text part, got {other:?}"),
        }
    }

    #[test]
    fn sanitize_parts_for_dispatch_replaces_unresolved_file_asset_images() {
        let parts = vec![ContentPart::Image {
            source: ImageSource::FileAsset {
                file_id: "f1".to_string(),
                media_type: "image/jpeg".to_string(),
            },
            detail: None,
        }];

        let sanitized = sanitize_parts_for_dispatch(parts);
        assert_eq!(sanitized.len(), 1);
        match &sanitized[0] {
            ContentPart::Text { text } => {
                assert_eq!(text, "[image attached — unresolved file asset reference]")
            }
            other => panic!("expected placeholder text, got {other:?}"),
        }
    }

    // --- detect_active_send_hints tests ---

    #[test]
    fn detect_send_flow_with_telegram_and_send_keyword() {
        let messages = vec![ChatMessage::assistant(
            "I can help you send a message via Telegram using the send_message tool.",
        )];
        let hints = detect_active_send_hints(&messages);
        assert!(hints.send_message);
        assert!(!hints.send_file);
    }

    #[test]
    fn detect_send_flow_no_channel_no_send() {
        let messages = vec![ChatMessage::assistant(
            "Sure, I'll help you with that task.",
        )];
        let hints = detect_active_send_hints(&messages);
        assert!(!hints.send_message);
        assert!(!hints.send_file);
    }

    #[test]
    fn detect_send_flow_empty_messages() {
        assert_eq!(detect_active_send_hints(&[]), ActiveSendHints::default());
    }

    #[test]
    fn detect_send_flow_user_only_messages() {
        let messages = vec![ChatMessage::user(
            "send message to telegram",
        )];
        let hints = detect_active_send_hints(&messages);
        assert!(!hints.send_message);
        assert!(!hints.send_file);
    }

    #[test]
    fn detect_send_flow_channel_without_send_keyword() {
        let messages = vec![ChatMessage::assistant(
            "Telegram is a messaging platform.",
        )];
        let hints = detect_active_send_hints(&messages);
        assert!(!hints.send_message);
        assert!(!hints.send_file);
    }

    #[test]
    fn detect_send_flow_chinese_context_without_tool_name() {
        // Chinese text mentioning "发送消息" but NOT the literal tool name "send_message"
        // should NOT trigger — tightened heuristic requires literal tool name.
        let messages = vec![ChatMessage::assistant(
            "好的，我将通过Telegram发送消息给您的联系人。",
        )];
        let hints = detect_active_send_hints(&messages);
        assert!(!hints.send_message);
        assert!(!hints.send_file);
    }

    #[test]
    fn detect_send_flow_chinese_context_with_tool_name() {
        // Chinese context that ALSO mentions the tool name should trigger.
        let messages = vec![ChatMessage::assistant(
            "好的，我将通过Telegram使用send_message工具发送消息。",
        )];
        let hints = detect_active_send_hints(&messages);
        assert!(hints.send_message);
        assert!(!hints.send_file);
    }

    #[test]
    fn detect_send_flow_only_recent_messages() {
        // Last 2 assistant messages (within 6-message raw window) are checked;
        // put the relevant one at position 3+ among assistants to exceed that.
        let mut messages = Vec::new();
        messages.push(ChatMessage::assistant(
            "I'll send via Telegram using send_message.",
        ));
        // Pad with 2 more assistant messages without send context
        for _ in 0..2 {
            messages.push(ChatMessage::assistant("Here is some other info."));
        }
        // The send flow message is now beyond the 2-assistant-message lookback
        let hints = detect_active_send_hints(&messages);
        assert!(!hints.send_message);
        assert!(!hints.send_file);
    }

    #[test]
    fn detect_send_flow_survives_bursty_user_messages() {
        // Tier 2: "收件人" + "Telegram" → cross-message backtrack: no clue → default send_message
        let messages = vec![
            ChatMessage::assistant("你的 Telegram 收件人是？"),
            ChatMessage::user("等一下"),
            ChatMessage::user("用 default"),
        ];
        let hints = detect_active_send_hints(&messages);
        assert!(hints.send_message);
        assert!(!hints.send_file);
    }

    #[test]
    fn detect_send_flow_discussion_about_telegram_no_tool() {
        // Talking about Telegram features without tool reference → no leak
        let messages = vec![ChatMessage::assistant(
            "Telegram is great for groups and channels.",
        )];
        let hints = detect_active_send_hints(&messages);
        assert!(!hints.send_message);
        assert!(!hints.send_file);
    }

    #[test]
    fn detect_send_flow_tier2_chinese_recipient_solicitation() {
        // Tier 2 → "收件人" + "Telegram" → no clue → default send_message
        let messages = vec![ChatMessage::assistant(
            "你的 Telegram 收件人是？",
        )];
        let hints = detect_active_send_hints(&messages);
        assert!(hints.send_message);
        assert!(!hints.send_file);
    }

    #[test]
    fn detect_send_flow_tier2_english_recipient_solicitation() {
        // Tier 2 → "send it to" + "Telegram" → no clue → default send_message
        let messages = vec![ChatMessage::assistant(
            "Who should I send it to on Telegram?",
        )];
        let hints = detect_active_send_hints(&messages);
        assert!(hints.send_message);
        assert!(!hints.send_file);
    }

    #[test]
    fn detect_send_flow_tier2_no_match_without_recipient_keyword() {
        // Chinese text with channel + generic send words but NO recipient keyword → false
        let messages = vec![ChatMessage::assistant(
            "好的，我将通过Telegram发送消息给您的联系人。",
        )];
        let hints = detect_active_send_hints(&messages);
        assert!(!hints.send_message);
        assert!(!hints.send_file);
    }

    // --- send_file flow detection tests ---

    #[test]
    fn detect_send_flow_with_send_file_tool_name() {
        let messages = vec![ChatMessage::assistant(
            "I'll use the send_file tool to send your photo via Telegram.",
        )];
        let hints = detect_active_send_hints(&messages);
        assert!(!hints.send_message);
        assert!(hints.send_file);
    }

    #[test]
    fn detect_send_flow_send_file_parens() {
        let messages = vec![ChatMessage::assistant(
            "Let me call send_file( for your iMessage attachment.",
        )];
        let hints = detect_active_send_hints(&messages);
        assert!(!hints.send_message);
        assert!(hints.send_file);
    }

    // --- new regression tests ---

    #[test]
    fn keepalive_recipient_followup_injects_only_send_file() {
        // Scenario: previous assistant turn mentioned send_file tool + Telegram,
        // user replies with just a recipient. Only send_file should be injected.
        let messages = vec![ChatMessage::assistant(
            "I'll use the send_file tool to send your photo via Telegram. What's the recipient?",
        )];
        let hints = detect_active_send_hints(&messages);
        assert!(hints.send_file, "send_file should be true (tool name in context)");
        assert!(!hints.send_message, "send_message should NOT be injected");
    }

    #[test]
    fn keepalive_recipient_followup_injects_only_send_message() {
        let messages = vec![ChatMessage::assistant(
            "I'll use send_message to send your text via Telegram. Who should I send it to?",
        )];
        let hints = detect_active_send_hints(&messages);
        assert!(hints.send_message, "send_message should be true");
        assert!(!hints.send_file, "send_file should NOT be injected");
    }

    #[test]
    fn keepalive_both_tools_mentioned() {
        let messages = vec![ChatMessage::assistant(
            "I can use send_message for text or send_file for attachments via Telegram.",
        )];
        let hints = detect_active_send_hints(&messages);
        assert!(hints.send_message);
        assert!(hints.send_file);
    }

    #[test]
    fn keepalive_recipient_only_defaults_to_send_message() {
        // Tier 2 recipient solicitation without file/message clue →
        // defaults to send_message (GAP 2)
        let messages = vec![ChatMessage::assistant(
            "你的 Telegram 收件人是？",
        )];
        let hints = detect_active_send_hints(&messages);
        assert!(hints.send_message, "recipient-only defaults to send_message");
        assert!(!hints.send_file, "send_file needs positive evidence");
    }

    #[test]
    fn keepalive_tier2_cross_message_backtrack() {
        // GAP 1: Turn N-1 mentions send_file tool, Turn N asks for recipient.
        // Turn N-1 Tier 1: "send_file" → send_file=true (accumulated)
        // Turn N Tier 2: channel+recipient → backtracks to Turn N-1 →
        //   finds "send_file" → send_file=true (no default fallback)
        let messages = vec![
            ChatMessage::assistant(
                "I'll use the send_file tool to send your photo via Telegram.",
            ),
            ChatMessage::user("等一下"),
            ChatMessage::assistant(
                "好的，你的 Telegram 收件人是？",
            ),
        ];
        let hints = detect_active_send_hints(&messages);
        assert!(hints.send_file, "send_file from Turn N-1 Tier 1 should propagate");
        assert!(!hints.send_message, "no send_message evidence; backtrack found send_file so no default");
    }

    #[test]
    fn tier2_cross_channel_backtrack_does_not_leak() {
        // Turn N-1: send_file for iMessage
        // Turn N: Telegram recipient follow-up
        // Backtrack should NOT pick up send_file (different channel)
        // → defaults to send_message (GAP 2 fallback)
        let messages = vec![
            ChatMessage::assistant(
                "I'll use the send_file tool to send your photo via iMessage.",
            ),
            ChatMessage::user("等一下"),
            ChatMessage::assistant(
                "好的，你的 Telegram 收件人是？",
            ),
        ];
        let hints = detect_active_send_hints(&messages);
        // Turn N-1 Tier 1 send_file is now blocked: iMessage ≠ Telegram
        assert!(!hints.send_file, "cross-channel Tier 1 should not leak");
        // Turn N Tier 2 backtracks but finds no shared-channel clue → default send_message
        assert!(hints.send_message, "Tier 2 defaults to send_message when no channel-matching clue");
    }

    // --- resolve_send_tool_choice unit tests ---

    use super::resolve_send_tool_choice;
    use openalpaca_llm::ToolChoice;

    #[test]
    fn tool_choice_both_from_keepalive_returns_any() {
        // Neither tool suggested by intent (both injected via keep-alive)
        let choice = resolve_send_tool_choice(false, false, true, true);
        assert_eq!(choice, Some(ToolChoice::Any));
    }

    #[test]
    fn tool_choice_intent_send_file_pins_send_file() {
        // Intent detected send_file, keep-alive added send_message
        let choice = resolve_send_tool_choice(false, true, true, true);
        assert_eq!(
            choice,
            Some(ToolChoice::Tool("send_file".to_string()))
        );
    }

    #[test]
    fn tool_choice_intent_send_msg_pins_send_message() {
        // Intent detected send_message, keep-alive added send_file
        let choice = resolve_send_tool_choice(true, false, true, true);
        assert_eq!(
            choice,
            Some(ToolChoice::Tool("send_message".to_string()))
        );
    }

    #[test]
    fn tool_choice_only_send_message_pins_it() {
        let choice = resolve_send_tool_choice(true, false, true, false);
        assert_eq!(
            choice,
            Some(ToolChoice::Tool("send_message".to_string()))
        );
    }

    #[test]
    fn tool_choice_only_send_file_pins_it() {
        let choice = resolve_send_tool_choice(false, true, false, true);
        assert_eq!(
            choice,
            Some(ToolChoice::Tool("send_file".to_string()))
        );
    }

    #[test]
    fn tool_choice_no_send_tools_returns_none() {
        let choice = resolve_send_tool_choice(false, false, false, false);
        assert_eq!(choice, None);
    }

    // --- apply_send_keepalive direct tests ---

    use super::apply_send_keepalive;

    #[test]
    fn apply_keepalive_injects_send_file_only() {
        let messages = vec![ChatMessage::assistant(
            "I'll use the send_file tool via Telegram.",
        )];
        let mut tool_names = vec!["web_fetch".to_string()];
        let (intent_msg, intent_file) = apply_send_keepalive(&mut tool_names, &messages);
        assert!(!intent_msg);
        assert!(!intent_file);
        assert!(tool_names.contains(&"send_file".to_string()));
        assert!(!tool_names.contains(&"send_message".to_string()));
    }

    #[test]
    fn apply_keepalive_skips_when_intent_has_tool() {
        let messages = vec![ChatMessage::assistant(
            "I'll use the send_message tool via Telegram.",
        )];
        let mut tool_names = vec!["send_message".to_string()];
        let (intent_msg, intent_file) = apply_send_keepalive(&mut tool_names, &messages);
        assert!(intent_msg);
        assert!(!intent_file);
        // No duplicate — still just one send_message
        assert_eq!(
            tool_names.iter().filter(|t| *t == "send_message").count(),
            1
        );
    }

    #[test]
    fn apply_keepalive_injects_both_when_both_hinted() {
        let messages = vec![ChatMessage::assistant(
            "I can use send_message for text or send_file for attachments via Telegram.",
        )];
        let mut tool_names = vec![];
        let (intent_msg, intent_file) = apply_send_keepalive(&mut tool_names, &messages);
        assert!(!intent_msg);
        assert!(!intent_file);
        assert!(tool_names.contains(&"send_message".to_string()));
        assert!(tool_names.contains(&"send_file".to_string()));
    }

    // --- end-to-end conflict path: intent + keep-alive → tool_choice ---

    #[test]
    fn keepalive_plain_text_continuation_resolves_to_send_file() {
        // Scenario: previous turn used send_file, user replies "好的，发吧".
        // Precise keep-alive: only send_file injected, intent has neither → Tool("send_file")
        let messages = vec![ChatMessage::assistant(
            "I'll use the send_file tool via Telegram.",
        )];
        let parser = crate::orchestrator::intent::IntentParser;
        let mut tool_names = parser.suggest_tools("好的，发吧");
        let (intent_msg, intent_file) = apply_send_keepalive(&mut tool_names, &messages);

        let has_msg = tool_names.contains(&"send_message".to_string());
        let has_file = tool_names.contains(&"send_file".to_string());
        let choice = resolve_send_tool_choice(intent_msg, intent_file, has_msg, has_file);
        assert_eq!(choice, Some(ToolChoice::Tool("send_file".to_string())));
    }

    #[test]
    fn keepalive_text_send_continuation_resolves_to_send_message() {
        // Scenario: previous turn used send_message, user replies "发消息给他".
        // Precise keep-alive: only send_message injected, intent also has send_message → pin
        let messages = vec![ChatMessage::assistant(
            "好的，我将通过Telegram使用send_message工具发送消息。",
        )];
        let parser = crate::orchestrator::intent::IntentParser;
        let mut tool_names = parser.suggest_tools("发消息给他");
        let (intent_msg, intent_file) = apply_send_keepalive(&mut tool_names, &messages);

        let has_msg = tool_names.contains(&"send_message".to_string());
        let has_file = tool_names.contains(&"send_file".to_string());
        let choice = resolve_send_tool_choice(intent_msg, intent_file, has_msg, has_file);
        assert_eq!(
            choice,
            Some(ToolChoice::Tool("send_message".to_string()))
        );
    }

    #[test]
    fn apply_keepalive_cross_channel_injects_only_send_message() {
        // iMessage send_file in Turn N-1, Telegram recipient follow-up in Turn N
        // → only send_message should be injected (from Tier 2 GAP 2 fallback)
        let messages = vec![
            ChatMessage::assistant(
                "I'll use the send_file tool to send your photo via iMessage.",
            ),
            ChatMessage::user("等一下"),
            ChatMessage::assistant(
                "好的，你的 Telegram 收件人是？",
            ),
        ];
        let mut tool_names = vec!["web_fetch".to_string()];
        let (intent_msg, intent_file) = apply_send_keepalive(&mut tool_names, &messages);
        assert!(!intent_msg);
        assert!(!intent_file);
        assert!(tool_names.contains(&"send_message".to_string()), "Tier 2 GAP 2 fallback");
        assert!(!tool_names.contains(&"send_file".to_string()), "cross-channel send_file must not leak");
        assert_eq!(tool_names.len(), 2); // web_fetch + send_message only
    }
}
