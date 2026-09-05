//! Simple query and social query handlers for the orchestrator.

use std::sync::Arc;

use super::{
    apply_send_keepalive, detect_active_send_hints, resolve_send_tool_choice,
    sanitize_parts_for_dispatch,
};
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
use crate::orchestrator::{ConversationContext, Orchestrator};
use crate::prompt_ctx::{SectionPriority, sources::{ContextRequest, ExecutionPath}};
use crate::runner::{LoopConfig, LoopFinishReason, run_agentic_loop_routed};
use crate::security::capabilities::Allowlist;
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
        principal: &crate::security::policy::Principal,
        scope: &crate::security::policy::Scope,
        lane_key: &str,
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

        // Skills catalog — per-request (SkillCatalog has its own internal cache)
        let catalog_block = self.build_skills_catalog_block();

        // Connector awareness
        let sendable_channels: Vec<String> = self
            .connector_sender
            .read()
            .ok()
            .and_then(|g| g.as_ref().map(|p| p.sendable_channels()))
            .unwrap_or_default();
        let statuses = if let Ok(guard) = self.connector_status.read()
            && let Some(ref provider) = *guard
        {
            provider.list_status()
        } else {
            vec![]
        };

        // ── Resolve tools ───────────────────────────────────────────────────
        let mut tool_names = self.intent_parser.suggest_tools(tool_suggestion_query);
        let intent_has_send =
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

        // Per-request ToolContext for owner-scoped tools. Built before the
        // loop-override resolution because the tool-mode main loop hands it
        // to `main_loop_tool_set` (identity for steer/followup re-entry).
        let tool_ctx = ToolContext {
            agent_id: None,
            task_id: None,
            owner_id: owner_id.map(|s| s.to_string()),
            workspace_id: scope_ctx.workspace_id.clone(),
            skill_stack: vec![],
            effective_constraints: None,
            lane_key: Some(lane_key.to_string()),
            source: Some(source.to_string()),
            request_id: Some(request_id),
            principal: Some(principal.clone()),
            scope: Some(scope.clone()),
            // Threaded only on the tool-mode main loop, where steer/followup
            // items persist it for re-entry scope; None elsewhere (other
            // paths carry no tools that read it).
            workspace_path: match &loop_overrides {
                Some(super::LoopOverrides::MainLoop { workspace_path }) => workspace_path.clone(),
                _ => None,
            },
        };

        // Apply loop overrides if provided (main loop)
        let (tool_defs, override_max_rounds, override_max_tools, main_loop_set) =
            match &loop_overrides {
                Some(super::LoopOverrides::MainLoop { .. }) => {
                    // Routing V2 main loop: budgets from
                    // `[orchestrator.routing]`; tool surface = base picks ∪
                    // the per-request set (core tools, MCP/plugin extension
                    // tools minus the global deny list, `invoke_skill`, and —
                    // when active — the workflow tools).
                    let routing = self.daemon_config.load().orchestrator.routing.clone();
                    let set = crate::tools::builtins::main_loop_tool_set(
                        self.task_dispatcher.clone(),
                        self.shared_context.clone(),
                        self.bus.clone(),
                        &routing,
                        self.db.clone(),
                        self.embedder.clone(),
                        self.daemon_config.clone(),
                        self.skill_catalog.clone(),
                        self.llm_router.clone(),
                        self.loop_config.max_cost,
                        &self.tool_registry,
                        lane_key,
                        &tool_ctx,
                    );
                    // Base surface: suggested picks ("core_union", default) or
                    // the whole registry minus the global deny list ("full").
                    // Either way `set.definitions` is unioned in below, so
                    // extension tools and `invoke_skill` are reachable in both
                    // modes (deduped by name).
                    let mut defs: Vec<openalpaca_llm::ToolDefinition> =
                        if routing.tool_selection == "full" {
                            let deny = self
                                .daemon_config
                                .load()
                                .execution
                                .skill_defaults
                                .global_tool_deny
                                .clone();
                            self.tool_registry
                                .registered_tool_names()
                                .iter()
                                .filter(|n| !deny.contains(n))
                                .filter_map(|n| {
                                    self.tool_registry.get(n).map(|t| t.definition.clone())
                                })
                                .collect()
                        } else {
                            tool_defs
                        };
                    for def in &set.definitions {
                        if !defs.iter().any(|d| d.name == def.name) {
                            defs.push(def.clone());
                        }
                    }
                    (
                        defs,
                        Some(routing.main_loop_max_rounds),
                        Some(routing.main_loop_max_tools_per_round),
                        Some(set),
                    )
                }
                None => (tool_defs, None, None, None),
            };

        // Keep the guard/telemetry name list in sync with the actual surface
        // on the main-loop path; other paths keep the suggested list verbatim.
        let tool_names: Vec<String> = if main_loop_set.is_some() {
            tool_defs.iter().map(|d| d.name.clone()).collect()
        } else {
            tool_names
        };

        let (tools_for_loop, policy_opt, config_for_loop);
        if !tool_defs.is_empty() {
            tracing::info!(
                "Simple query upgraded with {} tools: {:?}",
                tool_defs.len(),
                tool_names
            );

            // Lowercased: `check_agent_capability` lowercases the tool name
            // and expects allow-list entries pre-normalized (matters for
            // mixed-case MCP/plugin tool names on the default surface).
            let resolved: Vec<String> = tool_defs.iter().map(|t| t.name.to_lowercase()).collect();
            policy_opt = Some(SandboxPolicy {
                agent_id: "orchestrator".to_string(),
                // Closed set: the main loop may call exactly the surface it was
                // handed (this arm only runs when that surface is non-empty).
                allowed_capabilities: Allowlist::Only(resolved),
                denied_capabilities: vec![],
                require_confirmation_for: vec![],
                max_tool_calls: None,
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
                auto_approve: self.daemon_config.load().security.auto_approve_confirmations,
            });
            config_for_loop = LoopConfig {
                max_rounds: override_max_rounds.unwrap_or(4),
                max_tools_per_round: override_max_tools.unwrap_or(2),
                initial_tool_choice: resolve_send_tool_choice(
                    tool_defs.iter().any(|d| d.name == "send"),
                ),
                // Routing V2 deliberate flip: cache the system prompt + tools
                // on the simple-query loop.
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

        // ── Resolve model context window (drives Layer 5 trimming + budget) ──
        //
        // Default to 200_000 when no LLM router is present (echo-stub path).
        let model_window = self.llm_router.as_ref()
            .and_then(|r| config_for_loop.model.as_deref()
                .and_then(|m| r.model_registry().get_model_info(m)))
            .map(|info| info.context_window as usize)
            .unwrap_or(200_000);

        // ── Route system-prompt + message-list assembly through the layered
        // compose engine (Phase 5 Commit 2 — SimpleQuery migration).
        // `PersonaMode::Default` + `StaticPromptMode::Default` +
        // `DynamicContextMode::Default` + `HistoryMode::Default` reproduce the
        // pre-migration PromptBuilder chain output byte-identically. See the
        // `test_golden_simple_query_text_only_byte_identical` and
        // `test_golden_simple_query_multimodal_byte_identical` tests in
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

        // Connector summaries — package statuses + sendable_channels for Layer 2.
        let connector_summaries: Arc<Vec<ConnectorSummary>> = Arc::new(
            statuses
                .iter()
                .map(|(id, st)| ConnectorSummary {
                    sendable: sendable_channels.contains(id),
                    id: id.clone(),
                    status: st.clone(),
                })
                .collect(),
        );

        // send_tool_context is set only when `send` is in the resolved tool set.
        let send_tool_context: Option<Arc<str>> = if tools_for_loop.iter().any(|d| d.name == "send") {
            let send_ctx = self.build_send_context(owner_id);
            if send_ctx.is_empty() {
                None
            } else {
                Some(Arc::<str>::from(send_ctx))
            }
        } else {
            None
        };

        // raw_blocks: SimpleQuery emits the deterministic `send_rules` block
        // AFTER `message_source` (matches Layer 2 Default ordering — see
        // `compose/static_prompt.rs::build_default`). Pre-migration content
        // copied verbatim from simple_query_handler.rs:220-228.
        let mut raw_blocks: Vec<SystemBlock> = Vec::new();
        if tools_for_loop.iter().any(|d| d.name == "send") {
            let mut send_rules = String::from("<send_rules>\n");
            send_rules.push_str(
                "- If the user asks to send a message but did NOT provide specific text, \
                 compose a brief, natural message based on context, then call send (action: \"message\").\n\
                 - If the user asks to send a file, image, photo, or document, call send (action: \"file\").\n\
                 - NEVER claim a message or file was sent without calling the send tool.\n"
            );
            send_rules.push_str("- Only report success/failure based on the tool's actual return value.\n</send_rules>");
            raw_blocks.push(SystemBlock {
                name: "send_rules",
                content: Arc::<str>::from(send_rules),
                priority: SectionPriority::Normal,
            });
        }

        // bootstrap/skills_catalog block passing: pre-migration passed the
        // already-rendered strings via `builder.bootstrap(...)` /
        // `.skills_catalog(...)`. Layer 2 Default emits these via the dedicated
        // fields at the same position.
        let bootstrap_field: Option<Arc<str>> = if bootstrap_block.is_empty() {
            None
        } else {
            Some(Arc::<str>::from(bootstrap_block.clone()))
        };
        let skills_catalog_field: Option<Arc<str>> = if catalog_block.is_empty() {
            None
        } else {
            Some(Arc::<str>::from(catalog_block.clone()))
        };

        let static_prompt_input = StaticPromptInput {
            persona_output,
            agent_persona: Some(Arc::new(agent_persona.clone())),
            agent_config_fingerprint: [0u8; 32],
            skill_block: None,
            skills_catalog: skills_catalog_field,
            bootstrap: bootstrap_field,
            tools: Arc::new(tools_for_loop.clone()),
            connector_status: connector_summaries,
            send_tool_context,
            message_source: Some(Arc::<str>::from(source)),
            raw_blocks,
            mode: StaticPromptMode::Default,
            model_window: model_window as u32,
        };

        // Resolve dynamic context via ContextManager. reserved_tokens is
        // informational for the ContextManager's source selection heuristics;
        // Layer 3 does not use this field. Setting to 0 matches the
        // post-migration baseline (see plan doc Q4).
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
            reserved_tokens: 0,
        };
        let bundle = self.context_manager.resolve(&ctx_request).await;

        let dynamic_context_input = DynamicContextInput {
            context_bundle: Arc::new(bundle),
            query: Arc::from(query),
            memory_retrieval_hash: [0u8; 32],
            path: ExecutionPath::SimpleQuery,
            reserved_tokens: 0,
            mode: DynamicContextMode::Default,
        };

        // ── Multimodal pre-adaptation (loader-side, pre-compose) ──
        //
        // Layer 4 (History) remains pure. Adapt the recent messages' multimodal
        // parts for the target model's capabilities here, BEFORE handing them
        // to compose(). Likewise, adapt `current_parts` before constructing the
        // `current_user_turn` ChatMessage.
        let (adapted_recent, current_user_turn): (Vec<ChatMessage>, Option<ChatMessage>) =
            if let Some(ref router) = self.llm_router {
                let default_model = router.default_model();
                let target_model = config_for_loop
                    .model
                    .as_deref()
                    .unwrap_or(&default_model)
                    .to_string();
                let recent: Vec<ChatMessage> = ctx
                    .recent_messages
                    .iter()
                    .map(|msg| {
                        if msg.parts.is_some() {
                            let mut adapted = msg.clone();
                            adapted.parts = Some(self.adapt_parts_for_model(
                                sanitize_parts_for_dispatch(
                                    msg.parts.clone().unwrap_or_default(),
                                ),
                                &target_model,
                            ));
                            adapted
                        } else {
                            msg.clone()
                        }
                    })
                    .collect();
                let cur = if let Some(parts) = current_parts {
                    let adapted = self.adapt_parts_for_model(
                        sanitize_parts_for_dispatch(parts.to_vec()),
                        &target_model,
                    );
                    Some(ChatMessage::user_with_parts(adapted))
                } else {
                    Some(ChatMessage::user(query))
                };
                (recent, cur)
            } else {
                // Echo-stub path (no router) — pass messages through unchanged.
                (ctx.recent_messages.clone(), Some(ChatMessage::user(query)))
            };

        // Resolve ConversationLane for Tier-2 cache activation (Component 4).
        // `lane_key` is canonical "user_id:source" form; parse back to typed
        // LaneKey and fetch/insert via the LaneManager. `None` when the
        // string is malformed — the compose engine falls back to the global
        // Tier-1 cache in that case.
        let lane_opt: Option<Arc<crate::lane::ConversationLane>> =
            crate::lane::LaneKey::from_str(lane_key)
                .map(|k| self.lane_manager.get_or_create_conversation(k));

        let history_input = HistoryInput {
            lane_tip_fingerprint: lane_opt
                .as_ref()
                .map(|l| l.compute_tip_fingerprint())
                .unwrap_or([0u8; 32]),
            summary: ctx.summary.as_deref().map(Arc::<str>::from),
            summary_wrap_mode: SummaryWrapMode::UntrustedWrap,
            recent_messages: Arc::new(adapted_recent),
            current_user_turn,
            mode: HistoryMode::Default,
        };

        let request = ComposeRequest::SimpleQuery {
            lane_key: lane_key.to_string(),
            agent_persona: Arc::new(agent_persona.clone()),
            query: query.to_string(),
            current_parts: current_parts.map(|p| p.to_vec()),
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
            lane_opt.as_deref(),
        );

        // ── Build ContextBudgetManager for agentic loop. Register the
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

        let (response_content, is_structured) = if let Some(ref router) = self.llm_router {
            // The messages vec came out of the compose engine above, which
            // already stitched system_prompt + dynamic_context blocks/messages
            // + session summary + recent_messages + current_user_turn in the
            // same order the pre-migration manual assembly produced.
            let mut messages: Vec<ChatMessage> = composed.messages.as_ref().clone();

            // Routing V2: model-relay contract for the tool-mode main loop —
            // how to relay start_workflow / steer / cap results in the
            // model's own words. Injected at the same per-turn assembly
            // point as the workflow-context block below.
            if main_loop_set.is_some() {
                let insert_at = messages.len().saturating_sub(1);
                messages.insert(
                    insert_at,
                    ChatMessage::user(crate::tools::builtins::main_loop_relay_guidance()),
                );
            }

            // Routing V2: workflow-context block for lanes with active
            // workflows. Injected per-turn AFTER compose (never through the
            // compose layers) so a live status change can't be masked by the
            // Tier-2 per-lane cache — Layer 3's fingerprint is keyed on query
            // text and would replay a stale block for a repeated query.
            // Gated on the main loop: the block directs the model to
            // steer_workflow/queue_followup/task_status, which only that
            // path carries — rendering it on override-less turns (bootstrap,
            // forced-simple) would coach the model toward tools it cannot
            // call.
            if main_loop_set.is_some()
                && let Some(block) = super::render_workflow_context_block(
                    &self.shared_context,
                    self.db.as_ref(),
                    lane_key,
                )
            {
                // Before the current user turn (last message) so the user's
                // message stays the final input.
                let insert_at = messages.len().saturating_sub(1);
                messages.insert(insert_at, ChatMessage::user(&block));
            }

            // Routing V2: lazy injection of unprocessed steering leftovers.
            // Steering messages a workflow could not deliver before it ended
            // become `unprocessed_steering` rows; the lane's next main-loop
            // turn surfaces them exactly once (the helper marks them done)
            // so the model can acknowledge and act on them. Same per-turn
            // injection point as the workflow-context block above, and same
            // main-loop gate: only this path carries the tools the model
            // may need to re-dispatch the leftover instructions.
            if main_loop_set.is_some()
                && let Some(block) = super::take_unprocessed_steering_block(
                    self.db.as_ref(),
                    lane_key,
                )
            {
                let insert_at = messages.len().saturating_sub(1);
                messages.insert(insert_at, ChatMessage::user(&block));
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

            // Per-request registry: the tool-mode main loop's injected tools
            // (start_workflow, steer_workflow, …) must be reachable from the
            // sandbox execution path — clone the global registry and register
            // them into the clone (lead-runner per-request registry
            // precedent, `runner/lead_agent/mod.rs`). Legacy paths keep
            // sharing the global registry Arc.
            let loop_registry: Arc<crate::tools::ToolRegistry> = match &main_loop_set {
                Some(set) if !set.instances.is_empty() => {
                    let registry = (*self.tool_registry).clone();
                    set.register_into(&registry);
                    Arc::new(registry)
                }
                _ => self.tool_registry.clone(),
            };
            let mut per_request_sandbox = SandboxManager::new(
                loop_registry,
                self.bus.clone(),
                &self.daemon_config.load().security.circuit_breaker,
            );
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
                None,
            )
            .await;
            let latency_ms = call_start.elapsed().as_millis() as i64;

            // Routing V2: structured delegation from the start_workflow
            // result cell (SpawnSubagentTool result-cell precedent). The
            // model's own text remains the reply — no canonical-ack swap.
            if let Some(ref set) = main_loop_set
                && let Some(outcome) = set.start_workflow.outcome()
            {
                self.record_delegation(request_id, &outcome);
            }

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

            // Post-hoc guard: detect hallucinated send confirmations.
            // Narrowed (Routing V2): only with an actual send-intent signal —
            // the query suggested send, or recent turns show active send
            // hints — so a broad tool surface (e.g. tool_selection="full")
            // can't trip it on a bare checkmark.
            let send_intent_signal = intent_has_send
                || detect_active_send_hints(&ctx.recent_messages, &sendable_channels).send;
            let tool_name_refs: Vec<&str> = tool_names.iter().map(|s| s.as_str()).collect();
            if detect_hallucinated_send(
                &tool_name_refs,
                result.tool_calls_made,
                &result.final_content,
                send_intent_signal,
            ) {
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
        lane_key: &str,
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

        // Route system-prompt + message-list assembly through the layered
        // compose engine (Phase 4 Commit 1 — Social fast path migration).
        // `PersonaMode::Minimal` + `StaticPromptMode::SocialMinimal` +
        // `DynamicContextMode::Skip` + `HistoryMode::Default` reproduce the
        // pre-migration inline `format!` output byte-identically. See the
        // `test_golden_social_fast_path_byte_identical` test in
        // `compose/tests.rs` for the byte-identical invariant.
        use crate::compose::{
            ComposeOverrides, ComposeRequest, DynamicContextInput, DynamicContextMode,
            HistoryInput, HistoryMode, PersonaInput, PersonaMode, StaticPromptInput,
            StaticPromptMode, SummaryWrapMode,
        };
        use crate::prompt_ctx::{ContextBundle, ExecutionPath};

        let persona_input = PersonaInput {
            system_persona: Arc::new(system_persona),
            // Social path doesn't carry user/identity docs.
            user_document: Arc::new(None),
            identity_document: Arc::new(None),
            persona_version: self
                .persona_version
                .load(std::sync::atomic::Ordering::Relaxed),
            mode: PersonaMode::Minimal,
            identity_budget: None,
            user_budget: None,
        };

        // Pre-compute the persona output so StaticPromptInput has a real Arc;
        // `compose()` will replace it with its cache-hit Arc on subsequent
        // calls. Initial insertion still hits the cache on this first call
        // within `compose()` itself.
        let persona_output = Arc::new(crate::compose::persona::compute(&persona_input));

        let static_prompt_input = StaticPromptInput {
            persona_output,
            agent_persona: None,
            agent_config_fingerprint: [0u8; 32],
            skill_block: None,
            skills_catalog: None,
            bootstrap: None,
            tools: Arc::new(Vec::new()),
            connector_status: Arc::new(Vec::new()),
            send_tool_context: None,
            message_source: None,
            raw_blocks: Vec::new(),
            mode: StaticPromptMode::SocialMinimal,
            model_window: 8192,
        };

        let dynamic_context_input = DynamicContextInput {
            context_bundle: Arc::new(ContextBundle::empty()),
            query: Arc::from(query),
            memory_retrieval_hash: [0u8; 32],
            path: ExecutionPath::SimpleQuery,
            reserved_tokens: 0,
            mode: DynamicContextMode::Skip,
        };

        // Resolve ConversationLane for Tier-2 cache activation (Component 4).
        // `lane_key` is the canonical "user_id:source" form threaded down
        // from the gateway. Malformed input falls back to the global Tier-1
        // cache (lane_opt == None).
        let lane_opt: Option<Arc<crate::lane::ConversationLane>> =
            crate::lane::LaneKey::from_str(lane_key)
                .map(|k| self.lane_manager.get_or_create_conversation(k));

        let history_input = HistoryInput {
            lane_tip_fingerprint: lane_opt
                .as_ref()
                .map(|l| l.compute_tip_fingerprint())
                .unwrap_or([0u8; 32]),
            summary: None,
            summary_wrap_mode: SummaryWrapMode::Plain,
            recent_messages: Arc::new(ctx.recent_messages.clone()),
            current_user_turn: Some(ChatMessage::user(query)),
            mode: HistoryMode::Default,
        };

        let request = ComposeRequest::Social {
            lane_key: lane_key.to_string(),
            query: query.to_string(),
            overrides: ComposeOverrides::default(),
        };

        let composed = self.compose_engine.compose(
            &request,
            persona_input,
            static_prompt_input,
            dynamic_context_input,
            history_input,
            8192,
            Arc::new(Vec::new()),
            Some(&self.bus),
            lane_opt.as_deref(),
        );

        let messages: Vec<ChatMessage> = composed.messages.as_ref().clone();

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
        Ok(response)
    }
}
