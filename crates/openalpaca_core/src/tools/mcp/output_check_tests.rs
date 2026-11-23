//! M82's output check. The bounds (I1-I6) are proved with an injected
//! engine that blocks until released, sleeps or panics — never with a
//! hostile schema or value. The real engine sees only small, ordinary
//! schemas and values here.

use super::*;
use crate::tools::mcp::bridge::{SerializeOptions, finish_call_result};
use openalpaca_mcp::{CallToolResult, ContentBlock};
use std::assert_matches;
use std::sync::atomic::AtomicBool;

/// A short wait for the injected-engine tests, so none of them sleeps the
/// production two seconds; the production figure is pinned separately.
const TEST_WAIT: Duration = Duration::from_millis(300);

/// What no test may take longer than to get a call's answer: a call whose
/// future stays pending (a wait that never ends) fails here instead of
/// hanging the suite. It cannot catch a call that blocks its thread inside
/// a poll — a check run inline on the caller, a registration that waits
/// for its compile: `timeout` polls the call first and never gets its turn,
/// so such a regression hangs the tests it reaches, and the module's run
/// with them. Run filtered, it fails the tests that do not block on it:
/// `two_calls_at_once_are_checked_on_the_servers_one_thread`,
/// `the_stop_path_never_waits_on_a_stuck_thread` and
/// `registration_returns_while_a_compile_is_stuck` among them.
const OUTER: Duration = Duration::from_secs(10);

/// A door the injected engine waits at until the test opens it.
#[derive(Default)]
struct Gate {
    open: Mutex<bool>,
    opened: Condvar,
    /// Set while a thread waits at the door.
    waiting: AtomicBool,
}

impl Gate {
    fn pass(&self) {
        let mut open = lock(&self.open);
        self.waiting.store(true, Ordering::SeqCst);
        while !*open {
            open = self
                .opened
                .wait(open)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        self.waiting.store(false, Ordering::SeqCst);
    }

    fn release(&self) {
        *lock(&self.open) = true;
        self.opened.notify_all();
    }

    /// Wait (in real time, at most [`OUTER`]) until a thread is at the door.
    fn await_waiter(&self) {
        let start = Instant::now();
        while !self.waiting.load(Ordering::SeqCst) {
            assert!(start.elapsed() < OUTER, "nothing reached the gate");
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}

/// Opens the gate when the test ends, a failing one too, so no validation
/// thread is left blocked.
struct ReleaseOnDrop(Arc<Gate>);

impl Drop for ReleaseOnDrop {
    fn drop(&mut self) {
        self.0.release();
    }
}

/// The injected engine. A schema or value says what its job does:
/// `"block": true` waits at the probe's gate until the test opens it,
/// `"panic": true` panics, `"sleep_ms": n` sleeps; a value is valid when it
/// carries `"ok": true`.
#[derive(Default)]
struct Probe {
    gate: Arc<Gate>,
    /// The name of the thread each compile and check ran on.
    threads: Mutex<Vec<String>>,
    compiles: AtomicUsize,
    checks: AtomicUsize,
}

impl Probe {
    fn note_thread(&self) {
        let name = std::thread::current()
            .name()
            .unwrap_or("<unnamed>")
            .to_string();
        lock(&self.threads).push(name);
    }

    fn threads(&self) -> Vec<String> {
        lock(&self.threads).clone()
    }

    /// Do what `job` asks for: block, sleep or panic.
    fn act(&self, job: &Value) {
        if job.get("block") == Some(&Value::Bool(true)) {
            self.gate.pass();
        }
        if let Some(ms) = job.get("sleep_ms").and_then(Value::as_u64) {
            std::thread::sleep(Duration::from_millis(ms));
        }
        if job.get("panic") == Some(&Value::Bool(true)) {
            panic!("injected panic");
        }
    }
}

struct ProbeEngine(Arc<Probe>);

impl SchemaEngine for ProbeEngine {
    fn compile(&self, schema: &Value) -> Result<Arc<dyn CompiledSchema>, String> {
        self.0.note_thread();
        self.0.compiles.fetch_add(1, Ordering::SeqCst);
        self.0.act(schema);
        Ok(Arc::new(ProbeSchema(Arc::clone(&self.0))))
    }
}

struct ProbeSchema(Arc<Probe>);

impl CompiledSchema for ProbeSchema {
    fn is_valid(&self, value: &Value) -> bool {
        self.0.note_thread();
        self.0.checks.fetch_add(1, Ordering::SeqCst);
        self.0.act(value);
        value.get("ok") == Some(&Value::Bool(true))
    }
}

/// A probe and the guard that opens its gate when the test ends.
fn probe() -> (Arc<Probe>, ReleaseOnDrop) {
    let probe = Arc::new(Probe::default());
    let release = ReleaseOnDrop(Arc::clone(&probe.gate));
    (probe, release)
}

fn pool(probe: &Arc<Probe>) -> OutputChecks {
    OutputChecks::with_engine(Arc::new(ProbeEngine(Arc::clone(probe))), TEST_WAIT)
}

fn schema(value: Value) -> Arc<Map<String, Value>> {
    match value {
        Value::Object(map) => Arc::new(map),
        other => panic!("a schema is an object: {other}"),
    }
}

fn any_schema() -> Arc<Map<String, Value>> {
    schema(serde_json::json!({"type": "object"}))
}

/// A schema whose compile waits at the probe's gate.
fn blocking_schema() -> Arc<Map<String, Value>> {
    schema(serde_json::json!({"type": "object", "block": true}))
}

/// A value whose check waits at the probe's gate.
fn blocking_value(n: u64) -> Arc<Value> {
    Arc::new(serde_json::json!({"ok": true, "block": true, "n": n}))
}

/// Wait (real time, at most [`OUTER`]) until the tool's compile is done.
fn await_compiled(check: &OutputCheck) {
    let start = Instant::now();
    while matches!(*lock(&check.handle.slot.status), Status::Pending) {
        assert!(start.elapsed() < OUTER, "the compile never finished");
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn ok_value() -> Option<Arc<Value>> {
    Some(Arc::new(serde_json::json!({"ok": true})))
}

async fn line(check: &OutputCheck, value: Option<Arc<Value>>) -> Option<&'static str> {
    tokio::time::timeout(OUTER, check.line_for(value))
        .await
        .expect("a check never takes longer than its wait")
}

async fn finish(result: CallToolResult, check: Option<&OutputCheck>) -> Result<String, String> {
    tokio::time::timeout(
        OUTER,
        finish_call_result(result, &SerializeOptions::default(), check),
    )
    .await
    .expect("a call's result never waits longer than the check's wait")
}

fn structured(value: Value) -> CallToolResult {
    let mut result = CallToolResult::success(vec![ContentBlock::text("Done.".to_string())]);
    result.structured_content = Some(value);
    result
}

/// The real engine over one small, ordinary schema.
fn real_check(checks: &OutputChecks, server: &str, tool: &str) -> OutputCheck {
    let check = checks.register(
        server,
        tool,
        &schema(serde_json::json!({
            "type": "object",
            "properties": {"number": {"type": "integer"}},
            "required": ["number"]
        })),
    );
    await_compiled(&check);
    check
}

// ---------------------------------------------------------------------------
// What the model sees: (a)-(g)
// ---------------------------------------------------------------------------

/// The wait is the owner-delegated pick, two seconds.
#[test]
fn the_wait_limit_is_two_seconds() {
    assert_eq!(OUTPUT_CHECK_WAIT, Duration::from_secs(2));
    assert_eq!(OutputChecks::default().inner.wait, OUTPUT_CHECK_WAIT);
}

/// (a) A matching value: the result is exactly what it was, no line.
#[tokio::test]
async fn a_matching_structured_output_adds_nothing() {
    let checks = OutputChecks::default();
    let check = real_check(&checks, "srv-a", "srv-a__get");
    let value = serde_json::json!({"number": 7});
    let out = finish(structured(value.clone()), Some(&check))
        .await
        .unwrap();
    assert_eq!(
        out,
        format!("Done.\n{}", serde_json::to_string_pretty(&value).unwrap())
    );
}

/// (b) A mismatch: the result kept whole, then the one fixed line and
/// nothing else — no path, no schema text, no value text (I3).
#[tokio::test]
async fn a_mismatching_structured_output_gets_the_one_fixed_line() {
    let checks = OutputChecks::default();
    let check = real_check(&checks, "srv-b", "srv-b__get");
    let value = serde_json::json!({"number": "SECRET-VALUE"});
    let out = finish(structured(value.clone()), Some(&check))
        .await
        .unwrap();
    assert_eq!(
        out,
        format!(
            "Done.\n{}\n{MISMATCH_LINE}",
            serde_json::to_string_pretty(&value).unwrap()
        )
    );
    assert_eq!(
        MISMATCH_LINE,
        "[The structured output does not match the output schema this tool declares.]"
    );
    assert_eq!(
        out.matches("SECRET-VALUE").count(),
        1,
        "the value once, as returned"
    );
}

/// (c) Not compiled yet: the result kept with the "not checked" line, at
/// once — the call does not wait for the compile.
#[tokio::test]
async fn a_call_before_its_schema_is_compiled_is_not_checked() {
    let (probe, _release) = probe();
    let checks = pool(&probe);
    let check = checks.register("srv-c", "srv-c__get", &blocking_schema());
    let start = Instant::now();
    let out = finish(structured(serde_json::json!({"ok": true})), Some(&check))
        .await
        .unwrap();
    assert!(start.elapsed() < TEST_WAIT, "{:?}", start.elapsed());
    assert!(out.ends_with(&format!("\n{NOT_CHECKED_LINE}")), "{out}");
    assert_eq!(
        NOT_CHECKED_LINE,
        "[The structured output was not checked against the output schema this tool declares.]"
    );
    assert_eq!(probe.checks.load(Ordering::SeqCst), 0);
}

/// (d) A schema and no structured content (none, or JSON `null`) on a
/// successful result: one line saying so; alone when nothing else came.
#[tokio::test]
async fn a_declared_schema_with_no_structured_output_gets_its_line() {
    let checks = OutputChecks::default();
    let check = real_check(&checks, "srv-d", "srv-d__get");

    let out = finish(
        CallToolResult::success(vec![ContentBlock::text("Done.".to_string())]),
        Some(&check),
    )
    .await
    .unwrap();
    assert_eq!(out, format!("Done.\n{NO_STRUCTURED_LINE}"));

    let mut null = CallToolResult::success(vec![]);
    null.structured_content = Some(Value::Null);
    let out = finish(null, Some(&check)).await.unwrap();
    assert_eq!(out, NO_STRUCTURED_LINE);
    assert_eq!(
        NO_STRUCTURED_LINE,
        "[This tool declares an output schema, but its result has no structured output.]"
    );
}

/// (d) also while the schema's compile is still pending: a result with no
/// structured content (none, or JSON `null`) gets line (d), not the "not
/// checked" line, at once and with nothing checked.
#[tokio::test]
async fn no_structured_output_gets_its_line_while_the_compile_is_pending() {
    let (probe, _release) = probe();
    let checks = pool(&probe);
    let check = checks.register("srv-dp", "srv-dp__get", &blocking_schema());
    probe.gate.await_waiter();

    let start = Instant::now();
    let out = finish(
        CallToolResult::success(vec![ContentBlock::text("Done.".to_string())]),
        Some(&check),
    )
    .await
    .unwrap();
    assert_eq!(out, format!("Done.\n{NO_STRUCTURED_LINE}"));

    let mut null = CallToolResult::success(vec![]);
    null.structured_content = Some(Value::Null);
    assert_eq!(
        finish(null, Some(&check)).await.unwrap(),
        NO_STRUCTURED_LINE
    );
    assert!(start.elapsed() < TEST_WAIT, "{:?}", start.elapsed());

    assert_matches!(*lock(&check.handle.slot.status), Status::Pending);
    assert_eq!(probe.checks.load(Ordering::SeqCst), 0);
}

/// (e) An error result is not checked and gets no line.
#[tokio::test]
async fn an_error_result_is_not_checked_and_gets_no_line() {
    let (probe, _release) = probe();
    let checks = pool(&probe);
    let check = checks.register("srv-e", "srv-e__get", &any_schema());
    await_compiled(&check);
    let value = serde_json::json!({"ok": false});
    let err = finish(
        CallToolResult::structured_error(value.clone()),
        Some(&check),
    )
    .await
    .unwrap_err();
    assert_eq!(err, value.to_string());
    let err = finish(
        CallToolResult::error(vec![ContentBlock::text("boom".to_string())]),
        Some(&check),
    )
    .await
    .unwrap_err();
    assert_eq!(err, "boom");
    assert_eq!(
        probe.checks.load(Ordering::SeqCst),
        0,
        "nothing was checked"
    );
}

/// (f) No schema: the result is unchanged.
#[tokio::test]
async fn a_tool_without_a_schema_is_unchanged() {
    let value = serde_json::json!({"anything": [1, 2]});
    let out = finish(structured(value.clone()), None).await.unwrap();
    assert_eq!(
        out,
        format!("Done.\n{}", serde_json::to_string_pretty(&value).unwrap())
    );
    let out = finish(CallToolResult::success(vec![]), None).await.unwrap();
    assert_eq!(out, "");
}

/// (g) A schema that does not compile: the tool works as before with no
/// line, and one warning names the tool with its reason cut and escaped.
#[tracing_test::traced_test]
#[tokio::test]
async fn an_unusable_schema_warns_once_and_adds_no_line() {
    let checks = OutputChecks::default();
    let unusable = [
        ("srv-g__relative", serde_json::json!({"$ref": "other.json"})),
        (
            "srv-g__file",
            serde_json::json!({"$ref": "file:///etc/schema.json"}),
        ),
        (
            "srv-g__metaschema",
            serde_json::json!({"$schema": "https://example.invalid/\u{202e}evil", "type": "object"}),
        ),
        ("srv-g__malformed", serde_json::json!({"type": 12})),
        (
            "srv-g__pattern",
            serde_json::json!({"type": "string", "pattern": "("}),
        ),
    ];
    for (tool, raw) in &unusable {
        let check = checks.register("srv-g", tool, &schema(raw.clone()));
        await_compiled(&check);
        assert_matches!(
            *lock(&check.handle.slot.status),
            Status::Unusable,
            "{tool} should not compile"
        );
        for result in [
            structured(serde_json::json!({"number": "x"})),
            CallToolResult::success(vec![ContentBlock::text("Done.".to_string())]),
        ] {
            let plain = finish(result.clone(), None).await;
            assert_eq!(finish(result, Some(&check)).await, plain, "{tool}");
        }
    }
    let logs = String::from_utf8(
        tracing_test::internal::global_buf()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone(),
    )
    .unwrap();
    for (tool, _) in &unusable {
        let warnings = logs
            .lines()
            .filter(|l| l.contains(&format!("tool={tool} ")) && l.contains("cannot be used"))
            .count();
        assert_eq!(warnings, 1, "{tool}: {logs}");
    }
    assert!(logs.contains("<U+202E>evil"), "escaped: {logs}");
    assert!(!logs.contains('\u{202e}'), "never raw: {logs}");
}

/// The reason a log line carries is cut and escaped.
#[test]
fn a_compile_reason_is_cut_and_escaped_for_the_log() {
    assert_eq!(log_reason("bad\u{202e}\nline"), "bad<U+202E><U+000A>line");
    let long = "x".repeat(REASON_MAX_CHARS + 5);
    assert_eq!(
        log_reason(&long),
        format!(
            "{} ... (5 more characters not shown)",
            "x".repeat(REASON_MAX_CHARS)
        )
    );
}

/// No fetch is ever made: a `$ref` to a loopback listener fails the build
/// and the listener accepts nothing.
#[tokio::test]
async fn a_remote_ref_is_never_fetched() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    listener.set_nonblocking(true).expect("nonblocking");
    let port = listener.local_addr().unwrap().port();
    let checks = OutputChecks::default();
    let check = checks.register(
        "srv-ref",
        "srv-ref__get",
        &schema(serde_json::json!({"$ref": format!("http://127.0.0.1:{port}/s.json")})),
    );
    await_compiled(&check);
    assert_matches!(*lock(&check.handle.slot.status), Status::Unusable);
    let result = structured(serde_json::json!({"a": 1}));
    let plain = finish(result.clone(), None).await;
    assert_eq!(finish(result, Some(&check)).await, plain);
    assert_matches!(
        listener.accept(),
        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock,
        "the listener was reached"
    );
}

// ---------------------------------------------------------------------------
// I1: one thread per server
// ---------------------------------------------------------------------------

/// Two calls at once on one server: both checked, on the server's one
/// thread, never on the caller's.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_calls_at_once_are_checked_on_the_servers_one_thread() {
    let (probe, _release) = probe();
    let checks = pool(&probe);
    let check = checks.register("srv-i1", "srv-i1__get", &any_schema());
    await_compiled(&check);
    let slow = || Some(Arc::new(serde_json::json!({"ok": true, "sleep_ms": 20})));
    let (a, b) = tokio::join!(line(&check, slow()), line(&check, slow()));
    assert_eq!((a, b), (None, None));
    assert_eq!(probe.checks.load(Ordering::SeqCst), 2);
    assert_eq!(checks.threads_started(), 1);
    let threads = probe.threads();
    assert_eq!(threads.len(), 3, "{threads:?}");
    assert!(
        threads.iter().all(|t| t == "mcp-output-check:srv-i1"),
        "{threads:?}"
    );
}

/// A reconnect's re-registration while the thread is stuck: still one
/// thread, and the new registrations' results are "not checked".
#[tokio::test]
async fn a_reregistration_while_the_thread_is_stuck_starts_no_second_thread() {
    let (probe, _release) = probe();
    let checks = pool(&probe);
    let first = checks.register("srv-re", "srv-re__get", &blocking_schema());
    probe.gate.await_waiter();
    // The old load goes away entirely while its thread is stuck.
    drop(first);
    for _ in 0..3 {
        let again = checks.register("srv-re", "srv-re__get", &any_schema());
        assert_eq!(
            line(&again, ok_value()).await,
            Some(NOT_CHECKED_LINE),
            "a stuck thread's server is not checked"
        );
    }
    assert_eq!(checks.threads_started(), 1);
    assert_eq!(probe.compiles.load(Ordering::SeqCst), 1);
}

/// Two servers are independent: one stuck does not stop the other.
#[tokio::test]
async fn two_servers_have_a_thread_each() {
    let (probe, _release) = probe();
    let checks = pool(&probe);
    let stuck = checks.register("srv-one", "srv-one__get", &any_schema());
    let free = checks.register("srv-two", "srv-two__get", &any_schema());
    await_compiled(&stuck);
    await_compiled(&free);
    assert_eq!(
        line(&stuck, Some(blocking_value(1))).await,
        Some(NOT_CHECKED_LINE)
    );
    probe.gate.await_waiter();
    assert_eq!(line(&free, ok_value()).await, None);
    assert_eq!(checks.threads_started(), 2);
    let threads = probe.threads();
    assert!(threads.contains(&"mcp-output-check:srv-one".to_string()));
    assert!(threads.contains(&"mcp-output-check:srv-two".to_string()));
}

/// `owns` answers for its own pool only: a check registered through pool A
/// is A's and not B's, even under the same server and tool name. The
/// daemon's I1 wiring test relies on it: with an `owns` that always says
/// yes, a site that hands a tool a pool of its own would pass it.
#[test]
fn a_check_is_owned_by_its_own_pool_only() {
    let (probe, _release) = probe();
    let first = pool(&probe);
    let second = pool(&probe);
    let a = first.register("srv-own", "srv-own__get", &any_schema());
    let b = second.register("srv-own", "srv-own__get", &any_schema());
    assert!(first.owns(&a), "A's check is A's");
    assert!(!second.owns(&a), "A's check is not B's");
    assert!(second.owns(&b), "B's check is B's");
    assert!(!first.owns(&b), "B's check is not A's");
}

/// An idle server's thread ends when its last tool is gone, and a later
/// registration starts it again: one at a time.
#[test]
fn an_idle_servers_thread_ends_with_its_last_tool() {
    let (probe, _release) = probe();
    let checks = pool(&probe);
    let check = checks.register("srv-idle", "srv-idle__get", &any_schema());
    await_compiled(&check);
    assert!(checks.has_server("srv-idle"));
    drop(check);
    let start = Instant::now();
    while checks.has_server("srv-idle") {
        assert!(start.elapsed() < OUTER, "the idle thread never ended");
        std::thread::sleep(Duration::from_millis(2));
    }
    let again = checks.register("srv-idle", "srv-idle__get", &any_schema());
    await_compiled(&again);
    assert_eq!(checks.threads_started(), 2);
}

// ---------------------------------------------------------------------------
// I2: the wait limit; I4: bounded holding
// ---------------------------------------------------------------------------

/// A check that never answers: the call returns after about the wait with
/// the result kept and the "not checked" line.
#[tokio::test]
async fn a_stuck_check_returns_after_the_wait_with_the_result_kept() {
    let (probe, _release) = probe();
    let checks = pool(&probe);
    let check = checks.register("srv-i2", "srv-i2__get", &any_schema());
    await_compiled(&check);
    let value = serde_json::json!({"ok": true, "block": true});
    let start = Instant::now();
    let out = finish(structured(value.clone()), Some(&check))
        .await
        .unwrap();
    let took = start.elapsed();
    assert!(took >= TEST_WAIT && took < TEST_WAIT * 3, "{took:?}");
    assert_eq!(
        out,
        format!(
            "Done.\n{}\n{NOT_CHECKED_LINE}",
            serde_json::to_string_pretty(&value).unwrap()
        )
    );
}

/// Once the thread has been busy past the wait, a call does not queue
/// behind it: "not checked" at once.
#[tokio::test]
async fn a_server_busy_past_the_wait_is_not_queued_behind() {
    let (probe, _release) = probe();
    let checks = pool(&probe);
    let check = checks.register("srv-busy", "srv-busy__get", &any_schema());
    await_compiled(&check);
    assert_eq!(
        line(&check, Some(blocking_value(1))).await,
        Some(NOT_CHECKED_LINE)
    );
    // The thread's own busy clock started a little after the caller's
    // deadline did: wait until it reads busy past the wait.
    let waited = Instant::now();
    while !unavailable(&check.handle.server, TEST_WAIT) {
        assert!(waited.elapsed() < OUTER, "the thread never read busy");
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    let start = Instant::now();
    assert_eq!(line(&check, ok_value()).await, Some(NOT_CHECKED_LINE));
    assert!(start.elapsed() < TEST_WAIT / 2, "{:?}", start.elapsed());
    assert_eq!(probe.checks.load(Ordering::SeqCst), 1);
}

/// I4, the thread stuck in a check: it holds the one value it is checking;
/// a call that arrives meanwhile waits its turn (holding its own value)
/// until its wait ends, gets the "not checked" line, and leaves nothing held.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_call_queued_behind_a_stuck_check_gets_the_line_and_holds_nothing_after() {
    let (probe, _release) = probe();
    let checks = pool(&probe);
    let check = checks.register("srv-i4", "srv-i4__get", &any_schema());
    await_compiled(&check);

    let stuck = blocking_value(1);
    let behind = Arc::new(serde_json::json!({"ok": true, "n": 2}));
    let first = {
        let check = check.clone();
        let value = Arc::clone(&stuck);
        tokio::spawn(async move { line(&check, Some(value)).await })
    };
    probe.gate.await_waiter();
    let second = line(&check, Some(Arc::clone(&behind))).await;
    assert_eq!(second, Some(NOT_CHECKED_LINE));
    assert_eq!(
        Arc::strong_count(&behind),
        1,
        "a job not accepted holds nothing"
    );
    assert_eq!(first.await.unwrap(), Some(NOT_CHECKED_LINE));
    assert!(lock(&check.handle.server.state).check.is_none());
    assert_eq!(
        Arc::strong_count(&stuck),
        2,
        "the stuck thread holds its one value"
    );
    assert_eq!(probe.checks.load(Ordering::SeqCst), 1);
}

/// I4, the queue bound: with [`CHECK_QUEUE_MAX`] calls inside a check (one
/// stuck on the thread, the rest waiting their turn), one more is not
/// accepted — "not checked" at once — and its value is held by nothing
/// while the others still wait.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_call_beyond_the_queue_is_not_accepted_and_holds_nothing() {
    let (probe, _release) = probe();
    // A longer wait here, so the thread cannot read busy past it before
    // the extra call comes: what turns that call away is the bound alone.
    let wait = Duration::from_secs(1);
    let checks = OutputChecks::with_engine(Arc::new(ProbeEngine(Arc::clone(&probe))), wait);
    let check = checks.register("srv-q", "srv-q__get", &any_schema());
    await_compiled(&check);

    let mut calls = Vec::new();
    for n in 0..CHECK_QUEUE_MAX {
        let check = check.clone();
        let value = if n == 0 {
            blocking_value(0)
        } else {
            Arc::new(serde_json::json!({"ok": true, "n": n}))
        };
        calls.push(tokio::spawn(async move { line(&check, Some(value)).await }));
        if n == 0 {
            probe.gate.await_waiter();
        }
    }
    let start = Instant::now();
    while lock(&check.handle.server.state).callers < CHECK_QUEUE_MAX {
        assert!(start.elapsed() < OUTER, "the queue never filled");
        tokio::time::sleep(Duration::from_millis(1)).await;
    }

    let beyond = Arc::new(serde_json::json!({"ok": true, "n": "beyond"}));
    let start = Instant::now();
    assert_eq!(
        line(&check, Some(Arc::clone(&beyond))).await,
        Some(NOT_CHECKED_LINE)
    );
    assert!(start.elapsed() < wait / 4, "{:?}", start.elapsed());
    assert_eq!(
        Arc::strong_count(&beyond),
        1,
        "not accepted: held by nothing"
    );
    assert_eq!(
        lock(&check.handle.server.state).callers,
        CHECK_QUEUE_MAX,
        "the queue was still full"
    );
    for call in calls {
        assert_eq!(call.await.unwrap(), Some(NOT_CHECKED_LINE));
    }
    assert_eq!(lock(&check.handle.server.state).callers, 0);
    assert_eq!(probe.checks.load(Ordering::SeqCst), 1);
}

/// The queue bound is the owner-delegated pick, eight.
#[test]
fn the_queue_bound_is_eight() {
    assert_eq!(CHECK_QUEUE_MAX, 8);
}

/// I4, compiles: a tool dropped while its compile still waits behind a
/// stuck thread takes that compile with it, so nothing keeps its schema;
/// an older handle under the same name leaves the newer one's compile be.
#[test]
fn a_dropped_tools_queued_compile_goes_with_it() {
    let (probe, _release) = probe();
    let checks = pool(&probe);
    let stuck = checks.register("srv-drop", "srv-drop__stuck", &blocking_schema());
    probe.gate.await_waiter();

    let shared = any_schema();
    for n in 0..50 {
        drop(checks.register("srv-drop", &format!("srv-drop__t{n}"), &shared));
    }
    assert_eq!(
        Arc::strong_count(&shared),
        1,
        "a dropped tool's schema held"
    );

    let older = checks.register("srv-drop", "srv-drop__same", &any_schema());
    let newer_schema = any_schema();
    let newer = checks.register("srv-drop", "srv-drop__same", &newer_schema);
    drop(older);
    assert_eq!(
        Arc::strong_count(&newer_schema),
        2,
        "the newer registration's compile is still queued"
    );
    drop(newer);
    assert_eq!(Arc::strong_count(&newer_schema), 1);
    assert!(lock(&stuck.handle.server.state).compiles.is_empty());
}

/// I4, the thread busy compiling: a call's accepted job waits in the one
/// slot and is taken back when its caller stops waiting, so the value is
/// not held behind the busy thread.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_waiting_job_is_taken_back_when_its_caller_stops_waiting() {
    let (probe, _release) = probe();
    let checks = pool(&probe);
    let ready = checks.register("srv-back", "srv-back__ready", &any_schema());
    await_compiled(&ready);
    let _busy = checks.register("srv-back", "srv-back__slow", &blocking_schema());
    probe.gate.await_waiter();

    let value = Arc::new(serde_json::json!({"ok": true}));
    let call = {
        let check = ready.clone();
        let value = Arc::clone(&value);
        tokio::spawn(async move { line(&check, Some(value)).await })
    };
    let start = Instant::now();
    while lock(&ready.handle.server.state).check.is_none() {
        assert!(start.elapsed() < OUTER, "the job was never accepted");
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    assert_eq!(Arc::strong_count(&value), 2, "accepted: the slot holds it");
    assert_eq!(call.await.unwrap(), Some(NOT_CHECKED_LINE));
    assert!(lock(&ready.handle.server.state).check.is_none());
    assert_eq!(Arc::strong_count(&value), 1, "taken back");
    assert_eq!(probe.checks.load(Ordering::SeqCst), 0);
}

/// A value is moved to the thread, never deep-cloned: the thread reads the
/// very allocation the caller handed over.
#[tokio::test]
async fn a_value_is_handed_over_not_cloned() {
    struct SameValue(Arc<Mutex<Option<usize>>>);
    struct Same(Arc<Mutex<Option<usize>>>);
    impl SchemaEngine for SameValue {
        fn compile(&self, _: &Value) -> Result<Arc<dyn CompiledSchema>, String> {
            Ok(Arc::new(Same(Arc::clone(&self.0))))
        }
    }
    impl CompiledSchema for Same {
        fn is_valid(&self, value: &Value) -> bool {
            *lock(&self.0) = Some(std::ptr::from_ref(value) as usize);
            true
        }
    }
    let seen = Arc::new(Mutex::new(None));
    let checks = OutputChecks::with_engine(Arc::new(SameValue(Arc::clone(&seen))), TEST_WAIT);
    let check = checks.register("srv-move", "srv-move__get", &any_schema());
    await_compiled(&check);
    let value = Arc::new(serde_json::json!({"ok": true}));
    let address = std::ptr::from_ref(&*value) as usize;
    assert_eq!(line(&check, Some(value)).await, None);
    assert_eq!(*lock(&seen), Some(address));
}

// ---------------------------------------------------------------------------
// I5: the stop never waits on a validation thread
// ---------------------------------------------------------------------------

/// With a thread stuck in a check, a call waiting on it, every handle
/// dropped and the runtime shut down: nothing waits on the thread.
#[test]
fn the_stop_path_never_waits_on_a_stuck_thread() {
    let (probe, _release) = probe();
    let checks = pool(&probe);
    let check = checks.register("srv-i5", "srv-i5__get", &any_schema());
    await_compiled(&check);

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let waiting = {
        let check = check.clone();
        runtime.spawn(async move { check.line_for(Some(blocking_value(1))).await })
    };
    probe.gate.await_waiter();
    let start = Instant::now();
    drop(waiting);
    drop(check);
    drop(checks);
    runtime.shutdown_timeout(Duration::from_secs(5));
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "the stop waited on the validation thread: {:?}",
        start.elapsed()
    );
    assert!(
        probe.gate.waiting.load(Ordering::SeqCst),
        "the thread is still stuck"
    );
}

// ---------------------------------------------------------------------------
// I6: registration never waits on a compile
// ---------------------------------------------------------------------------

/// 500 tools register while the first compile is stuck: every
/// registration returns at once.
#[test]
fn registration_returns_while_a_compile_is_stuck() {
    let (probe, _release) = probe();
    let checks = Arc::new(pool(&probe));
    let (done, finished) = std::sync::mpsc::channel();
    let registering = Arc::clone(&checks);
    let worker = std::thread::spawn(move || {
        let handles: Vec<OutputCheck> = (0..500)
            .map(|n| registering.register("srv-i6", &format!("srv-i6__t{n}"), &blocking_schema()))
            .collect();
        let _ = done.send(handles.len());
    });
    let registered = finished
        .recv_timeout(Duration::from_secs(2))
        .expect("registration waited on a compile");
    assert_eq!(registered, 500);
    probe.gate.await_waiter();
    assert_eq!(
        probe.compiles.load(Ordering::SeqCst),
        1,
        "one compile, stuck"
    );
    probe.gate.release();
    worker.join().unwrap();
}

// ---------------------------------------------------------------------------
// A panic in the thread
// ---------------------------------------------------------------------------

/// A panic in a compile stops the server's checking for good: logged once,
/// every later call "not checked", no second thread ever; another server
/// is unaffected.
#[tracing_test::traced_test]
#[tokio::test]
async fn a_panic_stops_the_servers_checks_without_a_second_thread() {
    let (probe, _release) = probe();
    let checks = pool(&probe);
    let first = checks.register(
        "srv-panic",
        "srv-panic__get",
        &schema(serde_json::json!({"panic": true})),
    );
    let start = Instant::now();
    while !lock(&first.handle.server.state).stopped {
        assert!(
            start.elapsed() < OUTER,
            "the panic never stopped the thread"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    for _ in 0..3 {
        let again = checks.register("srv-panic", "srv-panic__get", &any_schema());
        assert_eq!(line(&again, ok_value()).await, Some(NOT_CHECKED_LINE));
    }
    assert_eq!(line(&first, ok_value()).await, Some(NOT_CHECKED_LINE));
    assert_eq!(checks.threads_started(), 1);
    assert_eq!(probe.compiles.load(Ordering::SeqCst), 1);

    let other = checks.register("srv-panic-other", "srv-panic-other__get", &any_schema());
    await_compiled(&other);
    assert_eq!(line(&other, ok_value()).await, None);
    assert_eq!(checks.threads_started(), 2);

    let logs = String::from_utf8(
        tracing_test::internal::global_buf()
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone(),
    )
    .unwrap();
    assert_eq!(
        logs.lines()
            .filter(|l| l.contains("stopped after a panic")
                && l.trim_end().ends_with("server=srv-panic"))
            .count(),
        1,
        "{logs}"
    );
}

/// A panic in a check answers that call "not checked" at once, not after
/// the whole wait, and stops the server's checks.
#[tokio::test]
async fn a_panic_in_a_check_answers_not_checked_at_once() {
    let (probe, _release) = probe();
    let checks = OutputChecks::with_engine(
        Arc::new(ProbeEngine(Arc::clone(&probe))),
        Duration::from_secs(5),
    );
    let check = checks.register("srv-panic-check", "srv-panic-check__get", &any_schema());
    await_compiled(&check);
    let start = Instant::now();
    let panicking = Some(Arc::new(serde_json::json!({"ok": true, "panic": true})));
    assert_eq!(line(&check, panicking).await, Some(NOT_CHECKED_LINE));
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "{:?}",
        start.elapsed()
    );
    assert_eq!(line(&check, ok_value()).await, Some(NOT_CHECKED_LINE));
    assert_eq!(checks.threads_started(), 1);
}
