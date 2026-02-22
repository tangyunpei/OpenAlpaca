//! LLM-based task planning: replaces keyword heuristics with a single LLM call
//! that classifies intent, generates a title, and assigns agents.
//!
//! Supports hierarchical planning: for complex tasks, the planner can decompose
//! the objective into a DAG of sub-tasks with dependencies.

use crate::agent::subagent::SubAgent;
use openalpaca_llm::{ChatMessage, LlmRouter, RequestContext, RouterRequest};
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// Maximum number of recent history messages to include in planning prompts.
const PLANNING_HISTORY_LIMIT: usize = 6;
/// Maximum character length for session summary in planning prompts.
const PLANNING_SUMMARY_MAX_CHARS: usize = 500;

// ── DAG types ────────────────────────────────────────────────────────

/// Status of a node in the task DAG.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DagNodeStatus {
    #[default]
    Pending,
    Ready,
    Running,
    Completed,
    Failed,
    Skipped,
}

/// A node in the task DAG — represents one sub-task.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagNode {
    pub node_id: String,
    pub title: String,
    pub description: String,
    pub agent_id: String,
    pub agent_name: String,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub status: DagNodeStatus,
    pub result_summary: Option<String>,
    #[serde(default)]
    pub workspace_keys: Vec<String>,
    pub output_key: Option<String>,
}

/// A directed acyclic graph of sub-tasks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskDag {
    pub nodes: Vec<DagNode>,
}

impl TaskDag {
    /// Run Kahn's algorithm on the DAG.
    /// Returns `(visited_count, topological_order)`.
    fn run_kahns(&self) -> (usize, Vec<String>) {
        let mut in_degree: HashMap<&str, usize> = HashMap::new();
        let mut adj: HashMap<&str, Vec<&str>> = HashMap::new();
        for node in &self.nodes {
            in_degree.entry(node.node_id.as_str()).or_insert(0);
            adj.entry(node.node_id.as_str()).or_default();
            for dep in &node.depends_on {
                *in_degree.entry(node.node_id.as_str()).or_insert(0) += 1;
                adj.entry(dep.as_str())
                    .or_default()
                    .push(node.node_id.as_str());
            }
        }

        let mut queue: VecDeque<&str> = in_degree
            .iter()
            .filter(|&(_, &deg)| deg == 0)
            .map(|(&id, _)| id)
            .collect();
        let mut order = Vec::new();

        while let Some(id) = queue.pop_front() {
            order.push(id.to_string());
            if let Some(neighbors) = adj.get(id) {
                for &neighbor in neighbors {
                    if let Some(deg) = in_degree.get_mut(neighbor) {
                        *deg -= 1;
                        if *deg == 0 {
                            queue.push_back(neighbor);
                        }
                    }
                }
            }
        }

        (order.len(), order)
    }

    /// Validate the DAG: no cycles, all dependencies exist, all agents available.
    pub fn validate(&self, available_agents: &[SubAgent]) -> Result<(), String> {
        let node_ids: HashSet<&str> = self.nodes.iter().map(|n| n.node_id.as_str()).collect();
        let agent_ids: HashSet<&str> = available_agents.iter().map(|a| a.id.as_str()).collect();

        if self.nodes.is_empty() {
            return Err("DAG has no nodes".to_string());
        }

        if self.nodes.len() < 2 {
            return Err(format!(
                "DAG requires at least 2 nodes (got {}); use lead agent for single-step tasks",
                self.nodes.len()
            ));
        }

        if self.nodes.len() > 8 {
            return Err(format!("DAG has {} nodes (max 8)", self.nodes.len()));
        }

        // Check all dependencies exist and all agents are available
        for node in &self.nodes {
            for dep in &node.depends_on {
                if !node_ids.contains(dep.as_str()) {
                    return Err(format!(
                        "Node '{}' depends on unknown node '{}'",
                        node.node_id, dep
                    ));
                }
            }
            if !agent_ids.contains(node.agent_id.as_str()) {
                return Err(format!(
                    "Node '{}' references unknown agent '{}'",
                    node.node_id, node.agent_id
                ));
            }
        }

        // Cycle detection via Kahn's algorithm
        let (visited, _) = self.run_kahns();
        if visited != self.nodes.len() {
            return Err("DAG contains a cycle".to_string());
        }

        Ok(())
    }

    /// Get nodes that are ready to run (all deps completed, status is Pending or Ready).
    pub fn ready_nodes(&self) -> Vec<&DagNode> {
        let completed: HashSet<&str> = self
            .nodes
            .iter()
            .filter(|n| n.status == DagNodeStatus::Completed)
            .map(|n| n.node_id.as_str())
            .collect();

        self.nodes
            .iter()
            .filter(|n| {
                matches!(n.status, DagNodeStatus::Pending | DagNodeStatus::Ready)
                    && n.depends_on
                        .iter()
                        .all(|dep| completed.contains(dep.as_str()))
            })
            .collect()
    }

    /// Mark a node as completed and return node_ids of newly-ready nodes.
    pub fn complete_node(&mut self, node_id: &str, summary: &str) -> Vec<String> {
        if let Some(node) = self.nodes.iter_mut().find(|n| n.node_id == node_id) {
            node.status = DagNodeStatus::Completed;
            node.result_summary = Some(summary.chars().take(500).collect());
        }
        // Return newly-ready nodes
        self.ready_nodes()
            .iter()
            .map(|n| n.node_id.clone())
            .collect()
    }

    /// Mark a node as failed with an error message.
    pub fn fail_node(&mut self, node_id: &str, error: &str) {
        if let Some(node) = self.nodes.iter_mut().find(|n| n.node_id == node_id) {
            node.status = DagNodeStatus::Failed;
            node.result_summary = Some(error.chars().take(500).collect());
        }
        // Skip all downstream dependents
        self.skip_dependents(node_id);
    }

    /// Iteratively skip nodes that depend (transitively) on a failed node.
    fn skip_dependents(&mut self, failed_id: &str) {
        let mut queue = VecDeque::new();
        let mut visited = HashSet::new();
        queue.push_back(failed_id.to_string());
        visited.insert(failed_id.to_string());

        while let Some(current_id) = queue.pop_front() {
            let dependents: Vec<String> = self
                .nodes
                .iter()
                .filter(|n| n.depends_on.contains(&current_id))
                .map(|n| n.node_id.clone())
                .collect();

            for dep_id in dependents {
                if visited.insert(dep_id.clone()) {
                    if let Some(node) = self.nodes.iter_mut().find(|n| n.node_id == dep_id)
                        && matches!(node.status, DagNodeStatus::Pending | DagNodeStatus::Ready)
                    {
                        node.status = DagNodeStatus::Skipped;
                    }
                    queue.push_back(dep_id);
                }
            }
        }
    }

    /// Check if the entire DAG is finished (all nodes completed, failed, or skipped).
    pub fn is_finished(&self) -> bool {
        self.nodes.iter().all(|n| {
            matches!(
                n.status,
                DagNodeStatus::Completed | DagNodeStatus::Failed | DagNodeStatus::Skipped
            )
        })
    }

    /// Topological sort — returns node_ids in valid execution order.
    pub fn topological_order(&self) -> Vec<String> {
        let (_, order) = self.run_kahns();
        order
    }

    /// Mark a node as running.
    pub fn mark_running(&mut self, node_id: &str) {
        if let Some(node) = self.nodes.iter_mut().find(|n| n.node_id == node_id) {
            node.status = DagNodeStatus::Running;
        }
    }

    /// Validate DAG structure only (dependencies exist, no cycles).
    /// Unlike `validate()`, does NOT check agent availability or node count limits.
    /// Used by `merge_replanned_dag` where agent checks are done separately.
    pub fn validate_structure(&self) -> Result<(), String> {
        if self.nodes.is_empty() {
            return Err("DAG has no nodes".to_string());
        }

        if self.nodes.len() < 2 {
            return Err("DAG requires at least 2 nodes".to_string());
        }

        let node_ids: HashSet<&str> = self.nodes.iter().map(|n| n.node_id.as_str()).collect();
        for node in &self.nodes {
            for dep in &node.depends_on {
                if !node_ids.contains(dep.as_str()) {
                    return Err(format!(
                        "Node '{}' depends on unknown node '{}'",
                        node.node_id, dep
                    ));
                }
            }
        }

        // Cycle detection via Kahn's algorithm
        let (visited, _) = self.run_kahns();
        if visited != self.nodes.len() {
            return Err("DAG contains a cycle".to_string());
        }

        Ok(())
    }

    /// Count completed nodes.
    pub fn completed_count(&self) -> usize {
        self.nodes
            .iter()
            .filter(|n| n.status == DagNodeStatus::Completed)
            .count()
    }

    /// Compute critical path length (hops to furthest descendant) per node.
    ///
    /// Nodes with no dependents have length 0. Uses reverse topological order
    /// so each node's length = max(length[child] + 1) for all children.
    /// Nodes on the critical path have the highest values and should be
    /// prioritized for scheduling to minimize overall DAG completion time.
    pub fn critical_path_lengths(&self) -> HashMap<String, usize> {
        // Build forward adjacency: node -> vec of nodes that depend on it
        let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();
        for node in &self.nodes {
            dependents.entry(node.node_id.as_str()).or_default();
            for dep in &node.depends_on {
                dependents
                    .entry(dep.as_str())
                    .or_default()
                    .push(node.node_id.as_str());
            }
        }

        // Get topological order, then reverse it so leaf nodes come first
        let (_, topo) = self.run_kahns();
        let mut lengths: HashMap<String, usize> = HashMap::new();

        // Process in reverse topological order (leaves first)
        for node_id in topo.iter().rev() {
            let max_child = dependents
                .get(node_id.as_str())
                .map(|children| {
                    children
                        .iter()
                        .filter_map(|c| lengths.get(*c).map(|l| l + 1))
                        .max()
                        .unwrap_or(0)
                })
                .unwrap_or(0);
            lengths.insert(node_id.clone(), max_child);
        }

        lengths
    }

    /// Get ready nodes sorted by critical path length (descending).
    ///
    /// Nodes with longer downstream paths are returned first, ensuring the
    /// critical path is prioritized when concurrency slots are limited.
    pub fn ready_nodes_prioritized(&self) -> Vec<&DagNode> {
        let lengths = self.critical_path_lengths();
        let mut ready = self.ready_nodes();
        ready.sort_by(|a, b| {
            let la = lengths.get(&a.node_id).copied().unwrap_or(0);
            let lb = lengths.get(&b.node_id).copied().unwrap_or(0);
            lb.cmp(&la)
        });
        ready
    }
}

// ── Plan types ───────────────────────────────────────────────────────

/// The result of an LLM planning call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskPlan {
    pub classification: String,
    pub title: Option<String>,
    pub assignments: Vec<PlannedAssignment>,
    pub reasoning: Option<String>,
    #[serde(default)]
    pub dag: Option<TaskDag>,
    /// When true, the task should be executed by a Lead Agent (full agentic loop
    /// with dynamic subagent spawning) instead of a static DAG or sequential pipeline.
    /// Defaults to `true` (lead agent is the safer fallback for uncertain tasks).
    #[serde(default = "default_use_lead_agent")]
    pub use_lead_agent: bool,
    /// Tracks why auto-promotion to lead agent occurred (observability).
    /// Set by parse_response() or plan_inner() when a safety net triggers.
    #[serde(skip)]
    pub auto_promotion_reason: Option<String>,
    /// V2 protocol: explicit execution mode from planner.
    /// When present, takes precedence over use_lead_agent/dag heuristic.
    /// Values: "lead_agent" | "dag" | "pipeline"
    #[serde(default)]
    pub execution_mode: Option<String>,
    /// V2 protocol: planner's confidence that the task has predictable structure (0.0-1.0).
    /// Higher values indicate the planner believes all steps are known upfront.
    #[serde(default)]
    pub predictability_score: Option<f64>,
}

/// Serde default for `use_lead_agent`: returns `true` so that missing
/// or omitted `use_lead_agent` fields default to lead agent mode (the safer option).
fn default_use_lead_agent() -> bool {
    true
}

/// An agent assignment decided by the LLM planner.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlannedAssignment {
    pub agent_id: String,
    pub agent_name: String,
    pub role_description: String,
    pub matched_skills: Vec<String>,
}

/// Errors from the task planner.
#[derive(Debug)]
pub enum PlanError {
    MalformedResponse(String),
    LlmError(String),
    /// The LLM call did not complete within the configured timeout.
    Timeout(u64),
}

impl std::fmt::Display for PlanError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlanError::MalformedResponse(msg) => write!(f, "Malformed response: {}", msg),
            PlanError::LlmError(msg) => write!(f, "LLM error: {}", msg),
            PlanError::Timeout(secs) => write!(f, "Planning timed out after {}s", secs),
        }
    }
}

// ── JSON extraction ─────────────────────────────────────────────────

/// Extract a JSON block from LLM output that may contain surrounding prose.
///
/// Handles (in order):
/// 1. Markdown ` ```json ... ``` ` fences
/// 2. Markdown ` ``` ... ``` ` fences
/// 3. Brace-matching fallback: outermost `{ ... }` respecting string literals
/// 4. Returns trimmed input unchanged if nothing matches
pub(crate) fn extract_json_block(content: &str) -> &str {
    let trimmed = content.trim();

    // Try ```json ... ``` first
    if let Some(start) = trimmed.find("```json") {
        let after_fence = &trimmed[start + 7..];
        if let Some(end) = after_fence.find("```") {
            return after_fence[..end].trim();
        }
    }

    // Try ``` ... ```
    if let Some(start) = trimmed.find("```") {
        let after_fence = &trimmed[start + 3..];
        if let Some(end) = after_fence.find("```") {
            return after_fence[..end].trim();
        }
    }

    // Brace-matching fallback: find outermost { ... }
    if let Some(json_slice) = find_outermost_braces(trimmed) {
        return json_slice;
    }

    trimmed
}

/// Find the outermost `{ ... }` in the string, respecting JSON string literals.
/// Returns the slice from the first `{` to the matching `}` (inclusive).
fn find_outermost_braces(s: &str) -> Option<&str> {
    let bytes = s.as_bytes();
    let mut start = None;
    let mut depth: i32 = 0;
    let mut in_string = false;
    let mut escape_next = false;

    for (i, &b) in bytes.iter().enumerate() {
        if escape_next {
            escape_next = false;
            continue;
        }
        if in_string {
            if b == b'\\' {
                escape_next = true;
            } else if b == b'"' {
                in_string = false;
            }
            continue;
        }
        match b {
            b'"' => in_string = true,
            b'{' => {
                if depth == 0 {
                    start = Some(i);
                }
                depth += 1;
            }
            b'}' => {
                depth -= 1;
                if depth == 0
                    && let Some(s_idx) = start
                {
                    return Some(&s[s_idx..=i]);
                }
            }
            _ => {}
        }
    }
    None
}

// ── Planning helpers ─────────────────────────────────────────────────

/// Render the `<agents>` prompt section into `out`.
fn format_agent_list(out: &mut String, agents: &[SubAgent]) {
    out.push_str("<agents>\n");
    if agents.is_empty() {
        out.push_str("No agents are currently available.\n");
    } else {
        for agent in agents {
            let desc = agent.description.as_deref().unwrap_or("No description");
            let skills_str: Vec<String> = agent
                .skills
                .iter()
                .map(|s| format!("{} ({:.1})", s.name, s.proficiency))
                .collect();
            out.push_str(&format!(
                "<agent id=\"{}\" name=\"{}\">\n{}\nSkills: {}\n</agent>\n",
                agent.id,
                agent.name,
                desc,
                if skills_str.is_empty() {
                    "none".to_string()
                } else {
                    skills_str.join(", ")
                }
            ));
        }
    }
    out.push_str("</agents>\n");
}

/// Build the message list for a planning LLM call.
///
/// Constructs: `[system_prompt, optional summary, optional active_tasks, history_tail…, user_message]`
static NUMBERED_LIST_RE: OnceLock<Regex> = OnceLock::new();
static BULLET_LIST_RE: OnceLock<Regex> = OnceLock::new();
static BATCH_KEYWORD_RE: OnceLock<Regex> = OnceLock::new();
static EXPLICIT_QUANTITY_RE: OnceLock<Regex> = OnceLock::new();

fn numbered_list_regex() -> &'static Regex {
    NUMBERED_LIST_RE.get_or_init(|| Regex::new(r"\b\d+\.\s").unwrap())
}

fn bullet_list_regex() -> &'static Regex {
    BULLET_LIST_RE.get_or_init(|| Regex::new(r"(?m)^[\s]*[-*]\s").unwrap())
}

fn batch_keyword_regex() -> &'static Regex {
    BATCH_KEYWORD_RE.get_or_init(|| {
        Regex::new(r"(?i)\b(each|all of|every|for each|respectively)\b").unwrap()
    })
}

fn explicit_quantity_regex() -> &'static Regex {
    EXPLICIT_QUANTITY_RE.get_or_init(|| Regex::new(r"(?i)\b(into|to|in)\s+\d+\s").unwrap())
}

/// Detect if a user message contains predictable parallel structure
/// (numbered lists, bullet lists, batch keywords, or explicit quantities).
fn has_predictable_structure(content: &str) -> bool {
    // Numbered list: "1. ...", "2. ..." etc. — at least 2 occurrences
    if numbered_list_regex().find_iter(content).count() >= 2 {
        return true;
    }

    // Bullet list: lines starting with "- " or "* " — at least 2 occurrences
    if bullet_list_regex().find_iter(content).count() >= 2 {
        return true;
    }

    // Comma + "and" + batch keyword: "translate into French, Spanish, and German for each"
    if content.contains(',') && content.contains(" and ") && batch_keyword_regex().is_match(content)
    {
        return true;
    }

    // Explicit quantity: "into 3 languages", "to 5 files"
    if explicit_quantity_regex().is_match(content) {
        return true;
    }

    false
}

fn build_messages(
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
            &super::wrap_untrusted_context(&format!("### SESSION SUMMARY ###\n{}", capped), "session_summary", "user_derived"),
        ));
    }

    if let Some(tasks_block) = active_tasks_block {
        messages.push(ChatMessage::user(
            &super::wrap_untrusted_context(tasks_block, "active_tasks", "user_derived"),
        ));
    }

    messages.extend_from_slice(history_tail);
    messages.push(ChatMessage::user(user_message));
    messages
}

/// Shared retry loop for hierarchical planning.
///
/// Builds a `RouterRequest` per attempt, calls the router with a timeout,
/// parses the response, and validates the DAG against available agents.
async fn plan_inner(
    router: &LlmRouter,
    messages: Vec<ChatMessage>,
    limits: PlannerLimits,
    idle_agents: &[SubAgent],
) -> Result<TaskPlan, PlanError> {
    let mut last_error = PlanError::MalformedResponse("no attempts made".to_string());
    let deadline = Duration::from_secs(limits.timeout_secs);

    for attempt in 0..=limits.max_retries {
        let request = RouterRequest {
            model: None,
            messages: messages.clone(),
            tools: Arc::new(vec![]),
            temperature: Some(0.0),
            max_tokens: Some(limits.max_tokens),
            context: RequestContext::default(),
        };

        let response = tokio::time::timeout(deadline, router.complete(request))
            .await
            .map_err(|_| PlanError::Timeout(limits.timeout_secs))?
            .map_err(|e| PlanError::LlmError(e.to_string()))?;

        let response_content = response.content.clone();

        match TaskPlanner::parse_response(&response.content) {
            Ok(plan) => {
                if let Some(ref dag) = plan.dag
                    && let Err(e) = dag.validate(idle_agents)
                {
                    let promoted = plan.assignments.is_empty() && !plan.use_lead_agent;
                    if promoted {
                        tracing::warn!(
                            classification = %plan.classification,
                            reasoning = ?plan.reasoning,
                            dag_error = %e,
                            "Auto-promoting to lead agent: DAG validation failed and plan \
                             has no flat assignments. Original use_lead_agent=false."
                        );
                    } else {
                        tracing::warn!(
                            classification = %plan.classification,
                            reasoning = ?plan.reasoning,
                            dag_error = %e,
                            "DAG validation failed, falling back to flat assignments"
                        );
                    }
                    return Ok(TaskPlan {
                        dag: None,
                        use_lead_agent: plan.use_lead_agent || promoted,
                        auto_promotion_reason: Some("dag_validation_failed".into()),
                        ..plan
                    });
                }
                return Ok(plan);
            }
            Err(PlanError::MalformedResponse(msg)) => {
                tracing::warn!(
                    "Hierarchical plan attempt {}/{} returned malformed response: {msg}",
                    attempt + 1,
                    limits.max_retries + 1,
                );
                last_error = PlanError::MalformedResponse(msg);
                // Fail fast: if response has no JSON structure at all (e.g. conversational
                // text in user's language), retrying with the same prompt won't help
                if !response_content.contains('{') {
                    tracing::warn!(
                        "Response contains no JSON structure, skipping remaining retries"
                    );
                    break;
                }
            }
            Err(e) => return Err(e),
        }
    }

    Err(last_error)
}

// ── Planner ─────────────────────────────────────────────────────────

/// Runtime limits for a planning call (timeout + retry budget).
///
/// Constructed from `PlannerConfig` in the daemon configuration.
#[derive(Debug, Clone, Copy)]
pub struct PlannerLimits {
    pub timeout_secs: u64,
    pub max_retries: usize,
    pub max_tokens: u32,
    /// When true, include execution_mode and predictability_score in the planner prompt.
    pub plan_protocol_v2_enabled: bool,
}

pub struct TaskPlanner;

impl TaskPlanner {
    /// Hierarchical planning: decompose a complex task into a DAG of sub-tasks.
    /// Falls back to flat assignment if DAG planning fails or returns simple_query.
    ///
    /// Retries up to `limits.max_retries` times on malformed responses. DAG validation
    /// failures do NOT trigger retries — the plan is returned with `dag: None`
    /// (existing fallback behaviour). Timeout and LLM errors are returned immediately.
    #[allow(clippy::too_many_arguments)]
    pub async fn plan_hierarchical(
        router: &LlmRouter,
        user_message: &str,
        idle_agents: &[SubAgent],
        history: &[ChatMessage],
        session_summary: Option<&str>,
        active_tasks_block: Option<&str>,
        limits: PlannerLimits,
        dag_prefer_predictable: bool,
    ) -> Result<TaskPlan, PlanError> {
        let system_prompt = Self::build_hierarchical_prompt(idle_agents, limits.plan_protocol_v2_enabled);
        let mut messages = build_messages(
            &system_prompt,
            user_message,
            history,
            session_summary,
            active_tasks_block,
        );

        // If enabled, inject a system hint before the final user message
        // when the message contains predictable parallel structure.
        if dag_prefer_predictable && has_predictable_structure(user_message) {
            let hint = ChatMessage::system(
                "[SYSTEM HINT: This message contains enumerated or parallel sub-tasks. \
                 Prefer a DAG with parallel nodes if all steps are known upfront. \
                 Set use_lead_agent to false when using DAG.]",
            );
            // Insert before the last message (the user message)
            let last_idx = messages.len().saturating_sub(1);
            messages.insert(last_idx, hint);
        }

        plan_inner(router, messages, limits, idle_agents).await
    }

    /// Build the hierarchical planning prompt with DAG support.
    fn build_hierarchical_prompt(idle_agents: &[SubAgent], plan_protocol_v2: bool) -> String {
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
- Set "use_lead_agent": true when the task is open-ended, exploratory, or adaptive (PREFERRED default). Use this when the number of sub-tasks is unknown, results from one step change what comes next, or the task requires iterative refinement (e.g. debugging, research, creative exploration).
- Provide a "dag" with nodes when ALL steps are known upfront and predictable (e.g. translating into N languages, a fixed pipeline of read-then-summarize-then-send, or batch-processing independent items).
- If unsure, default to "use_lead_agent": true. A lead agent can always execute a simple plan, but a DAG cannot adapt if the plan is wrong.
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

    /// Parse the LLM response into a TaskPlan.
    fn parse_response(content: &str) -> Result<TaskPlan, PlanError> {
        let json_str = Self::extract_json(content);

        // Primary: direct parse
        let plan = if let Ok(plan) = serde_json::from_str::<TaskPlan>(json_str) {
            plan
        } else if let Ok(obj) = serde_json::from_str::<serde_json::Value>(json_str) {
            // Fallback: LLM may have wrapped the plan in a parent object
            // (e.g. {"available_agents": ..., "classification": ...})
            if let Some(classification) = obj.get("classification").and_then(|v| v.as_str()) {
                TaskPlan {
                    classification: classification.to_string(),
                    title: obj.get("title").and_then(|v| v.as_str()).map(String::from),
                    assignments: obj
                        .get("assignments")
                        .and_then(|v| serde_json::from_value(v.clone()).ok())
                        .unwrap_or_default(),
                    reasoning: obj
                        .get("reasoning")
                        .and_then(|v| v.as_str())
                        .map(String::from),
                    dag: obj
                        .get("dag")
                        .and_then(|v| serde_json::from_value(v.clone()).ok()),
                    use_lead_agent: obj
                        .get("use_lead_agent")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(true),
                    auto_promotion_reason: None,
                    execution_mode: obj
                        .get("execution_mode")
                        .and_then(|v| v.as_str())
                        .map(String::from),
                    predictability_score: obj
                        .get("predictability_score")
                        .and_then(|v| v.as_f64()),
                }
            } else {
                return Err(PlanError::MalformedResponse(format!(
                    "Failed to parse JSON: missing field `classification` (input: {})",
                    &content.chars().take(200).collect::<String>()
                )));
            }
        } else {
            return Err(PlanError::MalformedResponse(format!(
                "Failed to parse JSON: missing field `classification` (input: {})",
                &content.chars().take(200).collect::<String>()
            )));
        };

        // V2 protocol: when execution_mode is present, use it to resolve the
        // execution path authoritatively (overrides use_lead_agent/dag heuristics).
        if let Some(ref mode) = plan.execution_mode {
            match mode.as_str() {
                "lead_agent" => {
                    return Ok(TaskPlan {
                        use_lead_agent: true,
                        dag: None,
                        ..plan
                    });
                }
                "dag" => {
                    if plan.dag.is_some() {
                        return Ok(TaskPlan {
                            use_lead_agent: false,
                            ..plan
                        });
                    }
                    // execution_mode says "dag" but no DAG provided — fall through to heuristics
                    tracing::warn!(
                        classification = %plan.classification,
                        "execution_mode='dag' but no DAG provided, falling through to heuristics"
                    );
                }
                "pipeline" => {
                    return Ok(TaskPlan {
                        use_lead_agent: false,
                        dag: None,
                        ..plan
                    });
                }
                _ => {
                    tracing::warn!(
                        classification = %plan.classification,
                        execution_mode = %mode,
                        "Unknown execution_mode value, falling through to heuristics"
                    );
                }
            }
        }

        // Mutual exclusivity: if planner returned both use_lead_agent and a DAG,
        // strip the DAG (lead agent takes priority as the safer single-orchestrator path).
        if plan.use_lead_agent && plan.dag.is_some() {
            tracing::warn!(
                classification = %plan.classification,
                "Stripping DAG: use_lead_agent=true and dag both present"
            );
            return Ok(TaskPlan {
                dag: None,
                auto_promotion_reason: Some("mutual_exclusivity_stripped".into()),
                ..plan
            });
        }

        // Safety net: if the LLM returned complex_task but provided no execution
        // path (no assignments, no DAG, no lead_agent), auto-promote to lead_agent
        // instead of letting dispatch_planned() fail with "No agents assigned".
        if plan.classification == "complex_task"
            && plan.assignments.is_empty()
            && plan.dag.is_none()
            && !plan.use_lead_agent
        {
            tracing::warn!(
                classification = %plan.classification,
                reasoning = ?plan.reasoning,
                title = ?plan.title,
                "Auto-promoting to lead agent: planner returned complex_task with no \
                 assignments, no DAG, and use_lead_agent=false. This may indicate a \
                 planning error — check the planner prompt and model output."
            );
            return Ok(TaskPlan {
                use_lead_agent: true,
                auto_promotion_reason: Some("empty_complex_task".into()),
                ..plan
            });
        }

        Ok(plan)
    }

    /// Extract JSON from a response that may be wrapped in markdown code fences
    /// or surrounded by prose.
    fn extract_json(content: &str) -> &str {
        extract_json_block(content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_query_response() {
        let json = r#"{"classification": "simple_query", "title": null, "assignments": [], "reasoning": "This is a greeting"}"#;
        let plan = TaskPlanner::parse_response(json).unwrap();
        assert_eq!(plan.classification, "simple_query");
        assert!(plan.title.is_none());
        assert!(plan.assignments.is_empty());
        assert_eq!(plan.reasoning.as_deref(), Some("This is a greeting"));
    }

    #[test]
    fn test_parse_complex_task_response() {
        let json = r#"{
            "classification": "complex_task",
            "title": "Research Rust async patterns",
            "assignments": [{
                "agent_id": "researcher-01",
                "agent_name": "Researcher",
                "role_description": "Search for information about Rust async patterns",
                "matched_skills": ["web_search", "summarize"]
            }],
            "reasoning": "User wants research, assigning researcher agent"
        }"#;
        let plan = TaskPlanner::parse_response(json).unwrap();
        assert_eq!(plan.classification, "complex_task");
        assert_eq!(plan.title.as_deref(), Some("Research Rust async patterns"));
        assert_eq!(plan.assignments.len(), 1);
        assert_eq!(plan.assignments[0].agent_id, "researcher-01");
        assert_eq!(
            plan.assignments[0].matched_skills,
            vec!["web_search", "summarize"]
        );
    }

    #[test]
    fn test_parse_response_with_markdown_fences() {
        let content = "```json\n{\"classification\": \"simple_query\", \"title\": null, \"assignments\": [], \"reasoning\": \"greeting\"}\n```";
        let plan = TaskPlanner::parse_response(content).unwrap();
        assert_eq!(plan.classification, "simple_query");
    }

    #[test]
    fn test_parse_response_with_plain_fences() {
        let content = "```\n{\"classification\": \"simple_query\", \"title\": null, \"assignments\": [], \"reasoning\": \"test\"}\n```";
        let plan = TaskPlanner::parse_response(content).unwrap();
        assert_eq!(plan.classification, "simple_query");
    }

    #[test]
    fn test_parse_malformed_response() {
        let result = TaskPlanner::parse_response("this is not json at all");
        assert!(result.is_err());
        match result.unwrap_err() {
            PlanError::MalformedResponse(msg) => {
                assert!(msg.contains("Failed to parse JSON"));
            }
            _ => panic!("Expected MalformedResponse"),
        }
    }

    #[test]
    fn test_extract_json_bare() {
        let input = r#"{"classification": "simple_query"}"#;
        assert_eq!(TaskPlanner::extract_json(input), input);
    }

    #[test]
    fn test_extract_json_with_whitespace() {
        let input = "  \n{\"classification\": \"simple_query\"}\n  ";
        assert_eq!(
            TaskPlanner::extract_json(input),
            "{\"classification\": \"simple_query\"}"
        );
    }

    #[test]
    fn test_extract_json_prose_around_braces() {
        let input = "Here is my analysis:\n{\"classification\": \"simple_query\", \"title\": null, \"assignments\": [], \"reasoning\": \"test\"}\nHope that helps!";
        let plan = TaskPlanner::parse_response(input).unwrap();
        assert_eq!(plan.classification, "simple_query");
    }

    #[test]
    fn test_extract_json_braces_with_strings_containing_braces() {
        let input = r#"Sure! {"classification": "simple_query", "title": null, "assignments": [], "reasoning": "The user said {hello}"}"#;
        let plan = TaskPlanner::parse_response(input).unwrap();
        assert_eq!(plan.classification, "simple_query");
        assert!(plan.reasoning.unwrap().contains("{hello}"));
    }

    #[test]
    fn test_extract_json_no_json_at_all() {
        let result = TaskPlanner::parse_response("No JSON here whatsoever.");
        assert!(result.is_err());
    }

    #[test]
    fn test_find_outermost_braces_escaped_quotes() {
        let input = r#"text {"key": "val with \"escaped\" quotes"} trailing"#;
        let extracted = extract_json_block(input);
        assert!(extracted.starts_with('{'));
        assert!(extracted.ends_with('}'));
    }

    #[test]
    fn test_parse_response_with_extra_fields() {
        // LLM sometimes echoes back agent info alongside the classification
        let json = r#"{
            "available_agents": [{"agent_id": "writing_agent", "name": "Writer"}],
            "classification": "simple_query",
            "title": null,
            "assignments": [],
            "reasoning": "Greeting detected"
        }"#;
        let plan = TaskPlanner::parse_response(json).unwrap();
        assert_eq!(plan.classification, "simple_query");
        assert!(plan.assignments.is_empty());
    }

    #[test]
    fn test_parse_response_no_classification_at_all() {
        let json = r#"{"available_agents": [{"agent_id": "writing_agent"}]}"#;
        let result = TaskPlanner::parse_response(json);
        assert!(result.is_err());
        match result.unwrap_err() {
            PlanError::MalformedResponse(msg) => {
                assert!(msg.contains("missing field `classification`"));
            }
            _ => panic!("Expected MalformedResponse"),
        }
    }

    #[test]
    fn test_complex_task_empty_auto_promotes_to_lead_agent() {
        // When complex_task has no assignments, no DAG, and no lead_agent,
        // parse_response auto-promotes to use_lead_agent=true as a safety net.
        let json = r#"{
            "classification": "complex_task",
            "title": "Do something",
            "assignments": [],
            "reasoning": "test"
        }"#;
        let plan = TaskPlanner::parse_response(json).unwrap();
        assert_eq!(plan.classification, "complex_task");
        assert!(plan.use_lead_agent);
        assert!(plan.dag.is_none());
        assert!(plan.assignments.is_empty());
    }

    #[test]
    fn test_parse_response_malformed_returns_correct_error_variant() {
        let result = TaskPlanner::parse_response("garbage text");
        assert!(matches!(result, Err(PlanError::MalformedResponse(_))));
    }

    #[test]
    fn test_parse_response_valid_json_missing_classification_is_malformed() {
        let result = TaskPlanner::parse_response(r#"{"foo": "bar"}"#);
        assert!(matches!(result, Err(PlanError::MalformedResponse(_))));
    }

    // ── DAG tests ─────────────────────────────────────────────────────

    use crate::agent::subagent::{AgentConstraints, AgentLlmConfig, AgentPreset, AgentStatus};

    fn make_agent(id: &str) -> SubAgent {
        SubAgent {
            id: id.to_string(),
            template_id: id.to_string(),
            name: format!("Agent {}", id),
            description: None,
            icon: None,
            status: AgentStatus::Idle,
            current_task: None,
            skills: vec![],
            preset: AgentPreset::default(),
            constraints: AgentConstraints::default(),
            llm_config: AgentLlmConfig::default(),
        }
    }

    fn make_dag_node(id: &str, agent_id: &str, deps: &[&str]) -> DagNode {
        DagNode {
            node_id: id.to_string(),
            title: format!("Task {}", id),
            description: format!("Do {}", id),
            agent_id: agent_id.to_string(),
            agent_name: format!("Agent {}", agent_id),
            depends_on: deps.iter().map(|d| d.to_string()).collect(),
            status: DagNodeStatus::Pending,
            result_summary: None,
            workspace_keys: vec![],
            output_key: Some(format!("{}_output", id)),
        }
    }

    #[test]
    fn test_dag_validate_valid() {
        let dag = TaskDag {
            nodes: vec![
                make_dag_node("n1", "a1", &[]),
                make_dag_node("n2", "a2", &["n1"]),
                make_dag_node("n3", "a1", &["n1"]),
                make_dag_node("n4", "a2", &["n2", "n3"]),
            ],
        };
        let agents = vec![make_agent("a1"), make_agent("a2")];
        assert!(dag.validate(&agents).is_ok());
    }

    #[test]
    fn test_dag_validate_empty() {
        let dag = TaskDag { nodes: vec![] };
        let agents = vec![make_agent("a1")];
        let err = dag.validate(&agents).unwrap_err();
        assert!(err.contains("no nodes"));
    }

    #[test]
    fn test_dag_validate_single_node() {
        let dag = TaskDag {
            nodes: vec![make_dag_node("n1", "a1", &[])],
        };
        let agents = vec![make_agent("a1")];
        let err = dag.validate(&agents).unwrap_err();
        assert!(err.contains("at least 2 nodes"));
    }

    #[test]
    fn test_dag_validate_structure_single_node() {
        let dag = TaskDag {
            nodes: vec![make_dag_node("n1", "a1", &[])],
        };
        let err = dag.validate_structure().unwrap_err();
        assert!(err.contains("at least 2 nodes"));
    }

    #[test]
    fn test_dag_validate_too_many_nodes() {
        let nodes: Vec<DagNode> = (0..9)
            .map(|i| make_dag_node(&format!("n{}", i), "a1", &[]))
            .collect();
        let dag = TaskDag { nodes };
        let agents = vec![make_agent("a1")];
        let err = dag.validate(&agents).unwrap_err();
        assert!(err.contains("max 8"));
    }

    #[test]
    fn test_dag_validate_unknown_dependency() {
        let dag = TaskDag {
            nodes: vec![
                make_dag_node("n1", "a1", &["nonexistent"]),
                make_dag_node("n2", "a1", &[]),
            ],
        };
        let agents = vec![make_agent("a1")];
        let err = dag.validate(&agents).unwrap_err();
        assert!(err.contains("unknown node"));
    }

    #[test]
    fn test_dag_validate_unknown_agent() {
        let dag = TaskDag {
            nodes: vec![
                make_dag_node("n1", "ghost_agent", &[]),
                make_dag_node("n2", "a1", &[]),
            ],
        };
        let agents = vec![make_agent("a1")];
        let err = dag.validate(&agents).unwrap_err();
        assert!(err.contains("unknown agent"));
    }

    #[test]
    fn test_dag_validate_cycle() {
        let dag = TaskDag {
            nodes: vec![
                make_dag_node("n1", "a1", &["n2"]),
                make_dag_node("n2", "a1", &["n1"]),
            ],
        };
        let agents = vec![make_agent("a1")];
        let err = dag.validate(&agents).unwrap_err();
        assert!(err.contains("cycle"));
    }

    #[test]
    fn test_dag_ready_nodes_initial() {
        let dag = TaskDag {
            nodes: vec![
                make_dag_node("n1", "a1", &[]),
                make_dag_node("n2", "a1", &[]),
                make_dag_node("n3", "a1", &["n1", "n2"]),
            ],
        };
        let ready: Vec<&str> = dag
            .ready_nodes()
            .iter()
            .map(|n| n.node_id.as_str())
            .collect();
        assert!(ready.contains(&"n1"));
        assert!(ready.contains(&"n2"));
        assert!(!ready.contains(&"n3"));
    }

    #[test]
    fn test_dag_complete_node_unlocks_dependents() {
        let mut dag = TaskDag {
            nodes: vec![
                make_dag_node("n1", "a1", &[]),
                make_dag_node("n2", "a1", &["n1"]),
            ],
        };
        // Initially only n1 is ready
        assert_eq!(dag.ready_nodes().len(), 1);

        // Complete n1 → n2 becomes ready
        let newly_ready = dag.complete_node("n1", "done");
        assert!(newly_ready.contains(&"n2".to_string()));
        assert_eq!(dag.nodes[0].status, DagNodeStatus::Completed);
        assert_eq!(dag.nodes[0].result_summary.as_deref(), Some("done"));
    }

    #[test]
    fn test_dag_fail_node_skips_dependents() {
        let mut dag = TaskDag {
            nodes: vec![
                make_dag_node("n1", "a1", &[]),
                make_dag_node("n2", "a1", &["n1"]),
                make_dag_node("n3", "a1", &["n2"]),
            ],
        };
        dag.fail_node("n1", "error");
        assert_eq!(dag.nodes[0].status, DagNodeStatus::Failed);
        assert_eq!(dag.nodes[1].status, DagNodeStatus::Skipped);
        assert_eq!(dag.nodes[2].status, DagNodeStatus::Skipped);
    }

    #[test]
    fn test_dag_fail_node_independent_branches_survive() {
        let mut dag = TaskDag {
            nodes: vec![
                make_dag_node("n1", "a1", &[]),
                make_dag_node("n2", "a1", &[]),
                make_dag_node("n3", "a1", &["n1"]),
            ],
        };
        dag.fail_node("n1", "error");
        // n2 is independent — should stay pending
        assert_eq!(dag.nodes[0].status, DagNodeStatus::Failed);
        assert_eq!(dag.nodes[1].status, DagNodeStatus::Pending);
        assert_eq!(dag.nodes[2].status, DagNodeStatus::Skipped);
    }

    #[test]
    fn test_skip_dependents_diamond_shape() {
        // n1 -> n2, n1 -> n3, n2 -> n4, n3 -> n4
        let mut dag = TaskDag {
            nodes: vec![
                make_dag_node("n1", "a1", &[]),
                make_dag_node("n2", "a1", &["n1"]),
                make_dag_node("n3", "a1", &["n1"]),
                make_dag_node("n4", "a1", &["n2", "n3"]),
            ],
        };
        dag.fail_node("n1", "error");
        assert_eq!(dag.nodes[0].status, DagNodeStatus::Failed);
        assert_eq!(dag.nodes[1].status, DagNodeStatus::Skipped);
        assert_eq!(dag.nodes[2].status, DagNodeStatus::Skipped);
        assert_eq!(dag.nodes[3].status, DagNodeStatus::Skipped);
    }

    #[test]
    fn test_skip_dependents_does_not_skip_running_nodes() {
        let mut dag = TaskDag {
            nodes: vec![
                make_dag_node("n1", "a1", &[]),
                make_dag_node("n2", "a1", &["n1"]),
            ],
        };
        dag.mark_running("n2");
        dag.fail_node("n1", "error");
        // n2 is Running, so it should NOT be skipped
        assert_eq!(dag.nodes[1].status, DagNodeStatus::Running);
    }

    #[test]
    fn test_dag_is_finished() {
        let mut dag = TaskDag {
            nodes: vec![
                make_dag_node("n1", "a1", &[]),
                make_dag_node("n2", "a1", &["n1"]),
            ],
        };
        assert!(!dag.is_finished());
        dag.complete_node("n1", "ok");
        assert!(!dag.is_finished());
        dag.complete_node("n2", "ok");
        assert!(dag.is_finished());
    }

    #[test]
    fn test_dag_is_finished_with_failures() {
        let mut dag = TaskDag {
            nodes: vec![
                make_dag_node("n1", "a1", &[]),
                make_dag_node("n2", "a1", &["n1"]),
            ],
        };
        dag.fail_node("n1", "error");
        // n2 gets skipped, so all nodes are terminal
        assert!(dag.is_finished());
    }

    #[test]
    fn test_dag_topological_order() {
        let dag = TaskDag {
            nodes: vec![
                make_dag_node("n3", "a1", &["n1", "n2"]),
                make_dag_node("n1", "a1", &[]),
                make_dag_node("n2", "a1", &["n1"]),
            ],
        };
        let order = dag.topological_order();
        assert_eq!(order.len(), 3);
        // n1 must come before n2 and n3
        let pos_n1 = order.iter().position(|id| id == "n1").unwrap();
        let pos_n2 = order.iter().position(|id| id == "n2").unwrap();
        let pos_n3 = order.iter().position(|id| id == "n3").unwrap();
        assert!(pos_n1 < pos_n2);
        assert!(pos_n1 < pos_n3);
        assert!(pos_n2 < pos_n3);
    }

    #[test]
    fn test_dag_mark_running() {
        let mut dag = TaskDag {
            nodes: vec![make_dag_node("n1", "a1", &[])],
        };
        dag.mark_running("n1");
        assert_eq!(dag.nodes[0].status, DagNodeStatus::Running);
    }

    #[test]
    fn test_dag_completed_count() {
        let mut dag = TaskDag {
            nodes: vec![
                make_dag_node("n1", "a1", &[]),
                make_dag_node("n2", "a1", &[]),
                make_dag_node("n3", "a1", &[]),
            ],
        };
        assert_eq!(dag.completed_count(), 0);
        dag.complete_node("n1", "ok");
        assert_eq!(dag.completed_count(), 1);
        dag.complete_node("n2", "ok");
        assert_eq!(dag.completed_count(), 2);
    }

    #[test]
    fn test_dag_serialization_roundtrip() {
        let dag = TaskDag {
            nodes: vec![
                make_dag_node("n1", "a1", &[]),
                make_dag_node("n2", "a2", &["n1"]),
            ],
        };
        let json = serde_json::to_string(&dag).unwrap();
        let deserialized: TaskDag = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.nodes.len(), 2);
        assert_eq!(deserialized.nodes[0].node_id, "n1");
        assert_eq!(deserialized.nodes[1].depends_on, vec!["n1"]);
    }

    #[test]
    fn test_parse_response_with_dag() {
        let json = r#"{
            "classification": "complex_task",
            "title": "Research and write",
            "assignments": [],
            "reasoning": "Multi-step task",
            "use_lead_agent": false,
            "dag": {
                "nodes": [
                    {"node_id": "n1", "title": "Research", "description": "Do research", "agent_id": "a1", "agent_name": "Agent a1", "depends_on": [], "workspace_keys": [], "output_key": "research"},
                    {"node_id": "n2", "title": "Write", "description": "Write summary", "agent_id": "a2", "agent_name": "Agent a2", "depends_on": ["n1"], "workspace_keys": ["research"], "output_key": "summary"}
                ]
            }
        }"#;
        let plan = TaskPlanner::parse_response(json).unwrap();
        assert_eq!(plan.classification, "complex_task");
        assert!(plan.dag.is_some());
        let dag = plan.dag.unwrap();
        assert_eq!(dag.nodes.len(), 2);
        assert_eq!(dag.nodes[0].node_id, "n1");
        assert_eq!(dag.nodes[1].depends_on, vec!["n1"]);
        assert_eq!(dag.nodes[1].workspace_keys, vec!["research"]);
    }

    #[test]
    fn test_parse_response_without_dag() {
        let json = r#"{"classification": "simple_query", "title": null, "assignments": [], "reasoning": "Greeting"}"#;
        let plan = TaskPlanner::parse_response(json).unwrap();
        assert!(plan.dag.is_none());
    }

    #[test]
    fn test_dag_complete_node_caps_summary() {
        let mut dag = TaskDag {
            nodes: vec![make_dag_node("n1", "a1", &[])],
        };
        let long_summary = "x".repeat(600);
        dag.complete_node("n1", &long_summary);
        assert_eq!(dag.nodes[0].result_summary.as_ref().unwrap().len(), 500);
    }

    #[test]
    fn test_dag_fail_node_caps_error() {
        let mut dag = TaskDag {
            nodes: vec![make_dag_node("n1", "a1", &[])],
        };
        let long_error = "e".repeat(600);
        dag.fail_node("n1", &long_error);
        assert_eq!(dag.nodes[0].result_summary.as_ref().unwrap().len(), 500);
    }

    #[test]
    fn test_build_hierarchical_prompt_with_agents() {
        let agents = vec![make_agent("a1")];
        let prompt = TaskPlanner::build_hierarchical_prompt(&agents, false);
        assert!(prompt.contains("a1"));
        // XML structure tags
        assert!(prompt.contains("<agents>"));
        assert!(prompt.contains("<instructions>"));
        assert!(prompt.contains("<examples>"));
        assert!(prompt.contains("<format>"));
        assert!(prompt.contains("<rules>"));
        // DAG fields still referenced
        assert!(prompt.contains("depends_on"));
        assert!(prompt.contains("workspace_keys"));
        // Concrete few-shot examples present
        assert!(prompt.contains("Translate"));
        assert!(prompt.contains("Research"));
        // "Grey area" example was removed in prompt cleanup (Step 4)
        assert!(prompt.contains("Do NOT set both"));
    }

    #[test]
    fn test_build_hierarchical_prompt_no_agents() {
        let prompt = TaskPlanner::build_hierarchical_prompt(&[], false);
        assert!(prompt.contains("No agents are currently available"));
    }

    #[test]
    fn test_prompt_includes_v2_fields_when_enabled() {
        let prompt = TaskPlanner::build_hierarchical_prompt(&[], true);
        assert!(prompt.contains("execution_mode"));
        assert!(prompt.contains("predictability_score"));
        assert!(prompt.contains("v2_protocol"));
    }

    #[test]
    fn test_prompt_excludes_v2_fields_when_disabled() {
        let prompt = TaskPlanner::build_hierarchical_prompt(&[], false);
        assert!(!prompt.contains("v2_protocol"));
    }

    #[test]
    fn test_taskplan_old_json_compat() {
        let json = r#"{"classification": "simple_query", "title": null, "assignments": [], "reasoning": "test", "dag": null, "use_lead_agent": false}"#;
        let plan: TaskPlan = serde_json::from_str(json).unwrap();
        assert!(plan.execution_mode.is_none());
        assert!(plan.predictability_score.is_none());
    }

    #[test]
    fn test_taskplan_v2_execution_mode_dag() {
        let json = r#"{"classification": "complex_task", "title": "Test", "assignments": [], "reasoning": "test", "dag": {"nodes": [{"node_id": "n1", "title": "A", "description": "D", "agent_id": "a1", "agent_name": "Agent", "depends_on": [], "workspace_keys": [], "output_key": null}, {"node_id": "n2", "title": "B", "description": "D", "agent_id": "a1", "agent_name": "Agent", "depends_on": ["n1"], "workspace_keys": [], "output_key": null}]}, "use_lead_agent": true, "execution_mode": "dag", "predictability_score": 0.9}"#;
        let plan = TaskPlanner::parse_response(json).unwrap();
        // execution_mode "dag" should override use_lead_agent=true
        assert!(!plan.use_lead_agent);
        assert!(plan.dag.is_some());
        assert_eq!(plan.execution_mode.as_deref(), Some("dag"));
        assert_eq!(plan.predictability_score, Some(0.9));
    }

    #[test]
    fn test_taskplan_v2_execution_mode_lead() {
        let json = r#"{"classification": "complex_task", "title": "Test", "assignments": [], "reasoning": "test", "dag": null, "use_lead_agent": false, "execution_mode": "lead_agent"}"#;
        let plan = TaskPlanner::parse_response(json).unwrap();
        assert!(plan.use_lead_agent);
        assert!(plan.dag.is_none());
    }

    #[test]
    fn test_taskplan_v2_execution_mode_pipeline() {
        let json = r#"{"classification": "complex_task", "title": "Test", "assignments": [{"agent_id": "a1", "agent_name": "Agent", "role_description": "Role", "matched_skills": ["coding"]}], "reasoning": "test", "dag": null, "use_lead_agent": true, "execution_mode": "pipeline"}"#;
        let plan = TaskPlanner::parse_response(json).unwrap();
        assert!(!plan.use_lead_agent);
        assert!(plan.dag.is_none());
    }

    #[test]
    fn test_taskplan_v2_predictability_score() {
        let json = r#"{"classification": "complex_task", "title": "Test", "assignments": [], "reasoning": "test", "dag": null, "use_lead_agent": true, "predictability_score": 0.85}"#;
        let plan: TaskPlan = serde_json::from_str(json).unwrap();
        assert_eq!(plan.predictability_score, Some(0.85));
    }

    // ── validate_structure tests ─────────────────────────────────

    #[test]
    fn test_validate_structure_valid_dag() {
        let dag = TaskDag {
            nodes: vec![
                make_dag_node("n1", "a1", &[]),
                make_dag_node("n2", "a1", &["n1"]),
                make_dag_node("n3", "a1", &["n1"]),
                make_dag_node("n4", "a1", &["n2", "n3"]),
            ],
        };
        assert!(dag.validate_structure().is_ok());
    }

    #[test]
    fn test_validate_structure_detects_cycle() {
        let dag = TaskDag {
            nodes: vec![
                make_dag_node("n1", "a1", &["n2"]),
                make_dag_node("n2", "a1", &["n1"]),
            ],
        };
        let err = dag.validate_structure().unwrap_err();
        assert!(err.contains("cycle"));
    }

    #[test]
    fn test_validate_structure_detects_unknown_dep() {
        let dag = TaskDag {
            nodes: vec![
                make_dag_node("n1", "a1", &["nonexistent"]),
                make_dag_node("n2", "a1", &[]),
            ],
        };
        let err = dag.validate_structure().unwrap_err();
        assert!(err.contains("unknown node"));
    }

    #[test]
    fn test_validate_structure_empty_dag() {
        let dag = TaskDag { nodes: vec![] };
        let err = dag.validate_structure().unwrap_err();
        assert!(err.contains("no nodes"));
    }

    // ── use_lead_agent tests ─────────────────────────────────────

    #[test]
    fn test_task_plan_use_lead_agent_defaults_to_false_but_promotes_when_orphaned() {
        // When use_lead_agent is missing from JSON, serde defaults to false,
        // but auto-promote kicks in because assignments+dag are also empty.
        let json = r#"{
            "classification": "complex_task",
            "title": "Some task",
            "assignments": [],
            "reasoning": "test"
        }"#;
        let plan = TaskPlanner::parse_response(json).unwrap();
        assert!(plan.use_lead_agent);
    }

    #[test]
    fn test_task_plan_use_lead_agent_true() {
        // When use_lead_agent is explicitly true
        let json = r#"{
            "classification": "complex_task",
            "title": "Dynamic research task",
            "assignments": [],
            "reasoning": "Task is exploratory",
            "use_lead_agent": true
        }"#;
        let plan = TaskPlanner::parse_response(json).unwrap();
        assert!(plan.use_lead_agent);
        assert_eq!(plan.classification, "complex_task");
        assert_eq!(plan.title.as_deref(), Some("Dynamic research task"));
    }

    #[test]
    fn test_task_plan_use_lead_agent_false_explicit_promotes_when_orphaned() {
        // Even with explicit use_lead_agent=false, if assignments and DAG are
        // both empty, auto-promote overrides to true as a safety net.
        let json = r#"{
            "classification": "complex_task",
            "title": "Predictable task",
            "assignments": [],
            "reasoning": "test",
            "use_lead_agent": false
        }"#;
        let plan = TaskPlanner::parse_response(json).unwrap();
        assert!(plan.use_lead_agent);
    }

    #[test]
    fn test_task_plan_use_lead_agent_with_dag() {
        // When both use_lead_agent and dag are present, DAG should be stripped
        // (mutual exclusivity: lead agent takes priority).
        let json = r#"{
            "classification": "complex_task",
            "title": "Complex task",
            "assignments": [],
            "reasoning": "test",
            "use_lead_agent": true,
            "dag": {
                "nodes": [
                    {"node_id": "n1", "title": "Step 1", "description": "Do step 1", "agent_id": "a1", "agent_name": "Agent a1", "depends_on": [], "workspace_keys": [], "output_key": "step1"}
                ]
            }
        }"#;
        let plan = TaskPlanner::parse_response(json).unwrap();
        assert!(plan.use_lead_agent);
        assert!(plan.dag.is_none(), "DAG should be stripped when use_lead_agent is true");
        assert_eq!(
            plan.auto_promotion_reason.as_deref(),
            Some("mutual_exclusivity_stripped")
        );
    }

    #[test]
    fn test_parse_response_fallback_extracts_use_lead_agent() {
        // When classification is embedded in a larger JSON object (fallback path)
        let json = r#"{
            "available_agents": [{"agent_id": "a1"}],
            "classification": "complex_task",
            "title": "Exploratory task",
            "assignments": [
                {"agent_id": "a1", "agent_name": "Agent a1", "role_description": "Lead", "matched_skills": ["research"]}
            ],
            "reasoning": "test",
            "use_lead_agent": true
        }"#;
        let plan = TaskPlanner::parse_response(json).unwrap();
        assert!(plan.use_lead_agent);
    }

    // ── has_predictable_structure tests ────────────────────────

    #[test]
    fn test_predictable_structure_numbered_list() {
        assert!(has_predictable_structure(
            "1. Translate to French\n2. Translate to Spanish\n3. Translate to German"
        ));
    }

    #[test]
    fn test_predictable_structure_bullet_list() {
        assert!(has_predictable_structure(
            "- Write the intro\n- Write the body\n- Write the conclusion"
        ));
    }

    #[test]
    fn test_predictable_structure_batch_translate() {
        assert!(has_predictable_structure(
            "Translate this into French, Spanish, and German for each chapter"
        ));
    }

    #[test]
    fn test_predictable_structure_explicit_quantity() {
        assert!(has_predictable_structure("Split this into 3 sections"));
    }

    #[test]
    fn test_predictable_structure_simple_message() {
        assert!(!has_predictable_structure("debug my test"));
    }

    #[test]
    fn test_predictable_structure_greeting() {
        assert!(!has_predictable_structure("hello, how are you?"));
    }

    // ── PlanError display tests ─────────────────────────────────

    #[test]
    fn test_plan_error_timeout_display() {
        let err = PlanError::Timeout(30);
        assert_eq!(err.to_string(), "Planning timed out after 30s");
    }

    #[test]
    fn test_plan_error_llm_error_display() {
        let err = PlanError::LlmError("connection refused".to_string());
        assert_eq!(err.to_string(), "LLM error: connection refused");
    }

    #[test]
    fn test_plan_error_malformed_display() {
        let err = PlanError::MalformedResponse("bad json".to_string());
        assert_eq!(err.to_string(), "Malformed response: bad json");
    }

    #[test]
    fn test_missing_use_lead_agent_defaults_true() {
        // When the LLM omits use_lead_agent entirely, it should default to true
        // (lead agent is the safer fallback for complex tasks).
        let json = r#"{
            "classification": "complex_task",
            "title": "Research something",
            "assignments": [{"agent_id": "a1", "agent_name": "Agent A", "role_description": "do stuff", "matched_skills": ["search"]}],
            "reasoning": "needs research"
        }"#;
        let plan = TaskPlanner::parse_response(json).unwrap();
        assert_eq!(plan.classification, "complex_task");
        assert!(
            plan.use_lead_agent,
            "Missing use_lead_agent should default to true"
        );
    }

    #[test]
    fn test_explicit_false_with_dag_stays_false() {
        // When the LLM explicitly sets use_lead_agent: false AND provides a DAG,
        // we respect the explicit choice.
        let json = r#"{
            "classification": "complex_task",
            "title": "Translate documents",
            "assignments": [],
            "reasoning": "Known steps, using DAG",
            "dag": {"nodes": [
                {"node_id": "n1", "title": "Translate EN", "description": "...", "agent_id": "translator-01", "agent_name": "Translator", "depends_on": [], "workspace_keys": [], "output_key": "en"},
                {"node_id": "n2", "title": "Translate FR", "description": "...", "agent_id": "translator-01", "agent_name": "Translator", "depends_on": [], "workspace_keys": [], "output_key": "fr"}
            ]},
            "use_lead_agent": false
        }"#;
        let plan = TaskPlanner::parse_response(json).unwrap();
        assert_eq!(plan.classification, "complex_task");
        assert!(
            !plan.use_lead_agent,
            "Explicit false with DAG should stay false"
        );
        assert!(plan.dag.is_some());
    }

    #[test]
    fn test_explicit_true_use_lead_agent() {
        let json = r#"{
            "classification": "complex_task",
            "title": "Debug failing tests",
            "assignments": [],
            "reasoning": "Exploratory task",
            "use_lead_agent": true
        }"#;
        let plan = TaskPlanner::parse_response(json).unwrap();
        assert!(plan.use_lead_agent);
        assert!(plan.dag.is_none());
    }

    // ── Critical path scheduling tests ──────────────────────────────

    #[test]
    fn test_critical_path_linear_chain() {
        // A -> B -> C: lengths = {A:2, B:1, C:0}
        let dag = TaskDag {
            nodes: vec![
                make_dag_node("A", "a1", &[]),
                make_dag_node("B", "a1", &["A"]),
                make_dag_node("C", "a1", &["B"]),
            ],
        };
        let lengths = dag.critical_path_lengths();
        assert_eq!(lengths["A"], 2);
        assert_eq!(lengths["B"], 1);
        assert_eq!(lengths["C"], 0);
    }

    #[test]
    fn test_critical_path_diamond() {
        // A -> {B, C} -> D: lengths = {A:2, B:1, C:1, D:0}
        let dag = TaskDag {
            nodes: vec![
                make_dag_node("A", "a1", &[]),
                make_dag_node("B", "a1", &["A"]),
                make_dag_node("C", "a1", &["A"]),
                make_dag_node("D", "a1", &["B", "C"]),
            ],
        };
        let lengths = dag.critical_path_lengths();
        assert_eq!(lengths["A"], 2);
        assert_eq!(lengths["B"], 1);
        assert_eq!(lengths["C"], 1);
        assert_eq!(lengths["D"], 0);
    }

    #[test]
    fn test_critical_path_wide_fan_out() {
        // A -> {B, C, D} all independent leaves: A:1, B/C/D:0
        let dag = TaskDag {
            nodes: vec![
                make_dag_node("A", "a1", &[]),
                make_dag_node("B", "a1", &["A"]),
                make_dag_node("C", "a1", &["A"]),
                make_dag_node("D", "a1", &["A"]),
            ],
        };
        let lengths = dag.critical_path_lengths();
        assert_eq!(lengths["A"], 1);
        assert_eq!(lengths["B"], 0);
        assert_eq!(lengths["C"], 0);
        assert_eq!(lengths["D"], 0);
    }

    #[test]
    fn test_critical_path_asymmetric() {
        // A -> B -> C (long path), A -> D (short path)
        // A:2, B:1, C:0, D:0; B should be prioritized over D
        let dag = TaskDag {
            nodes: vec![
                make_dag_node("A", "a1", &[]),
                make_dag_node("B", "a1", &["A"]),
                make_dag_node("C", "a1", &["B"]),
                make_dag_node("D", "a1", &["A"]),
            ],
        };
        let lengths = dag.critical_path_lengths();
        assert_eq!(lengths["A"], 2);
        assert_eq!(lengths["B"], 1);
        assert_eq!(lengths["C"], 0);
        assert_eq!(lengths["D"], 0);
    }

    #[test]
    fn test_ready_nodes_prioritized_ordering() {
        // After A completes: B (path length 1) and D (path length 0) both ready
        // B should come first because it has longer downstream path
        let mut dag = TaskDag {
            nodes: vec![
                make_dag_node("A", "a1", &[]),
                make_dag_node("B", "a1", &["A"]),
                make_dag_node("C", "a1", &["B"]),
                make_dag_node("D", "a1", &["A"]),
            ],
        };
        dag.complete_node("A", "done");

        let prioritized = dag.ready_nodes_prioritized();
        assert_eq!(prioritized.len(), 2);
        assert_eq!(prioritized[0].node_id, "B"); // longer downstream path
        assert_eq!(prioritized[1].node_id, "D"); // shorter downstream path
    }

    #[test]
    fn test_critical_path_disabled_uses_original_order() {
        // When not using prioritized, ready_nodes() returns in node order
        let mut dag = TaskDag {
            nodes: vec![
                make_dag_node("A", "a1", &[]),
                make_dag_node("B", "a1", &["A"]),
                make_dag_node("C", "a1", &["B"]),
                make_dag_node("D", "a1", &["A"]),
            ],
        };
        dag.complete_node("A", "done");

        let normal = dag.ready_nodes();
        let prioritized = dag.ready_nodes_prioritized();

        // Both return the same nodes, just potentially different order
        assert_eq!(normal.len(), prioritized.len());
        let normal_ids: HashSet<&str> = normal.iter().map(|n| n.node_id.as_str()).collect();
        let prio_ids: HashSet<&str> = prioritized.iter().map(|n| n.node_id.as_str()).collect();
        assert_eq!(normal_ids, prio_ids);
    }

    // ── build_messages prompt-injection hardening tests ────────────────

    #[test]
    fn test_build_messages_untrusted_context_uses_user_role() {
        let system_prompt = "You are the planner.";
        let user_msg = "Build me a web scraper";
        let summary = "User previously asked about Rust";
        let tasks_block = "### ACTIVE TASKS ###\n- [abc12345] Fix bug (in_progress)";

        let msgs = build_messages(
            system_prompt,
            user_msg,
            &[],
            Some(summary),
            Some(tasks_block),
        );

        // First message must be the system policy prompt
        assert_eq!(msgs[0].role, openalpaca_llm::Role::System);
        assert_eq!(msgs[0].content, system_prompt);

        // Session summary and active tasks must be User role, not System
        assert_eq!(msgs[1].role, openalpaca_llm::Role::User, "Summary should be user role");
        assert_eq!(msgs[2].role, openalpaca_llm::Role::User, "Tasks should be user role");

        // Both must contain the untrusted-context framing
        assert!(
            msgs[1].content.contains("context_data"),
            "Summary should be wrapped in <context_data>"
        );
        assert!(
            msgs[1].content.contains("NOT instructions"),
            "Summary should contain injection guard"
        );
        assert!(
            msgs[2].content.contains("context_data"),
            "Tasks should be wrapped in <context_data>"
        );

        // Final message is the user query
        let last = msgs.last().unwrap();
        assert_eq!(last.role, openalpaca_llm::Role::User);
        assert_eq!(last.content, user_msg);
    }

    #[test]
    fn test_build_messages_no_context_only_system_and_user() {
        let msgs = build_messages("System prompt.", "Hello", &[], None, None);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, openalpaca_llm::Role::System);
        assert_eq!(msgs[1].role, openalpaca_llm::Role::User);
    }
}
