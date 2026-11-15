//! Where this daemon's `PATH` came from (Q21): one boot line in
//! `daemon.log`.
//!
//! The app sets [`store::PATH_SOURCE_ENV`] on the daemon it spawns —
//! `login-shell <ms>` when its capture of the login shell's `PATH` answered,
//! `fallback-list <ms>` when it gave the fixed list instead. A daemon started
//! any other way (`openalpaca daemon start` in a terminal, by hand) has no
//! such variable and runs on the `PATH` it inherited. The line names the
//! source and the capture's duration, never the `PATH`, and never echoes
//! the variable's raw value: anything but a known source and a number reads
//! `inherited`.
//!
//! N-DAEMON-2: after the number the app may add the names of the variables
//! it passed beside `PATH` from the login shell, because the owner listed
//! them in `daemon.toml`'s `[daemon.env] passthrough_keys` —
//! `login-shell 212 AWS_PROFILE,KUBECONFIG` — and the line names them, never
//! their values. A list that is not variable names, comma-separated, reads
//! `inherited` like any other malformed value. This daemon does not read the
//! list itself: the app reads it before the spawn.

use openalpaca_storage::store;
use std::ffi::OsStr;

/// What the boot line says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathSource {
    /// `login-shell`, `fallback-list` or `inherited`.
    pub source: &'static str,
    /// The app's capture time, for the two sources the app sets.
    pub capture_ms: Option<u64>,
    /// The variables the app passed beside `PATH`, by name (N-DAEMON-2).
    pub passed_keys: Vec<String>,
}

/// A daemon nobody announced a `PATH` to.
const INHERITED: PathSource = PathSource {
    source: "inherited",
    capture_ms: None,
    passed_keys: Vec::new(),
};

/// Whether `name` is a variable name: a letter or `_`, then letters, digits
/// and `_`.
fn is_variable_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    matches!(bytes.next(), Some(b) if b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// [`store::PATH_SOURCE_ENV`]'s value read strictly: a known source, one
/// space, a whole number of milliseconds, and optionally one more space and
/// the names of the keys passed, comma-separated; anything else, or nothing,
/// is `inherited`.
pub fn parse_path_source(value: Option<&OsStr>) -> PathSource {
    let Some(text) = value.and_then(OsStr::to_str) else {
        return INHERITED;
    };
    let Some((source, rest)) = text.split_once(' ') else {
        return INHERITED;
    };
    let (ms, keys) = match rest.split_once(' ') {
        Some((ms, keys)) => (ms, Some(keys)),
        None => (rest, None),
    };
    let passed_keys = match keys {
        None => Vec::new(),
        Some(keys) => {
            let names: Vec<&str> = keys.split(',').collect();
            if !names.iter().all(|name| is_variable_name(name)) {
                return INHERITED;
            }
            names.into_iter().map(str::to_owned).collect()
        }
    };
    let source = match source {
        "login-shell" => "login-shell",
        "fallback-list" => "fallback-list",
        _ => return INHERITED,
    };
    if ms.is_empty() || !ms.bytes().all(|b| b.is_ascii_digit()) {
        return INHERITED;
    }
    match ms.parse::<u64>() {
        Ok(capture_ms) => PathSource {
            source,
            capture_ms: Some(capture_ms),
            passed_keys,
        },
        Err(_) => INHERITED,
    }
}

/// The boot line's text: the source and the capture's duration, then the
/// keys passed beside `PATH` by name, when there are any.
pub fn path_source_line(path: &PathSource) -> String {
    let mut line = match path.capture_ms {
        Some(ms) => format!(
            "PATH source: {} (the app's capture took {ms} ms)",
            path.source
        ),
        None => format!(
            "PATH source: {} (from whatever started this daemon)",
            path.source
        ),
    };
    if !path.passed_keys.is_empty() {
        line.push_str(
            "; also passed from the login shell, as [daemon.env] passthrough_keys lists: ",
        );
        line.push_str(&path.passed_keys.join(", "));
    }
    line
}

/// This daemon's [`PathSource`], from its environment.
pub fn read_path_source() -> PathSource {
    parse_path_source(std::env::var_os(store::PATH_SOURCE_ENV).as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_apps_two_sources_are_read_with_their_capture_time() {
        assert_eq!(
            parse_path_source(Some(OsStr::new("login-shell 212"))),
            PathSource {
                source: "login-shell",
                capture_ms: Some(212),
                passed_keys: Vec::new(),
            }
        );
        assert_eq!(
            parse_path_source(Some(OsStr::new("fallback-list 3001"))),
            PathSource {
                source: "fallback-list",
                capture_ms: Some(3001),
                passed_keys: Vec::new(),
            }
        );
    }

    /// No variable — a daemon started from a terminal — and anything that
    /// is not a known source and a number read `inherited`, and the line
    /// never carries what the variable held.
    #[test]
    fn anything_else_reads_inherited_and_is_never_echoed() {
        assert_eq!(parse_path_source(None), INHERITED);
        for raw in [
            "",
            "login-shell",
            "login-shell ",
            "login-shell 12x",
            "login-shell -1",
            "login-shell 1 2",
            "inherited 5",
            "/opt/homebrew/bin:/usr/bin 7",
            "login-shell /opt/q21-secret/bin",
            "fallback-list 99999999999999999999999",
        ] {
            let read = parse_path_source(Some(OsStr::new(raw)));
            assert_eq!(read, INHERITED, "{raw:?}");
            let line = path_source_line(&read);
            assert_eq!(
                line,
                "PATH source: inherited (from whatever started this daemon)"
            );
            if !raw.is_empty() {
                assert!(!line.contains(raw), "{line:?} echoes {raw:?}");
            }
        }
    }

    /// N-DAEMON-2: the keys the app passed beside `PATH`, read by name
    /// after the capture time, and named in the boot line, never a value.
    #[test]
    fn the_keys_passed_are_read_by_name_and_named_in_the_boot_line() {
        let read = parse_path_source(Some(OsStr::new("login-shell 212 AWS_PROFILE,KUBECONFIG")));
        assert_eq!(read.source, "login-shell");
        assert_eq!(read.capture_ms, Some(212));
        assert_eq!(read.passed_keys, ["AWS_PROFILE", "KUBECONFIG"]);
        assert_eq!(
            path_source_line(&read),
            "PATH source: login-shell (the app's capture took 212 ms); \
             also passed from the login shell, as [daemon.env] passthrough_keys lists: \
             AWS_PROFILE, KUBECONFIG"
        );
        for raw in [
            "login-shell 212 ",
            "login-shell 212 AWS_PROFILE,",
            "login-shell 212 ,AWS_PROFILE",
            "login-shell 212 AWS_PROFILE=q21-value",
            "login-shell 212 AWS_PROFILE KUBECONFIG",
            "login-shell 212 1BAD",
            "login-shell 212 /q21/secret",
        ] {
            let read = parse_path_source(Some(OsStr::new(raw)));
            assert_eq!(read, INHERITED, "{raw:?}");
            assert!(!path_source_line(&read).contains("q21"), "{raw:?}");
        }
    }

    #[test]
    fn the_boot_line_names_the_source_and_the_duration() {
        assert_eq!(
            path_source_line(&parse_path_source(Some(OsStr::new("login-shell 212")))),
            "PATH source: login-shell (the app's capture took 212 ms)"
        );
        assert_eq!(
            path_source_line(&parse_path_source(Some(OsStr::new("fallback-list 3001")))),
            "PATH source: fallback-list (the app's capture took 3001 ms)"
        );
    }
}
