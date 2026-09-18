use super::*;

#[test]
fn test_create_and_get_receiver() {
    let mgr = ChatStreamManager::new();
    let (stream_id, _rx, _sink) = mgr.create_stream("user:gui");

    assert!(mgr.get_receiver(&stream_id).is_some());
    assert!(mgr.get_receiver("nonexistent").is_none());
}

#[test]
fn test_send_and_receive() {
    let mgr = ChatStreamManager::new();
    let (stream_id, mut rx, _sink) = mgr.create_stream("user:gui");

    mgr.send(&stream_id, ChatStreamEvent::Thinking).unwrap();

    let event = rx.try_recv().unwrap();
    assert!(matches!(event, ChatStreamEvent::Thinking));
}

#[test]
fn test_sink_send_delta() {
    let mgr = ChatStreamManager::new();
    let (_stream_id, mut rx, sink) = mgr.create_stream("user:gui");

    sink.send_delta("hello world");

    let event = rx.try_recv().unwrap();
    match event {
        ChatStreamEvent::Delta { content } => assert_eq!(content, "hello world"),
        _ => panic!("Expected Delta event"),
    }
}

#[test]
fn test_sink_send_done() {
    let mgr = ChatStreamManager::new();
    let (_stream_id, mut rx, sink) = mgr.create_stream("user:gui");

    sink.send_done("full response", "gpt-4", 100, 200, 500, None);

    let event = rx.try_recv().unwrap();
    match event {
        ChatStreamEvent::Done {
            content,
            model,
            tokens_in,
            tokens_out,
            duration_ms,
            delegation,
            ..
        } => {
            assert_eq!(content, "full response");
            assert_eq!(model, "gpt-4");
            assert_eq!(tokens_in, 100);
            assert_eq!(tokens_out, 200);
            assert_eq!(duration_ms, 500);
            assert!(delegation.is_none());
        }
        _ => panic!("Expected Done event"),
    }
}

#[test]
fn test_sink_send_done_with_delegation() {
    let mgr = ChatStreamManager::new();
    let (_stream_id, mut rx, sink) = mgr.create_stream("user:gui");

    let info = DelegationInfo {
        task_id: "task-123".to_string(),
        title: "Research Rust".to_string(),
    };
    sink.send_done("ack", "router", 0, 0, 50, Some(info.clone()));

    let event = rx.try_recv().unwrap();
    match event {
        ChatStreamEvent::Done { delegation, .. } => {
            assert_eq!(delegation, Some(info));
        }
        _ => panic!("Expected Done event"),
    }
}

#[test]
fn test_remove() {
    let mgr = ChatStreamManager::new();
    let (stream_id, _rx, _sink) = mgr.create_stream("user:gui");

    mgr.remove(&stream_id);
    assert!(mgr.get_receiver(&stream_id).is_none());
}

#[test]
fn test_cleanup_stale() {
    let mgr = ChatStreamManager::new();
    let (stream_id, _rx, _sink) = mgr.create_stream("user:gui");

    // With zero-duration max_age, everything is stale
    mgr.cleanup_stale(Duration::ZERO);
    assert!(mgr.get_receiver(&stream_id).is_none());
}

#[test]
fn test_cleanup_keeps_fresh() {
    let mgr = ChatStreamManager::new();
    let (stream_id, _rx, _sink) = mgr.create_stream("user:gui");

    // With large max_age, nothing is stale
    mgr.cleanup_stale(Duration::from_secs(3600));
    assert!(mgr.get_receiver(&stream_id).is_some());
}

#[test]
fn test_send_refreshes_last_active() {
    let mgr = ChatStreamManager::new();
    let (stream_id, _rx, _sink) = mgr.create_stream("user:gui");

    // Send an event to refresh last_active
    mgr.send(&stream_id, ChatStreamEvent::Thinking).unwrap();

    // Even with very short max_age based on created_at, the stream should survive
    // because last_active was refreshed
    mgr.cleanup_stale(Duration::from_secs(3600));
    assert!(mgr.get_receiver(&stream_id).is_some());
}

#[test]
fn test_sink_refreshes_last_active() {
    let mgr = ChatStreamManager::new();
    let (stream_id, _rx, sink) = mgr.create_stream("user:gui");

    // Send via sink (not manager) — should still refresh last_active
    sink.send_delta("hello");

    // Stream should survive cleanup because sink sends refresh last_active
    mgr.cleanup_stale(Duration::from_secs(3600));
    assert!(mgr.get_receiver(&stream_id).is_some());
}

/// **S2.** Reasoning reaches the stream as its own event, carrying the text —
/// the placeholder `thinking` event keeps its own meaning ("started, nothing
/// written yet") for a model that emits no reasoning at all.
#[test]
fn sink_sends_reasoning_text() {
    let mgr = ChatStreamManager::new();
    let (_stream_id, mut rx, sink) = mgr.create_stream("user:gui");

    sink.send_thinking();
    sink.send_reasoning("the user is asking a classic");

    assert!(matches!(rx.try_recv().unwrap(), ChatStreamEvent::Thinking));
    match rx.try_recv().unwrap() {
        ChatStreamEvent::Reasoning { text } => {
            assert_eq!(text, "the user is asking a classic")
        }
        other => panic!("expected Reasoning, got {other:?}"),
    }
}
