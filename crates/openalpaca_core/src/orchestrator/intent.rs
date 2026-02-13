//! Intent classification for user messages.
//!
//! Keyword-based heuristics (LLM integration planned for Phase 5.1).

use crate::orchestrator::skill_catalog::SkillCatalog;
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
        for action in &["cancel", "pause", "resume"] {
            let prefix = format!("/{} ", action);
            if lower.starts_with(&prefix) {
                let task_id = lower[prefix.len()..].trim().to_string();
                if !task_id.is_empty() {
                    return Some(Intent::TaskControl {
                        task_id,
                        action: action.to_string(),
                    });
                }
            }
        }
        None
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
            "what are my tasks",
            "show tasks",
            "list tasks",
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
        if let Some(rest) = lower.strip_prefix("remember ") {
            let content = rest.trim();
            if !content.is_empty() {
                // Use the original casing for the content
                let start = original.len() - original.trim_start().len() + "remember ".len();
                return Some(original[start..].trim().to_string());
            }
        }
        // "please remember ..."
        if let Some(idx) = lower.find("please remember ") {
            let start = idx + "please remember ".len();
            let content = original[start..].trim();
            if !content.is_empty() {
                return Some(content.to_string());
            }
        }
        None
    }

    /// Detect "forget X" style commands.
    fn parse_forget_command(lower: &str, original: &str) -> Option<String> {
        if let Some(rest) = lower.strip_prefix("forget ") {
            let content = rest.trim();
            if !content.is_empty() {
                let start = original.len() - original.trim_start().len() + "forget ".len();
                return Some(original[start..].trim().to_string());
            }
        }
        if let Some(idx) = lower.find("please forget ") {
            let start = idx + "please forget ".len();
            let content = original[start..].trim();
            if !content.is_empty() {
                return Some(content.to_string());
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
        if trimmed.starts_with('/') {
            let without_slash = &trimmed[1..];
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parser() -> IntentParser {
        IntentParser
    }

    #[test]
    fn test_simple_query() {
        let intent = parser().parse("hello world");
        assert_eq!(
            intent,
            Intent::SimpleQuery {
                query: "hello world".to_string()
            }
        );
    }

    #[test]
    fn test_slash_status() {
        let intent = parser().parse("/status");
        assert_eq!(intent, Intent::TaskQuery { task_id: None });
    }

    #[test]
    fn test_status_with_id() {
        let intent = parser().parse("/status task-123");
        assert_eq!(
            intent,
            Intent::TaskQuery {
                task_id: Some("task-123".to_string())
            }
        );
    }

    #[test]
    fn test_cancel() {
        let intent = parser().parse("/cancel task-456");
        assert_eq!(
            intent,
            Intent::TaskControl {
                task_id: "task-456".to_string(),
                action: "cancel".to_string()
            }
        );
    }

    #[test]
    fn test_pause() {
        let intent = parser().parse("/pause task-789");
        assert_eq!(
            intent,
            Intent::TaskControl {
                task_id: "task-789".to_string(),
                action: "pause".to_string()
            }
        );
    }

    #[test]
    fn test_complex_multi_skill() {
        let intent = parser().parse("research about Rust and write a summary");
        match intent {
            Intent::ComplexTask {
                required_skills, ..
            } => {
                assert!(required_skills.contains(&"web_search".to_string()));
                assert!(required_skills.contains(&"text_generate".to_string()));
                assert!(required_skills.contains(&"summarize".to_string()));
            }
            _ => panic!("Expected ComplexTask, got {:?}", intent),
        }
    }

    #[test]
    fn test_natural_language_task_query() {
        let intent = parser().parse("what are my tasks?");
        assert_eq!(intent, Intent::TaskQuery { task_id: None });
    }

    #[test]
    fn test_single_keyword_no_signal() {
        // "search" alone without complexity signal -> SimpleQuery
        let intent = parser().parse("search for cats");
        // "search" matches web_search but no complexity signal -> SimpleQuery
        assert!(matches!(intent, Intent::SimpleQuery { .. }));
    }

    #[test]
    fn test_single_keyword_with_signal() {
        let intent = parser().parse("can you search for cats");
        match intent {
            Intent::ComplexTask {
                required_skills, ..
            } => {
                assert!(required_skills.contains(&"web_search".to_string()));
            }
            _ => panic!("Expected ComplexTask, got {:?}", intent),
        }
    }

    // --- suggest_tools tests ---

    #[test]
    fn test_suggest_tools_write_readme() {
        let tools = parser().suggest_tools("write README.md with installation instructions");
        assert!(
            tools.contains(&"file_write".to_string()),
            "Expected file_write, got: {:?}",
            tools
        );
    }

    #[test]
    fn test_suggest_tools_write_file_named() {
        let tools = parser().suggest_tools("write a file named README with docs");
        assert!(
            tools.contains(&"file_write".to_string()),
            "Expected file_write, got: {:?}",
            tools
        );
    }

    #[test]
    fn test_suggest_tools_write_story_no_file() {
        let tools = parser().suggest_tools("write me a story about files");
        assert!(
            !tools.contains(&"file_write".to_string()),
            "Should NOT have file_write: {:?}",
            tools
        );
    }

    #[test]
    fn test_suggest_tools_version_no_file_write() {
        let tools = parser().suggest_tools("support v1.x series");
        assert!(
            !tools.contains(&"file_write".to_string()),
            "Should NOT have file_write: {:?}",
            tools
        );
    }

    #[test]
    fn test_suggest_tools_update_version_no_file_write() {
        let tools = parser().suggest_tools("update to v1.2 and ship it");
        assert!(
            !tools.contains(&"file_write".to_string()),
            "Should NOT have file_write: {:?}",
            tools
        );
    }

    #[test]
    fn test_suggest_tools_fetch_url() {
        let tools = parser().suggest_tools("fetch https://example.com");
        assert!(
            tools.contains(&"web_fetch".to_string()),
            "Expected web_fetch, got: {:?}",
            tools
        );
    }

    #[test]
    fn test_suggest_tools_hello_world_empty() {
        let tools = parser().suggest_tools("hello world");
        assert!(tools.is_empty(), "Expected empty, got: {:?}", tools);
    }

    #[test]
    fn test_suggest_tools_run_command() {
        let tools = parser().suggest_tools("run command ls -la");
        assert!(
            tools.contains(&"shell_execute".to_string()),
            "Expected shell_execute, got: {:?}",
            tools
        );
    }

    #[test]
    fn test_suggest_tools_multi_tool_ordering() {
        let tools = parser().suggest_tools("fetch https://example.com and search for docs");
        assert!(tools.contains(&"web_fetch".to_string()));
        assert!(tools.contains(&"web_search".to_string()));
        // web_fetch comes before web_search in ToolFlags::to_vec
        let fetch_idx = tools.iter().position(|t| t == "web_fetch").unwrap();
        let search_idx = tools.iter().position(|t| t == "web_search").unwrap();
        assert!(
            fetch_idx < search_idx,
            "web_fetch should come before web_search"
        );
    }

    // --- update_soul intent routing tests ---

    #[test]
    fn test_suggest_tools_update_soul_persona() {
        let tools = parser().suggest_tools("update my persona to be more friendly");
        assert!(
            tools.contains(&"update_soul".to_string()),
            "Should suggest update_soul: {:?}",
            tools
        );
    }

    #[test]
    fn test_suggest_tools_edit_soul_md_suppresses_file_write() {
        let tools = parser().suggest_tools("edit SOUL.md with new vibe");
        assert!(
            tools.contains(&"update_soul".to_string()),
            "Should suggest update_soul: {:?}",
            tools
        );
        assert!(
            !tools.contains(&"file_write".to_string()),
            "update_soul should suppress file_write: {:?}",
            tools
        );
    }

    #[test]
    fn test_suggest_tools_change_personality() {
        let tools = parser().suggest_tools("change personality to pirate");
        assert!(
            tools.contains(&"update_soul".to_string()),
            "Should suggest update_soul: {:?}",
            tools
        );
    }

    #[test]
    fn test_suggest_tools_write_readme_no_update_soul() {
        let tools = parser().suggest_tools("write README.md with installation instructions");
        assert!(
            !tools.contains(&"update_soul".to_string()),
            "Should NOT suggest update_soul for unrelated write: {:?}",
            tools
        );
    }

    // --- parse_with_skills tests ---

    use std::io::Write;
    use tempfile::TempDir;

    fn create_test_skill_dir(parent: &std::path::Path, name: &str, content: &str) -> std::path::PathBuf {
        let dir = parent.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let md_path = dir.join("SKILL.md");
        let mut f = std::fs::File::create(&md_path).unwrap();
        f.write_all(content.as_bytes()).unwrap();
        dir
    }

    fn make_test_catalog() -> (TempDir, SkillCatalog) {
        let tmp = TempDir::new().unwrap();
        create_test_skill_dir(tmp.path(), "code-review", r#"---
name: "Code Review"
description: "Review code for bugs"
command: "review"
trigger_patterns:
  - "review.*code"
  - "code review"
tools_required:
  - "file_read"
auto_load: false
---

## Instructions

Review the code.
"#);
        create_test_skill_dir(tmp.path(), "explain-code", r#"---
name: "Explain Code"
description: "Explain what code does"
command: "explain-code"
trigger_patterns:
  - "explain.*code"
  - "what does.*do"
auto_load: false
---

## Instructions

Explain step by step.
"#);
        create_test_skill_dir(tmp.path(), "commit-message", r#"---
name: "Commit Message"
description: "Generate commit messages"
command: "commit"
trigger_patterns:
  - "commit message"
  - "git commit"
auto_load: false
---

## Instructions

Generate a conventional commit.
"#);

        let catalog = SkillCatalog::new();
        catalog.scan_directory(tmp.path());
        (tmp, catalog)
    }

    #[test]
    fn test_parse_with_skills_slash_command_review() {
        let (_tmp, catalog) = make_test_catalog();
        let intent = parser().parse_with_skills("/review some code", &catalog);
        match intent {
            Intent::SkillInvocation { skill_name, query } => {
                assert_eq!(skill_name, "Code Review");
                assert_eq!(query, "some code");
            }
            other => panic!("Expected SkillInvocation, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_with_skills_slash_command_explain() {
        let (_tmp, catalog) = make_test_catalog();
        let intent = parser().parse_with_skills("/explain-code main.rs", &catalog);
        match intent {
            Intent::SkillInvocation { skill_name, query } => {
                assert_eq!(skill_name, "Explain Code");
                assert_eq!(query, "main.rs");
            }
            other => panic!("Expected SkillInvocation, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_with_skills_slash_command_no_query() {
        let (_tmp, catalog) = make_test_catalog();
        let intent = parser().parse_with_skills("/review", &catalog);
        match intent {
            Intent::SkillInvocation { skill_name, query } => {
                assert_eq!(skill_name, "Code Review");
                assert_eq!(query, "/review"); // full text when no query
            }
            other => panic!("Expected SkillInvocation, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_with_skills_trigger_match() {
        let (_tmp, catalog) = make_test_catalog();
        let intent = parser().parse_with_skills("please review my code for bugs", &catalog);
        match intent {
            Intent::SkillInvocation { skill_name, query } => {
                assert_eq!(skill_name, "Code Review");
                assert_eq!(query, "please review my code for bugs");
            }
            other => panic!("Expected SkillInvocation, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_with_skills_trigger_commit() {
        let (_tmp, catalog) = make_test_catalog();
        let intent = parser().parse_with_skills("generate a git commit message", &catalog);
        match intent {
            Intent::SkillInvocation { skill_name, query } => {
                assert_eq!(skill_name, "Commit Message");
                assert_eq!(query, "generate a git commit message");
            }
            other => panic!("Expected SkillInvocation, got {:?}", other),
        }
    }

    #[test]
    fn test_parse_with_skills_no_match_fallthrough() {
        let (_tmp, catalog) = make_test_catalog();
        let intent = parser().parse_with_skills("hello world", &catalog);
        assert!(
            matches!(intent, Intent::SimpleQuery { .. }),
            "Should fall through to SimpleQuery, got {:?}",
            intent
        );
    }

    #[test]
    fn test_parse_with_skills_unknown_slash_fallthrough() {
        let (_tmp, catalog) = make_test_catalog();
        // /status is a built-in command, not a skill — should fall through to parse()
        let intent = parser().parse_with_skills("/status", &catalog);
        assert!(
            matches!(intent, Intent::TaskQuery { .. }),
            "Should fall through to TaskQuery, got {:?}",
            intent
        );
    }

    #[test]
    fn test_parse_with_skills_empty_catalog() {
        let catalog = SkillCatalog::new();
        let intent = parser().parse_with_skills("/review some code", &catalog);
        // No skills loaded — slash command won't match, falls through
        // "/review some code" doesn't match built-in slash commands either
        // It should become a SimpleQuery
        assert!(
            matches!(intent, Intent::SimpleQuery { .. }),
            "Should fall through to SimpleQuery with empty catalog, got {:?}",
            intent
        );
    }

    #[test]
    fn test_skill_invocation_intent_type() {
        let intent = Intent::SkillInvocation {
            skill_name: "Test".to_string(),
            query: "test query".to_string(),
        };
        assert_eq!(intent.intent_type(), "skill_invocation");
    }
}
