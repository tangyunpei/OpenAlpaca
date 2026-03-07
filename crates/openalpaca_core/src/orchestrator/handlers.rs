use super::{ConversationContext, Orchestrator, principal_id, role_label, wrap_untrusted_context};
use crate::events::SystemEvent;
use crate::gateway::ResolvedAttachment;
use crate::memory::scope_context::MemoryScopeContext;
use crate::security::gate::SecurityGate;
use crate::security::policy::{Principal, Scope};
use crate::types::Capability;
use base64::Engine as _;
use chrono::Utc;
use openalpaca_llm::{ContentPart, ImageSource};
use openalpaca_storage::repository::TaskRepository;
use openalpaca_storage::repository::orchestrator_latency::{
    OrchestratorLatencyRecord, OrchestratorLatencyRepository,
};
use std::sync::Arc;
use std::time::Instant;
use uuid::Uuid;

use super::intent::Intent;
use super::task_planner::{PlannerLimits, TaskPlanner};

impl Orchestrator {
    /// Build a formatted block of active tasks for planner context injection.
    fn build_active_tasks_block(&self, owner_id: &str) -> Option<String> {
        let db = self.db.as_ref()?;
        let task_repo = TaskRepository::new(db);
        match task_repo.list_active_by_creator(owner_id, 10) {
            Ok(tasks) if !tasks.is_empty() => {
                let mut block = String::from("### ACTIVE TASKS ###\n");
                for t in &tasks {
                    let progress = match (t.progress_current, t.progress_total) {
                        (Some(c), Some(total)) => format!(" [{}/{}]", c, total),
                        _ => String::new(),
                    };
                    block.push_str(&format!(
                        "- [{}] {} ({}{})\n",
                        &t.id[..8.min(t.id.len())],
                        t.title,
                        t.status.as_str(),
                        progress
                    ));
                }
                Some(block)
            }
            _ => None,
        }
    }

    /// Build the idle agents list and planner limits from current config.
    fn build_planner_inputs(&self) -> (Vec<crate::agent::SubAgent>, PlannerLimits, bool) {
        let templates = self.shared_context.agent_registry.list_templates();
        let idle_agents: Vec<crate::agent::SubAgent> = templates
            .iter()
            .map(|t| {
                let mut agent = t.to_subagent(&t.frontmatter.id, "");
                agent.status = crate::agent::AgentStatus::Idle;
                agent.current_task = None;
                agent
            })
            .collect();
        let planner_cfg = &self.daemon_config.load().execution.planner;
        let limits = PlannerLimits {
            timeout_secs: planner_cfg.planning_timeout_secs,
            max_retries: planner_cfg.max_retries,
            max_tokens: planner_cfg.max_tokens,
            plan_protocol_v2_enabled: planner_cfg.plan_protocol_v2_enabled,
        };
        let dag_prefer = planner_cfg.dag_prefer_predictable_enabled;
        (idle_agents, limits, dag_prefer)
    }

    /// Handle a user message through the full pipeline:
    /// 1. SecurityGate permission check (wraps TrustGate)
    /// 2. Input sanitization
    /// 3. Try slash commands / task queries (cheap, no LLM)
    /// 4. If LLM router configured: try LLM-based planning
    /// 5. Fallback: keyword heuristic routing
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
    ) -> Result<String, String> {
        let intent_source_content = content.clone();
        self.handle_message_internal(
            request_id,
            source,
            content,
            intent_source_content,
            false,
            None, // no structured parts for text-only messages
            principal,
            scope,
            lane_key,
            workspace_path,
        )
        .await
    }

    /// Internal message handler that separates the model input from the intent source.
    ///
    /// `intent_source_content` is used only for intent classification/fast-path checks.
    /// `model_input_content` is used for planner/LLM calls and context building.
    #[allow(clippy::too_many_arguments)]
    async fn handle_message_internal(
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

        let result: Result<String, String> = if self.is_bootstrapping() {
            mode = "bootstrap".to_string();
            self.handle_simple_query(
                request_id,
                &source,
                &model_input_content,
                &intent_source_content,
                &lane_key,
                &ctx,
                owner_id,
                &scope_ctx,
                current_parts.as_deref(),
            )
            .await
        } else if force_simple_query {
            mode = "forced_simple_query".to_string();
            self.handle_simple_query(
                request_id,
                &source,
                &model_input_content,
                &intent_source_content,
                &lane_key,
                &ctx,
                owner_id,
                &scope_ctx,
                current_parts.as_deref(),
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
            self.handle_social_query(request_id, &model_input_content, &ctx)
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
                &lane_key,
                &ctx,
                owner_id,
                &scope_ctx,
                current_parts.as_deref(),
            )
            .await
        } else if self.llm_router.is_some()
            && matches!(intent, Intent::SimpleQuery { .. })
            && self
                .daemon_config
                .load()
                .execution
                .planner
                .enhanced_pre_screen_enabled
            && self
                .intent_parser
                .is_enhanced_simple_query(&intent_source_content)
        {
            // Enhanced pre-screen: skip LLM planner for likely simple queries
            mode = "pre_screen_simple".to_string();
            self.bus.publish(SystemEvent::PlannerBypassed {
                request_id,
                reason: "enhanced_pre_screen".to_string(),
                timestamp: Utc::now(),
            });
            self.handle_simple_query(
                request_id,
                &source,
                &model_input_content,
                &intent_source_content,
                &lane_key,
                &ctx,
                owner_id,
                &scope_ctx,
                current_parts.as_deref(),
            )
            .await
        } else if let Some(ref router) = self.llm_router
            && matches!(intent, Intent::SimpleQuery { .. })
            && self.daemon_config.load().execution.planner.two_phase_enabled
        {
            // Two-phase: lightweight LLM classification before full planner
            let planner_cfg = self.daemon_config.load();
            let triage_model = planner_cfg.execution.planner.triage_model.as_deref();
            match TaskPlanner::classify_lightweight(
                router,
                triage_model,
                &intent_source_content,
                10,
            )
            .await
            {
                Ok(ref c) if c == "simple_query" => {
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
                        &lane_key,
                        &ctx,
                        owner_id,
                        &scope_ctx,
                        current_parts.as_deref(),
                    )
                    .await
                }
                _ => {
                    // Fall through to full planner — re-enter via heuristic dispatcher
                    // which handles all remaining intent types including ComplexTask
                    mode = "two_phase_complex".to_string();
                    let (idle_agents, limits, dag_prefer) = self.build_planner_inputs();
                    let active_tasks_block = owner_id.and_then(|id| self.build_active_tasks_block(id));

                    let planner_start = Instant::now();
                    let plan_result = TaskPlanner::plan_hierarchical(
                        router,
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
                                        &lane_key,
                                        &ctx,
                                        owner_id,
                                        &scope_ctx,
                                        current_parts.as_deref(),
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
                                        Ok(response) => Ok(response),
                                        Err(e) => {
                                            fallback_reason = Some(format!(
                                                "two_phase_dispatch_failed: {e}"
                                            ));
                                            self.handle_simple_query(
                                                request_id,
                                                &source,
                                                &model_input_content,
                                                &intent_source_content,
                                                &lane_key,
                                                &ctx,
                                                owner_id,
                                                &scope_ctx,
                                                current_parts.as_deref(),
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
                                        &lane_key,
                                        &ctx,
                                        owner_id,
                                        &scope_ctx,
                                        current_parts.as_deref(),
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
                                &lane_key,
                                &ctx,
                                owner_id,
                                &scope_ctx,
                                current_parts.as_deref(),
                            )
                            .await
                        }
                    }
                }
            }
        } else if let Some(ref router) = self.llm_router {
            let (idle_agents, limits, dag_prefer) = self.build_planner_inputs();
            let active_tasks_block = owner_id.and_then(|id| self.build_active_tasks_block(id));

            let planner_start = Instant::now();
            let plan_result = TaskPlanner::plan_hierarchical(
                router,
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
                                &lane_key,
                                &ctx,
                                owner_id,
                                &scope_ctx,
                                current_parts.as_deref(),
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
                                Ok(response) => Ok(response),
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
                                        &lane_key,
                                        &ctx,
                                        owner_id,
                                        &scope_ctx,
                                        current_parts.as_deref(),
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
                                &lane_key,
                                &ctx,
                                owner_id,
                                &scope_ctx,
                                current_parts.as_deref(),
                                None, // re-parse on rare fallback path
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
                        &lane_key,
                        &ctx,
                        owner_id,
                        &scope_ctx,
                        current_parts.as_deref(),
                        None, // re-parse on rare fallback path
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
                &lane_key,
                &ctx,
                owner_id,
                &scope_ctx,
                current_parts.as_deref(),
                Some(intent.clone()), // reuse cached intent (Opt-6)
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
                        && let (Some(counter), Some(user_path), Some(user_doc), Some(bus), Some(intent)) =
                            (bg_counter, bg_user_path, bg_user_doc, bg_bus, bg_intent)
                    {
                        extract_user_traits_background(
                            db, router, bg_config, counter, bg_embedder,
                            user_path, user_doc, bus,
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

    /// Handle a user message with file attachments.
    ///
    /// Injects attachment context as low-trust blocks before delegating to
    /// the standard `handle_message` pipeline.
    #[allow(clippy::too_many_arguments)]
    pub async fn handle_message_with_attachments(
        &self,
        request_id: Uuid,
        source: String,
        content: String,
        attachments: Vec<ResolvedAttachment>,
        principal: Principal,
        scope: Scope,
        lane_key: String,
        workspace_path: Option<String>,
    ) -> Result<String, String> {
        // 1. Build structured ContentParts from attachments
        let mut parts: Vec<ContentPart> = Vec::new();
        for att in &attachments {
            if att.mime_type.starts_with("image/") {
                match tokio::fs::read(&att.storage_path).await {
                    Ok(bytes) => {
                        let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
                        parts.push(ContentPart::Image {
                            source: ImageSource::Base64 {
                                media_type: att.mime_type.clone(),
                                data: Arc::new(b64),
                            },
                            detail: None,
                        });
                    }
                    Err(e) => {
                        tracing::warn!(
                            file_id = %att.file_id,
                            path = %att.storage_path,
                            "Failed to read image bytes for multimodal input: {e}"
                        );
                        parts.push(ContentPart::Text {
                            text: "[image attached — failed to read image bytes]".to_string(),
                        });
                    }
                }
            } else {
                parts.push(ContentPart::Document {
                    file_id: att.file_id.clone(),
                    filename: att.filename.clone(),
                    mime_type: att.mime_type.clone(),
                    extracted_text: att.extracted_text.clone(),
                });
                if att.extracted_text.is_none() && !att.mime_type.starts_with("audio/") {
                    parts.push(ContentPart::Text {
                        text: "[document attached — text extraction pending]".to_string(),
                    });
                }
            }
        }
        // Add text query as final part
        if !content.trim().is_empty() {
            parts.push(ContentPart::Text {
                text: content.clone(),
            });
        }

        // 2. Build text-only augmented string for intent classification
        //    (intent parser and planner only understand text)
        let mut augmented = String::new();
        for att in &attachments {
            let ctx_block = if let Some(ref text) = att.extracted_text {
                let truncated = text.chars().take(4000).collect::<String>();
                format!(
                    "[File: {} ({})]\n{}",
                    att.filename, att.mime_type, truncated
                )
            } else if att.mime_type.starts_with("image/") || att.mime_type.starts_with("audio/") {
                format!("[File: {} ({})]", att.filename, att.mime_type)
            } else {
                format!(
                    "[File: {} ({})]\n[document attached — text extraction pending]",
                    att.filename, att.mime_type
                )
            };
            let wrapped = wrap_untrusted_context(&ctx_block, "file_attachment", "user_derived");
            augmented.push_str(&wrapped);
            augmented.push('\n');
        }
        augmented.push_str(&content);

        let force_simple_query = content.trim().is_empty() && !attachments.is_empty();

        // 3. Pass BOTH the text augmented string AND the structured parts
        self.handle_message_internal(
            request_id,
            source,
            augmented,
            content,
            force_simple_query,
            Some(parts),
            principal,
            scope,
            lane_key,
            workspace_path,
        )
        .await
    }

    /// Augment a task description with conversation context (summary + recent exchanges).
    pub(super) fn augment_with_context(
        &self,
        description: &str,
        ctx: &ConversationContext,
    ) -> String {
        if let Some(ref summary) = ctx.summary {
            let recent_excerpt: String = ctx
                .recent_messages
                .iter()
                .rev()
                .take(6)
                .rev()
                .map(|m| {
                    format!(
                        "{}: {}",
                        role_label(&m.role),
                        m.content.chars().take(500).collect::<String>()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "{}\n\n## Conversation Context\n{}\n\n## Recent exchanges (last ~6):\n{}",
                description, summary, recent_excerpt
            )
        } else {
            description.to_string()
        }
    }

    /// Fallback dispatch using keyword-based intent classification and greedy skill matching.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn dispatch_with_heuristic(
        &self,
        request_id: Uuid,
        source: &str,
        intent_content: &str,
        model_input_content: &str,
        principal: &Principal,
        lane_key: &str,
        ctx: &ConversationContext,
        owner_id: Option<&str>,
        scope_ctx: &MemoryScopeContext,
        current_parts: Option<&[ContentPart]>,
        cached_intent: Option<Intent>,
    ) -> Result<String, String> {
        let intent = cached_intent.unwrap_or_else(|| {
            self.intent_parser.parse_with_skills_and_router(
                intent_content,
                &self.skill_catalog,
                &self.skill_router,
            )
        });

        self.bus.publish(SystemEvent::IntentClassified {
            request_id,
            intent_type: intent.intent_type().to_string(),
            timestamp: Utc::now(),
        });

        match intent {
            Intent::SimpleQuery { .. } => {
                self.handle_simple_query(
                    request_id,
                    source,
                    model_input_content,
                    intent_content,
                    lane_key,
                    ctx,
                    owner_id,
                    scope_ctx,
                    current_parts,
                )
                .await
            }
            Intent::TaskQuery { task_id } => {
                self.handle_task_query(task_id, &principal_id(principal))
            }
            Intent::ComplexTask {
                required_skills, ..
            } => {
                let augmented = self.augment_with_context(model_input_content, ctx);
                match self.task_dispatcher.dispatch(
                    request_id,
                    source,
                    &augmented,
                    &required_skills,
                    &principal_id(principal),
                    lane_key,
                    scope_ctx.workspace_id.clone(),
                ) {
                    Ok(response) => Ok(response),
                    Err(e) => {
                        tracing::info!(
                            "Heuristic dispatch failed ({e}), trying lead agent fallback"
                        );
                        match self.task_dispatcher.dispatch_lead_agent_heuristic(
                            request_id,
                            &augmented,
                            &principal_id(principal),
                            lane_key,
                            source,
                            scope_ctx.workspace_id.clone(),
                        ) {
                            Ok(response) => Ok(response),
                            Err(e2) => {
                                tracing::warn!(
                                    "Lead agent fallback also failed: {e2}, falling back to simple_query"
                                );
                                self.handle_simple_query(
                                    request_id,
                                    source,
                                    model_input_content,
                                    intent_content,
                                    lane_key,
                                    ctx,
                                    owner_id,
                                    scope_ctx,
                                    current_parts,
                                )
                                .await
                            }
                        }
                    }
                }
            }
            Intent::TaskControl { task_id, action } => self.handle_task_control(&task_id, &action),
            Intent::RememberCommand { content } => {
                self.handle_remember_command(&content, owner_id, scope_ctx)
                    .await
            }
            Intent::ForgetCommand { content } => {
                self.handle_forget_command(&content, owner_id).await
            }
            Intent::SkillInvocation { skill_name, query } => {
                // Capture route metadata for telemetry (re-route is cheap, no side effects)
                let route_result = self.skill_router.route(&query, &self.skill_catalog);
                let was_auto_selected =
                    route_result.selected.as_ref() == Some(&skill_name);
                let route_score = route_result
                    .scores
                    .iter()
                    .find(|s| s.skill_id == skill_name)
                    .map(|s| s.score);

                self.handle_skill_invocation(
                    request_id,
                    source,
                    &skill_name,
                    &query,
                    lane_key,
                    ctx,
                    owner_id,
                    scope_ctx,
                    route_score,
                    was_auto_selected,
                )
                .await
            }
        }
    }

    /// Adapt multimodal content parts for a model's capabilities.
    ///
    /// Replaces unsupported content types with text placeholders based on
    /// the model's capability flags in the registry.
    pub(super) fn adapt_parts_for_model(
        &self,
        parts: Vec<ContentPart>,
        model_id: &str,
    ) -> Vec<ContentPart> {
        let router = match &self.llm_router {
            Some(r) => r,
            None => return parts,
        };
        let registry = router.model_registry();

        let supports_image = registry.supports_image(model_id);
        let supports_audio = registry.supports_audio(model_id);
        let supports_document = registry.supports_document(model_id);

        parts
            .into_iter()
            .map(|part| match &part {
                ContentPart::Image { .. } if !supports_image => ContentPart::Text {
                    text: "[image attached — model does not support vision]".to_string(),
                },
                ContentPart::Audio { .. } if !supports_audio => ContentPart::Text {
                    text: "[audio attached — model does not support audio input]".to_string(),
                },
                ContentPart::Document { .. } if !supports_document => ContentPart::Text {
                    text: "[document attached — model does not support document input]".to_string(),
                },
                _ => part,
            })
            .collect()
    }
}
