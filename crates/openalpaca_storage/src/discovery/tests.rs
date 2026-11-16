use super::*;
use chrono::TimeZone;

#[test]
fn test_token_generation() {
    let token = generate_token();
    // 32 bytes -> 43 chars in base64url (no padding)
    assert_eq!(token.len(), 43);
    // Should be valid base64url
    assert!(URL_SAFE_NO_PAD.decode(&token).is_ok());
}

#[test]
fn test_make_discovery() {
    let d = make_discovery("127.0.0.1", 8080, "test-id".to_string(), "0.1.0");
    assert_eq!(d.schema, 1);
    assert_eq!(d.listen.host, "127.0.0.1");
    assert_eq!(d.listen.port, 8080);
    assert_eq!(d.instance_id, "test-id");
    assert_eq!(d.build.version, "0.1.0");
    assert!(d.auth.expires_at > Utc::now());
}

/// `make_discovery` writes the protocol the one read site reports (Q04):
/// `support::protocol::reported()`, which is `PROTOCOL` unless a debug build
/// is told otherwise.
#[test]
fn make_discovery_writes_the_reported_protocol() {
    let d = make_discovery("127.0.0.1", 9999, "my-instance".to_string(), "1.0.0");
    assert_eq!(d.build.protocol, crate::support::protocol::reported());
    assert_eq!(d.build.version, "1.0.0");
}

/// The protocol written is the one read site's, not a literal (C-20): in a
/// child of this test binary given the debug override — so no other test in
/// this process sees the variable — `make_discovery` writes `PROTOCOL + 1`.
#[cfg(debug_assertions)]
#[test]
fn make_discovery_s_protocol_is_the_one_read_site_s() {
    use crate::support::protocol::{PROTOCOL, PROTOCOL_PLUS_ONE_ENV};
    const CHILD: &str = "OPENALPACA_TEST_DISCOVERY_PROTOCOL_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let d = make_discovery("127.0.0.1", 9999, "my-instance".to_string(), "1.0.0");
        assert_eq!(d.build.protocol, PROTOCOL + 1);
        return;
    }
    let out = std::process::Command::new(std::env::current_exe().expect("this test binary"))
        .args([
            "--exact",
            "discovery::tests::make_discovery_s_protocol_is_the_one_read_site_s",
            "--test-threads=1",
        ])
        .env(CHILD, "1")
        .env(PROTOCOL_PLUS_ONE_ENV, "1")
        .output()
        .expect("the child run");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && stdout.contains("test result: ok. 1 passed"),
        "{stdout}{}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn test_connection_info_from_discovery() {
    let d = make_discovery("127.0.0.1", 9999, "my-instance".to_string(), "1.0.0");
    let info = ConnectionInfo::from(&d);
    assert_eq!(info.base_url, "http://127.0.0.1:9999");
    assert_eq!(info.instance_id, "my-instance");
    assert!(!info.token.is_empty());
}

// ============================================================================
// The wire format is pinned byte for byte (deps plan Q01)
// ============================================================================

/// The daemon writes `discovery.json` with `serde_json::to_vec_pretty`, so
/// its bytes are the struct's field order, serde's two-space layout and
/// chrono's RFC 3339 with as many fraction digits as the instant needs. The
/// CLI and the shell read it; moving these types to another crate (Q05) must
/// leave every byte in place, and this literal is what says so.
const PINNED_TOKEN: &str = "3q2-7wAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
const PINNED_DISCOVERY_JSON: &str = r#"{
  "schema": 1,
  "instance_id": "0f1e2d3c-4b5a-4968-8776-655443322110",
  "pid": 51823,
  "started_at": "2026-10-11T12:00:00.123456789Z",
  "listen": {
    "host": "127.0.0.1",
    "port": 51823
  },
  "auth": {
    "token": "3q2-7wAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
    "issued_at": "2026-10-11T12:00:00Z",
    "expires_at": "2026-10-12T12:00:00Z"
  },
  "build": {
    "version": "0.1.0",
    "protocol": 1
  }
}"#;

fn pinned_discovery() -> Discovery {
    let noon = Utc.with_ymd_and_hms(2026, 10, 11, 12, 0, 0).unwrap();
    Discovery {
        schema: 1,
        instance_id: "0f1e2d3c-4b5a-4968-8776-655443322110".to_string(),
        pid: 51823,
        started_at: noon + Duration::nanoseconds(123_456_789),
        listen: Listen {
            host: "127.0.0.1".to_string(),
            port: 51823,
        },
        auth: Auth {
            token: PINNED_TOKEN.to_string(),
            issued_at: noon,
            expires_at: noon + Duration::hours(24),
        },
        build: Build {
            version: "0.1.0".to_string(),
            protocol: 1,
        },
    }
}

#[test]
fn discovery_json_bytes_are_pinned() {
    let written = serde_json::to_vec_pretty(&pinned_discovery()).unwrap();
    assert_eq!(
        String::from_utf8(written.clone()).unwrap(),
        PINNED_DISCOVERY_JSON
    );
    // The reader reads what the writer writes: the pinned bytes parse, and
    // what they parse to writes back as the same bytes.
    let read: Discovery = serde_json::from_slice(PINNED_DISCOVERY_JSON.as_bytes()).unwrap();
    assert_eq!(serde_json::to_vec_pretty(&read).unwrap(), written);
}

/// `ConnectionInfo` crosses into the webview in camelCase, and the GUI checks
/// the three names before it trusts the value (`lib/connection.ts`,
/// `isConnectionInfo`): `baseUrl`, `token`, `instanceId`, in this order, then
/// Q04's two additive fields, the daemon's `build.version` and
/// `build.protocol` as `version` and `protocol`, which an older shell's
/// webview never asked for and a window reads as absent from an older one.
#[test]
fn connection_info_serde_names_are_the_guis() {
    let info = ConnectionInfo::from(&pinned_discovery());
    assert_eq!(
        serde_json::to_string(&info).unwrap(),
        concat!(
            r#"{"baseUrl":"http://127.0.0.1:51823","#,
            r#""token":"3q2-7wAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA","#,
            r#""instanceId":"0f1e2d3c-4b5a-4968-8776-655443322110","#,
            r#""version":"0.1.0","protocol":1}"#
        )
    );
}

// ============================================================================
// T21 (2): the token and its expiry rotated while the daemon runs
// ============================================================================

/// A rotation gives the file a new `auth` and leaves every other byte: the
/// GUI drops all its state on an `instance_id` change, and `started_at`,
/// `pid`, `listen` and `build` describe the run, not the token.
#[test]
fn a_rotation_replaces_the_auth_and_nothing_else() {
    let d = pinned_discovery();
    let at = d.auth.issued_at + ROTATE_AFTER;
    let rotated = d.with_rotated_auth(at);

    assert_ne!(rotated.auth.token, d.auth.token);
    assert_eq!(rotated.auth.token.len(), 43);
    assert_eq!(rotated.auth.issued_at, at);
    assert_eq!(rotated.auth.expires_at, at + TOKEN_LIFETIME);
    assert!(rotated.auth.expires_at > d.auth.expires_at);

    let mut without_auth = serde_json::to_value(&rotated).unwrap();
    without_auth["auth"] = serde_json::to_value(&d.auth).unwrap();
    assert_eq!(without_auth, serde_json::to_value(&d).unwrap());
}

/// The schedule: twelve hours after a token's issue, half its lifetime, so a
/// token read from the file always has at least twelve hours left; and at
/// once when the wall clock has gone back past the issue.
#[test]
fn rotation_is_due_twelve_hours_after_issue_or_when_the_clock_went_back() {
    assert_eq!(TOKEN_LIFETIME, Duration::hours(24));
    assert_eq!(ROTATE_AFTER, Duration::hours(12));
    let d = pinned_discovery();
    let issued = d.auth.issued_at;

    assert!(!d.rotation_due(issued));
    assert!(!d.rotation_due(issued + ROTATE_AFTER - Duration::seconds(1)));
    assert!(d.rotation_due(issued + ROTATE_AFTER));
    // A Mac asleep past the expiry: due at the first look after the wake.
    assert!(d.rotation_due(issued + Duration::days(3)));
    assert!(d.rotation_due(issued - Duration::seconds(1)));
}

/// What a client reads from a daemon that has kept its schedule is never
/// expired, however long the daemon has run; the file as the start wrote it,
/// which is all a daemon wrote before T21, is refused after a day.
#[test]
fn a_file_rotated_on_schedule_is_never_expired() {
    let now = Utc::now();
    let started = now - Duration::hours(25);
    let first =
        make_discovery("127.0.0.1", 51823, "i".to_string(), "0.1.0").with_rotated_auth(started);
    assert!(
        ensure_not_expired(&first).is_err(),
        "the start's token lasts a day"
    );

    let mut d = first.clone();

    let mut at = started;
    while at <= now {
        if d.rotation_due(at) {
            d = d.with_rotated_auth(at);
        }
        at += Duration::minutes(1);
    }
    assert_eq!(d.auth.issued_at, started + ROTATE_AFTER * 2);
    assert_eq!(d.started_at, first.started_at);
    ensure_not_expired(&d).expect("a rotated file is not expired");
}
