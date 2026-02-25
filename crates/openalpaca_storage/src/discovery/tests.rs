use super::*;

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
