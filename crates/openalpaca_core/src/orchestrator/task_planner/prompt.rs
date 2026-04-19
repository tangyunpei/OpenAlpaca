//! Prompt construction for the LLM-based task planner.
//!
//! Phase 4 Commit 3 migration: `build_hierarchical_prompt` + its
//! `format_agent_list` helper + `build_messages` were absorbed into
//! `compose::static_prompt::build_planner_hierarchical` + `compose::history`.
//! This module now only hosts the predictability-detection regex helpers
//! (called by `TaskPlanner::plan_hierarchical` for the DAG-hint injection).

use regex::Regex;
use std::sync::OnceLock;

// ── Predictable structure detection ─────────────────────────────────

static NUMBERED_LIST_RE: OnceLock<Regex> = OnceLock::new();
static BULLET_LIST_RE: OnceLock<Regex> = OnceLock::new();
static BATCH_KEYWORD_RE: OnceLock<Regex> = OnceLock::new();
static EXPLICIT_QUANTITY_RE: OnceLock<Regex> = OnceLock::new();
static CJK_ENUM_RE: OnceLock<Regex> = OnceLock::new();
static CONJ_LIST_RE: OnceLock<Regex> = OnceLock::new();

fn numbered_list_regex() -> &'static Regex {
    NUMBERED_LIST_RE.get_or_init(|| Regex::new(r"\b\d+\.\s").unwrap())
}

fn bullet_list_regex() -> &'static Regex {
    BULLET_LIST_RE.get_or_init(|| Regex::new(r"(?m)^[\s]*[-*]\s").unwrap())
}

fn batch_keyword_regex() -> &'static Regex {
    BATCH_KEYWORD_RE
        .get_or_init(|| Regex::new(r"(?i)\b(each|all of|every|for each|respectively)\b").unwrap())
}

fn explicit_quantity_regex() -> &'static Regex {
    EXPLICIT_QUANTITY_RE.get_or_init(|| Regex::new(r"(?i)\b(into|to|in)\s+\d+\s").unwrap())
}

/// Detect if a user message contains predictable parallel structure.
pub(super) fn has_predictable_structure(content: &str) -> bool {
    if numbered_list_regex().find_iter(content).count() >= 2 {
        return true;
    }

    if bullet_list_regex().find_iter(content).count() >= 2 {
        return true;
    }

    if content.contains(',') && content.contains(" and ") && batch_keyword_regex().is_match(content)
    {
        return true;
    }

    if explicit_quantity_regex().is_match(content) {
        return true;
    }

    let cjk_enum = CJK_ENUM_RE
        .get_or_init(|| Regex::new(r"[\u4e00-\u9fff]+[、，][\u4e00-\u9fff]+[、，]").unwrap());
    if cjk_enum.is_match(content) {
        return true;
    }

    let conj_list =
        CONJ_LIST_RE.get_or_init(|| Regex::new(r"(?i)\w+,\s+\w+,\s+").unwrap());
    if conj_list.is_match(content) {
        return true;
    }

    false
}
