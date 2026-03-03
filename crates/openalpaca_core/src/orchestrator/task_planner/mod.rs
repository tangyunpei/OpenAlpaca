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

    // Chinese parallel enumeration: "翻译成法语、西语和德语"
    let cjk_enum = CJK_ENUM_RE
        .get_or_init(|| Regex::new(r"[\u4e00-\u9fff]+[、，][\u4e00-\u9fff]+[、，]").unwrap());
    if cjk_enum.is_match(content) {
        return true;
    }

    // Conjunctive list without batch keyword: "X, Y, and Z" (3+ comma-separated items)
    let conj_list = CONJ_LIST_RE
        .get_or_init(|| Regex::new(r"(?i)\w+,\s+\w+,\s+").unwrap());
    if conj_list.is_match(content) {
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
        messages.push(ChatMessage::user(&super::wrap_untrusted_context(
            &format!("### SESSION SUMMARY ###\n{}", capped),
            "session_summary",
            "user_derived",
        )));
    }

    if let Some(tasks_block) = active_tasks_block {
        messages.push(ChatMessage::user(&super::wrap_untrusted_context(
            tasks_block,
            "active_tasks",
            "user_derived",
        )));
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
    let mut last_error_msg: Option<String> = None;
    let deadline = Duration::from_secs(limits.timeout_secs);

    for attempt in 0..=limits.max_retries {
        let mut attempt_messages = messages.clone();
        if attempt > 0
            && let Some(ref err) = last_error_msg
        {
            attempt_messages.push(ChatMessage::user(&format!(
                "Your previous response was invalid: {}. Respond with ONLY a valid JSON object.",
                err
            )));
        }
        let request = RouterRequest {
            model: None,
            messages: Arc::new(attempt_messages),
            tools: Arc::new(vec![]),
            temperature: Some(attempt as f32 * 0.1),
            max_tokens: Some(limits.max_tokens),
            context: RequestContext::default(),
            tool_choice: None,
            tools_token_estimate: None,
            enable_caching: false,
            thinking: None,
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
                    // Try to salvage as sequential pipeline via topological order.
                    // Only safe when the failure is structural (node count, deps) —
                    // NOT a cycle (no valid order) and NOT unknown agents (invalid IDs
                    // would propagate to pipeline dispatch and fail there too).
                    let salvageable = !e.contains("cycle") && !e.contains("unknown agent");
                    if salvageable && !plan.assignments.is_empty() {
                        tracing::warn!(
                            dag_error = %e,
                            "DAG validation failed, falling back to flat assignments"
                        );
                        return Ok(TaskPlan {
                            dag: None,
                            auto_promotion_reason: Some("dag_validation_salvaged_pipeline".into()),
                            ..plan
                        });
                    }

                    // Try extracting topological order as pipeline assignments
                    if salvageable {
                        let topo = dag.topological_order();
                        if !topo.is_empty() {
                            let pipeline_assignments: Vec<PlannedAssignment> = topo
                                .iter()
                                .filter_map(|nid| {
                                    dag.nodes.iter().find(|n| n.node_id == *nid)
                                })
                                .map(|node| PlannedAssignment {
                                    agent_id: node.agent_id.clone(),
                                    agent_name: node.agent_name.clone(),
                                    role_description: node.description.clone(),
                                    matched_skills: vec![],
                                })
                                .collect();
                            if !pipeline_assignments.is_empty() {
                                tracing::warn!(
                                    dag_error = %e,
                                    pipeline_steps = pipeline_assignments.len(),
                                    "DAG validation failed, salvaging as sequential pipeline"
                                );
                                return Ok(TaskPlan {
                                    dag: None,
                                    assignments: pipeline_assignments,
                                    use_lead_agent: false,
                                    auto_promotion_reason: Some(
                                        "dag_to_pipeline_salvage".into(),
                                    ),
                                    ..plan
                                });
                            }
                        }
                    }

                    // Fallback: promote to lead agent (only for cycles or truly broken DAGs)
                    let promoted = plan.assignments.is_empty() && !plan.use_lead_agent;
                    if promoted {
                        tracing::warn!(
                            dag_error = %e,
                            "Auto-promoting to lead agent: DAG validation failed with no salvage path"
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
                last_error_msg = Some(msg.clone());
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
        let system_prompt =
            Self::build_hierarchical_prompt(idle_agents, limits.plan_protocol_v2_enabled);
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
                    predictability_score: obj.get("predictability_score").and_then(|v| v.as_f64()),
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

    /// Lightweight LLM classification: uses a minimal prompt (~200 tokens) to determine
    /// if a message is simple_query or complex_task. Much cheaper than full planning.
    /// Uses `config.triage_model` (e.g. claude-haiku) to keep cost ~10x lower than
    /// the full planner call.
    pub async fn classify_lightweight(
        router: &LlmRouter,
        triage_model: Option<&str>,
        user_message: &str,
        timeout_secs: u64,
    ) -> Result<String, PlanError> {
        let messages = vec![
            ChatMessage::system(
                "Classify the user message. Respond ONLY with a JSON object:\n\
                 {\"classification\": \"simple_query\"} or {\"classification\": \"complex_task\"}\n\
                 simple_query = greetings, questions, conversation.\n\
                 complex_task = multi-step tasks needing agent work.",
            ),
            ChatMessage::user(user_message),
        ];
        let request = RouterRequest {
            model: triage_model.map(|s| s.to_string()),
            messages: Arc::new(messages),
            tools: Arc::new(vec![]),
            temperature: Some(0.0),
            max_tokens: Some(50),
            context: RequestContext::default(),
            tool_choice: None,
            tools_token_estimate: None,
            enable_caching: false,
            thinking: None,
        };
        let deadline = Duration::from_secs(timeout_secs);
        let response = tokio::time::timeout(deadline, router.complete(request))
            .await
            .map_err(|_| PlanError::Timeout(timeout_secs))?
            .map_err(|e| PlanError::LlmError(e.to_string()))?;

        let json_str = extract_json_block(&response.content);
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str)
            && let Some(c) = val.get("classification").and_then(|v| v.as_str())
        {
            return Ok(c.to_string());
        }
        Err(PlanError::MalformedResponse(
            "lightweight classification failed".into(),
        ))
    }
}

#[cfg(test)]
mod tests;
