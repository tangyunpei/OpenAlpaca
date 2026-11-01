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
    /// How the connection ends — [`Ending::Answer`] unless a test asks for a
    /// transport failure (M57).
    pub ending: Ending,
}

/// How a mock connection ends. Every way but [`Self::Answer`] is a transport
/// failure the client meets for real — the only way a test can show what a
/// provider makes of one (M57).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    /// The whole answer, then a clean close.
    Answer,
    /// The head, then less body than it promised, then a close: a
    /// `content-length` 64 bytes past what is written, or a chunked stream
    /// that never sends its last chunk.
    CutShort,
    /// No answer at all: the connection is closed once the request is read.
    Close,
    /// No answer, and the close is a reset (`SO_LINGER` 0).
    Reset,
    /// An answer that is not HTTP — a line of text — then a close.
    NotHttp,
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
            ending: Ending::Answer,
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
            ending: Ending::Answer,
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
            ending: Ending::Answer,
        }
    }

    /// Close the connection without answering, once the request is read.
    pub fn hang_up() -> Self {
        Self::error(200, "").ending(Ending::Close)
    }

    /// Reset the connection without answering, once the request is read.
    pub fn reset() -> Self {
        Self::error(200, "").ending(Ending::Reset)
    }

    /// Answer with something that is not HTTP, then close.
    pub fn not_http() -> Self {
        Self::error(200, "").ending(Ending::NotHttp)
    }

    /// Send less body than the head promises, then close.
    pub fn cut_short(self) -> Self {
        self.ending(Ending::CutShort)
    }

    fn ending(mut self, ending: Ending) -> Self {
        self.ending = ending;
        self
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
                    match response.ending {
                        Ending::Close => {
                            let _ = socket.shutdown().await;
                            return;
                        }
                        Ending::Reset => {
                            // Dropped with a zero linger, the socket sends RST.
                            let _ = socket.set_zero_linger();
                            return;
                        }
                        Ending::NotHttp => {
                            let _ = socket.write_all(b"hello\r\n\r\n").await;
                            let _ = socket.shutdown().await;
                            return;
                        }
                        Ending::Answer | Ending::CutShort => {}
                    }
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
                            if response.ending != Ending::CutShort {
                                let _ = socket.write_all(b"0\r\n\r\n").await;
                            }
                        }
                        None => {
                            let promised = match response.ending {
                                Ending::CutShort => response.body.len() + 64,
                                _ => response.body.len(),
                            };
                            let head = format!(
                                "HTTP/1.1 {} {}\r\ncontent-type: {}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                                response.status,
                                reason(response.status),
                                response.content_type,
                                promised,
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

/// A loopback address nothing listens on: bound, read and released, so a
/// connect to it is refused (M57).
#[cfg(feature = "openai")]
pub fn refused_addr() -> std::net::SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a loopback port");
    listener.local_addr().expect("local addr")
}

/// `http://127.0.0.1:<port>` for [`refused_addr`] — no trailing slash.
#[cfg(feature = "openai")]
pub fn refused_base_url() -> String {
    format!("http://{}", refused_addr())
}

/// A client shaped like `providers::build_http_client`'s (L7) — the same
/// connect timeout, read gap and pool — that no proxy reaches, so a request
/// to a loopback listener stays on loopback whatever `HTTP_PROXY` says.
#[cfg(feature = "openai")]
pub fn loopback_client(request_timeout: Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .pool_max_idle_per_host(10)
        .pool_idle_timeout(Duration::from_secs(90))
        .connect_timeout(Duration::from_secs(30).min(request_timeout))
        .read_timeout(request_timeout)
        .build()
        .expect("a test client")
}

/// A name under `.invalid` (RFC 6761), which no resolver may answer. The
/// tests never ask one: a client built by [`client_resolving_with`] answers
/// every name in-process, so not even a DNS query leaves the machine.
#[cfg(feature = "openai")]
pub const UNRESOLVABLE_HOST: &str = "openalpaca-test.invalid";

#[cfg(feature = "openai")]
pub use resolver::{Resolver, client_resolving_with};

/// Only the transport tests use it, and they need the OpenAI-compatible
/// provider.
#[cfg(feature = "openai")]
mod resolver {
    use std::net::SocketAddr;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[derive(Debug, Clone, Copy)]
    enum Answer {
        NoSuchName,
        Never,
        To(SocketAddr),
    }

    /// An in-process DNS resolver that counts its lookups — one per
    /// connection the client tried to open, so a test can count attempts
    /// even where nothing listens.
    #[derive(Debug, Clone)]
    pub struct Resolver {
        answer: Answer,
        lookups: Arc<AtomicUsize>,
    }

    impl Resolver {
        fn answering(answer: Answer) -> Self {
            Self {
                answer,
                lookups: Arc::new(AtomicUsize::new(0)),
            }
        }

        /// Every lookup fails at once: the name does not exist.
        pub fn no_such_name() -> Self {
            Self::answering(Answer::NoSuchName)
        }

        /// No lookup ever completes, so the connect phase can only time out.
        pub fn never() -> Self {
            Self::answering(Answer::Never)
        }

        /// Every name resolves to `addr`.
        pub fn to(addr: SocketAddr) -> Self {
            Self::answering(Answer::To(addr))
        }

        /// How many lookups the client has made so far.
        pub fn lookups(&self) -> usize {
            self.lookups.load(Ordering::SeqCst)
        }
    }

    impl reqwest::dns::Resolve for Resolver {
        fn resolve(&self, _name: reqwest::dns::Name) -> reqwest::dns::Resolving {
            self.lookups.fetch_add(1, Ordering::SeqCst);
            match self.answer {
                Answer::NoSuchName => Box::pin(std::future::ready(Err(
                    Box::new(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        "no such name (test resolver)",
                    )) as Box<dyn std::error::Error + Send + Sync>,
                ))),
                Answer::Never => Box::pin(std::future::pending()),
                Answer::To(addr) => Box::pin(std::future::ready(Ok(
                    Box::new(std::iter::once(addr)) as reqwest::dns::Addrs,
                ))),
            }
        }
    }

    /// A client shaped like the router's (L7) — a connect timeout and a read
    /// gap — whose names `resolver` answers and which no proxy reaches.
    pub fn client_resolving_with(
        resolver: &Resolver,
        connect_timeout: Duration,
        request_timeout: Duration,
    ) -> reqwest::Client {
        reqwest::Client::builder()
            .no_proxy()
            .dns_resolver(Arc::new(resolver.clone()))
            .connect_timeout(connect_timeout)
            .read_timeout(request_timeout)
            .build()
            .expect("a test client")
    }
}
