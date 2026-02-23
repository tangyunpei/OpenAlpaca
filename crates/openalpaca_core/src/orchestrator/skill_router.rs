//! Weighted scoring router for skill auto-selection.
//!
//! Scores each skill in the catalog against a user query using:
//! - Intent pattern matching (substring, case-insensitive)
//! - Keyword ratio matching
//! - Recency bonus (recently used skills)
//! - Negative keyword penalty

use crate::bus::EventBus;
use crate::events::SystemEvent;
use crate::orchestrator::skill_catalog::SkillCatalog;
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
const NEGATIVE_PENALTY: f64 = 0.6;

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
    pub fn new_with_bus(
        auto_select_threshold: f64,
        suggest_threshold: f64,
        bus: EventBus,
    ) -> Self {
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
            let recency_bonus = if recent.contains(skill_id) {
                1.0
            } else {
                0.0
            };

            // Negative keyword hit
            let negative_hit = fm
                .routing
                .negative_keywords
                .iter()
                .any(|nk| query_lower.contains(&nk.to_lowercase()));

            // Compute score
            let score = weights.base
                + if intent_match { weights.intent_weight } else { 0.0 }
                + keyword_ratio * weights.keyword_weight
                + recency_bonus * weights.recency_weight
                - if negative_hit { NEGATIVE_PENALTY } else { 0.0 };

            scores.push(RouteScore {
                skill_id: skill_id.clone(),
                score,
                intent_match,
                keyword_ratio,
                recency_bonus,
                negative_hit,
            });
        }

        // Sort by score descending
        scores.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

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
mod tests {
    use super::*;
    use crate::middleware::skill::SkillScope;
    use std::io::Write;
    use std::path::Path;
    use tempfile::TempDir;

    fn create_skill_dir(parent: &Path, name: &str, skill_md: &str) -> std::path::PathBuf {
        let dir = parent.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let md_path = dir.join("SKILL.md");
        let mut f = std::fs::File::create(&md_path).unwrap();
        f.write_all(skill_md.as_bytes()).unwrap();
        dir
    }

    fn default_router() -> SkillRouter {
        SkillRouter::new(0.65, 0.45)
    }

    #[test]
    fn test_intent_match_auto_select() {
        // intent match alone: base(0.2) + intent(0.45) = 0.65 -> auto-select
        let tmp = TempDir::new().unwrap();
        create_skill_dir(
            tmp.path(),
            "code-review",
            r#"---
name: "Code Review"
description: "Review code"
invoke:
  mode: auto
routing:
  intent:
    - "review code"
---
"#,
        );

        let catalog = SkillCatalog::new();
        catalog.scan_directory(tmp.path(), SkillScope::Project);

        let router = default_router();
        let result = router.route("please review code for me", &catalog);

        assert!(result.selected.is_some());
        assert_eq!(result.selected.unwrap(), "code-review");
    }

    #[test]
    fn test_keywords_only_below_threshold() {
        // 2/4 keywords: base(0.2) + 0.5 * 0.35 = 0.375 -> skip
        let tmp = TempDir::new().unwrap();
        create_skill_dir(
            tmp.path(),
            "data-analysis",
            r#"---
name: "Data Analysis"
description: "Analyze data"
invoke:
  mode: auto
routing:
  keywords:
    - "data"
    - "analyze"
    - "statistics"
    - "chart"
---
"#,
        );

        let catalog = SkillCatalog::new();
        catalog.scan_directory(tmp.path(), SkillScope::Project);

        let router = default_router();
        let result = router.route("analyze the data", &catalog);

        // Score 0.375 < 0.45 suggest threshold -> no selected, no suggestions
        assert!(result.selected.is_none());
        assert!(result.suggestions.is_empty());
        assert_eq!(result.scores.len(), 1);
        let score = &result.scores[0];
        assert!((score.score - 0.375).abs() < 0.01);
    }

    #[test]
    fn test_intent_plus_keywords_full_score() {
        // intent + all keywords: 0.2 + 0.45 + 1.0 * 0.35 = 1.0 -> auto-select
        let tmp = TempDir::new().unwrap();
        create_skill_dir(
            tmp.path(),
            "review",
            r#"---
name: "Full Review"
description: "Full review"
invoke:
  mode: auto
routing:
  intent:
    - "review"
  keywords:
    - "code"
    - "bugs"
---
"#,
        );

        let catalog = SkillCatalog::new();
        catalog.scan_directory(tmp.path(), SkillScope::Project);

        let router = default_router();
        let result = router.route("review code for bugs", &catalog);

        assert!(result.selected.is_some());
        let score = &result.scores[0];
        assert!((score.score - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_negative_keyword_penalty() {
        // intent match: 0.2 + 0.45 = 0.65, minus 0.6 penalty = 0.05
        let tmp = TempDir::new().unwrap();
        create_skill_dir(
            tmp.path(),
            "deploy",
            r#"---
name: "Deploy"
description: "Deploy code"
invoke:
  mode: auto
routing:
  intent:
    - "deploy"
  negative_keywords:
    - "rollback"
---
"#,
        );

        let catalog = SkillCatalog::new();
        catalog.scan_directory(tmp.path(), SkillScope::Project);

        let router = default_router();
        let result = router.route("deploy but also rollback if needed", &catalog);

        assert!(result.selected.is_none());
        assert!(result.suggestions.is_empty());
        let score = &result.scores[0];
        assert!(score.negative_hit);
        assert!((score.score - 0.05).abs() < 0.01);
    }

    #[test]
    fn test_recency_bonus() {
        // base(0.2) + recency(0.2) = 0.4, still below suggest threshold
        // With intent: 0.2 + 0.45 + 0.2 = 0.85 -> auto-select
        let tmp = TempDir::new().unwrap();
        create_skill_dir(
            tmp.path(),
            "review",
            r#"---
name: "Review"
description: "Review"
invoke:
  mode: auto
routing:
  intent:
    - "review"
---
"#,
        );

        let catalog = SkillCatalog::new();
        catalog.scan_directory(tmp.path(), SkillScope::Project);

        let router = default_router();
        router.record_usage("review");

        let result = router.route("review this", &catalog);
        let score = &result.scores[0];
        assert!((score.recency_bonus - 1.0).abs() < f64::EPSILON);
        assert!((score.score - 0.85).abs() < 0.01);
    }

    #[test]
    fn test_no_routing_config_base_only() {
        // No routing config -> score = base(0.2) -> skip
        let tmp = TempDir::new().unwrap();
        create_skill_dir(
            tmp.path(),
            "empty-routing",
            r#"---
name: "Empty"
description: "No routing"
invoke:
  mode: auto
---
"#,
        );

        let catalog = SkillCatalog::new();
        catalog.scan_directory(tmp.path(), SkillScope::Project);

        let router = default_router();
        let result = router.route("anything at all", &catalog);

        assert!(result.selected.is_none());
        assert!(result.suggestions.is_empty());
        let score = &result.scores[0];
        assert!((score.score - 0.2).abs() < 0.01);
    }

    #[test]
    fn test_disabled_skill_excluded() {
        let tmp = TempDir::new().unwrap();
        create_skill_dir(
            tmp.path(),
            "disabled",
            r#"---
name: "Disabled"
description: "Disabled skill"
invoke:
  mode: disabled
routing:
  intent:
    - "anything"
---
"#,
        );

        let catalog = SkillCatalog::new();
        catalog.scan_directory(tmp.path(), SkillScope::Project);

        let router = default_router();
        let result = router.route("anything", &catalog);

        // Disabled skill is not even in the catalog (filtered at scan time)
        assert!(result.scores.is_empty());
    }

    #[test]
    fn test_multiple_skills_highest_wins() {
        let tmp = TempDir::new().unwrap();
        create_skill_dir(
            tmp.path(),
            "code-review",
            r#"---
name: "Code Review"
description: "Review code"
invoke:
  mode: auto
routing:
  intent:
    - "review code"
  keywords:
    - "bugs"
    - "style"
---
"#,
        );
        create_skill_dir(
            tmp.path(),
            "explain",
            r#"---
name: "Explain"
description: "Explain code"
invoke:
  mode: auto
routing:
  intent:
    - "explain"
  keywords:
    - "understand"
---
"#,
        );

        let catalog = SkillCatalog::new();
        catalog.scan_directory(tmp.path(), SkillScope::Project);

        let router = default_router();
        let result = router.route("review code for bugs and style issues", &catalog);

        // code-review: intent(0.45) + keywords(2/2 * 0.35) + base(0.2) = 1.0
        // explain: base only = 0.2
        assert_eq!(result.selected, Some("code-review".to_string()));
        assert_eq!(result.scores[0].skill_id, "code-review");
    }

    #[test]
    fn test_manual_mode_not_auto_selected() {
        // Score >= 0.65 but mode is "manual" -> goes to suggestions, not selected
        let tmp = TempDir::new().unwrap();
        create_skill_dir(
            tmp.path(),
            "manual-skill",
            r#"---
name: "Manual Skill"
description: "Manual only"
invoke:
  mode: manual
routing:
  intent:
    - "manual trigger"
---
"#,
        );

        let catalog = SkillCatalog::new();
        catalog.scan_directory(tmp.path(), SkillScope::Project);

        let router = default_router();
        let result = router.route("manual trigger please", &catalog);

        // Score is 0.65 (intent match) but mode is manual -> suggestion only
        assert!(result.selected.is_none());
        assert_eq!(result.suggestions.len(), 1);
        assert_eq!(result.suggestions[0].skill_id, "manual-skill");
    }

    #[test]
    fn test_record_usage_recency_window() {
        let router = SkillRouter::new(0.65, 0.45);

        // Fill beyond the window
        for i in 0..15 {
            router.record_usage(&format!("skill-{}", i));
        }

        let recent = router.recent_skills.read().unwrap();
        assert_eq!(recent.len(), DEFAULT_RECENCY_WINDOW);
        // Oldest entries should be evicted
        assert!(!recent.contains(&"skill-0".to_string()));
        assert!(recent.contains(&"skill-14".to_string()));
    }

    #[test]
    fn test_record_usage_dedup() {
        let router = SkillRouter::new(0.65, 0.45);
        router.record_usage("skill-a");
        router.record_usage("skill-b");
        router.record_usage("skill-a"); // should move to end, not duplicate

        let recent = router.recent_skills.read().unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0], "skill-b");
        assert_eq!(recent[1], "skill-a");
    }

    #[test]
    fn test_suggest_range() {
        // Score 0.45..0.65 goes to suggestions
        let tmp = TempDir::new().unwrap();
        create_skill_dir(
            tmp.path(),
            "partial-match",
            r#"---
name: "Partial Match"
description: "Partially matches"
invoke:
  mode: auto
routing:
  keywords:
    - "data"
    - "analysis"
    - "chart"
---
"#,
        );

        let catalog = SkillCatalog::new();
        catalog.scan_directory(tmp.path(), SkillScope::Project);

        let router = default_router();
        // 2/3 keywords matched: 0.2 + (2/3) * 0.35 = 0.433... -> below suggest
        // 3/3 keywords: 0.2 + 1.0 * 0.35 = 0.55 -> in suggest range
        let result = router.route("data analysis chart", &catalog);

        // All 3 keywords match: score = 0.55
        assert!(result.selected.is_none()); // below 0.65
        assert_eq!(result.suggestions.len(), 1);
        assert!((result.suggestions[0].score - 0.55).abs() < 0.01);
    }
}
