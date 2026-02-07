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
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

use crate::tools::ToolRegistry;

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
    tool_registry: Arc<ToolRegistry>,
    db: Option<Database>,
}

impl TaskDispatcher {
    pub fn new(
        shared_context: Arc<SharedContext>,
        lane_manager: Arc<LaneManager>,
        bus: EventBus,
        llm_router: Option<Arc<LlmRouter>>,
        security_gate: Arc<SecurityGate>,
        tool_registry: Arc<ToolRegistry>,
        db: Option<Database>,
    ) -> Self {
        Self {
            shared_context,
            lane_manager,
            bus,
            skill_matcher: SkillMatcher,
            llm_router,
            security_gate,
            tool_registry,
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

        // Validate ALL planned agents are available (pipeline requires every step)
        let mut unavailable: Vec<String> = Vec::new();
        let mut matches: Vec<SkillMatch> = Vec::new();

        for a in &plan.assignments {
            let is_available = self
                .shared_context
                .agent_registry
                .get(&a.agent_id)
                .map(|agent| agent.status.is_available())
                .unwrap_or(false);

            if is_available {
                matches.push(SkillMatch {
                    agent_id: a.agent_id.clone(),
                    agent_name: a.agent_name.clone(),
                    matched_skills: a.matched_skills.clone(),
                    role_description: a.role_description.clone(),
                });
            } else {
                unavailable.push(format!("{} ({})", a.agent_name, a.agent_id));
            }
        }

        if !unavailable.is_empty() {
            return Err(format!(
                "Cannot start pipeline — these agents are unavailable: {}. All agents must be available for a sequential pipeline.",
                unavailable.join(", ")
            ));
        }

        if matches.is_empty() {
            return Err("No agents assigned by planner".to_string());
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
        let mut assignment_ids: HashMap<String, String> = HashMap::new();

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
                    result_output: None,
                };
                if let Err(e) = repo.create_assignment(&assignment) {
                    tracing::warn!("Failed to persist assignment to DB: {e}");
                }
                assignment_ids.insert(skill_match.agent_id.clone(), assignment.id.clone());
            }
        }

        // Collect agents with their assignment IDs and role descriptions for the pipeline
        let agents_with_assignments: Vec<(SubAgent, Option<String>, String)> = matches
            .iter()
            .filter_map(|skill_match| {
                let agent = self.shared_context.agent_registry.get(&skill_match.agent_id)?;
                let assign_id = assignment_ids.get(&skill_match.agent_id).cloned();
                Some((agent, assign_id, skill_match.role_description.clone()))
            })
            .collect();

        // Verify all agents were collected (guard against race between availability check and registry lookup)
        if agents_with_assignments.len() < matches.len() {
            tracing::error!(
                "Pipeline assembly failed: expected {} agents but got {}. Releasing all agents.",
                matches.len(),
                agents_with_assignments.len()
            );
            // Release all agents that were set to Busy
            let now = Utc::now();
            for skill_match in &matches {
                self.shared_context
                    .agent_registry
                    .update_status(&skill_match.agent_id, AgentStatus::Idle);
                self.bus.publish(SystemEvent::AgentStatusChanged {
                    agent_id: skill_match.agent_id.clone(),
                    status: "idle".to_string(),
                    current_task_id: None,
                    timestamp: now,
                });
            }
            self.shared_context
                .task_registry
                .update_status(&task_id, TaskEntryStatus::Failed);
            return Err(
                "Pipeline assembly failed: some agents became unavailable".to_string(),
            );
        }

        // Spawn sequential pipeline (agents run in step_order, each receives previous output)
        self.spawn_agent_pipeline(
            task_id.clone(),
            title.clone(),
            description.to_string(),
            agents_with_assignments,
            lane_key.to_string(),
        );

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

    /// Spawn a sequential pipeline: agents run in step_order, each receiving
    /// the previous agent's output as additional context.
    fn spawn_agent_pipeline(
        &self,
        task_id: String,
        task_title: String,
        description: String,
        agents_with_assignments: Vec<(SubAgent, Option<String>, String)>,
        lane_key: String,
    ) {
        let router = match &self.llm_router {
            Some(r) => r.clone(),
            None => {
                tracing::warn!(
                    "No LLM router configured — cannot execute pipeline for task '{}'",
                    task_id
                );
                return;
            }
        };

        let bus = self.bus.clone();
        let ctx = self.shared_context.clone();
        let db = self.db.clone();
        let security_gate = self.security_gate.clone();
        let tool_registry = self.tool_registry.clone();

        tokio::spawn(async move {
            let start_time = std::time::Instant::now();
            let total_agents = agents_with_assignments.len();

            // 1. Update task status → Running
            ctx.task_registry.update_status(&task_id, TaskEntryStatus::Running);
            bus.publish(SystemEvent::TaskUpdated {
                task_id: task_id.clone(),
                status: "running".to_string(),
                progress_current: Some(0),
                progress_total: Some(total_agents as i32),
                timestamp: Utc::now(),
            });
            if let Some(ref db) = db {
                let repo = openalpaca_storage::repository::TaskRepository::new(db);
                let _ = repo.update_status(&task_id, openalpaca_storage::TaskStatus::Running);
                let _ = repo.update_progress(&task_id, 0, total_agents as i32);
            }

            // 2. Run agents sequentially — each receives the previous agent's output
            let mut previous_output: Option<String> = None;
            let mut pipeline_success = true;
            let mut pipeline_error: Option<String> = None;
            let mut final_content = String::new();
            let mut last_processed_step = 0usize;
            let mut total_input_tokens: u32 = 0;
            let mut total_output_tokens: u32 = 0;

            for (step, (agent, assignment_id, role_description)) in agents_with_assignments.iter().enumerate() {
                last_processed_step = step;
                let agent_id = &agent.id;

                tracing::info!(
                    "Pipeline step {}/{}: agent '{}' starting on task '{}'",
                    step + 1, total_agents, agent_id, task_id
                );

                // Assignment → Running
                if let (Some(db), Some(assign_id)) = (&db, assignment_id) {
                    let repo = openalpaca_storage::repository::TaskRepository::new(db);
                    let _ = repo.update_assignment_status(
                        assign_id,
                        openalpaca_storage::AssignmentStatus::Running,
                    );
                }

                // Emit progress event for this step
                bus.publish(SystemEvent::TaskUpdated {
                    task_id: task_id.clone(),
                    status: "running".to_string(),
                    progress_current: Some((step + 1) as i32),
                    progress_total: Some(total_agents as i32),
                    timestamp: Utc::now(),
                });
                if let Some(ref db) = db {
                    let repo = openalpaca_storage::repository::TaskRepository::new(db);
                    let _ = repo.update_progress(&task_id, (step + 1) as i32, total_agents as i32);
                }

                // Build LoopConfig
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

                let sandbox_policy =
                    SandboxPolicy::from_constraints(agent_id, &agent.constraints);

                // Resolve tools
                let skill_names: Vec<String> =
                    agent.skills.iter().map(|s| s.name.clone()).collect();
                let tools = tool_registry.definitions_for_skills(&skill_names);
                tracing::info!(
                    "Agent '{}' loaded {} tool definitions for skills: {:?}",
                    agent_id, tools.len(), skill_names
                );

                // Build system prompt with role description and tool awareness
                let tool_list: String = tools
                    .iter()
                    .map(|t| format!("- {}: {}", t.name, t.description))
                    .collect::<Vec<_>>()
                    .join("\n");

                let system_prompt = if tool_list.is_empty() {
                    format!(
                        "{}\n\nYour role: {}\n\nComplete your assigned role to the best of your ability.",
                        agent.preset.persona, role_description
                    )
                } else {
                    format!(
                        "{}\n\nYour role: {}\n\nYou have access to the following tools:\n{}\n\nUse these tools to complete your role. Do NOT say you cannot access files or the internet — use the provided tools instead.",
                        agent.preset.persona, role_description, tool_list
                    )
                };

                // Build messages: system + task + optional previous output
                let mut messages = vec![
                    ChatMessage::system(&system_prompt),
                    ChatMessage::user(&description),
                ];

                // For pipeline step > 0, inject previous agent's output as additional context
                if let Some(ref prev) = previous_output {
                    messages.push(ChatMessage::user(&format!(
                        "## Previous Agent Output\n\
                         The previous agent produced the following result. \
                         Use this information to complete your role:\n\n{}",
                        prev
                    )));
                }

                // Run agentic loop for this agent
                let agent_start = std::time::Instant::now();
                let result = run_agentic_loop_routed(
                    router.as_ref(),
                    messages,
                    tools,
                    &loop_config,
                    Some(security_gate.sandbox()),
                    agent_id,
                    Some(&sandbox_policy),
                    Some(&task_id),
                )
                .await;

                let agent_runtime = agent_start.elapsed().as_secs() as i64;
                let now = Utc::now();

                tracing::info!(
                    "Agent '{}' finished step {}/{}: reason={:?}, rounds={}, tokens={}/{}",
                    agent_id, step + 1, total_agents, result.finish_reason,
                    result.rounds_used, result.total_input_tokens, result.total_output_tokens
                );

                let agent_success = matches!(
                    &result.finish_reason,
                    LoopFinishReason::Complete | LoopFinishReason::MaxRounds
                );

                // Accumulate token metrics
                total_input_tokens += result.total_input_tokens;
                total_output_tokens += result.total_output_tokens;

                // Assignment → Completed or Failed
                if let (Some(db), Some(assign_id)) = (&db, assignment_id) {
                    let repo = openalpaca_storage::repository::TaskRepository::new(db);
                    let status = if agent_success {
                        openalpaca_storage::AssignmentStatus::Completed
                    } else {
                        openalpaca_storage::AssignmentStatus::Failed
                    };
                    let _ = repo.update_assignment_status(assign_id, status);
                }

                // Persist per-agent output to DB
                if let (Some(db), Some(assign_id)) = (&db, assignment_id) {
                    let repo = openalpaca_storage::repository::TaskRepository::new(db);
                    let output = result.final_content.chars().take(5000).collect::<String>();
                    let _ = repo.set_assignment_output(assign_id, &output);
                }

                // Record per-agent history and metrics
                if let Some(ref db) = db {
                    let subagent_repo = openalpaca_storage::SubAgentRepository::new(db);
                    let history_entry = openalpaca_storage::AgentTaskHistory {
                        id: Uuid::new_v4().to_string(),
                        agent_id: agent_id.clone(),
                        task_id: task_id.clone(),
                        role: "executor".to_string(),
                        status: if agent_success { "completed" } else { "failed" }
                            .to_string(),
                        runtime_seconds: Some(agent_runtime),
                        completed_at: now,
                    };
                    if let Err(e) = subagent_repo.add_history(&history_entry) {
                        tracing::warn!("Failed to record agent task history: {e}");
                    }
                    if agent_success {
                        let _ =
                            subagent_repo.increment_completed(agent_id, agent_runtime);
                    } else {
                        let _ = subagent_repo.increment_failed(agent_id);
                    }
                }

                // Release this agent back to Idle (available for other tasks)
                ctx.agent_registry.update_status(agent_id, AgentStatus::Idle);
                bus.publish(SystemEvent::AgentStatusChanged {
                    agent_id: agent_id.clone(),
                    status: "idle".to_string(),
                    current_task_id: None,
                    timestamp: now,
                });

                if agent_success {
                    let raw_content = result.final_content.clone();

                    // For display/DB: synthetic summary if agent produced no text
                    let display_content = if raw_content.is_empty() {
                        format!(
                            "Agent completed in {} rounds ({} tool calls, {} tokens used)",
                            result.rounds_used, result.tool_calls_made,
                            result.total_input_tokens + result.total_output_tokens
                        )
                    } else {
                        raw_content.clone()
                    };

                    // Only pass actual content to next agent (not synthetic metadata)
                    if !raw_content.is_empty() {
                        previous_output = Some(raw_content);
                    }
                    // If empty, previous_output stays as-is from prior step

                    final_content = display_content;
                } else {
                    pipeline_success = false;
                    pipeline_error = Some(match &result.finish_reason {
                        LoopFinishReason::CostExceeded => {
                            "Agent cost limit exceeded".to_string()
                        }
                        LoopFinishReason::Error(err) => err.clone(),
                        _ => "Agent failed".to_string(),
                    });
                    break;
                }
            }

            // 3. Release remaining agents that never ran (pipeline broke early)
            let now = Utc::now();
            if !pipeline_success {
                for (step, (agent, _, _)) in agents_with_assignments.iter().enumerate() {
                    if step > last_processed_step {
                        ctx.agent_registry
                            .update_status(&agent.id, AgentStatus::Idle);
                        bus.publish(SystemEvent::AgentStatusChanged {
                            agent_id: agent.id.clone(),
                            status: "idle".to_string(),
                            current_task_id: None,
                            timestamp: now,
                        });
                    }
                }
            }

            // 4. Update task status
            let db_summary = if pipeline_success {
                final_content.chars().take(2000).collect::<String>()
            } else {
                pipeline_error.clone().unwrap_or_default()
            };

            if pipeline_success {
                ctx.task_registry
                    .update_status(&task_id, TaskEntryStatus::Completed);
                if let Some(ref db) = db {
                    let repo = openalpaca_storage::repository::TaskRepository::new(db);
                    let _ = repo.update_status(
                        &task_id,
                        openalpaca_storage::TaskStatus::Completed,
                    );
                    let _ = repo.set_result(&task_id, &db_summary);
                }
                bus.publish(SystemEvent::TaskCompleted {
                    task_id: task_id.clone(),
                    result_summary: Some(db_summary.clone()),
                    timestamp: now,
                });
            } else {
                let err = pipeline_error.clone().unwrap_or_default();
                ctx.task_registry
                    .update_status(&task_id, TaskEntryStatus::Failed);
                if let Some(ref db) = db {
                    let repo = openalpaca_storage::repository::TaskRepository::new(db);
                    let _ =
                        repo.update_status(&task_id, openalpaca_storage::TaskStatus::Failed);
                    let _ = repo.set_result(&task_id, &err);
                }
                bus.publish(SystemEvent::TaskFailed {
                    task_id: task_id.clone(),
                    error: err,
                    timestamp: now,
                });
            }

            // 5. Persist final result to conversation (single message for entire pipeline)
            let runtime_secs = start_time.elapsed().as_secs() as i64;
            if let Some(ref db) = db {
                let chat_text = if pipeline_success {
                    final_content
                } else {
                    pipeline_error.unwrap_or_default()
                };
                let content =
                    format_task_result(&task_title, &chat_text, pipeline_success);
                let conv_repo = ConversationRepository::new(db);
                let _ = conv_repo.insert(&ConversationMessage {
                    id: 0,
                    lane_key: lane_key.clone(),
                    role: "assistant".to_string(),
                    content,
                    model: None,
                    tokens_in: Some(total_input_tokens as i64),
                    tokens_out: Some(total_output_tokens as i64),
                    duration_ms: Some(runtime_secs * 1000),
                    created_at: String::new(),
                });
            }
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
        let tool_registry = Arc::new(crate::tools::ToolRegistry::new());
        let executor = Arc::new(crate::tools::RegistryToolExecutor::new(tool_registry.clone()));
        let sandbox = Arc::new(crate::security::sandbox::SandboxManager::new(executor, bus.clone()));
        let gate = Arc::new(crate::security::gate::SecurityGate::new(sandbox));
        TaskDispatcher::new(ctx, lane_mgr, bus, None, gate, tool_registry, None)
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
