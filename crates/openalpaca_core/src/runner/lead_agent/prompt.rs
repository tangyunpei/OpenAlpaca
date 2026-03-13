use crate::agent::template::AgentTemplate;
use crate::prompt::PromptBuilder;
use crate::prompt_ctx::section::SectionPriority;

/// Build the system prompt for the Lead Agent from agent templates.
///
/// Uses PromptBuilder for section management while preserving custom
/// lead-agent-specific sections via raw_system_block().
///
/// This is the preferred variant: the LLM sees template IDs and descriptions,
/// and can spawn multiple instances of the same template concurrently.
pub fn build_lead_agent_prompt_from_templates(
    base_persona: &str,
    templates: &[AgentTemplate],
) -> String {
    // Use a large default window — lead agent always gets generous context
    let builder = PromptBuilder::new(200_000);

    // Available agents catalog (High — important for delegation decisions)
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

    // Build and return just the system message string (callers expect String)
    let built = builder
        // Base persona (Critical — always included)
        .raw_system_block("lead_persona", base_persona, SectionPriority::Critical)
        // Role description (Critical)
        .raw_system_block(
            "lead_role",
            "<role>\n\
             You are a Lead Agent orchestrating a complex task. You are responsible for analyzing \
             the user's request, decomposing it into sub-objectives, delegating work to specialized \
             subagents, and synthesizing their results into a final response.\n\
             Do not attempt to perform specialized work (coding, research, analysis) yourself when \
             a suitable subagent is available. Your value is in orchestration and synthesis.\n\
             </role>",
            SectionPriority::Critical,
        )
        // Available agents catalog (High)
        .raw_system_block("agents_catalog", &agents_block, SectionPriority::High)
        // Workflow steps (High)
        .raw_system_block(
            "workflow",
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
             </workflow>",
            SectionPriority::High,
        )
        // Delegation criteria (Normal)
        .raw_system_block(
            "delegation_criteria",
            "<delegation-criteria>\n\
             Spawn subagents when:\n\
             - Tasks can run in parallel (e.g., research + implementation are independent)\n\
             - Tasks require isolated context or specialized skills\n\
             - Tasks involve independent workstreams that do not need shared state\n\n\
             Work directly (do NOT spawn) when:\n\
             - The task is simple enough to answer from your own knowledge\n\
             - You are synthesizing, summarizing, or formatting existing results\n\
             - The task requires maintaining context across sequential steps that one agent handles best\n\
             </delegation-criteria>",
            SectionPriority::Normal,
        )
        // Tool usage patterns (Normal)
        .raw_system_block(
            "tool_patterns",
            "<tools>\n\
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
             </tools>",
            SectionPriority::Normal,
        )
        // Failure recovery (Normal)
        .raw_system_block(
            "failure_recovery",
            "<failure-recovery>\n\
             If a subagent fails:\n\
             1. Read the error message to understand the failure type.\n\
             2. If the objective was too broad, split it into smaller sub-objectives and retry.\n\
             3. If the agent lacked the right skills, try a different agent.\n\
             4. If repeated failures occur, complete that sub-objective directly yourself.\n\
             5. Never silently drop a failed sub-objective — always report what succeeded and what did not.\n\
             </failure-recovery>",
            SectionPriority::Normal,
        )
        // Output expectations (Normal)
        .raw_system_block(
            "output_expectations",
            "<output>\n\
             Your final response must directly address the user's original request. \
             Synthesize all subagent results into a single coherent answer. \
             Do not simply list raw subagent outputs — integrate, summarize, and resolve any conflicts.\n\
             </output>",
            SectionPriority::Normal,
        )
        .build();

    built.system_message
}
