//! Weighted scoring router for skill auto-selection.
//!
//! Scores each skill in the catalog against a user query using:
//! - Intent pattern matching (substring, case-insensitive)
//! - Keyword ratio matching
//! - Recency bonus (recently used skills)
//! - Negative keyword penalty

use crate::bus::EventBus;
use crate::events::SystemEvent;
use crate::orchestrator::skill::catalog::SkillCatalog;
use chrono::Utc;
use std::sync::RwLock;

/// Result of routing a user query against the skill catalog.
#[derive(Debug, Clone)]
pub struct RouteResult {
    /// Skill ID auto-selected (score >= auto_select_threshold and invoke.mode == "auto").
    pub selected: Option<String>,
    /// Skills in the suggest range (score >= suggest_threshold but below auto-select or not "auto" mode).
    pub suggestions: Vec<RouteSuggestion>,
    /// All scores for debug/observability.
    pub scores: Vec<RouteScore>,
}

/// A suggestion for a skill that scored in the suggest range.
#[derive(Debug, Clone)]
pub struct RouteSuggestion {
    pub skill_id: String,
    pub skill_name: String,
    pub score: f64,
    pub reason: String,
}

/// Per-skill score breakdown.
#[derive(Debug, Clone)]
pub struct RouteScore {
    pub skill_id: String,
    pub score: f64,
    pub intent_match: bool,
    pub keyword_ratio: f64,
    pub recency_bonus: f64,
    pub negative_hit: bool,
    /// DEPRECATED: Always 0.0. Health-based scoring was never implemented.
    /// Kept for serialization backward compatibility; will be removed in a future release.
    pub health_bonus: f64,
}

/// Weighted scoring router for skill selection.
pub struct SkillRouter {
    recent_skills: RwLock<Vec<String>>,
    recency_window: usize,
    auto_select_threshold: f64,
    suggest_threshold: f64,
    /// Optional event bus for emitting lifecycle events.
    bus: Option<EventBus>,
}

const DEFAULT_RECENCY_WINDOW: usize = 10;

impl SkillRouter {
    pub fn new(auto_select_threshold: f64, suggest_threshold: f64) -> Self {
        Self {
            recent_skills: RwLock::new(Vec::new()),
            recency_window: DEFAULT_RECENCY_WINDOW,
            auto_select_threshold,
            suggest_threshold,
            bus: None,
        }
    }

    /// Create a new SkillRouter with an event bus for lifecycle events.
    pub fn new_with_bus(auto_select_threshold: f64, suggest_threshold: f64, bus: EventBus) -> Self {
        Self {
            recent_skills: RwLock::new(Vec::new()),
            recency_window: DEFAULT_RECENCY_WINDOW,
            auto_select_threshold,
            suggest_threshold,
            bus: Some(bus),
        }
    }

    /// Route a user query against the catalog, returning scored results.
    pub fn route(&self, user_query: &str, catalog: &SkillCatalog) -> RouteResult {
        let query_lower = user_query.to_lowercase();

        let recent = self
            .recent_skills
            .read()
            .map(|g| g.clone())
            .unwrap_or_default();

        let entries = catalog.entries_snapshot();
        let mut scores: Vec<RouteScore> = Vec::with_capacity(entries.len());

        for (skill_id, entry) in &entries {
            let fm = &entry.frontmatter;
            let weights = &fm.routing.weights;

            // Skip disabled skills
            if fm.invoke.mode == "disabled" {
                continue;
            }

            // Skip skills the ENABLE axis has made unrunnable — **any** one of
            // their required capabilities wholly withheld, or (legacy branch)
            // every allowed name owned by a withdrawn extension. The same
            // predicate invocation refuses on (design §6.2 #12, §10 case 3):
            // without this a `mode: auto` skill whose only capability went with
            // a disabled server still auto-selects at score >= 0.65 and runs
            // toolless. Nothing is announced here — nothing was attempted.
            if !catalog.is_satisfiable(fm) {
                continue;
            }

            // Intent match: any routing.intent phrase is a case-insensitive substring of query
            let intent_match = fm
                .routing
                .intent
                .iter()
                .any(|phrase| query_lower.contains(&phrase.to_lowercase()));

            // Keyword ratio: matched keywords / total keywords
            let keyword_ratio = if fm.routing.keywords.is_empty() {
                0.0
            } else {
                let matched = fm
                    .routing
                    .keywords
                    .iter()
                    .filter(|kw| query_lower.contains(&kw.to_lowercase()))
                    .count();
                matched as f64 / fm.routing.keywords.len() as f64
            };

            // Recency bonus: skill was recently used
            let recency_bonus = if recent.contains(skill_id) { 1.0 } else { 0.0 };

            // Negative keyword hit
            let negative_hit = fm
                .routing
                .negative_keywords
                .iter()
                .any(|nk| query_lower.contains(&nk.to_lowercase()));

            // Compute score
            let score = weights.base
                + if intent_match {
                    weights.intent_weight
                } else {
                    0.0
                }
                + keyword_ratio * weights.keyword_weight
                + recency_bonus * weights.recency_weight
                - if negative_hit {
                    weights.negative_penalty
                } else {
                    0.0
                };

            scores.push(RouteScore {
                skill_id: skill_id.clone(),
                score,
                intent_match,
                keyword_ratio,
                recency_bonus,
                negative_hit,
                health_bonus: 0.0,
            });
        }

        // Sort by score descending
        scores.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Determine selected and suggestions
        let mut selected: Option<String> = None;
        let mut suggestions: Vec<RouteSuggestion> = Vec::new();

        for rs in &scores {
            if rs.score < self.suggest_threshold {
                continue;
            }

            // Look up the entry to check invoke.mode
            let invoke_mode = entries
                .iter()
                .find(|(id, _)| *id == rs.skill_id)
                .map(|(_, e)| e.frontmatter.invoke.mode.as_str())
                .unwrap_or("manual");

            let skill_name = entries
                .iter()
                .find(|(id, _)| *id == rs.skill_id)
                .map(|(_, e)| e.frontmatter.name.clone())
                .unwrap_or_else(|| rs.skill_id.clone());

            if rs.score >= self.auto_select_threshold && invoke_mode == "auto" && selected.is_none()
            {
                selected = Some(rs.skill_id.clone());
            } else {
                let reason = build_reason(rs);
                suggestions.push(RouteSuggestion {
                    skill_id: rs.skill_id.clone(),
                    skill_name,
                    score: rs.score,
                    reason,
                });
            }
        }

        // Emit lifecycle event for auto-selected skill
        if let Some(ref selected_id) = selected
            && let Some(ref bus) = self.bus
        {
            let score = scores
                .iter()
                .find(|s| s.skill_id == *selected_id)
                .map(|s| s.score)
                .unwrap_or(0.0);
            bus.publish(SystemEvent::SkillSelected {
                skill_id: selected_id.clone(),
                score,
                query_preview: user_query.chars().take(100).collect(),
                timestamp: Utc::now(),
            });
        }

        RouteResult {
            selected,
            suggestions,
            scores,
        }
    }

    /// Record that a skill was used (for recency bonus).
    pub fn record_usage(&self, skill_id: &str) {
        if let Ok(mut recent) = self.recent_skills.write() {
            // Remove any existing occurrence to avoid duplicates
            recent.retain(|s| s != skill_id);
            recent.push(skill_id.to_string());
            // Trim to window size
            while recent.len() > self.recency_window {
                recent.remove(0);
            }
        }
    }
}

fn build_reason(rs: &RouteScore) -> String {
    let mut parts = Vec::new();
    if rs.intent_match {
        parts.push("intent match");
    }
    if rs.keyword_ratio > 0.0 {
        parts.push("keyword match");
    }
    if rs.recency_bonus > 0.0 {
        parts.push("recently used");
    }
    if rs.negative_hit {
        parts.push("negative keyword penalty");
    }
    if rs.health_bonus > 0.0 {
        parts.push("health bonus");
    }
    if parts.is_empty() {
        "base score only".to_string()
    } else {
        parts.join(", ")
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
