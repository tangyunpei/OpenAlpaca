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

// ── ChatTarget (plan §5.7) ──────────────────────────────────────────

/// The CLI now sends the project every turn belongs to, exactly as the GUI's
/// header does — §4.7's client story, whose CLI half was never wired.
#[test]
fn a_turn_carries_the_working_directory_as_its_project() {
    let target = ChatTarget::for_workspace(Some("/repo".to_string()));
    assert_eq!(
        target.headers(),
        vec![("x-workspace-path", "/repo".to_string())],
        "the daemon resolves this to a project root itself (R22)"
    );
}

/// R81: a header value cannot carry a non-ASCII byte, so a CJK project path —
/// an ordinary one here — is percent-encoded UTF-8 and the daemon decodes it.
/// Before this it was dropped in transit and the turn ran with no project.
#[test]
fn a_cjk_working_directory_is_sent_as_ascii() {
    let target = ChatTarget::for_workspace(Some("/Users/jun/项目/openalpaca".to_string()));
    let headers = target.headers();
    let (name, value) = headers.first().expect("one header");
    assert_eq!(*name, "x-workspace-path");
    assert_eq!(value, "/Users/jun/%E9%A1%B9%E7%9B%AE/openalpaca");
    assert!(
        value.is_ascii(),
        "a header value the daemon can read at all: {value}"
    );

    // A literal `%` is escaped too, so the daemon's decode gives the path back
    // byte for byte rather than turning `50%20off` into `50 off`.
    assert_eq!(
        ChatTarget::for_workspace(Some("/tmp/50%20off".to_string())).headers()[0].1,
        "/tmp/50%2520off",
    );
}

/// A CWD the CLI could not canonicalize is no project at all: sending a
/// relative or unresolvable path would be resolved against the *daemon's*
/// directory, which is the bug R22 closed.
#[test]
fn a_turn_with_no_resolvable_directory_sends_no_header() {
    let target = ChatTarget::for_workspace(None);
    assert!(target.headers().is_empty());
}

#[test]
fn an_ordinary_turn_names_no_session() {
    let body = ChatTarget::for_workspace(None).body("hello", &[]);
    assert_eq!(body["content"], "hello");
    assert!(
        body.get("session_id").is_none(),
        "an unnamed turn is what lets the daemon open a new conversation when \
         the project changed (R48)"
    );
    assert!(body.get("attachments").is_none());
}

/// `--resume` / `--session` name the conversation, and R49 then makes *its*
/// project govern the turn — which is why the header stays on the request
/// rather than being dropped: an unbound session takes it as a first binding.
#[test]
fn a_resumed_turn_names_its_session_and_keeps_the_project() {
    let target =
        ChatTarget::for_workspace(Some("/repo".to_string())).resuming("sess-1".to_string());
    let body = target.body("hello", &[]);
    assert_eq!(body["session_id"], "sess-1");
    assert_eq!(
        target.headers(),
        vec![("x-workspace-path", "/repo".to_string())]
    );
}

#[test]
fn attachments_ride_along_with_the_session() {
    let attachments = vec![serde_json::json!({ "file_id": "file-1" })];
    let body = ChatTarget::for_workspace(None)
        .resuming("sess-1".to_string())
        .body("look at this", &attachments);
    assert_eq!(body["attachments"][0]["file_id"], "file-1");
    assert_eq!(body["session_id"], "sess-1");
}
