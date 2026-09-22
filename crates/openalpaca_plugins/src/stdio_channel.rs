use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, ChildStdout};
use tokio::sync::{Semaphore, mpsc, oneshot};
use tracing::{debug, error, trace, warn};

use crate::error::PluginError;

/// Map of in-flight request IDs to their response senders.
type PendingMap = HashMap<u64, oneshot::Sender<Result<Value, PluginError>>>;

/// Critical sections only insert/remove senders and never await. A synchronous
/// mutex lets a dropped request future remove its entry immediately.
#[derive(Default)]
struct PendingRequests(Mutex<PendingMap>);

impl PendingRequests {
    fn register(
        &self,
        id: u64,
        sender: oneshot::Sender<Result<Value, PluginError>>,
    ) -> PendingRequest<'_> {
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(id, sender);
        PendingRequest { requests: self, id }
    }

    fn take(&self, id: u64) -> Option<oneshot::Sender<Result<Value, PluginError>>> {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).remove(&id)
    }

    fn drain(&self) {
        let requests = std::mem::take(&mut *self.0.lock().unwrap_or_else(|p| p.into_inner()));
        if !requests.is_empty() {
            warn!(
                count = requests.len(),
                "draining pending requests after process exit"
            );
        }
        for sender in requests.into_values() {
            let _ = sender.send(Err(PluginError::ProcessCrashed));
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.0.lock().unwrap().len()
    }
}

struct PendingRequest<'a> {
    requests: &'a PendingRequests,
    id: u64,
}

impl Drop for PendingRequest<'_> {
    fn drop(&mut self) {
        // The reader may already have completed it. Removal is idempotent.
        self.requests.take(self.id);
    }
}

/// Multiplexed JSON-RPC 2.0 transport over a child process's stdin/stdout.
///
/// Uses Content-Length framing (LSP/MCP standard). A single reader task
/// continuously parses stdout, correlating responses to pending requests via
/// JSON-RPC `id` fields. Notifications (messages without `id`) are forwarded
/// to a separate channel on a best-effort basis: if the notification receiver
/// is not being drained and the channel fills, further notifications are
/// dropped (with a warning) rather than blocking the reader loop — a blocked
/// reader would stall RPC response correlation and deadlock the plugin's own
/// in-flight calls.
#[derive(Clone)]
pub struct StdioChannel {
    writer: mpsc::Sender<Vec<u8>>,
    pending: Arc<PendingRequests>,
    next_id: Arc<AtomicU64>,
    semaphore: Arc<Semaphore>,
    /// Kept alive so cloned `StdioChannel` handles retain the notification sender.
    #[allow(dead_code)]
    notification_tx: mpsc::Sender<Value>,
    default_timeout: Duration,
}

impl StdioChannel {
    /// Create a new `StdioChannel`, spawning reader and writer background tasks.
    ///
    /// Returns the channel handle and a receiver for JSON-RPC notifications
    /// (messages that have no `id` field, e.g. `$/event`).
    pub fn new(
        stdin: ChildStdin,
        stdout: ChildStdout,
        max_concurrent: usize,
        default_timeout: Duration,
    ) -> (Self, mpsc::Receiver<Value>) {
        let (writer_tx, writer_rx) = mpsc::channel::<Vec<u8>>(64);
        let (notification_tx, notification_rx) = mpsc::channel::<Value>(64);
        let pending: Arc<PendingRequests> = Arc::new(PendingRequests::default());

        // Spawn writer task
        tokio::spawn(writer_task(stdin, writer_rx));

        // Spawn reader task
        tokio::spawn(reader_task(
            stdout,
            Arc::clone(&pending),
            notification_tx.clone(),
        ));

        let channel = Self {
            writer: writer_tx,
            pending,
            next_id: Arc::new(AtomicU64::new(1)),
            semaphore: Arc::new(Semaphore::new(max_concurrent)),
            notification_tx,
            default_timeout,
        };

        (channel, notification_rx)
    }

    /// Send a JSON-RPC request and await the correlated response.
    ///
    /// Uses the channel's default timeout.
    pub async fn call(&self, method: &str, params: Value) -> Result<Value, PluginError> {
        self.call_with_timeout(method, params, self.default_timeout)
            .await
    }

    /// Send a JSON-RPC request with a custom timeout.
    pub async fn call_with_timeout(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, PluginError> {
        let _permit = self
            .semaphore
            .acquire()
            .await
            .map_err(|_| PluginError::ChannelClosed)?;

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();

        // Covers normal completion, timeout, send failure and external cancellation.
        let _pending = self.pending.register(id, tx);

        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });

        let body = serde_json::to_string(&request)?;
        let frame = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);

        debug!(id, method, "sending JSON-RPC request");

        self.writer
            .send(frame.into_bytes())
            .await
            .map_err(|_| PluginError::ChannelClosed)?;
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(PluginError::ProcessCrashed),
            Err(_) => Err(PluginError::Timeout),
        }
    }

    /// Send a JSON-RPC notification (fire-and-forget, no response expected).
    pub async fn notify(&self, method: &str, params: Value) -> Result<(), PluginError> {
        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
        });

        let body = serde_json::to_string(&notification)?;
        let frame = format!("Content-Length: {}\r\n\r\n{}", body.len(), body);

        debug!(method, "sending JSON-RPC notification");

        self.writer
            .send(frame.into_bytes())
            .await
            .map_err(|_| PluginError::ChannelClosed)?;

        Ok(())
    }

    /// Drain all pending requests, sending `ProcessCrashed` to each.
    ///
    /// Called when the child process exits or crashes to unblock all waiters.
    pub fn drain_pending(&self) {
        self.pending.drain();
    }
}

/// Background task that writes Content-Length-framed messages to the child's stdin.
async fn writer_task(mut stdin: ChildStdin, mut rx: mpsc::Receiver<Vec<u8>>) {
    while let Some(data) = rx.recv().await {
        if let Err(e) = stdin.write_all(&data).await {
            error!(error = %e, "failed to write to plugin stdin");
            break;
        }
        if let Err(e) = stdin.flush().await {
            error!(error = %e, "failed to flush plugin stdin");
            break;
        }
        trace!(bytes = data.len(), "wrote frame to plugin stdin");
    }
    debug!("writer task exiting");
}

/// Background task that reads Content-Length-framed messages from the child's stdout.
///
/// On process exit (EOF), all pending senders receive `ProcessCrashed`.
///
/// Generic over the reader so tests can drive it with an in-memory stream;
/// production passes the child's `ChildStdout`.
async fn reader_task<R>(
    stdout: R,
    pending: Arc<PendingRequests>,
    notification_tx: mpsc::Sender<Value>,
) where
    R: tokio::io::AsyncRead + Unpin + Send + 'static,
{
    let mut reader = BufReader::new(stdout);

    // Upper bound on a single framed message. The Content-Length comes from the
    // (untrusted) plugin's stdout; without a cap, a bogus huge value (e.g.
    // usize::MAX) would `vec![0u8; len]` and OOM/abort the whole daemon.
    const MAX_MESSAGE_BYTES: usize = 64 * 1024 * 1024;

    loop {
        // Step 1: Read header line(s) until we find Content-Length.
        let content_length = match read_content_length(&mut reader).await {
            Ok(len) if len > MAX_MESSAGE_BYTES => {
                error!(
                    len,
                    max = MAX_MESSAGE_BYTES,
                    "plugin message exceeds maximum size; closing channel"
                );
                break;
            }
            Ok(len) => len,
            Err(e) => {
                debug!(error = %e, "reader loop ending");
                break;
            }
        };

        // Step 2: Read exactly `content_length` bytes as the body.
        let mut body_buf = vec![0u8; content_length];
        if let Err(e) = reader.read_exact(&mut body_buf).await {
            error!(error = %e, "failed to read message body");
            break;
        }

        // Step 3: Parse JSON.
        let msg: Value = match serde_json::from_slice(&body_buf) {
            Ok(v) => v,
            Err(e) => {
                warn!(error = %e, "received invalid JSON from plugin");
                continue;
            }
        };

        trace!(msg = %msg, "received message from plugin");

        // Step 4: Route the message.
        if let Some(id_val) = msg.get("id") {
            // Response — correlate with pending request.
            let id = match id_val.as_u64() {
                Some(n) => n,
                None => {
                    warn!(?id_val, "received response with non-u64 id");
                    continue;
                }
            };

            let result = if let Some(result) = msg.get("result") {
                Ok(result.clone())
            } else if let Some(err_obj) = msg.get("error") {
                let code = err_obj.get("code").and_then(|c| c.as_i64()).unwrap_or(-1);
                let message = err_obj
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or("unknown error")
                    .to_string();
                let data = err_obj.get("data").cloned();
                Err(PluginError::RpcError {
                    code,
                    message,
                    data,
                })
            } else {
                warn!(id, "response has neither result nor error");
                continue;
            };

            // Look up and notify the waiter — lock is not held across await.
            let sender = pending.take(id);

            if let Some(tx) = sender {
                let _ = tx.send(result);
            } else {
                warn!(id, "received response for unknown request id");
            }
        } else {
            // Notification — no `id` field. Use try_send so the reader loop can
            // never block on a full channel: responses for in-flight RPCs are
            // parsed by this same loop, so blocking here would deadlock the
            // plugin's own RPC handling once the channel (capacity 64) fills.
            match notification_tx.try_send(msg) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Full(dropped)) => {
                    let method = dropped
                        .get("method")
                        .and_then(|m| m.as_str())
                        .unwrap_or("<unknown>")
                        .to_string();
                    warn!(
                        method,
                        "notification channel full; dropping plugin notification"
                    );
                }
                Err(mpsc::error::TrySendError::Closed(_)) => {
                    debug!("notification receiver dropped, ignoring notification");
                }
            }
        }
    }

    // Process exited or stdout closed — drain all pending requests.
    pending.drain();
    debug!("reader task exiting");
}

/// Read Content-Length header from the stream.
///
/// Reads lines until a `Content-Length: N` header is found, then consumes the
/// blank separator line (`\r\n`). Returns the content length value.
async fn read_content_length<R: tokio::io::AsyncBufRead + Unpin>(
    reader: &mut R,
) -> Result<usize, std::io::Error> {
    let mut header_line = String::new();
    loop {
        header_line.clear();
        let bytes_read = reader.read_line(&mut header_line).await?;
        if bytes_read == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "EOF while reading header",
            ));
        }

        if let Some(len) = parse_content_length(&header_line) {
            // Consume the blank separator line after the header block.
            // If the header line already ended with \r\n and the next line is
            // also \r\n, that's our separator.
            let mut separator = String::new();
            let sep_bytes = reader.read_line(&mut separator).await?;
            if sep_bytes == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "EOF while reading header separator",
                ));
            }
            return Ok(len);
        }

        // Skip non-Content-Length headers or empty lines.
    }
}

/// Parse a `Content-Length` value from a header line.
///
/// Accepts formats like `Content-Length: 42\r\n` or `Content-Length:100`.
fn parse_content_length(line: &str) -> Option<usize> {
    let trimmed = line.trim();
    let prefix = "Content-Length:";
    if !trimmed.starts_with(prefix) {
        // Case-insensitive fallback.
        let lower = trimmed.to_lowercase();
        if !lower.starts_with("content-length:") {
            return None;
        }
        let after = &trimmed[prefix.len()..];
        return after.trim().parse().ok();
    }
    let after = &trimmed[prefix.len()..];
    after.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_content_length() {
        assert_eq!(parse_content_length("Content-Length: 42\r\n"), Some(42));
        assert_eq!(parse_content_length("Content-Length:100"), Some(100));
        assert_eq!(parse_content_length("invalid"), None);
    }

    #[test]
    fn test_parse_content_length_case_insensitive() {
        assert_eq!(parse_content_length("content-length: 256"), Some(256));
        assert_eq!(parse_content_length("CONTENT-LENGTH:  99 "), Some(99));
    }

    #[test]
    fn test_parse_content_length_edge_cases() {
        assert_eq!(parse_content_length(""), None);
        assert_eq!(parse_content_length("Content-Length: "), None);
        assert_eq!(parse_content_length("Content-Length: -1"), None);
        assert_eq!(parse_content_length("Content-Length: abc"), None);
        assert_eq!(parse_content_length("Content-Length: 0"), Some(0));
    }

    #[tokio::test]
    async fn test_read_content_length_from_stream() {
        let data = b"Content-Length: 13\r\n\r\n{\"test\":true}";
        let mut reader = tokio::io::BufReader::new(&data[..]);

        let len = read_content_length(&mut reader).await.unwrap();
        assert_eq!(len, 13);

        let mut body = vec![0u8; len];
        reader.read_exact(&mut body).await.unwrap();
        assert_eq!(&body, b"{\"test\":true}");
    }

    #[tokio::test]
    async fn test_read_content_length_eof() {
        let data = b"";
        let mut reader = tokio::io::BufReader::new(&data[..]);

        let result = read_content_length(&mut reader).await;
        assert!(result.is_err());
    }

    fn frame(body: &str) -> Vec<u8> {
        format!("Content-Length: {}\r\n\r\n{}", body.len(), body).into_bytes()
    }

    /// Regression test for the notification-receiver liveness hazard: a chatty
    /// plugin must not be able to block the reader loop (and thereby its own
    /// RPC response correlation) by filling the never-polled notification
    /// channel (capacity 64).
    #[tokio::test]
    async fn test_reader_loop_survives_unpolled_notification_flood() {
        let (mut client, server) = tokio::io::duplex(1024 * 1024);

        // Same capacity as StdioChannel::new.
        let (notification_tx, notification_rx) = mpsc::channel::<Value>(64);
        let pending: Arc<PendingRequests> = Arc::new(PendingRequests::default());
        let (tx, rx) = oneshot::channel();
        let _pending_request = pending.register(1, tx);

        tokio::spawn(reader_task(server, Arc::clone(&pending), notification_tx));

        // Flood with far more notifications than the channel holds, while the
        // receiver stays alive but is never polled (the production situation).
        for i in 0..200 {
            let body = format!(r#"{{"jsonrpc":"2.0","method":"$/event","params":{{"n":{i}}}}}"#);
            client.write_all(&frame(&body)).await.unwrap();
        }
        // Then a response the pending waiter is blocked on.
        client
            .write_all(&frame(r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#))
            .await
            .unwrap();

        // With the old blocking send().await the reader stalls at notification
        // 64 and this times out; with try_send it must complete promptly.
        let result = tokio::time::timeout(Duration::from_secs(5), rx)
            .await
            .expect("reader loop blocked by unpolled notification channel")
            .expect("oneshot sender dropped")
            .expect("expected Ok result");
        assert_eq!(result, serde_json::json!({"ok": true}));

        // Keep the receiver alive to the end so try_send saw Full (not Closed).
        drop(notification_rx);
    }

    fn test_channel() -> (StdioChannel, mpsc::Receiver<Vec<u8>>, mpsc::Receiver<Value>) {
        let (writer, frames) = mpsc::channel(1);
        let (notification_tx, notifications) = mpsc::channel(64);
        (
            StdioChannel {
                writer,
                pending: Arc::new(PendingRequests::default()),
                next_id: Arc::new(AtomicU64::new(1)),
                semaphore: Arc::new(Semaphore::new(1)),
                notification_tx,
                default_timeout: Duration::from_secs(60),
            },
            frames,
            notifications,
        )
    }

    #[tokio::test]
    async fn cancelling_a_call_releases_its_entry_and_permit_and_ignores_late_response() {
        let (channel, mut frames, _notifications) = test_channel();
        let (mut output, input) = tokio::io::duplex(4096);
        let reader = tokio::spawn(reader_task(
            input,
            channel.pending.clone(),
            channel.notification_tx.clone(),
        ));
        let first = tokio::spawn({
            let channel = channel.clone();
            async move { channel.call("first", Value::Null).await }
        });
        frames.recv().await.unwrap();
        assert_eq!(channel.pending.len(), 1);
        first.abort();
        assert!(first.await.unwrap_err().is_cancelled());
        assert_eq!(channel.pending.len(), 0);
        assert_eq!(channel.semaphore.available_permits(), 1);

        let second = tokio::spawn({
            let channel = channel.clone();
            async move { channel.call("second", Value::Null).await }
        });
        frames.recv().await.unwrap();
        output
            .write_all(&frame(r#"{"id":1,"result":"late"}"#))
            .await
            .unwrap();
        output
            .write_all(&frame(r#"{"id":2,"result":"current"}"#))
            .await
            .unwrap();
        assert_eq!(second.await.unwrap().unwrap(), "current");
        assert_eq!(channel.pending.len(), 0);
        drop(output);
        reader.await.unwrap();
    }

    #[tokio::test]
    async fn cancelling_while_the_writer_is_full_also_cleans_up() {
        let (channel, _frames, _notifications) = test_channel();
        channel.writer.try_send(Vec::new()).unwrap();
        let call = tokio::spawn({
            let channel = channel.clone();
            async move { channel.call("blocked", Value::Null).await }
        });
        tokio::task::yield_now().await;
        assert_eq!(channel.pending.len(), 1);
        call.abort();
        let _ = call.await;
        assert_eq!(channel.pending.len(), 0);
        assert_eq!(channel.semaphore.available_permits(), 1);
    }

    #[tokio::test]
    async fn timeout_closed_writer_and_process_exit_release_pending_requests() {
        let (channel, mut frames, _notifications) = test_channel();
        assert!(matches!(
            channel
                .call_with_timeout("timeout", Value::Null, Duration::ZERO)
                .await,
            Err(PluginError::Timeout)
        ));
        assert_eq!(channel.pending.len(), 0);
        frames.recv().await.unwrap();
        let call = tokio::spawn({
            let channel = channel.clone();
            async move { channel.call("crash", Value::Null).await }
        });
        frames.recv().await.unwrap();
        channel.drain_pending();
        assert!(matches!(
            call.await.unwrap(),
            Err(PluginError::ProcessCrashed)
        ));
        assert_eq!(channel.pending.len(), 0);
        drop(frames);
        assert!(matches!(
            channel.call("closed", Value::Null).await,
            Err(PluginError::ChannelClosed)
        ));
        assert_eq!(channel.pending.len(), 0);
    }
}
