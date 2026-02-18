//! Layer 2: Input sanitization.
//!
//! Validates and sanitizes user input and tool arguments to prevent
//! injection attacks and unsafe operations.

use super::capabilities::SecurityViolation;

/// Default maximum input length in bytes (32 KB).
const MAX_INPUT_LENGTH: usize = 32 * 1024;

/// Sanitizes user input and tool arguments.
pub struct InputSanitizer;

impl InputSanitizer {
    /// Sanitize user input before processing.
    ///
    /// Checks for:
    /// - Excessive length (> max_input_length, default 32KB)
    /// - Null bytes
    ///
    /// Path traversal and command injection checks are handled by
    /// `sanitize_tool_args`, which is context-aware (only shell tools
    /// are checked for injection patterns).
    ///
    /// Pass `None` for `max_input_length` to use the compiled default (32768 bytes).
    pub fn sanitize_user_input(input: &str, max_input_length: Option<usize>) -> Result<String, SecurityViolation> {
        let max_len = max_input_length.unwrap_or(MAX_INPUT_LENGTH);
        // Check length
        if input.len() > max_len {
            return Err(SecurityViolation::InputBlocked {
                reason: format!(
                    "Input exceeds maximum length ({} > {} bytes)",
                    input.len(),
                    max_len
                ),
            });
        }

        // Check null bytes
        if input.contains('\0') {
            return Err(SecurityViolation::InputBlocked {
                reason: "Input contains null bytes".to_string(),
            });
        }

        Ok(input.to_string())
    }

    /// Tools whose arguments should be checked for shell injection patterns.
    /// Non-shell tools (file_write, workspace_write, web_search, etc.) can
    /// legitimately contain semicolons, newlines, backticks, and pipes.
    const SHELL_TOOLS: &'static [&'static str] = &["shell_execute"];

    /// Sanitize tool call arguments.
    ///
    /// Checks:
    /// - Tool name is in the allowed list (if non-empty)
    /// - All string values are checked for path traversal and null bytes
    /// - Shell-related tool arguments are additionally checked for hidden
    ///   command injection patterns (backticks, `$(`, newlines). Normal shell
    ///   operators (pipes, redirections, chaining) are allowed because the
    ///   LLM agent constructs the full command intentionally. Non-shell tools
    ///   are NOT subject to these checks so that tools like `file_write` and
    ///   `workspace_write` can accept multi-line content.
    pub fn sanitize_tool_args(
        tool_name: &str,
        arguments: &serde_json::Value,
        allowed_tools: &[String],
    ) -> Result<(), SecurityViolation> {
        // Check tool is in allowed list
        if !allowed_tools.is_empty() && !allowed_tools.iter().any(|t| t == tool_name) {
            return Err(SecurityViolation::InputBlocked {
                reason: format!("Tool '{}' is not in the allowed tools list", tool_name),
            });
        }

        let is_shell_tool = Self::SHELL_TOOLS.contains(&tool_name);

        // Recursively check string values in arguments
        Self::check_value_safety(arguments, is_shell_tool)?;

        Ok(())
    }

    fn check_value_safety(value: &serde_json::Value, check_injection: bool) -> Result<(), SecurityViolation> {
        match value {
            serde_json::Value::String(s) => Self::check_string_safety(s, check_injection),
            serde_json::Value::Array(arr) => {
                for v in arr {
                    Self::check_value_safety(v, check_injection)?;
                }
                Ok(())
            }
            serde_json::Value::Object(obj) => {
                for v in obj.values() {
                    Self::check_value_safety(v, check_injection)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn check_string_safety(s: &str, check_injection: bool) -> Result<(), SecurityViolation> {
        // Path traversal — always checked for all tools
        if s.contains("../") || s.contains("..\\") {
            return Err(SecurityViolation::InputBlocked {
                reason: "Path traversal pattern detected".to_string(),
            });
        }

        // Null bytes — always checked for all tools
        if s.contains('\0') {
            return Err(SecurityViolation::InputBlocked {
                reason: "Argument contains null bytes".to_string(),
            });
        }

        // Command injection patterns — only checked for shell-related tools.
        //
        // We only block patterns that enable *hidden* command injection where
        // an attacker could sneak a second command into what the agent thinks
        // is a data string.  Normal shell features like pipes (`|`),
        // redirections (`>`, `<`), and chaining (`&&`, `||`) are intentionally
        // allowed because the LLM agent constructs the full command string and
        // these operators are fundamental to useful shell usage (e.g.
        // `grep foo file.txt | wc -l` or `cargo build > /dev/null 2>&1`).
        if check_injection {
            let injection_patterns = [
                ("`", "backtick command substitution"),
                ("$(", "command substitution ($()"),
                ("\n", "newline (command separator)"),
                ("\r", "carriage return"),
            ];

            for (pattern, desc) in &injection_patterns {
                if s.contains(pattern) {
                    return Err(SecurityViolation::InputBlocked {
                        reason: format!("Potential command injection: {}", desc),
                    });
                }
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
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
        let result = InputSanitizer::sanitize_tool_args("file_read", &args, &[]);
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
        let result = InputSanitizer::sanitize_tool_args("shell_execute", &args, &[]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_command_injection_backtick() {
        let args = serde_json::json!({"cmd": "echo `whoami`"});
        let result = InputSanitizer::sanitize_tool_args("shell_execute", &args, &[]);
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
        let result = InputSanitizer::sanitize_tool_args("shell_execute", &args, &[]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_shell_redirect_allowed() {
        // Output redirection is a normal shell feature — allowed
        let args = serde_json::json!({"command": "cargo build > /dev/null 2>&1"});
        let result = InputSanitizer::sanitize_tool_args("shell_execute", &args, &[]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_shell_chaining_allowed() {
        // && and || are normal shell control flow — allowed
        let args = serde_json::json!({"command": "cargo test && cargo build"});
        let result = InputSanitizer::sanitize_tool_args("shell_execute", &args, &[]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_non_shell_tool_allows_newlines() {
        // file_write content with newlines should NOT be blocked
        let args = serde_json::json!({"path": "test.txt", "content": "line1\nline2\nline3"});
        let result = InputSanitizer::sanitize_tool_args("file_write", &args, &[]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_non_shell_tool_allows_semicolons() {
        // web_search query with semicolons should NOT be blocked
        let args = serde_json::json!({"query": "node.js; tutorial"});
        let result = InputSanitizer::sanitize_tool_args("web_search", &args, &[]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_non_shell_tool_allows_backticks() {
        // file_write content with backticks (markdown) should NOT be blocked
        let args = serde_json::json!({"content": "```rust\nfn main() {}\n```"});
        let result = InputSanitizer::sanitize_tool_args("file_write", &args, &[]);
        assert!(result.is_ok());
    }

    #[test]
    fn test_non_shell_tool_still_blocks_traversal() {
        // Path traversal is still blocked for all tools
        let args = serde_json::json!({"path": "../../etc/passwd", "content": "safe"});
        let result = InputSanitizer::sanitize_tool_args("file_write", &args, &[]);
        assert!(result.is_err());
    }

    #[test]
    fn test_non_shell_tool_still_blocks_null_bytes() {
        let args = serde_json::json!({"content": "hello\0world"});
        let result = InputSanitizer::sanitize_tool_args("file_write", &args, &[]);
        assert!(result.is_err());
    }

    #[test]
    fn test_unknown_tool_blocked() {
        let args = serde_json::json!({});
        let allowed = vec!["web_search".to_string(), "summarize".to_string()];
        let result = InputSanitizer::sanitize_tool_args("shell_execute", &args, &allowed);
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
        let result = InputSanitizer::sanitize_tool_args("file_read", &args, &[]);
        assert!(result.is_err());
    }
}
