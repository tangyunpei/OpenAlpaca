//! M15: a `tools/call` a `2026-07-28` server answers `input_required`
//! (SEP-2322's multi round-trip request) is driven by the client's own
//! rounds in `McpClient::call_tool_under`, each round a request of its own
//! the server asked for, sent once.
//!
//! Most servers here run in-process over one half of a duplex pipe, served
//! in the `2026-07-28` lifecycle and installed into a real [`McpClient`], so
//! each call goes through `call_tool_within`, the registry's entry point.
//! The last test runs a `2026-07-28` stdio server (a POSIX `sh` loop that
//! answers `server/discover`, leading its own process group as the stubs in
//! `call_tests.rs` do), since only a real child can die mid-call and be
//! respawned by the next call.

use super::*;

use std::collections::BTreeMap;

use rmcp::model::{
    CallToolRequestParams, CallToolResult, ContentBlock, CreateTaskResult, DiscoverResult,
    InputRequest, InputRequiredResult, RequestId, ServerCapabilities, ServerJsonRpcMessage,
    ServerResult, Task, TaskStatus,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

// ============================================================================
// What the servers answer
// ============================================================================

/// The probe's answer: `2026-07-28`, with tools and no list changes, so no
/// `subscriptions/listen` stream is opened beside the calls.
fn discover_result() -> ServerResult {
    let capabilities = ServerCapabilities::builder().enable_tools().build();
    ServerResult::DiscoverResult(
        DiscoverResult::new(vec![ProtocolVersion::LATEST], capabilities)
            .with_server_info(Implementation::new("mrtr-stub", "1.0.0")),
    )
}

/// The final answer: one text block, `ok`.
fn complete() -> ServerResult {
    ServerResult::CallToolResult(CallToolResult::success(vec![ContentBlock::text("ok")]))
}

/// An `input_required` answer carrying only `state`: nothing to answer, the
/// state to echo.
fn state_only(state: &str) -> ServerResult {
    ServerResult::InputRequiredResult(InputRequiredResult::from_request_state(state))
}

/// An `input_required` answer asking, under the key `ask`, for a form
/// elicitation, and carrying `state`.
fn elicitation(state: &str) -> ServerResult {
    let ask: InputRequest = serde_json::from_value(serde_json::json!({
        "method": "elicitation/create",
        "params": {
            "mode": "form",
            "message": "Which file?",
            "requestedSchema": {
                "type": "object",
                "properties": { "path": { "type": "string" } }
            }
        }
    }))
    .expect("an elicitation input request");
    ServerResult::InputRequiredResult(InputRequiredResult::new(
        Some(BTreeMap::from([("ask".to_string(), ask)])),
        Some(state.to_string()),
    ))
}

/// A task in place of the result (SEP-2663): the client polls none (M83).
fn task() -> ServerResult {
    let started = "2026-11-19T00:00:00Z";
    ServerResult::CreateTaskResult(CreateTaskResult::new(Task::new(
        "t1",
        TaskStatus::Working,
        started,
        started,
    )))
}

// ============================================================================
// The in-process server
// ============================================================================

/// What the server answers its `n`th `tools/call` with (1-based).
type Script = Arc<dyn Fn(usize) -> ServerResult + Send + Sync>;

/// What an in-process server saw.
#[derive(Default)]
struct Seen {
    /// Each `tools/call`'s id and parameters, oldest first.
    calls: std::sync::Mutex<Vec<(RequestId, CallToolRequestParams)>>,
    /// The `requestId` of each `notifications/cancelled`, oldest first.
    cancels: std::sync::Mutex<Vec<serde_json::Value>>,
    /// When each `tools/call` arrived, oldest first.
    arrivals: std::sync::Mutex<Vec<tokio::time::Instant>>,
}

impl Seen {
    fn calls(&self) -> Vec<(RequestId, CallToolRequestParams)> {
        self.calls.lock().expect("lock").clone()
    }

    fn cancels(&self) -> Vec<serde_json::Value> {
        self.cancels.lock().expect("lock").clone()
    }
}

/// A `2026-07-28` server over one half of a duplex pipe: it answers the
/// probe and each `tools/call` as `script` says — bar the `silent`th, which
/// it reads and never answers — and records both the calls and any
/// `notifications/cancelled`.
async fn server(
    stream: tokio::io::DuplexStream,
    script: Script,
    silent: Option<usize>,
    seen: Arc<Seen>,
) {
    let (read_half, mut write_half) = tokio::io::split(stream);
    let mut lines = BufReader::new(read_half).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let raw: serde_json::Value = serde_json::from_str(&line).unwrap_or_default();
        if raw["method"] == "notifications/cancelled" {
            let id = raw["params"]["requestId"].clone();
            seen.cancels.lock().expect("lock").push(id);
            continue;
        }
        let Ok(msg) = serde_json::from_str::<rmcp::model::ClientJsonRpcMessage>(&line) else {
            continue;
        };
        let Some((request, id)) = msg.into_request() else {
            continue;
        };
        let reply = match request {
            ClientRequest::DiscoverRequest(_) => discover_result(),
            ClientRequest::CallToolRequest(call) => {
                seen.arrivals
                    .lock()
                    .expect("lock")
                    .push(tokio::time::Instant::now());
                let n = {
                    let mut calls = seen.calls.lock().expect("lock");
                    calls.push((id.clone(), call.params));
                    calls.len()
                };
                if silent == Some(n) {
                    continue;
                }
                script(n)
            }
            _ => continue,
        };
        let mut text = serde_json::to_string(&ServerJsonRpcMessage::response(reply, id))
            .expect("serialize the stub's reply");
        text.push('\n');
        if write_half.write_all(text.as_bytes()).await.is_err() {
            return;
        }
    }
}

/// A client served in the `2026-07-28` lifecycle over [`server`], installed
/// as `connect` installs one, and what its server saw.
async fn modern_client(script: Script) -> (McpClient, Arc<Seen>) {
    modern_client_silent_on(script, None).await
}

/// [`modern_client`], its server never answering the `silent`th call.
async fn modern_client_silent_on(script: Script, silent: Option<usize>) -> (McpClient, Arc<Seen>) {
    let seen = Arc::new(Seen::default());
    let (client_end, server_end) = tokio::io::duplex(64 * 1024);
    tokio::spawn(server(server_end, script, silent, Arc::clone(&seen)));
    let config = McpClientConfig {
        server_name: "mrtr".into(),
        request_timeout: Duration::from_secs(5),
        ..McpClientConfig::default()
    };
    let client = McpClient {
        inner: Arc::new(ClientInner::new(config, ConnectionState::Connecting)),
    };
    let handler = NotifyingHandler {
        server_name: "mrtr".into(),
        tx: client.inner.changes_tx.lock().await.clone(),
        client_info: client.inner.config.client_info.clone(),
    };
    let running = serve(handler, client_end, lifecycle(ProtocolMode::Auto, None))
        .await
        .expect("the modern handshake");
    assert_eq!(
        running
            .peer_info()
            .map(|info| info.protocol_version.to_string())
            .as_deref(),
        Some("2026-07-28"),
        "the stub is served in the 2026-07-28 lifecycle"
    );
    client
        .install_service(running)
        .await
        .expect("install the service");
    (client, seen)
}

/// One call to `echo`, as the registry makes it: under a deadline of the
/// caller's (`call_tool_within`).
async fn call(client: &McpClient) -> Result<CallToolResult, McpError> {
    client
        .call_tool_within(
            "echo",
            serde_json::json!({ "path": "a.txt" }),
            None,
            Duration::from_secs(5),
        )
        .await
}

fn text_of(result: &CallToolResult) -> Vec<String> {
    result
        .content
        .iter()
        .filter_map(|block| block.as_text().map(|t| t.text.clone()))
        .collect()
}

// ============================================================================
// The rounds
// ============================================================================

/// A server that answers the first round with state alone is sent a second
/// request — a new id, the same tool and arguments, the state echoed — and
/// its final answer is the call's result. The second request is the one the
/// server asked for, not a resend of the first.
#[tokio::test]
async fn a_state_only_round_is_a_second_request_and_its_answer_is_the_result() {
    let (client, seen) =
        modern_client(Arc::new(
            |n| {
                if n == 1 { state_only("s1") } else { complete() }
            },
        ))
        .await;

    let result = call(&client)
        .await
        .unwrap_or_else(|e| panic!("the call completes on its second round: {e}"));
    assert_eq!(text_of(&result), ["ok"]);
    let calls = seen.calls();
    assert_eq!(calls.len(), 2, "two rounds, each sent once");
    assert_ne!(calls[0].0, calls[1].0, "each round is a request of its own");
    assert_eq!(calls[0].1.request_state, None);
    assert_eq!(
        calls[1].1.request_state.as_deref(),
        Some("s1"),
        "the state echoed"
    );
    let arrivals = seen.arrivals.lock().expect("lock").clone();
    assert!(
        arrivals[1] - arrivals[0] >= Duration::from_millis(50),
        "a state-only round waits before the next: {:?}",
        arrivals[1] - arrivals[0]
    );
    for (_, params) in &calls {
        assert_eq!(params.name, "echo");
        assert_eq!(
            params.arguments,
            serde_json::json!({ "path": "a.txt" }).as_object().cloned()
        );
        assert_eq!(params.input_responses, None, "nothing was asked");
    }
    client.disconnect().await.expect("disconnect");
}

/// An elicitation the server asks for is answered through the client's
/// handler — `Decline`, since none is served (M84) — and the answer and the
/// state go out on the next round.
#[tokio::test]
async fn an_elicitation_is_answered_decline_on_the_next_round() {
    let (client, seen) = modern_client(Arc::new(|n| {
        if n == 1 {
            elicitation("e1")
        } else {
            complete()
        }
    }))
    .await;

    let result = call(&client)
        .await
        .unwrap_or_else(|e| panic!("the call completes on its second round: {e}"));
    assert_eq!(text_of(&result), ["ok"]);
    let calls = seen.calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].1.input_responses, None);
    assert_eq!(
        serde_json::to_value(&calls[1].1.input_responses).expect("serialize"),
        serde_json::json!({ "ask": { "action": "decline" } }),
        "the elicitation declined, under the server's key"
    );
    assert_eq!(calls[1].1.request_state.as_deref(), Some("e1"));
    client.disconnect().await.expect("disconnect");
}

/// A server that still asks on the tenth round (rmcp's
/// `DEFAULT_MRTR_MAX_ROUNDS`) ends the call with the cap's error, and no
/// eleventh request is sent. Elicitation rounds, so no backoff slows it.
#[tokio::test]
async fn a_server_still_asking_after_ten_rounds_ends_the_call_with_the_caps_error() {
    let (client, seen) = modern_client(Arc::new(|n| elicitation(&format!("e{n}")))).await;

    let outcome = call(&client).await;
    std::assert_matches!(
        outcome,
        Err(McpError::InputRoundsExceeded(10)),
        "the cap's error",
    );
    // Give an eleventh request the time it would take to land.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let calls = seen.calls();
    assert_eq!(
        calls.len(),
        rmcp::model::DEFAULT_MRTR_MAX_ROUNDS,
        "ten rounds, no more"
    );
    let ids: std::collections::HashSet<String> =
        calls.iter().map(|(id, _)| id.to_string()).collect();
    assert_eq!(ids.len(), 10, "each round a request of its own");
    assert_eq!(calls[9].1.request_state.as_deref(), Some("e9"));
    client.disconnect().await.expect("disconnect");
}

/// A task in place of the result is an unexpected response until M83
/// designs the polling: sent once, never resent.
#[tokio::test]
async fn a_task_answer_is_an_unexpected_response_sent_once() {
    let (client, seen) = modern_client(Arc::new(|_| task())).await;

    let outcome = call(&client).await;
    std::assert_matches!(&outcome, Err(McpError::ServerInternal(text)) if text.contains("unexpected response"),
        "an unexpected response",
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(seen.calls().len(), 1, "sent once");
    client.disconnect().await.expect("disconnect");
}

/// A sampling request — a capability the client does not declare — is
/// refused by the handler with a JSON-RPC error, and that error ends the
/// call: no next round is sent.
#[tokio::test]
async fn a_sampling_request_the_client_does_not_serve_ends_the_call_sent_once() {
    let (client, seen) = modern_client(Arc::new(|_| {
        let sample: InputRequest = serde_json::from_value(serde_json::json!({
            "method": "sampling/createMessage",
            "params": {
                "messages": [{ "role": "user", "content": { "type": "text", "text": "hi" } }],
                "maxTokens": 10
            }
        }))
        .expect("a sampling input request");
        ServerResult::InputRequiredResult(InputRequiredResult::new(
            Some(BTreeMap::from([("sample".to_string(), sample)])),
            None,
        ))
    }))
    .await;

    let outcome = call(&client).await;
    std::assert_matches!(
        outcome,
        Err(McpError::JsonRpc { .. }),
        "the handler's refusal",
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(seen.calls().len(), 1, "no round after the refusal");
    client.disconnect().await.expect("disconnect");
}

/// A round that carried state alone waits 50 ms, doubling each such round
/// in a row, at most 250 ms.
#[test]
fn a_state_only_round_waits_fifty_ms_doubling_to_a_quarter_second() {
    let waits: Vec<u128> = (0..6).map(|n| state_only_pause(n).as_millis()).collect();
    assert_eq!(waits, [50, 100, 200, 250, 250, 250]);
    assert_eq!(state_only_pause(u32::MAX), Duration::from_millis(250));
}

/// An `input_required` that asks nothing and carries no state is an
/// unexpected response: the identical call is not sent again.
#[tokio::test]
async fn an_input_required_with_nothing_asked_and_no_state_is_not_sent_again() {
    let (client, seen) = modern_client(Arc::new(|_| {
        ServerResult::InputRequiredResult(InputRequiredResult::new(Some(BTreeMap::new()), None))
    }))
    .await;

    let outcome = call(&client).await;
    assert!(outcome.is_err(), "an error, got a result");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(seen.calls().len(), 1, "sent once");
    client.disconnect().await.expect("disconnect");
}

/// A call answered at once is one request and no `notifications/cancelled`.
#[tokio::test]
async fn an_answered_call_is_one_request_and_no_cancel() {
    let (client, seen) = modern_client(Arc::new(|_| complete())).await;

    let result = call(&client)
        .await
        .unwrap_or_else(|e| panic!("answered: {e}"));
    assert_eq!(text_of(&result), ["ok"]);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(seen.calls().len(), 1);
    assert_eq!(
        seen.cancels(),
        Vec::<serde_json::Value>::new(),
        "nothing cancelled"
    );
    client.disconnect().await.expect("disconnect");
}

/// A call dropped while its second round is in flight sends exactly one
/// `notifications/cancelled`, naming that round's id — none for the first
/// round, which was answered. It pins the cancel contract on a call of more
/// than one round; rmcp's per-request guard, not code of ours, sends it.
#[tokio::test]
async fn a_call_dropped_in_its_second_round_cancels_that_round_alone() {
    let (client, seen) = modern_client_silent_on(Arc::new(|_| state_only("s1")), Some(2)).await;

    let outcome = client
        .call_tool_within(
            "echo",
            serde_json::json!({ "path": "a.txt" }),
            None,
            Duration::from_millis(500),
        )
        .await;
    std::assert_matches!(
        outcome,
        Err(McpError::Timeout(_)),
        "cut by its deadline in round 2",
    );
    // Give the notification the time it would take to land.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let calls = seen.calls();
    assert_eq!(calls.len(), 2, "two rounds, each sent once");
    assert_eq!(
        seen.cancels(),
        vec![serde_json::to_value(&calls[1].0).expect("serialize the id")],
        "one cancel, naming the round in flight"
    );
    client.disconnect().await.expect("disconnect");
}

// ============================================================================
// A dead transport between rounds (a real child)
// ============================================================================

#[cfg(unix)]
mod stdio {
    use super::*;

    use std::path::{Path, PathBuf};

    /// A `2026-07-28` stdio server that answers its `n`th `tools/call` with
    /// the file `reply-<n>` (else `reply-default`): a result as one line of
    /// JSON, or `EXIT`, on which it exits with code 3.
    struct Stub {
        script: PathBuf,
        spawns: PathBuf,
        calls: PathBuf,
    }

    impl Stub {
        fn new(dir: &Path, replies: &[(&str, String)]) -> Self {
            use std::os::unix::fs::PermissionsExt;

            let script = dir.join("modern-server.sh");
            let spawns = dir.join("spawns.log");
            let calls = dir.join("calls.log");
            std::fs::write(&spawns, "").expect("spawn log");
            std::fs::write(&calls, "").expect("call log");
            let discover =
                serde_json::to_string(&discover_result()).expect("serialize the discover result");
            std::fs::write(dir.join("discover.json"), discover).expect("discover result");
            for (name, reply) in replies {
                assert!(!reply.contains('\n'), "one line");
                std::fs::write(dir.join(name), reply).expect("a reply");
            }
            let body = r#"#!/bin/sh
if [ "$1" != --grouped ]; then
  exec perl -e 'setpgrp(0, 0) or die "setpgrp: $!\n"; exec @ARGV or die "exec: $!\n"' -- /bin/sh "$0" --grouped
fi
echo "$$" >> '@DIR@/spawns.log'
while IFS= read -r line; do
  id=`printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p'`
  case "$line" in
    *'"method":"server/discover"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":%s}\n' "$id" "`cat '@DIR@/discover.json'`"
      ;;
    *'"method":"tools/call"'*)
      printf '%s\n' "$line" >> '@DIR@/calls.log'
      n=`wc -l < '@DIR@/calls.log'`
      n=$((n))
      if [ -f "@DIR@/reply-$n" ]; then reply=`cat "@DIR@/reply-$n"`; else reply=`cat '@DIR@/reply-default'`; fi
      case "$reply" in
        EXIT) exit 3 ;;
        *) printf '{"jsonrpc":"2.0","id":%s,"result":%s}\n' "$id" "$reply" ;;
      esac
      ;;
  esac
done
"#
            .replace("@DIR@", &dir.display().to_string());
            std::fs::write(&script, body).expect("write the stub");
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
                .expect("chmod the stub");
            Self {
                script,
                spawns,
                calls,
            }
        }

        fn spawns(&self) -> Vec<i32> {
            std::fs::read_to_string(&self.spawns)
                .unwrap_or_default()
                .lines()
                .filter_map(|l| l.trim().parse().ok())
                .collect()
        }

        /// Each `tools/call` a server read, across incarnations, as JSON.
        fn calls(&self) -> Vec<serde_json::Value> {
            std::fs::read_to_string(&self.calls)
                .unwrap_or_default()
                .lines()
                .map(|l| serde_json::from_str(l).unwrap_or_default())
                .collect()
        }
    }

    /// Each incarnation leads its own process group: one SIGKILL per group
    /// ends whatever it started, whether or not the client killed it first.
    impl Drop for Stub {
        fn drop(&mut self) {
            for pgid in self.spawns() {
                if pgid > 1 {
                    let _ = std::process::Command::new("kill")
                        .args(["-KILL", "--", &(-pgid).to_string()])
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .status();
                }
            }
        }
    }

    fn json(result: ServerResult) -> String {
        serde_json::to_string(&result).expect("serialize a reply")
    }

    /// A server that exits on the call's second round: the call fails back
    /// to the caller with the two rounds sent once each — the second is not
    /// sent again to a fresh child — and the next call respawns the child
    /// and starts a call of its own (no state carried over).
    #[tokio::test]
    async fn a_transport_that_dies_between_rounds_fails_the_call_and_the_next_one_respawns() {
        let dir = tempfile::tempdir().expect("tempdir");
        let stub = Stub::new(
            dir.path(),
            &[
                ("reply-1", json(state_only("d1"))),
                ("reply-2", "EXIT".to_string()),
                ("reply-default", json(complete())),
            ],
        );
        let client = McpClient::connect(McpClientConfig {
            server_name: "mrtr-stdio".into(),
            transport: TransportKind::Stdio {
                command: stub.script.to_string_lossy().into_owned(),
                args: vec![],
                env: Default::default(),
                cwd: None,
            },
            request_timeout: Duration::from_secs(5),
            max_reconnect_attempts: 3,
            reconnect_backoff_ms: 1,
            ..McpClientConfig::default()
        })
        .await
        .unwrap_or_else(|e| panic!("connect: {e}"));
        assert_eq!(
            client
                .protocol_version()
                .map(ToString::to_string)
                .as_deref(),
            Some("2026-07-28"),
            "the stub is served in the 2026-07-28 lifecycle"
        );

        let first = tokio::time::timeout(Duration::from_secs(10), call(&client))
            .await
            .expect("the call ends within its own deadline");
        std::assert_matches!(
            first,
            Err(McpError::TransportClosed),
            "the dead transport is the caller's failure",
        );
        let calls = stub.calls();
        assert_eq!(calls.len(), 2, "two rounds, each sent once");
        assert_eq!(calls[1]["params"]["requestState"], "d1");
        assert_eq!(
            stub.spawns().len(),
            1,
            "not respawned for the call that failed"
        );

        let closed = async || {
            client
                .inner
                .service
                .lock()
                .await
                .as_ref()
                .is_none_or(|running| running.peer().is_transport_closed())
        };
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while !closed().await {
            assert!(
                tokio::time::Instant::now() < deadline,
                "rmcp must notice the exit"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }

        let second = call(&client)
            .await
            .unwrap_or_else(|e| panic!("the next call respawns and is answered: {e}"));
        assert_eq!(text_of(&second), ["ok"]);
        assert_eq!(stub.spawns().len(), 2, "the next call respawned the child");
        let calls = stub.calls();
        assert_eq!(calls.len(), 3, "one more request, the next call's own");
        assert!(
            calls[2]["params"].get("requestState").is_none(),
            "a fresh call, not the dead round resent: {}",
            calls[2]
        );
        client.disconnect().await.expect("disconnect");
    }
}
