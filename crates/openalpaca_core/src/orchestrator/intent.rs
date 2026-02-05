//! Intent classification for user messages.
//!
//! Keyword-based heuristics (LLM integration planned for Phase 5.1).

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
        if lower.starts_with("/status ") {
            let id = lower["/status ".len()..].trim().to_string();
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
}
