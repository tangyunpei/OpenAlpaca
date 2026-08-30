//! Helper methods for the orchestrator message handler: context building,
//! heuristic dispatch, and multimodal adaptation.

use super::{ConversationContext, Orchestrator, principal_id, role_label};
use crate::events::SystemEvent;
use crate::memory::scope_context::MemoryScopeContext;
use chrono::Utc;
use openalpaca_llm::ContentPart;
use openalpaca_storage::repository::TaskRepository;
use uuid::Uuid;

use super::dispatcher::DispatchOutcome;
use super::intent::Intent;
use super::task_planner::PlannerLimits;

impl Orchestrator {
    /// Record structured delegation metadata for the bridge to attach to the
    /// response (mirrors `llm_metadata_map`: inserted here, drained by the
    /// daemon's `build_result`).
    pub(super) fn record_delegation(&self, request_id: Uuid, outcome: &DispatchOutcome) {
        self.delegation_map.insert(
            request_id,
            crate::gateway::DelegationInfo {
                task_id: outcome.task_id.clone(),
                title: outcome.title.clone(),
            },
        );
    }

    /// Build a formatted block of active tasks for planner context injection.
    pub(super) fn build_active_tasks_block(&self, owner_id: &str) -> Option<String> {
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
    pub(super) fn build_planner_inputs(
        &self,
    ) -> (Vec<crate::agent::SubAgent>, PlannerLimits, bool) {
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
        principal: &crate::security::policy::Principal,
        scope: &crate::security::policy::Scope,
        lane_key: &str,
        ctx: &ConversationContext,
        owner_id: Option<&str>,
        scope_ctx: &MemoryScopeContext,
        current_parts: Option<&[ContentPart]>,
        cached_intent: Option<Intent>,
        stream_id: Option<&str>,
    ) -> Result<String, String> {
        let intent = cached_intent.unwrap_or_else(|| {
            self.intent_parser.parse_with_skills_and_router(
                intent_content,
                &self.skill_catalog,
                &self.skill_router,
            )
        });

        // Skill invocations publish IntentClassified inside
        // invoke_skill_with_telemetry (shared with the deterministic tier).
        if !matches!(intent, Intent::SkillInvocation { .. }) {
            self.bus.publish(SystemEvent::IntentClassified {
                request_id,
                intent_type: intent.intent_type().to_string(),
                timestamp: Utc::now(),
            });
        }

        match intent {
            Intent::SimpleQuery { .. } => {
                self.handle_simple_query(
                    request_id,
                    source,
                    model_input_content,
                    intent_content,
                    principal,
                    scope,
                    lane_key,
                    ctx,
                    owner_id,
                    scope_ctx,
                    current_parts,
                    stream_id,
                    None,
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
                    Ok(outcome) => {
                        self.record_delegation(request_id, &outcome);
                        Ok(outcome.ack)
                    }
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
                            Ok(outcome) => {
                                self.record_delegation(request_id, &outcome);
                                Ok(outcome.ack)
                            }
                            Err(e2) => {
                                tracing::warn!(
                                    "Lead agent fallback also failed: {e2}, falling back to simple_query"
                                );
                                self.handle_simple_query(
                                    request_id,
                                    source,
                                    model_input_content,
                                    intent_content,
                                    principal,
                                    scope,
                                    lane_key,
                                    ctx,
                                    owner_id,
                                    scope_ctx,
                                    current_parts,
                                    stream_id,
                                    None,
                                )
                                .await
                            }
                        }
                    }
                }
            }
            Intent::TaskControl { task_id, action } => match task_id {
                Some(id) => self.handle_task_control(&id, &action),
                None => self.handle_bare_task_control(&action, lane_key),
            },
            Intent::RememberCommand { content } => {
                self.handle_remember_command(&content, owner_id, scope_ctx)
                    .await
            }
            Intent::ForgetCommand { content } => {
                self.handle_forget_command(&content, owner_id).await
            }
            Intent::SkillInvocation { skill_name, query } => {
                self.invoke_skill_with_telemetry(
                    request_id,
                    source,
                    &skill_name,
                    &query,
                    lane_key,
                    ctx,
                    owner_id,
                    scope_ctx,
                    stream_id,
                )
                .await
            }
        }
    }

    /// Invoke a skill with route-telemetry capture and IntentClassified emission.
    ///
    /// Shared by the deterministic skill tier in `handle_message_internal` and
    /// the heuristic fallback's `SkillInvocation` arm.
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn invoke_skill_with_telemetry(
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
    ) -> Result<String, String> {
        self.bus.publish(SystemEvent::IntentClassified {
            request_id,
            intent_type: "skill_invocation".to_string(),
            timestamp: Utc::now(),
        });

        // Capture route metadata for telemetry (re-route is cheap, no side effects)
        let route_result = self.skill_router.route(query, &self.skill_catalog);
        let was_auto_selected = route_result.selected.as_deref() == Some(skill_name);
        let route_score = route_result
            .scores
            .iter()
            .find(|s| s.skill_id == skill_name)
            .map(|s| s.score);

        self.handle_skill_invocation(
            request_id,
            source,
            skill_name,
            query,
            lane_key,
            ctx,
            owner_id,
            scope_ctx,
            route_score,
            was_auto_selected,
            stream_id,
        )
        .await
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
