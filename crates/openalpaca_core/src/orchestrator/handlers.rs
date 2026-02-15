use super::{principal_id, role_label, ConversationContext, Orchestrator};
use crate::events::SystemEvent;
use crate::memory::scope_context::MemoryScopeContext;
use crate::security::gate::SecurityGate;
use crate::security::policy::{Principal, Scope};
use crate::types::Capability;
use chrono::Utc;
use openalpaca_storage::repository::TaskRepository;
use uuid::Uuid;

use super::intent::Intent;
use super::task_planner::TaskPlanner;

impl Orchestrator {
    /// Handle a user message through the full pipeline:
    /// 1. SecurityGate permission check (wraps TrustGate)
    /// 2. Input sanitization
    /// 3. Try slash commands / task queries (cheap, no LLM)
    /// 4. If LLM router configured: try LLM-based planning
    /// 5. Fallback: keyword heuristic routing
    pub async fn handle_message(
        &self,
        request_id: Uuid,
        source: String,
        content: String,
        principal: Principal,
        scope: Scope,
        lane_key: String,
    ) -> Result<String, String> {
        // 1. Permission check via SecurityGate (wraps TrustGate)
        let capability = Capability {
            name: "chat.respond".to_string(),
        };
        SecurityGate::check_access(&principal, &capability, &scope)?;

        // 2. Input sanitization
        let max_input_len = self.daemon_config.load().security.max_input_length;
        let content = SecurityGate::sanitize_input(&content, Some(max_input_len))?;

        // Extract owner_id from principal (before slash-command early return)
        let owner_id_str = principal_id(&principal);
        let owner_id = match &principal {
            Principal::User { .. } => Some(owner_id_str.as_str()),
            _ => None,
        };

        // Resolve workspace context for memory scoping
        let workspace_id = std::env::current_dir()
            .ok()
            .and_then(|d| crate::memory::workspace::resolve_workspace_id(&d));
        let scope_ctx = MemoryScopeContext::new(workspace_id);

        // 3. Try slash commands, task queries, and skill invocations first
        let intent = self.intent_parser.parse_with_skills(&content, &self.skill_catalog);
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
        let ctx = self.build_context(&lane_key, &content);

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
        let result: Result<String, String> = if let Some(ref router) = self.llm_router {
            let idle_agents = self.shared_context.agent_registry.list_idle();
            match TaskPlanner::plan_hierarchical(
                router,
                &content,
                &idle_agents,
                &ctx.recent_messages,
                ctx.summary.as_deref(),
                active_tasks_block.as_deref(),
            )
            .await
            {
                Ok(plan) => match plan.classification.as_str() {
                    "simple_query" => {
                        self.bus.publish(SystemEvent::IntentClassified {
                            request_id,
                            intent_type: "simple_query".to_string(),
                            timestamp: Utc::now(),
                        });
                        self.handle_simple_query(
                            request_id, &source, &content, &lane_key, &ctx, owner_id, &scope_ctx,
                        )
                        .await
                    }
                    "complex_task" => {
                        self.bus.publish(SystemEvent::IntentClassified {
                            request_id,
                            intent_type: "complex_task".to_string(),
                            timestamp: Utc::now(),
                        });
                        let description = &content;
                        let augmented = self.augment_with_context(description, &ctx);
                        match self.task_dispatcher.dispatch_planned(
                            &augmented,
                            plan,
                            &principal_id(&principal),
                            &lane_key,
                            &source,
                        ) {
                            Ok(response) => Ok(response),
                            Err(e) => {
                                tracing::warn!(
                                    "Dispatch planned failed: {e}, falling back to simple_query"
                                );
                                self.handle_simple_query(
                                    request_id, &source, &content, &lane_key, &ctx, owner_id, &scope_ctx,
                                )
                                .await
                            }
                        }
                    }
                    _other => {
                        tracing::warn!(
                            "LLM planner returned unknown classification '{}', falling back to heuristic",
                            _other
                        );
                        self.dispatch_with_heuristic(
                            request_id, &source, &content, &principal, &lane_key, &ctx, owner_id, &scope_ctx,
                        )
                        .await
                    }
                },
                Err(e) => {
                    tracing::warn!("LLM planning failed: {}, falling back to heuristic", e);
                    self.dispatch_with_heuristic(
                        request_id, &source, &content, &principal, &lane_key, &ctx, owner_id, &scope_ctx,
                    )
                    .await
                }
            }
        } else {
            // No LLM router — keyword heuristic
            self.dispatch_with_heuristic(
                request_id, &source, &content, &principal, &lane_key, &ctx, owner_id, &scope_ctx,
            )
            .await
        };

        // 6. Summary update ONCE, AFTER result, for ALL normal turns (D7)
        self.maybe_update_summary(&lane_key, &ctx).await;

        // 7. Automatic user trait extraction (post-response, fire-and-forget cost)
        if let Ok(ref response_text) = result {
            self.maybe_extract_user_traits(&lane_key, &content, response_text, owner_id)
                .await;
        }

        // 8. Check if bootstrap onboarding is complete
        self.maybe_complete_bootstrap().await;

        result
    }

    /// Augment a task description with conversation context (summary + recent exchanges).
    pub(super) fn augment_with_context(&self, description: &str, ctx: &ConversationContext) -> String {
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
    pub(super) async fn dispatch_with_heuristic(
        &self,
        request_id: Uuid,
        source: &str,
        content: &str,
        principal: &Principal,
        lane_key: &str,
        ctx: &ConversationContext,
        owner_id: Option<&str>,
        scope_ctx: &MemoryScopeContext,
    ) -> Result<String, String> {
        let intent = self.intent_parser.parse_with_skills(content, &self.skill_catalog);

        self.bus.publish(SystemEvent::IntentClassified {
            request_id,
            intent_type: intent.intent_type().to_string(),
            timestamp: Utc::now(),
        });

        match intent {
            Intent::SimpleQuery { query } => {
                self.handle_simple_query(request_id, source, &query, lane_key, ctx, owner_id, scope_ctx)
                    .await
            }
            Intent::TaskQuery { task_id } => {
                self.handle_task_query(task_id, &principal_id(principal))
            }
            Intent::ComplexTask {
                description,
                required_skills,
            } => {
                let augmented = self.augment_with_context(&description, ctx);
                match self.task_dispatcher.dispatch(
                    request_id,
                    source,
                    &augmented,
                    &required_skills,
                    &principal_id(principal),
                    lane_key,
                ) {
                    Ok(response) => Ok(response),
                    Err(e) => {
                        tracing::warn!(
                            "Heuristic dispatch failed: {e}, falling back to simple_query"
                        );
                        self.handle_simple_query(
                            request_id,
                            source,
                            &description,
                            lane_key,
                            ctx,
                            owner_id,
                            scope_ctx,
                        )
                        .await
                    }
                }
            }
            Intent::TaskControl { task_id, action } => self.handle_task_control(&task_id, &action),
            Intent::RememberCommand { content } => {
                self.handle_remember_command(&content, owner_id, scope_ctx).await
            }
            Intent::ForgetCommand { content } => {
                self.handle_forget_command(&content, owner_id).await
            }
            Intent::SkillInvocation { skill_name, query } => {
                self.handle_skill_invocation(
                    request_id, source, &skill_name, &query, lane_key, ctx, owner_id, scope_ctx,
                )
                .await
            }
        }
    }
}
