//! A local HTTP server for tests — never the real Ollama, never the network.
//!
//! The crate carries no HTTP-mock dev-dependency and every provider speaks
//! plain HTTP/1.1, so a listener on `127.0.0.1:0` that answers one request per
//! connection is all the provider and router paths need. The handler sees the
//! method, path and body and decides the reply, so one server can stand in for
//! `/api/tags`, `/api/show` and `/v1/chat/completions` at once.

use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Mutex;

/// One request the server saw, in arrival order.
#[derive(Debug, Clone)]
pub struct RecordedRequest {
    pub method: String,
    pub path: String,
    pub body: String,
}

/// What the handler wants written back.
#[derive(Debug, Clone)]
pub struct MockResponse {
    pub status: u16,
    pub content_type: String,
    pub body: String,
    /// When set, the body is sent as chunked frames with [`Self::frame_delay`]
    /// between them instead of one `content-length` write — the only way a test
    /// can tell a real stream from a completed response replayed as events
    /// (L5). `body` is then unused.
    pub frames: Option<Vec<String>>,
    /// How long the server waits before answering at all — a slow provider, for
    /// the timeout proofs (L7).
    pub delay: Duration,
    /// The pause between streamed frames.
    pub frame_delay: Duration,
}

impl MockResponse {
    pub fn json(body: impl Into<String>) -> Self {
        Self {
            status: 200,
            content_type: "application/json".to_string(),
            body: body.into(),
            frames: None,
            delay: Duration::ZERO,
            frame_delay: Duration::ZERO,
        }
    }

    pub fn error(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            content_type: "application/json".to_string(),
            body: body.into(),
            frames: None,
            delay: Duration::ZERO,
            frame_delay: Duration::ZERO,
        }
    }

    pub fn not_found() -> Self {
        Self::error(404, r#"{"error":"not found"}"#)
    }

    /// A `text/event-stream` answer written one frame at a time, chunked.
    ///
    /// Each frame is flushed on its own, so the client sees it before the next
    /// is written: a test that reads events as they arrive is reading a real
    /// stream, not a body that was complete before the first byte moved.
    pub fn sse(frames: Vec<String>) -> Self {
        Self {
            status: 200,
            content_type: "text/event-stream".to_string(),
            body: String::new(),
            frames: Some(frames),
            delay: Duration::ZERO,
            frame_delay: Duration::from_millis(20),
        }
    }

    /// Answer only after `delay` — a provider that is thinking, not a dead one.
    pub fn after(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    /// The pause between streamed frames (ignored unless [`Self::sse`]).
    pub fn every(mut self, frame_delay: Duration) -> Self {
        self.frame_delay = frame_delay;
        self
    }
}

/// A one-request-per-connection HTTP/1.1 server bound to a loopback port.
///
/// Dropping it aborts the accept loop, so a test that returns early leaves no
/// listener behind.
pub struct MockHttpServer {
    /// `http://127.0.0.1:<port>` — no trailing slash.
    pub base_url: String,
    seen: Arc<Mutex<Vec<RecordedRequest>>>,
    accept_loop: tokio::task::JoinHandle<()>,
}

impl Drop for MockHttpServer {
    fn drop(&mut self) {
        self.accept_loop.abort();
    }
}

impl MockHttpServer {
    /// Start a server whose `handler` answers every request.
    pub async fn start<F>(handler: F) -> Self
    where
        F: Fn(&RecordedRequest) -> MockResponse + Send + Sync + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind a loopback port");
        let addr = listener.local_addr().expect("local addr");
        let seen: Arc<Mutex<Vec<RecordedRequest>>> = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&seen);
        let handler = Arc::new(handler);

        let accept_loop = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let recorder = Arc::clone(&recorder);
                let handler = Arc::clone(&handler);
                tokio::spawn(async move {
                    let Some(request) = read_request(&mut socket).await else {
                        return;
                    };
                    recorder.lock().await.push(request.clone());
                    let response = handler(&request);
                    if !response.delay.is_zero() {
                        tokio::time::sleep(response.delay).await;
                    }
                    match response.frames {
                        Some(ref frames) => {
                            let head = format!(
                                "HTTP/1.1 {} {}\r\ncontent-type: {}\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n",
                                response.status,
                                reason(response.status),
                                response.content_type,
                            );
                            let _ = socket.write_all(head.as_bytes()).await;
                            let _ = socket.flush().await;
                            for frame in frames {
                                let chunk =
                                    format!("{:x}\r\n{}\r\n", frame.len(), frame);
                                if socket.write_all(chunk.as_bytes()).await.is_err() {
                                    return;
                                }
                                let _ = socket.flush().await;
                                if !response.frame_delay.is_zero() {
                                    tokio::time::sleep(response.frame_delay).await;
                                }
                            }
                            let _ = socket.write_all(b"0\r\n\r\n").await;
                        }
                        None => {
                            let head = format!(
                                "HTTP/1.1 {} {}\r\ncontent-type: {}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                                response.status,
                                reason(response.status),
                                response.content_type,
                                response.body.len(),
                            );
                            let _ = socket.write_all(head.as_bytes()).await;
                            let _ = socket.write_all(response.body.as_bytes()).await;
                        }
                    }
                    let _ = socket.flush().await;
                    let _ = socket.shutdown().await;
                });
            }
        });

        Self {
            base_url: format!("http://{addr}"),
            seen,
            accept_loop,
        }
    }

    /// Every request the server has answered so far.
    pub async fn requests(&self) -> Vec<RecordedRequest> {
        self.seen.lock().await.clone()
    }

    /// How many requests hit this path.
    pub async fn hits(&self, path: &str) -> usize {
        self.seen
            .lock()
            .await
            .iter()
            .filter(|r| r.path == path)
            .count()
    }
}

/// Read one HTTP/1.1 request: head up to the blank line, then `content-length`
/// bytes of body. Returns `None` when the peer hung up first.
async fn read_request(socket: &mut tokio::net::TcpStream) -> Option<RecordedRequest> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    let head_end = loop {
        if let Some(pos) = find_head_end(&buf) {
            break pos;
        }
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    };

    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.split("\r\n");
    let request_line = lines.next()?;
    let mut parts = request_line.split(' ');
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();

    let content_length = head
        .split("\r\n")
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .unwrap_or(0);

    let body_start = head_end + 4;
    while buf.len() < body_start + content_length {
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let body = String::from_utf8_lossy(&buf[body_start..buf.len().min(body_start + content_length)])
        .to_string();

    Some(RecordedRequest {
        method,
        path,
        body,
    })
}

fn find_head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        404 => "Not Found",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Status",
    }
}
