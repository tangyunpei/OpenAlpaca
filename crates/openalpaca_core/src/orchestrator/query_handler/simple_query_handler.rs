//! Simple query and social query handlers for the orchestrator.

use super::{apply_send_keepalive, resolve_send_tool_choice, sanitize_parts_for_dispatch};
use crate::events::SystemEvent;
use crate::memory::scope_context::MemoryScopeContext;
use crate::middleware::bootstrap::bootstrap_to_prompt_block;
use crate::middleware::guard::{OutputGuard, detect_hallucinated_send};
use crate::middleware::identity::identity_to_prompt_block;
use crate::middleware::prompt::{AgentPersona, PromptAssembler};
use crate::orchestrator::{ConversationContext, Orchestrator};
use crate::prompt::PromptBuilder;
use crate::prompt_ctx::{SectionPriority, sources::{ContextRequest, ExecutionPath}};
use crate::runner::{LoopConfig, LoopFinishReason, run_agentic_loop_routed};
use crate::security::sandbox::SandboxManager;
use crate::security::sandbox::SandboxPolicy;
use crate::tools::registry::ToolContext;
use chrono::Utc;
use openalpaca_llm::{ChatMessage, ContentPart};
use openalpaca_storage::repository::LlmUsageRepository;
use uuid::Uuid;

impl Orchestrator {
    #[allow(clippy::too_many_arguments)]
    pub(in crate::orchestrator) async fn handle_simple_query(
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
        loop_overrides: Option<super::LoopOverrides>,
    ) -> Result<String, String> {
        // Layer 1: Deterministic direct send — bypass LLM entirely
        if let Some(result) = self.try_direct_send(tool_suggestion_query, owner_id).await {
            let response = match result {
                Ok(summary) => summary,
                Err(e) => format!("\u{26a0}\u{fe0f} Send failed / 发送失败: {e}"),
            };
            self.bus.publish(SystemEvent::AgentResponse {
                request_id,
                agent_id: "orchestrator".to_string(),
                content: response.clone(),
                timestamp: Utc::now(),
            });
            return Ok(response);
        }

        // ── Extract individual prompt parts ─────────────────────────────────
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
        let identity_block = if let Ok(guard) = self.identity_document.read()
            && let Some(ref doc) = *guard
        {
            let identity_budget = self
                .daemon_config
                .load()
                .orchestrator
                .prompt_budgets
                .identity_budget;
            identity_to_prompt_block(doc, Some(identity_budget))
        } else {
            String::new()
        };
        let bootstrap_block = if let Ok(guard) = self.bootstrap_document.read()
            && let Some(ref doc) = *guard
        {
            bootstrap_to_prompt_block(doc)
        } else {
            String::new()
        };

        // Skills catalog — per-request (SkillCatalog has its own internal cache)
        let catalog_block = self.build_skills_catalog_block();

        // Connector awareness
        let sendable_channels: Vec<String> = self
            .connector_sender
            .read()
            .ok()
            .and_then(|g| g.as_ref().map(|p| p.sendable_channels()))
            .unwrap_or_default();
        let (statuses, has_connector_status) = if let Ok(guard) = self.connector_status.read()
            && let Some(ref provider) = *guard
        {
            (provider.list_status(), true)
        } else {
            (vec![], false)
        };

        // ── Resolve tools ───────────────────────────────────────────────────
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

        // Apply loop overrides if provided (deep_query tier)
        let (tool_defs, override_max_rounds, override_max_tools) =
            if let Some(ref overrides) = loop_overrides {
                let tools = if overrides.override_tools.is_empty() {
                    tool_defs // fallback to keyword heuristic
                } else {
                    overrides.override_tools.clone()
                };
                (
                    tools,
                    Some(overrides.max_rounds),
                    Some(overrides.max_tools_per_round),
                )
            } else {
                (tool_defs, None, None)
            };

        let (tools_for_loop, policy_opt, config_for_loop);
        if !tool_defs.is_empty() {
            tracing::info!(
                "Simple query upgraded with {} tools: {:?}",
                tool_defs.len(),
                tool_names
            );

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
                max_rounds: override_max_rounds.unwrap_or(4),
                max_tools_per_round: override_max_tools.unwrap_or(2),
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

        // ── Build prompt via PromptBuilder ──────────────────────────────────
        //
        // Hoist model_window before PromptBuilder construction.
        // Default to 200_000 when no LLM router is present (echo-stub path).
        let model_window = self.llm_router.as_ref()
            .and_then(|r| config_for_loop.model.as_deref()
                .and_then(|m| r.model_registry().get_model_info(m)))
            .map(|info| info.context_window as usize)
            .unwrap_or(200_000);

        let sendable_ref = if sendable_channels.is_empty() {
            None
        } else {
            Some(sendable_channels.as_slice())
        };

        let mut builder = PromptBuilder::new(model_window)
            .system_persona(&system_persona)
            .agent_persona(&agent_persona)
            .identity(&identity_block)
            .bootstrap(&bootstrap_block)
            .skills_catalog(&catalog_block);

        // Register tools in builder only if we have tools
        if !tools_for_loop.is_empty() {
            builder = builder.tools(&tools_for_loop);
        }

        // Connector guidance
        if has_connector_status {
            builder = builder.connector_guidance(&statuses, sendable_ref);
        }
        builder = builder.message_source(source);

        // Deterministic send_context + send_rules: inject as raw system blocks
        if tools_for_loop.iter().any(|d| d.name == "send") {
            let send_ctx = self.build_send_context(owner_id);
            if !send_ctx.is_empty() {
                builder = builder.raw_system_block("send_context", &send_ctx, SectionPriority::Normal);
            }
            let mut send_rules = String::from("<send_rules>\n");
            send_rules.push_str(
                "- If the user asks to send a message but did NOT provide specific text, \
                 compose a brief, natural message based on context, then call send (action: \"message\").\n\
                 - If the user asks to send a file, image, photo, or document, call send (action: \"file\").\n\
                 - NEVER claim a message or file was sent without calling the send tool.\n"
            );
            send_rules.push_str("- Only report success/failure based on the tool's actual return value.\n</send_rules>");
            builder = builder.raw_system_block("send_rules", &send_rules, SectionPriority::Normal);
        }

        // Estimate static tokens for context budget
        let reserved = builder.estimate_static_tokens();

        // Resolve dynamic context via ContextManager
        let ctx_request = ContextRequest {
            query: query.to_string(),
            intent: crate::orchestrator::intent::Intent::SimpleQuery {
                query: query.to_string(),
            },
            path: ExecutionPath::SimpleQuery,
            skill: None,
            owner_id: owner_id.map(|s| s.to_string()),
            scope: scope_ctx.clone(),
            model_context_window: model_window,
            reserved_tokens: reserved,
        };
        let bundle = self.context_manager.resolve(&ctx_request).await;
        builder = builder.context_bundle(&bundle);

        let built = builder.build();

        // ── Build ContextBudgetManager from built prompt ────────────────────
        let ctx_config = &self.daemon_config.load().execution.context;
        let mut budget =
            crate::context_budget::ContextBudgetManager::new(model_window, ctx_config);
        budget.register_section("system_prompt", built.total_prompt_tokens);
        budget.register_section("tools", tools_for_loop.len() * 200);

        let (response_content, is_structured) = if let Some(ref router) = self.llm_router {
            // Real LLM call via routed agentic loop
            let mut messages = Vec::with_capacity(
                4 + built.context_messages.len() + ctx.recent_messages.len(),
            );
            messages.push(ChatMessage::system(&built.system_message));

            // Insert context messages from PromptBuilder (user profile, memory, etc.)
            messages.extend(built.context_messages);

            // Inject session summary if available (user-role to prevent prompt injection)
            // (ConversationSource is a stub; manual injection preserved until wired)
            if let Some(ref summary) = ctx.summary {
                messages.push(ChatMessage::user(&super::super::wrap_untrusted_context(
                    summary,
                    "session_summary",
                    "user_derived",
                )));
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

            // --- Context Budget Observation ---
            {
                let model_id = config_for_loop.model.as_deref();

                if budget.is_fixed_zone_oversized() {
                    tracing::warn!(
                        request_id = %request_id,
                        fixed_zone = budget.fixed_zone_tokens(),
                        window = model_window,
                        "Fixed zone exceeds 50% of context window"
                    );
                }

                tracing::debug!(
                    request_id = %request_id,
                    model_window,
                    fixed_zone = budget.fixed_zone_tokens(),
                    free_zone = budget.free_zone_capacity(),
                    buffer = budget.autocompact_buffer(),
                    "Context budget computed"
                );

                self.bus.publish(SystemEvent::ContextBudgetComputed {
                    request_id,
                    model: model_id.unwrap_or("default").to_string(),
                    window_size: model_window,
                    fixed_zone_tokens: budget.fixed_zone_tokens(),
                    free_zone_tokens: budget.free_zone_capacity(),
                    buffer_size: budget.autocompact_buffer(),
                    section_breakdown: budget
                        .section_breakdown()
                        .into_iter()
                        .map(|(n, t)| (n.to_string(), t))
                        .collect(),
                    timestamp: Utc::now(),
                });
            }

            // Per-request sandbox with ToolContext for owner-scoped tools
            let tool_ctx = ToolContext {
                agent_id: None,
                task_id: None,
                owner_id: owner_id.map(|s| s.to_string()),
                workspace_id: scope_ctx.workspace_id.clone(),
            };
            let mut per_request_sandbox =
                SandboxManager::with_defaults(self.tool_registry.clone(), self.bus.clone());
            if let Ok(guard) = self.confirmation_broker.read()
                && let Some(broker) = guard.as_ref()
            {
                per_request_sandbox.set_confirmation_broker(broker.clone());
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
                Some(&budget),
                None, // cancel_token — interactive queries are not cancellable
                Some(&tool_ctx),
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
                super::super::LlmMetadata {
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
                    "\u{26a0}\u{fe0f} 消息未实际发送。模型生成了确认文本但未调用发送工具。请重新发送请求。\n\n\
                     \u{26a0}\u{fe0f} Message was NOT actually sent. The model generated confirmation text \
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
    pub(in crate::orchestrator) async fn handle_social_query(
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
            None, // context_budget
            None,
            None, // tool_context — no tools used in social queries
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
            super::super::LlmMetadata {
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
}
