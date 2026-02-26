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
        nodes: vec![make_dag_node("n1", &[]), make_dag_node("n2", &["n1"])],
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
fn test_analyze_v2_execution_mode_field_lead() {
    let mut plan = make_simple_plan();
    plan.execution_mode = Some("lead_agent".to_string());

    let decision = analyze_plan(&plan);
    assert_eq!(decision.mode, DispatchMode::LeadAgent);
    assert_eq!(decision.reason, DecisionReason::ExecutionModeField);
    assert_eq!(
        decision.planner_requested_mode.as_deref(),
        Some("lead_agent")
    );
}

#[test]
fn test_analyze_v2_execution_mode_field_dag() {
    let mut plan = make_simple_plan();
    plan.execution_mode = Some("dag".to_string());
    plan.dag = Some(TaskDag {
        nodes: vec![
            make_dag_node("n1", &[]),
            make_dag_node("n2", &["n1"]),
            make_dag_node("n3", &["n1"]),
        ],
    });

    let decision = analyze_plan(&plan);
    assert_eq!(decision.mode, DispatchMode::DagParallel);
    assert_eq!(decision.reason, DecisionReason::ExecutionModeField);
    assert_eq!(decision.agent_count, 3);
    assert_eq!(decision.dag_node_count, Some(3));
}

#[test]
fn test_analyze_v2_execution_mode_field_pipeline() {
    let mut plan = make_simple_plan();
    plan.execution_mode = Some("pipeline".to_string());
    plan.assignments = vec![PlannedAssignment {
        agent_id: "agent_1".to_string(),
        agent_name: "Agent One".to_string(),
        role_description: "Step 1".to_string(),
        matched_skills: vec!["coding".to_string()],
    }];

    let decision = analyze_plan(&plan);
    assert_eq!(decision.mode, DispatchMode::SequentialPipeline);
    assert_eq!(decision.reason, DecisionReason::ExecutionModeField);
    assert_eq!(decision.agent_count, 1);
}

#[test]
fn test_analyze_v2_unknown_mode_falls_through() {
    let mut plan = make_simple_plan();
    plan.execution_mode = Some("unknown_mode".to_string());
    // No use_lead_agent, no dag, no assignments → EmptyAssignmentsFallback
    let decision = analyze_plan(&plan);
    assert_eq!(decision.mode, DispatchMode::LeadAgent);
    assert_eq!(decision.reason, DecisionReason::EmptyAssignmentsFallback);
}

#[test]
fn test_analyze_v2_dag_mode_without_dag_falls_through() {
    let mut plan = make_simple_plan();
    plan.execution_mode = Some("dag".to_string());
    // execution_mode="dag" but no actual dag → falls through
    let decision = analyze_plan(&plan);
    assert_eq!(decision.mode, DispatchMode::LeadAgent);
    assert_eq!(decision.reason, DecisionReason::EmptyAssignmentsFallback);
}

#[test]
fn test_analyze_v2_pipeline_mode_without_assignments_falls_through() {
    let mut plan = make_simple_plan();
    plan.execution_mode = Some("pipeline".to_string());
    // execution_mode="pipeline" but no assignments → falls through to EmptyAssignmentsFallback
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
        error_message: None,
        timestamp: Utc::now(),
    };
    let json = serde_json::to_string(&decision).unwrap();
    let parsed: DispatchDecision = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.mode, DispatchMode::DagParallel);
    assert_eq!(parsed.agent_count, 3);
    assert_eq!(parsed.predictability_score, Some(0.85));
}

#[test]
fn test_heuristic_match_failed_reason_serialization() {
    let decision = DispatchDecision {
        mode: DispatchMode::SequentialPipeline,
        reason: DecisionReason::HeuristicMatchFailed,
        agent_count: 0,
        dag_node_count: None,
        predictability_score: None,
        planner_requested_mode: None,
        error_message: Some("No agents match the required skills".to_string()),
        timestamp: Utc::now(),
    };
    let json = serde_json::to_string(&decision).unwrap();
    assert!(json.contains("heuristic_match_failed"));
    assert!(json.contains("No agents match the required skills"));
    let parsed: DispatchDecision = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.reason, DecisionReason::HeuristicMatchFailed);
    assert!(parsed.error_message.is_some());
}
