//! Intent classification for user messages.
//!
//! Keyword-based heuristics (LLM integration planned for Phase 5.1).

use crate::orchestrator::skill_catalog::SkillCatalog;
use crate::orchestrator::skill_router::SkillRouter;
use regex::Regex;
use std::sync::OnceLock;

/// Classified intent from a user message.
#[derive(Debug, Clone, PartialEq)]
pub enum Intent {
    /// A simple query / chat message (echo stub for now).
    SimpleQuery { query: String },
    /// A request for task status (optionally for a specific task).
    TaskQuery { task_id: Option<String> },
    /// A complex task requiring agent dispatch.
    ComplexTask {
        description: String,
        required_skills: Vec<String>,
    },
    /// A task control command (cancel, pause, resume).
    TaskControl { task_id: String, action: String },
    /// User wants to remember something (store in profile or memory).
    RememberCommand { content: String },
    /// User wants to forget something (remove from profile or memory).
    ForgetCommand { content: String },
    /// A skill was invoked (via slash command or trigger pattern match).
    SkillInvocation { skill_name: String, query: String },
}

impl Intent {
    pub fn intent_type(&self) -> &'static str {
        match self {
            Intent::SimpleQuery { .. } => "simple_query",
            Intent::TaskQuery { .. } => "task_query",
            Intent::ComplexTask { .. } => "complex_task",
            Intent::TaskControl { .. } => "task_control",
            Intent::RememberCommand { .. } => "remember_command",
            Intent::ForgetCommand { .. } => "forget_command",
            Intent::SkillInvocation { .. } => "skill_invocation",
        }
    }
}

/// Keyword-to-skill mapping.
const SKILL_KEYWORDS: &[(&[&str], &str)] = &[
    (
        &["research", "search", "find information", "look up"],
        "web_search",
    ),
    (&["summarize", "summary"], "summarize"),
    (
        &[
            "write",
            "draft",
            "compose",
            "create document",
            "write a report",
        ],
        "text_generate",
    ),
    (
        &["organize files", "file cleanup", "sort files"],
        "file_organize",
    ),
    (&["read file", "open file"], "file_read"),
    (&["run command", "execute", "shell"], "shell_execute"),
    (&["browse", "fetch page", "open url"], "browser"),
];

/// Complexity signal words that promote a single-skill match to ComplexTask.
const COMPLEXITY_SIGNALS: &[&str] = &["please", "can you", "could you", "help me", "i need"];

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
    update_soul: bool,
    update_user: bool,
    send_message: bool,
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
        // Precedence: update_soul/update_user suppress file_write for their targets
        if self.update_soul {
            out.push("update_soul".to_string());
        } else if self.update_user {
            out.push("update_user".to_string());
        } else if self.file_write {
            out.push("file_write".to_string());
        }
        if self.shell_execute {
            out.push("shell_execute".to_string());
        }
        if self.send_message {
            out.push("send_message".to_string());
        }
        out
    }
}

/// Parses user messages into intents using keyword heuristics.
pub struct IntentParser;

impl IntentParser {
    /// Parse a user message into an Intent.
    pub fn parse(&self, content: &str) -> Intent {
        let trimmed = content.trim();
        let lower = trimmed.to_lowercase();

        // 1. Slash commands: /cancel, /pause, /resume
        if let Some(intent) = Self::parse_task_control(&lower) {
            return intent;
        }

        // 2. Slash commands: /status, /tasks
        if let Some(intent) = Self::parse_task_query(&lower) {
            return intent;
        }

        // 3. Natural language task query
        if Self::is_natural_task_query(&lower) {
            return Intent::TaskQuery { task_id: None };
        }

        // 3.5. Remember / Forget commands
        if let Some(content) = Self::parse_remember_command(&lower, trimmed) {
            return Intent::RememberCommand { content };
        }
        if let Some(content) = Self::parse_forget_command(&lower, trimmed) {
            return Intent::ForgetCommand { content };
        }

        // 4. Skill matching
        let matched_skills = Self::extract_skills(&lower);
        if matched_skills.len() > 1 {
            return Intent::ComplexTask {
                description: trimmed.to_string(),
                required_skills: matched_skills,
            };
        }
        if matched_skills.len() == 1 && Self::has_complexity_signal(&lower) {
            return Intent::ComplexTask {
                description: trimmed.to_string(),
                required_skills: matched_skills,
            };
        }

        // 5. Default: SimpleQuery
        Intent::SimpleQuery {
            query: trimmed.to_string(),
        }
    }

    fn parse_task_control(lower: &str) -> Option<Intent> {
        let (action, rest) = if let Some(r) = lower.strip_prefix("/cancel ") {
            ("cancel", r)
        } else if let Some(r) = lower.strip_prefix("/pause ") {
            ("pause", r)
        } else if let Some(r) = lower.strip_prefix("/resume ") {
            ("resume", r)
        } else {
            return None;
        };
        let task_id = rest.trim().to_string();
        if !task_id.is_empty() {
            Some(Intent::TaskControl {
                task_id,
                action: action.to_string(),
            })
        } else {
            None
        }
    }

    fn parse_task_query(lower: &str) -> Option<Intent> {
        if lower == "/status" || lower == "/tasks" {
            return Some(Intent::TaskQuery { task_id: None });
        }
        if let Some(rest) = lower.strip_prefix("/status ") {
            let id = rest.trim().to_string();
            if !id.is_empty() {
                return Some(Intent::TaskQuery { task_id: Some(id) });
            }
        }
        None
    }

    fn is_natural_task_query(lower: &str) -> bool {
        let patterns = [
            "task status",
            "task progress",
            "task result",
            "what are my tasks",
            "what is the task result",
            "how is the task",
            "show tasks",
            "list tasks",
            "任务状态",
            "任务进度",
            "任务结果",
            "我的任务",
            "任务怎么样",
            "任务完成了吗",
            "任务做完了吗",
        ];
        patterns.iter().any(|p| lower.contains(p))
    }

    fn extract_skills(lower: &str) -> Vec<String> {
        let mut skills = Vec::new();
        for (keywords, skill) in SKILL_KEYWORDS {
            for kw in *keywords {
                if lower.contains(kw) {
                    skills.push(skill.to_string());
                    break;
                }
            }
        }
        skills
    }

    fn has_complexity_signal(lower: &str) -> bool {
        COMPLEXITY_SIGNALS.iter().any(|s| lower.contains(s))
    }

    /// Detect "remember X" style commands.
    fn parse_remember_command(lower: &str, original: &str) -> Option<String> {
        // "remember that ...", "remember my ...", "remember I ..."
        if let Some(rest) = lower.strip_prefix("remember ")
            && !rest.trim().is_empty()
        {
            // Safe: "remember " is 9 ASCII bytes, original is already trimmed
            return Some(original["remember ".len()..].trim().to_string());
        }
        // "please remember ..."
        if let Some(idx) = lower.find("please remember ") {
            let start = idx + "please remember ".len();
            // Guard against UTF-8 boundary mismatch (lower indices may differ from original
            // for non-ASCII text due to to_lowercase() byte-length changes)
            if start <= original.len() && original.is_char_boundary(start) {
                let content = original[start..].trim();
                if !content.is_empty() {
                    return Some(content.to_string());
                }
            }
        }
        None
    }

    /// Detect "forget X" style commands.
    fn parse_forget_command(lower: &str, original: &str) -> Option<String> {
        if let Some(rest) = lower.strip_prefix("forget ")
            && !rest.trim().is_empty()
        {
            // Safe: "forget " is 7 ASCII bytes, original is already trimmed
            return Some(original["forget ".len()..].trim().to_string());
        }
        if let Some(idx) = lower.find("please forget ") {
            let start = idx + "please forget ".len();
            // Guard against UTF-8 boundary mismatch (see parse_remember_command)
            if start <= original.len() && original.is_char_boundary(start) {
                let content = original[start..].trim();
                if !content.is_empty() {
                    return Some(content.to_string());
                }
            }
        }
        None
    }

    /// Parse a user message into an Intent, checking the SkillCatalog first.
    ///
    /// Priority:
    /// 1. Slash-command skill invocation: `/review some code`
    /// 2. Trigger pattern matching against catalog
    /// 3. Fall through to standard `parse()` logic
    pub fn parse_with_skills(&self, content: &str, catalog: &SkillCatalog) -> Intent {
        let trimmed = content.trim();
        let lower = trimmed.to_lowercase();

        // 1. Slash-command skill invocation: "/review some code"
        if let Some(without_slash) = trimmed.strip_prefix('/') {
            let parts: Vec<&str> = without_slash.splitn(2, ' ').collect();
            let command = parts[0];
            let query = parts.get(1).map(|s| s.trim()).unwrap_or("");

            if let Some(entry) = catalog.get_by_command(command) {
                return Intent::SkillInvocation {
                    skill_name: entry.frontmatter.name.clone(),
                    query: if query.is_empty() {
                        trimmed.to_string()
                    } else {
                        query.to_string()
                    },
                };
            }
            // Fall through if no skill matches the slash command
        }

        // 2. Trigger pattern matching (only for non-slash-command inputs)
        if !trimmed.starts_with('/') {
            let matched = catalog.match_triggers(&lower);
            if !matched.is_empty() {
                // Use the first (most specific) match
                // Look up the actual display name from the catalog
                let skill_name = if let Some(entry) = catalog.get(&matched[0]) {
                    entry.frontmatter.name.clone()
                } else {
                    matched[0].clone()
                };
                return Intent::SkillInvocation {
                    skill_name,
                    query: trimmed.to_string(),
                };
            }
        }

        // 3. Fall through to existing parse() logic
        self.parse(content)
    }

    /// Parse a user message into an Intent, using the weighted SkillRouter
    /// for scoring-based skill selection instead of regex trigger matching.
    ///
    /// Priority:
    /// 1. Slash-command skill invocation: `/review some code`
    /// 2. Weighted router scoring (auto-select if score >= threshold and mode == "auto")
    /// 3. Fall through to standard `parse()` logic
    pub fn parse_with_skills_and_router(
        &self,
        content: &str,
        catalog: &SkillCatalog,
        router: &SkillRouter,
    ) -> Intent {
        let trimmed = content.trim();

        // 1. Slash-command skill invocation (same as parse_with_skills)
        if let Some(without_slash) = trimmed.strip_prefix('/') {
            let parts: Vec<&str> = without_slash.splitn(2, ' ').collect();
            let command = parts[0];
            let query = parts.get(1).map(|s| s.trim()).unwrap_or("");

            if let Some(entry) = catalog.get_by_command(command) {
                return Intent::SkillInvocation {
                    skill_name: entry.frontmatter.name.clone(),
                    query: if query.is_empty() {
                        trimmed.to_string()
                    } else {
                        query.to_string()
                    },
                };
            }
            // Fall through if no skill matches the slash command
        }

        // 2. Weighted router scoring (replaces trigger pattern matching)
        if !trimmed.starts_with('/') {
            let route_result = router.route(trimmed, catalog);

            if let Some(ref skill_id) = route_result.selected {
                let skill_name = catalog
                    .get(skill_id)
                    .map(|e| e.frontmatter.name.clone())
                    .unwrap_or_else(|| skill_id.clone());

                router.record_usage(skill_id);

                return Intent::SkillInvocation {
                    skill_name,
                    query: trimmed.to_string(),
                };
            }

            // Log suggestions for observability but don't auto-select
            if !route_result.suggestions.is_empty() {
                tracing::debug!(
                    "SkillRouter: {} suggestion(s) for query (top: {} score={:.2})",
                    route_result.suggestions.len(),
                    route_result.suggestions[0].skill_name,
                    route_result.suggestions[0].score,
                );
            }
        }

        // 3. Fall through to existing parse() logic
        self.parse(content)
    }

    pub fn suggest_tools(&self, content: &str) -> Vec<String> {
        let lower = content.to_lowercase();

        let flags = ToolFlags {
            file_write: Self::has_write_verb(&lower) && Self::mentions_filename(content),

            update_soul: Self::has_soul_target(&lower),

            update_user: Self::has_user_target(&lower),

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

            send_message: lower.contains("send message")
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
                || (lower.contains("send") && (lower.contains("imessage") || lower.contains("telegram")))
                || (lower.contains("发") && (lower.contains("imessage") || lower.contains("telegram")))
                || ((lower.contains("消息") || lower.contains("短信")) && (lower.contains("telegram") || lower.contains("imessage")))
                || (lower.contains("给") && (lower.contains("发") || lower.contains("消息") || lower.contains("短信"))
                    && (lower.contains("telegram") || lower.contains("imessage")))
                || (lower.contains("via ") && (lower.contains("telegram") || lower.contains("imessage"))),
        };

        flags.to_vec()
    }

    /// Detect if the user is targeting the SOUL / persona.
    fn has_soul_target(lower: &str) -> bool {
        const SOUL_NOUNS: &[&str] = &["soul", "persona", "personality", "vibe", "soul.md"];
        const SOUL_VERBS: &[&str] = &["update", "change", "edit", "modify", "set", "rewrite"];

        // Direct file mention
        if lower.contains("soul.md") {
            return true;
        }

        // Noun + verb combination
        let has_noun = SOUL_NOUNS.iter().any(|n| lower.contains(n));
        let has_verb = SOUL_VERBS.iter().any(|v| lower.contains(v));
        has_noun && has_verb
    }

    /// Detect if the user is targeting the USER profile.
    fn has_user_target(lower: &str) -> bool {
        const USER_NOUNS: &[&str] = &["user.md", "user profile", "my profile"];
        const USER_VERBS: &[&str] = &["update", "change", "edit", "modify", "set"];

        // Direct file mention
        if lower.contains("user.md") {
            return true;
        }

        // Noun + verb combination
        let has_noun = USER_NOUNS.iter().any(|n| lower.contains(n));
        let has_verb = USER_VERBS.iter().any(|v| lower.contains(v));
        has_noun && has_verb
    }

    fn has_write_verb(lower: &str) -> bool {
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

    fn mentions_filename(content: &str) -> bool {
        rel_path_regex().is_match(content) || file_named_regex().is_match(content)
    }

    /// Check if a message is eligible for the fast path (skip LLM planner).
    ///
    /// Returns true when the message is short, has no complexity or delegation
    /// signals, and doesn't contain task management verbs — meaning it's very
    /// likely a simple conversational query that doesn't need planning.
    pub fn is_fast_path_eligible(&self, content: &str) -> bool {
        const TASK_VERBS: &[&str] = &[
            "create a task",
            "build a plan",
            "step by step",
            "first ",
            " then ",
            "and then",
            "followed by",
            "multiple steps",
        ];
        const DELEGATION_SIGNALS: &[&str] = &["assign to", "delegate", "use agent", "spawn agent"];

        // 1. Short content only
        if content.len() > 200 {
            return false;
        }

        let lower = content.to_lowercase();

        // 2. No complexity signals
        if Self::has_complexity_signal(&lower) {
            return false;
        }

        // 3. At most one skill keyword
        if Self::extract_skills(&lower).len() > 1 {
            return false;
        }

        // 4. No task management verbs
        if TASK_VERBS.iter().any(|v| lower.contains(v)) {
            return false;
        }

        // 5. No delegation language
        if DELEGATION_SIGNALS.iter().any(|s| lower.contains(s)) {
            return false;
        }

        true
    }
}

#[cfg(test)]
mod tests;
