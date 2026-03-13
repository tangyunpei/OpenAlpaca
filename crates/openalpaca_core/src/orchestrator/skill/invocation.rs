//! Core skill invocation implementation: tool resolution, prompt building,
//! agentic loop execution, output validation and repair.

use super::context::inject_skill_context;
use super::handler::SkillInvocationResult;
use super::invoke_executor::{SkillInvocationBuiltInAdapter, SkillInvocationToolExecutor};
use super::output::{deterministic_repair, validate_skill_output};
use super::preflight::preflight_permissions;
use crate::events::SystemEvent;
use crate::memory::scope_context::MemoryScopeContext;
use crate::middleware::bootstrap::bootstrap_to_prompt_block;
use crate::middleware::guard::{OutputGuard, detect_hallucinated_send};
use crate::middleware::identity::identity_to_prompt_block;
use crate::middleware::prompt::AgentPersona;
use crate::middleware::skill::skill_to_prompt_block;
use crate::orchestrator::{ConversationContext, Orchestrator};
use crate::prompt::PromptBuilder;
use crate::prompt_ctx::sources::{ContextRequest, ExecutionPath};
use crate::prompt_ctx::SectionPriority;
use crate::runner::{LoopConfig, LoopFinishReason, run_agentic_loop_routed};
use crate::security::sandbox::SandboxManager;
use crate::security::sandbox::SandboxPolicy;
use crate::tools::builtins::ScriptToolBuiltIn;
use crate::tools::registry::{RegisteredTool, ToolBackend, ToolContext};
use chrono::Utc;
use openalpaca_llm::{ChatMessage, ToolChoice};
use openalpaca_storage::repository::LlmUsageRepository;
use std::sync::Arc;
use uuid::Uuid;

impl Orchestrator {
    /// Inner implementation of skill invocation (separated for lifecycle event wrapping).
    #[allow(clippy::too_many_arguments)]
    pub(in crate::orchestrator) async fn handle_skill_invocation_inner(
        &self,
        request_id: Uuid,
        source: &str,
        skill_name: &str,
        query: &str,
        ctx: &ConversationContext,
        owner_id: Option<&str>,
        scope_ctx: &MemoryScopeContext,
        stream_id: Option<&str>,
    ) -> Result<SkillInvocationResult, String> {
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

        // ── Build prompt via PromptBuilder ──────────────────────────────────
        //
        // Determine model context window for budget calculations.
        let model_window = self
            .llm_router
            .as_ref()
            .and_then(|r| {
                let default = r.default_model();
                r.model_registry().get_model_info(&default)
            })
            .map(|info| info.context_window as usize)
            .unwrap_or(200_000);

        // Extract prompt components
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

        let skill_block = skill_to_prompt_block(&skill_doc);

        let mut builder = PromptBuilder::new(model_window)
            .system_persona(&system_persona)
            .agent_persona(&agent_persona)
            .identity(&identity_block)
            .bootstrap(&bootstrap_block);

        // Skill block (High priority — this is an active skill invocation)
        if !skill_block.is_empty() {
            builder =
                builder.raw_system_block("skill_body", &skill_block, SectionPriority::High);
        }

        // Injected skill context sources (Normal)
        if !injected_context.is_empty() {
            builder = builder.raw_system_block(
                "skill_context",
                &injected_context,
                SectionPriority::Normal,
            );

            self.bus.publish(SystemEvent::SkillContextInjected {
                request_id,
                skill_id: skill_name.to_string(),
                context_bytes: injected_context.len(),
                timestamp: Utc::now(),
            });
        }

        // Message source
        builder = builder.message_source(source);

        // Resolve tools via capability model, falling back to legacy name-based matching.
        // Intent-suggested tools are intentionally NOT merged here to maintain
        // skill-level tool isolation (P1-1 security fix).
        let mut tool_defs: Vec<openalpaca_llm::ToolDefinition> =
            if !skill_doc.frontmatter.requires_capabilities.is_empty() {
                // New path: capability-based resolution
                self.tool_registry
                    .tools_for_capabilities(&skill_doc.frontmatter.requires_capabilities)
            } else if !skill_doc.frontmatter.tools.allow.is_empty() {
                // Legacy fallback: direct tool name matching
                let names = &skill_doc.frontmatter.tools.allow;
                let resolved: Vec<openalpaca_llm::ToolDefinition> = names
                    .iter()
                    .filter_map(|name| {
                        self.tool_registry.get(name).map(|t| t.definition.clone())
                    })
                    .collect();
                if resolved.len() < names.len() {
                    let resolved_names: Vec<&str> =
                        resolved.iter().map(|d| d.name.as_str()).collect();
                    let missing: Vec<&str> = names
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
                resolved
            } else {
                vec![]
            };

        // Force-include persona tools during bootstrap mode
        if self.is_bootstrapping() {
            for name in &["update_persona"] {
                if !tool_defs.iter().any(|d| &d.name == name) {
                    if let Some(t) = self.tool_registry.get(name) {
                        tool_defs.push(t.definition.clone());
                    }
                }
            }
        }

        // Apply deny list (both paths)
        let skill_deny = &skill_doc.frontmatter.tools.deny;
        let global_deny = &self
            .daemon_config
            .load()
            .execution
            .skill_defaults
            .global_tool_deny;

        tool_defs.retain(|t| !skill_deny.contains(&t.name) && !global_deny.contains(&t.name));

        // Resolve script tools from skill's scripts/ directory
        let script_tool_defs: Vec<openalpaca_llm::ToolDefinition> = skill_doc
            .frontmatter
            .scripts
            .iter()
            .map(|s| s.to_tool_definition())
            .collect();
        let script_tool_names: Vec<String> =
            script_tool_defs.iter().map(|d| d.name.clone()).collect();
        if !script_tool_defs.is_empty() {
            tracing::info!(
                "Skill '{}' has {} script tools: {:?}",
                skill_name,
                script_tool_defs.len(),
                script_tool_names
            );
            tool_defs.extend(script_tool_defs);
        }

        // Add invoke_skill:* synthetic tools (from depends_on)
        for dep_id in &skill_doc.frontmatter.depends_on {
            if let Some(dep_entry) = self.skill_catalog.get(dep_id) {
                tool_defs.push(openalpaca_llm::ToolDefinition {
                    name: format!("invoke_skill:{}", dep_id),
                    description: format!(
                        "Invoke the '{}' skill: {}",
                        dep_entry.frontmatter.name, dep_entry.frontmatter.description
                    ),
                    parameters: serde_json::json!({
                        "type": "object",
                        "properties": {
                            "query": {
                                "type": "string",
                                "description": "The input/query to pass to the skill"
                            }
                        },
                        "required": ["query"]
                    }),
                    strict: None,
                    input_examples: None,
                });
            } else {
                tracing::warn!(
                    "Skill '{}' depends on '{}' which is not in catalog",
                    skill_doc.frontmatter.name,
                    dep_id
                );
            }
        }

        // Connector guidance via PromptBuilder
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
            let sendable_ref = if sendable_channels.is_empty() {
                None
            } else {
                Some(sendable_channels.as_slice())
            };
            builder = builder.connector_guidance(&statuses, sendable_ref);
        }

        let (tools_for_loop, policy_opt, config_for_loop);
        if !tool_defs.is_empty() {
            let tool_names_log: Vec<&str> =
                tool_defs.iter().map(|d| d.name.as_str()).collect();
            tracing::info!(
                "Skill invocation '{}' with {} tools: {:?}",
                skill_name,
                tool_defs.len(),
                tool_names_log
            );
            // Register tools in builder
            builder = builder.tools(&tool_defs);

            // Inject factual send_context when send is available
            if tool_defs.iter().any(|d| d.name == "send") {
                let send_ctx = self.build_send_context(owner_id);
                if !send_ctx.is_empty() {
                    builder = builder.raw_system_block(
                        "send_context",
                        &send_ctx,
                        SectionPriority::Normal,
                    );
                }
            }
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
                require_confirmation_for: skill_doc
                    .frontmatter
                    .permissions
                    .confirm
                    .tools
                    .clone(),
                max_tool_calls: skill_doc
                    .frontmatter
                    .tools
                    .rate_limit
                    .max_calls
                    .map(|n| n as u32),
                max_tool_runtime_secs: self.loop_config.max_tool_runtime.as_secs(),
                stream_id: stream_id.map(|s| s.to_string()),
                lane_key: None,
                confirmation_timeout_secs: None,
                auto_approve: self
                    .daemon_config
                    .load()
                    .security
                    .auto_approve_confirmations,
            });
            let skill_cfg = &self.daemon_config.load().execution.skill_defaults;
            config_for_loop = LoopConfig {
                max_rounds: skill_cfg.max_rounds,
                max_tools_per_round: skill_cfg.max_tools_per_round,
                initial_tool_choice: if tool_defs.iter().any(|d| d.name == "send") {
                    Some(ToolChoice::Tool("send".to_string()))
                } else {
                    None
                },
                enable_caching: true,
                thinking: None,
                ..self.loop_config.clone()
            };
            tools_for_loop = tool_defs;
        } else {
            tools_for_loop = vec![];
            policy_opt = None;
            config_for_loop = self.loop_config.clone();
        }

        // ── Resolve dynamic context via ContextManager ─────────────────────
        let reserved = builder.estimate_static_tokens();
        let ctx_request = ContextRequest {
            query: query.to_string(),
            intent: crate::orchestrator::intent::Intent::SimpleQuery {
                query: query.to_string(),
            },
            path: ExecutionPath::SkillInvocation {
                skill_id: skill_name.to_string(),
            },
            skill: Some(Arc::new(skill_doc.clone())),
            owner_id: owner_id.map(|s| s.to_string()),
            scope: scope_ctx.clone(),
            model_context_window: model_window,
            reserved_tokens: reserved,
        };
        let bundle = self.context_manager.resolve(&ctx_request).await;
        builder = builder.context_bundle(&bundle);

        let built = builder.build();

        // Build ContextBudgetManager for agentic loop
        let ctx_config = &self.daemon_config.load().execution.context;
        let mut budget =
            crate::context_budget::ContextBudgetManager::new(model_window, ctx_config);
        budget.register_section("system_prompt", built.total_prompt_tokens);
        budget.register_section("tools", tools_for_loop.len() * 200);

        // --- Context Budget Telemetry ---
        {
            let model_id = self.llm_router.as_ref()
                .map(|r| r.default_model())
                .unwrap_or_else(|| "default".to_string());

            tracing::debug!(
                request_id = %request_id,
                skill = skill_name,
                model_window,
                fixed_zone = budget.fixed_zone_tokens(),
                free_zone = budget.free_zone_capacity(),
                buffer = budget.autocompact_buffer(),
                "Context budget computed (skill invocation)"
            );

            self.bus.publish(SystemEvent::ContextBudgetComputed {
                request_id,
                model: model_id,
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

        // Metadata accumulators for SkillInvocationResult
        let mut inv_finish_reason = LoopFinishReason::Complete;
        let mut inv_rounds_used = 0usize;
        let mut inv_tool_calls_made = 0usize;
        let mut inv_input_tokens = 0u32;
        let mut inv_output_tokens = 0u32;
        let mut inv_cost_usd = 0.0f64;
        let mut inv_model_used: Option<String> = None;

        let (response_content, is_structured) = if let Some(ref router) = self.llm_router {
            let mut messages = Vec::with_capacity(
                4 + built.context_messages.len() + ctx.recent_messages.len(),
            );
            messages.push(ChatMessage::system(&built.system_message));

            // Insert context messages from PromptBuilder (user profile, memory, etc.)
            messages.extend(built.context_messages);

            // Inject session summary if available (user-role to prevent prompt injection)
            // (ConversationSource is a stub; manual injection preserved until wired)
            if let Some(ref summary) = ctx.summary {
                messages.push(ChatMessage::user(
                    &crate::orchestrator::wrap_untrusted_context(
                        summary,
                        "session_summary",
                        "user_derived",
                    ),
                ));
            }

            messages.extend(ctx.recent_messages.clone());
            messages.push(ChatMessage::user(query));

            // Per-request sandbox with ToolContext
            let tool_ctx = ToolContext {
                agent_id: None,
                task_id: None,
                owner_id: owner_id.map(|s| s.to_string()),
                workspace_id: scope_ctx.workspace_id.clone(),
            };
            let needs_clone = !skill_doc.frontmatter.scripts.is_empty()
                || !skill_doc.frontmatter.depends_on.is_empty();
            let registry = if needs_clone {
                let mut cloned = (*self.tool_registry).clone();
                for cfg in &skill_doc.frontmatter.scripts {
                    let tool = ScriptToolBuiltIn::new(&entry.skill_dir, cfg)?;
                    cloned.register(RegisteredTool {
                        definition: ScriptToolBuiltIn::tool_definition(&cfg.name),
                        backend: ToolBackend::BuiltIn(Arc::new(tool)),
                        provides_capabilities: vec![],
                        exempt_from_timeout: false,
                    });
                }
                // Register invoke_skill:* backends so the sandbox can execute them
                if !skill_doc.frontmatter.depends_on.is_empty() {
                    let call_stack = vec![skill_name.to_string()];
                    let executor = Arc::new(SkillInvocationToolExecutor::new(
                        self.skill_catalog.clone(),
                        self.tool_registry.clone(),
                        router.clone(),
                        self.bus.clone(),
                        call_stack,
                        3, // max nesting depth
                        None,
                    ));
                    for dep_id in &skill_doc.frontmatter.depends_on {
                        if self.skill_catalog.get(dep_id).is_some() {
                            let tool_name = format!("invoke_skill:{}", dep_id);
                            cloned.register(RegisteredTool {
                                definition: openalpaca_llm::ToolDefinition {
                                    name: tool_name,
                                    description: format!(
                                        "Invoke the '{}' skill",
                                        dep_id
                                    ),
                                    parameters: serde_json::json!({
                                        "type": "object",
                                        "properties": {
                                            "query": {
                                                "type": "string",
                                                "description": "The input/query to pass to the skill"
                                            }
                                        },
                                        "required": ["query"]
                                    }),
                                    strict: None,
                                    input_examples: None,
                                },
                                backend: ToolBackend::BuiltIn(Arc::new(
                                    SkillInvocationBuiltInAdapter {
                                        executor: executor.clone(),
                                        skill_id: dep_id.clone(),
                                    },
                                )),
                                provides_capabilities: vec![],
                                exempt_from_timeout: true, // nested skills manage own timeouts
                            });
                        }
                    }
                }
                Arc::new(cloned)
            } else {
                self.tool_registry.clone()
            };
            let mut per_request_sandbox =
                SandboxManager::with_defaults(registry, self.bus.clone());
            if let Ok(guard) = self.confirmation_broker.read() {
                if let Some(broker) = guard.as_ref() {
                    per_request_sandbox.set_confirmation_broker(broker.clone());
                }
            }

            // Capture tool names before move (used for post-hoc hallucination guard)
            let resolved_tool_names: Vec<String> =
                tools_for_loop.iter().map(|d| d.name.clone()).collect();

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
                Some(&budget), // context_budget
                None,          // cancel_token — interactive skill calls are not cancellable
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
                LoopFinishReason::Complete
                | LoopFinishReason::MaxRounds
                | LoopFinishReason::Truncated => "success",
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
                crate::orchestrator::LlmMetadata {
                    model: actual_model.to_string(),
                    tokens_in: result.total_input_tokens,
                    tokens_out: result.total_output_tokens,
                },
            );

            // Capture loop metadata for SkillInvocationResult
            inv_finish_reason = result.finish_reason.clone();
            inv_rounds_used = result.rounds_used;
            inv_tool_calls_made = result.tool_calls_made;
            inv_input_tokens = result.total_input_tokens;
            inv_output_tokens = result.total_output_tokens;
            inv_cost_usd = call_cost;
            inv_model_used = result.model_used.clone();

            if let LoopFinishReason::Error(ref err) = result.finish_reason
                && result.final_content.trim().is_empty()
            {
                return Err(format!("LLM error: {}", err));
            }

            // Post-hoc guard: detect hallucinated send confirmations
            let tool_name_refs: Vec<&str> =
                resolved_tool_names.iter().map(|s| s.as_str()).collect();
            if detect_hallucinated_send(
                &tool_name_refs,
                result.tool_calls_made,
                &result.final_content,
            ) {
                tracing::warn!(
                    tool_calls = result.tool_calls_made,
                    "Detected hallucinated send confirmation in skill invocation; overriding response"
                );
                (
                    "\u{26a0}\u{fe0f} \u{6d88}\u{606f}\u{672a}\u{5b9e}\u{9645}\u{53d1}\u{9001}\u{3002}\u{6a21}\u{578b}\u{751f}\u{6210}\u{4e86}\u{786e}\u{8ba4}\u{6587}\u{672c}\u{4f46}\u{672a}\u{8c03}\u{7528}\u{53d1}\u{9001}\u{5de5}\u{5177}\u{3002}\u{8bf7}\u{91cd}\u{65b0}\u{53d1}\u{9001}\u{8bf7}\u{6c42}\u{3002}\n\n\
                     \u{26a0}\u{fe0f} Message was NOT actually sent. The model generated confirmation text \
                     without calling the send tool. Please retry your send request."
                        .to_string(),
                    false,
                )
            } else {
                (result.final_content, false)
            }
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
        let mut repair_attempted = false;
        let mut repair_succeeded = false;
        let mut validation_failures: Vec<String> = Vec::new();
        let validated = match validate_skill_output(&guarded, &skill_doc.frontmatter.output) {
            Ok(v) => v,
            Err(validation_err) => {
                validation_failures.push(validation_err.to_string());
                if skill_doc.frontmatter.output.auto_repair {
                    repair_attempted = true;
                    if let Some((repaired, ok)) =
                        deterministic_repair(&guarded, &validation_err)
                    {
                        repair_succeeded = ok;
                        tracing::info!(
                            "Skill '{}': deterministic repair {}",
                            skill_name,
                            if ok { "succeeded" } else { "failed" }
                        );
                        repaired
                    } else {
                        tracing::warn!(
                            "Skill '{}' output validation failed: {}. No deterministic fix available.",
                            skill_name,
                            validation_err
                        );
                        guarded
                    }
                } else {
                    tracing::warn!(
                        "Skill '{}' output validation failed: {}. Passing through as-is.",
                        skill_name,
                        validation_err
                    );
                    guarded
                }
            }
        };

        // Enforce max_length hard truncation
        let validated = if let Some(max_len) = skill_doc.frontmatter.output.max_length {
            if validated.chars().count() > max_len {
                tracing::info!(
                    "Skill '{}': output truncated from {} to {} chars (max_length)",
                    skill_name,
                    validated.chars().count(),
                    max_len
                );
                validated.chars().take(max_len).collect()
            } else {
                validated
            }
        } else {
            validated
        };

        // Emit AgentResponse event
        self.bus.publish(SystemEvent::AgentResponse {
            request_id,
            agent_id: "orchestrator".to_string(),
            content: validated.clone(),
            timestamp: Utc::now(),
        });

        Ok(SkillInvocationResult {
            content: validated,
            finish_reason: inv_finish_reason,
            rounds_used: inv_rounds_used,
            tool_calls_made: inv_tool_calls_made,
            input_tokens: inv_input_tokens,
            output_tokens: inv_output_tokens,
            cost_usd: inv_cost_usd,
            model_used: inv_model_used,
            repair_attempted,
            repair_succeeded,
            validation_failures,
        })
    }
}
