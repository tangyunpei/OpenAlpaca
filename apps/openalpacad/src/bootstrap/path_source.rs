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

use openalpaca_storage::store;
use std::ffi::OsStr;

/// What the boot line says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathSource {
    /// `login-shell`, `fallback-list` or `inherited`.
    pub source: &'static str,
    /// The app's capture time, for the two sources the app sets.
    pub capture_ms: Option<u64>,
}

/// A daemon nobody announced a `PATH` to.
const INHERITED: PathSource = PathSource {
    source: "inherited",
    capture_ms: None,
};

/// [`store::PATH_SOURCE_ENV`]'s value read strictly: a known source, one
/// space, a whole number of milliseconds; anything else, or nothing, is
/// `inherited`.
pub fn parse_path_source(value: Option<&OsStr>) -> PathSource {
    let Some(text) = value.and_then(OsStr::to_str) else {
        return INHERITED;
    };
    let Some((source, ms)) = text.split_once(' ') else {
        return INHERITED;
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
        },
        Err(_) => INHERITED,
    }
}

/// The boot line's text.
pub fn path_source_line(path: PathSource) -> String {
    match path.capture_ms {
        Some(ms) => format!(
            "PATH source: {} (the app's capture took {ms} ms)",
            path.source
        ),
        None => format!(
            "PATH source: {} (from whatever started this daemon)",
            path.source
        ),
    }
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
                capture_ms: Some(212)
            }
        );
        assert_eq!(
            parse_path_source(Some(OsStr::new("fallback-list 3001"))),
            PathSource {
                source: "fallback-list",
                capture_ms: Some(3001)
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
            let line = path_source_line(read);
            assert_eq!(
                line,
                "PATH source: inherited (from whatever started this daemon)"
            );
            if !raw.is_empty() {
                assert!(!line.contains(raw), "{line:?} echoes {raw:?}");
            }
        }
    }

    #[test]
    fn the_boot_line_names_the_source_and_the_duration() {
        assert_eq!(
            path_source_line(parse_path_source(Some(OsStr::new("login-shell 212")))),
            "PATH source: login-shell (the app's capture took 212 ms)"
        );
        assert_eq!(
            path_source_line(parse_path_source(Some(OsStr::new("fallback-list 3001")))),
            "PATH source: fallback-list (the app's capture took 3001 ms)"
        );
    }
}
