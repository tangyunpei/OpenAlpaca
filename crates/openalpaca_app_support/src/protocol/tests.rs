//! The protocol's numbers, the starting state's budget, and
//! `decide_reuse`'s table (Q04; plan-rev3 §7.2 G3-b, DU-29 (a), C-09, A-18,
//! A-20; moved from the app shell's `reuse` tests by Q05) over looks the
//! tests write: no file, no process table, no socket of the machine's, so
//! no test holds the home-store guard.

use super::*;
use crate::discovery::{self, Discovery};

// ============================================================================
// The numbers and the budget
// ============================================================================

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

/// The starting state's budget (Q04, the owner's answer "b" of
/// 2026-11-16): the MCP connect's shipped bound, 30 s in both
/// `mcp.toml`s the daemon can be given, plus a margin — 45 s. It is
/// below the connect plus one `tools/list` (`request_timeout_secs`, 30 s
/// shipped), which the figure's doc names as not covered: a change that
/// makes it cover that bring-up changes the doc too.
#[test]
fn the_starting_budget_is_the_shipped_mcp_connect_bound_plus_a_margin() {
    assert_eq!(STARTING_BUDGET, Duration::from_secs(45));
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let seconds = |text: &str, shipped: &str, key: &str| -> u64 {
        let lines: Vec<&str> = text
            .lines()
            .filter(|line| line.trim_start().starts_with(key))
            .collect();
        assert_eq!(lines.len(), 1, "{shipped} sets {key} once");
        lines[0]
            .split('=')
            .nth(1)
            .and_then(|value| value.split('#').next())
            .map(str::trim)
            .and_then(|value| value.parse().ok())
            .expect("a number of seconds")
    };
    for shipped in [
        "config/mcp.toml",
        "scripts/release/templates/config/mcp.toml",
    ] {
        let text = std::fs::read_to_string(root.join(shipped)).expect(shipped);
        let connect = seconds(&text, shipped, "connect_timeout_secs");
        let request = seconds(&text, shipped, "request_timeout_secs");
        assert_eq!(connect, 30, "{shipped}'s connect bound");
        assert_eq!(request, 30, "{shipped}'s request bound");
        assert!(
            STARTING_BUDGET >= Duration::from_secs(connect + 10),
            "{shipped}: the budget leaves a margin past the connect bound"
        );
        assert!(
            STARTING_BUDGET < Duration::from_secs(connect + request),
            "{shipped}: the budget now covers a server's whole bring-up; say so in its doc"
        );
    }
}

// ============================================================================
// The table
// ============================================================================

/// A client that speaks protocol 3 and reads back to 2, so the rows that
/// need a protocol older than its own but still supported, and one too old,
/// have values to run on (the build's own constants are both 1 today).
const CLIENT: ClientBuild = ClientBuild {
    protocol: 3,
    oldest_supported: 2,
    version: "0.4.0",
};

const INSTANCE: &str = "0f1e2d3c-4b5a-4968-8776-655443322110";

/// A daemon's discovery file: pid 4242, version `version`, protocol
/// `protocol`.
fn discovery_of(version: &str, protocol: u32) -> Discovery {
    let mut d = discovery::make_discovery("127.0.0.1", 51823, INSTANCE.into(), version);
    d.pid = 4242;
    d.build.protocol = protocol;
    d
}

/// Health's answer for `instance`, as a daemon writes it.
fn answered(instance: &str, protocol: Option<u32>, version: &str) -> Health {
    Health::Answered {
        instance_id: instance.into(),
        protocol,
        version: Some(version.into()),
    }
}

fn silent() -> Health {
    Health::Unanswered("connection refused".into())
}

fn look(discovery: DiscoveryRead, health: Health, pid_live: bool) -> Look {
    Look {
        discovery,
        health,
        pid_live,
    }
}

fn parsed(d: &Discovery) -> DiscoveryRead {
    DiscoveryRead::Parsed(d.clone())
}

/// What the table's verdict is, without the discovery file a reuse carries.
#[derive(Debug, PartialEq)]
enum Row {
    Spawn,
    Reuse(Option<Compat>),
    Mismatch {
        daemon_version: Option<String>,
        daemon_protocol: Option<u32>,
    },
    Starting(u32),
}

fn row(verdict: Verdict) -> Row {
    match verdict {
        Verdict::Spawn => Row::Spawn,
        Verdict::Reuse(reused) => Row::Reuse(reused.compat),
        Verdict::Mismatch(m) => Row::Mismatch {
            daemon_version: m.daemon_version,
            daemon_protocol: m.daemon_protocol,
        },
        Verdict::Starting { pid, .. } => Row::Starting(pid),
    }
}

fn decide(look: &Look) -> Row {
    row(decide_reuse(look, &CLIENT))
}

#[test]
fn no_discovery_file_spawns() {
    assert_eq!(
        decide(&look(DiscoveryRead::Missing, silent(), false)),
        Row::Spawn
    );
}

#[test]
fn a_compatible_daemon_is_reused_with_no_note() {
    let d = discovery_of("0.4.0", 3);
    assert_eq!(
        decide(&look(
            parsed(&d),
            answered(INSTANCE, Some(3), "0.4.0"),
            true
        )),
        Row::Reuse(None)
    );
}

/// The reused daemon is the file's, with the protocol and version its
/// health answered.
#[test]
fn a_reuse_carries_the_file_and_what_health_answered() {
    let d = discovery_of("0.4.0", 3);
    let Verdict::Reuse(reused) = decide_reuse(
        &look(parsed(&d), answered(INSTANCE, Some(3), "0.4.0"), true),
        &CLIENT,
    ) else {
        panic!("not a reuse");
    };
    assert_eq!(reused.discovery.instance_id, INSTANCE);
    assert_eq!(reused.discovery.auth.token, d.auth.token);
    assert_eq!(reused.protocol, 3);
    assert_eq!(reused.version, "0.4.0");
}

/// An older protocol the client still supports: reused, with the note's
/// cause the client words (the shell's sentence, `reuse_note`).
#[test]
fn an_older_supported_daemon_is_reused_noted_as_an_older_protocol() {
    let d = discovery_of("0.3.0", 2);
    assert_eq!(
        decide(&look(
            parsed(&d),
            answered(INSTANCE, Some(2), "0.3.0"),
            true
        )),
        Row::Reuse(Some(Compat::OlderProtocol))
    );
}

/// The seventh row (A-20): the client's protocol at another version —
/// reused, noted as another version.
#[test]
fn the_same_protocol_at_another_version_is_reused_noted_as_another_version() {
    let d = discovery_of("0.4.1", 3);
    assert_eq!(
        decide(&look(
            parsed(&d),
            answered(INSTANCE, Some(3), "0.4.1"),
            true
        )),
        Row::Reuse(Some(Compat::OtherVersion))
    );
}

#[test]
fn a_newer_daemon_is_a_mismatch() {
    let d = discovery_of("0.5.0", 4);
    assert_eq!(
        decide(&look(
            parsed(&d),
            answered(INSTANCE, Some(4), "0.5.0"),
            true
        )),
        Row::Mismatch {
            daemon_version: Some("0.5.0".into()),
            daemon_protocol: Some(4),
        }
    );
}

#[test]
fn a_daemon_older_than_the_oldest_supported_is_a_mismatch() {
    let d = discovery_of("0.1.0", 1);
    assert_eq!(
        decide(&look(
            parsed(&d),
            answered(INSTANCE, Some(1), "0.1.0"),
            true
        )),
        Row::Mismatch {
            daemon_version: Some("0.1.0".into()),
            daemon_protocol: Some(1),
        }
    );
}

/// A health body with no `protocol` is a daemon from before Q04: the
/// protocol its discovery file states is taken (every daemon has written
/// one), so it is judged as any other — never a spawn over a live daemon.
#[test]
fn a_health_answer_without_protocol_is_judged_by_the_files_protocol() {
    for (file_protocol, expected) in [
        (3, Row::Reuse(None)),
        (
            4,
            Row::Mismatch {
                daemon_version: Some("0.4.0".into()),
                daemon_protocol: Some(4),
            },
        ),
    ] {
        let d = discovery_of("0.4.0", file_protocol);
        assert_eq!(
            decide(&look(parsed(&d), answered(INSTANCE, None, "0.4.0"), true)),
            expected,
            "file protocol {file_protocol}"
        );
    }
}

/// What health answered is the daemon's word now; the file's `build` is
/// what it wrote at its start.
#[test]
fn health_s_protocol_wins_over_the_file_s() {
    let d = discovery_of("0.4.0", 3);
    assert_eq!(
        decide(&look(
            parsed(&d),
            answered(INSTANCE, Some(4), "0.4.0"),
            true
        )),
        Row::Mismatch {
            daemon_version: Some("0.4.0".into()),
            daemon_protocol: Some(4),
        }
    );
}

/// A file with no `build.protocol` (or any other field this build needs)
/// does not parse as this build's `Discovery`: over a live daemon's pid it
/// is a mismatch, never a spawn or a kill (*Rev 95* (4)).
#[test]
fn a_file_missing_its_protocol_over_a_live_daemon_is_a_mismatch() {
    let read = read_discovery_bytes(
        br#"{"schema":2,"pid":4242,"instance_id":"x","build":{"version":"0.9.0"}}"#,
    );
    assert_eq!(
        decide(&look(read, silent(), true)),
        Row::Mismatch {
            daemon_version: Some("0.9.0".into()),
            daemon_protocol: None,
        }
    );
}

/// The same file over a pid that is not a live daemon is no daemon.
#[test]
fn a_file_this_build_cannot_read_over_no_live_daemon_spawns() {
    let read = read_discovery_bytes(br#"{"schema":2,"pid":4242}"#);
    assert_eq!(decide(&look(read, silent(), false)), Row::Spawn);
}

/// A malformed file — not JSON, or no numeric `pid` — names nothing, so
/// there is nothing to reuse and nothing to protect: spawn (DU-29's first
/// step).
#[test]
fn a_malformed_file_spawns() {
    for bytes in [
        &b"not discovery"[..],
        &br#"{"pid":"4242"}"#[..],
        &br#"[4242]"#[..],
    ] {
        let read = read_discovery_bytes(bytes);
        std::assert_matches!(
            read,
            DiscoveryRead::Unreadable,
            "{:?}",
            String::from_utf8_lossy(bytes)
        );
        assert_eq!(decide(&look(read, silent(), true)), Row::Spawn);
    }
}

/// A file this build writes parses as itself.
#[test]
fn this_build_s_file_parses() {
    let d = discovery_of("0.4.0", 3);
    let read = read_discovery_bytes(&serde_json::to_vec_pretty(&d).unwrap());
    let DiscoveryRead::Parsed(back) = read else {
        panic!("not parsed");
    };
    assert_eq!(back.instance_id, d.instance_id);
}

/// A stale file (a crash, a reboot): its pid is not a live daemon and its
/// port answers nothing.
#[test]
fn a_stale_pid_with_a_dead_port_spawns() {
    let d = discovery_of("0.4.0", 3);
    assert_eq!(decide(&look(parsed(&d), silent(), false)), Row::Spawn);
}

/// Another instance answers at the address: the listener is not the
/// file's daemon. With the file's pid gone, spawn.
#[test]
fn an_instance_change_between_the_file_and_health_spawns_when_the_pid_is_gone() {
    let d = discovery_of("0.4.0", 3);
    assert_eq!(
        decide(&look(
            parsed(&d),
            answered("another-instance", Some(3), "0.4.0"),
            false
        )),
        Row::Spawn
    );
}

/// A live pid beats every spawn verdict: whatever the address says — silent,
/// a 404, another instance — while the file's pid is a live `openalpacad`,
/// a spawn would only lose the singleton lock to it (and rotate its log
/// under it), so the shell parks in `starting`.
#[test]
fn a_live_pid_parks_in_starting_whatever_the_address_says() {
    let d = discovery_of("0.4.0", 3);
    for health in [
        silent(),
        Health::Unanswered("HTTP 404".into()),
        answered("another-instance", Some(3), "0.4.0"),
    ] {
        assert_eq!(decide(&look(parsed(&d), health, true)), Row::Starting(4242));
    }
}

/// The rotated-token row (T21): what is judged is the daemon, not the
/// token's age, so a file past its first day — rotated or lapsed — over a
/// daemon whose health answers is reused.
#[test]
fn a_live_daemon_s_rotated_or_lapsed_token_is_reused() {
    let mut d = discovery_of("0.4.0", 3);
    let rotated = d.with_rotated_auth(d.started_at + discovery::ROTATE_AFTER * 3);
    assert_eq!(
        decide(&look(
            parsed(&rotated),
            answered(INSTANCE, Some(3), "0.4.0"),
            true
        )),
        Row::Reuse(None)
    );
    d.auth.expires_at = d.started_at - discovery::TOKEN_LIFETIME;
    assert_eq!(
        decide(&look(
            parsed(&d),
            answered(INSTANCE, Some(3), "0.4.0"),
            true
        )),
        Row::Reuse(None)
    );
}

// ============================================================================
// The health body, as both clients read it (CLI-PROTOCOL)
// ============================================================================

/// A daemon's body: its instance, protocol and version.
#[test]
fn a_health_body_is_read_with_its_instance_protocol_and_version() {
    let body = br#"{"status":"ok","version":"0.4.0","pid":7,"instance_id":"i-1","protocol":3}"#;
    assert_eq!(
        read_health_body(body),
        Health::Answered {
            instance_id: "i-1".into(),
            protocol: Some(3),
            version: Some("0.4.0".into()),
        }
    );
    // A daemon from before Q04 answers no protocol; `null` is the same.
    for body in [
        &br#"{"status":"ok","instance_id":"i-1"}"#[..],
        &br#"{"status":"ok","instance_id":"i-1","protocol":null}"#[..],
    ] {
        assert_eq!(
            read_health_body(body),
            Health::Answered {
                instance_id: "i-1".into(),
                protocol: None,
                version: None,
            }
        );
    }
}

/// What is no daemon's health, each with its reason.
#[test]
fn a_body_that_is_no_daemon_s_health_is_unanswered_with_why() {
    for (body, why) in [
        (&b"not json"[..], "a body that is not a JSON object"),
        (&b"[1]"[..], "a body that is not a JSON object"),
        (
            &br#"{"status":"starting","instance_id":"i-1"}"#[..],
            "a body whose status is not ok",
        ),
        (&br#"{"status":"ok"}"#[..], "a body with no instance_id"),
        (
            &br#"{"status":"ok","instance_id":"i-1","protocol":"3"}"#[..],
            "a protocol that is not a number",
        ),
        (
            &br#"{"status":"ok","instance_id":"i-1","protocol":4294967296}"#[..],
            "a protocol that is not a number",
        ),
    ] {
        assert_eq!(read_health_body(body), Health::Unanswered(why.into()));
    }
}
