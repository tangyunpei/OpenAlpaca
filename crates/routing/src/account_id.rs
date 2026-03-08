use std::sync::LazyLock;

use regex::Regex;

use openalpaca_core::types::{AccountId, DEFAULT_ACCOUNT_ID};

static VALID_ID: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z0-9][a-z0-9_-]{0,63}$").unwrap());

/// Normalize an account ID: lowercase, trim whitespace, validate format.
/// Returns `DEFAULT_ACCOUNT_ID` for `None` or empty/whitespace-only input.
/// Invalid characters cause fallback to DEFAULT_ACCOUNT_ID.
pub fn normalize_account_id(value: Option<&str>) -> AccountId {
    let raw = match value {
        Some(s) if !s.trim().is_empty() => s.trim().to_lowercase(),
        _ => return AccountId(DEFAULT_ACCOUNT_ID.to_string()),
    };

    if VALID_ID.is_match(&raw) {
        AccountId(raw)
    } else {
        // Strip invalid chars, replace with hyphen, then validate
        let cleaned: String = raw
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect();

        // Trim leading/trailing hyphens/underscores and truncate to 64 chars
        let cleaned = cleaned.trim_matches(['-', '_']);
        let cleaned = if cleaned.len() > 64 {
            &cleaned[..64]
        } else {
            cleaned
        };

        if cleaned.is_empty() || !VALID_ID.is_match(cleaned) {
            AccountId(DEFAULT_ACCOUNT_ID.to_string())
        } else {
            AccountId(cleaned.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_none_returns_default() {
        assert_eq!(normalize_account_id(None).0, DEFAULT_ACCOUNT_ID);
    }

    #[test]
    fn test_empty_returns_default() {
        assert_eq!(normalize_account_id(Some("")).0, DEFAULT_ACCOUNT_ID);
    }

    #[test]
    fn test_whitespace_returns_default() {
        assert_eq!(normalize_account_id(Some("   ")).0, DEFAULT_ACCOUNT_ID);
    }

    #[test]
    fn test_valid_id_preserved() {
        assert_eq!(normalize_account_id(Some("my-account")).0, "my-account");
    }

    #[test]
    fn test_uppercase_normalized() {
        assert_eq!(normalize_account_id(Some("FOO-Bar")).0, "foo-bar");
    }

    #[test]
    fn test_invalid_chars_cleaned() {
        assert_eq!(normalize_account_id(Some("foo@bar!")).0, "foo-bar");
    }

    #[test]
    fn test_leading_special_chars_stripped() {
        assert_eq!(normalize_account_id(Some("--test")).0, "test");
    }

    #[test]
    fn test_all_invalid_returns_default() {
        assert_eq!(normalize_account_id(Some("@@@")).0, DEFAULT_ACCOUNT_ID);
    }

    #[test]
    fn test_trims_whitespace() {
        assert_eq!(normalize_account_id(Some("  myaccount  ")).0, "myaccount");
    }
}
