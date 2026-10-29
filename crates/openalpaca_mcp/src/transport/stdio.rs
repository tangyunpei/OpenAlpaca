// crates/openalpaca_mcp/src/transport/stdio.rs

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitStatus;
use std::sync::Arc;

use async_trait::async_trait;
use process_wrap::tokio::CommandWrap;
use rmcp::transport::TokioChildProcess;
use tokio::process::Command;

use crate::error::McpError;

use super::{Transport, TransportConnection, TransportInner};

/// Told what a spawned server's wait returned, once per process — however the
/// wait was reached: rmcp's close when the server's output ends, its kill
/// after the 3 s close wait, its drop guard's kill (N-MCP-1b). Called on the
/// task that reaped the process, so it must not block.
pub type ExitObserver = Arc<dyn Fn(ExitStatus) + Send + Sync>;

/// Spawns an MCP server as a subprocess; communicates via stdin/stdout.
///
/// The server runs as the leader of its own process group (in a job object on
/// Windows), and the group ends with the connection (`server_command` below),
/// so a disabled server leaves nothing it started behind (ADR-030).
pub struct StdioTransport {
    server_name: String,
    command: String,
    args: Vec<String>,
    env: HashMap<String, String>,
    cwd: Option<PathBuf>,
    exit_observer: Option<ExitObserver>,
}

impl StdioTransport {
    pub fn new(server_name: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            server_name: server_name.into(),
            command: command.into(),
            args: Vec::new(),
            env: HashMap::new(),
            cwd: None,
            exit_observer: None,
        }
    }

    pub fn with_args(mut self, args: Vec<String>) -> Self {
        self.args = args;
        self
    }

    pub fn with_env(mut self, key: impl Into<String>, val: impl Into<String>) -> Self {
        self.env.insert(key.into(), val.into());
        self
    }

    pub fn with_cwd(mut self, cwd: PathBuf) -> Self {
        self.cwd = Some(cwd);
        self
    }

    /// Report each spawned process's exit status to `observer`.
    pub fn with_exit_observer(mut self, observer: ExitObserver) -> Self {
        self.exit_observer = Some(observer);
        self
    }

    /// Expose fields for testing. Not public API.
    #[cfg(test)]
    pub(crate) fn args(&self) -> &[String] {
        &self.args
    }

    #[cfg(test)]
    pub(crate) fn env(&self) -> &HashMap<String, String> {
        &self.env
    }
}

#[async_trait]
impl Transport for StdioTransport {
    async fn connect(&self) -> Result<TransportConnection, McpError> {
        let mut cmd = Command::new(&self.command);
        cmd.args(&self.args);
        for (k, v) in &self.env {
            cmd.env(k, v);
        }
        if let Some(cwd) = &self.cwd {
            cmd.current_dir(cwd);
        }

        tracing::info!(
            server_name = %self.server_name,
            command = %self.command,
            "connecting MCP stdio transport"
        );

        let command = server_command(cmd, &self.server_name, self.exit_observer.clone());
        let child = TokioChildProcess::new(command).map_err(|e| {
            // TokioChildProcess::new returns an io::Error on spawn failure.
            McpError::Transport(std::io::Error::other(
                format!("failed to spawn MCP server '{}': {e}", self.command),
            ))
        })?;

        Ok(TransportConnection {
            inner: TransportInner::Stdio(child),
        })
    }

    fn kind(&self) -> &'static str {
        "stdio"
    }

    fn server_name(&self) -> &str {
        &self.server_name
    }
}

/// Wrap the server's command so that a teardown reaches everything it started.
///
/// On unix the server leads a new process group, so rmcp's teardown `kill()` —
/// on its close after a 3 s wait, and from its drop guard — is a `killpg`
/// (`process-wrap-10.0.1/src/tokio/process_group.rs:45`, `:174-175`,
/// `:113-114`), and [`group::EndGroupWithLeader`] ends the group when the
/// leader exits by itself, where rmcp sends no signal at all. A child leaves
/// the daemon's own process group, so a terminal's Ctrl+C reaches it only
/// through the daemon's shutdown.
///
/// On Windows the server runs in a job object; no test exercises that leg
/// (DU-33).
///
/// An `observer` goes on last, outermost, so what it is told is the status
/// the reap beneath it returned, the group already ended (N-MCP-1b).
fn server_command(cmd: Command, server_name: &str, observer: Option<ExitObserver>) -> CommandWrap {
    let mut wrap = CommandWrap::from(cmd);
    #[cfg(unix)]
    wrap.wrap(process_wrap::tokio::ProcessGroup::leader())
        .wrap(group::EndGroupWithLeader {
            server_name: server_name.to_string(),
        });
    #[cfg(windows)]
    {
        let _ = server_name;
        wrap.wrap(process_wrap::tokio::JobObject);
    }
    if let Some(observer) = observer {
        wrap.wrap(exit::ReportExit { observer });
    }
    wrap
}

/// The exit observer's wrapper (N-MCP-1b).
///
/// rmcp owns the child and exposes no exit status: it waits for the process
/// inside its own close and logs the status at most (`graceful_shutdown`,
/// rmcp-3.5.1 `src/transport/child_process.rs:81-107`). Every wait rmcp makes
/// goes through the wrapper stack, so the outermost wrapper is where the
/// status can be read. When a server exits by itself its output ends, rmcp's
/// service loop closes the transport (`src/service.rs:1805`), and that close's
/// wait returns at once for a process already gone — before the service task
/// ends and `Peer::is_transport_closed()` turns true, so the exit is reported
/// before any call can find the transport closed.
mod exit {
    use std::fmt;
    use std::future::Future;
    use std::io;
    use std::pin::Pin;
    use std::process::ExitStatus;
    use std::sync::Arc;

    use process_wrap::tokio::{ChildWrapper, CommandWrap, CommandWrapper};

    use super::ExitObserver;

    pub(super) struct ReportExit {
        pub(super) observer: ExitObserver,
    }

    impl fmt::Debug for ReportExit {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("ReportExit").finish_non_exhaustive()
        }
    }

    impl CommandWrapper for ReportExit {
        fn wrap_child(
            &mut self,
            inner: Box<dyn ChildWrapper>,
            _core: &CommandWrap,
        ) -> io::Result<Box<dyn ChildWrapper>> {
            Ok(Box::new(ExitReportingChild {
                inner,
                observer: Arc::clone(&self.observer),
                reported: false,
            }))
        }
    }

    struct ExitReportingChild {
        inner: Box<dyn ChildWrapper>,
        observer: ExitObserver,
        reported: bool,
    }

    impl fmt::Debug for ExitReportingChild {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("ExitReportingChild")
                .field("inner", &self.inner)
                .field("reported", &self.reported)
                .finish_non_exhaustive()
        }
    }

    impl ExitReportingChild {
        /// Tell the observer, once: a process is reaped once, but `wait` can
        /// be asked again for the status it already returned.
        fn report(&mut self, status: ExitStatus) {
            if !std::mem::replace(&mut self.reported, true) {
                (self.observer)(status);
            }
        }
    }

    impl ChildWrapper for ExitReportingChild {
        fn inner(&self) -> &dyn ChildWrapper {
            self.inner.as_ref()
        }

        fn inner_mut(&mut self) -> &mut dyn ChildWrapper {
            self.inner.as_mut()
        }

        fn into_inner(self: Box<Self>) -> Box<dyn ChildWrapper> {
            self.inner
        }

        fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
            let status = self.inner.try_wait()?;
            if let Some(status) = status {
                self.report(status);
            }
            Ok(status)
        }

        fn wait(&mut self) -> Pin<Box<dyn Future<Output = io::Result<ExitStatus>> + Send + '_>> {
            Box::pin(async move {
                let status = self.inner.wait().await?;
                self.report(status);
                Ok(status)
            })
        }
    }
}

/// The server's process group ends with its leader.
///
/// `ProcessGroup::leader()` alone does not make a disabled server's
/// descendants go. rmcp closes a stdio connection by closing the server's
/// stdin and waiting up to 3 s for it to exit, and kills it — then by `killpg`
/// — only if it is still running (`rmcp-3.5.1/src/transport/child_process.rs:81-107`).
/// A server that exits on the EOF, as a well-behaved one does, is reaped and
/// nothing else is signalled: what it started and left running — a helper in
/// the background, a grandchild already handed to init — goes on running,
/// reparented away from the daemon, where no `waitpid` can see it.
///
/// Stacked over `ProcessGroup::leader()`, this wrapper closes that path: once
/// the leader has been waited for — however the wait was reached: rmcp's
/// graceful close, its kill after the 3 s, its drop guard's kill — it sends
/// `SIGKILL` to the leader's group, once, through the `ProcessGroupChild`
/// beneath it, whose `start_kill` is `killpg(pgid, SIGKILL)` (`:174-175`).
///
/// The signal follows the reap, so the leader no longer holds its pgid. While
/// any member of the group lives the id cannot be reused, and when none does
/// `killpg` finds no group — unless, between the reap and the signal, the pid
/// allocator has come round to exactly that number and the new process has
/// made itself a group leader; macOS and Linux both hand out pids in sequence,
/// so that takes a full wrap of the pid space inside that window. Signalling
/// before the reap, while the exited leader still holds the id, would need its
/// exit observed without reaping it (`waitid` with `WNOWAIT`), which nix does
/// not offer on macOS (`nix-0.31.3/src/sys/wait.rs:354-359`).
#[cfg(unix)]
mod group {
    use std::future::Future;
    use std::io;
    use std::pin::Pin;
    use std::process::ExitStatus;

    use process_wrap::tokio::{ChildWrapper, CommandWrap, CommandWrapper, ProcessGroupChild};

    #[derive(Debug)]
    pub(super) struct EndGroupWithLeader {
        pub(super) server_name: String,
    }

    impl CommandWrapper for EndGroupWithLeader {
        fn wrap_child(
            &mut self,
            inner: Box<dyn ChildWrapper>,
            _core: &CommandWrap,
        ) -> io::Result<Box<dyn ChildWrapper>> {
            let beneath: &dyn std::any::Any = &*inner;
            debug_assert!(
                beneath.is::<ProcessGroupChild>(),
                "EndGroupWithLeader must be wrapped after ProcessGroup::leader(): \
                 only a group leader's start_kill reaches its group"
            );
            Ok(Box::new(GroupEndingChild {
                pgid: inner.id(),
                server_name: self.server_name.clone(),
                inner,
                ended: false,
            }))
        }
    }

    #[derive(Debug)]
    struct GroupEndingChild {
        inner: Box<dyn ChildWrapper>,
        /// The leader's pid, which is the group's id; kept for the log, since
        /// the child stops reporting it once it is reaped.
        pgid: Option<u32>,
        server_name: String,
        ended: bool,
    }

    impl GroupEndingChild {
        /// `SIGKILL` the rest of the group, once, after the leader has exited.
        fn end_group(&mut self) {
            if std::mem::replace(&mut self.ended, true) {
                return;
            }
            // `killpg` fails with `ESRCH` — nothing was left in the group,
            // the usual outcome — or `EPERM`.
            match self.inner.start_kill() {
                Ok(()) => tracing::debug!(
                    server_name = %self.server_name,
                    pgid = ?self.pgid,
                    "MCP server exited leaving processes in its group; killed them"
                ),
                Err(e) if e.kind() == io::ErrorKind::PermissionDenied => tracing::warn!(
                    server_name = %self.server_name,
                    pgid = ?self.pgid,
                    error = %e,
                    "could not kill the processes the MCP server left in its group"
                ),
                Err(_) => {}
            }
        }
    }

    impl ChildWrapper for GroupEndingChild {
        fn inner(&self) -> &dyn ChildWrapper {
            self.inner.as_ref()
        }

        fn inner_mut(&mut self) -> &mut dyn ChildWrapper {
            self.inner.as_mut()
        }

        fn into_inner(self: Box<Self>) -> Box<dyn ChildWrapper> {
            self.inner
        }

        fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
            let status = self.inner.try_wait()?;
            if status.is_some() {
                self.end_group();
            }
            Ok(status)
        }

        fn wait(&mut self) -> Pin<Box<dyn Future<Output = io::Result<ExitStatus>> + Send + '_>> {
            Box::pin(async move {
                let status = self.inner.wait().await?;
                self.end_group();
                Ok(status)
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_stores_args_and_env() {
        let t = StdioTransport::new("srv", "npx")
            .with_args(vec!["-y".into(), "@modelcontextprotocol/server-filesystem".into()])
            .with_env("FOO", "bar")
            .with_env("BAZ", "qux");
        assert_eq!(t.args(), &["-y", "@modelcontextprotocol/server-filesystem"]);
        assert_eq!(t.env().get("FOO").map(String::as_str), Some("bar"));
        assert_eq!(t.env().get("BAZ").map(String::as_str), Some("qux"));
        assert_eq!(t.kind(), "stdio");
        assert_eq!(t.server_name(), "srv");
    }

    #[tokio::test]
    async fn connect_fails_on_nonexistent_command() {
        let t = StdioTransport::new("doesnotexist", "/definitely/not/a/real/command/xyzzy123");
        // `TransportConnection` wraps an rmcp `TokioChildProcess` which doesn't
        // implement `Debug`, so we can't use `.unwrap_err()` — match manually.
        match t.connect().await {
            Ok(_) => panic!("expected connect() to fail for nonexistent command"),
            Err(err) => assert!(
                matches!(err, McpError::Transport(_)),
                "expected Transport error, got {err:?}"
            ),
        }
    }

    /// What a forking fixture server does once its two descendants are started.
    #[cfg(unix)]
    #[derive(Clone, Copy)]
    enum Tail {
        /// Read stdin until EOF, then exit — a well-behaved server.
        ExitOnEof,
        /// Never read stdin; wait on the background child — a server that
        /// outlives rmcp's 3 s close wait and is killed.
        IgnoreEof,
    }

    /// A stdio server that starts a background child (`sleep 3600 &`) and a
    /// grandchild handed to init at once (`( sleep 3600 & )`), records both
    /// pids, and neither of them polices itself — so only a teardown that
    /// reaches the server's process group can end them.
    #[cfg(unix)]
    struct ForkingServer {
        _dir: tempfile::TempDir,
        script: PathBuf,
        log: PathBuf,
    }

    #[cfg(unix)]
    impl ForkingServer {
        fn new(tail: Tail) -> Self {
            use std::os::unix::fs::PermissionsExt;
            let dir = tempfile::tempdir().expect("tempdir");
            let log = dir.path().join("descendants.log");
            let script = dir.path().join("forking-server.sh");
            let tail = match tail {
                Tail::ExitOnEof => "while IFS= read -r line; do :; done\n",
                Tail::IgnoreEof => "wait\n",
            };
            std::fs::write(
                &script,
                format!(
                    "#!/bin/sh\nsleep 3600 &\necho \"$!\" >> '{log}'\n( sleep 3600 & echo \"$!\" >> '{log}' )\n{tail}",
                    log = log.display()
                ),
            )
            .expect("write the fixture");
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
                .expect("chmod the fixture");
            Self {
                _dir: dir,
                script,
                log,
            }
        }

        async fn connect(&self) -> TokioChildProcess {
            let conn = StdioTransport::new("forking", self.script.to_string_lossy())
                .connect()
                .await
                .expect("spawn the fixture");
            match conn.inner {
                TransportInner::Stdio(child) => child,
                TransportInner::Http(_) => unreachable!("a stdio transport"),
            }
        }

        /// The two descendants' pids, once both are recorded.
        async fn descendants(&self) -> Vec<u32> {
            for _ in 0..250 {
                let pids: Vec<u32> = std::fs::read_to_string(&self.log)
                    .unwrap_or_default()
                    .lines()
                    .filter_map(|l| l.trim().parse().ok())
                    .collect();
                if pids.len() == 2 {
                    return pids;
                }
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
            }
            panic!("the fixture never recorded its two descendants");
        }
    }

    /// Kills every pid it holds when dropped, so a failed assertion leaves no
    /// `sleep 3600` behind.
    #[cfg(unix)]
    struct KillOnDrop(Vec<u32>);

    #[cfg(unix)]
    impl Drop for KillOnDrop {
        fn drop(&mut self) {
            for pid in &self.0 {
                let _ = std::process::Command::new("kill")
                    .args(["-9", &pid.to_string()])
                    .stderr(std::process::Stdio::null())
                    .status();
            }
        }
    }

    #[cfg(unix)]
    fn alive(pid: u32) -> bool {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }

    #[cfg(unix)]
    fn pgid_of(pid: u32) -> Option<u32> {
        let out = std::process::Command::new("ps")
            .args(["-o", "pgid=", "-p", &pid.to_string()])
            .output()
            .ok()?;
        String::from_utf8_lossy(&out.stdout).trim().parse().ok()
    }

    #[cfg(unix)]
    async fn all_gone(pids: &[u32]) -> bool {
        for _ in 0..250 {
            if pids.iter().all(|p| !alive(*p)) {
                return true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        false
    }

    #[cfg(unix)]
    async fn close(child: &mut TokioChildProcess) -> std::time::Duration {
        let started = std::time::Instant::now();
        rmcp::transport::Transport::<rmcp::RoleClient>::close(child)
            .await
            .expect("close the connection");
        started.elapsed()
    }

    /// The graceful close: the server exits on the EOF, rmcp reaps it and
    /// signals nothing — and what it left running in its group goes too.
    /// Without [`group::EndGroupWithLeader`] both descendants outlive the close.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_server_that_exits_on_eof_takes_what_it_started_with_it() {
        let server = ForkingServer::new(Tail::ExitOnEof);
        let mut child = server.connect().await;
        let leader = child.id().expect("the leader's pid");
        let descendants = server.descendants().await;
        let _reap = KillOnDrop(descendants.clone());

        assert_eq!(
            pgid_of(leader),
            Some(leader),
            "the server leads its own group"
        );
        for pid in &descendants {
            assert_eq!(
                pgid_of(*pid),
                Some(leader),
                "descendant {pid} is in the server's group"
            );
            assert!(alive(*pid), "descendant {pid} runs before the close");
        }

        let took = close(&mut child).await;
        assert!(
            took < std::time::Duration::from_secs(3),
            "the server exited on the EOF, inside rmcp's 3 s wait (took {took:?}): \
             this is the graceful path, where rmcp sends no signal"
        );
        let mut all = descendants.clone();
        all.push(leader);
        assert!(
            all_gone(&all).await,
            "no process of the server's group outlives the close"
        );
    }

    /// A server that ignores the EOF is killed after rmcp's 3 s wait, and the
    /// kill is a `killpg`: its descendants go with it.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_server_that_ignores_eof_is_killed_with_its_group() {
        let server = ForkingServer::new(Tail::IgnoreEof);
        let mut child = server.connect().await;
        let leader = child.id().expect("the leader's pid");
        let descendants = server.descendants().await;
        let _reap = KillOnDrop(descendants.clone());

        let took = close(&mut child).await;
        assert!(
            took >= std::time::Duration::from_secs(3),
            "the server outlived rmcp's 3 s wait (took {took:?}): this is the kill path"
        );
        let mut all = descendants.clone();
        all.push(leader);
        assert!(all_gone(&all).await, "the kill reaches the whole group");
    }

    /// The exit observer is told what the process's wait returned — its own
    /// exit code, or the signal that ended it — once, through the stack the
    /// group wrappers sit in (N-MCP-1b).
    #[cfg(unix)]
    #[tokio::test]
    async fn the_exit_observer_is_told_the_status_a_server_ended_with_once() {
        for (script, expected) in [
            ("exit 3", "exit status: 3"),
            ("kill -TERM $$", "signal: 15 (SIGTERM)"),
        ] {
            let seen = Arc::new(std::sync::Mutex::new(Vec::<ExitStatus>::new()));
            let observer: ExitObserver = {
                let seen = Arc::clone(&seen);
                Arc::new(move |status| seen.lock().expect("lock").push(status))
            };
            let conn = StdioTransport::new("exits", "/bin/sh")
                .with_args(vec!["-c".into(), script.into()])
                .with_exit_observer(observer)
                .connect()
                .await
                .expect("spawn");
            let mut child = match conn.inner {
                TransportInner::Stdio(child) => child,
                TransportInner::Http(_) => unreachable!("a stdio transport"),
            };
            close(&mut child).await;
            drop(child);
            let seen: Vec<String> = seen
                .lock()
                .expect("lock")
                .iter()
                .map(ToString::to_string)
                .collect();
            assert_eq!(seen, [expected], "`{script}`: one report, its status");
        }
    }

    /// A connection dropped without a close — rmcp's drop guard, whose kill is
    /// all that ends a server that ignores the EOF — kills the group too.
    #[cfg(unix)]
    #[tokio::test]
    async fn dropping_the_connection_kills_the_group() {
        let server = ForkingServer::new(Tail::IgnoreEof);
        let child = server.connect().await;
        let leader = child.id().expect("the leader's pid");
        let descendants = server.descendants().await;
        let _reap = KillOnDrop(descendants.clone());

        drop(child);
        let mut all = descendants.clone();
        all.push(leader);
        assert!(
            all_gone(&all).await,
            "the drop guard's kill reaches the whole group"
        );
    }
}
