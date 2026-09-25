//! Intent classification for user messages.
//!
//! Deterministic tiers only (Routing V2): slash commands, task queries, the
//! chat memory commands, and skill routing. Everything else is a
//! `SimpleQuery` handled by the main loop, where task dispatch is the model's
//! tool choice.

mod skill_match;
mod tool_suggest;
#[cfg(test)]
mod tests;

use crate::utils::social::is_social_phrase;

/// Classified intent from a user message.
#[derive(Debug, Clone, PartialEq)]
pub enum Intent {
    /// A simple query / chat message (routed to the main loop).
    SimpleQuery { query: String },
    /// A request for task status (optionally for a specific task).
    TaskQuery { task_id: Option<String> },
    /// A task control command (cancel, pause, resume). A bare command
    /// (no id) carries `task_id: None` and is resolved against the lane's
    /// active workflows by the handler (Routing V2 Phase 3).
    TaskControl {
        task_id: Option<String>,
        action: String,
    },
    /// A skill was invoked (via slash command or trigger pattern match).
    SkillInvocation { skill_name: String, query: String },
    /// A chat memory command (design 09 §12, MD-13): `/remember`, `/forget`
    /// or `/memories`, answered by the daemon with no model. `args` are kept
    /// verbatim — never lower-cased — because the text `/remember` saves is
    /// exactly what the person typed.
    MemoryCommand { verb: MemoryVerb, args: String },
}

impl Intent {
    pub fn intent_type(&self) -> &'static str {
        match self {
            Intent::SimpleQuery { .. } => "simple_query",
            Intent::TaskQuery { .. } => "task_query",
            Intent::TaskControl { .. } => "task_control",
            Intent::SkillInvocation { .. } => "skill_invocation",
            Intent::MemoryCommand { .. } => "memory_command",
        }
    }
}

/// The three chat memory commands (design 09 §12.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryVerb {
    /// `/remember [--kind <k>] [--global | --project] <text>`
    Remember,
    /// `/forget <id-or-prefix>` or `/forget <text>`
    Forget,
    /// `/memories` or `/memories <query>`
    Memories,
}

impl MemoryVerb {
    pub const ALL: [MemoryVerb; 3] = [
        MemoryVerb::Remember,
        MemoryVerb::Forget,
        MemoryVerb::Memories,
    ];

    /// The command word, without its slash.
    pub fn command(self) -> &'static str {
        match self {
            MemoryVerb::Remember => "remember",
            MemoryVerb::Forget => "forget",
            MemoryVerb::Memories => "memories",
        }
    }

    /// The verb a command word names, in any case.
    fn from_command(word: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|verb| word.eq_ignore_ascii_case(verb.command()))
    }
}

/// The slash names the chat memory commands reserve (design 09 §12.3): a
/// skill claiming one is never reached by that slash — the command is parsed
/// first — so the skill loader warns about it and `create-skill` refuses it.
pub const RESERVED_SLASH_NAMES: [&str; 3] = ["remember", "forget", "memories"];

/// Whether `name` — with or without its slash, in any case — is one of
/// [`RESERVED_SLASH_NAMES`].
pub fn is_reserved_slash_name(name: &str) -> bool {
    let name = name.strip_prefix('/').unwrap_or(name);
    RESERVED_SLASH_NAMES
        .iter()
        .any(|reserved| name.eq_ignore_ascii_case(reserved))
}

/// Parses user messages into intents using deterministic rules.
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

        // 2. Chat memory commands: /remember, /forget, /memories — matched on
        //    the message as typed, so the arguments keep their case.
        if let Some(intent) = Self::parse_memory_command(trimmed) {
            return intent;
        }

        // 3. Slash commands: /status, /tasks
        if let Some(intent) = Self::parse_task_query(&lower) {
            return intent;
        }

        // 4. Natural language task query
        if Self::is_natural_task_query(&lower) {
            return Intent::TaskQuery { task_id: None };
        }

        // 5. Default: SimpleQuery
        Intent::SimpleQuery {
            query: trimmed.to_string(),
        }
    }

    /// Check if a message is a short social/acknowledgement phrase.
    ///
    /// Used by the social fast path to skip heavy prompt assembly for
    /// trivial conversational replies like "ok", "thanks", "好的".
    pub fn is_social_message(&self, content: &str) -> bool {
        is_social_phrase(content)
    }

    fn parse_task_control(lower: &str) -> Option<Intent> {
        let (action, rest) = if let Some(r) = lower.strip_prefix("/cancel") {
            ("cancel", r)
        } else if let Some(r) = lower.strip_prefix("/pause") {
            ("pause", r)
        } else if let Some(r) = lower.strip_prefix("/resume") {
            ("resume", r)
        } else {
            return None;
        };
        // Bare command ("/cancel") → id resolved from the lane's active
        // workflows by the handler. With a trailing id ("/cancel <id>") the
        // id is explicit. Anything else ("/cancellation") is not a command.
        if rest.is_empty() {
            return Some(Intent::TaskControl {
                task_id: None,
                action: action.to_string(),
            });
        }
        if !rest.starts_with(char::is_whitespace) {
            return None;
        }
        let task_id = rest.trim().to_string();
        Some(Intent::TaskControl {
            task_id: (!task_id.is_empty()).then_some(task_id),
            action: action.to_string(),
        })
    }

    /// Design 09 §12.3: the command word, in any case, followed by whitespace
    /// or the end of the message (`/remembering` is not `/remember`); the
    /// rest, trimmed, is the arguments as typed.
    fn parse_memory_command(trimmed: &str) -> Option<Intent> {
        let rest = trimmed.strip_prefix('/')?;
        let word_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let (word, args) = rest.split_at(word_end);
        let verb = MemoryVerb::from_command(word)?;
        Some(Intent::MemoryCommand {
            verb,
            args: args.trim().to_string(),
        })
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
}
