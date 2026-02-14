//! Lead Agent orchestrator: a full agentic loop that dynamically spawns
//! subagents, observes their results, adjusts strategy, and synthesizes
//! a final response. Follows Anthropic's recommended multi-agent pattern.
//!
//! The Lead Agent is a configurable `SubAgent` registered in the agent
//! registry with its own model, persona, and constraints. It receives a
//! `spawn_subagent` tool that delegates work to other agents.

use crate::agent::subagent::{AgentStatus, SubAgent};
use crate::bus::EventBus;
use crate::context::SharedContext;
use crate::events::SystemEvent;
use crate::middleware::prompt::format_tool_guidance;
use crate::runner::{LoopConfig, LoopResult, run_agentic_loop_routed};
use crate::security::sandbox::{SandboxManager, SandboxPolicy, ToolExecutor};
use crate::tools::registry::BuiltInTool;
use crate::tools::{ContextualToolExecutor, ToolExecutionContext, ToolRegistry};
use async_trait::async_trait;
use chrono::Utc;
use openalpaca_llm::{ChatMessage, LlmRouter, ToolDefinition};
use openalpaca_storage::Database;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

// ── Result types ─────────────────────────────────────────────────────

/// Result of a lead agent execution.
pub struct LeadAgentResult {
    pub success: bool,
    pub final_content: String,
    pub loop_result: LoopResult,
    pub subagents_spawned: usize,
}

// ── SpawnSubagentTool ────────────────────────────────────────────────

/// Built-in tool that allows the Lead Agent to spawn a subagent.
/// Each invocation runs a full agentic loop for the target agent
/// and returns the subagent's final output.
pub struct SpawnSubagentTool {
    router: Arc<LlmRouter>,
    tool_registry: Arc<ToolRegistry>,
    shared_context: Arc<SharedContext>,
    bus: EventBus,
    db: Option<Database>,
    task_id: String,
    created_by: String,
    /// Tracks how many subagents have been spawned (for observability).
    spawn_count: AtomicUsize,
}

impl SpawnSubagentTool {
    pub fn new(
        router: Arc<LlmRouter>,
        tool_registry: Arc<ToolRegistry>,
        shared_context: Arc<SharedContext>,
        bus: EventBus,
        db: Option<Database>,
        task_id: String,
        created_by: String,
    ) -> Self {
        Self {
            router,
            tool_registry,
            shared_context,
            bus,
            db,
            task_id,
            created_by,
            spawn_count: AtomicUsize::new(0),
        }
    }

    pub fn spawn_count(&self) -> usize {
        self.spawn_count.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl BuiltInTool for SpawnSubagentTool {
    async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        // 1. Parse arguments
        let agent_id = arguments
            .get("agent_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: agent_id".to_string())?;
        let objective = arguments
            .get("objective")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: objective".to_string())?;

        // 2. Look up agent from registry
        let agent = self
            .shared_context
            .agent_registry
            .get(agent_id)
            .ok_or_else(|| format!("Agent '{}' not found in registry", agent_id))?;

        if !agent.status.is_available() {
            return Err(format!(
                "Agent '{}' is not available (status: {})",
                agent_id, agent.status
            ));
        }

        // 3. Mark agent as Busy
        self.shared_context.agent_registry.update_status(
            agent_id,
            AgentStatus::Busy {
                task_id: self.task_id.clone(),
            },
        );
        self.bus.publish(SystemEvent::AgentStatusChanged {
            agent_id: agent_id.to_string(),
            status: "busy".to_string(),
            current_task_id: Some(self.task_id.clone()),
            timestamp: Utc::now(),
        });

        // 4. Emit DagNodeStarted (reusing event, node_id = UUID)
        let node_id = Uuid::new_v4().to_string();
        self.bus.publish(SystemEvent::DagNodeStarted {
            task_id: self.task_id.clone(),
            node_id: node_id.clone(),
            node_title: objective.chars().take(80).collect(),
            agent_id: agent_id.to_string(),
            timestamp: Utc::now(),
        });

        self.spawn_count.fetch_add(1, Ordering::SeqCst);
        let agent_start = std::time::Instant::now();

        // 5. Build ContextualToolExecutor + SandboxManager for subagent
        let ctx_exec = ToolExecutionContext {
            owner_id: Some(self.created_by.clone()),
            task_id: Some(self.task_id.clone()),
            agent_id: Some(agent_id.to_string()),
            db: self.db.clone(),
        };
        let contextual_executor = Arc::new(ContextualToolExecutor::new(
            self.tool_registry.clone(),
            ctx_exec,
        ));
        let sandbox = SandboxManager::new(contextual_executor, self.bus.clone());

        // 6. Resolve tools for subagent's skills
        let skill_names: Vec<String> = agent.skills.iter().map(|s| s.name.clone()).collect();
        let mut tools = self.tool_registry.definitions_for_skills(&skill_names);
        // Add workspace tools so subagent can read/write shared workspace
        tools.extend(crate::tools::builtins::workspace_tool_definitions());

        // 7. Build messages with agent persona + objective
        let tool_guidance = format_tool_guidance(&tools);
        let system_prompt = format!(
            "{}\n\nYour role: Complete the following objective to the best of your ability.{}",
            agent.preset.persona, tool_guidance
        );
        let messages = vec![
            ChatMessage::system(&system_prompt),
            ChatMessage::user(objective),
        ];

        // 8. Build LoopConfig from agent's constraints
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

        let sandbox_policy = SandboxPolicy::from_constraints(agent_id, &agent.constraints);

        // 9. Call run_agentic_loop_routed() — blocks until subagent finishes
        let result = run_agentic_loop_routed(
            self.router.as_ref(),
            messages,
            tools,
            &loop_config,
            Some(&sandbox),
            agent_id,
            Some(&sandbox_policy),
            Some(&self.task_id),
        )
        .await;

        let duration_ms = agent_start.elapsed().as_millis() as u64;
        let now = Utc::now();

        let agent_success = matches!(
            &result.finish_reason,
            crate::runner::LoopFinishReason::Complete
                | crate::runner::LoopFinishReason::MaxRounds
        );

        // 10. Mark agent as Idle
        self.shared_context
            .agent_registry
            .update_status(agent_id, AgentStatus::Idle);
        self.bus.publish(SystemEvent::AgentStatusChanged {
            agent_id: agent_id.to_string(),
            status: "idle".to_string(),
            current_task_id: None,
            timestamp: now,
        });

        // 11. Emit DagNodeCompleted
        self.bus.publish(SystemEvent::DagNodeCompleted {
            task_id: self.task_id.clone(),
            node_id: node_id.clone(),
            node_title: objective.chars().take(80).collect(),
            agent_id: agent_id.to_string(),
            success: agent_success,
            duration_ms,
            output_preview: if result.final_content.is_empty() {
                None
            } else {
                Some(result.final_content.chars().take(200).collect())
            },
            timestamp: now,
        });

        // 12. Record LLM usage + agent history
        let actual_model = result
            .model_used
            .as_deref()
            .or(loop_config.model.as_deref())
            .unwrap_or(self.router.default_model());
        let resolved_provider = self
            .router
            .model_registry()
            .resolve_provider(actual_model)
            .map(|p| p.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let call_cost = self.router.cost_tracker.calculate_cost(
            actual_model,
            result.total_input_tokens,
            result.total_output_tokens,
        );

        let call_status = if agent_success { "success" } else { "error" };
        let call_error = match &result.finish_reason {
            crate::runner::LoopFinishReason::Error(msg) => Some(msg.as_str()),
            _ => None,
        };

        if let Some(ref db) = self.db {
            let usage_repo = openalpaca_storage::repository::LlmUsageRepository::new(db);
            if let Err(e) = usage_repo.record_and_log(
                agent_id,
                Some(&self.task_id),
                &resolved_provider,
                actual_model,
                result.total_input_tokens as i32,
                result.total_output_tokens as i32,
                call_cost,
                duration_ms as i64,
                call_status,
                call_error,
            ) {
                tracing::warn!("Failed to persist LLM usage for subagent '{}': {e}", agent_id);
            }

            // Agent task history
            let subagent_repo = openalpaca_storage::SubAgentRepository::new(db);
            let history_entry = openalpaca_storage::AgentTaskHistory {
                id: Uuid::new_v4().to_string(),
                agent_id: agent_id.to_string(),
                task_id: self.task_id.clone(),
                role: "subagent".to_string(),
                status: if agent_success {
                    "completed"
                } else {
                    "failed"
                }
                .to_string(),
                runtime_seconds: Some(duration_ms as i64 / 1000),
                completed_at: now,
            };
            if let Err(e) = subagent_repo.add_history(&history_entry) {
                tracing::warn!("Failed to record agent task history: {e}");
            }
            if agent_success {
                let _ = subagent_repo.increment_completed(agent_id, duration_ms as i64 / 1000);
            } else {
                let _ = subagent_repo.increment_failed(agent_id);
            }
        }

        tracing::info!(
            "Subagent '{}' completed objective '{}': success={}, rounds={}, tokens={}/{}, duration={}ms",
            agent_id,
            &objective.chars().take(50).collect::<String>(),
            agent_success,
            result.rounds_used,
            result.total_input_tokens,
            result.total_output_tokens,
            duration_ms,
        );

        // 13. Return subagent's final_content (or error)
        if agent_success {
            if result.final_content.is_empty() {
                Ok(format!(
                    "Agent '{}' completed in {} rounds ({} tool calls) but produced no text output.",
                    agent_id, result.rounds_used, result.tool_calls_made
                ))
            } else {
                Ok(result.final_content)
            }
        } else {
            Err(format!(
                "Agent '{}' failed: {:?}. Last output: {}",
                agent_id,
                result.finish_reason,
                result
                    .final_content
                    .chars()
                    .take(500)
                    .collect::<String>()
            ))
        }
    }
}

/// Build the tool definition for `spawn_subagent`, dynamically listing available agents.
pub fn spawn_subagent_tool_definition(available_agents: &[SubAgent]) -> ToolDefinition {
    let agent_descriptions: Vec<String> = available_agents
        .iter()
        .map(|a| {
            let skills: Vec<&str> = a.skills.iter().map(|s| s.name.as_str()).collect();
            let desc = a.description.as_deref().unwrap_or("No description");
            format!(
                "- ID: \"{}\", Name: \"{}\", Skills: [{}], Description: \"{}\"",
                a.id,
                a.name,
                skills.join(", "),
                desc
            )
        })
        .collect();

    let agents_list = if agent_descriptions.is_empty() {
        "No agents available.".to_string()
    } else {
        agent_descriptions.join("\n")
    };

    ToolDefinition {
        name: "spawn_subagent".to_string(),
        description: format!(
            "Spawn a subagent to work on a specific objective. The subagent runs autonomously \
             and returns its result. Choose the right agent based on skills.\n\n\
             Available agents:\n{}",
            agents_list
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "agent_id": {
                    "type": "string",
                    "description": "The ID of the agent to spawn (must be from the available agents list)"
                },
                "objective": {
                    "type": "string",
                    "description": "A clear, specific objective for the subagent to accomplish"
                }
            },
            "required": ["agent_id", "objective"]
        }),
    }
}

// ── LeadAgentToolExecutor ────────────────────────────────────────────

/// A ToolExecutor that routes `spawn_subagent` to the SpawnSubagentTool
/// and all other tools (workspace_read, workspace_write, owner-scoped)
/// to the ContextualToolExecutor.
pub struct LeadAgentToolExecutor {
    spawn_tool: Arc<SpawnSubagentTool>,
    contextual_executor: Arc<ContextualToolExecutor>,
}

impl LeadAgentToolExecutor {
    pub fn new(
        spawn_tool: Arc<SpawnSubagentTool>,
        contextual_executor: Arc<ContextualToolExecutor>,
    ) -> Self {
        Self {
            spawn_tool,
            contextual_executor,
        }
    }
}

#[async_trait]
impl ToolExecutor for LeadAgentToolExecutor {
    async fn execute(
        &self,
        tool_name: &str,
        arguments: &serde_json::Value,
    ) -> Result<String, String> {
        match tool_name {
            "spawn_subagent" => self.spawn_tool.execute(arguments).await,
            _ => self.contextual_executor.execute(tool_name, arguments).await,
        }
    }

    fn registered_tools(&self) -> Vec<String> {
        let mut tools = self.contextual_executor.registered_tools();
        tools.push("spawn_subagent".to_string());
        tools
    }
}

// ── build_lead_agent_prompt ──────────────────────────────────────────

/// Build the system prompt for the Lead Agent, including its persona
/// and the list of available worker agents.
pub fn build_lead_agent_prompt(
    base_persona: &str,
    available_agents: &[SubAgent],
) -> String {
    let mut prompt = String::with_capacity(2048);

    // Base persona
    prompt.push_str(base_persona);
    prompt.push_str("\n\n");

    // Orchestration instructions
    prompt.push_str(
        "## Orchestration Role\n\
         You are a Lead Agent orchestrating a complex task. Your job is to:\n\
         1. Analyze the user's request and break it into sub-objectives\n\
         2. Delegate work by spawning subagents using the `spawn_subagent` tool\n\
         3. Observe each subagent's output and adjust your strategy\n\
         4. Synthesize all results into a comprehensive final response\n\n",
    );

    // Available agents block
    prompt.push_str("## Available Agents\n");
    if available_agents.is_empty() {
        prompt.push_str("No worker agents are currently available.\n");
    } else {
        for agent in available_agents {
            let desc = agent.description.as_deref().unwrap_or("No description");
            let skills: Vec<String> = agent
                .skills
                .iter()
                .map(|s| s.name.clone())
                .collect();
            let skills_str = if skills.is_empty() {
                "none".to_string()
            } else {
                skills.join(", ")
            };
            prompt.push_str(&format!(
                "- **{}** (ID: `{}`): {} | Skills: {}\n",
                agent.name, agent.id, desc, skills_str
            ));
        }
    }
    prompt.push('\n');

    // Guidelines
    prompt.push_str(
        "## Guidelines\n\
         - Choose the right agent for each sub-objective based on their skills\n\
         - After each subagent returns, evaluate whether the objective was met\n\
         - If a subagent fails, try an alternative approach or a different agent\n\
         - Be efficient: don't spawn agents for work you can synthesize yourself\n\
         - Use the `workspace_read` and `workspace_write` tools to share context between subagents\n\
         - When all sub-objectives are complete, produce a final synthesized response\n\
         - If no agents can help with the task, do your best to respond directly\n",
    );

    prompt
}

// ── run_lead_agent ───────────────────────────────────────────────────

/// Execute a task using the Lead Agent pattern: a full agentic loop
/// with a `spawn_subagent` tool for dynamic subagent delegation.
#[allow(clippy::too_many_arguments)]
pub async fn run_lead_agent(
    lead_agent: &SubAgent,
    task_description: &str,
    router: Arc<LlmRouter>,
    tool_registry: Arc<ToolRegistry>,
    shared_context: Arc<SharedContext>,
    bus: EventBus,
    db: Option<Database>,
    task_id: &str,
    created_by: &str,
) -> LeadAgentResult {
    // 1. List idle worker agents (all agents except the lead itself)
    let all_agents = shared_context.agent_registry.list_all();
    let worker_agents: Vec<SubAgent> = all_agents
        .into_iter()
        .filter(|a| a.id != lead_agent.id && a.status.is_available())
        .collect();

    // 2. Build spawn_subagent tool definition with dynamic agent list
    let spawn_tool_def = spawn_subagent_tool_definition(&worker_agents);

    // 3. Build tools: spawn_subagent + workspace_read + workspace_write
    let mut tools = vec![spawn_tool_def];
    tools.extend(crate::tools::builtins::workspace_tool_definitions());

    // 4. Build LeadAgentToolExecutor
    let spawn_tool = Arc::new(SpawnSubagentTool::new(
        router.clone(),
        tool_registry.clone(),
        shared_context.clone(),
        bus.clone(),
        db.clone(),
        task_id.to_string(),
        created_by.to_string(),
    ));

    let ctx_exec = ToolExecutionContext {
        owner_id: Some(created_by.to_string()),
        task_id: Some(task_id.to_string()),
        agent_id: Some(lead_agent.id.clone()),
        db: db.clone(),
    };
    let contextual_executor = Arc::new(ContextualToolExecutor::new(
        tool_registry.clone(),
        ctx_exec,
    ));

    let lead_executor = Arc::new(LeadAgentToolExecutor::new(
        spawn_tool.clone(),
        contextual_executor,
    ));

    // 5. Build SandboxManager with lead agent's policy
    let sandbox = SandboxManager::new(lead_executor, bus.clone());
    let sandbox_policy = SandboxPolicy::from_constraints(&lead_agent.id, &lead_agent.constraints);

    // 6. Build system prompt
    let system_prompt = build_lead_agent_prompt(&lead_agent.preset.persona, &worker_agents);
    let tool_guidance = format_tool_guidance(&tools);
    let full_system = format!("{}{}", system_prompt, tool_guidance);

    // 7. Build messages
    let messages = vec![
        ChatMessage::system(&full_system),
        ChatMessage::user(task_description),
    ];

    // 8. Build LoopConfig from lead agent's constraints
    // Lead agents get higher limits than regular agents
    let loop_config = LoopConfig {
        max_rounds: 30, // Higher than default 15 — lead agent needs more rounds
        max_tools_per_round: 3, // Each spawn_subagent is expensive, limit per round
        max_tool_runtime: Duration::from_secs(
            lead_agent.constraints.timeout_seconds.unwrap_or(300), // 5 min default
        ),
        max_cost: lead_agent.constraints.max_cost_per_task.unwrap_or(5.0), // Higher budget
        model: lead_agent.llm_config.model.clone(),
        fallback_models: lead_agent.llm_config.fallback_models.clone(),
    };

    // 9. Run the agentic loop
    let result = run_agentic_loop_routed(
        router.as_ref(),
        messages,
        tools,
        &loop_config,
        Some(&sandbox),
        &lead_agent.id,
        Some(&sandbox_policy),
        Some(task_id),
    )
    .await;

    let success = matches!(
        &result.finish_reason,
        crate::runner::LoopFinishReason::Complete
            | crate::runner::LoopFinishReason::MaxRounds
    );

    let subagents_spawned = spawn_tool.spawn_count();

    tracing::info!(
        "Lead agent '{}' finished task '{}': success={}, rounds={}, subagents_spawned={}, tokens={}/{}",
        lead_agent.id,
        task_id,
        success,
        result.rounds_used,
        subagents_spawned,
        result.total_input_tokens,
        result.total_output_tokens,
    );

    LeadAgentResult {
        success,
        final_content: result.final_content.clone(),
        loop_result: result,
        subagents_spawned,
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::subagent::{AgentConstraints, AgentLlmConfig, AgentPreset, Skill};

    fn make_agent(id: &str, name: &str, skills: &[&str]) -> SubAgent {
        SubAgent {
            id: id.to_string(),
            name: name.to_string(),
            description: Some(format!("{} agent", name)),
            icon: None,
            status: AgentStatus::Idle,
            current_task: None,
            skills: skills
                .iter()
                .map(|s| Skill {
                    name: s.to_string(),
                    category: "test".to_string(),
                    proficiency: 0.9,
                })
                .collect(),
            preset: AgentPreset::default(),
            constraints: AgentConstraints::default(),
            llm_config: AgentLlmConfig::default(),
        }
    }

    #[test]
    fn test_spawn_subagent_tool_definition() {
        let agents = vec![
            make_agent("researcher-01", "Researcher", &["web_search", "summarize"]),
            make_agent("writer-01", "Writer", &["text_generate"]),
        ];

        let def = spawn_subagent_tool_definition(&agents);
        assert_eq!(def.name, "spawn_subagent");
        assert!(def.description.contains("researcher-01"));
        assert!(def.description.contains("Writer"));
        assert!(def.description.contains("web_search"));
        assert!(def.description.contains("text_generate"));

        // Verify parameters
        let params = &def.parameters;
        assert_eq!(params["type"], "object");
        assert!(params["properties"]["agent_id"].is_object());
        assert!(params["properties"]["objective"].is_object());
        let required = params["required"].as_array().unwrap();
        assert!(required.contains(&serde_json::json!("agent_id")));
        assert!(required.contains(&serde_json::json!("objective")));
    }

    #[test]
    fn test_spawn_subagent_tool_definition_no_agents() {
        let def = spawn_subagent_tool_definition(&[]);
        assert_eq!(def.name, "spawn_subagent");
        assert!(def.description.contains("No agents available"));
    }

    #[test]
    fn test_build_lead_agent_prompt() {
        let agents = vec![
            make_agent("researcher-01", "Researcher", &["web_search", "summarize"]),
            make_agent("writer-01", "Writer", &["text_generate"]),
        ];

        let prompt = build_lead_agent_prompt("You are an orchestrator.", &agents);

        // Contains base persona
        assert!(prompt.contains("You are an orchestrator."));

        // Contains orchestration instructions
        assert!(prompt.contains("Lead Agent orchestrating"));
        assert!(prompt.contains("spawn_subagent"));

        // Contains agent listings
        assert!(prompt.contains("Researcher"));
        assert!(prompt.contains("researcher-01"));
        assert!(prompt.contains("web_search"));
        assert!(prompt.contains("Writer"));
        assert!(prompt.contains("writer-01"));
        assert!(prompt.contains("text_generate"));

        // Contains guidelines
        assert!(prompt.contains("Choose the right agent"));
        assert!(prompt.contains("workspace_read"));
    }

    #[test]
    fn test_build_lead_agent_prompt_no_agents() {
        let prompt = build_lead_agent_prompt("Base persona.", &[]);
        assert!(prompt.contains("No worker agents are currently available"));
    }

    #[test]
    fn test_lead_agent_tool_executor_routes_correctly() {
        // Test that LeadAgentToolExecutor lists spawn_subagent + contextual tools.
        // We test registered_tools() which only requires the struct, not actual execution.
        use crate::tools::registry::{RegisteredTool, ToolBackend};

        struct NoopTool;

        #[async_trait]
        impl BuiltInTool for NoopTool {
            async fn execute(
                &self,
                _arguments: &serde_json::Value,
            ) -> Result<String, String> {
                Ok("noop".to_string())
            }
        }

        let mut registry = ToolRegistry::new();
        registry.register(RegisteredTool {
            definition: ToolDefinition {
                name: "web_search".to_string(),
                description: "test".to_string(),
                parameters: serde_json::json!({"type": "object"}),
            },
            backend: ToolBackend::BuiltIn(Arc::new(NoopTool)),
        });
        let registry = Arc::new(registry);

        // Build a ContextualToolExecutor with task_id so workspace tools are listed
        let ctx_exec = ToolExecutionContext {
            owner_id: None,
            task_id: Some("task-1".to_string()),
            agent_id: None,
            db: None,
        };
        let contextual = Arc::new(ContextualToolExecutor::new(registry.clone(), ctx_exec));

        // Build a minimal SpawnSubagentTool — we won't call execute(), just need
        // it for the LeadAgentToolExecutor construction
        let spawn_tool = Arc::new(SpawnSubagentTool {
            router: Arc::new(openalpaca_llm::LlmRouter::new(
                std::collections::HashMap::new(),
                openalpaca_llm::ModelRegistry::new(std::collections::HashMap::new()),
                std::collections::HashMap::new(),
                Arc::new(openalpaca_llm::CostTracker::new(
                    openalpaca_llm::ModelRegistry::new(std::collections::HashMap::new()),
                )),
                "test-model".to_string(),
            )),
            tool_registry: registry,
            shared_context: Arc::new(SharedContext::new()),
            bus: EventBus::default(),
            db: None,
            task_id: "task-1".to_string(),
            created_by: "user-1".to_string(),
            spawn_count: AtomicUsize::new(0),
        });

        let executor = LeadAgentToolExecutor::new(spawn_tool, contextual);

        let tools = executor.registered_tools();
        assert!(tools.contains(&"spawn_subagent".to_string()));
        assert!(tools.contains(&"web_search".to_string()));
        // workspace tools should be listed since task_id is set
        assert!(tools.contains(&"workspace_read".to_string()));
        assert!(tools.contains(&"workspace_write".to_string()));
    }
}
