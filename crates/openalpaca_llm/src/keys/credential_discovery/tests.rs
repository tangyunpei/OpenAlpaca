use super::*;

#[test]
fn test_oauth_token_not_expired() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    let token = OAuthToken {
        access_token: "test".to_string(),
        refresh_token: None,
        expires_at: Some(now + 3600), // 1 hour from now
    };
    assert!(!token.is_expired());
}

#[test]
fn test_oauth_token_expired() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    let token = OAuthToken {
        access_token: "test".to_string(),
        refresh_token: None,
        expires_at: Some(now - 100), // 100 seconds ago
    };
    assert!(token.is_expired());
}

#[test]
fn test_oauth_token_nearly_expired() {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    let token = OAuthToken {
        access_token: "test".to_string(),
        refresh_token: None,
        expires_at: Some(now + 30), // 30s remaining, < 60s threshold
    };
    assert!(token.is_expired());
}

#[test]
fn test_oauth_token_no_expiry() {
    let token = OAuthToken {
        access_token: "test".to_string(),
        refresh_token: None,
        expires_at: None,
    };
    assert!(!token.is_expired());
}

#[tokio::test]
async fn test_discover_claude_code_missing_file() {
    // With a non-existent home, discover should return None gracefully
    let result = discover_claude_code().await;
    // Result depends on whether ~/.claude/.credentials.json exists
    // Main thing: no panic
    let _ = result;
}

#[tokio::test]
async fn test_discover_codex_missing_file() {
    let result = discover_codex().await;
    let _ = result;
}

#[tokio::test]
async fn test_discover_all_default_config() {
    let config = CredentialDiscoveryConfig::default();
    let results = discover_all(&config).await;
    // Should not panic, may return empty or found credentials
    let _ = results;
}

#[tokio::test]
async fn test_discover_all_disabled() {
    let config = CredentialDiscoveryConfig {
        claude_code: Some(false),
        codex: Some(false),
        refresh_interval_secs: None,
        fetch_external_usage: None,
    };
    let results = discover_all(&config).await;
    assert!(results.is_empty());
}

#[test]
fn test_discovered_credential_info_serialization() {
    let info = DiscoveredCredentialInfo {
        source: CredentialSource::ClaudeCode,
        provider: "anthropic".to_string(),
        status: "active".to_string(),
        expires_at: Some(1700000000),
        auto_refresh: true,
    };
    let json = serde_json::to_string(&info).unwrap();
    assert!(json.contains("claude_code"));
    assert!(json.contains("anthropic"));

    let parsed: DiscoveredCredentialInfo = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.source, CredentialSource::ClaudeCode);
    assert_eq!(parsed.provider, "anthropic");
}

#[tokio::test]
async fn test_discover_claude_code_valid_json() {
    // Create a temp dir to test file reading
    let temp = std::env::temp_dir().join("openalpaca_test_cred");
    let claude_dir = temp.join(".claude");
    let _ = tokio::fs::create_dir_all(&claude_dir).await;
    let cred_path = claude_dir.join(".credentials.json");

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    let token_json = serde_json::json!({
        "accessToken": "test-token-123",
        "refreshToken": "refresh-456",
        "expiresAt": now + 3600
    });
    tokio::fs::write(&cred_path, token_json.to_string().as_bytes())
        .await
        .unwrap();

    // Can't directly test discover_claude_code() since it reads from $HOME,
    // but we can test the OAuthToken parsing
    let data = tokio::fs::read(&cred_path).await.unwrap();
    let token: OAuthToken = serde_json::from_slice(&data).unwrap();
    assert_eq!(token.access_token, "test-token-123");
    assert_eq!(token.refresh_token.as_deref(), Some("refresh-456"));
    assert!(!token.is_expired());

    // Cleanup
    let _ = tokio::fs::remove_dir_all(&temp).await;
}

#[test]
fn test_discover_claude_code_malformed_json() {
    let malformed = b"{ not valid json }";
    let result = serde_json::from_slice::<OAuthToken>(malformed);
    assert!(result.is_err());
}
