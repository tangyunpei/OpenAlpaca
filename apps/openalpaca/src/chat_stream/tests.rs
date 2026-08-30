use super::*;

#[test]
fn test_format_token_count() {
    assert_eq!(format_token_count(0), "0");
    assert_eq!(format_token_count(999), "999");
    assert_eq!(format_token_count(1000), "1.0K");
    assert_eq!(format_token_count(1500), "1.5K");
    assert_eq!(format_token_count(1_000_000), "1.0M");
    assert_eq!(format_token_count(2_500_000), "2.5M");
}

#[test]
fn test_format_usage_line() {
    let info = UsageInfo {
        model: "gpt-4".to_string(),
        tokens_in: 1500,
        tokens_out: 500,
        duration_ms: 1234,
    };
    assert_eq!(
        format_usage_line(&info),
        "[gpt-4 | 1.5K in | 500 out | 1234ms]"
    );
}

#[test]
fn test_process_sse_event_thinking() {
    let mut state = SseState {
        usage: None,
        had_delta: false,
        delegation: None,
    };
    let result = process_sse_event("event: thinking\ndata: {}", false, &mut state);
    assert!(result.is_ok());
    assert!(state.usage.is_none());
    assert!(!state.had_delta);
}

#[test]
fn test_process_sse_event_delta() {
    let mut state = SseState {
        usage: None,
        had_delta: false,
        delegation: None,
    };
    let result = process_sse_event(
        "event: delta\ndata: {\"content\":\"hello\"}",
        false,
        &mut state,
    );
    assert!(result.is_ok());
    assert!(state.usage.is_none());
    assert!(state.had_delta);
}

#[test]
fn test_process_sse_event_done_with_prior_delta() {
    let mut state = SseState {
        usage: None,
        had_delta: true,
        delegation: None,
    };
    let result = process_sse_event(
        "event: done\ndata: {\"content\":\"hello\",\"model\":\"gpt-4\",\"tokens_in\":10,\"tokens_out\":20,\"duration_ms\":100}",
        false,
        &mut state,
    );
    assert!(result.is_ok());
    let usage = state.usage.as_ref().unwrap();
    assert_eq!(usage.model, "gpt-4");
    assert_eq!(usage.tokens_in, 10);
    assert_eq!(usage.tokens_out, 20);
    assert_eq!(usage.duration_ms, 100);
}

#[test]
fn test_process_sse_event_done_no_prior_delta() {
    let mut state = SseState {
        usage: None,
        had_delta: false,
        delegation: None,
    };
    let result = process_sse_event(
        "event: done\ndata: {\"content\":\"response text\",\"model\":\"gpt-4\",\"tokens_in\":5,\"tokens_out\":10,\"duration_ms\":50}",
        false,
        &mut state,
    );
    assert!(result.is_ok());
    let usage = state.usage.as_ref().unwrap();
    assert_eq!(usage.model, "gpt-4");
}

#[test]
fn test_process_sse_event_error() {
    let mut state = SseState {
        usage: None,
        had_delta: false,
        delegation: None,
    };
    let result = process_sse_event(
        "event: error\ndata: {\"message\":\"something failed\"}",
        false,
        &mut state,
    );
    assert!(result.is_ok());
    assert!(state.usage.is_none());
}

#[test]
fn test_process_sse_event_unknown() {
    let mut state = SseState {
        usage: None,
        had_delta: false,
        delegation: None,
    };
    let result = process_sse_event("event: unknown\ndata: {}", false, &mut state);
    assert!(result.is_ok());
    assert!(state.usage.is_none());
}

#[test]
fn test_find_event_boundary() {
    assert_eq!(find_event_boundary("event: delta\n\n"), Some(12));
    assert_eq!(find_event_boundary("event: delta\r\n\r\n"), Some(12));
    assert_eq!(find_event_boundary("no boundary"), None);
}

#[test]
fn test_process_sse_event_done_with_delegation() {
    let mut state = SseState {
        usage: None,
        had_delta: true,
        delegation: None,
    };
    let result = process_sse_event(
        "event: done\ndata: {\"content\":\"I've kicked off a task\",\"model\":\"router\",\"tokens_in\":0,\"tokens_out\":0,\"duration_ms\":50,\"delegation\":{\"task_id\":\"task-123\",\"title\":\"Research quantum computing\"}}",
        false,
        &mut state,
    );
    assert!(result.is_ok());
    let delegation = state.delegation.as_ref().unwrap();
    assert_eq!(delegation.task_id, "task-123");
    assert_eq!(delegation.title, "Research quantum computing");
}

#[test]
fn test_process_sse_event_done_without_delegation() {
    let mut state = SseState {
        usage: None,
        had_delta: true,
        delegation: None,
    };
    let result = process_sse_event(
        "event: done\ndata: {\"content\":\"a normal reply\",\"model\":\"gpt-4\",\"tokens_in\":10,\"tokens_out\":20,\"duration_ms\":100}",
        false,
        &mut state,
    );
    assert!(result.is_ok());
    assert!(state.delegation.is_none());
}

#[test]
fn test_process_sse_event_done_with_malformed_delegation() {
    let mut state = SseState {
        usage: None,
        had_delta: true,
        delegation: None,
    };
    // Missing required "title" field — must be ignored, not crash
    let result = process_sse_event(
        "event: done\ndata: {\"content\":\"x\",\"model\":\"m\",\"tokens_in\":0,\"tokens_out\":0,\"duration_ms\":1,\"delegation\":{\"task_id\":\"task-123\"}}",
        false,
        &mut state,
    );
    assert!(result.is_ok());
    assert!(state.delegation.is_none());
}

#[test]
fn test_stream_result_usage() {
    let info = UsageInfo {
        model: "gpt-4".to_string(),
        tokens_in: 10,
        tokens_out: 20,
        duration_ms: 100,
    };
    let result = StreamResult::Response(Some(info));
    assert!(result.usage().is_some());
    assert_eq!(result.usage().unwrap().model, "gpt-4");

    let result = StreamResult::Response(None);
    assert!(result.usage().is_none());

    let result = StreamResult::Delegation {
        usage: Some(UsageInfo {
            model: "router".to_string(),
            tokens_in: 0,
            tokens_out: 0,
            duration_ms: 50,
        }),
        delegation: DelegationInfo {
            task_id: "task-1".to_string(),
            title: "test".to_string(),
        },
    };
    assert!(result.usage().is_some());
}
