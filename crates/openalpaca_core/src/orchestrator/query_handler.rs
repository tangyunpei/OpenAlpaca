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
use std::time::Duration;
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

/// Hints for whether the send tool should be kept alive across turns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct ActiveSendHints {
    send: bool,
}

/// Analyze recent conversation to determine whether the send tool should be kept alive.
///
/// Uses a tiered priority system per assistant message (last 2 within 6-message window):
/// - **Tier 1**: Literal tool name (`send(`, `send tool`, `"send"`, `` `send` ``, `call send`, `use send`) — highest confidence.
/// - **Tier 2**: Channel + recipient-solicitation keywords — defaults to send active.
pub(super) fn detect_active_send_hints(
    recent_messages: &[ChatMessage],
    active_channels: &[String],
) -> ActiveSendHints {
    const FALLBACK_CHANNEL_KW: &[&str] = &["telegram", "imessage", "slack", "discord", "whatsapp", "wechat", "signal"];
    const SEND_KW: &[&str] = &["send(", "send tool", "\"send\"", "`send`", "call send", "use send"];
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

    for lower in &lowered {
        // Tier 1: tool name match (highest priority)
        if SEND_KW.iter().any(|k| lower.contains(k)) {
            hints.send = true;
            continue;
        }

        // Tier 2: channel + recipient-solicitation
        let has_channel = active_channels.iter().any(|k| lower.contains(&k.to_lowercase()))
            || FALLBACK_CHANNEL_KW.iter().any(|k| lower.contains(k));
        if !has_channel {
            continue;
        }
        let has_recipient = RECIPIENT_KW.iter().any(|k| lower.contains(k));
        if has_recipient {
            hints.send = true;
        }
    }

    hints
}

/// Resolve `initial_tool_choice` for the send tool.
fn resolve_send_tool_choice(has_send: bool) -> Option<ToolChoice> {
    if has_send {
        Some(ToolChoice::Tool("send".to_string()))
    } else {
        None
    }
}

/// Apply send-tool keep-alive injection to the tool list.
///
/// Snapshots intent-level flag before injection, then appends the send tool
/// if indicated by `detect_active_send_hints()` and not already present.
///
/// Returns whether the intent originally suggested send.
fn apply_send_keepalive(
    tool_names: &mut Vec<String>,
    recent_messages: &[ChatMessage],
    active_channels: &[String],
) -> bool {
    let intent_has_send = tool_names.contains(&"send".to_string());

    let keepalive = detect_active_send_hints(recent_messages, active_channels);
    if !intent_has_send && keepalive.send {
        tool_names.push("send".to_string());
    }

    intent_has_send
}

const BASE_PROMPT_TTL: Duration = Duration::from_secs(30);

impl Orchestrator {
    /// Get the base system prompt from cache or build fresh.
    /// Base prompt = persona + identity + bootstrap.
    /// Skills catalog, connector guidance, tools, and memory are per-request.
    pub(super) fn get_or_build_base_prompt(&self) -> String {
        // Fast path: check cache
        let current = self.cached_base_prompt.load();
        if let Some(ref cached) = **current
            && cached.built_at.elapsed() < BASE_PROMPT_TTL
        {
            return cached.base.clone();
        }

        // Release the Guard slot before the slow path to avoid holding a
        // hazard-pointer slot across multiple RwLock acquisitions.
        let current_arc: Arc<Option<super::CachedBasePrompt>> = Arc::clone(&*current);
        drop(current);

        // Slow path: build fresh
        let base = self.build_base_system_prompt();
        let new_entry = Arc::new(Some(super::CachedBasePrompt {
            base: base.clone(),
            built_at: std::time::Instant::now(),
        }));
        // CAS to avoid thundering herd: if another thread already rebuilt,
        // their value wins and we discard ours (harmless — same content).
        let _ = self
            .cached_base_prompt
            .compare_and_swap(&current_arc, new_entry);
        base
    }

    /// Build the invariant base system prompt from current documents.
    /// Excludes skills catalog — that has its own cache in SkillCatalog
    /// and is hot-reloaded independently via reload_skill().
    fn build_base_system_prompt(&self) -> String {
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
        let mut prompt = PromptAssembler::assemble(&system_persona, &agent_persona);

        // Bootstrap block
        if let Ok(guard) = self.bootstrap_document.read()
            && let Some(ref doc) = *guard
        {
            let block = bootstrap_to_prompt_block(doc);
            if !block.is_empty() {
                prompt.push('\n');
                prompt.push_str(&block);
            }
        }

        // Identity block
        // Note: identity_budget is read from daemon_config here and cached for up to
        // BASE_PROMPT_TTL (30s). If daemon_config is hot-reloaded, the new budget
        // takes effect after TTL expiry or explicit invalidation.
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
                prompt.push('\n');
                prompt.push_str(&id_block);
            }
        }

        prompt
    }

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
        stream_id: Option<&str>,
    ) -> Result<String, String> {
        // Layer 1: Deterministic direct send — bypass LLM entirely
        if let Some(result) = self.try_direct_send(tool_suggestion_query, owner_id).await {
            let response = match result {
                Ok(summary) => summary,
                Err(e) => format!("⚠️ Send failed / 发送失败: {e}"),
            };
            self.bus.publish(SystemEvent::AgentResponse {
                request_id,
                agent_id: "orchestrator".to_string(),
                content: response.clone(),
                timestamp: Utc::now(),
            });
            return Ok(response);
        }

        // Base prompt from cache (persona + identity + bootstrap)
        let mut system_prompt = self.get_or_build_base_prompt();

        // Skills catalog — per-request (SkillCatalog has its own internal cache;
        // excluded from base prompt cache to avoid cross-object invalidation on hot-reload)
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
        let _intent_has_send =
            apply_send_keepalive(&mut tool_names, &ctx.recent_messages, &sendable_channels);

        // Force-include persona tools during bootstrap mode
        if self.is_bootstrapping() {
            for name in &["update_persona"] {
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

            // Deterministic send_context: when send is in the tool list,
            // inject factual recipient info so the LLM doesn't guess or ask unnecessarily.
            if tool_defs.iter().any(|d| d.name == "send") {
                let send_ctx = self.build_send_context(owner_id);
                if !send_ctx.is_empty() {
                    system_prompt.push('\n');
                    system_prompt.push_str(&send_ctx);
                }
                let mut send_rules = String::from("\n<send_rules>\n");
                send_rules.push_str(
                    "- If the user asks to send a message but did NOT provide specific text, \
                     compose a brief, natural message based on context, then call send (action: \"message\").\n\
                     - If the user asks to send a file, image, photo, or document, call send (action: \"file\").\n\
                     - NEVER claim a message or file was sent without calling the send tool.\n"
                );
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
                stream_id: stream_id.map(|s| s.to_string()),
                lane_key: Some(_lane_key.to_string()),
                confirmation_timeout_secs: None,
                auto_approve: self.daemon_config.load().security.auto_approve_confirmations,
            });
            config_for_loop = LoopConfig {
                max_rounds: 4,
                max_tools_per_round: 2,
                initial_tool_choice: resolve_send_tool_choice(
                    tool_defs.iter().any(|d| d.name == "send"),
                ),
                enable_caching: false,
                thinking: None,
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
            let mut per_request_sandbox =
                SandboxManager::with_defaults(contextual_executor, self.bus.clone());
            if let Ok(guard) = self.confirmation_broker.read() {
                if let Some(broker) = guard.as_ref() {
                    per_request_sandbox.set_confirmation_broker(broker.clone());
                }
            }

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
                LoopFinishReason::Complete | LoopFinishReason::MaxRounds | LoopFinishReason::Truncated => "success",
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

    /// Ultra-fast path for social/acknowledgement messages ("ok", "thanks", "好的").
    ///
    /// Minimal prompt: system persona (SOUL) only. No identity, bootstrap, skills,
    /// connectors, memory retrieval, or tools. max_rounds=1.
    pub(super) async fn handle_social_query(
        &self,
        request_id: Uuid,
        query: &str,
        ctx: &ConversationContext,
    ) -> Result<String, String> {
        let router = self.llm_router.as_ref().ok_or_else(|| "No LLM router".to_string())?;

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
        let system_prompt = PromptAssembler::assemble(&system_persona, &agent_persona);

        let mut messages = Vec::with_capacity(2 + ctx.recent_messages.len());
        messages.push(ChatMessage::system(&system_prompt));
        messages.extend(ctx.recent_messages.iter().cloned());
        messages.push(ChatMessage::user(query));

        let config = LoopConfig {
            max_rounds: 1,
            max_tools_per_round: 0,
            enable_caching: false,
            thinking: None,
            ..self.loop_config.clone()
        };

        let call_start = std::time::Instant::now();
        let result = run_agentic_loop_routed(
            router.as_ref(),
            messages,
            vec![], // no tools
            &config,
            None, // no sandbox
            "orchestrator",
            None, // no policy
            None,
            None,
        )
        .await;
        let latency_ms = call_start.elapsed().as_millis() as i64;

        // LLM usage tracking
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
            LoopFinishReason::Complete | LoopFinishReason::MaxRounds | LoopFinishReason::Truncated => "success",
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

        self.llm_metadata_map.insert(
            request_id,
            super::LlmMetadata {
                model: actual_model.to_string(),
                tokens_in: result.total_input_tokens,
                tokens_out: result.total_output_tokens,
            },
        );

        if let LoopFinishReason::Error(ref err) = result.finish_reason
            && result.final_content.trim().is_empty()
        {
            return Err(format!("LLM error: {}", err));
        }

        let response = result.final_content;
        self.bus.publish(SystemEvent::AgentResponse {
            request_id,
            agent_id: "orchestrator".to_string(),
            content: response.clone(),
            timestamp: Utc::now(),
        });

        Ok(response)
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
    use super::{detect_active_send_hints, sanitize_parts_for_dispatch};
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
            "I can help you send a message via Telegram using the send tool.",
        )];
        let hints = detect_active_send_hints(&messages, &[]);
        assert!(hints.send);
    }

    #[test]
    fn detect_send_flow_no_channel_no_send() {
        let messages = vec![ChatMessage::assistant(
            "Sure, I'll help you with that task.",
        )];
        let hints = detect_active_send_hints(&messages, &[]);
        assert!(!hints.send);
    }

    #[test]
    fn detect_send_flow_empty_messages() {
        assert!(!detect_active_send_hints(&[], &[]).send);
    }

    #[test]
    fn detect_send_flow_user_only_messages() {
        let messages = vec![ChatMessage::user(
            "send message to telegram",
        )];
        let hints = detect_active_send_hints(&messages, &[]);
        assert!(!hints.send);
    }

    #[test]
    fn detect_send_flow_channel_without_send_keyword() {
        let messages = vec![ChatMessage::assistant(
            "Telegram is a messaging platform.",
        )];
        let hints = detect_active_send_hints(&messages, &[]);
        assert!(!hints.send);
    }

    #[test]
    fn detect_send_flow_chinese_context_without_tool_name() {
        let messages = vec![ChatMessage::assistant(
            "好的，我将通过Telegram发送消息给您的联系人。",
        )];
        let hints = detect_active_send_hints(&messages, &[]);
        assert!(!hints.send);
    }

    #[test]
    fn detect_send_flow_chinese_context_with_tool_name() {
        let messages = vec![ChatMessage::assistant(
            "好的，我将通过Telegram使用`send`工具发送消息。",
        )];
        let hints = detect_active_send_hints(&messages, &[]);
        assert!(hints.send);
    }

    #[test]
    fn detect_send_flow_only_recent_messages() {
        let mut messages = Vec::new();
        messages.push(ChatMessage::assistant(
            "I'll send via Telegram using `send`.",
        ));
        for _ in 0..2 {
            messages.push(ChatMessage::assistant("Here is some other info."));
        }
        let hints = detect_active_send_hints(&messages, &[]);
        assert!(!hints.send);
    }

    #[test]
    fn detect_send_flow_survives_bursty_user_messages() {
        let messages = vec![
            ChatMessage::assistant("你的 Telegram 收件人是？"),
            ChatMessage::user("等一下"),
            ChatMessage::user("用 default"),
        ];
        let hints = detect_active_send_hints(&messages, &[]);
        assert!(hints.send);
    }

    #[test]
    fn detect_send_flow_discussion_about_telegram_no_tool() {
        let messages = vec![ChatMessage::assistant(
            "Telegram is great for groups and channels.",
        )];
        let hints = detect_active_send_hints(&messages, &[]);
        assert!(!hints.send);
    }

    #[test]
    fn detect_send_flow_tier2_chinese_recipient_solicitation() {
        let messages = vec![ChatMessage::assistant(
            "你的 Telegram 收件人是？",
        )];
        let hints = detect_active_send_hints(&messages, &[]);
        assert!(hints.send);
    }

    #[test]
    fn detect_send_flow_tier2_english_recipient_solicitation() {
        let messages = vec![ChatMessage::assistant(
            "Who should I send it to on Telegram?",
        )];
        let hints = detect_active_send_hints(&messages, &[]);
        assert!(hints.send);
    }

    #[test]
    fn detect_send_flow_tier2_no_match_without_recipient_keyword() {
        let messages = vec![ChatMessage::assistant(
            "好的，我将通过Telegram发送消息给您的联系人。",
        )];
        let hints = detect_active_send_hints(&messages, &[]);
        assert!(!hints.send);
    }

    #[test]
    fn detect_send_flow_with_quoted_send_tool_name() {
        let messages = vec![ChatMessage::assistant(
            "I'll use the \"send\" tool to send your photo via Telegram.",
        )];
        let hints = detect_active_send_hints(&messages, &[]);
        assert!(hints.send);
    }

    #[test]
    fn detect_send_flow_send_parens() {
        let messages = vec![ChatMessage::assistant(
            "Let me call send( for your iMessage attachment.",
        )];
        let hints = detect_active_send_hints(&messages, &[]);
        assert!(hints.send);
    }

    #[test]
    fn keepalive_recipient_followup_with_send_tool() {
        let messages = vec![ChatMessage::assistant(
            "I'll use the \"send\" tool to send your photo via Telegram. What's the recipient?",
        )];
        let hints = detect_active_send_hints(&messages, &[]);
        assert!(hints.send);
    }

    #[test]
    fn keepalive_recipient_followup_with_call_send() {
        let messages = vec![ChatMessage::assistant(
            "I'll call send to send your text via Telegram. Who should I send it to?",
        )];
        let hints = detect_active_send_hints(&messages, &[]);
        assert!(hints.send);
    }

    #[test]
    fn keepalive_send_tool_mentioned_twice() {
        let messages = vec![ChatMessage::assistant(
            "I can use send for text or call send for attachments via Telegram.",
        )];
        let hints = detect_active_send_hints(&messages, &[]);
        assert!(hints.send);
    }

    #[test]
    fn keepalive_recipient_only_defaults_to_send() {
        let messages = vec![ChatMessage::assistant(
            "你的 Telegram 收件人是？",
        )];
        let hints = detect_active_send_hints(&messages, &[]);
        assert!(hints.send);
    }

    #[test]
    fn keepalive_tier2_cross_message_backtrack() {
        let messages = vec![
            ChatMessage::assistant(
                "I'll use the \"send\" tool to send your photo via Telegram.",
            ),
            ChatMessage::user("等一下"),
            ChatMessage::assistant(
                "好的，你的 Telegram 收件人是？",
            ),
        ];
        let hints = detect_active_send_hints(&messages, &[]);
        assert!(hints.send);
    }

    #[test]
    fn tier2_cross_channel_still_detects_send() {
        // Both turns mention send-related content → send should be active
        let messages = vec![
            ChatMessage::assistant(
                "I'll use the \"send\" tool to send your photo via iMessage.",
            ),
            ChatMessage::user("等一下"),
            ChatMessage::assistant(
                "好的，你的 Telegram 收件人是？",
            ),
        ];
        let hints = detect_active_send_hints(&messages, &[]);
        // Turn N-1 has "send" (Tier 1), Turn N has Tier 2 → both set send=true
        assert!(hints.send);
    }

    // --- resolve_send_tool_choice unit tests ---

    use super::resolve_send_tool_choice;
    use openalpaca_llm::ToolChoice;

    #[test]
    fn tool_choice_send_present_pins_send() {
        let choice = resolve_send_tool_choice(true);
        assert_eq!(choice, Some(ToolChoice::Tool("send".to_string())));
    }

    #[test]
    fn tool_choice_no_send_returns_none() {
        let choice = resolve_send_tool_choice(false);
        assert_eq!(choice, None);
    }

    // --- apply_send_keepalive direct tests ---

    use super::apply_send_keepalive;

    #[test]
    fn detect_send_hints_custom_channel_via_active_channels() {
        // "matrix" is NOT in FALLBACK_CHANNEL_KW
        let messages = vec![ChatMessage::assistant(
            "Your Matrix recipient is?",
        )];
        // With active_channels containing "matrix": should detect Tier 2 hit
        let active = vec!["matrix".to_string()];
        let hints = detect_active_send_hints(&messages, &active);
        assert!(hints.send);
        // Without: should NOT detect (proves dynamic path works)
        let hints_empty = detect_active_send_hints(&messages, &[]);
        assert!(!hints_empty.send);
    }

    #[test]
    fn apply_keepalive_injects_send() {
        let messages = vec![ChatMessage::assistant(
            "I'll use the \"send\" tool via Telegram.",
        )];
        let mut tool_names = vec!["web_fetch".to_string()];
        let intent_send = apply_send_keepalive(&mut tool_names, &messages, &[]);
        assert!(!intent_send);
        assert!(tool_names.contains(&"send".to_string()));
    }

    #[test]
    fn apply_keepalive_skips_when_intent_has_send() {
        let messages = vec![ChatMessage::assistant(
            "I'll use the send tool via Telegram.",
        )];
        let mut tool_names = vec!["send".to_string()];
        let intent_send = apply_send_keepalive(&mut tool_names, &messages, &[]);
        assert!(intent_send);
        // No duplicate
        assert_eq!(
            tool_names.iter().filter(|t| *t == "send").count(),
            1
        );
    }

    #[test]
    fn apply_keepalive_injects_send_when_hinted() {
        let messages = vec![ChatMessage::assistant(
            "I can use send for text or call send for attachments via Telegram.",
        )];
        let mut tool_names = vec![];
        let intent_send = apply_send_keepalive(&mut tool_names, &messages, &[]);
        assert!(!intent_send);
        assert!(tool_names.contains(&"send".to_string()));
    }

    // --- end-to-end conflict path: intent + keep-alive → tool_choice ---

    #[test]
    fn keepalive_plain_text_continuation_resolves_to_send() {
        let messages = vec![ChatMessage::assistant(
            "I'll use the \"send\" tool via Telegram.",
        )];
        let parser = crate::orchestrator::intent::IntentParser;
        let mut tool_names = parser.suggest_tools("好的，发吧");
        let _intent_send = apply_send_keepalive(&mut tool_names, &messages, &[]);

        let has_send = tool_names.contains(&"send".to_string());
        let choice = resolve_send_tool_choice(has_send);
        assert_eq!(choice, Some(ToolChoice::Tool("send".to_string())));
    }

    #[test]
    fn keepalive_text_send_continuation_resolves_to_send() {
        let messages = vec![ChatMessage::assistant(
            "好的，我将通过Telegram使用`send`工具发送消息。",
        )];
        let parser = crate::orchestrator::intent::IntentParser;
        let mut tool_names = parser.suggest_tools("发消息给他");
        let _intent_send = apply_send_keepalive(&mut tool_names, &messages, &[]);

        let has_send = tool_names.contains(&"send".to_string());
        let choice = resolve_send_tool_choice(has_send);
        assert_eq!(choice, Some(ToolChoice::Tool("send".to_string())));
    }

    #[test]
    fn apply_keepalive_cross_channel_injects_send() {
        let messages = vec![
            ChatMessage::assistant(
                "I'll use the \"send\" tool to send your photo via iMessage.",
            ),
            ChatMessage::user("等一下"),
            ChatMessage::assistant(
                "好的，你的 Telegram 收件人是？",
            ),
        ];
        let mut tool_names = vec!["web_fetch".to_string()];
        let intent_send = apply_send_keepalive(&mut tool_names, &messages, &[]);
        assert!(!intent_send);
        assert!(tool_names.contains(&"send".to_string()));
        assert_eq!(tool_names.len(), 2); // web_fetch + send
    }
}
