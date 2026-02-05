use regex::Regex;
use std::borrow::Cow;

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
}
