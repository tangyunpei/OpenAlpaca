//! Prompt construction for the LLM-based task planner.

use super::types::{PLANNING_HISTORY_LIMIT, PLANNING_SUMMARY_MAX_CHARS};
use crate::agent::subagent::SubAgent;
use openalpaca_llm::ChatMessage;
use regex::Regex;
use std::sync::OnceLock;

/// Render the `<agents>` prompt section into `out`.
pub(super) fn format_agent_list(out: &mut String, agents: &[SubAgent]) {
    out.push_str("<agents>\n");
    if agents.is_empty() {
        out.push_str("No agents are currently available.\n");
    } else {
        for agent in agents {
            let desc = agent.description.as_deref().unwrap_or("No description");
            let capabilities_str: Vec<String> = agent
                .capabilities
                .iter()
                .map(|s| format!("{} ({:.1})", s.name, s.proficiency))
                .collect();
            out.push_str(&format!(
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
    out.push_str("</agents>\n");
}

// ── Predictable structure detection ─────────────────────────────────

static NUMBERED_LIST_RE: OnceLock<Regex> = OnceLock::new();
static BULLET_LIST_RE: OnceLock<Regex> = OnceLock::new();
static BATCH_KEYWORD_RE: OnceLock<Regex> = OnceLock::new();
static EXPLICIT_QUANTITY_RE: OnceLock<Regex> = OnceLock::new();
static CJK_ENUM_RE: OnceLock<Regex> = OnceLock::new();
static CONJ_LIST_RE: OnceLock<Regex> = OnceLock::new();

fn numbered_list_regex() -> &'static Regex {
    NUMBERED_LIST_RE.get_or_init(|| Regex::new(r"\b\d+\.\s").unwrap())
}

fn bullet_list_regex() -> &'static Regex {
    BULLET_LIST_RE.get_or_init(|| Regex::new(r"(?m)^[\s]*[-*]\s").unwrap())
}

fn batch_keyword_regex() -> &'static Regex {
    BATCH_KEYWORD_RE
        .get_or_init(|| Regex::new(r"(?i)\b(each|all of|every|for each|respectively)\b").unwrap())
}

fn explicit_quantity_regex() -> &'static Regex {
    EXPLICIT_QUANTITY_RE.get_or_init(|| Regex::new(r"(?i)\b(into|to|in)\s+\d+\s").unwrap())
}

/// Detect if a user message contains predictable parallel structure.
pub(super) fn has_predictable_structure(content: &str) -> bool {
    if numbered_list_regex().find_iter(content).count() >= 2 {
        return true;
    }

    if bullet_list_regex().find_iter(content).count() >= 2 {
        return true;
    }

    if content.contains(',') && content.contains(" and ") && batch_keyword_regex().is_match(content)
    {
        return true;
    }

    if explicit_quantity_regex().is_match(content) {
        return true;
    }

    let cjk_enum = CJK_ENUM_RE
        .get_or_init(|| Regex::new(r"[\u4e00-\u9fff]+[、，][\u4e00-\u9fff]+[、，]").unwrap());
    if cjk_enum.is_match(content) {
        return true;
    }

    let conj_list =
        CONJ_LIST_RE.get_or_init(|| Regex::new(r"(?i)\w+,\s+\w+,\s+").unwrap());
    if conj_list.is_match(content) {
        return true;
    }

    false
}

/// Build the message list for a planning LLM call.
pub(super) fn build_messages(
    system_prompt: &str,
    user_message: &str,
    history: &[ChatMessage],
    session_summary: Option<&str>,
    active_tasks_block: Option<&str>,
) -> Vec<ChatMessage> {
    let history_tail = if history.len() > PLANNING_HISTORY_LIMIT {
        &history[history.len() - PLANNING_HISTORY_LIMIT..]
    } else {
        history
    };
    let mut messages = Vec::with_capacity(4 + history_tail.len());
    messages.push(ChatMessage::system(system_prompt));

    if let Some(summary) = session_summary {
        let capped: String = summary.chars().take(PLANNING_SUMMARY_MAX_CHARS).collect();
        messages.push(ChatMessage::user(
            &crate::orchestrator::wrap_untrusted_context(
                &format!("### SESSION SUMMARY ###\n{}", capped),
                "session_summary",
                "user_derived",
            ),
        ));
    }

    if let Some(tasks_block) = active_tasks_block {
        messages.push(ChatMessage::user(
            &crate::orchestrator::wrap_untrusted_context(
                tasks_block,
                "active_tasks",
                "user_derived",
            ),
        ));
    }

    messages.extend_from_slice(history_tail);
    messages.push(ChatMessage::user(user_message));
    messages
}

/// Build the hierarchical planning prompt with DAG support.
///
/// Exposed as `pub(crate)` so Layer 2 of the compose engine can call it from
/// `StaticPromptMode::PlannerHierarchical`. This helper is slated for deletion
/// in Phase 4 once the planner migration lands.
pub(crate) fn build_hierarchical_prompt(
    idle_agents: &[SubAgent],
    plan_protocol_v2: bool,
) -> String {
    let mut prompt = String::from(
        "You are a task planner for OpenAlpaca. Classify the user message and, \
         for complex tasks, decompose into a DAG of sub-tasks.\n\n",
    );

    format_agent_list(&mut prompt, idle_agents);

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

    prompt
}
