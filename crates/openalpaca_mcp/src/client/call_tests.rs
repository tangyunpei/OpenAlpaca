//! N-MCP-1: a call that may have reached the server is never sent again.
//!
//! Every server here is a POSIX `sh` loop over stdio that answers
//! `initialize`, records each `tools/call` it receives, and then does what the
//! test asks of it — answer, stay silent, or exit. Each incarnation leads its
//! own process group (`perl`'s `setpgrp`, as the daemon's stubs do), so the
//! stub's teardown ends whatever the shell started, a slow start's `sleep`
//! included, whether or not the client killed the shell first.

#![cfg(unix)]

use super::*;

use std::path::{Path, PathBuf};

/// What the server does with a `tools/call` once it has recorded it.
#[derive(Clone, Copy)]
enum OnCall {
    /// Answer `ok`.
    Answer,
    /// Say nothing to the first call (the client's deadline ends it), answer
    /// every later one.
    SilentFirst,
    /// Exit with code 3 after receiving the first call, answer every later one.
    DieFirst,
    /// Exit with code 3 after receiving every call.
    DieAlways,
}

struct Stub {
    script: PathBuf,
    spawns: PathBuf,
    calls: PathBuf,
}

impl Stub {
    fn lines(path: &Path) -> Vec<i32> {
        std::fs::read_to_string(path)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| l.trim().parse().ok())
            .collect()
    }

    /// The pid of every incarnation, oldest first.
    fn spawns(&self) -> Vec<i32> {
        Self::lines(&self.spawns)
    }

    /// One entry per `tools/call` a server received, across incarnations.
    fn calls(&self) -> usize {
        Self::lines(&self.calls).len()
    }

    /// SIGKILL the live incarnation (the newest pid), out of band.
    fn kill_current(&self) {
        if let Some(pid) = self.spawns().last() {
            signal(*pid, "-KILL");
        }
    }
}

/// Send a signal with the system's `kill`: this crate takes no `libc`.
fn signal(target: i32, sig: &str) {
    let _ = std::process::Command::new("kill")
        .args([sig, "--", &target.to_string()])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

impl Drop for Stub {
    fn drop(&mut self) {
        for pgid in self.spawns() {
            // Never 0 or 1: `kill -- -0` is this test's own group.
            if pgid > 1 {
                signal(-pgid, "-KILL");
            }
        }
    }
}

/// Write the server. `slow_respawn_secs` makes every incarnation after the
/// first wait that long before reading its first line.
fn stub(dir: &Path, tag: &str, on_call: OnCall, slow_respawn_secs: u64) -> Stub {
    use std::os::unix::fs::PermissionsExt;

    let script = dir.join(format!("{tag}-server.sh"));
    let spawns = dir.join(format!("{tag}-spawns.log"));
    let calls = dir.join(format!("{tag}-calls.log"));
    let mark = dir.join(format!("{tag}-first-call-seen"));
    let started = dir.join(format!("{tag}-started-once"));
    std::fs::write(&spawns, "").expect("spawn log");
    std::fs::write(&calls, "").expect("call log");

    let mark = mark.display();
    let reply = r#"printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"ok"}],"isError":false}}\n' "$id""#;
    let on_call = match on_call {
        OnCall::Answer => reply.to_string(),
        OnCall::SilentFirst => {
            format!("if [ -f '{mark}' ]; then {reply}; else : > '{mark}'; fi")
        }
        OnCall::DieFirst => {
            format!("if [ -f '{mark}' ]; then {reply}; else : > '{mark}'; exit 3; fi")
        }
        OnCall::DieAlways => "exit 3".to_string(),
    };
    let slow = if slow_respawn_secs > 0 {
        format!(
            "if [ -f '{started}' ]; then sleep {slow_respawn_secs}; fi\n: > '{started}'\n",
            started = started.display()
        )
    } else {
        String::new()
    };

    let body = format!(
        r#"#!/bin/sh
if [ "$1" != --grouped ]; then
  exec perl -e 'setpgrp(0, 0) or die "setpgrp: $!\n"; exec @ARGV or die "exec: $!\n"' -- /bin/sh "$0" --grouped
fi
echo "$$" >> '{spawns}'
{slow}while IFS= read -r line; do
  id=`printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p'`
  case "$line" in
    *'"method":"initialize"'*)
      pv=`printf '%s' "$line" | sed -n 's/.*"protocolVersion":"\([^"]*\)".*/\1/p'`
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{"protocolVersion":"%s","capabilities":{{"tools":{{}}}},"serverInfo":{{"name":"stub","version":"0.0.1"}}}}}}\n' "$id" "$pv"
      ;;
    *'"method":"tools/call"'*)
      echo "$$" >> '{calls}'
      {on_call}
      ;;
  esac
done
"#,
        spawns = spawns.display(),
        calls = calls.display(),
    );
    std::fs::write(&script, body).expect("write stub");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod stub");
    Stub {
        script,
        spawns,
        calls,
    }
}

fn config(stub: &Stub, request_timeout: Duration) -> McpClientConfig {
    McpClientConfig {
        server_name: "once".into(),
        transport: TransportKind::Stdio {
            command: stub.script.to_string_lossy().into_owned(),
            args: vec![],
            env: Default::default(),
            cwd: None,
        },
        request_timeout,
        max_reconnect_attempts: 3,
        reconnect_backoff_ms: 1,
        ..McpClientConfig::default()
    }
}

/// Poll a condition to a deadline.
async fn eventually(timeout: Duration, mut check: impl AsyncFnMut() -> bool) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if check().await {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Has the client's service seen its transport close (the child's stdout
/// reached EOF)? rmcp notices within milliseconds of the exit.
async fn transport_closed(client: &McpClient) -> bool {
    client
        .inner
        .service
        .lock()
        .await
        .as_ref()
        .is_none_or(|running| running.peer().is_transport_closed())
}

/// A call whose answer never comes is ended by its deadline and handed back as
/// the timeout — not re-sent to a fresh child, where a non-idempotent tool
/// would run a second time.
#[tokio::test]
async fn a_call_that_times_out_is_sent_once_and_the_caller_gets_the_timeout() {
    let dir = tempfile::tempdir().expect("tempdir");
    let stub = stub(dir.path(), "silent", OnCall::SilentFirst, 0);
    let client = McpClient::connect(config(&stub, Duration::from_secs(1)))
        .await
        .unwrap_or_else(|e| panic!("connect: {e}"));

    let outcome = client.call_tool("echo", serde_json::json!({}), None).await;
    assert!(
        matches!(outcome, Err(McpError::Timeout(_))),
        "the caller must receive the timeout, got {:?}",
        outcome.map(|_| "a result")
    );
    // Give a (wrong) re-send the time it would take to land.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(stub.calls(), 1, "the call reached the server once");
    assert_eq!(stub.spawns().len(), 1, "no child was respawned for it");

    // The connection is still good: the next call is sent on it and answered.
    let next = client
        .call_tool("echo", serde_json::json!({}), None)
        .await
        .unwrap_or_else(|e| panic!("the next call: {e}"));
    assert_eq!(next.content.len(), 1);
    assert_eq!(stub.spawns().len(), 1, "still the first child");
    client.disconnect().await.expect("disconnect");
}

/// A server that exits once it has the call: the call fails back to the
/// caller, sent once; the *next* call respawns the child and is answered.
#[tokio::test]
async fn a_server_that_dies_after_the_call_is_sent_sees_one_send_and_the_next_call_respawns_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let stub = stub(dir.path(), "dies", OnCall::DieFirst, 0);
    let client = McpClient::connect(config(&stub, Duration::from_secs(5)))
        .await
        .unwrap_or_else(|e| panic!("connect: {e}"));

    let first = tokio::time::timeout(
        Duration::from_secs(10),
        client.call_tool("echo", serde_json::json!({}), None),
    )
    .await
    .expect("the call must end within its own deadline");
    assert!(
        first.is_err(),
        "a call the server died on is the caller's failure, not a re-send"
    );
    assert_eq!(stub.calls(), 1, "sent once");
    assert_eq!(
        stub.spawns().len(),
        1,
        "not respawned for the call that failed"
    );

    let second = client
        .call_tool("echo", serde_json::json!({}), None)
        .await
        .unwrap_or_else(|e| panic!("the next call respawns and is answered: {e}"));
    assert_eq!(second.content.len(), 1);
    assert_eq!(stub.spawns().len(), 2, "the next call respawned the child");
    assert_eq!(stub.calls(), 2, "one send per call");
    client.disconnect().await.expect("disconnect");
}

/// A server that handshakes and then dies on every call reaches
/// `ReconnectExhausted` once `max_reconnect_attempts` respawns have handshaken
/// — a handshake alone does not reset the count — instead of looping until
/// the sandbox's cut (60 s for an agent, 300 s for the lead).
#[tokio::test]
async fn a_server_that_handshakes_but_fails_every_call_reaches_reconnect_exhausted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let stub = stub(dir.path(), "always", OnCall::DieAlways, 0);
    let client = McpClient::connect(config(&stub, Duration::from_secs(5)))
        .await
        .unwrap_or_else(|e| panic!("connect: {e}"));

    let mut outcomes = Vec::new();
    let reached = tokio::time::timeout(Duration::from_secs(8), async {
        for _ in 0..8 {
            let outcome = client.call_tool("echo", serde_json::json!({}), None).await;
            let exhausted = matches!(outcome, Err(McpError::ReconnectExhausted(3)));
            outcomes.push(outcome.err().map(|e| e.to_string()));
            if exhausted {
                return;
            }
            // A call issued before rmcp has seen the last exit is sent into the
            // closing transport and fails there; wait for the close so the next
            // call's pre-send check sees it.
            let _ = eventually(Duration::from_secs(2), async || {
                transport_closed(&client).await
            })
            .await;
        }
    })
    .await;
    assert!(
        reached.is_ok(),
        "ReconnectExhausted must end the ladder, not the test's deadline (outcomes {outcomes:?})"
    );
    assert_eq!(
        outcomes.last(),
        Some(&Some("reconnect attempts exhausted (tried 3)".to_string())),
        "outcomes {outcomes:?}"
    );
    assert_eq!(
        outcomes.len(),
        5,
        "one call, then three that respawned, then the refusal"
    );
    assert_eq!(
        stub.spawns().len(),
        4,
        "the first child and max_reconnect_attempts respawns"
    );
    assert_eq!(
        stub.calls(),
        4,
        "each handshaken child received its call once"
    );
}

/// The transparent respawn the extension design keeps (§3.6 item 1): a child
/// that died between calls is respawned once **before** the send, and the
/// call succeeds. (The parent's ladder reached the same outcome after a failed
/// send; this pins that the pre-send path keeps it.)
#[tokio::test]
async fn a_dead_stdio_child_is_respawned_once_before_the_send_and_the_call_succeeds() {
    let dir = tempfile::tempdir().expect("tempdir");
    let stub = stub(dir.path(), "respawn", OnCall::Answer, 0);
    let client = McpClient::connect(config(&stub, Duration::from_secs(5)))
        .await
        .unwrap_or_else(|e| panic!("connect: {e}"));

    stub.kill_current();
    assert!(
        eventually(Duration::from_secs(5), async || transport_closed(&client)
            .await)
        .await,
        "rmcp must notice the child's exit"
    );
    let result = client
        .call_tool("echo", serde_json::json!({}), None)
        .await
        .unwrap_or_else(|e| panic!("the respawned child answers: {e}"));
    assert_eq!(result.content.len(), 1);
    assert_eq!(stub.spawns().len(), 2, "respawned once");
    assert_eq!(stub.calls(), 1, "and sent once, to the new child");
    client.disconnect().await.expect("disconnect");
}

/// One deadline spans the pre-send reconnect and the call: a respawn whose
/// handshake outlives `request_timeout` ends the call at the deadline, not
/// after the handshake (N-MCP-2's residue: an in-session re-handshake was
/// bounded only by the idle bound).
#[tokio::test]
async fn one_deadline_spans_the_pre_send_reconnect_and_the_call() {
    let dir = tempfile::tempdir().expect("tempdir");
    let stub = stub(dir.path(), "slow", OnCall::Answer, 3);
    let client = McpClient::connect(config(&stub, Duration::from_secs(1)))
        .await
        .unwrap_or_else(|e| panic!("connect: {e}"));

    stub.kill_current();
    assert!(
        eventually(Duration::from_secs(5), async || transport_closed(&client)
            .await)
        .await,
        "rmcp must notice the child's exit"
    );
    let started = tokio::time::Instant::now();
    let outcome = client.call_tool("echo", serde_json::json!({}), None).await;
    let took = started.elapsed();
    assert!(
        matches!(outcome, Err(McpError::Timeout(_))),
        "the respawn's slow handshake must end at the call's deadline, got {:?}",
        outcome.map(|_| "a result")
    );
    assert!(took < Duration::from_millis(2500), "ended after {took:?}");
    assert_eq!(stub.calls(), 0, "nothing was sent");
}
