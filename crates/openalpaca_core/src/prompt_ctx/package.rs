use crate::prompt_ctx::section::{
    ContextBundle, ContextKey, ContextKind, ContextSection, InjectionMode, SectionPriority,
    TrustLevel,
};

/// What kind of section is in a sub-agent context package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageSectionKind {
    TaskDescription,
    PredecessorOutput,
    ConversationSummary,
    Memory,
    WorkspaceArtifact,
    UserProfile,
}

impl PackageSectionKind {
    /// The source name used when converting to ContextSection.
    pub fn source_name(&self) -> &'static str {
        match self {
            Self::TaskDescription => "task_description",
            Self::PredecessorOutput => "predecessor_output",
            Self::ConversationSummary => "conversation_summary",
            Self::Memory => "memory",
            Self::WorkspaceArtifact => "workspace_artifact",
            Self::UserProfile => "user_profile",
        }
    }

    /// Convert to the corresponding ContextKind.
    /// TaskDescription and PredecessorOutput don't have a direct ContextKind mapping,
    /// so they map to WorkspaceArtifact (closest semantic match for prompt injection safety).
    pub fn to_context_kind(&self) -> ContextKind {
        match self {
            Self::TaskDescription => ContextKind::WorkspaceArtifact,
            Self::PredecessorOutput => ContextKind::WorkspaceArtifact,
            Self::ConversationSummary => ContextKind::ConversationSummary,
            Self::Memory => ContextKind::Memory,
            Self::WorkspaceArtifact => ContextKind::WorkspaceArtifact,
            Self::UserProfile => ContextKind::UserProfile,
        }
    }

    /// Key name for ContextKey::Package.
    pub fn key_name(&self) -> String {
        self.source_name().to_string()
    }
}

impl From<&ContextKind> for PackageSectionKind {
    fn from(kind: &ContextKind) -> Self {
        match kind {
            ContextKind::Memory => Self::Memory,
            ContextKind::ConversationSummary => Self::ConversationSummary,
            ContextKind::SkillReference => Self::WorkspaceArtifact,
            ContextKind::WorkspaceArtifact => Self::WorkspaceArtifact,
            ContextKind::UserProfile => Self::UserProfile,
        }
    }
}

/// A section in a sub-agent context package.
#[derive(Debug, Clone)]
pub struct PackageSection {
    pub kind: PackageSectionKind,
    pub content: String,
    pub token_estimate: usize,
    pub priority: SectionPriority,
}

/// A context package prepared for a sub-agent.
#[derive(Debug, Clone)]
pub struct ContextPackage {
    pub sections: Vec<PackageSection>,
    pub total_tokens: usize,
    pub budget: usize,
    pub sub_agent_window: usize,
}

impl ContextPackage {
    /// Convert to ContextBundle for PromptBuilder consumption.
    ///
    /// Injection mode mapping (security-relevant):
    /// - TaskDescription -> SystemPrompt (trusted, system-generated)
    /// - PredecessorOutput -> UserMessage { trust: Untrusted } (agent output, potential injection)
    /// - ConversationSummary -> UserMessage { trust: Untrusted } (user-derived)
    /// - Memory -> UserMessage { trust: Untrusted } (retrieved content)
    /// - WorkspaceArtifact -> UserMessage { trust: Untrusted } (external content)
    /// - UserProfile -> SystemMessage (loaded from config, trusted)
    pub fn to_bundle(&self) -> ContextBundle {
        ContextBundle {
            sections: self
                .sections
                .iter()
                .map(|s| {
                    let injection = match s.kind {
                        PackageSectionKind::TaskDescription => InjectionMode::SystemPrompt,
                        PackageSectionKind::UserProfile => InjectionMode::SystemMessage,
                        PackageSectionKind::PredecessorOutput => InjectionMode::UserMessage {
                            tag: "predecessor_output".to_string(),
                            trust: TrustLevel::Untrusted,
                        },
                        PackageSectionKind::ConversationSummary => InjectionMode::UserMessage {
                            tag: "conversation_summary".to_string(),
                            trust: TrustLevel::Untrusted,
                        },
                        PackageSectionKind::Memory => InjectionMode::UserMessage {
                            tag: "retrieved_memory".to_string(),
                            trust: TrustLevel::Untrusted,
                        },
                        PackageSectionKind::WorkspaceArtifact => InjectionMode::UserMessage {
                            tag: "workspace_artifact".to_string(),
                            trust: TrustLevel::Untrusted,
                        },
                    };
                    ContextSection {
                        source: s.kind.source_name(),
                        kind: s.kind.to_context_kind(),
                        content: s.content.clone(),
                        token_estimate: s.token_estimate,
                        priority: s.priority,
                        relevance: 1.0,
                        key: ContextKey::Package(s.kind.key_name()),
                        injection,
                    }
                })
                .collect(),
            total_tokens: self.total_tokens,
            available_budget: self.budget,
        }
    }
}

/// Summary of an agent (used in HandoffContext).
#[derive(Debug, Clone)]
pub struct AgentSummary {
    pub name: String,
    pub role: String,
    pub step: usize,
}

/// Structured context from a pipeline predecessor or DAG dependency.
#[derive(Debug, Clone)]
pub struct HandoffContext {
    /// Which agent produced this
    pub producer: AgentSummary,
    /// The task that was assigned
    pub task_assigned: String,
    /// The agent's output
    pub output: String,
    /// Key decisions or findings
    pub decisions: Vec<String>,
    /// What this agent flagged for the next agent
    pub handoff_notes: Option<String>,
}

impl HandoffContext {
    /// Format as a structured text block for prompt injection.
    pub fn format(&self) -> String {
        let mut result = format!(
            "<predecessor_output agent=\"{}\" role=\"{}\" step=\"{}\">\n",
            self.producer.name, self.producer.role, self.producer.step
        );
        result.push_str(&format!("Task: {}\n\n", self.task_assigned));
        result.push_str(&format!("Output:\n{}\n", self.output));

        if !self.decisions.is_empty() {
            result.push_str("\nKey decisions:\n");
            for decision in &self.decisions {
                result.push_str(&format!("- {}\n", decision));
            }
        }

        if let Some(ref notes) = self.handoff_notes {
            result.push_str(&format!("\nHandoff notes: {}\n", notes));
        }

        result.push_str("</predecessor_output>");
        result
    }

    /// Estimate token cost of the formatted output.
    pub fn token_estimate(&self) -> usize {
        self.format().len() / 4
    }

    /// Merge multiple predecessor handoffs (for DAG nodes with multiple dependencies).
    pub fn merge(handoffs: &[HandoffContext]) -> HandoffContext {
        let mut merged_output = String::new();
        let mut merged_decisions = Vec::new();
        let mut merged_notes = Vec::new();
        let mut names = Vec::new();

        for handoff in handoffs {
            names.push(handoff.producer.name.clone());
            merged_output.push_str(&handoff.format());
            merged_output.push('\n');
            merged_decisions.extend(
                handoff
                    .decisions
                    .iter()
                    .map(|d| format!("[{}] {}", handoff.producer.name, d)),
            );
            if let Some(ref notes) = handoff.handoff_notes {
                merged_notes.push(format!("[{}] {}", handoff.producer.name, notes));
            }
        }

        HandoffContext {
            producer: AgentSummary {
                name: names.join(", "),
                role: "merged predecessors".to_string(),
                step: handoffs.last().map_or(0, |h| h.producer.step),
            },
            task_assigned: "Merged context from multiple predecessors".to_string(),
            output: merged_output,
            decisions: merged_decisions,
            handoff_notes: if merged_notes.is_empty() {
                None
            } else {
                Some(merged_notes.join("\n"))
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_bundle_injection_modes() {
        let package = ContextPackage {
            sections: vec![
                PackageSection {
                    kind: PackageSectionKind::TaskDescription,
                    content: "Analyze logs".to_string(),
                    token_estimate: 3,
                    priority: SectionPriority::Critical,
                },
                PackageSection {
                    kind: PackageSectionKind::UserProfile,
                    content: "Prefers verbose".to_string(),
                    token_estimate: 4,
                    priority: SectionPriority::Low,
                },
                PackageSection {
                    kind: PackageSectionKind::PredecessorOutput,
                    content: "Found error in auth.rs".to_string(),
                    token_estimate: 6,
                    priority: SectionPriority::High,
                },
                PackageSection {
                    kind: PackageSectionKind::Memory,
                    content: "User prefers Rust".to_string(),
                    token_estimate: 4,
                    priority: SectionPriority::Normal,
                },
                PackageSection {
                    kind: PackageSectionKind::ConversationSummary,
                    content: "Discussed auth bug".to_string(),
                    token_estimate: 4,
                    priority: SectionPriority::High,
                },
                PackageSection {
                    kind: PackageSectionKind::WorkspaceArtifact,
                    content: "Previous agent output".to_string(),
                    token_estimate: 5,
                    priority: SectionPriority::Normal,
                },
            ],
            total_tokens: 26,
            budget: 100,
            sub_agent_window: 200_000,
        };

        let bundle = package.to_bundle();
        assert_eq!(bundle.sections.len(), 6);
        assert_eq!(bundle.total_tokens, 26);
        assert_eq!(bundle.available_budget, 100);

        // TaskDescription -> SystemPrompt
        assert!(matches!(
            bundle.sections[0].injection,
            InjectionMode::SystemPrompt
        ));
        // UserProfile -> SystemMessage
        assert!(matches!(
            bundle.sections[1].injection,
            InjectionMode::SystemMessage
        ));
        // PredecessorOutput -> UserMessage { Untrusted }
        assert!(matches!(
            &bundle.sections[2].injection,
            InjectionMode::UserMessage { tag, trust }
            if tag == "predecessor_output" && *trust == TrustLevel::Untrusted
        ));
        // Memory -> UserMessage { Untrusted }
        assert!(matches!(
            &bundle.sections[3].injection,
            InjectionMode::UserMessage { tag, trust }
            if tag == "retrieved_memory" && *trust == TrustLevel::Untrusted
        ));
        // ConversationSummary -> UserMessage { Untrusted }
        assert!(matches!(
            &bundle.sections[4].injection,
            InjectionMode::UserMessage { tag, trust }
            if tag == "conversation_summary" && *trust == TrustLevel::Untrusted
        ));
        // WorkspaceArtifact -> UserMessage { Untrusted }
        assert!(matches!(
            &bundle.sections[5].injection,
            InjectionMode::UserMessage { tag, trust }
            if tag == "workspace_artifact" && *trust == TrustLevel::Untrusted
        ));
    }

    #[test]
    fn test_handoff_context_format() {
        let handoff = HandoffContext {
            producer: AgentSummary {
                name: "analyzer".to_string(),
                role: "code_analysis".to_string(),
                step: 1,
            },
            task_assigned: "Analyze auth module".to_string(),
            output: "Found NPE in auth.rs line 42".to_string(),
            decisions: vec!["Root cause is null check".to_string()],
            handoff_notes: Some("Next agent should fix the NPE".to_string()),
        };

        let formatted = handoff.format();
        assert!(formatted.contains("<predecessor_output"));
        assert!(formatted.contains("agent=\"analyzer\""));
        assert!(formatted.contains("role=\"code_analysis\""));
        assert!(formatted.contains("step=\"1\""));
        assert!(formatted.contains("Analyze auth module"));
        assert!(formatted.contains("Found NPE in auth.rs line 42"));
        assert!(formatted.contains("Root cause is null check"));
        assert!(formatted.contains("Next agent should fix the NPE"));
        assert!(formatted.contains("</predecessor_output>"));
    }

    #[test]
    fn test_handoff_context_merge() {
        let h1 = HandoffContext {
            producer: AgentSummary {
                name: "agent_a".to_string(),
                role: "analyzer".to_string(),
                step: 1,
            },
            task_assigned: "Task A".to_string(),
            output: "Output A".to_string(),
            decisions: vec!["Decision A".to_string()],
            handoff_notes: Some("Note A".to_string()),
        };
        let h2 = HandoffContext {
            producer: AgentSummary {
                name: "agent_b".to_string(),
                role: "researcher".to_string(),
                step: 2,
            },
            task_assigned: "Task B".to_string(),
            output: "Output B".to_string(),
            decisions: vec!["Decision B".to_string()],
            handoff_notes: None,
        };

        let merged = HandoffContext::merge(&[h1, h2]);

        // Producer attribution preserved
        assert!(merged.output.contains("agent=\"agent_a\""));
        assert!(merged.output.contains("agent=\"agent_b\""));
        // All predecessors' content appears
        assert!(merged.output.contains("Output A"));
        assert!(merged.output.contains("Output B"));
        // Decisions attributed
        assert_eq!(merged.decisions.len(), 2);
        assert!(merged.decisions[0].contains("[agent_a]"));
        assert!(merged.decisions[1].contains("[agent_b]"));
        // Notes merged
        assert!(merged
            .handoff_notes
            .as_ref()
            .unwrap()
            .contains("[agent_a] Note A"));
    }

    #[test]
    fn test_handoff_context_token_estimate() {
        let handoff = HandoffContext {
            producer: AgentSummary {
                name: "test".to_string(),
                role: "test".to_string(),
                step: 1,
            },
            task_assigned: "test task".to_string(),
            output: "test output".to_string(),
            decisions: vec![],
            handoff_notes: None,
        };
        let estimate = handoff.token_estimate();
        assert!(estimate > 0);
        assert_eq!(estimate, handoff.format().len() / 4);
    }

    #[test]
    fn test_package_section_kind_from_context_kind() {
        assert_eq!(
            PackageSectionKind::from(&ContextKind::Memory),
            PackageSectionKind::Memory
        );
        assert_eq!(
            PackageSectionKind::from(&ContextKind::ConversationSummary),
            PackageSectionKind::ConversationSummary
        );
        assert_eq!(
            PackageSectionKind::from(&ContextKind::UserProfile),
            PackageSectionKind::UserProfile
        );
        assert_eq!(
            PackageSectionKind::from(&ContextKind::WorkspaceArtifact),
            PackageSectionKind::WorkspaceArtifact
        );
        // SkillReference maps to WorkspaceArtifact as fallback
        assert_eq!(
            PackageSectionKind::from(&ContextKind::SkillReference),
            PackageSectionKind::WorkspaceArtifact
        );
    }

    #[test]
    fn test_merge_no_handoff_notes() {
        let h1 = HandoffContext {
            producer: AgentSummary {
                name: "a".to_string(),
                role: "r".to_string(),
                step: 1,
            },
            task_assigned: "t1".to_string(),
            output: "o1".to_string(),
            decisions: vec![],
            handoff_notes: None,
        };
        let h2 = HandoffContext {
            producer: AgentSummary {
                name: "b".to_string(),
                role: "r".to_string(),
                step: 2,
            },
            task_assigned: "t2".to_string(),
            output: "o2".to_string(),
            decisions: vec![],
            handoff_notes: None,
        };
        let merged = HandoffContext::merge(&[h1, h2]);
        assert!(merged.handoff_notes.is_none());
    }
}
