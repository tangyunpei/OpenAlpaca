use super::*;
use chrono::Utc;

// ── ServerEvent::TaskStatus backward-compat tests ─────────────────

#[test]
fn test_task_status_serialization_backward_compat() {
    // When outcome fields are None, they should NOT appear in serialized JSON
    let event = ServerEvent::TaskStatus {
        task_id: "t-1".into(),
        title: "Test Task".into(),
        status: "completed".into(),
        progress_current: None,
        progress_total: None,
        result_summary: Some("Done".into()),
        outcome_kind: None,
        artifact_count: None,
        outcome_summary: None,
        ts: Utc::now(),
        instance_id: "inst-1".into(),
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(!json.contains("outcome_kind"));
    assert!(!json.contains("artifact_count"));
    assert!(!json.contains("outcome_summary"));
    assert!(json.contains("\"result_summary\":\"Done\""));
}

#[test]
fn test_task_status_serialization_with_outcome() {
    // When outcome fields are present, they should appear in the JSON
    let event = ServerEvent::TaskStatus {
        task_id: "t-2".into(),
        title: "Task With Artifacts".into(),
        status: "completed".into(),
        progress_current: None,
        progress_total: None,
        result_summary: Some("All done".into()),
        outcome_kind: Some("mixed".into()),
        artifact_count: Some(3),
        outcome_summary: Some("Produced 3 artifacts".into()),
        ts: Utc::now(),
        instance_id: "inst-1".into(),
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("\"outcome_kind\":\"mixed\""));
    assert!(json.contains("\"artifact_count\":3"));
    assert!(json.contains("\"outcome_summary\":\"Produced 3 artifacts\""));
}

#[test]
fn test_task_status_deserialization_missing_outcome_fields() {
    // A JSON payload from an older daemon that lacks the new fields should
    // still deserialize successfully (forward compatibility).
    let json = r#"{
        "type": "task_status",
        "task_id": "t-3",
        "title": "Old Task",
        "status": "completed",
        "progress_current": null,
        "progress_total": null,
        "result_summary": "Legacy result",
        "ts": "2026-01-15T10:00:00Z",
        "instance_id": "inst-old"
    }"#;
    let event: ServerEvent = serde_json::from_str(json).unwrap();
    match event {
        ServerEvent::TaskStatus {
            task_id,
            outcome_kind,
            artifact_count,
            outcome_summary,
            ..
        } => {
            assert_eq!(task_id, "t-3");
            assert_eq!(outcome_kind, None);
            assert_eq!(artifact_count, None);
            assert_eq!(outcome_summary, None);
        }
        _ => panic!("Expected TaskStatus variant"),
    }
}

// ── ServerEvent workflow-lifecycle tests (Routing V2) ─────────────

#[test]
fn test_workflow_started_serialization() {
    let event = ServerEvent::WorkflowStarted {
        task_id: "t-1".into(),
        lane_key: "junpei:cli".into(),
        title: "Research task".into(),
        ts: Utc::now(),
        instance_id: "inst-1".into(),
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("\"type\":\"workflow_started\""));
    assert!(json.contains("\"task_id\":\"t-1\""));
    assert!(json.contains("\"lane_key\":\"junpei:cli\""));
    assert!(json.contains("\"title\":\"Research task\""));
}

#[test]
fn test_workflow_steered_serialization() {
    let event = ServerEvent::WorkflowSteered {
        task_id: "t-2".into(),
        lane_key: "junpei:cli".into(),
        ts: Utc::now(),
        instance_id: "inst-1".into(),
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("\"type\":\"workflow_steered\""));
    assert!(json.contains("\"task_id\":\"t-2\""));
}

#[test]
fn test_workflow_progress_serialization_roundtrip() {
    let event = ServerEvent::WorkflowProgress {
        task_id: "t-3".into(),
        lane_key: "junpei:cli".into(),
        message: "Halfway through the analysis".into(),
        ts: Utc::now(),
        instance_id: "inst-1".into(),
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("\"type\":\"workflow_progress\""));
    let deserialized: ServerEvent = serde_json::from_str(&json).unwrap();
    match deserialized {
        ServerEvent::WorkflowProgress {
            task_id, message, ..
        } => {
            assert_eq!(task_id, "t-3");
            assert_eq!(message, "Halfway through the analysis");
        }
        _ => panic!("Expected WorkflowProgress variant"),
    }
}

#[test]
fn test_followup_queued_serialization() {
    let event = ServerEvent::FollowupQueued {
        lane_key: "junpei:cli".into(),
        followup_id: 42,
        kind: "followup".into(),
        ts: Utc::now(),
        instance_id: "inst-1".into(),
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("\"type\":\"followup_queued\""));
    assert!(json.contains("\"followup_id\":42"));
    assert!(json.contains("\"kind\":\"followup\""));
}

/// GAP-03's other half. No `kind`: a cancel names the row, and the client
/// already knows what kind it was showing.
#[test]
fn test_followup_cancelled_serialization() {
    let event = ServerEvent::FollowupCancelled {
        lane_key: "junpei:cli".into(),
        followup_id: 42,
        ts: Utc::now(),
        instance_id: "inst-1".into(),
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains("\"type\":\"followup_cancelled\""));
    assert!(json.contains("\"lane_key\":\"junpei:cli\""));
    assert!(json.contains("\"followup_id\":42"));
    assert!(json.contains("\"instance_id\":\"inst-1\""));
}

// ── ServerEvent::ArtifactWritten (plan §4.9) ──────────────────────

/// The wire shape the GUI union mirrors, field for field. `kind` is the
/// snake_case `ArtifactKind` spelling and `path` is the head file's absolute
/// `storage_path`.
#[test]
fn test_artifact_written_wire_shape() {
    let event = ServerEvent::ArtifactWritten {
        artifact_id: "a-1".into(),
        task_id: Some("t-1".into()),
        agent_id: Some("writing_agent".into()),
        name: "01-quarterly-report.md".into(),
        kind: "markdown".into(),
        version: 2,
        path: "/p/.openalpaca/artifacts/2026-09-05-run/01-quarterly-report.md".into(),
        ts: Utc::now(),
        instance_id: "inst-1".into(),
    };
    let value = serde_json::to_value(&event).unwrap();
    assert_eq!(value["type"], "artifact_written");
    assert_eq!(value["artifact_id"], "a-1");
    assert_eq!(value["task_id"], "t-1");
    assert_eq!(value["agent_id"], "writing_agent");
    assert_eq!(value["name"], "01-quarterly-report.md");
    assert_eq!(value["kind"], "markdown");
    assert_eq!(value["version"], 2);
    assert_eq!(
        value["path"],
        "/p/.openalpaca/artifacts/2026-09-05-run/01-quarterly-report.md"
    );
    assert!(value["ts"].is_string());
    assert_eq!(value["instance_id"], "inst-1");
}

/// A loose artifact — a chat turn with no run and no agent attribution —
/// keeps both fields on the wire as `null` rather than dropping them, so the
/// client's union stays one shape.
#[test]
fn test_artifact_written_keeps_task_and_agent_nullable() {
    let event = ServerEvent::ArtifactWritten {
        artifact_id: "a-2".into(),
        task_id: None,
        agent_id: None,
        name: "01-notes.md".into(),
        kind: "markdown".into(),
        version: 1,
        path: "/h/.openalpaca/artifacts/loose/01-notes.md".into(),
        ts: Utc::now(),
        instance_id: "inst-1".into(),
    };
    let value = serde_json::to_value(&event).unwrap();
    assert!(value["task_id"].is_null());
    assert!(value["agent_id"].is_null());

    let round_tripped: ServerEvent =
        serde_json::from_str(&serde_json::to_string(&event).unwrap()).unwrap();
    match round_tripped {
        ServerEvent::ArtifactWritten {
            artifact_id,
            task_id,
            agent_id,
            version,
            ..
        } => {
            assert_eq!(artifact_id, "a-2");
            assert_eq!(task_id, None);
            assert_eq!(agent_id, None);
            assert_eq!(version, 1);
        }
        other => panic!("Expected ArtifactWritten, got {other:?}"),
    }
}

// ── ServerEvent::SubagentSpan (plan Phase 4, GAP-09) ──────────────

/// The open frame: a lane that has started and has not ended. The three
/// closing fields are `null`, not absent, so the client's union is one shape
/// for both halves of a span's life.
#[test]
fn test_subagent_span_open_wire_shape() {
    let event = ServerEvent::SubagentSpan {
        task_id: "t-1".into(),
        span_id: "node-1".into(),
        label: "review·3".into(),
        template_id: "review_agent".into(),
        agent_instance_id: "review_agent::a1b2c3d4".into(),
        state: "running".into(),
        detail: None,
        started_at: "2026-09-05T10:00:00.000Z".into(),
        ended_at: None,
        duration_ms: None,
        output_preview: None,
        ts: Utc::now(),
        instance_id: "inst-1".into(),
    };
    let value = serde_json::to_value(&event).unwrap();
    assert_eq!(value["type"], "subagent_span");
    assert_eq!(value["task_id"], "t-1");
    assert_eq!(value["span_id"], "node-1");
    assert_eq!(value["label"], "review·3");
    assert_eq!(value["template_id"], "review_agent");
    assert_eq!(value["agent_instance_id"], "review_agent::a1b2c3d4");
    assert_eq!(value["state"], "running");
    assert!(value["detail"].is_null());
    assert_eq!(value["started_at"], "2026-09-05T10:00:00.000Z");
    assert!(value["ended_at"].is_null());
    assert!(value["duration_ms"].is_null());
    assert!(value["output_preview"].is_null());
    assert!(value["ts"].is_string());
    assert_eq!(value["instance_id"], "inst-1");
}

/// The close frame round-trips, cancellation included — the state word the
/// timeline draws is carried verbatim, not folded into a success boolean.
#[test]
fn test_subagent_span_close_round_trips_a_cancellation() {
    let event = ServerEvent::SubagentSpan {
        task_id: "t-1".into(),
        span_id: "node-1".into(),
        label: "review·1".into(),
        template_id: "review_agent".into(),
        agent_instance_id: "review_agent::a1b2c3d4".into(),
        state: "cancelled".into(),
        detail: Some("cancelled before starting".into()),
        started_at: "2026-09-05T10:00:00.000Z".into(),
        ended_at: Some("2026-09-05T10:00:04.500Z".into()),
        duration_ms: Some(4_500),
        output_preview: None,
        ts: Utc::now(),
        instance_id: "inst-1".into(),
    };
    let round_tripped: ServerEvent =
        serde_json::from_str(&serde_json::to_string(&event).unwrap()).unwrap();
    match round_tripped {
        ServerEvent::SubagentSpan {
            span_id,
            state,
            detail,
            ended_at,
            duration_ms,
            ..
        } => {
            assert_eq!(span_id, "node-1");
            assert_eq!(state, "cancelled");
            assert_eq!(detail.as_deref(), Some("cancelled before starting"));
            assert_eq!(ended_at.as_deref(), Some("2026-09-05T10:00:04.500Z"));
            assert_eq!(duration_ms, Some(4_500));
        }
        other => panic!("Expected SubagentSpan, got {other:?}"),
    }
}

// ── Existing tests ────────────────────────────────────────────────

#[test]
fn test_event_source_serialization_roundtrip() {
    let source = EventSource::Telegram {
        chat_id: "123".into(),
        user_id: "456".into(),
    };
    let json = serde_json::to_string(&source).unwrap();
    let deserialized: EventSource = serde_json::from_str(&json).unwrap();
    assert_eq!(source, deserialized);
}

