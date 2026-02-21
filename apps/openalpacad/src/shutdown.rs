use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

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
    use std::os::fd::AsRawFd;

    let fd = std::io::stdin().as_raw_fd();

    // Non-interactive launch (GUI/service/redirect): no warning noise.
    // SAFETY: `isatty` is thread-safe and does not retain pointers.
    if unsafe { libc::isatty(fd) } != 1 {
        return;
    }

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
