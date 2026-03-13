use crate::agent::subagent::AgentConstraints;
use crate::prompt_ctx::package::{ContextPackage, HandoffContext, PackageSection, PackageSectionKind};
use crate::prompt_ctx::section::{ContextBundle, ContextKind, ContextSection, SectionPriority};
use crate::prompt_ctx::sources::{ContextRequest, ContextSource};
use arc_swap::ArcSwap;
use std::sync::Arc;

const SUB_AGENT_CONTEXT_RATIO: f64 = 0.40;

pub struct ContextManager {
    sources: Vec<Box<dyn ContextSource>>,
    config: Arc<ArcSwap<crate::daemon_config::DaemonConfig>>,
}

impl ContextManager {
    pub fn new(
        sources: Vec<Box<dyn ContextSource>>,
        config: Arc<ArcSwap<crate::daemon_config::DaemonConfig>>,
    ) -> Self {
        Self { sources, config }
    }

    pub fn noop() -> Self {
        Self {
            sources: Vec::new(),
            config: Arc::new(ArcSwap::from_pointee(
                crate::daemon_config::DaemonConfig::default(),
            )),
        }
    }

    fn autocompact_buffer_ratio(&self) -> f64 {
        self.config.load().execution.context.autocompact_buffer_ratio
    }

    pub async fn resolve(&self, request: &ContextRequest) -> ContextBundle {
        let buffer =
            (request.model_context_window as f64 * self.autocompact_buffer_ratio()) as usize;
        let available = request
            .model_context_window
            .saturating_sub(request.reserved_tokens)
            .saturating_sub(buffer);

        // Resolve all active sources in parallel
        let active: Vec<&dyn ContextSource> = self
            .sources
            .iter()
            .filter(|s| s.active_for(&request.path))
            .map(|s| s.as_ref())
            .collect();
        let results =
            futures_util::future::join_all(active.iter().map(|s| s.resolve(request))).await;
        let mut sections: Vec<ContextSection> = results.into_iter().flatten().collect();

        // Sort: highest priority first, then highest relevance within tier
        sections.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then(b.relevance.partial_cmp(&a.relevance).unwrap_or(std::cmp::Ordering::Equal))
        });

        // Greedy fill
        let mut used = 0usize;
        let mut included = Vec::new();
        for section in sections {
            if used + section.token_estimate <= available {
                used += section.token_estimate;
                included.push(section);
            } else if section.priority >= SectionPriority::Normal {
                let remaining = available.saturating_sub(used);
                if remaining > 100 {
                    let truncated = Self::truncate_section(section, remaining);
                    used += truncated.token_estimate;
                    included.push(truncated);
                }
            }
        }

        ContextBundle {
            sections: included,
            total_tokens: used,
            available_budget: available,
        }
    }

    fn truncate_section(mut section: ContextSection, max_tokens: usize) -> ContextSection {
        let max_chars = max_tokens * 4;
        if section.content.len() > max_chars && max_chars > 20 {
            let end = section.content.floor_char_boundary(max_chars);
            section.content.truncate(end);
            section.content.push_str("\n[...truncated]");
            section.token_estimate = section.content.len() / 4;
        }
        section
    }

    /// Create a [`ContextPackage`] for a sub-agent by distilling a parent bundle.
    ///
    /// Budget: the sub-agent gets at most 40% of its window for context.
    /// Access control: sections listed in `agent_constraints.denied_sections` are filtered out.
    /// Priority is remapped for sub-agent context (e.g. ConversationSummary → High).
    /// Remaining budget is filled with a greedy algorithm.
    pub fn distill(
        &self,
        parent_bundle: &ContextBundle,
        agent_constraints: &AgentConstraints,
        sub_agent_window: usize,
        task_description: &str,
        handoff: Option<&HandoffContext>,
    ) -> ContextPackage {
        let context_budget = (sub_agent_window as f64 * SUB_AGENT_CONTEXT_RATIO) as usize;

        // Access control: filter out denied sections
        let denied = &agent_constraints.denied_sections;
        let allowed_sections: Vec<&ContextSection> = parent_bundle
            .sections
            .iter()
            .filter(|s| !denied.iter().any(|d| d.eq_ignore_ascii_case(s.kind.as_str())))
            .collect();

        let mut package_sections = Vec::new();

        // Task assignment (Critical — always included)
        package_sections.push(PackageSection {
            kind: PackageSectionKind::TaskDescription,
            content: task_description.to_string(),
            token_estimate: task_description.len() / 4,
            priority: SectionPriority::Critical,
        });

        // Predecessor output (High — pipeline/DAG handoff)
        if let Some(handoff) = handoff {
            package_sections.push(PackageSection {
                kind: PackageSectionKind::PredecessorOutput,
                content: handoff.format(),
                token_estimate: handoff.token_estimate(),
                priority: SectionPriority::High,
            });
        }

        // Re-map parent sections with sub-agent-appropriate priorities
        for section in allowed_sections {
            let sub_priority = match section.kind {
                ContextKind::ConversationSummary => SectionPriority::High,
                ContextKind::Memory => SectionPriority::Normal,
                ContextKind::WorkspaceArtifact => SectionPriority::Normal,
                ContextKind::UserProfile => SectionPriority::Low,
                ContextKind::SkillReference => continue, // sub-agents have own skills
            };
            package_sections.push(PackageSection {
                kind: PackageSectionKind::from(&section.kind),
                content: section.content.clone(),
                token_estimate: section.token_estimate,
                priority: sub_priority,
            });
        }

        // Budget-fill with greedy algorithm
        // Sort: highest priority first, then smallest sections first within tier
        package_sections.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then(a.token_estimate.cmp(&b.token_estimate))
        });

        let mut used = 0usize;
        let mut included = Vec::new();
        for section in package_sections {
            if used + section.token_estimate <= context_budget {
                used += section.token_estimate;
                included.push(section);
            } else if section.priority >= SectionPriority::Normal {
                let remaining = context_budget.saturating_sub(used);
                if remaining > 100 {
                    let truncated = Self::truncate_package_section(section, remaining);
                    used += truncated.token_estimate;
                    included.push(truncated);
                }
            }
        }

        ContextPackage {
            sections: included,
            total_tokens: used,
            budget: context_budget,
            sub_agent_window,
        }
    }

    fn truncate_package_section(mut section: PackageSection, max_tokens: usize) -> PackageSection {
        let max_chars = max_tokens * 4;
        if section.content.len() > max_chars && max_chars > 20 {
            let end = section.content.floor_char_boundary(max_chars);
            section.content.truncate(end);
            section.content.push_str("\n[...truncated]");
            section.token_estimate = section.content.len() / 4;
        }
        section
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon_config::{ContextBudgetConfig, DaemonConfig};
    use crate::memory::scope_context::MemoryScopeContext;
    use crate::orchestrator::intent::Intent;
    use crate::agent::subagent::AgentConstraints;
    use crate::prompt_ctx::package::{AgentSummary, HandoffContext, PackageSectionKind};
    use crate::prompt_ctx::section::{ContextKey, ContextKind, InjectionMode};
    use crate::prompt_ctx::sources::ExecutionPath;
    use async_trait::async_trait;

    fn test_config_with_buffer(ratio: f64) -> Arc<ArcSwap<DaemonConfig>> {
        let mut config = DaemonConfig::default();
        config.execution.context = ContextBudgetConfig {
            autocompact_buffer_ratio: ratio,
            ..ContextBudgetConfig::default()
        };
        Arc::new(ArcSwap::from_pointee(config))
    }

    fn test_request(window: usize, reserved: usize) -> ContextRequest {
        ContextRequest {
            query: "test query".to_string(),
            intent: Intent::SimpleQuery {
                query: "test query".to_string(),
            },
            path: ExecutionPath::SimpleQuery,
            skill: None,
            owner_id: None,
            scope: MemoryScopeContext::global_only(),
            model_context_window: window,
            reserved_tokens: reserved,
        }
    }

    fn make_section(
        content: &str,
        priority: SectionPriority,
        tokens: usize,
        relevance: f32,
    ) -> ContextSection {
        ContextSection {
            source: "test",
            kind: ContextKind::Memory,
            content: content.to_string(),
            token_estimate: tokens,
            priority,
            relevance,
            key: ContextKey::ConversationSummary,
            injection: InjectionMode::SystemMessage,
        }
    }

    struct FakeSource {
        sections: Vec<ContextSection>,
    }

    #[async_trait]
    impl ContextSource for FakeSource {
        fn name(&self) -> &'static str {
            "fake"
        }

        async fn resolve(&self, _request: &ContextRequest) -> Vec<ContextSection> {
            self.sections.clone()
        }
    }

    #[tokio::test]
    async fn test_resolve_greedy_fill() {
        // window=1000, reserved=0, buffer_ratio=0.0 => available=1000
        let config = test_config_with_buffer(0.0);
        let sources: Vec<Box<dyn ContextSource>> = vec![Box::new(FakeSource {
            sections: vec![
                make_section("Section A", SectionPriority::High, 300, 0.9),
                make_section("Section B", SectionPriority::Normal, 300, 0.8),
            ],
        })];
        let manager = ContextManager::new(sources, config);
        let request = test_request(1000, 0);
        let bundle = manager.resolve(&request).await;

        assert_eq!(bundle.sections.len(), 2, "both sections should fit");
        assert_eq!(bundle.total_tokens, 600);
        assert_eq!(bundle.available_budget, 1000);
    }

    #[tokio::test]
    async fn test_resolve_drops_low_priority_when_full() {
        // window=500, reserved=0, buffer_ratio=0.0 => available=500
        // High(400 tokens) fits, Low(200 tokens) would exceed budget and is below Normal threshold
        let config = test_config_with_buffer(0.0);
        let sources: Vec<Box<dyn ContextSource>> = vec![Box::new(FakeSource {
            sections: vec![
                make_section("High content", SectionPriority::High, 400, 0.9),
                make_section("Low content", SectionPriority::Low, 200, 0.5),
            ],
        })];
        let manager = ContextManager::new(sources, config);
        let request = test_request(500, 0);
        let bundle = manager.resolve(&request).await;

        assert_eq!(bundle.sections.len(), 1, "only the High section fits");
        assert_eq!(bundle.sections[0].priority, SectionPriority::High);
        assert_eq!(bundle.total_tokens, 400);
    }

    #[test]
    fn test_distill_basic() {
        let config = test_config_with_buffer(0.0);
        let manager = ContextManager::new(vec![], config);

        let bundle = ContextBundle {
            sections: vec![
                make_section("memory content", SectionPriority::Normal, 100, 0.8),
                ContextSection {
                    source: "test",
                    kind: ContextKind::UserProfile,
                    content: "user prefs".to_string(),
                    token_estimate: 50,
                    priority: SectionPriority::Normal,
                    relevance: 0.5,
                    key: ContextKey::UserProfile,
                    injection: InjectionMode::SystemMessage,
                },
            ],
            total_tokens: 150,
            available_budget: 1000,
        };

        let constraints = AgentConstraints::default();
        let package = manager.distill(&bundle, &constraints, 10_000, "Analyze logs", None);

        // Budget = 10_000 * 0.40 = 4000
        assert_eq!(package.budget, 4000);
        assert_eq!(package.sub_agent_window, 10_000);
        // Should have: TaskDescription (Critical) + Memory (Normal) + UserProfile (Low)
        assert!(package
            .sections
            .iter()
            .any(|s| s.kind == PackageSectionKind::TaskDescription));
        assert!(package.total_tokens > 0);
    }

    #[test]
    fn test_distill_denied_sections() {
        let config = test_config_with_buffer(0.0);
        let manager = ContextManager::new(vec![], config);

        let bundle = ContextBundle {
            sections: vec![
                make_section("memory content", SectionPriority::Normal, 100, 0.8),
                ContextSection {
                    source: "test",
                    kind: ContextKind::UserProfile,
                    content: "user prefs".to_string(),
                    token_estimate: 50,
                    priority: SectionPriority::Normal,
                    relevance: 0.5,
                    key: ContextKey::UserProfile,
                    injection: InjectionMode::SystemMessage,
                },
            ],
            total_tokens: 150,
            available_budget: 1000,
        };

        let mut constraints = AgentConstraints::default();
        constraints.denied_sections = vec!["user_profile".to_string()];
        let package = manager.distill(&bundle, &constraints, 10_000, "Task", None);

        // UserProfile should be filtered out
        assert!(!package
            .sections
            .iter()
            .any(|s| s.kind == PackageSectionKind::UserProfile));
    }

    #[test]
    fn test_distill_with_handoff() {
        let config = test_config_with_buffer(0.0);
        let manager = ContextManager::new(vec![], config);

        let bundle = ContextBundle {
            sections: vec![],
            total_tokens: 0,
            available_budget: 1000,
        };

        let handoff = HandoffContext {
            producer: AgentSummary {
                name: "prev_agent".to_string(),
                role: "analyzer".to_string(),
                step: 1,
            },
            task_assigned: "Analyze auth".to_string(),
            output: "Found bug".to_string(),
            decisions: vec!["Fix NPE".to_string()],
            handoff_notes: None,
        };

        let constraints = AgentConstraints::default();
        let package =
            manager.distill(&bundle, &constraints, 10_000, "Fix bug", Some(&handoff));

        // Should have TaskDescription + PredecessorOutput
        assert!(package
            .sections
            .iter()
            .any(|s| s.kind == PackageSectionKind::TaskDescription));
        assert!(package
            .sections
            .iter()
            .any(|s| s.kind == PackageSectionKind::PredecessorOutput));
    }

    #[test]
    fn test_distill_priority_remapping() {
        let config = test_config_with_buffer(0.0);
        let manager = ContextManager::new(vec![], config);

        let bundle = ContextBundle {
            sections: vec![ContextSection {
                source: "test",
                kind: ContextKind::ConversationSummary,
                content: "conv summary".to_string(),
                token_estimate: 50,
                priority: SectionPriority::Normal, // parent priority
                relevance: 0.7,
                key: ContextKey::ConversationSummary,
                injection: InjectionMode::SystemMessage,
            }],
            total_tokens: 50,
            available_budget: 1000,
        };

        let constraints = AgentConstraints::default();
        let package = manager.distill(&bundle, &constraints, 10_000, "Task", None);

        // ConversationSummary should be remapped to High for sub-agents
        let conv = package
            .sections
            .iter()
            .find(|s| s.kind == PackageSectionKind::ConversationSummary)
            .unwrap();
        assert_eq!(conv.priority, SectionPriority::High);
    }
}
