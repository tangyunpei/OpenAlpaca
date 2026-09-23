use crate::commands::{status, tail};
use crate::manager;
use anyhow::Result;
use clap::{Args, Subcommand};
use openalpaca_storage::daemon_lifecycle::{self, StopOutcome};
use openalpaca_storage::store;
use std::path::Path;

#[derive(Args)]
pub struct DaemonArgs {
    #[command(subcommand)]
    pub action: DaemonAction,
}

#[derive(Subcommand)]
pub enum DaemonAction {
    /// Show daemon status
    Status,
    /// Stream events from daemon (Ctrl+C to stop)
    Tail {
        /// Number of events to show (0 = unlimited)
        #[arg(short, long, default_value = "0")]
        count: usize,
    },
    /// Start Daemon (and GUI by default)
    Start {
        /// Start only the daemon, skip GUI
        #[arg(long)]
        daemon_only: bool,
    },
    /// Stop the daemon (an open app window stays open and shows it stopped)
    Stop,
    /// Restart Daemon
    Restart,
}

pub async fn run(args: DaemonArgs) -> Result<()> {
    match args.action {
        DaemonAction::Status => status::run().await,
        DaemonAction::Tail { count } => tail::run(count).await,
        DaemonAction::Start { daemon_only } => {
            manager::start_daemon()?;
            if !daemon_only {
                // Brief wait for daemon to initialize resources if needed
                std::thread::sleep(std::time::Duration::from_secs(2));
                manager::start_gui()?;
            }
            Ok(())
        }
        DaemonAction::Stop => {
            // The daemon only: an open app window stays open, keeps its
            // unsent draft and shows `stopped elsewhere` with a Start button
            // (owner, 2026-09-22). `openalpaca gui stop` is the verb that
            // quits the app.
            let gone = stop_and_wait(false)?;
            if !gone {
                std::process::exit(2);
            }
            Ok(())
        }
        DaemonAction::Restart => {
            // Starting before the old daemon is really gone fails one of two
            // ways: `start_daemon` sees it still running and does nothing, or
            // the new daemon loses the non-blocking singleton lock and exits
            // 1 — either way the user is left with no daemon and no reason.
            if !stop_and_wait(true)? {
                std::process::exit(2);
            }
            manager::start_daemon()
        }
    }
}

/// Ask the daemon to stop, then wait until its process has exited and the
/// singleton lock is free. False — with the reason printed — when it is not
/// gone within `daemon_lifecycle::STOP_TIMEOUT`.
fn stop_and_wait(restarting: bool) -> Result<bool> {
    let manager::StopRequest::Signalled(pid) = manager::stop_daemon()? else {
        return Ok(true);
    };
    let outcome = daemon_lifecycle::wait_for_pid_exit(pid, daemon_lifecycle::STOP_TIMEOUT);
    if outcome.is_clear() {
        println!("✅ Daemon stopped.");
        return Ok(true);
    }
    if let Some(report) = stop_failure_report(pid, &outcome, restarting) {
        eprintln!("{report}");
    }
    Ok(false)
}

/// [`stop_failure_message`] with the daemon log resolved through
/// `store::daemon_log_path()` — never a literal `~/.openalpaca`, because
/// `OPENALPACA_HOME_STORE` moves the root. A log that is not there is not
/// offered: telling the user to `tail` a missing file is no help.
fn stop_failure_report(pid: u32, outcome: &StopOutcome, restarting: bool) -> Option<String> {
    let log = store::daemon_log_path().ok().filter(|path| path.is_file());
    stop_failure_message(pid, outcome, restarting, log.as_deref())
}

/// What to tell the user when a stop did not finish: the pid, how long we
/// waited, and the exact commands that finish the job by hand. `None` for an
/// outcome that is clear.
fn stop_failure_message(
    pid: u32,
    outcome: &StopOutcome,
    restarting: bool,
    log: Option<&Path>,
) -> Option<String> {
    let not_restarted = if restarting {
        ", so nothing was restarted"
    } else {
        ""
    };
    match outcome {
        StopOutcome::NotRunning | StopOutcome::Stopped { .. } => None,
        StopOutcome::StillAlive { waited, .. } => {
            let secs = waited.as_secs();
            let inspect = match log {
                Some(log) => format!(
                    "Find out what it is doing — `tail -n 50 {}` — then stop it",
                    shell_word(&log.display().to_string())
                ),
                None => "Stop it".to_string(),
            };
            let then_start = if restarting {
                " and run `openalpaca daemon start`"
            } else {
                ""
            };
            Some(format!(
                "❌ The daemon (PID {pid}) is still running {secs}s after it was asked to stop{not_restarted}.\n   \
                 {inspect} by hand with `kill -9 {pid}`{then_start}."
            ))
        }
        StopOutcome::LockStillHeld { waited } => {
            let secs = waited.as_secs();
            let then_start = if restarting {
                ", then run `openalpaca daemon start` once it is clear"
            } else {
                ""
            };
            Some(format!(
                "❌ The daemon (PID {pid}) exited, but something still holds the single-instance lock {secs}s after it was asked to stop{not_restarted}.\n   \
                 Another daemon may already be running on this store. Check `openalpaca daemon status`{then_start}."
            ))
        }
    }
}

/// `word` as one shell word: as it is when it needs no quoting, single-quoted
/// otherwise — so the printed command can be pasted even when the store root
/// has a space in it.
fn shell_word(word: &str) -> String {
    let plain = !word.is_empty()
        && word
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "/._-+@%:,=".contains(c));
    if plain {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', "'\\''"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::EnvSandbox;
    use std::time::Duration;

    const PID: u32 = 41_287;
    const BUDGET: Duration = daemon_lifecycle::STOP_TIMEOUT;

    fn still_alive() -> StopOutcome {
        StopOutcome::StillAlive {
            pid: PID,
            waited: BUDGET,
        }
    }

    /// The restart that could not finish: the pid, the budget, the exact way
    /// to finish by hand, and a log path that is the one `store` resolves —
    /// under `OPENALPACA_HOME_STORE`, not a hard-coded `~/.openalpaca`.
    #[test]
    fn a_restart_that_outlives_the_budget_names_the_pid_the_budget_the_fallback_and_the_log() {
        let tmp = tempfile::tempdir().unwrap();
        let _env = EnvSandbox::enter(tmp.path());
        let log = store::daemon_log_path().unwrap();
        assert!(
            log.starts_with(tmp.path()),
            "the log path escaped the sandbox ({}); refusing to write",
            log.display()
        );
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();
        std::fs::write(&log, b"last words\n").unwrap();

        let report = stop_failure_report(PID, &still_alive(), true).expect("a failure is reported");

        assert!(report.contains("The daemon (PID 41287) is still running 15s after it was asked to stop, so nothing was restarted."), "{report}");
        assert!(
            report.contains(&format!("`tail -n 50 {}`", log.display())),
            "{report}"
        );
        assert!(
            report.contains("`kill -9 41287` and run `openalpaca daemon start`."),
            "{report}"
        );
        assert!(!report.contains("~/.openalpaca"), "{report}");
    }

    /// A daemon started some other way may have no `daemon.log`; the report
    /// then offers only the commands that work.
    #[test]
    fn a_missing_daemon_log_is_not_offered() {
        let tmp = tempfile::tempdir().unwrap();
        let _env = EnvSandbox::enter(tmp.path());
        assert!(store::daemon_log_path().unwrap().starts_with(tmp.path()));

        let report = stop_failure_report(PID, &still_alive(), true).expect("a failure is reported");

        assert!(!report.contains("tail"), "{report}");
        assert!(
            report.contains(
                "Stop it by hand with `kill -9 41287` and run `openalpaca daemon start`."
            ),
            "{report}"
        );
    }

    /// `stop` did not promise a restart, so its report neither says nothing
    /// was restarted nor tells the user to start one.
    #[test]
    fn a_stop_that_outlives_the_budget_does_not_talk_about_restarting() {
        let report = stop_failure_message(PID, &still_alive(), false, None).unwrap();
        assert!(
            report.contains("(PID 41287) is still running 15s after it was asked to stop."),
            "{report}"
        );
        assert!(report.contains("`kill -9 41287`."), "{report}");
        assert!(!report.contains("restart"), "{report}");
        assert!(!report.contains("daemon start"), "{report}");
    }

    #[test]
    fn a_lock_still_held_after_the_process_exited_says_so_and_points_at_status() {
        let held = StopOutcome::LockStillHeld { waited: BUDGET };

        let restart = stop_failure_message(PID, &held, true, None).unwrap();
        assert!(restart.contains("(PID 41287) exited, but something still holds the single-instance lock 15s after it was asked to stop, so nothing was restarted."), "{restart}");
        assert!(restart.contains("Check `openalpaca daemon status`, then run `openalpaca daemon start` once it is clear."), "{restart}");

        let stop = stop_failure_message(PID, &held, false, None).unwrap();
        assert!(stop.contains("Check `openalpaca daemon status`."), "{stop}");
        assert!(!stop.contains("daemon start"), "{stop}");
    }

    #[test]
    fn a_clear_outcome_reports_nothing() {
        for outcome in [
            StopOutcome::NotRunning,
            StopOutcome::Stopped {
                waited: Duration::from_millis(300),
            },
        ] {
            assert_eq!(stop_failure_message(PID, &outcome, true, None), None);
        }
    }

    #[test]
    fn a_log_path_that_needs_quoting_is_printed_as_one_shell_word() {
        assert_eq!(shell_word("/tmp/a/daemon.log"), "/tmp/a/daemon.log");
        assert_eq!(
            shell_word("/Users/Jo Doe/it's/daemon.log"),
            "'/Users/Jo Doe/it'\\''s/daemon.log'"
        );
    }
}
