use super::*;

#[test]
fn test_clean_input_passes() {
    let result = InputSanitizer::sanitize_user_input("Hello, how are you?", None);
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "Hello, how are you?");
}

#[test]
fn test_oversized_input_blocked() {
    let large = "x".repeat(MAX_INPUT_LENGTH + 1);
    let result = InputSanitizer::sanitize_user_input(&large, None);
    assert!(result.is_err());
    match result.unwrap_err() {
        SecurityViolation::InputBlocked { reason } => {
            assert!(reason.contains("maximum length"));
        }
        other => panic!("Expected InputBlocked, got: {:?}", other),
    }
}

#[test]
fn test_null_byte_blocked() {
    let result = InputSanitizer::sanitize_user_input("hello\0world", None);
    assert!(result.is_err());
    match result.unwrap_err() {
        SecurityViolation::InputBlocked { reason } => {
            assert!(reason.contains("null bytes"));
        }
        other => panic!("Expected InputBlocked, got: {:?}", other),
    }
}

#[test]
fn test_path_traversal_in_args() {
    let args = serde_json::json!({"path": "../../etc/passwd"});
    let result = InputSanitizer::sanitize_tool_args("file_read", &args, &[], &[]);
    assert!(result.is_err());
    match result.unwrap_err() {
        SecurityViolation::InputBlocked { reason } => {
            assert!(reason.contains("Path traversal"));
        }
        other => panic!("Expected InputBlocked, got: {:?}", other),
    }
}

#[test]
fn test_shell_semicolon_allowed() {
    // Semicolons are normal shell separators; not blocked
    let args = serde_json::json!({"cmd": "cd /tmp; ls"});
    let result = InputSanitizer::sanitize_tool_args("shell_execute", &args, &[], &[]);
    assert!(result.is_ok());
}

#[test]
fn test_command_injection_backtick() {
    let args = serde_json::json!({"cmd": "echo `whoami`"});
    let result = InputSanitizer::sanitize_tool_args("shell_execute", &args, &[], &[]);
    assert!(result.is_err());
    match result.unwrap_err() {
        SecurityViolation::InputBlocked { reason } => {
            assert!(reason.contains("backtick"));
        }
        other => panic!("Expected InputBlocked, got: {:?}", other),
    }
}

#[test]
fn test_shell_pipe_allowed() {
    // Pipes are a fundamental shell feature — allowed
    let args = serde_json::json!({"command": "grep TODO src/*.rs | wc -l"});
    let result = InputSanitizer::sanitize_tool_args("shell_execute", &args, &[], &[]);
    assert!(result.is_ok());
}

#[test]
fn test_shell_redirect_allowed() {
    // Output redirection is a normal shell feature — allowed
    let args = serde_json::json!({"command": "cargo build > /dev/null 2>&1"});
    let result = InputSanitizer::sanitize_tool_args("shell_execute", &args, &[], &[]);
    assert!(result.is_ok());
}

#[test]
fn test_shell_chaining_allowed() {
    // && and || are normal shell control flow — allowed
    let args = serde_json::json!({"command": "cargo test && cargo build"});
    let result = InputSanitizer::sanitize_tool_args("shell_execute", &args, &[], &[]);
    assert!(result.is_ok());
}

#[test]
fn test_non_shell_tool_allows_newlines() {
    // file_write content with newlines should NOT be blocked
    let args = serde_json::json!({"path": "test.txt", "content": "line1\nline2\nline3"});
    let result = InputSanitizer::sanitize_tool_args("file_write", &args, &[], &[]);
    assert!(result.is_ok());
}

#[test]
fn test_non_shell_tool_allows_semicolons() {
    // web_search query with semicolons should NOT be blocked
    let args = serde_json::json!({"query": "node.js; tutorial"});
    let result = InputSanitizer::sanitize_tool_args("web_search", &args, &[], &[]);
    assert!(result.is_ok());
}

#[test]
fn test_non_shell_tool_allows_backticks() {
    // file_write content with backticks (markdown) should NOT be blocked
    let args = serde_json::json!({"content": "```rust\nfn main() {}\n```"});
    let result = InputSanitizer::sanitize_tool_args("file_write", &args, &[], &[]);
    assert!(result.is_ok());
}

#[test]
fn test_non_shell_tool_still_blocks_traversal() {
    // Path traversal is still blocked for all tools
    let args = serde_json::json!({"path": "../../etc/passwd", "content": "safe"});
    let result = InputSanitizer::sanitize_tool_args("file_write", &args, &[], &[]);
    assert!(result.is_err());
}

#[test]
fn test_non_shell_tool_still_blocks_null_bytes() {
    let args = serde_json::json!({"content": "hello\0world"});
    let result = InputSanitizer::sanitize_tool_args("file_write", &args, &[], &[]);
    assert!(result.is_err());
}

#[test]
fn test_unknown_tool_blocked() {
    let args = serde_json::json!({});
    let allowed = vec!["web_search".to_string(), "summarize".to_string()];
    let result = InputSanitizer::sanitize_tool_args("shell_execute", &args, &allowed, &[]);
    assert!(result.is_err());
    match result.unwrap_err() {
        SecurityViolation::InputBlocked { reason } => {
            assert!(reason.contains("not in the allowed tools list"));
        }
        other => panic!("Expected InputBlocked, got: {:?}", other),
    }
}

#[test]
fn test_nested_traversal_in_array() {
    let args = serde_json::json!({"files": ["safe.txt", "../../../secret"]});
    let result = InputSanitizer::sanitize_tool_args("file_read", &args, &[], &[]);
    assert!(result.is_err());
}

#[test]
fn test_command_backend_tool_injection_blocked_via_extra_shell_tools() {
    // A command-backend tool listed in extra_shell_tools should have
    // injection patterns blocked, just like shell_execute.
    let args = serde_json::json!({"query": "test `whoami`"});
    let extra = vec!["git_log".to_string()];
    let result = InputSanitizer::sanitize_tool_args("git_log", &args, &[], &extra);
    assert!(result.is_err());
    match result.unwrap_err() {
        SecurityViolation::InputBlocked { reason } => {
            assert!(reason.contains("backtick"));
        }
        other => panic!("Expected InputBlocked, got: {:?}", other),
    }
}

#[test]
fn test_command_backend_tool_blocks_command_substitution() {
    let args = serde_json::json!({"count": "5$(rm -rf /)"});
    let extra = vec!["git_log".to_string()];
    let result = InputSanitizer::sanitize_tool_args("git_log", &args, &[], &extra);
    assert!(result.is_err());
    match result.unwrap_err() {
        SecurityViolation::InputBlocked { reason } => {
            assert!(reason.contains("command substitution"));
        }
        other => panic!("Expected InputBlocked, got: {:?}", other),
    }
}

#[test]
fn test_command_backend_tool_allows_safe_args() {
    // Safe arguments should pass for command-backend tools
    let args = serde_json::json!({"count": "10"});
    let extra = vec!["git_log".to_string()];
    let result = InputSanitizer::sanitize_tool_args("git_log", &args, &[], &extra);
    assert!(result.is_ok());
}

#[test]
fn test_non_extra_shell_tool_not_affected() {
    // A tool NOT in extra_shell_tools should still allow backticks
    let args = serde_json::json!({"content": "use `backticks` in markdown"});
    let extra = vec!["git_log".to_string()];
    let result = InputSanitizer::sanitize_tool_args("file_write", &args, &[], &extra);
    assert!(result.is_ok());
}
