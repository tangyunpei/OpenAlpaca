use super::{Orchestrator, principal_id};
use crate::events::SystemEvent;
use crate::memory::scope_context::MemoryScopeContext;
use crate::security::gate::SecurityGate;
use crate::security::policy::{Principal, Scope};
use crate::types::Capability;
use chrono::Utc;
use openalpaca_llm::ContentPart;
use openalpaca_storage::repository::orchestrator_latency::{
    OrchestratorLatencyRecord, OrchestratorLatencyRepository,
};
use std::time::Instant;
use uuid::Uuid;

use super::intent::Intent;
use super::task_planner::TaskPlanner;

impl Orchestrator {
    /// Public entry point for processing a user message.
    #[allow(clippy::too_many_arguments)]
    pub async fn handle_message(
        &self,
        request_id: Uuid,
        source: String,
        content: String,
        principal: Principal,
        scope: Scope,
        lane_key: String,
        workspace_path: Option<String>,
        stream_id: Option<String>,
    ) -> Result<String, String> {
        self.handle_message_internal(
            request_id,
            source,
            content.clone(),
            content,
            false,
            None,
            principal,
            scope,
            lane_key,
            workspace_path,
            stream_id,
        )
        .await
    }

    /// Internal message handler that separates the model input from the intent source.
    ///
    /// `intent_source_content` is used only for intent classification/fast-path checks.
    /// `model_input_content` is used for planner/LLM calls and context building.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn handle_message_internal(
        &self,
        request_id: Uuid,
        source: String,
        model_input_content: String,
        intent_source_content: String,
        force_simple_query: bool,
        current_parts: Option<Vec<ContentPart>>,
        principal: Principal,
        scope: Scope,
        lane_key: String,
        workspace_path: Option<String>,
        stream_id: Option<String>,
    ) -> Result<String, String> {
        let ack_start = Instant::now();

        // 1. Permission check via SecurityGate (wraps TrustGate)
        let capability = Capability {
            name: "chat.respond".to_string(),
        };
        SecurityGate::check_access(&principal, &capability, &scope)?;

        // 2. Input sanitization
        let max_input_len = self.daemon_config.load().security.max_input_length;
        let model_input_content =
            SecurityGate::sanitize_input(&model_input_content, Some(max_input_len))?;
        let intent_source_content =
            SecurityGate::sanitize_input(&intent_source_content, Some(max_input_len))?;

        // Extract owner_id from principal (before slash-command early return)
        let owner_id_str = principal_id(&principal);
        let owner_id = match &principal {
            Principal::User { .. } => Some(owner_id_str.as_str()),
            _ => None,
        };

        // Resolve workspace context for memory scoping.
        // Prefer request-provided workspace path (from GUI/CLI) over daemon CWD.
        let workspace_id = if let Some(ref ws_path) = workspace_path {
            crate::memory::workspace::resolve_workspace_id(std::path::Path::new(ws_path))
        } else {
            tracing::debug!("No workspace_path in request, falling back to daemon CWD");
            std::env::current_dir()
                .ok()
                .and_then(|d| crate::memory::workspace::resolve_workspace_id(&d))
        };
        let scope_ctx = MemoryScopeContext::new(workspace_id);

        // 3. Try slash commands, task queries, and skill invocations first
        let intent = if force_simple_query {
            Intent::SimpleQuery {
                query: intent_source_content.trim().to_string(),
            }
        } else {
            self.intent_parser.parse_with_skills_and_router(
                &intent_source_content,
                &self.skill_catalog,
                &self.skill_router,
            )
        };
        match &intent {
            Intent::TaskQuery { .. } | Intent::TaskControl { .. } => {
                self.bus.publish(SystemEvent::IntentClassified {
                    request_id,
                    intent_type: intent.intent_type().to_string(),
                    timestamp: Utc::now(),
                });
                return match intent {
                    Intent::TaskQuery { task_id } => self.handle_task_query(task_id, &owner_id_str),
                    Intent::TaskControl { task_id, action } => {
                        self.handle_task_control(&task_id, &action)
                    }
                    _ => unreachable!(),
                };
            }
            _ => {}
        }

        // 4. Build context ONCE for all remaining paths (D6: single dedup location)
        let ctx = self.build_context(&lane_key, &model_input_content);

        // 5. Compute result — planner path or heuristic fallback
        //    Track timing for observability (Step 1: OrchestrationStage metrics)
        let mut planner_ms: u64 = 0;
        let mut dispatch_ms: u64 = 0;
        let mode: String;
        let mut fallback_reason: Option<String> = None;
        let mut auto_promotion_reason: Option<String> = None;

        let result: Result<String, String> = if !force_simple_query
            && self.daemon_config.load().orchestrator.routing.steering_enabled
            && let Some(steer_text) = intent_source_content.strip_prefix("/steer ")
        {
            // Deterministic steering override (Routing V2): guaranteed
            // injection into the lane's running workflow, bypassing the
            // model. With steering_enabled=false (default) this arm is
            // skipped and "/steer ..." routes exactly as before.
            mode = "steered".to_string();
            self.handle_steer_prefix(
                request_id,
                steer_text,
                &principal,
                &scope,
                &lane_key,
                workspace_path.clone(),
            )
        } else if let Intent::SkillInvocation {
            ref skill_name,
            ref query,
        } = intent
        {
            // Deterministic skill tier: slash commands and router-selected
            // skills execute directly, before the bootstrap check and the
            // planner ladder (Routing V2 Phase 0.5).
            mode = "skill_command".to_string();
            self.invoke_skill_with_telemetry(
                request_id,
                &source,
                skill_name,
                query,
                &lane_key,
                &ctx,
                owner_id,
                &scope_ctx,
                stream_id.as_deref(),
            )
            .await
        } else if self.is_bootstrapping() {
            mode = "bootstrap".to_string();
            self.handle_simple_query(
                request_id,
                &source,
                &model_input_content,
                &intent_source_content,
                &principal,
                &scope,
                &lane_key,
                &ctx,
                owner_id,
                &scope_ctx,
                current_parts.as_deref(),
                stream_id.as_deref(),
                None,
            )
            .await
        } else if force_simple_query {
            mode = "forced_simple_query".to_string();
            self.handle_simple_query(
                request_id,
                &source,
                &model_input_content,
                &intent_source_content,
                &principal,
                &scope,
                &lane_key,
                &ctx,
                owner_id,
                &scope_ctx,
                current_parts.as_deref(),
                stream_id.as_deref(),
                None,
            )
            .await
        } else if self.llm_router.is_some()
            && matches!(intent, Intent::SimpleQuery { .. })
            && self.intent_parser.is_social_message(&intent_source_content)
            && {
                let sendable_for_guard: Vec<String> = self
                    .connector_sender
                    .read()
                    .ok()
                    .and_then(|g| g.as_ref().map(|p| p.sendable_channels()))
                    .unwrap_or_default();
                super::query_handler::detect_active_send_hints(&ctx.recent_messages, &sendable_for_guard)
                    == super::query_handler::ActiveSendHints::default()
            }
        {
            // Social fast path: ultra-light prompt for "ok", "thanks", "好的" etc.
            mode = "social_fast_path".to_string();
            self.bus.publish(SystemEvent::PlannerBypassed {
                request_id,
                reason: "social_fast_path".to_string(),
                timestamp: Utc::now(),
            });
            self.handle_social_query(request_id, &model_input_content, &lane_key, &ctx)
                .await
        } else if self.daemon_config.load().orchestrator.routing.mode == "tool" {
            // Routing V2 tool-mode main loop: the front door for everything
            // that survived the deterministic tier and the social branch.
            // The fast-path / two-phase / full-planner ladder is skipped
            // entirely — chat vs. task vs. steer is the model's tool choice
            // (`start_workflow` / `steer_workflow` via the per-request core
            // tool set assembled inside handle_simple_query).
            mode = "main_loop".to_string();
            self.handle_simple_query(
                request_id,
                &source,
                &model_input_content,
                &intent_source_content,
                &principal,
                &scope,
                &lane_key,
                &ctx,
                owner_id,
                &scope_ctx,
                current_parts.as_deref(),
                stream_id.as_deref(),
                Some(super::query_handler::LoopOverrides::MainLoop {
                    workspace_path: workspace_path.clone(),
                }),
            )
            .await
        } else if self.llm_router.is_some()
            && matches!(intent, Intent::SimpleQuery { .. })
            && self
                .intent_parser
                .is_fast_path_eligible(&intent_source_content)
        {
            // Fast path: skip LLM planner for obviously simple messages
            mode = "fast_path".to_string();
            self.bus.publish(SystemEvent::PlannerBypassed {
                request_id,
                reason: "fast_path".to_string(),
                timestamp: Utc::now(),
            });
            self.handle_simple_query(
                request_id,
                &source,
                &model_input_content,
                &intent_source_content,
                &principal,
                &scope,
                &lane_key,
                &ctx,
                owner_id,
                &scope_ctx,
                current_parts.as_deref(),
                stream_id.as_deref(),
                None,
            )
            .await
        } else if let Some(ref router) = self.llm_router
            && matches!(intent, Intent::SimpleQuery { .. })
            && self.daemon_config.load().execution.planner.two_phase_enabled
        {
            // Two-phase: 3-way LLM triage (simple_query / deep_query / complex_task)
            let planner_cfg = self.daemon_config.load();
            let triage_model = planner_cfg.execution.planner.triage_model.as_deref();
            let available_tool_names = self.tool_registry.registered_tool_names();
            match TaskPlanner::classify_lightweight(
                router,
                triage_model,
                &intent_source_content,
                10,
                &available_tool_names,
            )
            .await
            {
                Ok(triage) => {
                    match triage.classification.as_str() {
                        "simple_query" => {
                            mode = "two_phase_simple".to_string();
                            self.bus.publish(SystemEvent::PlannerBypassed {
                                request_id,
                                reason: "two_phase_lightweight".to_string(),
                                timestamp: Utc::now(),
                            });
                            self.handle_simple_query(
                                request_id,
                                &source,
                                &model_input_content,
                                &intent_source_content,
                                &principal,
                                &scope,
                                &lane_key,
                                &ctx,
                                owner_id,
                                &scope_ctx,
                                current_parts.as_deref(),
                                stream_id.as_deref(),
                                None,
                            )
                            .await
                        }
                        "complex_task" => {
                            // Fall through to full planner for multi-agent tasks
                            mode = "two_phase_complex".to_string();
                            let (idle_agents, limits, dag_prefer) = self.build_planner_inputs();
                            let active_tasks_block = owner_id.and_then(|id| self.build_active_tasks_block(id));

                            let planner_start = Instant::now();
                            let plan_result = TaskPlanner::plan_hierarchical(
                                router,
                                self.compose_engine.as_ref(),
                                &model_input_content,
                                &idle_agents,
                                &ctx.recent_messages,
                                ctx.summary.as_deref(),
                                active_tasks_block.as_deref(),
                                limits,
                                dag_prefer,
                            )
                            .await;
                            planner_ms = planner_start.elapsed().as_millis() as u64;

                            match plan_result {
                                Ok(plan) => {
                                    auto_promotion_reason = plan.auto_promotion_reason.clone();
                                    match plan.classification.as_str() {
                                        "simple_query" => {
                                            self.handle_simple_query(
                                                request_id,
                                                &source,
                                                &model_input_content,
                                                &intent_source_content,
                                                &principal,
                                                &scope,
                                                &lane_key,
                                                &ctx,
                                                owner_id,
                                                &scope_ctx,
                                                current_parts.as_deref(),
                                                stream_id.as_deref(),
                                                None,
                                            )
                                            .await
                                        }
                                        "complex_task" => {
                                            let description = &model_input_content;
                                            let augmented =
                                                self.augment_with_context(description, &ctx);
                                            let dispatch_start = Instant::now();
                                            let dispatch_result =
                                                self.task_dispatcher.dispatch_planned(
                                                    request_id,
                                                    &augmented,
                                                    plan,
                                                    &principal_id(&principal),
                                                    &lane_key,
                                                    &source,
                                                    scope_ctx.workspace_id.clone(),
                                                );
                                            dispatch_ms =
                                                dispatch_start.elapsed().as_millis() as u64;
                                            match dispatch_result {
                                                Ok(outcome) => {
                                                    self.record_delegation(request_id, &outcome);
                                                    Ok(outcome.ack)
                                                }
                                                Err(e) => {
                                                    fallback_reason = Some(format!(
                                                        "two_phase_dispatch_failed: {e}"
                                                    ));
                                                    self.handle_simple_query(
                                                        request_id,
                                                        &source,
                                                        &model_input_content,
                                                        &intent_source_content,
                                                        &principal,
                                                        &scope,
                                                        &lane_key,
                                                        &ctx,
                                                        owner_id,
                                                        &scope_ctx,
                                                        current_parts.as_deref(),
                                                        stream_id.as_deref(),
                                                        None,
                                                    )
                                                    .await
                                                }
                                            }
                                        }
                                        _ => {
                                            fallback_reason =
                                                Some("two_phase_unknown_classification".to_string());
                                            self.handle_simple_query(
                                                request_id,
                                                &source,
                                                &model_input_content,
                                                &intent_source_content,
                                                &principal,
                                                &scope,
                                                &lane_key,
                                                &ctx,
                                                owner_id,
                                                &scope_ctx,
                                                current_parts.as_deref(),
                                                stream_id.as_deref(),
                                                None,
                                            )
                                            .await
                                        }
                                    }
                                }
                                Err(e) => {
                                    fallback_reason =
                                        Some(format!("two_phase_planner_failed: {e}"));
                                    self.handle_simple_query(
                                        request_id,
                                        &source,
                                        &model_input_content,
                                        &intent_source_content,
                                        &principal,
                                        &scope,
                                        &lane_key,
                                        &ctx,
                                        owner_id,
                                        &scope_ctx,
                                        current_parts.as_deref(),
                                        stream_id.as_deref(),
                                        None,
                                    )
                                    .await
                                }
                            }
                        }
                        _ => {
                            // deep_query (or unknown tier): expanded agentic loop with LLM-suggested tools
                            mode = "two_phase_deep_query".to_string();
                            let deep_cfg = &planner_cfg.execution.planner;
                            let override_tools: Vec<openalpaca_llm::ToolDefinition> = triage
                                .suggested_tools
                                .iter()
                                .filter_map(|name| self.tool_registry.get(name))
                                .map(|t| t.definition.clone())
                                .collect();
                            let overrides = super::query_handler::LoopOverrides::DeepQuery {
                                max_rounds: deep_cfg.deep_query_max_rounds,
                                max_tools_per_round: deep_cfg.deep_query_max_tools_per_round,
                                override_tools,
                            };
                            self.handle_simple_query(
                                request_id,
                                &source,
                                &model_input_content,
                                &intent_source_content,
                                &principal,
                                &scope,
                                &lane_key,
                                &ctx,
                                owner_id,
                                &scope_ctx,
                                current_parts.as_deref(),
                                stream_id.as_deref(),
                                Some(overrides),
                            )
                            .await
                        }
                    }
                }
                Err(e) => {
                    // Triage failed: fall back to simple query
                    tracing::warn!("Lightweight triage failed: {e}, falling back to simple_query");
                    mode = "two_phase_triage_failed".to_string();
                    fallback_reason = Some(format!("triage_failed: {e}"));
                    self.handle_simple_query(
                        request_id,
                        &source,
                        &model_input_content,
                        &intent_source_content,
                        &principal,
                        &scope,
                        &lane_key,
                        &ctx,
                        owner_id,
                        &scope_ctx,
                        current_parts.as_deref(),
                        stream_id.as_deref(),
                        None,
                    )
                    .await
                }
            }
        } else if let Some(ref router) = self.llm_router {
            let (idle_agents, limits, dag_prefer) = self.build_planner_inputs();
            let active_tasks_block = owner_id.and_then(|id| self.build_active_tasks_block(id));

            let planner_start = Instant::now();
            let plan_result = TaskPlanner::plan_hierarchical(
                router,
                self.compose_engine.as_ref(),
                &model_input_content,
                &idle_agents,
                &ctx.recent_messages,
                ctx.summary.as_deref(),
                active_tasks_block.as_deref(),
                limits,
                dag_prefer,
            )
            .await;
            planner_ms = planner_start.elapsed().as_millis() as u64;

            match plan_result {
                Ok(plan) => {
                    auto_promotion_reason = plan.auto_promotion_reason.clone();
                    match plan.classification.as_str() {
                        "simple_query" => {
                            mode = "planner_simple_query".to_string();
                            self.bus.publish(SystemEvent::IntentClassified {
                                request_id,
                                intent_type: "simple_query".to_string(),
                                timestamp: Utc::now(),
                            });
                            self.handle_simple_query(
                                request_id,
                                &source,
                                &model_input_content,
                                &intent_source_content,
                                &principal,
                                &scope,
                                &lane_key,
                                &ctx,
                                owner_id,
                                &scope_ctx,
                                current_parts.as_deref(),
                                stream_id.as_deref(),
                                None,
                            )
                            .await
                        }
                        "complex_task" => {
                            mode = "planner_complex_task".to_string();
                            self.bus.publish(SystemEvent::IntentClassified {
                                request_id,
                                intent_type: "complex_task".to_string(),
                                timestamp: Utc::now(),
                            });
                            let description = &model_input_content;
                            let augmented = self.augment_with_context(description, &ctx);

                            let dispatch_start = Instant::now();
                            let dispatch_result = self.task_dispatcher.dispatch_planned(
                                request_id,
                                &augmented,
                                plan,
                                &principal_id(&principal),
                                &lane_key,
                                &source,
                                scope_ctx.workspace_id.clone(),
                            );
                            dispatch_ms = dispatch_start.elapsed().as_millis() as u64;

                            match dispatch_result {
                                Ok(outcome) => {
                                    self.record_delegation(request_id, &outcome);
                                    Ok(outcome.ack)
                                }
                                Err(e) => {
                                    fallback_reason = Some(format!("dispatch_planned_failed: {e}"));
                                    tracing::warn!(
                                        "Dispatch planned failed: {e}, falling back to simple_query"
                                    );
                                    self.handle_simple_query(
                                        request_id,
                                        &source,
                                        &model_input_content,
                                        &intent_source_content,
                                        &principal,
                                        &scope,
                                        &lane_key,
                                        &ctx,
                                        owner_id,
                                        &scope_ctx,
                                        current_parts.as_deref(),
                                        stream_id.as_deref(),
                                        None,
                                    )
                                    .await
                                }
                            }
                        }
                        other => {
                            mode = "planner_unknown".to_string();
                            fallback_reason = Some(format!("unknown_classification: {other}"));
                            tracing::warn!(
                                "LLM planner returned unknown classification '{}', falling back to heuristic",
                                other
                            );
                            self.dispatch_with_heuristic(
                                request_id,
                                &source,
                                &intent_source_content,
                                &model_input_content,
                                &principal,
                                &scope,
                                &lane_key,
                                &ctx,
                                owner_id,
                                &scope_ctx,
                                current_parts.as_deref(),
                                None, // re-parse on rare fallback path
                                stream_id.as_deref(),
                            )
                            .await
                        }
                    }
                }
                Err(e) => {
                    mode = "planner_failed".to_string();
                    fallback_reason = Some(format!("planning_error: {e}"));
                    tracing::warn!("LLM planning failed: {}, falling back to heuristic", e);
                    self.dispatch_with_heuristic(
                        request_id,
                        &source,
                        &intent_source_content,
                        &model_input_content,
                        &principal,
                        &scope,
                        &lane_key,
                        &ctx,
                        owner_id,
                        &scope_ctx,
                        current_parts.as_deref(),
                        None, // re-parse on rare fallback path
                        stream_id.as_deref(),
                    )
                    .await
                }
            }
        } else {
            mode = "no_llm".to_string();
            self.dispatch_with_heuristic(
                request_id,
                &source,
                &intent_source_content,
                &model_input_content,
                &principal,
                &scope,
                &lane_key,
                &ctx,
                owner_id,
                &scope_ctx,
                current_parts.as_deref(),
                Some(intent.clone()), // reuse cached intent (Opt-6)
                stream_id.as_deref(),
            )
            .await
        };

        let ack_ms = ack_start.elapsed().as_millis() as u64;

        // Emit OrchestrationStage event
        self.bus.publish(SystemEvent::OrchestrationStage {
            request_id,
            mode: mode.clone(),
            planner_ms,
            dispatch_ms,
            ack_ms,
            fallback_reason: fallback_reason.clone(),
            auto_promotion_reason: auto_promotion_reason.clone(),
            timestamp: Utc::now(),
        });

        // Persist latency record (best-effort)
        if let Some(ref db) = self.db {
            let repo = OrchestratorLatencyRepository::new(db);
            if let Err(e) = repo.record(&OrchestratorLatencyRecord {
                id: None,
                request_id: request_id.to_string(),
                mode,
                planner_ms,
                dispatch_ms,
                ack_ms,
                fallback_reason,
                auto_promotion_reason,
                timestamp: None,
            }) {
                tracing::debug!("Failed to persist orchestrator latency: {e}");
            }
        }

        // 6 + 7. Summary update and user trait extraction run concurrently
        // in a background spawn (fire-and-forget, never blocks the response).
        {
            use super::extraction::extract_user_traits_background;
            use super::summary::update_summary_background;

            let needs_extraction = owner_id.is_some() && result.is_ok();

            // Shared fields (both tasks need these)
            let bg_db = self.db.clone();
            let bg_router = self.llm_router.clone();
            let bg_config = self.daemon_config.clone();
            let bg_lane = lane_key.to_string();

            // Extraction-only fields (clone only when needed)
            let bg_counter = if needs_extraction { Some(self.extraction_turn_counter.clone()) } else { None };
            let bg_embedder = if needs_extraction { self.embedder.clone() } else { None };
            let bg_user_path = if needs_extraction { Some(self.user_path.clone()) } else { None };
            let bg_user_doc = if needs_extraction { Some(self.user_document.clone()) } else { None };
            let bg_persona_version = if needs_extraction { Some(self.persona_version.clone()) } else { None };
            let bg_bus = if needs_extraction { Some(self.bus.clone()) } else { None };
            let bg_intent = if needs_extraction { Some(intent_source_content.to_string()) } else { None };
            let bg_owner = owner_id.map(|s| s.to_string());
            let bg_response = result.as_ref().ok().cloned();

            // NOTE: ctx is intentionally moved into this spawn — do not reference it after this block
            tokio::spawn(async move {
                // Clone shared fields for summary (extraction consumes the originals)
                let sum_db = bg_db.clone();
                let sum_router = bg_router.clone();
                let sum_config = bg_config.clone();
                let sum_lane = bg_lane.clone();

                let summary_fut = async {
                    if let (Some(db), Some(router)) = (sum_db, sum_router) {
                        update_summary_background(db, router, sum_config, sum_lane, ctx).await;
                    }
                };

                let extract_fut = async {
                    if let (Some(response_text), Some(owner)) = (&bg_response, &bg_owner)
                        && let (Some(db), Some(router)) = (bg_db, bg_router)
                        && let (
                            Some(counter),
                            Some(user_path),
                            Some(user_doc),
                            Some(persona_ver),
                            Some(bus),
                            Some(intent),
                        ) = (
                            bg_counter,
                            bg_user_path,
                            bg_user_doc,
                            bg_persona_version,
                            bg_bus,
                            bg_intent,
                        )
                    {
                        extract_user_traits_background(
                            db, router, bg_config, counter, bg_embedder,
                            user_path, user_doc, persona_ver, bus,
                            bg_lane, intent, response_text.clone(), owner.clone(),
                        )
                        .await;
                    }
                };

                tokio::join!(summary_fut, extract_fut);
            });
        }

        // 8. Check if bootstrap onboarding is complete
        self.maybe_complete_bootstrap().await;

        result
    }
}
