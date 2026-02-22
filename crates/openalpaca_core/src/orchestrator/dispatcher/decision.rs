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
}

impl std::fmt::Display for DecisionReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PlannerExplicit => write!(f, "planner_explicit"),
            Self::ExecutionModeField => write!(f, "execution_mode_field"),
            Self::EmptyAssignmentsFallback => write!(f, "empty_assignments_fallback"),
            Self::HeuristicFallback => write!(f, "heuristic_fallback"),
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
    pub timestamp: DateTime<Utc>,
}

/// Analyze a TaskPlan and produce a DispatchDecision without executing anything.
///
/// This mirrors the decision tree in `dispatch_planned()` but returns analysis only.
pub fn analyze_plan(plan: &TaskPlan) -> DispatchDecision {
    let now = Utc::now();

    // 1. Lead Agent path: use_lead_agent=true
    if plan.use_lead_agent {
        return DispatchDecision {
            mode: DispatchMode::LeadAgent,
            reason: DecisionReason::PlannerExplicit,
            agent_count: 0,
            dag_node_count: None,
            predictability_score: plan.predictability_score,
            planner_requested_mode: plan.execution_mode.clone(),
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
        timestamp: now,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::orchestrator::task_planner::{DagNode, DagNodeStatus, PlannedAssignment, TaskDag};

    fn make_simple_plan() -> TaskPlan {
        TaskPlan {
            classification: "complex_task".to_string(),
            title: Some("Test task".to_string()),
            assignments: vec![],
            reasoning: None,
            dag: None,
            use_lead_agent: false,
            auto_promotion_reason: None,
            execution_mode: None,
            predictability_score: None,
        }
    }

    fn make_dag_node(id: &str, deps: &[&str]) -> DagNode {
        DagNode {
            node_id: id.to_string(),
            title: format!("Node {id}"),
            description: format!("Description for {id}"),
            agent_id: "test_agent".to_string(),
            agent_name: "Test Agent".to_string(),
            depends_on: deps.iter().map(|d| d.to_string()).collect(),
            status: DagNodeStatus::Pending,
            result_summary: None,
            workspace_keys: vec![],
            output_key: None,
        }
    }

    #[test]
    fn test_analyze_dispatch_lead_agent() {
        let mut plan = make_simple_plan();
        plan.use_lead_agent = true;

        let decision = analyze_plan(&plan);
        assert_eq!(decision.mode, DispatchMode::LeadAgent);
        assert_eq!(decision.reason, DecisionReason::PlannerExplicit);
        assert_eq!(decision.agent_count, 0);
        assert!(decision.dag_node_count.is_none());
    }

    #[test]
    fn test_analyze_dispatch_dag() {
        let mut plan = make_simple_plan();
        plan.dag = Some(TaskDag {
            nodes: vec![
                make_dag_node("n1", &[]),
                make_dag_node("n2", &["n1"]),
            ],
        });

        let decision = analyze_plan(&plan);
        assert_eq!(decision.mode, DispatchMode::DagParallel);
        assert_eq!(decision.reason, DecisionReason::PlannerExplicit);
        assert_eq!(decision.agent_count, 2);
        assert_eq!(decision.dag_node_count, Some(2));
    }

    #[test]
    fn test_analyze_dispatch_pipeline() {
        let mut plan = make_simple_plan();
        plan.assignments = vec![PlannedAssignment {
            agent_id: "agent_1".to_string(),
            agent_name: "Agent One".to_string(),
            role_description: "Step 1".to_string(),
            matched_skills: vec!["coding".to_string()],
        }];

        let decision = analyze_plan(&plan);
        assert_eq!(decision.mode, DispatchMode::SequentialPipeline);
        assert_eq!(decision.reason, DecisionReason::PlannerExplicit);
        assert_eq!(decision.agent_count, 1);
    }

    #[test]
    fn test_analyze_dispatch_empty_fallback() {
        let plan = make_simple_plan();

        let decision = analyze_plan(&plan);
        assert_eq!(decision.mode, DispatchMode::LeadAgent);
        assert_eq!(decision.reason, DecisionReason::EmptyAssignmentsFallback);
    }

    #[test]
    fn test_dispatch_decision_serialization() {
        let decision = DispatchDecision {
            mode: DispatchMode::DagParallel,
            reason: DecisionReason::PlannerExplicit,
            agent_count: 3,
            dag_node_count: Some(3),
            predictability_score: Some(0.85),
            planner_requested_mode: Some("dag".to_string()),
            timestamp: Utc::now(),
        };
        let json = serde_json::to_string(&decision).unwrap();
        let parsed: DispatchDecision = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.mode, DispatchMode::DagParallel);
        assert_eq!(parsed.agent_count, 3);
        assert_eq!(parsed.predictability_score, Some(0.85));
    }
}
