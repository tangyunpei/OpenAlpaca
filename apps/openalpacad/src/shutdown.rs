use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

/// Install OS signal handlers (SIGINT + SIGTERM) using `signal-hook`.
///
/// **MUST be called in `fn main()` before the tokio runtime starts.**
///
/// This uses `signal-hook::flag::register()` which calls `sigaction()` immediately
/// in the current (and only) thread. The handler sets an `AtomicBool` flag from
/// within the signal handler context — this works regardless of which thread
/// receives the signal, unlike tokio's self-pipe approach which can miss signals
/// delivered to threads that have the signal masked (e.g. ONNX Runtime thread pool).
///
/// Returns the `Arc<AtomicBool>` flag that will be set to `true` on SIGINT/SIGTERM.
pub fn install_signal_handlers() -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(false));

    // Register SIGINT (Ctrl+C)
    signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&flag))
        .expect("Failed to register SIGINT handler");

    // Register SIGTERM
    signal_hook::flag::register(signal_hook::consts::SIGTERM, Arc::clone(&flag))
        .expect("Failed to register SIGTERM handler");

    flag
}

/// Warn once at startup when the current TTY is configured such that Ctrl+C may
/// not generate SIGINT for this process.
///
/// Best-effort only:
/// - If stdin is not a TTY, returns silently.
/// - If terminal attrs cannot be read, returns silently.
/// - Never fails startup.
#[cfg(unix)]
pub fn warn_if_ctrl_c_unavailable_on_tty() {
    use std::io::IsTerminal;
    use std::os::fd::AsRawFd;

    let stdin = std::io::stdin();

    // Non-interactive launch (GUI/service/redirect): no warning noise.
    if !stdin.is_terminal() {
        return;
    }
    let fd = stdin.as_raw_fd();

    let mut termios = std::mem::MaybeUninit::<libc::termios>::uninit();
    // SAFETY: valid fd from stdin; `tcgetattr` initializes `termios` on success.
    if unsafe { libc::tcgetattr(fd, termios.as_mut_ptr()) } != 0 {
        return;
    }
    // SAFETY: guarded by successful `tcgetattr` above.
    let termios = unsafe { termios.assume_init() };

    let isig_enabled = (termios.c_lflag & libc::ISIG) != 0;
    let intr = termios.c_cc[libc::VINTR];

    if !isig_enabled || intr != 0x03 {
        tracing::warn!(
            "TTY config may prevent Ctrl+C from sending SIGINT (isig={}, intr=0x{:02x}, expected 0x03). \
             If Ctrl+C is ignored, run `stty sane` or `stty isig intr '^C'`.",
            isig_enabled,
            intr
        );
    }
}

/// No-op on non-Unix targets.
#[cfg(not(unix))]
pub fn warn_if_ctrl_c_unavailable_on_tty() {}

/// Wait asynchronously until the shutdown flag is set.
///
/// Polls the `AtomicBool` flag with a short sleep interval. This is intentionally
/// simple and robust — no self-pipes, no tokio signal infrastructure, just an
/// atomic flag check.
pub async fn wait_for_shutdown(flag: &AtomicBool) {
    loop {
        if flag.load(Ordering::Relaxed) {
            tracing::info!("Received shutdown signal (SIGINT or SIGTERM)");
            return;
        }
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
    }
}

/// The instant the force-exit watchdog ends the process: what it published
/// when it saw the cancellation, or — unset, the watchdog not having seen it
/// yet — the whole window from now, since nothing of it has been spent.
pub(crate) fn force_exit_deadline(force_exit_at: &OnceLock<Instant>) -> Instant {
    force_exit_at
        .get()
        .copied()
        .unwrap_or_else(|| Instant::now() + crate::FORCE_EXIT_GRACE)
}

/// What the connector and wake stages leave of the shutdown window: they end
/// this long before the force-exit instant, so the stages after a stop that
/// never returns — the handles after it, the wake manager's stop, the
/// discovery file's removal — run before the watchdog, not in a race with it.
/// The extension sweep leaves these stages its own reserve
/// (`managers::extensions::POST_SWEEP_RESERVE`, 3 s); this is the tail's.
pub(crate) const TAIL_RESERVE: Duration = Duration::from_secs(1);

/// The instant the connector and wake stages end: [`TAIL_RESERVE`] before the
/// force-exit instant ([`force_exit_deadline`]). Already past when the stages
/// before them spent the window — [`within`] still polls a stage once.
pub(crate) fn stage_deadline(force_exit_at: &OnceLock<Instant>) -> Instant {
    let force_exit = force_exit_deadline(force_exit_at);
    force_exit.checked_sub(TAIL_RESERVE).unwrap_or(force_exit)
}

/// The force-exit watchdog. Once `cancel` fires it publishes, in the cell it
/// returns, the instant it will end the process — `grace` from then — so
/// every stage of the tail can bound itself by what is *left* of the window;
/// then it waits that long and, the tail not having returned by then, removes
/// the discovery file the boot wrote ([`before_forced_exit`]) and calls
/// `exit(1)` — `std::process::exit` in the daemon.
pub(crate) fn spawn_force_exit_watchdog(
    cancel: CancellationToken,
    discovery: PathBuf,
    grace: Duration,
    exit: impl FnOnce(i32) + Send + 'static,
) -> Arc<OnceLock<Instant>> {
    let force_exit_at = Arc::new(OnceLock::new());
    let published = force_exit_at.clone();
    tokio::spawn(async move {
        cancel.cancelled().await;
        let _ = published.set(Instant::now() + grace);
        tokio::time::sleep(grace).await;
        tracing::warn!("Graceful shutdown timed out after {grace:?}, forcing exit");
        before_forced_exit(&discovery);
        exit(1);
    });
    force_exit_at
}

/// Runs one stage of the shutdown tail until `deadline` and no longer.
///
/// A stage that has not finished by then is given up with a warning naming
/// it, and the tail goes on, so a stop that never returns — a Telegram
/// dispatcher still draining, a wake watcher stuck — cannot keep the stages
/// after it from running before the watchdog ends the process. The stage is
/// always polled once, even past the deadline: a stage whose work is a signal
/// — a connector's cancel token — has given it by then.
pub(crate) async fn within<F: Future>(stage: &str, deadline: Instant, fut: F) -> Option<F::Output> {
    match tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), fut).await {
        Ok(output) => Some(output),
        Err(_) => {
            tracing::warn!(
                "{stage} did not finish within the shutdown window; the shutdown goes on without it"
            );
            None
        }
    }
}

/// How long the blocking work still running once the shutdown tail has run
/// — a memory index rebuild, a learning chunk still storing — gets before
/// the process ends anyway ([`finish_runtime`]). What it has not finished by
/// then is cut by the exit: an index rebuild or a learning chunk is redone
/// at the next start, and a file a tool was still writing is left as the cut
/// leaves it. An attachment's text extraction is cut whatever it does in
/// that second: the task awaiting it is dropped, so its result is not kept,
/// and the next start counts it as an interrupted attempt. A fixed second,
/// not what is left of [`FORCE_EXIT_GRACE`](crate::FORCE_EXIT_GRACE): the
/// tail is bounded by that window, and this is the one bound after it.
pub(crate) const RUNTIME_END: Duration = Duration::from_secs(1);

/// Ends the daemon's runtime once `async_main` has returned, waiting at most
/// `bound` for its blocking threads.
///
/// Dropping a `Runtime` waits for every blocking task with no bound, so a
/// stop during a long extraction kept the process alive — its lock held,
/// its `discovery.json` already gone — for as long as that extraction ran.
/// `Runtime::shutdown_timeout` tells the worker threads to stop and waits at
/// most `bound` for every runtime thread; the workers drop their async tasks
/// during that wait, running those tasks' destructors (a running shell
/// group's `killpg`, a plugin child's `kill_on_drop`), so a bound of a
/// second leaves them time to, where `shutdown_background` (a zero bound)
/// or a `process::exit` leaves none. A blocking thread still running after
/// `bound` is left to the process's exit.
pub(crate) fn finish_runtime(runtime: tokio::runtime::Runtime, bound: Duration) {
    runtime.shutdown_timeout(bound);
}

/// The force-exit watchdog's last step before `process::exit(1)`: the
/// discovery file the boot wrote is removed, so no client reads the port and
/// token of a daemon that is gone. Best effort — a file the shutdown tail
/// already removed is no error, and a removal that fails is logged and the
/// exit goes ahead. The file is this daemon's: it holds the single-instance
/// lock until the process ends, so no other daemon can have written it.
pub(crate) fn before_forced_exit(discovery: &Path) {
    match std::fs::remove_file(discovery) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => tracing::warn!(
            "Failed to remove {} before the forced exit: {e}",
            discovery.display()
        ),
    }
}
