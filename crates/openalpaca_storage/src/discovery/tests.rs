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
/// `isConnectionInfo`): `baseUrl`, `token`, `instanceId`, in this order.
#[test]
fn connection_info_serde_names_are_the_guis() {
    let info = ConnectionInfo::from(&pinned_discovery());
    assert_eq!(
        serde_json::to_string(&info).unwrap(),
        concat!(
            r#"{"baseUrl":"http://127.0.0.1:51823","#,
            r#""token":"3q2-7wAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA","#,
            r#""instanceId":"0f1e2d3c-4b5a-4968-8776-655443322110"}"#
        )
    );
}
