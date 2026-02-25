//! Dispatcher analysis layer: produces a `DispatchDecision` that captures
//! the routing analysis before execution begins.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::orchestrator::task_planner::TaskPlan;

/// The execution mode selected by the dispatcher.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DispatchMode {
    LeadAgent,
    DagParallel,
    SequentialPipeline,
}

impl std::fmt::Display for DispatchMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LeadAgent => write!(f, "lead_agent"),
            Self::DagParallel => write!(f, "dag_parallel"),
            Self::SequentialPipeline => write!(f, "sequential_pipeline"),
        }
    }
}

/// Why a particular execution mode was chosen.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DecisionReason {
    /// Planner explicitly set use_lead_agent=true or provided a DAG.
    PlannerExplicit,
    /// V2 execution_mode field was present and used.
    ExecutionModeField,
    /// No assignments and no DAG — fell back to lead agent.
    EmptyAssignmentsFallback,
    /// Heuristic skill matching chose this path.
    HeuristicFallback,
    /// Heuristic skill matching was attempted but failed (no matching agents found).
    HeuristicMatchFailed,
}

impl std::fmt::Display for DecisionReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PlannerExplicit => write!(f, "planner_explicit"),
            Self::ExecutionModeField => write!(f, "execution_mode_field"),
            Self::EmptyAssignmentsFallback => write!(f, "empty_assignments_fallback"),
            Self::HeuristicFallback => write!(f, "heuristic_fallback"),
            Self::HeuristicMatchFailed => write!(f, "heuristic_match_failed"),
        }
    }
}

/// Captures the dispatcher's analysis before execution begins.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DispatchDecision {
    pub mode: DispatchMode,
    pub reason: DecisionReason,
    pub agent_count: usize,
    pub dag_node_count: Option<usize>,
    pub predictability_score: Option<f64>,
    pub planner_requested_mode: Option<String>,
    pub error_message: Option<String>,
    pub timestamp: DateTime<Utc>,
}

/// Analyze a TaskPlan and produce a DispatchDecision without executing anything.
///
/// This mirrors the decision tree in `dispatch_planned()` but returns analysis only.
pub fn analyze_plan(plan: &TaskPlan) -> DispatchDecision {
    let now = Utc::now();

    // 0. V2 protocol: execution_mode field takes priority when present
    if let Some(ref em) = plan.execution_mode {
        let mode = match em.as_str() {
            "lead_agent" => Some(DispatchMode::LeadAgent),
            "dag" if plan.dag.is_some() => Some(DispatchMode::DagParallel),
            "pipeline" if !plan.assignments.is_empty() => Some(DispatchMode::SequentialPipeline),
            _ => None,
        };
        if let Some(mode) = mode {
            return DispatchDecision {
                mode,
                reason: DecisionReason::ExecutionModeField,
                agent_count: plan
                    .dag
                    .as_ref()
                    .map(|d| d.nodes.len())
                    .unwrap_or(plan.assignments.len()),
                dag_node_count: plan.dag.as_ref().map(|d| d.nodes.len()),
                predictability_score: plan.predictability_score,
                planner_requested_mode: plan.execution_mode.clone(),
                error_message: None,
                timestamp: now,
            };
        }
    }

    // 1. Lead Agent path: use_lead_agent=true
    if plan.use_lead_agent {
        return DispatchDecision {
            mode: DispatchMode::LeadAgent,
            reason: DecisionReason::PlannerExplicit,
            agent_count: 0,
            dag_node_count: None,
            predictability_score: plan.predictability_score,
            planner_requested_mode: plan.execution_mode.clone(),
            error_message: None,
            timestamp: now,
        };
    }

    // 2. DAG path: dag is present
    if let Some(ref dag) = plan.dag {
        return DispatchDecision {
            mode: DispatchMode::DagParallel,
            reason: DecisionReason::PlannerExplicit,
            agent_count: dag.nodes.len(),
            dag_node_count: Some(dag.nodes.len()),
            predictability_score: plan.predictability_score,
            planner_requested_mode: plan.execution_mode.clone(),
            error_message: None,
            timestamp: now,
        };
    }

    // 3. Empty assignments — fallback to lead agent
    if plan.assignments.is_empty() {
        return DispatchDecision {
            mode: DispatchMode::LeadAgent,
            reason: DecisionReason::EmptyAssignmentsFallback,
            agent_count: 0,
            dag_node_count: None,
            predictability_score: plan.predictability_score,
            planner_requested_mode: plan.execution_mode.clone(),
            error_message: None,
            timestamp: now,
        };
    }

    // 4. Sequential pipeline: assignments provided
    DispatchDecision {
        mode: DispatchMode::SequentialPipeline,
        reason: DecisionReason::PlannerExplicit,
        agent_count: plan.assignments.len(),
        dag_node_count: None,
        predictability_score: plan.predictability_score,
        planner_requested_mode: plan.execution_mode.clone(),
        error_message: None,
        timestamp: now,
    }
}

#[cfg(test)]
mod tests;
