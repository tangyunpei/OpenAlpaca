//! iMessage sender via AppleScript
//!
//! Sends iMessage replies by executing AppleScript through `osascript`.
//! Supports both direct (1-to-1) and group chat messages.

use std::time::Duration;
use tokio::process::Command;

/// Sends iMessage replies using `osascript` (AppleScript).
pub struct IMessageSender;

impl IMessageSender {
    /// Send a message to the given recipient or group chat.
    ///
    /// - For direct messages (`is_group = false`), the `recipient` is treated
    ///   as a phone number or email and the message is sent via the iMessage
    ///   service account.
    /// - For group chats (`is_group = true`), the `recipient` is treated as
    ///   the chat identifier and the message is sent to that chat directly.
    ///
    /// If `from_account` is provided (e.g. `"user@icloud.com"`), the sender
    /// selects the matching iMessage account instead of blindly picking the
    /// first one. This prevents replies from landing in the wrong thread when
    /// the Mac has multiple iMessage addresses (phone + email).
    pub async fn send(recipient: &str, message: &str, is_group: bool) -> Result<(), String> {
        Self::send_from(recipient, message, is_group, None).await
    }

    /// Like [`send`] but accepts an explicit `from_account` to choose the
    /// outgoing iMessage service account.
    pub async fn send_from(
        recipient: &str,
        message: &str,
        is_group: bool,
        from_account: Option<&str>,
    ) -> Result<(), String> {
        let escaped_message = escape_applescript(message);
        let escaped_recipient = escape_applescript(recipient);

        let script = if is_group {
            format!(
                r#"tell application "Messages"
    set targetChat to chat id "{}"
    send "{}" to targetChat
end tell"#,
                escaped_recipient, escaped_message,
            )
        } else if let Some(acct) = from_account {
            // Use the explicit account to avoid routing into the wrong thread
            // on Macs with multiple iMessage addresses (phone + email).
            let escaped_account = escape_applescript(acct);
            format!(
                r#"tell application "Messages"
    set targetService to 1st account whose id = "{}"
    set targetBuddy to participant "{}" of targetService
    send "{}" to targetBuddy
end tell"#,
                escaped_account, escaped_recipient, escaped_message,
            )
        } else {
            // Fallback: pick the first iMessage account.
            format!(
                r#"tell application "Messages"
    set targetService to 1st account whose service type = iMessage
    set targetBuddy to participant "{}" of targetService
    send "{}" to targetBuddy
end tell"#,
                escaped_recipient, escaped_message,
            )
        };

        let output = tokio::time::timeout(
            Duration::from_secs(15),
            Command::new("osascript")
                .arg("-e")
                .arg(&script)
                .kill_on_drop(true)
                .output(),
        )
        .await
        .map_err(|_| "osascript timed out after 15 seconds".to_string())?
        .map_err(|e| format!("Failed to execute osascript: {}", e))?;

        if output.status.success() {
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(format!("osascript failed: {}", stderr))
        }
    }
}

/// Escape a string for safe embedding inside AppleScript double-quoted literals.
///
/// Backslashes are escaped first, then double quotes, then newlines and
/// carriage returns are neutralised to prevent breaking out of the string
/// context and executing arbitrary AppleScript code.
fn escape_applescript(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_applescript_plain() {
        assert_eq!(escape_applescript("hello world"), "hello world");
    }

    #[test]
    fn test_escape_applescript_quotes() {
        assert_eq!(escape_applescript(r#"say "hi""#), r#"say \"hi\""#);
    }

    #[test]
    fn test_escape_applescript_backslash() {
        assert_eq!(escape_applescript(r"path\to\file"), r"path\\to\\file");
    }

    #[test]
    fn test_escape_applescript_mixed() {
        assert_eq!(escape_applescript(r#"a\"b"#), r#"a\\\"b"#,);
    }

    #[test]
    fn test_escape_applescript_newlines() {
        assert_eq!(escape_applescript("line1\nline2"), "line1\\nline2");
        assert_eq!(escape_applescript("line1\rline2"), "line1\\rline2");
        assert_eq!(escape_applescript("line1\r\nline2"), "line1\\r\\nline2");
    }

    #[test]
    fn test_escape_applescript_injection_attempt() {
        // A crafted message that tries to break out of the AppleScript string
        let malicious = "hello\"\ndo shell script \"rm -rf ~/\"";
        let escaped = escape_applescript(malicious);
        assert!(!escaped.contains('\n'));
        assert!(!escaped.contains('\r'));
        assert_eq!(escaped, "hello\\\"\\ndo shell script \\\"rm -rf ~/\\\"");
    }
}
