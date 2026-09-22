use openalpaca_storage::repository::PreferenceRepository;

/// Whether this owner has a usable default route for a currently sendable channel.
/// Preserve each channel's existing parsing rules and best-effort DB reads.
pub(super) fn has_default_recipient(
    pref_repo: &PreferenceRepository<'_>,
    owner: &str,
    channel: &str,
) -> bool {
    match channel {
        "telegram" => pref_repo
            .get(owner, "telegram.last_chat_id")
            .ok()
            .flatten()
            .and_then(|p| p.value.parse::<i64>().ok())
            .is_some(),
        "imessage" => {
            pref_repo
                .get(owner, "imessage.last_reply_target")
                .ok()
                .flatten()
                .is_some()
                || pref_repo
                    .get(owner, "imessage.last_chat_id")
                    .ok()
                    .flatten()
                    .is_some()
        }
        "discord" => pref_repo
            .get(owner, "discord.last_channel_id")
            .ok()
            .flatten()
            .and_then(|p| p.value.parse::<u64>().ok())
            .is_some(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_channel_specific_and_owner_scoped() {
        let tmp = tempfile::tempdir().unwrap();
        let db = openalpaca_storage::Database::open(&tmp.path().join("preferences.db")).unwrap();
        let prefs = PreferenceRepository::new(&db);
        for channel in ["telegram", "imessage", "discord", "unknown"] {
            assert!(!has_default_recipient(&prefs, "owner", channel));
        }
        for (channel, key, value, expected) in [
            ("telegram", "telegram.last_chat_id", "-123", true),
            ("telegram", "telegram.last_chat_id", "bad", false),
            ("discord", "discord.last_channel_id", "123", true),
            ("discord", "discord.last_channel_id", "-123", false),
            ("discord", "discord.last_channel_id", "bad", false),
            ("imessage", "imessage.last_reply_target", "", true),
        ] {
            prefs.set("owner", key, value, None).unwrap();
            assert_eq!(
                has_default_recipient(&prefs, "owner", channel),
                expected,
                "{channel}: {value}"
            );
            assert!(!has_default_recipient(&prefs, "other", channel));
        }
        prefs.delete("owner", "imessage.last_reply_target").unwrap();
        prefs
            .set("owner", "imessage.last_chat_id", "chat-1", None)
            .unwrap();
        assert!(has_default_recipient(&prefs, "owner", "imessage"));
    }
}
