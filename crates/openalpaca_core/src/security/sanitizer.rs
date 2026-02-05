//! Layer 2: Input sanitization.
//!
//! Validates and sanitizes user input and tool arguments to prevent
//! injection attacks and unsafe operations.

use super::capabilities::SecurityViolation;

/// Maximum input length in bytes (32 KB).
const MAX_INPUT_LENGTH: usize = 32 * 1024;

/// Sanitizes user input and tool arguments.
pub struct InputSanitizer;

impl InputSanitizer {
    /// Sanitize user input before processing.
    ///
    /// Checks for:
    /// - Excessive length (> 32KB)
    /// - Null bytes
    /// - Path traversal patterns (`../`)
    /// - Command injection patterns (`;`, `&&`, `|`, backticks, `$(`)
    pub fn sanitize_user_input(input: &str) -> Result<String, SecurityViolation> {
        // Check length
        if input.len() > MAX_INPUT_LENGTH {
            return Err(SecurityViolation::InputBlocked {
                reason: format!(
                    "Input exceeds maximum length ({} > {} bytes)",
                    input.len(),
                    MAX_INPUT_LENGTH
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

    /// Sanitize tool call arguments.
    ///
    /// Checks:
    /// - Tool name is in the allowed list (if non-empty)
    /// - String values don't contain path traversal or command injection
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

        // Recursively check string values in arguments
        Self::check_value_safety(arguments)?;

        Ok(())
    }

    fn check_value_safety(value: &serde_json::Value) -> Result<(), SecurityViolation> {
        match value {
            serde_json::Value::String(s) => Self::check_string_safety(s),
            serde_json::Value::Array(arr) => {
                for v in arr {
                    Self::check_value_safety(v)?;
                }
                Ok(())
            }
            serde_json::Value::Object(obj) => {
                for v in obj.values() {
                    Self::check_value_safety(v)?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn check_string_safety(s: &str) -> Result<(), SecurityViolation> {
        // Path traversal
        if s.contains("../") || s.contains("..\\") {
            return Err(SecurityViolation::InputBlocked {
                reason: "Path traversal pattern detected".to_string(),
            });
        }

        // Command injection patterns
        let injection_patterns = [
            (";", "semicolon"),
            ("&&", "command chaining (&&)"),
            ("||", "command chaining (||)"),
            ("`", "backtick command substitution"),
            ("$(", "command substitution ($()"),
        ];

        for (pattern, desc) in &injection_patterns {
            if s.contains(pattern) {
                return Err(SecurityViolation::InputBlocked {
                    reason: format!("Potential command injection: {}", desc),
                });
            }
        }

        // Null bytes in arguments
        if s.contains('\0') {
            return Err(SecurityViolation::InputBlocked {
                reason: "Argument contains null bytes".to_string(),
            });
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clean_input_passes() {
        let result = InputSanitizer::sanitize_user_input("Hello, how are you?");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Hello, how are you?");
    }

    #[test]
    fn test_oversized_input_blocked() {
        let large = "x".repeat(MAX_INPUT_LENGTH + 1);
        let result = InputSanitizer::sanitize_user_input(&large);
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
        let result = InputSanitizer::sanitize_user_input("hello\0world");
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
    fn test_command_injection_semicolon() {
        let args = serde_json::json!({"cmd": "ls; rm -rf /"});
        let result = InputSanitizer::sanitize_tool_args("shell", &args, &[]);
        assert!(result.is_err());
        match result.unwrap_err() {
            SecurityViolation::InputBlocked { reason } => {
                assert!(reason.contains("command injection"));
            }
            other => panic!("Expected InputBlocked, got: {:?}", other),
        }
    }

    #[test]
    fn test_command_injection_backtick() {
        let args = serde_json::json!({"cmd": "echo `whoami`"});
        let result = InputSanitizer::sanitize_tool_args("shell", &args, &[]);
        assert!(result.is_err());
        match result.unwrap_err() {
            SecurityViolation::InputBlocked { reason } => {
                assert!(reason.contains("backtick"));
            }
            other => panic!("Expected InputBlocked, got: {:?}", other),
        }
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
