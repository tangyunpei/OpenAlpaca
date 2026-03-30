//! Tool suggestion heuristics: detect file/URL/shell/persona signals in user messages.

use regex::Regex;
use std::sync::OnceLock;

static REL_PATH_WITH_EXT_RE: OnceLock<Regex> = OnceLock::new();
static FILE_NAMED_RE: OnceLock<Regex> = OnceLock::new();

fn rel_path_regex() -> &'static Regex {
    REL_PATH_WITH_EXT_RE.get_or_init(|| {
        Regex::new(r"(?i)(?:^|[^A-Za-z0-9._/\-])((?:\./)?(?:[A-Za-z0-9._\-]+/)*[A-Za-z0-9._\-]+\.[A-Za-z]{2,10})(?:$|[^A-Za-z0-9._/\-])").unwrap()
    })
}

fn file_named_regex() -> &'static Regex {
    FILE_NAMED_RE.get_or_init(|| {
        Regex::new(r"(?i)\bfile\s+(?:named|called)\s+([A-Za-z0-9][A-Za-z0-9._/\-]{0,200})\b")
            .unwrap()
    })
}

#[derive(Default)]
struct ToolFlags {
    web_fetch: bool,
    web_search: bool,
    file_read: bool,
    file_write: bool,
    shell_execute: bool,
    update_persona: bool,
    send: bool,
}

impl ToolFlags {
    fn to_vec(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.web_fetch {
            out.push("web_fetch".to_string());
        }
        if self.web_search {
            out.push("web_search".to_string());
        }
        if self.file_read {
            out.push("file_read".to_string());
        }
        // update_persona suppresses file_write for persona document targets
        if self.update_persona {
            out.push("update_persona".to_string());
        } else if self.file_write {
            out.push("file_write".to_string());
        }
        if self.shell_execute {
            out.push("shell_execute".to_string());
        }
        if self.send {
            out.push("send".to_string());
        }
        out
    }
}

impl super::IntentParser {
    pub fn suggest_tools(&self, content: &str) -> Vec<String> {
        let lower = content.to_lowercase();

        let flags = ToolFlags {
            file_write: Self::has_write_verb(&lower) && Self::mentions_filename(content),

            update_persona: Self::has_soul_target(&lower)
                || Self::has_user_target(&lower)
                || Self::has_identity_target(&lower),

            web_fetch: content.contains("http://")
                || content.contains("https://")
                || lower.contains("fetch ")
                || lower.contains("download ")
                || lower.contains("open url"),

            web_search: lower.contains("search for")
                || lower.contains("look up")
                || lower.contains("find information"),

            file_read: lower.contains("read file")
                || lower.contains("open file")
                || lower.contains("show file")
                || lower.contains("cat "),

            shell_execute: lower.contains("run command")
                || lower.contains("execute")
                || lower.contains("in terminal")
                || lower.contains("in shell")
                || lower.contains("bash")
                || lower.contains("zsh"),

            send: lower.contains("send message")
                || lower.contains("send to")
                || lower.contains("发消息")
                || lower.contains("发到")
                || lower.contains("发给")
                || lower.contains("转发")
                || lower.contains("通过telegram")
                || lower.contains("通过imessage")
                || lower.contains("message to")
                || lower.contains("text to")
                || lower.contains("msg to")
                || lower.contains("reply via")
                || lower.contains("forward to")
                || lower.contains("send file")
                || lower.contains("send photo")
                || lower.contains("send image")
                || lower.contains("send document")
                || lower.contains("send a file")
                || lower.contains("send a photo")
                || lower.contains("send an image")
                || lower.contains("发文件")
                || lower.contains("发图片")
                || lower.contains("发照片")
                || lower.contains("发视频")
                || lower.contains("发附件")
                || lower.contains("发文档")
                || (lower.contains("send") && (lower.contains("imessage") || lower.contains("telegram")))
                || (lower.contains("发") && (lower.contains("imessage") || lower.contains("telegram")))
                || ((lower.contains("消息") || lower.contains("短信")) && (lower.contains("telegram") || lower.contains("imessage")))
                || (lower.contains("给") && (lower.contains("发") || lower.contains("消息") || lower.contains("短信"))
                    && (lower.contains("telegram") || lower.contains("imessage")))
                || (lower.contains("via ") && (lower.contains("telegram") || lower.contains("imessage")))
                || (lower.contains("file") && lower.contains("send") && (lower.contains("imessage") || lower.contains("telegram")))
                || (lower.contains("photo") && lower.contains("send") && (lower.contains("imessage") || lower.contains("telegram")))
                || (lower.contains("image") && lower.contains("send") && (lower.contains("imessage") || lower.contains("telegram"))),
        };

        flags.to_vec()
    }

    /// Detect if the user is targeting the SOUL / persona.
    pub(super) fn has_soul_target(lower: &str) -> bool {
        const SOUL_NOUNS: &[&str] = &["soul", "persona", "personality", "vibe", "soul.md"];
        const SOUL_VERBS: &[&str] = &["update", "change", "edit", "modify", "set", "rewrite"];

        if lower.contains("soul.md") {
            return true;
        }

        let has_noun = SOUL_NOUNS.iter().any(|n| lower.contains(n));
        let has_verb = SOUL_VERBS.iter().any(|v| lower.contains(v));
        has_noun && has_verb
    }

    /// Detect if the user is targeting the USER profile.
    pub(super) fn has_user_target(lower: &str) -> bool {
        const USER_NOUNS: &[&str] = &["user.md", "user profile", "my profile"];
        const USER_VERBS: &[&str] = &["update", "change", "edit", "modify", "set"];

        if lower.contains("user.md") {
            return true;
        }

        let has_noun = USER_NOUNS.iter().any(|n| lower.contains(n));
        let has_verb = USER_VERBS.iter().any(|v| lower.contains(v));
        has_noun && has_verb
    }

    /// Detect if the user is targeting the IDENTITY document.
    pub(super) fn has_identity_target(lower: &str) -> bool {
        const IDENTITY_NOUNS: &[&str] = &["identity", "identity.md"];
        const IDENTITY_VERBS: &[&str] = &["update", "change", "edit", "modify", "set"];

        if lower.contains("identity.md") {
            return true;
        }

        let has_noun = IDENTITY_NOUNS.iter().any(|n| lower.contains(n));
        let has_verb = IDENTITY_VERBS.iter().any(|v| lower.contains(v));
        has_noun && has_verb
    }

    pub(super) fn has_write_verb(lower: &str) -> bool {
        const WRITE_VERBS: &[&str] = &[
            "write",
            "save",
            "create",
            "update",
            "edit",
            "append",
            "overwrite",
        ];
        WRITE_VERBS.iter().any(|v| lower.contains(v))
    }

    pub(super) fn mentions_filename(content: &str) -> bool {
        rel_path_regex().is_match(content) || file_named_regex().is_match(content)
    }
}
