//! Platform-specific helpers for tool execution.

use std::process::{Output, Stdio};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::process::{Child, Command};

/// Build a `tokio::process::Command` that runs the given shell command
/// using the platform's default shell.
///
/// - macOS / Linux: `sh -c "<cmd>"`
/// - Windows: `cmd /c "<cmd>"`
pub fn shell_command(cmd: &str) -> tokio::process::Command {
    if cfg!(target_os = "windows") {
        let mut c = tokio::process::Command::new("cmd");
        c.arg("/c").arg(cmd);
        c
    } else {
        let mut c = tokio::process::Command::new("sh");
        c.arg("-c").arg(cmd);
        c
    }
}

/// Why [`run_bounded`] produced no output.
#[derive(Debug)]
pub enum RunFailure {
    /// The program could not be started.
    Spawn(std::io::Error),
    /// `timeout` passed; the process (and its group, on unix) was killed.
    TimedOut,
    /// Reading its output or waiting for it failed; it was killed the same
    /// way.
    Io(std::io::Error),
}

/// Run `cmd` to its end with stdin null and stdout/stderr captured. Kill it
/// when `timeout` passes **or the returned future is dropped**. The sandbox's
/// per-tool timeout and a run cancelled mid-call both drop the tool's
/// future (R45, storage design §11).
///
/// unix: the child is started in a new process group (`process_group(0)`).
/// A guard owning the `Child` sends `killpg(pgid, SIGKILL)` on every exit
/// that is not a normal end, before it lets the `Child` drop, so the
/// processes the command started go too. A normal end disarms the guard. A
/// process that leaves the group on its own (`setsid`) is out of reach.
/// Windows: `kill_on_drop(true)` kills the direct child only, and processes
/// it started survive ("where the platform allows", R45).
pub async fn run_bounded(mut cmd: Command, timeout: Duration) -> Result<Output, RunFailure> {
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    {
        cmd.process_group(0);
    }
    let mut run = KillGuard::new(cmd.spawn().map_err(RunFailure::Spawn)?);
    let out_pipe = run.child.stdout.take();
    let err_pipe = run.child.stderr.take();
    let ended = tokio::time::timeout(timeout, async {
        let (status, stdout, stderr) =
            tokio::try_join!(run.child.wait(), read_all(out_pipe), read_all(err_pipe))?;
        Ok::<Output, std::io::Error>(Output {
            status,
            stdout,
            stderr,
        })
    })
    .await;
    match ended {
        Ok(Ok(output)) => {
            run.disarm();
            Ok(output)
        }
        Ok(Err(e)) => Err(RunFailure::Io(e)),
        Err(_elapsed) => Err(RunFailure::TimedOut),
    }
}

/// Everything a pipe yields until it closes; nothing for a pipe not taken.
async fn read_all(pipe: Option<impl AsyncRead + Unpin>) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    if let Some(mut pipe) = pipe {
        pipe.read_to_end(&mut bytes).await?;
    }
    Ok(bytes)
}

/// The running child of [`run_bounded`], killed with its process group on
/// every exit that is not a normal end (R45).
struct KillGuard {
    child: Child,
    /// The group to kill: the child's pid, which `process_group(0)` made the
    /// group's id. `None` once a normal end disarmed the guard. Never
    /// `Some(0)`: `killpg(0, …)` would signal the daemon's own group.
    #[cfg(unix)]
    group: Option<i32>,
}

impl KillGuard {
    fn new(child: Child) -> Self {
        Self {
            #[cfg(unix)]
            group: child
                .id()
                .and_then(|pid| i32::try_from(pid).ok())
                .filter(|pid| *pid > 0),
            child,
        }
    }

    /// A normal end: the group is not signalled.
    fn disarm(&mut self) {
        #[cfg(unix)]
        {
            self.group = None;
        }
    }
}

#[cfg(unix)]
impl Drop for KillGuard {
    fn drop(&mut self) {
        if let Some(group) = self.group.take() {
            // SAFETY: `killpg` only sends a signal. `group` is the positive id
            // of the group this run's child leads, signalled only while the
            // run has not ended normally, so the child or a process it started
            // still holds it.
            unsafe {
                libc::killpg(group, libc::SIGKILL);
            }
        }
        // `child` drops after this body, and `kill_on_drop(true)` kills it:
        // on Windows the only process that is killed.
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shell_command_creates_command() {
        // Verify it doesn't panic — actual execution is OS-dependent
        let _cmd = shell_command("echo hello");
    }

    #[tokio::test]
    async fn a_program_that_cannot_start_is_a_spawn_failure() {
        let dir = tempfile::tempdir().unwrap();
        let mut cmd = Command::new(dir.path().join("no-such-program"));
        cmd.current_dir(dir.path());
        let outcome = run_bounded(cmd, Duration::from_secs(5)).await;
        assert!(matches!(outcome, Err(RunFailure::Spawn(_))), "{outcome:?}");
    }

    /// `sleep 30` started in the background, its pid written to `pidfile`,
    /// and waited on: a command that started a process of its own (R45).
    #[cfg(unix)]
    fn starts_a_sleep(dir: &std::path::Path, pidfile: &std::path::Path) -> Command {
        let mut cmd = shell_command(&format!(
            "sleep 30 & echo $! > '{}'; wait",
            pidfile.display()
        ));
        cmd.current_dir(dir);
        cmd
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_command_that_ends_returns_its_status_and_both_streams() {
        let dir = tempfile::tempdir().unwrap();
        let mut cmd = shell_command("printf out; printf err >&2; exit 3");
        cmd.current_dir(dir.path());
        let output = run_bounded(cmd, Duration::from_secs(5)).await.unwrap();
        assert_eq!(output.status.code(), Some(3));
        assert_eq!(output.stdout, b"out");
        assert_eq!(output.stderr, b"err");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_command_past_its_time_is_killed_with_the_processes_it_started() {
        use crate::test_util::{GONE_WITHIN, KillOnFail, pid_from_file, wait_until_gone};
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("pid");
        let outcome = run_bounded(starts_a_sleep(dir.path(), &pidfile), Duration::from_secs(1)).await;
        let pid = pid_from_file(&pidfile, Duration::from_secs(1)).await;
        let _kill = KillOnFail(pid);
        assert!(matches!(outcome, Err(RunFailure::TimedOut)), "{outcome:?}");
        assert!(
            wait_until_gone(pid, GONE_WITHIN).await,
            "the background sleep {pid} outlived the timeout"
        );
    }

    /// The sandbox's per-tool timeout and a cancelled run both drop the
    /// tool's future, and dropping it is a kill too.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_dropped_run_kills_the_processes_the_command_started() {
        use crate::test_util::{GONE_WITHIN, KillOnFail, pid_from_file, wait_until_gone};
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("pid");
        let run = tokio::spawn(run_bounded(
            starts_a_sleep(dir.path(), &pidfile),
            Duration::from_secs(60),
        ));
        let pid = pid_from_file(&pidfile, Duration::from_secs(5)).await;
        let _kill = KillOnFail(pid);
        run.abort();
        // Awaiting the handle is what makes the runtime drop the future.
        let _ = run.await;
        assert!(
            wait_until_gone(pid, GONE_WITHIN).await,
            "the background sleep {pid} outlived the dropped run"
        );
    }

    /// A normal end signals nothing: once a group is empty the system may
    /// give its id to another, so an ended run's id is never used. A process
    /// the command detached on purpose keeps running.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_normal_end_leaves_a_process_the_command_detached() {
        use crate::test_util::{KillOnFail, pid_from_file, process_is_gone};
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("pid");
        let mut cmd = shell_command(&format!(
            "sleep 30 > /dev/null 2>&1 & echo $! > '{}'",
            pidfile.display()
        ));
        cmd.current_dir(dir.path());
        let output = run_bounded(cmd, Duration::from_secs(5))
            .await
            .expect("the shell ends at once");
        assert!(output.status.success());
        let pid = pid_from_file(&pidfile, Duration::from_secs(1)).await;
        // This test's own cleanup: the guard kills the sleep it left.
        let _kill = KillOnFail(pid);
        assert!(!process_is_gone(pid), "a normal end signalled the group");
    }
}
