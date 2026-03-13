# Prompt & Context Management v2 — Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Unify prompt assembly and context management into a two-layer system (ContextManager + PromptBuilder) with budget-aware resolution, graduated compaction, and inter-agent context flow.

**Architecture:** ContextManager resolves, scores, and budgets context from 5 sources (memory, conversation, skill, workspace, user profile). PromptBuilder assembles static sections (persona, identity, tools) + dynamic sections (from ContextBundle) with pre-flight budget enforcement. Graduated compaction replaces binary compression with 5 tiers. Sub-agents receive distilled context packages adapted to their model window.

**Tech Stack:** Rust (tokio, async_trait, serde, futures_util), SQLite (rusqlite), existing openalpaca_llm/openalpaca_storage crates.

**Spec:** `docs/superpowers/specs/2026-03-13-prompt-context-management-v2-design.md`

**Naming deviation:** The spec uses `context/` as the new module name, but `lib.rs` already has `pub mod context;` (for `SharedContext`). This plan uses `prompt_ctx/` instead to avoid the conflict. The `prompt/` module is used as-is.

**API deviation:** `PromptBuilder::identity()` and `bootstrap()` take `&str` (pre-rendered text) instead of the spec's `&IdentityDocument` / `&BootstrapDocument`. Rationale: the builder doesn't need to know about document types — callers render to string before calling. This keeps the builder decoupled from document internals.

---

## Chunk 1: Phase 1 — Foundation (No Behavior Change)

Create the core types and PromptBuilder. No call sites changed, no behavior change.

### Task 1: Create `prompt_ctx` module with core types

**Files:**
- Create: `crates/openalpaca_core/src/prompt_ctx/mod.rs`
- Create: `crates/openalpaca_core/src/prompt_ctx/section.rs`
- Modify: `crates/openalpaca_core/src/lib.rs` (add `pub mod prompt_ctx;`)

- [ ] **Step 1: Create `section.rs` with core types**

```rust
// crates/openalpaca_core/src/prompt_ctx/section.rs

/// Ordering: Optional(0) < Low(1) < Normal(2) < High(3) < Critical(4).
/// Use `b.priority.cmp(&a.priority)` for highest-first sorting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SectionPriority {
    Optional,
    Low,
    Normal,
    High,
    Critical,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ContextKind {
    Memory,
    ConversationSummary,
    SkillReference,
    WorkspaceArtifact,
    UserProfile,
}

impl ContextKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::ConversationSummary => "conversation_summary",
            Self::SkillReference => "skill_reference",
            Self::WorkspaceArtifact => "workspace_artifact",
            Self::UserProfile => "user_profile",
        }
    }
}

#[derive(Debug, Clone)]
pub enum InjectionMode {
    SystemPrompt,
    SystemMessage,
    UserMessage { tag: String, trust: TrustLevel },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrustLevel {
    Trusted,
    Untrusted,
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum ContextKey {
    Memory(i64),
    ConversationSummary,
    SkillSource(String, String),
    WorkspaceArtifact(String),
    UserProfile,
    Package(String),
}

#[derive(Debug, Clone)]
pub struct ContextSection {
    pub source: &'static str,
    pub kind: ContextKind,
    pub content: String,
    pub token_estimate: usize,
    pub priority: SectionPriority,
    pub relevance: f32,
    pub key: ContextKey,
    pub injection: InjectionMode,
}

#[derive(Debug, Clone)]
pub struct ContextBundle {
    pub sections: Vec<ContextSection>,
    pub total_tokens: usize,
    pub available_budget: usize,
}

impl ContextBundle {
    pub fn empty() -> Self {
        Self {
            sections: Vec::new(),
            total_tokens: 0,
            available_budget: 0,
        }
    }
}
```

- [ ] **Step 2: Create `mod.rs` re-exporting types**

```rust
// crates/openalpaca_core/src/prompt_ctx/mod.rs
pub mod section;

pub use section::{
    ContextBundle, ContextKey, ContextKind, ContextSection,
    InjectionMode, SectionPriority, TrustLevel,
};
```

- [ ] **Step 3: Register module in `lib.rs`**

Add `pub mod prompt_ctx;` to `crates/openalpaca_core/src/lib.rs` (after the existing `pub mod context_budget;` line).

- [ ] **Step 4: Run `cargo check -p openalpaca_core`**

Expected: compiles clean.

- [ ] **Step 5: Commit**

```bash
git add crates/openalpaca_core/src/prompt_ctx/ crates/openalpaca_core/src/lib.rs
git commit -m "feat(prompt_ctx): add core types — SectionPriority, ContextSection, ContextBundle"
```

---

### Task 2: Create PromptBuilder with section registration

**Files:**
- Create: `crates/openalpaca_core/src/prompt/mod.rs`
- Create: `crates/openalpaca_core/src/prompt/builder.rs`
- Modify: `crates/openalpaca_core/src/lib.rs` (add `pub mod prompt;`)

- [ ] **Step 1: Write failing test for PromptBuilder basics**

Create `crates/openalpaca_core/src/prompt/builder.rs` with the test at the bottom:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::middleware::prompt::{AgentPersona, SystemPersona};

    #[test]
    fn test_build_basic_prompt() {
        let mut builder = PromptBuilder::new(200_000);
        let persona = SystemPersona::default();
        let agent = AgentPersona {
            role: "Assistant".to_string(),
            tone: "Concise".to_string(),
            domain_knowledge: vec![],
        };
        builder.system_persona(&persona).agent_persona(&agent);
        let built = builder.build();

        assert!(built.system_message.contains("<system_instructions>"));
        assert!(built.system_message.contains("<agent_role>"));
        assert!(built.total_prompt_tokens > 0);
        assert!(built.remaining_for_conversation > 0);
        assert!(!built.section_registry.is_empty());
    }

    #[test]
    fn test_estimate_static_tokens() {
        let mut builder = PromptBuilder::new(200_000);
        let persona = SystemPersona::default();
        builder.system_persona(&persona);
        let estimate = builder.estimate_static_tokens();
        assert!(estimate > 0);
        assert!(estimate < 1000); // persona is small
    }

    #[test]
    fn test_context_messages_from_bundle() {
        use crate::prompt_ctx::*;
        let mut builder = PromptBuilder::new(200_000);
        let persona = SystemPersona::default();
        builder.system_persona(&persona);

        let bundle = ContextBundle {
            sections: vec![ContextSection {
                source: "test",
                kind: ContextKind::Memory,
                content: "remembered fact".to_string(),
                token_estimate: 10,
                priority: SectionPriority::Normal,
                relevance: 0.9,
                key: ContextKey::Memory(1),
                injection: InjectionMode::UserMessage {
                    tag: "memory".to_string(),
                    trust: TrustLevel::Untrusted,
                },
            }],
            total_tokens: 10,
            available_budget: 50000,
        };
        builder.context_bundle(&bundle);
        let built = builder.build();

        assert_eq!(built.context_messages.len(), 1);
        assert!(built.context_messages[0].content.contains("remembered fact"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p openalpaca_core prompt::builder::tests --no-run 2>&1 | head -20`
Expected: compilation error — `PromptBuilder` not defined.

- [ ] **Step 3: Implement PromptBuilder**

Write the implementation above the tests in `builder.rs`:

```rust
use crate::middleware::prompt::{
    format_connector_guidance, format_message_source, format_tool_guidance,
    AgentPersona, SystemPersona,
};
use crate::prompt_ctx::{ContextBundle, InjectionMode, SectionPriority, TrustLevel};
use openalpaca_llm::{ChatMessage, ToolDefinition};

#[derive(Debug)]
struct PromptSection {
    name: &'static str,
    content: String,
    token_estimate: usize,
    priority: SectionPriority,
    target: SectionTarget,
}

#[derive(Debug)]
enum SectionTarget {
    SystemPromptBlock,
    Message(ChatMessage),
}

pub struct PromptBuilder {
    model_context_window: usize,
    sections: Vec<PromptSection>,
}

pub struct BuiltPrompt {
    pub system_message: String,
    pub context_messages: Vec<ChatMessage>,
    pub section_registry: Vec<(&'static str, usize)>,
    pub total_prompt_tokens: usize,
    pub remaining_for_conversation: usize,
}

impl PromptBuilder {
    pub fn new(model_context_window: usize) -> Self {
        Self {
            model_context_window,
            sections: Vec::new(),
        }
    }

    pub fn system_persona(&mut self, persona: &SystemPersona) -> &mut Self {
        let content = format!(
            "<system_instructions>\nIdentity: {}\nCore Values:\n{}\nSafety Rules:\n{}\nBase Instructions: {}\n</system_instructions>\n",
            persona.name,
            persona.core_values.iter().map(|v| format!("- {v}")).collect::<Vec<_>>().join("\n"),
            persona.safety_rules.iter().map(|r| format!("- {r}")).collect::<Vec<_>>().join("\n"),
            persona.base_instructions,
        );
        let token_estimate = content.len() / 4;
        self.sections.push(PromptSection {
            name: "system_persona",
            content,
            token_estimate,
            priority: SectionPriority::Critical,
            target: SectionTarget::SystemPromptBlock,
        });
        self
    }

    pub fn agent_persona(&mut self, persona: &AgentPersona) -> &mut Self {
        let mut content = format!("<agent_role>\nRole: {}\nTone: {}\n", persona.role, persona.tone);
        if !persona.domain_knowledge.is_empty() {
            content.push_str("Domain Knowledge:\n");
            for d in &persona.domain_knowledge {
                content.push_str(&format!("- {d}\n"));
            }
        }
        content.push_str("</agent_role>\n");
        let token_estimate = content.len() / 4;
        self.sections.push(PromptSection {
            name: "agent_persona",
            content,
            token_estimate,
            priority: SectionPriority::Critical,
            target: SectionTarget::SystemPromptBlock,
        });
        self
    }

    pub fn identity(&mut self, block: &str) -> &mut Self {
        if !block.is_empty() {
            let token_estimate = block.len() / 4;
            self.sections.push(PromptSection {
                name: "identity",
                content: block.to_string(),
                token_estimate,
                priority: SectionPriority::High,
                target: SectionTarget::SystemPromptBlock,
            });
        }
        self
    }

    pub fn bootstrap(&mut self, block: &str) -> &mut Self {
        if !block.is_empty() {
            let token_estimate = block.len() / 4;
            self.sections.push(PromptSection {
                name: "bootstrap",
                content: block.to_string(),
                token_estimate,
                priority: SectionPriority::Optional,
                target: SectionTarget::SystemPromptBlock,
            });
        }
        self
    }

    pub fn skills_catalog(&mut self, block: &str) -> &mut Self {
        if !block.is_empty() {
            let token_estimate = block.len() / 4;
            self.sections.push(PromptSection {
                name: "skills_catalog",
                content: block.to_string(),
                token_estimate,
                priority: SectionPriority::Optional,
                target: SectionTarget::SystemPromptBlock,
            });
        }
        self
    }

    pub fn tools(&mut self, tool_defs: &[ToolDefinition]) -> &mut Self {
        let block = format_tool_guidance(tool_defs);
        if !block.is_empty() {
            let token_estimate = block.len() / 4;
            self.sections.push(PromptSection {
                name: "tools",
                content: block,
                token_estimate,
                priority: SectionPriority::High,
                target: SectionTarget::SystemPromptBlock,
            });
        }
        self
    }

    pub fn connector_guidance(
        &mut self,
        statuses: &[(String, String)],
        sendable: Option<&[String]>,
    ) -> &mut Self {
        let block = format_connector_guidance(statuses, sendable);
        if !block.is_empty() {
            let token_estimate = block.len() / 4;
            self.sections.push(PromptSection {
                name: "connector_guidance",
                content: block,
                token_estimate,
                priority: SectionPriority::Normal,
                target: SectionTarget::SystemPromptBlock,
            });
        }
        self
    }

    pub fn message_source(&mut self, source: &str) -> &mut Self {
        let block = format_message_source(source);
        if !block.is_empty() {
            let token_estimate = block.len() / 4;
            self.sections.push(PromptSection {
                name: "message_source",
                content: block,
                token_estimate,
                priority: SectionPriority::Normal,
                target: SectionTarget::SystemPromptBlock,
            });
        }
        self
    }

    pub fn context_bundle(&mut self, bundle: &ContextBundle) -> &mut Self {
        for section in &bundle.sections {
            let target = match &section.injection {
                InjectionMode::SystemPrompt => {
                    SectionTarget::SystemPromptBlock
                }
                InjectionMode::SystemMessage => {
                    SectionTarget::Message(ChatMessage::system(&section.content))
                }
                InjectionMode::UserMessage { tag, trust } => {
                    let content = match trust {
                        TrustLevel::Trusted => section.content.clone(),
                        TrustLevel::Untrusted => {
                            crate::orchestrator::wrap_untrusted_context(
                                &section.content,
                                tag,
                                "retrieved",
                            )
                        }
                    };
                    SectionTarget::Message(ChatMessage::user(&content))
                }
            };
            self.sections.push(PromptSection {
                name: section.source,
                content: section.content.clone(),
                token_estimate: section.token_estimate,
                priority: section.priority,
                target,
            });
        }
        self
    }

    /// Raw block appended to system prompt (for path-specific content
    /// like send_context that doesn't fit the standard section types).
    pub fn raw_system_block(&mut self, name: &'static str, block: &str, priority: SectionPriority) -> &mut Self {
        if !block.is_empty() {
            let token_estimate = block.len() / 4;
            self.sections.push(PromptSection {
                name,
                content: block.to_string(),
                token_estimate,
                priority,
                target: SectionTarget::SystemPromptBlock,
            });
        }
        self
    }

    pub fn estimate_static_tokens(&self) -> usize {
        self.sections.iter().map(|s| s.token_estimate).sum()
    }

    pub fn build(mut self) -> BuiltPrompt {
        let max_prompt_tokens = (self.model_context_window as f64 * 0.75) as usize;
        let total: usize = self.sections.iter().map(|s| s.token_estimate).sum();
        if total > max_prompt_tokens {
            self.trim_to_budget(max_prompt_tokens);
        }

        let mut system_prompt = String::new();
        let mut context_messages = Vec::new();
        let mut section_registry = Vec::new();

        for section in &self.sections {
            section_registry.push((section.name, section.token_estimate));
            match &section.target {
                SectionTarget::SystemPromptBlock => {
                    system_prompt.push_str(&section.content);
                    system_prompt.push('\n');
                }
                SectionTarget::Message(msg) => {
                    context_messages.push(msg.clone());
                }
            }
        }

        let total_prompt_tokens: usize = self.sections.iter().map(|s| s.token_estimate).sum();

        BuiltPrompt {
            system_message: system_prompt,
            context_messages,
            section_registry,
            total_prompt_tokens,
            remaining_for_conversation: self.model_context_window.saturating_sub(total_prompt_tokens),
        }
    }

    fn trim_to_budget(&mut self, budget: usize) {
        let mut total: usize = self.sections.iter().map(|s| s.token_estimate).sum();

        // Phase 1: Drop Optional
        if total > budget {
            self.sections.retain(|s| {
                if s.priority == SectionPriority::Optional && total > budget {
                    total -= s.token_estimate;
                    false
                } else {
                    true
                }
            });
        }

        // Phase 2: Drop Low
        if total > budget {
            self.sections.retain(|s| {
                if s.priority == SectionPriority::Low && total > budget {
                    total -= s.token_estimate;
                    false
                } else {
                    true
                }
            });
        }

        // Phase 3: Proportionally truncate Normal
        if total > budget {
            let normal_tokens: usize = self.sections.iter()
                .filter(|s| s.priority == SectionPriority::Normal)
                .map(|s| s.token_estimate)
                .sum();
            let non_normal = total - normal_tokens;
            let normal_budget = budget.saturating_sub(non_normal);
            if normal_tokens > 0 {
                let ratio = normal_budget as f64 / normal_tokens as f64;
                for section in &mut self.sections {
                    if section.priority == SectionPriority::Normal {
                        let new_budget = (section.token_estimate as f64 * ratio) as usize;
                        Self::truncate_section(section, new_budget);
                    }
                }
            }
            total = self.sections.iter().map(|s| s.token_estimate).sum();
        }

        // Phase 4: Proportionally truncate High
        if total > budget {
            let high_tokens: usize = self.sections.iter()
                .filter(|s| s.priority == SectionPriority::High)
                .map(|s| s.token_estimate)
                .sum();
            let non_high = total - high_tokens;
            let high_budget = budget.saturating_sub(non_high);
            if high_tokens > 0 {
                let ratio = high_budget as f64 / high_tokens as f64;
                for section in &mut self.sections {
                    if section.priority == SectionPriority::High {
                        let new_budget = (section.token_estimate as f64 * ratio) as usize;
                        Self::truncate_section(section, new_budget);
                    }
                }
            }
        }

        // Critical sections are never trimmed
    }

    fn truncate_section(section: &mut PromptSection, max_tokens: usize) {
        let max_chars = max_tokens * 4;
        if section.content.len() > max_chars && max_chars > 20 {
            let end = section.content.floor_char_boundary(max_chars);
            section.content.truncate(end);
            section.content.push_str("\n[...truncated]");
            section.token_estimate = section.content.len() / 4;
            // Update Message target to reflect truncated content
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
}
```

- [ ] **Step 4: Create `prompt/mod.rs`**

```rust
// crates/openalpaca_core/src/prompt/mod.rs
mod builder;

pub use builder::{BuiltPrompt, PromptBuilder};
```

- [ ] **Step 5: Register module in `lib.rs`**

Add `pub mod prompt;` to `crates/openalpaca_core/src/lib.rs`.

- [ ] **Step 6: Run tests**

Run: `cargo test -p openalpaca_core prompt::builder::tests -v`
Expected: all 3 tests pass.

- [ ] **Step 7: Commit**

```bash
git add crates/openalpaca_core/src/prompt/ crates/openalpaca_core/src/lib.rs
git commit -m "feat(prompt): add PromptBuilder with section registration and adaptive trimming"
```

---

### Task 3: Add trimming tests

**Files:**
- Modify: `crates/openalpaca_core/src/prompt/builder.rs` (add tests)

- [ ] **Step 1: Add trimming test**

Append to the `tests` module in `builder.rs`:

```rust
    #[test]
    fn test_trimming_drops_optional_first() {
        // Use a tiny window so trimming is forced
        let mut builder = PromptBuilder::new(100); // 100 tokens = 400 chars
        let persona = SystemPersona::default();
        builder.system_persona(&persona); // ~125 tokens (Critical — exceeds 75-token cap alone, but never dropped)
        builder.bootstrap("This is a long bootstrap block that should be dropped first because it is Optional priority and takes many tokens");
        let built = builder.build();

        // bootstrap should be dropped (Optional), persona should survive (Critical)
        assert!(built.system_message.contains("<system_instructions>"));
        assert!(!built.system_message.contains("bootstrap"));
    }

    #[test]
    fn test_section_registry_tracks_all_sections() {
        let mut builder = PromptBuilder::new(200_000);
        let persona = SystemPersona::default();
        let agent = AgentPersona {
            role: "Tester".to_string(),
            tone: "Precise".to_string(),
            domain_knowledge: vec![],
        };
        builder
            .system_persona(&persona)
            .agent_persona(&agent)
            .identity("### IDENTITY ###\nName: Test");
        let built = builder.build();

        let names: Vec<&str> = built.section_registry.iter().map(|(n, _)| *n).collect();
        assert!(names.contains(&"system_persona"));
        assert!(names.contains(&"agent_persona"));
        assert!(names.contains(&"identity"));
    }
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p openalpaca_core prompt::builder::tests -v`
Expected: all 5 tests pass.

- [ ] **Step 3: Commit**

```bash
git add crates/openalpaca_core/src/prompt/builder.rs
git commit -m "test(prompt): add trimming and section registry tests"
```

---

## Chunk 2: Phase 2 — ContextManager + Sources (No Behavior Change)

### Task 4: Create ContextSource trait and ContextManager skeleton

**Files:**
- Create: `crates/openalpaca_core/src/prompt_ctx/sources/mod.rs`
- Create: `crates/openalpaca_core/src/prompt_ctx/manager.rs`
- Modify: `crates/openalpaca_core/src/prompt_ctx/mod.rs`

- [ ] **Step 1: Create ContextSource trait**

```rust
// crates/openalpaca_core/src/prompt_ctx/sources/mod.rs
pub mod memory;
pub mod conversation;
pub mod user_profile;
pub mod skill;
pub mod workspace;

use crate::prompt_ctx::section::ContextSection;
use async_trait::async_trait;

/// Which execution path is requesting context.
#[derive(Debug, Clone)]
pub enum ExecutionPath {
    SimpleQuery,
    SocialQuery,
    SkillInvocation { skill_id: String },
    PipelineStep { step: usize, total: usize },
    DagNode { node_id: String },
    LeadAgent,
}

/// Request for context resolution.
#[derive(Debug, Clone)]
pub struct ContextRequest {
    pub query: String,
    pub intent: crate::orchestrator::intent::Intent,
    pub path: ExecutionPath,
    pub skill: Option<std::sync::Arc<crate::middleware::skill::types::SkillDocument>>,
    pub owner_id: Option<String>,
    pub scope: crate::memory::scope_context::MemoryScopeContext,
    pub model_context_window: usize,
    pub reserved_tokens: usize,
}

#[async_trait]
pub trait ContextSource: Send + Sync {
    fn name(&self) -> &'static str;
    async fn resolve(&self, request: &ContextRequest) -> Vec<ContextSection>;
    fn active_for(&self, _path: &ExecutionPath) -> bool {
        true
    }
}
```

- [ ] **Step 2: Create ContextManager skeleton**

```rust
// crates/openalpaca_core/src/prompt_ctx/manager.rs
use crate::prompt_ctx::section::{ContextBundle, ContextSection, SectionPriority};
use crate::prompt_ctx::sources::{ContextRequest, ContextSource};
use arc_swap::ArcSwap;
use std::sync::Arc;

pub struct ContextManager {
    sources: Vec<Box<dyn ContextSource>>,
    config: Arc<ArcSwap<crate::daemon_config::DaemonConfig>>,
}

impl ContextManager {
    /// Construct with dependency injection. The spec constructor wires the 5 concrete
    /// sources internally. `config` provides live-reloadable `autocompact_buffer_ratio`
    /// via `config.load().execution.context.autocompact_buffer_ratio`.
    ///
    /// For tests / CLI without storage, pass an empty `sources` vec.
    pub fn new(
        sources: Vec<Box<dyn ContextSource>>,
        config: Arc<ArcSwap<crate::daemon_config::DaemonConfig>>,
    ) -> Self {
        Self { sources, config }
    }

    /// No-op manager for CLI / test environments without storage.
    pub fn noop() -> Self {
        use arc_swap::ArcSwap;
        Self {
            sources: Vec::new(),
            config: Arc::new(ArcSwap::from_pointee(crate::daemon_config::DaemonConfig::default())),
        }
    }

    fn autocompact_buffer_ratio(&self) -> f64 {
        self.config.load().execution.context.autocompact_buffer_ratio
    }

    pub async fn resolve(&self, request: &ContextRequest) -> ContextBundle {
        let buffer = (request.model_context_window as f64 * self.autocompact_buffer_ratio()) as usize;
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompt_ctx::*;
    use crate::prompt_ctx::sources::ExecutionPath;
    use arc_swap::ArcSwap;
    use std::sync::Arc;

    fn test_config_with_buffer(ratio: f64) -> crate::daemon_config::DaemonConfig {
        let mut config = crate::daemon_config::DaemonConfig::default();
        config.execution.context.autocompact_buffer_ratio = ratio;
        config
    }

    fn test_request(window: usize, reserved: usize) -> ContextRequest {
        ContextRequest {
            query: "test".to_string(),
            intent: crate::orchestrator::intent::Intent::SimpleQuery,
            path: ExecutionPath::SimpleQuery,
            skill: None,
            owner_id: None,
            scope: Default::default(),
            model_context_window: window,
            reserved_tokens: reserved,
        }
    }

    struct FakeSource {
        sections: Vec<ContextSection>,
    }

    #[async_trait::async_trait]
    impl ContextSource for FakeSource {
        fn name(&self) -> &'static str { "fake" }
        async fn resolve(&self, _: &ContextRequest) -> Vec<ContextSection> {
            self.sections.clone()
        }
    }

    #[tokio::test]
    async fn test_resolve_greedy_fill() {
        let source = FakeSource {
            sections: vec![
                ContextSection {
                    source: "fake",
                    kind: ContextKind::Memory,
                    content: "a".repeat(400), // 100 tokens
                    token_estimate: 100,
                    priority: SectionPriority::High,
                    relevance: 0.9,
                    key: ContextKey::Memory(1),
                    injection: InjectionMode::SystemPrompt,
                },
                ContextSection {
                    source: "fake",
                    kind: ContextKind::Memory,
                    content: "b".repeat(400),
                    token_estimate: 100,
                    priority: SectionPriority::Normal,
                    relevance: 0.5,
                    key: ContextKey::Memory(2),
                    injection: InjectionMode::SystemPrompt,
                },
            ],
        };

        let config = Arc::new(ArcSwap::from_pointee(test_config_with_buffer(0.165)));
        let mgr = ContextManager::new(vec![Box::new(source)], config);
        let request = test_request(1000, 500);
        // available = 1000 - 500 - 165 = 335
        let bundle = mgr.resolve(&request).await;

        // Both sections (100 each = 200) fit in 335 budget
        assert_eq!(bundle.sections.len(), 2);
        assert_eq!(bundle.total_tokens, 200);
    }

    #[tokio::test]
    async fn test_resolve_drops_low_priority_when_full() {
        let source = FakeSource {
            sections: vec![
                ContextSection {
                    source: "fake",
                    kind: ContextKind::Memory,
                    content: "a".repeat(800), // 200 tokens
                    token_estimate: 200,
                    priority: SectionPriority::High,
                    relevance: 0.9,
                    key: ContextKey::Memory(1),
                    injection: InjectionMode::SystemPrompt,
                },
                ContextSection {
                    source: "fake",
                    kind: ContextKind::WorkspaceArtifact,
                    content: "b".repeat(800), // 200 tokens
                    token_estimate: 200,
                    priority: SectionPriority::Low, // Low — silently dropped
                    relevance: 0.5,
                    key: ContextKey::WorkspaceArtifact("x".into()),
                    injection: InjectionMode::SystemPrompt,
                },
            ],
        };

        let config = Arc::new(ArcSwap::from_pointee(test_config_with_buffer(0.0)));
        let mgr = ContextManager::new(vec![Box::new(source)], config);
        let request = test_request(500, 200);
        // available = 500 - 200 - 0 = 300. Only fits 200 (High), not both.
        let bundle = mgr.resolve(&request).await;

        assert_eq!(bundle.sections.len(), 1);
        assert_eq!(bundle.sections[0].priority, SectionPriority::High);
    }
}
```

- [ ] **Step 3: Create stub source files**

Create empty stub files for each source (to be implemented in subsequent tasks):

```rust
// crates/openalpaca_core/src/prompt_ctx/sources/memory.rs
// MemorySource — hybrid FTS+vector search
// TODO: implement in Task 5

// crates/openalpaca_core/src/prompt_ctx/sources/conversation.rs
// ConversationSource — session summary
// TODO: implement in Task 5

// crates/openalpaca_core/src/prompt_ctx/sources/user_profile.rs
// UserProfileSource — USER.md profile
// TODO: implement in Task 5

// crates/openalpaca_core/src/prompt_ctx/sources/skill.rs
// SkillContextSource — skill reference files
// TODO: implement in Task 5

// crates/openalpaca_core/src/prompt_ctx/sources/workspace.rs
// WorkspaceSource — artifacts, previous agent outputs
// TODO: implement in Task 5
```

- [ ] **Step 4: Update `prompt_ctx/mod.rs`**

```rust
pub mod manager;
pub mod section;
pub mod sources;

pub use section::{
    ContextBundle, ContextKey, ContextKind, ContextSection,
    InjectionMode, SectionPriority, TrustLevel,
};
pub use manager::ContextManager;
pub use sources::{ContextRequest, ContextSource, ExecutionPath};
```

- [ ] **Step 5: Run tests**

Run: `cargo test -p openalpaca_core prompt_ctx::manager::tests -v`
Expected: 2 tests pass.

- [ ] **Step 6: Commit**

```bash
git add crates/openalpaca_core/src/prompt_ctx/
git commit -m "feat(prompt_ctx): add ContextManager with greedy budget-aware resolution"
```

---

### Task 5: Implement concrete context sources

**Files:**
- Modify: `crates/openalpaca_core/src/prompt_ctx/sources/memory.rs`
- Modify: `crates/openalpaca_core/src/prompt_ctx/sources/conversation.rs`
- Modify: `crates/openalpaca_core/src/prompt_ctx/sources/user_profile.rs`
- Modify: `crates/openalpaca_core/src/prompt_ctx/sources/skill.rs`
- Modify: `crates/openalpaca_core/src/prompt_ctx/sources/workspace.rs`

Each source follows the same pattern: implement `ContextSource` trait, returning `Vec<ContextSection>` with appropriate `InjectionMode`, `SectionPriority`, and `ContextKey`.

Implementation details for each source:

**MemorySource:** Wraps `MemoryRepository::search_hybrid_cascade()`. Returns one `ContextSection` per memory entry with `priority: Normal`, `injection: UserMessage { tag: "retrieved_memory", trust: Untrusted }`. `active_for` returns `false` for `SocialQuery`.

**ConversationSource:** Takes a session summary string (from `ConversationContext.summary`). Returns a single `ContextSection` with `priority: High`, `injection: UserMessage { tag: "session_summary", trust: Untrusted }`. `active_for` returns `false` for `SocialQuery`.

**UserProfileSource:** Wraps `Arc<RwLock<Option<UserDocument>>>`. Calls `user_to_prompt_block()`. Returns a single `ContextSection` with `priority: Normal`, `injection: SystemMessage`. `active_for` returns `false` for `SocialQuery`.

**SkillContextSource:** Wraps `inject_skill_context()`. Returns one `ContextSection` per context source file with `priority: Normal`, `injection: SystemPrompt`. `active_for` returns `true` only for `SkillInvocation`.

**WorkspaceSource:** Returns workspace artifact sections from `TaskState` / cached outputs. `priority: Low`, `injection: UserMessage { tag: "workspace_artifact", trust: Untrusted }`. `active_for` returns `true` for `PipelineStep`, `DagNode`, `LeadAgent`.

- [ ] **Step 1: Implement each source with constructor and real dependencies**

Each source must have a `new()` constructor accepting its dependencies:

- `MemorySource::new(db: Arc<Database>, embedder: Option<Arc<dyn Embedder>>)` — calls `MemoryRepository::search_hybrid_cascade()` with `request.scope`
- `ConversationSource::new()` — takes session summary from request context
- `UserProfileSource::new(user_document: Arc<RwLock<Option<UserDocument>>>)` — calls `user_to_prompt_block()`
- `SkillContextSource::new(skill_catalog: Arc<SkillCatalog>)` — uses `request.skill` to resolve context files
- `WorkspaceSource::new()` — returns cached workspace artifacts

The key pattern for each source file:

```rust
use crate::prompt_ctx::*;
use crate::prompt_ctx::sources::{ContextRequest, ContextSource, ExecutionPath};
use async_trait::async_trait;

pub struct MemorySource {
    db: Arc<Database>,
    embedder: Option<Arc<dyn Embedder>>,
}

impl MemorySource {
    pub fn new(db: Arc<Database>, embedder: Option<Arc<dyn Embedder>>) -> Self {
        Self { db, embedder }
    }
}

#[async_trait]
impl ContextSource for MemorySource {
    fn name(&self) -> &'static str { "memory" }

    async fn resolve(&self, request: &ContextRequest) -> Vec<ContextSection> {
        let repo = MemoryRepository::new(&self.db);
        let results = repo.search_hybrid_cascade(&request.query, &request.scope, 10).await;
        results.into_iter().map(|entry| ContextSection {
            source: "memory",
            kind: ContextKind::Memory,
            content: entry.content.clone(),
            token_estimate: entry.content.len() / 4,
            priority: SectionPriority::Normal,
            relevance: entry.score as f32,
            key: ContextKey::Memory(entry.id),
            injection: InjectionMode::UserMessage {
                tag: "retrieved_memory".to_string(),
                trust: TrustLevel::Untrusted,
            },
        }).collect()
    }

    fn active_for(&self, path: &ExecutionPath) -> bool {
        !matches!(path, ExecutionPath::SocialQuery)
    }
}
```

- [ ] **Step 2: Write integration test for MemorySource**

Test that `MemorySource` returns non-empty sections when given a seeded database. Use an in-memory SQLite database with `MemoryRepository::insert()` to seed test data, then verify `resolve()` returns sections with correct `ContextKind::Memory` and `InjectionMode::UserMessage`.

- [ ] **Step 3: Run `cargo check -p openalpaca_core` and `cargo test -p openalpaca_core prompt_ctx`**

Expected: compiles clean, MemorySource integration test passes.

- [ ] **Step 4: Commit**

```bash
git add crates/openalpaca_core/src/prompt_ctx/sources/
git commit -m "feat(prompt_ctx): implement 5 context sources (memory, conversation, user, skill, workspace)"
```

---

## Chunk 3: Phase 3 — Wire SimpleQuery (First Behavior Change)

### Task 6: Wire PromptBuilder into SimpleQuery handler

**Files:**
- Modify: `crates/openalpaca_core/src/orchestrator/query_handler/simple_query_handler.rs`

This is the highest-impact change: replace ad-hoc prompt assembly with PromptBuilder, and pass `ContextBudgetManager` to the agentic loop.

- [ ] **Step 1: Import new types at top of `simple_query_handler.rs`**

Add:
```rust
use crate::prompt::PromptBuilder;
use crate::prompt_ctx::{ContextManager, ContextRequest, ExecutionPath};
```

- [ ] **Step 2: Replace prompt assembly section**

The existing code builds `system_prompt` via `get_or_build_base_prompt()` + manual string concatenation. Replace with:

```rust
// Hoist model_window before PromptBuilder construction.
// Default to 200_000 when no LLM router is present (echo-stub path).
let model_window = self.llm_router.as_ref()
    .and_then(|r| config_for_loop.model.as_deref()
        .and_then(|m| r.model_registry().get_model_info(m)))
    .map(|info| info.context_window as usize)
    .unwrap_or(200_000);

// Build static sections first
let mut builder = PromptBuilder::new(model_window);
builder
    .system_persona(&soul_persona)
    .agent_persona(&agent_persona)
    .identity(&identity_block)
    .bootstrap(&bootstrap_block)
    .skills_catalog(&catalog_block)
    .tools(&tool_defs)
    .connector_guidance(&statuses, sendable.as_deref())
    .message_source(source);

// Estimate static tokens for context budget
let reserved = builder.estimate_static_tokens();

// Resolve dynamic context
let ctx_request = ContextRequest {
    query: query.to_string(),
    path: ExecutionPath::SimpleQuery,
    owner_id: owner_id.map(|s| s.to_string()),
    model_context_window: model_window,
    reserved_tokens: reserved,
};
let bundle = self.context_manager.resolve(&ctx_request).await;
builder.context_bundle(&bundle);

// Build and register with budget manager
let built = builder.build();
let mut budget = ContextBudgetManager::new(model_window, &budget_config);
for (name, tokens) in &built.section_registry {
    budget.register_section(name, *tokens);
}
```

- [ ] **Step 3: Pass budget to agentic loop**

Change the `run_agentic_loop_routed()` call from `context_budget: None` to `context_budget: Some(&budget)`.

- [ ] **Step 4: Add `context_manager` field to `Orchestrator`**

Add `context_manager: ContextManager` (private field) to the `Orchestrator` struct in `orchestrator/mod.rs`. Initialize inside `new()` using:
```rust
let context_manager = if let Some(ref db) = db {
    let sources: Vec<Box<dyn ContextSource>> = vec![
        Box::new(MemorySource::new(db.clone(), embedder.clone())),
        Box::new(ConversationSource::new()),
        Box::new(UserProfileSource::new(user_document.clone())),
        Box::new(SkillContextSource::new(skill_catalog.clone())),
        Box::new(WorkspaceSource::new()),
    ];
    ContextManager::new(sources, config.clone())
} else {
    ContextManager::noop() // CLI mode without storage
};
```
No change to `Orchestrator::new()` public signature — `context_manager` is constructed from existing args.

- [ ] **Step 5: Preserve existing handler-specific behaviors**

Keep these in the handler (not in PromptBuilder):
- `adapt_parts_for_model()` — before message assembly
- `try_direct_send()` — before PromptBuilder
- `apply_send_keepalive` — after tool resolution
- `loop_overrides` — applied to LoopConfig
- `build_send_context()` + `send_rules` — both remain in handler. Inject via `builder.raw_system_block("send_context", &send_ctx, SectionPriority::Normal)` and `builder.raw_system_block("send_rules", &send_rules, SectionPriority::Normal)` in the same conditional branch that checks `tool_defs.iter().any(|d| d.name == "send")`

- [ ] **Step 6a: Write smoke test**

Add a test in `orchestrator/query_handler/simple_query_handler.rs` tests that asserts:
- `built.system_message` contains `<system_instructions>`, `<agent_role>`, identity block
- `built.section_registry` contains "system_persona", "agent_persona", "identity", "tools", "connector_guidance"
- `built.total_prompt_tokens` is within 10% of the token total from the previous `system_prompt.len() / 4` estimate

- [ ] **Step 6b: Run `cargo test -p openalpaca_core` (full crate)**

Expected: all existing tests pass plus new smoke test passes.

- [ ] **Step 7: Commit**

```bash
git add crates/openalpaca_core/src/orchestrator/
git commit -m "feat: wire PromptBuilder + ContextManager into SimpleQuery handler

Closes gap #1: ContextBudgetManager now passed to agentic loop.
Closes gap #3: Memory budget is dynamic (from ContextManager).
Closes gap #4: Section budgets derived from context window."
```

---

## Chunk 4: Phase 4 — Graduated Compaction

### Task 7: Add `compaction_tier()` to ContextBudgetManager

**Files:**
- Modify: `crates/openalpaca_core/src/context_budget/budget.rs`

- [ ] **Step 1: Write failing test**

```rust
#[test]
fn test_compaction_tier_thresholds() {
    let config = ContextBudgetConfig {
        autocompact_buffer_ratio: 0.165,
        compaction_target_ratio: 0.50,
        compaction_model: None,
        max_extractions_per_compaction: 10,
        min_recent_messages: 4,
    };
    let budget = ContextBudgetManager::new(200_000, &config);

    // 50% utilization → None
    assert_eq!(budget.compaction_tier(100_000), CompactionTier::None);
    // 65% → TruncateToolResults
    assert_eq!(budget.compaction_tier(130_000), CompactionTier::TruncateToolResults);
    // 75% → DropMultimedia
    assert_eq!(budget.compaction_tier(150_000), CompactionTier::DropMultimedia);
    // 80% → DiscardSocial
    assert_eq!(budget.compaction_tier(160_000), CompactionTier::DiscardSocial);
    // 85% → HeuristicSummary
    assert_eq!(budget.compaction_tier(170_000), CompactionTier::HeuristicSummary);
    // 90% → LlmSummary
    assert_eq!(budget.compaction_tier(180_000), CompactionTier::LlmSummary);
}
```

- [ ] **Step 2: Add CompactionTier enum and implement `compaction_tier()`**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CompactionTier {
    None,
    TruncateToolResults,
    DropMultimedia,
    DiscardSocial,
    HeuristicSummary,
    LlmSummary,
}

impl CompactionTier {
    pub fn next(self) -> Option<CompactionTier> {
        match self {
            Self::None => Some(Self::TruncateToolResults),
            Self::TruncateToolResults => Some(Self::DropMultimedia),
            Self::DropMultimedia => Some(Self::DiscardSocial),
            Self::DiscardSocial => Some(Self::HeuristicSummary),
            Self::HeuristicSummary => Some(Self::LlmSummary),
            Self::LlmSummary => None,
        }
    }
}

impl ContextBudgetManager {
    pub fn compaction_tier(&self, message_tokens: usize) -> CompactionTier {
        let total = self.fixed_zone_tokens() + message_tokens;
        let utilization = total as f64 / self.model_context_window as f64;
        match utilization {
            u if u < 0.60 => CompactionTier::None,
            u if u < 0.70 => CompactionTier::TruncateToolResults,
            u if u < 0.75 => CompactionTier::DropMultimedia,
            u if u < 0.80 => CompactionTier::DiscardSocial,
            u if u < 0.85 => CompactionTier::HeuristicSummary,
            _ => CompactionTier::LlmSummary,
        }
    }
}
```

- [ ] **Step 3: Run tests, commit**

Run: `cargo test -p openalpaca_core context_budget -v`

```bash
git commit -m "feat(context_budget): add CompactionTier enum and compaction_tier() method"
```

---

### Task 8: Implement GraduatedCompactor

**Files:**
- Create: `crates/openalpaca_core/src/prompt_ctx/compaction/mod.rs`
- Create: `crates/openalpaca_core/src/prompt_ctx/compaction/graduated.rs`
- Modify: `crates/openalpaca_core/src/prompt_ctx/mod.rs` (add `pub mod compaction;`)

Note: `GraduatedCompactor` lives in `prompt_ctx/compaction/` (not `context_budget/`) because `context_budget/` is marked for deprecation in Phase 6. `CompactionTier` stays in `context_budget/budget.rs` since it's a method on `ContextBudgetManager`.

- [ ] **Step 1: Write failing test for truncate_tool_results**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use openalpaca_llm::{ChatMessage, Role};

    #[test]
    fn test_truncate_tool_results() {
        let mut messages = vec![
            ChatMessage::system("system"),
            ChatMessage::user("hello"),
            ChatMessage { role: Role::Tool, content: "x".repeat(1000), ..Default::default() },
        ];
        truncate_tool_results(&mut messages);
        // Tool result should be truncated to 50%
        assert!(messages[2].content.len() <= 520); // 500 + "[...truncated]"
    }

    #[test]
    fn test_drop_multimedia() {
        use openalpaca_llm::ContentPart;
        let mut messages = vec![
            ChatMessage::system("system"),
            ChatMessage {
                role: Role::User,
                content: String::new(),
                parts: Some(vec![
                    ContentPart::Image { url: "data:image/png;base64,abc".into(), detail: None },
                    ContentPart::Text { text: "describe this".into() },
                ]),
                ..Default::default()
            },
        ];
        drop_multimedia(&mut messages);
        let parts = messages[1].parts.as_ref().unwrap();
        assert_eq!(parts.len(), 2);
        assert!(matches!(&parts[0], ContentPart::Text { text } if text.contains("[image removed")));
    }
}
```

- [ ] **Step 2: Implement GraduatedCompactor and tier functions**

```rust
// crates/openalpaca_core/src/prompt_ctx/compaction/graduated.rs
use crate::context_budget::{CompactionTier, ContextBudgetManager};
use crate::context_budget::compaction::{MemoryExtractor, Summarizer};
use openalpaca_llm::{ChatMessage, ContentPart, Role};

/// Report of compaction actions taken.
#[derive(Debug, Default)]
pub struct CompactionReport {
    pub tiers_applied: Vec<CompactionTier>,
    pub initial_tokens: usize,
    pub final_tokens: usize,
}

impl CompactionReport {
    pub fn record_tier(&mut self, tier: CompactionTier) {
        self.tiers_applied.push(tier);
    }
}

pub struct GraduatedCompactor<'a> {
    budget: &'a ContextBudgetManager,
    extractor: &'a dyn MemoryExtractor,
    summarizer: &'a dyn Summarizer,
}

impl<'a> GraduatedCompactor<'a> {
    pub fn new(
        budget: &'a ContextBudgetManager,
        extractor: &'a dyn MemoryExtractor,
        summarizer: &'a dyn Summarizer,
    ) -> Self {
        Self { budget, extractor, summarizer }
    }

    pub async fn compact(
        &self,
        messages: &mut Vec<ChatMessage>,
        tail_keep: usize,
    ) -> CompactionReport {
        let mut report = CompactionReport {
            initial_tokens: crate::runner::agentic_loop::context::estimate_messages_tokens(messages) as usize,
            ..Default::default()
        };

        let mut current_tier = self.budget.compaction_tier(report.initial_tokens);
        if current_tier == CompactionTier::None {
            report.final_tokens = report.initial_tokens;
            return report;
        }

        loop {
            match current_tier {
                CompactionTier::None => break,
                CompactionTier::TruncateToolResults => truncate_tool_results(messages),
                CompactionTier::DropMultimedia => drop_multimedia(messages),
                CompactionTier::DiscardSocial => {
                    let min_recent = self.budget.min_recent_messages();
                    let cleaned = crate::context_budget::compaction::CompactionPipeline::discard_social(messages, min_recent);
                    *messages = cleaned;
                }
                CompactionTier::HeuristicSummary => {
                    crate::runner::agentic_loop::context::compress_context(messages, tail_keep, Some(self.budget));
                }
                CompactionTier::LlmSummary => {
                    // Use existing CompactionPipeline for LLM-based summarization
                    let pipeline = crate::context_budget::compaction::CompactionPipeline::new(
                        self.extractor, self.summarizer,
                    );
                    pipeline.compact(messages, tail_keep, Some(self.budget)).await;
                }
            }
            report.record_tier(current_tier);

            let tokens_now = crate::runner::agentic_loop::context::estimate_messages_tokens(messages) as usize;
            let new_tier = self.budget.compaction_tier(tokens_now);
            if new_tier <= current_tier {
                break; // No progress or regression — stop
            }
            match current_tier.next() {
                Some(next) => current_tier = next,
                None => {
                    tracing::warn!("Compaction exhausted all tiers, still at {tokens_now} tokens");
                    break;
                }
            }
        }

        report.final_tokens = crate::runner::agentic_loop::context::estimate_messages_tokens(messages) as usize;
        report
    }
}

/// Tier 1: Truncate tool result messages to 50% of their size.
pub fn truncate_tool_results(messages: &mut [ChatMessage]) {
    for msg in messages.iter_mut() {
        if msg.role == Role::Tool && msg.content.len() > 200 {
            let target = msg.content.len() / 2;
            let end = msg.content.floor_char_boundary(target);
            msg.content.truncate(end);
            msg.content.push_str("\n[...truncated]");
        }
    }
}

/// Tier 2: Replace image/audio/document parts with text placeholders.
pub fn drop_multimedia(messages: &mut [ChatMessage]) {
    for msg in messages.iter_mut() {
        if let Some(ref mut parts) = msg.parts {
            for part in parts.iter_mut() {
                match part {
                    ContentPart::Image { .. } => {
                        *part = ContentPart::Text { text: "[image removed to save context]".into() };
                    }
                    ContentPart::Audio { .. } => {
                        *part = ContentPart::Text { text: "[audio removed to save context]".into() };
                    }
                    ContentPart::Document { filename, .. } => {
                        let name = filename.clone();
                        *part = ContentPart::Text { text: format!("[document '{name}' removed to save context]") };
                    }
                    _ => {}
                }
            }
        }
    }
}
```

- [ ] **Step 3: Create `compaction/mod.rs` and register in `prompt_ctx/mod.rs`**

```rust
// crates/openalpaca_core/src/prompt_ctx/compaction/mod.rs
mod graduated;
pub use graduated::{CompactionReport, GraduatedCompactor, truncate_tool_results, drop_multimedia};
```

- [ ] **Step 4: Run tests, commit**

```bash
cargo test -p openalpaca_core prompt_ctx::compaction -v
git commit -m "feat(prompt_ctx): implement GraduatedCompactor with 5 compaction tiers"
```

---

### Task 9: Wire graduated compaction into agentic loop

**Files:**
- Modify: `crates/openalpaca_core/src/runner/agentic_loop/mod.rs` (lines 280-358)
- Create: `crates/openalpaca_core/src/prompt_ctx/lifecycle.rs`

- [ ] **Step 1: Create ContextLifecycle**

```rust
// crates/openalpaca_core/src/prompt_ctx/lifecycle.rs
use crate::prompt_ctx::ContextKey;
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub struct ContextLifecycle {
    seen: HashMap<ContextKey, SeenEntry>,
}

struct SeenEntry {
    message_index: usize,
    injected_at: Instant,
    token_cost: usize,
}

impl ContextLifecycle {
    pub fn new() -> Self {
        Self { seen: HashMap::new() }
    }

    pub fn should_inject(&self, key: &ContextKey, staleness_threshold: Duration) -> bool {
        match self.seen.get(key) {
            None => true,
            Some(entry) => entry.injected_at.elapsed() > staleness_threshold,
        }
    }

    pub fn mark_injected(&mut self, key: ContextKey, message_index: usize, tokens: usize) {
        self.seen.insert(key, SeenEntry {
            message_index,
            injected_at: Instant::now(),
            token_cost: tokens,
        });
    }

    pub fn tokens_before(&self, index: usize) -> usize {
        self.seen.values()
            .filter(|e| e.message_index < index)
            .map(|e| e.token_cost)
            .sum()
    }
}
```

- [ ] **Step 2: Replace binary compaction block in `run_agentic_loop_inner`**

Replace lines 280-358 with graduated compactor call. Keep legacy fallback path for `context_budget: None`.

- [ ] **Step 3: Run full test suite**

Run: `cargo test -p openalpaca_core -v`

- [ ] **Step 4: Commit**

```bash
git commit -m "feat: wire GraduatedCompactor into agentic loop, replacing binary compaction"
```

---

## Chunk 5: Phase 5 — Inter-Agent Context Flow

### Task 10: Implement ContextPackage v2 and HandoffContext

**Files:**
- Create: `crates/openalpaca_core/src/prompt_ctx/package.rs`
- Modify: `crates/openalpaca_core/src/prompt_ctx/mod.rs`

- [ ] **Step 1: Implement PackageSection, PackageSectionKind, HandoffContext, ContextPackage v2**

Follow the spec section 6. Include `to_bundle()` with the injection mode mapping table. Include `HandoffContext::format()` and `HandoffContext::merge()`.

- [ ] **Step 2: Write tests for `to_bundle()` injection modes, `HandoffContext::format()`, and `HandoffContext::merge()`**

Test `merge()` verifies: all predecessors' content appears in merged output, producer attribution is preserved for each predecessor.

- [ ] **Step 3: Commit**

```bash
git commit -m "feat(prompt_ctx): add ContextPackage v2 with HandoffContext and to_bundle()"
```

---

### Task 11: Add `distill()` to ContextManager

**Files:**
- Modify: `crates/openalpaca_core/src/prompt_ctx/manager.rs`

- [ ] **Step 1: Implement `distill()` method**

Follow spec section 6.1. Re-prioritize parent sections for sub-agent, budget-fill with greedy algorithm, access control via `denied_sections`. Hardcode `const SUB_AGENT_CONTEXT_RATIO: f64 = 0.40` for now — config wiring is deferred to Task 17 Step 3.

- [ ] **Step 2: Write tests for distillation (budget adaptation, denied sections, priority remapping)**

- [ ] **Step 3: Commit**

```bash
git commit -m "feat(prompt_ctx): add ContextManager::distill() for sub-agent context packages"
```

---

### Task 12: Wire Pipeline steps with HandoffContext

**Files:**
- Modify: `crates/openalpaca_core/src/orchestrator/dispatcher/pipeline_step.rs`

- [ ] **Step 1: Replace raw workspace text with HandoffContext**

After each pipeline step completes, create a `HandoffContext` from the agent's output and pass it to the next step's `distill()` call.

- [ ] **Step 2: Wire PromptBuilder for pipeline steps**

Replace ad-hoc prompt assembly with PromptBuilder chain.

- [ ] **Step 3: Run tests, commit**

```bash
git commit -m "feat: wire Pipeline steps with HandoffContext and PromptBuilder"
```

---

### Task 13: Wire DAG nodes with distilled packages

**Files:**
- Modify: `crates/openalpaca_core/src/runner/dag_executor/node_runner.rs`
- Modify: `crates/openalpaca_core/src/events.rs` (update `ContextPackageBuilt` fields)
- Modify: `apps/openalpacad/src/event_bridge.rs` (update serialization pattern match)
- Modify: `apps/openalpaca-gui/src/lib/daemon.ts` (add `context_package_built` variant to `ServerEvent`)

- [ ] **Step 1: Replace ContextPackageBuilder with ContextManager::distill()**

- [ ] **Step 2: Wire PromptBuilder for DAG nodes**

- [ ] **Step 3: Update `SystemEvent::ContextPackageBuilt` event schema**

In `events.rs`: change `ContextPackageBuilt` from `{sections_included, total_tokens, memories_count}` to `{sections: Vec<(String, usize)>, total_tokens, budget, sub_agent_window}`.

In `event_bridge.rs`: update the pattern match (currently ~line 516) for the new field names.

In `daemon.ts`: add `context_package_built` variant to the `ServerEvent` TypeScript union with fields `{ type: "context_package_built"; agent_id: string; sections: [string, number][]; total_tokens: number; budget: number; sub_agent_window: number; ts: string; instance_id: string; _id: number }`.

- [ ] **Step 4: Run tests, commit**

```bash
git commit -m "feat: wire DAG nodes with distilled context packages and PromptBuilder"
```

---

### Task 14: Wire Lead Agent sub-agent spawning

**Files:**
- Modify: `crates/openalpaca_core/src/runner/lead_agent/mod.rs`
- Modify: `crates/openalpaca_core/src/runner/lead_agent/tools.rs`

- [ ] **Step 1: Pass ContextManager to lead agent**

- [ ] **Step 2: In `spawn_subagent` tool handler, call `distill()` to create package for spawned agent**

- [ ] **Step 3: Run tests, commit**

```bash
git commit -m "feat: wire Lead Agent sub-agent spawning with context distillation"
```

---

## Chunk 6: Phase 6 — Wire Remaining Paths + Cleanup

### Task 15: Wire Skill Invocation with PromptBuilder

**Files:**
- Modify: `crates/openalpaca_core/src/orchestrator/skill/invocation.rs`

- [ ] **Step 1: Replace manual prompt assembly with ContextManager + PromptBuilder**

The skill invocation path already has the most complex prompt assembly (~80 lines). Construct a `ContextRequest` with `ExecutionPath::SkillInvocation` and call `context_manager.resolve()` to get a `ContextBundle`, then pass it to `PromptBuilder`. Remove the manual memory retrieval block (lines 313-376 of `invocation.rs`) and the hardcoded `budget = 2000` (line 355). Preserve skill-specific sections (skill block, context sources, send context) via `raw_system_block()`.

- [ ] **Step 2: Run tests, commit**

```bash
git commit -m "feat: wire Skill Invocation with PromptBuilder and ContextManager"
```

---

### Task 16: Wire Lead Agent prompt with PromptBuilder

**Files:**
- Modify: `crates/openalpaca_core/src/runner/lead_agent/prompt.rs`

- [ ] **Step 1: Replace `build_lead_agent_prompt_from_templates()` with PromptBuilder**

The lead agent has custom sections (agents list, workflow, delegation criteria). Use `raw_system_block()` for these.

- [ ] **Step 2: Run tests, commit**

```bash
git commit -m "feat: wire Lead Agent prompt with PromptBuilder"
```

---

### Task 17: Delete deprecated code + cleanup

**Files:**
- Modify: `crates/openalpaca_core/src/middleware/prompt.rs` (remove `PromptAssembler`)
- Delete: `crates/openalpaca_core/src/context_budget/package.rs` (old ContextPackage)
- Delete: `crates/openalpaca_core/src/context_budget/budget.rs` (moved to `prompt_ctx/` — move `ContextBudgetManager` + `CompactionTier` first)
- Delete: `crates/openalpaca_core/src/context_budget/compaction.rs` (moved to `prompt_ctx/compaction/`)
- Delete: `crates/openalpaca_core/src/context_budget/tests.rs` (migrated with budget.rs)
- Modify: `crates/openalpaca_core/src/context_budget/mod.rs` (reduce to forwarding re-exports: `pub use crate::prompt_ctx::*;`)

**Prerequisite:** Verify `SystemEvent::ContextPackageBuilt` was migrated to the new field shape (`sections`, `budget`, `sub_agent_window`) in Task 13. If not done, do it now before deleting `context_budget/package.rs`.

- [ ] **Step 1: Remove `PromptAssembler` from `middleware/prompt.rs`**

Keep `SystemPersona`, `AgentPersona`, `format_tool_guidance()`, `format_connector_guidance()`, `format_message_source()`. Only remove `PromptAssembler` and its test.

- [ ] **Step 2: Move `ContextBudgetManager` and `CompactionTier` to `prompt_ctx/budget.rs`**

Create `prompt_ctx/budget.rs` with the moved types. Update all imports. Then reduce `context_budget/mod.rs` to `pub use crate::prompt_ctx::*;` as a forwarding shim. Delete `context_budget/budget.rs`, `context_budget/compaction.rs`, `context_budget/package.rs`, `context_budget/tests.rs`.

- [ ] **Step 3: Move hardcoded budgets to `daemon.toml`**

Add the `[orchestrator.context]` config section from the spec to `daemon_config/execution.rs` and `config/daemon.toml`.

- [ ] **Step 4: Run full test suite**

Run: `cargo test --all -v`

- [ ] **Step 5: Commit**

```bash
git commit -m "refactor: remove PromptAssembler and old ContextPackage, centralize budgets in config"
```

---

### Task 18: Add telemetry for all paths

**Files:**
- Modify: `crates/openalpaca_core/src/events.rs`
- Modify: Each wired execution path

- [ ] **Step 1: Emit `ContextBudgetComputed` event with full section breakdown from all paths**

Ensure every path (SimpleQuery, Skill, Pipeline, DAG, LeadAgent) emits the budget event after PromptBuilder.build().

- [ ] **Step 2: Run full test suite, commit**

```bash
git commit -m "feat: emit ContextBudgetComputed telemetry from all execution paths"
```

---

## Verification Checklist

After all tasks are complete:

- [ ] `cargo build --all` compiles clean
- [ ] `cargo test --all` passes (all ~1019 tests)
- [ ] `cargo clippy --all` has no new warnings
- [ ] SimpleQuery path passes `ContextBudgetManager` to agentic loop
- [ ] Graduated compaction fires at correct utilization thresholds
- [ ] Pipeline steps receive `HandoffContext` from predecessors
- [ ] DAG nodes receive distilled context packages
- [ ] Lead Agent spawned sub-agents receive distilled packages
- [ ] Skill invocation uses PromptBuilder
- [ ] Old `PromptAssembler` and `ContextPackageBuilder` are deleted
- [ ] All section budgets come from `daemon.toml` config, not hardcoded
