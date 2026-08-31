//! Core skill invocation implementation: tool resolution, prompt assembly
//! via `ComposeEngine::compose(ComposeRequest::Skill{..})`, agentic loop
//! execution, output validation and repair.

use super::context::inject_skill_context;
use super::handler::SkillInvocationResult;
use super::invoke_executor::{SkillInvocationBuiltInAdapter, SkillInvocationToolExecutor};
use super::output::{deterministic_repair, validate_skill_output};
use super::preflight::preflight_permissions;
use crate::compose::{
    ComposeOverrides, ComposeRequest, ConnectorSummary, DynamicContextInput, DynamicContextMode,
    HistoryInput, HistoryMode, PersonaInput, PersonaMode, StaticPromptInput, StaticPromptMode,
    SummaryWrapMode, SystemBlock,
};
use crate::events::SystemEvent;
use crate::memory::scope_context::MemoryScopeContext;
use crate::middleware::bootstrap::bootstrap_to_prompt_block;
use crate::middleware::guard::{OutputGuard, detect_hallucinated_send};
use crate::middleware::prompt::AgentPersona;
use crate::middleware::skill::skill_to_prompt_block;
use crate::orchestrator::{ConversationContext, Orchestrator};
use crate::prompt_ctx::SectionPriority;
use crate::prompt_ctx::sources::{ContextRequest, ExecutionPath};
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
        lane_key: &str,
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

        // Plugin-backed skills execute out-of-process via their
        // PluginSkillExecutor; everything below (compose + agentic loop)
        // is the file-based path.
        if let crate::orchestrator::skill::catalog::SkillSource::Plugin {
            ref plugin_id,
            ref executor,
        } = entry.source
        {
            return self
                .invoke_plugin_skill(
                    request_id,
                    source,
                    skill_name,
                    plugin_id,
                    executor.clone(),
                    query,
                    lane_key,
                    owner_id,
                    scope_ctx,
                    stream_id,
                    &skill_doc,
                )
                .await;
        }

        // Context injection from skill's context.sources (file-based skills only)
        let injected_context = if let Some(ref skill_dir) = entry.skill_dir {
            inject_skill_context(&skill_doc.frontmatter.context, skill_dir).await?
        } else {
            String::new()
        };

        // ── Resolve model context window (drives Layer 5 trimming + budget) ──
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
        // Identity + user blocks are derived by Layer 1 Default from
        // `PersonaInput.identity_document` + `PersonaInput.identity_budget`
        // and `PersonaInput.user_document` + `PersonaInput.user_budget`.
        // Both budgets sourced from `daemon.orchestrator.prompt_budgets`.
        let bootstrap_block = if let Ok(guard) = self.bootstrap_document.read()
            && let Some(ref doc) = *guard
        {
            bootstrap_to_prompt_block(doc)
        } else {
            String::new()
        };

        let skill_block = skill_to_prompt_block(&skill_doc);

        // Emit SkillContextInjected when injected_context is present. Moved here
        // from inside the former PromptBuilder chain; the event contract is
        // unchanged (request_id, skill_id, context_bytes, timestamp).
        if !injected_context.is_empty() {
            self.bus.publish(SystemEvent::SkillContextInjected {
                request_id,
                skill_id: skill_name.to_string(),
                context_bytes: injected_context.len(),
                timestamp: Utc::now(),
            });
        }

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

        // Connector status + sendable channels — packaged into
        // `ConnectorSummary[]` for Layer 2's `connector_guidance` rendering.
        let sendable_channels: Vec<String> = self
            .connector_sender
            .read()
            .ok()
            .and_then(|g| g.as_ref().map(|p| p.sendable_channels()))
            .unwrap_or_default();
        let connector_summaries_vec: Vec<ConnectorSummary> = if let Ok(guard) =
            self.connector_status.read()
            && let Some(ref provider) = *guard
        {
            provider
                .list_status()
                .into_iter()
                .map(|(id, status)| ConnectorSummary {
                    sendable: sendable_channels.contains(&id),
                    id,
                    status,
                })
                .collect()
        } else {
            Vec::new()
        };
        let connector_summaries: Arc<Vec<ConnectorSummary>> =
            Arc::new(connector_summaries_vec);

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
                lane_key: Some(lane_key.to_string()),
                confirmation_timeout_secs: Some(
                    self.daemon_config
                        .load()
                        .execution
                        .agent_defaults
                        .confirmation_timeout_secs,
                ),
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

        // ── Route system-prompt + message-list assembly through the layered
        // compose engine (Phase 5 Commit 1 — Skill Invocation migration).
        // `PersonaMode::Default` + `StaticPromptMode::Default` +
        // `DynamicContextMode::Default` + `HistoryMode::Default` reproduce the
        // pre-migration PromptBuilder chain output byte-identically. See the
        // `test_golden_skill_invocation_byte_identical` test in
        // `compose/tests.rs` for the byte-identical invariant.
        let user_document = match self.user_document.read() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        };
        let identity_document = match self.identity_document.read() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        };

        let persona_input = PersonaInput {
            system_persona: Arc::new(system_persona),
            user_document: Arc::new(user_document),
            identity_document: Arc::new(identity_document),
            persona_version: self
                .persona_version
                .load(std::sync::atomic::Ordering::Relaxed),
            mode: PersonaMode::Default,
            identity_budget: Some(
                self.daemon_config
                    .load()
                    .orchestrator
                    .prompt_budgets
                    .identity_budget,
            ),
            user_budget: Some(
                self.daemon_config
                    .load()
                    .orchestrator
                    .prompt_budgets
                    .user_profile_budget,
            ),
        };
        let persona_output = Arc::new(crate::compose::persona::compute(&persona_input));

        // send_tool_context is set only when `send` is in the resolved tool set.
        let send_tool_context: Option<Arc<str>> = if tools_for_loop
            .iter()
            .any(|d| d.name == "send")
        {
            let send_ctx = self.build_send_context(owner_id);
            if send_ctx.is_empty() {
                None
            } else {
                Some(Arc::<str>::from(send_ctx))
            }
        } else {
            None
        };

        // raw_blocks carry the loader-injected blocks that Layer 2 Default emits
        // between skill_body and message_source. Skill's only raw_block today is
        // `skill_context` (populated when injected_context is non-empty). The
        // pre-migration order was: skill_body → skill_context → message_source.
        let mut raw_blocks: Vec<SystemBlock> = Vec::new();
        if !injected_context.is_empty() {
            raw_blocks.push(SystemBlock {
                name: "skill_context",
                content: Arc::<str>::from(injected_context.clone()),
                priority: SectionPriority::Normal,
            });
        }

        // identity/bootstrap block passing: pre-migration passed the already-
        // rendered strings via `builder.identity(&identity_block)` /
        // `.bootstrap(&bootstrap_block)`. Layer 1 Default re-derives these from
        // `identity_document` directly (not from `identity_block` text). Pass
        // `bootstrap` via the dedicated field so Layer 2 Default emits it at
        // the same position. Identity gets routed through Layer 1 Default's
        // `identity_document_block` (in PersonaInput.identity_document), so
        // we don't re-pass the rendered `identity_block` string here.
        let bootstrap_field: Option<Arc<str>> = if bootstrap_block.is_empty() {
            None
        } else {
            Some(Arc::<str>::from(bootstrap_block.clone()))
        };
        let skill_block_field: Option<Arc<str>> = if skill_block.is_empty() {
            None
        } else {
            Some(Arc::<str>::from(skill_block.clone()))
        };

        let static_prompt_input = StaticPromptInput {
            persona_output,
            agent_persona: Some(Arc::new(agent_persona.clone())),
            agent_config_fingerprint: [0u8; 32],
            skill_block: skill_block_field,
            skills_catalog: None,
            bootstrap: bootstrap_field,
            tools: Arc::new(tools_for_loop.clone()),
            connector_status: connector_summaries,
            send_tool_context,
            message_source: Some(Arc::<str>::from(source)),
            raw_blocks,
            mode: StaticPromptMode::SkillInvocationDefault,
            model_window: model_window as u32,
        };

        // ── Resolve dynamic context via ContextManager ─────────────────────
        // reserved_tokens is informational for the ContextManager's source
        // selection heuristics; Layer 3 does not use this field. Setting to 0
        // matches the post-migration baseline (see plan doc Q4).
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
            reserved_tokens: 0,
        };
        let bundle = self.context_manager.resolve(&ctx_request).await;

        let dynamic_context_input = DynamicContextInput {
            context_bundle: Arc::new(bundle),
            query: Arc::from(query),
            memory_retrieval_hash: [0u8; 32],
            path: ExecutionPath::SkillInvocation {
                skill_id: skill_name.to_string(),
            },
            reserved_tokens: 0,
            mode: DynamicContextMode::Default,
        };

        let history_input = HistoryInput {
            lane_tip_fingerprint: [0u8; 32],
            summary: ctx.summary.as_deref().map(Arc::<str>::from),
            summary_wrap_mode: SummaryWrapMode::UntrustedWrap,
            recent_messages: Arc::new(ctx.recent_messages.clone()),
            current_user_turn: Some(ChatMessage::user(query)),
            mode: HistoryMode::Default,
        };

        let request = ComposeRequest::Skill {
            lane_key: String::new(),
            agent_persona: Arc::new(agent_persona.clone()),
            skill_id: skill_name.to_string(),
            skill_block: Arc::<str>::from(skill_block.clone()),
            injected_context: Arc::<str>::from(injected_context.clone()),
            query: query.to_string(),
            message_source: Arc::<str>::from(source),
            overrides: ComposeOverrides::default(),
        };

        let composed = self.compose_engine.compose(
            &request,
            persona_input,
            static_prompt_input,
            dynamic_context_input,
            history_input,
            model_window as u32,
            Arc::new(tools_for_loop.clone()),
            Some(&self.bus),
            None, // lane: per-lane cache for Skill deferred (plan Q2)
        );

        // Build ContextBudgetManager for agentic loop. Register the
        // combined static-prompt + dynamic-context tokens under `system_prompt`
        // to preserve the pre-migration budget accounting (pre-migration
        // `built.total_prompt_tokens` summed all registered sections including
        // context_bundle sections).
        let ctx_config = &self.daemon_config.load().execution.context;
        let mut budget =
            crate::context_budget::ContextBudgetManager::new(model_window, ctx_config);
        let system_prompt_tokens = (composed.token_budget.static_prompt_tokens
            + composed.token_budget.dynamic_context_tokens)
            as usize;
        budget.register_section("system_prompt", system_prompt_tokens);
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
            // The messages vec came out of the compose engine above, which
            // already stitched system_prompt + dynamic_context blocks/messages
            // + session summary + recent_messages + current_user_turn in the
            // same order the pre-migration manual assembly produced.
            let messages: Vec<ChatMessage> = composed.messages.as_ref().clone();

            // Per-request sandbox with ToolContext
            let tool_ctx = ToolContext {
                agent_id: None,
                task_id: None,
                owner_id: owner_id.map(|s| s.to_string()),
                workspace_id: scope_ctx.workspace_id.clone(),
                skill_stack: vec![skill_name.to_string()],
                effective_constraints: None,
                lane_key: Some(lane_key.to_string()),
                source: Some(source.to_string()),
                request_id: Some(request_id),
                principal: None,
                scope: None,
                workspace_path: None,
            };
            let needs_clone = !skill_doc.frontmatter.scripts.is_empty()
                || !skill_doc.frontmatter.depends_on.is_empty();
            let registry = if needs_clone {
                let cloned = (*self.tool_registry).clone();
                if let Some(ref skill_dir) = entry.skill_dir {
                    for cfg in &skill_doc.frontmatter.scripts {
                        let tool = ScriptToolBuiltIn::new(skill_dir, cfg)?;
                        cloned.register(RegisteredTool {
                            definition: ScriptToolBuiltIn::tool_definition(&cfg.name),
                            backend: ToolBackend::BuiltIn(Arc::new(tool)),
                            provides_capabilities: vec![],
                            exempt_from_timeout: false,
                            annotations: None,
                            version: env!("CARGO_PKG_VERSION").to_string(),
                            author: format!("skill:{}", skill_name),
                            created_at: chrono::Utc::now(),
                        })?;
                    }
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
                        None,                          // cost_accumulator (top-level)
                        Some(tool_ctx.clone()),         // parent_tool_context
                        config_for_loop.max_cost,       // parent_max_cost
                        self.daemon_config
                            .load()
                            .security
                            .auto_approve_confirmations, // auto_approve
                        global_deny.clone(),            // global_tool_deny
                        self.daemon_config.load().security.circuit_breaker.clone(),
                        self.daemon_config
                            .load()
                            .execution
                            .agent_defaults
                            .confirmation_timeout_secs,
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
                                annotations: None,
                                version: env!("CARGO_PKG_VERSION").to_string(),
                                author: format!("skill:{}", skill_name),
                                created_at: chrono::Utc::now(),
                            })?;
                        }
                    }
                }
                Arc::new(cloned)
            } else {
                self.tool_registry.clone()
            };
            let mut per_request_sandbox = SandboxManager::new(
                registry,
                self.bus.clone(),
                &self.daemon_config.load().security.circuit_breaker,
            );
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

            // Post-hoc guard: detect hallucinated send confirmations.
            // Narrowed (Routing V2): only with an actual send-intent signal —
            // the query suggested send, or recent turns show active send
            // hints — a skill merely declaring `send` must not trip it.
            let send_intent_signal = self
                .intent_parser
                .suggest_tools(query)
                .contains(&"send".to_string())
                || crate::orchestrator::query_handler::detect_active_send_hints(
                    &ctx.recent_messages,
                    &sendable_channels,
                )
                .send;
            let tool_name_refs: Vec<&str> =
                resolved_tool_names.iter().map(|s| s.as_str()).collect();
            if detect_hallucinated_send(
                &tool_name_refs,
                result.tool_calls_made,
                &result.final_content,
                send_intent_signal,
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

    /// Execute a plugin-backed skill out-of-process.
    ///
    /// The plugin's `PluginSkillExecutor` drives its own reasoning loop inside
    /// the plugin process; tool callbacks it requests are proxied through the
    /// sandboxed execute path (mirroring `runner::plugin_agent`), so plugin
    /// skills get the same capability checks, confirmation gating, and
    /// timeouts as file-based skills. Lifecycle events
    /// (SkillInvocationStarted/Completed/Failed) and execution telemetry are
    /// emitted by the `handle_skill_invocation` wrapper, which this path
    /// shares with file-based skills. No LLM router is involved — token and
    /// cost fields in the result are zero.
    #[allow(clippy::too_many_arguments)]
    async fn invoke_plugin_skill(
        &self,
        request_id: Uuid,
        source: &str,
        skill_name: &str,
        plugin_id: &str,
        executor: Arc<dyn openalpaca_api::plugin_traits::PluginSkillExecutor>,
        query: &str,
        lane_key: &str,
        owner_id: Option<&str>,
        scope_ctx: &MemoryScopeContext,
        stream_id: Option<&str>,
        skill_doc: &crate::middleware::skill::SkillDocument,
    ) -> Result<SkillInvocationResult, String> {
        let fm = &skill_doc.frontmatter;

        // Allowed tool set: capability-resolved names (same resolution as the
        // file-based path), falling back to the legacy allow list.
        let allowed: Vec<String> = if !fm.requires_capabilities.is_empty() {
            self.tool_registry
                .tools_for_capabilities(&fm.requires_capabilities)
                .into_iter()
                .map(|d| d.name)
                .collect()
        } else {
            fm.tools.allow.clone()
        };
        let mut denied: Vec<String> = fm.tools.deny.clone();
        for g in &self
            .daemon_config
            .load()
            .execution
            .skill_defaults
            .global_tool_deny
        {
            if !denied.contains(g) {
                denied.push(g.clone());
            }
        }

        let policy = SandboxPolicy {
            agent_id: format!("plugin:{plugin_id}"),
            allowed_capabilities: allowed,
            denied_capabilities: denied,
            require_confirmation_for: fm.permissions.confirm.tools.clone(),
            max_tool_calls: fm.tools.rate_limit.max_calls.map(|n| n as u32),
            max_tool_runtime_secs: self.loop_config.max_tool_runtime.as_secs(),
            stream_id: stream_id.map(|s| s.to_string()),
            lane_key: Some(lane_key.to_string()),
            confirmation_timeout_secs: Some(
                self.daemon_config
                    .load()
                    .execution
                    .agent_defaults
                    .confirmation_timeout_secs,
            ),
            auto_approve: self
                .daemon_config
                .load()
                .security
                .auto_approve_confirmations,
        };

        let mut sandbox = SandboxManager::new(
            self.tool_registry.clone(),
            self.bus.clone(),
            &self.daemon_config.load().security.circuit_breaker,
        );
        if let Ok(guard) = self.confirmation_broker.read() {
            if let Some(broker) = guard.as_ref() {
                sandbox.set_confirmation_broker(broker.clone());
            }
        }

        let tool_ctx = ToolContext {
            agent_id: None,
            task_id: None,
            owner_id: owner_id.map(|s| s.to_string()),
            workspace_id: scope_ctx.workspace_id.clone(),
            skill_stack: vec![skill_name.to_string()],
            effective_constraints: None,
            lane_key: Some(lane_key.to_string()),
            source: Some(source.to_string()),
            request_id: Some(request_id),
            principal: None,
            scope: None,
            workspace_path: None,
        };

        let callback = SandboxToolCallback {
            sandbox,
            policy,
            tool_ctx,
            calls: std::sync::atomic::AtomicUsize::new(0),
        };

        let context = serde_json::json!({
            "source": source,
            "lane_key": lane_key,
            "owner_id": owner_id,
        });

        tracing::info!(
            skill = skill_name,
            plugin = plugin_id,
            "Invoking plugin-backed skill"
        );
        let content = executor
            .invoke(query, &context, &callback)
            .await
            .map_err(|e| format!("Plugin skill '{skill_name}' failed: {e}"))?;

        Ok(SkillInvocationResult {
            content,
            finish_reason: LoopFinishReason::Complete,
            rounds_used: 1,
            tool_calls_made: callback.calls.load(std::sync::atomic::Ordering::Relaxed),
            input_tokens: 0,
            output_tokens: 0,
            cost_usd: 0.0,
            model_used: None,
            repair_attempted: false,
            repair_succeeded: false,
            validation_failures: Vec::new(),
        })
    }
}

/// Adapter giving a plugin skill sandboxed tool access during `invoke()`.
///
/// Every tool call the plugin requests goes through the same sandboxed
/// execute path as file-based skill tools: capability checks, input
/// sanitization, confirmation gating, circuit breaker, and timeout.
struct SandboxToolCallback {
    sandbox: SandboxManager,
    policy: SandboxPolicy,
    tool_ctx: ToolContext,
    calls: std::sync::atomic::AtomicUsize,
}

#[async_trait::async_trait]
impl openalpaca_api::plugin_traits::ToolCallbackExecutor for SandboxToolCallback {
    async fn execute_tool(
        &self,
        tool_name: &str,
        arguments: &serde_json::Value,
    ) -> Result<String, String> {
        self.calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let call = openalpaca_llm::ToolCall {
            id: Uuid::new_v4().to_string(),
            name: tool_name.to_string(),
            arguments: arguments.clone(),
        };
        self.sandbox
            .execute_tool(&call, &self.policy, &self.tool_ctx)
            .await
    }
}
