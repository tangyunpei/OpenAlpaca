use crate::agent::template::AgentTemplate;
use crate::compose::{
    ComposeEngine, ComposeOverrides, ComposeRequest, DynamicContextInput, DynamicContextMode,
    HistoryInput, HistoryMode, PersonaInput, PersonaMode, StaticPromptInput, StaticPromptMode,
    SummaryWrapMode, SystemBlock,
};
use crate::middleware::prompt::SystemPersona;
use crate::prompt_ctx::section::{ContextBundle, SectionPriority};
use crate::prompt_ctx::ExecutionPath;
use std::sync::Arc;

/// Build the system prompt for the Lead Agent from agent templates.
///
/// Phase 6 Commit 3: routes through `ComposeEngine::compose` with
/// `PersonaMode::Skip` + `StaticPromptMode::SubagentMinimal` +
/// `DynamicContextMode::Skip` + `HistoryMode::Skip`. The 8 lead-agent
/// raw_system_blocks (lead_persona, lead_role, agents_catalog, workflow,
/// delegation_criteria, tool_patterns, failure_recovery,
/// output_expectations) are carried into `StaticPromptInput.raw_blocks` in
/// canonical registration order. No tools / connector_guidance /
/// context_bundle / history flow through this entry-point — those are layered
/// in by `run_lead_agent` at the call site.
///
/// Returns just the system prompt string (callers expect `String`);
/// `run_lead_agent` then concatenates `format_tool_guidance(...)` +
/// `connector_suffix` inline before building the message vec. See the golden
/// fixture `test_golden_lead_agent_byte_identical` for the invariant.
pub fn build_lead_agent_prompt_from_templates(
    compose_engine: &ComposeEngine,
    base_persona: &str,
    templates: &[AgentTemplate],
) -> String {
    // Use a large default window — lead agent always gets generous context.
    let model_window: usize = 200_000;

    // Available agents catalog (High — important for delegation decisions).
    let agents_block = if templates.is_empty() {
        "<agents>\nNo worker agents are currently available. Complete the task directly.\n</agents>".to_string()
    } else {
        let mut block = String::from("<agents>\n");
        for t in templates {
            let fm = &t.frontmatter;
            let capabilities_str = if fm.capabilities.is_empty() {
                "none".to_string()
            } else {
                fm.capabilities.join(", ")
            };
            block.push_str(&format!(
                "- id=\"{}\" name=\"{}\" capabilities=[{}]: {}\n",
                fm.id, fm.name, capabilities_str, fm.description
            ));
        }
        block.push_str("</agents>");
        block
    };

    // ── The 8 lead-agent raw blocks in canonical registration order. ──────
    // String constants copied verbatim from the pre-migration
    // PromptBuilder chain; the exact bytes are load-bearing.
    let role_text = "<role>\n\
         You are a Lead Agent orchestrating a complex task. You are responsible for analyzing \
         the user's request, decomposing it into sub-objectives, delegating work to specialized \
         subagents, and synthesizing their results into a final response.\n\
         Do not attempt to perform specialized work (coding, research, analysis) yourself when \
         a suitable subagent is available. Your value is in orchestration and synthesis.\n\
         </role>";
    let workflow_text = "<workflow>\n\
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
         </workflow>";
    let delegation_text = "<delegation-criteria>\n\
         Spawn subagents when:\n\
         - Tasks can run in parallel (e.g., research + implementation are independent)\n\
         - Tasks require isolated context or specialized skills\n\
         - Tasks involve independent workstreams that do not need shared state\n\n\
         Work directly (do NOT spawn) when:\n\
         - The task is simple enough to answer from your own knowledge\n\
         - You are synthesizing, summarizing, or formatting existing results\n\
         - The task requires maintaining context across sequential steps that one agent handles best\n\
         </delegation-criteria>";
    let tool_patterns_text = "<tools>\n\
         spawn_subagent: Spawning is always immediate — returns a run_id instantly. The system \
         automatically queues execution if LLM capacity is limited. Spawn all independent \
         objectives in a single round before waiting — this is the preferred pattern.\n\
         spawn_subagents_batch: When spawning 3+ independent subagents, use spawn_subagents_batch \
         for parallel spawning instead of individual spawn_subagent calls. This is more efficient \
         and reduces round-trips.\n\
         check_subagent_status: Poll a single subagent by run_id. Shows whether the subagent is \
         queued, running, completed, or failed.\n\
         wait_for_subagents: Block until ALL spawned subagents finish, including any that are \
         queued for execution. Returns a summary of all results. Call this after spawning all \
         subagents.\n\
         workspace_read / workspace_write: Share context between subagents. Write setup data before spawning; \
         read results after completion.\n\
         </tools>";
    let failure_recovery_text = "<failure-recovery>\n\
         If a subagent fails:\n\
         1. Read the error message to understand the failure type.\n\
         2. If the objective was too broad, split it into smaller sub-objectives and retry.\n\
         3. If the agent lacked the right skills, try a different agent.\n\
         4. If repeated failures occur, complete that sub-objective directly yourself.\n\
         5. Never silently drop a failed sub-objective — always report what succeeded and what did not.\n\
         </failure-recovery>";
    let output_expectations_text = "<output>\n\
         Your final response must directly address the user's original request. \
         Synthesize all subagent results into a single coherent answer. \
         Do not simply list raw subagent outputs — integrate, summarize, and resolve any conflicts.\n\
         </output>";

    let raw_blocks: Vec<SystemBlock> = vec![
        SystemBlock {
            name: "lead_persona",
            content: Arc::<str>::from(base_persona.to_string()),
            priority: SectionPriority::Critical,
        },
        SystemBlock {
            name: "lead_role",
            content: Arc::<str>::from(role_text.to_string()),
            priority: SectionPriority::Critical,
        },
        SystemBlock {
            name: "agents_catalog",
            content: Arc::<str>::from(agents_block.clone()),
            priority: SectionPriority::High,
        },
        SystemBlock {
            name: "workflow",
            content: Arc::<str>::from(workflow_text.to_string()),
            priority: SectionPriority::High,
        },
        SystemBlock {
            name: "delegation_criteria",
            content: Arc::<str>::from(delegation_text.to_string()),
            priority: SectionPriority::Normal,
        },
        SystemBlock {
            name: "tool_patterns",
            content: Arc::<str>::from(tool_patterns_text.to_string()),
            priority: SectionPriority::Normal,
        },
        SystemBlock {
            name: "failure_recovery",
            content: Arc::<str>::from(failure_recovery_text.to_string()),
            priority: SectionPriority::Normal,
        },
        SystemBlock {
            name: "output_expectations",
            content: Arc::<str>::from(output_expectations_text.to_string()),
            priority: SectionPriority::Normal,
        },
    ];

    let persona_input = PersonaInput {
        system_persona: Arc::new(SystemPersona::default()),
        user_document: Arc::new(None),
        identity_document: Arc::new(None),
        persona_version: 0,
        mode: PersonaMode::Skip,
        identity_budget: None,
        user_budget: None,
    };
    let persona_output = Arc::new(crate::compose::persona::compute(&persona_input));

    let static_prompt_input = StaticPromptInput {
        persona_output,
        agent_persona: None,
        agent_config_fingerprint: [0u8; 32],
        skill_block: None,
        skills_catalog: None,
        bootstrap: None,
        tools: Arc::new(Vec::new()),
        connector_status: Arc::new(Vec::new()),
        send_tool_context: None,
        message_source: None,
        raw_blocks,
        planner_agents: None,
        planner_protocol_v2: false,
        mode: StaticPromptMode::SubagentMinimal,
        model_window: model_window as u32,
    };

    // No bundle / no history — this helper returns ONLY the system prompt
    // string. run_lead_agent adds tools + connector_guidance + history via
    // inline string concatenation after this call returns.
    let dynamic_context_input = DynamicContextInput {
        context_bundle: Arc::new(ContextBundle::empty()),
        query: Arc::from(""),
        memory_retrieval_hash: [0u8; 32],
        path: ExecutionPath::LeadAgent,
        reserved_tokens: 0,
        mode: DynamicContextMode::Skip,
    };

    let history_input = HistoryInput {
        lane_tip_fingerprint: [0u8; 32],
        summary: None,
        summary_wrap_mode: SummaryWrapMode::Plain,
        recent_messages: Arc::new(Vec::new()),
        current_user_turn: None,
        mode: HistoryMode::Skip,
    };

    let request = ComposeRequest::LeadAgent {
        base_persona: Arc::<str>::from(base_persona.to_string()),
        agents_catalog: Arc::<str>::from(agents_block),
        objective: Arc::<str>::from(""),
        overrides: ComposeOverrides::default(),
    };

    let composed = compose_engine.compose(
        &request,
        persona_input,
        static_prompt_input,
        dynamic_context_input,
        history_input,
        model_window as u32,
        Arc::new(Vec::new()),
        None, // no bus handle in scope here
        None, // lane: lead agent prompt is not lane-scoped
    );

    // Extract the system message from the composed output. SubagentMinimal +
    // Skip modes yield exactly one system message; fall back to empty string
    // if assembly ever drifts.
    composed
        .messages
        .iter()
        .find(|m| matches!(m.role, openalpaca_llm::Role::System))
        .map(|m| m.content.clone())
        .unwrap_or_default()
}
