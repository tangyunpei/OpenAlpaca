//! M82: an MCP tool's structured output checked against the output schema
//! the tool declares (MCP `Tool.outputSchema`: "Clients SHOULD validate
//! structured results against this schema").
//!
//! Built in-process on the owner's ruling of 2026-11-22, in its least
//! harmful form, because a server supplies both the schema and the value and
//! `jsonschema` 0.62.0 is unbounded in time on such input (measured: tens of
//! seconds to compile or to check; gigabytes to compile):
//!
//! - **One thread per server** (I1). Every `jsonschema` call of a server's
//!   tools — compiling a schema and checking a value — runs on one dedicated
//!   OS thread named `mcp-output-check:<server>`, never on a tokio worker,
//!   the blocking pool or the caller. At most one such thread works per
//!   server name in an [`OutputChecks`] (one per daemon: the production
//!   registry's), across reconnects, reloads, a disable/enable cycle and a
//!   tool-list refresh: a busy thread is never joined by a second one, so a
//!   hostile server holds one core at most and reconnecting buys it nothing.
//!   A thread whose server has no registered tool left ends when idle; one
//!   stuck on a job stays until the process ends. (An idle thread that is
//!   ending may overlap for an instant with its successor; it holds no job
//!   and does no work.)
//! - **A bounded wait** (I2). A call waits at most [`OUTPUT_CHECK_WAIT`] for
//!   its answer, asynchronously; past it, while the thread has been busy
//!   longer than that, or before the tool's schema is compiled, the result
//!   is kept unchanged with [`NOT_CHECKED_LINE`]. Nothing is refused because
//!   of a check.
//! - **Yes or no** (I3). A check is `Validator::is_valid` and nothing else; a
//!   mismatch is one fixed line, [`MISMATCH_LINE`], with no detail, so no
//!   third-party text enters a result from here and there is nothing to
//!   escape.
//! - **Bounded holding** (I4). At most [`CHECK_QUEUE_MAX`] calls of a server
//!   are inside a check at once, each holding its own value; a call beyond
//!   them is not accepted and gets [`NOT_CHECKED_LINE`] at once. A value is
//!   handed over as an `Arc` (moved, never deep-cloned), one job at a time; a
//!   caller that stops waiting takes its waiting job back, and a tool dropped
//!   before its compile ran takes its queued compile with it. What
//!   `jsonschema` itself allocates while compiling a hostile schema is NOT
//!   bounded here (measured 1.6 GB): a known limit.
//! - **A stack overflow is not caught.** A panic on the thread is (below),
//!   but a stack overflow there ends the whole daemon process. `jsonschema`
//!   0.62.0 has no general recursion limit, so a hostile schema (a deep
//!   `$ref` chain, say) might cause one: not probed (no hostile input is fed
//!   to the engine), a known limit.
//! - **Never joined** (I5): nothing waits for a validation thread; the
//!   daemon's stop and the process exit end it.
//! - **Registration never waits** (I6): [`OutputChecks::register`] queues the
//!   compile on the server's thread and returns.
//!
//! A schema that does not compile (a remote, relative or file `$ref` — no
//! fetch is ever made: the build is `offline()` and the crate's HTTP and file
//! retrievers are not compiled in — an unknown `$schema`, a malformed schema,
//! a pattern the engine refuses) leaves the tool registered and working as
//! before: one warning in the log names the tool and the reason, cut and
//! escaped, and its results get no line. Defaults otherwise: draft 2020-12
//! unless `$schema` names another, the fancy-regex engine, and `format` as an
//! annotation under 2019-09 and 2020-12 (asserted under drafts 4, 6 and 7).
//!
//! The thread per owner and the fixed lines are `pub(crate)` so SA-4 (a
//! subagent's `result_schema`, not built) can reuse them.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use serde_json::{Map, Value};

/// How long a tool call waits for its output check: the lead's pick with the
/// advisor under the owner's delegation (2026-11-22), his to reverse. An
/// ordinary schema checks in microseconds to milliseconds. A constant, not a
/// config key. The sandbox stays the one clock for the call as a whole: this
/// wait runs inside it, so its cut ends the call as it ends any other.
pub(crate) const OUTPUT_CHECK_WAIT: Duration = Duration::from_secs(2);

/// Appended when the structured output did not match the declared schema.
pub(crate) const MISMATCH_LINE: &str =
    "[The structured output does not match the output schema this tool declares.]";

/// Appended when the structured output could not be checked in time: the
/// tool's schema not compiled yet, the server's checking thread busy or
/// stopped, or the answer later than [`OUTPUT_CHECK_WAIT`].
pub(crate) const NOT_CHECKED_LINE: &str =
    "[The structured output was not checked against the output schema this tool declares.]";

/// Appended when a tool that declares an output schema returned a result
/// with no structured output (JSON `null` counts as none) and no error: the
/// MCP specification says such a server MUST return structured output.
pub(crate) const NO_STRUCTURED_LINE: &str =
    "[This tool declares an output schema, but its result has no structured output.]";

/// How many calls of one server may be inside a check at once — the one its
/// thread runs and those waiting their turn, each holding its own value; a
/// call beyond them is not accepted and gets [`NOT_CHECKED_LINE`] at once
/// (I4). The lead's pick under the owner's delegation, his to reverse: room
/// for one round's parallel calls to one server (the shipped per-round tool
/// limits are 3 to 5) with a second caller beside it.
pub(crate) const CHECK_QUEUE_MAX: usize = 8;

/// How many characters of a compile error's reason the log keeps, before
/// escaping: the reason can carry a server-supplied URI.
const REASON_MAX_CHARS: usize = 200;

/// Compiles a schema. The production engine is `jsonschema`'s; a test hands
/// [`OutputChecks::with_engine`] a stand-in that blocks, sleeps or panics.
/// Only the server's validation thread calls it.
pub(crate) trait SchemaEngine: Send + Sync + 'static {
    /// The compiled schema, or why it cannot be used.
    fn compile(&self, schema: &Value) -> Result<Arc<dyn CompiledSchema>, String>;
}

/// A compiled schema. Only the server's validation thread calls it.
pub(crate) trait CompiledSchema: Send + Sync + 'static {
    /// Yes or no: whether `value` matches. Nothing else is ever asked (I3).
    fn is_valid(&self, value: &Value) -> bool;
}

/// `jsonschema` 0.62.0, offline, its options otherwise at their defaults.
struct JsonSchemaEngine;

impl SchemaEngine for JsonSchemaEngine {
    fn compile(&self, schema: &Value) -> Result<Arc<dyn CompiledSchema>, String> {
        match jsonschema::options().offline().build(schema) {
            Ok(validator) => Ok(Arc::new(JsonSchemaValidator(validator))),
            // `masked()` keeps no instance text; the reason may still name a
            // server's URI, which the log line escapes.
            Err(error) => Err(error.masked().to_string()),
        }
    }
}

struct JsonSchemaValidator(jsonschema::Validator);

impl CompiledSchema for JsonSchemaValidator {
    fn is_valid(&self, value: &Value) -> bool {
        self.0.is_valid(value)
    }
}

/// The validation threads of one registry, one per MCP server name, and how
/// long a call waits on one. The daemon's production registry holds the one
/// that matters; every clone of a registry shares it.
pub struct OutputChecks {
    inner: Arc<Pool>,
}

impl std::fmt::Debug for OutputChecks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OutputChecks")
            .field("wait", &self.inner.wait)
            .finish_non_exhaustive()
    }
}

impl Default for OutputChecks {
    fn default() -> Self {
        Self::with_engine(Arc::new(JsonSchemaEngine), OUTPUT_CHECK_WAIT)
    }
}

struct Pool {
    engine: Arc<dyn SchemaEngine>,
    wait: Duration,
    servers: Mutex<HashMap<String, Arc<Server>>>,
    /// Threads started, ever: what the one-thread tests read.
    spawned: AtomicUsize,
}

struct Server {
    name: String,
    state: Mutex<ServerState>,
    /// Wakes the thread: a job queued, or the last user gone.
    wake: Condvar,
    /// One submitter at a time, so at most one job sits in `check`; up to
    /// [`CHECK_QUEUE_MAX`] callers wait for it within their own wait limit,
    /// each holding its own value (I4).
    submit: tokio::sync::Semaphore,
}

#[derive(Default)]
struct ServerState {
    /// Live [`OutputCheck`] handles of this server's tools.
    users: usize,
    /// Compiles waiting, one per tool name: the latest registration wins.
    compiles: HashMap<String, CompileJob>,
    /// The one check handed to the thread and not yet taken (I4).
    check: Option<CheckJob>,
    /// Calls inside a check now, at most [`CHECK_QUEUE_MAX`] (I4).
    callers: usize,
    next_job: u64,
    /// When the thread took the job it runs now.
    busy_since: Option<Instant>,
    /// The thread stopped after a panic: nothing is checked from then on.
    stopped: bool,
}

struct CompileJob {
    slot: Weak<Slot>,
    schema: Arc<Map<String, Value>>,
}

struct CheckJob {
    id: u64,
    validator: Arc<dyn CompiledSchema>,
    value: Arc<Value>,
    reply: tokio::sync::oneshot::Sender<bool>,
}

/// One registered tool's schema, as its compile left it.
struct Slot {
    tool: String,
    status: Mutex<Status>,
}

#[derive(Clone)]
enum Status {
    Pending,
    Ready(Arc<dyn CompiledSchema>),
    Unusable,
}

impl std::fmt::Debug for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Status::Pending => "Pending",
            Status::Ready(_) => "Ready",
            Status::Unusable => "Unusable",
        })
    }
}

/// One registered tool's output check: carried on `ToolBackend::Mcp` beside
/// its `output_schema`. Cloning shares it; when the last clone of every tool
/// of a server is gone, that server's idle thread ends.
#[derive(Clone)]
pub struct OutputCheck {
    handle: Arc<Handle>,
}

struct Handle {
    pool: Arc<Pool>,
    server: Arc<Server>,
    slot: Arc<Slot>,
}

impl Drop for Handle {
    fn drop(&mut self) {
        let mut state = lock(&self.server.state);
        state.users = state.users.saturating_sub(1);
        // Its compile, if still queued, goes with it, so a queued job never
        // keeps a dropped tool's schema alive behind a busy thread (I4).
        // Only its own: a later registration under the same name owns the
        // entry it replaced this one's with.
        if state
            .compiles
            .get(&self.slot.tool)
            .is_some_and(|job| std::ptr::eq(job.slot.as_ptr(), Arc::as_ptr(&self.slot)))
        {
            state.compiles.remove(&self.slot.tool);
        }
        drop(state);
        self.server.wake.notify_one();
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| {
        tracing::warn!("MCP output check: a lock was poisoned; recovering");
        poisoned.into_inner()
    })
}

impl OutputChecks {
    /// A pool over `engine`, whose calls wait at most `wait`.
    pub(crate) fn with_engine(engine: Arc<dyn SchemaEngine>, wait: Duration) -> Self {
        Self {
            inner: Arc::new(Pool {
                engine,
                wait,
                servers: Mutex::new(HashMap::new()),
                spawned: AtomicUsize::new(0),
            }),
        }
    }

    /// Hand tool `tool` of server `server`'s schema to the server's thread
    /// and return at once (I6): the compile runs there, and a call that
    /// arrives before it is done is answered "not checked".
    pub(crate) fn register(
        &self,
        server: &str,
        tool: &str,
        schema: &Arc<Map<String, Value>>,
    ) -> OutputCheck {
        let server = self.server_for(server);
        let slot = Arc::new(Slot {
            tool: tool.to_string(),
            status: Mutex::new(Status::Pending),
        });
        let mut state = lock(&server.state);
        if !state.stopped {
            state.compiles.insert(
                tool.to_string(),
                CompileJob {
                    slot: Arc::downgrade(&slot),
                    schema: Arc::clone(schema),
                },
            );
        }
        drop(state);
        server.wake.notify_one();
        OutputCheck {
            handle: Arc::new(Handle {
                pool: Arc::clone(&self.inner),
                server,
                slot,
            }),
        }
    }

    /// The server's entry, its thread started if it has none, with one more
    /// user counted under the map's lock (so a retiring thread sees it).
    fn server_for(&self, name: &str) -> Arc<Server> {
        let mut servers = lock(&self.inner.servers);
        if let Some(server) = servers.get(name) {
            lock(&server.state).users += 1;
            return Arc::clone(server);
        }
        let server = Arc::new(Server {
            name: name.to_string(),
            state: Mutex::new(ServerState {
                users: 1,
                ..ServerState::default()
            }),
            wake: Condvar::new(),
            submit: tokio::sync::Semaphore::new(1),
        });
        let pool = Arc::clone(&self.inner);
        let for_thread = Arc::clone(&server);
        let spawned = std::thread::Builder::new()
            .name(format!("mcp-output-check:{name}"))
            .spawn(move || run(&pool, &for_thread));
        match spawned {
            // Never joined (I5): the daemon's stop and the process exit end it.
            Ok(_detached) => {
                self.inner.spawned.fetch_add(1, Ordering::SeqCst);
            }
            Err(error) => {
                tracing::warn!(
                    server = %name,
                    error = %error,
                    "could not start the MCP output-check thread; this server's structured output is not checked"
                );
                lock(&server.state).stopped = true;
            }
        }
        servers.insert(name.to_string(), Arc::clone(&server));
        server
    }

    /// Threads started by this pool, ever.
    #[cfg(test)]
    pub(crate) fn threads_started(&self) -> usize {
        self.inner.spawned.load(Ordering::SeqCst)
    }

    /// Whether `server` has an entry (a live or stopped thread).
    #[cfg(test)]
    pub(crate) fn has_server(&self, server: &str) -> bool {
        lock(&self.inner.servers).contains_key(server)
    }
}

/// The server's thread: checks first (a caller waits on each), then
/// compiles; ends when no tool of the server is left and nothing is queued,
/// or after a panic.
fn run(pool: &Arc<Pool>, server: &Arc<Server>) {
    loop {
        let job = {
            let mut state = lock(&server.state);
            loop {
                if let Some(check) = state.check.take() {
                    state.busy_since = Some(Instant::now());
                    break Job::Check(check);
                }
                if let Some(tool) = state.compiles.keys().next().cloned()
                    && let Some(compile) = state.compiles.remove(&tool)
                {
                    state.busy_since = Some(Instant::now());
                    break Job::Compile(compile);
                }
                if state.users == 0 {
                    drop(state);
                    if retire(pool, server) {
                        return;
                    }
                    state = lock(&server.state);
                    continue;
                }
                state = server
                    .wake
                    .wait(state)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
        };
        let outcome = catch_unwind(AssertUnwindSafe(|| job.run(&*pool.engine, &server.name)));
        let mut state = lock(&server.state);
        state.busy_since = None;
        if outcome.is_err() {
            // The lead's reading: a panicked thread means this server's
            // output is not checked from then on, and no second thread is
            // ever started for it (its entry stays, stopped).
            state.stopped = true;
            state.compiles.clear();
            state.check = None;
            drop(state);
            tracing::warn!(
                server = %server.name,
                "the MCP output check stopped after a panic; this server's structured output is not checked until the daemon restarts"
            );
            return;
        }
    }
}

/// Remove an idle server with no user left from the map, under the map's
/// lock (taken before the state's, as [`OutputChecks::server_for`] does).
fn retire(pool: &Pool, server: &Arc<Server>) -> bool {
    let mut servers = lock(&pool.servers);
    let state = lock(&server.state);
    if state.users != 0 || state.check.is_some() || !state.compiles.is_empty() {
        return false;
    }
    if servers
        .get(&server.name)
        .is_some_and(|entry| Arc::ptr_eq(entry, server))
    {
        servers.remove(&server.name);
    }
    true
}

enum Job {
    Compile(CompileJob),
    Check(CheckJob),
}

impl Job {
    fn run(self, engine: &dyn SchemaEngine, server: &str) {
        match self {
            Job::Check(check) => {
                // A caller that stopped waiting wants no answer.
                if check.reply.is_closed() {
                    return;
                }
                let valid = check.validator.is_valid(&check.value);
                let _ = check.reply.send(valid);
            }
            Job::Compile(compile) => {
                // A tool unregistered before its turn is not compiled.
                let Some(slot) = compile.slot.upgrade() else {
                    return;
                };
                let schema = Value::Object((*compile.schema).clone());
                let status = match engine.compile(&schema) {
                    Ok(validator) => Status::Ready(validator),
                    Err(reason) => {
                        tracing::warn!(
                            server = %server,
                            tool = %slot.tool,
                            reason = %log_reason(&reason),
                            "an MCP tool's output schema cannot be used; its results are not checked"
                        );
                        Status::Unusable
                    }
                };
                *lock(&slot.status) = status;
            }
        }
    }
}

/// A compile error's reason for the log: its first [`REASON_MAX_CHARS`]
/// characters, every hidden character and line break written `<U+XXXX>`.
fn log_reason(reason: &str) -> String {
    let total = reason.chars().count();
    let kept: String = reason.chars().take(REASON_MAX_CHARS).collect();
    let mut line = crate::utils::text::escape_hidden_line(&kept).text;
    if let Some(cut) = crate::utils::text::cut_notice(total.saturating_sub(REASON_MAX_CHARS)) {
        line.push(' ');
        line.push_str(&cut);
    }
    line
}

impl OutputCheck {
    /// The one line a successful call's result gets, or `None`: `None` when
    /// the value matched or the schema cannot be used; otherwise
    /// [`NO_STRUCTURED_LINE`], [`MISMATCH_LINE`] or [`NOT_CHECKED_LINE`].
    /// `structured` is the result's structured content with JSON `null`
    /// read as absent; it is moved to the server's thread, never cloned.
    pub(crate) async fn line_for(&self, structured: Option<Arc<Value>>) -> Option<&'static str> {
        let status = lock(&self.handle.slot.status).clone();
        if matches!(status, Status::Unusable) {
            return None;
        }
        let Some(value) = structured else {
            return Some(NO_STRUCTURED_LINE);
        };
        let Status::Ready(validator) = status else {
            return Some(NOT_CHECKED_LINE);
        };
        match self.check(validator, value).await {
            Some(true) => None,
            Some(false) => Some(MISMATCH_LINE),
            None => Some(NOT_CHECKED_LINE),
        }
    }

    /// Ask the server's thread, waiting at most the pool's wait; `None` when
    /// no answer came.
    async fn check(&self, validator: Arc<dyn CompiledSchema>, value: Arc<Value>) -> Option<bool> {
        let Handle { pool, server, .. } = &*self.handle;
        let deadline = tokio::time::Instant::now() + pool.wait;
        if unavailable(server, pool.wait) {
            return None;
        }
        // Beyond the queue a call is not accepted: it returns at once and
        // its value is dropped here, held by nothing of this module (I4).
        let _seat = Seat::take(server)?;
        let _permit = tokio::time::timeout_at(deadline, server.submit.acquire())
            .await
            .ok()?
            .ok()?;
        if unavailable(server, pool.wait) {
            return None;
        }
        let (reply, answer) = tokio::sync::oneshot::channel();
        let id = {
            let mut state = lock(&server.state);
            if state.stopped {
                return None;
            }
            state.next_job += 1;
            let id = state.next_job;
            // Any job still here is a gone caller's: only the permit's
            // holder submits.
            state.check = Some(CheckJob {
                id,
                validator,
                value,
                reply,
            });
            id
        };
        let _take_back = TakeBack { server, id };
        server.wake.notify_one();
        match tokio::time::timeout_at(deadline, answer).await {
            Ok(Ok(valid)) => Some(valid),
            _ => None,
        }
    }
}

/// Whether the server's thread has stopped, or has run its current job for
/// longer than a call may wait: a call then does not queue behind it.
fn unavailable(server: &Server, wait: Duration) -> bool {
    let state = lock(&server.state);
    state.stopped
        || state
            .busy_since
            .is_some_and(|since| since.elapsed() >= wait)
}

/// One call's place among a server's [`CHECK_QUEUE_MAX`] callers, given up
/// when the call returns or is dropped.
struct Seat<'a> {
    server: &'a Server,
}

impl<'a> Seat<'a> {
    /// A place, or `None` when every one is taken.
    fn take(server: &'a Server) -> Option<Self> {
        let mut state = lock(&server.state);
        if state.callers >= CHECK_QUEUE_MAX {
            return None;
        }
        state.callers += 1;
        Some(Self { server })
    }
}

impl Drop for Seat<'_> {
    fn drop(&mut self) {
        let mut state = lock(&self.server.state);
        state.callers = state.callers.saturating_sub(1);
    }
}

/// Takes a caller's job back from the queue when the caller stops waiting,
/// so its value is not held behind a busy thread (I4).
struct TakeBack<'a> {
    server: &'a Server,
    id: u64,
}

impl Drop for TakeBack<'_> {
    fn drop(&mut self) {
        let mut state = lock(&self.server.state);
        if state.check.as_ref().is_some_and(|job| job.id == self.id) {
            state.check = None;
        }
    }
}

/// `body` with `line` appended on a line of its own (`line` alone when the
/// body is empty).
pub(crate) fn with_line(body: String, line: Option<&str>) -> String {
    match line {
        None => body,
        Some(line) if body.is_empty() => line.to_string(),
        Some(line) => format!("{body}\n{line}"),
    }
}

#[cfg(test)]
#[path = "output_check_tests.rs"]
mod tests;
