//! Pre-screening heuristics: fast path eligibility, enhanced simple query detection,
//! and social message classification.

use super::IntentParser;
use crate::utils::social::is_social_phrase;

impl IntentParser {
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

    /// Enhanced pre-screening: broader simple query detection using tool signals
    /// and task-verb analysis. Catches cases that `is_fast_path_eligible()` misses
    /// (e.g. messages with "please"/"can you" that are still simple queries).
    ///
    /// Returns true when the message has no actionable tool signals AND no task
    /// verbs, meaning it's overwhelmingly likely to be a simple conversational
    /// query that doesn't need LLM planning.
    pub fn is_enhanced_simple_query(&self, content: &str) -> bool {
        // Rule 1: No tool signals — suggest_tools() returns empty
        if !self.suggest_tools(content).is_empty() {
            return false;
        }

        let lower = content.to_lowercase();

        // Rule 2: Social/follow-up patterns (always simple)
        let trimmed_lower = lower.trim();
        if is_social_phrase(content) {
            return true;
        }

        // Compute task verbs BEFORE the short-message check to prevent
        // false positives like "Fix the bug" or "Deploy the app" being
        // treated as simple queries just because they're under 100 chars.
        // This also prevents Chinese task messages (no whitespace between
        // words) from being caught by the word-count heuristic.
        const TASK_VERBS: &[&str] = &[
            "create",
            "build",
            "write",
            "research",
            "translate",
            "send",
            "run",
            "debug",
            "fix",
            "deploy",
            "implement",
            "analyze",
            "generate",
            "design",
            "organize",
            "fetch",
            "download",
            "summarize",
            "search for",
            "look up",
            "编写",
            "创建",
            "翻译",
            "研究",
            "发送",
            "修复",
            "部署",
            "分析",
            "生成",
            "设计",
            "组织",
            "下载",
            "总结",
            "搜索",
            "运行",
            "调试",
            "实现",
            "构建",
            "获取",
            "查找",
            "写",
            "做",
            "改",
            "找",
            "删",
            "测",
            "装",
        ];
        let has_task_verb = TASK_VERBS.iter().any(|v| lower.contains(v));

        // Rule 2b: Very short (≤2 whitespace words) + no task verb → simple
        // Gated on !has_task_verb so Chinese task messages (which have no
        // spaces and appear as 1 "word") are not falsely classified.
        if trimmed_lower.split_whitespace().count() <= 2 && !has_task_verb {
            return true;
        }

        // Rule 3: Short + no task verbs → simple query
        // "What is a closure?" (19 chars, no verb) → true
        // "Fix the bug" (11 chars, "fix" verb) → false
        if content.len() < 100 && !has_task_verb {
            return true;
        }

        // Rule 4: Any length + no task verbs + no tool signals → simple query
        if !has_task_verb {
            return true;
        }

        false
    }

    /// Check if a message is a short social/acknowledgement phrase.
    ///
    /// Used by the social fast path to skip heavy prompt assembly for
    /// trivial conversational replies like "ok", "thanks", "好的".
    pub fn is_social_message(&self, content: &str) -> bool {
        is_social_phrase(content)
    }
}
