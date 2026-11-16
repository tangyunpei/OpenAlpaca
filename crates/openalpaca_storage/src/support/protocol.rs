//! The daemon's wire protocol as a number (DU-29 (a), Q04): the one source
//! `make_discovery` (`discovery.json`'s `build.protocol`), `GET /v1/health`
//! (`protocol`) and the app shell's `decide_reuse` read.
//!
//! A daemon reports [`reported`]; a client accepts a daemon whose protocol
//! is from [`OLDEST_SUPPORTED`] to [`PROTOCOL`], reuses one it speaks
//! exactly, reuses an older one it still supports with a note, and refuses
//! any other with a mismatch it names — never killing, never spawning beside
//! it. When the two numbers move is API-CONTRACT's written rule, not this
//! module's.

/// The protocol this build speaks.
pub const PROTOCOL: u32 = 1;

/// The oldest protocol this build still works with.
pub const OLDEST_SUPPORTED: u32 = 1;

/// The variable a **debug** build's daemon reads to report `PROTOCOL + 1`
/// (C-20): set to `1`, a hand-started debug daemon looks newer than every
/// app built from the same commit, which is how the native mismatch check
/// gets a daemon to refuse. Compiled only under `debug_assertions`, so a
/// release build neither reads it nor carries its name.
#[cfg(debug_assertions)]
pub const PROTOCOL_PLUS_ONE_ENV: &str = "OPENALPACA_DEBUG_PROTOCOL_PLUS_ONE";

/// The protocol this daemon reports — the one read site of the debug
/// override: [`PROTOCOL`], or in a debug build with
/// [`PROTOCOL_PLUS_ONE_ENV`] set to `1`, `PROTOCOL + 1`.
pub fn reported() -> u32 {
    #[cfg(debug_assertions)]
    {
        reported_given(std::env::var_os(PROTOCOL_PLUS_ONE_ENV).as_deref())
    }
    #[cfg(not(debug_assertions))]
    {
        PROTOCOL
    }
}

/// [`reported`]'s decision over the variable's value, apart from the
/// environment, so a test drives it without setting a variable every other
/// test reads.
#[cfg(debug_assertions)]
fn reported_given(value: Option<&std::ffi::OsStr>) -> u32 {
    if value.is_some_and(|v| v == "1") {
        PROTOCOL + 1
    } else {
        PROTOCOL
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_supported_range_is_not_empty() {
        const { assert!(OLDEST_SUPPORTED <= PROTOCOL) };
    }

    #[cfg(debug_assertions)]
    #[test]
    fn a_debug_build_reports_one_more_only_when_told_exactly() {
        use std::ffi::OsStr;
        assert_eq!(reported_given(None), PROTOCOL);
        assert_eq!(reported_given(Some(OsStr::new("1"))), PROTOCOL + 1);
        for other in ["", "0", "2", "true", " 1"] {
            assert_eq!(
                reported_given(Some(OsStr::new(other))),
                PROTOCOL,
                "{other:?}"
            );
        }
    }
}
