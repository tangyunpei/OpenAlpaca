pub(crate) mod json;
pub(crate) mod markdown;
pub mod social;
/// A UTF-8 prefix bounded by bytes, without allocating or adding a suffix.
pub(crate) fn prefix_by_bytes(text: &str, limit: usize) -> &str {
    &text[..text.floor_char_boundary(limit.min(text.len()))]
}

#[cfg(test)]
mod tests {
    use super::prefix_by_bytes;

    #[test]
    fn byte_prefix_keeps_utf8_and_never_exceeds_the_budget() {
        for text in ["", "ascii", "中", "a中🙂z"] {
            for limit in 0..=text.len() + 2 {
                let prefix = prefix_by_bytes(text, limit);
                assert!(text.starts_with(prefix));
                assert!(prefix.len() <= limit);
                assert!(
                    text[prefix.len()..]
                        .chars()
                        .next()
                        .is_none_or(|next| prefix.len() + next.len_utf8() > limit)
                );
            }
        }
    }
}
