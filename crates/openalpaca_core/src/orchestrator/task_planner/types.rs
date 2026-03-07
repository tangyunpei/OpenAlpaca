//! Data types for task planning: DAG nodes, plans, assignments, errors, and limits.

use crate::agent::subagent::SubAgent;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet, VecDeque};

/// Maximum number of recent history messages to include in planning prompts.
pub(super) const PLANNING_HISTORY_LIMIT: usize = 6;
/// Maximum character length for session summary in planning prompts.
pub(super) const PLANNING_SUMMARY_MAX_CHARS: usize = 500;

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
    pub(super) fn run_kahns(&self) -> (usize, Vec<String>) {
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
    /// When true, the task should be executed by a Lead Agent.
    #[serde(default = "default_use_lead_agent")]
    pub use_lead_agent: bool,
    /// Tracks why auto-promotion to lead agent occurred (observability).
    #[serde(skip)]
    pub auto_promotion_reason: Option<String>,
    /// V2 protocol: explicit execution mode from planner.
    #[serde(default)]
    pub execution_mode: Option<String>,
    /// V2 protocol: planner's confidence that the task has predictable structure (0.0-1.0).
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

/// Runtime limits for a planning call (timeout + retry budget).
#[derive(Debug, Clone, Copy)]
pub struct PlannerLimits {
    pub timeout_secs: u64,
    pub max_retries: usize,
    pub max_tokens: u32,
    /// When true, include execution_mode and predictability_score in the planner prompt.
    pub plan_protocol_v2_enabled: bool,
}
