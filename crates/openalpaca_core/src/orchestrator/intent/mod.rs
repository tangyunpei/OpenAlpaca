//! Intent classification for user messages.
//!
//! Keyword-based heuristics (LLM integration planned for Phase 5.1).

mod pre_screen;
mod skill_match;
mod tool_suggest;
#[cfg(test)]
mod tests;

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
}
