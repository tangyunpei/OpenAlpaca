use super::*;

fn make_assignments() -> Vec<(String, String, String)> {
    vec![
        (
            "a1".to_string(),
            "Agent A1".to_string(),
            "Researcher".to_string(),
        ),
        (
            "a2".to_string(),
            "Agent A2".to_string(),
            "Writer".to_string(),
        ),
    ]
}

#[test]
fn test_initial_state() {
    let state = TaskState::initial("Test objective", &make_assignments());
    assert_eq!(state.objective, "Test objective");
    assert_eq!(state.steps.len(), 2);
    assert_eq!(state.steps[0].step_order, 0);
    assert_eq!(state.steps[0].agent_id, "a1");
    assert_eq!(state.steps[0].status, "pending");
    assert_eq!(state.steps[1].step_order, 1);
    assert_eq!(state.steps[1].agent_id, "a2");
    assert_eq!(state.constraints.max_agents, 2);
    assert!(state.constraints.pipeline_sequential);
}

#[test]
fn test_mark_step_running() {
    let mut state = TaskState::initial("obj", &make_assignments());
    state.mark_step_running(0);
    assert_eq!(state.steps[0].status, "running");
    assert!(state.steps[0].started_at.is_some());
    assert_eq!(state.steps[1].status, "pending");
}

#[test]
fn test_mark_step_completed() {
    let mut state = TaskState::initial("obj", &make_assignments());
    state.mark_step_running(0);
    state.mark_step_completed(0, "Done successfully");
    assert_eq!(state.steps[0].status, "completed");
    assert_eq!(
        state.steps[0].result_summary.as_deref(),
        Some("Done successfully")
    );
    assert!(state.steps[0].completed_at.is_some());
}

#[test]
fn test_mark_step_completed_caps_summary() {
    let mut state = TaskState::initial("obj", &make_assignments());
    let long_summary = "x".repeat(600);
    state.mark_step_completed(0, &long_summary);
    assert_eq!(state.steps[0].result_summary.as_ref().unwrap().len(), 500);
}

#[test]
fn test_mark_step_failed() {
    let mut state = TaskState::initial("obj", &make_assignments());
    state.mark_step_running(0);
    state.mark_step_failed(0, "Something went wrong");
    assert_eq!(state.steps[0].status, "failed");
    assert_eq!(
        state.steps[0].result_summary.as_deref(),
        Some("Something went wrong")
    );
    assert!(state.steps[0].completed_at.is_some());
}

#[test]
fn test_mark_step_failed_caps_error() {
    let mut state = TaskState::initial("obj", &make_assignments());
    let long_error = "e".repeat(600);
    state.mark_step_failed(0, &long_error);
    assert_eq!(state.steps[0].result_summary.as_ref().unwrap().len(), 500);
}

#[test]
fn test_to_json_roundtrip() {
    let mut state = TaskState::initial("Test roundtrip", &make_assignments());
    state.mark_step_running(0);
    state.mark_step_completed(0, "Step 1 done");

    let json = state.to_json();
    let deserialized: TaskState = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.objective, "Test roundtrip");
    assert_eq!(deserialized.steps.len(), 2);
    assert_eq!(deserialized.steps[0].status, "completed");
    assert_eq!(
        deserialized.steps[0].result_summary.as_deref(),
        Some("Step 1 done")
    );
    assert_eq!(deserialized.steps[1].status, "pending");
}

// ── Workspace tests ──────────────────────────────────────────────

#[test]
fn test_workspace_write_and_read() {
    let mut ws = TaskWorkspace::default();
    ws.write("key1", "hello", "agent_a", WorkspaceEntryType::Text, &[])
        .unwrap();
    ws.write("key2", "world", "agent_b", WorkspaceEntryType::Summary, &[])
        .unwrap();

    let all = ws.read("");
    assert_eq!(all.len(), 2);

    let one = ws.read("key1");
    assert_eq!(one.len(), 1);
    assert_eq!(one[0].content, "hello");
    assert_eq!(one[0].author_agent_id, "agent_a");
}

#[test]
fn test_workspace_upsert() {
    let mut ws = TaskWorkspace::default();
    ws.write("key1", "v1", "agent_a", WorkspaceEntryType::Text, &[])
        .unwrap();
    ws.write("key1", "v2", "agent_b", WorkspaceEntryType::Artifact, &[])
        .unwrap();

    assert_eq!(ws.entries.len(), 1);
    assert_eq!(ws.entries[0].content, "v2");
    assert_eq!(ws.entries[0].author_agent_id, "agent_b");
    assert_eq!(ws.entries[0].entry_type, WorkspaceEntryType::Artifact);
}

#[test]
fn test_workspace_caps_content() {
    let mut ws = TaskWorkspace {
        max_entry_size: 10,
        ..Default::default()
    };
    ws.write(
        "key1",
        "abcdefghijklmnop",
        "agent_a",
        WorkspaceEntryType::Text,
        &[],
    )
    .unwrap();
    assert_eq!(ws.entries[0].content.len(), 10);
}

#[test]
fn test_workspace_max_entries_evicts_oldest() {
    let mut ws = TaskWorkspace {
        max_entries: 2,
        ..Default::default()
    };
    ws.write("k1", "a", "agent", WorkspaceEntryType::Text, &[])
        .unwrap();
    ws.write("k2", "b", "agent", WorkspaceEntryType::Text, &[])
        .unwrap();
    // Third write should evict the oldest entry ("k1") instead of failing
    let result = ws.write("k3", "c", "agent", WorkspaceEntryType::Text, &[]);
    assert!(result.is_ok());
    assert_eq!(ws.entries.len(), 2);
    // k1 should be evicted, k2 and k3 should remain
    assert!(ws.read("k1").is_empty());
    assert!(!ws.read("k2").is_empty());
    assert!(!ws.read("k3").is_empty());
}

#[test]
fn test_workspace_list_keys() {
    let mut ws = TaskWorkspace::default();
    ws.write("research", "data", "agent_a", WorkspaceEntryType::Text, &[])
        .unwrap();
    ws.write(
        "outline",
        "structure",
        "agent_b",
        WorkspaceEntryType::Summary,
        &[],
    )
    .unwrap();

    let keys = ws.list_keys();
    assert_eq!(keys.len(), 2);
    assert_eq!(keys[0].0, "research");
    assert_eq!(keys[1].0, "outline");
}

#[test]
fn test_workspace_format_for_prompt() {
    let mut ws = TaskWorkspace::default();
    ws.write(
        "research",
        "AI data",
        "agent_a",
        WorkspaceEntryType::Text,
        &[],
    )
    .unwrap();
    ws.write(
        "outline",
        "sections",
        "agent_b",
        WorkspaceEntryType::Summary,
        &[],
    )
    .unwrap();

    // Format all
    let all = ws.format_for_prompt(&[]);
    assert!(all.contains("research"));
    assert!(all.contains("outline"));

    // Format filtered
    let filtered = ws.format_for_prompt(&["research".to_string()]);
    assert!(filtered.contains("research"));
    assert!(!filtered.contains("outline"));

    // Format empty workspace
    let empty_ws = TaskWorkspace::default();
    assert!(empty_ws.format_for_prompt(&[]).is_empty());
}

#[test]
fn test_workspace_roundtrip_in_task_state() {
    let mut state = TaskState::initial("Test workspace", &make_assignments());
    state
        .workspace
        .write("key1", "data", "agent", WorkspaceEntryType::Text, &[])
        .unwrap();

    let json = state.to_json();
    let deserialized: TaskState = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.workspace.entries.len(), 1);
    assert_eq!(deserialized.workspace.entries[0].key, "key1");
    assert_eq!(deserialized.workspace.entries[0].content, "data");
}

#[test]
fn test_workspace_eviction_respects_protected_keys() {
    let mut ws = TaskWorkspace {
        max_entries: 2,
        ..Default::default()
    };
    ws.write("k1", "a", "agent", WorkspaceEntryType::Text, &[])
        .unwrap();
    ws.write("k2", "b", "agent", WorkspaceEntryType::Text, &[])
        .unwrap();
    // k1 is protected, so k2 (the oldest unprotected) should be evicted
    let result = ws.write(
        "k3",
        "c",
        "agent",
        WorkspaceEntryType::Text,
        &["k1".to_string()],
    );
    assert!(result.is_ok());
    assert!(!ws.read("k1").is_empty()); // protected — kept
    assert!(ws.read("k2").is_empty()); // evicted
    assert!(!ws.read("k3").is_empty()); // written
}

#[test]
fn test_workspace_eviction_all_protected_returns_error() {
    let mut ws = TaskWorkspace {
        max_entries: 2,
        ..Default::default()
    };
    ws.write("k1", "a", "agent", WorkspaceEntryType::Text, &[])
        .unwrap();
    ws.write("k2", "b", "agent", WorkspaceEntryType::Text, &[])
        .unwrap();
    // Both existing keys are protected — write should fail
    let result = ws.write(
        "k3",
        "c",
        "agent",
        WorkspaceEntryType::Text,
        &["k1".to_string(), "k2".to_string()],
    );
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("all are protected"));
}

#[test]
fn test_backward_compat_no_workspace_field() {
    // Simulate old TaskState JSON without workspace field
    let old_json = r#"{
        "objective": "test",
        "steps": [],
        "constraints": {"max_agents": 1, "pipeline_sequential": true},
        "created_at": "2024-01-01T00:00:00Z",
        "updated_at": "2024-01-01T00:00:00Z"
    }"#;
    let state: TaskState = serde_json::from_str(old_json).unwrap();
    assert_eq!(state.objective, "test");
    assert!(state.workspace.entries.is_empty());
    assert_eq!(state.workspace.max_entries, 50);
}

// ── Artifact pointer and outcome tests ──────────────────────────

#[test]
fn test_step_add_artifact() {
    let mut state = TaskState::initial("obj", &make_assignments());
    state.mark_step_running(0);
    state.add_step_artifact(0, "report.pdf", "Final report");
    assert_eq!(state.steps[0].artifact_pointers.len(), 1);
    // Stored as JSON to avoid delimiter ambiguity
    let stored: serde_json::Value =
        serde_json::from_str(&state.steps[0].artifact_pointers[0]).unwrap();
    assert_eq!(stored["key"], "report.pdf");
    assert_eq!(stored["label"], "Final report");
}

#[test]
fn test_collect_artifacts_from_completed_steps_only() {
    let mut state = TaskState::initial("obj", &make_assignments());
    // Step 0: completed with artifact
    state.mark_step_running(0);
    state.add_step_artifact(0, "data.csv", "Raw data");
    state.mark_step_completed(0, "Gathered data");

    // Step 1: failed with artifact (should NOT be collected)
    state.mark_step_running(1);
    state.add_step_artifact(1, "draft.txt", "Draft");
    state.mark_step_failed(1, "Failed");

    let artifacts = state.collect_artifacts();
    assert_eq!(artifacts.len(), 1);
    assert_eq!(artifacts[0].key, "data.csv");
    assert_eq!(artifacts[0].agent_id, "a1");
    assert_eq!(artifacts[0].step_order, 0);
}

#[test]
fn test_build_outcome_text_only() {
    let mut state = TaskState::initial("obj", &make_assignments());
    state.mark_step_running(0);
    state.mark_step_completed(0, "Analysis complete");
    state.mark_step_running(1);
    state.mark_step_completed(1, "Summary written");

    let outcome = state.build_outcome("fallback", None);
    assert_eq!(outcome.outcome_kind, OutcomeKind::TextOnly);
    assert!(outcome.artifacts.is_empty());
    assert!(outcome.no_artifact_reason.is_some());
    assert!(outcome.summary.contains("Analysis complete"));
    assert!(outcome.summary.contains("Summary written"));
}

#[test]
fn test_build_outcome_artifact_only() {
    let mut state = TaskState::initial("obj", &make_assignments());
    state.mark_step_running(0);
    state.add_step_artifact(0, "output.zip", "Results");
    // Complete with empty summary
    state.mark_step_completed(0, "");

    let outcome = state.build_outcome("", None);
    assert_eq!(outcome.outcome_kind, OutcomeKind::ArtifactOnly);
    assert_eq!(outcome.artifacts.len(), 1);
    assert!(outcome.no_artifact_reason.is_none());
    // Summary should fall back to default
    assert_eq!(outcome.summary, "Task completed.");
}

#[test]
fn test_build_outcome_mixed() {
    let mut state = TaskState::initial("obj", &make_assignments());
    state.mark_step_running(0);
    state.add_step_artifact(0, "report.pdf", "Report");
    state.mark_step_completed(0, "Report generated successfully");

    let outcome = state.build_outcome("fallback", None);
    assert_eq!(outcome.outcome_kind, OutcomeKind::Mixed);
    assert_eq!(outcome.artifacts.len(), 1);
    assert!(outcome.no_artifact_reason.is_none());
    assert!(outcome.summary.contains("Report generated"));
}

#[test]
fn test_build_outcome_failed() {
    let mut state = TaskState::initial("obj", &make_assignments());
    state.mark_step_running(0);
    state.mark_step_failed(0, "Network error");
    state.mark_step_running(1);
    state.mark_step_failed(1, "Dependency failed");

    let outcome = state.build_outcome("All steps failed", None);
    assert_eq!(outcome.outcome_kind, OutcomeKind::Failed);
    assert!(outcome.artifacts.is_empty());
}

#[test]
fn test_build_outcome_partial_failure_not_classified_as_failed() {
    // Some steps completed, some failed — should NOT be "failed"
    let mut state = TaskState::initial("obj", &make_assignments());
    state.mark_step_running(0);
    state.mark_step_completed(0, "Step 1 succeeded");
    state.mark_step_running(1);
    state.mark_step_failed(1, "Step 2 failed");

    let outcome = state.build_outcome("fallback", None);
    // Partial failure with text output = TextOnly (not Failed)
    assert_eq!(outcome.outcome_kind, OutcomeKind::TextOnly);
    assert!(outcome.summary.contains("Step 1 succeeded"));
}

#[test]
fn test_build_outcome_fallback_summary() {
    // No steps completed — should use fallback
    let state = TaskState::initial("obj", &make_assignments());
    let outcome = state.build_outcome("Custom fallback text", None);
    assert_eq!(outcome.summary, "Custom fallback text");
}

#[test]
fn test_build_outcome_empty_fallback_uses_default() {
    let state = TaskState::initial("obj", &make_assignments());
    let outcome = state.build_outcome("", None);
    assert_eq!(outcome.summary, "Task completed.");
}

#[test]
fn test_build_outcome_custom_no_artifact_reason() {
    let mut state = TaskState::initial("obj", &make_assignments());
    state.mark_step_running(0);
    state.mark_step_completed(0, "Analysis done");

    let outcome = state.build_outcome(
        "fallback",
        Some("This was a text-only analysis task.".to_string()),
    );
    assert_eq!(
        outcome.no_artifact_reason.as_deref(),
        Some("This was a text-only analysis task.")
    );
}

#[test]
fn test_task_outcome_serialization_roundtrip() {
    let outcome = TaskOutcome {
        summary: "Task completed".to_string(),
        outcome_kind: OutcomeKind::Mixed,
        artifacts: vec![ArtifactPointer {
            key: "report.pdf".to_string(),
            label: "Final report".to_string(),
            agent_id: "a1".to_string(),
            step_order: 0,
        }],
        no_artifact_reason: None,
    };
    let json = serde_json::to_string(&outcome).unwrap();
    let deserialized: TaskOutcome = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.summary, "Task completed");
    assert_eq!(deserialized.artifacts.len(), 1);
    assert_eq!(deserialized.outcome_kind, OutcomeKind::Mixed);
}
