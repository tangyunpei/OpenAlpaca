//! What the daemon does with a command line (W1).
//!
//! `openalpacad` takes no arguments — it is configured by files and
//! environment — and until this module existed it did not *read* any either.
//! A mistyped `openalpacad --help`, the reflex of anyone meeting a binary for
//! the first time, therefore **booted a daemon**: store root, master key,
//! singleton lock, discovery file and database, all created against whatever
//! `~/.openalpaca` the machine had, for someone who only wanted usage text.
//!
//! So the parse happens before any of that, and it is deliberately tiny: no
//! clap, no subcommands, nothing that could grow into a second way to
//! configure the daemon. Three outcomes and a usage string.

/// What the command line asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArgsOutcome {
    /// No arguments — boot the daemon.
    Run,
    /// `--help` / `-h`: print [`USAGE`] on stdout, exit 0.
    Help,
    /// `--version` / `-V`: print the version on stdout, exit 0.
    Version,
    /// Anything else: name it on stderr with [`USAGE`], exit 2.
    Unknown(String),
}

/// The whole of the daemon's command-line contract.
pub const USAGE: &str = "\
openalpacad — the OpenAlpaca daemon: HTTP/WS API, orchestrator, scheduler.

Usage:
  openalpacad              Run the daemon in the foreground (takes no arguments).
  openalpacad --help       Print this text and exit.
  openalpacad --version    Print the version and exit.

The daemon is configured by files and environment, never by flags. The usual
way to run it is `openalpaca daemon start` (`stop`, `status` for the rest).

Environment:
  OPENALPACA_HOME_STORE  Absolute path to the store root (default ~/.openalpaca).
                         Its state/ holds the database, discovery file, singleton
                         lock, master key and logs.
  OPENALPACA_CONFIG_DIR  Directory holding daemon.toml, llm.toml, mcp.toml and the
                         agent/skill/persona files. A path that does not exist is
                         warned about and ignored.";

/// Classify the arguments **after** the program name.
///
/// First argument wins: the scan stops at the first thing it recognises, so
/// `--help --bogus` is help and `--bogus --help` is a refusal. Nothing here
/// touches the filesystem, the environment or the logger — that is the point.
pub fn parse_args(mut args: impl Iterator<Item = String>) -> ArgsOutcome {
    match args.next() {
        None => ArgsOutcome::Run,
        Some(arg) => match arg.as_str() {
            "--help" | "-h" => ArgsOutcome::Help,
            "--version" | "-V" => ArgsOutcome::Version,
            other => ArgsOutcome::Unknown(other.to_string()),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> ArgsOutcome {
        parse_args(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn no_arguments_runs_the_daemon() {
        assert_eq!(parse(&[]), ArgsOutcome::Run);
    }

    #[test]
    fn both_spellings_of_help_and_version_are_understood() {
        assert_eq!(parse(&["--help"]), ArgsOutcome::Help);
        assert_eq!(parse(&["-h"]), ArgsOutcome::Help);
        assert_eq!(parse(&["--version"]), ArgsOutcome::Version);
        assert_eq!(parse(&["-V"]), ArgsOutcome::Version);
    }

    /// The whole reason the module exists: an argument the daemon does not
    /// know must never fall through into a boot.
    #[test]
    fn anything_else_is_named_and_refused() {
        assert_eq!(
            parse(&["--port=8080"]),
            ArgsOutcome::Unknown("--port=8080".to_string())
        );
        assert_eq!(parse(&["start"]), ArgsOutcome::Unknown("start".to_string()));
        assert_eq!(parse(&["-v"]), ArgsOutcome::Unknown("-v".to_string()));
        assert_eq!(parse(&[""]), ArgsOutcome::Unknown(String::new()));
    }

    #[test]
    fn the_first_argument_decides() {
        assert_eq!(parse(&["--help", "--bogus"]), ArgsOutcome::Help);
        assert_eq!(
            parse(&["--bogus", "--help"]),
            ArgsOutcome::Unknown("--bogus".to_string())
        );
    }

    /// The usage text has a job beyond existing: it must name the two
    /// environment variables and the CLI verb, because that is all a reader
    /// who typed `--help` has to go on.
    #[test]
    fn the_usage_text_names_what_steers_the_daemon() {
        assert!(USAGE.contains("OPENALPACA_HOME_STORE"));
        assert!(USAGE.contains("OPENALPACA_CONFIG_DIR"));
        assert!(USAGE.contains("openalpaca daemon start"));
        assert!(USAGE.contains("takes no arguments"));
    }
}
