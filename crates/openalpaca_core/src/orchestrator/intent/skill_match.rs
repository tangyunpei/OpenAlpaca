//! Skill-aware intent parsing: slash commands, trigger patterns, and weighted router scoring.

use super::{Intent, IntentParser};
use crate::orchestrator::skill_catalog::SkillCatalog;
use crate::orchestrator::skill_router::SkillRouter;

impl IntentParser {
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
}
