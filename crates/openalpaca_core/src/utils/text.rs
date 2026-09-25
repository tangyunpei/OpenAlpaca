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
//!
//! [`one_line`] is what a person reads before acting on it — an approval
//! card, a list to pick an id from: one line with nothing hidden in it.

/// Borrow at most `max_bytes` of `s`, ending on a character boundary.
///
/// The result is never longer than `max_bytes`; when the budget lands inside a
/// character, that whole character is dropped. A budget at or past the end of
/// the string returns the whole string, and a budget of 0 returns `""`.
pub(crate) fn byte_prefix(s: &str, max_bytes: usize) -> &str {
    &s[..s.floor_char_boundary(max_bytes)]
}

/// `text` on one line for a person to read and act on: runs of whitespace —
/// newlines and the line and paragraph separators (U+2028, U+2029) among
/// them — as one space, and no [hidden](is_hidden) character. Hidden
/// characters are dropped, not replaced with a visible mark: what is left is
/// the text as stored, in its own order, and a mark would litter the
/// legitimate uses (an emoji's zero-width joiner, a soft hyphen) on the very
/// line that must read clearly.
pub(crate) fn one_line(text: &str) -> String {
    text.split_whitespace()
        .map(|word| word.replace(is_hidden, ""))
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// A character that changes what a reader sees without being seen itself:
/// a control character (Cc) — at a terminal an ESC sequence can rewrite the
/// line — or a Unicode *format* character (Cf): the bidi embeddings,
/// overrides and isolates (U+202A–U+202E, U+2066–U+2069) that reorder the
/// text around them, the zero-width characters (U+200B–U+200F,
/// U+2060–U+2064, U+FEFF), the soft hyphen and the tag characters.
/// Unicode 16.0's `Cf`, held equal to the `regex` crate's own `\p{Cf}` by
/// a test.
pub(crate) fn is_hidden(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{ad}'
                | '\u{600}'..='\u{605}'
                | '\u{61c}'
                | '\u{6dd}'
                | '\u{70f}'
                | '\u{890}'..='\u{891}'
                | '\u{8e2}'
                | '\u{180e}'
                | '\u{200b}'..='\u{200f}'
                | '\u{202a}'..='\u{202e}'
                | '\u{2060}'..='\u{2064}'
                | '\u{2066}'..='\u{206f}'
                | '\u{feff}'
                | '\u{fff9}'..='\u{fffb}'
                | '\u{110bd}'
                | '\u{110cd}'
                | '\u{13430}'..='\u{1343f}'
                | '\u{1bca0}'..='\u{1bca3}'
                | '\u{1d173}'..='\u{1d17a}'
                | '\u{e0001}'
                | '\u{e0020}'..='\u{e007f}'
        )
}

#[cfg(test)]
mod tests {
    use super::{byte_prefix, is_hidden, one_line};

    /// The hand-written table is Unicode's: every scalar value is hidden
    /// exactly when it is `Cc` or `Cf` by the `regex` crate's tables, so a
    /// dependency upgrade that brings a newer Unicode fails here rather than
    /// letting a new format character through.
    #[test]
    fn is_hidden_is_exactly_cc_and_cf() {
        let unicode = regex::Regex::new(r"^[\p{Cc}\p{Cf}]$").unwrap();
        let mut buf = [0u8; 4];
        for c in (0..=0x10ffff_u32).filter_map(char::from_u32) {
            assert_eq!(
                is_hidden(c),
                unicode.is_match(c.encode_utf8(&mut buf)),
                "U+{:04X}",
                c as u32
            );
        }
    }

    #[test]
    fn one_line_flattens_whitespace_and_drops_hidden_characters() {
        let cases: &[(&str, &str)] = &[
            ("", ""),
            ("plain text", "plain text"),
            ("  two\n\tlines \r\n", "two lines"),
            // Line and paragraph separators are whitespace.
            ("one\u{2028}two\u{2029}three", "one two three"),
            // Controls: ESC sequences lose their ESC, BEL and NUL go.
            ("a\u{1b}[2Jb\u{7}\u{0}", "a[2Jb"),
            // A bidi override is dropped, not obeyed: the stored order shows.
            ("Invoice \u{202e}gpj.exe\u{202c}", "Invoice gpj.exe"),
            // Isolates and embeddings.
            ("\u{2066}x\u{2069} \u{202a}y\u{202c}", "x y"),
            // Zero-width characters, the BOM and the soft hyphen.
            ("Edi\u{200b}tor\u{ad}\u{feff}", "Editor"),
            // A word made only of hidden characters leaves no double space.
            ("a \u{200b}\u{200d} b", "a b"),
            // What is visible stays, emoji and CJK included.
            ("日本語 🙂", "日本語 🙂"),
        ];
        for (input, expected) in cases {
            assert_eq!(one_line(input), *expected, "one_line({input:?})");
        }
    }

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
