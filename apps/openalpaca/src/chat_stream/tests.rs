use super::*;

/// The shape every legacy assertion here was written against: no terminal, so
/// nothing is printed before `done`.
fn piped() -> StreamOptions {
    StreamOptions {
        verbose: false,
        tty: false,
    }
}

/// A terminal: deltas and reasoning are shown as they arrive.
fn terminal() -> StreamOptions {
    StreamOptions {
        verbose: false,
        tty: true,
    }
}

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
    let mut state = SseState::default();
    let result = process_sse_event("event: thinking\ndata: {}", &piped(), &mut state);
    assert!(result.is_ok());
    assert!(state.usage.is_none());
    assert!(state.shown.is_empty());
}

#[test]
fn test_process_sse_event_delta() {
    let mut state = SseState::default();
    let result = process_sse_event(
        "event: delta\ndata: {\"content\":\"hello\"}",
        &piped(),
        &mut state,
    );
    assert!(result.is_ok());
    assert!(state.usage.is_none());
    // A pipe prints nothing until `done` (S13), so nothing was shown.
    assert!(state.shown.is_empty());
}

#[test]
fn test_process_sse_event_done_with_prior_delta() {
    let mut state = SseState {
        shown: "hello".to_string(),
        ..SseState::default()
    };
    let result = process_sse_event(
        "event: done\ndata: {\"content\":\"hello\",\"model\":\"gpt-4\",\"tokens_in\":10,\"tokens_out\":20,\"duration_ms\":100}",
        &piped(),
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
    let mut state = SseState::default();
    let result = process_sse_event(
        "event: done\ndata: {\"content\":\"response text\",\"model\":\"gpt-4\",\"tokens_in\":5,\"tokens_out\":10,\"duration_ms\":50}",
        &piped(),
        &mut state,
    );
    assert!(result.is_ok());
    let usage = state.usage.as_ref().unwrap();
    assert_eq!(usage.model, "gpt-4");
}

#[test]
fn test_process_sse_event_error() {
    let mut state = SseState::default();
    let result = process_sse_event(
        "event: error\ndata: {\"message\":\"something failed\"}",
        &piped(),
        &mut state,
    );
    assert!(result.is_ok());
    assert!(state.usage.is_none());
    assert_eq!(state.failure.as_deref(), Some("something failed"));
}

/// L12: the turn failed, and the process has to say so with more than ink.
/// `openalpaca chat --message …` printed `Error: LLM error: …` and exited 0 —
/// on an Ollama-only install that is every turn until a provider is enabled,
/// and no script could tell it from an answer.
#[test]
fn a_failed_turn_is_carried_out_of_the_stream_as_a_failure() {
    let mut state = SseState::default();
    process_sse_event(
        "event: error\ndata: {\"message\":\"LLM error: no routable model\"}",
        &piped(),
        &mut state,
    )
    .expect("an error event is parsed, not refused");

    let result = StreamResult::Failed {
        message: state.failure.clone().expect("the event recorded a failure"),
    };
    assert_eq!(result.failure(), Some("LLM error: no routable model"));
    assert!(result.usage().is_none());
}

/// An `error` event the CLI cannot parse is still a failed turn; the one thing
/// it must not become is a silent success.
#[test]
fn an_unreadable_error_event_still_fails_the_turn() {
    let mut state = SseState::default();
    process_sse_event("event: error\ndata: not-json", &piped(), &mut state).expect("no panic");
    assert_eq!(state.failure.as_deref(), Some("Unknown error"));

    let mut state = SseState::default();
    process_sse_event("event: error\ndata: {\"detail\":\"x\"}", &piped(), &mut state)
        .expect("no panic");
    assert_eq!(state.failure.as_deref(), Some("Unknown error"));
}

/// An answered turn carries no failure — the flag has to discriminate, not
/// just exist.
#[test]
fn an_answered_turn_carries_no_failure() {
    let mut state = SseState::default();
    process_sse_event(
        "event: done\ndata: {\"content\":\"hi\",\"model\":\"m\",\"tokens_in\":1,\"tokens_out\":1,\"duration_ms\":1}",
        &piped(),
        &mut state,
    )
    .expect("done parses");
    assert!(state.failure.is_none());
    assert!(StreamResult::Response(state.usage).failure().is_none());
}

#[test]
fn test_process_sse_event_unknown() {
    let mut state = SseState::default();
    let result = process_sse_event("event: unknown\ndata: {}", &piped(), &mut state);
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
        shown: "hello".to_string(),
        ..SseState::default()
    };
    let result = process_sse_event(
        "event: done\ndata: {\"content\":\"I've kicked off a task\",\"model\":\"router\",\"tokens_in\":0,\"tokens_out\":0,\"duration_ms\":50,\"delegation\":{\"task_id\":\"task-123\",\"title\":\"Research quantum computing\"}}",
        &piped(),
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
        shown: "hello".to_string(),
        ..SseState::default()
    };
    let result = process_sse_event(
        "event: done\ndata: {\"content\":\"a normal reply\",\"model\":\"gpt-4\",\"tokens_in\":10,\"tokens_out\":20,\"duration_ms\":100}",
        &piped(),
        &mut state,
    );
    assert!(result.is_ok());
    assert!(state.delegation.is_none());
}

#[test]
fn test_process_sse_event_done_with_malformed_delegation() {
    let mut state = SseState {
        shown: "hello".to_string(),
        ..SseState::default()
    };
    // Missing required "title" field — must be ignored, not crash
    let result = process_sse_event(
        "event: done\ndata: {\"content\":\"x\",\"model\":\"m\",\"tokens_in\":0,\"tokens_out\":0,\"duration_ms\":1,\"delegation\":{\"task_id\":\"task-123\"}}",
        &piped(),
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

// ── What the terminal ends on (S13) ─────────────────────────────────

/// Deltas that add up to the answer: the terminal already has it, and `done`
/// must not print it a second time.
#[test]
fn a_complete_stream_owes_the_reader_nothing() {
    assert_eq!(reconcile("Paris.", "Paris."), Reconciliation::Nothing);
    // A delegation's `done` carries no content at all.
    assert_eq!(reconcile("working on it", ""), Reconciliation::Nothing);
    assert_eq!(reconcile("", ""), Reconciliation::Nothing);
}

/// A stream that broke mid-way, or a provider that streamed nothing: what was
/// shown is a prefix of the answer, so only the tail is owed.
#[test]
fn a_partial_stream_is_finished_rather_than_repeated() {
    assert_eq!(
        reconcile("The capital of ", "The capital of France is Paris."),
        Reconciliation::Append("France is Paris.".to_string())
    );
    // A pipe shows nothing, so the whole answer is the tail — printed once,
    // with no leading blank line.
    assert_eq!(
        reconcile("", "Paris."),
        Reconciliation::Append("Paris.".to_string())
    );
}

/// A multi-round turn: the model narrated before calling a tool, and the
/// answer that followed is not a continuation of that narration. The terminal
/// ends on the answer, once.
#[test]
fn a_diverged_stream_ends_on_the_authoritative_answer() {
    assert_eq!(
        reconcile("Let me check the file.", "The file lists three connectors."),
        Reconciliation::Redraw("The file lists three connectors.".to_string())
    );
}

/// The bug S13 names: before this, `done.content` was printed **only** when no
/// delta had arrived, so a partial or multi-round stream left the terminal
/// holding a prefix — or a sentence that was never the answer — and the answer
/// itself was never shown.
#[test]
fn a_streamed_turn_still_ends_on_the_answer() {
    colored::control::set_override(false);
    let opts = terminal();
    let mut state = SseState::default();

    process_sse_event(
        "event: delta\ndata: {\"content\":\"The capital of \"}",
        &opts,
        &mut state,
    )
    .expect("a delta is shown");
    assert_eq!(state.shown, "The capital of ");

    process_sse_event(
        "event: done\ndata: {\"content\":\"The capital of France is Paris.\",\"model\":\"m\",\"tokens_in\":1,\"tokens_out\":1,\"duration_ms\":1}",
        &opts,
        &mut state,
    )
    .expect("done reconciles");
    assert_eq!(
        state.shown, "The capital of France is Paris.",
        "the terminal holds the whole answer, exactly once"
    );
}

/// A pipe is somebody else's input: nothing reaches it before `done`, and what
/// `done` writes is the authoritative answer and nothing else — which is the
/// only way to keep that guarantee now that deltas can diverge from it.
#[test]
fn a_piped_turn_sees_only_the_answer() {
    let opts = piped();
    let mut state = SseState::default();

    process_sse_event(
        "event: delta\ndata: {\"content\":\"Let me check the file.\"}",
        &opts,
        &mut state,
    )
    .expect("a delta is swallowed");
    assert!(state.shown.is_empty(), "nothing was written to the pipe");

    process_sse_event(
        "event: done\ndata: {\"content\":\"The file lists three connectors.\",\"model\":\"m\",\"tokens_in\":1,\"tokens_out\":1,\"duration_ms\":1}",
        &opts,
        &mut state,
    )
    .expect("done prints the answer");
    assert_eq!(state.shown, "The file lists three connectors.");
}

// ── Reasoning (S2) ──────────────────────────────────────────────────

/// Dim on a terminal, nothing when piped — and never part of the answer.
#[test]
fn reasoning_is_shown_to_a_terminal_and_withheld_from_a_pipe() {
    assert_eq!(
        reasoning_to_print(true, "the user is asking about"),
        Some("the user is asking about")
    );
    assert_eq!(reasoning_to_print(false, "the user is asking about"), None);
    assert_eq!(reasoning_to_print(true, ""), None, "an empty frame prints nothing");
}

/// The frame the daemon sends is `reasoning` with a `text` field (S2's wire),
/// and it must not be mistaken for the answer: `shown` stays empty, so `done`
/// still owes the reader the whole of `done.content`.
#[test]
fn reasoning_never_becomes_the_answer() {
    colored::control::set_override(false);
    let opts = terminal();
    let mut state = SseState::default();

    process_sse_event(
        "event: reasoning\ndata: {\"text\":\"they want the capital\"}",
        &opts,
        &mut state,
    )
    .expect("a reasoning frame is rendered");
    assert!(
        state.shown.is_empty(),
        "reasoning is not the answer and is never counted as shown"
    );
    assert!(state.reasoning_open, "the run is open on the current line");

    process_sse_event(
        "event: done\ndata: {\"content\":\"Paris.\",\"model\":\"m\",\"tokens_in\":1,\"tokens_out\":1,\"duration_ms\":1}",
        &opts,
        &mut state,
    )
    .expect("done prints the answer");
    assert_eq!(state.shown, "Paris.");
    assert!(!state.reasoning_open, "the answer closed the reasoning run");
}

/// A `reasoning` frame on a piped run writes nothing at all — not even the
/// newline that separates a terminal's reasoning from its answer.
#[test]
fn a_piped_turn_sees_no_reasoning() {
    let opts = piped();
    let mut state = SseState::default();
    process_sse_event(
        "event: reasoning\ndata: {\"text\":\"they want the capital\"}",
        &opts,
        &mut state,
    )
    .expect("no panic");
    assert!(state.shown.is_empty());
    assert!(!state.reasoning_open);
}

/// V7: `done` and `error` are terminal, and the reader must know it.
///
/// The daemon keeps the SSE open for five seconds after the last frame so a
/// late subscriber can still read the turn; a reader that stops only when the
/// socket closes therefore idles through all five on every one-shot.
#[test]
fn a_terminal_frame_ends_the_turn_and_an_ordinary_one_does_not() {
    let mut state = SseState::default();
    process_sse_event(
        "event: delta\ndata: {\"content\":\"hi\"}",
        &piped(),
        &mut state,
    )
    .expect("a delta is read");
    assert!(!state.finished, "there is more of this turn to come");

    process_sse_event(
        "event: done\ndata: {\"content\":\"hi\",\"model\":\"m\",\"tokens_in\":1,\"tokens_out\":1,\"duration_ms\":1}",
        &piped(),
        &mut state,
    )
    .expect("done is read");
    assert!(
        state.finished,
        "`done` is the last frame of an answered turn"
    );

    let mut failed = SseState::default();
    process_sse_event(
        "event: error\ndata: {\"message\":\"no routable model\"}",
        &piped(),
        &mut failed,
    )
    .expect("an error is read");
    assert!(
        failed.finished,
        "a turn that errored sends no `done` afterwards"
    );
}

/// The same rule, driven end to end against a socket that stays open exactly
/// the way the daemon's does (V7).
///
/// Before this the reader returned when the server dropped the connection, so
/// `openalpaca chat --message …` printed its answer and then sat for the
/// daemon's whole late-subscriber window — a ~5 s tail on every turn, measured
/// at 9.06 s → 14.06 s in the round-3 acceptance run.
#[tokio::test]
async fn a_one_shot_returns_when_the_turn_is_over_not_when_the_socket_closes() {
    /// Longer than the assertion below by enough that an unfixed reader
    /// cannot pass by luck, shorter than the daemon's real 5 s so the test
    /// does not pay for what it is proving.
    const SERVER_HOLD: std::time::Duration = std::time::Duration::from_secs(3);

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let addr = listener.local_addr().expect("the bound address");
    let server = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().expect("the reader connects");
        // Drain the request line and headers first: a response written into a
        // socket whose request has not been read is what hyper rejects as an
        // unexpected message.
        let mut request = [0u8; 1024];
        let _ = std::io::Read::read(&mut socket, &mut request);
        socket
            .write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n\
event: delta\ndata: {\"content\":\"hi\"}\n\n\
event: done\ndata: {\"content\":\"hi\",\"model\":\"qwen3\",\"tokens_in\":1,\"tokens_out\":2,\"duration_ms\":900}\n\n",
            )
            .expect("the frames are written");
        socket.flush().ok();
        // What `ChatService::send_message` does after the last frame: hold the
        // stream for late subscribers, then drop it.
        std::thread::sleep(SERVER_HOLD);
    });

    let client = DaemonClient::for_tests(&format!("http://{addr}"), "token");
    let response = client
        .get_sse_stream("/v1/chat/stream/stream-1?token=token")
        .await
        .expect("the stream opens");

    let started = std::time::Instant::now();
    let result = stream_sse_events(response, &piped(), &client)
        .await
        .expect("the turn is read");
    let elapsed = started.elapsed();

    assert!(
        matches!(result, StreamResult::Response(Some(_))),
        "the turn answered, with usage"
    );
    assert!(
        elapsed < std::time::Duration::from_millis(1500),
        "the reader returned on `done`, not on the socket closing — took {elapsed:?}"
    );

    server.join().expect("the server thread finishes");
}
