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

// ── Existing tests ────────────────────────────────────────────────

#[test]
fn test_unified_event_creation() {
    let event = UnifiedEvent::new(
        EventSource::Api {
            request_id: "req-1".into(),
        },
        EventPayload::UserMessage {
            content: "hello".into(),
            attachment_ids: Vec::new(),
        },
    );
    assert!(!event.id.is_nil());
    assert_eq!(
        event.source,
        EventSource::Api {
            request_id: "req-1".into()
        }
    );
}

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

#[test]
fn test_event_payload_serialization_roundtrip() {
    let payload = EventPayload::Command {
        name: "help".into(),
        args: vec!["--verbose".into()],
    };
    let json = serde_json::to_string(&payload).unwrap();
    let deserialized: EventPayload = serde_json::from_str(&json).unwrap();
    assert_eq!(payload, deserialized);
}

#[test]
fn test_unified_event_serialization_roundtrip() {
    let event = UnifiedEvent::new(
        EventSource::Internal,
        EventPayload::StatusChange {
            entity: "agent".into(),
            old: "idle".into(),
            new: "running".into(),
        },
    );
    let json = serde_json::to_string(&event).unwrap();
    let deserialized: UnifiedEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(event.id, deserialized.id);
    assert_eq!(event.source, deserialized.source);
    assert_eq!(event.payload, deserialized.payload);
}
