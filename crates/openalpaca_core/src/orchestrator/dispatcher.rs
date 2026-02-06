//! Task dispatcher: creates tasks, assigns agents, starts task lanes.

use crate::agent::subagent::{AgentStatus, SubAgent};
use crate::bus::EventBus;
use crate::context::{SharedContext, TaskEntryStatus};
use crate::events::SystemEvent;
use crate::lane::LaneManager;
use crate::runner::{LoopConfig, LoopFinishReason, run_agentic_loop_routed};
use crate::security::gate::SecurityGate;
use crate::security::sandbox::SandboxPolicy;
use chrono::Utc;
use openalpaca_llm::{ChatMessage, LlmRouter};
use openalpaca_storage::{ConversationMessage, ConversationRepository, Database};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

use super::skill_matcher::{SkillMatch, SkillMatcher};
use super::task_planner::TaskPlan;

/// Dispatches complex tasks by matching skills to agents and creating task lanes.
pub struct TaskDispatcher {
    shared_context: Arc<SharedContext>,
    lane_manager: Arc<LaneManager>,
    bus: EventBus,
    skill_matcher: SkillMatcher,
    llm_router: Option<Arc<LlmRouter>>,
    security_gate: Arc<SecurityGate>,
    db: Option<Database>,
}

impl TaskDispatcher {
    pub fn new(
        shared_context: Arc<SharedContext>,
        lane_manager: Arc<LaneManager>,
        bus: EventBus,
        llm_router: Option<Arc<LlmRouter>>,
        security_gate: Arc<SecurityGate>,
        db: Option<Database>,
    ) -> Self {
        Self {
            shared_context,
            lane_manager,
            bus,
            skill_matcher: SkillMatcher,
            llm_router,
            security_gate,
            db,
        }
    }

    /// Dispatch a complex task using heuristic skill matching:
    /// Matches required skills to idle agents, then delegates to dispatch_core.
    pub fn dispatch(
        &self,
        _request_id: Uuid,
        _source: &str,
        description: &str,
        required_skills: &[String],
        created_by: &str,
        lane_key: &str,
    ) -> Result<String, String> {
        let matches = self
            .skill_matcher
            .match_skills(required_skills, &self.shared_context.agent_registry)?;
        let title = generate_title(description);
        self.dispatch_core(description, title, matches, created_by, lane_key)
    }

    /// Dispatch a complex task using an LLM-generated plan.
    /// Validates that assigned agents exist and are idle, then delegates to dispatch_core.
    pub fn dispatch_planned(
        &self,
        description: &str,
        plan: TaskPlan,
        created_by: &str,
        lane_key: &str,
    ) -> Result<String, String> {
        if plan.assignments.is_empty() {
            return Err("No agents assigned by planner".to_string());
        }

        let matches: Vec<SkillMatch> = plan
            .assignments
            .iter()
            .filter(|a| {
                self.shared_context
                    .agent_registry
                    .get(&a.agent_id)
                    .map(|agent| agent.status.is_available())
                    .unwrap_or(false)
            })
            .map(|a| SkillMatch {
                agent_id: a.agent_id.clone(),
                agent_name: a.agent_name.clone(),
                matched_skills: a.matched_skills.clone(),
                role_description: a.role_description.clone(),
            })
            .collect();

        if matches.is_empty() {
            return Err("All assigned agents are unavailable".to_string());
        }

        let title = plan
            .title
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| generate_title(description));

        self.dispatch_core(description, title, matches, created_by, lane_key)
    }

    /// Core dispatch logic shared by both heuristic and LLM-planned paths.
    fn dispatch_core(
        &self,
        description: &str,
        title: String,
        matches: Vec<SkillMatch>,
        created_by: &str,
        lane_key: &str,
    ) -> Result<String, String> {
        let task_id = Uuid::new_v4().to_string();

        // Register in task_registry
        self.shared_context
            .task_registry
            .register(task_id.clone(), title.clone());

        // Create TaskLane
        let task_lane = self.lane_manager.create_task_lane(&task_id);

        // Assign agents
        let mut assignments = Vec::new();
        let now = Utc::now();
        for skill_match in &matches {
            task_lane.assign_agent(skill_match.agent_id.clone());

            // Update agent status to Busy
            self.shared_context.agent_registry.update_status(
                &skill_match.agent_id,
                AgentStatus::Busy {
                    task_id: task_id.clone(),
                },
            );

            // Emit AgentStatusChanged
            self.bus.publish(SystemEvent::AgentStatusChanged {
                agent_id: skill_match.agent_id.clone(),
                status: "busy".to_string(),
                current_task_id: Some(task_id.clone()),
                timestamp: now,
            });

            assignments.push(serde_json::json!({
                "agent_id": skill_match.agent_id,
                "agent_name": skill_match.agent_name,
                "matched_skills": skill_match.matched_skills,
                "role": skill_match.role_description,
            }));
        }

        // Emit TaskCreated
        self.bus.publish(SystemEvent::TaskCreated {
            task_id: task_id.clone(),
            title: title.clone(),
            created_by: created_by.to_string(),
            timestamp: now,
        });

        // Persist task and assignments to DB
        if let Some(ref db) = self.db {
            let repo = openalpaca_storage::repository::TaskRepository::new(db);
            let task = openalpaca_storage::Task {
                id: task_id.clone(),
                title: title.clone(),
                description: Some(description.to_string()),
                status: openalpaca_storage::TaskStatus::Queued,
                priority: 0,
                progress_current: None,
                progress_total: None,
                result_summary: None,
                created_by: created_by.to_string(),
                source_lane: lane_key.to_string(),
                created_at: now,
                updated_at: now,
                completed_at: None,
            };
            if let Err(e) = repo.create(&task) {
                tracing::warn!("Failed to persist task to DB: {e}");
            }

            for (i, skill_match) in matches.iter().enumerate() {
                let assignment = openalpaca_storage::TaskAgentAssignment {
                    id: Uuid::new_v4().to_string(),
                    task_id: task_id.clone(),
                    agent_id: skill_match.agent_id.clone(),
                    role: skill_match.role_description.clone(),
                    status: openalpaca_storage::AssignmentStatus::Pending,
                    step_order: Some(i as i32),
                    started_at: None,
                    completed_at: None,
                };
                if let Err(e) = repo.create_assignment(&assignment) {
                    tracing::warn!("Failed to persist assignment to DB: {e}");
                }
            }
        }

        // Spawn background execution for each assigned agent
        for skill_match in &matches {
            if let Some(agent) = self.shared_context.agent_registry.get(&skill_match.agent_id) {
                self.spawn_agent_execution(task_id.clone(), title.clone(), description.to_string(), agent, lane_key.to_string());
            }
        }

        // Build human-readable response for chat
        let agent_list: Vec<String> = assignments.iter().map(|a| {
            format!("- {} ({})", a["agent_name"].as_str().unwrap_or("Unknown"),
                    a["matched_skills"].as_array().map(|s|
                        s.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(", ")
                    ).unwrap_or_default())
        }).collect();

        Ok(format!(
            "I've created a task and assigned it to the following agents:\n\n{}\n\nTask: {}\nYou'll see the results here when the task completes.",
            agent_list.join("\n"), title
        ))
    }

    /// Spawn a background task to run an agent's agentic loop.
    fn spawn_agent_execution(&self, task_id: String, task_title: String, description: String, agent: SubAgent, lane_key: String) {
        let router = match &self.llm_router {
            Some(r) => r.clone(),
            None => {
                tracing::warn!(
                    "No LLM router configured — cannot execute agent '{}' for task '{}'",
                    agent.id, task_id
                );
                return;
            }
        };

        let bus = self.bus.clone();
        let ctx = self.shared_context.clone();
        let db = self.db.clone();
        let security_gate = self.security_gate.clone();
        let agent_id = agent.id.clone();

        tokio::spawn(async move {
            let start_time = std::time::Instant::now();

            // 1. Update status → Running (in-memory + DB + event)
            ctx.task_registry.update_status(&task_id, TaskEntryStatus::Running);

            bus.publish(SystemEvent::TaskUpdated {
                task_id: task_id.clone(),
                status: "running".to_string(),
                progress_current: None,
                progress_total: None,
                timestamp: Utc::now(),
            });

            if let Some(ref db) = db {
                let repo = openalpaca_storage::repository::TaskRepository::new(db);
                let _ = repo.update_status(&task_id, openalpaca_storage::TaskStatus::Running);
            }

            // 2. Build LoopConfig from agent constraints
            let loop_config = LoopConfig {
                max_rounds: 15,
                max_tools_per_round: 5,
                max_tool_runtime: Duration::from_secs(
                    agent.constraints.timeout_seconds.unwrap_or(60),
                ),
                max_cost: agent.constraints.max_cost_per_task.unwrap_or(1.0),
                model: agent.llm_config.model.clone(),
                fallback_models: agent.llm_config.fallback_models.clone(),
            };

            // 3. Build SandboxPolicy
            let sandbox_policy = SandboxPolicy::from_constraints(&agent_id, &agent.constraints);

            // 4. Build messages
            let system_prompt = format!(
                "{}\n\nYou have been assigned a task. Complete it to the best of your ability.\n\nTask: {}",
                agent.preset.persona, description
            );
            let messages = vec![
                ChatMessage::system(&system_prompt),
                ChatMessage::user(&description),
            ];

            // 5. Run agentic loop
            tracing::info!(
                "Starting agentic loop for agent '{}' on task '{}'",
                agent_id, task_id
            );

            let result = run_agentic_loop_routed(
                router.as_ref(),
                messages,
                vec![], // Tools will be provided by the sandbox
                &loop_config,
                Some(security_gate.sandbox()),
                &agent_id,
                Some(&sandbox_policy),
                Some(&task_id),
            )
            .await;

            tracing::info!(
                "Agent '{}' finished task '{}': reason={:?}, rounds={}, tokens={}/{}",
                agent_id, task_id, result.finish_reason,
                result.rounds_used, result.total_input_tokens, result.total_output_tokens
            );

            // 6. Update completion status
            let now = Utc::now();
            match result.finish_reason {
                LoopFinishReason::Complete | LoopFinishReason::MaxRounds => {
                    // Task completed
                    let summary = if result.final_content.is_empty() {
                        format!(
                            "Completed in {} rounds ({} tool calls)",
                            result.rounds_used, result.tool_calls_made
                        )
                    } else {
                        result.final_content.chars().take(1000).collect()
                    };

                    ctx.task_registry.update_status(&task_id, TaskEntryStatus::Completed);

                    if let Some(ref db) = db {
                        let repo = openalpaca_storage::repository::TaskRepository::new(db);
                        let _ = repo.update_status(&task_id, openalpaca_storage::TaskStatus::Completed);
                        let _ = repo.set_result(&task_id, &summary);
                    }

                    bus.publish(SystemEvent::TaskCompleted {
                        task_id: task_id.clone(),
                        result_summary: Some(summary),
                        timestamp: now,
                    });
                }
                LoopFinishReason::CostExceeded => {
                    let error_msg = "Agent cost limit exceeded".to_string();
                    ctx.task_registry.update_status(&task_id, TaskEntryStatus::Failed);

                    if let Some(ref db) = db {
                        let repo = openalpaca_storage::repository::TaskRepository::new(db);
                        let _ = repo.update_status(&task_id, openalpaca_storage::TaskStatus::Failed);
                        let _ = repo.set_result(&task_id, &error_msg);
                    }

                    bus.publish(SystemEvent::TaskFailed {
                        task_id: task_id.clone(),
                        error: error_msg,
                        timestamp: now,
                    });
                }
                LoopFinishReason::Error(ref err) => {
                    ctx.task_registry.update_status(&task_id, TaskEntryStatus::Failed);

                    if let Some(ref db) = db {
                        let repo = openalpaca_storage::repository::TaskRepository::new(db);
                        let _ = repo.update_status(&task_id, openalpaca_storage::TaskStatus::Failed);
                        let _ = repo.set_result(&task_id, err);
                    }

                    bus.publish(SystemEvent::TaskFailed {
                        task_id: task_id.clone(),
                        error: err.clone(),
                        timestamp: now,
                    });
                }
            }

            // 7. Persist task result to conversation so it appears in chat
            let runtime_secs = start_time.elapsed().as_secs() as i64;
            if let Some(ref db) = db {
                let (is_success, summary_text) = match result.finish_reason {
                    LoopFinishReason::Complete | LoopFinishReason::MaxRounds => {
                        let s = if result.final_content.is_empty() {
                            format!("Completed in {} rounds ({} tool calls)", result.rounds_used, result.tool_calls_made)
                        } else {
                            result.final_content.chars().take(1000).collect()
                        };
                        (true, s)
                    }
                    LoopFinishReason::CostExceeded => (false, "Agent cost limit exceeded".to_string()),
                    LoopFinishReason::Error(ref err) => (false, err.clone()),
                };
                let content = format_task_result(&task_title, &summary_text, is_success);
                let conv_repo = ConversationRepository::new(db);
                let _ = conv_repo.insert(&ConversationMessage {
                    id: 0,
                    lane_key: lane_key.clone(),
                    role: "assistant".to_string(),
                    content,
                    model: None,
                    tokens_in: None,
                    tokens_out: None,
                    duration_ms: Some(runtime_secs * 1000),
                    created_at: String::new(),
                });
            }

            // 8. Record agent task history and update metrics
            let history_status = match result.finish_reason {
                LoopFinishReason::Complete | LoopFinishReason::MaxRounds => "completed",
                _ => "failed",
            };

            if let Some(ref db) = db {
                let subagent_repo = openalpaca_storage::SubAgentRepository::new(db);
                let history_entry = openalpaca_storage::AgentTaskHistory {
                    id: Uuid::new_v4().to_string(),
                    agent_id: agent_id.clone(),
                    task_id: task_id.clone(),
                    role: "executor".to_string(),
                    status: history_status.to_string(),
                    runtime_seconds: Some(runtime_secs),
                    completed_at: now,
                };
                if let Err(e) = subagent_repo.add_history(&history_entry) {
                    tracing::warn!("Failed to record agent task history: {e}");
                }

                // Update agent metrics
                if history_status == "completed" {
                    let _ = subagent_repo.increment_completed(&agent_id, runtime_secs);
                } else {
                    let _ = subagent_repo.increment_failed(&agent_id);
                }
            }

            // 8. Reset agent status to Idle
            ctx.agent_registry.update_status(&agent_id, AgentStatus::Idle);
            bus.publish(SystemEvent::AgentStatusChanged {
                agent_id: agent_id.clone(),
                status: "idle".to_string(),
                current_task_id: None,
                timestamp: now,
            });
        });
    }
}

/// Generate a concise task title from a description by stripping filler prefixes
/// and truncating to a reasonable length.
fn generate_title(description: &str) -> String {
    let lower = description.to_lowercase();
    // Strip filler prefixes
    let cleaned = lower
        .trim_start_matches("can you ")
        .trim_start_matches("could you ")
        .trim_start_matches("please ")
        .trim_start_matches("help me ")
        .trim_start_matches("i need to ")
        .trim_start_matches("i want to ");
    // Capitalize first letter
    let mut chars = cleaned.chars();
    let title: String = match chars.next() {
        Some(c) => c.to_uppercase().to_string() + chars.as_str(),
        None => description.to_string(),
    };
    // Take first 8 words or 50 chars
    let words: Vec<&str> = title.split_whitespace().take(8).collect();
    let result = words.join(" ");
    if result.len() > 50 {
        format!("{}...", &result[..47])
    } else if words.len() == 8 && title.split_whitespace().count() > 8 {
        format!("{}...", result)
    } else {
        result
    }
}

/// Format a task result for display in the chat conversation.
fn format_task_result(title: &str, summary: &str, is_success: bool) -> String {
    if is_success {
        format!("**Task completed: {}**\n\n{}", title, summary)
    } else {
        format!("**Task failed: {}**\n\n{}", title, summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::subagent::{AgentConstraints, AgentLlmConfig, AgentPreset, AgentStatus, Skill, SubAgent};

    fn make_agent(id: &str, skills: Vec<&str>) -> SubAgent {
        SubAgent {
            id: id.to_string(),
            name: format!("Agent {}", id),
            description: Some(format!("{} agent", id)),
            icon: None,
            status: AgentStatus::Idle,
            current_task: None,
            skills: skills
                .into_iter()
                .map(|s| Skill {
                    name: s.to_string(),
                    category: "test".to_string(),
                    proficiency: 1.0,
                })
                .collect(),
            preset: AgentPreset::default(),
            constraints: AgentConstraints::default(),
            llm_config: AgentLlmConfig::default(),
        }
    }

    fn setup(agents: Vec<SubAgent>) -> TaskDispatcher {
        let ctx = Arc::new(SharedContext::new());
        for a in agents {
            ctx.agent_registry.register(a);
        }
        let lane_mgr = Arc::new(LaneManager::new());
        let bus = EventBus::default();
        let stub_executor = Arc::new(crate::runner::StubToolExecutor);
        let sandbox = Arc::new(crate::security::sandbox::SandboxManager::new(stub_executor, bus.clone()));
        let gate = Arc::new(crate::security::gate::SecurityGate::new(sandbox));
        TaskDispatcher::new(ctx, lane_mgr, bus, None, gate, None)
    }

    #[test]
    fn test_creates_task_and_lane() {
        let dispatcher = setup(vec![make_agent("a1", vec!["web_search"])]);
        let result = dispatcher.dispatch(
            Uuid::new_v4(),
            "cli",
            "Search for Rust docs",
            &["web_search".to_string()],
            "user1",
            "user1:cli",
        );

        assert!(result.is_ok());
        let text = result.unwrap();
        // Response is now human-readable, not JSON
        assert!(text.contains("assigned"));
        assert!(text.contains("Agent a1"));

        // Verify task registered
        assert_eq!(dispatcher.shared_context.task_registry.count(), 1);
        // Verify lane created
        assert_eq!(dispatcher.lane_manager.task_count(), 1);
    }

    #[test]
    fn test_fails_no_matching() {
        let dispatcher = setup(vec![make_agent("a1", vec!["web_search"])]);
        let result = dispatcher.dispatch(
            Uuid::new_v4(),
            "cli",
            "Generate text",
            &["text_generate".to_string()],
            "user1",
            "user1:cli",
        );
        assert!(result.is_err());
    }

    #[test]
    fn test_assigns_multiple() {
        let dispatcher = setup(vec![
            make_agent("a1", vec!["web_search"]),
            make_agent("a2", vec!["text_generate"]),
        ]);
        let result = dispatcher
            .dispatch(
                Uuid::new_v4(),
                "cli",
                "Research and write report",
                &["web_search".to_string(), "text_generate".to_string()],
                "user1",
                "user1:cli",
            )
            .unwrap();

        // Human-readable response should mention both agents
        assert!(result.contains("Agent a1"));
        assert!(result.contains("Agent a2"));
    }

    #[test]
    fn test_title_generation_short() {
        // Test the generate_title function directly
        assert_eq!(generate_title("Short task"), "Short task");
    }

    #[test]
    fn test_title_generation_strips_filler() {
        assert_eq!(
            generate_title("can you search for Rust docs"),
            "Search for rust docs"
        );
        assert_eq!(
            generate_title("please help me find the answer"),
            "Find the answer"
        );
    }

    #[test]
    fn test_title_generation_truncates_long() {
        let long_desc = "search for the latest version of rust and all related documentation";
        let title = generate_title(long_desc);
        assert!(title.len() <= 53); // 50 + "..."
        assert!(title.ends_with("..."));
    }
}
