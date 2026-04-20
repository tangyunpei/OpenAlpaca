use super::guard::AgentBusyGuard;
use super::tracker::{SubagentStatus, SubagentTracker};
use crate::agent::template::AgentTemplate;
use crate::bus::EventBus;
use crate::compose::{
    ComposeOverrides, ComposeRequest, DynamicContextInput, DynamicContextMode, HistoryInput,
    HistoryMode, PersonaInput, PersonaMode, StaticPromptInput, StaticPromptMode, SummaryWrapMode,
    SystemBlock,
};
use crate::context::SharedContext;
use crate::daemon_config::DaemonConfig;
use crate::events::SystemEvent;
use crate::middleware::prompt::{format_tool_guidance, SystemPersona};
use crate::prompt_ctx::ContextManager;
use crate::prompt_ctx::section::ContextBundle;
use crate::prompt_ctx::{ExecutionPath, SectionPriority};
use crate::runner::{LoopConfig, run_agentic_loop_routed};
use crate::security::sandbox::{SandboxManager, SandboxPolicy};
use crate::tools::registry::{BuiltInTool, RegisteredTool, ToolBackend, ToolContext};
use crate::tools::ToolRegistry;
use arc_swap::ArcSwap;
use async_trait::async_trait;
use chrono::Utc;
use openalpaca_llm::{ChatMessage, LlmRouter, ToolDefinition};
use openalpaca_storage::Database;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Maximum nesting depth for subagent spawning.
/// Prevents indirect recursion (e.g., A spawns B spawns C spawns A...).
/// Depth 0 = top-level lead agent, depth 1 = its direct subagents, etc.
const MAX_SUBAGENT_DEPTH: u32 = 3;

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
    /// Pre-computed static part of the subagent system prompt (Opt-LA-3).
    /// Contains `{PERSONA}` and `{TOOL_GUIDANCE}` placeholders for substitution.
    /// Kept for fallback; main path uses PromptBuilder with context distillation.
    #[allow(dead_code)]
    prompt_template: String,
    /// Optional confirmation broker for interactive tool approval.
    confirmation_broker: Option<Arc<crate::security::confirmation::ConfirmationBroker>>,
    /// ContextManager for distilling parent context into sub-agent packages.
    context_manager: Arc<ContextManager>,
    /// Parent context bundle (resolved once, shared across spawns).
    parent_bundle: Arc<ContextBundle>,
    /// Layered compose engine. Phase 6 Commit 3: routes subagent
    /// system-prompt + message-list assembly through `ComposeEngine::compose`
    /// with `PersonaMode::Skip` + `StaticPromptMode::SubagentMinimal`.
    compose_engine: Arc<crate::compose::ComposeEngine>,
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
        confirmation_broker: Option<Arc<crate::security::confirmation::ConfirmationBroker>>,
        context_manager: Arc<ContextManager>,
        parent_bundle: Arc<ContextBundle>,
        compose_engine: Arc<crate::compose::ComposeEngine>,
    ) -> Self {
        let prompt_template = "\
            <identity>\n{PERSONA}\n</identity>\n\n\
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
            </constraints>{TOOL_GUIDANCE}"
            .to_string();

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
            prompt_template,
            confirmation_broker,
            context_manager,
            parent_bundle,
            compose_engine,
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

        // 5. Build SandboxManager for subagent
        let subagent_tool_ctx = ToolContext {
            agent_id: Some(agent_id.to_string()),
            task_id: Some(self.task_id.clone()),
            owner_id: Some(self.created_by.clone()),
            workspace_id: self.workspace_id.clone(),
            skill_stack: vec![],
            effective_constraints: None,
        };
        let mut sandbox = SandboxManager::with_defaults(self.tool_registry.clone(), self.bus.clone());
        if let Some(ref broker) = self.confirmation_broker {
            sandbox.set_confirmation_broker(broker.clone());
        }

        // 6. Resolve tools for subagent's skills
        let tools = crate::tools::resolve_agent_tools(&agent, &self.tool_registry);

        // 7. Build LoopConfig from daemon defaults + agent constraints
        let mut loop_config =
            LoopConfig::from_agent(&self.daemon_config.load().execution.agent_defaults, &agent)
                .with_context_window(
                    self.router.model_registry(),
                    agent.llm_config.model.as_deref(),
                );
        loop_config.event_bus = Some(self.bus.clone());
        loop_config.experimental_ephemeral_pressure = self
            .daemon_config
            .load()
            .experimental
            .ephemeral_pressure_layer;

        // 8. Build messages with context distillation via PromptBuilder
        let default_model = self.router.default_model();
        let model_id = agent.llm_config.model.as_deref()
            .unwrap_or(&default_model);
        let model_window = self.router.model_registry()
            .get_model_info(model_id)
            .map(|info| info.context_window as usize)
            .unwrap_or(200_000);

        // Distill parent context for this sub-agent
        let context_package = self.context_manager.distill(
            &self.parent_bundle,
            &agent.constraints,
            model_window,
            objective,
            None, // no predecessor handoff for lead agent spawns
        );
        let bundle = context_package.to_bundle();

        // ── Phase 6 Commit 3: route system-prompt + message-list assembly
        // through the layered compose engine. `PersonaMode::Skip` +
        // `StaticPromptMode::SubagentMinimal` (raw-blocks-only) +
        // `DynamicContextMode::Default` (bundle → context messages) +
        // `HistoryMode::Default` (objective → current_user_turn) reproduces
        // the pre-migration PromptBuilder chain byte-identically. See
        // `test_golden_lead_agent_spawn_subagent_byte_identical` for the
        // invariant.
        let identity_block = format!("<identity>\n{}\n</identity>", agent.preset.persona);
        let scope_block = "<scope>\nYou are a subagent working on a single objective assigned by a lead agent. \
                 Focus exclusively on your assigned objective. Do not attempt work outside your scope.\n</scope>";
        let output_block = "<output-format>\nProvide a clear, complete result. Start with a brief summary of what you accomplished, \
                 followed by the detailed output. The lead agent will use your result to synthesize a \
                 final response, so be thorough and specific.\n</output-format>";
        let constraints_block = "<constraints>\nYou operate independently — you cannot communicate with other subagents directly. \
                 Use workspace_read and workspace_write tools to access or share data across agents.\n</constraints>";

        let mut raw_blocks: Vec<SystemBlock> = Vec::with_capacity(5);
        raw_blocks.push(SystemBlock {
            name: "agent_identity",
            content: Arc::<str>::from(identity_block),
            priority: SectionPriority::High,
        });
        raw_blocks.push(SystemBlock {
            name: "scope",
            content: Arc::<str>::from(scope_block.to_string()),
            priority: SectionPriority::Normal,
        });
        raw_blocks.push(SystemBlock {
            name: "output_format",
            content: Arc::<str>::from(output_block.to_string()),
            priority: SectionPriority::Normal,
        });
        raw_blocks.push(SystemBlock {
            name: "constraints",
            content: Arc::<str>::from(constraints_block.to_string()),
            priority: SectionPriority::Normal,
        });
        let tools_rendered = format_tool_guidance(&tools);
        if !tools_rendered.is_empty() {
            raw_blocks.push(SystemBlock {
                name: "tools",
                content: Arc::<str>::from(tools_rendered),
                priority: SectionPriority::Normal,
            });
        }

        let persona_input = PersonaInput {
            system_persona: Arc::new(SystemPersona::default()),
            user_document: Arc::new(None),
            identity_document: Arc::new(None),
            persona_version: 0,
            mode: PersonaMode::Skip,
        };
        let persona_output = Arc::new(crate::compose::persona::compute(&persona_input));

        let tools_arc: Arc<Vec<ToolDefinition>> = Arc::new(tools.clone());

        let static_prompt_input = StaticPromptInput {
            persona_output,
            agent_persona: None,
            agent_config_fingerprint: [0u8; 32],
            skill_block: None,
            skills_catalog: None,
            bootstrap: None,
            tools: tools_arc.clone(),
            connector_status: Arc::new(Vec::new()),
            send_tool_context: None,
            message_source: None,
            raw_blocks,
            planner_agents: None,
            planner_protocol_v2: false,
            mode: StaticPromptMode::SubagentMinimal,
            model_window: model_window as u32,
        };

        // Bundle routes through Layer 3 (DynamicContextMode::Default) — same
        // routing as PromptBuilder::context_bundle.
        let dynamic_context_input = DynamicContextInput {
            context_bundle: Arc::new(bundle),
            query: Arc::from(objective),
            memory_retrieval_hash: [0u8; 32],
            path: ExecutionPath::LeadAgent,
            reserved_tokens: 0,
            mode: DynamicContextMode::Default,
        };

        // objective → current_user_turn (no summary, no recent history).
        let history_input = HistoryInput {
            lane_tip_fingerprint: [0u8; 32],
            summary: None,
            summary_wrap_mode: SummaryWrapMode::Plain,
            recent_messages: Arc::new(Vec::new()),
            current_user_turn: Some(ChatMessage::user(objective)),
            mode: HistoryMode::Default,
        };

        let compose_request = ComposeRequest::DagNode {
            agent: Arc::new(agent.clone()),
            assignment: Arc::<str>::from(objective.to_string()),
            workspace_context: Arc::<str>::from(""),
            tools: tools_arc.clone(),
            overrides: ComposeOverrides::default(),
        };

        let composed = self.compose_engine.compose(
            &compose_request,
            persona_input,
            static_prompt_input,
            dynamic_context_input,
            history_input,
            model_window as u32,
            tools_arc.clone(),
            Some(&self.bus),
            None, // spawned subagents have no natural lane key.
        );

        // Clone out of the Arc — run_agentic_loop_routed needs Vec<ChatMessage>.
        let messages: Vec<ChatMessage> = composed.messages.as_ref().clone();

        let mut sandbox_policy = SandboxPolicy::from_constraints(&instance_id, &agent.constraints);
        if self.daemon_config.load().security.auto_approve_confirmations {
            sandbox_policy.auto_approve = true;
        }

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
                None, // context_budget
                child_token,
                Some(&subagent_tool_ctx),
                None,
            )
            .await;

            let duration_ms = agent_start.elapsed().as_millis() as u64;
            let now = Utc::now();

            let agent_success = matches!(
                &result.finish_reason,
                crate::runner::LoopFinishReason::Complete
                    | crate::runner::LoopFinishReason::MaxRounds
                    | crate::runner::LoopFinishReason::Truncated
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
            let capabilities = fm.capabilities.join(", ");
            format!(
                "- ID: \"{}\", Name: \"{}\", Capabilities: [{}]",
                fm.id, fm.name, capabilities
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
        strict: None,
        input_examples: None,
    }
}

// ── CheckSubagentStatusTool ──────────────────────────────────────────

/// Tool that allows the lead agent to check the status of a spawned subagent.
pub struct CheckSubagentStatusTool {
    pub(super) tracker: Arc<SubagentTracker>,
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
        strict: None,
        input_examples: None,
    }
}

// ── WaitForSubagentsTool ─────────────────────────────────────────────

/// Tool that blocks until all spawned subagents have completed,
/// then returns a summary of all results.
pub struct WaitForSubagentsTool {
    pub(super) tracker: Arc<SubagentTracker>,
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
        strict: None,
        input_examples: None,
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
            let capabilities = fm.capabilities.join(", ");
            format!(
                "- ID: \"{}\", Name: \"{}\", Capabilities: [{}], Description: \"{}\"",
                fm.id, fm.name, capabilities, fm.description
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
        strict: None,
        input_examples: None,
    }
}

// ── register_coordination_tools ──────────────────────────────────────

/// Register the 4 lead-agent coordination tools into a `ToolRegistry`.
///
/// Can be called at any time — the registry uses DashMap internally and
/// does not require `&mut`.
#[allow(clippy::too_many_arguments)]
pub fn register_coordination_tools(
    registry: &crate::tools::ToolRegistry,
    spawn_tool: Arc<SpawnSubagentTool>,
    batch_spawn_tool: Option<Arc<SpawnSubagentsBatchTool>>,
    check_status_tool: Arc<CheckSubagentStatusTool>,
    wait_tool: Arc<WaitForSubagentsTool>,
    spawn_def: openalpaca_llm::ToolDefinition,
    batch_def: Option<openalpaca_llm::ToolDefinition>,
    check_def: openalpaca_llm::ToolDefinition,
    wait_def: openalpaca_llm::ToolDefinition,
) {
    // These are known-good builtin tool definitions — unwrap is safe.
    registry.register(RegisteredTool {
        definition: spawn_def,
        backend: ToolBackend::BuiltIn(spawn_tool),
        provides_capabilities: vec!["orchestration".to_string()],
        exempt_from_timeout: false,
        annotations: None,
        version: env!("CARGO_PKG_VERSION").to_string(),
        author: "builtin".to_string(),
        created_at: chrono::Utc::now(),
    }).unwrap();
    if let (Some(batch), Some(def)) = (batch_spawn_tool, batch_def) {
        registry.register(RegisteredTool {
            definition: def,
            backend: ToolBackend::BuiltIn(batch),
            provides_capabilities: vec!["orchestration".to_string()],
            exempt_from_timeout: false,
            annotations: None,
            version: env!("CARGO_PKG_VERSION").to_string(),
            author: "builtin".to_string(),
            created_at: chrono::Utc::now(),
        }).unwrap();
    }
    registry.register(RegisteredTool {
        definition: check_def,
        backend: ToolBackend::BuiltIn(check_status_tool),
        provides_capabilities: vec!["orchestration".to_string()],
        exempt_from_timeout: true,
        annotations: None,
        version: env!("CARGO_PKG_VERSION").to_string(),
        author: "builtin".to_string(),
        created_at: chrono::Utc::now(),
    }).unwrap();
    registry.register(RegisteredTool {
        definition: wait_def,
        backend: ToolBackend::BuiltIn(wait_tool),
        provides_capabilities: vec!["orchestration".to_string()],
        exempt_from_timeout: true,
        annotations: None,
        version: env!("CARGO_PKG_VERSION").to_string(),
        author: "builtin".to_string(),
        created_at: chrono::Utc::now(),
    }).unwrap();
}
