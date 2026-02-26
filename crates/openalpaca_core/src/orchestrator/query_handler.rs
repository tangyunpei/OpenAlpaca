use super::{ConversationContext, Orchestrator};
use crate::events::SystemEvent;
use crate::memory::scope_context::MemoryScopeContext;
use crate::middleware::bootstrap::bootstrap_to_prompt_block;
use crate::middleware::guard::OutputGuard;
use crate::middleware::identity::identity_to_prompt_block;
use crate::middleware::prompt::{AgentPersona, PromptAssembler, format_tool_guidance};
use crate::middleware::user::user_to_prompt_block;
use crate::runner::{LoopConfig, LoopFinishReason, run_agentic_loop_routed};
use crate::security::sandbox::SandboxManager;
use crate::security::sandbox::SandboxPolicy;
use crate::tools::{ContextualToolExecutor, ToolExecutionContext};
use chrono::Utc;
use openalpaca_llm::{ChatMessage, ContentPart, ImageSource};
use openalpaca_storage::repository::{LlmUsageRepository, MemoryRepository};
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

impl Orchestrator {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn handle_simple_query(
        &self,
        request_id: Uuid,
        _source: &str,
        query: &str,
        tool_suggestion_query: &str,
        _lane_key: &str,
        ctx: &ConversationContext,
        owner_id: Option<&str>,
        scope_ctx: &MemoryScopeContext,
        current_parts: Option<&[ContentPart]>,
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

        // Resolve tools based on intent analysis
        // Tool suggestion should be based on raw user intent text, not attachment-injected context.
        let mut tool_names = self.intent_parser.suggest_tools(tool_suggestion_query);

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
    use super::sanitize_parts_for_dispatch;
    use openalpaca_llm::{ContentPart, ImageSource};

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
}
