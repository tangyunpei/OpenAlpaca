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
    /// `intent_source_content` is used only for intent classification checks.
    /// `model_input_content` is used for LLM calls and context building.
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
                let result = match intent {
                    Intent::TaskQuery { task_id } => self.handle_task_query(task_id, &owner_id_str),
                    Intent::TaskControl {
                        task_id: Some(task_id),
                        action,
                    } => self.handle_task_control(&task_id, &action),
                    Intent::TaskControl {
                        task_id: None,
                        action,
                    } => {
                        // Bare /cancel|/pause|/resume: resolve the target from
                        // the lane's active workflows (Routing V2 Phase 3).
                        self.handle_bare_task_control(&action, &lane_key)
                    }
                    _ => unreachable!(),
                };
                // Task ops skip the routing ladder entirely, but every routed
                // message must still be observable (Routing V2 Phase 3):
                // emit OrchestrationStage + the latency record before returning.
                self.record_orchestration_stage(
                    request_id,
                    "task_ops".to_string(),
                    0,
                    0,
                    ack_start.elapsed().as_millis() as u64,
                    None,
                    None,
                );
                return result;
            }
            _ => {}
        }

        // 4. Build context ONCE for all remaining paths (D6: single dedup location)
        let ctx = self.build_context(&lane_key, &model_input_content);

        // 5. Compute result — deterministic tiers, then the main loop.
        //    Track timing for observability (OrchestrationStage metrics).
        //    `planner_ms`/`dispatch_ms` are kept at 0 for schema stability
        //    (the planner ladder was deleted in Routing V2 Phase 5).
        let mode: String;

        let result: Result<String, String> = if !force_simple_query
            && self.daemon_config.load().orchestrator.routing.steering_enabled
            && let Some(steer_text) = intent_source_content.strip_prefix("/steer ")
        {
            // Deterministic steering override (Routing V2): guaranteed
            // injection into the lane's running workflow, bypassing the
            // model. With steering_enabled=false (rollback) this arm is
            // skipped and "/steer ..." routes like any other message.
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
            // main loop (Routing V2 Phase 0.5).
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
            self.handle_social_query(request_id, &model_input_content, &lane_key, &ctx)
                .await
        } else {
            // Routing V2 main loop: the front door for everything that
            // survived the deterministic tier and the social branch.
            // Chat vs. task vs. steer is the model's tool choice
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
        };

        let ack_ms = ack_start.elapsed().as_millis() as u64;

        // Emit OrchestrationStage event + persist the latency record
        self.record_orchestration_stage(request_id, mode, 0, 0, ack_ms, None, None);

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

    /// Publish an `OrchestrationStage` event and persist the matching
    /// latency record (best-effort). Shared by the routing ladder and the
    /// task-ops early return so every routed message is observable.
    #[allow(clippy::too_many_arguments)]
    fn record_orchestration_stage(
        &self,
        request_id: Uuid,
        mode: String,
        planner_ms: u64,
        dispatch_ms: u64,
        ack_ms: u64,
        fallback_reason: Option<String>,
        auto_promotion_reason: Option<String>,
    ) {
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
    }
}
