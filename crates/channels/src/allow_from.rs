/// Check if a sender ID is in the allowlist.
///
/// - Wildcard `"*"` matches any sender.
/// - Empty allowlist: returns `allow_when_empty`.
/// - `None` sender_id: returns false (unless wildcard present).
/// - Matching is case-insensitive.
pub fn is_sender_allowed(
    allow_from: &[String],
    sender_id: Option<&str>,
    allow_when_empty: bool,
) -> bool {
    if allow_from.is_empty() {
        return allow_when_empty;
    }

    // Check for wildcard
    if allow_from.iter().any(|entry| entry == "*") {
        return true;
    }

    let sender = match sender_id {
        Some(s) if !s.is_empty() => s,
        _ => return false,
    };

    let sender_lower = sender.to_lowercase();
    allow_from
        .iter()
        .any(|entry| entry.to_lowercase() == sender_lower)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_allowlist_uses_default() {
        assert!(is_sender_allowed(&[], Some("user1"), true));
        assert!(!is_sender_allowed(&[], Some("user1"), false));
    }

    #[test]
    fn test_wildcard_allows_any() {
        let list = vec!["*".to_string()];
        assert!(is_sender_allowed(&list, Some("anyone"), false));
        assert!(is_sender_allowed(&list, None, false));
    }

    #[test]
    fn test_exact_match() {
        let list = vec!["user1".to_string(), "user2".to_string()];
        assert!(is_sender_allowed(&list, Some("user1"), false));
        assert!(is_sender_allowed(&list, Some("user2"), false));
        assert!(!is_sender_allowed(&list, Some("user3"), false));
    }

    #[test]
    fn test_case_insensitive() {
        let list = vec!["User1".to_string()];
        assert!(is_sender_allowed(&list, Some("user1"), false));
        assert!(is_sender_allowed(&list, Some("USER1"), false));
    }

    #[test]
    fn test_none_sender_rejected() {
        let list = vec!["user1".to_string()];
        assert!(!is_sender_allowed(&list, None, false));
    }

    #[test]
    fn test_empty_sender_rejected() {
        let list = vec!["user1".to_string()];
        assert!(!is_sender_allowed(&list, Some(""), false));
    }
}
