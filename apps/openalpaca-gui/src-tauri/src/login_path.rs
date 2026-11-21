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
//! run as a login command that prints a start marker, the environment
//! (`/usr/bin/env -0`) and an end marker — zsh and bash as an interactive
//! one (`-i -l -c`), so zsh reads `~/.zshrc` and bash a `~/.bashrc` its
//! profile sources, every other shell as `-l -c` ([`login_shell_args`];
//! LOGIN-CAPTURE). The child leads a session and process group of its own,
//! with no terminal and without a Terminal tab's `TERM_SESSION_ID` (so
//! Apple's session scripts leave that tab's files alone), and the whole
//! group is killed once the shell has exited with status 0 — a child a
//! profile put in the background goes with it — or at
//! [`CAPTURE_DEADLINE`]. An interactive profile that waits (on input, on a
//! child) takes the capture to the deadline and the daemon to the fallback
//! list below. An interactive profile that ends the shell, or replaces it
//! with a program that exits (`exit`, `exec /bin/sh`), before it prints the
//! start marker has the shell
//! run once more as `-l -c`, in the time left of the same deadline — its
//! login files without the interactive ones — and that `PATH` is announced
//! as `login-shell-noninteractive` ([`capture_login_shell`]; CAPTURE-RETRY,
//! *Rev 99* (2), the advisor's pick). Of what lies
//! between the markers one key is taken, `PATH` —
//! the allowlist is `PATH` alone, so nothing else the login shell exports (an
//! API key, a credential pointer) reaches the daemon this way. When the
//! capture fails — no shell, a non-zero exit, no markers, no `PATH`, too much
//! output, or the deadline — the daemon gets [`FALLBACK_DIRS`] and
//! `~/.cargo/bin` instead, never launchd's bare `PATH`. A failed capture stays
//! failed until the app is opened again: it is taken once.
//!
//! What the daemon was given is announced by its source and the capture's
//! duration, never by its value: in `gui.log`; through support's
//! `paths::PATH_SOURCE_ENV`, to one boot line in `daemon.log`; and on the
//! connection the webview reads (Settings → Connection).
//!
//! **The keys listed beside `PATH` (N-DAEMON-2).** A credential pointer the
//! login shell exports — `AWS_PROFILE`, `GOOGLE_APPLICATION_CREDENTIALS`,
//! `KUBECONFIG`, or any other variable — reaches the daemon from the login
//! shell only when the owner names it in `daemon.toml`,
//! `[daemon.env] passthrough_keys = [...]`:
//! variable names only, never values, empty by default. The shell reads that
//! list from the store's `config/daemon.toml` — the file the daemon it spawns
//! reads, through `OPENALPACA_CONFIG_DIR` — on the capture's thread, as the
//! capture starts ([`configured_passthrough_keys`]), so the allowlist taken
//! from between the markers is `PATH` and the listed names. The list reaches
//! only a daemon this launch spawns: a daemon still running is reused as it
//! is (`ensure_daemon_running`), so an edit reaches the daemon once it is
//! stopped and the app opened again, and the capture line's `passed_keys`
//! name what a spawned daemon would get, not the running one's. A
//! listed key the login shell exports is set on the daemon's command beside
//! `PATH`; the announcement names the keys passed, by name, never their
//! values. No `OPENALPACA_` name is ever taken from the list (LOGIN-CAPTURE,
//! *Rev 92* (1)): those are the app's and the daemon's own. A listed key is
//! in the daemon's whole environment, so every command, MCP server and
//! plugin it runs sees it. Claude Code's design (`settings.json`'s `env`, a
//! map the user sets), not ZCode's, which passes the three by name
//! (ZCode:packages/services/src/runtime-tools/runtimeCommandEnv.ts:48-106).
//!
//! **What the list does not govern.** The daemon's command is the app's own
//! environment with these variables set over it, nothing removed: an app
//! opened from the Finder or the Dock carries launchd's minimal environment,
//! so there "only through the list" holds; an app started from a terminal
//! (`bun run tauri dev`, its binary run by hand) hands its daemon whatever
//! that terminal exported, listed or not (*Rev 91* (4), *Rev 92* (2): no
//! change).
//!
//! The design is ZCode's capture read as a design and written fresh
//! (ZCode:packages/services/src/runtime-tools/runtimeLoginShellEnvCapture.ts:6-11,
//! :53-58, :54-101, :173-216), with the entry's differences: the account's
//! shell from the user database where ZCode reads `$SHELL`, one key where
//! ZCode keeps the whole environment, the fallback list with
//! `/opt/homebrew/bin` first and `~/.cargo/bin` added. The session of its
//! own matches ZCode's detached spawn (`detached`, which on a Unix makes the
//! child a session's leader).

use openalpaca_app_support::paths;
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

/// How long, after the group is killed, the capture waits for the end of
/// the shell's output once the end marker is in: only a process that left
/// the group (a session of its own) can still hold the pipe then.
const READER_GRACE: Duration = Duration::from_millis(250);

/// The prefix of the app's and the daemon's own variables: no name with it
/// is passed from the login shell (LOGIN-CAPTURE, *Rev 92* (1)).
const OWN_PREFIX: &str = "OPENALPACA_";

/// The capture's output readers still running, for the tests.
#[cfg(test)]
pub(crate) static OUTPUT_READERS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// How long, in milliseconds, the output's reader waits after the pipe's
/// end before it counts itself ended: 0 but in the test that pins the
/// capture's wait for its reader, which sets it below [`READER_GRACE`].
#[cfg(test)]
pub(crate) static READER_END_DELAY_MS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// The line printed before the environment.
const START_MARKER: &str = "__OPENALPACA_LOGIN_ENV_START__";

/// The line printed after it.
const END_MARKER: &str = "__OPENALPACA_LOGIN_ENV_END__";

/// What the login shell runs: the start marker, the environment as
/// NUL-terminated `KEY=value` entries (`env` by its absolute path, so a
/// profile that empties `PATH` still runs it), a line break, the end marker.
/// `echo` is a builtin of every shell the capture runs.
pub(crate) const CAPTURE_COMMAND: &str =
    "echo __OPENALPACA_LOGIN_ENV_START__; /usr/bin/env -0; echo; echo __OPENALPACA_LOGIN_ENV_END__";

/// The variables the shell sets on the daemon's command itself
/// ([`crate::daemon_command`]): a listed one is not passed from the login
/// shell — the app's own value stands — and `gui.log` says so.
pub(crate) const SET_BY_THE_APP: [&str; 4] = [
    "PATH",
    "OPENALPACA_CONFIG_DIR",
    openalpaca_app_support::paths::MANAGED_LOG_ENV,
    openalpaca_app_support::paths::PATH_SOURCE_ENV,
];

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
    /// The login shell's, read by its second run, as `-l -c`: run
    /// interactively, it ended before it printed the start marker
    /// (CAPTURE-RETRY, *Rev 99* (2)).
    LoginShellNoninteractive,
    /// [`FALLBACK_DIRS`] and `~/.cargo/bin`: the capture failed.
    FallbackList,
}

impl PathSource {
    /// The word `gui.log`, the daemon's boot line and the webview read.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            PathSource::LoginShell => "login-shell",
            PathSource::LoginShellNoninteractive => "login-shell-noninteractive",
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

/// What a capture that answered took from between the markers: `PATH`,
/// and each listed key the login shell exported, in the list's order, and
/// whether the second run, as `-l -c`, gave them. Its `Debug` leaves every
/// value out.
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct Captured {
    pub(crate) path: OsString,
    pub(crate) passed: Vec<(String, OsString)>,
    /// The interactive run ended before its markers and this is the second
    /// run's (CAPTURE-RETRY).
    pub(crate) second_run: bool,
}

impl std::fmt::Debug for Captured {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<&str> = self.passed.iter().map(|(k, _)| k.as_str()).collect();
        f.debug_struct("Captured")
            .field("passed_keys", &names)
            .field("second_run", &self.second_run)
            .finish_non_exhaustive()
    }
}

/// The `PATH` the daemon is given, where it came from and how long the
/// capture took, with the listed keys passed beside it (N-DAEMON-2). Its
/// `Debug` leaves every value out: the values are set on the daemon's
/// command and written nowhere else.
#[derive(Clone)]
pub(crate) struct DaemonPath {
    pub(crate) source: PathSource,
    value: OsString,
    pub(crate) capture: Duration,
    pub(crate) failure: Option<CaptureFailure>,
    /// The keys `[daemon.env] passthrough_keys` listed, as the capture read
    /// them (empty when the capture's thread had not read them by the wait's
    /// deadline).
    listed: Vec<String>,
    /// Those of them the login shell exported, with its values.
    passed: Vec<(String, OsString)>,
}

impl std::fmt::Debug for DaemonPath {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DaemonPath")
            .field("source", &self.source)
            .field("capture", &self.capture)
            .field("failure", &self.failure)
            .field("passed_keys", &self.passed_keys())
            .finish_non_exhaustive()
    }
}

impl DaemonPath {
    /// The `PATH` value, for the daemon's command alone.
    pub(crate) fn value(&self) -> &OsString {
        &self.value
    }

    /// The listed keys passed beside `PATH`, with their values, for the
    /// daemon's command alone.
    pub(crate) fn passed(&self) -> &[(String, OsString)] {
        &self.passed
    }

    /// The names of the keys passed, in the list's order.
    pub(crate) fn passed_keys(&self) -> Vec<&str> {
        self.passed.iter().map(|(key, _)| key.as_str()).collect()
    }

    /// The listed keys not passed: the login shell did not export them, or
    /// the capture failed.
    fn absent_keys(&self) -> Vec<&str> {
        self.listed
            .iter()
            .map(String::as_str)
            .filter(|key| !self.passed.iter().any(|(passed, _)| passed == key))
            .collect()
    }

    /// The capture's duration in whole milliseconds.
    pub(crate) fn capture_ms(&self) -> u64 {
        u64::try_from(self.capture.as_millis()).unwrap_or(u64::MAX)
    }

    /// Support's `paths::PATH_SOURCE_ENV` value: the source and the
    /// capture's milliseconds, `login-shell 212`, then the names of the keys
    /// passed, comma-separated, when there are any —
    /// `login-shell 212 AWS_PROFILE,KUBECONFIG` — never a value.
    pub(crate) fn announcement(&self) -> String {
        let mut text = format!("{} {}", self.source.as_str(), self.capture_ms());
        if !self.passed.is_empty() {
            text.push(' ');
            text.push_str(&self.passed_keys().join(","));
        }
        text
    }

    /// What the webview reads (Settings → Connection).
    pub(crate) fn report(&self) -> DaemonPathReport {
        DaemonPathReport {
            source: self.source,
            capture_ms: self.capture_ms(),
            reason: self.failure.as_ref().map(ToString::to_string),
            passed_keys: self.passed_keys().into_iter().map(str::to_owned).collect(),
        }
    }
}

/// The daemon's `PATH` as the webview reads it, on the connection of a
/// daemon this app spawned: `{ source, captureMs, reason, passedKeys }`,
/// `reason` the capture's failure as a sentence, `null` for the login
/// shell's; `passedKeys` the names of the listed keys passed beside it
/// (N-DAEMON-2), never their values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DaemonPathReport {
    pub(crate) source: PathSource,
    pub(crate) capture_ms: u64,
    pub(crate) reason: Option<String>,
    pub(crate) passed_keys: Vec<String>,
}

/// [`FALLBACK_DIRS`] joined, then `<home>/.cargo/bin` when the home is known.
/// A home that is not usable (`paths::usable_home`: empty or relative) is
/// not known: its `.cargo/bin` would be an entry relative to the daemon's
/// directory.
pub(crate) fn fallback_path(home: Option<&Path>) -> OsString {
    let mut path = OsString::from(FALLBACK_DIRS.join(":"));
    if let Some(home) = paths::usable_home(home.map(Path::to_path_buf)) {
        path.push(":");
        path.push(home.join(".cargo/bin"));
    }
    path
}

/// The account the capture runs for: its login shell, the home whose
/// `~/.cargo/bin` the fallback list ends with, and the keys
/// `[daemon.env] passthrough_keys` lists (N-DAEMON-2).
pub(crate) struct Account {
    pub(crate) shell: Option<PathBuf>,
    pub(crate) home: Option<PathBuf>,
    pub(crate) keys: Vec<String>,
}

/// Whether `name` is a variable name: a letter or `_`, then letters, digits
/// and `_` — so never `KEY=value`.
fn is_variable_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    matches!(bytes.next(), Some(b) if b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// The keys `<config_dir>/daemon.toml` lists under `[daemon.env]
/// passthrough_keys`, in its order, each once. No file, no `[daemon.env]`
/// table or no list is an empty list. A file that cannot be read or is not
/// TOML, a value that is not a list, an entry that is not a string or not a
/// variable name, a variable the app sets itself ([`SET_BY_THE_APP`]) and
/// any other `OPENALPACA_` name (LOGIN-CAPTURE, *Rev 92* (1): a listed
/// `OPENALPACA_HOME_STORE` would point the daemon at a store the app does not
/// read `discovery.json` from) are each a `gui.log` warning and pass nothing;
/// a warning names a dropped entry by its place in the list, never by what it
/// holds (`KEY=value` would be a value), but a variable name by name.
pub(crate) fn passthrough_keys(config_dir: &Path) -> Vec<String> {
    let file = config_dir.join("daemon.toml");
    let text = match std::fs::read_to_string(&file) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(e) => {
            tracing::warn!(
                file = %file.display(),
                "Could not read [daemon.env] passthrough_keys ({}); no key is passed beside PATH.",
                e.kind()
            );
            return Vec::new();
        }
    };
    // Parsed as a bare table and walked by hand, so no parser message — which
    // may quote the file — reaches the log.
    let Ok(table) = text.parse::<toml::Table>() else {
        tracing::warn!(
            file = %file.display(),
            "daemon.toml is not valid TOML; no key is passed beside PATH."
        );
        return Vec::new();
    };
    let Some(daemon) = table.get("daemon") else {
        return Vec::new();
    };
    let Some(env) = daemon.as_table().map(|daemon| daemon.get("env")) else {
        tracing::warn!("daemon.toml's [daemon] is not a table; no key is passed beside PATH.");
        return Vec::new();
    };
    let Some(env) = env else {
        return Vec::new();
    };
    let Some(list) = env.as_table().map(|env| env.get("passthrough_keys")) else {
        tracing::warn!("daemon.toml's [daemon.env] is not a table; no key is passed beside PATH.");
        return Vec::new();
    };
    let Some(list) = list else {
        return Vec::new();
    };
    let Some(entries) = list.as_array() else {
        tracing::warn!(
            "[daemon.env] passthrough_keys is not a list of variable names; no key is passed \
             beside PATH."
        );
        return Vec::new();
    };
    let mut keys: Vec<String> = Vec::new();
    for (index, entry) in entries.iter().enumerate() {
        let place = index + 1;
        match entry.as_str() {
            Some(name) if SET_BY_THE_APP.contains(&name) => tracing::warn!(
                key = name,
                "[daemon.env] passthrough_keys lists a variable the app sets on the daemon \
                 itself; the app's value stands, and it is not passed from the login shell."
            ),
            Some(name) if is_variable_name(name) && name.starts_with(OWN_PREFIX) => {
                tracing::warn!(
                    key = name,
                    "[daemon.env] passthrough_keys lists one of OpenAlpaca's own variables; no \
                     OPENALPACA_ name is passed from the login shell."
                )
            }
            Some(name) if is_variable_name(name) => {
                if !keys.iter().any(|key| key == name) {
                    keys.push(name.to_owned());
                }
            }
            _ => tracing::warn!(
                entry = place,
                "[daemon.env] passthrough_keys: entry {place} is not a variable name; it is not \
                 passed."
            ),
        }
    }
    keys
}

/// [`passthrough_keys`] of the store's `config/daemon.toml` — the file the
/// daemon the app spawns reads, its `OPENALPACA_CONFIG_DIR` being the
/// store's `config/` (`paths::ensure_runtime_config_dir`). Empty when the
/// store cannot be resolved.
pub(crate) fn configured_passthrough_keys() -> Vec<String> {
    match openalpaca_app_support::paths::runtime_config_dir() {
        Ok(config_dir) => passthrough_keys(&config_dir),
        Err(e) => {
            tracing::warn!(
                "Could not find daemon.toml for [daemon.env] passthrough_keys ({e:#}); no key is \
                 passed beside PATH."
            );
            Vec::new()
        }
    }
}

/// The capture: started once, on a thread of its own; [`Capture::wait`]
/// answers what it concluded, waiting at most until its deadline.
pub(crate) struct Capture {
    started: Instant,
    deadline: Duration,
    /// The account's home, once the capture's thread has resolved it.
    home: OnceLock<Option<PathBuf>>,
    /// The keys listed beside `PATH`, once the capture's thread has read
    /// them.
    keys: OnceLock<Vec<String>>,
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
            keys: OnceLock::new(),
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
                    let keys = runner.keys.get_or_init(|| account.keys);
                    let outcome = match account.shell {
                        Some(shell) => {
                            capture_login_shell(&shell, runner.started, runner.deadline, keys)
                        }
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
    /// duration, a failure's reason, the keys passed by name, never a value.
    fn settle(&self, outcome: Result<Captured, CaptureFailure>) {
        let mut result = self.result.lock().unwrap_or_else(|p| p.into_inner());
        if result.is_some() {
            return;
        }
        let path = self.daemon_path(outcome);
        log_capture(&path);
        *result = Some(path);
        self.settled.notify_all();
    }

    fn daemon_path(&self, outcome: Result<Captured, CaptureFailure>) -> DaemonPath {
        let capture = self.started.elapsed();
        let listed = self.keys.get().cloned().unwrap_or_default();
        match outcome {
            Ok(captured) => DaemonPath {
                source: if captured.second_run {
                    PathSource::LoginShellNoninteractive
                } else {
                    PathSource::LoginShell
                },
                value: captured.path,
                capture,
                failure: None,
                listed,
                passed: captured.passed,
            },
            Err(failure) => DaemonPath {
                source: PathSource::FallbackList,
                value: fallback_path(self.home.get().and_then(Option::as_deref)),
                capture,
                failure: Some(failure),
                listed,
                passed: Vec::new(),
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

/// The capture's line in `gui.log`, with the keys passed beside `PATH` and
/// the listed ones not passed, by name.
fn log_capture(path: &DaemonPath) {
    let passed = path.passed_keys().join(",");
    let absent = path.absent_keys().join(",");
    match &path.failure {
        None if path.source == PathSource::LoginShellNoninteractive => tracing::info!(
            source = path.source.as_str(),
            capture_ms = path.capture_ms(),
            passed_keys = passed.as_str(),
            absent_keys = absent.as_str(),
            "The daemon's PATH is the login shell's, read as a login shell without -i: what \
             only the interactive startup files add is not in it."
        ),
        None => tracing::info!(
            source = path.source.as_str(),
            capture_ms = path.capture_ms(),
            passed_keys = passed.as_str(),
            absent_keys = absent.as_str(),
            "The daemon's PATH is the login shell's."
        ),
        Some(failure) => tracing::warn!(
            source = path.source.as_str(),
            capture_ms = path.capture_ms(),
            passed_keys = passed.as_str(),
            absent_keys = absent.as_str(),
            reason = %failure,
            "The daemon's PATH is the fallback list."
        ),
    }
}

/// The capture [`start`] began, for every spawn of this app's run.
static CAPTURE: OnceLock<Arc<Capture>> = OnceLock::new();

/// Begin the app's one capture (`run`): the account's login shell, the home
/// `paths::user_home` reports and the keys `daemon.toml` lists beside
/// `PATH`, all looked up on the capture's thread, [`CAPTURE_DEADLINE`]. Unix
/// only; elsewhere the daemon inherits the app's `PATH` and environment, as
/// it did.
pub(crate) fn start() {
    #[cfg(unix)]
    {
        let _ = CAPTURE.get_or_init(|| Capture::start(this_account, CAPTURE_DEADLINE));
    }
}

/// The account the app's capture runs for, resolved on the capture's
/// thread: the login shell the user database names, the home
/// `paths::user_home` reports (`std::env::home_dir`'s, when absolute), and
/// the keys the store's `daemon.toml` lists beside `PATH`.
#[cfg(unix)]
pub(crate) fn this_account() -> Account {
    Account {
        shell: account_login_shell(),
        home: paths::user_home(),
        keys: configured_passthrough_keys(),
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

/// The arguments the login shell is run with, [`CAPTURE_COMMAND`] after
/// them: zsh and bash (by the file's name) as an interactive login shell,
/// `-i -l -c`, as ZCode does
/// (ZCode:packages/services/src/runtime-tools/runtimeLoginShellEnvCapture.ts:53-58),
/// so zsh reads `~/.zshrc` and bash a `~/.bashrc` its `~/.bash_profile`
/// sources (a login bash never reads `~/.bashrc` itself); every other shell
/// as `-l -c` (a csh or tcsh takes `-l` only alone, and fails either way).
#[cfg(unix)]
pub(crate) fn login_shell_args(shell: &Path) -> &'static [&'static str] {
    match shell.file_name().and_then(|name| name.to_str()) {
        Some("zsh" | "bash") => &["-i", "-l", "-c"],
        _ => LOGIN_ARGS,
    }
}

/// A login command, not interactive: every shell but zsh and bash from the
/// start, and zsh's or bash's second run (CAPTURE-RETRY).
#[cfg(unix)]
const LOGIN_ARGS: &[&str] = &["-l", "-c"];

/// The capture of `shell`'s `PATH` the app makes: one run of
/// [`capture_login_path`], and, when that run was interactive (`-i`) and the
/// shell ended — exited, with any status, or was replaced by a program that
/// exited — before it printed the start marker, one more run as `-l -c` in
/// the time left of the same `started + deadline` (CAPTURE-RETRY, *Rev 99*
/// (2), the advisor's pick of 2026-11-16). Each run's process group is
/// handled as one run's is ([`capture_login_path`]: killed after a
/// successful exit or at the deadline, left after a failed exit). A run that
/// printed the start marker, one still running at the deadline (a profile
/// that waits), one that printed too much, or a shell that could not start
/// is never run again; nor is a run with no time left (among them a status-0
/// exit whose output a child in a session of its own still holds: the reader
/// is waited for to the deadline). When the second run fails too, its
/// failure is the capture's. `gui.log` says why the second
/// run was made, naming the first run's end.
#[cfg(unix)]
pub(crate) fn capture_login_shell(
    shell: &Path,
    started: Instant,
    deadline: Duration,
    keys: &[String],
) -> Result<Captured, CaptureFailure> {
    let args = login_shell_args(shell);
    let first = run_login_shell(shell, args, started, deadline, keys);
    let ended_first = !first.printed_start
        && args.contains(&"-i")
        && matches!(
            first.outcome,
            Err(CaptureFailure::Exited(_) | CaptureFailure::NoMarkers)
        );
    if !ended_first || Instant::now() >= started + deadline {
        return first.outcome;
    }
    if let Err(failure) = &first.outcome {
        tracing::info!(
            reason = %failure,
            "The login shell ended before it printed its environment when run interactively; \
             it is run once more as a login shell without -i, in the time left."
        );
    }
    run_login_shell(shell, LOGIN_ARGS, started, deadline, keys)
        .outcome
        .map(|captured| Captured {
            second_run: true,
            ..captured
        })
}

#[cfg(not(unix))]
pub(crate) fn capture_login_shell(
    _shell: &Path,
    _started: Instant,
    _deadline: Duration,
    _keys: &[String],
) -> Result<Captured, CaptureFailure> {
    Err(CaptureFailure::NoShell)
}

/// The shell `pid`'s exit, seen without reaping it (`waitid` with
/// `WNOWAIT`): `None` while it runs, `Some(true)` once it has exited with
/// status 0, `Some(false)` for any other end. While it is unreaped its pid
/// names this capture's process group and no other, so the group can still
/// be killed by it.
#[cfg(unix)]
fn exit_seen(pid: libc::pid_t) -> std::io::Result<Option<bool>> {
    // SAFETY: `siginfo_t` is plain data; zeroed, it reads "nothing exited".
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    // SAFETY: a plain syscall on this capture's own child, writing only to
    // `info`, which this frame owns.
    let rc = unsafe {
        libc::waitid(
            libc::P_PID,
            pid as libc::id_t,
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if rc == -1 {
        let e = std::io::Error::last_os_error();
        return if e.kind() == std::io::ErrorKind::Interrupted {
            Ok(None)
        } else {
            Err(e)
        };
    }
    // SAFETY: `info` is initialized (zeroed, then filled by `waitid`); its
    // pid stays zero while the child has not exited (WNOHANG).
    if unsafe { info.si_pid() } != pid {
        return Ok(None);
    }
    // SAFETY: as above; `si_status` is the exit status under `CLD_EXITED`.
    Ok(Some(
        info.si_code == libc::CLD_EXITED && unsafe { info.si_status() } == 0,
    ))
}

/// One run of `shell` with [`login_shell_args`], never a second: the capture
/// as it was before CAPTURE-RETRY, kept for the tests that pin one run's
/// rules ([`run_login_shell`]); the app's capture is [`capture_login_shell`].
#[cfg(all(unix, test))]
pub(crate) fn capture_login_path(
    shell: &Path,
    started: Instant,
    deadline: Duration,
    keys: &[String],
) -> Result<Captured, CaptureFailure> {
    run_login_shell(shell, login_shell_args(shell), started, deadline, keys).outcome
}

/// One run of the login shell: what it concluded, and whether that
/// conclusion rules a second run out.
#[cfg(unix)]
struct Run {
    outcome: Result<Captured, CaptureFailure>,
    /// True when the run's output held the start marker — the line the
    /// capture's command prints first, so a run without it ended (or was
    /// replaced) before the command ran — and for a conclusion that is
    /// never a reason for a second run whatever the output held: a shell
    /// that did not start, the deadline, the output cap, a failed wait.
    printed_start: bool,
}

/// Run `shell` with `args` and [`CAPTURE_COMMAND`] in a
/// session and process group of its own and take `PATH`, and each of `keys`
/// it exports, from what it prints between the markers. Settled on the
/// shell's exit, not on the end of its output (a profile's background child
/// may hold the pipe open). Once the shell has exited with status 0, or at
/// `started + deadline`, the whole group is killed — a child the profile put
/// in the background with it (LOGIN-CAPTURE, *Rev 91* (3)) — and the shell
/// reaped; after a non-zero exit the shell is reaped and its group left as
/// it was (`tasks/pending-decisions.md` §1 *Rev 99*). After a successful exit the output is read to its end, which the
/// kill brings at once, bounded by the deadline and, once the end marker is
/// in, by [`READER_GRACE`]: only a process that left the group can still
/// hold the pipe, and then the output's reader thread stays until it lets
/// go. After a failed exit of an interactive run (`-i` among `args`) it
/// waits, at most [`READER_GRACE`] and the deadline, for the output's reader
/// to reach the pipe's end or the start marker, so a run that printed the
/// marker and then failed is told from one that ended first
/// (CAPTURE-RETRY); the group is left as the failed exit left it.
#[cfg(unix)]
fn run_login_shell(
    shell: &Path,
    args: &[&str],
    started: Instant,
    deadline: Duration,
    keys: &[String],
) -> Run {
    // A conclusion that is never a reason for a second run.
    let unread = |outcome| Run {
        outcome,
        printed_start: true,
    };
    use std::io::Read;
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    let mut command = Command::new(shell);
    command
        .args(args)
        .arg(CAPTURE_COMMAND)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .env("TERM", "dumb")
        // An app started from Apple's Terminal lends the shell its tab's
        // session id, and an interactive zsh or bash then runs
        // `/etc/zshrc_Apple_Terminal` (`/etc/bashrc_Apple_Terminal`), whose
        // exit hook rewrites that tab's `~/.zsh_sessions/<id>.*` (or
        // `~/.bash_sessions/`) and the history file in the person's own
        // home; both scripts do nothing without the id. For the capture's
        // shell only: the daemon's command never takes it from here.
        .env_remove("TERM_SESSION_ID");
    // A session of its own, so an interactive shell has no terminal to take
    // (an app started from one would otherwise lend it its own) and no job
    // control to give a background child a group of its own; the shell
    // leads the session's one group, its pid the group's id.
    // SAFETY: `setsid` is async-signal-safe and changes only the child.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => return unread(Err(CaptureFailure::Start(e.to_string()))),
    };
    let group = child.id() as libc::pid_t;
    let span = deadline;
    let deadline = started + deadline;

    // The output, as it arrives, read on a thread of its own: the shell is
    // waited on here, so a pipe held open by a descendant never holds the
    // capture. `true` once the pipe has ended.
    let output = Arc::new(Mutex::new((Vec::new(), false)));
    if let Some(stdout) = child.stdout.take() {
        let sink = Arc::clone(&output);
        #[cfg(test)]
        OUTPUT_READERS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let spawned = std::thread::Builder::new()
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
                drop(stdout);
                #[cfg(test)]
                std::thread::sleep(Duration::from_millis(
                    READER_END_DELAY_MS.load(std::sync::atomic::Ordering::SeqCst),
                ));
                #[cfg(test)]
                OUTPUT_READERS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
                sink.lock().unwrap_or_else(|p| p.into_inner()).1 = true;
            });
        #[cfg(test)]
        if spawned.is_err() {
            OUTPUT_READERS.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
        }
        drop(spawned);
    }

    // Called only before the shell is reaped: its id then names this
    // capture's group and no other.
    let kill_group = || {
        // SAFETY: a plain syscall on this capture's own group.
        unsafe {
            libc::killpg(group, libc::SIGKILL);
        }
    };
    let failed = |child: &mut std::process::Child, failure: CaptureFailure| {
        kill_group();
        let _ = child.wait();
        unread(Err(failure))
    };

    let poll = Duration::from_millis(10);
    let succeeded = loop {
        match exit_seen(group) {
            Ok(Some(succeeded)) => break succeeded,
            Ok(None) => {}
            Err(e) => return failed(&mut child, CaptureFailure::Exited(e.to_string())),
        }
        let now = Instant::now();
        if now >= deadline {
            return failed(&mut child, CaptureFailure::TimedOut(span));
        }
        if output.lock().unwrap_or_else(|p| p.into_inner()).0.len() > OUTPUT_CAP {
            return failed(&mut child, CaptureFailure::TooMuchOutput);
        }
        std::thread::sleep(poll.min(deadline - now));
    };
    // The shell has exited: after a successful exit its group goes now,
    // with whatever the profile left running, and then the shell is reaped.
    // After a failed one the group is left as the parent left it (the
    // pick's words, *Rev 91* (3): after a successful exit; *Rev 99*).
    if succeeded {
        kill_group();
    }
    let status = match child.wait() {
        Ok(status) => status,
        Err(e) => return unread(Err(CaptureFailure::Exited(e.to_string()))),
    };
    let start_line = format!("{START_MARKER}\n");
    if !status.success() {
        let failure = Err(CaptureFailure::Exited(status.to_string()));
        if !args.contains(&"-i") {
            return unread(failure);
        }
        // Whether the shell ended before its command ran: what it printed
        // is in the pipe, but a child it left may hold the pipe open, so
        // the wait is bounded.
        let until = (Instant::now() + READER_GRACE).min(deadline);
        loop {
            {
                let (bytes, ended) = &*output.lock().unwrap_or_else(|p| p.into_inner());
                let printed_start = contains(bytes, start_line.as_bytes());
                if printed_start || *ended || Instant::now() >= until {
                    return Run {
                        outcome: failure,
                        printed_start,
                    };
                }
            }
            std::thread::sleep(poll);
        }
    }

    // What the shell printed is in the pipe, and nothing in its group holds
    // the pipe any more: wait for the reader to reach the end — at most to
    // the deadline, and once the end marker is in, at most READER_GRACE.
    let grace = Instant::now() + READER_GRACE;
    loop {
        {
            let (bytes, ended) = &*output.lock().unwrap_or_else(|p| p.into_inner());
            if bytes.len() > OUTPUT_CAP {
                return unread(Err(CaptureFailure::TooMuchOutput));
            }
            let now = Instant::now();
            if *ended || now >= deadline || (now >= grace && contains(bytes, END_MARKER.as_bytes()))
            {
                return Run {
                    outcome: environment_between_markers(bytes, keys),
                    printed_start: contains(bytes, start_line.as_bytes()),
                };
            }
        }
        std::thread::sleep(poll);
    }
}

#[cfg(all(not(unix), test))]
pub(crate) fn capture_login_path(
    _shell: &Path,
    _started: Instant,
    _deadline: Duration,
    _keys: &[String],
) -> Result<Captured, CaptureFailure> {
    Err(CaptureFailure::NoShell)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// `PATH`, and each of `keys` present, from the environment printed between
/// the last end marker and the last start marker before it: NUL-terminated
/// `KEY=value` entries. Every other key is dropped (the allowlist is `PATH`
/// and the keys `[daemon.env] passthrough_keys` lists). A listed key is
/// passed with whatever value it holds, an empty one included.
pub(crate) fn environment_between_markers(
    output: &[u8],
    keys: &[String],
) -> Result<Captured, CaptureFailure> {
    let start = format!("{START_MARKER}\n");
    let end = format!("\n{END_MARKER}");
    let end_at = rfind(output, end.as_bytes()).ok_or(CaptureFailure::NoMarkers)?;
    let start_at = rfind(&output[..end_at], start.as_bytes()).ok_or(CaptureFailure::NoMarkers)?;
    let environment = &output[start_at + start.len()..end_at];
    let mut path = None;
    let mut found: Vec<Option<&[u8]>> = vec![None; keys.len()];
    for entry in environment.split(|&b| b == 0) {
        if let Some(value) = entry.strip_prefix(b"PATH=") {
            path = Some(value);
            continue;
        }
        for (key, slot) in keys.iter().zip(found.iter_mut()) {
            if let Some(value) = entry
                .strip_prefix(key.as_bytes())
                .and_then(|rest| rest.strip_prefix(b"="))
            {
                *slot = Some(value);
            }
        }
    }
    match path {
        Some(value) if !value.is_empty() => Ok(Captured {
            path: os_string(value),
            passed: keys
                .iter()
                .zip(found)
                .filter_map(|(key, value)| value.map(|value| (key.clone(), os_string(value))))
                .collect(),
            second_run: false,
        }),
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
