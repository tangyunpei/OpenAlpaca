//! Byte-budget text cuts that cannot panic.
//!
//! A log preview or a refusal message wants a **byte** ceiling — a budget in
//! characters lets one CJK line grow to four times the bytes it was meant to
//! cost. `&s[..n]` is the obvious way to spend that budget and it panics the
//! moment `n` lands inside a multi-byte character, which user- and
//! model-authored text does constantly (a 3-byte CJK character, a 4-byte
//! emoji). [`byte_prefix`] keeps the byte budget and rounds the cut **down**
//! to the nearest character boundary.
//!
//! This is not [`crate::runner::agentic_loop::tool_helpers`]'s tool-result
//! truncation: that one is a different mechanism (sentence/line/word
//! boundaries, head-and-tail, a session-log spill) answering a different
//! question. This is the one-line cut for a preview.

/// Borrow at most `max_bytes` of `s`, ending on a character boundary.
///
/// The result is never longer than `max_bytes`; when the budget lands inside a
/// character, that whole character is dropped. A budget at or past the end of
/// the string returns the whole string, and a budget of 0 returns `""`.
pub(crate) fn byte_prefix(s: &str, max_bytes: usize) -> &str {
    &s[..s.floor_char_boundary(max_bytes)]
}

#[cfg(test)]
mod tests {
    use super::byte_prefix;

    /// Every one of these but the ASCII rows panicked under `&s[..n]`.
    #[test]
    fn byte_prefix_rounds_down_to_a_character_boundary() {
        // (input, budget, expected)
        let cases: &[(&str, usize, &str)] = &[
            // Empty string: nothing to cut, any budget.
            ("", 0, ""),
            ("", 80, ""),
            // Zero budget: nothing kept, even when the first char is wide.
            ("hello", 0, ""),
            ("日本語", 0, ""),
            // Budget at or past the end: the whole string.
            ("hello", 5, "hello"),
            ("hello", 500, "hello"),
            ("日本語", 9, "日本語"),
            ("日本語", 500, "日本語"),
            // ASCII under budget: an exact byte cut, unchanged behaviour.
            ("hello world", 5, "hello"),
            // 2-byte characters: budget 2 is exactly on the boundary, budget
            // 1 and 3 land inside one and drop it.
            ("üü", 1, ""),
            ("üü", 2, "ü"),
            ("üü", 3, "ü"),
            ("üü", 4, "üü"),
            // Mixed widths: "ünïcödé" is ü(2) n(1) ï(2) …, so byte 3 IS a
            // boundary and byte 4 is not.
            ("ünïcödé", 3, "ün"),
            ("ünïcödé", 4, "ün"),
            ("ünïcödé", 5, "ünï"),
            // 3-byte CJK: budget 4 and 5 both land inside the second char.
            ("日本語", 3, "日"),
            ("日本語", 4, "日"),
            ("日本語", 5, "日"),
            ("日本語", 6, "日本"),
            // 4-byte emoji: only a multiple of 4 keeps a whole one.
            ("🙂🙂", 4, "🙂"),
            ("🙂🙂", 5, "🙂"),
            ("🙂🙂", 7, "🙂"),
            ("🙂🙂", 8, "🙂🙂"),
            // Mixed: the budget must not cut the emoji in half.
            ("ok🙂", 3, "ok"),
            ("ok🙂", 6, "ok🙂"),
        ];

        for (input, budget, expected) in cases {
            assert_eq!(
                byte_prefix(input, *budget),
                *expected,
                "byte_prefix({input:?}, {budget})"
            );
            assert!(
                byte_prefix(input, *budget).len() <= *budget,
                "byte_prefix({input:?}, {budget}) overspent its byte budget"
            );
        }
    }
}
