//! Simple query and social query handlers for the orchestrator.

use super::{apply_send_keepalive, resolve_send_tool_choice, sanitize_parts_for_dispatch};
use crate::events::SystemEvent;
use crate::memory::scope_context::MemoryScopeContext;
use crate::middleware::guard::{OutputGuard, detect_hallucinated_send};
use crate::middleware::prompt::{
    format_connector_guidance, format_message_source, format_tool_guidance, AgentPersona,
    PromptAssembler,
};
use crate::middleware::user::user_to_prompt_block;
use crate::orchestrator::{ConversationContext, Orchestrator};
use crate::runner::{LoopConfig, LoopFinishReason, run_agentic_loop_routed};
use crate::security::sandbox::SandboxManager;
use crate::security::sandbox::SandboxPolicy;
use crate::tools::{ContextualToolExecutor, ToolExecutionContext};
use chrono::Utc;
use openalpaca_llm::{ChatMessage, ContentPart};
use openalpaca_storage::repository::{LlmUsageRepository, MemoryRepository};
use std::sync::Arc;
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
                messages.push(ChatMessage::user(&super::super::wrap_untrusted_context(
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
                    messages.push(ChatMessage::user(&super::super::wrap_untrusted_context(
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
