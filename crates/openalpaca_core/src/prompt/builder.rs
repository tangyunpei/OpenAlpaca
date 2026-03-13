//! PromptBuilder — assembles a structured prompt from registered sections,
//! then trims to fit within the model's context window budget.

use crate::middleware::prompt::{
    AgentPersona, SystemPersona, format_connector_guidance, format_message_source,
    format_tool_guidance,
};
use crate::prompt_ctx::{ContextBundle, InjectionMode, SectionPriority, TrustLevel};
use openalpaca_llm::{ChatMessage, ToolDefinition};

// ── Token estimation ─────────────────────────────────────────────────────────

/// Rough token estimate: 1 token ≈ 4 chars (industry heuristic).
fn estimate_tokens(s: &str) -> usize {
    s.len() / 4
}

// ── SectionTarget ────────────────────────────────────────────────────────────

/// Where a section's content is injected in the final prompt.
#[derive(Debug, Clone)]
enum SectionTarget {
    /// Appended to the system prompt string.
    SystemPromptBlock,
    /// Emitted as an independent `ChatMessage` in `context_messages`.
    Message(ChatMessage),
}

// ── PromptSection ─────────────────────────────────────────────────────────────

/// An individual named section tracked by `PromptBuilder`.
#[derive(Debug, Clone)]
struct PromptSection {
    /// Stable name used in `section_registry` and for diagnostics.
    name: &'static str,
    /// The text content of this section.
    content: String,
    /// Token budget consumed by this section.
    tokens: usize,
    /// How important this section is during trimming.
    priority: SectionPriority,
    /// Where this section ends up in the assembled prompt.
    target: SectionTarget,
}

impl PromptSection {
    fn new(
        name: &'static str,
        content: String,
        priority: SectionPriority,
        target: SectionTarget,
    ) -> Self {
        let tokens = estimate_tokens(&content);
        Self {
            name,
            content,
            tokens,
            priority,
            target,
        }
    }
}

// ── BuiltPrompt ───────────────────────────────────────────────────────────────

/// The assembled output of `PromptBuilder::build()`.
#[derive(Debug, Clone)]
pub struct BuiltPrompt {
    /// Combined system prompt string (all `SystemPromptBlock` sections).
    pub system_message: String,
    /// Ordered context messages (all `Message` sections), inserted before the
    /// conversation history in the LLM request.
    pub context_messages: Vec<ChatMessage>,
    /// All section names that were registered, in registration order.
    /// Sections that were trimmed away are still listed here.
    pub section_registry: Vec<&'static str>,
    /// Total tokens estimated across all sections that made it into the prompt.
    pub total_prompt_tokens: usize,
    /// How many tokens remain for the live conversation (history + reply).
    pub remaining_for_conversation: usize,
}

// ── PromptBuilder ─────────────────────────────────────────────────────────────

/// Builds a prompt from registered sections with adaptive token-budget trimming.
///
/// Call the registration methods in any order, then call `build()`.
pub struct PromptBuilder {
    model_context_window: usize,
    sections: Vec<PromptSection>,
}

impl PromptBuilder {
    /// Create a new builder for a model with the given context-window size (in tokens).
    pub fn new(model_context_window: usize) -> Self {
        Self {
            model_context_window,
            sections: Vec::new(),
        }
    }

    // ── Registration helpers ─────────────────────────────────────────────────

    fn push(&mut self, name: &'static str, content: String, priority: SectionPriority, target: SectionTarget) {
        if !content.is_empty() {
            self.sections.push(PromptSection::new(name, content, priority, target));
        }
    }

    fn push_sys(&mut self, name: &'static str, content: String, priority: SectionPriority) {
        self.push(name, content, priority, SectionTarget::SystemPromptBlock);
    }

    // ── Public section registration methods ─────────────────────────────────

    /// Register the immutable system persona (`<system_instructions>` block).
    pub fn system_persona(mut self, persona: &SystemPersona) -> Self {
        let mut block = String::new();
        block.push_str("<system_instructions>\n");
        block.push_str(&format!("Identity: {}\n", persona.name));
        block.push_str("Core Values:\n");
        for v in &persona.core_values {
            block.push_str(&format!("- {}\n", v));
        }
        block.push_str("Safety Rules:\n");
        for r in &persona.safety_rules {
            block.push_str(&format!("- {}\n", r));
        }
        block.push_str(&format!("Base Instructions: {}\n", persona.base_instructions));
        block.push_str("</system_instructions>\n");
        self.push_sys("system_persona", block, SectionPriority::Critical);
        self
    }

    /// Register the mutable agent persona (`<agent_role>` block).
    pub fn agent_persona(mut self, persona: &AgentPersona) -> Self {
        let mut block = String::new();
        block.push_str("<agent_role>\n");
        block.push_str(&format!("Role: {}\n", persona.role));
        block.push_str(&format!("Tone: {}\n", persona.tone));
        if !persona.domain_knowledge.is_empty() {
            block.push_str("Domain Knowledge:\n");
            for d in &persona.domain_knowledge {
                block.push_str(&format!("- {}\n", d));
            }
        }
        block.push_str("</agent_role>\n");
        self.push_sys("agent_persona", block, SectionPriority::High);
        self
    }

    /// Register the agent identity document.
    pub fn identity(mut self, content: &str) -> Self {
        self.push_sys("identity", content.to_string(), SectionPriority::High);
        self
    }

    /// Register the bootstrap / orientation document.
    pub fn bootstrap(mut self, content: &str) -> Self {
        self.push_sys("bootstrap", content.to_string(), SectionPriority::Optional);
        self
    }

    /// Register the skills catalog block.
    pub fn skills_catalog(mut self, content: &str) -> Self {
        self.push_sys("skills_catalog", content.to_string(), SectionPriority::Normal);
        self
    }

    /// Register available tools as a system-prompt guidance block.
    pub fn tools(mut self, tools: &[ToolDefinition]) -> Self {
        let content = format_tool_guidance(tools);
        self.push_sys("tools", content, SectionPriority::Normal);
        self
    }

    /// Register the connector status block.
    pub fn connector_guidance(
        mut self,
        statuses: &[(String, String)],
        sendable_channels: Option<&[String]>,
    ) -> Self {
        let content = format_connector_guidance(statuses, sendable_channels);
        self.push_sys("connector_guidance", content, SectionPriority::Low);
        self
    }

    /// Register the message-source block.
    pub fn message_source(mut self, source: &str) -> Self {
        let content = format_message_source(source);
        self.push_sys("message_source", content, SectionPriority::Low);
        self
    }

    /// Register all sections from a `ContextBundle`, mapping each section's
    /// `InjectionMode` to the appropriate `SectionTarget`.
    pub fn context_bundle(mut self, bundle: &ContextBundle) -> Self {
        for section in &bundle.sections {
            let target = match &section.injection {
                InjectionMode::SystemPrompt => SectionTarget::SystemPromptBlock,
                InjectionMode::SystemMessage => {
                    SectionTarget::Message(ChatMessage::system(&section.content))
                }
                InjectionMode::UserMessage { tag, trust } => {
                    let msg_content = match trust {
                        TrustLevel::Untrusted => {
                            crate::orchestrator::wrap_untrusted_context(
                                &section.content,
                                tag,
                                "retrieved",
                            )
                        }
                        TrustLevel::Trusted => section.content.clone(),
                    };
                    SectionTarget::Message(ChatMessage::user(&msg_content))
                }
            };

            // Use a leaked static name derived from section.source.  Since
            // ContextSection.source is already `&'static str`, we can use it directly.
            let name: &'static str = section.source;
            if !section.content.is_empty() {
                self.sections.push(PromptSection {
                    name,
                    content: section.content.clone(),
                    tokens: section.token_estimate,
                    priority: section.priority,
                    target,
                });
            }
        }
        self
    }

    /// Register an arbitrary raw text block into the system prompt.
    pub fn raw_system_block(
        mut self,
        name: &'static str,
        content: &str,
        priority: SectionPriority,
    ) -> Self {
        self.push_sys(name, content.to_string(), priority);
        self
    }

    // ── Token accounting ─────────────────────────────────────────────────────

    /// Returns the sum of token estimates across all currently registered sections.
    pub fn estimate_static_tokens(&self) -> usize {
        self.sections.iter().map(|s| s.tokens).sum()
    }

    // ── Build ────────────────────────────────────────────────────────────────

    /// Trim sections and assemble the final `BuiltPrompt`.
    ///
    /// The total prompt is capped at 75 % of `model_context_window`.  Within
    /// that budget, trimming happens in four phases (cheapest first):
    ///   1. Drop all `Optional` sections.
    ///   2. Drop all `Low` sections.
    ///   3. Proportionally shrink `Normal` sections.
    ///   4. Proportionally shrink `High` sections.
    ///
    /// `Critical` sections are never touched.
    pub fn build(mut self) -> BuiltPrompt {
        // Budget: 75 % of the context window.
        let budget = (self.model_context_window * 3) / 4;
        self.trim_to_budget(budget);

        // Collect registry names (all registered, including dropped ones).
        let section_registry: Vec<&'static str> = self.sections.iter().map(|s| s.name).collect();

        // Assemble system prompt and context messages.
        let mut system_parts: Vec<String> = Vec::new();
        let mut context_messages: Vec<ChatMessage> = Vec::new();

        for section in &self.sections {
            match &section.target {
                SectionTarget::SystemPromptBlock => {
                    system_parts.push(section.content.clone());
                }
                SectionTarget::Message(msg) => {
                    context_messages.push(msg.clone());
                }
            }
        }

        let system_message = system_parts.join("\n");
        let total_prompt_tokens: usize = self.sections.iter().map(|s| s.tokens).sum();
        let remaining_for_conversation = budget.saturating_sub(total_prompt_tokens);

        BuiltPrompt {
            system_message,
            context_messages,
            section_registry,
            total_prompt_tokens,
            remaining_for_conversation,
        }
    }

    // ── Trimming ─────────────────────────────────────────────────────────────

    /// Trim sections in four phases until total tokens fit within `budget`.
    fn trim_to_budget(&mut self, budget: usize) {
        if self.estimate_static_tokens() <= budget {
            return;
        }

        // Phase 1: drop Optional sections entirely.
        self.drop_priority(SectionPriority::Optional);
        if self.estimate_static_tokens() <= budget {
            return;
        }

        // Phase 2: drop Low sections entirely.
        self.drop_priority(SectionPriority::Low);
        if self.estimate_static_tokens() <= budget {
            return;
        }

        // Phase 3: proportionally shrink Normal sections.
        self.shrink_priority(SectionPriority::Normal, budget);
        if self.estimate_static_tokens() <= budget {
            return;
        }

        // Phase 4: proportionally shrink High sections.
        self.shrink_priority(SectionPriority::High, budget);
    }

    /// Remove all sections with the given priority.
    fn drop_priority(&mut self, priority: SectionPriority) {
        self.sections.retain(|s| s.priority != priority);
    }

    /// Proportionally shrink sections of the given priority to reclaim tokens.
    fn shrink_priority(&mut self, priority: SectionPriority, budget: usize) {
        let current_total: usize = self.estimate_static_tokens();
        if current_total <= budget {
            return;
        }

        let overage = current_total - budget;
        let priority_tokens: usize = self
            .sections
            .iter()
            .filter(|s| s.priority == priority)
            .map(|s| s.tokens)
            .sum();

        if priority_tokens == 0 {
            return;
        }

        // How many chars to cut across all sections of this priority.
        let chars_to_cut = overage * 4; // tokens × 4 chars/token

        for section in self.sections.iter_mut().filter(|s| s.priority == priority) {
            // Fraction of chars this section contributes.
            let section_chars = section.content.len();
            let cut = (section_chars * chars_to_cut).div_ceil(priority_tokens * 4);
            let new_len = section_chars.saturating_sub(cut);
            Self::truncate_section(section, new_len);
        }
    }

    /// Truncate `section.content` to at most `max_chars` bytes (honoring UTF-8
    /// char boundaries) and update `section.tokens` and the inner `ChatMessage`
    /// content (if the target is a `Message`).
    fn truncate_section(section: &mut PromptSection, max_chars: usize) {
        if section.content.len() <= max_chars {
            return;
        }

        let boundary = section.content.floor_char_boundary(max_chars);
        section.content.truncate(boundary);
        section.tokens = estimate_tokens(&section.content);

        // Keep the ChatMessage content in sync.
        match &section.target {
            SectionTarget::Message(msg) => {
                let new_msg = ChatMessage {
                    role: msg.role.clone(),
                    content: section.content.clone(),
                    ..msg.clone()
                };
                section.target = SectionTarget::Message(new_msg);
            }
            SectionTarget::SystemPromptBlock => {}
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::middleware::prompt::{AgentPersona, SystemPersona};
    use crate::prompt_ctx::section::{
        ContextBundle, ContextKey, ContextKind, ContextSection, InjectionMode, SectionPriority,
        TrustLevel,
    };

    // ── Task 2 tests ─────────────────────────────────────────────────────────

    #[test]
    fn test_build_basic_prompt() {
        let system = SystemPersona::default();
        let agent = AgentPersona {
            role: "Researcher".to_string(),
            tone: "Analytical".to_string(),
            domain_knowledge: vec!["Rust".to_string()],
        };

        let built = PromptBuilder::new(200_000)
            .system_persona(&system)
            .agent_persona(&agent)
            .build();

        assert!(
            built.system_message.contains("<system_instructions>"),
            "system_message should contain <system_instructions>"
        );
        assert!(
            built.system_message.contains("<agent_role>"),
            "system_message should contain <agent_role>"
        );
    }

    #[test]
    fn test_estimate_static_tokens() {
        let system = SystemPersona::default();
        let agent = AgentPersona {
            role: "Researcher".to_string(),
            tone: "Analytical".to_string(),
            domain_knowledge: vec![],
        };

        let builder = PromptBuilder::new(200_000)
            .system_persona(&system)
            .agent_persona(&agent);

        let tokens = builder.estimate_static_tokens();
        assert!(tokens > 0, "token estimate should be > 0, got {}", tokens);
        assert!(tokens < 1000, "token estimate should be < 1000, got {}", tokens);
    }

    #[test]
    fn test_context_messages_from_bundle() {
        let section = ContextSection {
            source: "test_memory",
            kind: ContextKind::Memory,
            content: "remembered fact about the user".to_string(),
            token_estimate: 10,
            priority: SectionPriority::Normal,
            relevance: 0.9,
            key: ContextKey::Memory(1),
            injection: InjectionMode::UserMessage {
                tag: "memory".to_string(),
                trust: TrustLevel::Untrusted,
            },
        };
        let bundle = ContextBundle {
            sections: vec![section],
            total_tokens: 10,
            available_budget: 1000,
        };

        let built = PromptBuilder::new(200_000).context_bundle(&bundle).build();

        assert_eq!(built.context_messages.len(), 1, "should have 1 context message");
        let msg = &built.context_messages[0];
        assert!(
            msg.content.contains("remembered fact"),
            "context message should contain the original content, got: {}",
            msg.content
        );
    }

    // ── Task 3 tests ─────────────────────────────────────────────────────────

    #[test]
    fn test_trimming_drops_optional_first() {
        // Budget: 100 tokens.
        // system_persona default is ~125 tokens (Critical) — will survive.
        // bootstrap is Optional — should be dropped when over budget.
        let system = SystemPersona::default();
        let bootstrap_text = "bootstrap orientation document ".repeat(10); // ~70 chars → ~17 tokens

        let built = PromptBuilder::new(100)
            .system_persona(&system)  // Critical — never dropped
            .bootstrap(&bootstrap_text)  // Optional — first to go
            .build();

        assert!(
            built.system_message.contains("<system_instructions>"),
            "Critical section must survive trimming"
        );
        assert!(
            !built.system_message.contains("bootstrap orientation document"),
            "Optional section must be dropped when over budget"
        );
    }

    #[test]
    fn test_section_registry_tracks_all_sections() {
        let system = SystemPersona::default();
        let agent = AgentPersona {
            role: "Helper".to_string(),
            tone: "Friendly".to_string(),
            domain_knowledge: vec![],
        };

        let built = PromptBuilder::new(200_000)
            .system_persona(&system)
            .agent_persona(&agent)
            .identity("I am the OpenAlpaca identity document.")
            .build();

        assert!(
            built.section_registry.contains(&"system_persona"),
            "registry must contain system_persona"
        );
        assert!(
            built.section_registry.contains(&"agent_persona"),
            "registry must contain agent_persona"
        );
        assert!(
            built.section_registry.contains(&"identity"),
            "registry must contain identity"
        );
    }
}
