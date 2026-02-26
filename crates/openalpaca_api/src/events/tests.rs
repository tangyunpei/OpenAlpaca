use super::*;

#[test]
fn test_unified_event_creation() {
    let event = UnifiedEvent::new(
        EventSource::Cli {
            session_id: "sess-1".into(),
        },
        EventPayload::UserMessage {
            content: "hello".into(),
            attachment_ids: Vec::new(),
        },
    );
    assert!(!event.id.is_nil());
    assert_eq!(
        event.source,
        EventSource::Cli {
            session_id: "sess-1".into()
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
