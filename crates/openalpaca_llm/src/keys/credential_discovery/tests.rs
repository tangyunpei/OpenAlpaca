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

/// A temporary home with no keychain fallback: discovery here never reads the
/// developer's own credential files or keychain item.
fn host_at(home: &std::path::Path) -> DiscoveryHost {
    DiscoveryHost {
        home: Some(home.to_path_buf()),
        keychain: false,
    }
}

async fn plant(home: &std::path::Path, rel: &str, access_token: &str) {
    let path = home.join(rel);
    tokio::fs::create_dir_all(path.parent().unwrap())
        .await
        .unwrap();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let json = serde_json::json!({ "accessToken": access_token, "expiresAt": now + 3600 });
    tokio::fs::write(&path, json.to_string()).await.unwrap();
}

#[tokio::test]
async fn test_discover_claude_code_missing_file() {
    let home = tempfile::tempdir().unwrap();
    assert!(
        discover_claude_code_on(&host_at(home.path()))
            .await
            .is_none()
    );
}

#[tokio::test]
async fn test_discover_codex_missing_file() {
    let home = tempfile::tempdir().unwrap();
    assert!(discover_codex_on(&host_at(home.path())).await.is_none());
}

#[tokio::test]
async fn test_discover_all_default_config() {
    let config = CredentialDiscoveryConfig::default();
    let home = tempfile::tempdir().unwrap();
    assert!(
        discover_all_on(&config, &host_at(home.path()))
            .await
            .is_empty()
    );

    plant(home.path(), ".claude/.credentials.json", "claude-token").await;
    plant(home.path(), ".codex/auth.json", "codex-token").await;
    let found = discover_all_on(&config, &host_at(home.path())).await;
    let sources: Vec<_> = found.iter().map(|c| c.source).collect();
    assert_eq!(
        sources,
        vec![CredentialSource::ClaudeCode, CredentialSource::Codex]
    );
    assert_eq!(found[0].token.access_token, "claude-token");
    assert_eq!(found[1].token.access_token, "codex-token");
}

/// 121-2: with no usable home (the daemon's `paths::user_home` gives none
/// for an unset-and-unknown, empty or relative one) discovery looks for no
/// credential file, even with both sources on, and no keychain here.
#[tokio::test]
async fn with_no_home_discovery_finds_nothing() {
    let host = DiscoveryHost {
        home: None,
        keychain: false,
    };
    let config = CredentialDiscoveryConfig::default();
    assert!(discover_all_on(&config, &host).await.is_empty());
    assert!(discover_claude_code_on(&host).await.is_none());
    assert!(discover_codex_on(&host).await.is_none());
}

#[tokio::test]
async fn test_discover_all_disabled() {
    let config = CredentialDiscoveryConfig {
        claude_code: Some(false),
        codex: Some(false),
        refresh_interval_secs: None,
        fetch_external_usage: None,
    };
    let results = discover_all(&config, None).await;
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
    let home = tempfile::tempdir().unwrap();
    let cred_path = home.path().join(".claude").join(".credentials.json");
    tokio::fs::create_dir_all(cred_path.parent().unwrap())
        .await
        .unwrap();

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

    let found = discover_claude_code_on(&host_at(home.path()))
        .await
        .expect("the planted credentials are discovered");
    assert_eq!(found.source, CredentialSource::ClaudeCode);
    assert_eq!(found.token.access_token, "test-token-123");
    assert_eq!(found.token.refresh_token.as_deref(), Some("refresh-456"));
    assert!(!found.token.is_expired());
}

#[test]
fn test_discover_claude_code_malformed_json() {
    let malformed = b"{ not valid json }";
    let result = serde_json::from_slice::<OAuthToken>(malformed);
    assert!(result.is_err());
}

/// The machine's host, now given its home by the caller (121-2: the daemon
/// passes `openalpaca_app_support::paths::user_home`), still refuses to be
/// built in this crate's unit tests, whatever home it is handed: no test
/// reaches the developer's credential files or keychain item.
#[test]
#[should_panic(expected = "credential discovery reached the host in a test")]
fn the_machines_host_still_panics_in_a_unit_test() {
    let home = tempfile::tempdir().unwrap();
    let _ = DiscoveryHost::system(Some(home.path().to_path_buf()));
}
