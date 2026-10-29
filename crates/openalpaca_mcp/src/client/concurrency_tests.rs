//! N-MCP-3: a call does not hold the service lock across its RPC.
//!
//! The server is in-process, over one half of a duplex pipe, so it can answer
//! two requests at once — a `sh` loop over stdio reads one line at a time and
//! could not show the difference.

use super::*;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Answer `initialize` at once and every `tools/call` after `hold`, each in its
/// own task, so concurrent calls are answered concurrently.
async fn concurrent_server(stream: tokio::io::DuplexStream, hold: Duration) {
    let (read_half, write_half) = tokio::io::split(stream);
    let writer = Arc::new(Mutex::new(write_half));
    let mut lines = BufReader::new(read_half).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let Ok(msg) = serde_json::from_str::<rmcp::model::ClientJsonRpcMessage>(&line) else {
            continue;
        };
        let Some((request, id)) = msg.into_request() else {
            continue; // the `initialized` notification carries no id
        };
        let (result, delay) = match request {
            rmcp::model::ClientRequest::InitializeRequest(_) => (
                rmcp::model::ServerResult::InitializeResult(rmcp::model::InitializeResult {
                    protocol_version: ProtocolVersion::default(),
                    capabilities: Default::default(),
                    server_info: Implementation {
                        name: "stub".into(),
                        version: "0.0.1".into(),
                        ..Default::default()
                    },
                    instructions: None,
                }),
                Duration::ZERO,
            ),
            _ => (
                rmcp::model::ServerResult::CallToolResult(rmcp::model::CallToolResult::success(
                    vec![rmcp::model::Content::text("ok")],
                )),
                hold,
            ),
        };
        let writer = Arc::clone(&writer);
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            let mut out =
                serde_json::to_string(&rmcp::model::ServerJsonRpcMessage::response(result, id))
                    .expect("serialize response");
            out.push('\n');
            let _ = writer.lock().await.write_all(out.as_bytes()).await;
        });
    }
}

/// A client whose service is a completed handshake with [`concurrent_server`].
async fn client_over(hold: Duration) -> McpClient {
    let (client_end, server_end) = tokio::io::duplex(64 * 1024);
    tokio::spawn(concurrent_server(server_end, hold));
    let handler = NotifyingHandler {
        server_name: "concurrent".into(),
        tx: None,
    };
    let running = handler.serve(client_end).await.expect("stub handshake");
    let client = McpClient {
        inner: Arc::new(ClientInner::new(
            McpClientConfig {
                server_name: "concurrent".into(),
                request_timeout: Duration::from_secs(10),
                ..McpClientConfig::default()
            },
            ConnectionState::Connecting,
        )),
    };
    client.install_service(running).await.expect("install");
    client
}

/// Two calls to a server that holds each for 2 s finish together, in about
/// 2 s: neither waits for the other's answer under the service lock (and a
/// queued call no longer spends its own deadline waiting for that lock).
#[tokio::test]
async fn two_concurrent_calls_to_a_server_that_holds_each_for_two_seconds_both_finish_in_about_two_seconds()
 {
    let client = client_over(Duration::from_secs(2)).await;
    let started = tokio::time::Instant::now();
    let (a, b) = tokio::join!(
        client.call_tool("first", serde_json::json!({}), None),
        client.call_tool("second", serde_json::json!({}), None),
    );
    let took = started.elapsed();
    a.unwrap_or_else(|e| panic!("first call: {e}"));
    b.unwrap_or_else(|e| panic!("second call: {e}"));
    assert!(
        took < Duration::from_millis(3000),
        "two 2 s calls took {took:?}: the second waited for the first"
    );
    client.disconnect().await.expect("disconnect");
}

/// A call in flight on a cloned peer does not hold up `disconnect`: the close
/// ends it, and it reports the deliberate close, not an unexpected one.
#[tokio::test]
async fn a_disconnect_does_not_wait_for_a_call_in_flight_and_the_call_reads_closed() {
    let client = client_over(Duration::from_secs(30)).await;
    let call = tokio::spawn({
        let client = client.clone();
        async move { client.call_tool("held", serde_json::json!({}), None).await }
    });
    // Let the request leave.
    tokio::time::sleep(Duration::from_millis(200)).await;

    let started = tokio::time::Instant::now();
    tokio::time::timeout(Duration::from_secs(8), client.clone().disconnect())
        .await
        .expect("disconnect must not wait for the call's own deadline")
        .expect("disconnect");
    assert!(
        started.elapsed() < Duration::from_secs(6),
        "disconnect took {:?}",
        started.elapsed()
    );

    let outcome = tokio::time::timeout(Duration::from_secs(5), call)
        .await
        .expect("the call ends with the close")
        .expect("call task");
    assert!(
        matches!(outcome, Err(McpError::Closed)),
        "a call the owner's disable cut off reads Closed, got {:?}",
        outcome.map(|_| "a result")
    );
}
