//! Lead Agent orchestrator: a full agentic loop that dynamically spawns
//! subagents, observes their results, adjusts strategy, and synthesizes
//! a final response. Follows Anthropic's recommended multi-agent pattern.
//!
//! The Lead Agent is a configurable `SubAgent` registered in the agent
//! registry with its own model, persona, and constraints. It receives a
//! `spawn_subagent` tool that delegates work to other agents.

#[cfg(test)]
use crate::agent::subagent::AgentStatus;
use crate::agent::subagent::SubAgent;
use crate::agent::template::AgentTemplate;
use crate::bus::EventBus;
use crate::context::SharedContext;
use crate::daemon_config::DaemonConfig;
use crate::events::SystemEvent;
use crate::middleware::prompt::format_tool_guidance;
use crate::runner::{LoopConfig, LoopResult, run_agentic_loop_routed};
use crate::security::sandbox::{SandboxManager, SandboxPolicy, ToolExecutor};
use crate::tools::registry::BuiltInTool;
use crate::tools::{ContextualToolExecutor, ToolExecutionContext, ToolRegistry};
use arc_swap::ArcSwap;
use async_trait::async_trait;
use chrono::Utc;
use openalpaca_llm::{ChatMessage, LlmRouter, ToolDefinition};
use openalpaca_storage::Database;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Maximum nesting depth for subagent spawning.
/// Prevents indirect recursion (e.g., A spawns B spawns C spawns A...).
/// Depth 0 = top-level lead agent, depth 1 = its direct subagents, etc.
const MAX_SUBAGENT_DEPTH: u32 = 3;

/// Default maximum number of concurrent subagents (used in tests).
#[cfg(test)]
const DEFAULT_MAX_CONCURRENT_SUBAGENTS: usize = 5;

// ── SubagentTracker (shared state for non-blocking spawn) ────────────

/// Status of a background-spawned subagent.
#[derive(Debug, Clone)]
pub enum SubagentStatus {
    Queued,
    Running,
    Completed { content: String, success: bool },
    Failed { error: String },
}

/// Shared tracker for background subagent tasks.
/// Allows the lead agent to spawn multiple subagents concurrently
/// and check/wait for their results.
pub struct SubagentTracker {
    pub statuses: Mutex<HashMap<String, SubagentStatus>>,
    /// Notifies waiters when a subagent completes or fails.
    pub notify: tokio::sync::Notify,
}

impl Default for SubagentTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl SubagentTracker {
    pub fn new() -> Self {
        Self {
            statuses: Mutex::new(HashMap::new()),
            notify: tokio::sync::Notify::new(),
        }
    }

    pub fn register(&self, run_id: &str) {
        let mut map = self.statuses.lock().unwrap_or_else(|p| p.into_inner());
        map.insert(run_id.to_string(), SubagentStatus::Queued);
    }

    pub fn complete(&self, run_id: &str, content: String, success: bool) {
        let mut map = self.statuses.lock().unwrap_or_else(|p| p.into_inner());
        map.insert(
            run_id.to_string(),
            SubagentStatus::Completed { content, success },
        );
        drop(map);
        self.notify.notify_waiters();
    }

    pub fn fail(&self, run_id: &str, error: String) {
        let mut map = self.statuses.lock().unwrap_or_else(|p| p.into_inner());
        map.insert(run_id.to_string(), SubagentStatus::Failed { error });
        drop(map);
        self.notify.notify_waiters();
    }

    pub fn set_status(&self, run_id: &str, status: SubagentStatus) {
        let mut map = self.statuses.lock().unwrap_or_else(|p| p.into_inner());
        map.insert(run_id.to_string(), status);
        drop(map);
        self.notify.notify_waiters();
    }

    pub fn get(&self, run_id: &str) -> Option<SubagentStatus> {
        let map = self.statuses.lock().unwrap_or_else(|p| p.into_inner());
        map.get(run_id).cloned()
    }

    pub fn all_done(&self) -> bool {
        let map = self.statuses.lock().unwrap_or_else(|p| p.into_inner());
        map.values().all(|s| {
            matches!(
                s,
                SubagentStatus::Completed { .. } | SubagentStatus::Failed { .. }
            )
        })
    }

    pub fn status_counts(&self) -> (usize, usize, usize, usize) {
        let map = self.statuses.lock().unwrap_or_else(|p| p.into_inner());
        let (mut queued, mut running, mut completed, mut failed) = (0, 0, 0, 0);
        for s in map.values() {
            match s {
                SubagentStatus::Queued => queued += 1,
                SubagentStatus::Running => running += 1,
                SubagentStatus::Completed { .. } => completed += 1,
                SubagentStatus::Failed { .. } => failed += 1,
            }
        }
        (queued, running, completed, failed)
    }

    pub fn summary(&self) -> String {
        let map = self.statuses.lock().unwrap_or_else(|p| p.into_inner());
        if map.is_empty() {
            return "No subagents have been spawned yet.".to_string();
        }
        let mut parts = Vec::new();
        for (id, status) in map.iter() {
            match status {
                SubagentStatus::Queued => {
                    parts.push(format!("- **{}**: queued (waiting for execution slot)", id));
                }
                SubagentStatus::Running => {
                    parts.push(format!("- **{}**: still running", id));
                }
                SubagentStatus::Completed { content, success } => {
                    let preview: String = content.chars().take(500).collect();
                    parts.push(format!(
                        "- **{}**: {} — {}",
                        id,
                        if *success {
                            "completed"
                        } else {
                            "completed (partial)"
                        },
                        preview
                    ));
                }
                SubagentStatus::Failed { error } => {
                    parts.push(format!("- **{}**: failed — {}", id, error));
                }
            }
        }
        parts.join("\n")
    }
}

// ── Result types ─────────────────────────────────────────────────────

/// Result of a lead agent execution.
pub struct LeadAgentResult {
    pub success: bool,
    pub final_content: String,
    pub loop_result: LoopResult,
    pub subagents_spawned: usize,
}

// ── AgentBusyGuard ───────────────────────────────────────────────────

/// RAII guard that cleans up an agent instance on drop.
///
/// For **non-singleton** instances: calls `destroy_instance()` to remove
/// the ephemeral instance from the registry entirely.
///
/// For **singleton** instances (like lead_agent): calls `destroy_instance()`
/// which resets the singleton to Idle so it can be reused.
///
/// This ensures the instance is not permanently stuck in Busy state if the
/// subagent loop panics or returns early without cleanup.
pub(crate) struct AgentBusyGuard {
    instance_id: String,
    template_id: String,
    agent_registry: Arc<crate::agent::registry::AgentRegistry>,
    bus: EventBus,
    /// Set to true once the instance has been explicitly cleaned up.
    /// Prevents double-cleanup in the normal (non-panic) code path.
    restored: bool,
}

impl AgentBusyGuard {
    pub(crate) fn new(
        instance_id: String,
        template_id: String,
        agent_registry: Arc<crate::agent::registry::AgentRegistry>,
        bus: EventBus,
    ) -> Self {
        Self {
            instance_id,
            template_id,
            agent_registry,
            bus,
            restored: false,
        }
    }

    pub(crate) fn restore(&mut self) {
        if !self.restored {
            self.restored = true;
            let outcome = self.agent_registry.destroy_instance(&self.instance_id);
            let status = match outcome {
                crate::agent::registry::DestroyOutcome::ResetToIdle => "idle",
                _ => "destroyed",
            };
            self.bus.publish(SystemEvent::AgentStatusChanged {
                agent_id: self.instance_id.clone(),
                instance_id: self.instance_id.clone(),
                template_id: self.template_id.clone(),
                status: status.to_string(),
                current_task_id: None,
                timestamp: Utc::now(),
            });
        }
    }
}

impl Drop for AgentBusyGuard {
    fn drop(&mut self) {
        if !self.restored {
            tracing::warn!(
                instance_id = %self.instance_id,
                "AgentBusyGuard dropped without explicit restore — destroying instance"
            );
            self.restore();
        }
    }
}

// ── SpawnSubagentTool ────────────────────────────────────────────────

/// Built-in tool that allows the Lead Agent to spawn a subagent.
/// Each invocation spawns the subagent as a background task and returns
/// immediately with a run_id. Use `check_subagent_status` or
/// `wait_for_subagents` to collect results.
pub struct SpawnSubagentTool {
    router: Arc<LlmRouter>,
    tool_registry: Arc<ToolRegistry>,
    shared_context: Arc<SharedContext>,
    bus: EventBus,
    db: Option<Database>,
    task_id: String,
    created_by: String,
    lead_template_id: String,
    daemon_config: Arc<ArcSwap<DaemonConfig>>,
    /// Tracks how many subagents have been spawned (for observability).
    spawn_count: AtomicUsize,
    workspace_id: Option<String>,
    /// Cancellation token from the parent lead agent task.
    /// Child tokens are created for each subagent so they auto-cancel
    /// when the parent task is cancelled.
    cancel_token: Option<CancellationToken>,
    /// Shared tracker for background subagent status.
    tracker: Arc<SubagentTracker>,
    /// Current recursion depth (0 = top-level lead agent).
    /// Propagated to child subagents as depth + 1.
    depth: u32,
    /// Configured maximum concurrent subagents for this lead agent.
    max_concurrent_subagents: usize,
    /// Semaphore limiting concurrent subagent spawns per lead agent.
    concurrency_semaphore: Arc<tokio::sync::Semaphore>,
}

impl SpawnSubagentTool {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        router: Arc<LlmRouter>,
        tool_registry: Arc<ToolRegistry>,
        shared_context: Arc<SharedContext>,
        bus: EventBus,
        db: Option<Database>,
        task_id: String,
        created_by: String,
        lead_template_id: String,
        daemon_config: Arc<ArcSwap<DaemonConfig>>,
        cancel_token: Option<CancellationToken>,
        tracker: Arc<SubagentTracker>,
        depth: u32,
        max_concurrent_subagents: usize,
        workspace_id: Option<String>,
    ) -> Self {
        Self {
            router,
            tool_registry,
            shared_context,
            bus,
            db,
            task_id,
            created_by,
            lead_template_id,
            daemon_config,
            spawn_count: AtomicUsize::new(0),
            cancel_token,
            tracker,
            depth,
            max_concurrent_subagents,
            concurrency_semaphore: Arc::new(tokio::sync::Semaphore::new(max_concurrent_subagents)),
            workspace_id,
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

        tracing::info!(
            target_agent = agent_id,
            objective_preview = &objective[..objective.len().min(80)],
            task_id = %self.task_id,
            "Lead agent spawning subagent"
        );

        // 2. Prevent recursion: direct self-spawning and depth limit
        if agent_id == self.lead_template_id {
            return Err(format!(
                "Agent '{}' cannot spawn itself — would cause infinite recursion",
                agent_id
            ));
        }

        if self.depth >= MAX_SUBAGENT_DEPTH {
            return Err(format!(
                "Maximum subagent depth ({}) reached — cannot spawn further subagents. \
                 Current depth: {}. Complete this objective directly instead of delegating.",
                MAX_SUBAGENT_DEPTH, self.depth
            ));
        }

        // Enforce concurrency limit — wait up to 30s for a slot
        let permit = tokio::time::timeout(
            Duration::from_secs(30),
            self.concurrency_semaphore.clone().acquire_owned(),
        )
        .await
        .map_err(|_| {
            format!(
                "Timed out waiting 30s for a subagent slot (max concurrent: {}). \
                 Wait for existing subagents to complete before spawning new ones. \
                 Use `wait_for_subagents` or `check_subagent_status` first.",
                self.max_concurrent_subagents
            )
        })?
        .map_err(|_| "Subagent concurrency semaphore closed unexpectedly".to_string())?;

        // 3. Spawn a new instance from the agent template
        //    (agent_id is really a template_id — the LLM picks from the template catalog)
        let agent = self
            .shared_context
            .agent_registry
            .spawn_instance(agent_id, self.task_id.clone())
            .map_err(|e| format!("Cannot spawn agent '{}': {}", agent_id, e))?;
        let instance_id = agent.id.clone();
        self.bus.publish(SystemEvent::AgentStatusChanged {
            agent_id: instance_id.clone(),
            instance_id: instance_id.clone(),
            template_id: agent_id.to_string(),
            status: "spawned".to_string(),
            current_task_id: Some(self.task_id.clone()),
            timestamp: Utc::now(),
        });
        let mut busy_guard = AgentBusyGuard::new(
            instance_id.clone(),
            agent_id.to_string(),
            self.shared_context.agent_registry.clone(),
            self.bus.clone(),
        );

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
            workspace_id: self.workspace_id.clone(),
        };
        let contextual_executor = Arc::new(ContextualToolExecutor::new(
            self.tool_registry.clone(),
            ctx_exec,
        ));
        let sandbox = SandboxManager::with_defaults(contextual_executor, self.bus.clone());

        // 6. Resolve tools for subagent's skills
        let tools = crate::tools::resolve_agent_tools(&agent, &self.tool_registry);

        // 7. Build messages with agent persona + objective
        let tool_guidance = format_tool_guidance(&tools);
        let system_prompt = format!(
            "<identity>\n{}\n</identity>\n\n\
             <scope>\n\
             You are a subagent working on a single objective assigned by a lead agent. \
             Focus exclusively on your assigned objective. Do not attempt work outside your scope.\n\
             </scope>\n\n\
             <output-format>\n\
             Provide a clear, complete result. Start with a brief summary of what you accomplished, \
             followed by the detailed output. The lead agent will use your result to synthesize a \
             final response, so be thorough and specific.\n\
             </output-format>\n\n\
             <constraints>\n\
             You operate independently — you cannot communicate with other subagents directly. \
             Use workspace_read and workspace_write tools to access or share data across agents.\n\
             </constraints>{}",
            agent.preset.persona, tool_guidance
        );
        let messages = vec![
            ChatMessage::system(&system_prompt),
            ChatMessage::user(objective),
        ];

        // 8. Build LoopConfig from daemon defaults + agent constraints
        let loop_config =
            LoopConfig::from_agent(&self.daemon_config.load().execution.agent_defaults, &agent)
                .with_model_pricing(
                    self.router.model_registry(),
                    agent.llm_config.model.as_deref(),
                )
                .with_context_window(
                    self.router.model_registry(),
                    agent.llm_config.model.as_deref(),
                );

        let sandbox_policy = SandboxPolicy::from_constraints(&instance_id, &agent.constraints);

        // 9. Spawn subagent as a background task (non-blocking).
        //    This allows the lead agent to spawn multiple subagents in parallel.
        let run_id = format!(
            "{}::{}",
            agent_id,
            &instance_id.split("::").last().unwrap_or(&instance_id)
        );
        self.tracker.register(&run_id);

        let child_token = self.cancel_token.as_ref().map(|t| t.child_token());
        let tracker = self.tracker.clone();
        let run_id_clone = run_id.clone();
        let router = self.router.clone();
        let task_id = self.task_id.clone();
        let bus = self.bus.clone();
        let db = self.db.clone();
        let agent_id_owned = agent_id.to_string();
        let objective_preview: String = objective.chars().take(50).collect();
        tokio::task::spawn(async move {
            // Hold the concurrency permit for the lifetime of this subagent.
            // It is automatically released when this async block completes.
            let _permit = permit;

            // Check for cancellation before starting.
            if let Some(ref token) = child_token
                && token.is_cancelled()
            {
                tracker.fail(
                    &run_id_clone,
                    "Cancelled before starting (parent task was cancelled)".to_string(),
                );
                busy_guard.restore();
                return;
            }

            tracker.set_status(&run_id_clone, SubagentStatus::Running);

            let result = run_agentic_loop_routed(
                router.as_ref(),
                messages,
                tools,
                &loop_config,
                Some(&sandbox),
                &instance_id,
                Some(&sandbox_policy),
                Some(&task_id),
                child_token,
            )
            .await;

            let duration_ms = agent_start.elapsed().as_millis() as u64;
            let now = Utc::now();

            let agent_success = matches!(
                &result.finish_reason,
                crate::runner::LoopFinishReason::Complete
                    | crate::runner::LoopFinishReason::MaxRounds
            );

            // Destroy instance (explicit restore; guard is backup for panics)
            busy_guard.restore();

            // Emit DagNodeCompleted
            bus.publish(SystemEvent::DagNodeCompleted {
                task_id: task_id.clone(),
                node_id: node_id.clone(),
                node_title: objective_preview.clone(),
                agent_id: instance_id.clone(),
                success: agent_success,
                duration_ms,
                output_preview: if result.final_content.is_empty() {
                    None
                } else {
                    Some(result.final_content.chars().take(200).collect())
                },
                timestamp: now,
            });

            // Record LLM usage + agent history
            crate::orchestrator::dispatcher::usage::record_llm_usage(
                &router,
                &result,
                loop_config.model.as_deref(),
                &agent_id_owned,
                &task_id,
                duration_ms as i64,
                db.as_ref(),
                &bus,
            );

            if let Some(ref db) = db {
                crate::orchestrator::dispatcher::usage::record_agent_history(
                    db,
                    &agent_id_owned,
                    &task_id,
                    "subagent",
                    agent_success,
                    duration_ms as i64 / 1000,
                );
            }

            tracing::info!(
                "Subagent '{}' (instance '{}') completed objective '{}': success={}, rounds={}, tokens={}/{}, duration={}ms",
                agent_id_owned,
                instance_id,
                &objective_preview,
                agent_success,
                result.rounds_used,
                result.total_input_tokens,
                result.total_output_tokens,
                duration_ms,
            );

            // Update tracker with result
            if agent_success {
                let content = if result.final_content.is_empty() {
                    format!(
                        "Agent '{}' completed in {} rounds ({} tool calls) but produced no text output.",
                        agent_id_owned, result.rounds_used, result.tool_calls_made
                    )
                } else {
                    result.final_content
                };
                tracker.complete(&run_id_clone, content, true);
            } else {
                tracker.fail(
                    &run_id_clone,
                    format!(
                        "Agent '{}' failed: {:?}. Last output: {}",
                        agent_id_owned,
                        result.finish_reason,
                        result.final_content.chars().take(500).collect::<String>()
                    ),
                );
            }
        });

        // Return immediately — the subagent is queued/running in the background
        Ok(format!(
            "Subagent '{}' spawned (run_id: '{}'). It will start executing when an LLM slot \
             is available (or immediately if capacity permits). Spawn more subagents if needed, \
             then call `wait_for_subagents` to collect all results, or `check_subagent_status` \
             with this run_id to check individually.",
            agent_id, run_id
        ))
    }
}

// ── SpawnSubagentsBatchTool ──────────────────────────────────────────

/// Batch variant of `spawn_subagent`: spawns 1-8 subagents in a single tool call.
/// Delegates each spawn to the existing `SpawnSubagentTool` for consistent behavior.
pub struct SpawnSubagentsBatchTool {
    inner: Arc<SpawnSubagentTool>,
}

impl SpawnSubagentsBatchTool {
    pub fn new(inner: Arc<SpawnSubagentTool>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl BuiltInTool for SpawnSubagentsBatchTool {
    async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let subagents = arguments
            .get("subagents")
            .and_then(|v| v.as_array())
            .ok_or_else(|| {
                "Missing required parameter: subagents (must be a JSON array)".to_string()
            })?;

        if subagents.is_empty() {
            return Err("subagents array must contain at least 1 item".to_string());
        }
        if subagents.len() > 8 {
            return Err(format!(
                "subagents array has {} items (max 8). Split into multiple batch calls.",
                subagents.len()
            ));
        }

        let mut results = Vec::with_capacity(subagents.len());
        let mut success_count = 0usize;

        for (i, entry) in subagents.iter().enumerate() {
            let agent_id = entry
                .get("agent_id")
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("subagents[{}]: missing agent_id", i))?;
            let objective = entry
                .get("objective")
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("subagents[{}]: missing objective", i))?;

            let single_args = serde_json::json!({
                "agent_id": agent_id,
                "objective": objective,
            });

            match self.inner.execute(&single_args).await {
                Ok(msg) => {
                    success_count += 1;
                    results.push(format!("[{}] OK: {}", i + 1, msg));
                }
                Err(e) => {
                    results.push(format!("[{}] FAILED ({}): {}", i + 1, agent_id, e));
                }
            }
        }

        let summary = format!(
            "Batch spawn: {}/{} subagents spawned successfully.\n{}",
            success_count,
            subagents.len(),
            results.join("\n")
        );
        Ok(summary)
    }
}

/// Tool definition for `spawn_subagents_batch`.
pub fn spawn_subagents_batch_tool_definition(templates: &[AgentTemplate]) -> ToolDefinition {
    let agent_descriptions: Vec<String> = templates
        .iter()
        .map(|t| {
            let fm = &t.frontmatter;
            let skills = fm.skills.join(", ");
            format!(
                "- ID: \"{}\", Name: \"{}\", Skills: [{}]",
                fm.id, fm.name, skills
            )
        })
        .collect();

    let agents_list = if agent_descriptions.is_empty() {
        "No agents available.".to_string()
    } else {
        agent_descriptions.join("\n")
    };

    ToolDefinition {
        name: "spawn_subagents_batch".to_string(),
        description: format!(
            "Spawn multiple subagents in a single call (1-8). More efficient than calling \
             spawn_subagent repeatedly. Each entry specifies an agent_id and objective. \
             All subagents start executing immediately in parallel. Use wait_for_subagents \
             to collect results.\n\nAvailable agents:\n{}",
            agents_list
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "subagents": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "agent_id": {
                                "type": "string",
                                "description": "The ID of the agent template to spawn"
                            },
                            "objective": {
                                "type": "string",
                                "description": "A clear, specific objective for this subagent"
                            }
                        },
                        "required": ["agent_id", "objective"]
                    },
                    "minItems": 1,
                    "maxItems": 8,
                    "description": "Array of subagent specs to spawn"
                }
            },
            "required": ["subagents"]
        }),
    }
}

// ── CheckSubagentStatusTool ──────────────────────────────────────────

/// Tool that allows the lead agent to check the status of a spawned subagent.
pub struct CheckSubagentStatusTool {
    tracker: Arc<SubagentTracker>,
}

#[async_trait]
impl BuiltInTool for CheckSubagentStatusTool {
    async fn execute(&self, arguments: &serde_json::Value) -> Result<String, String> {
        let run_id = arguments
            .get("subagent_run_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "Missing required parameter: subagent_run_id".to_string())?;

        match self.tracker.get(run_id) {
            Some(SubagentStatus::Queued) => Ok(format!(
                "Subagent '{}' is queued, waiting for an execution slot. Check again later or use `wait_for_subagents`.",
                run_id
            )),
            Some(SubagentStatus::Running) => Ok(format!(
                "Subagent '{}' is still running. Check again later or use `wait_for_subagents`.",
                run_id
            )),
            Some(SubagentStatus::Completed { content, success }) => Ok(format!(
                "Subagent '{}' {}: {}",
                run_id,
                if success {
                    "completed successfully"
                } else {
                    "completed with issues"
                },
                content,
            )),
            Some(SubagentStatus::Failed { error }) => {
                Err(format!("Subagent '{}' failed: {}", run_id, error))
            }
            None => Err(format!(
                "Unknown subagent_run_id: '{}'. Valid run IDs are shown when calling spawn_subagent.",
                run_id
            )),
        }
    }
}

/// Build the tool definition for `check_subagent_status`.
pub fn check_subagent_status_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "check_subagent_status".to_string(),
        description: "Check the status of a previously spawned subagent. Returns whether the \
                       subagent is queued (waiting for an execution slot), running, completed, \
                       or failed."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "subagent_run_id": {
                    "type": "string",
                    "description": "The run_id returned by spawn_subagent"
                }
            },
            "required": ["subagent_run_id"]
        }),
    }
}

// ── WaitForSubagentsTool ─────────────────────────────────────────────

/// Tool that blocks until all spawned subagents have completed,
/// then returns a summary of all results.
pub struct WaitForSubagentsTool {
    tracker: Arc<SubagentTracker>,
}

#[async_trait]
impl BuiltInTool for WaitForSubagentsTool {
    async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
        let max_wait = Duration::from_secs(600); // 10 minutes safety cap
        let deadline = tokio::time::Instant::now() + max_wait;

        loop {
            if self.tracker.all_done() {
                break;
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                let (queued, running, completed, failed) = self.tracker.status_counts();
                return Ok(format!(
                    "Timed out after {}s waiting for subagents. \
                     Status: {} completed, {} failed, {} queued, {} running.\n\n{}",
                    max_wait.as_secs(),
                    completed,
                    failed,
                    queued,
                    running,
                    self.tracker.summary()
                ));
            }
            // Wait for a subagent status change or timeout
            tokio::select! {
                _ = self.tracker.notify.notified() => { /* re-check all_done */ }
                _ = tokio::time::sleep(remaining) => { /* timeout */ }
            }
        }

        let (queued, running, completed, failed) = self.tracker.status_counts();
        let header = format!(
            "All subagents finished. {} completed, {} failed.\n\n",
            completed, failed
        );
        // Include queued/running counts only if non-zero (shouldn't happen but be safe)
        let header = if queued > 0 || running > 0 {
            format!(
                "All subagents finished. {} completed, {} failed, {} queued, {} running.\n\n",
                completed, failed, queued, running
            )
        } else {
            header
        };
        Ok(format!("{}{}", header, self.tracker.summary()))
    }
}

/// Build the tool definition for `wait_for_subagents`.
pub fn wait_for_subagents_tool_definition() -> ToolDefinition {
    ToolDefinition {
        name: "wait_for_subagents".to_string(),
        description: "Wait for ALL previously spawned subagents to complete, including any that \
                       are queued for execution. Returns a summary of all results. Use this after \
                       spawning all subagents to collect all outputs before synthesizing a response."
            .to_string(),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {},
            "required": []
        }),
    }
}

// ── Tool definitions ─────────────────────────────────────────────────

/// Build the tool definition for `spawn_subagent` from agent templates.
///
/// This is the preferred variant: the LLM sees template IDs (not instance IDs),
/// and `spawn_instance()` creates a fresh instance for each invocation.
pub fn spawn_subagent_tool_definition_from_templates(
    templates: &[AgentTemplate],
) -> ToolDefinition {
    let agent_descriptions: Vec<String> = templates
        .iter()
        .map(|t| {
            let fm = &t.frontmatter;
            let skills = fm.skills.join(", ");
            format!(
                "- ID: \"{}\", Name: \"{}\", Skills: [{}], Description: \"{}\"",
                fm.id, fm.name, skills, fm.description
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
            "Spawn a subagent to work on a specific objective. Spawning is always immediate — \
             the system automatically queues execution if LLM capacity is limited. Spawn all \
             independent objectives in a single round, then use wait_for_subagents to collect \
             results. Multiple instances of the same agent can run concurrently.\n\n\
             Available agents:\n{}",
            agents_list
        ),
        parameters: serde_json::json!({
            "type": "object",
            "properties": {
                "agent_id": {
                    "type": "string",
                    "description": "The ID of the agent template to spawn (must be from the available agents list)"
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

/// A ToolExecutor that routes `spawn_subagent`, `check_subagent_status`,
/// and `wait_for_subagents` to their respective tool implementations,
/// and all other tools to the ContextualToolExecutor.
pub struct LeadAgentToolExecutor {
    spawn_tool: Arc<SpawnSubagentTool>,
    batch_spawn_tool: Option<Arc<SpawnSubagentsBatchTool>>,
    check_status_tool: Arc<CheckSubagentStatusTool>,
    wait_tool: Arc<WaitForSubagentsTool>,
    contextual_executor: Arc<ContextualToolExecutor>,
}

impl LeadAgentToolExecutor {
    pub fn new(
        spawn_tool: Arc<SpawnSubagentTool>,
        batch_spawn_tool: Option<Arc<SpawnSubagentsBatchTool>>,
        check_status_tool: Arc<CheckSubagentStatusTool>,
        wait_tool: Arc<WaitForSubagentsTool>,
        contextual_executor: Arc<ContextualToolExecutor>,
    ) -> Self {
        Self {
            spawn_tool,
            batch_spawn_tool,
            check_status_tool,
            wait_tool,
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
            "spawn_subagents_batch" => match &self.batch_spawn_tool {
                Some(tool) => tool.execute(arguments).await,
                None => Err("spawn_subagents_batch tool is not enabled".to_string()),
            },
            "check_subagent_status" => self.check_status_tool.execute(arguments).await,
            "wait_for_subagents" => self.wait_tool.execute(arguments).await,
            _ => self.contextual_executor.execute(tool_name, arguments).await,
        }
    }

    fn registered_tools(&self) -> Vec<String> {
        let mut tools = self.contextual_executor.registered_tools();
        tools.push("spawn_subagent".to_string());
        if self.batch_spawn_tool.is_some() {
            tools.push("spawn_subagents_batch".to_string());
        }
        tools.push("check_subagent_status".to_string());
        tools.push("wait_for_subagents".to_string());
        tools
    }
}

/// Build the system prompt for the Lead Agent from agent templates.
///
/// This is the preferred variant: the LLM sees template IDs and descriptions,
/// and can spawn multiple instances of the same template concurrently.
pub fn build_lead_agent_prompt_from_templates(
    base_persona: &str,
    templates: &[AgentTemplate],
) -> String {
    let mut prompt = String::with_capacity(3072);

    prompt.push_str(base_persona);
    prompt.push_str("\n\n");

    // Role and scope
    prompt.push_str(
        "<role>\n\
         You are a Lead Agent orchestrating a complex task. You are responsible for analyzing \
         the user's request, decomposing it into sub-objectives, delegating work to specialized \
         subagents, and synthesizing their results into a final response.\n\
         Do not attempt to perform specialized work (coding, research, analysis) yourself when \
         a suitable subagent is available. Your value is in orchestration and synthesis.\n\
         </role>\n\n",
    );

    // Available agents catalog
    prompt.push_str("<agents>\n");
    if templates.is_empty() {
        prompt.push_str("No worker agents are currently available. Complete the task directly.\n");
    } else {
        for t in templates {
            let fm = &t.frontmatter;
            let skills_str = if fm.skills.is_empty() {
                "none".to_string()
            } else {
                fm.skills.join(", ")
            };
            prompt.push_str(&format!(
                "- id=\"{}\" name=\"{}\" skills=[{}]: {}\n",
                fm.id, fm.name, skills_str, fm.description
            ));
        }
    }
    prompt.push_str("</agents>\n\n");

    // Explicit workflow steps
    prompt.push_str(
        "<workflow>\n\
         Step 1: Analyze the user's request. Identify the core goal and any constraints.\n\
         Step 2: Decompose into sub-objectives. Each sub-objective should map to one subagent.\n\
         Step 3: Spawn ALL subagents for independent objectives in a single round. Match each \
         sub-objective to the best agent by skills. Spawning is always immediate — the system \
         automatically manages execution ordering based on available LLM capacity. Subagents may \
         be queued if capacity is limited — this is handled automatically and transparently.\n\
         Step 4: Collect results. Call wait_for_subagents to block until all complete (including \
         queued ones), or check_subagent_status for individual progress.\n\
         Step 5: Evaluate and iterate. If a subagent failed or produced incomplete results, \
         retry with an adjusted objective or a different agent.\n\
         Step 6: Synthesize. Combine all subagent outputs into a coherent final response \
         that directly addresses the user's original request.\n\
         </workflow>\n\n",
    );

    // Delegation criteria
    prompt.push_str(
        "<delegation-criteria>\n\
         Spawn subagents when:\n\
         - Tasks can run in parallel (e.g., research + implementation are independent)\n\
         - Tasks require isolated context or specialized skills\n\
         - Tasks involve independent workstreams that do not need shared state\n\n\
         Work directly (do NOT spawn) when:\n\
         - The task is simple enough to answer from your own knowledge\n\
         - You are synthesizing, summarizing, or formatting existing results\n\
         - The task requires maintaining context across sequential steps that one agent handles best\n\
         </delegation-criteria>\n\n",
    );

    // Tool usage pattern
    prompt.push_str(
        "<tools>\n\
         spawn_subagent: Spawning is always immediate — returns a run_id instantly. The system \
         automatically queues execution if LLM capacity is limited. Spawn all independent \
         objectives in a single round before waiting — this is the preferred pattern.\n\
         check_subagent_status: Poll a single subagent by run_id. Shows whether the subagent is \
         queued, running, completed, or failed.\n\
         wait_for_subagents: Block until ALL spawned subagents finish, including any that are \
         queued for execution. Returns a summary of all results. Call this after spawning all \
         subagents.\n\
         workspace_read / workspace_write: Share context between subagents. Write setup data before spawning; \
         read results after completion.\n\
         </tools>\n\n",
    );

    // Failure recovery
    prompt.push_str(
        "<failure-recovery>\n\
         If a subagent fails:\n\
         1. Read the error message to understand the failure type.\n\
         2. If the objective was too broad, split it into smaller sub-objectives and retry.\n\
         3. If the agent lacked the right skills, try a different agent.\n\
         4. If repeated failures occur, complete that sub-objective directly yourself.\n\
         5. Never silently drop a failed sub-objective — always report what succeeded and what did not.\n\
         </failure-recovery>\n\n",
    );

    // Output expectations
    prompt.push_str(
        "<output>\n\
         Your final response must directly address the user's original request. \
         Synthesize all subagent results into a single coherent answer. \
         Do not simply list raw subagent outputs — integrate, summarize, and resolve any conflicts.\n\
         </output>\n",
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
    embedder: Option<Arc<dyn openalpaca_llm::Embedder>>,
    task_id: &str,
    created_by: &str,
    daemon_config: &Arc<ArcSwap<DaemonConfig>>,
    workspace_id: Option<String>,
    cancel_token: Option<CancellationToken>,
) -> LeadAgentResult {
    tracing::info!(
        lead_agent = %lead_agent.id,
        task_id = task_id,
        "Lead agent starting execution"
    );

    // 1. List worker agent templates (all templates except the lead itself)
    let all_templates = shared_context.agent_registry.list_templates();
    let worker_templates: Vec<AgentTemplate> = all_templates
        .into_iter()
        .filter(|t| t.frontmatter.id != lead_agent.template_id)
        .collect();

    tracing::info!(
        lead_agent = %lead_agent.id,
        task_id = task_id,
        worker_templates = worker_templates.len(),
        "Lead agent found worker templates"
    );

    // 2. Build spawn_subagent tool definition from templates
    let spawn_tool_def = spawn_subagent_tool_definition_from_templates(&worker_templates);

    // 3. Build tools: spawn_subagent + check/wait + workspace + memory_search
    let batch_spawn_enabled = daemon_config
        .load()
        .execution
        .lead_agent_defaults
        .batch_spawn_enabled;
    let mut tools = vec![spawn_tool_def];
    if batch_spawn_enabled {
        tools.push(spawn_subagents_batch_tool_definition(&worker_templates));
    }
    tools.push(check_subagent_status_tool_definition());
    tools.push(wait_for_subagents_tool_definition());
    tools.extend(crate::tools::builtins::workspace_tool_definitions());
    // Add memory_search so the lead agent can query user memories directly
    if let Some(mem_tool) = tool_registry.get("memory_search") {
        tools.push(mem_tool.definition.clone());
    }

    // 4. Build LeadAgentToolExecutor with shared SubagentTracker
    let tracker = Arc::new(SubagentTracker::new());

    let spawn_tool = Arc::new(SpawnSubagentTool::new(
        router.clone(),
        tool_registry.clone(),
        shared_context.clone(),
        bus.clone(),
        db.clone(),
        task_id.to_string(),
        created_by.to_string(),
        lead_agent.template_id.clone(),
        daemon_config.clone(),
        cancel_token.clone(),
        tracker.clone(),
        0, // depth: top-level lead agent
        daemon_config
            .load()
            .execution
            .lead_agent_defaults
            .max_concurrent_subagents,
        workspace_id.clone(),
    ));

    let check_status_tool = Arc::new(CheckSubagentStatusTool {
        tracker: tracker.clone(),
    });
    let wait_tool = Arc::new(WaitForSubagentsTool {
        tracker: tracker.clone(),
    });

    let ctx_exec = ToolExecutionContext {
        owner_id: Some(created_by.to_string()),
        task_id: Some(task_id.to_string()),
        agent_id: Some(lead_agent.id.clone()),
        db: db.clone(),
        workspace_id: workspace_id.clone(),
    };
    let contextual_executor =
        Arc::new(ContextualToolExecutor::new(tool_registry.clone(), ctx_exec));

    let batch_spawn_tool = if batch_spawn_enabled {
        Some(Arc::new(SpawnSubagentsBatchTool::new(spawn_tool.clone())))
    } else {
        None
    };

    let lead_executor = Arc::new(LeadAgentToolExecutor::new(
        spawn_tool.clone(),
        batch_spawn_tool,
        check_status_tool,
        wait_tool,
        contextual_executor,
    ));

    // 5. Build SandboxManager with lead agent's policy
    let sandbox = SandboxManager::with_defaults(lead_executor, bus.clone());
    let sandbox_policy = SandboxPolicy::from_constraints(&lead_agent.id, &lead_agent.constraints);

    // 6. Build system prompt from templates
    let system_prompt =
        build_lead_agent_prompt_from_templates(&lead_agent.preset.persona, &worker_templates);
    let tool_guidance = format_tool_guidance(&tools);
    let full_system = format!("{}{}", system_prompt, tool_guidance);

    // 7. Build messages (with proactive memory injection)
    let mut messages = vec![ChatMessage::system(&full_system)];

    // Inject retrieved memory context so lead agent has user preferences/facts
    if let Some(ref db) = db {
        let repo = openalpaca_storage::repository::MemoryRepository::new(db);
        let query_embedding = if let Some(ref emb) = embedder {
            emb.embed(&[task_description])
                .await
                .ok()
                .and_then(|v| v.into_iter().next())
        } else {
            None
        };
        let scope_ctx = workspace_id
            .as_ref()
            .map(|ws| crate::memory::scope_context::MemoryScopeContext::new(Some(ws.clone())));
        let memories = if let Some(ref ctx) = scope_ctx {
            let cascade_scopes = ctx.cascade_scopes();
            repo.search_hybrid_cascade(
                created_by,
                task_description,
                query_embedding.as_deref(),
                5,
                None,
                &cascade_scopes,
            )
            .unwrap_or_default()
        } else {
            repo.search_hybrid(
                created_by,
                task_description,
                query_embedding.as_deref(),
                5,
                None,
                None,
                None,
            )
            .unwrap_or_default()
        };
        if !memories.is_empty() {
            // Track access for importance decay + boost
            let ids: Vec<i64> = memories.iter().map(|m| m.id).collect();
            let access_boost = daemon_config.load().orchestrator.memory.decay.access_boost;
            if let Err(e) = repo.touch_accessed(&ids, access_boost) {
                tracing::warn!("Failed to track memory access: {e}");
            }

            let mut block = String::from("### RETRIEVED MEMORY ###\n");
            let mut budget = 2000usize;
            for m in &memories {
                let entry = format!(
                    "- [{}] {}\n",
                    m.kind.as_str(),
                    m.content.chars().take(300).collect::<String>()
                );
                if entry.len() > budget {
                    break;
                }
                budget -= entry.len();
                block.push_str(&entry);
            }
            messages.push(ChatMessage::user(
                &crate::orchestrator::wrap_untrusted_context(&block, "retrieved_memory", "retrieved"),
            ));
        }
    }

    messages.push(ChatMessage::user(task_description));

    // 8. Build LoopConfig from lead agent defaults + agent constraint overrides
    let loop_config = LoopConfig::from_lead_agent(
        &daemon_config.load().execution.lead_agent_defaults,
        lead_agent,
    )
    .with_model_pricing(
        router.model_registry(),
        lead_agent.llm_config.model.as_deref(),
    )
    .with_context_window(
        router.model_registry(),
        lead_agent.llm_config.model.as_deref(),
    );

    // 9. Run the agentic loop
    tracing::info!(
        lead_agent = %lead_agent.id,
        task_id = task_id,
        tools_count = tools.len(),
        max_rounds = loop_config.max_rounds,
        max_cost = loop_config.max_cost,
        "Lead agent entering agentic loop"
    );

    let result = run_agentic_loop_routed(
        router.as_ref(),
        messages,
        tools,
        &loop_config,
        Some(&sandbox),
        &lead_agent.id,
        Some(&sandbox_policy),
        Some(task_id),
        cancel_token,
    )
    .await;

    let success = matches!(
        &result.finish_reason,
        crate::runner::LoopFinishReason::Complete | crate::runner::LoopFinishReason::MaxRounds
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
            template_id: id.to_string(),
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
    fn test_lead_agent_tool_executor_routes_correctly() {
        // Test that LeadAgentToolExecutor lists spawn_subagent + contextual tools.
        // We test registered_tools() which only requires the struct, not actual execution.
        use crate::tools::registry::{RegisteredTool, ToolBackend};

        struct NoopTool;

        #[async_trait]
        impl BuiltInTool for NoopTool {
            async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
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
            workspace_id: None,
        };
        let contextual = Arc::new(ContextualToolExecutor::new(registry.clone(), ctx_exec));

        // Build a minimal SpawnSubagentTool — we won't call execute(), just need
        // it for the LeadAgentToolExecutor construction
        let tracker = Arc::new(SubagentTracker::new());
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
            lead_template_id: "test-lead".to_string(),
            daemon_config: Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
            spawn_count: AtomicUsize::new(0),
            workspace_id: None,
            cancel_token: None,
            tracker: tracker.clone(),
            depth: 0,
            max_concurrent_subagents: DEFAULT_MAX_CONCURRENT_SUBAGENTS,
            concurrency_semaphore: Arc::new(tokio::sync::Semaphore::new(
                DEFAULT_MAX_CONCURRENT_SUBAGENTS,
            )),
        });
        let check_status_tool = Arc::new(CheckSubagentStatusTool {
            tracker: tracker.clone(),
        });
        let wait_tool = Arc::new(WaitForSubagentsTool { tracker });

        let executor =
            LeadAgentToolExecutor::new(spawn_tool, None, check_status_tool, wait_tool, contextual);

        let tools = executor.registered_tools();
        assert!(tools.contains(&"spawn_subagent".to_string()));
        assert!(!tools.contains(&"spawn_subagents_batch".to_string()));
        assert!(tools.contains(&"check_subagent_status".to_string()));
        assert!(tools.contains(&"wait_for_subagents".to_string()));
        assert!(tools.contains(&"web_search".to_string()));
        // workspace tools should be listed since task_id is set
        assert!(tools.contains(&"workspace_read".to_string()));
        assert!(tools.contains(&"workspace_write".to_string()));
    }

    // ── SubagentTracker tests ────────────────────────────────────────

    #[test]
    fn test_tracker_register_and_status() {
        let tracker = SubagentTracker::new();

        tracker.register("run-1");
        assert!(matches!(tracker.get("run-1"), Some(SubagentStatus::Queued)));
        assert!(!tracker.all_done());
    }

    #[test]
    fn test_tracker_complete() {
        let tracker = SubagentTracker::new();

        tracker.register("run-1");
        tracker.complete("run-1", "Result text".to_string(), true);

        match tracker.get("run-1") {
            Some(SubagentStatus::Completed { content, success }) => {
                assert_eq!(content, "Result text");
                assert!(success);
            }
            other => panic!("Expected Completed, got {:?}", other),
        }
        assert!(tracker.all_done());
    }

    #[test]
    fn test_tracker_fail() {
        let tracker = SubagentTracker::new();

        tracker.register("run-1");
        tracker.fail("run-1", "Some error".to_string());

        match tracker.get("run-1") {
            Some(SubagentStatus::Failed { error }) => {
                assert_eq!(error, "Some error");
            }
            other => panic!("Expected Failed, got {:?}", other),
        }
        assert!(tracker.all_done());
    }

    #[test]
    fn test_tracker_all_done_mixed() {
        let tracker = SubagentTracker::new();

        tracker.register("run-1");
        tracker.register("run-2");
        tracker.register("run-3");

        // Partially complete
        tracker.complete("run-1", "done".to_string(), true);
        assert!(!tracker.all_done());

        tracker.fail("run-2", "err".to_string());
        assert!(!tracker.all_done());

        tracker.complete("run-3", "done too".to_string(), true);
        assert!(tracker.all_done());
    }

    #[test]
    fn test_tracker_summary() {
        let tracker = SubagentTracker::new();

        tracker.register("run-1");
        tracker.complete("run-1", "Research result".to_string(), true);
        tracker.register("run-2");
        tracker.fail("run-2", "Timeout".to_string());

        let summary = tracker.summary();
        assert!(summary.contains("run-1"));
        assert!(summary.contains("completed"));
        assert!(summary.contains("Research result"));
        assert!(summary.contains("run-2"));
        assert!(summary.contains("failed"));
        assert!(summary.contains("Timeout"));
    }

    #[test]
    fn test_tracker_empty_summary() {
        let tracker = SubagentTracker::new();
        assert!(tracker.summary().contains("No subagents"));
    }

    #[test]
    fn test_tracker_set_status() {
        let tracker = SubagentTracker::new();

        tracker.register("run-1");
        assert!(matches!(tracker.get("run-1"), Some(SubagentStatus::Queued)));

        tracker.set_status("run-1", SubagentStatus::Running);
        assert!(matches!(
            tracker.get("run-1"),
            Some(SubagentStatus::Running)
        ));
        assert!(!tracker.all_done());

        tracker.set_status(
            "run-1",
            SubagentStatus::Completed {
                content: "done".to_string(),
                success: true,
            },
        );
        assert!(tracker.all_done());
    }

    #[test]
    fn test_tracker_all_done_with_queued() {
        let tracker = SubagentTracker::new();

        tracker.register("run-1");
        tracker.register("run-2");

        // Both queued — not done
        assert!(!tracker.all_done());

        // One running, one queued — not done
        tracker.set_status("run-1", SubagentStatus::Running);
        assert!(!tracker.all_done());

        // One completed, one queued — not done
        tracker.complete("run-1", "done".to_string(), true);
        assert!(!tracker.all_done());

        // Both completed — done
        tracker.complete("run-2", "done too".to_string(), true);
        assert!(tracker.all_done());
    }

    #[test]
    fn test_tracker_status_counts() {
        let tracker = SubagentTracker::new();

        tracker.register("run-1"); // queued
        tracker.register("run-2"); // queued
        tracker.register("run-3"); // queued
        tracker.register("run-4"); // queued

        let (queued, running, completed, failed) = tracker.status_counts();
        assert_eq!((queued, running, completed, failed), (4, 0, 0, 0));

        tracker.set_status("run-1", SubagentStatus::Running);
        tracker.complete("run-2", "done".to_string(), true);
        tracker.fail("run-3", "err".to_string());

        let (queued, running, completed, failed) = tracker.status_counts();
        assert_eq!((queued, running, completed, failed), (1, 1, 1, 1));
    }

    #[test]
    fn test_tracker_summary_with_queued() {
        let tracker = SubagentTracker::new();

        tracker.register("run-1");
        let summary = tracker.summary();
        assert!(summary.contains("run-1"));
        assert!(summary.contains("queued"));
        assert!(summary.contains("waiting for execution slot"));
    }

    #[tokio::test]
    async fn test_check_subagent_status_tool() {
        let tracker = Arc::new(SubagentTracker::new());
        tracker.register("run-abc");
        tracker.complete("run-abc", "Done!".to_string(), true);

        let tool = CheckSubagentStatusTool {
            tracker: tracker.clone(),
        };

        // Completed
        let result = tool
            .execute(&serde_json::json!({"subagent_run_id": "run-abc"}))
            .await;
        assert!(result.is_ok());
        assert!(result.unwrap().contains("completed successfully"));

        // Unknown
        let result = tool
            .execute(&serde_json::json!({"subagent_run_id": "no-such"}))
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_check_subagent_status_tool_queued() {
        let tracker = Arc::new(SubagentTracker::new());
        tracker.register("run-queued");

        let tool = CheckSubagentStatusTool {
            tracker: tracker.clone(),
        };

        // Queued
        let result = tool
            .execute(&serde_json::json!({"subagent_run_id": "run-queued"}))
            .await;
        assert!(result.is_ok());
        let msg = result.unwrap();
        assert!(msg.contains("queued"));
        assert!(msg.contains("execution slot"));

        // Transition to Running
        tracker.set_status("run-queued", SubagentStatus::Running);
        let result = tool
            .execute(&serde_json::json!({"subagent_run_id": "run-queued"}))
            .await;
        assert!(result.is_ok());
        assert!(result.unwrap().contains("still running"));
    }

    // ── Batch spawn tool tests ────────────────────────────────────────

    #[test]
    fn test_batch_spawn_tool_definition_includes_agents() {
        use crate::agent::template::AgentTemplateFrontmatter;

        let templates = vec![AgentTemplate {
            frontmatter: AgentTemplateFrontmatter {
                id: "researcher".to_string(),
                name: "Researcher".to_string(),
                description: "Research agent".to_string(),
                icon: None,
                singleton: false,
                skills: vec!["web_search".to_string()],
                denied_skills: vec![],
                temperature: 0.5,
                verbosity: "normal".to_string(),
                model: None,
                fallback_models: vec![],
                max_tool_calls: None,
                timeout_seconds: None,
                max_cost_per_task: None,
                max_rounds: None,
                require_confirmation_for: vec![],
            },
            body: String::new(),
            sections: HashMap::new(),
        }];

        let def = spawn_subagents_batch_tool_definition(&templates);
        assert_eq!(def.name, "spawn_subagents_batch");
        assert!(def.description.contains("researcher"));
        assert!(def.description.contains("Researcher"));
    }

    #[test]
    fn test_batch_spawn_tool_hidden_when_disabled() {
        // When batch_spawn_enabled is false, registered_tools should NOT list it
        use crate::tools::registry::{RegisteredTool, ToolBackend};

        struct NoopTool;
        #[async_trait]
        impl BuiltInTool for NoopTool {
            async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
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

        let tracker = Arc::new(SubagentTracker::new());
        let spawn_tool = Arc::new(SpawnSubagentTool::new(
            Arc::new(openalpaca_llm::LlmRouter::new(
                std::collections::HashMap::new(),
                openalpaca_llm::ModelRegistry::new(std::collections::HashMap::new()),
                std::collections::HashMap::new(),
                Arc::new(openalpaca_llm::CostTracker::new(
                    openalpaca_llm::ModelRegistry::new(std::collections::HashMap::new()),
                )),
                "test-model".to_string(),
            )),
            registry.clone(),
            Arc::new(SharedContext::new()),
            EventBus::default(),
            None,
            "task-1".to_string(),
            "user-1".to_string(),
            "test-lead".to_string(),
            Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
            None,
            tracker.clone(),
            0,
            DEFAULT_MAX_CONCURRENT_SUBAGENTS,
            None, // workspace_id
        ));
        let check_tool = Arc::new(CheckSubagentStatusTool {
            tracker: tracker.clone(),
        });
        let wait_tool = Arc::new(WaitForSubagentsTool { tracker });
        let ctx_exec = ToolExecutionContext {
            owner_id: None,
            task_id: Some("task-1".to_string()),
            agent_id: None,
            db: None,
            workspace_id: None,
        };
        let contextual = Arc::new(ContextualToolExecutor::new(registry, ctx_exec));

        // batch_spawn_tool = None -> not in registered_tools
        let executor =
            LeadAgentToolExecutor::new(spawn_tool, None, check_tool, wait_tool, contextual);
        let tools = executor.registered_tools();
        assert!(!tools.contains(&"spawn_subagents_batch".to_string()));
        assert!(tools.contains(&"spawn_subagent".to_string()));
    }

    #[test]
    fn test_batch_spawn_tool_present_when_enabled() {
        use crate::tools::registry::{RegisteredTool, ToolBackend};

        struct NoopTool;
        #[async_trait]
        impl BuiltInTool for NoopTool {
            async fn execute(&self, _arguments: &serde_json::Value) -> Result<String, String> {
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

        let tracker = Arc::new(SubagentTracker::new());
        let spawn_tool = Arc::new(SpawnSubagentTool::new(
            Arc::new(openalpaca_llm::LlmRouter::new(
                std::collections::HashMap::new(),
                openalpaca_llm::ModelRegistry::new(std::collections::HashMap::new()),
                std::collections::HashMap::new(),
                Arc::new(openalpaca_llm::CostTracker::new(
                    openalpaca_llm::ModelRegistry::new(std::collections::HashMap::new()),
                )),
                "test-model".to_string(),
            )),
            registry.clone(),
            Arc::new(SharedContext::new()),
            EventBus::default(),
            None,
            "task-1".to_string(),
            "user-1".to_string(),
            "test-lead".to_string(),
            Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
            None,
            tracker.clone(),
            0,
            DEFAULT_MAX_CONCURRENT_SUBAGENTS,
            None, // workspace_id
        ));
        let batch_tool = Some(Arc::new(SpawnSubagentsBatchTool::new(spawn_tool.clone())));
        let check_tool = Arc::new(CheckSubagentStatusTool {
            tracker: tracker.clone(),
        });
        let wait_tool = Arc::new(WaitForSubagentsTool { tracker });
        let ctx_exec = ToolExecutionContext {
            owner_id: None,
            task_id: Some("task-1".to_string()),
            agent_id: None,
            db: None,
            workspace_id: None,
        };
        let contextual = Arc::new(ContextualToolExecutor::new(registry, ctx_exec));

        // batch_spawn_tool = Some -> IS in registered_tools
        let executor = LeadAgentToolExecutor::new(
            spawn_tool,
            batch_tool,
            check_tool,
            wait_tool,
            contextual,
        );
        let tools = executor.registered_tools();
        assert!(tools.contains(&"spawn_subagents_batch".to_string()));
        assert!(tools.contains(&"spawn_subagent".to_string()));
    }

    #[tokio::test]
    async fn test_batch_spawn_empty_array_error() {
        let tracker = Arc::new(SubagentTracker::new());
        let spawn_tool = Arc::new(SpawnSubagentTool::new(
            Arc::new(openalpaca_llm::LlmRouter::new(
                std::collections::HashMap::new(),
                openalpaca_llm::ModelRegistry::new(std::collections::HashMap::new()),
                std::collections::HashMap::new(),
                Arc::new(openalpaca_llm::CostTracker::new(
                    openalpaca_llm::ModelRegistry::new(std::collections::HashMap::new()),
                )),
                "test-model".to_string(),
            )),
            Arc::new(ToolRegistry::new()),
            Arc::new(SharedContext::new()),
            EventBus::default(),
            None,
            "task-1".to_string(),
            "user-1".to_string(),
            "test-lead".to_string(),
            Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
            None,
            tracker,
            0,
            DEFAULT_MAX_CONCURRENT_SUBAGENTS,
            None, // workspace_id
        ));
        let batch_tool = SpawnSubagentsBatchTool::new(spawn_tool);

        let result = batch_tool
            .execute(&serde_json::json!({"subagents": []}))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("at least 1"));
    }

    #[tokio::test]
    async fn test_batch_spawn_exceeds_max_error() {
        let tracker = Arc::new(SubagentTracker::new());
        let spawn_tool = Arc::new(SpawnSubagentTool::new(
            Arc::new(openalpaca_llm::LlmRouter::new(
                std::collections::HashMap::new(),
                openalpaca_llm::ModelRegistry::new(std::collections::HashMap::new()),
                std::collections::HashMap::new(),
                Arc::new(openalpaca_llm::CostTracker::new(
                    openalpaca_llm::ModelRegistry::new(std::collections::HashMap::new()),
                )),
                "test-model".to_string(),
            )),
            Arc::new(ToolRegistry::new()),
            Arc::new(SharedContext::new()),
            EventBus::default(),
            None,
            "task-1".to_string(),
            "user-1".to_string(),
            "test-lead".to_string(),
            Arc::new(ArcSwap::from_pointee(DaemonConfig::default())),
            None,
            tracker,
            0,
            DEFAULT_MAX_CONCURRENT_SUBAGENTS,
            None, // workspace_id
        ));
        let batch_tool = SpawnSubagentsBatchTool::new(spawn_tool);

        // 9 items should fail (max 8)
        let items: Vec<serde_json::Value> = (0..9)
            .map(|i| {
                serde_json::json!({
                    "agent_id": format!("agent-{}", i),
                    "objective": format!("task-{}", i)
                })
            })
            .collect();
        let result = batch_tool
            .execute(&serde_json::json!({"subagents": items}))
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("max 8"));
    }
}
