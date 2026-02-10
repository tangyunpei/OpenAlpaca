//! Intent classification for user messages.
//!
//! Keyword-based heuristics (LLM integration planned for Phase 5.1).

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
}

impl Intent {
    pub fn intent_type(&self) -> &'static str {
        match self {
            Intent::SimpleQuery { .. } => "simple_query",
            Intent::TaskQuery { .. } => "task_query",
            Intent::ComplexTask { .. } => "complex_task",
            Intent::TaskControl { .. } => "task_control",
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
        Regex::new(r"(?i)\bfile\s+(?:named|called)\s+([A-Za-z0-9][A-Za-z0-9._/\-]{0,200})\b").unwrap()
    })
}

#[derive(Default)]
struct ToolFlags {
    web_fetch: bool,
    web_search: bool,
    file_read: bool,
    file_write: bool,
    shell_execute: bool,
}

impl ToolFlags {
    fn to_vec(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.web_fetch { out.push("web_fetch".to_string()); }
        if self.web_search { out.push("web_search".to_string()); }
        if self.file_read { out.push("file_read".to_string()); }
        if self.file_write { out.push("file_write".to_string()); }
        if self.shell_execute { out.push("shell_execute".to_string()); }
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
                return Some(Intent::TaskQuery {
                    task_id: Some(id),
                });
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

    pub fn suggest_tools(&self, content: &str) -> Vec<String> {
        let lower = content.to_lowercase();

        let flags = ToolFlags {
            file_write: Self::has_write_verb(&lower) && Self::mentions_filename(content),

            web_fetch: content.contains("http://") || content.contains("https://")
                || lower.contains("fetch ") || lower.contains("download ")
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

    fn has_write_verb(lower: &str) -> bool {
        const WRITE_VERBS: &[&str] = &["write", "save", "create", "update", "edit", "append", "overwrite"];
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
        assert!(tools.contains(&"file_write".to_string()), "Expected file_write, got: {:?}", tools);
    }

    #[test]
    fn test_suggest_tools_write_file_named() {
        let tools = parser().suggest_tools("write a file named README with docs");
        assert!(tools.contains(&"file_write".to_string()), "Expected file_write, got: {:?}", tools);
    }

    #[test]
    fn test_suggest_tools_write_story_no_file() {
        let tools = parser().suggest_tools("write me a story about files");
        assert!(!tools.contains(&"file_write".to_string()), "Should NOT have file_write: {:?}", tools);
    }

    #[test]
    fn test_suggest_tools_version_no_file_write() {
        let tools = parser().suggest_tools("support v1.x series");
        assert!(!tools.contains(&"file_write".to_string()), "Should NOT have file_write: {:?}", tools);
    }

    #[test]
    fn test_suggest_tools_update_version_no_file_write() {
        let tools = parser().suggest_tools("update to v1.2 and ship it");
        assert!(!tools.contains(&"file_write".to_string()), "Should NOT have file_write: {:?}", tools);
    }

    #[test]
    fn test_suggest_tools_fetch_url() {
        let tools = parser().suggest_tools("fetch https://example.com");
        assert!(tools.contains(&"web_fetch".to_string()), "Expected web_fetch, got: {:?}", tools);
    }

    #[test]
    fn test_suggest_tools_hello_world_empty() {
        let tools = parser().suggest_tools("hello world");
        assert!(tools.is_empty(), "Expected empty, got: {:?}", tools);
    }

    #[test]
    fn test_suggest_tools_run_command() {
        let tools = parser().suggest_tools("run command ls -la");
        assert!(tools.contains(&"shell_execute".to_string()), "Expected shell_execute, got: {:?}", tools);
    }

    #[test]
    fn test_suggest_tools_multi_tool_ordering() {
        let tools = parser().suggest_tools("fetch https://example.com and search for docs");
        assert!(tools.contains(&"web_fetch".to_string()));
        assert!(tools.contains(&"web_search".to_string()));
        // web_fetch comes before web_search in ToolFlags::to_vec
        let fetch_idx = tools.iter().position(|t| t == "web_fetch").unwrap();
        let search_idx = tools.iter().position(|t| t == "web_search").unwrap();
        assert!(fetch_idx < search_idx, "web_fetch should come before web_search");
    }
}
