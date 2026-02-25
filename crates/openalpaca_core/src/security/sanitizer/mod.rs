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
    pub fn sanitize_user_input(
        input: &str,
        max_input_length: Option<usize>,
    ) -> Result<String, SecurityViolation> {
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
    ///
    /// `extra_shell_tools` lists additional tool names (e.g., command-backend
    /// tools loaded from TOML config) that should receive shell injection checks
    /// alongside the hardcoded `SHELL_TOOLS` list.
    pub fn sanitize_tool_args(
        tool_name: &str,
        arguments: &serde_json::Value,
        allowed_tools: &[String],
        extra_shell_tools: &[String],
    ) -> Result<(), SecurityViolation> {
        // Check tool is in allowed list
        if !allowed_tools.is_empty() && !allowed_tools.iter().any(|t| t == tool_name) {
            return Err(SecurityViolation::InputBlocked {
                reason: format!("Tool '{}' is not in the allowed tools list", tool_name),
            });
        }

        let is_shell_tool = Self::SHELL_TOOLS.contains(&tool_name)
            || extra_shell_tools.iter().any(|t| t == tool_name);

        // Recursively check string values in arguments
        Self::check_value_safety(arguments, is_shell_tool)?;

        Ok(())
    }

    fn check_value_safety(
        value: &serde_json::Value,
        check_injection: bool,
    ) -> Result<(), SecurityViolation> {
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
mod tests;
