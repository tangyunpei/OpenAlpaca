use std::sync::LazyLock;

use regex::Regex;

static SENSITIVE_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        // OpenAI API keys
        Regex::new(r"sk-[A-Za-z0-9]{20,}").unwrap(),
        // Slack bot tokens
        Regex::new(r"xoxb-[A-Za-z0-9\-]+").unwrap(),
        // Slack app tokens
        Regex::new(r"xapp-[A-Za-z0-9\-]+").unwrap(),
        // Discord bot tokens (base64-ish)
        Regex::new(r"[A-Za-z0-9]{24}\.[A-Za-z0-9_-]{6}\.[A-Za-z0-9_-]{27,}").unwrap(),
        // Generic "Bearer" tokens in strings
        Regex::new(r"Bearer\s+[A-Za-z0-9._\-]+").unwrap(),
        // Telegram bot tokens (numeric:alphanumeric)
        Regex::new(r"\d{8,}:[A-Za-z0-9_-]{35}").unwrap(),
    ]
});

/// Redact sensitive patterns (API keys, tokens) from a string.
pub fn redact_sensitive(input: &str) -> String {
    let mut result = input.to_string();
    for pattern in SENSITIVE_PATTERNS.iter() {
        result = pattern.replace_all(&result, "[REDACTED]").to_string();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redact_openai_key() {
        let input = "key: sk-abcdefghijklmnopqrstuvwxyz123456";
        let redacted = redact_sensitive(input);
        assert!(!redacted.contains("sk-abcdef"));
        assert!(redacted.contains("[REDACTED]"));
    }

    #[test]
    fn test_redact_slack_token() {
        let input = "token: xoxb-123-456-abcdefghij";
        let redacted = redact_sensitive(input);
        assert!(!redacted.contains("xoxb-"));
        assert!(redacted.contains("[REDACTED]"));
    }

    #[test]
    fn test_redact_telegram_token() {
        let input = "bot: 123456789:ABCDefgh_ijKLMnopQRStuvwxyz12345678";
        let redacted = redact_sensitive(input);
        assert!(!redacted.contains("123456789:ABC"));
        assert!(redacted.contains("[REDACTED]"));
    }

    #[test]
    fn test_no_redaction_needed() {
        let input = "normal log message without secrets";
        assert_eq!(redact_sensitive(input), input);
    }

    #[test]
    fn test_redact_multiple() {
        let input = "key1: sk-aaaabbbbccccddddeeeeffffgggg key2: xoxb-111-222-333";
        let redacted = redact_sensitive(input);
        assert!(!redacted.contains("sk-"));
        assert!(!redacted.contains("xoxb-"));
        assert_eq!(redacted.matches("[REDACTED]").count(), 2);
    }
}
