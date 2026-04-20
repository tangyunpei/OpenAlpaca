//! Layer 2 — Static Prompt.
//!
//! Assembles the cachable, tenant-stable part of the system prompt:
//! persona blocks + agent persona + skills + bootstrap + tools + connector
//! guidance + raw blocks. Three modes:
//!
//! - `Default` — wraps the existing `PromptBuilder` with all provided inputs.
//! - `PlannerHierarchical` — emits the task-planner system prompt
//!   (preamble + `<agents>` block + `<instructions>` / `<examples>` /
//!   `<critical>` / `<format>` / `<rules>` + optional `<v2_protocol>` trailer).
//!   Absorbs the formerly separate `task_planner::prompt::build_hierarchical_prompt`
//!   helper (deleted in Phase 4 Commit 3).
//! - `SocialMinimal` — replicates `simple_query_handler.rs:512-526`'s inline
//!   `<system_instructions>` / `<agent_role>` format byte-for-byte.

use std::sync::Arc;

use super::fingerprint::{
    hash_connector_status, hash_opt_arc_str, hash_raw_blocks, hash_tool_set,
};
use super::types::{
    SectionPriority, StaticPromptInput, StaticPromptMode, StaticPromptOutput,
};

pub fn compute(input: &StaticPromptInput) -> StaticPromptOutput {
    match input.mode {
        StaticPromptMode::Default => build_default(input),
        StaticPromptMode::PlannerHierarchical => build_planner_hierarchical(input),
        StaticPromptMode::SocialMinimal => build_social_minimal(input),
        StaticPromptMode::ReplannerHierarchical => build_replanner_hierarchical(input),
    }
}

/// Public fingerprint helper used by `ComposeEngine::lookup_or_build_static_prompt`
/// to compute the cache key before the output is built.
pub(super) fn compute_fingerprint(input: &StaticPromptInput) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(&input.persona_output.fingerprint);
    h.update(&input.agent_config_fingerprint);
    h.update(&hash_tool_set(&input.tools));
    h.update(&hash_opt_arc_str(&input.skill_block));
    h.update(&hash_opt_arc_str(&input.skills_catalog));
    h.update(&hash_opt_arc_str(&input.bootstrap));
    h.update(&hash_connector_status(&input.connector_status));
    h.update(&hash_opt_arc_str(&input.send_tool_context));
    h.update(&hash_opt_arc_str(&input.message_source));
    h.update(&hash_raw_blocks(&input.raw_blocks));
    h.update(&[mode_tag(input.mode)]);
    h.update(&input.model_window.to_le_bytes());
    // AgentPersona: Debug-derive-based cheap canonical serializer. A future
    // phase may switch to bincode/serde_json for a stricter hash, but Debug
    // output is stable across a single deployment.
    if let Some(ref ap) = input.agent_persona {
        h.update(&[1u8]);
        let ap_bytes = format!("{:?}", ap).into_bytes();
        h.update(&(ap_bytes.len() as u64).to_le_bytes());
        h.update(&ap_bytes);
    } else {
        h.update(&[0u8]);
    }
    // Planner-mode inputs — included in the fingerprint so agent-list changes
    // bust the cache on PlannerHierarchical mode. Fingerprint remains stable
    // for non-planner modes because the fields are None/false by default.
    if let Some(ref agents) = input.planner_agents {
        h.update(&[1u8]);
        h.update(&(agents.len() as u64).to_le_bytes());
        // Hash each agent's id + name + description + capabilities list. This
        // is the minimum slice of SubAgent that affects the Planner system
        // prompt's `<agents>` block (see `build_planner_hierarchical` below).
        for agent in agents.iter() {
            h.update(&(agent.id.len() as u64).to_le_bytes());
            h.update(agent.id.as_bytes());
            h.update(&(agent.name.len() as u64).to_le_bytes());
            h.update(agent.name.as_bytes());
            let desc = agent.description.as_deref().unwrap_or("");
            h.update(&(desc.len() as u64).to_le_bytes());
            h.update(desc.as_bytes());
            h.update(&(agent.capabilities.len() as u64).to_le_bytes());
            for cap in &agent.capabilities {
                h.update(&(cap.name.len() as u64).to_le_bytes());
                h.update(cap.name.as_bytes());
                h.update(&cap.proficiency.to_le_bytes());
            }
        }
    } else {
        h.update(&[0u8]);
    }
    h.update(&[input.planner_protocol_v2 as u8]);
    h.finalize().into()
}

fn mode_tag(mode: StaticPromptMode) -> u8 {
    match mode {
        StaticPromptMode::Default => 0,
        StaticPromptMode::PlannerHierarchical => 1,
        StaticPromptMode::SocialMinimal => 2,
        StaticPromptMode::ReplannerHierarchical => 3,
    }
}

fn build_default(input: &StaticPromptInput) -> StaticPromptOutput {
    use crate::prompt::PromptBuilder;

    // PromptBuilder takes a usize window; our input uses u32.
    let mut builder = PromptBuilder::new(input.model_window as usize);

    // Ship Layer 1's blocks as raw system blocks. Layer 1 Default mode emits
    // the full `<system_instructions>` wrap so this block is byte-identical to
    // what `PromptBuilder::system_persona` would produce.
    for block in &input.persona_output.blocks {
        builder = builder.raw_system_block(block.name, &block.content, block.priority);
    }

    // Agent persona (migration sites populate this).
    if let Some(ref ap) = input.agent_persona {
        builder = builder.agent_persona(ap);
    }

    if let Some(ref b) = input.bootstrap {
        builder = builder.bootstrap(b);
    }
    if let Some(ref sc) = input.skills_catalog {
        builder = builder.skills_catalog(sc);
    }
    if let Some(ref sb) = input.skill_block {
        builder = builder.raw_system_block("skill_body", sb, SectionPriority::High);
    }

    // raw_blocks are emitted HERE — after skill_body, before message_source —
    // so the Skill migration can inject "skill_context" in its pre-migration
    // position (between skill_body and message_source; see
    // orchestrator/skill/invocation.rs pre-migration order).
    for block in &input.raw_blocks {
        builder = builder.raw_system_block(block.name, &block.content, block.priority);
    }

    // message_source BEFORE connector_guidance/tools to preserve byte-identical
    // order vs pre-migration Skill invocation (invocation.rs calls
    // `.message_source(...)` before tool resolution/`.tools(...)`).
    if let Some(ref ms) = input.message_source {
        builder = builder.message_source(ms);
    }

    // PromptBuilder's `connector_guidance` expects `&[(String, String)]` +
    // optional sendable list — adapt from our ConnectorSummary wrapper.
    if !input.connector_status.is_empty() {
        let statuses: Vec<(String, String)> = input
            .connector_status
            .iter()
            .map(|s| (s.id.clone(), s.status.clone()))
            .collect();
        let sendable: Vec<String> = input
            .connector_status
            .iter()
            .filter(|s| s.sendable)
            .map(|s| s.id.clone())
            .collect();
        let sendable_slice = if sendable.is_empty() {
            None
        } else {
            Some(sendable.as_slice())
        };
        builder = builder.connector_guidance(&statuses, sendable_slice);
    }

    if !input.tools.is_empty() {
        builder = builder.tools(&input.tools);
    }

    if let Some(ref stc) = input.send_tool_context {
        builder = builder.raw_system_block("send_context", stc, SectionPriority::Normal);
    }

    let built = builder.build();

    StaticPromptOutput {
        system_message: Arc::<str>::from(built.system_message),
        section_registry: built.section_registry,
        fingerprint: compute_fingerprint(input),
    }
}

fn build_planner_hierarchical(input: &StaticPromptInput) -> StaticPromptOutput {
    // Absorbs the formerly separate `task_planner::prompt::build_hierarchical_prompt`
    // (deleted in the same commit as this migration — Phase 4 Commit 3).
    //
    // `planner_agents` provides `&[SubAgent]` equivalent; `planner_protocol_v2`
    // maps to `plan_protocol_v2` in the old helper's signature.
    let empty_agents: Arc<Vec<super::types::AgentConfig>> = Arc::new(Vec::new());
    let agents = input.planner_agents.as_ref().unwrap_or(&empty_agents);
    let plan_protocol_v2 = input.planner_protocol_v2;

    let mut prompt = String::from(
        "You are a task planner for OpenAlpaca. Classify the user message and, \
         for complex tasks, decompose into a DAG of sub-tasks.\n\n",
    );

    // Inlined `format_agent_list` (previously at task_planner/prompt.rs:10-36).
    prompt.push_str("<agents>\n");
    if agents.is_empty() {
        prompt.push_str("No agents are currently available.\n");
    } else {
        for agent in agents.iter() {
            let desc = agent.description.as_deref().unwrap_or("No description");
            let capabilities_str: Vec<String> = agent
                .capabilities
                .iter()
                .map(|s| format!("{} ({:.1})", s.name, s.proficiency))
                .collect();
            prompt.push_str(&format!(
                "<agent id=\"{}\" name=\"{}\">\n{}\nCapabilities: {}\n</agent>\n",
                agent.id,
                agent.name,
                desc,
                if capabilities_str.is_empty() {
                    "none".to_string()
                } else {
                    capabilities_str.join(", ")
                }
            ));
        }
    }
    prompt.push_str("</agents>\n");

    // Verbatim paste of the old `build_hierarchical_prompt`'s main r#"..."# body
    // (task_planner/prompt.rs:156-244 pre-migration). DO NOT modify any
    // character including whitespace, quotes, or newlines.
    prompt.push_str(
        r#"
<instructions>
Classify the user's message into one of two categories:
- "simple_query": greetings, short questions, casual conversation, or anything answerable directly without agent work.
- "complex_task": multi-step tasks that require one or more agents to execute.

Think step-by-step before producing your JSON response:
1. Is this a simple greeting, question, or chat message? If yes, classify as "simple_query".
2. If it is a task, are all steps known upfront and predictable, or is it exploratory/dynamic?
3. Which available agents have the right skills for the task?
4. Write your reasoning into the "reasoning" field, then produce the JSON.

For complex tasks, choose exactly one execution strategy:
- Set "use_lead_agent": true when the task is genuinely exploratory, requires iterative refinement, or when the number of steps cannot be determined (e.g. debugging, open-ended research, creative exploration).
- Provide a "dag" with nodes when steps are enumerable upfront (even if partially dependent). Use DAG when multiple independent sub-tasks are visible in the user's message.
- Choose lead agent when the task is genuinely exploratory, adaptive, or requires iterative refinement. If the steps are clear, prefer DAG.

When choosing an execution strategy:
- lead_agent: Task is exploratory, adaptive, or requires iterative refinement.
- dag: 2+ steps known upfront; some steps can run in parallel.
- pipeline (assignments array): Steps are known upfront AND strictly sequential with no parallelism.
</instructions>

<examples>
Example 1 — Simple query:
User: "Hello, how are you?"
{"classification": "simple_query", "title": null, "assignments": [], "reasoning": "This is a greeting, not a task.", "dag": null, "use_lead_agent": false}

Example 2 — Complex task with lead agent (exploratory):
User: "Research the best caching strategy for our REST API and recommend one."
{"classification": "complex_task", "title": "Research API caching strategies", "assignments": [], "reasoning": "This is an open-ended research task. The user wants evaluation of options, which requires iterative exploration. Using lead agent.", "dag": null, "use_lead_agent": true}

Example 3 — Complex task with DAG (predictable steps):
User: "Translate this document into French, Spanish, and German."
{"classification": "complex_task", "title": "Translate document into 3 languages", "assignments": [], "reasoning": "All three translations are known upfront and independent. Using a DAG with parallel nodes.", "dag": {"nodes": [
  {"node_id": "node_1", "title": "Translate to French", "description": "Translate the document into French.", "agent_id": "translator-01", "agent_name": "Translator", "depends_on": [], "workspace_keys": [], "output_key": "french_translation"},
  {"node_id": "node_2", "title": "Translate to Spanish", "description": "Translate the document into Spanish.", "agent_id": "translator-01", "agent_name": "Translator", "depends_on": [], "workspace_keys": [], "output_key": "spanish_translation"},
  {"node_id": "node_3", "title": "Translate to German", "description": "Translate the document into German.", "agent_id": "translator-01", "agent_name": "Translator", "depends_on": [], "workspace_keys": [], "output_key": "german_translation"}
]}, "use_lead_agent": false}

Example 4 — Complex task with DAG (sequential dependencies):
User: "Read the report, summarize key findings, then send the summary to the team."
{"classification": "complex_task", "title": "Read, summarize, and send report", "assignments": [], "reasoning": "Three steps with sequential dependencies: read → summarize → send. Using DAG with dependency edges.", "dag": {"nodes": [
  {"node_id": "n1", "title": "Read report", "description": "Read and extract content from the report.", "agent_id": "general-agent-01", "agent_name": "General Agent", "depends_on": [], "workspace_keys": [], "output_key": "report_content"},
  {"node_id": "n2", "title": "Summarize findings", "description": "Summarize the key findings from the report.", "agent_id": "general-agent-01", "agent_name": "General Agent", "depends_on": ["n1"], "workspace_keys": ["report_content"], "output_key": "summary"},
  {"node_id": "n3", "title": "Send summary", "description": "Send the summary to the team.", "agent_id": "general-agent-01", "agent_name": "General Agent", "depends_on": ["n2"], "workspace_keys": ["summary"]}
]}, "use_lead_agent": false}

Example 5 — Sequential pipeline (strict linear dependency, no parallelism):
User: "Read the data file, analyze the trends, and write a report."
{"classification": "complex_task", "title": "Analyze data and write report", "assignments": [
  {"agent_id": "general-agent-01", "agent_name": "General Agent", "role_description": "Read and parse the data file", "matched_skills": ["file_read"]},
  {"agent_id": "general-agent-01", "agent_name": "General Agent", "role_description": "Analyze trends in the data", "matched_skills": ["analysis"]},
  {"agent_id": "general-agent-01", "agent_name": "General Agent", "role_description": "Write the final report", "matched_skills": ["text_generate"]}
], "reasoning": "Strict linear pipeline: each step depends on the previous. No parallelism opportunity.", "dag": null, "use_lead_agent": false}

</examples>

<critical>
IMPORTANT: Regardless of the language of the user's message, you MUST ALWAYS respond with
ONLY a valid JSON object. Never reply conversationally. Never respond in the user's language.
Your ENTIRE output must be a single JSON object starting with '{' and ending with '}'.
</critical>

<format>
Respond with ONLY a single JSON object. No markdown fences, no explanation, no other text.

JSON schema:
{"classification": "simple_query" | "complex_task", "title": string | null, "assignments": [], "reasoning": "...", "dag": null | {"nodes": [...]}, "use_lead_agent": boolean}

When "classification" is "complex_task", you MUST provide exactly one execution path:
1. "use_lead_agent": true (with "dag": null) — for exploratory or dynamic tasks
2. "dag" with 2-8 nodes (with "use_lead_agent": false) — for fully predictable tasks
Do NOT set both "use_lead_agent": true and "dag" simultaneously.
Returning "complex_task" with no DAG and use_lead_agent=false is INVALID.
</format>

<rules>
DAG construction rules:
- Each node is a sub-task assigned to one agent (use exact agent_id values from the agents list)
- "depends_on": list of node_ids that must complete before this node starts
- Nodes with no shared dependencies run in parallel — express parallelism for independent tasks
- "workspace_keys": workspace entries this node reads (from other nodes' output_key)
- "output_key": workspace key where this node writes its result
- 2-8 nodes maximum
- Decompose into distinct stages that require different skills
</rules>
"#,
    );

    // Optional v2_protocol trailer (task_planner/prompt.rs:247-263 pre-migration).
    if plan_protocol_v2 {
        prompt.push_str(
            r#"

<v2_protocol>
Additional optional fields (v2 protocol):
- "execution_mode": "lead_agent" | "dag" | "pipeline" — explicit execution path.
  When set, this takes priority over use_lead_agent/dag inference.
- "predictability_score": 0.0-1.0 — your confidence that all task steps are known upfront.
  0.0 = fully exploratory, 1.0 = fully predictable.

When you include "execution_mode", you SHOULD also set "predictability_score".
Example:
{"classification": "complex_task", "title": "Batch process items", "assignments": [], "reasoning": "...", "dag": {...}, "use_lead_agent": false, "execution_mode": "dag", "predictability_score": 0.9}
</v2_protocol>
"#,
        );
    }

    StaticPromptOutput {
        system_message: Arc::<str>::from(prompt),
        section_registry: vec!["planner_hierarchical"],
        fingerprint: compute_fingerprint(input),
    }
}

fn build_replanner_hierarchical(input: &StaticPromptInput) -> StaticPromptOutput {
    // Absorbs orchestrator/replanner/mod.rs:build_replan_prompt's format.
    //
    // Loader contract: `raw_blocks` arrives pre-serialized by the caller in
    // this order with these `name` values:
    //   "original_objective", "dag_state", "workspace" (optional), "context"
    // Each block's content is already wrapped in its own XML tags and ends
    // with "\n\n".
    //
    // The <available_agents> block comes from `planner_agents` (reused from
    // PlannerHierarchical mode for cache-fingerprint meaningfulness).
    //
    // The static preamble and static response_format/rules block are emitted
    // verbatim from this function — never from raw_blocks.
    //
    // Pre-migration emission order (from the deleted `build_replan_prompt`):
    //   preamble, objective, dag_state, workspace (optional),
    //   <available_agents>, <context>, <response_format>+<rules>.
    // We preserve that order by splitting the raw_blocks iteration: first pass
    // emits every block EXCEPT the one named "context", then <available_agents>,
    // then the "context" block last.

    let mut prompt = String::from(
        "You are a task replanner for OpenAlpaca. Evaluate whether the current \
         execution plan is still on track or needs modification.\n\n",
    );

    // Caller-provided blocks in loader-defined order — EXCEPT the "context"
    // block, which pre-migration emitted AFTER <available_agents>. Rendering
    // it here preserves byte-identical order vs the pre-migration
    // `build_replan_prompt`.
    for block in input.raw_blocks.iter().filter(|b| b.name != "context") {
        prompt.push_str(&block.content);
    }

    // Available agents (reuses planner_agents for cache meaningfulness).
    prompt.push_str("<available_agents>\n");
    let empty_agents: Arc<Vec<super::types::AgentConfig>> = Arc::new(Vec::new());
    let agents = input.planner_agents.as_ref().unwrap_or(&empty_agents);
    if agents.is_empty() {
        prompt.push_str("No agents are currently available.\n");
    } else {
        for agent in agents.iter() {
            let desc = agent.description.as_deref().unwrap_or("No description");
            prompt.push_str(&format!(
                "- ID: \"{}\", Name: \"{}\", Description: \"{}\"\n",
                agent.id, agent.name, desc
            ));
        }
    }
    prompt.push_str("</available_agents>\n\n");

    // Context block last — pre-migration emitted this AFTER <available_agents>.
    if let Some(ctx_block) = input.raw_blocks.iter().find(|b| b.name == "context") {
        prompt.push_str(&ctx_block.content);
    }

    // Static response_format + rules (absorbed from build_replan_prompt
    // orchestrator/replanner/mod.rs lines 184-210).
    prompt.push_str(
        r#"<response_format>
Respond with ONLY a single JSON object. No markdown, no explanation, no other text.

If the plan is on track:
{"decision": "continue"}

If the plan needs modification (replace remaining PENDING/READY nodes with new nodes):
{"decision": "modify_dag", "dag": {"nodes": [
  {"node_id": "new_1", "title": "...", "description": "...", "agent_id": "...", "agent_name": "...", "depends_on": [], "workspace_keys": [], "output_key": "..."},
  ...
]}}

If the task should be abandoned:
{"decision": "abort", "reason": "Explanation of why the task cannot be completed"}
</response_format>

<rules>
- Prefer "continue" unless completed results clearly show the remaining plan is wrong
- A "modify_dag" replaces only PENDING/READY/SKIPPED nodes; COMPLETED/RUNNING nodes are kept
- New nodes in modify_dag can reference output_keys from already-completed nodes
- Use exact agent_id values from the Available Agents list
- 2-8 nodes max in modified DAG
- Only abort if the task is fundamentally impossible given completed results
</rules>
"#,
    );

    StaticPromptOutput {
        system_message: Arc::<str>::from(prompt),
        section_registry: vec!["replanner_hierarchical"],
        fingerprint: compute_fingerprint(input),
    }
}

fn build_social_minimal(input: &StaticPromptInput) -> StaticPromptOutput {
    // Replicates crates/openalpaca_core/src/orchestrator/query_handler/
    // simple_query_handler.rs:512-526 verbatim.
    //
    //     let system_prompt = format!(
    //         "<system_instructions>\n{}\n</system_instructions>\n\n\
    //          <agent_role>\nRole: Assistant\nTone: Concise and professional\n</agent_role>",
    //         system_persona.base_instructions
    //     );
    //
    // Layer 1 in Minimal mode already placed system_persona.base_instructions
    // (verbatim) as the content of the first block, so we pull from there.
    let persona_text = input
        .persona_output
        .blocks
        .first()
        .map(|b| b.content.as_ref())
        .unwrap_or("");

    let system_message = format!(
        "<system_instructions>\n{}\n</system_instructions>\n\n\
         <agent_role>\nRole: Assistant\nTone: Concise and professional\n</agent_role>",
        persona_text
    );

    StaticPromptOutput {
        system_message: Arc::<str>::from(system_message),
        section_registry: vec!["social_minimal"],
        fingerprint: compute_fingerprint(input),
    }
}
