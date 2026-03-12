/// Short social/acknowledgement phrases that never require planning or tool use.
pub const SOCIAL_PHRASES: &[&str] = &[
    "thanks", "thank you", "ok", "okay", "got it", "sounds good",
    "yes", "no", "sure", "right",
    "好的", "没问题", "谢谢", "嗯", "明白", "收到", "对", "是的", "不是", "不用",
];

/// Check if a message is a pure social/acknowledgement phrase.
pub fn is_social_phrase(content: &str) -> bool {
    let trimmed = content.trim().to_lowercase();
    SOCIAL_PHRASES.contains(&trimmed.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_social_phrases() {
        assert!(is_social_phrase("thanks"));
        assert!(is_social_phrase("  OK  "));
        assert!(is_social_phrase("好的"));
        assert!(!is_social_phrase("write me a function"));
        assert!(!is_social_phrase(""));
    }
}
