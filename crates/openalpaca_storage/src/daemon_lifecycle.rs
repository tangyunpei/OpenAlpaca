//! "Is the daemon running, and is it really gone yet?" — the one
//! implementation every surface that checks, restarts or replaces a daemon
//! uses: the CLI's `daemon stop|restart|start`, the factory reset's refusal
//! and the app shell.
//!
//! **Waiting on the port is wrong.** The daemon drops its listener when
//! `axum::serve` resolves, which is *before* it flushes the cost tracker and
//! the session logs, sweeps MCP and plugin children, stops the connectors and
//! removes `discovery.json` — a tail bounded at 10 s. The singleton lock is
//! held for all of it and is released by the OS when the process exits,
//! whether it exited cleanly or was force-exited by its own watchdog (which
//! skips the `discovery.json` removal, so a stale file is normal after a
//! forced exit). The daemon takes that lock without blocking and exits 1 when
//! it cannot, so a replacement started early leaves nothing running. The
//! order is therefore: the process, then the lock.
//!
//! This module signals nothing. Killing is platform work and belongs to the
//! surface that owns a kill (today: the CLI, on Unix). Waiting is shared.

use std::time::{Duration, Instant};

/// The default budget for a daemon to be completely gone.
///
/// The daemon's own force-exit watchdog is 10 s (`openalpacad`'s
/// `FORCE_EXIT_GRACE`), so this is that plus 5 s for process teardown and for
/// the OS to release the advisory lock. A stop that has not finished by then
/// is not slow, it is stuck.
pub const STOP_TIMEOUT: Duration = Duration::from_secs(15);

/// How often the two conditions are re-read.
pub const STOP_POLL: Duration = Duration::from_millis(100);

/// What waiting for a daemon to go away concluded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopOutcome {
    /// Nothing was running when we started looking (no `discovery.json`, or it
    /// names a pid that is not a live `openalpacad`). Not an error: the
    /// caller's goal is already met.
    NotRunning,
    /// The process exited and the singleton lock is free. It is safe to start.
    Stopped { waited: Duration },
    /// The process exited but the lock is still held after the budget. Someone
    /// else holds it — another daemon, or a stale holder — so starting now
    /// would exit 1 on the lock.
    LockStillHeld { waited: Duration },
    /// The process is still alive after the budget.
    StillAlive { pid: u32, waited: Duration },
}

impl StopOutcome {
    /// Whether it is safe to start a daemon now.
    pub fn is_clear(&self) -> bool {
        matches!(self, StopOutcome::NotRunning | StopOutcome::Stopped { .. })
    }
}

/// Whether `pid` is a live `openalpacad`.
///
/// A stale `discovery.json` (a crash, a reboot, a forced exit) can name a pid
/// the OS has since recycled; trusting it blindly would make a caller signal,
/// wait on or report an unrelated process. So the process's name — or its
/// executable's file name — must say `openalpacad`.
pub fn pid_is_daemon(pid: u32) -> bool {
    let mut system = sysinfo::System::new();
    system.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    match system.process(sysinfo::Pid::from_u32(pid)) {
        Some(process) => {
            process.name().to_string_lossy().contains("openalpacad")
                || process
                    .exe()
                    .and_then(|path| path.file_name())
                    .map(|name| name.to_string_lossy().contains("openalpacad"))
                    .unwrap_or(false)
        }
        None => false,
    }
}

/// The pid `discovery.json` names, when it names a live `openalpacad`.
///
/// A missing or unreadable `discovery.json` is "not running": a daemon that is
/// up has written a well-formed one.
pub fn running_daemon_pid() -> Option<u32> {
    let discovery = crate::discovery::read_discovery().ok().flatten()?;
    pid_is_daemon(discovery.pid).then_some(discovery.pid)
}

/// Whether the singleton lock can be taken right now.
///
/// Takes it and immediately drops it — the guard releases on drop — so this is
/// a probe, not a claim. A caller that acts on `true` still races anything
/// else probing at the same instant; the daemon's own non-blocking
/// acquisition is the real arbiter and refuses cleanly. Like that
/// acquisition, the probe creates `state/` and the lock file when they are
/// missing, so call it only once a daemon has been found.
pub fn singleton_lock_is_free() -> bool {
    crate::discovery::acquire_single_instance_lock(false).is_ok()
}

/// Wait for the daemon to be completely gone: first the process, then the
/// lock, polling every [`STOP_POLL`] for at most `timeout`.
///
/// Reads the real `discovery.json`, process table and lock. See
/// [`wait_for_exit_with`] for the injected form every branch is tested
/// through.
pub fn wait_for_daemon_exit(timeout: Duration) -> StopOutcome {
    let Some(pid) = running_daemon_pid() else {
        return StopOutcome::NotRunning;
    };
    let started = Instant::now();
    wait_for_exit_with(
        pid,
        || pid_is_daemon(pid),
        singleton_lock_is_free,
        timeout,
        || started.elapsed(),
        || std::thread::sleep(STOP_POLL),
    )
}

/// The decision, with the world injected: whether `pid` is alive, whether the
/// lock is free, how long has passed, and how to wait between reads.
///
/// Touches no filesystem, reads no process table and keeps no clock of its
/// own, so every branch is a deterministic unit test.
pub fn wait_for_exit_with(
    pid: u32,
    mut alive: impl FnMut() -> bool,
    mut lock_free: impl FnMut() -> bool,
    timeout: Duration,
    mut elapsed: impl FnMut() -> Duration,
    mut sleep: impl FnMut(),
) -> StopOutcome {
    while alive() {
        let waited = elapsed();
        if waited >= timeout {
            return StopOutcome::StillAlive { pid, waited };
        }
        sleep();
    }
    while !lock_free() {
        let waited = elapsed();
        if waited >= timeout {
            return StopOutcome::LockStillHeld { waited };
        }
        sleep();
    }
    StopOutcome::Stopped { waited: elapsed() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    const PID: u32 = 41_287;
    const POLL: Duration = Duration::from_millis(100);

    /// Drives `wait_for_exit_with` on a fake clock that only the injected
    /// sleep advances, so a 15 s budget costs no real time.
    ///
    /// `alive_polls` is how many reads say the process is still alive before
    /// it exits (`None`: it never exits); `held_polls` the same for the lock.
    fn run(alive_polls: Option<u32>, held_polls: Option<u32>, timeout: Duration) -> StopOutcome {
        let now = Cell::new(Duration::ZERO);
        let alive_reads = Cell::new(0u32);
        let lock_reads = Cell::new(0u32);
        wait_for_exit_with(
            PID,
            || {
                let n = alive_reads.get();
                alive_reads.set(n + 1);
                alive_polls.is_none_or(|limit| n < limit)
            },
            || {
                let n = lock_reads.get();
                lock_reads.set(n + 1);
                held_polls.is_some_and(|limit| n >= limit)
            },
            timeout,
            || now.get(),
            || now.set(now.get() + POLL),
        )
    }

    #[test]
    fn a_process_that_is_already_gone_with_a_free_lock_is_stopped_at_once() {
        let outcome = run(Some(0), Some(0), STOP_TIMEOUT);
        assert_eq!(
            outcome,
            StopOutcome::Stopped {
                waited: Duration::ZERO
            }
        );
        assert!(outcome.is_clear());
    }

    /// The restart case: the process lingers through its shutdown tail, then
    /// the lock lingers a little past the exit. `waited` is the whole of both.
    #[test]
    fn a_process_alive_for_some_polls_then_a_lock_that_clears_is_stopped() {
        let outcome = run(Some(5), Some(3), STOP_TIMEOUT);
        assert_eq!(outcome, StopOutcome::Stopped { waited: POLL * 8 });
        assert!(outcome.is_clear());
    }

    #[test]
    fn a_process_that_never_exits_is_still_alive_at_the_budget() {
        let outcome = run(None, Some(0), STOP_TIMEOUT);
        assert_eq!(
            outcome,
            StopOutcome::StillAlive {
                pid: PID,
                waited: STOP_TIMEOUT
            }
        );
        assert!(!outcome.is_clear());
    }

    /// The lock is never probed while the process is alive: a live daemon
    /// holds it by definition, and a probe that succeeded against a process
    /// mid-exit would say "clear" too early.
    #[test]
    fn the_lock_is_not_read_until_the_process_has_exited() {
        let lock_reads = Cell::new(0u32);
        let now = Cell::new(Duration::ZERO);
        let outcome = wait_for_exit_with(
            PID,
            || true,
            || {
                lock_reads.set(lock_reads.get() + 1);
                true
            },
            STOP_TIMEOUT,
            || now.get(),
            || now.set(now.get() + POLL),
        );
        assert!(matches!(outcome, StopOutcome::StillAlive { .. }));
        assert_eq!(lock_reads.get(), 0);
    }

    /// A forced exit or another daemon: the process is gone, but starting now
    /// would lose the non-blocking lock race, so the budget runs out on the
    /// lock and the outcome says so rather than "stopped".
    #[test]
    fn a_process_that_exits_while_the_lock_stays_held_is_lock_still_held() {
        let outcome = run(Some(2), None, STOP_TIMEOUT);
        assert_eq!(
            outcome,
            StopOutcome::LockStillHeld {
                waited: STOP_TIMEOUT
            }
        );
        assert!(!outcome.is_clear());
    }

    #[test]
    fn not_running_is_clear() {
        assert!(StopOutcome::NotRunning.is_clear());
    }
}
