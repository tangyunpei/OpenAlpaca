use regex::Regex;
use std::borrow::Cow;

/// Detect if the model hallucinated a send confirmation without calling the tool.
/// Returns true if `send` was available, no tools were called, and the
/// response text contains send-success patterns.
pub fn detect_hallucinated_send(
    tool_names: &[&str],
    tool_calls_made: usize,
    response: &str,
) -> bool {
    if tool_calls_made > 0 {
        return false;
    }
    if !tool_names.contains(&"send") {
        return false;
    }

    // Chinese patterns (message)
    const CN_PATTERNS: &[&str] = &["发送成功", "已发送", "已成功发送", "发送完成"];
    // English patterns (message, matched case-insensitively on ASCII)
    const EN_PATTERNS: &[&str] = &[
        "sent successfully",
        "message sent",
        "has been sent",
        "was sent",
        "been delivered",
    ];
    // File-specific patterns
    const FILE_EN_PATTERNS: &[&str] = &["file sent", "sent the file"];
    const FILE_CN_PATTERNS: &[&str] = &["文件已发送", "已发送文件", "文件发送成功"];

    let lower = response.to_ascii_lowercase();

    // ✅ emoji is a strong signal on its own when combined with send tool + 0 calls
    if response.contains('✅') {
        return true;
    }

    for pat in CN_PATTERNS {
        if response.contains(pat) {
            return true;
        }
    }

    for pat in EN_PATTERNS {
        if lower.contains(pat) {
            return true;
        }
    }

    // File-specific hallucination patterns (always checked since unified `send` handles both)
    for pat in FILE_CN_PATTERNS {
        if response.contains(pat) {
            return true;
        }
    }
    for pat in FILE_EN_PATTERNS {
        if lower.contains(pat) {
            return true;
        }
    }

    false
}

/// Middleware to ensure Agent Output meets strict format constraints.
pub struct OutputGuard;

impl OutputGuard {
    /// Attempts to extract valid JSON from a potentially messy string.
    /// E.g. extracts `{...}` from "Here is the JSON: ```json { "foo": "bar" } ```"
    pub fn ensure_json(content: &str) -> Result<String, String> {
        // 1. Fast check: is it already valid?
        if serde_json::from_str::<serde_json::Value>(content).is_ok() {
            return Ok(content.to_string());
        }

        // 2. Repair: Strip markdown code blocks
        let repaired = Self::strip_markdown_json(content);
        if serde_json::from_str::<serde_json::Value>(&repaired).is_ok() {
            return Ok(repaired.into_owned());
        }

        // 3. Last Resort: Regex search for first { ... } block (Simple recursive regex is hard in Rust's regex crate,
        // so we assume top-level object starts with { and ends with })
        // A naive trim scan:
        if let Some(start) = content.find('{')
            && let Some(end) = content.rfind('}')
            && end > start
        {
            let candidate = &content[start..=end];
            if serde_json::from_str::<serde_json::Value>(candidate).is_ok() {
                return Ok(candidate.to_string());
            }
        }

        Err("Failed to extract valid JSON from output.".to_string())
    }

    fn strip_markdown_json(input: &str) -> Cow<'_, str> {
        let pattern = Regex::new(r"```(?:json)?\s*([\s\S]*?)\s*```").unwrap();
        if let Some(caps) = pattern.captures(input) {
            // Return the capture group 1
            return Cow::Owned(caps[1].to_string());
        }
        Cow::Borrowed(input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ensure_json_valid() {
        let input = r#"{"key": "value"}"#;
        assert_eq!(OutputGuard::ensure_json(input).unwrap(), input);
    }

    #[test]
    fn test_ensure_json_markdown() {
        let input = "Here is the code:\n```json\n{\"key\": \"value\"}\n```";
        let expected = r#"{"key": "value"}"#;
        assert_eq!(OutputGuard::ensure_json(input).unwrap(), expected);
    }

    #[test]
    fn test_ensure_json_mixed_text() {
        let input = "Sure, { \"foo\": 123 } is the answer.";
        let expected = "{ \"foo\": 123 }";
        assert_eq!(OutputGuard::ensure_json(input).unwrap(), expected);
    }

    #[test]
    fn test_ensure_json_fail() {
        let input = "Just some text.";
        assert!(OutputGuard::ensure_json(input).is_err());
    }

    // --- detect_hallucinated_send tests ---

    #[test]
    fn hallucination_detected_chinese_success() {
        assert!(detect_hallucinated_send(
            &["send"],
            0,
            "✅ 发送成功！消息已通过Telegram发出。"
        ));
    }

    #[test]
    fn hallucination_detected_english_success() {
        assert!(detect_hallucinated_send(
            &["send"],
            0,
            "Message sent successfully via Telegram!"
        ));
    }

    #[test]
    fn no_hallucination_when_tool_called() {
        // tool_calls_made > 0 → not hallucination, even with success text
        assert!(!detect_hallucinated_send(
            &["send"],
            1,
            "✅ 发送成功！"
        ));
    }

    #[test]
    fn no_hallucination_without_send_tool() {
        // send not in tool list → not our concern
        assert!(!detect_hallucinated_send(
            &["web_fetch"],
            0,
            "✅ 发送成功！"
        ));
    }

    #[test]
    fn no_hallucination_normal_response() {
        // No success patterns → not hallucination
        assert!(!detect_hallucinated_send(
            &["send"],
            0,
            "好的，我可以帮你发送消息。请告诉我收件人。"
        ));
    }

    #[test]
    fn hallucination_detected_already_sent() {
        assert!(detect_hallucinated_send(
            &["send"],
            0,
            "消息已发送到你的Telegram联系人。"
        ));
    }

    // --- send_file hallucination guard tests ---

    #[test]
    fn hallucination_detected_send_file_english() {
        assert!(detect_hallucinated_send(
            &["send"],
            0,
            "File sent successfully via Telegram!"
        ));
    }

    #[test]
    fn hallucination_detected_send_file_chinese() {
        assert!(detect_hallucinated_send(
            &["send"],
            0,
            "文件已发送到你的Telegram联系人。"
        ));
    }

    #[test]
    fn no_hallucination_send_file_when_tool_called() {
        assert!(!detect_hallucinated_send(
            &["send"],
            1,
            "文件已发送到你的Telegram联系人。"
        ));
    }

    #[test]
    fn no_hallucination_send_file_normal_response() {
        assert!(!detect_hallucinated_send(
            &["send"],
            0,
            "请告诉我文件路径，我来帮你发送。"
        ));
    }
}
