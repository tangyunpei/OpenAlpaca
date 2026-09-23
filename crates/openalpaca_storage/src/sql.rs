//! SQL text helpers shared by the crate's queries.

/// Escapes a user-supplied `LIKE` needle so `%`, `_` and the escape character
/// itself match literally. Paired with `ESCAPE '\'` on every pattern built from
/// it — without both halves a search for `100%` matches every row. Unrelated to
/// FTS5 query escaping.
pub(crate) fn escape_like(needle: &str) -> String {
    let mut out = String::with_capacity(needle.len());
    for ch in needle.chars() {
        if matches!(ch, '\\' | '%' | '_') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::escape_like;

    #[test]
    fn escape_like_prefixes_exactly_the_three_metacharacters() {
        assert_eq!(escape_like(""), "");
        assert_eq!(escape_like("plain é 字"), "plain é 字");
        assert_eq!(escape_like("100%_a\\b"), "100\\%\\_a\\\\b");
        assert_eq!(escape_like("%%"), "\\%\\%");
    }
}
