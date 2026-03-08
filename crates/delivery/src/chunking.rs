/// Split text into chunks respecting a character (codepoint) limit.
///
/// The limit is measured in Unicode codepoints, not bytes, matching how
/// messaging platforms (Telegram, Discord, Slack) count characters.
///
/// Prefers splitting at paragraph breaks (`\n\n`), then line breaks (`\n`),
/// then spaces. Falls back to hard splits at a char boundary if no break
/// point is found.
pub fn chunk_text(text: &str, limit: usize) -> Vec<String> {
    if limit == 0 {
        return vec![];
    }
    if char_count(text) <= limit {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut remaining = text;

    while !remaining.is_empty() {
        if char_count(remaining) <= limit {
            chunks.push(remaining.to_string());
            break;
        }

        let split_at = find_split_point(remaining, limit);
        let (chunk, rest) = remaining.split_at(split_at);
        chunks.push(chunk.to_string());
        remaining = rest.trim_start_matches('\n');
    }

    chunks
}

/// Count Unicode codepoints in a string.
fn char_count(s: &str) -> usize {
    s.chars().count()
}

/// Get the byte index of the nth codepoint (or string length if n >= char count).
fn byte_index_at_char(text: &str, n: usize) -> usize {
    text.char_indices()
        .nth(n)
        .map(|(i, _)| i)
        .unwrap_or(text.len())
}

fn find_split_point(text: &str, limit: usize) -> usize {
    let byte_limit = byte_index_at_char(text, limit);
    let search_range = &text[..byte_limit];

    // Prefer paragraph break
    if let Some(pos) = search_range.rfind("\n\n") {
        return pos + 1; // include one newline
    }

    // Then line break
    if let Some(pos) = search_range.rfind('\n') {
        return pos + 1;
    }

    // Then space
    if let Some(pos) = search_range.rfind(' ') {
        return pos + 1;
    }

    // Hard split at char boundary
    byte_limit
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_short_text_single_chunk() {
        let chunks = chunk_text("hello world", 100);
        assert_eq!(chunks, vec!["hello world"]);
    }

    #[test]
    fn test_empty_text() {
        let chunks = chunk_text("", 100);
        assert_eq!(chunks, vec![""]);
    }

    #[test]
    fn test_zero_limit() {
        let chunks = chunk_text("hello", 0);
        assert!(chunks.is_empty());
    }

    #[test]
    fn test_split_at_paragraph() {
        let text = "paragraph one\n\nparagraph two\n\nparagraph three";
        // With limit 20, the first paragraph break at pos 14 is the split point
        let chunks = chunk_text(text, 20);
        assert!(chunks.len() >= 2);
        assert_eq!(chunks[0], "paragraph one\n");
        assert!(chunks[1].starts_with("paragraph two"));
    }

    #[test]
    fn test_split_at_newline() {
        let text = "line one\nline two\nline three";
        let chunks = chunk_text(text, 15);
        assert!(chunks.len() >= 2);
        assert_eq!(chunks[0], "line one\n");
    }

    #[test]
    fn test_split_at_space() {
        let text = "word1 word2 word3 word4";
        let chunks = chunk_text(text, 12);
        assert!(chunks.len() >= 2);
        assert!(chunks[0].len() <= 12);
    }

    #[test]
    fn test_hard_split_no_break() {
        let text = "abcdefghijklmnop";
        let chunks = chunk_text(text, 5);
        assert_eq!(chunks, vec!["abcde", "fghij", "klmno", "p"]);
    }

    #[test]
    fn test_telegram_limit() {
        let text = "a".repeat(8000);
        let chunks = chunk_text(&text, 4000);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].len(), 4000);
        assert_eq!(chunks[1].len(), 4000);
    }

    #[test]
    fn test_multibyte_emoji_no_panic() {
        // "abc🎉def" = 7 codepoints, 10 bytes (emoji is 4 bytes)
        let text = "abc🎉def";
        assert_eq!(text.len(), 10);
        assert_eq!(text.chars().count(), 7);

        // limit=5 codepoints should split after the emoji
        let chunks = chunk_text(text, 5);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0], "abc🎉d");
        assert_eq!(chunks[1], "ef");
    }

    #[test]
    fn test_cjk_characters() {
        // Each CJK char is 3 bytes. 6 chars = 18 bytes.
        let text = "你好世界测试";
        assert_eq!(text.chars().count(), 6);

        let chunks = chunk_text(text, 4);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0], "你好世界");
        assert_eq!(chunks[1], "测试");
    }

    #[test]
    fn test_multibyte_split_at_space() {
        let text = "hello 🌍🌎 world";
        let chunks = chunk_text(text, 8);
        assert!(chunks.len() >= 2);
        // Should split at space after "hello", not panic mid-emoji
        assert_eq!(chunks[0], "hello ");
    }

    #[test]
    fn test_short_multibyte_single_chunk() {
        let text = "🎉🎊🎈";
        assert_eq!(text.chars().count(), 3);
        let chunks = chunk_text(text, 10);
        assert_eq!(chunks, vec!["🎉🎊🎈"]);
    }
}
