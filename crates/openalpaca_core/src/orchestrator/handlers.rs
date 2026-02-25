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

        // Build active tasks block for planner
        let active_tasks_block = if let Some(ref db) = self.db {
            let task_repo = TaskRepository::new(db);
            match task_repo.list_active_by_creator(&owner_id_str, 10) {
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
        } else {
            None
        };

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
                &lane_key,
                &ctx,
                owner_id,
                &scope_ctx,
                current_parts.as_deref(),
            )
            .await
        } else if self.llm_router.is_some()
            && matches!(intent, Intent::SimpleQuery { .. })
            && self.intent_parser.is_fast_path_eligible(&intent_source_content)
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
                &lane_key,
                &ctx,
                owner_id,
                &scope_ctx,
                current_parts.as_deref(),
            )
            .await
        } else if let Some(ref router) = self.llm_router {
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

            let planner_start = Instant::now();
            let plan_result = TaskPlanner::plan_hierarchical(
                router,
                &model_input_content,
                &idle_agents,
                &ctx.recent_messages,
                ctx.summary.as_deref(),
                active_tasks_block.as_deref(),
                limits,
                planner_cfg.dag_prefer_predictable_enabled,
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
                                    fallback_reason =
                                        Some(format!("dispatch_planned_failed: {e}"));
                                    tracing::warn!(
                                        "Dispatch planned failed: {e}, falling back to simple_query"
                                    );
                                    self.handle_simple_query(
                                        request_id,
                                        &source,
                                        &model_input_content,
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
                            fallback_reason =
                                Some(format!("unknown_classification: {other}"));
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
        // (both are fire-and-forget LLM calls that don't affect the response).
        {
            let summary_fut = self.maybe_update_summary(&lane_key, &ctx);
            let extract_fut = async {
                if let Ok(ref response_text) = result {
                    self.maybe_extract_user_traits(
                        &lane_key,
                        &intent_source_content,
                        response_text,
                        owner_id,
                    )
                    .await;
                }
            };
            tokio::join!(summary_fut, extract_fut);
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
                format!("[File: {} ({})]\n{}", att.filename, att.mime_type, truncated)
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
    ) -> Result<String, String> {
        let intent = self
            .intent_parser
            .parse_with_skills_and_router(intent_content, &self.skill_catalog, &self.skill_router);

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
            Intent::ComplexTask { required_skills, .. } => {
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
                self.handle_skill_invocation(
                    request_id,
                    source,
                    &skill_name,
                    &query,
                    lane_key,
                    ctx,
                    owner_id,
                    scope_ctx,
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
