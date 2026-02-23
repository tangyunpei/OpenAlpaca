use super::*;

#[test]
fn test_flatten_messages() {
    let messages = vec![
        ChatMessage::system("You are helpful."),
        ChatMessage::user("Hello"),
        ChatMessage::assistant("Hi!"),
    ];
    let flattened = flatten_for_cli(&messages);
    assert!(flattened.contains("[System] You are helpful."));
    assert!(flattened.contains("Hello"));
    assert!(flattened.contains("[Assistant] Hi!"));
}

#[test]
fn test_parse_claude_json_output() {
    let stdout = r#"{"result": "Hello there!", "is_error": false}"#;
    let response = parse_claude_output(stdout).unwrap();
    assert_eq!(response.content, "Hello there!");
    assert_eq!(response.model, "claude_cli");
    assert_eq!(response.usage.input_tokens, 0);
    assert_eq!(response.usage.output_tokens, 0);
    assert_eq!(response.finish_reason, FinishReason::Stop);
}

#[test]
fn test_parse_claude_error_output() {
    let stdout = r#"{"result": "Rate limited", "is_error": true}"#;
    let result = parse_claude_output(stdout);
    assert!(result.is_err());
    match result.unwrap_err() {
        LlmError::CliBackend(msg) => assert_eq!(msg, "Rate limited"),
        other => panic!("Expected CliBackend error, got: {:?}", other),
    }
}

#[test]
fn test_parse_raw_stdout_fallback() {
    let stdout = "Just plain text output\nwith multiple lines";
    let response = parse_claude_output(stdout).unwrap();
    assert_eq!(
        response.content,
        "Just plain text output\nwith multiple lines"
    );
}

#[test]
fn test_parse_codex_json_output() {
    let stdout = r#"{"response": "The answer is 42"}"#;
    let response = parse_codex_output(stdout).unwrap();
    assert_eq!(response.content, "The answer is 42");
    assert_eq!(response.model, "codex_cli");
}

#[test]
fn test_parse_codex_raw_fallback() {
    let stdout = "raw codex output";
    let response = parse_codex_output(stdout).unwrap();
    assert_eq!(response.content, "raw codex output");
}

#[test]
fn test_supports_tools_false() {
    // Verify at the type level that CLI providers don't support tools
    let provider =
        ClaudeCodeCliProvider::new(PathBuf::from("/usr/bin/claude"), Duration::from_secs(120));
    assert!(!provider.supports_tools());

    let provider =
        CodexCliProvider::new(PathBuf::from("/usr/bin/codex"), Duration::from_secs(120));
    assert!(!provider.supports_tools());
}

#[test]
fn test_usage_is_zero() {
    let stdout = r#"{"result": "test", "is_error": false}"#;
    let response = parse_claude_output(stdout).unwrap();
    assert_eq!(response.usage.input_tokens, 0);
    assert_eq!(response.usage.output_tokens, 0);
}

#[test]
fn test_detect_cli_backends_default() {
    let config = CliBackendsConfig::default();
    let statuses = detect_cli_backends(&config);
    assert_eq!(statuses.len(), 2);
    assert_eq!(statuses[0].name, "claude_code");
    assert_eq!(statuses[1].name, "codex");
    // Both should default to enabled=true
    assert!(statuses[0].enabled);
    assert!(statuses[1].enabled);
}

#[test]
fn test_detect_cli_backends_disabled() {
    let config = CliBackendsConfig {
        claude_code: Some(CliBackendConfig {
            path: None,
            enabled: Some(false),
            timeout_secs: None,
        }),
        codex: Some(CliBackendConfig {
            path: None,
            enabled: Some(false),
            timeout_secs: None,
        }),
    };
    let statuses = detect_cli_backends(&config);
    assert!(!statuses[0].enabled);
    assert!(!statuses[1].enabled);
}
