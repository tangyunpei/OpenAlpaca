//! LLM-based task planning: replaces keyword heuristics with a single LLM call
//! that classifies intent, generates a title, and assigns agents.
//!
//! Supports hierarchical planning: for complex tasks, the planner can decompose
//! the objective into a DAG of sub-tasks with dependencies.

use crate::agent::subagent::SubAgent;
use openalpaca_llm::{ChatMessage, LlmRouter, RequestContext, RouterRequest};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};
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

        if self.nodes.len() > 8 {
            return Err(format!(
                "DAG has {} nodes (max 8)",
                self.nodes.len()
            ));
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
                    && n.depends_on.iter().all(|dep| completed.contains(dep.as_str()))
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

/// Render the "## Available Agents" prompt section into `out`.
fn format_agent_list(out: &mut String, agents: &[SubAgent]) {
    out.push_str("## Available Agents\n");
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
                "- ID: \"{}\", Name: \"{}\", Description: \"{}\", Skills: {}\n",
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
}

/// Build the message list for a planning LLM call.
///
/// Constructs: `[system_prompt, optional summary, optional active_tasks, history_tail…, user_message]`
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
        messages.push(ChatMessage::system(&format!(
            "### SESSION SUMMARY ###\n{}",
            capped
        )));
    }

    if let Some(tasks_block) = active_tasks_block {
        messages.push(ChatMessage::system(tasks_block));
    }

    messages.extend_from_slice(history_tail);
    messages.push(ChatMessage::user(user_message));
    messages
}

/// Internal planning mode — captures the differences between flat and hierarchical.
enum PlanMode<'a> {
    Flat,
    Hierarchical { idle_agents: &'a [SubAgent] },
}

/// Shared retry loop for both flat and hierarchical planning.
///
/// Builds a `RouterRequest` per attempt, calls the router with a timeout,
/// parses the response, and optionally validates the DAG (hierarchical only).
async fn plan_inner(
    router: &LlmRouter,
    messages: Vec<ChatMessage>,
    limits: PlannerLimits,
    mode: PlanMode<'_>,
) -> Result<TaskPlan, PlanError> {
    let (max_tokens, log_prefix) = match &mode {
        PlanMode::Flat => (512u32, "Plan"),
        PlanMode::Hierarchical { .. } => (2048u32, "Hierarchical plan"),
    };

    let mut last_error = PlanError::MalformedResponse("no attempts made".to_string());
    let deadline = Duration::from_secs(limits.timeout_secs);

    for attempt in 0..=limits.max_retries {
        let request = RouterRequest {
            model: None,
            messages: messages.clone(),
            tools: vec![],
            temperature: Some(0.0),
            max_tokens: Some(max_tokens),
            context: RequestContext::default(),
        };

        let response = tokio::time::timeout(deadline, router.complete(request))
            .await
            .map_err(|_| PlanError::Timeout(limits.timeout_secs))?
            .map_err(|e| PlanError::LlmError(e.to_string()))?;

        match TaskPlanner::parse_response(&response.content) {
            Ok(plan) => {
                if let PlanMode::Hierarchical { idle_agents } = &mode
                    && let Some(ref dag) = plan.dag
                    && let Err(e) = dag.validate(idle_agents)
                {
                    let promoted = plan.assignments.is_empty() && !plan.use_lead_agent;
                    if promoted {
                        tracing::warn!(
                            "DAG validation failed ({e}) and plan has no flat assignments \
                             — auto-promoting to use_lead_agent"
                        );
                    } else {
                        tracing::warn!(
                            "DAG validation failed: {e}, falling back to flat assignments"
                        );
                    }
                    return Ok(TaskPlan {
                        dag: None,
                        use_lead_agent: plan.use_lead_agent || promoted,
                        ..plan
                    });
                }
                return Ok(plan);
            }
            Err(PlanError::MalformedResponse(msg)) => {
                tracing::warn!(
                    "{log_prefix} attempt {}/{} returned malformed response: {msg}",
                    attempt + 1,
                    limits.max_retries + 1,
                );
                last_error = PlanError::MalformedResponse(msg);
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
}

pub struct TaskPlanner;

impl TaskPlanner {
    /// Call the LLM to classify a user message and optionally assign agents.
    ///
    /// Retries up to `limits.max_retries` times on malformed responses before
    /// propagating the error. LLM transport and timeout errors are returned
    /// immediately without retrying.
    pub async fn plan(
        router: &LlmRouter,
        user_message: &str,
        idle_agents: &[SubAgent],
        history: &[ChatMessage],
        session_summary: Option<&str>,
        active_tasks_block: Option<&str>,
        limits: PlannerLimits,
    ) -> Result<TaskPlan, PlanError> {
        let system_prompt = Self::build_system_prompt(idle_agents);
        let messages = build_messages(
            &system_prompt,
            user_message,
            history,
            session_summary,
            active_tasks_block,
        );
        plan_inner(router, messages, limits, PlanMode::Flat).await
    }

    /// Hierarchical planning: decompose a complex task into a DAG of sub-tasks.
    /// Falls back to flat assignment if DAG planning fails or returns simple_query.
    ///
    /// Retries up to `limits.max_retries` times on malformed responses. DAG validation
    /// failures do NOT trigger retries — the plan is returned with `dag: None`
    /// (existing fallback behaviour). Timeout and LLM errors are returned immediately.
    pub async fn plan_hierarchical(
        router: &LlmRouter,
        user_message: &str,
        idle_agents: &[SubAgent],
        history: &[ChatMessage],
        session_summary: Option<&str>,
        active_tasks_block: Option<&str>,
        limits: PlannerLimits,
    ) -> Result<TaskPlan, PlanError> {
        let system_prompt = Self::build_hierarchical_prompt(idle_agents);
        let messages = build_messages(
            &system_prompt,
            user_message,
            history,
            session_summary,
            active_tasks_block,
        );
        plan_inner(router, messages, limits, PlanMode::Hierarchical { idle_agents }).await
    }

    /// Build the hierarchical planning prompt with DAG support.
    fn build_hierarchical_prompt(idle_agents: &[SubAgent]) -> String {
        let mut prompt = String::from(
            "You are a task planner for OpenAlpaca. Classify the user message and, \
             for complex tasks, decompose into a DAG of sub-tasks.\n\n",
        );

        format_agent_list(&mut prompt, idle_agents);

        prompt.push_str(
            r#"
## Response Format
Respond with ONLY a single JSON object. No markdown, no explanation, no other text.

For simple queries:
{"classification": "simple_query", "title": null, "assignments": [], "reasoning": "...", "dag": null, "use_lead_agent": false}

For complex tasks that are dynamic, exploratory, or require adaptive decomposition, use the lead agent (PREFERRED for most complex tasks):
{"classification": "complex_task", "title": "...", "assignments": [], "reasoning": "...", "dag": null, "use_lead_agent": true}

For complex tasks where ALL steps are known upfront and predictable, provide a DAG of sub-tasks:
{"classification": "complex_task", "title": "...", "assignments": [], "reasoning": "...", "dag": {"nodes": [
  {"node_id": "node_1", "title": "Research topic", "description": "Search for...", "agent_id": "...", "agent_name": "...", "depends_on": [], "workspace_keys": [], "output_key": "research_results"},
  {"node_id": "node_2", "title": "Write summary", "description": "Using the research...", "agent_id": "...", "agent_name": "...", "depends_on": ["node_1"], "workspace_keys": ["research_results"], "output_key": "summary"}
]}, "use_lead_agent": false}

## When to use `use_lead_agent: true`
Use the lead agent when the task is open-ended or adaptive:
- **Research & analysis**: "Find the best approach for X" — requires evaluating options
- **Debugging**: "Fix the failing tests" — requires iterating until resolved
- **Creative exploration**: "Design a logo concept" — requires back-and-forth refinement
- The number of sub-tasks cannot be determined before starting
- Results from one step change what steps come next

## When to use a DAG (`dag` with nodes, `use_lead_agent: false`)
Use a DAG when all steps are known upfront:
- **Translation**: "Translate this into 5 languages" — each language is a known, independent node
- **Pipeline**: "Read file X, then summarize it, then email the summary" — fixed sequence
- **Batch processing**: "Run these 3 analyses on the dataset" — parallel independent tasks
- All sub-tasks and their dependencies can be enumerated now
- No step's output changes what steps exist

## Grey area — default to lead agent
If unsure whether the task is predictable or exploratory, prefer `use_lead_agent: true`.
A lead agent can always execute a simple plan, but a DAG cannot adapt if the plan is wrong.

## DAG Rules
- Each node is a sub-task assigned to one agent
- `depends_on`: list of node_ids that must complete before this node starts
- Nodes with no dependencies can run in parallel
- `workspace_keys`: workspace entries this node should read (from other nodes' output_key)
- `output_key`: the workspace key where this node writes its result
- 2-8 nodes max
- Use exact agent_id values from the list above
- Decompose into distinct stages requiring different skills
- Express parallelism: if two tasks are independent, give them no shared dependencies
- Simple queries (greetings, short phrases) should still be "simple_query"

## CRITICAL: complex_task MUST have an execution path
When you classify a message as "complex_task", you MUST provide exactly one of:
1. `use_lead_agent: true` — for exploratory, research, or dynamic tasks (PREFERRED default)
2. `dag` with nodes — for tasks with known, predictable steps
3. `assignments` with agent list — for simple sequential pipelines
Returning "complex_task" with empty assignments, no DAG, and use_lead_agent=false is INVALID.
If in doubt, always set `use_lead_agent: true`.
"#,
        );

        prompt
    }

    /// Build the system prompt listing available agents.
    fn build_system_prompt(idle_agents: &[SubAgent]) -> String {
        let mut prompt = String::from(
            "You are a task router for OpenAlpaca. Classify the user message and, if it requires work, assign agents.\n\n",
        );

        format_agent_list(&mut prompt, idle_agents);

        prompt.push_str(
            r#"
## Response Format
Respond with ONLY a single JSON object. No markdown, no explanation, no other text.
The JSON object MUST contain exactly these four keys: "classification", "title", "assignments", "reasoning".
Do NOT include keys like "available_agents" or repeat the agent list.

Simple query example:
{"classification": "simple_query", "title": null, "assignments": [], "reasoning": "This is a casual greeting"}

Complex task example:
{"classification": "complex_task", "title": "Research Rust async patterns", "assignments": [{"agent_id": "...", "agent_name": "...", "role_description": "...", "matched_skills": ["..."]}], "reasoning": "User needs research done"}

## Rules
- "classification" MUST be either "simple_query" or "complex_task"
- Use exact agent_id values from the list above
- Title: imperative, max 50 chars (e.g. "Research Rust async patterns")
- Only classify as complex_task if agent work is needed
- **Agents run as a sequential pipeline**: agent 1 runs first, then agent 2 receives agent 1's output as context, and so on. Order matters — list them in execution order.
- Use multiple agents when the task has distinct stages requiring different skills (e.g. agent with file_read reads a file → agent with text_generate writes a polished summary using the file content).
- Use a single agent when one agent can handle the entire task alone (e.g. a file_read agent can also summarize what it reads via its LLM).
- Casual messages, greetings, short phrases, numbers, or anything that doesn't require agent tools should be "simple_query".
- If an active task already covers the user's request, classify as "simple_query" and explain the existing task in your reasoning.
"#,
        );

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

        // Safety net: if the LLM returned complex_task but provided no execution
        // path (no assignments, no DAG, no lead_agent), auto-promote to lead_agent
        // instead of letting dispatch_planned() fail with "No agents assigned".
        if plan.classification == "complex_task"
            && plan.assignments.is_empty()
            && plan.dag.is_none()
            && !plan.use_lead_agent
        {
            tracing::warn!(
                "LLM planner returned complex_task with no assignments, no DAG, \
                 and use_lead_agent=false — auto-promoting to use_lead_agent. Reasoning: {:?}",
                plan.reasoning
            );
            return Ok(TaskPlan {
                use_lead_agent: true,
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
        assert_eq!(plan.assignments[0].matched_skills, vec!["web_search", "summarize"]);
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
    fn test_build_system_prompt_with_agents() {
        use crate::agent::subagent::{
            AgentConstraints, AgentLlmConfig, AgentPreset, AgentStatus, Skill,
        };

        let agents = vec![SubAgent {
            id: "researcher-01".to_string(),
            template_id: "researcher-01".to_string(),
            name: "Researcher".to_string(),
            description: Some("Research agent".to_string()),
            icon: None,
            status: AgentStatus::Idle,
            current_task: None,
            skills: vec![
                Skill {
                    name: "web_search".to_string(),
                    category: "research".to_string(),
                    proficiency: 0.9,
                },
                Skill {
                    name: "summarize".to_string(),
                    category: "research".to_string(),
                    proficiency: 0.8,
                },
            ],
            preset: AgentPreset::default(),
            constraints: AgentConstraints::default(),
            llm_config: AgentLlmConfig::default(),
        }];

        let prompt = TaskPlanner::build_system_prompt(&agents);
        assert!(prompt.contains("researcher-01"));
        assert!(prompt.contains("Researcher"));
        assert!(prompt.contains("Research agent"));
        assert!(prompt.contains("web_search (0.9)"));
        assert!(prompt.contains("summarize (0.8)"));
        assert!(prompt.contains("If an active task already covers"));
    }

    #[test]
    fn test_build_system_prompt_no_agents() {
        let prompt = TaskPlanner::build_system_prompt(&[]);
        assert!(prompt.contains("No agents are currently available"));
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

    use crate::agent::subagent::{
        AgentConstraints, AgentLlmConfig, AgentPreset, AgentStatus,
    };

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
            nodes: vec![make_dag_node("n1", "a1", &["nonexistent"])],
        };
        let agents = vec![make_agent("a1")];
        let err = dag.validate(&agents).unwrap_err();
        assert!(err.contains("unknown node"));
    }

    #[test]
    fn test_dag_validate_unknown_agent() {
        let dag = TaskDag {
            nodes: vec![make_dag_node("n1", "ghost_agent", &[])],
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
        let ready: Vec<&str> = dag.ready_nodes().iter().map(|n| n.node_id.as_str()).collect();
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
        let prompt = TaskPlanner::build_hierarchical_prompt(&agents);
        assert!(prompt.contains("a1"));
        assert!(prompt.contains("DAG Rules"));
        assert!(prompt.contains("depends_on"));
        assert!(prompt.contains("workspace_keys"));
        // Verify concrete examples are present
        assert!(prompt.contains("Translation"));
        assert!(prompt.contains("Debugging"));
        assert!(prompt.contains("Grey area"));
    }

    #[test]
    fn test_build_hierarchical_prompt_no_agents() {
        let prompt = TaskPlanner::build_hierarchical_prompt(&[]);
        assert!(prompt.contains("No agents are currently available"));
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
            nodes: vec![make_dag_node("n1", "a1", &["nonexistent"])],
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
        // use_lead_agent and dag can coexist (lead agent would be preferred)
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
        assert!(plan.dag.is_some());
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
        assert!(plan.use_lead_agent, "Missing use_lead_agent should default to true");
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
        assert!(!plan.use_lead_agent, "Explicit false with DAG should stay false");
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
}
