//! Layer 2 — Static Prompt.
//!
//! Assembles the cachable, tenant-stable part of the system prompt:
//! persona blocks + agent persona + skills + bootstrap + tools + connector
//! guidance + raw blocks. Three modes:
//!
//! - `Default` — wraps the existing `PromptBuilder` with all provided inputs.
//! - `PlannerHierarchical` — calls `task_planner::prompt::build_hierarchical_prompt`
//!   (existing helper; deletion deferred to Phase 4's Planner migration).
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
        // is the minimum slice of SubAgent that affects build_hierarchical_prompt's
        // output (see orchestrator/task_planner/prompt.rs:format_agent_list).
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

    // Ship Layer 1's blocks as raw system blocks. Layer 1 has already done the
    // persona transformation; re-entering PromptBuilder::system_persona would
    // duplicate work.
    for block in &input.persona_output.blocks {
        builder = builder.raw_system_block(block.name, &block.content, block.priority);
    }

    // Agent persona (Phase 6 migration sites populate this).
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
    if !input.tools.is_empty() {
        builder = builder.tools(&input.tools);
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

    if let Some(ref stc) = input.send_tool_context {
        builder = builder.raw_system_block("send_context", stc, SectionPriority::Normal);
    }
    if let Some(ref ms) = input.message_source {
        builder = builder.message_source(ms);
    }

    for block in &input.raw_blocks {
        builder = builder.raw_system_block(block.name, &block.content, block.priority);
    }

    let built = builder.build();

    StaticPromptOutput {
        system_message: Arc::<str>::from(built.system_message),
        section_registry: built.section_registry,
        fingerprint: compute_fingerprint(input),
    }
}

fn build_planner_hierarchical(input: &StaticPromptInput) -> StaticPromptOutput {
    // Absorbs the existing `build_hierarchical_prompt` (task_planner/prompt.rs)
    // without modification. Phase 4's Planner migration will delete the helper
    // once all callers route through compose.
    //
    // Implementation note: the real helper takes `&[SubAgent]` + a bool. Per
    // plan §2.5 "Known Tricky Parts", we widen StaticPromptInput with
    // optional `planner_agents` + `planner_protocol_v2` fields (backward
    // compatible — defaults are None/false). See types.rs.
    let empty_agents: Arc<Vec<super::types::AgentConfig>> = Arc::new(Vec::new());
    let agents = input
        .planner_agents
        .as_ref()
        .unwrap_or(&empty_agents);
    let system_message = crate::orchestrator::task_planner::build_hierarchical_prompt(
        agents,
        input.planner_protocol_v2,
    );

    StaticPromptOutput {
        system_message: Arc::<str>::from(system_message),
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
