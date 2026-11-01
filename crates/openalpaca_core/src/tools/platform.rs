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

/// The most of each output stream a tool keeps from a command: 512 KB
/// (R-T28). [`run_bounded`] keeps one byte more of each, which says the
/// command wrote more than this, and the tool cuts to the cap.
pub const OUTPUT_CAP: usize = 512 * 1024;

/// One output stream as a tool shows it (residual X7): its first
/// [`OUTPUT_CAP`] bytes as text (an invalid sequence, or a character the cap
/// splits, read as U+FFFD), and — when [`run_bounded`] kept the byte past the
/// cap, which says the command wrote more — one more line naming the cut, so
/// the model can tell output that was cut from output that was exactly the
/// cap. `stream` names it: `stdout` or `stderr`.
pub fn shown_output(kept: &[u8], stream: &str) -> String {
    let mut text = String::from_utf8_lossy(&kept[..kept.len().min(OUTPUT_CAP)]).into_owned();
    if kept.len() > OUTPUT_CAP {
        text.push_str(&format!(
            "\n[{stream} cut at 512 KB; the rest was not kept]"
        ));
    }
    text
}

/// What a command stopped at its time limit is told — by the shell's own
/// bound and by the sandbox's cut alike (N-TOOL-4, Z1-13: the hint belongs to
/// whoever enforced the limit, not to a guess the loop makes from the words).
/// The message it follows names the limit; no tool takes a parameter that
/// raises it (N-TOOL-1, Z1-05).
pub const COMMAND_TIMED_OUT_HINT: &str = "Hint: that is the most a command may run here; split \
    the work into smaller commands that each finish sooner.";

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
/// Each stream is kept to its first [`OUTPUT_CAP`] bytes and one more, which
/// says the command wrote more; the rest is read and dropped while the
/// command runs, so it never blocks on a full pipe and its output never
/// fills the daemon's memory (R-T28). The caller cuts to the cap and says
/// so ([`shown_output`]).
///
/// unix: the child is started in a new process group (`process_group(0)`).
/// A guard owning the `Child` sends `killpg(pgid, SIGKILL)` on every exit
/// that is not a normal end, before it lets the `Child` drop, so the
/// processes the command started go too. A normal end disarms the guard. The
/// child is waited for only after both pipes closed, so the group id the guard
/// signals stays the child's own, alive or an unreaped zombie, until the run
/// ends and can never be one the system reused. A process that leaves the
/// group on its own (`setsid`) is out of reach; while it holds the output open
/// the run reads on until its timeout. Windows: `kill_on_drop(true)` kills the
/// direct child only, and processes it started survive ("where the platform
/// allows", R45).
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
        // Both pipes to their end first, together (so neither fills), and
        // only then the wait, as std's `Command::output` does. Until a normal
        // end the child is alive or an unreaped zombie, so it keeps its pid,
        // which is the group id `KillGuard` signals: the system cannot give
        // that id to another group. Waiting alongside the reads would reap
        // the child while a process that left the group (`setsid`) still held
        // a pipe open, and a kill at the timeout or the drop could then reach
        // whatever group was given the freed id.
        let (stdout, stderr) = tokio::try_join!(read_bounded(out_pipe), read_bounded(err_pipe))?;
        let status = run.child.wait().await?;
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

/// A pipe read until it closes, keeping its first [`OUTPUT_CAP`] bytes and
/// one more (R-T28): the rest is read and dropped, so the writer never blocks
/// on a full pipe and the daemon never holds more than that of one stream.
/// Nothing for a pipe not taken.
async fn read_bounded(pipe: Option<impl AsyncRead + Unpin>) -> std::io::Result<Vec<u8>> {
    const KEEP: usize = OUTPUT_CAP + 1;
    let Some(mut pipe) = pipe else {
        return Ok(Vec::new());
    };
    // Sized once to what is kept: a buffer grown by doubling from empty
    // would end at twice the cap when its last byte arrived (residual X7).
    let mut bytes = Vec::with_capacity(KEEP);
    (&mut pipe)
        .take(KEEP as u64)
        .read_to_end(&mut bytes)
        .await?;
    tokio::io::copy(&mut pipe, &mut tokio::io::sink()).await?;
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
            // run has not ended normally. The child is reaped only by
            // `run_bounded`'s own wait, which starts after both pipes closed,
            // and a wait that reaped it disarms the guard (nothing else in the
            // process reaps a child). So here it is alive or an unreaped
            // zombie: it still holds its pid, and with it the group id, which
            // the system therefore cannot have given to another group.
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
        let outcome =
            run_bounded(starts_a_sleep(dir.path(), &pidfile), Duration::from_secs(1)).await;
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

    /// A process that leaves the group (`setsid`) and keeps the output pipes
    /// open holds the run past the shell's end. Until the run ends the shell
    /// is not reaped: it keeps its pid, which is the group id the guard
    /// signals, so the system cannot give that id to another group and a kill
    /// at the timeout or the drop never reaches a stranger's group (S1 review,
    /// I1). The detached process itself is out of reach: this test kills it.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_process_that_left_the_group_cannot_free_the_group_id_before_the_run_ends() {
        use crate::test_util::{KillOnFail, pid_from_file};
        let dir = tempfile::tempdir().unwrap();
        let leader_file = dir.path().join("leader");
        let detached_file = dir.path().join("detached");
        let mut cmd = shell_command(&format!(
            "echo $$ > '{}'; perl -MPOSIX -e 'POSIX::setsid(); sleep 30' & echo $! > '{}'; exit 0",
            leader_file.display(),
            detached_file.display()
        ));
        cmd.current_dir(dir.path());
        let run = tokio::spawn(run_bounded(cmd, Duration::from_secs(60)));
        // The detached process is out of the run's reach, so it is guarded
        // the moment its pid is known, before anything else can fail.
        let detached = pid_from_file(&detached_file, Duration::from_secs(5)).await;
        let _kill = KillOnFail(detached);
        let leader = pid_from_file(&leader_file, Duration::from_secs(5)).await;
        // The shell ends at once; the detached process keeps both pipes open.
        for _ in 0..50 {
            tokio::time::sleep(Duration::from_millis(20)).await;
            // SAFETY: signal 0 sends nothing; it only asks whether `leader`
            // still holds its pid (a process or an unreaped zombie).
            let held = unsafe { libc::kill(leader, 0) } == 0;
            assert!(
                held,
                "the shell {leader} was reaped while the run still read its output, \
                 so its group id was free for the system to reuse"
            );
        }
        assert!(
            !run.is_finished(),
            "the detached process {detached} holds the output open"
        );
        run.abort();
        let _ = run.await;
    }

    /// R-T28: each pipe keeps at most the cap and one byte — the byte that
    /// says the command wrote more — and drains the rest, so the writer never
    /// blocks and the daemon never holds more than that, however much the
    /// command writes. Both pipes are still read to their end before the
    /// shell is reaped (I1).
    #[cfg(unix)]
    #[tokio::test]
    async fn a_command_writing_far_more_than_the_cap_ends_with_each_stream_kept_to_the_cap() {
        use crate::test_util::{GuardedProject, pid_from_file, process_is_gone};
        let gp = GuardedProject::new();
        let pidfile = gp.root.join("shell.pid");
        let mut cmd = shell_command(&format!(
            "echo $$ > '{}'; head -c 50000000 /dev/zero; head -c 50000000 /dev/zero >&2",
            pidfile.display()
        ));
        cmd.current_dir(&gp.root);
        let outcome = run_bounded(cmd, Duration::from_secs(60)).await;
        let shell = pid_from_file(&pidfile, Duration::from_secs(1)).await;
        let output = outcome.expect("the command runs to its end");
        assert!(output.status.success(), "{:?}", output.status);
        assert_eq!(output.stdout.len(), OUTPUT_CAP + 1, "the stdout kept");
        assert_eq!(output.stderr.len(), OUTPUT_CAP + 1, "the stderr kept");
        assert!(
            output
                .stdout
                .iter()
                .chain(&output.stderr)
                .all(|byte| *byte == 0),
            "what is kept is the start of what was written"
        );
        assert!(process_is_gone(shell), "the shell {shell} was reaped");
    }

    /// A normal end signals nothing: once a group is empty the system may
    /// give its id to another, so an ended run's id is never used. A process
    /// the command detached on purpose keeps running.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_normal_end_leaves_a_process_the_command_detached() {
        use crate::test_util::{GONE_WITHIN, KillOnFail, pid_from_file, wait_until_gone};
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("pid");
        let mut cmd = shell_command(&format!(
            "sleep 30 > /dev/null 2>&1 & echo $! > '{}'",
            pidfile.display()
        ));
        cmd.current_dir(dir.path());
        let outcome = run_bounded(cmd, Duration::from_secs(5)).await;
        // This test's own cleanup, before anything is asserted: a normal end
        // leaves the sleep running, so the guard kills it.
        let pid = pid_from_file(&pidfile, Duration::from_secs(1)).await;
        let _kill = KillOnFail(pid);
        let output = outcome.expect("the shell ends at once");
        assert!(output.status.success());
        // A killed process takes a moment to read as gone (the system reaps
        // an orphan asynchronously), so the sleep is watched for as long as
        // the kill tests give a killed process to go.
        assert!(
            !wait_until_gone(pid, GONE_WITHIN).await,
            "a normal end signalled the group"
        );
    }
}
