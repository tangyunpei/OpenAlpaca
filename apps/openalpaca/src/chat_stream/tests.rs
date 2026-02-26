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
        done_content: None,
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
        done_content: None,
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
        done_content: None,
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
        done_content: None,
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
        done_content: None,
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
        done_content: None,
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
fn test_is_delegation_true() {
    let usage = Some(UsageInfo {
        model: "router".to_string(),
        tokens_in: 0,
        tokens_out: 0,
        duration_ms: 50,
    });
    assert!(is_delegation(
        &usage,
        "I've created a task for the research team."
    ));
}

#[test]
fn test_is_delegation_false_nonzero_tokens() {
    let usage = Some(UsageInfo {
        model: "gpt-4".to_string(),
        tokens_in: 100,
        tokens_out: 200,
        duration_ms: 500,
    });
    assert!(!is_delegation(
        &usage,
        "I've created a task for the research team."
    ));
}

#[test]
fn test_is_delegation_false_no_marker() {
    let usage = Some(UsageInfo {
        model: "router".to_string(),
        tokens_in: 0,
        tokens_out: 0,
        duration_ms: 50,
    });
    assert!(!is_delegation(&usage, "Here is a normal response."));
}

#[test]
fn test_parse_task_title_found() {
    let content =
        "I've created a task for this.\nTask: Research quantum computing\nYou'll see results soon.";
    assert_eq!(
        parse_task_title(content),
        Some("Research quantum computing".to_string())
    );
}

#[test]
fn test_parse_task_title_at_end() {
    let content = "I've created a task.\nTask: Deploy the service";
    assert_eq!(
        parse_task_title(content),
        Some("Deploy the service".to_string())
    );
}

#[test]
fn test_parse_task_title_not_found() {
    let content = "No task here, just a normal response.";
    assert_eq!(parse_task_title(content), None);
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
        task_title: "test".to_string(),
    };
    assert!(result.usage().is_some());
}
