//! JSON extraction policies used by internal model calls.
use serde_json::Value;

/// The fenced body opened at `start`, where `fence` was found.
fn fenced_from<'a>(text: &'a str, start: usize, fence: &str) -> Option<&'a str> {
    let after = &text[start + fence.len()..];
    Some(&after[..after.find("```")?])
}

fn fenced_content<'a>(text: &'a str, fence: &str) -> Option<&'a str> {
    fenced_from(text, text.find(fence)?, fence)
}

/// Utility jobs select a JSON fence when present and retain the parse error.
/// An unterminated JSON fence must not fall back to an earlier plain fence.
pub(crate) fn parse_utility_json_response(content: &str) -> Result<Value, serde_json::Error> {
    let text = content.trim();
    serde_json::from_str(text).or_else(|_| {
        let fenced = match text.find("```json") {
            Some(start) => fenced_from(text, start, "```json"),
            None => fenced_content(text, "```"),
        };
        serde_json::from_str(fenced.unwrap_or(text).trim())
    })
}

/// Task extraction historically tries both fence kinds and discards parse errors.
pub fn parse_json_response(content: &str) -> Option<Value> {
    let text = content.trim();
    serde_json::from_str(text).ok().or_else(|| {
        ["```json", "```"]
            .into_iter()
            .filter_map(|fence| fenced_content(text, fence))
            .find_map(|candidate| serde_json::from_str(candidate.trim()).ok())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utility_and_task_parsers_preserve_different_fence_fallbacks() {
        for content in [
            "```\n{\"ok\": true}\n```\n```json\ninvalid\n```",
            "```\n{\"ok\": true}\n```\n```json\nunterminated",
        ] {
            assert!(parse_utility_json_response(content).is_err());
            assert_eq!(parse_json_response(content).unwrap()["ok"], true);
        }
        for content in [
            "null",
            "prefix ```json\nnull\n``` suffix",
            "prefix ```\nnull\n``` suffix",
        ] {
            assert_eq!(parse_utility_json_response(content).unwrap(), Value::Null);
            assert_eq!(parse_json_response(content), Some(Value::Null));
        }
        assert!(parse_utility_json_response("```json\n{broken}").is_err());
    }
}
