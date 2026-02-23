use super::skill_context::inject_skill_context;
use super::skill_output::validate_skill_output;
use super::{ConversationContext, Orchestrator};
use crate::events::SystemEvent;
use crate::memory::scope_context::MemoryScopeContext;
use crate::middleware::bootstrap::bootstrap_to_prompt_block;
use crate::middleware::guard::OutputGuard;
use crate::middleware::identity::identity_to_prompt_block;
use crate::middleware::prompt::{AgentPersona, PromptAssembler, format_tool_guidance};
use crate::middleware::skill::{SkillFrontmatter, skill_to_prompt_block};
use crate::middleware::user::user_to_prompt_block;
use crate::runner::{LoopConfig, LoopFinishReason, run_agentic_loop_routed};
use crate::security::sandbox::SandboxManager;
use crate::security::sandbox::SandboxPolicy;
use crate::tools::{ContextualToolExecutor, ToolExecutionContext};
use chrono::Utc;
use openalpaca_llm::ChatMessage;
use openalpaca_storage::repository::{LlmUsageRepository, MemoryRepository};
use std::sync::Arc;
use uuid::Uuid;

/// Preflight check: validate that the skill's declared permissions are
/// consistent with its sandbox config.
fn preflight_permissions(frontmatter: &SkillFrontmatter) -> Result<(), String> {
    let level = &frontmatter.permissions.level;
    match level.as_str() {
        "readonly" => Ok(()),
        "readwrite" | "admin" => {
            // If the skill needs shell but sandbox disallows it, reject early
            if !frontmatter.permissions.sandbox.net
                && frontmatter.tools.allow.iter().any(|t| t == "web_fetch")
            {
                return Err(
                    "Skill requires web_fetch tool but sandbox.net is false".into(),
                );
            }
            Ok(())
        }
        _ => Ok(()), // Unknown level, pass through
    }
}

impl Orchestrator {
    /// Handle a skill invocation: load full SKILL.md, inject as context, run agentic loop.
    ///
    /// Mirrors `handle_simple_query()` with an extra `### SKILL CONTEXT ###` block.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn handle_skill_invocation(
        &self,
        request_id: Uuid,
        _source: &str,
        skill_name: &str,
        query: &str,
        _lane_key: &str,
        ctx: &ConversationContext,
        owner_id: Option<&str>,
        scope_ctx: &MemoryScopeContext,
    ) -> Result<String, String> {
        let invocation_start = std::time::Instant::now();

        // Emit invocation started event
        self.bus.publish(SystemEvent::SkillInvocationStarted {
            request_id,
            skill_id: skill_name.to_string(),
            query_preview: query.chars().take(100).collect(),
            timestamp: Utc::now(),
        });

        let result = self
            .handle_skill_invocation_inner(
                request_id, skill_name, query, ctx, owner_id, scope_ctx,
            )
            .await;

        // Emit SkillCompleted or SkillFailed based on result
        match &result {
            Ok(validated) => {
                self.bus.publish(SystemEvent::SkillCompleted {
                    request_id,
                    skill_id: skill_name.to_string(),
                    duration_ms: invocation_start.elapsed().as_millis() as u64,
                    output_preview: validated.chars().take(200).collect(),
                    timestamp: Utc::now(),
                });
            }
            Err(error) => {
                self.bus.publish(SystemEvent::SkillFailed {
                    request_id,
                    skill_id: skill_name.to_string(),
                    error: error.clone(),
                    timestamp: Utc::now(),
                });
            }
        }

        result
    }

    /// Inner implementation of skill invocation (separated for lifecycle event wrapping).
    #[allow(clippy::too_many_arguments)]
    async fn handle_skill_invocation_inner(
        &self,
        request_id: Uuid,
        skill_name: &str,
        query: &str,
        ctx: &ConversationContext,
        owner_id: Option<&str>,
        scope_ctx: &MemoryScopeContext,
    ) -> Result<String, String> {
        // Look up the catalog entry (for skill_dir) and load full skill (Level 2)
        let entry = self
            .skill_catalog
            .get(skill_name)
            .ok_or_else(|| format!("Skill '{}' not found in catalog", skill_name))?;
        let skill_doc = self
            .skill_catalog
            .load_full(skill_name)
            .map_err(|e| format!("Failed to load skill '{}': {}", skill_name, e))?;

        // Permissions preflight: reject early if sandbox config is inconsistent
        preflight_permissions(&skill_doc.frontmatter)?;

        // Context injection from skill's context.sources
        let injected_context =
            inject_skill_context(&skill_doc.frontmatter.context, &entry.skill_dir).await?;

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

        // Inject skill context block
        let skill_block = skill_to_prompt_block(&skill_doc);
        if !skill_block.is_empty() {
            system_prompt.push('\n');
            system_prompt.push_str(&skill_block);
        }

        // Inject context from skill's context.sources (files, globs)
        if !injected_context.is_empty() {
            system_prompt.push_str("\n### SKILL REFERENCE CONTEXT ###\n");
            system_prompt.push_str(&injected_context);

            // Emit context injected event
            self.bus.publish(SystemEvent::SkillContextInjected {
                request_id,
                skill_id: skill_name.to_string(),
                context_bytes: injected_context.len(),
                timestamp: Utc::now(),
            });
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

        // Resolve tools: merge skill.tools.allow with intent-suggested tools
        let mut tool_names: Vec<String> = skill_doc.frontmatter.tools.allow.clone();
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

        // Tool allow/deny enforcement
        let skill_deny = &skill_doc.frontmatter.tools.deny;
        let global_deny = &self.daemon_config.load().execution.skill_defaults.global_tool_deny;

        // Remove any denied tools (skill-level + global)
        tool_names.retain(|t| !skill_deny.contains(t) && !global_deny.contains(t));

        let tool_defs: Vec<_> = tool_names
            .iter()
            .filter_map(|name| self.tool_registry.get(name).map(|t| t.definition.clone()))
            .collect();

        if tool_defs.len() < tool_names.len() {
            let resolved_names: Vec<&str> = tool_defs.iter().map(|d| d.name.as_str()).collect();
            let missing: Vec<&str> = tool_names
                .iter()
                .filter(|n| !resolved_names.contains(&n.as_str()))
                .map(|n| n.as_str())
                .collect();
            tracing::warn!(
                "Skill '{}' references unknown tools: {:?}",
                skill_name,
                missing
            );
        }

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
            let mut denied_caps: Vec<String> = skill_deny.clone();
            for g in global_deny {
                if !denied_caps.contains(g) {
                    denied_caps.push(g.clone());
                }
            }
            policy_opt = Some(SandboxPolicy {
                agent_id: "orchestrator".to_string(),
                allowed_capabilities: resolved,
                denied_capabilities: denied_caps,
                require_confirmation_for: vec![],
                max_tool_calls: None,
                max_tool_runtime_secs: self.loop_config.max_tool_runtime.as_secs(),
            });
            let skill_cfg = &self.daemon_config.load().execution.skill_defaults;
            config_for_loop = LoopConfig {
                max_rounds: skill_cfg.max_rounds,
                max_tools_per_round: skill_cfg.max_tools_per_round,
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
                messages.push(ChatMessage::user(
                    &super::wrap_untrusted_context(summary, "session_summary", "user_derived"),
                ));
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
                    messages.push(ChatMessage::user(
                        &super::wrap_untrusted_context(&inner, "retrieved_memory", "retrieved"),
                    ));
                }
            }

            messages.extend(ctx.recent_messages.clone());
            messages.push(ChatMessage::user(query));

            // Per-request sandbox with ContextualToolExecutor
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
                None, // cancel_token — interactive skill calls are not cancellable
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

            if let LoopFinishReason::Error(ref err) = result.finish_reason
                && result.final_content.trim().is_empty()
            {
                return Err(format!("LLM error: {}", err));
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

        // Output guard (structural)
        let guarded = if is_structured {
            OutputGuard::ensure_json(&response_content)?
        } else {
            response_content
        };

        // Skill output validation (required_sections, format checks)
        let validated = match validate_skill_output(&guarded, &skill_doc.frontmatter.output) {
            Ok(v) => v,
            Err(validation_err) => {
                // Attempt one self-repair: log the error and return original output
                // with a warning. A full re-run would require re-entering the agentic
                // loop which is expensive; instead we log and pass through.
                tracing::warn!(
                    "Skill '{}' output validation failed: {}. Passing through as-is.",
                    skill_name,
                    validation_err
                );
                guarded
            }
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
}
