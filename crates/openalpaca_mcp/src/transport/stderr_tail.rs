// crates/openalpaca_mcp/src/transport/stderr_tail.rs

//! A stdio server's stderr, kept as a bounded tail (N-MCP-5).
//!
//! The server's stderr is piped, never inherited, and a reader task drains
//! the pipe in fixed chunks for as long as the process writes, keeping only
//! the last [`TAIL_BYTES`]: a server that floods its stderr can never block on
//! a full pipe, and nothing it writes reaches the daemon's own log. The bytes
//! are kept raw here; what a person or a model is shown is composed elsewhere
//! (`openalpaca_core::tools::extensions::stderr`), redacted with the values
//! this handle carries and escaped there, and only for a failed server.
//!
//! The handle is the caller's: created before the connect, so a connect that
//! fails — a server that dies at its start, the commonest failure — still
//! leaves its tail with the caller after the client is gone. Each spawn starts
//! a fresh tail ([`StderrTail::current`] is the newest spawn's).

use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::process::ChildStderr;
use tokio::sync::Notify;
use tokio::task::AbortHandle;
use tokio_util::sync::CancellationToken;

/// How many bytes of a spawn's stderr are kept: the last ones written.
pub const TAIL_BYTES: usize = 4096;

/// How long a server's stderr is still read once its process has been reaped,
/// and how long a caller waits for a spawn's stderr to end before it reads the
/// tail as it stands. The group the server led is ended at the reap, so its
/// stderr ends at once — unless a process that left the group still holds the
/// pipe, which this bounds.
pub const STDERR_SETTLE: Duration = Duration::from_millis(250);

/// The size of one read from the pipe.
const CHUNK: usize = 1024;

/// The stderr of the stdio server one client spawns — the caller's handle.
///
/// Cheap to clone; every clone sees the same spawns. Its `Debug` prints only
/// how many bytes the current spawn's tail holds: neither the bytes nor the
/// values it is to be redacted with.
#[derive(Clone)]
pub struct StderrTail {
    inner: Arc<TailInner>,
}

impl Default for StderrTail {
    fn default() -> Self {
        Self::new([])
    }
}

struct TailInner {
    /// The values a composed tail masks — the ones the load resolved for this
    /// server. Never logged.
    secrets: Vec<String>,
    /// The newest spawn's tail.
    current: Mutex<Arc<Ring>>,
}

impl StderrTail {
    /// A handle whose tails are to be masked with `secrets` (empty values are
    /// dropped, the rest kept longest first).
    pub fn new(secrets: impl IntoIterator<Item = String>) -> Self {
        let mut secrets: Vec<String> = secrets.into_iter().filter(|s| !s.is_empty()).collect();
        secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
        secrets.dedup();
        Self {
            inner: Arc::new(TailInner {
                secrets,
                current: Mutex::new(Arc::new(Ring::spawned_nothing())),
            }),
        }
    }

    /// The values a composed tail masks, longest first. Secret: never log
    /// them.
    pub fn secrets(&self) -> &[String] {
        &self.inner.secrets
    }

    /// The newest spawn's tail (an empty, ended one before the first spawn,
    /// and for an HTTP server, which has none: waiting on it takes no time).
    pub fn current(&self) -> SpawnTail {
        SpawnTail(Arc::clone(
            &self.inner.current.lock().unwrap_or_else(|p| p.into_inner()),
        ))
    }

    /// How many bytes the newest spawn's tail holds.
    pub fn len(&self) -> usize {
        self.current().len()
    }

    /// `true` when the newest spawn's tail holds nothing.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Start a new spawn's tail, ending the previous spawn's reader.
    pub(crate) fn begin_spawn(&self) -> SpawnTail {
        let fresh = Arc::new(Ring::default());
        let previous = std::mem::replace(
            &mut *self.inner.current.lock().unwrap_or_else(|p| p.into_inner()),
            Arc::clone(&fresh),
        );
        previous.stop_reader();
        SpawnTail(fresh)
    }
}

impl fmt::Debug for StderrTail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StderrTail")
            .field("len", &self.len())
            .finish_non_exhaustive()
    }
}

/// One spawn's tail. Cheap to clone; a clone taken when a process exits keeps
/// reading that process's tail after a respawn has started a new one.
#[derive(Clone)]
pub struct SpawnTail(Arc<Ring>);

/// An empty, ended tail.
impl Default for SpawnTail {
    fn default() -> Self {
        Self(Arc::new(Ring::spawned_nothing()))
    }
}

impl fmt::Debug for SpawnTail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SpawnTail")
            .field("len", &self.len())
            .finish_non_exhaustive()
    }
}

impl SpawnTail {
    /// How many bytes it holds.
    pub fn len(&self) -> usize {
        self.0.lock().bytes.len()
    }

    /// `true` when it holds nothing.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The tail once the spawn's stderr has ended — or as it stands after
    /// `bound`, whichever is first.
    pub async fn settle(&self, bound: Duration) -> TailSnapshot {
        let deadline = tokio::time::Instant::now() + bound;
        loop {
            let notified = self.0.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.0.lock().ended {
                break;
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                break;
            }
        }
        self.snapshot()
    }

    /// The tail as it stands.
    pub fn snapshot(&self) -> TailSnapshot {
        let state = self.0.lock();
        TailSnapshot {
            bytes: state.bytes.iter().copied().collect(),
            truncated: state.dropped > 0,
            ended: state.ended,
        }
    }

    /// Append what the pipe gave, keeping the last [`TAIL_BYTES`].
    fn push(&self, chunk: &[u8]) {
        let mut state = self.0.lock();
        state.bytes.extend(chunk);
        let over = state.bytes.len().saturating_sub(TAIL_BYTES);
        if over > 0 {
            state.bytes.drain(..over);
            state.dropped += over as u64;
        }
    }

    /// The spawn's stderr is over: its end, an error, the reader stopped, or
    /// a spawn that never started.
    pub(crate) fn end(&self) {
        self.0.lock().ended = true;
        self.0.changed.notify_waiters();
    }
}

/// A spawn's tail as read: the raw bytes, oldest first.
#[derive(Clone, Default)]
pub struct TailSnapshot {
    /// The last bytes the server wrote, at most [`TAIL_BYTES`].
    pub bytes: Vec<u8>,
    /// Bytes before these were written and are no longer kept: the first
    /// bytes here may begin in the middle of a line, or of a value.
    pub truncated: bool,
    /// The spawn's stderr had ended when this was read.
    pub ended: bool,
}

impl fmt::Debug for TailSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TailSnapshot")
            .field("len", &self.bytes.len())
            .field("truncated", &self.truncated)
            .field("ended", &self.ended)
            .finish()
    }
}

/// One spawn's bytes and its reader.
#[derive(Default)]
struct Ring {
    state: Mutex<RingState>,
    changed: Notify,
    reader: Mutex<Option<AbortHandle>>,
}

#[derive(Default)]
struct RingState {
    bytes: VecDeque<u8>,
    dropped: u64,
    ended: bool,
}

impl Ring {
    /// The tail of no spawn: empty, and over.
    fn spawned_nothing() -> Self {
        let ring = Self::default();
        ring.lock().ended = true;
        ring
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, RingState> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn stop_reader(&self) {
        if let Some(reader) = self.reader.lock().unwrap_or_else(|p| p.into_inner()).take() {
            reader.abort();
        }
        // An aborted reader never reaches its own end; the tail it filled
        // stays readable.
        self.lock().ended = true;
        self.changed.notify_waiters();
    }
}

/// Drain `stderr` into `tail` until it ends — or, once `reaped` fires, for at
/// most [`STDERR_SETTLE`] more. The reader holds the only end of the pipe the
/// daemon has, so a server writing faster than anything reads never blocks.
pub(crate) fn start_reader(tail: SpawnTail, stderr: ChildStderr, reaped: CancellationToken) {
    let ring = Arc::clone(&tail.0);
    let task = tokio::spawn(drain(tail, stderr, reaped));
    *ring.reader.lock().unwrap_or_else(|p| p.into_inner()) = Some(task.abort_handle());
}

async fn drain(tail: SpawnTail, mut stderr: ChildStderr, reaped: CancellationToken) {
    // Marks the end however this task stops, an abort included.
    struct EndOnDrop(SpawnTail);
    impl Drop for EndOnDrop {
        fn drop(&mut self) {
            self.0.end();
        }
    }
    let tail = EndOnDrop(tail);

    let mut buf = [0u8; CHUNK];
    let mut deadline: Option<tokio::time::Instant> = None;
    loop {
        let read = match deadline {
            None => tokio::select! {
                biased;
                read = stderr.read(&mut buf) => read,
                () = reaped.cancelled() => {
                    deadline = Some(tokio::time::Instant::now() + STDERR_SETTLE);
                    continue;
                }
            },
            Some(at) => match tokio::time::timeout_at(at, stderr.read(&mut buf)).await {
                Ok(read) => read,
                Err(_elapsed) => break,
            },
        };
        match read {
            Ok(0) | Err(_) => break,
            Ok(n) => tail.0.push(&buf[..n]),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ring_keeps_the_last_bytes_written_and_says_it_dropped_some() {
        let tail = StderrTail::default().begin_spawn();
        tail.push(&[b'a'; 3000]);
        assert!(!tail.snapshot().truncated, "nothing dropped yet");
        tail.push(&[b'b'; 3000]);
        let snap = tail.snapshot();
        assert_eq!(snap.bytes.len(), TAIL_BYTES, "bounded");
        assert!(snap.truncated, "and says so");
        assert_eq!(&snap.bytes[TAIL_BYTES - 3000..], &[b'b'; 3000][..]);
        assert_eq!(
            &snap.bytes[..TAIL_BYTES - 3000],
            &[b'a'; TAIL_BYTES - 3000][..]
        );
    }

    #[test]
    fn debug_prints_a_length_and_no_value() {
        let handle = StderrTail::new(["s3cret-value".to_string(), String::new()]);
        handle.begin_spawn().push(b"hello s3cret-value");
        let shown = format!(
            "{handle:?} {:?} {:?}",
            handle.current(),
            handle.current().snapshot()
        );
        assert!(!shown.contains("s3cret"), "{shown}");
        assert!(!shown.contains("hello"), "{shown}");
        assert!(shown.contains("len: 18"), "{shown}");
        assert_eq!(
            handle.secrets(),
            ["s3cret-value"],
            "the empty value is dropped"
        );
    }

    /// Before any spawn — and always, for an HTTP server — there is nothing
    /// to wait for.
    #[tokio::test]
    async fn a_handle_that_spawned_nothing_settles_at_once() {
        let started = std::time::Instant::now();
        let snap = StderrTail::default()
            .current()
            .settle(Duration::from_secs(5))
            .await;
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(snap.ended && snap.bytes.is_empty());
    }

    #[test]
    fn secrets_are_kept_longest_first() {
        let handle = StderrTail::new(["ab".into(), "abcd".into(), "abc".into(), "abcd".into()]);
        assert_eq!(handle.secrets(), ["abcd", "abc", "ab"]);
    }

    #[test]
    fn a_new_spawn_starts_a_fresh_tail_and_the_old_one_stays_readable() {
        let handle = StderrTail::default();
        let first = handle.begin_spawn();
        first.push(b"first");
        let second = handle.begin_spawn();
        second.push(b"second");
        assert_eq!(handle.current().snapshot().bytes, b"second");
        let old = first.snapshot();
        assert_eq!(old.bytes, b"first", "a clone kept from the first spawn");
        assert!(old.ended, "the respawn ended the first spawn's reading");
    }

    /// A spawned process's stderr, read through the transport's own spawn.
    async fn spawn_writing(script: &str) -> (StderrTail, super::super::TransportConnection) {
        let tail = StderrTail::default();
        let conn = super::super::StdioTransport::new("tail", "/bin/sh")
            .with_args(vec!["-c".into(), script.into()])
            .with_stderr_tail(tail.clone())
            .connect()
            .await
            .expect("spawn");
        (tail, conn)
    }

    /// Kill the spawned process's group and reap it, here, before the test's
    /// runtime ends — a drop's kill runs on a task that runtime may never run.
    async fn kill(conn: super::super::TransportConnection) {
        let super::super::TransportInner::Stdio(child) = conn.inner else {
            unreachable!("a stdio transport")
        };
        if let Some(mut inner) = child.into_inner() {
            let _ = Box::into_pin(inner.kill()).await;
        }
    }

    #[tokio::test]
    async fn a_flood_with_no_newline_is_drained_and_cut_to_the_bound() {
        // 64 KB and no newline, more than a pipe's buffer: the writer would
        // block on a pipe nothing reads, and `exit 7` would never run.
        let (tail, conn) =
            spawn_writing("head -c 65536 /dev/zero | tr '\\000' 'x' >&2; printf 'END' >&2; exit 7")
                .await;
        let snap = tail.current().settle(Duration::from_secs(10)).await;
        assert!(snap.ended, "the pipe was drained to its end");
        assert!(snap.truncated);
        assert_eq!(snap.bytes.len(), TAIL_BYTES);
        assert!(snap.bytes.ends_with(b"xxEND"));
        assert!(!snap.bytes.contains(&b'\n'));
        kill(conn).await;
    }

    #[tokio::test]
    async fn settle_returns_at_the_end_of_stderr_not_at_its_bound() {
        let (tail, conn) = spawn_writing("printf 'one\\ntwo\\n' >&2; exec 2>&-; sleep 5").await;
        let started = std::time::Instant::now();
        let snap = tail.current().settle(Duration::from_secs(4)).await;
        assert!(snap.ended, "stderr closed while the process lives");
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(snap.bytes, b"one\ntwo\n");
        assert!(!snap.truncated);
        kill(conn).await;
    }

    #[tokio::test]
    async fn settle_gives_the_tail_as_it_stands_at_its_bound() {
        let (tail, conn) = spawn_writing("printf 'partial\\n' >&2; sleep 5").await;
        let started = std::time::Instant::now();
        let snap = tail.current().settle(Duration::from_millis(300)).await;
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(!snap.ended, "the process still holds its stderr");
        assert_eq!(snap.bytes, b"partial\n");
        kill(conn).await;
    }
}
