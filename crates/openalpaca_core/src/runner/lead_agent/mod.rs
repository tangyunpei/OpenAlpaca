//! Lead Agent orchestrator: a full agentic loop that dynamically spawns
//! subagents, observes their results, adjusts strategy, and synthesizes
//! a final response. Follows Anthropic's recommended multi-agent pattern.
//!
//! The Lead Agent is a configurable `SubAgent` registered in the agent
//! registry with its own model, persona, and constraints. It receives a
//! `spawn_subagent` tool that delegates work to other agents.

mod guard;
mod prompt;
mod tools;
mod tracker;

// External API (unchanged)
pub(crate) use guard::AgentBusyGuard;
pub use prompt::build_lead_agent_prompt_from_templates;
pub use tools::{
    check_subagent_status_tool_definition, register_coordination_tools,
    spawn_subagent_tool_definition_from_templates,
    spawn_subagents_batch_tool_definition, wait_for_subagents_tool_definition,
    CheckSubagentStatusTool, SpawnSubagentTool,
    SpawnSubagentsBatchTool, WaitForSubagentsTool,
};
pub use tracker::{SubagentStatus, SubagentTracker};

use crate::agent::subagent::SubAgent;
use crate::agent::template::AgentTemplate;
use crate::bus::EventBus;
use crate::context::SharedContext;
use crate::daemon_config::DaemonConfig;
use crate::middleware::prompt::format_tool_guidance;
use crate::prompt_ctx::ContextManager;
use crate::prompt_ctx::section::ContextBundle;
use crate::runner::{LoopConfig, LoopResult, run_agentic_loop_routed};
use crate::security::sandbox::{SandboxManager, SandboxPolicy};
use crate::tools::ToolRegistry;
use crate::tools::registry::ToolContext;
use arc_swap::ArcSwap;
use openalpaca_llm::{ChatMessage, LlmRouter};
use openalpaca_storage::Database;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

// Test-only imports (for `use super::*` in tests.rs)
#[cfg(test)]
use async_trait::async_trait;
#[cfg(test)]
use crate::tools::registry::BuiltInTool;
#[cfg(test)]
use openalpaca_llm::ToolDefinition;
#[cfg(test)]
use std::collections::HashMap;

/// Default maximum number of concurrent subagents (used in tests).
#[cfg(test)]
const DEFAULT_MAX_CONCURRENT_SUBAGENTS: usize = 5;

// ── Result types ─────────────────────────────────────────────────────

/// Result of a lead agent execution.
pub struct LeadAgentResult {
    pub success: bool,
    pub final_content: String,
    pub loop_result: LoopResult,
    pub subagents_spawned: usize,
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
    connector_guidance: &str,
    confirmation_broker: Option<Arc<crate::security::confirmation::ConfirmationBroker>>,
    context_manager: Arc<ContextManager>,
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

    // 4. Build coordination tools with shared SubagentTracker
    let tracker = Arc::new(SubagentTracker::new());

    // Resolve parent context bundle once, shared across all subagent spawns.
    // TODO: In future, resolve via context_manager.resolve() with LeadAgent path
    let parent_bundle = Arc::new(ContextBundle::empty());

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
        confirmation_broker.clone(),
        context_manager,
        parent_bundle,
    ));

    let check_status_tool = Arc::new(CheckSubagentStatusTool {
        tracker: tracker.clone(),
    });
    let wait_tool = Arc::new(WaitForSubagentsTool {
        tracker: tracker.clone(),
    });

    let tool_ctx = ToolContext {
        agent_id: Some(lead_agent.id.clone()),
        task_id: Some(task_id.to_string()),
        owner_id: Some(created_by.to_string()),
        workspace_id: workspace_id.clone(),
    };

    // Build a per-request ToolRegistry containing the base tools plus
    // lead agent coordination tools (spawn, check_status, wait, batch_spawn).
    let lead_registry = (*tool_registry).clone();
    let batch_tool = if batch_spawn_enabled {
        Some(Arc::new(SpawnSubagentsBatchTool::new(spawn_tool.clone())))
    } else {
        None
    };
    let batch_def = if batch_spawn_enabled {
        Some(spawn_subagents_batch_tool_definition(&worker_templates))
    } else {
        None
    };
    register_coordination_tools(
        &lead_registry,
        spawn_tool.clone(),
        batch_tool,
        check_status_tool,
        wait_tool,
        spawn_subagent_tool_definition_from_templates(&worker_templates),
        batch_def,
        check_subagent_status_tool_definition(),
        wait_for_subagents_tool_definition(),
    );
    let lead_registry = Arc::new(lead_registry);

    // 5. Build SandboxManager with lead agent's policy
    let mut sandbox = SandboxManager::with_defaults(lead_registry, bus.clone());
    if let Some(ref broker) = confirmation_broker {
        sandbox.set_confirmation_broker(broker.clone());
    }
    let mut sandbox_policy = SandboxPolicy::from_constraints(&lead_agent.id, &lead_agent.constraints);
    if daemon_config.load().security.auto_approve_confirmations {
        sandbox_policy.auto_approve = true;
    }

    // 6. Build system prompt from templates
    let system_prompt =
        build_lead_agent_prompt_from_templates(&lead_agent.preset.persona, &worker_templates);
    let tool_guidance = format_tool_guidance(&tools);
    let connector_suffix = if !connector_guidance.is_empty() {
        format!("\n{}", connector_guidance)
    } else {
        String::new()
    };
    let full_system = format!("{}{}{}", system_prompt, tool_guidance, connector_suffix);

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
                &crate::orchestrator::wrap_untrusted_context(
                    &block,
                    "retrieved_memory",
                    "retrieved",
                ),
            ));
        }
    }

    messages.push(ChatMessage::user(task_description));

    // 8. Build LoopConfig from lead agent defaults + agent constraint overrides
    let mut loop_config = LoopConfig::from_lead_agent(
        &daemon_config.load().execution.lead_agent_defaults,
        lead_agent,
    )
    .with_context_window(
        router.model_registry(),
        lead_agent.llm_config.model.as_deref(),
    );

    // Set compaction model from daemon config
    loop_config.compaction_model = daemon_config.load()
        .execution.context.compaction_model.clone();
    loop_config.event_bus = Some(bus.clone());

    // Instantiate ContextBudgetManager for budget-aware compaction
    let context_budget = {
        let default_model = router.default_model();
        let model_id = lead_agent.llm_config.model.as_deref()
            .unwrap_or(&default_model);
        let context_window = router.model_registry()
            .get_model_info(model_id)
            .map(|info| info.context_window as usize)
            .unwrap_or(200_000);
        crate::context_budget::ContextBudgetManager::new(
            context_window,
            &daemon_config.load().execution.context,
        )
    };

    // --- Context Budget Telemetry ---
    {
        let default_model = router.default_model();
        let model_id = lead_agent.llm_config.model.as_deref()
            .unwrap_or(&default_model);
        let model_window = context_budget.model_context_window();
        let request_id = uuid::Uuid::new_v4();
        // Estimate system prompt tokens (chars / 4 heuristic)
        let system_prompt_tokens = full_system.len() / 4;
        let mut budget_snapshot =
            crate::context_budget::ContextBudgetManager::new(
                model_window,
                &daemon_config.load().execution.context,
            );
        budget_snapshot.register_section("system_prompt", system_prompt_tokens);
        budget_snapshot.register_section("tools", tools.len() * 200);

        tracing::debug!(
            request_id = %request_id,
            agent_id = %lead_agent.id,
            model_window,
            fixed_zone = budget_snapshot.fixed_zone_tokens(),
            free_zone = budget_snapshot.free_zone_capacity(),
            buffer = budget_snapshot.autocompact_buffer(),
            "Context budget computed (lead agent)"
        );

        bus.publish(crate::events::SystemEvent::ContextBudgetComputed {
            request_id,
            model: model_id.to_string(),
            window_size: model_window,
            fixed_zone_tokens: budget_snapshot.fixed_zone_tokens(),
            free_zone_tokens: budget_snapshot.free_zone_capacity(),
            buffer_size: budget_snapshot.autocompact_buffer(),
            section_breakdown: budget_snapshot
                .section_breakdown()
                .into_iter()
                .map(|(n, t)| (n.to_string(), t))
                .collect(),
            timestamp: chrono::Utc::now(),
        });
    }

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
        Some(&context_budget),
        cancel_token,
        Some(&tool_ctx),
    )
    .await;

    let success = matches!(
        &result.finish_reason,
        crate::runner::LoopFinishReason::Complete
            | crate::runner::LoopFinishReason::MaxRounds
            | crate::runner::LoopFinishReason::Truncated
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
mod tests;
