//! The `PATH` the app's daemon runs on (Q21).
//!
//! An app opened from the Finder or the Dock gets the minimal environment
//! macOS gives apps, not the person's shell's, and the daemon the app spawns
//! inherits it — so a stdio MCP server named by a bare program (`npx`,
//! `uvx`, Homebrew's `node`) or a CLI backend (`claude`) was found by a
//! daemon started from a terminal and missing for the one the app started.
//!
//! So the shell asks the account's login shell, once, at start, on a thread
//! of its own, so the window's first paint never waits on it: the shell is
//! run as a non-interactive login command (`-l -c`) that prints a start
//! marker, the environment (`/usr/bin/env -0`) and an end marker. The child
//! leads its own process group, and at [`CAPTURE_DEADLINE`] the whole group
//! is killed. Of what lies between the markers one key is taken, `PATH` —
//! the allowlist is `PATH` alone, so nothing else the login shell exports (an
//! API key, a credential pointer) reaches the daemon this way. When the
//! capture fails — no shell, a non-zero exit, no markers, no `PATH`, too much
//! output, or the deadline — the daemon gets [`FALLBACK_DIRS`] and
//! `~/.cargo/bin` instead, never launchd's bare `PATH`. A failed capture stays
//! failed until the app is opened again: it is taken once.
//!
//! What the daemon was given is announced by its source and the capture's
//! duration, never by its value: in `gui.log`; through storage's
//! `store::PATH_SOURCE_ENV`, to one boot line in `daemon.log`; and on the
//! connection the webview reads (Settings → Connection).
//!
//! The design is ZCode's capture read as a design and written fresh
//! (ZCode:packages/services/src/runtime-tools/runtimeLoginShellEnvCapture.ts:6-11,
//! :54-101, :173-216), with the entry's differences: a non-interactive login
//! shell where ZCode asks zsh and bash for an interactive one, the account's
//! shell from the user database where ZCode reads `$SHELL`, one key where
//! ZCode keeps the whole environment, and the fallback list with
//! `/opt/homebrew/bin` first and `~/.cargo/bin` added.

use serde::Serialize;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How long the capture may take, from the app's start: the login shell is
/// killed with its process group at this point, and a daemon spawned before
/// it waits for the capture at most until then.
pub(crate) const CAPTURE_DEADLINE: Duration = Duration::from_secs(3);

/// The most output the capture reads; past it the capture fails.
const OUTPUT_CAP: usize = 1 << 20;

/// The line printed before the environment.
const START_MARKER: &str = "__OPENALPACA_LOGIN_ENV_START__";

/// The line printed after it.
const END_MARKER: &str = "__OPENALPACA_LOGIN_ENV_END__";

/// What the login shell runs: the start marker, the environment as
/// NUL-terminated `KEY=value` entries (`env` by its absolute path, so a
/// profile that empties `PATH` still runs it), a line break, the end marker.
/// `echo` is a builtin of every shell `-l -c` reaches.
pub(crate) const CAPTURE_COMMAND: &str =
    "echo __OPENALPACA_LOGIN_ENV_START__; /usr/bin/env -0; echo; echo __OPENALPACA_LOGIN_ENV_END__";

/// The fixed list the daemon gets when the capture fails, `~/.cargo/bin`
/// after it.
pub(crate) const FALLBACK_DIRS: [&str; 6] = [
    "/opt/homebrew/bin",
    "/usr/local/bin",
    "/usr/bin",
    "/bin",
    "/usr/sbin",
    "/sbin",
];

/// Where the daemon's `PATH` came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum PathSource {
    /// The account's login shell's, captured at the app's start.
    LoginShell,
    /// [`FALLBACK_DIRS`] and `~/.cargo/bin`: the capture failed.
    FallbackList,
}

impl PathSource {
    /// The word `gui.log`, the daemon's boot line and the webview read.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            PathSource::LoginShell => "login-shell",
            PathSource::FallbackList => "fallback-list",
        }
    }
}

/// Why the capture gave no `PATH`, as a sentence that names no value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CaptureFailure {
    /// The user database names no absolute login shell for the account.
    NoShell,
    /// The login shell could not be started.
    Start(String),
    /// It had not exited by the deadline, and was killed with its group.
    TimedOut(Duration),
    /// It exited with a failure.
    Exited(String),
    /// It printed more than [`OUTPUT_CAP`].
    TooMuchOutput,
    /// Its output held no environment between the two markers.
    NoMarkers,
    /// The environment it printed has no `PATH`, or an empty one.
    NoPath,
}

impl std::fmt::Display for CaptureFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CaptureFailure::NoShell => {
                write!(f, "the account has no login shell the app can run")
            }
            CaptureFailure::Start(e) => write!(f, "the login shell could not be started: {e}"),
            CaptureFailure::TimedOut(deadline) => write!(
                f,
                "the login shell did not finish within {} ms",
                deadline.as_millis()
            ),
            CaptureFailure::Exited(status) => write!(f, "the login shell failed ({status})"),
            CaptureFailure::TooMuchOutput => {
                write!(f, "the login shell printed more than {} bytes", OUTPUT_CAP)
            }
            CaptureFailure::NoMarkers => {
                write!(f, "the login shell did not print its environment")
            }
            CaptureFailure::NoPath => write!(f, "the login shell's environment has no PATH"),
        }
    }
}

/// The `PATH` the daemon is given, where it came from and how long the
/// capture took. Its `Debug` leaves the value out: the value is set on the
/// daemon's command and written nowhere else.
#[derive(Clone)]
pub(crate) struct DaemonPath {
    pub(crate) source: PathSource,
    value: OsString,
    pub(crate) capture: Duration,
    pub(crate) failure: Option<CaptureFailure>,
}

impl std::fmt::Debug for DaemonPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DaemonPath")
            .field("source", &self.source)
            .field("capture", &self.capture)
            .field("failure", &self.failure)
            .finish_non_exhaustive()
    }
}

impl DaemonPath {
    /// The `PATH` value, for the daemon's command alone.
    pub(crate) fn value(&self) -> &OsString {
        &self.value
    }

    /// The capture's duration in whole milliseconds.
    pub(crate) fn capture_ms(&self) -> u64 {
        u64::try_from(self.capture.as_millis()).unwrap_or(u64::MAX)
    }

    /// Storage's `store::PATH_SOURCE_ENV` value: the source and the
    /// capture's milliseconds, `login-shell 212` — never the `PATH` itself.
    pub(crate) fn announcement(&self) -> String {
        format!("{} {}", self.source.as_str(), self.capture_ms())
    }

    /// What the webview reads (Settings → Connection).
    pub(crate) fn report(&self) -> DaemonPathReport {
        DaemonPathReport {
            source: self.source,
            capture_ms: self.capture_ms(),
            reason: self.failure.as_ref().map(ToString::to_string),
        }
    }
}

/// The daemon's `PATH` as the webview reads it, on the connection of a
/// daemon this app spawned: `{ source, captureMs, reason }`, `reason` the
/// capture's failure as a sentence, `null` for the login shell's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DaemonPathReport {
    pub(crate) source: PathSource,
    pub(crate) capture_ms: u64,
    pub(crate) reason: Option<String>,
}

/// [`FALLBACK_DIRS`] joined, then `<home>/.cargo/bin` when the home is known.
pub(crate) fn fallback_path(home: Option<&Path>) -> OsString {
    let mut path = OsString::from(FALLBACK_DIRS.join(":"));
    if let Some(home) = home {
        path.push(":");
        path.push(home.join(".cargo/bin"));
    }
    path
}

/// The account the capture runs for: its login shell, and the home whose
/// `~/.cargo/bin` the fallback list ends with.
pub(crate) struct Account {
    pub(crate) shell: Option<PathBuf>,
    pub(crate) home: Option<PathBuf>,
}

/// The capture: started once, on a thread of its own; [`Capture::wait`]
/// answers what it concluded, waiting at most until its deadline.
pub(crate) struct Capture {
    started: Instant,
    deadline: Duration,
    /// The account's home, once the capture's thread has resolved it.
    home: OnceLock<Option<PathBuf>>,
    result: Mutex<Option<DaemonPath>>,
    settled: Condvar,
}

impl Capture {
    /// Start the capture on a thread of its own and return at once: the
    /// thread resolves the account (`resolve`: the user database's lookup
    /// and the home's run there, not on the caller's thread), then captures
    /// its login shell's `PATH`. The deadline counts from now. The thread
    /// logs through the subscriber that is the caller's default now (the
    /// process's, in the app).
    pub(crate) fn start(
        resolve: impl FnOnce() -> Account + Send + 'static,
        deadline: Duration,
    ) -> Arc<Capture> {
        let capture = Arc::new(Capture {
            started: Instant::now(),
            deadline,
            home: OnceLock::new(),
            result: Mutex::new(None),
            settled: Condvar::new(),
        });
        let dispatch = tracing::dispatcher::get_default(Clone::clone);
        let runner = Arc::clone(&capture);
        let spawned = std::thread::Builder::new()
            .name("login-path-capture".into())
            .spawn(move || {
                tracing::dispatcher::with_default(&dispatch, || {
                    let account = resolve();
                    let _ = runner.home.set(account.home);
                    let outcome = match account.shell {
                        Some(shell) => capture_login_path(&shell, runner.started, runner.deadline),
                        None => Err(CaptureFailure::NoShell),
                    };
                    runner.settle(outcome);
                });
            });
        if let Err(e) = spawned {
            capture.settle(Err(CaptureFailure::Start(format!(
                "no thread for the capture: {e}"
            ))));
        }
        capture
    }

    /// Record what the capture concluded, once, and log it: its source and
    /// duration, a failure's reason, never the value.
    fn settle(&self, outcome: Result<OsString, CaptureFailure>) {
        let mut result = self.result.lock().unwrap_or_else(|p| p.into_inner());
        if result.is_some() {
            return;
        }
        let path = self.daemon_path(outcome);
        log_capture(&path);
        *result = Some(path);
        self.settled.notify_all();
    }

    fn daemon_path(&self, outcome: Result<OsString, CaptureFailure>) -> DaemonPath {
        let capture = self.started.elapsed();
        match outcome {
            Ok(value) => DaemonPath {
                source: PathSource::LoginShell,
                value,
                capture,
                failure: None,
            },
            Err(failure) => DaemonPath {
                source: PathSource::FallbackList,
                value: fallback_path(self.home.get().and_then(Option::as_deref)),
                capture,
                failure: Some(failure),
            },
        }
    }

    /// What the capture concluded, waiting for it at most until its
    /// deadline; a capture still unsettled then is taken as timed out and
    /// gives the fallback list (without `~/.cargo/bin` if the thread has not
    /// resolved the home by then), which every later wait answers too, the
    /// thread's own conclusion after it ignored.
    pub(crate) fn wait(&self) -> DaemonPath {
        let limit = self.started + self.deadline;
        let mut result = self.result.lock().unwrap_or_else(|p| p.into_inner());
        while result.is_none() {
            let now = Instant::now();
            if now >= limit {
                break;
            }
            result = self
                .settled
                .wait_timeout(result, limit - now)
                .map(|(guard, _)| guard)
                .unwrap_or_else(|p| p.into_inner().0);
        }
        if let Some(path) = result.as_ref() {
            return path.clone();
        }
        let path = self.daemon_path(Err(CaptureFailure::TimedOut(self.deadline)));
        log_capture(&path);
        *result = Some(path.clone());
        path
    }
}

/// The capture's line in `gui.log`.
fn log_capture(path: &DaemonPath) {
    match &path.failure {
        None => tracing::info!(
            source = path.source.as_str(),
            capture_ms = path.capture_ms(),
            "The daemon's PATH is the login shell's."
        ),
        Some(failure) => tracing::warn!(
            source = path.source.as_str(),
            capture_ms = path.capture_ms(),
            reason = %failure,
            "The daemon's PATH is the fallback list."
        ),
    }
}

/// The capture [`start`] began, for every spawn of this app's run.
static CAPTURE: OnceLock<Arc<Capture>> = OnceLock::new();

/// Begin the app's one capture (`run`): the account's login shell, the home
/// `directories` resolves, both looked up on the capture's thread,
/// [`CAPTURE_DEADLINE`]. Unix only; elsewhere the daemon inherits the app's
/// `PATH`, as it did.
pub(crate) fn start() {
    #[cfg(unix)]
    {
        let _ = CAPTURE.get_or_init(|| {
            Capture::start(
                || Account {
                    shell: account_login_shell(),
                    home: directories::BaseDirs::new().map(|dirs| dirs.home_dir().to_path_buf()),
                },
                CAPTURE_DEADLINE,
            )
        });
    }
}

/// The `PATH` for a daemon about to be spawned: the capture's, waiting for
/// it at most until its deadline — started now if `run` did not (a test
/// never spawns). `None` off Unix: the daemon inherits the app's.
pub(crate) fn for_the_daemon() -> Option<DaemonPath> {
    start();
    CAPTURE.get().map(|capture| capture.wait())
}

/// The account's login shell, as the user database names it (`getpwuid_r`),
/// when it is an absolute path: what a Finder launch, whose environment may
/// carry no `SHELL`, would otherwise not know.
#[cfg(unix)]
fn account_login_shell() -> Option<PathBuf> {
    use std::ffi::{CStr, OsStr};
    use std::os::unix::ffi::OsStrExt;

    let mut entry = std::mem::MaybeUninit::<libc::passwd>::uninit();
    let mut found: *mut libc::passwd = std::ptr::null_mut();
    let mut buffer = vec![0 as libc::c_char; 16 * 1024];
    // SAFETY: every pointer is to memory this frame owns, `buffer.len()` is
    // its length, and `entry` is read only when `found` says it was filled.
    let rc = unsafe {
        libc::getpwuid_r(
            libc::getuid(),
            entry.as_mut_ptr(),
            buffer.as_mut_ptr(),
            buffer.len(),
            &mut found,
        )
    };
    if rc != 0 || found.is_null() {
        return None;
    }
    // SAFETY: `found` is non-null, so `entry` was filled; its `pw_shell`
    // points into `buffer`, alive for this frame.
    let shell = unsafe { entry.assume_init_ref().pw_shell };
    if shell.is_null() {
        return None;
    }
    // SAFETY: as above, a NUL-terminated string in `buffer`.
    let bytes = unsafe { CStr::from_ptr(shell) }.to_bytes();
    let path = PathBuf::from(OsStr::from_bytes(bytes));
    path.is_absolute().then_some(path)
}

/// Run `shell -l -c` [`CAPTURE_COMMAND`] in a process group of its own and
/// take `PATH` from what it prints between the markers. Settled on the
/// shell's exit, not on the end of its output (a profile's background child
/// may hold the pipe open); at `started + deadline` the group is killed and
/// the shell reaped. After a successful exit the group is left alone: a
/// child the profile put in the background keeps running, and the output's
/// reader thread with it until the child closes the pipe (an owner's item,
/// `tasks/pending-decisions.md` §1, Rev 91).
#[cfg(unix)]
pub(crate) fn capture_login_path(
    shell: &Path,
    started: Instant,
    deadline: Duration,
) -> Result<OsString, CaptureFailure> {
    use std::io::Read;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    let mut child = Command::new(shell)
        .args(["-l", "-c", CAPTURE_COMMAND])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env("TERM", "dumb")
        .process_group(0)
        .spawn()
        .map_err(|e| CaptureFailure::Start(e.to_string()))?;
    let group = child.id();
    let span = deadline;
    let deadline = started + deadline;

    // The output, as it arrives, read on a thread of its own: the shell is
    // waited on here, so a pipe held open by a descendant never holds the
    // capture. `true` once the pipe has ended.
    let output = Arc::new(Mutex::new((Vec::new(), false)));
    if let Some(stdout) = child.stdout.take() {
        let sink = Arc::clone(&output);
        let _ = std::thread::Builder::new()
            .name("login-path-output".into())
            .spawn(move || {
                let mut stdout = stdout.take(OUTPUT_CAP as u64 + 1);
                let mut chunk = [0u8; 8192];
                loop {
                    match stdout.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => sink
                            .lock()
                            .unwrap_or_else(|p| p.into_inner())
                            .0
                            .extend_from_slice(&chunk[..n]),
                    }
                }
                sink.lock().unwrap_or_else(|p| p.into_inner()).1 = true;
            });
    }

    let kill_group = |child: &mut std::process::Child| {
        // SAFETY: a plain syscall. The group's leader is the shell, not yet
        // reaped, so its id names this capture's group and no other.
        unsafe {
            libc::killpg(group as libc::pid_t, libc::SIGKILL);
        }
        let _ = child.wait();
    };

    let poll = Duration::from_millis(10);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {}
            Err(e) => {
                kill_group(&mut child);
                return Err(CaptureFailure::Exited(e.to_string()));
            }
        }
        let now = Instant::now();
        if now >= deadline {
            kill_group(&mut child);
            return Err(CaptureFailure::TimedOut(span));
        }
        if output.lock().unwrap_or_else(|p| p.into_inner()).0.len() > OUTPUT_CAP {
            kill_group(&mut child);
            return Err(CaptureFailure::TooMuchOutput);
        }
        std::thread::sleep(poll.min(deadline - now));
    };
    if !status.success() {
        return Err(CaptureFailure::Exited(status.to_string()));
    }

    // The shell has exited, so what it printed is in the pipe: wait for the
    // reader to reach the end, or the end marker, or the deadline.
    loop {
        {
            let (bytes, ended) = &*output.lock().unwrap_or_else(|p| p.into_inner());
            if bytes.len() > OUTPUT_CAP {
                return Err(CaptureFailure::TooMuchOutput);
            }
            if *ended || Instant::now() >= deadline || contains(bytes, END_MARKER.as_bytes()) {
                return path_between_markers(bytes);
            }
        }
        std::thread::sleep(poll);
    }
}

#[cfg(not(unix))]
pub(crate) fn capture_login_path(
    _shell: &Path,
    _started: Instant,
    _deadline: Duration,
) -> Result<OsString, CaptureFailure> {
    Err(CaptureFailure::NoShell)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// `PATH` from the environment printed between the last end marker and the
/// last start marker before it: NUL-terminated `KEY=value` entries. Every
/// other key is dropped (the allowlist is `PATH` alone).
pub(crate) fn path_between_markers(output: &[u8]) -> Result<OsString, CaptureFailure> {
    let start = format!("{START_MARKER}\n");
    let end = format!("\n{END_MARKER}");
    let end_at = rfind(output, end.as_bytes()).ok_or(CaptureFailure::NoMarkers)?;
    let start_at = rfind(&output[..end_at], start.as_bytes()).ok_or(CaptureFailure::NoMarkers)?;
    let environment = &output[start_at + start.len()..end_at];
    let mut path = None;
    for entry in environment.split(|&b| b == 0) {
        if let Some(value) = entry.strip_prefix(b"PATH=") {
            path = Some(value);
        }
    }
    match path {
        Some(value) if !value.is_empty() => Ok(os_string(value)),
        _ => Err(CaptureFailure::NoPath),
    }
}

fn rfind(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).rposition(|w| w == needle)
}

#[cfg(unix)]
fn os_string(bytes: &[u8]) -> OsString {
    use std::os::unix::ffi::OsStrExt;
    std::ffi::OsStr::from_bytes(bytes).to_owned()
}

#[cfg(not(unix))]
fn os_string(bytes: &[u8]) -> OsString {
    OsString::from(String::from_utf8_lossy(bytes).into_owned())
}
